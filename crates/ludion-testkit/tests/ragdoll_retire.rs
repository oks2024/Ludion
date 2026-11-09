//! The ragdoll retire idiom's CPU proofs (`ludion docs physics-joints` "Ragdoll budget"): the
//! switched mannequin settles, `common::retire_settled` drops every part's `Collider`, `Joint` and
//! `Velocity`, and the bodies and joints leave the `PhysicsWorld` while the skin stays posed. The run
//! hashes the retire, restores across it, hashes identically at every thread count, detects a static
//! pin, and re-admits a retired corpse. All comparisons are in-process: no golden, no committed trace.

mod common;

use std::path::PathBuf;

use ludion::prelude::{Body, Collider, Joint, JointKind, PhysicsWorld, Shape, Velocity};
use ludion_core::ids::EntityId;
use ludion_core::math::glam::Mat4;
use ludion_core::par;
use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{Engine, EngineBuilder, QualityParams, SkinBone, Transform, extract_at};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use serde_json::Value;

const MAX_TICKS: usize = 900;
const THREAD_COUNTS: [usize; 3] = [1, 2, 8];
const SKIN: EntityId = EntityId::new(3, 0);

fn gallery_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn scene_text() -> String {
    std::fs::read_to_string(gallery_root().join("ragdoll-skin-switch.yaml"))
        .expect("the committed scene")
}

fn build_with(builder: EngineBuilder) -> Engine {
    let mut engine = builder
        .project_root(gallery_root())
        .build()
        .expect("the engine builds");
    engine
        .load_inline_scene(&scene_text())
        .expect("the committed scene loads");
    engine
}

fn parts(engine: &mut Engine) -> Vec<EntityId> {
    common::skin_parts(engine.world_mut(), SKIN)
}

fn retired(engine: &mut Engine) -> bool {
    let parts = parts(engine);
    !parts.is_empty()
        && parts
            .iter()
            .all(|&part| !engine.world().has::<Collider>(part))
}

/// The tick (index of the advance) on which `retire_builder` drops the colliders.
fn retire_tick() -> usize {
    let mut engine = build_with(common::retire_builder());
    for tick in 0..MAX_TICKS {
        engine.advance_live();
        if retired(&mut engine) {
            return tick;
        }
    }
    panic!("the ragdoll never settled within {MAX_TICKS} ticks");
}

/// `R + 60`: the retire, then a minute of the corpse lying still.
fn ticks() -> usize {
    retire_tick() + 60
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

fn run_with(builder: fn() -> EngineBuilder, n: usize) -> Vec<String> {
    trace_from(&mut build_with(builder()), 0, n)
}

fn counts(engine: &Engine) -> (usize, usize) {
    let physics = engine.world().resource::<PhysicsWorld>().expect("physics");
    (physics.body_count(), physics.joint_count())
}

/// `extract_at`'s posed bone palette for `engine`'s world at its current frame.
fn bone_palette(engine: &Engine) -> Vec<Mat4> {
    extract_at(
        engine.world(),
        (320, 180),
        ludion_assets::AssetRoot::content(engine.project_root()),
        SimTime::from_ticks(engine.frame().as_u64(), TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )
    .expect("the world extracts")
    .bone_palette
}

fn transforms(engine: &mut Engine) -> Vec<Transform> {
    let ids = parts(engine);
    ids.iter()
        .map(|&id| *engine.world().get::<Transform>(id).unwrap().unwrap())
        .collect()
}

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

#[test]
fn a_settled_ragdoll_retires_and_its_bodies_leave_the_physics_world() {
    let r = retire_tick();
    let mut engine = build_with(common::retire_builder());
    for _ in 0..r {
        engine.advance_live();
    }
    let ids = parts(&mut engine);
    assert_eq!(ids.len(), 11);
    assert!(
        common::group_settled(engine.world(), &ids),
        "every part is asleep the tick before the retire"
    );
    let (bodies, joints) = counts(&engine);

    engine.advance_live();
    let (bodies_after, joints_after) = counts(&engine);
    assert_eq!(bodies - bodies_after, 11, "all 11 bodies leave");
    assert_eq!(joints - joints_after, 10, "all 10 joints leave");
    for &id in &ids {
        let world = engine.world();
        assert!(world.has::<Transform>(id) && world.has::<SkinBone>(id));
        assert!(!world.has::<Collider>(id) && !world.has::<Joint>(id));
        assert!(!world.has::<Velocity>(id));
    }
    let missing = engine
        .world_mut()
        .resource_mut::<PhysicsWorld>()
        .unwrap()
        .take_missing_joints();
    assert!(missing.is_empty(), "no inert joint: {missing:?}");
}

#[test]
fn a_retired_skin_stays_posed() {
    let r = retire_tick();
    let mut engine = build_with(common::retire_builder());
    let mut palettes = Vec::new();
    let mut poses = Vec::new();
    for tick in 0..=r + 60 {
        engine.advance_live();
        if tick == common::SWITCH_SKIN_TICK as usize || [r - 1, r, r + 60].contains(&tick) {
            palettes.push((tick, bone_palette(&engine)));
        }
        if tick >= r {
            poses.push(transforms(&mut engine));
        }
    }
    let at = |wanted: usize| &palettes.iter().find(|(tick, _)| *tick == wanted).unwrap().1;
    assert_eq!(at(r - 1), at(r), "the palette holds across the retire");
    assert_eq!(at(r), at(r + 60), "and stays put afterwards");
    assert_ne!(
        at(common::SWITCH_SKIN_TICK as usize),
        at(r),
        "the corpse fell: it is not the rest pose"
    );
    assert!(
        poses.windows(2).all(|pair| pair[0] == pair[1]),
        "part transforms are frozen from the retire"
    );
}

#[test]
fn retiring_is_hashed_state() {
    let r = retire_tick();
    let with = run_with(common::retire_builder, r + 1);
    let without = run_with(common::skin_switch_builder, r + 1);
    assert_eq!(with[..r], without[..r]);
    assert_ne!(with[r], without[r], "the retire moves the hash");
}

fn restored_at(
    at: usize,
    builder: fn() -> EngineBuilder,
    total: usize,
) -> (Vec<String>, Vec<String>) {
    let expected = run_with(builder, total);
    let mut warm = build_with(builder());
    assert_eq!(trace_from(&mut warm, 0, at), expected[..at]);
    let snapshot = snapshot_of(&mut warm);

    let mut resumed = build_with(builder());
    let response = resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    assert!(
        matches!(response.body, ResponseBody::Restored { .. }),
        "expected a restore, got {:?}",
        response.body
    );
    (expected[at..].to_vec(), trace_from(&mut resumed, at, total))
}

#[test]
fn a_restore_after_the_retire_reproduces_the_trace() {
    let r = retire_tick();
    let (expected, actual) = restored_at(r + 10, common::retire_builder, r + 60);
    assert_eq!(actual, expected);
}

/// The sleep state rides the snapshot, so a restore before the settle retires on the same tick.
#[test]
fn a_restore_before_the_settle_reproduces_the_trace() {
    let r = retire_tick();
    let at = common::SWITCH_SKIN_TICK as usize + 20;
    assert!(at < r, "the snapshot is taken before the retire");
    let (expected, actual) = restored_at(at, common::retire_builder, r + 60);
    assert_eq!(actual, expected);
}

#[test]
fn retiring_hashes_identically_at_every_thread_count() {
    let total = ticks();
    #[allow(clippy::disallowed_methods)]
    let [one, two, eight] = std::thread::scope(|scope| {
        THREAD_COUNTS
            .map(|n| {
                scope
                    .spawn(move || par::with_threads(n, || run_with(common::retire_builder, total)))
            })
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });
    assert_eq!(one, two, "the 1-thread and 2-thread traces diverge");
    assert_eq!(one, eight, "the 1-thread and 8-thread traces diverge");
}

#[test]
fn a_static_pin_marks_the_group_pinned() {
    let mut engine = build_with(common::skin_switch_builder());
    for _ in 0..=common::SWITCH_SKIN_TICK {
        engine.advance_live();
    }
    let ids = parts(&mut engine);
    // The parts' own joints join group parts to each other: not a pin.
    assert!(!common::group_pinned(engine.world_mut(), &ids));

    let torso = *ids
        .iter()
        .find(|&&id| engine.world().get::<SkinBone>(id).unwrap().unwrap().bone == "torso")
        .expect("a torso part");
    let at = *engine.world().get::<Transform>(torso).unwrap().unwrap();
    let world = engine.world_mut();
    let anchor = world.spawn();
    world.insert(anchor, at).unwrap();
    world
        .insert(
            anchor,
            Collider {
                shape: Shape::Sphere { radius: 0.05 },
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .unwrap();
    let carrier = world.spawn();
    world
        .insert(
            carrier,
            Joint {
                a: anchor,
                b: torso,
                kind: JointKind::Ball { limits: None },
                anchor: [0.0; 3],
                break_impulse: None,
                collide: false,
            },
        )
        .unwrap();
    assert!(
        common::group_pinned(world, &ids),
        "a static pin is detected"
    );
}

#[test]
fn a_retired_ragdoll_can_be_readmitted() {
    let r = retire_tick();
    let run = || {
        let mut engine = build_with(common::skin_switch_builder());
        for _ in 0..=r {
            engine.advance_live();
        }
        let ids = parts(&mut engine);
        let (bodies, joints) = counts(&engine);
        let retired = common::retire_group(engine.world_mut(), &ids);
        assert_eq!(retired.len(), 11);
        let mut trace = trace_from(&mut engine, 0, 20);
        assert_eq!(counts(&engine), (bodies - 11, joints - 10));

        let torso = ids
            .iter()
            .copied()
            .find(|&id| engine.world().get::<SkinBone>(id).unwrap().unwrap().bone == "torso")
            .unwrap();
        let before = *engine.world().get::<Transform>(torso).unwrap().unwrap();
        common::readmit_group(engine.world_mut(), &retired);
        engine
            .world_mut()
            .insert(
                torso,
                Velocity {
                    linear: [0.0, 3.0, 0.0],
                    angular: [0.0; 3],
                },
            )
            .unwrap();
        trace.extend(trace_from(&mut engine, 0, 30));

        assert_eq!(
            counts(&engine),
            (bodies, joints),
            "bodies and joints are back"
        );
        let after = *engine.world().get::<Transform>(torso).unwrap().unwrap();
        assert_ne!(before, after, "the hit moves the torso");
        let missing = engine
            .world_mut()
            .resource_mut::<PhysicsWorld>()
            .unwrap()
            .take_missing_joints();
        assert!(missing.is_empty(), "no inert joint: {missing:?}");
        trace
    };
    assert_eq!(run(), run());
}
