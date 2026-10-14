//! `JointMotor` at the engine level: a driven hinge/ball chain (its motor targets rewritten every
//! tick through `component.set`) resumes from an engine snapshot bit-identically — the
//! `physics_joints.rs` proof with a moving drive — hashes identically at 1, 2 and 8 threads, and a
//! motor that cannot drive its joint reports `L3037` once.

use crossbeam_channel::{bounded, unbounded};
use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_engine::live::{StampedCommand, run_live, shutdown_channel};
use ludion_engine::{Egress, EgressChannels, Engine, EngineBuilder};
use ludion_protocol::command::{
    ComponentSetParams, EngineRestoreParams, SimStepParams, StreamSubscribeParams, SubscribeMode,
};
use ludion_protocol::event::{Event, LogEvent};
use ludion_protocol::frame::FrameFormat;
use ludion_protocol::{Channel, Command, ResponseBody};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

const TICKS: usize = 200;
const SNAPSHOT_AT: usize = 80;
const MIN_DISTINCT: usize = 100;
const THREAD_COUNTS: [usize; 3] = [1, 2, 8];

fn fresh_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-joint-motor-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn game(name: &str) -> EngineBuilder {
    Engine::builder().project_root(fresh_dir(name))
}

/// A static anchor (document position 0) and a three-link chain — hinge, ball, hinge — each link a
/// fat rigid sphere (a motor scales with the row's own inertia, so a small sphere on a long lever
/// barely moves) carrying its own `Joint` back to the previous link and a `JointMotor`.
fn scene() -> String {
    let mut text = String::from("format: 1\nseed: 5\nphysics:\n  space: free\nentities:\n");
    text.push_str(
        "  - name: anchor\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - 0.0\n          - 0.0\n      Collider:\n        shape:\n          sphere:\n            radius: 0.1\n        body: static\n",
    );
    let kinds = [
        "          hinge:\n            axis:\n              - 0.0\n              - 0.0\n              - 1.0\n",
        "          ball: {}\n",
        "          hinge:\n            axis:\n              - 0.0\n              - 0.0\n              - 1.0\n",
    ];
    let targets = [
        "          - 0.0\n",
        "          - 0.0\n          - 0.0\n          - 0.0\n",
        "          - 0.0\n",
    ];
    for (i, (kind, target)) in kinds.iter().zip(targets).enumerate() {
        let (parent, index) = (i, i + 1);
        text.push_str(&format!(
            "  - name: link{i}\n    components:\n      Transform:\n        translation:\n          - {index}.0\n          - 0.0\n          - 0.0\n"
        ));
        text.push_str(
            "      Collider:\n        shape:\n          sphere:\n            radius: 0.4\n        body:\n          rigid:\n            mass: 1.0\n      Velocity: {}\n      Joint:\n",
        );
        text.push_str(&format!(
            "        a:\n          index: {parent}\n          generation: 0\n        b:\n          index: {index}\n          generation: 0\n        kind:\n{kind}"
        ));
        text.push_str(
            "        anchor:\n          - 0.0\n          - 0.0\n          - 0.0\n        collide: false\n",
        );
        text.push_str(&format!(
            "      JointMotor:\n        target:\n{target}        stiffness: 170.0\n        damping: 27.0\n"
        ));
    }
    text
}

fn set_motor(engine: &mut Engine, link: u32, target: &[f32]) {
    engine
        .execute(Command::ComponentSet(ComponentSetParams {
            entity: EntityId::new(link, 0),
            component: "JointMotor".into(),
            value: json!({ "target": target, "stiffness": 170.0, "damping": 27.0 }),
        }))
        .expect("a well-formed motor sets");
}

/// Rewrites every link's target from the tick index — a triangle, no transcendentals — before
/// `tick`'s step.
fn drive(engine: &mut Engine, tick: usize) {
    let phase = tick % 40;
    let triangle = (if phase < 20 { phase } else { 40 - phase }) as f32 * 1.5;
    set_motor(engine, 1, &[triangle]);
    set_motor(engine, 2, &[0.0, 0.0, 30.0 - triangle]);
    set_motor(engine, 3, &[-triangle]);
}

fn driven_trace(engine: &mut Engine, ticks: std::ops::Range<usize>) -> Vec<u64> {
    ticks
        .map(|tick| {
            drive(engine, tick);
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

fn restore(engine: &mut Engine, snapshot: Value) {
    match engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
}

#[test]
fn a_restored_driven_chain_reproduces_the_uninterrupted_trace() {
    let text = scene();

    let mut uninterrupted = game("uninterrupted").build().expect("boots");
    uninterrupted.load_inline_scene(&text).unwrap();
    let reference = driven_trace(&mut uninterrupted, 0..TICKS);
    let distinct: BTreeSet<u64> = reference.iter().copied().collect();
    assert!(
        distinct.len() > MIN_DISTINCT,
        "the reference trace holds {} distinct hashes of {TICKS}; a trace this flat means the \
         chain never moved",
        distinct.len()
    );

    let mut snapshotted = game("snapshotted").build().expect("boots");
    snapshotted.load_inline_scene(&text).unwrap();
    let first_half = driven_trace(&mut snapshotted, 0..SNAPSHOT_AT);
    assert_eq!(
        first_half,
        reference[..SNAPSHOT_AT],
        "two runs of one scene"
    );
    let document = snapshot_of(&mut snapshotted);

    let mut resumed = game("resumed").build().expect("boots");
    resumed.load_inline_scene(&text).unwrap();
    restore(&mut resumed, document);
    assert_eq!(
        resumed.world_hash().unwrap().as_u64(),
        reference[SNAPSHOT_AT - 1],
        "the restore boundary itself matches"
    );
    let second_half = driven_trace(&mut resumed, SNAPSHOT_AT..TICKS);
    for (offset, (resumed_hash, expected)) in second_half
        .iter()
        .zip(&reference[SNAPSHOT_AT..])
        .enumerate()
    {
        assert_eq!(
            resumed_hash,
            expected,
            "tick {} diverged after the resume",
            SNAPSHOT_AT + offset + 1
        );
    }
}

fn thread_trace(threads: usize) -> Vec<u64> {
    let mut engine = game(&format!("threads-{threads}")).build().expect("boots");
    engine.load_inline_scene(&scene()).unwrap();
    driven_trace(&mut engine, 0..TICKS)
}

/// The driven run at 1, 2 and 8 threads — each override installed inside its own spawned thread,
/// the `thread_identity.rs` shape (the override is thread-local).
#[test]
fn a_driven_chain_hashes_identically_at_every_thread_count() {
    #[allow(clippy::disallowed_methods)]
    let [one, two, eight] = std::thread::scope(|scope| {
        THREAD_COUNTS
            .map(|n| scope.spawn(move || par::with_threads(n, || thread_trace(n))))
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });
    assert_eq!(one, two, "the 1-thread and 2-thread traces diverge");
    assert_eq!(one, eight, "the 1-thread and 8-thread traces diverge");
}

/// The `log`-channel lines observed after `ticks` steps of a live engine — the public seam onto
/// `Diagnostics`. Mirrors `physics_scene.rs`'s `log_lines_after`.
fn log_lines_after(mut engine: Engine, ticks: u32) -> Vec<LogEvent> {
    engine
        .execute(Command::SimPause)
        .expect("the engine pauses");
    let (intake, intake_rx) = unbounded();
    let (shutdown, shutdown_rx) = shutdown_channel();
    let (frames, _frames_rx) = bounded::<Egress>(1);
    let (events, events_rx) = unbounded::<Egress>();
    let handle = std::thread::spawn(move || {
        run_live(
            engine,
            intake_rx,
            shutdown_rx,
            Some(EgressChannels { frames, events }),
        )
    });

    let send = |command: Command| {
        let (stamped, reply) = StampedCommand::new(command);
        intake.send(stamped).unwrap();
        reply
            .recv_timeout(Duration::from_secs(5))
            .expect("the sim thread replies")
            .expect("the command succeeds");
    };
    send(Command::StreamSubscribe(StreamSubscribeParams {
        channel: Channel::Log,
        mode: SubscribeMode::LatestWins { max_fps: None },
        format: FrameFormat::Qoi,
    }));
    for _ in 0..ticks {
        send(Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        }));
    }

    let mut lines = Vec::new();
    let mut wait = Duration::from_secs(5);
    while let Ok(Egress::Event(event)) = events_rx.recv_timeout(wait) {
        if let Event::Log(log) = event.message.event {
            lines.push(log);
        }
        wait = Duration::from_millis(300);
    }
    shutdown.signal();
    let _ = handle.join();
    lines
}

/// A motor on a `fixed` joint cannot drive it: one `L3037` across ten ticks, naming the carrier.
#[test]
fn a_motor_on_a_fixed_joint_reports_l3037_once() {
    let mut text = String::from("format: 1\nseed: 5\nphysics:\n  space: free\nentities:\n");
    text.push_str(
        "  - name: anchor\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - 0.0\n          - 0.0\n      Collider:\n        shape:\n          sphere:\n            radius: 0.1\n        body: static\n",
    );
    text.push_str(
        "  - name: welded\n    components:\n      Transform:\n        translation:\n          - 1.0\n          - 0.0\n          - 0.0\n      Collider:\n        shape:\n          sphere:\n            radius: 0.1\n        body:\n          rigid:\n            mass: 1.0\n      Velocity: {}\n      Joint:\n        a:\n          index: 0\n          generation: 0\n        b:\n          index: 1\n          generation: 0\n        kind:\n          fixed: {}\n      JointMotor:\n        target:\n          - 10.0\n        stiffness: 40.0\n        damping: 8.0\n",
    );
    let mut engine = game("fixed-mismatch").build().expect("boots");
    engine.load_inline_scene(&text).unwrap();

    let lines = log_lines_after(engine, 10);
    let reported: Vec<&LogEvent> = lines.iter().filter(|l| code_is(l, "L3037")).collect();
    assert_eq!(reported.len(), 1, "{lines:?}");
    assert!(
        reported[0].message.contains("welded"),
        "{}",
        reported[0].message
    );
    assert!(
        reported[0].message.contains("fixed"),
        "{}",
        reported[0].message
    );
}

/// Whether a log record carries the diagnostic `code` (the structured field, not the rendered text).
fn code_is(log: &LogEvent, code: &str) -> bool {
    log.diagnostic
        .as_ref()
        .is_some_and(|d| d.code.as_str() == code)
}
