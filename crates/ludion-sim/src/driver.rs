//! The fixed-timestep tick loop driver.
//!
//! [`SimDriver`] owns the [`World`] outright — no `Arc`, no `Mutex`, no sharing. Everything that
//! reads or writes the world does so through the driver (or through the command queue that feeds
//! it, one layer up in `ludion-engine`), which is the structural form of the "sim thread solely
//! owns `World`" rule from the architecture doc §3.
//!
//! # What a tick is (normative order)
//!
//! [`advance`](SimDriver::advance) performs at most one tick. A tick of frame *N* is exactly:
//!
//! 1. **Inputs** — clear the [`InputState`] edge sets, then apply every scripted event addressed
//!    to frame *N*, in script order.
//!    Then, if the world holds an [`Actions`] resource, resolve it against the fresh [`InputState`]
//!    (gestures advance one tick; a world without one is untouched).
//! 2. **Gameplay clock** — fold the [`GameClock`] resource, if the world carries one: a frame the
//!    game marked frozen becomes one more held frame, and the fold reports whether gameplay time
//!    advanced into this tick. A world with no clock behaves as an unheld one.
//! 3. **Systems** — run the [`Schedule`] once, in registration order, with the context of frame
//!    *N* read through the folded clock (`SystemCtx::new(N, rate).with_clock(…)`).
//! 4. **Advance** — the frame counter becomes *N + 1*.
//!
//! A system that panics is caught by the schedule, which skips the rest of the tick's systems. The
//! driver removes the resulting [`SystemPanic`] right after step 3, renders it as the `L4007`
//! [`Diagnostic`], pauses, and holds the diagnostic until [`take_system_panic`](SimDriver::take_system_panic)
//! — so a bare driver never keeps ticking a world a system left half-written, and a caller cannot
//! miss the failure by not looking.
//!
//! Time enters only as `SimTime`/`FrameIndex` derived from the tick count; there is no wall clock
//! anywhere in this crate (the clippy denylist makes that a build error). Pacing a *live* run —
//! sleeping between `advance` calls according to [`Speed`] — is the embedder's job; headless
//! callers just call `advance` in a tight loop and get an as-fast-as-possible, bit-identical run.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::rng::{DeterministicRng, WorldSeed};
use ludion_core::time::{FrameIndex, GameClock, SimTime, TickRate};
use ludion_ecs::{Schedule, ScheduleObserver, SystemCtx, SystemPanic, World};

use crate::actions::Actions;
use crate::input::{InputEvent, InputScript, InputState};
use crate::run_state::{RunState, Speed};

/// What, if anything, watches a tick's systems — the private argument threaded from the three
/// `advance*` entry points down to [`SimDriver::do_tick`]. All three variants run the same systems
/// in the same order; only the watching differs.
enum Watch<'a> {
    /// A plain tick ([`SimDriver::advance`]).
    None,
    /// Name-only boundaries ([`SimDriver::advance_observed`]).
    Observer(&'a mut dyn ScheduleObserver),
    /// Per-system state ([`SimDriver::advance_probed`]).
    Probe(&'a mut dyn FnMut(&'static str, &World)),
}

/// Deterministic run parameters: everything that must be recorded to reproduce a run.
///
/// A `(SimConfig, initial world, schedule)` triple fully determines every tick — this struct is
/// what a replay header captures.
#[derive(Clone, Debug, Default)]
pub struct SimConfig {
    /// The fixed tick rate (60 Hz by default). `dt` is `1 / tick_rate` and never varies.
    pub tick_rate: TickRate,
    /// Seed for the world's single [`DeterministicRng`] resource.
    pub seed: u64,
    /// The input script; events apply at their addressed frame's tick boundary.
    pub input: InputScript,
}

/// An [`SimDriver::inject_input`] addressed to a frame that has already run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PastFrameInject {
    /// The frame the event was addressed to.
    pub requested: FrameIndex,
    /// The next frame to tick.
    pub current: FrameIndex,
}

/// The exact driver state a snapshot restores — everything [`SimDriver::new`] would otherwise
/// reseed or reset. Consumed by [`SimDriver::restore`].
pub struct DriverRestore {
    /// The RNG state at the snapshot point (inserted verbatim; never reseeded).
    pub rng: DeterministicRng,
    /// The run's seed, re-inserted as the [`WorldSeed`] resource. Distinct from `rng`: the RNG
    /// carries an *advanced* state the seed cannot be read back out of, and derived streams need
    /// the seed as a value. A restore continues the same run, so this is the seed the run started
    /// with, not a fresh one.
    pub world_seed: u64,
    /// The frame the restored run resumes at.
    pub frame: FrameIndex,
    /// The run-state (running / paused / stepping) to resume in.
    pub run_state: RunState,
    /// The wall-clock pacing factor to resume with.
    pub speed: Speed,
    /// The fixed tick rate (must match the run being restored).
    pub tick_rate: TickRate,
}

/// Owns the [`World`] and advances it deterministically at a fixed timestep.
///
/// # Example
/// ```
/// use ludion_ecs::{Schedule, SystemCtx, World};
/// use ludion_sim::{SimConfig, SimDriver};
///
/// fn count(world: &mut World, _ctx: &SystemCtx) {
///     *world.resource_mut::<u64>().unwrap() += 1;
/// }
///
/// let mut world = World::new();
/// world.insert_resource(0u64);
/// let mut schedule = Schedule::new();
/// schedule.add_system("count", count);
///
/// let mut driver = SimDriver::new(world, schedule, SimConfig::default());
/// driver.run_ticks(10);
/// assert_eq!(driver.frame().as_u64(), 10);
/// assert_eq!(*driver.world().resource::<u64>().unwrap(), 10);
/// ```
pub struct SimDriver {
    world: World,
    schedule: Schedule,
    tick_rate: TickRate,
    frame: FrameIndex,
    run_state: RunState,
    speed: Speed,
    script: InputScript,
    /// Index of the first script event not yet applied (events are sorted by frame).
    cursor: usize,
    /// The `L4007` diagnostic of a system panic caught in a tick, held until taken.
    system_panic: Option<Diagnostic>,
}

impl SimDriver {
    /// Takes ownership of `world` and `schedule` and prepares frame 0.
    ///
    /// Seeds the world's determinism resources: inserts a [`DeterministicRng`] built from
    /// `config.seed`, the seed itself as a [`WorldSeed`] (so systems can build derived streams),
    /// and a fresh [`InputState`] — replacing any the caller pre-inserted, because the config is
    /// the single source of truth for a run's seed.
    pub fn new(mut world: World, schedule: Schedule, config: SimConfig) -> Self {
        world.insert_resource(DeterministicRng::new(config.seed));
        world.insert_resource(WorldSeed(config.seed));
        world.insert_resource(InputState::default());
        SimDriver {
            world,
            schedule,
            tick_rate: config.tick_rate,
            frame: FrameIndex::ZERO,
            run_state: RunState::Running,
            speed: Speed::NORMAL,
            script: config.input,
            cursor: 0,
            system_panic: None,
        }
    }

    /// Reconstructs a driver at an exact point in a run, for snapshot restore and replay seeking.
    ///
    /// Unlike [`new`](SimDriver::new), this does **not** reseed: the caller supplies the exact RNG,
    /// frame, run-state, and speed (everything `new` would otherwise reset). `world` must already
    /// carry its components; this inserts the given [`DeterministicRng`], the run's [`WorldSeed`],
    /// and a fresh [`InputState`] with an empty input script. The engine overrides both per rebuild
    /// — [`carry_input_from`](SimDriver::carry_input_from) on a scene switch, the snapshot's input
    /// and [`set_pending_input`](SimDriver::set_pending_input) tail on a restore — and a seeked run
    /// is otherwise driven by re-injected inputs ([`inject_input`](SimDriver::inject_input)) or live
    /// dispatch, not a pre-loaded script.
    pub fn restore(mut world: World, schedule: Schedule, restore: DriverRestore) -> Self {
        world.insert_resource(restore.rng);
        world.insert_resource(WorldSeed(restore.world_seed));
        world.insert_resource(InputState::default());
        SimDriver {
            world,
            schedule,
            tick_rate: restore.tick_rate,
            frame: restore.frame,
            run_state: restore.run_state,
            speed: restore.speed,
            script: InputScript::new(),
            cursor: 0,
            system_panic: None,
        }
    }

    // --- Observation ----------------------------------------------------------------------------

    /// The world, for queries and assertions between ticks.
    pub fn world(&self) -> &World {
        &self.world
    }

    /// The world, mutably — for setup and for command application at frame boundaries.
    ///
    /// Determinism contract: mutate only *between* ticks (which is the only time you can hold this
    /// borrow), and make every mutation part of the recorded command stream if the run must be
    /// reproducible.
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// The names of the schedule's systems, in execution order.
    pub fn system_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.schedule.system_names()
    }

    /// The next frame to be simulated — equivalently, the number of ticks completed so far.
    pub fn frame(&self) -> FrameIndex {
        self.frame
    }

    /// The current simulated time (exact tick count at the fixed rate).
    pub fn time(&self) -> SimTime {
        SimTime::from_ticks(self.frame.as_u64(), self.tick_rate)
    }

    /// The fixed tick rate.
    pub fn tick_rate(&self) -> TickRate {
        self.tick_rate
    }

    /// How many input events the script still retains. Applied-prefix compaction (in
    /// `do_tick`) keeps this bounded across a long live session, so it never
    /// tracks the total number ever injected. Exposed for tests asserting the backlog stays bounded.
    pub fn pending_input_len(&self) -> usize {
        self.script.len()
    }

    /// The `L4007` diagnostic of a system panic caught in a tick, if one is held.
    pub fn system_panic(&self) -> Option<&Diagnostic> {
        self.system_panic.as_ref()
    }

    /// Takes the held system-panic diagnostic, leaving `None`: the panic is reported to exactly one
    /// taker. The driver paused when it caught the panic and stays paused; the world is as the
    /// panicking tick left it.
    pub fn take_system_panic(&mut self) -> Option<Diagnostic> {
        self.system_panic.take()
    }

    // --- Run-state ------------------------------------------------------------------------------

    /// The current run-state.
    pub fn run_state(&self) -> RunState {
        self.run_state
    }

    /// Stops ticking until [`resume`](SimDriver::resume) or [`step`](SimDriver::step). Cancels any
    /// in-flight step. Idempotent.
    pub fn pause(&mut self) {
        self.run_state = RunState::Paused;
    }

    /// Resumes free running. Idempotent.
    pub fn resume(&mut self) {
        self.run_state = RunState::Running;
    }

    /// Arms the driver to perform exactly `ticks` more ticks, then pause — the agent's
    /// single-step debugger (`sim.step` on the wire). `step(0)` pauses immediately. Replaces any
    /// previous run-state; it does not accumulate.
    pub fn step(&mut self, ticks: u64) {
        self.run_state = if ticks == 0 {
            RunState::Paused
        } else {
            RunState::Stepping { remaining: ticks }
        };
    }

    /// The wall-clock pacing factor for live scheduling. Never consulted by this crate.
    pub fn speed(&self) -> Speed {
        self.speed
    }

    /// Sets the pacing factor. Affects only when a live scheduler calls
    /// [`advance`](SimDriver::advance); tick contents, `dt`, and hashes are untouched by
    /// construction (this crate never reads the value).
    pub fn set_speed(&mut self, speed: Speed) {
        self.speed = speed;
    }

    // --- Input ----------------------------------------------------------------------------------

    /// Injects one input event addressed to `frame` (or the next tick, when `frame` is `None`),
    /// returning the frame it will actually apply at.
    ///
    /// This is the live/protocol counterpart to a pre-loaded [`InputScript`]: `input.inject` on the
    /// wire lands here. The event is applied at the start of that frame's tick, before any system
    /// runs, exactly like a scripted event. An event addressed to a frame that has already run is
    /// refused — a live injection cannot rewrite the past — and the script is left unchanged. The
    /// script's sorted order and the driver's cursor invariant are preserved.
    pub fn inject_input(
        &mut self,
        frame: Option<FrameIndex>,
        event: InputEvent,
    ) -> Result<FrameIndex, PastFrameInject> {
        let at = frame.unwrap_or(self.frame);
        if at < self.frame {
            return Err(PastFrameInject {
                requested: at,
                current: self.frame,
            });
        }
        self.script.push(at, event);
        Ok(at)
    }

    /// Moves `departing`'s held input and its not-yet-applied injected events onto this driver —
    /// the scene-switch carry. Both drivers must stand at the same frame.
    pub fn carry_input_from(&mut self, departing: &mut SimDriver) {
        debug_assert_eq!(self.frame, departing.frame);
        debug_assert!(self.script.is_empty() && self.cursor == 0);
        if let Some(input) = departing.world.resource::<InputState>() {
            self.world.insert_resource(input.clone());
        }
        departing.script.drain_applied(departing.cursor);
        departing.cursor = 0;
        self.script = std::mem::take(&mut departing.script);
        self.cursor = 0;
    }

    /// The injected events not yet applied, sorted by frame — the script's tail past the cursor,
    /// which a snapshot carries so a restore keeps an `input.inject` addressed to a later frame.
    pub fn pending_input(&self) -> InputScript {
        InputScript::from(self.script.events()[self.cursor..].to_vec())
    }

    /// Replaces the script with `script`, to apply from the current frame on. Called right after
    /// [`restore`](SimDriver::restore), with the tail a snapshot carried.
    pub fn set_pending_input(&mut self, script: InputScript) {
        self.script = script;
        self.cursor = 0;
    }

    // --- Ticking --------------------------------------------------------------------------------

    /// Performs at most one tick, as the run-state allows; returns whether a tick was performed.
    ///
    /// [`Running`](RunState::Running) ticks; [`Paused`](RunState::Paused) does nothing;
    /// [`Stepping`](RunState::Stepping) ticks and decrements, pausing when the count hits zero. A
    /// tick in which a system panicked pauses the driver whatever the run-state was; see
    /// [`take_system_panic`](SimDriver::take_system_panic).
    /// This is the only method that advances the simulation — a live scheduler calls it on its
    /// wall-clock cadence, a headless run calls it in a tight loop.
    pub fn advance(&mut self) -> bool {
        self.advance_inner(Watch::None)
    }

    /// Like [`advance`](SimDriver::advance), but attaches a [`ScheduleObserver`] to the tick's
    /// [`Schedule::run_observed`] so a caller can watch per-system boundaries.
    ///
    /// The observer is pure observation: it cannot change what runs, so the tick — and therefore the
    /// `WorldHash` afterward — is bit-identical to the one [`advance`](SimDriver::advance) performs
    /// with no observer attached. This is the sim-side seam the profile driver (in `ludion-engine`,
    /// where the wall clock lives) measures through; this crate stays clock-free.
    pub fn advance_observed(&mut self, observer: &mut dyn ScheduleObserver) -> bool {
        self.advance_inner(Watch::Observer(observer))
    }

    /// Like [`advance`](SimDriver::advance), but runs the tick through [`Schedule::run_probed`] so
    /// `probe` sees the world **as each system left it** — the divergence-localization seam.
    ///
    /// Where [`advance_observed`](SimDriver::advance_observed) watches boundaries by name only, this
    /// exposes the state at each boundary, so two runs of the same frame under different ambient
    /// configurations can be compared system by system until one differs. Like the observer, the
    /// probe is pure observation: the tick — and the `WorldHash` afterward — is bit-identical to
    /// [`advance`](SimDriver::advance)'s.
    pub fn advance_probed(&mut self, probe: &mut dyn FnMut(&'static str, &World)) -> bool {
        self.advance_inner(Watch::Probe(probe))
    }

    /// The single tick step behind [`advance`](SimDriver::advance),
    /// [`advance_observed`](SimDriver::advance_observed), and
    /// [`advance_probed`](SimDriver::advance_probed): performs at most one tick as the run-state
    /// allows, threading the watcher through to [`do_tick`](SimDriver::do_tick).
    fn advance_inner(&mut self, observer: Watch<'_>) -> bool {
        match self.run_state {
            RunState::Paused => false,
            RunState::Running => {
                if self.do_tick(observer) {
                    self.run_state = RunState::Paused;
                }
                true
            }
            RunState::Stepping { remaining: 0 } => {
                // Unreachable via the public API (`step` normalizes 0 to Paused), but a
                // deserialized run-state could carry it; normalize instead of panicking.
                self.run_state = RunState::Paused;
                false
            }
            RunState::Stepping { remaining } => {
                let panicked = self.do_tick(observer);
                self.run_state = if remaining == 1 || panicked {
                    RunState::Paused
                } else {
                    RunState::Stepping {
                        remaining: remaining - 1,
                    }
                };
                true
            }
        }
    }

    /// Headless convenience: performs exactly `ticks` ticks as fast as possible, then leaves the
    /// driver paused. Equivalent to [`step(ticks)`](SimDriver::step) followed by draining
    /// [`advance`](SimDriver::advance); like `step`, it replaces the current run-state. It stops
    /// early at a tick whose system panicked — check
    /// [`take_system_panic`](SimDriver::take_system_panic) afterwards.
    pub fn run_ticks(&mut self, ticks: u64) {
        self.step(ticks);
        while self.advance() {}
    }

    /// One tick of the current frame, per the normative order in the module docs.
    ///
    /// `observer`, when present, watches each system's boundaries (see
    /// [`advance_observed`](SimDriver::advance_observed)) or its per-system state (see
    /// [`advance_probed`](SimDriver::advance_probed)); neither ever alters the tick. Returns whether
    /// a system panicked in it.
    fn do_tick(&mut self, observer: Watch<'_>) -> bool {
        let frame = self.frame;

        // 1. Inputs: clear edges, then apply this frame's scripted events in script order.
        let events = self.script.events();
        let input = self
            .world
            .resource_mut::<InputState>()
            .expect("SimDriver::new inserts the InputState resource");
        input.begin_tick();
        // `<=` rather than `==` is defensive self-healing: by invariant (sorted script, cursor
        // advanced every tick) no event addressed to an earlier frame can remain unapplied.
        while self.cursor < events.len() && events[self.cursor].frame <= frame {
            input.apply(events[self.cursor].event);
            self.cursor += 1;
        }
        // Resolve the action layer against the fresh input. The resource is taken out for the call
        // (the world offers no two-resource borrow) and put straight back.
        if let Some(mut actions) = self.world.remove_resource::<Actions>() {
            let input = self
                .world
                .resource::<InputState>()
                .expect("SimDriver::new inserts the InputState resource");
            actions.resolve(input);
            self.world.insert_resource(actions);
        }

        // 2. The gameplay clock: fold the frame the game marked frozen, if it marked one. A world
        //    with no `GameClock` — a bare driver, a game that never freezes — folds nothing and
        //    reads as an unheld clock, so its context is exactly what it was before the clock
        //    existed.
        let (clock, advanced) = match self.world.resource_mut::<GameClock>() {
            Some(clock) => {
                let advanced = clock.advance();
                (*clock, advanced)
            }
            None => (GameClock::default(), true),
        };

        // 3. Systems, in registration order, with time as parameters — never a wall clock.
        let ctx = SystemCtx::new(frame, self.tick_rate).with_clock(&clock, advanced);
        match observer {
            Watch::Observer(observer) => {
                self.schedule.run_observed(&mut self.world, &ctx, observer)
            }
            Watch::Probe(probe) => self.schedule.run_probed(&mut self.world, &ctx, probe),
            Watch::None => self.schedule.run(&mut self.world, &ctx),
        }
        let panicked = match self.world.remove_resource::<SystemPanic>() {
            Some(panic) => {
                self.system_panic = Some(system_panic_diagnostic(&panic, frame));
                true
            }
            None => false,
        };
        let panicked = self.restore_removed_input(frame) || panicked;

        // 4. Advance the frame counter.
        self.frame = frame.next();

        // 5. Compaction: drop the applied prefix so a long-lived live session (an event injected
        //    every frame) does not grow the script without bound. Only events the cursor already
        //    passed are removed; the unapplied remainder and its order are untouched — invisible to
        //    the sim and to replay (a replay-loaded script consumes the same events at the same
        //    frames either way). The cursor resets to the new front.
        if self.cursor > 0 {
            self.script.drain_applied(self.cursor);
            self.cursor = 0;
        }
        panicked
    }

    /// Step 1 of every tick expects the `InputState` resource. A system that removed it gets an
    /// empty one back and an `L4007` pause rather than a dead sim thread; a real panic's
    /// diagnostic, when both happened, is kept. Returns whether it restored.
    fn restore_removed_input(&mut self, frame: FrameIndex) -> bool {
        if self.world.resource::<InputState>().is_some() {
            return false;
        }
        self.world.insert_resource(InputState::default());
        self.system_panic.get_or_insert_with(|| {
            Diagnostic::new(
                code::SYSTEM_PANICKED,
                format!(
                    "a system removed the `InputState` resource on frame {}; the engine restored \
                     an empty one, so held input is lost",
                    frame.as_u64()
                ),
            )
            .with_help(format!(
                "reset it in place (`*input = InputState::default()`) rather than removing it; \
                 search for `remove_resource::<InputState>`. `sim.resume` continues at frame {}",
                frame.as_u64() + 1
            ))
        });
        true
    }
}

/// Renders a caught [`SystemPanic`] as the `L4007` diagnostic.
///
/// Says what panicked (the system's registration name), where (`file:line:column`, when the hook saw
/// it), at which frame, and what the payload said — the four things a reader needs before opening an
/// editor. The backtrace rides along only when `RUST_BACKTRACE` asked for one; the boundary swallows
/// the default hook's stderr output, so without this the flag would silently stop working.
fn system_panic_diagnostic(panic: &SystemPanic, frame: FrameIndex) -> Diagnostic {
    let at = match &panic.location {
        Some(location) => format!(" at {location}"),
        None => String::new(),
    };
    let backtrace = match &panic.backtrace {
        Some(backtrace) => format!("\n\nbacktrace:\n{backtrace}"),
        None => String::new(),
    };
    Diagnostic::new(
        code::SYSTEM_PANICKED,
        format!(
            "the system `{}` panicked{at} on frame {}: {}{backtrace}",
            panic.system,
            frame.as_u64(),
            panic.message,
        ),
    )
    .with_help(format!(
        "the sim paused after frame {frame}'s tick; the systems after `{system}` were skipped and \
         the world holds whatever the panic left — `entity.query` / `component.get` / `sim.hash` \
         inspect it (note `sim.hash` reports frame {next}). Fix the system (an unwrapped missing \
         component or resource is the usual cause) and restart. `sim.resume` does not retry: it \
         continues at frame {next} from the first system, so the systems before `{system}` run \
         again on a world they already changed; to retry, restore a snapshot (`engine.restore`) \
         or reload the scene",
        frame = frame.as_u64(),
        next = frame.as_u64() + 1,
        system = panic.system,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{InputEvent, Key};

    fn tally(world: &mut World, _ctx: &SystemCtx) {
        *world.resource_mut::<u64>().unwrap() += 1;
    }

    fn driver_with_tally() -> SimDriver {
        let mut world = World::new();
        world.insert_resource(0u64);
        let mut schedule = Schedule::new();
        schedule.add_system("tally", tally);
        SimDriver::new(world, schedule, SimConfig::default())
    }

    fn ticks_done(driver: &SimDriver) -> u64 {
        *driver.world().resource::<u64>().unwrap()
    }

    /// One row per tick: `(ctx.frame, ctx.game_frame, ctx.game_advanced(), ctx.game_dt == 0.0)`.
    #[derive(Default)]
    struct ClockTrace(Vec<(u64, u64, bool, bool)>);

    /// The shape a frozen game has: every frame of the window `10..=14` is held, and every tick
    /// records what the context it ran with said.
    fn hold_window(world: &mut World, ctx: &SystemCtx) {
        world.resource_mut::<ClockTrace>().unwrap().0.push((
            ctx.frame.as_u64(),
            ctx.game_frame.as_u64(),
            ctx.game_advanced(),
            ctx.game_dt == 0.0,
        ));
        if (10..=14).contains(&ctx.frame.as_u64())
            && let Some(clock) = world.resource_mut::<GameClock>()
        {
            clock.hold();
        }
    }

    fn driver_holding(with_clock: bool) -> SimDriver {
        let mut world = World::new();
        world.insert_resource(ClockTrace::default());
        if with_clock {
            world.insert_resource(GameClock::default());
        }
        let mut schedule = Schedule::new();
        schedule.add_system("hold_window", hold_window);
        SimDriver::new(world, schedule, SimConfig::default())
    }

    fn trace(driver: &SimDriver, frames: std::ops::RangeInclusive<usize>) -> Vec<(u64, u64, bool)> {
        driver.world().resource::<ClockTrace>().unwrap().0[frames]
            .iter()
            .map(|&(frame, game_frame, advanced, _)| (frame, game_frame, advanced))
            .collect()
    }

    /// The normative sequence of the tick's clock fold, read off the contexts systems actually ran
    /// with: gameplay frame 10 is seen for the whole window and one frame past it, "advanced" is
    /// true exactly once for it, and 11 follows with nothing skipped.
    #[test]
    fn a_held_window_freezes_the_gameplay_frame_systems_see() {
        let mut driver = driver_holding(true);
        driver.run_ticks(18);
        assert_eq!(
            trace(&driver, 8..=17),
            vec![
                (8, 8, true),
                (9, 9, true),
                (10, 10, true),
                (11, 10, false),
                (12, 10, false),
                (13, 10, false),
                (14, 10, false),
                (15, 10, false),
                (16, 11, true),
                (17, 12, true),
            ]
        );
        assert_eq!(
            driver.world().resource::<GameClock>().unwrap().held(),
            5,
            "exactly the five held frames left gameplay time"
        );
    }

    /// The gameplay timestep is zero on exactly the frames gameplay time did not advance into — so
    /// a system integrating `pos += v * ctx.game_dt` freezes without knowing about the clock.
    #[test]
    fn the_gameplay_timestep_is_zero_on_a_held_frame() {
        let mut driver = driver_holding(true);
        driver.run_ticks(18);
        let frozen: Vec<u64> = driver
            .world()
            .resource::<ClockTrace>()
            .unwrap()
            .0
            .iter()
            .filter(|&&(_, _, _, dt_is_zero)| dt_is_zero)
            .map(|&(frame, ..)| frame)
            .collect();
        assert_eq!(frozen, vec![11, 12, 13, 14, 15]);
    }

    /// A world with no [`GameClock`] ticks exactly as it did before the clock existed: gameplay
    /// time is sim time, every frame.
    #[test]
    fn a_clockless_world_reads_gameplay_time_as_sim_time() {
        let mut driver = driver_holding(false);
        driver.run_ticks(18);
        let rows = driver.world().resource::<ClockTrace>().unwrap().0.clone();
        assert_eq!(rows.len(), 18);
        for (frame, game_frame, advanced, dt_is_zero) in rows {
            assert_eq!(game_frame, frame);
            assert!(advanced);
            assert!(!dt_is_zero);
        }
    }

    #[test]
    fn new_driver_is_running_at_frame_zero_with_seeded_resources() {
        let driver = driver_with_tally();
        assert_eq!(driver.run_state(), RunState::Running);
        assert_eq!(driver.frame(), FrameIndex::ZERO);
        assert_eq!(driver.time().ticks(), 0);
        assert_eq!(
            *driver.world().resource::<DeterministicRng>().unwrap(),
            DeterministicRng::new(0)
        );
        assert!(driver.world().resource::<InputState>().is_some());
    }

    #[test]
    fn paused_driver_does_not_tick() {
        let mut driver = driver_with_tally();
        driver.pause();
        assert!(!driver.advance());
        assert!(!driver.advance());
        assert_eq!(driver.frame(), FrameIndex::ZERO);
        assert_eq!(ticks_done(&driver), 0);
    }

    #[test]
    fn step_runs_exactly_n_ticks_then_pauses() {
        let mut driver = driver_with_tally();
        driver.pause();
        driver.step(3);
        assert_eq!(driver.run_state(), RunState::Stepping { remaining: 3 });

        let mut performed = 0;
        while driver.advance() {
            performed += 1;
        }
        assert_eq!(performed, 3);
        assert_eq!(ticks_done(&driver), 3);
        assert_eq!(driver.frame().as_u64(), 3);
        assert_eq!(driver.run_state(), RunState::Paused);
    }

    #[test]
    fn step_zero_pauses_immediately() {
        let mut driver = driver_with_tally();
        driver.step(0);
        assert_eq!(driver.run_state(), RunState::Paused);
        assert!(!driver.advance());
    }

    #[test]
    fn resume_after_pause_ticks_again() {
        let mut driver = driver_with_tally();
        driver.run_ticks(2);
        assert_eq!(driver.run_state(), RunState::Paused);
        driver.resume();
        assert!(driver.advance());
        assert_eq!(ticks_done(&driver), 3);
    }

    #[test]
    fn speed_is_stored_state_only() {
        let mut driver = driver_with_tally();
        driver.set_speed(Speed::new(0.25).unwrap());
        assert_eq!(driver.speed().factor(), 0.25);
        // Ticks are unaffected: dt and outcomes are identical (proven end-to-end in the
        // determinism suite; here we just pin that ticking still works).
        driver.run_ticks(4);
        assert_eq!(ticks_done(&driver), 4);
    }

    #[test]
    fn new_publishes_the_config_seed_as_a_resource() {
        let driver = SimDriver::new(
            World::new(),
            Schedule::new(),
            SimConfig {
                seed: 4321,
                ..SimConfig::default()
            },
        );
        assert_eq!(
            driver.world().resource::<WorldSeed>().copied(),
            Some(WorldSeed(4321)),
        );
    }

    #[test]
    fn restore_resumes_exact_state_without_reseeding() {
        // Advance a driver, then advance its RNG to a non-initial state.
        let mut driver = driver_with_tally();
        driver.run_ticks(4);
        for _ in 0..5 {
            driver
                .world_mut()
                .resource_mut::<DeterministicRng>()
                .unwrap()
                .next_u32();
        }
        let rng_at_restore = *driver.world().resource::<DeterministicRng>().unwrap();

        // Rebuild a driver at the same frame with the same RNG (no reseed).
        let mut restored_world = World::new();
        restored_world.insert_resource(*driver.world().resource::<u64>().unwrap());
        let mut restored = SimDriver::restore(
            restored_world,
            Schedule::new(),
            DriverRestore {
                rng: rng_at_restore,
                world_seed: 0,
                frame: driver.frame(),
                run_state: RunState::Paused,
                speed: Speed::NORMAL,
                tick_rate: TickRate::DEFAULT,
            },
        );
        assert_eq!(restored.frame(), FrameIndex::new(4));
        assert_eq!(restored.run_state(), RunState::Paused);

        // The RNG continues identically from the restore point.
        let mut expected = rng_at_restore;
        assert_eq!(
            restored
                .world_mut()
                .resource_mut::<DeterministicRng>()
                .unwrap()
                .next_u32(),
            expected.next_u32(),
        );
    }

    #[test]
    fn inject_input_applies_at_the_next_tick() {
        fn probe(world: &mut World, ctx: &SystemCtx) {
            let held = world.resource::<InputState>().unwrap().is_held(Key::Space);
            world
                .resource_mut::<Vec<(u64, bool)>>()
                .unwrap()
                .push((ctx.frame.as_u64(), held));
        }

        let mut world = World::new();
        world.insert_resource(Vec::<(u64, bool)>::new());
        let mut schedule = Schedule::new();
        schedule.add_system("probe", probe);

        let mut driver = SimDriver::new(world, schedule, SimConfig::default());
        driver.run_ticks(1); // frame 0 runs, no input yet
        driver.resume();
        // Inject for "the next tick" (None → current frame 1).
        let at = driver
            .inject_input(None, InputEvent::KeyDown { key: Key::Space })
            .unwrap();
        assert_eq!(at, FrameIndex::new(1));
        driver.step(2);
        while driver.advance() {}

        let log = driver.world().resource::<Vec<(u64, bool)>>().unwrap();
        assert_eq!(log, &vec![(0, false), (1, true), (2, true)]);
    }

    #[test]
    fn inject_input_refuses_a_past_frame() {
        let mut driver = SimDriver::new(World::new(), Schedule::new(), SimConfig::default());
        driver.run_ticks(3);
        let key = InputEvent::KeyDown { key: Key::Space };

        let refused = driver
            .inject_input(Some(FrameIndex::new(1)), key)
            .unwrap_err();
        assert_eq!(
            refused,
            PastFrameInject {
                requested: FrameIndex::new(1),
                current: FrameIndex::new(3),
            }
        );
        assert_eq!(driver.pending_input_len(), 0);

        let at = driver.inject_input(Some(FrameIndex::new(3)), key).unwrap();
        assert_eq!(at, FrameIndex::new(3));
        assert_eq!(driver.pending_input_len(), 1);
    }

    fn driver_at(frame: FrameIndex, world: World, schedule: Schedule) -> SimDriver {
        SimDriver::restore(
            world,
            schedule,
            DriverRestore {
                rng: DeterministicRng::new(0),
                world_seed: 0,
                frame,
                run_state: RunState::Running,
                speed: Speed::NORMAL,
                tick_rate: TickRate::DEFAULT,
            },
        )
    }

    #[test]
    fn carry_input_keeps_held_input_without_a_new_press() {
        type Seen = (bool, bool, [f32; 2]);
        fn probe(world: &mut World, _ctx: &SystemCtx) {
            let input = world.resource::<InputState>().unwrap();
            let seen: Seen = (
                input.is_held(Key::W),
                input.was_pressed(Key::W),
                input.mouse().position(),
            );
            world.resource_mut::<Vec<Seen>>().unwrap().push(seen);
        }

        let mut departing = SimDriver::new(World::new(), Schedule::new(), SimConfig::default());
        departing
            .inject_input(None, InputEvent::KeyDown { key: Key::W })
            .unwrap();
        departing
            .inject_input(
                None,
                InputEvent::MouseMove {
                    position: [12.5, 40.0],
                },
            )
            .unwrap();
        departing.run_ticks(1);

        let mut world = World::new();
        world.insert_resource(Vec::<Seen>::new());
        let mut schedule = Schedule::new();
        schedule.add_system("probe", probe);
        let mut arriving = driver_at(departing.frame(), world, schedule);
        arriving.carry_input_from(&mut departing);
        arriving.run_ticks(1);

        let log = arriving.world().resource::<Vec<Seen>>().unwrap();
        assert_eq!(log, &vec![(true, false, [12.5, 40.0])]);
    }

    #[test]
    fn a_restored_driver_keeps_the_pending_tail_it_is_given() {
        let mut original = SimDriver::new(World::new(), Schedule::new(), SimConfig::default());
        original.run_ticks(1);
        original
            .inject_input(
                Some(FrameIndex::new(5)),
                InputEvent::KeyDown { key: Key::Space },
            )
            .unwrap();
        let tail = original.pending_input();
        assert_eq!(tail.len(), 1);

        let mut restored = driver_at(original.frame(), World::new(), Schedule::new());
        assert!(restored.pending_input().is_empty());
        restored.set_pending_input(tail);
        let space_held = |d: &SimDriver| {
            d.world()
                .resource::<InputState>()
                .unwrap()
                .is_held(Key::Space)
        };
        restored.run_ticks(4); // frames 1..=4
        assert!(!space_held(&restored));
        restored.run_ticks(1); // frame 5
        assert!(space_held(&restored));
        assert!(restored.pending_input().is_empty());
    }

    #[test]
    fn carry_input_moves_the_pending_script_tail() {
        let mut departing = SimDriver::new(World::new(), Schedule::new(), SimConfig::default());
        departing.run_ticks(1);
        departing
            .inject_input(
                Some(FrameIndex::new(3)),
                InputEvent::KeyDown { key: Key::Space },
            )
            .unwrap();

        let mut arriving = driver_at(departing.frame(), World::new(), Schedule::new());
        arriving.carry_input_from(&mut departing);
        assert_eq!(departing.pending_input_len(), 0);
        assert_eq!(arriving.pending_input_len(), 1);

        let space_held = |d: &SimDriver| {
            d.world()
                .resource::<InputState>()
                .unwrap()
                .is_held(Key::Space)
        };
        arriving.run_ticks(2); // frames 1 and 2: not yet
        assert!(!space_held(&arriving));
        arriving.run_ticks(1); // frame 3
        assert!(space_held(&arriving));
    }

    #[test]
    fn live_injection_does_not_grow_the_script_without_bound() {
        // A long-lived live session injects an event (or several) every frame. Without compaction the
        // script's applied prefix would accumulate one entry per injected event forever; with it, the
        // retained backlog is bounded by a single frame's injections, never the total ever injected.
        let mut driver = driver_with_tally(); // Running by default
        for _ in 0..1000 {
            driver
                .inject_input(None, InputEvent::KeyDown { key: Key::Space })
                .unwrap();
            driver
                .inject_input(None, InputEvent::KeyUp { key: Key::Space })
                .unwrap();
            driver.resume(); // stay free-running
            assert!(driver.advance(), "a running driver ticks");
        }
        assert_eq!(driver.frame().as_u64(), 1000);
        assert!(
            driver.pending_input_len() <= 2,
            "the script retained {} events after 2000 injections — compaction should bound it to a \
             frame's worth",
            driver.pending_input_len(),
        );
    }

    #[test]
    fn same_frame_events_apply_in_script_order_at_that_tick() {
        // Frame 1: W down then W up in the same frame — held is false after both, but the
        // pressed AND released edges are both visible to frame 1's systems.
        fn probe(world: &mut World, ctx: &SystemCtx) {
            let (pressed, released, held) = {
                let input = world.resource::<InputState>().unwrap();
                (
                    input.was_pressed(Key::W),
                    input.was_released(Key::W),
                    input.is_held(Key::W),
                )
            };
            world
                .resource_mut::<Vec<(u64, bool, bool, bool)>>()
                .unwrap()
                .push((ctx.frame.as_u64(), pressed, released, held));
        }

        let mut world = World::new();
        world.insert_resource(Vec::<(u64, bool, bool, bool)>::new());
        let mut schedule = Schedule::new();
        schedule.add_system("probe", probe);

        let mut script = InputScript::new();
        script.push(FrameIndex::new(1), InputEvent::KeyDown { key: Key::W });
        script.push(FrameIndex::new(1), InputEvent::KeyUp { key: Key::W });

        let mut driver = SimDriver::new(
            world,
            schedule,
            SimConfig {
                input: script,
                ..SimConfig::default()
            },
        );
        driver.run_ticks(3);

        let log = driver
            .world()
            .resource::<Vec<(u64, bool, bool, bool)>>()
            .unwrap();
        assert_eq!(
            log,
            &vec![
                (0, false, false, false),
                (1, true, true, false), // both edges, no level
                (2, false, false, false),
            ]
        );
    }

    fn panics_on_frame_two(world: &mut World, ctx: &SystemCtx) {
        *world.resource_mut::<u64>().unwrap() += 1;
        if ctx.frame.as_u64() == 2 {
            panic!("boom");
        }
    }

    fn driver_panicking_on_frame_two() -> SimDriver {
        let mut world = World::new();
        world.insert_resource(0u64);
        let mut schedule = Schedule::new();
        schedule.add_system("panics_on_frame_two", panics_on_frame_two);
        SimDriver::new(world, schedule, SimConfig::default())
    }

    #[test]
    fn run_ticks_stops_at_the_panicking_tick_and_holds_l4007_until_taken() {
        let mut driver = driver_panicking_on_frame_two();
        driver.run_ticks(5);
        assert_eq!(ticks_done(&driver), 3);
        assert_eq!(driver.frame().as_u64(), 3);
        assert_eq!(driver.run_state(), RunState::Paused);
        assert!(driver.world().resource::<SystemPanic>().is_none());

        let held = driver.system_panic().expect("held until taken");
        assert_eq!(held.code, code::SYSTEM_PANICKED);
        let taken = driver.take_system_panic().expect("first take");
        assert_eq!(taken.code, code::SYSTEM_PANICKED);
        assert!(taken.message.contains("frame 2"), "{}", taken.message);
        assert!(taken.message.contains("boom"), "{}", taken.message);
        let help = taken.help.as_deref().expect("help");
        assert!(help.contains("frame 2"), "{help}");
        assert!(help.contains("frame 3"), "{help}");
        assert!(help.contains("does not retry"), "{help}");
        assert!(driver.take_system_panic().is_none());
    }

    fn removes_input_on_frame_two(world: &mut World, ctx: &SystemCtx) {
        if ctx.frame.as_u64() == 2 {
            world.remove_resource::<InputState>();
        }
    }

    #[test]
    fn a_system_removing_input_state_pauses_with_l4007_and_the_sim_continues() {
        let mut schedule = Schedule::new();
        schedule.add_system("removes_input_on_frame_two", removes_input_on_frame_two);
        let mut driver = SimDriver::new(World::new(), schedule, SimConfig::default());
        driver.run_ticks(5);
        assert_eq!(driver.frame().as_u64(), 3);
        assert_eq!(driver.run_state(), RunState::Paused);
        assert!(driver.world().resource::<InputState>().is_some());
        let held = driver.take_system_panic().expect("L4007 held");
        assert_eq!(held.code, code::SYSTEM_PANICKED);
        assert!(held.message.contains("InputState"), "{}", held.message);
        driver.run_ticks(2);
        assert_eq!(driver.frame().as_u64(), 5);
        assert!(driver.take_system_panic().is_none());
    }

    #[test]
    fn a_running_driver_pauses_on_a_panicked_tick() {
        let mut driver = driver_panicking_on_frame_two();
        assert!(driver.advance());
        assert!(driver.advance());
        assert_eq!(driver.run_state(), RunState::Running);
        assert!(
            driver.advance(),
            "the panicking tick still counts as performed"
        );
        assert_eq!(driver.run_state(), RunState::Paused);
        assert!(!driver.advance());
        assert!(driver.take_system_panic().is_some());
    }
}
