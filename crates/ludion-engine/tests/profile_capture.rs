//! A held `profile.capture` that can no longer close is answered `L4023` at once and cancelled, so
//! the next capture arms (no `L4013`) — over a real [`run_live`] thread.
//!
//! The capture is the only deferred reply in `run_live`; a `sim.pause` or a `sim.set_speed` after it
//! armed used to leave the request waiting out the transport's reply budget as a stalled sim.

use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::Sender;
use ludion_core::diagnostics::Diagnostic;
use ludion_engine::live::{
    ReplyReceiver, ShutdownSignal, StampedCommand, run_live, shutdown_channel,
};
use ludion_engine::{Dispatched, Engine};
use ludion_protocol::command::SimSetSpeedParams;
use ludion_protocol::perf::ProfileCaptureParams;
use ludion_protocol::{Command, ResponseBody};

/// A [`run_live`] loop on its own thread over an empty scene.
struct Session {
    intake: Sender<StampedCommand>,
    shutdown: ShutdownSignal,
    handle: Option<JoinHandle<Result<(), Diagnostic>>>,
}

impl Session {
    fn start() -> Session {
        let mut engine = Engine::new();
        engine
            .load_inline_scene("format: 1\nseed: 7\nentities: []\n")
            .expect("the scene loads");
        let (intake, intake_rx) = crossbeam_channel::unbounded();
        let (shutdown, shutdown_rx) = shutdown_channel();
        let handle = thread::spawn(move || run_live(engine, intake_rx, shutdown_rx, None));
        Session {
            intake,
            shutdown,
            handle: Some(handle),
        }
    }

    /// Sends without waiting for the reply.
    fn post(&self, command: Command) -> ReplyReceiver {
        let (stamped, reply) = StampedCommand::new(command);
        self.intake.send(stamped).unwrap();
        reply
    }

    fn send(&self, command: Command) -> Result<Dispatched, Diagnostic> {
        wait(&self.post(command))
    }

    fn capture(&self, frames: u64) -> ReplyReceiver {
        self.post(Command::ProfileCapture(ProfileCaptureParams {
            frames,
            include_samples: false,
        }))
    }
}

fn wait(reply: &ReplyReceiver) -> Result<Dispatched, Diagnostic> {
    reply
        .recv_timeout(Duration::from_secs(5))
        .expect("the sim thread replied in time — an unanswered capture would time out here")
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
fn pausing_under_a_held_capture_answers_l4023_and_frees_the_slot() {
    let session = Session::start();
    let held = session.capture(3600);
    session.send(Command::SimPause).expect("pause");

    let error = wait(&held).expect_err("the held capture is cancelled");
    assert_eq!(error.code.as_str(), "L4023");
    assert!(error.help.is_some());

    session.send(Command::SimResume).expect("resume");
    let report = wait(&session.capture(2)).expect("the next capture arms, not L4013");
    assert!(matches!(report.body, ResponseBody::ProfileReport { .. }));
}

#[test]
fn slowing_the_sim_under_a_held_capture_answers_l4023() {
    let session = Session::start();
    let held = session.capture(600);
    session
        .send(Command::SimSetSpeed(SimSetSpeedParams {
            speed: 1.0 / 64.0,
        }))
        .expect("set speed");

    let error = wait(&held).expect_err("600 frames no longer fit in a minute");
    assert_eq!(error.code.as_str(), "L4023");
}
