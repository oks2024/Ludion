//! A panicking game system is a diagnostic, not a dead host (`L4007`).
//!
//! Game code panics readily by design — an `unwrap` on a component the world no longer has is one
//! line — and before the schedule's unwind boundary such a panic took the sim thread with it, after
//! which every request answered `L3009` and the process was a zombie with nothing to say about the
//! cause. These tests pin the replacement behaviour end to end: the panic is caught, reported as
//! `L4007` naming the system that raised it, the sim auto-pauses at the failing frame, and the
//! engine keeps answering.
//!
//! Two arrangements, because the two audiences discover it differently: a direct `sim.step` gets the
//! diagnostic as its own reply, and a client watching a free-running sim over the wire gets it as an
//! `Error` event on the `log` channel.

use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded, unbounded};
use ludion_core::ids::EntityId;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::live::{ShutdownSignal, StampedCommand, run_live, shutdown_channel};
use ludion_engine::{Egress, EgressChannels, Engine, EngineBuilder};
use ludion_protocol::command::{StreamSubscribeParams, SubscribeMode};
use ludion_protocol::event::{Event, LogLevel};
use ludion_protocol::frame::FrameFormat;
use ludion_protocol::{Channel, Command, ResponseBody};
use ludion_sim::RunState;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The frame `reap` despawns the player on, and therefore the frame `damage_player` panics on.
const DOOM: u64 = 2;

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug)]
struct Health {
    hp: i32,
}

/// One entity carrying `Health` — the target the panicking system assumes will always be there.
const SCENE: &str = "\
format: 1
entities:
  - name: player
    components:
      Health:
        hp: 100
";

/// Despawns the player at frame [`DOOM`], out from under the system registered after it.
fn reap(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != DOOM {
        return;
    }
    let doomed: Vec<EntityId> = world.query_ids::<(&Health,)>().map(|(id, _)| id).collect();
    for id in doomed {
        world.despawn(id).unwrap();
    }
}

/// The shape the audit found across vigil: the system assumes its target is still there and unwraps.
fn damage_player(world: &mut World, _ctx: &SystemCtx) {
    let health = world
        .query::<(&mut Health,)>()
        .next()
        .expect("the player's Health")
        .0;
    health.hp -= 1;
}

/// Runs after the panicking system, so its absence from the world proves the tick stopped.
fn tally(world: &mut World, _ctx: &SystemCtx) {
    for (health,) in world.query::<(&mut Health,)>() {
        health.hp += 1000;
    }
}

fn engine() -> Engine {
    let mut engine = EngineBuilder::new()
        .components(|registry| registry.register::<Health>("Health"))
        .system("reap", reap)
        .system("damage_player", damage_player)
        .system("tally", tally)
        .build()
        .expect("the test engine builds");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    engine
}

// --- In-process: the request that drove the failing tick gets the diagnostic ---------------------

#[test]
fn a_panicking_system_answers_l4007_and_leaves_the_engine_usable() {
    let mut engine = engine();

    // Frames before the doomed one step cleanly.
    engine
        .execute(Command::SimStep(step(DOOM)))
        .expect("the clean frames step");
    assert_eq!(engine.frame().as_u64(), DOOM);

    let error = engine
        .execute(Command::SimStep(step(1)))
        .expect_err("the step that panics answers with a diagnostic, not an Ack");

    assert_eq!(error.code.as_str(), "L4007");
    assert!(
        error.message.contains("damage_player"),
        "the diagnostic must name the system that panicked: {}",
        error.message
    );
    assert!(
        error.message.contains("the player's Health"),
        "the diagnostic must quote the panic payload: {}",
        error.message
    );
    assert!(
        error.message.contains("system_panic.rs"),
        "the diagnostic must locate the panic site: {}",
        error.message
    );
    assert!(error.help.is_some(), "L4007 carries a likely fix");

    // The auto-pause: the sim stopped at the failing frame instead of ticking into it again.
    assert_eq!(engine.run_state(), RunState::Paused);

    // The process is alive and answering — the whole point. `sim.hash` is a real answer, not L3009.
    let hash = engine
        .execute(Command::SimHash)
        .expect("a non-ticking command still answers after a caught panic");
    assert!(matches!(hash.body, ResponseBody::SimHash { .. }));

    // The world is exactly as the panic left it: `reap` ran, `damage_player` failed part-way, and
    // `tally` — registered after it — never ran at all.
    assert_eq!(
        engine.world().entity_count(),
        0,
        "the despawn that set up the panic did happen"
    );
}

#[test]
fn a_clean_run_never_reports_a_panic() {
    let mut engine = engine();
    engine
        .execute(Command::SimStep(step(DOOM)))
        .expect("the clean frames step");
    assert_eq!(engine.run_state(), RunState::Paused, "sim.step pauses");
    assert!(engine.take_system_panic().is_none());
}

fn step(ticks: u64) -> ludion_protocol::command::SimStepParams {
    ludion_protocol::command::SimStepParams { ticks, hash: false }
}

// --- Over the wire: a client watching a free-running sim discovers the pause ---------------------

#[test]
fn a_free_running_panic_reaches_the_log_channel_and_the_host_keeps_serving() {
    // A fresh engine is Running: paused here, the doomed frame cannot arrive before the subscription
    // and the resume do — on a slow host it otherwise did, and the late resume ran a second panic.
    let mut engine = engine();
    engine
        .execute(Command::SimPause)
        .expect("a fresh engine pauses");
    let session = Session::start(engine);
    session.subscribe(Channel::Log);

    // Free-run: the sim reaches the doomed frame on its own wall-clock cadence.
    session.send(Command::SimResume);

    let message = session
        .next_log(Duration::from_secs(5))
        .expect("the caught panic is delivered on the log channel");
    assert_eq!(message.level, LogLevel::Error);
    assert_eq!(
        message.diagnostic.as_ref().map(|d| d.code.as_str()),
        Some("L4007"),
        "the log event carries the structured diagnostic"
    );
    assert!(
        message.message.contains("L4007") && message.message.contains("damage_player"),
        "the log event must carry the code and the system name: {}",
        message.message
    );

    // The auto-pause is what a client sees next: the frame stamp stops advancing even though the sim
    // was told to run, and the host answers throughout.
    let first = session.send(Command::SimHash).frame;
    thread::sleep(Duration::from_millis(120));
    let second = session.send(Command::SimHash).frame;
    assert_eq!(
        first, second,
        "a resumed-but-auto-paused sim must not advance past the failing frame"
    );
    assert_eq!(first.as_u64(), DOOM + 1);
}

/// A live [`run_live`] loop on its own thread, with handles to feed it commands and read its egress
/// (the `egress.rs` harness, trimmed to the event lane this file needs).
struct Session {
    intake: Sender<StampedCommand>,
    shutdown: ShutdownSignal,
    events: Receiver<Egress>,
    handle: Option<JoinHandle<Result<(), ludion_core::diagnostics::Diagnostic>>>,
}

impl Session {
    fn start(engine: Engine) -> Session {
        let (intake_tx, intake_rx) = unbounded();
        let (shutdown_tx, shutdown_rx) = shutdown_channel();
        let (frame_tx, _frame_rx) = bounded::<Egress>(1);
        let (event_tx, event_rx) = unbounded::<Egress>();
        let egress = EgressChannels {
            frames: frame_tx,
            events: event_tx,
        };
        let handle = thread::spawn(move || run_live(engine, intake_rx, shutdown_rx, Some(egress)));
        Session {
            intake: intake_tx,
            shutdown: shutdown_tx,
            events: event_rx,
            handle: Some(handle),
        }
    }

    fn send(&self, command: Command) -> ludion_engine::Dispatched {
        let (stamped, reply) = StampedCommand::new(command);
        self.intake.send(stamped).unwrap();
        reply
            .recv_timeout(Duration::from_secs(5))
            .expect("the sim thread replied in time — a dead one would time out here")
            .expect("command succeeded")
    }

    fn subscribe(&self, channel: Channel) {
        let response = self.send(Command::StreamSubscribe(StreamSubscribeParams {
            channel,
            mode: SubscribeMode::LatestWins { max_fps: None },
            format: FrameFormat::Qoi,
        }));
        assert!(matches!(response.body, ResponseBody::Subscribed { .. }));
    }

    /// The next `log` event on the event lane, or a timeout.
    fn next_log(
        &self,
        timeout: Duration,
    ) -> Result<ludion_protocol::event::LogEvent, RecvTimeoutError> {
        loop {
            match self.events.recv_timeout(timeout)? {
                Egress::Event(event) => {
                    if let Event::Log(log) = event.message.event {
                        return Ok(log);
                    }
                }
                Egress::Frame(_) => {}
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.shutdown.signal();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
