//! Impulses at the engine level: a game system queuing blasts and at-point impulses on the
//! `PhysicsWorld` resource every few ticks resumes from an engine snapshot bit-identically (the
//! blast hits ride the blob), hashes identically at 1, 2 and 8 threads, and an impulse aimed at a
//! static body reports `L3038` once.

use crossbeam_channel::{bounded, unbounded};
use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::live::{StampedCommand, run_live, shutdown_channel};
use ludion_engine::{Egress, EgressChannels, Engine, EngineBuilder};
use ludion_physics::{Blast, Falloff, PhysicsWorld, QueryFilter};
use ludion_protocol::command::{
    EngineRestoreParams, SimStepParams, StreamSubscribeParams, SubscribeMode,
};
use ludion_protocol::event::{Event, LogEvent};
use ludion_protocol::frame::FrameFormat;
use ludion_protocol::{Channel, Command, ResponseBody};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

const TICKS: usize = 200;
/// One tick after the blast at frame 60, so the blob carries that blast's hits.
const SNAPSHOT_AT: usize = 61;
const MIN_DISTINCT: usize = 100;
const THREAD_COUNTS: [usize; 3] = [1, 2, 8];

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-impulse-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn game(name: &str, system: fn(&mut World, &SystemCtx)) -> EngineBuilder {
    Engine::builder()
        .project_root(fresh_dir(name))
        .system("fire", system)
}

/// A static floor (document position 0) and eight rigid boxes above it, block style — the only
/// style scene files accept.
fn scene() -> String {
    let mut text = String::from("format: 1\nseed: 5\nphysics:\n  space: free\nentities:\n");
    text.push_str(
        "  - name: floor\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - -0.5\n          - 0.0\n      Collider:\n        shape:\n          box:\n            half_extents:\n              - 12.0\n              - 0.5\n              - 12.0\n        body: static\n",
    );
    for i in 0..8 {
        let x = (i % 4) as f32 * 1.1 - 1.6;
        let z = (i / 4) as f32 * 1.1 - 0.5;
        text.push_str(&format!(
            "  - name: crate{i}\n    components:\n      Transform:\n        translation:\n          - {x:?}\n          - {:?}\n          - {z:?}\n      Collider:\n        shape:\n          box:\n            half_extents:\n              - 0.4\n              - 0.4\n              - 0.4\n        body:\n          rigid:\n            mass: 1.0\n      Velocity: {{}}\n",
            0.4 + i as f32 * 0.05,
        ));
    }
    text
}

/// A `Linear` blast every 30 ticks from a centre walking along `x`, and an at-point impulse on the
/// first crate every 45 — tick-derived, no transcendentals.
fn fire(world: &mut World, ctx: &SystemCtx) {
    let tick = ctx.frame.as_u64();
    let Some(physics) = world.resource_mut::<PhysicsWorld>() else {
        return;
    };
    if tick != 0 && tick.is_multiple_of(30) {
        let x = (tick % 90) as f32 / 15.0 - 3.0;
        physics
            .radial_impulse(Blast {
                center: [x, 0.3, 0.0],
                radius: 3.0,
                impulse: 10.0,
                falloff: Falloff::Linear,
                filter: QueryFilter::new(),
            })
            .expect("a valid blast queues");
    }
    if tick != 0 && tick.is_multiple_of(45) {
        physics
            .apply_impulse_at_point(EntityId::new(1, 0), [2.0, 5.0, 0.0], [-1.4, 0.6, 0.2])
            .expect("finite inputs queue");
    }
}

fn traced(engine: &mut Engine, ticks: usize) -> Vec<u64> {
    (0..ticks)
        .map(|_| {
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
fn a_restored_blasted_scene_reproduces_the_uninterrupted_trace() {
    let text = scene();

    let mut uninterrupted = game("uninterrupted", fire).build().expect("boots");
    uninterrupted.load_inline_scene(&text).unwrap();
    let reference = traced(&mut uninterrupted, TICKS);
    let distinct: BTreeSet<u64> = reference.iter().copied().collect();
    assert!(
        distinct.len() > MIN_DISTINCT,
        "the reference trace holds {} distinct hashes of {TICKS}; a trace this flat means the \
         impulses never moved anything",
        distinct.len()
    );

    let mut snapshotted = game("snapshotted", fire).build().expect("boots");
    snapshotted.load_inline_scene(&text).unwrap();
    let first_half = traced(&mut snapshotted, SNAPSHOT_AT);
    assert_eq!(
        first_half,
        reference[..SNAPSHOT_AT],
        "two runs of one scene"
    );
    assert!(
        !snapshotted
            .world()
            .resource::<PhysicsWorld>()
            .expect("the scene has physics")
            .blast_hits()
            .is_empty(),
        "the snapshot must land right after a blast that hit something"
    );
    let document = snapshot_of(&mut snapshotted);

    let mut resumed = game("resumed", fire).build().expect("boots");
    resumed.load_inline_scene(&text).unwrap();
    restore(&mut resumed, document);
    assert_eq!(
        resumed.world_hash().unwrap().as_u64(),
        reference[SNAPSHOT_AT - 1],
        "the restore boundary itself matches"
    );
    let second_half = traced(&mut resumed, TICKS - SNAPSHOT_AT);
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
    let mut engine = game(&format!("threads-{threads}"), fire)
        .build()
        .expect("boots");
    engine.load_inline_scene(&scene()).unwrap();
    traced(&mut engine, TICKS)
}

/// The blasted run at 1, 2 and 8 threads — each override installed inside its own spawned thread,
/// the `thread_identity.rs` shape (the override is thread-local).
#[test]
fn a_blasted_scene_hashes_identically_at_every_thread_count() {
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

/// Pushes the static floor (entity 0) every tick.
fn shove_the_floor(world: &mut World, _ctx: &SystemCtx) {
    if let Some(physics) = world.resource_mut::<PhysicsWorld>() {
        physics
            .apply_impulse(EntityId::new(0, 0), [1.0, 0.0, 0.0])
            .expect("a finite impulse queues");
    }
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

/// An impulse on a static body is dropped: one `L3038` across ten ticks, naming the entity.
#[test]
fn an_impulse_on_a_static_body_reports_l3038_once() {
    let mut engine = game("static-target", shove_the_floor)
        .build()
        .expect("boots");
    engine.load_inline_scene(&scene()).unwrap();

    let lines = log_lines_after(engine, 10);
    let reported: Vec<&LogEvent> = lines.iter().filter(|l| code_is(l, "L3038")).collect();
    assert_eq!(reported.len(), 1, "{lines:?}");
    assert!(
        reported[0].message.contains("floor"),
        "{}",
        reported[0].message
    );
    assert!(
        reported[0].message.contains("the impulse was dropped"),
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
