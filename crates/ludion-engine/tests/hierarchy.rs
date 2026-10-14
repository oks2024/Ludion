//! `Transform` under a `Parent` (`Plan/transform-hierarchy-and-prefabs.md` §Rulings 1, 3): the
//! world walk against a flat `f64` oracle, shear, roots staying bit-identical, and the three
//! malformed chains degrading with one diagnostic each.

use std::path::PathBuf;
use std::time::Duration;

use crossbeam_channel::{bounded, unbounded};
use ludion_core::ids::EntityId;
use ludion_core::math::euler_xyz_degrees_to_quat;
use ludion_core::math::glam::{DMat4, DQuat, DVec3};
use ludion_ecs::{Parent, SystemCtx, World};
use ludion_engine::live::{StampedCommand, run_live, shutdown_channel};
use ludion_engine::{
    Egress, EgressChannels, Engine, Transform, world_affine, world_pose_transform, world_transform,
};
use ludion_protocol::command::{SimStepParams, StreamSubscribeParams, SubscribeMode};
use ludion_protocol::event::{Event, LogEvent};
use ludion_protocol::frame::FrameFormat;
use ludion_protocol::{Channel, Command};
use proptest::prelude::*;

fn oracle(t: &Transform) -> DMat4 {
    let q = euler_xyz_degrees_to_quat(t.rotation);
    DMat4::from_scale_rotation_translation(
        DVec3::new(t.scale[0] as f64, t.scale[1] as f64, t.scale[2] as f64),
        DQuat::from_xyzw(q[0] as f64, q[1] as f64, q[2] as f64, q[3] as f64),
        DVec3::new(
            t.translation[0] as f64,
            t.translation[1] as f64,
            t.translation[2] as f64,
        ),
    )
}

fn assert_affine_close(
    got: &ludion_core::math::Affine3,
    want: &DMat4,
    bound: &DMat4,
    rel: f64,
    what: &str,
) -> Result<(), TestCaseError> {
    let want = want.to_cols_array_2d();
    let bound = bound.to_cols_array_2d();
    for (c, col) in got.cols.iter().enumerate() {
        for (r, value) in col.iter().enumerate() {
            let expected = want[c][r];
            // `bound` is the forward-error scale: a chain of large terms that cancel to a small entry
            // carries error of the large terms, so the entry's own magnitude is the wrong yardstick.
            let tolerance = rel * (1.0 + bound[c][r]);
            prop_assert!(
                (*value as f64 - expected).abs() <= tolerance,
                "{what}: col {c} row {r}: {value} vs {expected}"
            );
        }
    }
    Ok(())
}

fn spawn(world: &mut World, transform: Transform, parent: Option<EntityId>) -> EntityId {
    let id = world.spawn();
    world.insert(id, transform).unwrap();
    if let Some(parent) = parent {
        world.insert(id, Parent(parent)).unwrap();
    }
    id
}

fn arb_transform() -> impl Strategy<Value = Transform> {
    (
        prop::array::uniform3(-50.0f32..50.0),
        prop::array::uniform3(-180.0f32..180.0),
        0.25f32..4.0,
    )
        .prop_map(|(translation, rotation, s)| Transform {
            translation,
            rotation,
            scale: [s; 3],
        })
}

/// Each entity's parent index among the earlier ones (or none), limited to depth 6.
fn arb_forest() -> impl Strategy<Value = Vec<(Transform, Option<usize>)>> {
    prop::collection::vec((arb_transform(), prop::option::of(0usize..1000)), 1..=24)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, rng_seed: proptest::test_runner::RngSeed::Fixed(7919), failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn world_walk_matches_the_flat_oracle(forest in arb_forest()) {
        let mut world = World::new();
        let mut flat = World::new();
        let mut ids: Vec<EntityId> = Vec::new();
        let mut depth: Vec<usize> = Vec::new();
        let mut oracles: Vec<DMat4> = Vec::new();
        let mut bounds: Vec<DMat4> = Vec::new();
        for (i, (transform, pick)) in forest.iter().enumerate() {
            let parent = pick
                .filter(|_| i > 0)
                .map(|p| p % i)
                .filter(|&p| depth[p] < 6);
            let id = spawn(&mut world, *transform, parent.map(|p| ids[p]));
            depth.push(parent.map_or(0, |p| depth[p] + 1));
            oracles.push(match parent {
                Some(p) => oracles[p] * oracle(transform),
                None => oracle(transform),
            });
            bounds.push(match parent {
                Some(p) => bounds[p] * oracle(transform).abs(),
                None => oracle(transform).abs(),
            });
            ids.push(id);
        }
        for (i, id) in ids.iter().enumerate() {
            let pose = world_transform(&world, *id).unwrap();
            assert_affine_close(&pose.trs.to_affine(), &oracles[i], &bounds[i], 1e-4, "world_transform")?;
            prop_assert!(!pose.sheared);
            let affine = world_affine(&world, *id).unwrap();
            assert_affine_close(&affine, &oracles[i], &bounds[i], 1e-4, "world_affine")?;

            let flat_pose = world_pose_transform(&world, *id).unwrap();
            let flat_id = spawn(&mut flat, flat_pose, None);
            let flat_affine = world_affine(&flat, flat_id).unwrap();
            assert_affine_close(&flat_affine, &oracles[i], &bounds[i], 1e-3, "flattened")?;
        }
    }
}

#[test]
fn non_uniform_scale_under_rotation_shears() {
    let mut world = World::new();
    let parent = spawn(
        &mut world,
        Transform {
            translation: [1.0, 2.0, 3.0],
            scale: [2.0, 1.0, 0.5],
            ..Transform::default()
        },
        None,
    );
    let child = Transform {
        translation: [4.0, 0.0, 1.0],
        rotation: [0.0, 0.0, 45.0],
        ..Transform::default()
    };
    let child = spawn(&mut world, child, Some(parent));
    let pose = world_transform(&world, child).unwrap();
    assert!(pose.sheared);
    let want = oracle(&Transform {
        translation: [1.0, 2.0, 3.0],
        scale: [2.0, 1.0, 0.5],
        ..Transform::default()
    }) * oracle(&Transform {
        translation: [4.0, 0.0, 1.0],
        rotation: [0.0, 0.0, 45.0],
        ..Transform::default()
    });
    let got = world_affine(&world, child).unwrap();
    assert_affine_close(&got, &want, &want.abs(), 1e-5, "affine").unwrap();
    let t = want.w_axis;
    for (axis, expected) in pose.trs.translation.iter().zip([t.x, t.y, t.z]) {
        assert!((*axis as f64 - expected).abs() < 1e-5, "{pose:?}");
    }
}

#[test]
fn a_root_keeps_its_stored_transform_bit_for_bit() {
    let mut world = World::new();
    let stored = Transform {
        translation: [0.1, -7.3, 1e-7],
        rotation: [33.3, -91.7, 179.9],
        scale: [1.1, 2.2, 3.3],
    };
    let id = spawn(&mut world, stored, None);
    let got = world_pose_transform(&world, id).unwrap();
    for (a, b) in got
        .translation
        .iter()
        .chain(&got.rotation)
        .chain(&got.scale)
        .zip(
            stored
                .translation
                .iter()
                .chain(&stored.rotation)
                .chain(&stored.scale),
        )
    {
        assert_eq!(a.to_bits(), b.to_bits());
    }
    let pose = world_transform(&world, id).unwrap();
    assert_eq!(pose.trs.translation, stored.translation);
}

/// Builds a valid two-level tree, then a 2-cycle and a dangling parent, on the first tick.
fn build_faults(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 1 {
        return;
    }
    let at = |x: f32| Transform {
        translation: [x, 0.0, 0.0],
        ..Transform::default()
    };
    let root = spawn(world, at(1.0), None);
    spawn(world, at(2.0), Some(root));
    let a = spawn(world, at(3.0), None);
    let b = spawn(world, at(4.0), Some(a));
    world.insert(a, Parent(b)).unwrap();
    let gone = spawn(world, at(5.0), None);
    spawn(world, at(6.0), None);
    let orphan = spawn(world, at(7.0), None);
    world.despawn(gone).unwrap();
    world.insert(orphan, Parent(gone)).unwrap();
}

/// The `log`-channel lines a live session emits over `ticks` ticks (the `path_follow.rs` recipe).
fn log_lines(mut engine: Engine, ticks: u64) -> Vec<LogEvent> {
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
    send(Command::SimStep(SimStepParams { ticks, hash: false }));
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

#[test]
fn malformed_chains_report_once_and_never_hang() {
    let root: PathBuf =
        std::env::temp_dir().join(format!("ludion-hierarchy-faults-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut engine = Engine::builder()
        .project_root(&root)
        .system("build_faults", build_faults)
        .build()
        .expect("the game builds");
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .expect("the scene loads");
    let lines = log_lines(engine, 20);
    let count = |code: &str| lines.iter().filter(|l| code_is(l, code)).count();
    assert_eq!(count("L1218"), 1, "{lines:?}");
    assert_eq!(count("L1219"), 1, "{lines:?}");
    assert_eq!(count("L1220"), 0, "{lines:?}");
}

/// Whether a log record carries the diagnostic `code` (the structured field, not the rendered text).
fn code_is(log: &LogEvent, code: &str) -> bool {
    log.diagnostic
        .as_ref()
        .is_some_and(|d| d.code.as_str() == code)
}
