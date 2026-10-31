//! [`assert_deterministic`] — run a test body twice over fresh engines and, on a mismatch, say *where*:
//! the first frame, the first system, and a capped component diff, instead of two dumped hash vectors.
//!
//! It watches the engine through [`TickObserver`], which sees every tick the body causes, so the body
//! is plain `&mut Engine` code (`step`, `inject`, `world_mut` edits, scene loads).

use std::sync::{Arc, Mutex, MutexGuard};

use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::FrameIndex;
use ludion_ecs::{Registry, World};
use ludion_engine::{BisectReport, Engine, TickObserver, WorldCapture, world_diff};
use ludion_sim::WorldHash;

/// Where inside the divergent tick the two worlds are compared.
#[derive(Clone, Copy, PartialEq)]
enum Boundary {
    /// Before the tick's first system: the worlds already differed as the tick began.
    Pre,
    /// After the system at this index in registration order.
    System(usize),
    /// After the schedule: the tick's frame-boundary drains.
    End,
}

/// What a run records. `Hash` is the cheap pass; the other two re-run the divergent tick only.
#[derive(Clone, Copy)]
enum Mode {
    Hash,
    Probe { tick: usize },
    Capture { tick: usize, at: Boundary },
}

#[derive(Default)]
struct RunLog {
    ticks: Vec<(FrameIndex, WorldHash)>,
    pre_tick: Option<WorldHash>,
    systems: Vec<(&'static str, WorldHash)>,
    capture: Option<WorldCapture>,
    error: Option<Diagnostic>,
}

impl RunLog {
    fn keep<T>(&mut self, result: Result<T, Diagnostic>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.error.get_or_insert(error);
                None
            }
        }
    }
}

struct Recorder {
    mode: Mode,
    log: Arc<Mutex<RunLog>>,
    tick: usize,
    system: usize,
}

impl Recorder {
    fn log(&self) -> MutexGuard<'_, RunLog> {
        self.log.lock().expect("the run log is never poisoned")
    }
}

impl TickObserver for Recorder {
    fn before_tick(&mut self, _: FrameIndex, world: &World, registry: &Registry) -> bool {
        self.system = 0;
        let ordinal = self.tick;
        match self.mode {
            Mode::Hash => false,
            Mode::Probe { tick } if tick == ordinal => {
                let mut log = self.log();
                let hash = log.keep(WorldHash::compute(world, registry));
                log.pre_tick = hash;
                true
            }
            Mode::Capture { tick, at } if tick == ordinal => {
                let mut log = self.log();
                if at == Boundary::Pre {
                    let capture = log.keep(WorldCapture::capture(world, registry));
                    log.capture = capture;
                }
                matches!(at, Boundary::System(_))
            }
            _ => false,
        }
    }

    fn after_system(&mut self, system: &'static str, world: &World, registry: &Registry) {
        let index = self.system;
        self.system += 1;
        match self.mode {
            Mode::Probe { .. } => {
                let mut log = self.log();
                if let Some(hash) = log.keep(WorldHash::compute(world, registry)) {
                    log.systems.push((system, hash));
                }
            }
            Mode::Capture {
                at: Boundary::System(at),
                ..
            } if at == index => {
                let mut log = self.log();
                let capture = log.keep(WorldCapture::capture(world, registry));
                log.capture = capture;
            }
            _ => {}
        }
    }

    fn after_tick(&mut self, frame: FrameIndex, world: &World, registry: &Registry) {
        let ordinal = self.tick;
        self.tick += 1;
        match self.mode {
            Mode::Hash => {
                let mut log = self.log();
                if let Some(hash) = log.keep(WorldHash::compute(world, registry)) {
                    log.ticks.push((frame, hash));
                }
            }
            Mode::Capture {
                tick,
                at: Boundary::End,
            } if tick == ordinal => {
                let mut log = self.log();
                let capture = log.keep(WorldCapture::capture(world, registry));
                log.capture = capture;
            }
            _ => {}
        }
    }
}

struct Run {
    engine: Engine,
    log: RunLog,
    pre_body: Option<(WorldHash, WorldCapture)>,
}

/// One fresh engine, one body, observed in `mode`. A hash or capture failure surfaces as a panic
/// after the body has finished.
fn run(build: &impl Fn() -> Engine, body: &impl Fn(&mut Engine), mode: Mode) -> Run {
    let mut engine = build();
    let log = Arc::new(Mutex::new(RunLog::default()));
    let pre_body = matches!(mode, Mode::Hash).then(|| {
        let hash = WorldHash::compute(engine.world(), engine.registry()).unwrap_or_else(|e| {
            panic!("assert_deterministic: hashing the built world failed: {e}")
        });
        let capture =
            WorldCapture::capture(engine.world(), engine.registry()).unwrap_or_else(|e| {
                panic!("assert_deterministic: reading the built world failed: {e}")
            });
        (hash, capture)
    });
    engine.set_tick_observer(Recorder {
        mode,
        log: log.clone(),
        tick: 0,
        system: 0,
    });
    body(&mut engine);
    engine.clear_tick_observer();
    let mut log = std::mem::take(&mut *log.lock().expect("the run log is never poisoned"));
    if let Some(error) = log.error.take() {
        panic!("assert_deterministic: reading the world failed: {error}");
    }
    Run {
        engine,
        log,
        pre_body,
    }
}

/// Runs `body` over two fresh engines from `build` and panics if they diverge, naming the frame, the
/// first system after which the worlds differ, and a capped component diff.
///
/// The engine is observed on every tick the body causes (`sim.step`, the live loop), so the body is
/// ordinary `&mut Engine` code. The report never says which body call preceded a divergence; when the
/// worlds already differ as the divergent tick begins it says so (the body's own edits or commands
/// before that step), and a run that merely stops sooner is its own failure.
///
/// A passing check runs the body twice. A failure costs 6 runs: 2 to find the divergent tick, 2 to
/// find the system, 2 to capture the worlds there — so `build` and `body` must be repeatable.
///
/// # Panics
/// On any divergence, as above; also if a world cannot be hashed or read (`L0007`).
///
/// # Example
/// ```
/// use ludion_engine::EngineBuilder;
/// use ludion_testkit::{assert_deterministic, loaded_engine, step};
///
/// assert_deterministic(
///     || loaded_engine(EngineBuilder::new(), "format: 1\nentities: []\n"),
///     |engine| step(engine, 5),
/// );
/// ```
pub fn assert_deterministic(build: impl Fn() -> Engine, body: impl Fn(&mut Engine)) {
    let a = run(&build, &body, Mode::Hash);
    let b = run(&build, &body, Mode::Hash);

    let (a_pre, a_capture) = a
        .pre_body
        .as_ref()
        .expect("the hash pass captures the build");
    let (b_pre, b_capture) = b
        .pre_body
        .as_ref()
        .expect("the hash pass captures the build");
    if a_pre != b_pre {
        panic!(
            "assert_deterministic: build() is not deterministic — two fresh engines differ before \
             the body ran\n{}",
            a_capture.diff(b_capture)
        );
    }

    let (ticks_a, ticks_b) = (&a.log.ticks, &b.log.ticks);
    if let Some(tick) = ticks_a.iter().zip(ticks_b).position(|(l, r)| l != r) {
        localize(&build, &body, tick, ticks_a[tick].0);
    }
    if ticks_a.len() != ticks_b.len() {
        let (short_name, short, long_name, long) = if ticks_b.len() < ticks_a.len() {
            ("B", &b, "A", &a)
        } else {
            ("A", &a, "B", &b)
        };
        panic!(
            "assert_deterministic: run {short_name} stopped at frame {} after {} ticks; run \
             {long_name} ran {} ticks to frame {}. The runs agree on every tick both ran.",
            short.engine.frame().as_u64(),
            short.log.ticks.len(),
            long.log.ticks.len(),
            long.engine.frame().as_u64(),
        );
    }
    let a_end = WorldHash::compute(a.engine.world(), a.engine.registry());
    let b_end = WorldHash::compute(b.engine.world(), b.engine.registry());
    if a_end.ok() != b_end.ok() {
        let diff = world_diff(&a.engine, &b.engine).map_or_else(
            |e| format!("  (reading the worlds failed: {e})"),
            |d| d.to_string(),
        );
        panic!(
            "assert_deterministic: the runs agree on every tick but end in different worlds — the \
             body changed state after its last step\n{diff}"
        );
    }
}

/// Re-runs the divergent `tick` to find the first differing system, then captures both worlds there,
/// and panics with the report. Never returns.
fn localize(
    build: &impl Fn() -> Engine,
    body: &impl Fn(&mut Engine),
    tick: usize,
    frame: FrameIndex,
) -> ! {
    let a = run(build, body, Mode::Probe { tick });
    let b = run(build, body, Mode::Probe { tick });
    let boundary = if a.log.pre_tick != b.log.pre_tick {
        Boundary::Pre
    } else {
        a.log
            .systems
            .iter()
            .zip(&b.log.systems)
            .position(|(l, r)| l != r)
            .map_or(Boundary::End, Boundary::System)
    };
    let a_capture = run(build, body, Mode::Capture { tick, at: boundary })
        .log
        .capture;
    let b_capture = run(build, body, Mode::Capture { tick, at: boundary })
        .log
        .capture;
    let (Some(a_capture), Some(b_capture)) = (a_capture, b_capture) else {
        panic!(
            "assert_deterministic: the runs diverge at frame {} but the divergent tick did not \
             reproduce on re-run — the body or build() is nondeterministic across runs",
            frame.as_u64()
        );
    };
    let (system, systems_probed) = match boundary {
        Boundary::Pre => (None, 0),
        Boundary::System(index) => (
            Some(a.log.systems[index].0.to_string()),
            a.log.systems.len(),
        ),
        Boundary::End => (None, a.log.systems.len()),
    };
    let report = BisectReport {
        left: "run A".to_string(),
        right: "run B".to_string(),
        frame,
        system,
        systems_probed,
        world: a_capture.diff(&b_capture),
    };
    let note = if boundary == Boundary::Pre {
        "\n  the worlds already differed as the tick began: the body's own edits or commands before \
         that step diverged"
    } else {
        ""
    };
    panic!("assert_deterministic: two runs of the body diverge{note}\n{report}");
}
