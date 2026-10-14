//! The live profiler: what the frames a running engine is *already* producing actually cost.
//!
//! [`profile`](crate::profile) answers the offline question — load a scene, drive N frames, print
//! medians. It cannot answer the live one, because the frames it measures are not the frames a
//! player (or a viewer, or an agent) is watching. [`LivePerf`] is the counterpart: attached to a
//! live engine it assembles three layers over the frames the [`live`](crate::live) loop already
//! runs —
//!
//! 1. **sim**, per-system wall time through the clock-free [`ScheduleObserver`] seam plus the
//!    tick's total;
//! 2. **host**, the `extract` and `submit` spans of the frame the loop streamed (see
//!    [`LIVE_SEQUENTIAL_NAMES`] — the live tick has no `world_hash` or `command_drain` to time);
//! 3. **GPU**, per-pass times drained from a [`PassTimingRing`], which arrive one to three frames
//!    late and are filed against the frame they measured;
//!
//! — into fixed-size [`ProfileWindowData`]s of [`PROFILE_WINDOW_FRAMES`] frames, each carrying both the
//! window's medians and its raw per-frame samples (medians cannot make a timeline). Completed
//! windows queue up for the host to take; a *capture* is the same machinery armed for one window of
//! a caller-chosen length.
//!
//! # Zero cost when disarmed
//!
//! [`Engine::perf_armed`](crate::Engine::perf_armed) is one `Option` test per tick. A disarmed
//! engine allocates nothing, reads no clock, and attaches no observer — the live loop takes exactly
//! the code path it took before this module existed.
//!
//! # The wall-clock site
//!
//! This module is `ludion-engine`'s third sanctioned wall-clock site, after
//! [`profile`](crate::profile)'s driver and [`live`](crate::live)'s tick scheduler, and it holds the
//! crate's live-path clock reads in one place: the private `Span` is the only thing that calls
//! [`Instant::now`], so the engine's armed paths open spans instead of reading the clock. Like the
//! offline profiler the measurement is pure **observation** — a span's result is written into this
//! host-side state and never into the world — so an armed run's
//! [`WorldHash`](crate::Engine::world_hash) sequence and its recorded `.lrp` are bit-identical to a
//! disarmed run's (`live_perf_does_not_change_world_hash` and
//! `profiled_recording_is_byte_identical` below).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ludion_core::time::FrameIndex;
use ludion_ecs::ScheduleObserver;
use ludion_renderer::{PassTimingRing, Renderer};

use crate::profile::{PassTiming, SystemTiming, TimingObserver, median_f64};

/// Frames in one rolling [`ProfileWindowData`] — 60, so a window is one second of medians at the default
/// 60 Hz tick rate and a stream of them reads as ~1 Hz.
pub const PROFILE_WINDOW_FRAMES: usize = 60;

/// The live tick's **sequential** spans, in report order — the per-frame, per-*world* host costs a
/// live loop pays outside the schedule.
///
/// Deliberately shorter than the offline [`SEQUENTIAL_NAMES`](crate::SEQUENTIAL_NAMES): a live loop
/// hashes the world only on request and drains commands only when one arrives, so timing either
/// here would report a cost the measured frames did not pay. What a streaming frame *does* pay is
/// the render-scene extraction and the submit-plus-collect of the streamed frame.
pub const LIVE_SEQUENTIAL_NAMES: [&str; 2] = ["extract", "submit"];

/// How many further frames a full window waits for its late GPU samples before closing without them.
///
/// A window cannot simply wait for completeness. The ring is [`PassTimingRing::DEPTH`] frames deep,
/// so a sample that will *ever* arrive arrives within that many collections — but a frame the ring
/// skipped (a full ring, an oversize graph) or never saw (a tick the host did not render) produces
/// no sample at all, and waiting on it would wedge the window forever. One frame past the ring depth
/// is therefore the entire wait, and a window closes with whatever GPU data reached it.
const GPU_GRACE_FRAMES: usize = PassTimingRing::<FrameIndex>::DEPTH + 1;

// The close rule assumes at most one full-but-waiting rolling window at a time (see
// `LivePerf::builders`), which holds only while the grace is shorter than a window.
const _: () = assert!(GPU_GRACE_FRAMES < PROFILE_WINDOW_FRAMES);

/// Completed rolling windows held for a host that is not taking them. Beyond this the oldest is
/// dropped: the live stream is latest-wins, and an armed engine whose consumer has gone quiet must
/// not grow without bound.
const MAX_QUEUED_WINDOWS: usize = 8;

/// An open wall-clock span. The one place `ludion-engine`'s live path reads the clock.
///
/// Constructed only where a measurement is wanted — `armed.then(Span::start)` reads no clock at all
/// when disarmed, which is what makes the profiler free when it is off.
pub(crate) struct Span(Instant);

impl Span {
    /// Opens a span at the current instant.
    // Sanctioned wall-clock site (see the module docs): a span's result is host-side output, never
    // sim state, so it cannot perturb the `WorldHash`.
    #[allow(clippy::disallowed_methods)]
    pub(crate) fn start() -> Span {
        Span(Instant::now())
    }

    /// Closes the span, returning its length in milliseconds.
    // Sanctioned wall-clock site, as in `start`: host-side output only.
    #[allow(clippy::disallowed_methods)]
    pub(crate) fn ms(self) -> f64 {
        millis(self.0.elapsed())
    }
}

/// One live frame's raw measurements — the row a [`ProfileWindowData`]'s columns are built from.
///
/// Filled in two stages: the sim half at the tick, then the host and GPU halves as the frame's
/// render happens and its timing sample comes back (see [`LivePerf`]).
#[derive(Debug, Clone)]
pub struct FrameSample {
    /// The frame the tick ran as (the index captured *before* the driver advanced it).
    pub frame: FrameIndex,
    /// Per-system wall time in milliseconds, in schedule order (names in
    /// [`ProfileSamples::system_names`], captured once for the window).
    pub systems: Vec<f64>,
    /// The whole tick's wall time in milliseconds — the observed schedule run plus per-frame
    /// overhead, and never any of [`LIVE_SEQUENTIAL_NAMES`].
    pub frame_total_ms: f64,
    /// Render-scene extraction time, or `None` for a tick the host did not render.
    pub extract_ms: Option<f64>,
    /// Frame submit-and-collect time, or `None` for a tick the host did not render.
    pub submit_ms: Option<f64>,
    /// The stamp the streamed render of this tick's world carried — the key a late GPU sample is
    /// filed by. `None` until (or unless) this tick is rendered.
    ///
    /// Held rather than derived: the renderer stamps the frame the *loop* was on when it submitted,
    /// which is one past the frame the tick ran as, and a paused or unsubscribed loop breaks the
    /// relation entirely.
    pub(crate) render_stamp: Option<FrameIndex>,
    /// Per-pass GPU times in scheduled order (`None` per pass on an adapter without
    /// `TIMESTAMP_QUERY`), or `None` for a frame whose sample never arrived.
    pub(crate) passes: Option<Vec<(String, Option<f64>)>>,
}

// The window shapes are wire types: a closed window goes out verbatim as a `profile` subscription
// event, and a capture's window becomes the `profile.capture` reply's report. They therefore live in
// `ludion-protocol` and are used from here directly — one shape, no engine-side twin to drift.
pub use ludion_protocol::perf::{LiveGpuProfile, ProfileSamples, ProfileWindowData};

/// The live profiler's whole state: the observer seam, the GPU timing ring, the windows being
/// filled, and the completed reports waiting to be taken.
///
/// Owned by the [`Engine`](crate::Engine) as an `Option<Box<LivePerf>>` — present means armed. The
/// engine drives it (a tick's sim half, a render's host half, a drain of the ring); a host reads it
/// through [`Engine::perf_take_windows`](crate::Engine::perf_take_windows) and
/// [`Engine::perf_take_capture`](crate::Engine::perf_take_capture).
pub struct LivePerf {
    /// The pipelined per-pass GPU timing ring, created on the first armed render (an engine that
    /// never renders never asks for an adapter).
    ring: Option<PassTimingRing<FrameIndex>>,
    /// The per-system timing seam, reused across ticks.
    observer: TimingObserver,
    /// System names in schedule order, captured from the observed frames — the schedule is stable,
    /// so one copy serves every window.
    system_names: Vec<String>,
    /// Windows currently accepting or awaiting samples: at most one open rolling window, at most one
    /// full-but-waiting rolling window (see [`GPU_GRACE_FRAMES`], which is far below
    /// [`PROFILE_WINDOW_FRAMES`]), and at most one capture.
    builders: Vec<WindowBuilder>,
    /// Completed rolling windows, oldest first.
    windows: VecDeque<ProfileWindowData>,
    /// A completed capture waiting to be taken.
    capture_ready: Option<ProfileWindowData>,
}

impl Default for LivePerf {
    fn default() -> Self {
        LivePerf::new()
    }
}

impl LivePerf {
    /// A freshly armed profiler: no ring yet, no samples, no windows.
    pub fn new() -> LivePerf {
        LivePerf {
            ring: None,
            observer: TimingObserver::default(),
            system_names: Vec::new(),
            builders: Vec::new(),
            windows: VecDeque::new(),
            capture_ready: None,
        }
    }

    /// Whether this run can time GPU passes at all — `false` on an adapter without
    /// `TIMESTAMP_QUERY`, and before the first armed render created the ring.
    pub fn timestamps_supported(&self) -> bool {
        self.ring
            .as_ref()
            .is_some_and(PassTimingRing::timestamps_supported)
    }

    /// Takes every completed rolling window, oldest first.
    pub fn take_windows(&mut self) -> Vec<ProfileWindowData> {
        self.windows.drain(..).collect()
    }

    /// Arms a capture over the next `frames` live frames. Returns `false` — arming nothing — when a
    /// capture is already in flight or its report has not been taken.
    pub fn begin_capture(&mut self, frames: u64) -> bool {
        if self.capture_in_flight() {
            return false;
        }
        let target = usize::try_from(frames).unwrap_or(usize::MAX).max(1);
        self.builders.push(WindowBuilder::new(target, true));
        true
    }

    /// Whether a capture is armed, or has completed and not yet been taken.
    pub fn capture_in_flight(&self) -> bool {
        self.capture_ready.is_some() || self.builders.iter().any(|builder| builder.capture)
    }

    /// Frames the open capture still needs, or `None` when no capture is open. `Some(0)` is a full
    /// window waiting for its closing frame.
    pub fn capture_remaining(&self) -> Option<u64> {
        self.builders
            .iter()
            .find(|builder| builder.capture)
            .map(|builder| builder.target.saturating_sub(builder.samples.len()) as u64)
    }

    /// Drops the open capture and any finished one, freeing the slot for the next.
    pub fn cancel_capture(&mut self) {
        self.builders.retain(|builder| !builder.capture);
        self.capture_ready = None;
    }

    /// Takes the completed capture, if one is waiting.
    pub fn take_capture(&mut self) -> Option<ProfileWindowData> {
        self.capture_ready.take()
    }

    /// The observer to attach to the coming tick, cleared of the last one's spans.
    pub(crate) fn begin_frame(&mut self) -> &mut dyn ScheduleObserver {
        self.observer.frame.clear();
        &mut self.observer
    }

    /// Closes the tick opened by [`begin_frame`](LivePerf::begin_frame), filing its sim half as a
    /// new [`FrameSample`].
    pub(crate) fn end_frame(&mut self, frame: FrameIndex, frame_total_ms: f64) {
        if self.system_names.len() != self.observer.frame.len() {
            self.system_names = self
                .observer
                .frame
                .iter()
                .map(|(name, _)| (*name).to_string())
                .collect();
        }
        let systems = self
            .observer
            .frame
            .iter()
            .map(|(_, elapsed)| millis(*elapsed))
            .collect();
        self.push_sample(FrameSample {
            frame,
            systems,
            frame_total_ms,
            extract_ms: None,
            submit_ms: None,
            render_stamp: None,
            passes: None,
        });
    }

    /// Files a streamed render's host spans against the tick that produced the world it drew — the
    /// newest sample that has not been rendered yet.
    ///
    /// A render with no such sample (the loop is paused, or re-rendered an unchanged world) is
    /// dropped: the live profiler samples ticked frames.
    pub(crate) fn record_render(
        &mut self,
        stamp: FrameIndex,
        extract_ms: Option<f64>,
        submit_ms: Option<f64>,
    ) {
        for builder in &mut self.builders {
            if let Some(sample) = builder.samples.last_mut()
                && sample.render_stamp.is_none()
            {
                sample.render_stamp = Some(stamp);
                sample.extract_ms = extract_ms;
                sample.submit_ms = submit_ms;
            }
        }
    }

    /// The GPU timing ring to arm the coming render with, created on first use.
    pub(crate) fn timing_ring(&mut self, renderer: &Renderer) -> &mut PassTimingRing<FrameIndex> {
        self.ring
            .get_or_insert_with(|| renderer.new_pass_timing_ring())
    }

    /// Drains every GPU timing sample the ring has finished, filing each against the frame it
    /// measured (never the frame that collected it).
    pub(crate) fn collect_gpu(&mut self, renderer: &Renderer) {
        let mut collected = Vec::new();
        if let Some(ring) = self.ring.as_mut() {
            while let Some(sample) = renderer.collect_pass_timings(ring) {
                collected.push(sample);
            }
        }
        for (stamp, passes) in collected {
            self.file_gpu(
                stamp,
                passes
                    .into_iter()
                    .map(|pass| (pass.name, pass.gpu_time_ms))
                    .collect(),
            );
        }
    }

    /// Appends one sample to every window accepting them, ages the full ones, and starts a fresh
    /// rolling window when the previous one filled.
    fn push_sample(&mut self, sample: FrameSample) {
        let mut rolling_open = false;
        for builder in &mut self.builders {
            if builder.full() {
                builder.waited += 1;
            } else {
                builder.samples.push(sample.clone());
                rolling_open |= !builder.capture;
            }
        }
        if !rolling_open {
            let mut builder = WindowBuilder::new(PROFILE_WINDOW_FRAMES, false);
            builder.samples.push(sample);
            self.builders.push(builder);
        }
        self.harvest();
    }

    /// Files one late GPU sample against the frame whose render carried `stamp`. A stamp matching no
    /// open window (its window already closed, or the render was never sampled) is dropped.
    fn file_gpu(&mut self, stamp: FrameIndex, passes: Vec<(String, Option<f64>)>) {
        for builder in &mut self.builders {
            if let Some(sample) = builder
                .samples
                .iter_mut()
                .rev()
                .find(|sample| sample.render_stamp == Some(stamp))
            {
                sample.passes = Some(passes.clone());
            }
        }
        self.harvest();
    }

    /// Closes every window whose close rule is satisfied, in fill order.
    fn harvest(&mut self) {
        let mut index = 0;
        while index < self.builders.len() {
            if !self.builders[index].ready() {
                index += 1;
                continue;
            }
            let builder = self.builders.remove(index);
            let capture = builder.capture;
            let window = builder.close(&self.system_names, self.timestamps_supported());
            if capture {
                self.capture_ready = Some(window);
            } else {
                if self.windows.len() >= MAX_QUEUED_WINDOWS {
                    self.windows.pop_front();
                }
                self.windows.push_back(window);
            }
        }
    }
}

/// One window under construction.
struct WindowBuilder {
    /// Frames this window closes at.
    target: usize,
    /// Whether this window answers a capture request rather than the rolling stream.
    capture: bool,
    samples: Vec<FrameSample>,
    /// Frames observed since this window filled — the age-out clock (see [`GPU_GRACE_FRAMES`]).
    waited: usize,
}

impl WindowBuilder {
    fn new(target: usize, capture: bool) -> WindowBuilder {
        WindowBuilder {
            target,
            capture,
            samples: Vec::with_capacity(target.min(PROFILE_WINDOW_FRAMES)),
            waited: 0,
        }
    }

    fn full(&self) -> bool {
        self.samples.len() >= self.target
    }

    /// Whether the window may close: it is full, at least one further frame has passed (so its last
    /// sample has had its render), and either every rendered frame's GPU sample arrived or the
    /// grace period expired.
    fn ready(&self) -> bool {
        self.full() && self.waited >= 1 && (self.waited >= GPU_GRACE_FRAMES || self.gpu_complete())
    }

    /// Whether every frame that *could* carry a GPU sample does.
    fn gpu_complete(&self) -> bool {
        self.samples
            .iter()
            .all(|sample| sample.render_stamp.is_none() || sample.passes.is_some())
    }

    /// Reduces the window's samples to medians plus their columnar form.
    fn close(self, system_names: &[String], timestamps_supported: bool) -> ProfileWindowData {
        let frames = self.samples.len();
        let pass_names: Vec<String> = self
            .samples
            .iter()
            .find_map(|sample| sample.passes.as_ref())
            .map(|passes| passes.iter().map(|(name, _)| name.clone()).collect())
            .unwrap_or_default();

        let mut system_ms: Vec<Vec<f64>> = vec![Vec::with_capacity(frames); system_names.len()];
        let mut pass_ms: Vec<Vec<Option<f64>>> = vec![Vec::with_capacity(frames); pass_names.len()];
        let mut frame_indices = Vec::with_capacity(frames);
        let mut frame_total_ms = Vec::with_capacity(frames);
        let mut extract_ms = Vec::with_capacity(frames);
        let mut submit_ms = Vec::with_capacity(frames);
        let mut sampled_frames = 0u64;

        for sample in &self.samples {
            frame_indices.push(sample.frame.as_u64());
            frame_total_ms.push(sample.frame_total_ms);
            extract_ms.push(sample.extract_ms);
            submit_ms.push(sample.submit_ms);
            for (column, value) in system_ms.iter_mut().zip(&sample.systems) {
                column.push(*value);
            }
            // Defensive, like the offline driver's: a stable schedule and a stable graph report the
            // same names every frame, but a shorter row must not shift the columns.
            for (index, column) in pass_ms.iter_mut().enumerate() {
                column.push(
                    sample
                        .passes
                        .as_ref()
                        .and_then(|passes| passes.get(index))
                        .and_then(|(_, ms)| *ms),
                );
            }
            if sample.passes.is_some() {
                sampled_frames += 1;
            }
        }

        let systems = system_names
            .iter()
            .zip(&system_ms)
            .map(|(name, column)| SystemTiming {
                name: name.clone(),
                median_ms: median_f64(&mut column.clone()),
                tail: None,
            })
            .collect();
        let sequential = LIVE_SEQUENTIAL_NAMES
            .iter()
            .zip([&extract_ms, &submit_ms])
            .map(|(name, column)| SystemTiming {
                name: (*name).to_string(),
                median_ms: median_present(column).unwrap_or(0.0),
                tail: None,
            })
            .collect();
        let passes = pass_names
            .iter()
            .zip(&pass_ms)
            .map(|(name, column)| PassTiming {
                name: name.clone(),
                median_ms: median_present(column),
            })
            .collect();

        ProfileWindowData {
            first_frame: frame_indices.first().copied().unwrap_or_default(),
            last_frame: frame_indices.last().copied().unwrap_or_default(),
            frames: frames as u64,
            capture: self.capture,
            systems,
            sequential,
            frame_total_median_ms: median_f64(&mut frame_total_ms.clone()),
            threads: ludion_core::par::threads(),
            // Filled by the server before the event goes out; the engine cannot see delivery.
            transport: None,
            gpu: LiveGpuProfile {
                timestamps_supported,
                sampled_frames,
                passes,
            },
            samples: ProfileSamples {
                frames: frame_indices,
                system_names: system_names.to_vec(),
                system_ms,
                frame_total_ms,
                extract_ms,
                submit_ms,
                pass_names,
                pass_ms,
            },
        }
    }
}

/// A duration in milliseconds.
fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

/// Median of the measured entries in a sparse column, or `None` when none was measured — the
/// difference between "zero milliseconds" and "never sampled".
fn median_present(column: &[Option<f64>]) -> Option<f64> {
    let mut values: Vec<f64> = column.iter().filter_map(|value| *value).collect();
    (!values.is_empty()).then(|| median_f64(&mut values))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;
    use ludion_ecs::World;
    use ludion_sim::WorldHash;
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
    struct Counter {
        n: i64,
    }

    /// A system that both mutates a component and advances the RNG, so the `WorldHash` genuinely
    /// depends on how many ticks ran — the strongest form of the determinism check.
    fn bump(world: &mut World, _ctx: &ludion_ecs::SystemCtx) {
        let step = world
            .resource_mut::<ludion_core::rng::DeterministicRng>()
            .unwrap()
            .next_u32() as i64;
        for (counter,) in world.query::<(&mut Counter,)>() {
            counter.n += 1 + step;
        }
    }

    fn counter_engine() -> crate::EngineBuilder {
        Engine::builder()
            .components(|reg| reg.register::<Counter>("Counter"))
            .system("bump", bump)
    }

    const SCENE: &str =
        "format: 1\nseed: 7\nentities:\n  - components:\n      Counter:\n        n: 0\n";

    fn loaded(builder: crate::EngineBuilder) -> Engine {
        let mut engine = builder.build().unwrap();
        engine.load_inline_scene(SCENE).unwrap();
        engine
    }

    /// A sample with `count` systems, no render and no GPU data.
    fn sample(frame: u64) -> FrameSample {
        FrameSample {
            frame: FrameIndex::new(frame),
            systems: vec![0.5],
            frame_total_ms: 1.0,
            extract_ms: None,
            submit_ms: None,
            render_stamp: None,
            passes: None,
        }
    }

    /// Drives `perf` through `frames` sim ticks starting at `from`, rendering each one (so every
    /// sample carries host spans and a render stamp) but with no GPU behind it. Returns the frame
    /// the next call should start at.
    fn drive(perf: &mut LivePerf, from: u64, frames: u64) -> u64 {
        for frame in from..from + frames {
            perf.push_sample(sample(frame));
            perf.record_render(FrameIndex::new(frame + 1), Some(0.2), Some(0.3));
        }
        from + frames
    }

    #[test]
    fn live_perf_does_not_change_world_hash() {
        // The load-bearing determinism proof, lifted from the offline profiler's
        // `observer_does_not_change_world_hash` to the *live* path: an armed engine and a disarmed
        // one, driven the same number of live ticks, must agree bit for bit.
        let mut plain = loaded(counter_engine());
        let mut armed = loaded(counter_engine());
        armed.perf_arm();

        // One tick past a full window, so the run also proves the armed path actually produced one.
        let mut plain_hashes: Vec<WorldHash> = Vec::new();
        let mut armed_hashes: Vec<WorldHash> = Vec::new();
        for _ in 0..=PROFILE_WINDOW_FRAMES {
            plain.advance_live();
            armed.advance_live();
            plain_hashes.push(plain.world_hash().unwrap());
            armed_hashes.push(armed.world_hash().unwrap());
        }

        assert_eq!(
            plain_hashes, armed_hashes,
            "arming the live profiler must not perturb the WorldHash sequence"
        );
        assert_eq!(plain.frame(), armed.frame(), "no tick was added or lost");
        // And the seam actually saw the schedule (the test would pass trivially if it had not).
        let windows = armed.perf_take_windows();
        assert!(!windows.is_empty(), "50 ticks close at least one window");
        assert_eq!(windows[0].systems.len(), 1);
        assert_eq!(windows[0].systems[0].name, "bump");
    }

    #[test]
    fn profiled_recording_is_byte_identical() {
        // Nothing timing-related may reach the `.lrp`: two recordings of the same live session, one
        // armed, must be byte-for-byte identical.
        let record = |armed: bool, path: &std::path::Path| {
            let mut engine = loaded(counter_engine());
            if armed {
                engine.perf_arm();
            }
            engine
                .set_recording(std::fs::File::create(path).unwrap())
                .unwrap();
            for _ in 0..50 {
                engine.advance_live();
            }
            engine.stop_recording().unwrap();
            std::fs::read(path).unwrap()
        };

        let dir = std::env::temp_dir();
        let plain_path = dir.join(format!("ludion-perf-plain-{}.lrp", std::process::id()));
        let armed_path = dir.join(format!("ludion-perf-armed-{}.lrp", std::process::id()));
        let plain = record(false, &plain_path);
        let armed = record(true, &armed_path);
        let _ = std::fs::remove_file(&plain_path);
        let _ = std::fs::remove_file(&armed_path);

        assert!(!plain.is_empty(), "the recording wrote something");
        assert_eq!(
            plain, armed,
            "a profiled live recording must be byte-identical to an unprofiled one"
        );
    }

    #[test]
    fn a_disarmed_engine_never_profiles() {
        let mut engine = loaded(counter_engine());
        assert!(!engine.perf_armed());
        for _ in 0..(PROFILE_WINDOW_FRAMES as u64 * 2) {
            engine.advance_live();
        }
        assert!(engine.perf_take_windows().is_empty());
        assert!(engine.perf_take_capture().is_none());
        assert!(!engine.perf_capture_in_flight());

        // Arming mid-run starts sampling from the next tick, and disarming drops the state.
        engine.perf_arm();
        assert!(engine.perf_armed());
        engine.perf_disarm();
        assert!(!engine.perf_armed());
        for _ in 0..(PROFILE_WINDOW_FRAMES as u64 * 2) {
            engine.advance_live();
        }
        assert!(engine.perf_take_windows().is_empty());
    }

    #[test]
    fn a_window_closes_over_exactly_its_own_frames() {
        let mut perf = LivePerf::new();
        perf.system_names = vec!["bump".to_string()];
        let next_frame = drive(&mut perf, 0, PROFILE_WINDOW_FRAMES as u64);
        assert!(
            perf.take_windows().is_empty(),
            "a window that has only just filled still awaits its frames' GPU samples"
        );

        // Every frame was rendered and no GPU sample ever arrives, so the window closes on the
        // grace period rather than on completeness.
        let next_frame = drive(&mut perf, next_frame, GPU_GRACE_FRAMES as u64);
        let windows = perf.take_windows();
        assert_eq!(windows.len(), 1);
        let window = &windows[0];
        assert_eq!(window.frames, PROFILE_WINDOW_FRAMES as u64);
        assert!(!window.capture);
        assert_eq!(window.first_frame, 0);
        assert_eq!(window.last_frame, PROFILE_WINDOW_FRAMES as u64 - 1);
        assert_eq!(window.samples.frames.len(), PROFILE_WINDOW_FRAMES);
        assert_eq!(window.samples.system_names, ["bump"]);
        assert_eq!(window.samples.system_ms[0].len(), PROFILE_WINDOW_FRAMES);
        assert_eq!(window.systems[0].median_ms, 0.5);
        assert_eq!(window.frame_total_median_ms, 1.0);
        // The two live sequential spans, in their fixed order, over the rendered frames.
        let names: Vec<&str> = window.sequential.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, LIVE_SEQUENTIAL_NAMES);
        assert_eq!(window.sequential[0].median_ms, 0.2);
        assert_eq!(window.sequential[1].median_ms, 0.3);

        // The next window is a fresh 60 frames, not a rolling re-cut of the first.
        drive(&mut perf, next_frame, PROFILE_WINDOW_FRAMES as u64);
        let next = perf.take_windows();
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].first_frame, PROFILE_WINDOW_FRAMES as u64);
        assert_eq!(next[0].frames, PROFILE_WINDOW_FRAMES as u64);
    }

    #[test]
    fn gpu_samples_are_filed_by_their_own_stamp() {
        let mut perf = LivePerf::new();
        perf.system_names = vec!["bump".to_string()];
        // Three frames ticked and rendered, then their timings arrive out of the frames that
        // collected them — frame 0's render carried stamp 1, and so on.
        drive(&mut perf, 0, 3);
        perf.file_gpu(FrameIndex::new(2), vec![("scene".to_string(), Some(4.0))]);
        perf.file_gpu(FrameIndex::new(1), vec![("scene".to_string(), Some(2.0))]);

        let builder = &perf.builders[0];
        assert_eq!(builder.samples[0].frame.as_u64(), 0);
        assert_eq!(
            builder.samples[0].passes.as_deref(),
            Some(&[("scene".to_string(), Some(2.0))][..]),
            "the sample stamped 1 measured the frame that ticked as 0"
        );
        assert_eq!(
            builder.samples[1].passes.as_deref(),
            Some(&[("scene".to_string(), Some(4.0))][..])
        );
        assert!(builder.samples[2].passes.is_none());

        // A stamp no open sample claims is dropped rather than mis-filed.
        perf.file_gpu(FrameIndex::new(900), vec![("scene".to_string(), Some(9.0))]);
        assert!(
            perf.builders[0]
                .samples
                .iter()
                .all(|s| s.passes.as_deref() != Some(&[("scene".to_string(), Some(9.0))][..]))
        );
    }

    #[test]
    fn a_missing_gpu_sample_does_not_wedge_the_window() {
        let mut perf = LivePerf::new();
        perf.system_names = vec!["bump".to_string()];
        let mut next_frame = drive(&mut perf, 0, PROFILE_WINDOW_FRAMES as u64);
        // Every frame was rendered but only one timing sample ever comes back — the ring skipped
        // the rest. The window must age out and close anyway.
        perf.file_gpu(FrameIndex::new(1), vec![("scene".to_string(), Some(3.0))]);
        for _ in 0..GPU_GRACE_FRAMES {
            assert!(perf.take_windows().is_empty());
            next_frame = drive(&mut perf, next_frame, 1);
        }

        let windows = perf.take_windows();
        assert_eq!(windows.len(), 1, "the window closed without its GPU data");
        let window = &windows[0];
        assert_eq!(window.gpu.sampled_frames, 1);
        assert_eq!(window.gpu.passes.len(), 1);
        assert_eq!(window.gpu.passes[0].median_ms, Some(3.0));
        assert_eq!(window.samples.pass_ms[0].len(), PROFILE_WINDOW_FRAMES);
        assert_eq!(window.samples.pass_ms[0][0], Some(3.0));
        assert_eq!(window.samples.pass_ms[0][1], None);
    }

    #[test]
    fn a_complete_window_closes_without_waiting_out_the_grace() {
        let mut perf = LivePerf::new();
        perf.system_names = vec!["bump".to_string()];
        let next_frame = drive(&mut perf, 0, PROFILE_WINDOW_FRAMES as u64);
        for frame in 0..PROFILE_WINDOW_FRAMES as u64 {
            perf.file_gpu(
                FrameIndex::new(frame + 1),
                vec![("scene".to_string(), Some(1.0))],
            );
        }
        drive(&mut perf, next_frame, 1);
        let windows = perf.take_windows();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].gpu.sampled_frames, PROFILE_WINDOW_FRAMES as u64);
    }

    #[test]
    fn a_capture_answers_its_own_frame_count() {
        let mut perf = LivePerf::new();
        perf.system_names = vec!["bump".to_string()];
        assert!(!perf.capture_in_flight());
        assert!(perf.begin_capture(10));
        assert!(perf.capture_in_flight());
        assert!(
            !perf.begin_capture(10),
            "a second capture is refused while one is in flight"
        );

        drive(&mut perf, 0, 10 + GPU_GRACE_FRAMES as u64);
        let capture = perf.take_capture().expect("the capture completed");
        assert!(capture.capture);
        assert_eq!(capture.frames, 10);
        assert_eq!(capture.samples.frames.len(), 10);
        assert!(
            !perf.capture_in_flight(),
            "the taken capture frees the slot"
        );
        // The rolling stream kept filling underneath it.
        assert!(perf.take_windows().is_empty());
        assert_eq!(perf.builders.len(), 1);
    }

    #[test]
    fn an_undrained_stream_stops_growing() {
        let mut perf = LivePerf::new();
        perf.system_names = vec!["bump".to_string()];
        drive(
            &mut perf,
            0,
            (PROFILE_WINDOW_FRAMES * (MAX_QUEUED_WINDOWS + 4)) as u64,
        );
        assert_eq!(perf.windows.len(), MAX_QUEUED_WINDOWS);
        assert!(
            perf.windows.back().unwrap().first_frame > perf.windows.front().unwrap().first_frame,
            "the queue drops the oldest window, not the newest"
        );
    }

    #[test]
    fn window_shape_is_stable() {
        // The window serializes to a fixed field structure (numbers are not asserted — they are
        // wall-clock measurements). Constructed through the builder so the shape is the real one.
        let mut perf = LivePerf::new();
        perf.system_names = vec!["bump".to_string()];
        drive(&mut perf, 0, 4);
        perf.file_gpu(FrameIndex::new(1), vec![("scene".to_string(), Some(3.0))]);
        let mut builder = perf.builders.remove(0);
        builder.target = 4;
        let window = builder.close(&perf.system_names, true);

        let value = serde_json::to_value(&window).unwrap();
        for key in [
            "first_frame",
            "last_frame",
            "frames",
            "capture",
            "systems",
            "sequential",
            "frame_total_median_ms",
            "gpu",
            "samples",
        ] {
            assert!(value.get(key).is_some(), "missing top-level key `{key}`");
        }
        assert_eq!(value["systems"][0]["name"], "bump");
        assert!(value["systems"][0]["median_ms"].is_number());
        for (index, name) in LIVE_SEQUENTIAL_NAMES.iter().enumerate() {
            assert_eq!(value["sequential"][index]["name"], *name);
            assert!(value["sequential"][index]["median_ms"].is_number());
        }
        assert_eq!(value["gpu"]["timestamps_supported"], true);
        assert!(value["gpu"]["sampled_frames"].is_number());
        assert_eq!(value["gpu"]["passes"][0]["name"], "scene");
        for key in [
            "frames",
            "system_names",
            "system_ms",
            "frame_total_ms",
            "extract_ms",
            "submit_ms",
            "pass_names",
            "pass_ms",
        ] {
            assert!(
                value["samples"].get(key).is_some(),
                "missing samples key `{key}`"
            );
        }

        // And it reads back into the same shape it wrote (a window is an artifact tooling parses).
        let text = serde_json::to_string(&window).unwrap();
        let parsed: ProfileWindowData = serde_json::from_str(&text).unwrap();
        assert_eq!(serde_json::to_string(&parsed).unwrap(), text);
    }
}
