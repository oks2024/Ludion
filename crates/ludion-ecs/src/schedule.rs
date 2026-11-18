//! Sequential system scheduling.
//!
//! A [`Schedule`] is an ordered list of systems — plain `fn(&mut World, &SystemCtx)` — run strictly
//! in **registration order**. This is a determinism rule, not a scheduling limitation: there is no
//! parallelism, no automatic ordering, no dependency inference. What you add is exactly what runs,
//! in exactly the order you added it (architecture doc §5, rule 5).
//!
//! # The unwind boundary
//!
//! Every system body runs inside [`catch_unwind`](std::panic::catch_unwind). Game code panics
//! readily by design — an `unwrap` on a component the world no longer has is one line — and without
//! a boundary that panic takes the sim thread with it, leaving a host that answers every subsequent
//! request with `L3009` and no message about what actually broke. It is the one authoring mistake
//! that would otherwise produce no diagnostic at all, in the layer an agent writes most.
//!
//! A caught panic **skips the rest of that tick's systems** and leaves a [`SystemPanic`] resource in
//! the world. That resource is the only channel back to the host: a [`Schedule`] is owned by the
//! driver that runs it, so the world is the one thing both sides hold. The host (`SimDriver` in
//! `ludion-sim`) removes it right after the tick, turns it into the `L4007` diagnostic, and pauses —
//! so the panic is reported once, not once per frame forever. The hook that keeps the panic off
//! stderr lives in [`ludion_core::panic_capture`]; its flag reaches the workers of a `par` region
//! inside the system, so a parallel panic reports its lowest chunk's message and site.
//!
//! On the happy path the boundary is behaviourally inert: no allocation, no branch on the value, and
//! the same systems in the same order, so world hashes and committed replays are byte-identical to
//! what they were before it existed. A caught panic is as deterministic as the panic itself — the
//! same inputs reach the same system at the same frame and fail the same way.
//!
//! **This needs unwinding.** Under `panic = "abort"` a panic never unwinds, so nothing is caught and
//! the process dies as before. No workspace profile sets it.

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;

use ludion_core::codes::code;
use ludion_core::panic_capture;
use ludion_core::time::{FrameIndex, GameClock, GameTime, Q16_PER_TICK, SimTime, TickRate};

use crate::{Phase, World};

/// Per-run context handed to every system.
///
/// Time enters the sim only through this struct — never a wall clock. `dt` is the fixed timestep in
/// **seconds as `f32`** (`1.0 / tick_rate`); it is a constant of the simulation and does not vary
/// frame to frame (`sim.set_speed` changes *when* ticks happen, never `dt`).
///
/// # Two clocks
///
/// `frame`/`time`/`dt` are the sim's own: they advance every tick, always, because the host and a
/// paused game's own overlay still have to run. `game_frame`/`game_time`/`game_dt` are the same
/// three read through the world's [`GameClock`] — the clock a game holds while it is frozen. On an
/// unheld run the two agree exactly; while a game holds, the gameplay three stand still
/// (`game_dt` is `0.0`, so a system integrating `pos += v * ctx.game_dt` freezes for free) and
/// [`game_advanced`](SystemCtx::game_advanced) is `false`.
///
/// A game may also slow gameplay time ([`GameClock::scale`]): `game_dt` shrinks to `dt × s`,
/// [`game_instant`](SystemCtx::game_instant) is the exact fractional gameplay time, `game_frame` /
/// `game_time` are its floor, and [`game_frames_crossed`](SystemCtx::game_frames_crossed) says
/// whether a whole gameplay frame passed this tick. `game_advanced` stays `true` while time moves.
///
/// Build one with [`new`](SystemCtx::new) (unheld) and, if a clock applies,
/// [`with_clock`](SystemCtx::with_clock) — never field by field, so a new clock field cannot be
/// silently forgotten at a call site.
#[derive(Clone, Copy, Debug)]
pub struct SystemCtx {
    /// The frame being simulated.
    pub frame: FrameIndex,
    /// The current simulated time (exact tick count).
    pub time: SimTime,
    /// The fixed timestep, in seconds.
    pub dt: f32,
    /// [`frame`](SystemCtx::frame) in gameplay frames — every frame the game held excluded.
    pub game_frame: FrameIndex,
    /// [`time`](SystemCtx::time) in gameplay time — the clock every closed-form evaluator reads.
    pub game_time: SimTime,
    /// The gameplay timestep: [`dt`](SystemCtx::dt) on a frame gameplay time advanced into, `0.0`
    /// on a held one.
    pub game_dt: f32,
    /// [`time`](SystemCtx::time) in exact fractional gameplay time; its floor is
    /// [`game_time`](SystemCtx::game_time).
    pub game_instant: GameTime,
    game_step_q16: u32,
}

impl SystemCtx {
    /// The context of `frame` at `rate`, with gameplay time equal to sim time — the shape every
    /// caller that has no [`GameClock`] wants (a bare driver, a test, a tool that runs a schedule
    /// by hand).
    pub fn new(frame: FrameIndex, rate: TickRate) -> Self {
        #[allow(clippy::cast_possible_truncation)] // dt is the presentation of an exact 1/hz
        let dt = rate.seconds_per_tick() as f32;
        SystemCtx {
            frame,
            time: SimTime::from_ticks(frame.as_u64(), rate),
            dt,
            game_frame: frame,
            game_time: SimTime::from_ticks(frame.as_u64(), rate),
            game_dt: dt,
            game_instant: GameTime::from_ticks(frame.as_u64(), rate),
            game_step_q16: Q16_PER_TICK as u32,
        }
    }

    /// The same context read through `clock`, `advanced` being what the tick's
    /// [`GameClock::advance`] fold returned.
    pub fn with_clock(self, clock: &GameClock, advanced: bool) -> Self {
        let step = if advanced { clock.step_q16() } else { 0 };
        let game_instant = clock.game_instant(self.time);
        SystemCtx {
            game_frame: FrameIndex::new(game_instant.ticks_floor()),
            game_time: game_instant.floor(),
            game_dt: self.dt * (step as f32 / 65536.0),
            game_instant,
            game_step_q16: step,
            ..self
        }
    }

    /// The fraction of a full tick gameplay time advanced by this tick: `1.0` unscaled, `0.0` held.
    pub fn game_scale(&self) -> f32 {
        self.game_step_q16 as f32 / 65536.0
    }

    /// How many whole gameplay frames this tick crossed: `0` or `1`, and `1` on every unscaled
    /// advanced tick. The guard for "once per gameplay frame" when time is scaled.
    pub fn game_frames_crossed(&self) -> u64 {
        self.game_instant.ticks_crossed(self.game_step_q16)
    }

    /// Whether gameplay time advanced into this tick. `false` on a frame the game held — the guard
    /// a system that must not fire twice for one gameplay frame returns on.
    pub fn game_advanced(&self) -> bool {
        self.game_dt > 0.0
    }
}

/// The shareable body of a scheduled system. An `Arc<dyn Fn…>` rather than a `fn` pointer so a
/// caller may register a closure (e.g. one carrying per-system metadata in `ludion-engine`): a
/// generic `F` cannot be coerced to a bare `fn` pointer without `unsafe`, which the workspace
/// forbids. Systems remain plain `fn`s by convention — state lives in the `World`, never a captured
/// closure — so the per-tick cost is unchanged: one indirect call either way. The `Arc` also lets a
/// body be cloned (a refcount bump) into a rebuilt schedule.
pub type SystemFn = Arc<dyn Fn(&mut World, &SystemCtx) + Send + Sync>;

/// A single named system.
struct System {
    name: &'static str,
    run: SystemFn,
    phase: Phase,
}

/// What a caught system panic left behind, as a [`World`] resource — see [`Schedule::run`] for the
/// boundary that produces it.
///
/// Present only between the panic and the end of the tick — the host (`SimDriver`) removes it,
/// renders it as the `L4007` diagnostic, and pauses. Never registered as a component, so it
/// is invisible to the `WorldHash`, to scenes, and to snapshots; a world reset drops it with every
/// other resource.
#[derive(Debug)]
pub struct SystemPanic {
    /// The registration name of the system whose body panicked.
    pub system: &'static str,
    /// The panic payload as text — what `panic!`/`unwrap` formatted, or a placeholder when the
    /// payload is neither `&str` nor `String`.
    pub message: String,
    /// `file:line:column` of the panic site, or `None` if the hook never saw it (another hook was
    /// installed over ours).
    pub location: Option<String>,
    /// The captured backtrace, present only when `RUST_BACKTRACE` asked for one — the boundary
    /// swallows the default hook's output, so this is what keeps that workflow working.
    pub backtrace: Option<String>,
}

/// Runs one system inside the unwind boundary, returning whether it completed.
///
/// On a panic the [`SystemPanic`] record goes into the world and the caller stops the tick.
///
/// The world is in the system's [`Phase`] only for the body: it is back to [`Phase::Host`] before
/// the panic record is written (a presentation-phase write of it would itself be refused), so no
/// path leaves a non-`Host` phase behind.
///
/// The [`AssertUnwindSafe`] is a real claim, not a formality: `&mut World` is not `UnwindSafe`
/// precisely because a half-completed mutation can escape the boundary, and one can — a system may
/// have spawned half its entities or written one field of two. The claim is sound because nothing
/// downstream trusts the world to be consistent afterwards: the caller abandons the remaining
/// systems of the tick, and the host auto-pauses before another tick can observe the wreckage. The
/// world is deliberately left exactly as the panic left it, because that shape is the evidence.
fn run_guarded(system: &System, world: &mut World, ctx: &SystemCtx) -> bool {
    world.set_phase(system.phase, Some(system.name));
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        panic_capture::scoped(true, || (system.run)(world, ctx))
    }));
    world.set_phase(Phase::Host, None);
    match result {
        Ok(()) => true,
        Err(payload) => {
            let (location, backtrace) = panic_capture::take_site()
                .map_or((None, None), |site| (site.location, site.backtrace));
            world.insert_resource(SystemPanic {
                system: system.name,
                message: payload_text(payload.as_ref()),
                location,
                backtrace,
            });
            false
        }
    }
}

/// The panic payload as text. `panic!` produces a `&'static str` for a literal and a `String` for a
/// formatted message; anything else came from an explicit `panic_any` and has no text to show.
pub fn payload_text(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&'static str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "panicked with a non-string payload".to_string()
    }
}

/// A clock-free observer of a schedule's per-system boundaries.
///
/// [`Schedule::run_observed`] calls [`before_system`](ScheduleObserver::before_system) immediately
/// before each system runs and [`after_system`](ScheduleObserver::after_system) immediately after it
/// returns, handing over the system's registration name. The trait is deliberately **timing-free**:
/// it reports *when* a boundary is crossed, never *how long* anything took. An observer that measures
/// duration (with a wall clock) belongs above the sim boundary — the profile driver in
/// `ludion-engine` — so this crate keeps its no-wall-clock determinism rule. Because an observer only
/// watches, an observed run mutates the world exactly as a plain [`run`](Schedule::run) would: same
/// systems, same order, same resulting state and `WorldHash`.
///
/// # Example
/// ```
/// use ludion_ecs::{Schedule, ScheduleObserver, SystemCtx, World};
/// use ludion_core::time::{FrameIndex, TickRate};
///
/// #[derive(Default)]
/// struct Trace(Vec<String>);
/// impl ScheduleObserver for Trace {
///     fn before_system(&mut self, name: &'static str) {
///         self.0.push(format!("before {name}"));
///     }
///     fn after_system(&mut self, name: &'static str) {
///         self.0.push(format!("after {name}"));
///     }
/// }
///
/// fn tick(world: &mut World, _ctx: &SystemCtx) {
///     *world.resource_mut::<u32>().unwrap() += 1;
/// }
///
/// let mut world = World::new();
/// world.insert_resource(0u32);
/// let mut schedule = Schedule::new();
/// schedule.add_system("tick", tick);
///
/// let ctx = SystemCtx::new(FrameIndex::ZERO, TickRate::DEFAULT);
/// let mut trace = Trace::default();
/// schedule.run_observed(&mut world, &ctx, &mut trace);
/// assert_eq!(*world.resource::<u32>().unwrap(), 1); // identical to `run`
/// assert_eq!(trace.0, ["before tick", "after tick"]);
/// ```
pub trait ScheduleObserver {
    /// Called immediately before the named system runs.
    fn before_system(&mut self, name: &'static str);
    /// Called immediately after the named system returns.
    fn after_system(&mut self, name: &'static str);
}

/// The hooks around one system's run inside [`Schedule::run_with`]; a plain run has none.
trait Boundary {
    fn before(&mut self, _name: &'static str) {}
    fn after(&mut self, _name: &'static str, _world: &mut World) {}
}

impl Boundary for () {}

struct ObserverBoundary<'a>(&'a mut dyn ScheduleObserver);

impl Boundary for ObserverBoundary<'_> {
    fn before(&mut self, name: &'static str) {
        self.0.before_system(name);
    }
    fn after(&mut self, name: &'static str, _world: &mut World) {
        self.0.after_system(name);
    }
}

/// Runs the probe with the recording system cleared: the probe is host-side work, not the system's.
struct ProbeBoundary<'a>(&'a mut dyn FnMut(&'static str, &World));

impl Boundary for ProbeBoundary<'_> {
    fn after(&mut self, name: &'static str, world: &mut World) {
        world.clear_recording_system();
        (self.0)(name, world);
    }
}

/// An ordered list of systems, run sequentially in registration order.
///
/// # Example
/// ```
/// use ludion_ecs::{Schedule, SystemCtx, World};
/// use ludion_core::time::{FrameIndex, TickRate};
///
/// fn tick(world: &mut World, _ctx: &SystemCtx) {
///     let counter = world.resource_mut::<u32>().unwrap();
///     *counter += 1;
/// }
///
/// let mut world = World::new();
/// world.insert_resource(0u32);
///
/// let mut schedule = Schedule::new();
/// schedule.add_system("tick", tick);
///
/// let ctx = SystemCtx::new(FrameIndex::ZERO, TickRate::DEFAULT);
/// schedule.run(&mut world, &ctx);
/// assert_eq!(*world.resource::<u32>().unwrap(), 1);
/// ```
#[derive(Default)]
pub struct Schedule {
    systems: Vec<System>,
}

impl Schedule {
    /// Creates an empty schedule.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a system. Returns `&mut Self` for chaining. Systems run in the order added.
    ///
    /// Takes a bare `fn` pointer — the plain-`fn` convention systems follow — and wraps it in the
    /// `Arc` the schedule stores. Register a closure (e.g. one carrying metadata) with
    /// [`add_system_shared`](Schedule::add_system_shared).
    ///
    /// # Panics
    /// Panics with `L4018` if a system named `name` is already registered.
    pub fn add_system(&mut self, name: &'static str, run: fn(&mut World, &SystemCtx)) -> &mut Self {
        self.add_system_shared(name, Arc::new(run))
    }

    /// Appends a system from an already-boxed body. The additive sibling of
    /// [`add_system`](Schedule::add_system) for a caller that needs to register a closure — one
    /// capturing metadata, say — instead of a bare `fn`. Same determinism contract: systems run once
    /// each in the order added.
    ///
    /// # Panics
    /// Panics with `L4018` if a system named `name` is already registered.
    pub fn add_system_shared(&mut self, name: &'static str, run: SystemFn) -> &mut Self {
        self.push_system(name, run, Phase::Sim)
    }

    /// Appends a presentation system: it runs in [`Phase::Presentation`], where sim state is
    /// read-only and `L0028` guards the crossing (see [`Phase`]). The name check is shared with the
    /// sim systems.
    ///
    /// # Panics
    /// Panics with `L4018` if a system named `name` is already registered.
    pub fn add_presentation_system(
        &mut self,
        name: &'static str,
        run: fn(&mut World, &SystemCtx),
    ) -> &mut Self {
        self.add_presentation_system_shared(name, Arc::new(run))
    }

    /// The closure-taking sibling of [`add_presentation_system`](Schedule::add_presentation_system).
    ///
    /// # Panics
    /// Panics with `L4018` if a system named `name` is already registered.
    pub fn add_presentation_system_shared(
        &mut self,
        name: &'static str,
        run: SystemFn,
    ) -> &mut Self {
        self.push_system(name, run, Phase::Presentation)
    }

    fn push_system(&mut self, name: &'static str, run: SystemFn, phase: Phase) -> &mut Self {
        assert!(
            self.systems.iter().all(|system| system.name != name),
            "{}: the system name `{name}` is registered twice in one schedule. Recording, query drift, \
             profiling and panic reports all key systems by name — give each a distinct one",
            code::SYSTEM_NAME_DUPLICATE,
        );
        self.systems.push(System { name, run, phase });
        self
    }

    /// Runs every system once, in registration order.
    ///
    /// Each body runs inside [`catch_unwind`](std::panic::catch_unwind): a panicking system leaves a
    /// [`SystemPanic`] in `world`, the systems after it are skipped for this tick, and the host turns
    /// that resource into the `L4007` diagnostic and pauses the sim — so the failure is reported once
    /// with a message naming the system, instead of taking the sim thread down silently.
    ///
    /// On the happy path the boundary is behaviourally inert, so world hashes and committed replays
    /// are byte-identical to what they were before it existed. It needs unwinding: under
    /// `panic = "abort"` nothing is caught and the process dies as before.
    pub fn run(&mut self, world: &mut World, ctx: &SystemCtx) {
        self.run_with(world, ctx, &mut ());
    }

    /// Runs every system once, in registration order, notifying `observer` at each system boundary.
    ///
    /// Identical to [`run`](Schedule::run) but for the [`ScheduleObserver`] callbacks bracketing each
    /// system. The observer only watches — it cannot change what runs or in what order — so this
    /// leaves the world in exactly the state [`run`](Schedule::run) would (see [`ScheduleObserver`]
    /// for the worked example).
    pub fn run_observed(
        &mut self,
        world: &mut World,
        ctx: &SystemCtx,
        observer: &mut dyn ScheduleObserver,
    ) {
        self.run_with(world, ctx, &mut ObserverBoundary(observer));
    }

    /// Runs every system once, in registration order, handing `probe` each system's name and the
    /// world **as that system left it** — the divergence-localization seam.
    ///
    /// This is deliberately *not* a [`ScheduleObserver`]: an observer is state-free by design (it
    /// sees names, never data), and localizing a divergence needs exactly the thing an observer
    /// refuses to show — the state after each system, so two runs of the same tick can be compared
    /// boundary by boundary until one differs.
    ///
    /// The probe only reads. It cannot change what runs or in what order, so the world this leaves —
    /// and its `WorldHash` — is exactly what [`run`](Schedule::run) would leave.
    ///
    /// Attribution is unchanged: each system runs with the world's recording system set to its name,
    /// and the probe runs with it **cleared** — the probe is host-side work, not the system's, and
    /// its reads must not be attributed to whichever system it just followed.
    pub fn run_probed(
        &mut self,
        world: &mut World,
        ctx: &SystemCtx,
        probe: &mut dyn FnMut(&'static str, &World),
    ) {
        self.run_with(world, ctx, &mut ProbeBoundary(probe));
    }

    /// The one loop behind [`run`](Schedule::run), [`run_observed`](Schedule::run_observed) and
    /// [`run_probed`](Schedule::run_probed): each system's boundary hooks bracket its guarded run,
    /// and a caught panic ends the tick after `after` has closed the system's span.
    fn run_with(&mut self, world: &mut World, ctx: &SystemCtx, boundary: &mut dyn Boundary) {
        panic_capture::install_hook();
        for system in &self.systems {
            // Attribute any component access this system makes to its name (a no-op unless the world
            // has opt-in recording enabled — see `World::set_recording_system`).
            world.set_recording_system(system.name);
            boundary.before(system.name);
            let completed = run_guarded(system, world, ctx);
            // `after` runs on the panic path too — control did come back here — so an observer's
            // before/after pairs stay balanced.
            boundary.after(system.name, world);
            if !completed {
                break;
            }
        }
        // Whatever the host does between ticks — extract, hash, apply commands — is nobody's system.
        world.clear_recording_system();
        world.set_phase(Phase::Host, None);
    }

    /// The system names, in registration order.
    pub fn system_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.systems.iter().map(|s| s.name)
    }

    /// Each system's name and [`Phase`], in registration order.
    pub fn system_phases(&self) -> impl Iterator<Item = (&'static str, Phase)> + '_ {
        self.systems.iter().map(|s| (s.name, s.phase))
    }

    /// The number of registered systems.
    pub fn len(&self) -> usize {
        self.systems.len()
    }

    /// Whether no systems are registered.
    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_core::time::TickRate;

    #[test]
    fn a_new_ctx_is_the_identity_clock() {
        let ctx = SystemCtx::new(FrameIndex::new(5), TickRate::DEFAULT);
        assert_eq!(ctx.game_scale(), 1.0);
        assert_eq!(ctx.game_frames_crossed(), 1);
        assert_eq!(ctx.game_instant, GameTime::from_ticks(5, TickRate::DEFAULT));
        let via = ctx.with_clock(&GameClock::default(), true);
        assert_eq!(via.game_dt, ctx.dt);
        assert_eq!(via.game_frames_crossed(), 1);
    }

    #[test]
    fn a_quarter_scale_crosses_a_frame_every_fourth_tick() {
        let mut clock = GameClock::default();
        let mut crossed = Vec::new();
        for frame in 1..=8u64 {
            clock.scale(0.25);
            let advanced = clock.advance();
            let ctx = SystemCtx::new(FrameIndex::new(frame), TickRate::DEFAULT)
                .with_clock(&clock, advanced);
            assert_eq!(ctx.game_scale(), 0.25);
            assert_eq!(ctx.game_dt, ctx.dt * 0.25);
            assert!(ctx.game_advanced());
            crossed.push(ctx.game_frames_crossed());
        }
        assert_eq!(crossed, vec![0, 0, 0, 1, 0, 0, 0, 1]);
    }

    #[test]
    fn a_held_ctx_crosses_nothing() {
        let mut clock = GameClock::default();
        clock.hold();
        let advanced = clock.advance();
        let ctx =
            SystemCtx::new(FrameIndex::new(3), TickRate::DEFAULT).with_clock(&clock, advanced);
        assert_eq!(ctx.game_frames_crossed(), 0);
        assert_eq!(ctx.game_scale(), 0.0);
        assert_eq!(ctx.game_dt, 0.0);
        assert!(!ctx.game_advanced());
    }

    fn ctx() -> SystemCtx {
        SystemCtx::new(FrameIndex::ZERO, TickRate::DEFAULT)
    }

    #[test]
    fn add_system_shared_runs_in_registration_order() {
        // A world resource records the order systems mutate it; three closures registered through
        // `add_system_shared` must run in the exact order added (the determinism contract).
        let mut world = World::new();
        world.insert_resource(Vec::<u32>::new());

        let mut schedule = Schedule::new();
        for (name, tag) in [("tag_a", 10u32), ("tag_b", 20), ("tag_c", 30)] {
            let run: SystemFn = Arc::new(move |world: &mut World, _ctx: &SystemCtx| {
                world.resource_mut::<Vec<u32>>().unwrap().push(tag);
            });
            schedule.add_system_shared(name, run);
        }

        schedule.run(&mut world, &ctx());
        assert_eq!(*world.resource::<Vec<u32>>().unwrap(), vec![10, 20, 30]);
    }

    #[test]
    #[should_panic(expected = "L4018")]
    fn a_duplicate_system_name_is_refused() {
        let mut schedule = Schedule::new();
        schedule.add_system("tick", |_: &mut World, _: &SystemCtx| {});
        schedule.add_system("tick", |_: &mut World, _: &SystemCtx| {});
    }

    /// A schedule whose middle system panics: `first` runs, `boom` panics, `last` is skipped.
    fn schedule_with_a_panicking_middle() -> (World, Schedule) {
        fn first(world: &mut World, _ctx: &SystemCtx) {
            world.resource_mut::<Vec<u32>>().unwrap().push(1);
        }
        fn boom(world: &mut World, _ctx: &SystemCtx) {
            // The shape a game hits: unwrapping a resource the world does not carry.
            world
                .resource_mut::<Vec<i8>>()
                .expect("the enemy's Health")
                .push(2);
        }
        fn last(world: &mut World, _ctx: &SystemCtx) {
            world.resource_mut::<Vec<u32>>().unwrap().push(3);
        }

        let mut world = World::new();
        world.insert_resource(Vec::<u32>::new());
        let mut schedule = Schedule::new();
        schedule.add_system("first", first);
        schedule.add_system("boom", boom);
        schedule.add_system("last", last);
        (world, schedule)
    }

    #[test]
    fn a_panicking_system_is_caught_named_and_stops_the_tick() {
        let (mut world, mut schedule) = schedule_with_a_panicking_middle();

        schedule.run(&mut world, &ctx());

        let panic = world
            .resource::<SystemPanic>()
            .expect("the caught panic is left in the world for the host");
        assert_eq!(panic.system, "boom");
        assert!(
            panic.message.contains("the enemy's Health"),
            "the payload text must survive: {}",
            panic.message
        );
        assert!(
            panic
                .location
                .as_deref()
                .is_some_and(|at| at.contains("schedule.rs")),
            "the hook must record the panic site: {:?}",
            panic.location
        );
        assert_eq!(
            *world.resource::<Vec<u32>>().unwrap(),
            vec![1],
            "systems before the panic ran; the ones after it were skipped"
        );
    }

    #[test]
    fn an_observer_sees_a_balanced_span_for_a_panicking_system() {
        #[derive(Default)]
        struct Trace(Vec<String>);
        impl ScheduleObserver for Trace {
            fn before_system(&mut self, name: &'static str) {
                self.0.push(format!("before {name}"));
            }
            fn after_system(&mut self, name: &'static str) {
                self.0.push(format!("after {name}"));
            }
        }

        let (mut world, mut schedule) = schedule_with_a_panicking_middle();
        let mut trace = Trace::default();
        schedule.run_observed(&mut world, &ctx(), &mut trace);

        assert_eq!(
            trace.0,
            ["before first", "after first", "before boom", "after boom"],
            "the panicking system's span closes, and no system after it opens one"
        );
    }

    #[test]
    fn the_world_still_works_after_a_caught_panic() {
        let (mut world, mut schedule) = schedule_with_a_panicking_middle();
        schedule.run(&mut world, &ctx());

        // The whole point: the caller is still standing and holds a usable world.
        let entity = world.spawn();
        assert!(world.is_alive(entity));
        world.remove_resource::<SystemPanic>();
        world.insert_resource(Vec::<u32>::new());

        // With the record cleared, a rerun reports the panic afresh — it is a per-tick observation,
        // not a latch.
        schedule.run(&mut world, &ctx());
        assert_eq!(world.resource::<SystemPanic>().unwrap().system, "boom");
    }

    #[test]
    fn a_clean_run_leaves_no_panic_record() {
        let mut world = World::new();
        world.insert_resource(0u32);
        let mut schedule = Schedule::new();
        schedule.add_system("tick", |world: &mut World, _ctx: &SystemCtx| {
            *world.resource_mut::<u32>().unwrap() += 1;
        });

        schedule.run(&mut world, &ctx());
        schedule.run(&mut world, &ctx());

        assert_eq!(*world.resource::<u32>().unwrap(), 2);
        assert!(world.resource::<SystemPanic>().is_none());
    }
}
