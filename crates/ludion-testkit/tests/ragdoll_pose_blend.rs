//! The gallery `ragdoll-pose-blend` scene's CPU proofs: a skinned, a voxel and a wall-pinned voxel
//! ragdoll curl under `PoseBlend` (`common::pose_blend_builder`). A weight of zero is physically
//! identical to no `PoseBlend`; a held weight writes no motor after its first tick and the bodies
//! sleep once it returns to zero; the ramp traces deterministically, matches the committed cross-arch
//! trace, restores mid-blend and hashes identically at every thread count. CPU-only on purpose: the
//! scene's golden is its tick-0 render.

mod common;

use std::path::PathBuf;

use common::CurlDrive;
use ludion::prelude::{Collider, JointMotor, JointPoses, PoseBlend, Velocity};
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_engine::{Diagnostics, Engine, EngineBuilder, Name, Transform};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use serde_json::Value;

const TICKS: usize = 240;
const THREAD_COUNTS: [usize; 3] = [1, 2, 8];
/// The tick by which every body of the three groups sleeps once a held weight has returned to zero
/// (`HOLD_END`, tick 150). Measured: all asleep from tick 349; ~50 % margin on the 199 ticks after
/// the hold.
const SETTLE_BY: usize = 450;

fn gallery_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn scene_path() -> PathBuf {
    gallery_root().join("ragdoll-pose-blend.yaml")
}

fn trace_path() -> PathBuf {
    gallery_root().join("ragdoll-pose-blend.trace.txt")
}

fn build_with(builder: EngineBuilder) -> Engine {
    let mut engine = builder
        .project_root(gallery_root())
        .build()
        .expect("the engine builds");
    engine
        .load_inline_scene(&std::fs::read_to_string(scene_path()).expect("the committed scene"))
        .expect("the committed scene loads");
    engine
}

fn build() -> Engine {
    build_with(common::pose_blend_builder(CurlDrive::Ramp))
}

/// One `<tick:04> <hash:016x>` line per tick, from tick `from`.
fn trace_from(engine: &mut Engine, from: usize, to: usize) -> Vec<String> {
    (from..to)
        .map(|tick| {
            engine.advance_live();
            let hash = engine.world_hash().unwrap().as_u64();
            format!("{tick:04} {hash:016x}")
        })
        .collect()
}

fn run(n: usize) -> Vec<String> {
    trace_from(&mut build(), 0, n)
}

fn committed_trace() -> Vec<String> {
    std::fs::read_to_string(trace_path())
        .expect("the committed reference trace")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Every collider-and-velocity entity's `Transform` and `Velocity` as exact bit patterns, ascending
/// id: the physics state a `PoseBlend` must not move at weight zero.
fn body_state(engine: &mut Engine) -> String {
    let world = engine.world_mut();
    let mut bodies: Vec<EntityId> = world
        .query_ids::<(&Collider, &Velocity, &Transform)>()
        .map(|(id, _)| id)
        .collect();
    bodies.sort();
    let bits = |values: &[f32]| {
        values
            .iter()
            .map(|v| format!("{:08x}", v.to_bits()))
            .collect::<Vec<_>>()
            .join(",")
    };
    bodies
        .iter()
        .map(|&id| {
            let transform = world.get::<Transform>(id).unwrap().unwrap();
            let velocity = world.get::<Velocity>(id).unwrap().unwrap();
            format!(
                "{id} t[{}] r[{}] lv[{}] av[{}]",
                bits(&transform.translation),
                bits(&transform.rotation),
                bits(&velocity.linear),
                bits(&velocity.angular),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether any entity carries a `JointMotor`.
fn any_motor(engine: &mut Engine) -> bool {
    engine
        .world_mut()
        .query_ids::<(&JointMotor,)>()
        .next()
        .is_some()
}

/// The parts of the three ragdoll groups: the skin's `SkinBone` parts, then the two voxel figures'
/// parts (named `<root>/<part>`).
fn group_parts(engine: &mut Engine) -> [Vec<EntityId>; 3] {
    let world = engine.world_mut();
    let skin = common::skin_parts(world, common::POSE_SKIN);
    let named = |world: &mut ludion_ecs::World, prefix: &str| {
        let mut parts: Vec<EntityId> = world
            .query_ids::<(&Name,)>()
            .filter(|(_, (name,))| name.0.starts_with(prefix))
            .map(|(id, _)| id)
            .collect();
        parts.sort();
        parts
    };
    let figure = named(world, "figure/");
    let pinned = named(world, "pinned-figure/");
    [skin, figure, pinned]
}

fn all_settled(engine: &mut Engine) -> bool {
    let groups = group_parts(engine);
    groups
        .iter()
        .all(|parts| common::group_settled(engine.world(), parts))
}

#[test]
fn two_in_process_runs_of_the_ragdoll_pose_blend_are_identical() {
    assert_eq!(run(TICKS), run(TICKS));
}

/// The ramp's peak drives every `JointPoses` part to the full curl; its end leaves none driven.
#[test]
fn the_curl_reaches_full_pose_and_returns_limp() {
    let mut engine = build();
    let peak = (common::RAMP_START + 30) as usize;
    for _ in 0..=peak {
        engine.advance_live();
    }
    let parts = common::pose_blend_parts(engine.world_mut());
    assert!(
        parts.len() > 11,
        "the skinned and voxel parts all carry poses"
    );
    for &part in &parts {
        let poses = engine
            .world()
            .get::<JointPoses>(part)
            .unwrap()
            .unwrap()
            .clone();
        let motor = engine
            .world()
            .get::<JointMotor>(part)
            .unwrap()
            .cloned()
            .unwrap_or_else(|| panic!("part {part} is driven at the peak"));
        assert_eq!(
            motor,
            JointMotor {
                target: poses.poses["curl"].clone(),
                stiffness: 40.0,
                damping: 8.0,
                max_force: Some(200.0),
            },
            "part {part} at full weight"
        );
    }
    for _ in peak + 1..=(common::RAMP_START + 60) as usize {
        engine.advance_live();
    }
    assert!(!any_motor(&mut engine), "the ramp's end leaves no motor");
    let diagnostics = engine.world().resource::<Diagnostics>().unwrap();
    assert_eq!(
        diagnostics.reported(code::POSE_BLEND_POSE_MISSING).count(),
        0
    );
    assert_eq!(diagnostics.reported(code::JOINT_MOTOR_MISMATCH).count(), 0);
}

/// (a) A `PoseBlend` at weight zero moves no body and writes no motor: the same parts, the same
/// `JointPoses`, with and without the component. (`PoseBlend` is hashed, so the proof is on the
/// physics state, not `WorldHash`.)
#[test]
fn weight_zero_moves_bodies_exactly_like_no_pose_blend() {
    let mut zero = build_with(common::pose_blend_builder(CurlDrive::Zero));
    let mut absent = build_with(common::pose_blend_builder(CurlDrive::Absent));
    for tick in 0..TICKS {
        zero.advance_live();
        absent.advance_live();
        assert_eq!(
            body_state(&mut zero),
            body_state(&mut absent),
            "the bodies diverge at tick {tick}"
        );
        assert!(
            !any_motor(&mut zero),
            "a motor appears at weight 0, tick {tick}"
        );
        assert!(
            !any_motor(&mut absent),
            "a motor appears with no blend, tick {tick}"
        );
    }
}

fn motors(engine: &mut Engine, parts: &[EntityId]) -> Vec<Option<JointMotor>> {
    parts
        .iter()
        .map(|&part| engine.world().get::<JointMotor>(part).unwrap().cloned())
        .collect()
}

/// (b) A held weight writes each motor once, on the first tick it applies, and never again; when the
/// weight returns to zero the motors go and the bodies fall asleep.
#[test]
fn a_held_weight_writes_no_motor_after_its_first_tick_and_limp_bodies_sleep() {
    let mut engine = build_with(common::pose_blend_builder(CurlDrive::Hold));
    let hold_start = common::HOLD_START as usize;
    let hold_end = common::HOLD_END as usize;
    let mut parts = Vec::new();
    let mut first = Vec::new();
    let mut settled_at = None;
    for tick in 0..SETTLE_BY {
        engine.advance_live();
        if tick == 0 {
            parts = common::pose_blend_parts(engine.world_mut());
        }
        if tick < hold_start {
            assert!(
                !any_motor(&mut engine),
                "no motor before the hold, tick {tick}"
            );
        } else if tick == hold_start {
            first = motors(&mut engine, &parts);
            assert!(
                first.iter().all(Option::is_some),
                "every part is driven on the first held tick"
            );
        } else if tick < hold_end {
            assert_eq!(
                motors(&mut engine, &parts),
                first,
                "a motor was rewritten at tick {tick}"
            );
        } else {
            assert!(
                !any_motor(&mut engine),
                "a motor survives the hold, tick {tick}"
            );
            if settled_at.is_none() && all_settled(&mut engine) {
                settled_at = Some(tick);
            }
        }
    }
    assert!(
        settled_at.is_some_and(|tick| tick <= SETTLE_BY),
        "the limp ragdolls fall asleep by tick {SETTLE_BY}: {settled_at:?}"
    );
}

#[test]
fn the_ragdoll_pose_blend_matches_the_committed_cross_arch_trace() {
    let trace = run(TICKS);
    if ludion_core::bless::bless_requested() {
        let header = "\
# Per-tick WorldHash of examples/gallery/ragdoll-pose-blend.yaml under common::pose_blend_builder(Ramp)\n\
# (crates/ludion-testkit/tests/ragdoll_pose_blend.rs).\n\
# Recorded with: LUDION_BLESS=1 cargo nextest run -p ludion-testkit --test ragdoll_pose_blend -E 'test(cross_arch)'\n\
# The M4 must reproduce this file byte-identically - never fabricated by copying the Windows trace.\n";
        std::fs::write(trace_path(), format!("{header}{}\n", trace.join("\n")))
            .expect("the gallery directory is committed");
        return;
    }
    let committed = committed_trace();
    assert_eq!(
        committed.len(),
        TICKS,
        "the committed trace covers every tick"
    );
    for (tick, (actual, expected)) in trace.iter().zip(&committed).enumerate() {
        assert_eq!(actual, expected, "the trace diverges at tick {tick}");
    }
}

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

/// A snapshot taken while the weight is rising, restored into a fresh engine, continues on the
/// uninterrupted trace.
#[test]
fn a_restore_mid_blend_reproduces_the_trace() {
    let at = common::RAMP_START as usize + 20;
    let expected = run(TICKS);

    let mut warm = build();
    assert_eq!(trace_from(&mut warm, 0, at), expected[..at]);
    assert!(any_motor(&mut warm), "the snapshot is taken mid-blend");
    let blend = warm
        .world_mut()
        .query_ids::<(&PoseBlend,)>()
        .map(|(_, (blend,))| blend.weight)
        .fold(0.0_f32, f32::max);
    assert!(blend > 0.0, "the weight is non-zero at the snapshot");
    let snapshot = snapshot_of(&mut warm);

    let mut resumed = build();
    let response = resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    assert!(
        matches!(response.body, ResponseBody::Restored { .. }),
        "expected a restore, got {:?}",
        response.body
    );
    assert_eq!(
        format!("{:016x}", resumed.world_hash().unwrap().as_u64()),
        expected[at - 1].split_whitespace().nth(1).unwrap(),
        "the restore boundary matches tick {}",
        at - 1
    );
    assert_eq!(trace_from(&mut resumed, at, TICKS), expected[at..]);
}

/// The full run at 1, 2 and 8 threads, each override installed inside its own spawned thread (the
/// override is thread-local).
#[test]
fn the_pose_blend_hashes_identically_at_every_thread_count() {
    #[allow(clippy::disallowed_methods)]
    let [one, two, eight] = std::thread::scope(|scope| {
        THREAD_COUNTS
            .map(|n| scope.spawn(move || par::with_threads(n, || run(TICKS))))
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });
    assert_eq!(one, two, "the 1-thread and 2-thread traces diverge");
    assert_eq!(one, eight, "the 1-thread and 8-thread traces diverge");
}
