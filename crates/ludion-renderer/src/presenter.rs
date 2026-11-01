//! The presenter: a thread that owns a window's renderer and swapchain and paces itself to the
//! display (`Plan/display-paced-present.md`).
//!
//! The sim thread extracts a [`RenderScene`] once per tick and [`publish`](Presenter::publish)es it as
//! a [`PresentPacket`] into a one-slot mailbox — latest wins, the sim never waits beyond a pointer
//! swap. The presenter thread renders a packet it has not seen (or, when the packet carries blend data,
//! every refresh at the alpha between the last two ticks), then presents the last output; under
//! `Fifo` the present blocks in swapchain acquire, so the loop runs at the display's refresh rate and
//! a 120 Hz panel re-presents each tick's image without re-rendering it. Faults and the renderer's
//! probe resolve come back as [`PresenterNote`]s the sim thread drains.
//!
//! The thread talks to its renderer and surface through [`PresentBackend`]; [`WindowBackend`] is the
//! real one, tests substitute a fake. Nothing here touches `World`, `Engine` or a command, and nothing
//! it reads from the clock reaches the sim: this file is the renderer crate's one wall-clock site.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::FrameIndex;
use ludion_render_graph::{FrameSinks, Gpu, PresentOutcome, PresentTarget, StreamReadback};

use crate::blend::{BlendData, blend};
use crate::graph::{AntiAliasing, FrameSize};
use crate::probe_capture::ProbeResolve;
use crate::quality::{Quality, QualityParams};
use crate::render::Renderer;
use crate::scene::RenderScene;

/// The longest the loop parks on the mailbox when there is nothing to present (a minimized window,
/// or no packet yet) — short enough to notice a restore, long enough never to spin.
const IDLE_WAIT: Duration = Duration::from_millis(100);

/// How long the loop parks after a present that was skipped without blocking.
const SKIP_WAIT: Duration = Duration::from_millis(4);

/// Presents between two `LUDION_PRESENT_TRACE` summaries.
const TRACE_INTERVAL: usize = 600;

/// The env var that arms the present trace. Any non-empty value other than `"0"` arms it.
const TRACE_ENV: &str = "LUDION_PRESENT_TRACE";

/// The render-side state the engine pushes into its renderer each frame; the presenter applies it
/// when it differs from the last applied.
#[derive(Clone, Debug, PartialEq)]
pub struct PresentSettings {
    /// The quality preset.
    pub quality: Quality,
    /// The preset's tunable parameters.
    pub params: QualityParams,
    /// The anti-aliasing mode.
    pub anti_aliasing: AntiAliasing,
    /// Whether the baked GI baseline is held.
    pub gi_hold: bool,
    /// The reflection-probe store directory, if any.
    pub probe_store: Option<PathBuf>,
}

/// When a tick ran on the sim thread's wall clock, for the presenter's trace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TickStamp {
    /// The instant the tick ran.
    pub at: Instant,
    /// The wall-clock length one tick should occupy.
    pub period: Duration,
    /// Whether the live loop re-based its deadline for this tick (a lag, a speed change, a resume).
    pub rebased: bool,
}

/// One tick's worth of render input, handed from the sim thread to the presenter.
#[derive(Clone)]
pub struct PresentPacket {
    /// The tick's frame index.
    pub frame: FrameIndex,
    /// The extracted scene; immutable, shared with any stream render of the same pass.
    pub scene: Arc<RenderScene>,
    /// The render and output extents.
    pub size: FrameSize,
    /// Render-side settings in force for this frame.
    pub settings: PresentSettings,
    /// The tick's wall-clock stamp, when the live loop supplies one.
    pub tick: Option<TickStamp>,
    /// The poses and keys that let a display-rate frame blend toward this tick, when the host
    /// collected them.
    pub blend: Option<Arc<BlendData>>,
}

/// What the presenter sends back to the sim thread.
#[derive(Debug)]
pub enum PresenterNote {
    /// A permanent fault (`L2007` dead surface, `L2011` lost device); the presenter has stopped presenting.
    Fault(Diagnostic),
    /// The renderer's probe resolve changed.
    Probes(ProbeResolve),
    /// A non-fatal failure to log, deduplicated.
    Log(Diagnostic),
}

/// The renderer and surface behind a [`Presenter`]; the thread owns it for its lifetime.
pub trait PresentBackend: Send {
    /// Renders `packet` into the backend's own output, applying its settings first.
    ///
    /// # Errors
    /// Whatever the render raises (`L2007`/`L2011` are fatal to the presenter; anything else is logged).
    fn render(&mut self, packet: &PresentPacket) -> Result<(), Diagnostic>;
    /// Presents the last rendered output; blocks at the display's rate. A no-op before the first
    /// render. Returns [`PresentOutcome::Skipped`] when nothing was presented and the call did not
    /// block, so the loop paces itself instead of spinning.
    ///
    /// # Errors
    /// `L2007` if the surface is permanently unavailable.
    fn present(&mut self) -> Result<PresentOutcome, Diagnostic>;
    /// The renderer's probe resolve so far, if it has one.
    fn probe_resolve(&self) -> Option<ProbeResolve>;
    /// Hot-reloads one shader on the backend's renderer; returns the new shader generation.
    ///
    /// # Errors
    /// `SHADER_RELOAD_INVALID` for an unknown path, or the compile error.
    fn reload_shader(&mut self, path: &str, source: String) -> Result<u64, Diagnostic>;
    /// Whether there is nothing to present into (a 0×0 surface).
    fn idle(&self) -> bool;
}

struct Slot {
    packet: Option<PresentPacket>,
    /// The first tick whose cuts `packet` carries: its own frame, or an earlier one merged in.
    covers_from: FrameIndex,
    reloads: Vec<(String, String)>,
    closed: bool,
}

struct Shared {
    slot: Mutex<Slot>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Slot> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The handle to a running presenter thread. Dropping it closes the mailbox and joins the thread.
pub struct Presenter {
    shared: Arc<Shared>,
    notes: Receiver<PresenterNote>,
    thread: Option<JoinHandle<()>>,
}

impl Presenter {
    /// Starts the thread over `backend`. Reads `LUDION_PRESENT_TRACE` once.
    pub fn spawn(backend: Box<dyn PresentBackend>) -> Presenter {
        let shared = Arc::new(Shared {
            slot: Mutex::new(Slot {
                packet: None,
                covers_from: FrameIndex(0),
                reloads: Vec::new(),
                closed: false,
            }),
            wake: Condvar::new(),
        });
        let (notes_tx, notes) = mpsc::channel();
        let trace = PresentTrace::from_env();
        let thread = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("ludion-presenter".to_owned())
                .spawn(move || run_loop(&shared, backend, &notes_tx, trace))
                .expect("spawning the presenter thread")
        };
        Presenter {
            shared,
            notes,
            thread: Some(thread),
        }
    }

    /// Replaces the mailbox's packet with `packet` and wakes the presenter. Never waits on it. A packet
    /// it replaces unseen hands its blend cuts to `packet`, so the presenter still blends across the
    /// ticks it never saw.
    pub fn publish(&self, mut packet: PresentPacket) {
        let mut slot = self.shared.lock();
        slot.covers_from = match slot.packet.take() {
            Some(replaced) => merge_replaced(replaced, &mut packet, slot.covers_from),
            None => packet.frame,
        };
        slot.packet = Some(packet);
        drop(slot);
        self.shared.wake.notify_one();
    }

    /// Queues a shader hot reload for the presenter's renderer; a failure comes back as a
    /// [`PresenterNote::Log`].
    pub fn reload_shader(&self, path: &str, source: String) {
        self.shared.lock().reloads.push((path.to_owned(), source));
        self.shared.wake.notify_one();
    }

    /// Drains the notes the presenter has sent since the last call, oldest first.
    pub fn take_notes(&self) -> Vec<PresenterNote> {
        self.notes.try_iter().collect()
    }
}

impl Drop for Presenter {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.wake.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The diagnostics that end presenting: a dead surface, a rejected GPU, a lost device.
fn is_fatal(diag: &Diagnostic) -> bool {
    diag.code == code::WINDOW_PRESENTATION_UNAVAILABLE || diag.code == code::RENDER_DEVICE_LOST
}

/// The presenter loop — the renderer crate's one wall-clock site, reading `Instant::now` only to
/// time its own acquire+present and feed the opt-in trace; nothing it reads reaches the sim.
#[allow(clippy::disallowed_methods)]
fn run_loop(
    shared: &Shared,
    mut backend: Box<dyn PresentBackend>,
    notes: &Sender<PresenterNote>,
    mut trace: Option<PresentTrace>,
) {
    let mut current: Option<PresentPacket> = None;
    let mut prev_blend: Option<(Arc<RenderScene>, Arc<BlendData>)> = None;
    // The last drawn frame, and whether it was drawn at alpha 1 (a settled image needs no redraw).
    let mut drawn: Option<(FrameIndex, bool)> = None;
    let mut dead = false;
    let mut last_probes = ProbeResolve::default();
    let mut last_error: Option<String> = None;
    let mut skipped = false;
    loop {
        let (packet, covers_from, reloads) = {
            let mut slot = shared.lock();
            if slot.closed {
                return;
            }
            (
                slot.packet.take(),
                slot.covers_from,
                std::mem::take(&mut slot.reloads),
            )
        };
        for (path, source) in reloads {
            if let Err(diag) = backend.reload_shader(&path, source) {
                let _ = notes.send(PresenterNote::Log(diag));
            }
        }
        if dead {
            wait(shared, IDLE_WAIT);
            continue;
        }
        let mut new_tick = false;
        if let Some(packet) = packet
            && current.as_ref().is_none_or(|c| c.frame != packet.frame)
        {
            if let (Some(trace), Some(tick)) = (trace.as_mut(), packet.tick) {
                trace.note_tick(tick);
            }
            prev_blend = blend_source(current.as_ref(), &packet, covers_from);
            current = Some(packet);
            new_tick = true;
        }
        let mut blended_now = false;
        let mut failure = None;
        // After a skipped present, only a new tick is worth a render: a blend redrawn for a frame
        // nobody is acquiring would just burn the GPU.
        if let Some(cur) = current.as_ref()
            && (new_tick || !skipped)
        {
            let blending = match (&prev_blend, &cur.blend) {
                (Some((prev_scene, prev)), Some(next)) => {
                    let alpha = alpha_at(Instant::now(), cur.tick);
                    (alpha < 1.0).then_some((prev_scene, prev, next, alpha))
                }
                _ => None,
            };
            let settled = blending.is_none();
            if !(settled && drawn == Some((cur.frame, true))) {
                let result = match blending {
                    Some((prev_scene, prev, next, alpha)) => backend.render(&PresentPacket {
                        scene: Arc::new(blend(prev_scene, prev, &cur.scene, next, alpha)),
                        ..cur.clone()
                    }),
                    None => backend.render(cur),
                };
                match result {
                    Ok(()) => {
                        drawn = Some((cur.frame, settled));
                        blended_now = !settled;
                        if let Some(probes) = backend.probe_resolve()
                            && probes != last_probes
                        {
                            last_probes = probes.clone();
                            let _ = notes.send(PresenterNote::Probes(probes));
                        }
                    }
                    Err(diag) => failure = Some(diag),
                }
            }
        }
        if let Some(diag) = failure {
            // Not retried: the failed packet is dropped and the last good image keeps presenting.
            current = None;
            prev_blend = None;
            if report(notes, &mut last_error, diag) {
                dead = true;
                continue;
            }
        }
        if backend.idle() || drawn.is_none() {
            wait(shared, IDLE_WAIT);
            continue;
        }
        let started = Instant::now();
        skipped = false;
        match backend.present() {
            Err(diag) => {
                if report(notes, &mut last_error, diag) {
                    dead = true;
                }
            }
            Ok(PresentOutcome::Presented) => {
                if let (Some(trace), Some((frame, _))) = (trace.as_mut(), drawn) {
                    trace.note_present(frame, started.elapsed(), blended_now);
                }
            }
            // Nothing paced this iteration (occluded, stale or timed-out swapchain): park instead of
            // re-rendering a blend in a tight loop.
            Ok(PresentOutcome::Skipped) => {
                skipped = true;
                if let Some(trace) = trace.as_mut() {
                    trace.skips += 1;
                }
                wait(shared, SKIP_WAIT);
            }
        }
    }
}

/// Folds the blend cuts of `replaced` — a packet the presenter never took — into `packet`, the
/// next one published, and returns the first tick whose cuts `packet` now carries (`covers_from` is
/// `replaced`'s own). A gap in frame numbers means a tick was never published: the chain restarts.
/// A replaced packet without blend data, or a re-based one, forces a snap.
fn merge_replaced(
    replaced: PresentPacket,
    packet: &mut PresentPacket,
    covers_from: FrameIndex,
) -> FrameIndex {
    if replaced.frame.0 + 1 != packet.frame.0 {
        return packet.frame;
    }
    let rebased = replaced.tick.is_some_and(|tick| tick.rebased);
    match (&replaced.blend, packet.blend.as_mut()) {
        (Some(old), Some(new)) => {
            if !old.snaps.is_empty() {
                let new = Arc::make_mut(new);
                new.snaps.extend_from_slice(&old.snaps);
                new.snaps.sort_unstable();
                new.snaps.dedup();
            }
        }
        (None, Some(_)) => return packet.frame,
        _ => {}
    }
    if rebased && let Some(tick) = packet.tick.as_mut() {
        tick.rebased = true;
    }
    covers_from
}

/// The previous tick's scene and blend data when `next` can blend toward it: both ticks carry data, every
/// tick between them reached the presenter as a merged cut set (`covers_from`) and `next` was not a
/// re-based tick.
fn blend_source(
    current: Option<&PresentPacket>,
    next: &PresentPacket,
    covers_from: FrameIndex,
) -> Option<(Arc<RenderScene>, Arc<BlendData>)> {
    let current = current?;
    next.blend.as_ref()?;
    let contiguous = current.frame.0 + 1 == covers_from.0;
    let rebased = next.tick.is_some_and(|tick| tick.rebased);
    (contiguous && !rebased)
        .then(|| current.blend.clone().map(|d| (current.scene.clone(), d)))
        .flatten()
}

/// How far `now` is through `tick`'s period, clamped to `0..=1`; `1.0` with no tick, a zero period
/// or a `now` before the tick. Takes `now` so it is not a clock site.
fn alpha_at(now: Instant, tick: Option<TickStamp>) -> f32 {
    let Some(tick) = tick else { return 1.0 };
    if tick.period.is_zero() || now < tick.at {
        return 1.0;
    }
    (now.duration_since(tick.at).as_secs_f32() / tick.period.as_secs_f32()).clamp(0.0, 1.0)
}

/// Sends `diag` as a fault or a deduplicated log note; true when it was fatal.
fn report(
    notes: &Sender<PresenterNote>,
    last_error: &mut Option<String>,
    diag: Diagnostic,
) -> bool {
    if is_fatal(&diag) {
        let _ = notes.send(PresenterNote::Fault(diag));
        return true;
    }
    let rendered = diag.to_string();
    if last_error.as_deref() != Some(rendered.as_str()) {
        let _ = notes.send(PresenterNote::Log(diag));
        *last_error = Some(rendered);
    }
    false
}

/// Parks until something is published, a reload is queued, the handle closes, or `timeout` passes.
fn wait(shared: &Shared, timeout: Duration) {
    let slot = shared.lock();
    if slot.closed || slot.packet.is_some() || !slot.reloads.is_empty() {
        return;
    }
    let _ = shared
        .wake
        .wait_timeout(slot, timeout)
        .unwrap_or_else(PoisonError::into_inner);
}

/// The armed `LUDION_PRESENT_TRACE` state: counts over one [`TRACE_INTERVAL`] of presents, printed
/// to stderr. Pure observation, shaped like the live loop's `TickTrace`.
struct PresentTrace {
    presents: usize,
    distinct: usize,
    repeats: usize,
    gaps: u64,
    rebased: usize,
    blended: usize,
    skips: usize,
    last_frame: Option<FrameIndex>,
    waits_ms: Vec<f64>,
    window_start: Option<Instant>,
}

impl PresentTrace {
    fn from_env() -> Option<PresentTrace> {
        let armed = std::env::var(TRACE_ENV).is_ok_and(|value| !value.is_empty() && value != "0");
        armed.then(|| PresentTrace {
            presents: 0,
            distinct: 0,
            repeats: 0,
            gaps: 0,
            rebased: 0,
            blended: 0,
            skips: 0,
            last_frame: None,
            waits_ms: Vec::with_capacity(TRACE_INTERVAL),
            window_start: None,
        })
    }

    fn note_tick(&mut self, tick: TickStamp) {
        if tick.rebased {
            self.rebased += 1;
        }
    }

    #[allow(clippy::disallowed_methods)]
    fn note_present(&mut self, frame: FrameIndex, wait: Duration, blended: bool) {
        let now = Instant::now();
        let start = *self.window_start.get_or_insert(now - wait);
        self.presents += 1;
        self.blended += usize::from(blended);
        match self.last_frame {
            Some(last) if last == frame => self.repeats += 1,
            Some(last) => {
                self.distinct += 1;
                self.gaps += frame.0.saturating_sub(last.0).saturating_sub(1);
            }
            None => self.distinct += 1,
        }
        self.last_frame = Some(frame);
        self.waits_ms.push(wait.as_secs_f64() * 1000.0);
        if self.presents >= TRACE_INTERVAL {
            self.report(now.duration_since(start));
        }
    }

    fn report(&mut self, elapsed: Duration) {
        self.waits_ms.sort_by(f64::total_cmp);
        let at = |fraction: f64| {
            let index = ((self.waits_ms.len() as f64 * fraction).ceil() as usize).max(1) - 1;
            self.waits_ms[index.min(self.waits_ms.len() - 1)]
        };
        eprintln!(
            "present-trace: {:.1} presents/s, {} distinct frames, {} repeats, {} frame-index gaps, \
             {} rebased ticks, {} blended presents, {} skipped presents, \
             acquire+present p50 {:.2} ms p99 {:.2} ms",
            self.presents as f64 / elapsed.as_secs_f64().max(1e-9),
            self.distinct,
            self.repeats,
            self.gaps,
            self.rebased,
            self.blended,
            self.skips,
            at(0.50),
            at(0.99),
        );
        self.presents = 0;
        self.distinct = 0;
        self.repeats = 0;
        self.gaps = 0;
        self.rebased = 0;
        self.blended = 0;
        self.skips = 0;
        self.waits_ms.clear();
        self.window_start = None;
    }
}

/// The real backend: a [`Renderer`], the private ring whose textures hold the rendered output, and
/// the window's [`PresentTarget`].
pub struct WindowBackend {
    renderer: Renderer,
    ring: Option<StreamReadback<FrameIndex>>,
    ring_size: Option<FrameSize>,
    target: PresentTarget,
    applied: Option<PresentSettings>,
}

impl WindowBackend {
    /// Builds a backend over `gpu` and the window's `target`. The renderer is built on `gpu`
    /// itself, and the blit to the window goes through the renderer's context; the caller passes a
    /// [`Gpu::share`] when it keeps its own handle.
    ///
    /// # Errors
    /// Whatever [`Renderer::from_gpu`] raises.
    pub fn new(gpu: Gpu, target: PresentTarget) -> Result<WindowBackend, Diagnostic> {
        let renderer = Renderer::from_gpu(gpu)?;
        Ok(WindowBackend {
            renderer,
            ring: None,
            ring_size: None,
            target,
            applied: None,
        })
    }

    fn apply(&mut self, settings: &PresentSettings) {
        if self.applied.as_ref() == Some(settings) {
            return;
        }
        self.renderer.set_quality(settings.quality, settings.params);
        self.renderer.set_anti_aliasing(settings.anti_aliasing);
        self.renderer.set_gi_hold(settings.gi_hold);
        self.renderer.set_probe_store(settings.probe_store.clone());
        self.applied = Some(settings.clone());
    }
}

impl PresentBackend for WindowBackend {
    fn render(&mut self, packet: &PresentPacket) -> Result<(), Diagnostic> {
        self.apply(&packet.settings);
        if self.ring.is_none() || self.ring_size != Some(packet.size) {
            self.ring = Some(self.renderer.new_stream_readback(packet.size)?);
            self.ring_size = Some(packet.size);
        }
        let ring = self.ring.as_mut().expect("ring built above");
        self.renderer.render_frame(
            &packet.scene,
            packet.size,
            ring,
            packet.frame,
            None,
            FrameSinks {
                readback: false,
                present: None,
            },
        )?;
        Ok(())
    }

    fn present(&mut self) -> Result<PresentOutcome, Diagnostic> {
        match self.ring.as_ref().and_then(StreamReadback::last_output) {
            Some(output) => self.target.present(self.renderer.gpu(), output),
            None => Ok(PresentOutcome::Skipped),
        }
    }

    fn probe_resolve(&self) -> Option<ProbeResolve> {
        Some(self.renderer.probe_resolve())
    }

    fn reload_shader(&mut self, path: &str, source: String) -> Result<u64, Diagnostic> {
        self.renderer.reload_shader(path, source)
    }

    fn idle(&self) -> bool {
        let (width, height) = self.target.requested_size();
        width == 0 || height == 0
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // tests stamp ticks on the wall clock
mod tests {
    use super::*;

    use crate::scene::{MeshInstance, MeshShape};
    use ludion_core::ids::EntityId;
    use ludion_core::math::glam::{Mat3, Mat4, Vec3};

    #[derive(Default)]
    struct Log {
        renders: Vec<u64>,
        /// The x translation of the first mesh at each render, parallel to `renders`.
        xs: Vec<f32>,
        /// Every mesh's x translation at each render.
        mesh_xs: Vec<Vec<f32>>,
        presents: usize,
    }

    /// Renders are logged; `present` blocks on `gate` until a token arrives (or the sender drops),
    /// standing in for a display that paces the loop.
    struct Fake {
        log: Arc<Mutex<Log>>,
        gate: Receiver<()>,
    }

    impl PresentBackend for Fake {
        fn render(&mut self, packet: &PresentPacket) -> Result<(), Diagnostic> {
            let mut log = self.log.lock().unwrap();
            log.renders.push(packet.frame.0);
            if let Some(mesh) = packet.scene.meshes.first() {
                log.xs.push(mesh.model.w_axis.x);
            }
            log.mesh_xs.push(
                packet
                    .scene
                    .meshes
                    .iter()
                    .map(|m| m.model.w_axis.x)
                    .collect(),
            );
            Ok(())
        }
        fn present(&mut self) -> Result<PresentOutcome, Diagnostic> {
            let _ = self.gate.recv();
            self.log.lock().unwrap().presents += 1;
            Ok(PresentOutcome::Presented)
        }
        fn probe_resolve(&self) -> Option<ProbeResolve> {
            None
        }
        fn reload_shader(&mut self, _: &str, _: String) -> Result<u64, Diagnostic> {
            Ok(0)
        }
        fn idle(&self) -> bool {
            false
        }
    }

    fn packet(frame: u64) -> PresentPacket {
        PresentPacket {
            frame: FrameIndex(frame),
            scene: Arc::new(RenderScene::empty((16, 16))),
            size: FrameSize::native(16, 16),
            settings: PresentSettings {
                quality: Quality::FULL,
                params: QualityParams::MEDIUM,
                anti_aliasing: AntiAliasing::Off,
                gi_hold: false,
                probe_store: None,
            },
            tick: None,
            blend: None,
        }
    }

    const KEY: EntityId = EntityId::new(1, 0);
    const KEY_B: EntityId = EntityId::new(2, 0);

    fn mesh_at(model: Mat4) -> MeshInstance {
        MeshInstance {
            shape: MeshShape::Cube,
            model,
            normal_matrix: Mat3::from_mat4(model),
            color: [1.0; 4],
            material: [0.5, 0.0, 0.0],
            use_vertex_material: false,
            fade: 1.0,
            fade_height_min: -f32::MAX,
            flash: 0.0,
            flash_color: [1.0; 3],
            bone_offset: 0,
        }
    }

    /// Two keyed meshes: `KEY` at x = `frame`, `KEY_B` at x = 10 × `frame`; `snaps` names the entities
    /// cut this tick. A 10 s period keeps alpha near 0 for the whole test.
    fn pair(frame: u64, now: Instant, snaps: &[EntityId]) -> PresentPacket {
        let a = Mat4::from_translation(Vec3::new(frame as f32, 0.0, 0.0));
        let b = Mat4::from_translation(Vec3::new(10.0 * frame as f32, 0.0, 0.0));
        let mut scene = RenderScene::empty((16, 16));
        scene.meshes.extend([mesh_at(a), mesh_at(b)]);
        PresentPacket {
            scene: Arc::new(scene),
            tick: Some(TickStamp {
                at: now,
                period: Duration::from_secs(10),
                rebased: false,
            }),
            blend: Some(Arc::new(BlendData {
                poses: vec![(KEY, a), (KEY_B, b)],
                snaps: snaps.to_vec(),
                meshes: vec![Some(KEY), Some(KEY_B)],
                ..BlendData::default()
            })),
            ..packet(frame)
        }
    }

    /// Frames 1, 2, 3 reach the mailbox while the loop is blocked presenting frame 1, so 2 is
    /// replaced unseen; returns every mesh's x in the render of frame 3.
    fn frame_three_xs(snaps_on_two: &[EntityId]) -> Vec<f32> {
        let (presenter, log, gate_tx) = spawn_fake();
        let now = Instant::now();
        presenter.publish(pair(1, now, &[]));
        until("frame 1 rendered", || {
            log.lock().unwrap().renders.len() == 1
        });
        presenter.publish(pair(2, now, snaps_on_two));
        presenter.publish(pair(3, now, &[]));
        gate_tx.send(()).unwrap();
        until("frame 3 rendered", || {
            log.lock().unwrap().renders.len() >= 2
        });
        let log = log.lock().unwrap();
        assert_eq!(log.renders[1], 3, "frame 2 was replaced unseen");
        log.mesh_xs[1].clone()
    }

    #[test]
    fn a_replaced_packet_keeps_its_cuts_and_the_blend_spans_the_gap() {
        // The entity cut on the lost tick snaps; the other blends from frame 1 (alpha ~ 0).
        let xs = frame_three_xs(&[KEY_B]);
        assert!((xs[0] - 1.0).abs() < 0.01, "{xs:?}");
        assert_eq!(xs[1], 30.0, "{xs:?}");
        // No cuts: neither entity snaps.
        let xs = frame_three_xs(&[]);
        assert!((xs[0] - 1.0).abs() < 0.01, "{xs:?}");
        assert!((xs[1] - 10.0).abs() < 0.1, "{xs:?}");
    }

    #[test]
    fn a_skipped_present_parks_instead_of_spinning() {
        struct Skipping(Arc<Mutex<Log>>);
        impl PresentBackend for Skipping {
            fn render(&mut self, packet: &PresentPacket) -> Result<(), Diagnostic> {
                self.0.lock().unwrap().renders.push(packet.frame.0);
                Ok(())
            }
            fn present(&mut self) -> Result<PresentOutcome, Diagnostic> {
                self.0.lock().unwrap().presents += 1;
                Ok(PresentOutcome::Skipped)
            }
            fn probe_resolve(&self) -> Option<ProbeResolve> {
                None
            }
            fn reload_shader(&mut self, _: &str, _: String) -> Result<u64, Diagnostic> {
                Ok(0)
            }
            fn idle(&self) -> bool {
                false
            }
        }
        let log = Arc::new(Mutex::new(Log::default()));
        let presenter = Presenter::spawn(Box::new(Skipping(Arc::clone(&log))));
        let now = Instant::now();
        presenter.publish(pair(1, now, &[]));
        presenter.publish(pair(2, now, &[]));
        std::thread::sleep(Duration::from_millis(300));
        drop(presenter);
        let log = log.lock().unwrap();
        // 4 ms parks bound the attempts near 75; a spin would be orders of magnitude more.
        assert!(log.presents < 150, "{} present attempts", log.presents);
        assert!(
            log.renders.len() <= 2,
            "blends re-rendered: {:?}",
            log.renders
        );
    }

    /// A one-mesh packet whose entity sits at x = `frame`, with matching blend data, stamped `at`
    /// with a `period` tick.
    fn blendable(frame: u64, at: Instant, period: Duration, rebased: bool) -> PresentPacket {
        let model = Mat4::from_translation(Vec3::new(frame as f32, 0.0, 0.0));
        let mut scene = RenderScene::empty((16, 16));
        scene.meshes.push(mesh_at(model));
        PresentPacket {
            scene: Arc::new(scene),
            tick: Some(TickStamp {
                at,
                period,
                rebased,
            }),
            blend: Some(Arc::new(BlendData {
                poses: vec![(KEY, model)],
                meshes: vec![Some(KEY)],
                ..BlendData::default()
            })),
            ..packet(frame)
        }
    }

    fn spawn_fake() -> (Presenter, Arc<Mutex<Log>>, mpsc::Sender<()>) {
        let log = Arc::new(Mutex::new(Log::default()));
        let (gate_tx, gate) = mpsc::channel();
        let presenter = Presenter::spawn(Box::new(Fake {
            log: Arc::clone(&log),
            gate,
        }));
        (presenter, log, gate_tx)
    }

    #[test]
    fn a_packet_blends_between_ticks_and_never_extrapolates() {
        let (presenter, log, gate_tx) = spawn_fake();
        let far = Duration::from_secs(10);
        let now = Instant::now();
        presenter.publish(blendable(1, now, far, false));
        until("frame 1 rendered", || {
            log.lock().unwrap().renders.len() == 1
        });
        presenter.publish(blendable(2, now, far, false));
        for _ in 0..4 {
            gate_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        until("blended renders", || log.lock().unwrap().renders.len() >= 4);
        let log = log.lock().unwrap();
        let xs = &log.xs[1..];
        assert!(log.renders[1..].iter().all(|&f| f == 2));
        assert!(xs.iter().all(|&x| (1.0..2.0).contains(&x)), "{xs:?}");
        // Back-to-back renders can land within f32 resolution of alpha, so only require monotone.
        assert!(xs.windows(2).all(|w| w[0] <= w[1]), "rising: {xs:?}");
        assert!(xs.last() > xs.first(), "no progress: {xs:?}");
    }

    #[test]
    fn a_rebased_tick_and_a_gap_snap_to_the_newer_pose() {
        for (second, rebased) in [(2, true), (3, false)] {
            let (presenter, log, gate_tx) = spawn_fake();
            let far = Duration::from_secs(10);
            let now = Instant::now();
            presenter.publish(blendable(1, now, far, false));
            until("frame 1 rendered", || {
                log.lock().unwrap().renders.len() == 1
            });
            presenter.publish(blendable(second, now, far, rebased));
            for _ in 0..3 {
                gate_tx.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(15));
            }
            until("presents", || log.lock().unwrap().presents >= 3);
            let log = log.lock().unwrap();
            assert_eq!(log.xs, [1.0, second as f32], "one snapped render");
        }
    }

    #[test]
    fn a_settled_blend_stops_re_rendering() {
        let (presenter, log, gate_tx) = spawn_fake();
        let now = Instant::now();
        let tick = Duration::from_millis(1);
        presenter.publish(blendable(1, now, tick, false));
        until("frame 1 rendered", || {
            log.lock().unwrap().renders.len() == 1
        });
        presenter.publish(blendable(2, now, tick, false));
        std::thread::sleep(Duration::from_millis(30));
        for _ in 0..3 {
            gate_tx.send(()).unwrap();
        }
        until("presents", || log.lock().unwrap().presents >= 3);
        let settled = log.lock().unwrap().renders.len();
        for _ in 0..5 {
            gate_tx.send(()).unwrap();
        }
        until("more presents", || log.lock().unwrap().presents >= 8);
        let log = log.lock().unwrap();
        assert_eq!(log.renders.len(), settled, "no re-render once alpha is 1");
        assert_eq!(*log.xs.last().unwrap(), 2.0);
    }

    #[test]
    fn alpha_tracks_the_tick_and_clamps() {
        let at = Instant::now();
        let period = Duration::from_millis(100);
        let stamp = |period| {
            Some(TickStamp {
                at,
                period,
                rebased: false,
            })
        };
        assert_eq!(alpha_at(at, None), 1.0);
        assert_eq!(alpha_at(at + period, stamp(Duration::ZERO)), 1.0);
        assert_eq!(alpha_at(at, stamp(period)), 0.0);
        assert_eq!(alpha_at(at + period / 2, stamp(period)), 0.5);
        assert_eq!(alpha_at(at + period * 3, stamp(period)), 1.0);
        let later = at + period;
        assert_eq!(
            alpha_at(
                at,
                Some(TickStamp {
                    at: later,
                    period,
                    rebased: false
                })
            ),
            1.0
        );
    }

    fn until(what: &str, mut done: impl FnMut() -> bool) {
        for _ in 0..500 {
            if done() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for {what}");
    }

    #[test]
    fn presenter_types_are_send() {
        fn assert_send<T: Send>() {}
        assert_send::<PresentPacket>();
        assert_send::<PresenterNote>();
        assert_send::<Presenter>();
    }

    /// Packets 1–3 published while the loop is blocked in `present` collapse to the newest, and the
    /// later presents repeat it without rendering again.
    #[test]
    fn the_newest_packet_wins_and_a_repeat_present_does_not_re_render() {
        let log = Arc::new(Mutex::new(Log::default()));
        let (gate_tx, gate) = mpsc::channel();
        let presenter = Presenter::spawn(Box::new(Fake {
            log: Arc::clone(&log),
            gate,
        }));
        presenter.publish(packet(0));
        until("frame 0 rendered", || log.lock().unwrap().renders == [0]);
        for frame in 1..=3 {
            presenter.publish(packet(frame));
        }
        gate_tx.send(()).unwrap();
        until("frame 3 rendered", || log.lock().unwrap().renders == [0, 3]);
        for _ in 0..3 {
            gate_tx.send(()).unwrap();
        }
        until("repeat presents", || log.lock().unwrap().presents >= 4);
        assert_eq!(
            log.lock().unwrap().renders,
            [0, 3],
            "1 and 2 were superseded"
        );
        drop(gate_tx);
        drop(presenter);
    }

    #[test]
    fn a_fatal_render_error_is_a_fault_note_and_presenting_stops() {
        struct Dying(Arc<Mutex<Log>>);
        impl PresentBackend for Dying {
            fn render(&mut self, _: &PresentPacket) -> Result<(), Diagnostic> {
                Err(Diagnostic::new(code::RENDER_DEVICE_LOST, "lost"))
            }
            fn present(&mut self) -> Result<PresentOutcome, Diagnostic> {
                self.0.lock().unwrap().presents += 1;
                Ok(PresentOutcome::Presented)
            }
            fn probe_resolve(&self) -> Option<ProbeResolve> {
                None
            }
            fn reload_shader(&mut self, _: &str, _: String) -> Result<u64, Diagnostic> {
                Ok(0)
            }
            fn idle(&self) -> bool {
                false
            }
        }
        let log = Arc::new(Mutex::new(Log::default()));
        let presenter = Presenter::spawn(Box::new(Dying(Arc::clone(&log))));
        presenter.publish(packet(0));
        let mut notes = Vec::new();
        until("a fault note", || {
            notes.extend(presenter.take_notes());
            !notes.is_empty()
        });
        assert!(matches!(&notes[0], PresenterNote::Fault(d) if d.code == code::RENDER_DEVICE_LOST));
        assert_eq!(log.lock().unwrap().presents, 0);
    }
}
