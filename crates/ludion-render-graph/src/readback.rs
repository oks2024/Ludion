//! Texture readback: copy the final target into a mappable buffer, wait, and unpad it to RGBA8.
//!
//! Two paths, per `engine-architecture.md` §3 and risk-register row 2:
//!
//! - `read_texture_to_image` — the **blocking** path used by lockstep / CLI rendering. A single
//!   `device.poll(Wait)` deliberately stalls until the copy is mapped, because the answer must
//!   correspond to an exact frame. This is the correct, non-flaky behavior for `ludion render`.
//! - [`StreamReadback`] — the **3-deep async** ring live streaming uses. Up to three frames'
//!   readbacks are in flight at once (`map_async`), so no single frame stalls the render thread:
//!   [`Gpu::render_streamed`] submits a frame and non-blocking-collects every finished readback,
//!   delivering the newest (latest-wins inside the ring); a full ring waits once, then empties
//!   (which caps latency at ~2 frames in the worst case). The same ring backs the
//!   [`Gpu::render_frames_via_ring`] measurement seam.
//!
//! Both handle wgpu's 256-byte `COPY_BYTES_PER_ROW_ALIGNMENT` row padding: the staging buffer's
//! rows are padded up to the alignment, and unpadding copies only the real `width * 4` bytes back.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::time::Instant;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

use crate::execute::{Gpu, PassRecorder, PhysicalCache, RenderedImage};
use crate::pass_timing::{FramePlan, PassTimingRing};
use crate::present::PresentTarget;
use crate::resource::{ResourceId, TextureDesc};
use crate::schedule::Schedule;

/// Rounds `value` up to the next multiple of wgpu's copy row alignment (256 bytes).
fn padded_bytes_per_row(unpadded: u32) -> u32 {
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    unpadded.div_ceil(align) * align
}

/// The row layout of a texture's readback: real vs. padded bytes-per-row, and the buffer size.
#[derive(Clone, Copy)]
struct RowLayout {
    width: u32,
    height: u32,
    unpadded_bpr: u32,
    padded_bpr: u32,
}

impl RowLayout {
    fn of(desc: &TextureDesc) -> RowLayout {
        let width = desc.extent.width;
        let height = desc.extent.height;
        let unpadded_bpr = width * desc.format.bytes_per_texel();
        RowLayout {
            width,
            height,
            unpadded_bpr,
            padded_bpr: padded_bytes_per_row(unpadded_bpr),
        }
    }

    fn buffer_size(&self) -> u64 {
        u64::from(self.padded_bpr) * u64::from(self.height)
    }

    /// Copies mapped, row-padded bytes into a tightly packed RGBA8 image.
    fn unpad(&self, mapped: &[u8]) -> Vec<u8> {
        let mut rgba = Vec::with_capacity((self.unpadded_bpr * self.height) as usize);
        for row in 0..self.height {
            let start = (row * self.padded_bpr) as usize;
            let end = start + self.unpadded_bpr as usize;
            rgba.extend_from_slice(&mapped[start..end]);
        }
        rgba
    }
}

/// Creates a `MAP_READ | COPY_DST` staging buffer sized for `layout`.
fn staging_buffer(device: &wgpu::Device, layout: &RowLayout) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ludion-render-graph readback"),
        size: layout.buffer_size(),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Records a texture→buffer copy respecting the padded row layout.
fn copy_texture_to_buffer(
    encoder: &mut wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    buffer: &wgpu::Buffer,
    layout: &RowLayout,
) {
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(layout.padded_bpr),
                rows_per_image: Some(layout.height),
            },
        },
        wgpu::Extent3d {
            width: layout.width,
            height: layout.height,
            depth_or_array_layers: 1,
        },
    );
}

/// The blocking readback path: copy `texture` to a staging buffer, `poll(Wait)`, and return packed
/// RGBA8. Used by `ludion render` and any lockstep caller.
///
/// # Errors
/// `L2005` if the buffer cannot be mapped.
pub(crate) fn read_texture_to_image(
    gpu: &Gpu,
    texture: &wgpu::Texture,
    desc: &TextureDesc,
) -> Result<RenderedImage, Diagnostic> {
    let layout = RowLayout::of(desc);
    let buffer = staging_buffer(&gpu.device, &layout);

    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ludion-render-graph readback copy"),
        });
    copy_texture_to_buffer(&mut encoder, texture, &buffer, &layout);
    gpu.queue.submit([encoder.finish()]);

    let (tx, rx) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
    // Deliberate stall: block until the GPU finishes and the map callback fires.
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|error| map_failure(format!("device poll failed: {error}")))?;

    match rx.recv() {
        Ok(Ok(())) => {}
        other => {
            return Err(map_failure(format!(
                "buffer map did not complete: {other:?}"
            )));
        }
    }

    let rgba = {
        let mapped = buffer
            .slice(..)
            .get_mapped_range()
            .map_err(|error| map_failure(format!("could not access mapped bytes: {error}")))?;
        layout.unpad(&mapped)
    };
    buffer.unmap();

    Ok(RenderedImage {
        width: layout.width,
        height: layout.height,
        rgba,
    })
}

fn map_failure(detail: String) -> Diagnostic {
    Diagnostic::new(
        code::RENDER_READBACK_FAILED,
        format!("could not read the rendered frame back from the GPU: {detail}"),
    )
    .with_help(
        "this usually indicates a lost device or an out-of-memory condition; retry, and check GPU \
         driver stability",
    )
}

/// Maximum in-flight readbacks — the ring's depth, and the latency cap.
const RING_DEPTH: usize = 3;

/// The env var that arms [`SubmitTrace`]. Any non-empty value other than `"0"` arms it, matching
/// `LUDION_TICK_TRACE`'s convention.
const SUBMIT_TRACE_ENV: &str = "LUDION_SUBMIT_TRACE";

/// Frames between periodic stderr summaries — 300 at 60 fps is ~5 s, often enough to catch a
/// periodic spike without flooding the log.
const SUBMIT_TRACE_REPORT_INTERVAL: u64 = 300;

/// The segments one [`Gpu::render_streamed`] call is split into, in the order they run.
///
/// `submit_ms` — the span the live profiler reports — brackets all of them at once, so a spike in
/// it names nothing on its own. These are the parts that can each independently be the spike.
#[derive(Clone, Copy)]
enum Segment {
    /// `device.poll(wait)` in the ring-full stall: the blocking wait for the in-flight maps.
    StallPoll,
    /// The row-unpad memcpy of the frame that stall freed.
    StallUnpad,
    /// `Gpu::allocate_physicals` — every physical texture the graph declares.
    Allocate,
    /// CPU pass recording (`record_external_passes`).
    Record,
    /// `queue.submit`.
    Submit,
    /// The non-blocking `device.poll(Poll)` of the ordinary collect.
    CollectPoll,
    /// The row-unpad memcpy of an ordinary collect.
    CollectUnpad,
    /// The in-process present blit, when a [`PresentTarget`] is attached (acquire + blit + present).
    Present,
    /// The whole call, for comparison against the profiler's `submit` span.
    Total,
}

/// Every segment's stderr name, indexed by `Segment as usize`.
const SEGMENT_NAMES: [&str; 9] = [
    "stall_poll",
    "stall_unpad",
    "allocate",
    "record",
    "submit",
    "collect_poll",
    "collect_unpad",
    "present",
    "total",
];

/// A stopwatch that only reads the clock while a trace is armed — a disarmed run pays one `bool`
/// test per segment and takes no reading at all.
#[derive(Clone, Copy)]
struct Watch(Option<Instant>);

impl Watch {
    fn start(armed: bool) -> Watch {
        Watch(armed.then(Instant::now))
    }

    fn ms(self) -> f64 {
        self.0
            .map_or(0.0, |start| start.elapsed().as_secs_f64() * 1000.0)
    }
}

/// One segment's accumulated cost.
#[derive(Clone, Copy, Default)]
struct SegmentStats {
    calls: u64,
    total_ms: f64,
    max_ms: f64,
}

/// An armed per-segment breakdown of [`Gpu::render_streamed`], written to stderr.
///
/// A developer trace, not a profile-report section: the report's `submit_ms` is deliberately one
/// number (the pipelined ring folds readback into its own submit span), so splitting it needs a
/// separate instrument rather than a wire field. Pure observation — it reads only clocks and the
/// ring's own occupancy and never touches sim state, so an armed run's `WorldHash` is a disarmed
/// run's.
///
/// Fixed-size accumulation, no allocation per frame: this runs for the life of the process, which
/// has no natural window-closes point.
struct SubmitTrace {
    frames: u64,
    frames_since_report: u64,
    /// How often the ring-full branch blocked — the count matters as much as the duration, since a
    /// stall that never fires cannot be the spike.
    stalls: u64,
    /// Pending readbacks observed on entry, counted by occupancy — what decides whether the next
    /// call stalls.
    occupancy: [u64; RING_DEPTH + 1],
    segments: [SegmentStats; SEGMENT_NAMES.len()],
}

impl SubmitTrace {
    fn from_env() -> Option<Box<SubmitTrace>> {
        let armed =
            std::env::var(SUBMIT_TRACE_ENV).is_ok_and(|value| !value.is_empty() && value != "0");
        armed.then(|| {
            Box::new(SubmitTrace {
                frames: 0,
                frames_since_report: 0,
                stalls: 0,
                occupancy: [0; RING_DEPTH + 1],
                segments: [SegmentStats::default(); SEGMENT_NAMES.len()],
            })
        })
    }

    fn record(&mut self, segment: Segment, ms: f64) {
        let stat = &mut self.segments[segment as usize];
        stat.calls += 1;
        stat.total_ms += ms;
        stat.max_ms = stat.max_ms.max(ms);
    }

    /// Closes one `render_streamed` call, emitting the periodic summary when the interval is up.
    fn frame_done(&mut self) {
        self.frames += 1;
        self.frames_since_report += 1;
        if self.frames_since_report >= SUBMIT_TRACE_REPORT_INTERVAL {
            self.report("periodic");
            self.frames_since_report = 0;
        }
    }

    /// Emits the one-line greppable summary this trace exists to produce: per segment, mean/max ms
    /// and the number of calls that contributed.
    fn report(&self, reason: &str) {
        if self.frames == 0 {
            return;
        }
        let mut segments = String::new();
        for (index, name) in SEGMENT_NAMES.iter().enumerate() {
            let stat = self.segments[index];
            let mean = if stat.calls > 0 {
                stat.total_ms / stat.calls as f64
            } else {
                0.0
            };
            let _ = write!(
                segments,
                " {name}_ms={mean:.3}/{:.3}(n={})",
                stat.max_ms, stat.calls
            );
        }
        let mut occupancy = String::new();
        for (slots, count) in self.occupancy.iter().enumerate() {
            let _ = write!(
                occupancy,
                "{}{slots}:{count}",
                if slots > 0 { "," } else { "" }
            );
        }
        eprintln!(
            "[ludion-submit-trace] reason={reason} frames={} stalls={} occupancy_on_entry=[{occupancy}]{segments}",
            self.frames, self.stalls,
        );
    }
}

impl Drop for SubmitTrace {
    /// The ring is dropped from several places (a size change, losing the last subscriber, host
    /// teardown); a `Drop` impl reports once on whichever fires.
    fn drop(&mut self) {
        self.report("exit");
    }
}

/// A persistent 3-deep ring of staging buffers for pipelined, non-stalling readback (live streaming).
///
/// Created once for a given row layout ([`Gpu::new_stream_readback`]) and reused across frames; the
/// caller recreates it when the render size changes. Each frame's copy is issued and `map_async`'d
/// immediately; a buffer is read (and unmapped) only once its map completes, and is not reused until
/// then. At most [`StreamReadback::DEPTH`] frames are in flight, so a slow map for one frame never
/// blocks issuing the next. `S` is a caller-chosen stamp carried through each pending readback
/// (a [`FrameIndex`](ludion_core::time::FrameIndex) for streaming, a frame ordinal for the
/// measurement seam) so the collected image reports the frame it actually depicts.
///
/// [`Gpu::render_streamed`] and [`try_collect`](StreamReadback::try_collect) are the two entry
/// points: submit-and-collect, and drain-only (for tail frames after a burst).
pub struct StreamReadback<S> {
    buffers: Vec<wgpu::Buffer>,
    layout: RowLayout,
    slots: Slots<S>,
    /// The armed per-segment trace, or `None` (the default) — see [`SubmitTrace`].
    trace: Option<Box<SubmitTrace>>,
    /// The frame graph's physical textures, kept alive between frames. The ring is the live path's
    /// per-session GPU-resource holder — it already outlives a frame and is already rebuilt when the
    /// render size moves — so the cache rides it rather than becoming a second thing to keep in step.
    physicals: Option<PhysicalCache>,
    /// The output texture of the last frame rendered through this ring (a cheap handle clone), so a
    /// presenter can blit it again without rendering.
    last_output: Option<wgpu::Texture>,
}

struct Pending<S> {
    stamp: S,
    slot: usize,
    rx: std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
}

/// The ring's slot bookkeeping, apart from the GPU buffers so it can be tested without a device.
struct Slots<S> {
    /// In-flight readbacks, oldest first.
    pending: VecDeque<Pending<S>>,
    /// The slot the next submission writes (round-robin). Free whenever the ring is not full.
    next_slot: usize,
}

impl<S> Default for Slots<S> {
    fn default() -> Self {
        Slots {
            pending: VecDeque::new(),
            next_slot: 0,
        }
    }
}

impl<S> Slots<S> {
    /// Whether every slot is in flight — the one condition under which a submission must block.
    fn is_full(&self) -> bool {
        self.pending.len() >= RING_DEPTH
    }

    /// Files a readback into the next slot, advances the round-robin, and returns the slot.
    fn file(
        &mut self,
        stamp: S,
        rx: std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    ) -> usize {
        let slot = self.next_slot;
        self.pending.push_back(Pending { stamp, slot, rx });
        self.next_slot = (slot + 1) % RING_DEPTH;
        slot
    }

    /// Pops every completed readback from the front and returns the newest, handing each older
    /// finished slot to `release` (its buffer still needs unmapping). Stops at the first readback
    /// still in flight. A failed or disconnected map is popped, the held frame released, and the
    /// failure returned as `L2005`.
    fn take_newest_ready(
        &mut self,
        mut release: impl FnMut(usize),
    ) -> Result<Option<(S, usize)>, Diagnostic> {
        let mut newest: Option<(S, usize)> = None;
        loop {
            let outcome = match self.pending.front() {
                None => break,
                Some(pending) => pending.rx.try_recv(),
            };
            let failure = match outcome {
                Ok(Ok(())) => None,
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Ok(Err(error)) => Some(format!("buffer map failed: {error:?}")),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some("buffer map channel disconnected".to_string())
                }
            };
            let Pending { stamp, slot, .. } =
                self.pending.pop_front().expect("front was just observed");
            if let Some((_, held)) = newest.take() {
                release(held);
            }
            if let Some(detail) = failure {
                return Err(map_failure(detail));
            }
            newest = Some((stamp, slot));
        }
        Ok(newest)
    }
}

/// Where one frame's finished output texture goes — the argument that makes
/// [`Gpu::render_frame`] general.
///
/// The two sinks are independent and neither changes what is drawn: the passes are recorded once
/// and write the same pixels whatever is asked of them afterwards. `readback` files the frame into
/// the [`StreamReadback`] ring for encoding and streaming (the only sink before in-process
/// presentation existed, and the one every golden and screenshot path uses); `present` blits the
/// same texture into a window. A windowed host with no subscriber turns `readback` off and pays no
/// encode; a headless host leaves `present` at `None` and submits exactly the commands it always
/// did.
pub struct FrameSinks<'a> {
    /// Whether the frame is copied back to CPU memory through the ring.
    pub readback: bool,
    /// The window to blit the frame into after its own submit, if any.
    pub present: Option<&'a mut PresentTarget>,
}

impl<S> StreamReadback<S> {
    /// Maximum in-flight readbacks (the latency cap): a submission blocks only when all slots are busy.
    pub const DEPTH: usize = RING_DEPTH;

    fn new(device: &wgpu::Device, layout: RowLayout) -> StreamReadback<S> {
        let buffers = (0..Self::DEPTH)
            .map(|_| staging_buffer(device, &layout))
            .collect();
        StreamReadback {
            buffers,
            layout,
            slots: Slots::default(),
            // Read once per ring, never per frame: a disarmed run costs one `Option` test per
            // segment and reads no clock (the `TickTrace` discipline).
            trace: SubmitTrace::from_env(),
            physicals: None,
            last_output: None,
        }
    }

    /// The output texture of the most recent frame rendered through this ring; `None` before the
    /// first.
    pub fn last_output(&self) -> Option<&wgpu::Texture> {
        self.last_output.as_ref()
    }

    /// Whether the per-segment trace is armed — the one test a disarmed run pays.
    fn traced(&self) -> bool {
        self.trace.is_some()
    }

    /// Files one segment's elapsed time, if the trace is armed.
    fn trace_segment(&mut self, segment: Segment, watch: Watch) {
        if let Some(trace) = self.trace.as_mut() {
            trace.record(segment, watch.ms());
        }
    }

    /// The pixel dimensions the ring's buffers are sized for. A caller recreates the ring when its
    /// render size no longer matches these.
    pub fn dimensions(&self) -> (u32, u32) {
        (self.layout.width, self.layout.height)
    }

    /// Drops every in-flight readback and frees its slot, keeping the ring itself — for a caller
    /// that stops streaming but must not lose the ring's `PhysicalCache`, which is the frame
    /// graph's whole set of textures.
    ///
    /// A pending slot's staging buffer is replaced rather than unmapped: dropping the buffer is
    /// exactly what dropping the ring did to a pending entry, and it needs no map to have completed
    /// (unmapping one that has not is a validation error). A ring with nothing in flight touches no
    /// GPU resource at all, so the steady-state cost of calling this every idle pass is one length
    /// test.
    pub fn discard_pending(&mut self, gpu: &Gpu) {
        if self.slots.pending.is_empty() {
            return;
        }
        for Pending { slot, .. } in self.slots.pending.drain(..) {
            self.buffers[slot] = staging_buffer(&gpu.device, &self.layout);
        }
        self.slots.next_slot = 0;
    }

    /// Non-blocking drain of every completed readback, delivering the newest and unmapping the
    /// older ones, or `None` if the oldest is still in flight (or the ring is empty). Advances map
    /// callbacks with a non-blocking `poll(Poll)` first, then takes completed channels from the
    /// front — leaving a readback pending if it is not yet ready.
    ///
    /// A pass with nothing new to submit calls this so tail frames of a burst still drain.
    ///
    /// # Errors
    /// `L2005` if a readback's map completed with an error; `L2010` / `L2011` if the GPU
    /// reported a fault (see [`Gpu::take_fault`]).
    pub fn try_collect(&mut self, gpu: &Gpu) -> Result<Option<(S, RenderedImage)>, Diagnostic> {
        gpu.guarded(|| self.try_collect_unguarded(gpu))
    }

    fn try_collect_unguarded(
        &mut self,
        gpu: &Gpu,
    ) -> Result<Option<(S, RenderedImage)>, Diagnostic> {
        let watch = Watch::start(self.traced());
        gpu.device
            .poll(wgpu::PollType::Poll)
            .map_err(|error| map_failure(format!("device poll failed: {error}")))?;
        self.trace_segment(Segment::CollectPoll, watch);
        let Some((stamp, slot)) = self
            .slots
            .take_newest_ready(|slot| self.buffers[slot].unmap())?
        else {
            return Ok(None);
        };
        let watch = Watch::start(self.traced());
        let image = self.read_slot(slot)?;
        self.trace_segment(Segment::CollectUnpad, watch);
        Ok(Some((stamp, image)))
    }

    /// Blocks until every in-flight map completes, then drains them as [`try_collect`] does and
    /// returns the newest. Used when the ring is full (the latency cap): one wait empties it.
    fn collect_after_stall(&mut self, gpu: &Gpu) -> Result<(S, RenderedImage), Diagnostic> {
        let watch = Watch::start(self.traced());
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| map_failure(format!("device poll failed: {error}")))?;
        self.trace_segment(Segment::StallPoll, watch);
        let watch = Watch::start(self.traced());
        let Some((stamp, slot)) = self
            .slots
            .take_newest_ready(|slot| self.buffers[slot].unmap())?
        else {
            return Err(map_failure(
                "the oldest readback's map did not complete after a blocking poll".to_string(),
            ));
        };
        let image = self.read_slot(slot)?;
        self.trace_segment(Segment::StallUnpad, watch);
        Ok((stamp, image))
    }

    /// Blocks on the oldest in-flight readback, reads it, unmaps its buffer, and returns
    /// `(stamp, image)`. The measurement seam uses it: it needs every frame.
    fn collect_oldest_blocking(&mut self, gpu: &Gpu) -> Result<(S, RenderedImage), Diagnostic> {
        let Pending { stamp, slot, rx } = self
            .slots
            .pending
            .pop_front()
            .expect("collect_oldest_blocking called with no pending readback");
        let watch = Watch::start(self.traced());
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| map_failure(format!("device poll failed: {error}")))?;
        match rx.recv() {
            Ok(Ok(())) => {}
            other => {
                return Err(map_failure(format!(
                    "buffer map did not complete: {other:?}"
                )));
            }
        }
        self.trace_segment(Segment::StallPoll, watch);
        let watch = Watch::start(self.traced());
        let image = self.read_slot(slot)?;
        self.trace_segment(Segment::StallUnpad, watch);
        Ok((stamp, image))
    }

    /// Reads a mapped slot's buffer into a packed RGBA8 image and unmaps it.
    fn read_slot(&self, slot: usize) -> Result<RenderedImage, Diagnostic> {
        let buffer = &self.buffers[slot];
        let rgba = {
            let mapped = buffer
                .slice(..)
                .get_mapped_range()
                .map_err(|error| map_failure(format!("could not access mapped bytes: {error}")))?;
            self.layout.unpad(&mapped)
        };
        buffer.unmap();
        Ok(RenderedImage {
            width: self.layout.width,
            height: self.layout.height,
            rgba,
        })
    }
}

impl Gpu {
    /// Creates a [`StreamReadback`] ring sized for `output`'s row layout, reusable across frames.
    pub fn new_stream_readback<S>(
        &self,
        schedule: &Schedule,
        output: ResourceId,
    ) -> StreamReadback<S> {
        let layout = RowLayout::of(schedule.resource(output).desc());
        StreamReadback::new(&self.device, layout)
    }

    /// Submits one frame's render + texture→buffer copy into the ring's next slot, then non-blocking-
    /// collects every finished readback and delivers the newest (latest-wins inside the ring).
    /// Returns `Some((stamp, image))` if a readback completed this pass, or `None` if the
    /// newly-submitted frame's predecessors are all still in flight.
    ///
    /// Blocks only when the ring is full (every slot in flight): it then waits once, drains every
    /// finished slot and returns the newest, so the ring is not full again on the next pass.
    /// In-flight latency is capped at [`StreamReadback::DEPTH`] frames.
    ///
    /// The graph's physical textures are allocated once and reused across frames (see
    /// `PhysicalCache`), so a pass here is handed a target still holding the previous frame's pixels
    /// — it must clear it or cover it whole. The blocking paths allocate per call.
    ///
    /// `timing`, when given, brackets each pass with timestamp writes and resolves them into the
    /// [`PassTimingRing`]'s next slot; the sample is collected later via
    /// [`try_collect_timings`](PassTimingRing::try_collect_timings). `None` is the untimed path —
    /// no query set is bound, no resolve is recorded, and the submitted commands are exactly what
    /// they would be without a timing ring.
    ///
    /// # Errors
    /// `L2005` if a readback cannot be mapped. Timing never errors: a frame that cannot be timed is
    /// simply not.
    pub fn render_streamed<S: Clone>(
        &self,
        schedule: &Schedule,
        output: ResourceId,
        recorder: &dyn PassRecorder,
        ring: &mut StreamReadback<S>,
        stamp: S,
        timing: Option<&mut PassTimingRing<S>>,
    ) -> Result<Option<(S, RenderedImage)>, Diagnostic> {
        self.render_frame(
            schedule,
            output,
            recorder,
            ring,
            stamp,
            timing,
            FrameSinks {
                readback: true,
                present: None,
            },
        )
    }

    /// Renders one frame and hands the result to the sinks `sinks` names — see [`FrameSinks`].
    ///
    /// The passes are recorded exactly once whatever the sinks are; what changes is only what
    /// happens to the finished output texture. This is the general form of
    /// [`render_streamed`](Gpu::render_streamed), which is the both-defaults case
    /// (`readback: true`, no present target).
    ///
    /// With `readback` off nothing about the ring moves: no slot is consumed, no texture→buffer copy
    /// is recorded, no pending entry is filed, and the call returns `Ok(None)` — the window-only path
    /// that pays no encode. The physical-texture cache and the timing ring behave exactly as they do
    /// with readback on.
    ///
    /// A present target, when given, is blitted *after* the frame's own submit, so it reads finished
    /// pixels and adds nothing to the submitted commands. With no present target the submitted
    /// commands are byte-for-byte what they were before presentation existed.
    ///
    /// # Errors
    /// `L2005` if a readback cannot be mapped, `L2007` if the present target's surface fails
    /// permanently, `L2010` / `L2011` if the GPU rejected the work or the device was lost (see
    /// [`Gpu::take_fault`]). Timing never errors: a frame that cannot be timed is simply not.
    // One argument past the lint's limit: `render_streamed`'s six plus the sinks. Bundling any of
    // them into a struct would only rename the same values, and every one of them is a distinct
    // per-frame input the caller genuinely varies.
    #[allow(clippy::too_many_arguments)]
    pub fn render_frame<S: Clone>(
        &self,
        schedule: &Schedule,
        output: ResourceId,
        recorder: &dyn PassRecorder,
        ring: &mut StreamReadback<S>,
        stamp: S,
        timing: Option<&mut PassTimingRing<S>>,
        sinks: FrameSinks<'_>,
    ) -> Result<Option<(S, RenderedImage)>, Diagnostic> {
        self.guarded(|| {
            self.render_frame_unguarded(schedule, output, recorder, ring, stamp, timing, sinks)
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn render_frame_unguarded<S: Clone>(
        &self,
        schedule: &Schedule,
        output: ResourceId,
        recorder: &dyn PassRecorder,
        ring: &mut StreamReadback<S>,
        stamp: S,
        timing: Option<&mut PassTimingRing<S>>,
        sinks: FrameSinks<'_>,
    ) -> Result<Option<(S, RenderedImage)>, Diagnostic> {
        let traced = ring.traced();
        let total = Watch::start(traced);
        let occupancy = ring.slots.pending.len();
        if let Some(trace) = ring.trace.as_mut() {
            trace.occupancy[occupancy.min(RING_DEPTH)] += 1;
        }

        // Cap latency: if every slot is in flight, wait once and drain them all, freeing the slot
        // we reuse.
        // A frame that files no readback needs no slot, so it never stalls.
        let freed = if sinks.readback && ring.slots.is_full() {
            if let Some(trace) = ring.trace.as_mut() {
                trace.stalls += 1;
            }
            Some(ring.collect_after_stall(self)?)
        } else {
            None
        };

        let plan = match timing.as_deref() {
            Some(timing) => timing.plan_frame(schedule.order.len()),
            None => FramePlan::Skip,
        };
        let slot = ring.slots.next_slot;
        // Lifted out of the ring for the rest of the call so the ring stays mutably usable (tracing,
        // slot bookkeeping) while passes record into the textures the cache owns; put back below.
        let mut physicals = ring.physicals.take();
        let watch = Watch::start(traced);
        let frame_resources = self.ensure_physicals(schedule, &mut physicals);
        ring.trace_segment(Segment::Allocate, watch);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ludion-render-graph streamed frame"),
            });
        let query_set = match (timing.as_deref(), &plan) {
            (Some(timing), FramePlan::Timed { slot, .. }) => Some(timing.query_set(*slot)),
            _ => None,
        };
        let watch = Watch::start(traced);
        self.record_external_passes(schedule, frame_resources, &mut encoder, recorder, query_set);
        ring.trace_segment(Segment::Record, watch);
        if let (Some(timing), FramePlan::Timed { slot, queries }) = (timing.as_deref(), &plan) {
            timing.record_resolve(&mut encoder, *slot, *queries);
        }
        // The timed frame's stamp is cloned only when a sample will actually be filed.
        let timed_stamp = match plan {
            FramePlan::Skip => None,
            _ => Some(stamp.clone()),
        };
        let output_texture = &frame_resources[schedule.aliasing.assignment[output.index()]].texture;
        ring.last_output = Some(output_texture.clone());
        if sinks.readback {
            self.stream_track(ring, slot, output_texture, encoder, stamp);
        } else {
            let watch = Watch::start(traced);
            self.queue.submit([encoder.finish()]);
            ring.trace_segment(Segment::Submit, watch);
        }
        // After the frame's own submit, so the blit samples finished pixels.
        let presented = match sinks.present {
            Some(present) => {
                let watch = Watch::start(traced);
                let result = present.present(self, output_texture).map(|_| ());
                ring.trace_segment(Segment::Present, watch);
                result
            }
            None => Ok(()),
        };
        ring.physicals = physicals;
        // Filed after submit: the readback map can only be requested once the work is queued.
        if let (Some(timing), Some(timed_stamp)) = (timing, timed_stamp) {
            let names = schedule
                .order_names()
                .into_iter()
                .map(str::to_owned)
                .collect();
            timing.file(plan, names, timed_stamp);
        }
        // Raised only after the frame's own bookkeeping is closed, so a dying surface leaves the
        // ring and the timing ring in the state the frame actually left them in.
        presented?;

        // Return the newest frame the stall drained, or a non-blocking collect of what finished.
        let collected = match (freed, sinks.readback) {
            (Some(frame), _) => Ok(Some(frame)),
            (None, true) => ring.try_collect(self),
            (None, false) => Ok(None),
        };
        ring.trace_segment(Segment::Total, total);
        if let Some(trace) = ring.trace.as_mut() {
            trace.frame_done();
        }
        collected
    }

    /// Records `output`'s copy into `slot`, submits the encoder, `map_async`'s the slot, and tracks it
    /// as pending with `stamp`. Shared by [`render_streamed`](Gpu::render_streamed) and the
    /// measurement seam.
    fn stream_track<S>(
        &self,
        ring: &mut StreamReadback<S>,
        slot: usize,
        output: &wgpu::Texture,
        mut encoder: wgpu::CommandEncoder,
        stamp: S,
    ) {
        copy_texture_to_buffer(&mut encoder, output, &ring.buffers[slot], &ring.layout);
        let watch = Watch::start(ring.traced());
        self.queue.submit([encoder.finish()]);
        ring.trace_segment(Segment::Submit, watch);
        let (tx, rx) = std::sync::mpsc::channel();
        ring.buffers[slot]
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        ring.slots.file(stamp, rx);
    }

    /// Renders `count` frames through the async staging ring and returns them in frame order.
    ///
    /// All frames render the same deterministic graph, so every returned image is byte-identical;
    /// the point of the method is to exercise the pipelined readback path (risk-register row 2) —
    /// up to [`StreamReadback::DEPTH`] `map_async` readbacks in flight — rather than to produce
    /// distinct frames. It shares the ring's buffer lifecycle and collection with the live path; only
    /// the pass recording differs (the fixed proving graph's built-in passes, not an external
    /// recorder).
    ///
    /// # Errors
    /// `L2005` if any frame's readback cannot be mapped; `L2010` / `L2011` if the GPU reported a
    /// fault (see [`Gpu::take_fault`]).
    pub fn render_frames_via_ring(
        &self,
        schedule: &Schedule,
        output: ResourceId,
        count: usize,
    ) -> Result<Vec<RenderedImage>, Diagnostic> {
        self.guarded(|| self.render_frames_via_ring_unguarded(schedule, output, count))
    }

    fn render_frames_via_ring_unguarded(
        &self,
        schedule: &Schedule,
        output: ResourceId,
        count: usize,
    ) -> Result<Vec<RenderedImage>, Diagnostic> {
        let mut ring: StreamReadback<usize> = self.new_stream_readback(schedule, output);
        let frame_resources = self.allocate_physicals(schedule);
        let output_physical = &frame_resources[schedule.aliasing.assignment[output.index()]];

        let mut results: Vec<Option<RenderedImage>> = (0..count).map(|_| None).collect();

        for frame in 0..count {
            // Cap in-flight readbacks at DEPTH; the queue is FIFO, so the oldest holds the slot we
            // are about to reuse when the ring is full.
            if ring.slots.is_full() {
                let (done_frame, image) = ring.collect_oldest_blocking(self)?;
                results[done_frame] = Some(image);
            }

            let slot = ring.slots.next_slot;
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("ludion-render-graph ring frame"),
                });
            self.record_passes(schedule, &frame_resources, &mut encoder, None);
            self.stream_track(&mut ring, slot, &output_physical.texture, encoder, frame);
        }

        while !ring.slots.pending.is_empty() {
            let (done_frame, image) = ring.collect_oldest_blocking(self)?;
            results[done_frame] = Some(image);
        }

        Ok(results
            .into_iter()
            .map(|image| image.expect("every frame's readback was collected"))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Sender, channel};

    use super::*;

    type Completion = Sender<Result<(), wgpu::BufferAsyncError>>;

    fn file(slots: &mut Slots<u32>, stamp: u32) -> Completion {
        let (tx, rx) = channel();
        slots.file(stamp, rx);
        tx
    }

    fn take(
        slots: &mut Slots<u32>,
        released: &mut Vec<usize>,
    ) -> Result<Option<(u32, usize)>, Diagnostic> {
        slots.take_newest_ready(|slot| released.push(slot))
    }

    #[test]
    fn a_stall_drains_every_completed_slot_and_keeps_the_newest() {
        let mut slots = Slots::default();
        let senders: Vec<_> = (0..RING_DEPTH as u32)
            .map(|f| file(&mut slots, f))
            .collect();
        assert!(slots.is_full());
        for tx in &senders {
            tx.send(Ok(())).unwrap();
        }
        let mut released = Vec::new();
        let newest = take(&mut slots, &mut released).unwrap();
        let last = RING_DEPTH as u32 - 1;
        assert_eq!(newest, Some((last, last as usize)));
        assert_eq!(released, (0..last as usize).collect::<Vec<_>>());
        file(&mut slots, 99);
        assert!(
            !slots.is_full(),
            "the ring must not be full again after the stall"
        );
        assert_eq!(slots.pending.len(), 1);
    }

    #[test]
    fn a_collect_after_a_hitch_delivers_the_newest_and_keeps_the_in_flight() {
        let mut slots = Slots::default();
        let senders: Vec<_> = (0..3).map(|f| file(&mut slots, f)).collect();
        senders[0].send(Ok(())).unwrap();
        senders[1].send(Ok(())).unwrap();
        let mut released = Vec::new();
        assert_eq!(take(&mut slots, &mut released).unwrap(), Some((1, 1)));
        assert_eq!(released, vec![0]);
        assert_eq!(slots.pending.len(), 1);
        assert_eq!(slots.pending.front().unwrap().stamp, 2);
    }

    #[test]
    fn a_failed_map_releases_the_held_frame_and_is_l2005() {
        let mut slots = Slots::default();
        let first = file(&mut slots, 0);
        let second = file(&mut slots, 1);
        first.send(Ok(())).unwrap();
        second.send(Err(wgpu::BufferAsyncError)).unwrap();
        let mut released = Vec::new();
        let error = take(&mut slots, &mut released).unwrap_err();
        assert_eq!(error.code, code::RENDER_READBACK_FAILED);
        assert_eq!(released, vec![0]);
    }

    #[test]
    fn nothing_ready_takes_nothing() {
        let mut slots = Slots::default();
        let _tx = file(&mut slots, 0);
        let mut released = Vec::new();
        assert_eq!(take(&mut slots, &mut released).unwrap(), None);
        assert!(released.is_empty());
        assert_eq!(slots.pending.len(), 1);
    }
}
