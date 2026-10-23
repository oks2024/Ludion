//! The profile driver: sim per-system timings, GPU per-pass timings, and frame totals, as one report.
//!
//! [`profile_scene`] loads a scene, drives it for a warmup then N measured frames, and combines
//! three measurements into a [`ProfileReport`]: the sim schedule's per-system wall time (through the
//! clock-free [`ScheduleObserver`] seam), the render pipeline's per-pass
//! GPU time (the existing `dump-timings` timestamp path), and the whole-frame total. Every number is
//! a **median** over the measured frames — the noise-resistant statistic for wall-clock timing on a
//! busy machine. It powers the `ludion profile` command: this is the *offline* driver, which loads a
//! scene file and needs no running host. Its live counterpart is
//! [`perf_live`](crate::perf_live) — same report shape ([`ludion_protocol::perf`], re-exported here),
//! measured over the frames a running engine is already producing.
//!
//! # The one wall-clock site
//!
//! This module is the *only* place in `ludion-engine` that reads the wall clock. Like
//! [`live`](crate::live)'s tick scheduler, the measurement is pure **observation**: the
//! [`ScheduleObserver`] it attaches cannot mutate sim state, so a
//! profiled run's [`WorldHash`](Engine::world_hash) is bit-identical to an unprofiled one (asserted
//! by the `observer_does_not_change_world_hash` test below). The crate's clippy denylist bans
//! [`std::time::Instant::now`] everywhere; the functions here that call it — and `LoadClock` and
//! `SpanClock`, the implementors of the clock-free observers — scope an `allow` with that rationale,
//! exactly as `live.rs` does for the tick clock.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, Instant};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::suggest;
use ludion_ecs::ScheduleObserver;
use ludion_protocol::Command;
use ludion_protocol::command::SimStepParams;
use ludion_protocol::perf::{AssetLoadCounters, LoadProfile, ProfileSamples};
use ludion_sim::RunState;
use serde::Deserialize;

use crate::commands::collapse;
use crate::load_observer::{LoadKind, LoadObserver, LoadPhase};
use crate::replay::{
    DriveHook, PlainDrive, drive_actions_with, prepare_replay, require_monotonic_frames,
    resolve_window, total_frames,
};
use crate::{Engine, EngineBuilder};

/// The resolution the profile renders at when collecting GPU pass timings — the engine's ≥1080p
/// target, so the per-pass numbers reflect a real frame, not a toy one.
const PROFILE_SIZE: (u32, u32) = (1920, 1080);

// The report shapes are wire types: `profile.capture` answers with the very same structures this
// offline driver writes, so they live in `ludion-protocol` and are re-exported here (and from the
// crate root) — one shape, two producers, no parallel definition to drift.
pub use ludion_protocol::perf::{
    ClusterProfile, GpuProfile, PassTiming, ProfileReport, QualityProfile, SimProfile,
    SystemTiming, TimingTail,
};

/// Profiles the scene at `path`: warmup, then `frames` measured frames, returning the median report.
///
/// Loads the scene through the built-in registry (the CLI construction path), drives the sim with a
/// timing [`ScheduleObserver`], then collects GPU per-pass timings by rendering the extracted scene
/// `frames` more times. `scale` is the caller's explicit render scale, which beats the one `quality`
/// carries; `None` — the ordinary case — measures at whatever quality asks for, which is native at
/// every preset but `low`. Either way a budget calibrated at one extent does not transfer to
/// another, so the pixel count is part of what the report describes. A missing GPU is not an error —
/// the sim section is still measured and the GPU section reports `available: false`.
///
/// `quality` is this machine's render quality ([`resolve_startup_quality`](crate::resolve_startup_quality)):
/// it decides which optional effects the measured frames render, how hard each works, and — absent a
/// `scale` — how many pixels they cover. It is reported back in [`ProfileReport::quality`] so a
/// number is never read as if it had been paid at another setting.
///
/// # Errors
/// A `Vec<Diagnostic>`: `L1015`/`L1001`–`L1014` for a scene that cannot be read or validated,
/// `L1101`/`L2101`… if the world cannot be extracted, `L2004`/`L2005` for a GPU device/readback
/// failure (a *missing* adapter, `L2003`, is handled gracefully, not returned).
///
/// # Example
/// ```no_run
/// use std::path::Path;
/// use ludion_engine::profile_scene;
///
/// // 5 warmup frames (discarded), then 60 measured, at the quality's own extent. On an
/// // adapterless host the GPU section reports `available: false`; the sim section is always
/// // measured.
/// let quality = ludion_engine::QualitySetting::default();
/// let report = profile_scene(Path::new("scenes/main.yaml"), 60, 5, None, quality)?;
/// println!("frame total: {} ms", report.sim.frame_total_median_ms);
/// # Ok::<(), Vec<ludion_core::diagnostics::Diagnostic>>(())
/// ```
pub fn profile_scene(
    path: &Path,
    frames: u64,
    warmup: u64,
    scale: Option<ludion_renderer::RenderScale>,
    quality: crate::QualitySetting,
) -> Result<ProfileReport, Vec<Diagnostic>> {
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    let mut engine = Engine::with_project_root(root);
    if let Some(scale) = scale {
        engine.set_render_scale(scale);
    }
    engine.set_quality(quality);
    let source = std::fs::read_to_string(path).map_err(|error| {
        vec![
            Diagnostic::new(
                code::SCENE_IO,
                format!("could not read `{}`: {error}", path.display()),
            )
            .with_help("check the path (relative to the current directory) and permissions"),
        ]
    })?;
    engine.observe_load();
    engine.load_inline_scene(&source)?;

    profile_engine(&mut engine, path.display().to_string(), frames, warmup)
}

/// Profiles an already-constructed, scene-loaded [`Engine`]: warmup, then `frames` measured frames,
/// returning the median report labelled `scene`.
///
/// This is the driver shared by both hosts. [`profile_scene`] builds a built-in-registry engine and
/// calls it; `ludion::run`'s in-process `--profile` mode builds the *game's* engine
/// ([`EngineBuilder`] — its components, systems, and default scene) and calls it
/// with the same loop, so a game's own systems appear in [`SimProfile::systems`] instead of only the
/// engine-registered ones. The [`ScheduleObserver`] seam and median statistics stay single-sourced here.
///
/// # Errors
/// A `Vec<Diagnostic>`: `L1101`/`L2101`… if the world cannot be extracted, `L2004`/`L2005` for a GPU
/// device/readback failure (a *missing* adapter, `L2003`, is handled gracefully — the sim section is
/// still measured and the GPU section reports `available: false`).
///
/// # Example
/// ```no_run
/// use ludion_engine::{profile_engine, Engine};
///
/// let mut engine = Engine::new();
/// engine.load_inline_scene("format: 1\nentities: []\n").unwrap();
/// let report = profile_engine(&mut engine, "inline".to_string(), 60, 5)?;
/// println!("frame total: {} ms", report.sim.frame_total_median_ms);
/// # Ok::<(), Vec<ludion_core::diagnostics::Diagnostic>>(())
/// ```
pub fn profile_engine(
    engine: &mut Engine,
    scene: String,
    frames: u64,
    warmup: u64,
) -> Result<ProfileReport, Vec<Diagnostic>> {
    if frames == 0 {
        return Err(vec![
            Diagnostic::new(
                code::REPLAY_FRAME_WINDOW,
                "profile measured no frame: `frames` is 0, so every median would be an empty column",
            )
            .with_help("measure at least one frame (`--frames 1` or more)"),
        ]);
    }
    let load = measure_load(engine)?;
    let (sim, samples) = profile_sim(engine, frames, warmup);
    let cluster = profile_cluster(engine, frames, warmup).map_err(|d| vec![d])?;
    let gpu = profile_gpu(engine, frames, warmup).map_err(|d| vec![d])?;

    Ok(ProfileReport {
        scene,
        warmup_frames: warmup,
        measured_frames: frames,
        // Which effects the GPU numbers above paid for, from the host that owns the setting. Two
        // reports taken at different qualities are not comparable, and only the report can say so.
        quality: Some(engine.quality().descriptor()),
        sim,
        gpu,
        // The offline driver delivers no frames, so it has no transport to measure — that section
        // stays absent. Its raw per-frame samples, unlike transport, are entirely its own to take:
        // `profile_sim` below fills them, so a `--perfetto` export needs no `--attach`.
        transport: None,
        cluster: Some(cluster),
        load,
        probes: engine.probe_profile(),
        samples: Some(samples),
    })
}

/// Warmup frames as a fraction of the measured count, clamped so a tiny `frames` still warms once and
/// a large one never pays more than a modest fixed settling cost. Shared by every host that drives the
/// profile loop, so warmup is calibrated identically whether the run comes from the CLI or a game's own
/// `--profile`.
pub fn warmup_for(frames: u64) -> u64 {
    (frames / 4).clamp(1, 30)
}

/// Closes the load measurement: one rendered first frame (the first decode of every sprite and mesh
/// asset happens in its extract; the renderer's creation, uploads and GI bake follow), then the
/// observer is detached and reduced to a [`LoadProfile`]. `None` when no observer was attached — an
/// unobserved host reads no clock for loading. A host with no GPU adapter (`L2003`) still gets its
/// sim-side load: the render phases it never reached report absent.
///
/// The frame is a pure world read, so it is hash-safe; it also means the warmup that follows starts
/// warm.
fn measure_load(engine: &mut Engine) -> Result<Option<LoadProfile>, Vec<Diagnostic>> {
    if !engine.load_observed() {
        return Ok(None);
    }
    let (width, height) = PROFILE_SIZE;
    match engine.render_image(width, height, false) {
        Ok(_) => {}
        Err(diagnostics) if matches!(diagnostics.as_slice(), [only] if only.code == code::RENDER_NO_ADAPTER) =>
            {}
        Err(diagnostics) => return Err(diagnostics),
    }
    Ok(finish_load(engine))
}

/// Detaches the engine's load observer and reduces it to a [`LoadProfile`]; `None` when no
/// [`LoadClock`] was attached.
pub(crate) fn finish_load(engine: &mut Engine) -> Option<LoadProfile> {
    let observer = engine.take_load_observer()?;
    let observer: Box<dyn std::any::Any> = observer;
    observer
        .downcast::<LoadClock>()
        .ok()
        .map(|clock| clock.finish())
}

/// The wall-clock [`LoadObserver`]: attributes **exclusive** time to each phase (a nested phase's
/// span is subtracted from its parent's) and counts decodes and reuse per asset kind.
#[derive(Default)]
pub(crate) struct LoadClock {
    origin: Option<Instant>,
    stack: Vec<(LoadPhase, Instant, Duration)>,
    last_end: Option<Instant>,
    scene: Duration,
    mesh: Duration,
    texture: Duration,
    other: Duration,
    // The render phases are `Some` once they have begun, so a run that never reached one (no
    // adapter, no `GiVolume`) reports it absent rather than zero.
    first_frame: Option<Duration>,
    voxelize: Option<Duration>,
    renderer_init: Option<Duration>,
    gpu_upload: Option<Duration>,
    gi_bake: Option<Duration>,
    probe_bake: Option<Duration>,
    decoded: [u64; 8],
    cooked: [u64; 8],
    reused: [u64; 8],
    bytes: [u64; 8],
    seen: BTreeSet<(usize, String)>,
}

impl LoadClock {
    fn index(kind: LoadKind) -> usize {
        LoadKind::ALL
            .iter()
            .position(|k| *k == kind)
            .expect("ALL lists every kind")
    }

    /// The accumulator of a render phase, or `None` for the four always-present ones.
    fn optional_total(&mut self, phase: LoadPhase) -> Option<&mut Option<Duration>> {
        match phase {
            LoadPhase::FirstFrame => Some(&mut self.first_frame),
            LoadPhase::Voxelize => Some(&mut self.voxelize),
            LoadPhase::RendererInit => Some(&mut self.renderer_init),
            LoadPhase::GpuUpload => Some(&mut self.gpu_upload),
            LoadPhase::GiBake => Some(&mut self.gi_bake),
            LoadPhase::ProbeBake => Some(&mut self.probe_bake),
            LoadPhase::Scene | LoadPhase::Mesh | LoadPhase::Texture | LoadPhase::Other => None,
        }
    }

    fn finish(self) -> LoadProfile {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        LoadProfile {
            scene_ms: ms(self.scene),
            mesh_ms: ms(self.mesh),
            texture_ms: ms(self.texture),
            other_ms: ms(self.other),
            gpu_upload_ms: self.gpu_upload.map(ms),
            first_frame_ms: self.first_frame.map(ms),
            voxelize_ms: self.voxelize.map(ms),
            renderer_init_ms: self.renderer_init.map(ms),
            gi_bake_ms: self.gi_bake.map(ms),
            probe_bake_ms: self.probe_bake.map(ms),
            total_ms: match (self.origin, self.last_end) {
                (Some(origin), Some(end)) => ms(end.saturating_duration_since(origin)),
                _ => 0.0,
            },
            assets: LoadKind::ALL
                .iter()
                .enumerate()
                .map(|(i, kind)| AssetLoadCounters {
                    kind: kind.name().to_string(),
                    decoded: self.decoded[i],
                    cooked: self.cooked[i],
                    reused: self.reused[i],
                    bytes_read: self.bytes[i],
                })
                .collect(),
        }
    }
}

/// A [`ludion_renderer::FrameObserver`] that records the wall time of each `ProbeBake` span, in
/// order, and ignores every other phase.
#[derive(Default)]
pub(crate) struct SpanClock {
    started: Option<Instant>,
    pub(crate) spans: Vec<Duration>,
}

impl ludion_renderer::FrameObserver for SpanClock {
    // Sanctioned wall-clock site (see the module docs).
    #[allow(clippy::disallowed_methods)]
    fn begin(&mut self, phase: ludion_renderer::FramePhase) {
        if phase == ludion_renderer::FramePhase::ProbeBake {
            self.started = Some(Instant::now());
        }
    }

    // Sanctioned wall-clock site (see the module docs).
    #[allow(clippy::disallowed_methods)]
    fn end(&mut self, phase: ludion_renderer::FramePhase) {
        if phase == ludion_renderer::FramePhase::ProbeBake
            && let Some(start) = self.started.take()
        {
            self.spans.push(start.elapsed());
        }
    }
}

impl LoadObserver for LoadClock {
    // Sanctioned wall-clock site (see the module docs).
    #[allow(clippy::disallowed_methods)]
    fn begin(&mut self, phase: LoadPhase) {
        let now = Instant::now();
        self.origin.get_or_insert(now);
        if let Some(total) = self.optional_total(phase) {
            total.get_or_insert(Duration::ZERO);
        }
        self.stack.push((phase, now, Duration::ZERO));
    }

    // Sanctioned wall-clock site (see the module docs).
    #[allow(clippy::disallowed_methods)]
    fn end(&mut self, _phase: LoadPhase) {
        let Some((phase, start, child)) = self.stack.pop() else {
            return;
        };
        let now = Instant::now();
        let elapsed = now.duration_since(start);
        let exclusive = elapsed.saturating_sub(child);
        match phase {
            LoadPhase::Scene => self.scene += exclusive,
            LoadPhase::Mesh => self.mesh += exclusive,
            LoadPhase::Texture => self.texture += exclusive,
            LoadPhase::Other => self.other += exclusive,
            _ => {
                if let Some(total) = self.optional_total(phase) {
                    *total = Some(total.unwrap_or_default() + exclusive);
                }
            }
        }
        match self.stack.last_mut() {
            Some(parent) => parent.2 += elapsed,
            None => self.last_end = Some(now),
        }
    }

    fn decoded(&mut self, kind: LoadKind, key: &str, bytes: u64) {
        let i = Self::index(kind);
        self.decoded[i] += 1;
        self.bytes[i] += bytes;
        self.seen.insert((i, key.to_string()));
    }

    fn reused(&mut self, kind: LoadKind, key: &str) {
        let i = Self::index(kind);
        if self.seen.insert((i, key.to_string())) {
            self.reused[i] += 1;
        }
    }

    fn cooked(&mut self, kind: LoadKind, key: &str, bytes: u64) {
        let i = Self::index(kind);
        self.cooked[i] += 1;
        self.bytes[i] += bytes;
        self.seen.insert((i, key.to_string()));
    }
}

/// Accumulates one frame's per-system wall times through the clock-free observer seam.
///
/// [`before_system`](ScheduleObserver::before_system) stamps a start; the paired
/// [`after_system`](ScheduleObserver::after_system) records the elapsed span against the system name.
/// The vector is in run (registration) order and is cleared between frames by the driver loop.
///
/// Shared with the live profiler ([`perf_live`](crate::perf_live)), which attaches the same seam to
/// the live loop — one observer implementation, two drivers.
#[derive(Default)]
pub(crate) struct TimingObserver {
    started: Option<Instant>,
    pub(crate) frame: Vec<(&'static str, Duration)>,
}

impl ScheduleObserver for TimingObserver {
    // The profile driver is a sanctioned wall-clock site (see the module docs): measurement is
    // observation, output-only — it never enters sim state, so the WorldHash is provably unaffected.
    #[allow(clippy::disallowed_methods)]
    fn before_system(&mut self, _name: &'static str) {
        self.started = Some(Instant::now());
    }

    // Sanctioned wall-clock site, as in `before_system`: observation only.
    #[allow(clippy::disallowed_methods)]
    fn after_system(&mut self, name: &'static str) {
        let elapsed = self.started.take().map(|s| s.elapsed()).unwrap_or_default();
        self.frame.push((name, elapsed));
    }
}

/// The [`SimProfile::sequential`] entries, in report order — the fixed set of per-tick costs a
/// per-entity job system could never divide: hashing the world, extracting the render scene, and
/// draining one command through [`Engine::execute`].
pub const SEQUENTIAL_NAMES: [&str; 3] = ["world_hash", "extract", "command_drain"];

/// Measures one frame's [`SEQUENTIAL_NAMES`] costs, in that order.
///
/// All three run *after* the tick and outside the observer's per-system spans, so neither the
/// per-system medians nor the frame total contains any of them. None of the three touches sim state:
/// the hash and the extract only read the world (extract writes the CPU asset cache), and the drain
/// executes a **zero-tick** `sim.step` — the clone, dispatch, panic-check, reply, and record path a
/// host pays per tick, with the tick itself deliberately excluded because it is already
/// `frame_total_median_ms`. Live play hashes on request only; timing a hash the ordinary loop never
/// performs is the point (the replay oracle pays it per tick), and a hash is a pure read.
///
/// `sim.step` leaves the driver paused, so the run state is captured and restored — without that the
/// next measured frame would tick nothing. Being a real dispatch, it would also reach an active
/// recorder: a profiled run is never a recording one (every `profile_engine` caller drives its own
/// freshly built engine), and profiling one would put a zero-tick `SimStep` per frame in the `.lrp`.
// Sanctioned wall-clock site (see the module docs).
#[allow(clippy::disallowed_methods)]
fn measure_sequential(engine: &mut Engine) -> [Duration; 3] {
    let (width, height) = PROFILE_SIZE;

    let start = Instant::now();
    let _ = engine.world_hash();
    let hash = start.elapsed();

    let start = Instant::now();
    let _ = engine.extract_render_scene(width, height);
    let extract = start.elapsed();

    let run_state = engine.run_state();
    let start = Instant::now();
    let _ = engine.execute(Command::SimStep(SimStepParams {
        ticks: 0,
        hash: false,
    }));
    let drain = start.elapsed();
    match run_state {
        RunState::Running => engine.driver_mut().resume(),
        RunState::Paused => engine.driver_mut().pause(),
        RunState::Stepping { remaining } => engine.driver_mut().step(remaining),
    }

    [hash, extract, drain]
}

/// Drives the sim for `warmup` discarded then `frames` measured ticks, returning per-system,
/// sequential, and frame-total medians alongside the raw per-frame columns those medians were taken
/// over — the [`ProfileSamples`] a `--perfetto` export needs, so the offline driver can build a
/// timeline without an `--attach`.
// Sanctioned wall-clock site: the frame-total `Instant::now` here measures observation only and
// never feeds a tick (see the module docs).
#[allow(clippy::disallowed_methods)]
fn profile_sim(engine: &mut Engine, frames: u64, warmup: u64) -> (SimProfile, ProfileSamples) {
    let mut observer = TimingObserver::default();

    for _ in 0..warmup {
        observer.frame.clear();
        engine.advance_observed(&mut observer);
        // Warmed too: the first extract pays every asset cache miss in the scene.
        let _ = measure_sequential(engine);
    }

    let mut systems = SystemColumns::default();
    let mut per_sequential: [Vec<Duration>; SEQUENTIAL_NAMES.len()] = Default::default();
    let mut frame_totals: Vec<Duration> = Vec::new();
    let mut frame_indices: Vec<u64> = Vec::new();

    for _ in 0..frames {
        observer.frame.clear();
        let start = Instant::now();
        engine.advance_observed(&mut observer);
        frame_totals.push(start.elapsed());
        let frame = engine.frame().as_u64();
        frame_indices.push(frame);

        for (samples, elapsed) in per_sequential.iter_mut().zip(measure_sequential(engine)) {
            samples.push(elapsed);
        }

        systems.push(frame, &observer.frame);
    }

    reduce_sim(systems, per_sequential, frame_totals, frame_indices)
}

/// Per-system samples keyed by name, in first-appearance order: a schedule rebuilt mid-run (a switch
/// that adds `path_follow` or physics) cannot shift one system's samples onto another. A schedule
/// refuses a duplicate system name (`L4018`), so a name appears at most once per frame.
#[derive(Default)]
struct SystemColumns {
    names: Vec<&'static str>,
    /// `(frame, duration)` samples, one list per name.
    samples: Vec<Vec<(u64, Duration)>>,
}

impl SystemColumns {
    fn push(&mut self, frame: u64, observed: &[(&'static str, Duration)]) {
        for (name, duration) in observed {
            let column = match self.names.iter().position(|known| known == name) {
                Some(column) => column,
                None => {
                    self.names.push(name);
                    self.samples.push(Vec::new());
                    self.names.len() - 1
                }
            };
            self.samples[column].push((frame, *duration));
        }
    }
}

/// Reduces the measured columns to the report: medians and tails per system, per sequential cost and
/// for the frame total, plus the raw columns as [`ProfileSamples`]. Shared by every offline driver
/// (`profile_sim`, `profile_replay`), so a scene profile and a replay profile report the same
/// statistics.
fn reduce_sim(
    systems: SystemColumns,
    mut per_sequential: [Vec<Duration>; SEQUENTIAL_NAMES.len()],
    mut frame_totals: Vec<Duration>,
    frame_indices: Vec<u64>,
) -> (SimProfile, ProfileSamples) {
    let SystemColumns {
        names,
        samples: columns,
    } = systems;
    // A system's own column holds only the frames it ran in, so its median and tail read that column;
    // the raw `ProfileSamples` matrix is dense over `frame_indices`, with `0.0` where a system did not
    // run that frame (a perfetto reader treats a missing entry the same way).
    let system_ms: Vec<Vec<f64>> = columns
        .iter()
        .map(|column| {
            frame_indices
                .iter()
                .map(|frame| {
                    column
                        .binary_search_by_key(frame, |(f, _)| *f)
                        .map_or(0.0, |at| duration_ms(column[at].1))
                })
                .collect()
        })
        .collect();
    // Captured in measured order before the median passes below sort each column in place
    // (`median_ms` sorts its slice) — the raw columns need frame order, the medians don't.
    let sequential_ms: Vec<Vec<f64>> = per_sequential
        .iter()
        .map(|column| column.iter().map(|d| duration_ms(*d)).collect())
        .collect();
    let frame_total_ms: Vec<f64> = frame_totals.iter().map(|d| duration_ms(*d)).collect();

    let systems = names
        .iter()
        .zip(columns)
        .map(|(name, column)| {
            let (own_frames, mut own_ms): (Vec<u64>, Vec<f64>) = column
                .into_iter()
                .map(|(frame, duration)| (frame, duration_ms(duration)))
                .unzip();
            let own_tail = tail(&own_ms, &own_frames);
            SystemTiming {
                name: (*name).to_string(),
                median_ms: median_f64(&mut own_ms),
                tail: own_tail,
            }
        })
        .collect();

    let sequential = SEQUENTIAL_NAMES
        .iter()
        .zip(&mut per_sequential)
        .zip(&sequential_ms)
        .map(|((name, samples), column)| SystemTiming {
            name: (*name).to_string(),
            median_ms: median_ms(samples),
            tail: tail(column, &frame_indices),
        })
        .collect();

    let sim = SimProfile {
        systems,
        sequential,
        frame_total_median_ms: median_ms(&mut frame_totals),
        frame_total_tail: tail(&frame_total_ms, &frame_indices),
        // Read here rather than passed in: this driver just ran the schedule on whatever the
        // process resolved, and that is the count these medians describe.
        threads: Some(ludion_core::par::threads()),
    };

    let measured = frame_indices.len();
    let samples = ProfileSamples {
        frames: frame_indices,
        system_names: names.iter().map(|name| (*name).to_string()).collect(),
        system_ms,
        frame_total_ms,
        // `ProfileSamples::extract_ms`/`submit_ms` are the *live* tick's own spans
        // (`LIVE_SEQUENTIAL_NAMES` in `perf_live`, filed per rendered frame). The offline driver has
        // no such spans — its own `extract` measurement lives in `sim.sequential` above, taken beside
        // the tick rather than per rendered frame — so both columns are absent for every frame.
        extract_ms: vec![None; measured],
        submit_ms: vec![None; measured],
        // The offline GPU path (`profile_gpu` below) only ever produces one median per pass, not a
        // per-frame breakdown, so there is no per-frame column to report here.
        pass_names: Vec::new(),
        pass_ms: Vec::new(),
    };

    (sim, samples)
}

/// Profiles a recorded `.lrp` replay tick by tick through the game's own builder, returning the
/// report for the frames `from..=to` (`to` defaults to the replay's last frame).
///
/// The replay drives through the same loop [`replay_with`](crate::replay_with) uses, so the sim it
/// times is bit-identical to an unprofiled replay's (a test asserts the final `WorldHash`). Ticks
/// that produce a frame before `from` run unmeasured and count as [`ProfileReport::warmup_frames`];
/// frame 0 is the post-load world, so `from` 0 and 1 measure the same. Every sample, and every
/// [`TimingTail::max_frame`], is the produced-frame index (`engine.frame()` after the tick).
///
/// The report is the sim half only: `gpu` is `available: false` and `cluster` is `None`. Its
/// per-system, sequential and frame-total entries carry [`TimingTail`]s beside the medians, and
/// `samples` is the per-frame series. `label` names the replay in [`ProfileReport::scene`].
///
/// # Errors
/// `L3021` if the window falls outside the replay's recorded range or measures no frame, or if the
/// replay's `scene.load` (after a tick) or `engine.restore` resets the frame; and every error
/// [`replay_with`](crate::replay_with) can raise (a malformed `.lrp`, an unresolved scene, a
/// dispatch failure while driving).
///
/// # Example
/// ```
/// use ludion_engine::{drive_hashes, profile_replay, Engine};
/// use ludion_protocol::Command;
/// use ludion_protocol::command::SimStepParams;
///
/// // Record ten ticks of an inline scene into memory.
/// let sink = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
/// struct Sink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
/// impl std::io::Write for Sink {
///     fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
///         self.0.lock().unwrap().extend_from_slice(buf);
///         Ok(buf.len())
///     }
///     fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
/// }
/// let mut live = Engine::new();
/// live.load_inline_scene("format: 1\nseed: 1\nentities: []\n").unwrap();
/// live.set_recording(Sink(sink.clone())).unwrap();
/// drive_hashes(&mut live, vec![Command::SimStep(SimStepParams { ticks: 10, hash: false })]).unwrap();
/// live.stop_recording().unwrap();
/// let bytes = sink.lock().unwrap().clone();
///
/// // Profile frames 3..=7 of it.
/// let report = profile_replay(Engine::builder(), &bytes, ".", 3, Some(7), "run.lrp".to_string())
///     .unwrap();
/// assert_eq!(report.measured_frames, 5);
/// let tail = report.sim.frame_total_tail.unwrap();
/// assert!((3..=7).contains(&tail.max_frame));
/// ```
pub fn profile_replay(
    builder: EngineBuilder,
    bytes: &[u8],
    project_root: impl AsRef<Path>,
    from: u64,
    to: Option<u64>,
    label: String,
) -> Result<ProfileReport, Diagnostic> {
    profile_replay_engine(builder, bytes, project_root, from, to, label).map(|(report, _)| report)
}

/// [`profile_replay`]'s core, also returning the driven engine so a test can compare its hash.
pub(crate) fn profile_replay_engine(
    builder: EngineBuilder,
    bytes: &[u8],
    project_root: impl AsRef<Path>,
    from: u64,
    to: Option<u64>,
    label: String,
) -> Result<(ProfileReport, Engine), Diagnostic> {
    let (mut engine, actions) = prepare_replay(builder, bytes, project_root)?;
    require_monotonic_frames("profile-replay", &actions)?;
    let to = resolve_window("profile-replay", from, to, total_frames(&actions)?)?;
    let mut hook = ReplayProfile {
        from,
        to,
        warmup: 0,
        observer: TimingObserver::default(),
        systems: SystemColumns::default(),
        per_sequential: Default::default(),
        frame_totals: Vec::new(),
        frame_indices: Vec::new(),
    };
    drive_actions_with(&mut engine, actions, &mut hook)?;

    if hook.frame_indices.is_empty() {
        return Err(Diagnostic::new(
            code::REPLAY_FRAME_WINDOW,
            format!(
                "profile-replay measured no frame in the window `--from {from} --to {to}`: frame 0 \
                 is the loaded world and no tick produces it"
            ),
        )
        .with_help(
            "widen the window to include at least one ticked frame (`--to 1` or later), or check \
             that the replay records a sim.step",
        ));
    }
    let measured = hook.frame_indices.len() as u64;
    let (sim, samples) = reduce_sim(
        hook.systems,
        hook.per_sequential,
        hook.frame_totals,
        hook.frame_indices,
    );
    let report = ProfileReport {
        scene: label,
        warmup_frames: hook.warmup,
        measured_frames: measured,
        quality: None,
        sim,
        gpu: GpuProfile {
            available: false,
            timestamps_supported: false,
            passes: Vec::new(),
        },
        transport: None,
        cluster: None,
        load: None,
        probes: None,
        samples: Some(samples),
    };
    Ok((report, engine))
}

/// The [`DriveHook`] behind [`profile_replay`]: plain ticks outside the window, an observed and timed
/// tick inside it, and the end of the drive once the window is past.
struct ReplayProfile {
    from: u64,
    to: u64,
    warmup: u64,
    observer: TimingObserver,
    systems: SystemColumns,
    per_sequential: [Vec<Duration>; SEQUENTIAL_NAMES.len()],
    frame_totals: Vec<Duration>,
    frame_indices: Vec<u64>,
}

impl DriveHook for ReplayProfile {
    // Sanctioned wall-clock site: the frame-total `Instant::now` here measures observation only and
    // never feeds a tick (see the module docs).
    #[allow(clippy::disallowed_methods)]
    fn tick(&mut self, engine: &mut Engine) -> Result<bool, Diagnostic> {
        // The tick that *produces* frame N starts at frame N-1.
        let produced = engine.frame().as_u64() + 1;
        if produced < self.from {
            self.warmup += 1;
            return PlainDrive.tick(engine);
        }
        if produced > self.to {
            return Ok(false);
        }

        self.observer.frame.clear();
        let start = Instant::now();
        engine.step_ticks_observed(1, &mut self.observer);
        let elapsed = start.elapsed();
        // What `sim.step` does after its ticks; whether this panic is the recorded reproduction is
        // decided by the drive loop.
        if let Some(panic) = engine.take_system_panic() {
            return Err(panic);
        }
        let frame = engine.frame().as_u64();
        self.frame_totals.push(elapsed);
        self.frame_indices.push(frame);

        for (samples, elapsed) in self
            .per_sequential
            .iter_mut()
            .zip(measure_sequential(engine))
        {
            samples.push(elapsed);
        }

        self.systems.push(frame, &self.observer.frame);
        Ok(true)
    }
}

/// A [`Duration`] in milliseconds, as an `f64` — the per-sample conversion the median helpers below
/// don't expose (they sort and reduce, they don't hand back the individual values).
fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

/// The worst frame and nearest-rank 99th percentile of a per-frame column in frame order, `frames`
/// naming the frame each entry was measured at. `None` for an empty column. The maximum is the
/// first one when several tie.
fn tail(column_ms: &[f64], frames: &[u64]) -> Option<TimingTail> {
    let mut max_index = 0;
    for (i, ms) in column_ms.iter().enumerate() {
        if *ms > column_ms[max_index] {
            max_index = i;
        }
    }
    let max_ms = *column_ms.get(max_index)?;
    let mut sorted = column_ms.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let rank = (sorted.len() * 99).div_ceil(100);
    Some(TimingTail {
        max_ms,
        max_frame: frames.get(max_index).copied().unwrap_or_default(),
        p99_ms: sorted[rank - 1],
    })
}

/// Measures the clustered-forward light binning: the CPU build's median wall time over `frames`
/// builds, plus the occupancy of the tables the last one produced.
///
/// **No adapter is involved.** The binning is pure CPU arithmetic over the extracted scene
/// ([`ludion_renderer::cluster_stats`]), so this section is filled on a headless host exactly as it
/// is on a GPU one — the only part of a report that never degrades. It is also *why* the section can
/// exist at all without a profiler perturbing what it measures: the renderer's own per-frame
/// prologue is untouched, and an unprofiled frame pays exactly what it always paid.
///
/// The occupancy is taken from one build rather than reduced across `frames` of them because it is
/// not a measurement: the scene is fixed here (the same extracted scene is rebound each time, as in
/// [`profile_gpu`]), so every build produces byte-identical tables and a median over them would be a
/// more expensive spelling of any one.
// Sanctioned wall-clock site (see the module docs): this measures a pure function of an already
// extracted scene and never enters sim state.
#[allow(clippy::disallowed_methods)]
fn profile_cluster(
    engine: &mut Engine,
    frames: u64,
    warmup: u64,
) -> Result<ClusterProfile, Diagnostic> {
    let (width, height) = PROFILE_SIZE;
    // The *render* extent, not the output one: the tile half of the binning is a function of the
    // extent the scene is drawn at, which supersampling moves.
    let extent = engine.frame_size(width, height)?.render;
    let scene = engine
        .extract_render_scene(width, height)
        .map_err(collapse)?;

    for _ in 0..warmup {
        let _ = ludion_renderer::cluster_stats(&scene, extent);
    }

    let mut stats = ludion_renderer::cluster_stats(&scene, extent);
    let mut samples: Vec<f64> = Vec::new();
    for _ in 0..frames {
        let start = Instant::now();
        stats = ludion_renderer::cluster_stats(&scene, extent);
        samples.push(duration_ms(start.elapsed()));
    }

    Ok(ClusterProfile {
        build_median_ms: median_f64(&mut samples),
        lights: u64::from(stats.lights),
        cells: u64::from(stats.cells),
        occupied_cells: u64::from(stats.occupied_cells),
        total_refs: stats.total_refs,
        max_list_len: u64::from(stats.max_list_len),
        mean_list_len: stats.mean_list_len,
    })
}

/// Collects GPU per-pass timings by rendering the extracted scene `warmup` + `frames` times and
/// taking the per-pass median. Returns `available: false` (never an error) when there is no adapter.
fn profile_gpu(engine: &mut Engine, frames: u64, warmup: u64) -> Result<GpuProfile, Diagnostic> {
    let (width, height) = PROFILE_SIZE;
    let size = engine.frame_size(width, height)?;
    let scene = engine
        .extract_render_scene(width, height)
        .map_err(collapse)?;

    let renderer = match engine.renderer() {
        Ok(renderer) => renderer,
        // A headless host with no adapter is not a profiling failure — the sim section still stands.
        Err(diag) if diag.code == code::RENDER_NO_ADAPTER => {
            return Ok(GpuProfile {
                available: false,
                timestamps_supported: false,
                passes: Vec::new(),
            });
        }
        Err(diag) => return Err(diag),
    };

    for _ in 0..warmup {
        let _ = renderer.dump_timings_json(&scene, size)?;
    }

    let mut timestamps_supported = true;
    let mut names: Vec<String> = Vec::new();
    let mut per_pass: Vec<Vec<f64>> = Vec::new();

    for _ in 0..frames {
        let raw: RawTimings = serde_json::from_str(&renderer.dump_timings_json(&scene, size)?)
            .map_err(|error| {
                Diagnostic::new(
                    code::RENDER_READBACK_FAILED,
                    format!("the renderer's GPU timings could not be parsed: {error}"),
                )
                .with_help(
                    "report this as a renderer bug: `dump_timings_json` must emit its schema",
                )
            })?;
        timestamps_supported = raw.timestamps_supported;
        if names.is_empty() {
            names = raw.passes.iter().map(|p| p.name.clone()).collect();
            per_pass = vec![Vec::new(); names.len()];
        }
        for (i, pass) in raw.passes.iter().enumerate() {
            if let (Some(samples), Some(ms)) = (per_pass.get_mut(i), pass.gpu_time_ms) {
                samples.push(ms);
            }
        }
    }

    let passes = names
        .into_iter()
        .zip(per_pass)
        .map(|(name, mut samples)| PassTiming {
            name,
            median_ms: if samples.is_empty() {
                None
            } else {
                Some(median_f64(&mut samples))
            },
        })
        .collect();

    Ok(GpuProfile {
        available: true,
        timestamps_supported,
        passes,
    })
}

/// The `dump_timings_json` shape, parsed back to lift per-pass numbers out (reusing the renderer's
/// one timings machinery rather than duplicating the timestamp path).
#[derive(serde::Deserialize)]
struct RawTimings {
    timestamps_supported: bool,
    passes: Vec<RawPass>,
}

#[derive(serde::Deserialize)]
struct RawPass {
    name: String,
    gpu_time_ms: Option<f64>,
}

/// Median of a duration sample set, in milliseconds (`0.0` for an empty set). Sorts in place.
fn median_ms(samples: &mut [Duration]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.sort_unstable();
    let mid = samples.len() / 2;
    let ms = |d: Duration| d.as_secs_f64() * 1_000.0;
    if samples.len() % 2 == 1 {
        ms(samples[mid])
    } else {
        (ms(samples[mid - 1]) + ms(samples[mid])) / 2.0
    }
}

/// Median of an `f64` sample set (`0.0` for an empty set). Sorts in place.
///
/// Shared with the live profiler ([`perf_live`](crate::perf_live)): one median statistic, so an
/// offline report and a live window are comparable numbers.
pub(crate) fn median_f64(samples: &mut [f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.sort_by(|a, b| a.total_cmp(b));
    let mid = samples.len() / 2;
    if samples.len() % 2 == 1 {
        samples[mid]
    } else {
        (samples[mid - 1] + samples[mid]) / 2.0
    }
}

/// A profile budget: millisecond ceilings for the frame total, the cluster build, the sim-thread
/// extract cost, individual systems, GPU passes, and post-process effect groups. Every section is
/// optional — a budget checks only the lines it names. Parsed from a host's
/// `--check`/`--profile-check` TOML file by [`load_budget`]; compared against a report by
/// [`check_budget`].
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    /// Ceiling on the median sim-frame total, in milliseconds.
    #[serde(default)]
    pub frame_total_ms: Option<f64>,
    /// Ceiling on the sum of every post-process group's member passes, in milliseconds — the whole
    /// post chain as one number, whichever of its alternative sub-chains this scene ran.
    #[serde(default)]
    pub post_total_ms: Option<f64>,
    /// Ceiling on the median clustered-forward cluster build, in milliseconds — the CPU cost of
    /// deciding which lights each cell's list holds. A scalar rather than a table because there is
    /// one build per frame; skipped (never failed) when the report carries no cluster section, which
    /// is every live capture.
    #[serde(default)]
    pub cluster_build_ms: Option<f64>,
    /// Ceiling on the median of the report's `extract` sequential timing — the sim thread's
    /// per-frame extract cost, the one budget line on CPU work that scales with *visual* population
    /// (particles are evaluated there). Checked against [`SimProfile::sequential`]'s `extract` entry,
    /// which both the offline driver (`SEQUENTIAL_NAMES`) and a live capture (`LIVE_SEQUENTIAL_NAMES`)
    /// carry, so this line is checkable on either host; skipped (never failed) when no `extract`
    /// entry exists at all.
    #[serde(default)]
    pub extract_ms: Option<f64>,
    /// Ceiling on the report's `load.total_ms` — one run's wall time from the first load phase to the
    /// end of the first frame, renderer creation included. A single sample, not a median. A report
    /// with no `load` section (a live capture, `--profile-replay`, an unobserved host) is `L4025`.
    #[serde(default)]
    pub load_total_ms: Option<f64>,
    /// Ceiling on the worst single sim frame, in milliseconds — what a median hides. Checked
    /// against the frame total's [`TimingTail::max_ms`]; skipped (never failed) when the report
    /// carries no tail, which is every live capture.
    #[serde(default)]
    pub frame_total_max_ms: Option<f64>,
    /// Per-system median ceilings, keyed by system name.
    #[serde(default)]
    pub systems: BTreeMap<String, f64>,
    /// Per-system worst-frame ceilings, keyed by system name; same skip rule as
    /// `frame_total_max_ms`. A failure names the frame the worst sample occurred at.
    #[serde(default)]
    pub systems_max: BTreeMap<String, f64>,
    /// Per-GPU-pass median ceilings, keyed by pass name.
    #[serde(default)]
    pub passes: BTreeMap<String, f64>,
    /// Per-effect-group ceilings, keyed by post-process group name (a fixed set, listed by
    /// `ludion explain L4006`). A group's measured value is the sum of its member passes' medians,
    /// so one entry budgets a whole effect however many passes its implementation spends — a mip
    /// pyramid is one number, not twelve.
    #[serde(default)]
    pub post_groups: BTreeMap<String, f64>,
}

/// Reads and parses a budget file. Any read error or malformed TOML is an `L4004`.
///
/// # Errors
/// One `L4004` [`Diagnostic`] when the file cannot be read or is not a valid budget.
pub fn load_budget(path: &Path) -> Result<Budget, Diagnostic> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| budget_invalid(path, format!("could not read the file: {error}")))?;
    toml::from_str(&text).map_err(|error| budget_invalid(path, error.to_string()))
}

/// The frame graph's fixed pass names — the full universe
/// [`build_frame_graph`](ludion_renderer::build_frame_graph) can ever produce (`shadow` only for a
/// scene with a sun, `overlay` only for a scene with UI, `resolve` only above a render scale of 1, `upscale` only below it,
/// `gi_inject` only for a scene with a bouncing `GiVolume` and a light marked dynamic,
/// `probe_relight` only for a scene with probes).
/// Static and scene-independent, unlike
/// [`SimProfile::systems`]: a report's own `gpu.passes` list is *not* the producible set, because a
/// missing adapter or a UI-free scene leaves it incomplete without that being a budget-key mistake.
const KNOWN_PASS_NAMES: [&str; 36] = [
    ludion_renderer::graph::pass::SHADOW,
    ludion_renderer::graph::pass::PREPASS,
    ludion_renderer::graph::pass::SSAO,
    ludion_renderer::graph::pass::SSAO_BLUR,
    ludion_renderer::graph::pass::GI_INJECT,
    ludion_renderer::graph::pass::SCENE,
    ludion_renderer::graph::pass::REFLECTIONS,
    ludion_renderer::graph::pass::TRANSPARENTS,
    ludion_renderer::graph::pass::HIZ_SEED,
    ludion_renderer::graph::pass::HIZ_REDUCE,
    ludion_renderer::graph::pass::SSR_COLOR_SEED,
    ludion_renderer::graph::pass::SSR_COLOR_REDUCE,
    ludion_renderer::graph::pass::SSR_TRACE,
    ludion_renderer::graph::pass::SSR_BLEND,
    ludion_renderer::graph::pass::SSR_DEBUG,
    ludion_renderer::graph::pass::PROBE_DEBUG,
    ludion_renderer::graph::pass::PROBE_RELIGHT,
    ludion_renderer::graph::pass::SHAFTS,
    ludion_renderer::graph::pass::SHAFTS_BLUR,
    ludion_renderer::graph::pass::DOF_COC,
    ludion_renderer::graph::pass::DOF,
    ludion_renderer::graph::pass::BLOOM_THRESHOLD,
    ludion_renderer::graph::pass::BLOOM_DOWN[0],
    ludion_renderer::graph::pass::BLOOM_DOWN[1],
    ludion_renderer::graph::pass::BLOOM_DOWN[2],
    ludion_renderer::graph::pass::BLOOM_DOWN[3],
    ludion_renderer::graph::pass::BLOOM_DOWN[4],
    ludion_renderer::graph::pass::BLOOM_UP[0],
    ludion_renderer::graph::pass::BLOOM_UP[1],
    ludion_renderer::graph::pass::BLOOM_UP[2],
    ludion_renderer::graph::pass::BLOOM_UP[3],
    ludion_renderer::graph::pass::BLOOM_UP[4],
    ludion_renderer::graph::pass::COMPOSITE,
    ludion_renderer::graph::pass::RESOLVE,
    ludion_renderer::graph::pass::UPSCALE,
    ludion_renderer::graph::pass::OVERLAY,
];

/// The post-process effect groups a budget can put a ceiling on, and the passes each one sums.
///
/// A group exists so a budget can name an *effect* rather than an implementation: `bloom` is eleven
/// passes today and was three before the pyramid replaced the single blur, and a budget that had to
/// enumerate them would have to be rewritten every time the chain's shape changed.
///
/// Members reference the [`pass`](ludion_renderer::graph::pass) constants (never string literals),
/// exactly as [`KNOWN_PASS_NAMES`] does, so a pass rename cannot silently empty a group.
/// Only groups whose passes exist are listed — an effect with no passes yet gets its group when it
/// gets its passes.
///
/// **There is deliberately no `composite` group.** Composite is one pass, and folding is what
/// guarantees it stays one pass however many effects it grows: a group beside `[passes] composite`
/// would be a second way to budget the same number. By the same rule there is no `prepass` group: the
/// prepass is one pass, it exists for exactly one reason, and turning ambient occlusion on is what
/// schedules it — so it is a *member of* `ssao`, which is the effect a budget wants to name. If a
/// second consumer of the prepass ever arrives (depth/normal outlines are the recorded candidate), it
/// stops being SSAO's cost alone and earns its own line.
///
/// `ssr` is the pyramids and the trace, not `reflections` (which runs every frame for the sky) and not
/// the `ssr_debug` view.
const POST_GROUPS: [(&str, &[&str]); 5] = [
    (
        "bloom",
        &[
            ludion_renderer::graph::pass::BLOOM_THRESHOLD,
            ludion_renderer::graph::pass::BLOOM_DOWN[0],
            ludion_renderer::graph::pass::BLOOM_DOWN[1],
            ludion_renderer::graph::pass::BLOOM_DOWN[2],
            ludion_renderer::graph::pass::BLOOM_DOWN[3],
            ludion_renderer::graph::pass::BLOOM_DOWN[4],
            ludion_renderer::graph::pass::BLOOM_UP[0],
            ludion_renderer::graph::pass::BLOOM_UP[1],
            ludion_renderer::graph::pass::BLOOM_UP[2],
            ludion_renderer::graph::pass::BLOOM_UP[3],
            ludion_renderer::graph::pass::BLOOM_UP[4],
        ],
    ),
    (
        "ssao",
        &[
            ludion_renderer::graph::pass::PREPASS,
            ludion_renderer::graph::pass::SSAO,
            ludion_renderer::graph::pass::SSAO_BLUR,
        ],
    ),
    (
        "shafts",
        &[
            ludion_renderer::graph::pass::SHAFTS,
            ludion_renderer::graph::pass::SHAFTS_BLUR,
        ],
    ),
    (
        "dof",
        &[
            ludion_renderer::graph::pass::DOF_COC,
            ludion_renderer::graph::pass::DOF,
        ],
    ),
    (
        "ssr",
        &[
            ludion_renderer::graph::pass::HIZ_SEED,
            ludion_renderer::graph::pass::HIZ_REDUCE,
            ludion_renderer::graph::pass::SSR_COLOR_SEED,
            ludion_renderer::graph::pass::SSR_COLOR_REDUCE,
            ludion_renderer::graph::pass::SSR_TRACE,
            ludion_renderer::graph::pass::SSR_BLEND,
        ],
    ),
];

/// Pass kinds whose graph names carry a mip suffix (`hiz_reduce_3`): one pass per pyramid level, and
/// the level count depends on the render extent.
const MIPPED_PASS_KINDS: [&str; 2] = [
    ludion_renderer::graph::pass::HIZ_REDUCE,
    ludion_renderer::graph::pass::SSR_COLOR_REDUCE,
];

/// Whether the pass `name` is `member` itself, or — for a mipped kind — `member` plus `_<level>`.
fn pass_matches(member: &str, name: &str) -> bool {
    if member == name {
        return true;
    }
    MIPPED_PASS_KINDS.contains(&member)
        && name
            .strip_prefix(member)
            .and_then(|rest| rest.strip_prefix('_'))
            .is_some_and(|level| !level.is_empty() && level.bytes().all(|b| b.is_ascii_digit()))
}

/// The deepest pyramid level a graph can name: a `u32` extent has at most 32 mips and level 0 is the
/// seed, so reduce passes run `_1` to `_31`.
const MAX_PYRAMID_LEVEL: u32 = 31;

/// Whether a `[passes]` budget key names a pass this engine can produce: a fixed pass name, or a
/// mipped kind with a level suffix in `1..=MAX_PYRAMID_LEVEL`. A bare mipped kind is not a pass
/// (the graph only emits `<kind>_<level>`), so it would never match a measurement.
fn budget_pass_producible(name: &str) -> bool {
    KNOWN_PASS_NAMES.iter().any(|member| {
        if !MIPPED_PASS_KINDS.contains(member) {
            return member == &name;
        }
        name.strip_prefix(member)
            .and_then(|rest| rest.strip_prefix('_'))
            .and_then(|level| level.parse::<u32>().ok())
            .is_some_and(|level| (1..=MAX_PYRAMID_LEVEL).contains(&level))
    })
}

/// Sums the medians of the passes any of the `members` matches that this report actually measured,
/// returning the sum and how many contributed — or `None` when *none* did, which is the group-level
/// form of "producible but unmeasured" and skips rather than fails.
fn measured_sum(report: &ProfileReport, members: &[&str]) -> Option<(f64, usize)> {
    let mut total = 0.0;
    let mut counted = 0usize;
    for pass in &report.gpu.passes {
        if members.iter().any(|m| pass_matches(m, &pass.name))
            && let Some(ms) = pass.median_ms
        {
            total += ms;
            counted += 1;
        }
    }
    (counted > 0).then_some((total, counted))
}

/// Compares the report against the budget, returning one `L4005` diagnostic per over-budget line
/// plus one `L4006` per budget key that matches nothing the profiled engine could ever produce
/// (empty when everything is within budget and every key is producible).
///
/// A budgeted name that is producible but simply absent from *this* report — a `[passes]` entry for
/// a pass a UI-free scene never schedules (`overlay`), or every pass when the run has no GPU adapter
/// — is not a violation: a budget can only be *exceeded* by something that was measured. But a name
/// matching **nothing** producible is not that case: it can only be a rename, a deletion, or a typo,
/// and left unchecked the tripwire it names is permanently disarmed (`ludion explain L4006`).
///
/// The three categories use different producible sets:
/// - **`[systems]`**: the schedule runs every registered system unconditionally each tick (no
///   scene-dependent subset), so [`SimProfile::systems`] — this report's own measured list — already
///   *is* the full producible set for the engine that produced it (as long as at least one frame was
///   measured). A key absent from it cannot be a system this engine will ever run.
/// - **`[passes]`**: the frame graph's pass set is fixed and scene-independent
///   (`KNOWN_PASS_NAMES`), so it is checked against that static list rather than against
///   `report.gpu.passes` (which a missing adapter or a UI-free scene can under-report without that
///   being a mistake).
/// - **`[post_groups]`**: a group name is neither a system nor a pass but a fixed label over a set
///   of passes (`POST_GROUPS`), so it is checked against the group table's own names. The
///   *membership* is deliberately wider than any one run: a group spans every implementation of its
///   effect (both bloom chains), so an absent member is the norm, not a signal. Which is why the
///   skip rule moves up a level — the group is checked when **any** member was measured and skipped
///   only when none was.
///
/// A budget that names any of those GPU lines against a report with no GPU measurement (no adapter,
/// or no timestamp queries) is `L4019` — the skips above are per-scene, never per-machine.
///
/// `post_total_ms` is the same sum across every group's members, under the same skip rule; it is a
/// ceiling on the whole post chain, so it needs no producible-set check of its own.
/// `cluster_build_ms` is a single named measurement rather than a lookup, so it has no producible
/// set either — it is checked when the report carries a cluster section and skipped when it does
/// not.
/// `frame_total_max_ms` and `[systems_max]` check the worst frame ([`TimingTail::max_ms`]) under the
/// same rules as their median twins, skipped when the report carries no tail.
/// `load_total_ms` checks the report's single-run `load.total_ms`; unlike the lines above, a report
/// with no `load` section is `L4025` rather than a skip, since a live capture would otherwise pass.
/// `extract_ms` is a lookup by name (`"extract"`) into `sim.sequential` rather than a section
/// presence check like `cluster_build_ms` — both the offline driver and a live capture carry that
/// entry, so this line is checkable on either host and skipped only when a report carries no
/// `extract` entry at all.
pub fn check_budget(report: &ProfileReport, budget: &Budget) -> Vec<Diagnostic> {
    let mut over = Vec::new();

    if let Some(limit) = budget.frame_total_ms {
        let measured = report.sim.frame_total_median_ms;
        if measured > limit {
            over.push(exceeded("frame total", "median", measured, limit));
        }
    }

    // A report with no cluster section measured no build, and a budget can only be *exceeded* by
    // something that was measured — the same skip rule `[post_groups]` uses one level up. A live
    // capture always takes that branch, so a budget file gates the same line on whichever host can
    // answer for it and stays quiet on the one that cannot.
    if let (Some(limit), Some(cluster)) = (budget.cluster_build_ms, report.cluster.as_ref())
        && cluster.build_median_ms > limit
    {
        over.push(exceeded(
            "cluster build",
            "median",
            cluster.build_median_ms,
            limit,
        ));
    }

    // Same skip rule as `cluster_build_ms`, but a name lookup rather than a section presence check:
    // both the offline driver's `SEQUENTIAL_NAMES` and a live capture's `LIVE_SEQUENTIAL_NAMES` carry
    // an `extract` entry, so this line is checkable on either host and quiet only when neither does.
    if let (Some(limit), Some(extract)) = (
        budget.extract_ms,
        report.sim.sequential.iter().find(|s| s.name == "extract"),
    ) && extract.median_ms > limit
    {
        over.push(exceeded("extract", "median", extract.median_ms, limit));
    }

    match (budget.load_total_ms, &report.load) {
        (Some(limit), Some(load)) if load.total_ms > limit => {
            over.push(exceeded("load total", "single run", load.total_ms, limit));
        }
        (Some(_), None) => over.push(load_unmeasured()),
        _ => {}
    }

    // The worst-frame ceilings follow the same skip rule: a report without a tail (a live capture, or
    // one predating the field) measured no maximum, and nothing unmeasured can be over budget.
    if let (Some(limit), Some(tail)) = (budget.frame_total_max_ms, &report.sim.frame_total_tail)
        && tail.max_ms > limit
    {
        over.push(exceeded(
            &format!("frame total (worst at frame {})", tail.max_frame),
            "max",
            tail.max_ms,
            limit,
        ));
    }

    let known_systems: Vec<&str> = report.sim.systems.iter().map(|s| s.name.as_str()).collect();
    for (name, &limit) in &budget.systems {
        match report.sim.systems.iter().find(|s| &s.name == name) {
            Some(system) if system.median_ms > limit => over.push(exceeded(
                &format!("system `{name}`"),
                "median",
                system.median_ms,
                limit,
            )),
            Some(_) => {}
            None => over.push(unknown_budget_key("system", name, &known_systems)),
        }
    }

    for (name, &limit) in &budget.systems_max {
        match report.sim.systems.iter().find(|s| &s.name == name) {
            Some(system) => {
                if let Some(tail) = &system.tail
                    && tail.max_ms > limit
                {
                    over.push(exceeded(
                        &format!("system `{name}` (worst at frame {})", tail.max_frame),
                        "max",
                        tail.max_ms,
                        limit,
                    ));
                }
            }
            None => over.push(unknown_budget_key("system", name, &known_systems)),
        }
    }

    // The per-key skips below are right for one scene that never schedules a pass, but a run that
    // measured no GPU at all would skip every line and read as a pass.
    let gates_gpu = !budget.passes.is_empty()
        || !budget.post_groups.is_empty()
        || budget.post_total_ms.is_some();
    if gates_gpu && !(report.gpu.available && report.gpu.timestamps_supported) {
        over.push(gpu_unmeasured(report));
    }

    for (name, &limit) in &budget.passes {
        match report.gpu.passes.iter().find(|p| &p.name == name) {
            Some(pass) => {
                if let Some(measured) = pass.median_ms
                    && measured > limit
                {
                    over.push(exceeded(
                        &format!("GPU pass `{name}`"),
                        "median",
                        measured,
                        limit,
                    ));
                }
            }
            None if budget_pass_producible(name) => {
                // Producible (a fixed frame-graph pass name) but not in this report — a UI-free
                // scene, or no GPU adapter. Not a violation.
            }
            None if MIPPED_PASS_KINDS.contains(&name.as_str()) => over.push(
                unknown_budget_key("GPU pass", name, &KNOWN_PASS_NAMES).with_help(format!(
                    "`{name}` is a pyramid kind: the graph names its passes `{name}_<level>`, one per \
                     mip level — budget the `ssr` post group for the whole pyramid, or a level such as \
                     `{name}_1`"
                )),
            ),
            None => over.push(unknown_budget_key("GPU pass", name, &KNOWN_PASS_NAMES)),
        }
    }

    let group_names: Vec<&str> = POST_GROUPS.iter().map(|(name, _)| *name).collect();
    for (name, &limit) in &budget.post_groups {
        match POST_GROUPS.iter().find(|(group, _)| group == name) {
            Some((_, members)) => {
                // No member measured — the whole effect was absent from this run (or there was no
                // adapter). Nothing was measured, so nothing can be over budget.
                if let Some((measured, counted)) = measured_sum(report, members)
                    && measured > limit
                {
                    over.push(exceeded(
                        &format!("post group `{name}` ({counted} passes)"),
                        "summed medians",
                        measured,
                        limit,
                    ));
                }
            }
            None => over.push(unknown_budget_key("post group", name, &group_names)),
        }
    }

    if let Some(limit) = budget.post_total_ms {
        let members: Vec<&str> = POST_GROUPS
            .iter()
            .flat_map(|(_, members)| members.iter().copied())
            .collect();
        if let Some((measured, counted)) = measured_sum(report, &members)
            && measured > limit
        {
            over.push(exceeded(
                &format!("post total ({counted} passes)"),
                "summed medians",
                measured,
                limit,
            ));
        }
    }

    over
}

/// Builds the `L4019` for a budget that gates GPU timings against a run that measured none.
fn gpu_unmeasured(report: &ProfileReport) -> Diagnostic {
    let why = if report.gpu.available {
        "the GPU adapter has no timestamp queries"
    } else {
        "no GPU adapter was available"
    };
    Diagnostic::new(
        code::BUDGET_GPU_UNMEASURED,
        format!(
            "budget gates GPU timings (`[passes]`, `[post_groups]` or `post_total_ms`) but none were \
             measured: {why}"
        ),
    )
    .with_help(
        "run the check on the pinned box whose adapter the budget was calibrated on, or remove the \
         GPU lines from a CPU-only budget",
    )
}

/// Builds the `L4025` for a budget that gates the load total against a report with no load section.
fn load_unmeasured() -> Diagnostic {
    Diagnostic::new(
        code::BUDGET_LOAD_UNMEASURED,
        "budget gates `load_total_ms` but the report has no load section: this run measured no load",
    )
    .with_help(
        "measure with `ludion profile --scene …` or a game's `--profile` (both observe the load); \
         live captures and `--profile-replay` never carry one",
    )
}

/// Builds one `L4006` for a budget key matching no producible name, naming the nearest known name
/// (registration order, or the fixed pass order) as the likely fix if one is close enough to plausibly
/// be a typo or a stale rename.
fn unknown_budget_key(kind: &str, name: &str, known: &[&str]) -> Diagnostic {
    let message = format!(
        "budget names {kind} `{name}`, which does not match any {kind} name this profile run could \
         ever produce"
    );
    let help = match suggest::nearest(name, known.iter().copied()) {
        Some(nearest) => format!(
            "nearest known {kind} name is `{nearest}` — likely a rename or typo; fix the budget key, \
             or remove the entry if the {kind} was intentionally deleted"
        ),
        None => format!(
            "no known {kind} name is close enough to be an obvious typo — check for a deleted {kind} \
             or a stale budget entry"
        ),
    };
    Diagnostic::new(code::BUDGET_UNKNOWN_KEY, message).with_help(help)
}

/// Builds one `L4005` for an over-budget line, naming the measured value and the ceiling it crossed.
/// `statistic` names what the number *is* — a single line's `median`, or a group's `summed medians`
/// — so a reader can tell one pass from an aggregate over several without consulting the budget.
fn exceeded(what: &str, statistic: &str, measured_ms: f64, limit_ms: f64) -> Diagnostic {
    Diagnostic::new(
        code::BUDGET_EXCEEDED,
        format!("{what}: {statistic} {measured_ms:.3} ms exceeds budget {limit_ms:.3} ms"),
    )
    .with_help(
        "confirm the regression reproduces with another `ludion profile` run, then fix the code that \
         got slower — re-calibrate the budget only if the new cost is intended and permanent",
    )
}

/// Builds the `L4004` for an unreadable or invalid budget file.
fn budget_invalid(path: &Path, detail: String) -> Diagnostic {
    Diagnostic::new(
        code::BUDGET_FILE_INVALID,
        format!(
            "`{}` is not a valid profile budget: {detail}",
            path.display()
        ),
    )
    .with_help(
        "a budget is TOML: optional `frame_total_ms`, `frame_total_max_ms`, `post_total_ms`, \
         `cluster_build_ms`, `extract_ms` and `load_total_ms` scalars, and `[systems]`, \
         `[systems_max]`, `[passes]` and `[post_groups]` tables of millisecond ceilings — see \
         `ludion explain L4004`",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_core::par;
    use ludion_renderer::RenderScene;
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

    use ludion_ecs::World;

    fn counter_engine() -> Engine {
        Engine::builder()
            .components(|reg| reg.register::<Counter>("Counter"))
            .system("bump", bump)
            .build()
            .unwrap()
    }

    const SCENE: &str =
        "format: 1\nseed: 7\nentities:\n  - components:\n      Counter:\n        n: 0\n";

    /// The `sequential` section a hand-built report carries: the fixed three, one flat number each.
    fn sample_sequential() -> Vec<SystemTiming> {
        SEQUENTIAL_NAMES
            .iter()
            .map(|name| SystemTiming {
                name: (*name).to_string(),
                median_ms: 0.01,
                tail: None,
            })
            .collect()
    }

    #[test]
    fn observer_does_not_change_world_hash() {
        // Drive one engine the ordinary way, another through the observed profile path; the hooked
        // run must leave the world in bit-identical state.
        let mut plain = counter_engine();
        plain.load_inline_scene(SCENE).unwrap();
        for _ in 0..50 {
            plain.advance_live();
        }
        let plain_hash = plain.world_hash().unwrap();

        let mut observed = counter_engine();
        observed.observe_load();
        observed.load_inline_scene(SCENE).unwrap();
        let load = finish_load(&mut observed).expect("an observed load reports");
        assert!(load.scene_ms.is_finite() && load.scene_ms >= 0.0);
        let mut observer = TimingObserver::default();
        for _ in 0..50 {
            observer.frame.clear();
            observed.advance_observed(&mut observer);
        }
        let observed_hash = observed.world_hash().unwrap();

        assert_eq!(
            plain_hash, observed_hash,
            "attaching the schedule observer must not perturb the WorldHash"
        );
        // And the hook actually saw the system each tick.
        assert_eq!(observer.frame.len(), 1);
        assert_eq!(observer.frame[0].0, "bump");
    }

    /// A one-triangle mesh document binding `../textures/t.png` as its base-colour map.
    const TEXTURED_MESH: &str = "format: 1
kind: mesh
positions:
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
uvs:
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 1.0
base_color_texture:
  path: ../textures/t.png
indices:
  - 0
  - 1
  - 2
";

    fn mesh_entity(asset: &str) -> String {
        format!(
            "  - components:
      Mesh:
        shape:
          asset: {asset}
"
        )
    }

    fn counters<'a>(load: &'a LoadProfile, kind: &str) -> &'a AssetLoadCounters {
        load.assets
            .iter()
            .find(|a| a.kind == kind)
            .expect("every kind is listed")
    }

    #[test]
    fn needs_gpu_load_profile_counts_decodes_and_reuse() {
        let dir = std::env::temp_dir().join(format!("ludion-load-profile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("meshes")).unwrap();
        std::fs::create_dir_all(dir.join("textures")).unwrap();
        let texture = ludion_assets::encode_png(1, 1, &[200, 20, 30, 255]).unwrap();
        let sprite = ludion_assets::encode_png(2, 2, &[10; 16]).unwrap();
        std::fs::write(dir.join("textures/t.png"), &texture).unwrap();
        std::fs::write(dir.join("textures/s.png"), &sprite).unwrap();
        std::fs::write(dir.join("meshes/a.mesh.yaml"), TEXTURED_MESH).unwrap();
        std::fs::write(dir.join("meshes/b.mesh.yaml"), TEXTURED_MESH).unwrap();

        let mut scene = String::from(
            "format: 1
entities:
",
        );
        for _ in 0..3 {
            scene.push_str(&mesh_entity("meshes/a.mesh.yaml"));
        }
        scene.push_str(&mesh_entity("meshes/b.mesh.yaml"));
        scene.push_str(
            "  - components:
      Sprite:
        texture: textures/s.png
        size:
          - 1.0
          - 1.0
",
        );

        let mut engine = Engine::with_project_root(&dir);
        engine.observe_load();
        engine.load_inline_scene(&scene).unwrap();
        let load = measure_load(&mut engine).unwrap().expect("observed");

        let names: Vec<&str> = load.assets.iter().map(|a| a.kind.as_str()).collect();
        let expected: Vec<&str> = LoadKind::ALL.iter().map(|k| k.name()).collect();
        assert_eq!(names, expected);
        let doc_len = TEXTURED_MESH.len() as u64;
        let mesh = counters(&load, "mesh");
        assert_eq!(
            (mesh.decoded, mesh.reused, mesh.bytes_read),
            (2, 0, 2 * doc_len)
        );
        let tex = counters(&load, "texture");
        assert_eq!(
            (tex.decoded, tex.reused, tex.bytes_read),
            (2, 0, texture.len() as u64 + sprite.len() as u64),
            "the map two meshes share is one decode"
        );
        for kind in [
            "voxel",
            "voxel_grid",
            "sculpt",
            "assembly",
            "character",
            "atlas",
        ] {
            let c = counters(&load, kind);
            assert_eq!((c.decoded, c.reused, c.bytes_read), (0, 0, 0), "{kind}");
        }

        // A second window over the same, still-warm cache decodes nothing.
        engine.observe_load();
        let again = measure_load(&mut engine).unwrap().expect("observed");
        assert_eq!(counters(&again, "mesh").decoded, 0);
        assert_eq!(counters(&again, "mesh").reused, 2);
        assert_eq!(counters(&again, "texture").decoded, 0);
        assert_eq!(counters(&again, "texture").reused, 1);
        assert!(again.first_frame_ms.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A cooked read counts apart from a decode, and a report written before the field existed
    /// reads back with zero.
    #[test]
    fn load_profile_counts_cooked_reads_apart() {
        let mut clock = LoadClock::default();
        clock.decoded(LoadKind::Texture, "a.png", 10);
        clock.cooked(LoadKind::Texture, "b.ktx2", 20);
        clock.cooked(LoadKind::Texture, "c.ktx2", 30);
        let load = clock.finish();
        let tex = counters(&load, "texture");
        assert_eq!((tex.decoded, tex.cooked, tex.bytes_read), (1, 2, 60));
        assert_eq!(counters(&load, "mesh").cooked, 0);

        let mut json = serde_json::to_value(tex).unwrap();
        assert_eq!(json["cooked"], 2);
        json.as_object_mut().unwrap().remove("cooked");
        let old: AssetLoadCounters = serde_json::from_value(json).unwrap();
        assert_eq!(old.cooked, 0);
    }

    /// The prefetch drops its failures: the extract that follows reports them exactly as it does on a
    /// cold cache with no prefetch at all.
    #[test]
    fn prefetch_failures_surface_through_extract_unchanged() {
        let dir = std::env::temp_dir().join(format!("ludion-prefetch-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("meshes")).unwrap();
        std::fs::create_dir_all(dir.join("textures")).unwrap();
        std::fs::write(dir.join("textures/t.png"), b"not a png").unwrap();
        std::fs::write(dir.join("meshes/a.mesh.yaml"), TEXTURED_MESH).unwrap();
        let scene = format!(
            "format: 1
entities:
{}{}  - components:
      Sprite:
        texture: textures/missing.png
        size:
          - 1.0
          - 1.0
",
            mesh_entity("meshes/missing.mesh.yaml"),
            mesh_entity("meshes/a.mesh.yaml"),
        );

        let mut engine = Engine::with_project_root(&dir);
        engine.load_inline_scene(&scene).unwrap();
        let prefetched = engine.extract_render_scene(320, 180);
        let cold = crate::extract::extract_through(
            engine.world(),
            (320, 180),
            &dir,
            &mut crate::asset_cache::AssetCache::new(),
            ludion_core::time::SimTime::from_ticks(0, ludion_core::time::TickRate::DEFAULT),
            ludion_renderer::QualityParams::FULL.particle_density,
            None,
            &mut Vec::new(),
        );
        let _ = std::fs::remove_dir_all(&dir);

        let (prefetched, cold) = (prefetched.err().unwrap(), cold.err().unwrap());
        assert!(!cold.is_empty());
        assert_eq!(format!("{prefetched:?}"), format!("{cold:?}"));
    }

    /// After a load's first extract every walked asset is cached, and each kind decoded exactly its
    /// unique files: the map two meshes share counts once.
    #[test]
    fn first_extract_after_a_load_is_all_hits() {
        let dir = std::env::temp_dir().join(format!("ludion-first-extract-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("meshes")).unwrap();
        std::fs::create_dir_all(dir.join("textures")).unwrap();
        let png = ludion_assets::encode_png(1, 1, &[200, 20, 30, 255]).unwrap();
        std::fs::write(dir.join("textures/t.png"), &png).unwrap();
        std::fs::write(dir.join("textures/s.png"), &png).unwrap();
        std::fs::write(dir.join("meshes/a.mesh.yaml"), TEXTURED_MESH).unwrap();
        std::fs::write(dir.join("meshes/b.mesh.yaml"), TEXTURED_MESH).unwrap();
        let scene = format!(
            "format: 1
entities:
{}{}  - components:
      Sprite:
        texture: textures/s.png
        size:
          - 1.0
          - 1.0
",
            mesh_entity("meshes/a.mesh.yaml"),
            mesh_entity("meshes/b.mesh.yaml"),
        );

        let mut engine = Engine::with_project_root(&dir);
        engine.observe_load();
        engine.load_inline_scene(&scene).unwrap();
        engine.extract_render_scene(320, 180).unwrap();
        let load = finish_load(&mut engine).expect("observed");
        let cache = engine.asset_cache();
        for mesh in ["meshes/a.mesh.yaml", "meshes/b.mesh.yaml"] {
            assert!(cache.contains_mesh(mesh), "{mesh}");
        }
        assert!(cache.contains_mesh_map("textures/t.png"));
        assert!(cache.contains("textures/s.png"));
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(counters(&load, "mesh").decoded, 2);
        assert_eq!(counters(&load, "texture").decoded, 2);
    }

    /// A scratch project for the retain tests: meshes `a`, `s` (shared) and `b`, each binding its own
    /// base-colour map, `a` and `s` also binding `shared.png` as their emissive map, plus one loose
    /// sprite PNG per scene.
    fn retain_project(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ludion-retain-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("meshes")).unwrap();
        std::fs::create_dir_all(dir.join("textures")).unwrap();
        let png = |red: u8| ludion_assets::encode_png(1, 1, &[red, 20, 30, 255]).unwrap();
        for (name, red) in [
            ("a", 10),
            ("s", 20),
            ("b", 30),
            ("shared", 40),
            ("sp_a", 50),
            ("sp_b", 60),
        ] {
            std::fs::write(dir.join(format!("textures/{name}.png")), png(red)).unwrap();
        }
        for (name, shared) in [("a", true), ("s", true), ("b", false)] {
            let mut doc = TEXTURED_MESH.replace("t.png", &format!("{name}.png"));
            if shared {
                doc.push_str("emissive_texture:\n  path: ../textures/shared.png\n");
            }
            std::fs::write(dir.join(format!("meshes/{name}.mesh.yaml")), doc).unwrap();
        }
        dir
    }

    /// A scene naming `meshes` (by stem) and one loose sprite.
    fn retain_scene(meshes: &[&str], sprite: &str) -> String {
        let mut scene = String::from("format: 1\nentities:\n");
        for mesh in meshes {
            scene.push_str(&mesh_entity(&format!("meshes/{mesh}.mesh.yaml")));
        }
        scene.push_str(&format!(
            "  - components:
      Sprite:
        texture: textures/{sprite}.png
        size:
          - 1.0
          - 1.0
"
        ));
        scene
    }

    /// Loads `scene` and runs its first extract.
    fn load_and_extract(engine: &mut Engine, scene: &str) -> RenderScene {
        engine.load_inline_scene(scene).unwrap();
        engine.extract_render_scene(320, 180).unwrap()
    }

    #[test]
    fn a_second_scene_reuses_what_it_shares() {
        let dir = retain_project("shares");
        let mut engine = Engine::with_project_root(&dir);
        load_and_extract(&mut engine, &retain_scene(&["a", "s"], "sp_a"));

        engine.observe_load();
        load_and_extract(&mut engine, &retain_scene(&["b", "s"], "sp_b"));
        let load = finish_load(&mut engine).expect("observed");

        let mesh = counters(&load, "mesh");
        assert_eq!((mesh.decoded, mesh.reused), (1, 1), "b decoded, s reused");
        let texture = counters(&load, "texture");
        assert_eq!(texture.decoded, 2, "b.png and sp_b.png are B's own");
        assert!(texture.reused >= 1, "the shared map is held");
        let cache = engine.asset_cache();
        assert!(!cache.contains_mesh("meshes/a.mesh.yaml"));
        assert!(!cache.contains_mesh_map("textures/a.png"));
        assert!(!cache.contains("textures/sp_a.png"));
        assert!(cache.contains_mesh_map("textures/shared.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_edited_shared_map_decodes_again_and_nothing_else() {
        let dir = retain_project("edited");
        let mut engine = Engine::with_project_root(&dir);
        let scene = retain_scene(&["a", "s"], "sp_a");
        let before = load_and_extract(&mut engine, &scene);

        let edited = ludion_assets::encode_png(1, 1, &[99, 20, 30, 255]).unwrap();
        std::fs::write(dir.join("textures/shared.png"), &edited).unwrap();
        engine.observe_load();
        let after = load_and_extract(&mut engine, &scene);
        let load = finish_load(&mut engine).expect("observed");

        assert_eq!(counters(&load, "texture").decoded, 1, "only shared.png");
        assert!(
            counters(&load, "texture").reused >= 3,
            "the sprite and both own maps"
        );
        assert_eq!(counters(&load, "mesh").decoded, 2, "a and s name it");
        assert_eq!(counters(&load, "mesh").reused, 0);
        let hashes = |scene: &RenderScene| -> Vec<u64> {
            scene.mesh_assets.iter().map(|mesh| mesh.hash).collect()
        };
        assert_eq!(hashes(&before).len(), 2);
        assert!(
            hashes(&before)
                .iter()
                .zip(hashes(&after))
                .all(|(old, new)| *old != new),
            "both meshes carry the edited map's hash"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_b_a_decodes_only_what_b_evicted() {
        let dir = retain_project("aba");
        let mut engine = Engine::with_project_root(&dir);
        let a = retain_scene(&["a", "s"], "sp_a");
        load_and_extract(&mut engine, &a);
        load_and_extract(&mut engine, &retain_scene(&["b", "s"], "sp_b"));

        engine.observe_load();
        load_and_extract(&mut engine, &a);
        let load = finish_load(&mut engine).expect("observed");

        assert_eq!(counters(&load, "mesh").decoded, 1, "a only; s is held");
        assert_eq!(counters(&load, "texture").decoded, 2, "a.png and sp_a.png");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_switch_chain_holds_only_the_current_world() {
        let dir = retain_project("chain");
        let scene = |mesh: &str, sprite: &str| {
            let mut text = retain_scene(&[mesh], sprite);
            text.push_str(
                "  - name: anchor
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
",
            );
            text
        };
        let scenes = [("a", "sp_a"), ("s", "sp_b"), ("b", "sp_a")];
        let mut engine = Engine::with_project_root(&dir);
        load_and_extract(&mut engine, &scene(scenes[0].0, scenes[0].1));
        for (index, (mesh, sprite)) in scenes.iter().enumerate() {
            if index > 0 {
                let text = scene(mesh, sprite);
                std::fs::write(dir.join(format!("district{index}.yaml")), &text).unwrap();
                let switch = crate::transition::SwitchTo {
                    scene_path: format!("district{index}.yaml"),
                    spawn_anchor: "anchor".into(),
                    carry: vec![],
                };
                crate::transition::perform(&mut engine, switch, &text).unwrap();
                engine.extract_render_scene(320, 180).unwrap();
            }
            let cache = engine.asset_cache();
            for other in ["a", "s", "b"] {
                let held = other == *mesh;
                let key = format!("meshes/{other}.mesh.yaml");
                assert_eq!(cache.contains_mesh(&key), held, "scene {index}: {key}");
                let own = format!("textures/{other}.png");
                assert_eq!(cache.contains_mesh_map(&own), held, "scene {index}: {own}");
            }
            // `a` and `s` both bind `shared.png`; `b` does not.
            assert_eq!(
                cache.contains_mesh_map("textures/shared.png"),
                *mesh != "b",
                "scene {index}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The old clear's case: the same path holding new bytes at the next load is never served stale.
    #[test]
    fn same_path_new_bytes_is_never_served_stale() {
        let dir = retain_project("stale");
        let mut engine = Engine::with_project_root(&dir);
        let scene = retain_scene(&[], "sp_a");
        load_and_extract(&mut engine, &scene);

        let new = ludion_assets::encode_png(1, 1, &[77, 20, 30, 255]).unwrap();
        std::fs::write(dir.join("textures/sp_a.png"), &new).unwrap();
        let extracted = load_and_extract(&mut engine, &scene);

        let hash = ludion_assets::content_hash(&new);
        assert!(
            extracted
                .textures
                .iter()
                .any(|texture| texture.hash == hash)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn needs_gpu_observed_load_fills_render_phases() {
        let dir = std::env::temp_dir().join(format!("ludion-load-phases-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("meshes")).unwrap();
        std::fs::create_dir_all(dir.join("textures")).unwrap();
        let texture = ludion_assets::encode_png(1, 1, &[200, 20, 30, 255]).unwrap();
        std::fs::write(dir.join("textures/t.png"), &texture).unwrap();
        std::fs::write(dir.join("meshes/a.mesh.yaml"), TEXTURED_MESH).unwrap();
        let scene = format!(
            "format: 1
entities:
{}",
            mesh_entity("meshes/a.mesh.yaml")
        );

        let mut engine = Engine::with_project_root(&dir);
        engine.observe_load();
        engine.load_inline_scene(&scene).unwrap();
        let load = measure_load(&mut engine).unwrap().expect("observed");
        let _ = std::fs::remove_dir_all(&dir);

        let (Some(first_frame), Some(init), Some(upload)) = (
            load.first_frame_ms,
            load.renderer_init_ms,
            load.gpu_upload_ms,
        ) else {
            eprintln!("skipping: no GPU adapter available (L2003)");
            return;
        };
        assert!(first_frame >= 0.0 && init >= 0.0 && upload >= 0.0);
        let phases = load.scene_ms
            + load.mesh_ms
            + load.texture_ms
            + load.other_ms
            + first_frame
            + init
            + upload
            + load.voxelize_ms.unwrap_or(0.0)
            + load.gi_bake_ms.unwrap_or(0.0)
            + load.probe_bake_ms.unwrap_or(0.0);
        assert!(
            load.total_ms >= phases - 1e-6,
            "total {} ms is less than its phases' {phases} ms",
            load.total_ms
        );
    }

    #[test]
    fn sequential_measurements_do_not_change_world_hash() {
        // Same contract as `observer_does_not_change_world_hash`, one level up: a full profiled run
        // — which hashes, extracts, and drains a zero-tick `sim.step` after every tick — must leave
        // the world where an unprofiled run of the same tick count leaves it. The extra `sim.step`
        // is the sharp edge: it pauses the driver, and a run that forgot to restore the run state
        // would tick once and then stand still, which this catches as a hash mismatch.
        let mut plain = counter_engine();
        plain.load_inline_scene(SCENE).unwrap();
        for _ in 0..50 {
            plain.advance_live();
        }
        let plain_hash = plain.world_hash().unwrap();

        let mut profiled = counter_engine();
        profiled.load_inline_scene(SCENE).unwrap();
        let (sim, samples) = profile_sim(&mut profiled, 40, 10);

        assert_eq!(
            plain_hash,
            profiled.world_hash().unwrap(),
            "the sequential measurements must not perturb the WorldHash"
        );
        assert_eq!(profiled.frame().as_u64(), 50, "no tick was added or lost");
        assert_eq!(profiled.run_state(), RunState::Running);
        assert_eq!(sim.sequential.len(), SEQUENTIAL_NAMES.len());
        assert_eq!(
            samples.frames.len(),
            40,
            "one column entry per measured frame"
        );
    }

    #[test]
    fn profiled_run_reports_the_sequential_section() {
        let mut engine = counter_engine();
        engine.load_inline_scene(SCENE).unwrap();
        let (sim, _samples) = profile_sim(&mut engine, 8, 1);

        let names: Vec<&str> = sim.sequential.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, SEQUENTIAL_NAMES);
        for timing in &sim.sequential {
            assert!(
                timing.median_ms.is_finite() && timing.median_ms >= 0.0,
                "`{}` is not a measurement: {} ms",
                timing.name,
                timing.median_ms
            );
        }

        // Hashing and extraction walk the world, so on any world they cost measurably more than the
        // clock's ~100 ns granularity. `command_drain` is deliberately not held to that: a zero-tick
        // dispatch is world-size independent and sits *at* that granularity, so its median rounds to
        // zero on a one-entity scene — a real number, not a stopped clock.
        let by_name = |name: &str| {
            sim.sequential
                .iter()
                .find(|t| t.name == name)
                .expect("every sequential entry is reported")
                .median_ms
        };
        assert!(by_name("world_hash") > 0.0);
        assert!(by_name("extract") > 0.0);
    }

    #[test]
    fn report_round_trips_through_json() {
        // Reports are artifacts tooling reads back (a committed baseline, an Amdahl split computed
        // off it), so the serialized shape must parse into the same types that wrote it.
        let report = sample_report();
        let text = serde_json::to_string(&report).unwrap();
        let parsed: ProfileReport = serde_json::from_str(&text).unwrap();
        assert_eq!(serde_json::to_string(&parsed).unwrap(), text);
    }

    #[test]
    fn report_shape_is_stable() {
        // The report serializes to a fixed field structure (numbers are not asserted — they are
        // wall-clock measurements). Constructed directly so the test needs no GPU.
        let report = ProfileReport {
            scene: "scenes/main.yaml".to_string(),
            warmup_frames: 5,
            measured_frames: 60,
            quality: None,
            sim: SimProfile {
                systems: vec![SystemTiming {
                    name: "bump".to_string(),
                    median_ms: 0.01,
                    tail: None,
                }],
                sequential: sample_sequential(),
                frame_total_median_ms: 0.02,
                frame_total_tail: None,
                threads: Some(1),
            },
            gpu: GpuProfile {
                available: true,
                timestamps_supported: true,
                passes: vec![PassTiming {
                    name: "scene".to_string(),
                    median_ms: Some(1.5),
                }],
            },
            transport: None,
            cluster: None,
            load: None,
            probes: None,
            samples: None,
        };

        let value: serde_json::Value = serde_json::to_value(&report).unwrap();
        // Top-level keys.
        for key in ["scene", "warmup_frames", "measured_frames", "sim", "gpu"] {
            assert!(value.get(key).is_some(), "missing top-level key `{key}`");
        }
        // `None` here serializes to an absent key, not `null` — the shape a committed baseline JSON
        // written before a section existed still has (`stress-*.json` and friends), and the
        // shape a live report without `include_samples` carries for `samples`. `transport` is also
        // this offline driver's real shape: it delivers no frames, so it never measures transport.
        // (`samples` and `cluster`, unlike `transport`, are *not* what `profile_engine` itself
        // produces any more — see `profile_engine_fills_raw_samples_offline` and
        // `profile_engine_fills_the_cluster_section_offline` below for those.)
        for key in ["quality", "transport", "cluster", "samples"] {
            assert!(
                value.get(key).is_none(),
                "`None` sections are omitted, not null: `{key}`"
            );
        }
        let reparsed: ProfileReport = serde_json::from_value(value.clone()).unwrap();
        assert!(
            reparsed.quality.is_none()
                && reparsed.transport.is_none()
                && reparsed.cluster.is_none()
                && reparsed.samples.is_none()
        );
        assert_eq!(serde_json::to_value(&reparsed).unwrap(), value);
        // Sim section shape.
        assert!(value["sim"]["frame_total_median_ms"].is_number());
        assert_eq!(value["sim"]["systems"][0]["name"], "bump");
        assert!(value["sim"]["systems"][0]["median_ms"].is_number());
        // Sequential section: the same `{name, median_ms}` shape, in the fixed order.
        for (i, name) in SEQUENTIAL_NAMES.iter().enumerate() {
            assert_eq!(value["sim"]["sequential"][i]["name"], *name);
            assert!(value["sim"]["sequential"][i]["median_ms"].is_number());
        }
        // GPU section shape.
        assert_eq!(value["gpu"]["available"], true);
        assert_eq!(value["gpu"]["timestamps_supported"], true);
        assert_eq!(value["gpu"]["passes"][0]["name"], "scene");
        assert!(value["gpu"]["passes"][0]["median_ms"].is_number());
    }

    #[test]
    fn median_handles_odd_even_and_empty() {
        assert_eq!(median_f64(&mut []), 0.0);
        assert_eq!(median_f64(&mut [3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median_f64(&mut [4.0, 1.0, 3.0, 2.0]), 2.5);
        let mut durs = [
            Duration::from_millis(2),
            Duration::from_millis(1),
            Duration::from_millis(3),
        ];
        assert_eq!(median_ms(&mut durs), 2.0);
    }

    #[test]
    fn profile_driver_reports_a_builder_registered_system() {
        // The driver is parameterized over the engine the caller built. Given a builder-made engine
        // that registered a game system (`bump`), profiling it must surface that system by name — the
        // capability the CLI's built-in-registry path structurally cannot have. GPU is not exercised
        // here (see `profile_engine`'s end-to-end use); this is the sim-side driver unit.
        let mut engine = counter_engine();
        engine.load_inline_scene(SCENE).unwrap();
        let (sim, samples) = profile_sim(&mut engine, 4, 1);
        assert_eq!(sim.systems.len(), 1);
        assert_eq!(sim.systems[0].name, "bump");
        assert!(sim.frame_total_median_ms >= 0.0);
        assert_eq!(samples.system_names, ["bump"]);
    }

    #[test]
    fn profile_sim_reports_the_physics_triple_for_a_physics_scene() {
        // The offline driver links no game code, but a scene with a `physics:` block gets the
        // engine-registered write/step/mirror triple, and `budgets/pinned-rtx3090-physics-stress.toml`
        // gates those names — an unnamed row would leave that budget checking nothing.
        let gallery = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery");
        let mut engine = Engine::with_project_root(&gallery);
        let source = std::fs::read_to_string(gallery.join("physics-pile.yaml"))
            .expect("the committed gallery scene reads");
        engine
            .load_inline_scene(&source)
            .expect("the committed gallery scene validates");
        let (sim, _samples) = profile_sim(&mut engine, 3, 1);
        let names: Vec<&str> = sim.systems.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                ludion_physics::WRITE,
                ludion_physics::STEP,
                ludion_physics::MIRROR
            ]
        );
    }

    #[test]
    fn profile_sim_reports_raw_per_frame_columns_in_measured_order() {
        // The `ProfileSamples` a `--perfetto` export builds a timeline from: frame indices, one
        // column per system, and the frame-total column, all in the order the frames actually ran —
        // proven here by driving a system whose cost strictly increases so the recorded values could
        // not be an artifact of the median passes' internal sorting.
        fn growing(world: &mut World, ctx: &ludion_ecs::SystemCtx) {
            // Busy-work growing geometrically with the tick (base * 4^tick): the last measured
            // frame does ~1024x the first's work, tens of ms against sub-millisecond, a gap that
            // scheduler noise or cold caches on a loaded box cannot invert (a linear ramp could be).
            // An xorshift step (not a running sum) so an opt-level-1 test build can't fold the loop
            // to a closed form; writing the result into the world keeps it from being dead code.
            let spins = 8000u64.saturating_mul(4u64.saturating_pow(ctx.frame.as_u64() as u32));
            let mut x = 0x9E37_79B9_7F4A_7C15u64 ^ ctx.frame.as_u64().wrapping_add(1);
            for _ in 0..spins {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
            }
            for (counter,) in world.query::<(&mut Counter,)>() {
                counter.n = x as i64;
            }
        }
        let mut engine = Engine::builder()
            .components(|reg| reg.register::<Counter>("Counter"))
            .system("growing", growing)
            .build()
            .unwrap();
        engine.load_inline_scene(SCENE).unwrap();

        let (sim, samples) = profile_sim(&mut engine, 6, 1);

        assert_eq!(samples.frames.len(), 6);
        // Ascending and contiguous: the driver's own tick counter, one tick per measured frame.
        for pair in samples.frames.windows(2) {
            assert_eq!(pair[1], pair[0] + 1);
        }
        assert_eq!(samples.system_names, ["growing"]);
        assert_eq!(samples.system_ms.len(), 1);
        assert_eq!(samples.system_ms[0].len(), 6);
        assert_eq!(samples.frame_total_ms.len(), 6);
        // The growing system's later columns must exceed its earlier ones — proof the column is the
        // measured order, not resorted (the median computed alongside it may sort its own copy, but
        // must not perturb this one).
        assert!(
            samples.system_ms[0][5] > samples.system_ms[0][0],
            "{:?}",
            samples.system_ms[0]
        );
        assert!(samples.frame_total_ms.iter().all(|ms| *ms >= 0.0));
        // Neither offline column this driver cannot fill: absent, not zero, for every frame.
        assert!(samples.extract_ms.iter().all(Option::is_none));
        assert!(samples.submit_ms.iter().all(Option::is_none));
        // The offline GPU path produces medians only — no per-frame pass columns to carry here.
        assert!(samples.pass_names.is_empty());
        assert!(samples.pass_ms.is_empty());
        assert_eq!(sim.systems[0].name, "growing");
    }

    #[test]
    fn needs_gpu_profile_engine_fills_raw_samples_offline() {
        // The gap this closes: the offline driver used to discard its raw per-frame numbers after
        // taking medians, so `report.samples` was always `None` and `ludion profile --scene ...
        // --perfetto ...` refused for lack of a timeline to build. It no longer does — the driver
        // fills `samples` itself, so a trace export needs no `--attach`.
        let mut engine = counter_engine();
        engine.load_inline_scene(SCENE).unwrap();
        let report = profile_engine(&mut engine, "inline".to_string(), 5, 1).unwrap();

        let samples = report.samples.expect("the offline driver fills samples");
        assert_eq!(samples.frames.len(), 5);
        assert_eq!(samples.system_names, ["bump"]);
        assert_eq!(samples.system_ms.len(), 1);
        assert_eq!(samples.system_ms[0].len(), 5);
        assert_eq!(samples.frame_total_ms.len(), 5);
        assert_eq!(samples.extract_ms, vec![None; 5]);
        assert_eq!(samples.submit_ms, vec![None; 5]);
        // The offline GPU path never carries a per-frame breakdown, adapter or not.
        assert!(samples.pass_names.is_empty());
        assert!(samples.pass_ms.is_empty());
        // Still absent: the offline driver delivers no frames, so it never measures transport.
        assert!(report.transport.is_none());
    }

    /// The report says what its GPU numbers were paid for, and it reads that off the host that was
    /// actually driven — not off a default. Without this, a measurement taken on a machine at `low`
    /// would be indistinguishable from one taken at `full`, which is the whole reason the section
    /// exists.
    #[test]
    fn needs_gpu_profile_engine_reports_the_hosts_own_quality() {
        let mut engine = counter_engine();
        engine.load_inline_scene(SCENE).unwrap();
        engine.set_quality(
            crate::resolve_startup_quality(Some("low"), None).expect("`low` is a preset name"),
        );
        let report = profile_engine(&mut engine, "inline".to_string(), 5, 1).unwrap();

        let quality = report.quality.expect("every capture names its quality");
        assert_eq!(quality.preset, "low");
        assert!(
            !quality.ssao
                && !quality.shafts
                && !quality.dof
                && !quality.gi_dynamic
                && !quality.shadows
        );
        // The axes are the *setting's*, not the build's: a report taken here must be recognizable
        // as not comparable with one taken at `full`.
        assert_eq!(
            quality.ssao_samples,
            ludion_renderer::QualityParams::LOW.ssao_samples,
        );
        assert_eq!(
            quality.shadow_map_size,
            ludion_renderer::QualityParams::LOW.shadow_map_size,
        );
        assert_ne!(
            quality.shadow_map_size,
            ludion_renderer::QualityParams::FULL.shadow_map_size,
            "the sun's map is a modulated axis too, so a cheap preset must not report the ceiling",
        );
    }

    #[test]
    fn needs_gpu_profile_engine_fills_the_cluster_section_offline() {
        // The mirror image of `transport`: the section only the *offline* driver can take, because
        // only it owns its own frame loop. It also needs no adapter — the binning is pure CPU
        // arithmetic over the extracted scene — so this runs identically on a headless host, which
        // no other GPU-adjacent number in the report does.
        let mut engine = counter_engine();
        engine.load_inline_scene(SCENE).unwrap();
        let report = profile_engine(&mut engine, "inline".to_string(), 5, 1).unwrap();

        let cluster = report
            .cluster
            .expect("the offline driver fills the cluster section");
        assert!(cluster.build_median_ms >= 0.0);
        // A scene with no `PointLight` bins nothing, over a grid whose cell count is a compile-time
        // constant — so the empty case is fully determined, and asserting it pins that an empty
        // scene reports "no lights" rather than the one padding element the buffers carry.
        assert_eq!(cluster.lights, 0);
        assert_eq!(cluster.occupied_cells, 0);
        assert_eq!(cluster.total_refs, 0);
        assert_eq!(cluster.max_list_len, 0);
        assert_eq!(cluster.mean_list_len, 0.0);
        assert!(cluster.cells > 0, "the grid always has cells");
    }

    /// The clustered-forward CPU build, measured **in context** — on a committed scene rather than
    /// on a synthetic light set (`Plan/clustered-forward.md` wave 2).
    ///
    /// `ludion-renderer`'s own `cluster_build_cost_at_the_stress_population` times a hand-built
    /// 500-light array, which is the right unit test and the wrong calibration: it cannot see what
    /// [`Engine::extract_render_scene`] really hands the renderer for
    /// `examples/gallery/lights-stress.yaml` — the actual positions, the actual camera, the actual
    /// [`PROFILE_SIZE`] extent. This closes that gap, and its output is where
    /// `budgets/pinned-rtx3090-lights-stress.toml`'s calibration prose gets both its numbers: the
    /// build's wall cost (decision 3's GPU-compute-build trigger is a measured 0.3 ms at this
    /// population) and the occupancy panel (the deferred tightening work's trigger is list bloat
    /// that measures on the `scene` pass).
    ///
    /// No GPU is touched — the cluster build is pure CPU arithmetic over the extracted scene, which
    /// is exactly why it can be measured independently of the pass it feeds. `#[ignore]`d for the
    /// same reason its renderer-side sibling is: a wall-clock assertion in the default lane is a
    /// flake waiting to happen.
    ///
    /// ```text
    /// cargo test --release -p ludion-engine --lib cluster_build_cost -- --ignored --nocapture
    /// ```
    // Sanctioned wall-clock site: this module is the crate's one measurement home (see the module
    // docs), and this measures a pure function of an already-extracted scene.
    #[allow(clippy::disallowed_methods)]
    #[test]
    #[ignore = "a timing measurement, not a gate — run with --release --ignored --nocapture"]
    fn cluster_build_cost_on_committed_scenes() {
        // The stress scene first, then the three-light scene the sibling budget is calibrated on: a
        // build cost that is not near-zero on the latter would mean the per-frame *fixed* work grew,
        // which is a different regression from the per-light work growing.
        for scene in ["lights-stress.yaml", "lights.yaml"] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/gallery")
                .join(scene);
            let mut engine =
                Engine::with_project_root(path.parent().expect("a scene has a parent"));
            let source = std::fs::read_to_string(&path).expect("the committed gallery scene reads");
            engine
                .load_inline_scene(&source)
                .expect("the committed gallery scene validates");
            let (width, height) = PROFILE_SIZE;
            let render_scene = engine
                .extract_render_scene(width, height)
                .expect("the committed gallery scene extracts");

            // One warm-up, then a run long enough that the clock's resolution is not the measurement.
            let mut stats = ludion_renderer::cluster_stats(&render_scene, PROFILE_SIZE);
            const RUNS: u32 = 200;
            let start = Instant::now();
            for _ in 0..RUNS {
                stats = ludion_renderer::cluster_stats(&render_scene, PROFILE_SIZE);
            }
            let per_build = start.elapsed().as_secs_f64() * 1_000.0 / f64::from(RUNS);

            println!(
                "{scene} @ {width}x{height}\n  \
                 cluster build {per_build:.4} ms for {} lights\n  \
                 refs {} | occupied {}/{} cells | max list {} | mean list (occupied) {:.2}",
                stats.lights,
                stats.total_refs,
                stats.occupied_cells,
                stats.cells,
                stats.max_list_len,
                stats.mean_list_len,
            );
        }
    }

    /// The particle share of `extract`, measured **in context** — on the committed stress scene
    /// rather than a synthetic emitter list (`Plan/particles-engine-and-editor.md` decision 9).
    ///
    /// Three scenes read as one comparison: the stress population, the three-emitter reference scene
    /// the sibling budget line would be sanity-checked against, and a no-particle baseline so the
    /// *particle share* of extract is the difference between the stress number and the baseline
    /// rather than the whole extract cost (extract also walks every mesh, light, and sprite in the
    /// scene). Each scene is read twice, on one thread and at `par::threads()`: the trigger this
    /// feeds — `ludion_core::par` over the emitter list — is built, so the pair is the sequential
    /// cost against the parallel one. The second trigger, a stateless vertex-shader evaluation, is
    /// not built; it waits on the parallel reading passing 1 ms at the stress population.
    ///
    /// ```text
    /// cargo test --release -p ludion-engine --lib particle_extract_cost -- --ignored --nocapture
    /// ```
    // Sanctioned wall-clock site: this module is the crate's one measurement home (see the module
    // docs), and this measures a pure function of an engine's own state (extract touches no wall
    // clock itself; the wrapping `Instant` pair is the sanctioned span).
    #[allow(clippy::disallowed_methods)]
    #[test]
    #[ignore = "a timing measurement, not a gate — run with --release --ignored --nocapture"]
    fn particle_extract_cost_on_the_stress_scene() {
        for scene in ["particles-stress.yaml", "particles.yaml", "lights.yaml"] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/gallery")
                .join(scene);
            let mut engine =
                Engine::with_project_root(path.parent().expect("a scene has a parent"));
            let source = std::fs::read_to_string(&path).expect("the committed gallery scene reads");
            engine
                .load_inline_scene(&source)
                .expect("the committed gallery scene validates");
            let (width, height) = PROFILE_SIZE;

            // One warm-up, then a run long enough that the clock's resolution is not the measurement.
            const RUNS: u32 = 200;
            for threads in [1, par::threads()] {
                let (per_extract, particles) = par::with_threads(threads, || {
                    let mut render_scene = engine
                        .extract_render_scene(width, height)
                        .expect("the committed gallery scene extracts");
                    let start = Instant::now();
                    for _ in 0..RUNS {
                        render_scene = engine
                            .extract_render_scene(width, height)
                            .expect("the committed gallery scene extracts");
                    }
                    (
                        start.elapsed().as_secs_f64() * 1_000.0 / f64::from(RUNS),
                        render_scene.particles.len(),
                    )
                });
                println!(
                    "{scene} @ {width}x{height}\n  extract {per_extract:.4} ms @ {threads} threads | {particles} particles",
                );
            }
        }
    }

    #[test]
    fn warmup_for_clamps() {
        assert_eq!(warmup_for(0), 1);
        assert_eq!(warmup_for(4), 1);
        assert_eq!(warmup_for(60), 15);
        assert_eq!(warmup_for(10_000), 30);
    }

    fn sample_report() -> ProfileReport {
        ProfileReport {
            scene: "scenes/main.yaml".to_string(),
            warmup_frames: 5,
            measured_frames: 60,
            quality: None,
            sim: SimProfile {
                systems: vec![SystemTiming {
                    name: "bump".to_string(),
                    median_ms: 0.40,
                    tail: None,
                }],
                sequential: sample_sequential(),
                frame_total_median_ms: 0.50,
                frame_total_tail: None,
                threads: Some(1),
            },
            gpu: GpuProfile {
                available: true,
                timestamps_supported: true,
                passes: vec![PassTiming {
                    name: "scene".to_string(),
                    median_ms: Some(3.0),
                }],
            },
            transport: None,
            cluster: None,
            load: None,
            probes: None,
            samples: None,
        }
    }

    /// A report carrying a cluster section: one build measured at 0.2 ms over a lightly-occupied
    /// grid. Kept beside [`sample_report`] (which has none) so the budget tests can exercise both
    /// halves of the skip rule.
    fn clustered_report(build_median_ms: f64) -> ProfileReport {
        ProfileReport {
            cluster: Some(ClusterProfile {
                build_median_ms,
                lights: 500,
                cells: 2304,
                occupied_cells: 794,
                total_refs: 17_121,
                max_list_len: 74,
                mean_list_len: 21.56,
            }),
            ..sample_report()
        }
    }

    /// A report carrying a load section with the given total and nothing else measured. Beside
    /// [`sample_report`] (which has none) so the budget tests can exercise both halves of `L4025`.
    fn loaded_report(total_ms: f64) -> ProfileReport {
        ProfileReport {
            load: Some(LoadProfile {
                scene_ms: 0.0,
                mesh_ms: 0.0,
                texture_ms: 0.0,
                other_ms: 0.0,
                gpu_upload_ms: None,
                first_frame_ms: None,
                voxelize_ms: None,
                renderer_init_ms: None,
                gi_bake_ms: None,
                probe_bake_ms: None,
                total_ms,
                assets: Vec::new(),
            }),
            ..sample_report()
        }
    }

    /// A report whose GPU section ran the *pyramid* bloom chain: the shared bright-pass plus the ten
    /// pyramid passes, each 0.01 ms, and no Gaussian blurs. Total bloom cost: 0.11 ms.
    fn pyramid_report() -> ProfileReport {
        let mut report = sample_report();
        let mut bloom = vec!["bloom_threshold".to_string()];
        bloom.extend((1..=5).map(|i| format!("bloom_down_{i}")));
        bloom.extend((0..=4).map(|i| format!("bloom_up_{i}")));
        report
            .gpu
            .passes
            .extend(bloom.into_iter().map(|name| PassTiming {
                name,
                median_ms: Some(0.01),
            }));
        report
    }

    #[test]
    fn tiny_budget_reports_every_over_budget_line() {
        let budget = Budget {
            frame_total_ms: Some(0.001),
            systems: BTreeMap::from([("bump".to_string(), 0.001)]),
            passes: BTreeMap::from([("scene".to_string(), 0.001)]),
            ..Budget::default()
        };
        let over = check_budget(&sample_report(), &budget);
        assert_eq!(
            over.len(),
            3,
            "frame total, one system, and one pass all exceed"
        );
        assert!(over.iter().all(|d| d.code == code::BUDGET_EXCEEDED));
    }

    #[test]
    fn generous_budget_passes() {
        let budget = Budget {
            frame_total_ms: Some(100.0),
            systems: BTreeMap::from([("bump".to_string(), 100.0)]),
            passes: BTreeMap::from([("scene".to_string(), 100.0)]),
            ..Budget::default()
        };
        assert!(check_budget(&sample_report(), &budget).is_empty());
    }

    #[test]
    fn producible_but_unmeasured_pass_is_not_a_violation() {
        // `overlay` (UI-only) and `shadow` (sun-only) are real frame-graph pass names — both absent
        // from `sample_report`'s single-pass GPU section, but producible, so they stay quiet. `shadow`
        // proves a sunless budget scene can carry a `[passes] shadow` line without an L4006.
        let budget = Budget {
            frame_total_ms: None,
            systems: BTreeMap::new(),
            passes: BTreeMap::from([("overlay".to_string(), 0.0), ("shadow".to_string(), 0.0)]),
            ..Budget::default()
        };
        assert!(check_budget(&sample_report(), &budget).is_empty());
    }

    #[test]
    fn gpu_lines_with_no_gpu_measurement_are_l4019() {
        let no_adapter = |mut report: ProfileReport| {
            report.gpu = GpuProfile {
                available: false,
                timestamps_supported: false,
                passes: Vec::new(),
            };
            report
        };
        let no_timestamps = |mut report: ProfileReport| {
            report.gpu.timestamps_supported = false;
            report.gpu.passes[0].median_ms = None;
            report
        };
        let budgets = [
            Budget {
                passes: BTreeMap::from([("scene".to_string(), 100.0)]),
                ..Budget::default()
            },
            Budget {
                post_groups: BTreeMap::from([("bloom".to_string(), 100.0)]),
                ..Budget::default()
            },
            Budget {
                post_total_ms: Some(100.0),
                ..Budget::default()
            },
        ];
        for budget in &budgets {
            for report in [no_adapter(sample_report()), no_timestamps(sample_report())] {
                let over = check_budget(&report, budget);
                assert_eq!(over.len(), 1, "{over:?}");
                assert_eq!(over[0].code, code::BUDGET_GPU_UNMEASURED);
            }
        }
        // A CPU-only budget does not care whether a GPU was there.
        let cpu_only = Budget {
            frame_total_ms: Some(100.0),
            ..Budget::default()
        };
        assert!(check_budget(&no_adapter(sample_report()), &cpu_only).is_empty());
    }

    #[test]
    fn load_total_under_the_ceiling_passes() {
        let budget = Budget {
            load_total_ms: Some(5000.0),
            ..Budget::default()
        };
        assert!(check_budget(&loaded_report(4999.0), &budget).is_empty());
    }

    #[test]
    fn load_total_over_the_ceiling_is_l4005() {
        let budget = Budget {
            load_total_ms: Some(5000.0),
            ..Budget::default()
        };
        let over = check_budget(&loaded_report(7123.4), &budget);
        assert_eq!(over.len(), 1, "{over:?}");
        assert_eq!(over[0].code, code::BUDGET_EXCEEDED);
        assert!(
            over[0].message.contains("load total"),
            "{}",
            over[0].message
        );
        assert!(
            over[0].message.contains("single run"),
            "{}",
            over[0].message
        );
    }

    #[test]
    fn load_line_against_a_report_with_no_load_is_l4025() {
        let budget = Budget {
            load_total_ms: Some(5000.0),
            ..Budget::default()
        };
        let over = check_budget(&sample_report(), &budget);
        assert_eq!(over.len(), 1, "{over:?}");
        assert_eq!(over[0].code, code::BUDGET_LOAD_UNMEASURED);
    }

    #[test]
    fn budget_without_a_load_line_ignores_a_report_with_no_load() {
        let budget = Budget {
            frame_total_ms: Some(100.0),
            ..Budget::default()
        };
        assert!(check_budget(&sample_report(), &budget).is_empty());
    }

    #[test]
    fn load_total_ms_parses_from_toml() {
        let budget: Budget = toml::from_str("load_total_ms = 5000.0\n").expect("parses");
        assert_eq!(budget.load_total_ms, Some(5000.0));
    }

    /// Every committed profile budget (`budgets/*.toml`, `games/*/budgets/*.toml`) parses under
    /// `deny_unknown_fields` and names only producible passes and groups, so a pass rename fails
    /// here on every push rather than on the next pinned-box run. The `image-stats-*` and `reflection-parity-*` files are a
    /// different schema.
    #[test]
    fn committed_budget_files_parse_and_name_producible_keys() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut dirs = vec![root.join("budgets")];
        for game in std::fs::read_dir(root.join("games")).expect("games/ exists") {
            dirs.push(game.expect("readable entry").path().join("budgets"));
        }
        let mut files = Vec::new();
        for dir in dirs.iter().filter(|dir| dir.is_dir()) {
            for entry in std::fs::read_dir(dir).expect("readable budgets dir") {
                let path = entry.expect("readable entry").path();
                let is_budget = path.extension().is_some_and(|ext| ext == "toml")
                    && !path.file_name().is_some_and(|name| {
                        let name = name.to_string_lossy();
                        name.starts_with("image-stats") || name.starts_with("reflection-parity")
                    });
                if is_budget {
                    files.push(path);
                }
            }
        }
        assert!(files.len() >= 10, "found only {files:?} — wrong root?");
        for path in files {
            let budget = load_budget(&path).unwrap_or_else(|d| panic!("{}: {d}", path.display()));
            // Every producible name measured at zero: only an unknown key can produce a diagnostic.
            let mut report = loaded_report(0.0);
            report.gpu.passes = KNOWN_PASS_NAMES
                .iter()
                .map(|name| PassTiming {
                    name: (*name).to_string(),
                    median_ms: Some(0.0),
                })
                .collect();
            report.sim.systems = budget
                .systems
                .keys()
                .chain(budget.systems_max.keys())
                .map(|name| SystemTiming {
                    name: name.clone(),
                    median_ms: 0.0,
                    tail: None,
                })
                .collect();
            let over = check_budget(&report, &budget);
            assert!(over.is_empty(), "{}: {over:?}", path.display());
        }
    }

    #[test]
    fn unproducible_system_name_is_l4006() {
        // "nonexistent" matches no system this engine ever registered — the misnamed-budget-key bug
        // (audit finding 1.6): a renamed/deleted/typo'd system must not silently pass.
        let budget = Budget {
            frame_total_ms: None,
            systems: BTreeMap::from([("nonexistent".to_string(), 0.0)]),
            passes: BTreeMap::new(),
            ..Budget::default()
        };
        let over = check_budget(&sample_report(), &budget);
        assert_eq!(over.len(), 1);
        assert_eq!(over[0].code, code::BUDGET_UNKNOWN_KEY);
        assert!(over[0].message.contains("nonexistent"));
    }

    #[test]
    fn unproducible_pass_name_is_l4006() {
        // Not one of the frame graph's fixed pass names — cannot ever be produced.
        let budget = Budget {
            frame_total_ms: None,
            systems: BTreeMap::new(),
            passes: BTreeMap::from([("nonexistent".to_string(), 0.0)]),
            ..Budget::default()
        };
        let over = check_budget(&sample_report(), &budget);
        assert_eq!(over.len(), 1);
        assert_eq!(over[0].code, code::BUDGET_UNKNOWN_KEY);
    }

    #[test]
    fn renamed_system_key_is_l4006_even_without_a_close_typo_match() {
        // The audit's concrete near-miss (games/vigil, G2): a system renamed `auto_fire` -> `fire`.
        // The two names are too far apart (5-character edit distance) for the typo-suggestion budget,
        // so no "nearest match" fires — but the stale key must still be flagged rather than silently
        // passing, which is the bug this fixes.
        let mut report = sample_report();
        report.sim.systems.push(SystemTiming {
            name: "fire".to_string(),
            median_ms: 0.01,
            tail: None,
        });
        let budget = Budget {
            frame_total_ms: None,
            systems: BTreeMap::from([("auto_fire".to_string(), 0.05)]),
            passes: BTreeMap::new(),
            ..Budget::default()
        };
        let over = check_budget(&report, &budget);
        assert_eq!(over.len(), 1);
        assert_eq!(over[0].code, code::BUDGET_UNKNOWN_KEY);
    }

    #[test]
    fn typo_system_key_suggests_the_nearest_match() {
        // A genuine typo (one transposed letter) is close enough for `suggest::nearest` to name the
        // likely fix in the help text.
        let budget = Budget {
            frame_total_ms: None,
            systems: BTreeMap::from([("bmup".to_string(), 0.05)]),
            passes: BTreeMap::new(),
            ..Budget::default()
        };
        let over = check_budget(&sample_report(), &budget);
        assert_eq!(over.len(), 1);
        assert_eq!(over[0].code, code::BUDGET_UNKNOWN_KEY);
        let help = over[0].help.as_deref().unwrap_or_default();
        assert!(
            help.contains("bump"),
            "help should name the nearest match `bump`: {help}"
        );
    }

    #[test]
    fn cluster_build_over_budget_is_l4005_and_under_is_quiet() {
        let over = check_budget(
            &clustered_report(0.2),
            &Budget {
                cluster_build_ms: Some(0.15),
                ..Budget::default()
            },
        );
        assert_eq!(over.len(), 1);
        assert_eq!(over[0].code, code::BUDGET_EXCEEDED);
        assert!(
            over[0].message.contains("cluster build"),
            "the message must name the line: {}",
            over[0].message
        );

        assert!(
            check_budget(
                &clustered_report(0.2),
                &Budget {
                    cluster_build_ms: Some(0.25),
                    ..Budget::default()
                }
            )
            .is_empty()
        );
    }

    #[test]
    fn a_report_without_a_cluster_section_skips_the_cluster_ceiling() {
        // The shape every live (`--attach`) capture has: the frames it measured built their cluster
        // tables inside a render, so nothing was measured here and nothing can be over budget. A
        // zero ceiling must stay quiet rather than fail — the same skip rule `[post_groups]` uses,
        // so one budget file gates the same line on the host that can answer for it and says
        // nothing on the one that cannot.
        assert!(sample_report().cluster.is_none());
        assert!(
            check_budget(
                &sample_report(),
                &Budget {
                    cluster_build_ms: Some(0.0),
                    ..Budget::default()
                }
            )
            .is_empty()
        );
    }

    #[test]
    fn extract_over_budget_is_l4005_and_under_is_quiet() {
        // `sample_sequential` gives every `SEQUENTIAL_NAMES` entry (including `extract`) a 0.01 ms
        // median, so `sample_report` already carries the entry this line checks.
        let over = check_budget(
            &sample_report(),
            &Budget {
                extract_ms: Some(0.005),
                ..Budget::default()
            },
        );
        assert_eq!(over.len(), 1);
        assert_eq!(over[0].code, code::BUDGET_EXCEEDED);
        assert!(
            over[0].message.contains("extract"),
            "the message must name the line: {}",
            over[0].message
        );

        assert!(
            check_budget(
                &sample_report(),
                &Budget {
                    extract_ms: Some(0.02),
                    ..Budget::default()
                }
            )
            .is_empty()
        );
    }

    #[test]
    fn a_report_without_an_extract_entry_skips_the_extract_ceiling() {
        // The shape a hand-built report without a `sim.sequential` section has: nothing was
        // measured, so nothing can be over budget — the same skip rule `cluster_build_ms` uses.
        let mut report = sample_report();
        report.sim.sequential.clear();
        assert!(
            check_budget(
                &report,
                &Budget {
                    extract_ms: Some(0.0),
                    ..Budget::default()
                }
            )
            .is_empty()
        );
    }

    #[test]
    fn unreadable_budget_file_is_l4004() {
        let diagnostic = load_budget(Path::new("does/not/exist.toml")).unwrap_err();
        assert_eq!(diagnostic.code, code::BUDGET_FILE_INVALID);
    }

    #[test]
    fn invalid_budget_toml_is_l4004() {
        let dir =
            std::env::temp_dir().join(format!("ludion-engine-budget-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad-budget.toml");
        // An unknown key trips `deny_unknown_fields`; malformed syntax would too.
        std::fs::write(&path, "frame_total = 1.0\n").unwrap();
        let diagnostic = load_budget(&path).unwrap_err();
        assert_eq!(diagnostic.code, code::BUDGET_FILE_INVALID);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn valid_budget_parses() {
        let text = "frame_total_ms = 1.0\npost_total_ms = 0.4\ncluster_build_ms = 0.25\nextract_ms = 0.5\n\n[systems]\nbump = 0.8\n\n[passes]\nscene = 6.0\ncomposite = 3.0\n\n[post_groups]\nbloom = 0.3\n";
        let budget: Budget = toml::from_str(text).unwrap();
        assert_eq!(budget.frame_total_ms, Some(1.0));
        assert_eq!(budget.post_total_ms, Some(0.4));
        assert_eq!(budget.cluster_build_ms, Some(0.25));
        assert_eq!(budget.extract_ms, Some(0.5));
        assert_eq!(budget.systems["bump"], 0.8);
        assert_eq!(budget.passes["scene"], 6.0);
        assert_eq!(budget.post_groups["bloom"], 0.3);
    }

    #[test]
    fn post_group_sums_its_measured_members() {
        // Eleven pyramid-chain passes at 0.01 ms each = 0.11 ms. A ceiling just above passes; the
        // same ceiling applied to any single member would pass trivially, so the sum is what is
        // being tested.
        let report = pyramid_report();
        let under = Budget {
            post_groups: BTreeMap::from([("bloom".to_string(), 0.111)]),
            ..Budget::default()
        };
        assert!(check_budget(&report, &under).is_empty());

        let over = check_budget(
            &report,
            &Budget {
                post_groups: BTreeMap::from([("bloom".to_string(), 0.109)]),
                ..Budget::default()
            },
        );
        assert_eq!(
            over.len(),
            1,
            "one diagnostic for the group, not per member"
        );
        assert_eq!(over[0].code, code::BUDGET_EXCEEDED);
        assert!(
            over[0].message.contains("post group `bloom`") && over[0].message.contains("11 passes"),
            "the message must read as an aggregate, not a single pass: {}",
            over[0].message
        );
    }

    #[test]
    fn post_group_with_no_measured_member_skips() {
        // `sample_report` measured only `scene` — the shape of a run with no GPU adapter, or of a
        // scene whose effect never ran. Nothing was measured, so nothing can be over budget: a zero
        // ceiling must stay quiet rather than fail.
        let budget = Budget {
            post_total_ms: Some(0.0),
            post_groups: BTreeMap::from([("bloom".to_string(), 0.0)]),
            ..Budget::default()
        };
        assert!(check_budget(&sample_report(), &budget).is_empty());
    }

    #[test]
    fn a_bare_or_out_of_range_pyramid_pass_key_is_l4006() {
        let key = |name: &str| Budget {
            passes: BTreeMap::from([(name.to_string(), 1.0)]),
            ..Budget::default()
        };
        for name in ["hiz_reduce", "ssr_color_reduce"] {
            let over = check_budget(&pyramid_report(), &key(name));
            assert_eq!(over.len(), 1, "{name}: {over:?}");
            assert_eq!(over[0].code, code::BUDGET_UNKNOWN_KEY);
            let help = over[0].help.as_deref().unwrap_or_default();
            assert!(
                help.contains("`ssr` post group") && help.contains(&format!("{name}_1")),
                "{name}: {help}"
            );
        }
        for name in ["hiz_reduce_99", "hiz_reduce_0", "ssr_color_reduce_32"] {
            let over = check_budget(&pyramid_report(), &key(name));
            assert_eq!(over.len(), 1, "{name}: {over:?}");
            assert_eq!(over[0].code, code::BUDGET_UNKNOWN_KEY);
        }
        assert!(check_budget(&pyramid_report(), &key("hiz_reduce_1")).is_empty());
    }

    #[test]
    fn unknown_post_group_name_is_l4006() {
        // Not one of the fixed group names — a rename, a deletion, or (here) a typo. Left
        // unchecked the ceiling would never fire again.
        let budget = Budget {
            post_groups: BTreeMap::from([("blom".to_string(), 0.0)]),
            ..Budget::default()
        };
        let over = check_budget(&pyramid_report(), &budget);
        assert_eq!(over.len(), 1);
        assert_eq!(over[0].code, code::BUDGET_UNKNOWN_KEY);
        assert!(over[0].message.contains("post group `blom`"));
        let help = over[0].help.as_deref().unwrap_or_default();
        assert!(
            help.contains("bloom"),
            "help should name the nearest group `bloom`: {help}"
        );
    }

    #[test]
    fn post_total_sums_every_group() {
        // With one group defined, the total equals it — but it is computed independently, over the
        // union of every group's members, and gets its own diagnostic.
        let report = pyramid_report();
        assert!(
            check_budget(
                &report,
                &Budget {
                    post_total_ms: Some(0.111),
                    ..Budget::default()
                }
            )
            .is_empty()
        );

        let over = check_budget(
            &report,
            &Budget {
                post_total_ms: Some(0.109),
                ..Budget::default()
            },
        );
        assert_eq!(over.len(), 1);
        assert_eq!(over[0].code, code::BUDGET_EXCEEDED);
        assert!(over[0].message.contains("post total"));
    }

    #[test]
    fn every_post_group_member_is_a_producible_pass() {
        // A group's members are `pass::*` constants, so this cannot drift by a typo — but it can
        // drift by a *deletion*: a pass removed from the frame graph leaves `KNOWN_PASS_NAMES` and
        // would leave a group silently summing one fewer thing.
        for (group, members) in POST_GROUPS {
            for member in members {
                assert!(
                    KNOWN_PASS_NAMES.contains(member),
                    "post group `{group}` names `{member}`, which is not a producible pass"
                );
            }
        }
    }

    /// Every pass any chain can schedule — each optional effect on, at a super-sampled and a
    /// sub-sampled render scale — is a name a budget may use. This is what keeps `KNOWN_PASS_NAMES`
    /// from drifting behind the frame graph.
    #[test]
    fn every_scheduled_pass_is_producible() {
        use ludion_renderer::{
            DebugView, FrameSize, PostChain, RenderScale, dump_frame_graph_json,
        };
        let everything = PostChain {
            ui: true,
            shadow_map: Some(1024),
            ssao: true,
            shafts: true,
            dof: true,
            gi_inject: Some([4, 4, 4]),
            debug_lines: true,
            ssr: Some(0.5),
            ssr_view: Some(DebugView::SsrConfidence),
            probe_view: true,
            probe_relight: true,
        };
        let sizes = [
            FrameSize::native(1280, 720),
            FrameSize::scaled(1280, 720, RenderScale::new(2.0).unwrap()),
            FrameSize::scaled(1280, 720, RenderScale::new(0.5).unwrap()),
        ];
        for size in sizes {
            for chain in [PostChain::default(), everything] {
                let json = dump_frame_graph_json(size, chain).expect("the graph schedules");
                let dump: serde_json::Value = serde_json::from_str(&json).unwrap();
                for pass in dump["passes"].as_array().expect("passes array") {
                    let name = pass["name"].as_str().expect("pass name");
                    assert!(
                        KNOWN_PASS_NAMES.iter().any(|m| pass_matches(m, name)),
                        "the frame graph schedules `{name}`, which no budget key can name"
                    );
                }
            }
        }
    }

    #[test]
    fn pass_matches_takes_a_level_suffix_only_on_mipped_kinds() {
        assert!(pass_matches("hiz_reduce", "hiz_reduce_3"));
        assert!(pass_matches("ssr_color_reduce", "ssr_color_reduce_12"));
        assert!(pass_matches("hiz_reduce", "hiz_reduce"));
        assert!(!pass_matches("hiz_reduce", "hiz_reduce_"));
        assert!(!pass_matches("hiz_reduce", "hiz_reduce_x"));
        assert!(!pass_matches("hiz_reduce", "hiz_reduce3"));
        assert!(!pass_matches("ssr_trace", "ssr_trace_1"));
    }

    #[test]
    fn the_ssr_group_sums_its_pyramid_levels_and_the_trace() {
        let mut report = sample_report();
        report.gpu.passes.extend(
            [
                "hiz_seed",
                "hiz_reduce_1",
                "hiz_reduce_2",
                "ssr_trace",
                "ssr_blend",
                "reflections",
            ]
            .map(|name| PassTiming {
                name: name.to_string(),
                median_ms: Some(0.5),
            }),
        );
        let (sum, counted) = measured_sum(
            &report,
            POST_GROUPS.iter().find(|(g, _)| *g == "ssr").unwrap().1,
        )
        .expect("measured");
        assert_eq!(counted, 5, "reflections is not in the group");
        assert!((sum - 2.5).abs() < 1e-9);

        let budget = |ms| Budget {
            post_groups: BTreeMap::from([("ssr".to_string(), ms)]),
            ..Budget::default()
        };
        assert!(check_budget(&report, &budget(3.0)).is_empty());
        let over = check_budget(&report, &budget(1.0));
        assert_eq!(over.len(), 1);
        assert_eq!(over[0].code, code::BUDGET_EXCEEDED);

        let by_level = Budget {
            passes: BTreeMap::from([("hiz_reduce_4".to_string(), 1.0)]),
            ..Budget::default()
        };
        assert!(check_budget(&report, &by_level).is_empty());
    }

    /// The `ssao` group spans the whole effect — the prepass included. The prepass is not itself a
    /// post pass, but it exists for exactly one reason and turning ambient occlusion on is what
    /// schedules it, so a budget that named only the two screen-space passes would put a ceiling on
    /// part of a cost and call it the effect's.
    #[test]
    fn the_ssao_group_budgets_the_whole_effect_including_its_prepass() {
        let (_, members) = POST_GROUPS
            .iter()
            .find(|(name, _)| *name == "ssao")
            .expect("the ssao group exists");
        assert_eq!(
            *members,
            [
                ludion_renderer::graph::pass::PREPASS,
                ludion_renderer::graph::pass::SSAO,
                ludion_renderer::graph::pass::SSAO_BLUR,
            ],
        );
        // And no one-member group beside it: `prepass` is budgetable as a `[passes]` line, which is
        // the one way to put a ceiling on one pass.
        assert!(
            !POST_GROUPS.iter().any(|(name, _)| *name == "prepass"),
            "a one-member `prepass` group would be a second way to budget one number",
        );
    }

    // ---- replay profile and tails ----

    struct Sink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn counter_builder() -> crate::EngineBuilder {
        Engine::builder()
            .components(|reg| reg.register::<Counter>("Counter"))
            .system("bump", bump)
    }

    /// A ten-tick recording of the counter scene, in memory.
    fn ten_tick_replay() -> Vec<u8> {
        let sink = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut live = counter_engine();
        live.load_inline_scene(SCENE).unwrap();
        live.set_recording(Sink(sink.clone())).unwrap();
        crate::drive_hashes(
            &mut live,
            vec![Command::SimStep(SimStepParams {
                ticks: 10,
                hash: false,
            })],
        )
        .unwrap();
        live.stop_recording().unwrap();
        sink.lock().unwrap().clone()
    }

    #[test]
    fn replay_profile_is_hash_neutral() {
        let bytes = ten_tick_replay();
        let mut plain = crate::replay_with(counter_builder(), &bytes, ".", |_, _| {
            Ok(std::ops::ControlFlow::Continue(()))
        })
        .unwrap();
        let (report, mut profiled) =
            profile_replay_engine(counter_builder(), &bytes, ".", 3, None, "run".into()).unwrap();

        assert_eq!(
            plain.world_hash().unwrap(),
            profiled.world_hash().unwrap(),
            "profiling a replay must not perturb the WorldHash"
        );
        assert_eq!(profiled.frame().as_u64(), 10);
        assert_eq!(report.sim.systems[0].name, "bump");
    }

    #[test]
    fn replay_profile_window_and_tails() {
        let bytes = ten_tick_replay();
        let report =
            profile_replay(counter_builder(), &bytes, ".", 3, Some(7), "run".into()).unwrap();

        assert_eq!(report.measured_frames, 5);
        assert_eq!(report.warmup_frames, 2);
        assert_eq!(report.scene, "run");
        assert!(!report.gpu.available);
        assert!(report.cluster.is_none());
        let samples = report.samples.as_ref().unwrap();
        assert_eq!(samples.frames, vec![3, 4, 5, 6, 7]);

        let mut timings: Vec<&SystemTiming> = report.sim.systems.iter().collect();
        timings.extend(&report.sim.sequential);
        assert!(!timings.is_empty());
        for timing in timings {
            let tail = timing
                .tail
                .as_ref()
                .expect("a replay profile fills every tail");
            assert!(
                (3..=7).contains(&tail.max_frame),
                "{}: {tail:?}",
                timing.name
            );
            assert!(timing.median_ms <= tail.p99_ms && tail.p99_ms <= tail.max_ms);
        }
        let total = report.sim.frame_total_tail.as_ref().unwrap();
        assert!((3..=7).contains(&total.max_frame));
        assert!(report.sim.frame_total_median_ms <= total.p99_ms && total.p99_ms <= total.max_ms);
    }

    #[test]
    fn replay_profile_window_out_of_range_is_l3021() {
        let bytes = ten_tick_replay();
        for (from, to) in [(3, Some(11)), (8, Some(7))] {
            let error = profile_replay(counter_builder(), &bytes, ".", from, to, "run".into())
                .expect_err("the window is outside the recording");
            assert_eq!(error.code, code::REPLAY_FRAME_WINDOW);
            assert!(
                error.message.contains("profile-replay"),
                "{}",
                error.message
            );
        }
    }

    #[test]
    fn replay_profile_window_measuring_no_frame_is_l3021() {
        let bytes = ten_tick_replay();
        let error = profile_replay(counter_builder(), &bytes, ".", 0, Some(0), "run".into())
            .expect_err("frame 0 is the loaded world; no tick produces it");
        assert_eq!(error.code, code::REPLAY_FRAME_WINDOW);
        assert!(
            error.message.contains("measured no frame"),
            "{}",
            error.message
        );
    }

    #[test]
    fn profiling_zero_frames_of_a_scene_is_l3021() {
        let mut engine = counter_engine();
        engine.load_inline_scene(SCENE).unwrap();
        let errors = profile_engine(&mut engine, "inline".into(), 0, 1)
            .expect_err("a zero-frame profile has no column to reduce");
        assert_eq!(errors[0].code, code::REPLAY_FRAME_WINDOW);
    }

    /// A scratch directory holding the scenes a recording loads.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ludion-profile-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("reload.yaml"), SCENE).unwrap();
        dir
    }

    /// Records `commands` against the counter scene, in memory.
    fn record_counter(dir: &Path, commands: Vec<Command>) -> Vec<u8> {
        let sink = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut live = counter_builder().project_root(dir).build().unwrap();
        live.load_inline_scene(SCENE).unwrap();
        live.set_recording(Sink(sink.clone())).unwrap();
        crate::drive_hashes(&mut live, commands).unwrap();
        live.stop_recording().unwrap();
        sink.lock().unwrap().clone()
    }

    fn step(ticks: u64) -> Command {
        Command::SimStep(SimStepParams { ticks, hash: false })
    }

    fn scene_load() -> Command {
        Command::SceneLoad(ludion_protocol::command::SceneLoadParams {
            path: "reload.yaml".into(),
        })
    }

    #[test]
    fn a_scene_load_after_a_tick_refuses_the_frame_windowed_tools() {
        let dir = scratch("load-after-tick");
        let bytes = record_counter(&dir, vec![step(3), scene_load(), step(3)]);

        let error = profile_replay(counter_builder(), &bytes, &dir, 1, None, "run".into())
            .expect_err("the load resets the frame mid-replay");
        assert_eq!(error.code, code::REPLAY_FRAME_WINDOW);
        assert!(
            error.message.contains("scene.load") && error.message.contains("drive action #"),
            "{}",
            error.message
        );

        let error = crate::replay_render_frames(
            counter_builder(),
            &bytes,
            &dir,
            crate::ReplayFramesRequest {
                from: 0,
                to: None,
                width: 64,
                height: 64,
            },
            |_, _| panic!("a refused replay renders nothing"),
        )
        .expect_err("replay-to-frames refuses it too");
        assert_eq!(error.code, code::REPLAY_FRAME_WINDOW);
    }

    #[test]
    fn a_scene_load_before_the_first_tick_still_profiles() {
        let dir = scratch("load-before-tick");
        let bytes = record_counter(&dir, vec![scene_load(), step(4)]);
        let report = profile_replay(counter_builder(), &bytes, &dir, 1, None, "run".into())
            .expect("a load at frame 0 resets 0 to 0");
        assert!(report.measured_frames > 0);
    }

    #[test]
    fn an_engine_restore_refuses_the_frame_windowed_tools() {
        let dir = scratch("restore");
        let sink = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut live = counter_builder().project_root(&dir).build().unwrap();
        live.load_inline_scene(SCENE).unwrap();
        live.set_recording(Sink(sink.clone())).unwrap();
        crate::drive_hashes(&mut live, vec![step(2)]).unwrap();
        let ludion_protocol::ResponseBody::Snapshot { snapshot } =
            live.execute(Command::EngineSnapshot).unwrap().body
        else {
            panic!("expected a snapshot");
        };
        crate::drive_hashes(
            &mut live,
            vec![
                Command::EngineRestore(ludion_protocol::command::EngineRestoreParams { snapshot }),
                step(2),
            ],
        )
        .unwrap();
        live.stop_recording().unwrap();
        let bytes = sink.lock().unwrap().clone();

        let error = profile_replay(counter_builder(), &bytes, &dir, 1, None, "run".into())
            .expect_err("a restore jumps the frame");
        assert_eq!(error.code, code::REPLAY_FRAME_WINDOW);
        assert!(
            error.message.contains("engine.restore"),
            "{}",
            error.message
        );
    }

    /// Two `sim.step` records whose ticks overflow `u64` when summed: hand-written, because the
    /// recorder can never produce them.
    #[test]
    fn step_records_totalling_more_than_u64_are_l3010_not_an_overflow() {
        use ludion_protocol::replay::{
            REPLAY_FORMAT_VERSION, ReplayHeader, ReplayWriter, SceneRef,
        };
        let header = ReplayHeader {
            format_version: REPLAY_FORMAT_VERSION,
            engine_version: "0.0.0".into(),
            protocol_version: ludion_protocol::PROTOCOL_VERSION,
            schema_hash: ludion_protocol::schema::schema_hash(),
            scene: SceneRef::Inline {
                document: SCENE.into(),
            },
            seed: 7,
            profile: None,
            render_extent: None,
        };
        let mut writer = ReplayWriter::new(Vec::new(), &header).unwrap();
        let huge = Command::SimStep(SimStepParams {
            ticks: u64::MAX / 2 + 1,
            hash: false,
        });
        writer
            .write_command(ludion_core::time::FrameIndex::ZERO, &huge)
            .unwrap();
        writer
            .write_command(ludion_core::time::FrameIndex::ZERO, &huge)
            .unwrap();
        let bytes = writer.into_inner().unwrap();

        let error = profile_replay(counter_builder(), &bytes, ".", 0, Some(0), "run".into())
            .expect_err("the total does not fit a frame index");
        assert_eq!(error.code, code::REPLAY_FORMAT);
    }

    const SWITCH_START: &str = "\
format: 1
seed: 7
entities:
  - name: player
    components:
      Transform: {}
";

    const SWITCH_NEXT: &str = "\
format: 1
seed: 9
entities:
  - name: gate-from-start
    components:
      Transform: {}
  - name: rider
    components:
      Transform: {}
      PathFollower: {}
";

    /// Requests a switch to `next.yaml` on frame 2, carrying the `player` entity.
    fn switch_on_frame_two(world: &mut World, ctx: &ludion_ecs::SystemCtx) {
        if ctx.frame.as_u64() != 2 {
            return;
        }
        let player = world.entities().find(|&id| {
            world
                .get::<crate::components::Name>(id)
                .ok()
                .flatten()
                .is_some_and(|name| name.0 == "player")
        });
        if let (Some(player), Some(switch)) = (
            player,
            world.resource_mut::<crate::transition::SceneSwitch>(),
        ) {
            switch.request(
                "next.yaml".to_string(),
                "gate-from-start".to_string(),
                vec![player],
            );
        }
    }

    /// A schedule rebuilt mid-run (the district switch adds `path_follow`) must not shift one
    /// system's samples onto another: the late system gets its own column.
    #[test]
    fn a_schedule_rebuilt_mid_replay_keeps_each_systems_samples() {
        let dir = scratch("schedule-rebuild");
        std::fs::write(dir.join("next.yaml"), SWITCH_NEXT).unwrap();
        let game = || {
            Engine::builder()
                .system("switch_on_frame_two", switch_on_frame_two)
                .project_root(&dir)
        };
        let sink = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut live = game().build().unwrap();
        live.load_inline_scene(SWITCH_START).unwrap();
        live.set_recording(Sink(sink.clone())).unwrap();
        crate::drive_hashes(&mut live, vec![step(6)]).unwrap();
        live.stop_recording().unwrap();
        let bytes = sink.lock().unwrap().clone();

        let report = profile_replay(game(), &bytes, &dir, 1, Some(6), "run".into()).unwrap();
        let samples = report.samples.as_ref().unwrap();
        assert_eq!(samples.frames, vec![1, 2, 3, 4, 5, 6]);
        assert!(
            samples
                .system_ms
                .iter()
                .all(|row| row.len() == samples.frames.len()),
            "the raw matrix stays dense over the measured frames"
        );

        let at = |name: &str| {
            samples
                .system_names
                .iter()
                .position(|n| n == name)
                .unwrap_or_else(|| panic!("`{name}` has no column: {:?}", samples.system_names))
        };
        let switch = report
            .sim
            .systems
            .iter()
            .find(|timing| timing.name == "switch_on_frame_two")
            .unwrap();
        assert!(switch.tail.is_some());
        let follow = report
            .sim
            .systems
            .iter()
            .find(|timing| timing.name == "path_follow")
            .expect("path_follow joined the schedule after the switch");
        assert!(
            follow.tail.as_ref().unwrap().max_frame >= 3,
            "{:?}",
            follow.tail
        );
        // Before the switch landed, `path_follow` did not run: those frames read 0.0.
        let row = &samples.system_ms[at("path_follow")];
        assert_eq!(row[0], 0.0);
        assert_eq!(row[1], 0.0);
        assert!(samples.system_ms[at("switch_on_frame_two")][0] > 0.0);
    }

    #[test]
    fn tail_p99_nearest_rank() {
        // 100 distinct samples 1.0..=100.0 ms, shuffled so the worst sits mid-run.
        let mut column: Vec<f64> = (1..=100).map(f64::from).collect();
        column.swap(0, 41);
        column.swap(60, 99);
        let frames: Vec<u64> = (500..600).collect();
        let tail = tail(&column, &frames).unwrap();
        // Nearest rank: the 99th smallest of 100.
        assert_eq!(tail.p99_ms, 99.0);
        assert_eq!(tail.max_ms, 100.0);
        assert_eq!(tail.max_frame, 500 + 60);

        assert_eq!(super::tail(&[], &[]), None);
        let one = super::tail(&[2.5], &[9]).unwrap();
        assert_eq!((one.max_ms, one.p99_ms, one.max_frame), (2.5, 2.5, 9));
    }

    fn tailed(max_ms: f64, max_frame: u64) -> Option<TimingTail> {
        Some(TimingTail {
            max_ms,
            max_frame,
            p99_ms: max_ms,
        })
    }

    #[test]
    fn system_max_over_budget_is_l4005_with_frame() {
        let mut report = sample_report();
        report.sim.systems[0].tail = tailed(0.9, 412);
        report.sim.frame_total_tail = tailed(2.0, 77);
        let budget = Budget {
            frame_total_max_ms: Some(1.0),
            systems_max: BTreeMap::from([("bump".to_string(), 0.5)]),
            ..Budget::default()
        };
        let over = check_budget(&report, &budget);
        assert_eq!(over.len(), 2);
        assert!(over.iter().all(|d| d.code == code::BUDGET_EXCEEDED));
        assert!(
            over.iter().any(|d| d.message.contains("`bump`")
                && d.message.contains("frame 412")
                && d.message.contains("max")),
            "{over:?}"
        );
        assert!(
            over.iter().any(|d| d.message.contains("frame 77")),
            "{over:?}"
        );

        let roomy = Budget {
            frame_total_max_ms: Some(3.0),
            systems_max: BTreeMap::from([("bump".to_string(), 1.0)]),
            ..Budget::default()
        };
        assert!(check_budget(&report, &roomy).is_empty());
    }

    #[test]
    fn systems_max_skipped_without_tail() {
        // `sample_report` carries no tails, the shape of a live capture.
        let budget = Budget {
            frame_total_max_ms: Some(0.0),
            systems_max: BTreeMap::from([("bump".to_string(), 0.0)]),
            ..Budget::default()
        };
        assert!(check_budget(&sample_report(), &budget).is_empty());
    }

    #[test]
    fn unknown_systems_max_key_is_l4006() {
        let budget = Budget {
            systems_max: BTreeMap::from([("bmup".to_string(), 1.0)]),
            ..Budget::default()
        };
        let over = check_budget(&sample_report(), &budget);
        assert_eq!(over.len(), 1);
        assert_eq!(over[0].code, code::BUDGET_UNKNOWN_KEY);
    }
}
