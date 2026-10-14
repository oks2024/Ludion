//! A panicking command handler is a diagnostic, not a dead host (`L4016`), and a speed change
//! re-bases the live loop's tick deadline (`L4001` bounds the factor).
//!
//! The command-path twin of `system_panic.rs`: a handler that panics used to unwind `run_live`, drop
//! the engine and leave a host that answered `L3009` to everything. These tests pin the replacement
//! over a real [`run_live`] thread: the request gets `L4016`, the sim pauses, the next request is
//! answered, and the thread ends cleanly.

use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{Sender, unbounded};
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_engine::live::{ShutdownSignal, StampedCommand, run_live, shutdown_channel};
use ludion_engine::{Dispatched, Engine, EngineBuilder};
use ludion_protocol::Command;
use ludion_protocol::command::{ComponentSetParams, SimSetSpeedParams};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

const PANIC_TEXT: &str = "the fuse blew";

/// A component whose registry deserialization panics when armed — a handler bug on demand, since
/// `component.set` deserializes inside the handler.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug)]
struct Fuse {
    #[serde(deserialize_with = "blow_when_armed")]
    armed: bool,
}

fn blow_when_armed<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    let armed = bool::deserialize(deserializer)?;
    assert!(!armed, "{PANIC_TEXT}");
    Ok(armed)
}

const SCENE: &str = "\
format: 1
entities:
  - name: wire
    components:
      Fuse:
        armed: false
";

/// A [`run_live`] loop on its own thread over the one-entity fuse scene, plus the entity to target.
struct Session {
    intake: Sender<StampedCommand>,
    shutdown: ShutdownSignal,
    handle: Option<JoinHandle<Result<(), Diagnostic>>>,
    fuse: EntityId,
}

impl Session {
    fn start() -> Session {
        let mut engine: Engine = EngineBuilder::new()
            .components(|registry| registry.register::<Fuse>("Fuse"))
            .build()
            .expect("the test engine builds");
        engine.load_inline_scene(SCENE).expect("the scene loads");
        let fuse = engine
            .world_mut()
            .query_ids::<(&Fuse,)>()
            .map(|(id, _)| id)
            .next()
            .expect("the scene has the fuse entity");

        let (intake, intake_rx) = unbounded();
        let (shutdown, shutdown_rx) = shutdown_channel();
        let handle = thread::spawn(move || run_live(engine, intake_rx, shutdown_rx, None));
        Session {
            intake,
            shutdown,
            handle: Some(handle),
            fuse,
        }
    }

    fn try_send(&self, command: Command) -> Result<Dispatched, Diagnostic> {
        let (stamped, reply) = StampedCommand::new(command);
        self.intake.send(stamped).unwrap();
        reply
            .recv_timeout(Duration::from_secs(5))
            .expect("the sim thread replied in time — a dead one would time out here")
    }

    fn send(&self, command: Command) -> Dispatched {
        self.try_send(command).expect("command succeeded")
    }

    fn set_fuse(&self, armed: bool) -> Result<Dispatched, Diagnostic> {
        self.try_send(Command::ComponentSet(ComponentSetParams {
            entity: self.fuse,
            component: "Fuse".into(),
            value: serde_json::json!({ "armed": armed }),
        }))
    }

    fn set_speed(&self, speed: f64) {
        self.send(Command::SimSetSpeed(SimSetSpeedParams { speed }));
    }

    fn frame(&self) -> u64 {
        self.send(Command::SimHash).frame.as_u64()
    }

    /// Polls `sim.hash` until the frame stamp reaches `target`; `false` once `within` has elapsed.
    /// The deadline is a channel timer, since the sim-side lint bans `Instant::now` in tests too.
    fn reaches_frame(&self, target: u64, within: Duration) -> bool {
        let deadline = crossbeam_channel::after(within);
        while deadline.try_recv().is_err() {
            if self.frame() >= target {
                return true;
            }
            thread::sleep(Duration::from_millis(2));
        }
        false
    }

    /// Signals the loop and joins it, returning whether the sim thread ended without a panic.
    fn finish(mut self) -> bool {
        self.shutdown.signal();
        self.handle.take().expect("not yet joined").join().is_ok()
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

#[test]
fn a_panicking_command_answers_l4016_pauses_and_the_host_keeps_serving() {
    let session = Session::start();
    session.send(Command::SimResume);

    let error = session
        .set_fuse(true)
        .expect_err("the panicking handler answers with a diagnostic, not an Ack");
    assert_eq!(error.code.as_str(), "L4016");
    assert!(
        error.message.contains("component.set") && error.message.contains(PANIC_TEXT),
        "the diagnostic names the method and quotes the payload: {}",
        error.message
    );
    assert!(error.help.is_some(), "L4016 carries a likely fix");

    // The host still answers, and the sim was told to run but is paused: the frame stamp holds still.
    let first = session.frame();
    thread::sleep(Duration::from_millis(120));
    assert_eq!(
        session.frame(),
        first,
        "a resumed sim must not tick on after a command panic"
    );

    // A well-formed command still works on the same host.
    session
        .set_fuse(false)
        .expect("a disarmed fuse is accepted");
    assert!(session.finish(), "the sim thread ends cleanly");
}

#[test]
fn a_speed_change_rebases_the_tick_deadline() {
    let session = Session::start();
    session.send(Command::SimResume);

    // At 1/64 the next tick is ~1.07 s out. Wait for one to land so the loop has scheduled the
    // *following* deadline at that slow period.
    session.set_speed(1.0 / 64.0);
    let start = session.frame();
    assert!(
        session.reaches_frame(start + 1, Duration::from_secs(4)),
        "the slow loop still ticks"
    );

    // Pre-fix the deadline stayed ~1 s out and this stalled; re-based, three ticks at 64x take a
    // few milliseconds, so 500 ms is a wide margin for a loaded box.
    session.set_speed(64.0);
    let slow = session.frame();
    assert!(
        session.reaches_frame(slow + 3, Duration::from_millis(500)),
        "raising the speed must take effect without waiting out the slow deadline"
    );
}
