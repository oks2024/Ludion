//! The live driver seam: the channels a transport plugs into, and [`run_live`] — the sim-thread loop.
//!
//! This is the engine half of the threading model in engine-architecture.md §3. The engine owns the
//! world on one thread and answers every command through a single queue; a transport (the
//! `ludion-server` WebSocket host) pushes work into that queue and reads replies back. The seam is
//! deliberately transport-agnostic and **tokio-free**: it is two [`crossbeam_channel`]s, nothing
//! more, so `ludion-engine` never depends on an async runtime (the boundary rule that keeps the
//! server plugging into the engine, not the reverse).
//!
//! - **Intake** — a [`crossbeam_channel`] of [`StampedCommand`]s. Any number of producers (one per
//!   connection) push commands; the sim thread is the single consumer.
//! - **Reply** — each [`StampedCommand`] carries its own capacity-1 reply channel. The sim thread
//!   sends the [`Dispatched`] result (or the command's [`Diagnostic`]) back on it; that send never
//!   blocks (the channel has room for exactly the one reply), so the sim loop can never stall on a
//!   slow consumer. The bridge is race-free by construction: one command, one reply, one receiver.
//!   A handler that panics is answered `L4016` and pauses the sim; the loop keeps serving.
//!
//! # The one wall-clock site
//!
//! [`run_live`] is the *only* place in the sim-side crates that reads the wall clock, and it uses it
//! solely to decide **when** to tick — never to compute anything a tick observes (architecture §3).
//! The crate's clippy denylist bans [`std::time::Instant::now`] everywhere; this one function scopes
//! an `allow` with that rationale. Everything a tick sees still arrives as `SimTime` / `FrameIndex`.
//!
//! # Example
//! ```no_run
//! use std::thread;
//! use ludion_engine::{Engine, live::{run_live, shutdown_channel, StampedCommand}};
//! use ludion_protocol::Command;
//!
//! let mut engine = Engine::new();
//! engine.load_inline_scene("format: 1\nentities: []\n").unwrap();
//!
//! let (intake_tx, intake_rx) = crossbeam_channel::unbounded();
//! let (shutdown_tx, shutdown_rx) = shutdown_channel();
//! // `None` for the egress channels: this transport reads command replies only, not the frame/event
//! // stream. A viewer would pass `Some(EgressChannels { frames, events })` — a bounded(1) frame slot
//! // and an unbounded event lane — and drain rendered frames + events.
//! let sim = thread::spawn(move || run_live(engine, intake_rx, shutdown_rx, None));
//!
//! // A transport thread turns a wire request into a StampedCommand and blocks for the reply.
//! let (cmd, reply) = StampedCommand::new(Command::SimHash);
//! intake_tx.send(cmd).unwrap();
//! let dispatched = reply.recv().unwrap().unwrap();
//! println!("world hash at frame {}", dispatched.frame.as_u64());
//!
//! shutdown_tx.signal();
//! sim.join().unwrap().unwrap();
//! ```

use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TryRecvError};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::FrameIndex;
use ludion_ecs::panic_payload_text;
use ludion_protocol::event::{Event, LogEvent};
use ludion_protocol::perf::MAX_CAPTURE_WALL_SECONDS;
use ludion_protocol::{Channel, Command, EventMessage, ResponseBody};
use ludion_sim::{RunState, Speed};

use crate::Engine;
use crate::commands;
use crate::dispatch::Dispatched;
use crate::egress::{Egress, EgressChannels, EgressEvent, EgressFrame, event_channel};

/// The result the sim thread sends back for one [`StampedCommand`] — exactly what
/// [`Engine::execute`](crate::Engine::execute) returns.
pub type CommandResult = Result<Dispatched, Diagnostic>;

/// The receiving half of a command's one-shot reply channel (held by the transport that submitted
/// the command).
pub type ReplyReceiver = Receiver<CommandResult>;

/// The sending half of a command's one-shot reply channel (carried by the [`StampedCommand`] into
/// the sim thread).
pub type ReplySender = Sender<CommandResult>;

/// A command in flight to the sim thread, paired with the one-shot channel its reply comes back on.
///
/// Build one with [`StampedCommand::new`], push it into the intake channel, and block on the
/// returned [`ReplyReceiver`]. The sim thread runs the command through
/// [`Engine::execute`](crate::Engine::execute) at a frame boundary and sends the result back.
pub struct StampedCommand {
    /// The command to run.
    pub command: Command,
    /// The one-shot sender the sim thread replies on (a non-blocking, capacity-1 send).
    pub reply: ReplySender,
}

impl StampedCommand {
    /// Pairs `command` with a fresh one-shot reply channel, returning the stamped command (to push
    /// into the intake channel) and the receiver its reply will arrive on.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::live::StampedCommand;
    /// use ludion_protocol::Command;
    ///
    /// let (stamped, reply) = StampedCommand::new(Command::SimHash);
    /// assert_eq!(stamped.command.method(), "sim.hash");
    /// drop(reply); // (a real caller blocks on `reply.recv()`)
    /// ```
    pub fn new(command: Command) -> (StampedCommand, ReplyReceiver) {
        // Capacity 1: the sim thread's single reply always fits, so its send never blocks.
        let (reply, receiver) = crossbeam_channel::bounded(1);
        (StampedCommand { command, reply }, receiver)
    }
}

/// Signals the [`run_live`] loop to stop. Cloneable and safe to drop; either dropping every clone
/// or calling [`ShutdownSignal::signal`] stops the loop.
#[derive(Clone)]
pub struct ShutdownSignal(Sender<()>, Arc<AtomicU32>);

impl ShutdownSignal {
    /// Tells the sim loop to finish its current drain-and-tick pass and return.
    pub fn signal(&self) {
        // A capacity-1 channel; the loop only ever needs to observe one signal.
        let _ = self.0.try_send(());
    }

    /// What the sim loop is doing right now — what a host that gave up waiting for it reports.
    pub fn activity(&self) -> Activity {
        Activity::decode(self.1.load(Ordering::Relaxed))
    }
}

/// The receiving half of a [`ShutdownSignal`], handed to [`run_live`].
pub struct ShutdownReceiver(Receiver<()>, Arc<AtomicU32>);

/// Creates a linked [`ShutdownSignal`] / [`ShutdownReceiver`] pair for a [`run_live`] loop.
pub fn shutdown_channel() -> (ShutdownSignal, ShutdownReceiver) {
    let (tx, rx) = crossbeam_channel::bounded(1);
    let activity = Arc::new(AtomicU32::new(ACTIVITY_BOOTING));
    (
        ShutdownSignal(tx, activity.clone()),
        ShutdownReceiver(rx, activity),
    )
}

const ACTIVITY_IDLE: u32 = 0;
const ACTIVITY_TICK: u32 = 1;
const ACTIVITY_EMIT: u32 = 2;
const ACTIVITY_BOOTING: u32 = 3;
const ACTIVITY_STOPPING: u32 = 4;
/// A running command is `ACTIVITY_COMMAND + i`, `i` its index in [`Command::METHODS`].
const ACTIVITY_COMMAND: u32 = 5;

/// What the sim loop is in the middle of. The loop looks at the shutdown signal only between these
/// steps, so the one it is in is what a stuck stop is waiting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    /// Between steps, or parked.
    Idle,
    /// Running the command with this wire method name.
    Command(&'static str),
    /// Running a sim tick.
    Tick,
    /// Rendering and emitting a frame and events.
    Emit,
    /// Before the loop starts: loading the scene and building the renderer.
    Booting,
    /// After the loop returned: dropping the engine, flushing a recording, releasing the GPU.
    Stopping,
}

impl Activity {
    fn decode(value: u32) -> Activity {
        match value {
            ACTIVITY_IDLE => Activity::Idle,
            ACTIVITY_TICK => Activity::Tick,
            ACTIVITY_EMIT => Activity::Emit,
            ACTIVITY_BOOTING => Activity::Booting,
            ACTIVITY_STOPPING => Activity::Stopping,
            n => Command::METHODS
                .get((n - ACTIVITY_COMMAND) as usize)
                .map_or(Activity::Idle, |method| Activity::Command(method)),
        }
    }

    fn command_code(method: &str) -> u32 {
        Command::METHODS
            .iter()
            .position(|m| *m == method)
            .map_or(ACTIVITY_IDLE, |i| ACTIVITY_COMMAND + i as u32)
    }
}

/// How long the loop parks between polls while the engine is paused (no tick deadline to wake for).
///
/// Small enough that a `sim.resume` or `sim.step` arriving over the wire is noticed promptly, large
/// enough that an idle paused engine costs no measurable CPU.
const PAUSED_POLL: Duration = Duration::from_millis(20);

/// The sim-thread loop: drain every pending command, tick if the run-state and wall clock allow, then
/// emit any rendered frame and subscription events over the optional egress channel.
///
/// This owns the [`Engine`] for the lifetime of the loop (the "sim thread solely owns `World`" rule)
/// and is the single arrangement `ludion-server`'s live mode runs. One pass is:
///
/// 1. **Drain** every command currently in the intake channel, each through
///    [`Engine::execute`](Engine::execute) (so recording, if armed, sees them), replying on each
///    command's one-shot channel. A *paused* engine still reaches this step and still answers —
///    reads, `sim.pause`/`sim.step`, and mutations all apply here at the frame boundary.
/// 2. **Tick** at most once, if the run-state is not [`Paused`](RunState::Paused) and enough
///    wall-clock time has elapsed for the next fixed-rate tick (the rate scaled by the current
///    [`speed`](Engine::speed)). A speed change from any source re-bases the deadline first, so a
///    slow deadline never strands a raised speed. Ticking reuses the driver's own [`advance`] — no
///    new stepping logic — so `Running` free-runs and an in-flight `sim.step` decrements exactly as
///    elsewhere.
/// 3. **Emit** (only when an `egress` sender was supplied, or a window is attached — a window is
///    presented even with no transport): fan every queued event out to its
///    channel's subscribers, then render one frame for the `frames` subscribers — but only if the
///    world may have changed since the last emitted frame and the egress slot has room. A render
///    failure that is not a missing GPU is surfaced as a deduplicated error-level `log` event rather
///    than swallowed, so a stream that stops producing frames says why. See `emit_egress`.
/// 4. **Park** on the two channels until the next command arrives, the shutdown fires, or the tick
///    deadline is reached — so the loop never busy-spins.
///
/// The wall clock is confined to this function: it schedules ticks and touches nothing a tick reads.
/// Frame rendering (step 3) is a pure read of the world — it never mutates sim state or the
/// `WorldHash`, so an attached frame subscriber cannot perturb determinism.
///
/// `egress` is the outbound seam (see [`crate::egress`]): the two sender halves a transport hands over
/// to receive [`Egress`] messages ([`EgressChannels`] — a latest-wins frame lane and an ordered event
/// lane), or `None` for a transport that reads command replies only (an attached window is still
/// presented, with no readback). A `crossbeam_channel::bounded(1)`
/// frame sender gives the intended latest-wins-at-source behavior; the event sender should be unbounded
/// so a burst of events is never dropped at this seam.
///
/// The loop returns when [`ShutdownSignal::signal`] fires (or its every clone is dropped), or when
/// the intake channel disconnects (every producer gone). It then stops an armed recording, so the
/// file is complete before the host reports a clean exit.
///
/// # Errors
/// The armed recording's failure, either `L3042` (it died mid-session) or the error of its final
/// flush. The loop has already ended, so the host reports it as a failed exit.
///
/// [`advance`]: ludion_sim::SimDriver::advance
pub fn run_live(
    mut engine: Engine,
    intake: Receiver<StampedCommand>,
    shutdown: ShutdownReceiver,
    egress: Option<EgressChannels>,
) -> Result<(), Diagnostic> {
    shutdown.1.store(ACTIVITY_IDLE, Ordering::Relaxed);
    live_loop(&mut engine, intake, &shutdown, egress);
    // The engine (wgpu, a recording's flush) drops after this store, so a stop that hangs there is
    // reported as winding down rather than as "between steps".
    shutdown.1.store(ACTIVITY_STOPPING, Ordering::Relaxed);
    engine.stop_recording()
}

/// The most commands one boundary applies. A client pipelining faster than `reply` executes would
/// otherwise keep the channel non-empty forever and starve the tick, the egress and the capture
/// flush; the remainder keeps its order and is drained at the next boundary.
const MAX_COMMANDS_PER_DRAIN: usize = 1024;

/// Applies the pending commands in arrival order, at most [`MAX_COMMANDS_PER_DRAIN`], returning how
/// many it applied, or `None` once every producer is gone.
fn drain(
    engine: &mut Engine,
    intake: &Receiver<StampedCommand>,
    pending_capture: &mut Option<PendingCapture>,
    egress_state: &mut EgressState,
    activity: &AtomicU32,
) -> Option<usize> {
    let mut applied = 0;
    while applied < MAX_COMMANDS_PER_DRAIN {
        match intake.try_recv() {
            Ok(stamped) => {
                // Only a command that can change the image owes a frame subscriber a render the
                // frame index would not already trigger (a `component.set` does; a poll or a
                // queued input does not). Re-submitting an unchanged world on every poll fills
                // the readback ring, and a full ring makes the next collect block this thread.
                let dirty = stamped.command.may_change_frame();
                // `or` keeps an already-held capture: a reply we owe is never abandoned for a newer
                // one (the handler refuses a second capture anyway, so this cannot fire).
                *pending_capture = pending_capture.take().or(reply(engine, stamped, activity));
                egress_state.frame_dirty |= dirty;
                applied += 1;
            }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => return None,
        }
    }
    Some(applied)
}

// The live scheduler is the one legitimate wall-clock site in the sim-side crates
// (engine-architecture.md §3): `Instant::now` decides only *when* to tick, never what a tick
// observes. The crate clippy denylist bans it everywhere else; this allow is scoped to exactly this
// function with that rationale.
#[allow(clippy::disallowed_methods)]
fn live_loop(
    engine: &mut Engine,
    intake: Receiver<StampedCommand>,
    shutdown: &ShutdownReceiver,
    egress: Option<EgressChannels>,
) {
    let mut next_tick = Instant::now();
    let mut paced_speed = engine.speed();
    let mut egress_state = EgressState::new(engine.subscriptions().frames_opened());
    let mut pending_capture: Option<PendingCapture> = None;
    // Read once, never per-pass: an unset `LUDION_TICK_TRACE` costs one `Option` test per tick and
    // nothing else (the same discipline as the live profiler's `Option<Box<LivePerf>>`). See
    // `TickTrace` for what it measures and why.
    let mut tick_trace = TickTrace::from_env(tick_period(engine));

    loop {
        // 1. Drain the commands pending at this frame boundary, up to the cap.
        if drain(
            engine,
            &intake,
            &mut pending_capture,
            &mut egress_state,
            &shutdown.1,
        )
        .is_none()
        {
            return; // every producer is gone
        }

        // 1b. Match the profiler's armed state to what the drain left behind (see
        //     `sync_perf_arming`) — a `profile` subscribe/unsubscribe is the whole on/off switch.
        sync_perf_arming(engine);

        // 2. Shutdown requested — by the transport (the server's shutdown signal), or by an
        //    `engine.shutdown` command drained just above. In the command case the reply (Ack) has
        //    already been sent on that command's one-shot channel in the drain, so returning now lets
        //    the host flush it and run the same graceful teardown Ctrl-C triggers.
        match shutdown.0.try_recv() {
            Ok(()) | Err(TryRecvError::Disconnected) => return,
            Err(TryRecvError::Empty) => {}
        }
        if engine.shutdown_requested() {
            return;
        }

        // 3. Tick when the run-state allows and the wall clock says the next tick is due. A speed
        //    change, from any source, first re-bases the deadline: it was computed at the old speed.
        if engine.speed() != paced_speed {
            paced_speed = engine.speed();
            next_tick = Instant::now() + tick_period(engine);
        }
        let paused = engine.run_state() == RunState::Paused;
        if !paused {
            let now = Instant::now();
            if now >= next_tick {
                // `now` is already read for the deadline check above; the trace reuses it rather
                // than taking a second reading, so an armed trace never adds a clock read the loop
                // did not already pay for.
                if let Some(trace) = &mut tick_trace {
                    trace.record(now, next_tick);
                }
                let period = tick_period(engine);
                shutdown.1.store(ACTIVITY_TICK, Ordering::Relaxed);
                engine.advance_live();
                shutdown.1.store(ACTIVITY_IDLE, Ordering::Relaxed);
                // Schedule the next tick one period on. If we fell more than a whole period behind
                // (a long drain, a scheduler hiccup), reset the baseline to *now* instead of firing
                // a burst of catch-up ticks — a live view paces by the clock, it does not owe the
                // sim a fixed tick count (deterministic verification uses lockstep, not this loop).
                next_tick += period;
                if next_tick <= now {
                    next_tick = now + period;
                }
            }
        }

        // 4. Emit rendered frames and events to a subscribed transport (no-op without an egress
        //    channel, or with no matching subscribers), and present into an attached window even with
        //    no transport at all. Any performance window that closed this pass joins the event outbox
        //    first, so it rides the ordinary ordered event lane.
        shutdown.1.store(ACTIVITY_EMIT, Ordering::Relaxed);
        if egress.is_some() {
            publish_perf_windows(engine);
        }
        if let Some(fault) = engine.take_queued_render_fault() {
            absorb_render_failure(engine, &mut egress_state, fault);
        }
        if egress.is_some() || engine.present_attached() {
            emit_egress(engine, egress.as_ref(), &mut egress_state);
        }
        shutdown.1.store(ACTIVITY_IDLE, Ordering::Relaxed);

        // 5. Answer a `profile.capture` whose window has now closed (see `PendingCapture`).
        flush_pending_capture(engine, &mut pending_capture);
        if pending_capture.is_some() {
            abandon_unclosable_capture(engine, &mut pending_capture, Instant::now());
        }

        // 6. Park until the next command, the shutdown, or the tick deadline — never busy-spin.
        let timeout = if engine.run_state() == RunState::Paused {
            PAUSED_POLL
        } else {
            next_tick.saturating_duration_since(Instant::now())
        };
        crossbeam_channel::select! {
            recv(intake) -> msg => match msg {
                Ok(stamped) => {
                    let dirty = stamped.command.may_change_frame();
                    pending_capture = pending_capture.or(reply(engine, stamped, &shutdown.1));
                    egress_state.frame_dirty |= dirty;
                }
                Err(_) => return, // disconnected
            },
            recv(shutdown.0) -> _ => return,
            default(timeout) => {}
        }
    }
}

/// The `L4017` diagnostic for a sim thread whose `join()` returned this panic payload: a panic
/// outside the per-command and per-system unwind boundaries. The one builder the server teardown and
/// the embed host share.
pub fn sim_thread_panicked(payload: &(dyn std::any::Any + Send)) -> Diagnostic {
    Diagnostic::new(
        code::SIM_THREAD_PANICKED,
        format!("the sim thread panicked: {}", panic_payload_text(payload)),
    )
    .with_help(
        "the panic's location is on the host's stderr; restart the host to get a fresh sim (`ludion explain L4017`)",
    )
}

/// How long a host waits for the sim thread to stop before it reports [`SIM_THREAD_STUCK`] and
/// leaves the thread detached.
///
/// [`SIM_THREAD_STUCK`]: code::SIM_THREAD_STUCK
pub const SIM_STOP_GRACE: Duration = Duration::from_secs(5);

/// How often [`join_sim`] re-checks whether the thread has finished.
const JOIN_POLL: Duration = Duration::from_millis(5);

/// Joins the sim thread, waiting at most `grace`.
///
/// A finished thread yields its `join()` result (an `Err` is a panic payload, for
/// [`sim_thread_panicked`]). One that is still running after `grace` is detached — dropping the
/// handle — and the answer is the `L4022` diagnostic naming what `signal` says it is doing.
/// Polls `is_finished` with a sleep count rather than a clock read: the crate's clippy denylist bans
/// `Instant::now` outside [`run_live`].
///
/// # Errors
/// `L4022` when the thread does not finish within `grace`.
pub fn join_sim<T>(
    handle: JoinHandle<T>,
    grace: Duration,
    signal: &ShutdownSignal,
) -> Result<std::thread::Result<T>, Diagnostic> {
    let polls = grace.as_millis() / JOIN_POLL.as_millis();
    for _ in 0..=polls {
        if handle.is_finished() {
            return Ok(handle.join());
        }
        std::thread::sleep(JOIN_POLL);
    }
    drop(handle);
    Err(sim_thread_stuck(signal.activity(), grace))
}

/// The `L4022` diagnostic for a sim thread still busy `grace` after it was asked to stop.
pub fn sim_thread_stuck(activity: Activity, grace: Duration) -> Diagnostic {
    let doing = match activity {
        Activity::Command(method) => format!("still running `{method}`"),
        Activity::Tick => "still running a sim tick".to_string(),
        Activity::Emit => "still rendering a frame".to_string(),
        Activity::Idle => "still between steps".to_string(),
        Activity::Booting => "still booting (loading the scene, building the renderer)".to_string(),
        Activity::Stopping => {
            "still winding down after its last step (dropping the engine, flushing a recording, releasing the GPU)"
                .to_string()
        }
    };
    let help = match activity {
        Activity::Booting | Activity::Stopping => {
            "the stop is slow, not wedged: a big scene or a recording flush can outlast the grace; the host exits without waiting and an armed recording may be incomplete (`ludion explain L4022`)"
        }
        _ => {
            "a command or system that never returns holds the sim (a loop in a system, a huge `sim.step`); the host exits without it and an armed recording is incomplete — split long steps (`ludion explain L4022`)"
        }
    };
    Diagnostic::new(
        code::SIM_THREAD_STUCK,
        format!(
            "the sim thread did not stop within {} s: it is {doing}",
            grace.as_secs_f64()
        ),
    )
    .with_help(help)
}

/// Runs one command through the recording-aware entry point and sends its result back.
///
/// A handler that panics does not take the loop down: the unwind is caught here and the request is
/// answered with `L4016` (see [`Engine::command_panicked`]), the sim paused.
///
/// The reply send is non-blocking (capacity-1 channel) and ignores a dropped receiver: if the
/// transport gave up on the request, the sim thread simply moves on.
///
/// One command does not answer here: a successful `profile.capture` measures frames that have not
/// run yet, so its reply channel comes back to the caller as a [`PendingCapture`] instead of being
/// used (see [`flush_pending_capture`]). A *failed* capture — a bad frame count, a capture already
/// in flight — answers immediately like everything else.
#[must_use = "a pending capture's reply channel must be held until its window closes"]
fn reply(
    engine: &mut Engine,
    stamped: StampedCommand,
    activity: &AtomicU32,
) -> Option<PendingCapture> {
    let capture = match &stamped.command {
        Command::ProfileCapture(params) => Some(params.include_samples),
        _ => None,
    };
    let method = stamped.command.method();
    activity.store(Activity::command_code(method), Ordering::Relaxed);
    // `AssertUnwindSafe` is a real claim: a handler may have half-applied its edit before it
    // panicked. It is sound because `command_panicked` pauses the sim, so no tick runs on the
    // wreckage, and the world is left as the panic left it — that shape is the evidence.
    let result = match panic::catch_unwind(AssertUnwindSafe(|| engine.execute(stamped.command))) {
        Ok(result) => result,
        Err(payload) => Err(engine.command_panicked(method, &panic_payload_text(payload.as_ref()))),
    };
    activity.store(ACTIVITY_IDLE, Ordering::Relaxed);
    if let (Some(include_samples), Ok(_)) = (capture, &result) {
        return Some(PendingCapture {
            reply: stamped.reply,
            include_samples,
            armed_at: None,
            seen: None,
        });
    }
    let _ = stamped.reply.try_send(result);
    None
}

/// A `profile.capture` that has been armed but not yet answered.
///
/// The command's handler can only *start* a capture: the report describes live frames that run after
/// the dispatch returns. So the loop keeps the request's one-shot reply channel here and answers it
/// with the finished [`ResponseBody::ProfileReport`] when the window closes — the wire client sees
/// one ordinary response to its request id, just a later one.
///
/// Exactly one can be outstanding: while a capture is in flight the handler refuses another
/// (`L4013`), so this is never overwritten. If the loop returns first (shutdown, or every producer
/// gone) the channel drops unanswered, which the transport already treats as any other in-flight
/// request at teardown.
struct PendingCapture {
    /// The request's one-shot reply channel, held open across the capture's frames.
    reply: ReplySender,
    /// Whether the request asked for the window's raw per-frame columns.
    include_samples: bool,
    /// When `run_live` first saw this capture held — the start of its wall-clock budget. Set there,
    /// not in `reply`, which stays outside the clock allow.
    armed_at: Option<Instant>,
    /// The run state and speed the last cancellation check saw; the check repeats only on a change.
    seen: Option<(RunState, Speed)>,
}

/// Arms or disarms the live profiler to match what is actually being watched.
///
/// Subscribing to the `profile` channel **is** the arming switch — there is no start/stop verb — so
/// the rule is one line: measure while someone is subscribed or a capture is in flight, and pay
/// nothing otherwise. Unsubscribing disarms, and so does a disconnect, which the transport turns
/// into a `stream.unsubscribe` for every subscription the connection owned.
fn sync_perf_arming(engine: &mut Engine) {
    let wanted =
        engine.subscriptions().has_subscriber(Channel::Profile) || engine.perf_capture_in_flight();
    if wanted {
        engine.perf_arm();
    } else if engine.perf_armed() {
        engine.perf_disarm();
    }
}

/// Moves every performance window that closed since the last pass into the event outbox.
///
/// From there a window is an ordinary [`Event`]: `emit_egress` addresses it to each `profile`
/// subscriber with that subscription's own sequence and sends it on the ordered event lane, exactly
/// like a log record. Windows with no subscriber are dropped there, as all events are.
fn publish_perf_windows(engine: &mut Engine) {
    for window in engine.perf_take_windows() {
        engine.push_event(Event::ProfileWindow(Box::new(window)));
    }
}

/// Answers a held [`PendingCapture`] once its window has closed; a no-op until then.
///
/// The report is labelled with the loaded scene, so a capture reads like the offline driver's report
/// of the same scene — the shape a budget file or a trace exporter already understands.
fn flush_pending_capture(engine: &mut Engine, pending: &mut Option<PendingCapture>) {
    if pending.is_none() {
        return;
    }
    let Some(window) = engine.perf_take_capture() else {
        return;
    };
    let scene = engine.scene_name().unwrap_or("live").to_string();
    let entry = pending.take().expect("checked above");
    let mut report = window.into_report(scene, entry.include_samples);
    // A window measures frames; the quality they were rendered at belongs to this host, so it is
    // filled here rather than carried through the conversion (the `transport` pattern).
    report.quality = Some(engine.quality().descriptor());
    let _ = entry.reply.try_send(Ok(Dispatched {
        body: ResponseBody::ProfileReport {
            report: Box::new(report),
        },
        frame: engine.frame(),
        game_frame: engine.game_frame(),
    }));
}

/// Cancels a held capture that can no longer close and answers it `L4023`.
///
/// A capture's window closes on sim frames, so a `sim.pause` or a `sim.set_speed` after arming can
/// leave it unable to finish within what is left of its wall-clock budget. Called after
/// [`flush_pending_capture`], so a capture a `sim.step` completed is answered with its report.
///
/// Re-checks only when the run state or speed differs from the last pass's: a host slower than its
/// tick rate must not be cancelled by its own lateness. The server's 2× reply budget is the backstop
/// for a capture that merely runs long.
fn abandon_unclosable_capture(
    engine: &mut Engine,
    pending: &mut Option<PendingCapture>,
    now: Instant,
) {
    let Some(entry) = pending.as_mut() else {
        return;
    };
    let armed_at = *entry.armed_at.get_or_insert(now);
    let state = (engine.run_state(), engine.speed());
    if entry.seen == Some(state) {
        return;
    }
    entry.seen = Some(state);
    let Some(remaining) = engine.perf_capture_remaining() else {
        return;
    };
    let budget_left = Duration::from_secs_f64(MAX_CAPTURE_WALL_SECONDS)
        .saturating_sub(now.saturating_duration_since(armed_at));
    let Some(diag) = commands::perf::cannot_close(engine, remaining, budget_left, true) else {
        return;
    };
    engine.perf_cancel_capture();
    let entry = pending.take().expect("checked above");
    let _ = entry.reply.try_send(Err(diag));
}

/// The cross-pass state the egress step carries between loop iterations.
struct EgressState {
    /// The frame index of the last render *submitted* to the stream ring (`None` before any). Gates
    /// re-submission: with submit and collect decoupled, a frame already in flight for an unchanged
    /// world must not be submitted again — so this tracks submission, not delivery.
    last_submitted: Option<FrameIndex>,
    /// Whether a command that [`may_change_frame`](Command::may_change_frame) has been drained since
    /// the last submitted frame — so a paused engine that received a mutation still refreshes exactly
    /// once, while a poll or a queued input (applied at the next tick, which advances the frame index
    /// on its own) submits nothing.
    frame_dirty: bool,
    /// The frame index last blitted into an attached window (`None` before any, and on every headless
    /// host). A frame is presented at most once: the window is redrawn when the frame index moves,
    /// never on the wire client's schedule — the present shares no slot with the stream and waits on
    /// nothing.
    last_presented: Option<FrameIndex>,
    /// Set once a render reports no GPU adapter (`L2003`), a lost device (`L2011`) or a dead window
    /// surface (`L2007`); suppresses further render attempts until a new frame subscription opens,
    /// so a dead GPU is not re-extracted every pass.
    gpu_unavailable: bool,
    /// The `frames_opened` epoch last observed — a change means a new frame subscription opened, which
    /// clears [`gpu_unavailable`](EgressState::gpu_unavailable) so the adapter is retried.
    frames_epoch: u64,
    /// The rendered text of the last render failure surfaced as a `log` event, so an identical failure
    /// repeating every pass is logged only *once*. Reset to `None` by a render success (or a different
    /// failure replaces it), so a genuinely new problem — or the same one after a recovery — logs again.
    /// A new frame subscription resets it too, so a reconnecting client is told why the stream is
    /// dead.
    last_render_error: Option<String>,
}

impl EgressState {
    fn new(frames_epoch: u64) -> Self {
        EgressState {
            last_submitted: None,
            frame_dirty: false,
            last_presented: None,
            gpu_unavailable: false,
            frames_epoch,
            last_render_error: None,
        }
    }
}

/// A newly opened frame subscription clears a latched no-GPU state so the adapter is retried (and
/// never otherwise, so an adapterless host does not thrash the request). It also clears the
/// logged-error latch, so the new subscriber is told why the stream is dead.
fn observe_frames_epoch(state: &mut EgressState, epoch: u64) {
    if epoch != state.frames_epoch {
        state.frames_epoch = epoch;
        state.gpu_unavailable = false;
        state.last_render_error = None;
    }
}

/// Everything the frame half of one egress pass decides on — read off the engine and the state at
/// the top of [`emit_egress`], so [`plan_egress`] is a pure function of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EgressInput {
    /// A latched missing adapter, lost device or dead surface: nothing may be rendered until a new
    /// frame subscription retries it.
    gpu_unavailable: bool,
    /// Somebody wants the *streamed* image — a `frames` subscriber, or the armed profiler, which
    /// measures only frames that are actually produced.
    stream_wanted: bool,
    /// A window is attached ([`Engine::present_attached`]).
    present: bool,
    /// The world may have changed since the last frame submitted to the ring.
    stream_changed: bool,
    /// The latest-wins frame slot has room, so a collected frame could be delivered.
    room: bool,
    /// This frame index has already been blitted into the window.
    presented_already: bool,
}

/// What one egress pass does with the frame: at most one render, plus the ring bookkeeping around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EgressPlan {
    /// Drop the in-flight readbacks first — nothing is watching the stream. With no window attached
    /// that drops the whole ring; with one it keeps the ring, whose textures the presented frames are
    /// drawn into (`Engine::discard_stream_ring`).
    discard: bool,
    /// Render one frame into these sinks, or `None` to submit nothing this pass.
    render: Option<FrameSinkRequest>,
    /// Collect the ring's tail — instead of a render, or after one that filed no readback of its own.
    drain: bool,
}

/// The sinks one planned render asks for; the pair [`Engine::render_frame`] takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FrameSinkRequest {
    readback: bool,
    present: bool,
}

/// Decides one pass's frame work. Pure, so the rules below are the whole contract and are tested
/// directly rather than through a GPU.
///
/// With no window attached (`present: false`) this is exactly the rule the stream has always
/// followed — pinned by a test that walks every such input. A window adds one independent consumer:
/// it is never gated on the stream's subscriber, its slot or its dirty flag, and a frame reaches it
/// at most once, so a window keeps redrawing at the tick rate while a wedged wire client waits.
fn plan_egress(input: EgressInput) -> EgressPlan {
    let mut plan = EgressPlan {
        discard: false,
        render: None,
        drain: false,
    };
    if input.gpu_unavailable {
        plan.discard = true;
        return plan;
    }
    if !input.stream_wanted {
        // Never collect a readback for zero subscribers, and start clean when a viewer next attaches.
        // A window still owes its frame, so this is a discard *and continue*, not a discard and stop.
        plan.discard = true;
        if !input.present {
            return plan;
        }
    }
    let readback = input.stream_wanted && input.stream_changed && input.room;
    let present = input.present && !input.presented_already;
    if readback || present {
        plan.render = Some(FrameSinkRequest { readback, present });
    }
    // A pass that files no readback of its own still drains the tail of a burst, whether it rendered
    // for the window or submitted nothing at all.
    plan.drain = !readback && input.stream_wanted && input.room;
    plan
}

/// Emits one pass worth of egress: every queued event, then at most one rendered frame.
///
/// **Events** — the engine's event outbox is drained and each event addressed to every open
/// subscriber on its channel, one [`EgressEvent`] per (event, subscriber) with the subscription's own
/// monotonic sequence, sent on the ordered [`events`](EgressChannels::events) lane. That lane is
/// unbounded, so a burst of events in one pass is delivered in full and in order — the seq stream a
/// subscriber sees is gapless (audit finding 1.3). A send fails only if the receiver has vanished
/// (teardown); the sequence still advanced, so any far-downstream drop at a wedged connection stays a
/// visible gap, never a silent loss (architecture §6). Events with no subscriber are dropped.
///
/// **Frames** — submission and collection are decoupled across the stream ring, so the sim thread
/// never stalls on a full render+readback. When a `frames` subscriber exists, the egress slot has
/// room, and the world may have changed since the last *submit* (the frame index advanced, or a
/// command that [`may_change_frame`](Command::may_change_frame) drained), this submits one render
/// at the default size and non-blocking-collects every finished readback, delivering the newest (an
/// *older* frame), which is sent stamped with the index it actually depicts. A pass with nothing
/// new to submit still drains the finished readbacks, so the tail of a burst still arrives; a full
/// egress slot holds the in-flight readbacks for a later pass rather than dropping them. Submission
/// blocks only when every ring slot is in flight: it waits once and empties the ring (~2 frames is
/// the worst-case latency). It never touches sim state. A missing GPU (`L2003`) latches
/// `gpu_unavailable` until a new frame subscription opens; losing the last subscriber discards the
/// ring so nothing is collected for zero subscribers — unless the live profiler is armed, which
/// renders for the measurement alone and discards the image.
///
/// **The window**, when one is attached ([`Engine::attach_surface`]), is a second and independent
/// consumer of the same frame: [`plan_egress`] decides both sinks in one place, a frame is blitted at
/// most once, and the present is gated on neither the stream's subscriber nor its slot — so a window
/// keeps redrawing while a wedged wire client waits, and a windowed host with nobody subscribed
/// renders without paying a readback at all.
///
/// A lost device (`L2011`) or a dead window surface (`L2007`) latches the same no-render state and
/// is reported once to an armed host ([`Engine::arm_render_fault_notify`]), which ends the session;
/// the sim never ends itself.
///
/// Any render failure but a missing GPU — a missing texture (`L1101`), a bad extract — is *loud*:
/// the diagnostic is rendered into an error-level [`Event::Log`] pushed to the event outbox, so any
/// `log`-channel subscriber sees exactly what went wrong over the wire instead of a silently frozen
/// stream. It stays deduplicated: the identical failure repeating every pass logs only once (its
/// rendered text is held in [`last_render_error`](EgressState::last_render_error)), and a render
/// success — or a *different* failure — resets that latch so the next distinct problem, or a
/// recurrence after recovery, logs again. An ordinary failure leaves the frame dirty for the next
/// pass, so a transient one self-heals.
fn emit_egress(engine: &mut Engine, channels: Option<&EgressChannels>, state: &mut EgressState) {
    // 1. Events: drain the outbox and fan each out to its channel's subscribers on the ordered,
    //    unbounded event lane — never the latest-wins frame slot, so a pending frame can no longer
    //    displace an event (audit finding 1.3). `send` on an unbounded channel never blocks; it fails
    //    only if the receiver is gone (teardown). With no transport the outbox is left untouched.
    if let Some(channels) = channels {
        let events = engine.take_events();
        if !events.is_empty() {
            let frame = engine.frame();
            for event in events {
                let channel = event_channel(&event);
                for (sub, seq) in engine.subscriptions_mut().assign_event_seqs(channel) {
                    let message = EventMessage {
                        event: event.clone(),
                        sub,
                        seq,
                        frame: Some(frame),
                    };
                    let _ = channels.events.send(Egress::Event(EgressEvent { message }));
                }
            }
        }
    }

    // 2. Frames. A newly opened frame subscription clears a latched no-GPU state so the adapter is
    //    retried (and never otherwise, so an adapterless host does not thrash the request).
    observe_frames_epoch(state, engine.subscriptions().frames_opened());
    // An *armed* profiler counts as a watcher: its GPU pass times and its `extract`/`submit` spans
    // only exist for a frame that is actually produced, so a headless capture (an agent attached
    // with no viewer) would otherwise report a sim section and nothing else. The image such a pass
    // collects reaches no subscriber and is dropped below.
    let stream_wanted = channels.is_some()
        && (engine.subscriptions().has_frame_subscriber() || engine.perf_armed());
    let frame = engine.frame();
    let plan = plan_egress(EgressInput {
        gpu_unavailable: state.gpu_unavailable,
        stream_wanted,
        present: engine.present_attached(),
        stream_changed: state.frame_dirty || state.last_submitted != Some(frame),
        // The frame slot is full while its previous frame is undrained (latest-wins at the source):
        // with no room to deliver, we neither submit nor collect for the stream — the in-flight
        // readbacks wait in the ring.
        room: channels.is_some_and(|channels| !channels.frames.is_full()),
        presented_already: state.last_presented == Some(frame),
    });

    if plan.discard {
        engine.discard_stream_ring();
    }
    let collected = match plan.render {
        Some(sinks) => {
            // Submit this frame (pipelined, non-stalling) into whichever sinks were planned.
            match engine.render_frame(frame, sinks.readback, sinks.present) {
                Ok(collected) => {
                    if sinks.readback {
                        state.frame_dirty = false;
                        state.last_submitted = Some(frame);
                    }
                    if sinks.present {
                        state.last_presented = Some(frame);
                    }
                    if plan.drain {
                        // The render carried no readback of its own (it was the window's), so the
                        // ring's tail is still drained for the stream.
                        engine.collect_stream_ring()
                    } else {
                        Ok(collected)
                    }
                }
                Err(diag) => Err(diag),
            }
        }
        // Nothing new to submit, but keep draining so the tail frames of a burst still arrive.
        None if plan.drain => engine.collect_stream_ring(),
        None => return,
    };

    let collected = match collected {
        Ok(collected) => {
            // A successful submit/collect clears any latched render-failure text so a later failure
            // logs afresh.
            state.last_render_error = None;
            collected
        }
        Err(diag) => {
            absorb_render_failure(engine, state, diag);
            return;
        }
    };

    // Deliver the collected frame stamped with the index it actually depicts (not the frame current
    // now — the readback that completed is for an earlier submission).
    if let (Some(channels), Some((stamp, image))) = (channels, collected) {
        let subs = engine.subscriptions().frame_subscribers();
        if subs.is_empty() {
            // Rendered only so the armed profiler had a frame to measure (see the guard above):
            // there is nobody to deliver it to, so the image ends here.
            return;
        }
        let message = Egress::Frame(EgressFrame {
            frame: stamp,
            width: image.width,
            height: image.height,
            rgba: image.rgba,
            subs,
            gi_stale: engine.gi_stale(),
        });
        let _ = channels.frames.try_send(message);
    }
}

/// Handles a failed render or collect: latch, report and log it as its code demands.
fn absorb_render_failure(engine: &mut Engine, state: &mut EgressState, diag: Diagnostic) {
    // No GPU: stop trying until a new frame subscription opens (see the epoch check in
    // `emit_egress`).
    if diag.code == code::RENDER_NO_ADAPTER {
        state.gpu_unavailable = true;
        return;
    }
    // A lost device (`L2011`) or a dead window surface (`L2007`) never recovers: latch no-render so
    // the dead GPU is not re-extracted every pass, and tell an armed host once. The session keeps
    // simulating; ending it is the window host's call, never the sim thread's.
    if diag.code == code::RENDER_DEVICE_LOST || diag.code == code::WINDOW_PRESENTATION_UNAVAILABLE {
        state.gpu_unavailable = true;
        engine.report_render_fault(&diag);
    }
    // Any failure but a missing adapter (a missing texture `L1101`, a bad extract, a readback error
    // `L2005`, and the fatal faults above): surface it loudly on the `log` channel rather than
    // skipping in silence, but only once per distinct diagnostic — the same failure recurs every
    // pass, and a frozen stream should not become a flood of log spam. The frame stays dirty, so
    // the next pass retries a transient failure and a recovery self-heals.
    let rendered = diag.to_string();
    if state.last_render_error.as_deref() != Some(rendered.as_str()) {
        engine.push_event(Event::Log(LogEvent::from_diagnostic(diag)));
        state.last_render_error = Some(rendered);
    }
}

/// The wall-clock duration one tick should occupy, given the engine's fixed tick rate and the
/// current pacing [`speed`](Engine::speed). Speed scales *when* ticks happen, never `dt`.
fn tick_period(engine: &Engine) -> Duration {
    let seconds = engine.seconds_per_tick() / engine.speed().factor();
    Duration::from_secs_f64(seconds)
}

/// The env var that arms [`TickTrace`]. Any non-empty value other than `"0"` arms it, matching
/// `LUDION_BLESS`'s convention elsewhere in the workspace.
const TICK_TRACE_ENV: &str = "LUDION_TICK_TRACE";

/// Ticks between periodic stderr summaries — 600 at the default 60 Hz tick rate is ~10s, often
/// enough to see jitter develop without flooding the log.
const TICK_TRACE_REPORT_INTERVAL: u64 = 600;

/// Lateness histogram bucket width in milliseconds and bucket count. 0.25 ms across 128 buckets
/// covers 0..32 ms — wide enough to resolve the ~15.6 ms Windows timer-quantum signature the
/// cadence-jitter hypothesis predicts (roughly one bucket per quantum step), with one further
/// overflow bucket for anything past it (a wedged drain, a debugger pause).
const LATENESS_BUCKET_MS: f64 = 0.25;
const LATENESS_BUCKETS: usize = 128;

/// An armed [`TickTrace`]'s whole state: how far each tick actually fired past its own deadline,
/// and how long the wall clock said the previous tick to this one actually took.
///
/// Built to answer one question — is the live loop's tick cadence jittery on this box, and by how
/// much — without perturbing what it measures. Everything here is pure observation: it reads
/// `now` values the loop already computed, writes only into this struct, and is never touched by
/// [`World`](ludion_ecs::World) or the sim tick, so an armed run's `WorldHash` sequence is
/// identical to a disarmed one's for the same reason [`crate::perf_live::LivePerf`] is.
///
/// A fixed-size histogram, not a growing `Vec` of samples, because this runs for the life of the
/// process: the loop has no natural "window closes" point the way the profiler's rolling windows
/// do, so accumulating forever in bounded memory is the simpler shape. Percentiles are read off
/// the histogram's cumulative counts (bucket-width resolution, not exact), which is precise enough
/// for "is this jittery" without storing every tick ever seen.
struct TickTrace {
    /// The tick period at loop entry (speed 1x, the common case this hypothesis cares about) —
    /// reported alongside the mean actual period as the baseline it should track.
    intended_period_ms: f64,
    /// The `now` of the previous recorded tick, to compute the actual elapsed period between
    /// consecutive ticks. `None` before the first tick, when there is no previous one to diff.
    last_tick_at: Option<Instant>,
    /// Ticks recorded since the loop armed, for the summary line's tick count.
    ticks: u64,
    /// Ticks recorded since the last report, reset on each report — the periodic-emission clock.
    ticks_since_report: u64,
    /// Sum of actual inter-tick periods in milliseconds, and how many contributed (one fewer than
    /// `ticks`, since the first tick has no predecessor to diff against) — together give the mean.
    period_sum_ms: f64,
    period_samples: u64,
    /// Counts of `now - deadline` at the moment each tick fired, bucketed at
    /// `LATENESS_BUCKET_MS` resolution; index `LATENESS_BUCKETS` is the overflow bucket.
    lateness_histogram: [u32; LATENESS_BUCKETS + 1],
    /// The single largest lateness observed, kept exactly (not bucketed) since "worst case" is
    /// worth reporting precisely and costs nothing extra to track.
    lateness_max_ms: f64,
}

impl TickTrace {
    /// Reads [`TICK_TRACE_ENV`] once. `intended_period` is the tick period at the moment the live
    /// loop starts, used only as the reported baseline — this trace does not re-read it per tick,
    /// so a mid-run speed change is not reflected in "intended".
    fn from_env(intended_period: Duration) -> Option<TickTrace> {
        let armed =
            std::env::var(TICK_TRACE_ENV).is_ok_and(|value| !value.is_empty() && value != "0");
        armed.then(|| TickTrace {
            intended_period_ms: intended_period.as_secs_f64() * 1000.0,
            last_tick_at: None,
            ticks: 0,
            ticks_since_report: 0,
            period_sum_ms: 0.0,
            period_samples: 0,
            lateness_histogram: [0; LATENESS_BUCKETS + 1],
            lateness_max_ms: 0.0,
        })
    }

    /// Records one tick firing: `now` is the instant the loop observed `now >= deadline` (already
    /// read by the caller), and `deadline` is `next_tick` at that same moment, before the loop
    /// advances it — so `now - deadline` is exactly how far past its own schedule this tick fired.
    fn record(&mut self, now: Instant, deadline: Instant) {
        if let Some(last) = self.last_tick_at {
            self.period_sum_ms += now.saturating_duration_since(last).as_secs_f64() * 1000.0;
            self.period_samples += 1;
        }
        self.last_tick_at = Some(now);
        self.ticks += 1;
        self.ticks_since_report += 1;

        let lateness_ms = now.saturating_duration_since(deadline).as_secs_f64() * 1000.0;
        self.lateness_max_ms = self.lateness_max_ms.max(lateness_ms);
        let bucket = ((lateness_ms / LATENESS_BUCKET_MS) as usize).min(LATENESS_BUCKETS);
        self.lateness_histogram[bucket] += 1;

        if self.ticks_since_report >= TICK_TRACE_REPORT_INTERVAL {
            self.report("periodic");
            self.ticks_since_report = 0;
        }
    }

    /// The lateness at histogram-bucket resolution below which `fraction` of recorded ticks fall
    /// (e.g. `fraction = 0.5` is p50). A percentile landing in the overflow bucket is reported as
    /// the observed max instead of an unbounded guess — the histogram has no upper edge for it.
    fn lateness_percentile_ms(&self, fraction: f64, total: u32) -> f64 {
        let target = ((fraction * f64::from(total)).ceil() as u32).max(1);
        let mut cumulative = 0u32;
        for (bucket, &count) in self.lateness_histogram.iter().enumerate() {
            cumulative += count;
            if cumulative >= target {
                return if bucket == LATENESS_BUCKETS {
                    self.lateness_max_ms
                } else {
                    bucket as f64 * LATENESS_BUCKET_MS
                };
            }
        }
        self.lateness_max_ms
    }

    /// Emits the one-line greppable summary this trace exists to produce.
    fn report(&self, reason: &str) {
        let total: u32 = self.lateness_histogram.iter().sum();
        if total == 0 {
            return;
        }
        let mean_period_ms = if self.period_samples > 0 {
            self.period_sum_ms / self.period_samples as f64
        } else {
            0.0
        };
        eprintln!(
            "[ludion-tick-trace] reason={reason} ticks={} intended_period_ms={:.3} \
             mean_period_ms={mean_period_ms:.3} lateness_p50_ms={:.3} lateness_p95_ms={:.3} \
             lateness_max_ms={:.3}",
            self.ticks,
            self.intended_period_ms,
            self.lateness_percentile_ms(0.50, total),
            self.lateness_percentile_ms(0.95, total),
            self.lateness_max_ms,
        );
    }
}

impl Drop for TickTrace {
    /// The loop has several `return` points (shutdown signal, disconnect, `engine.shutdown`); a
    /// `Drop` impl reports exactly once on whichever one fires, with no per-return-site call to
    /// remember.
    fn drop(&mut self) {
        self.report("exit");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_protocol::perf::ProfileCaptureParams;

    #[test]
    fn join_sim_returns_a_finished_thread_and_reports_a_parked_one() {
        let (signal, _rx) = shutdown_channel();
        let done = std::thread::spawn(|| 7);
        assert_eq!(
            join_sim(done, Duration::from_secs(5), &signal)
                .unwrap()
                .unwrap(),
            7
        );

        let (release, parked_rx) = crossbeam_channel::bounded::<()>(0);
        let parked = std::thread::spawn(move || {
            let _ = parked_rx.recv();
        });
        let stuck = join_sim(parked, Duration::from_millis(50), &signal).unwrap_err();
        assert_eq!(stuck.code, code::SIM_THREAD_STUCK);
        drop(release);
    }

    #[test]
    fn activity_round_trips_through_the_slot() {
        let (signal, rx) = shutdown_channel();
        assert_eq!(signal.activity(), Activity::Booting);
        rx.1.store(ACTIVITY_IDLE, Ordering::Relaxed);
        assert_eq!(signal.activity(), Activity::Idle);
        rx.1.store(Activity::command_code("sim.step"), Ordering::Relaxed);
        assert_eq!(signal.activity(), Activity::Command("sim.step"));
        rx.1.store(ACTIVITY_TICK, Ordering::Relaxed);
        assert_eq!(signal.activity(), Activity::Tick);
        rx.1.store(ACTIVITY_STOPPING, Ordering::Relaxed);
        assert_eq!(signal.activity(), Activity::Stopping);
    }

    #[test]
    fn a_stuck_report_names_booting_and_winding_down() {
        let grace = Duration::from_secs(5);
        let booting = sim_thread_stuck(Activity::Booting, grace);
        assert!(booting.message.contains("booting"), "{}", booting.message);
        let stopping = sim_thread_stuck(Activity::Stopping, grace);
        assert!(
            stopping.message.contains("winding down"),
            "{}",
            stopping.message
        );
    }

    /// A pipelining client cannot starve the loop: one drain applies at most the cap and leaves the
    /// rest, in order, for the next.
    #[test]
    fn a_drain_applies_at_most_the_cap_and_keeps_the_remainder() {
        let mut engine = engine();
        let (intake_tx, intake) = crossbeam_channel::unbounded();
        let replies: Vec<_> = (0..MAX_COMMANDS_PER_DRAIN + 3)
            .map(|_| {
                let (stamped, reply) = StampedCommand::new(Command::SimHash);
                intake_tx.send(stamped).unwrap();
                reply
            })
            .collect();
        let (_signal, receiver) = shutdown_channel();
        let mut pending = None;
        let mut egress_state = EgressState::new(engine.subscriptions().frames_opened());

        let first = drain(
            &mut engine,
            &intake,
            &mut pending,
            &mut egress_state,
            &receiver.1,
        );
        assert_eq!(first, Some(MAX_COMMANDS_PER_DRAIN));
        assert_eq!(intake.len(), 3);
        for reply in &replies[..MAX_COMMANDS_PER_DRAIN] {
            assert!(reply.try_recv().is_ok_and(|result| result.is_ok()));
        }
        assert!(replies[MAX_COMMANDS_PER_DRAIN].try_recv().is_err());

        let second = drain(
            &mut engine,
            &intake,
            &mut pending,
            &mut egress_state,
            &receiver.1,
        );
        assert_eq!(second, Some(3));
        assert!(replies[MAX_COMMANDS_PER_DRAIN + 2].try_recv().is_ok());

        drop(intake_tx);
        assert_eq!(
            drain(
                &mut engine,
                &intake,
                &mut pending,
                &mut egress_state,
                &receiver.1
            ),
            None
        );
    }

    fn engine() -> Engine {
        let mut engine = Engine::new();
        engine
            .load_inline_scene("format: 1\nseed: 7\nentities: []\n")
            .unwrap();
        engine
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn a_slow_host_is_not_cancelled_but_a_speed_change_is() {
        let mut engine = engine();
        crate::dispatch(
            &mut engine,
            Command::ProfileCapture(ProfileCaptureParams {
                frames: 3600,
                include_samples: false,
            }),
        )
        .expect("3600 frames fit the wall budget at speed 1");
        let (reply, answer) = crossbeam_channel::bounded(1);
        let mut pending = Some(PendingCapture {
            reply,
            include_samples: false,
            armed_at: None,
            seen: None,
        });

        // A second of wall time passes with no ticks run: the host is late, the capture is not doomed.
        let t0 = Instant::now();
        abandon_unclosable_capture(&mut engine, &mut pending, t0);
        abandon_unclosable_capture(&mut engine, &mut pending, t0 + Duration::from_secs(1));
        assert!(pending.is_some());
        assert!(engine.perf_capture_in_flight());

        // Halving the speed makes the remaining frames genuinely unclosable.
        engine.driver_mut().set_speed(Speed::new(0.5).unwrap());
        abandon_unclosable_capture(&mut engine, &mut pending, t0 + Duration::from_secs(2));
        assert!(pending.is_none());
        let err = answer.try_recv().unwrap().unwrap_err();
        assert_eq!(err.code, code::PROFILE_CAPTURE_CANNOT_CLOSE);
    }

    #[test]
    fn a_profile_subscription_is_the_arming_switch() {
        // The whole on/off rule, asserted directly: nothing else turns the profiler on, and losing
        // the last subscriber turns it off (which is also how a disconnect disarms — the transport
        // queues a `stream.unsubscribe` for every subscription the connection owned).
        let mut engine = engine();
        sync_perf_arming(&mut engine);
        assert!(!engine.perf_armed(), "an unwatched engine measures nothing");

        let sub = engine.subscriptions_mut().open(Channel::Profile);
        sync_perf_arming(&mut engine);
        assert!(engine.perf_armed());

        // Another channel's subscriber is not a watcher of this one.
        let frames = engine.subscriptions_mut().open(Channel::Frames);
        engine.subscriptions_mut().close(sub).unwrap();
        sync_perf_arming(&mut engine);
        assert!(!engine.perf_armed());
        engine.subscriptions_mut().close(frames).unwrap();
    }

    #[test]
    fn an_in_flight_capture_keeps_the_profiler_armed_without_a_subscriber() {
        let mut engine = engine();
        crate::dispatch(
            &mut engine,
            Command::ProfileCapture(ProfileCaptureParams {
                frames: 4,
                include_samples: false,
            }),
        )
        .expect("the capture arms");
        assert!(engine.perf_capture_in_flight());

        // Nobody is subscribed, yet the sync must not disarm out from under the capture.
        sync_perf_arming(&mut engine);
        assert!(engine.perf_armed());

        // Once the capture's report is taken, nothing wants the profiler and it turns off.
        for _ in 0..16 {
            engine.advance_live();
        }
        assert!(engine.perf_take_capture().is_some(), "the window closed");
        sync_perf_arming(&mut engine);
        assert!(!engine.perf_armed());
    }

    /// A quiet pass: no GPU trouble, nobody watching, no window, nothing new.
    const IDLE: EgressInput = EgressInput {
        gpu_unavailable: false,
        stream_wanted: false,
        present: false,
        stream_changed: false,
        room: false,
        presented_already: false,
    };

    fn render(readback: bool, present: bool) -> Option<FrameSinkRequest> {
        Some(FrameSinkRequest { readback, present })
    }

    #[test]
    fn nothing_watching_and_no_window_renders_nothing() {
        let plan = plan_egress(EgressInput {
            stream_changed: true,
            room: true,
            ..IDLE
        });
        assert_eq!(plan.render, None);
        assert!(plan.discard, "in-flight readbacks are dropped");
        assert!(!plan.drain);
    }

    #[test]
    fn a_window_alone_presents_a_new_frame_without_a_readback() {
        let plan = plan_egress(EgressInput {
            present: true,
            stream_changed: true,
            room: true,
            ..IDLE
        });
        assert_eq!(plan.render, render(false, true));
        // The ring survives this discard (it holds the graph's textures) — `Engine::discard_stream_ring`
        // drops only the in-flight readbacks while a present target is attached.
        assert!(plan.discard);
        assert!(!plan.drain, "nobody is watching the stream");
    }

    #[test]
    fn a_watched_window_renders_once_into_both_sinks() {
        let plan = plan_egress(EgressInput {
            stream_wanted: true,
            present: true,
            stream_changed: true,
            room: true,
            ..IDLE
        });
        assert_eq!(plan.render, render(true, true));
        assert!(!plan.discard);
        assert!(!plan.drain, "the render collects its own readback");
    }

    #[test]
    fn a_full_stream_slot_still_presents() {
        // The wire client's slot is undrained; the window must not wait on it.
        let plan = plan_egress(EgressInput {
            stream_wanted: true,
            present: true,
            stream_changed: true,
            room: false,
            ..IDLE
        });
        assert_eq!(plan.render, render(false, true));
        assert!(!plan.drain);
    }

    #[test]
    fn one_frame_is_presented_exactly_once() {
        let input = EgressInput {
            present: true,
            stream_changed: true,
            room: true,
            presented_already: true,
            ..IDLE
        };
        assert_eq!(plan_egress(input).render, None, "the frame is already up");
    }

    #[test]
    fn a_subscriber_arriving_after_a_present_reads_that_same_frame_back() {
        // A legitimate second render of one frame index — the same class as today's `frame_dirty`
        // re-render: the window has the frame, the stream has never seen it.
        let plan = plan_egress(EgressInput {
            stream_wanted: true,
            present: true,
            stream_changed: true,
            room: true,
            presented_already: true,
            ..IDLE
        });
        assert_eq!(plan.render, render(true, false));
        assert!(!plan.drain);
    }

    /// The rule as it stood before in-process presentation, written out as the oracle: no GPU or no
    /// watcher discards and stops; a changed world with room submits; otherwise a pass with room
    /// drains the tail; otherwise nothing happens.
    fn plan_before_presentation(input: EgressInput) -> EgressPlan {
        let mut plan = EgressPlan {
            discard: false,
            render: None,
            drain: false,
        };
        if input.gpu_unavailable || !input.stream_wanted {
            plan.discard = true;
        } else if input.stream_changed && input.room {
            plan.render = render(true, false);
        } else if input.room {
            plan.drain = true;
        }
        plan
    }

    #[test]
    fn a_windowless_host_plans_exactly_what_it_always_did() {
        for bits in 0..16u8 {
            let input = EgressInput {
                gpu_unavailable: bits & 1 != 0,
                stream_wanted: bits & 2 != 0,
                present: false,
                stream_changed: bits & 4 != 0,
                room: bits & 8 != 0,
                // Dead state with no window, and deliberately varied against `stream_changed` so a
                // stray read of it would show up here.
                presented_already: bits & 4 == 0,
            };
            assert_eq!(
                plan_egress(input),
                plan_before_presentation(input),
                "the frame stream must be byte-for-byte unmoved for {input:?}"
            );
        }
    }

    /// Builds a trace directly (bypassing `from_env`, which reads process-global state that other
    /// tests in this binary must not race with).
    fn blank_trace(intended_period_ms: f64) -> TickTrace {
        TickTrace {
            intended_period_ms,
            last_tick_at: None,
            ticks: 0,
            ticks_since_report: 0,
            period_sum_ms: 0.0,
            period_samples: 0,
            lateness_histogram: [0; LATENESS_BUCKETS + 1],
            lateness_max_ms: 0.0,
        }
    }

    // Sanctioned wall-clock site: `Instant::now` here is only a synthetic baseline for constructing
    // test `Instant`s (there is no other way to get one), never a real measurement — the trace math
    // under test runs entirely on the offsets built from it.
    #[allow(clippy::disallowed_methods)]
    #[test]
    fn tick_trace_reports_bucket_resolution_percentiles_and_exact_mean_period() {
        let mut trace = blank_trace(16.7);
        let base = Instant::now();
        let period = Duration::from_micros(16_700); // exactly 16.7 ms, so the mean is exact
        // Ten ticks, evenly spaced by `period`, each fired the given number of microseconds past
        // its own deadline. Sorted ascending so the expected percentiles can be read off by hand.
        let lateness_us: [u64; 10] = [
            100, 300, 600, 1_000, 2_000, 5_000, 5_000, 5_000, 5_000, 40_000,
        ];

        for (index, lateness) in lateness_us.iter().enumerate() {
            let now = base + period * index as u32;
            let deadline = now - Duration::from_micros(*lateness);
            trace.record(now, deadline);
        }

        assert_eq!(trace.ticks, 10);
        assert_eq!(trace.lateness_max_ms, 40.0, "the exact max, not bucketed");

        // period_samples is one fewer than ticks (the first tick has no predecessor), and every
        // interval was exactly `period`, so the mean is exact too.
        assert_eq!(trace.period_samples, 9);
        let mean_period_ms = trace.period_sum_ms / trace.period_samples as f64;
        assert!(
            (mean_period_ms - 16.7).abs() < 1e-9,
            "mean_period_ms was {mean_period_ms}"
        );

        let total: u32 = trace.lateness_histogram.iter().sum();
        assert_eq!(total, 10);
        // p50 (target = ceil(0.5 * 10) = 5th sample, 2.0 ms) lands cleanly inside the histogram:
        // the cumulative count reaches 5 at the 2.0 ms bucket (0.1/0.3/0.6/1.0/2.0 ms so far).
        assert_eq!(trace.lateness_percentile_ms(0.50, total), 2.0);
        // p95 (target = ceil(0.95 * 10) = 10th sample) is the 40 ms outlier, which falls in the
        // overflow bucket — reported as the exact observed max rather than an unbounded guess.
        assert_eq!(trace.lateness_percentile_ms(0.95, total), 40.0);
    }

    // See the rationale on the previous test: a synthetic baseline `Instant`, not a real reading.
    #[allow(clippy::disallowed_methods)]
    #[test]
    fn tick_trace_periodic_report_resets_its_own_counter() {
        let mut trace = blank_trace(16.7);
        let base = Instant::now();
        let period = Duration::from_millis(17);
        for index in 0..TICK_TRACE_REPORT_INTERVAL {
            let now = base + period * index as u32;
            trace.record(now, now); // zero lateness; only the counter wrap is under test
        }
        assert_eq!(
            trace.ticks_since_report, 0,
            "600 ticks trip the periodic report, which resets the count that gates it"
        );
        assert_eq!(trace.ticks, TICK_TRACE_REPORT_INTERVAL);
    }

    #[test]
    fn a_panicked_sim_thread_is_l4017_quoting_the_payload() {
        let panicked = std::thread::spawn(|| panic!("sim exploded")).join();
        let payload = panicked.expect_err("the thread panicked");
        let diagnostic = sim_thread_panicked(payload.as_ref());
        assert_eq!(diagnostic.code, code::SIM_THREAD_PANICKED);
        assert!(
            diagnostic.message.contains("sim exploded"),
            "{diagnostic:?}"
        );
    }

    fn fault(code: ludion_core::diagnostics::DiagnosticCode) -> Diagnostic {
        Diagnostic::new(code, "render fault")
    }

    fn absorbed(diag: Diagnostic) -> (EgressState, Receiver<Diagnostic>, Engine) {
        let mut engine = engine();
        let (tx, rx) = crossbeam_channel::bounded(1);
        engine.arm_render_fault_notify(tx);
        let mut state = EgressState::new(0);
        absorb_render_failure(&mut engine, &mut state, diag);
        (state, rx, engine)
    }

    fn assert_fatal(diag: Diagnostic) {
        let expected = diag.code;
        let (mut state, rx, mut engine) = absorbed(diag.clone());
        assert!(state.gpu_unavailable, "the fault latches no-render");
        assert_eq!(rx.try_recv().expect("reported to the host").code, expected);
        assert!(rx.try_recv().is_err(), "reported once");
        let logs = engine
            .take_events()
            .into_iter()
            .filter(|event| matches!(event, Event::Log(_)))
            .count();
        assert_eq!(logs, 1);
        // The same fault again does not log a second time.
        absorb_render_failure(&mut engine, &mut state, diag);
        assert!(engine.take_events().is_empty());
        let plan = plan_egress(EgressInput {
            gpu_unavailable: state.gpu_unavailable,
            present: true,
            stream_wanted: true,
            stream_changed: true,
            room: true,
            ..IDLE
        });
        assert_eq!(plan.render, None);
    }

    #[test]
    fn a_new_frame_subscription_logs_a_sticky_fault_again() {
        let (mut state, _rx, mut engine) = absorbed(fault(code::RENDER_DEVICE_LOST));
        let logs = |engine: &mut Engine| {
            engine
                .take_events()
                .into_iter()
                .filter(|event| matches!(event, Event::Log(_)))
                .count()
        };
        assert_eq!(logs(&mut engine), 1);
        absorb_render_failure(&mut engine, &mut state, fault(code::RENDER_DEVICE_LOST));
        assert_eq!(logs(&mut engine), 0);
        let next = state.frames_epoch + 1;
        observe_frames_epoch(&mut state, next);
        absorb_render_failure(&mut engine, &mut state, fault(code::RENDER_DEVICE_LOST));
        assert_eq!(logs(&mut engine), 1);
    }

    #[test]
    fn a_lost_device_latches_no_render_and_reports_once() {
        assert_fatal(fault(code::RENDER_DEVICE_LOST));
    }

    #[test]
    fn a_dead_window_surface_is_fatal_too() {
        assert_fatal(fault(code::WINDOW_PRESENTATION_UNAVAILABLE));
    }

    #[test]
    fn an_ordinary_failure_retries_and_never_reaches_the_host() {
        for ordinary in [code::RENDER_READBACK_FAILED, code::ASSET_FILE_IO] {
            let (state, rx, mut engine) = absorbed(fault(ordinary));
            assert!(!state.gpu_unavailable, "an ordinary failure retries");
            assert!(rx.try_recv().is_err(), "the host is not told");
            assert_eq!(engine.take_events().len(), 1);
        }
    }
}
