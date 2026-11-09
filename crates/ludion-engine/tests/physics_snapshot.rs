//! Gate 2 of `Plan/physics.md` W1 at the engine level: a physics session resumes from an engine
//! snapshot **on the bincode path**, bit-identically.
//!
//! W0's extension proved resume on a JSON document written by a throwaway spike. That proof does not
//! transfer: the snapshot field carries base64 of a binary codec instead, and a codec is exactly the
//! kind of thing a resume proof is about. So this re-establishes it against the shipping path —
//! `engine.snapshot` / `engine.restore` over `SnapshotDoc`'s additive `physics` field — rather than
//! assuming it.
//!
//! The shape of the proof: run a falling pile of rigid bodies 200 ticks uninterrupted, recording the
//! `WorldHash` after each. Then run a second engine 80 ticks, snapshot it, restore that document into
//! a *third* engine built fresh from the same scene, and run ticks 81..200 there. Every hash must
//! match the uninterrupted trace — including tick 81's, which is the first one the resumed solver
//! produces and so the one a decode fault shows up in. The trace is also asserted *varied*, because
//! a pile that collapsed into non-finite values, or that never moved, would hash identically on both
//! sides and prove nothing.

use ludion_engine::{Engine, EngineBuilder};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use ludion_spatial::Plane;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Ticks the uninterrupted reference run covers.
const TICKS: usize = 200;
/// The tick the snapshot is taken at; the resumed run covers `SNAPSHOT_AT + 1 ..= TICKS`.
const SNAPSHOT_AT: usize = 80;
/// How many distinct hashes the reference trace must hold before it counts as varied.
const MIN_DISTINCT: usize = 150;

/// An empty project root, so the engine's content walk finds nothing and the scene is the whole
/// input. One per test name, wiped on entry, so a re-run never inherits a previous one.
fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-physics-snapshot-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A bare-engine builder on a fresh project root; physics is the scene's own `physics:` block, not
/// a builder opt-in.
fn game(name: &str) -> EngineBuilder {
    Engine::builder().project_root(fresh_dir(name))
}

/// A static floor plus `bodies` unit boxes in a jittered lattice above it, high enough that the
/// pile is still falling and settling at tick 200. `space` is the scene's `physics.space` — `free`
/// ignores the engine's default `Plane`, so the pile falls on `y`; `planar` locks the off-plane
/// axis under it.
///
/// Every coordinate is derived from the entity's index by integer arithmetic and a power-of-two
/// division, so the scene text is identical on every machine and every float in it is exact. The
/// offsets are deliberately not a clean grid: a perfectly aligned stack settles into a symmetric
/// pile whose trace is far less varied than a real one, which would hollow out the variety
/// assertion.
fn scene(bodies: usize, space: &str) -> String {
    let mut text = format!("format: 1\nseed: 11\nphysics:\n  space: {space}\nentities:\n");
    text.push_str(&entity(
        "floor",
        [0.0, -0.5, 0.0],
        [0.0, 0.0, 0.0],
        [20.0, 0.5, 20.0],
        false,
    ));
    for i in 0..bodies {
        // Two coprime walks that do not repeat a column for several rows.
        let column = ((i as i32 * 5) % 7) - 3;
        let row = ((i as i32 * 3) % 5) - 2;
        // Quarter- and eighth-unit jitters, exact in binary, enough to keep two boxes in one
        // column from landing perfectly face to face.
        let jitter_x = ((i % 4) as f32) / 4.0;
        let jitter_z = ((i % 8) as f32) / 8.0;
        let tilt = ((i % 9) as f32) * 5.0;
        text.push_str(&entity(
            &format!("crate{i}"),
            [
                column as f32 + jitter_x,
                1.5 + (i as f32) * 1.25,
                row as f32 + jitter_z,
            ],
            [tilt, tilt * 2.0, tilt / 2.0],
            [0.5, 0.5, 0.5],
            true,
        ));
    }
    text
}

/// One scene entity in block style — the only style scene files accept (`L1005` rejects a non-empty
/// flow sequence). `rotation` is Euler XYZ degrees; `rigid` is false for the static floor.
fn entity(
    name: &str,
    translation: [f32; 3],
    rotation: [f32; 3],
    half_extents: [f32; 3],
    rigid: bool,
) -> String {
    let mut text = format!("  - name: {name}\n    components:\n      Transform:\n");
    text.push_str(&vec3("        ", "translation", translation));
    text.push_str(&vec3("        ", "rotation", rotation));
    text.push_str("      Collider:\n        shape:\n          box:\n");
    text.push_str(&vec3("            ", "half_extents", half_extents));
    text.push_str(if rigid {
        "        body:\n          rigid:\n            mass: 1.0\n            friction: 0.5\n            restitution: 0.25\n      Velocity: {}\n"
    } else {
        "        body: static\n"
    });
    text
}

fn vec3(indent: &str, key: &str, value: [f32; 3]) -> String {
    let mut text = format!("{indent}{key}:\n");
    for component in value {
        text.push_str(&format!("{indent}  - {component:?}\n"));
    }
    text
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

/// Advances `n` live ticks, returning the per-tick `WorldHash`.
fn advance_trace(engine: &mut Engine, n: usize) -> Vec<u64> {
    (0..n)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

#[test]
fn a_restored_physics_session_reproduces_the_uninterrupted_trace() {
    let text = scene(30, "free");

    let mut uninterrupted = game("uninterrupted").build().expect("boots");
    uninterrupted.load_inline_scene(&text).unwrap();
    let reference = advance_trace(&mut uninterrupted, TICKS);

    // A pile that went non-finite — or that never moved — would hash identically on both sides and
    // prove nothing, so the reference has to be varied before it is worth comparing against.
    let distinct: BTreeSet<u64> = reference.iter().copied().collect();
    assert!(
        distinct.len() > MIN_DISTINCT,
        "the reference trace holds {} distinct hashes of {TICKS}; a trace this flat means the pile \
         degenerated rather than simulated",
        distinct.len()
    );

    let mut snapshotted = game("snapshotted").build().expect("boots");
    snapshotted.load_inline_scene(&text).unwrap();
    let first_half = advance_trace(&mut snapshotted, SNAPSHOT_AT);
    assert_eq!(
        first_half,
        reference[..SNAPSHOT_AT],
        "two runs of one scene"
    );
    let document = snapshot_of(&mut snapshotted);

    assert!(
        document.get("physics").and_then(Value::as_str).is_some(),
        "a session with rigid bodies carries the solver state as the `physics` field"
    );

    // A third engine, built fresh and loaded from the same scene, so the restore is the only thing
    // carrying the solver's state across — not a world that happens to still hold it.
    let mut resumed = game("resumed").build().expect("boots");
    resumed.load_inline_scene(&text).unwrap();
    restore(&mut resumed, document);
    assert_eq!(
        resumed.world_hash().unwrap().as_u64(),
        reference[SNAPSHOT_AT - 1],
        "the restore boundary itself matches, so a component fault and a solver fault cannot be \
         confused with each other"
    );

    let second_half = advance_trace(&mut resumed, TICKS - SNAPSHOT_AT);
    assert_eq!(
        second_half.len(),
        reference.len() - SNAPSHOT_AT,
        "the resumed run covers ticks {}..={TICKS}",
        SNAPSHOT_AT + 1
    );
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

#[test]
fn a_scene_with_no_rigid_bodies_carries_no_physics_field() {
    let mut engine = game("no-bodies").build().expect("boots");
    // A `circle`/`static` collider is the 2D tier: physics claims neither a 2D shape nor a
    // non-rigid body, so nothing is ever created and the resource never appears — even though the
    // scene's own `physics:` block installs the triple, proving it idle rather than absent.
    engine
        .load_inline_scene(
            "format: 1\nseed: 3\nphysics:\n  space: free\nentities:\n  - name: marker\n    components:\n      Transform:\n        translation:\n          - 1.0\n          - 2.0\n          - 3.0\n      Collider:\n        shape:\n          circle:\n            radius: 0.5\n        body: static\n",
        )
        .unwrap();
    for _ in 0..10 {
        engine.advance_live();
    }
    let document = snapshot_of(&mut engine);
    assert!(
        document.get("physics").is_none(),
        "a session that never claimed an entity has no solver to write, so the field stays absent"
    );
    assert!(
        document.get("scene_physics").is_some(),
        "the scene declared a physics: block, so the snapshot must say so even with an idle solver"
    );
}

/// The other half of the ruling: `Planar` under the engine's default `Xz` plane freezes the world
/// `y` axis on every rigid body, and the lock is observable straight through the mirrored
/// `Transform` — a box that would have fallen a metre in a second does not move at all.
#[test]
fn planar_space_locks_the_off_plane_axis_through_the_mirror() {
    let text = scene(1, "planar");
    let mut engine = Engine::builder()
        .plane(Plane::Xz)
        .project_root(fresh_dir("planar"))
        .build()
        .expect("boots");
    engine.load_inline_scene(&text).unwrap();
    let start = crate_height(&engine);
    for _ in 0..60 {
        engine.advance_live();
    }
    assert_eq!(
        crate_height(&engine),
        start,
        "Xz locks the world y axis, so the one rigid box cannot fall"
    );
}

/// The `y` of the scene's single `crate0`, read off its mirrored `Transform`.
fn crate_height(engine: &Engine) -> f32 {
    let world = engine.world();
    let entity = world
        .entities()
        .find(|&entity| {
            matches!(world.get::<ludion_engine::Name>(entity), Ok(Some(name)) if name.0 == "crate0")
        })
        .expect("the scene authors one crate");
    world
        .get::<ludion_engine::Transform>(entity)
        .expect("a live entity")
        .expect("its transform")
        .translation[1]
}

/// The blob is a pure function of the solver state, so capturing a restored session writes the same
/// document again — which is what lets a replay keyframe be compared against a re-driven run.
#[test]
fn capture_restore_capture_is_byte_identical() {
    let text = scene(12, "free");
    let mut engine = game("round-trip").build().expect("boots");
    engine.load_inline_scene(&text).unwrap();
    for _ in 0..40 {
        engine.advance_live();
    }
    let first = snapshot_of(&mut engine);

    let mut restored = game("round-trip-b").build().expect("boots");
    restored.load_inline_scene(&text).unwrap();
    restore(&mut restored, first.clone());
    let second = snapshot_of(&mut restored);

    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap(),
        "capture -> restore -> capture is the same document text"
    );
}

/// A `physics` field that is not base64, or not a solver state this build can read, is `L3033` with
/// the fix in the message — never a panic and never a silently physics-less restore.
#[test]
fn a_corrupt_physics_blob_is_reported_as_l3033() {
    let text = scene(8, "free");
    let mut engine = game("corrupt").build().expect("boots");
    engine.load_inline_scene(&text).unwrap();
    for _ in 0..20 {
        engine.advance_live();
    }
    let mut document = snapshot_of(&mut engine);
    document["physics"] = Value::String("not base64 at all !!!".to_string());

    let mut target = game("corrupt-target").build().expect("boots");
    target.load_inline_scene(&text).unwrap();
    let diagnostic = target
        .execute(Command::EngineRestore(EngineRestoreParams {
            snapshot: document,
        }))
        .expect_err("a corrupt physics blob fails the restore");
    assert_eq!(diagnostic.code.as_str(), "L3033");
    assert!(
        diagnostic.message.contains("could not be decoded"),
        "the message names the failure: {}",
        diagnostic.message
    );
}

/// Two overlapping `agents` motors on a layer that ignores itself, and a floor.
const MOTOR_PAIR: &str = "format: 1
seed: 3
physics:
  space: free
  layers:
    - name: agents
      ignores:
        - agents
entities:
  - name: floor
    components:
      Transform:
        translation:
          - 0.0
          - -0.5
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 10.0
              - 0.5
              - 10.0
        body: static
  - name: a
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      CharacterMotor:
        layer: agents
  - name: b
    components:
      Transform:
        translation:
          - 0.5
          - 0.0
          - 0.0
      CharacterMotor:
        layer: agents
";

fn push_motors(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
    let mut motors: Vec<_> = world
        .query_ids::<(&ludion_physics::CharacterMotor,)>()
        .map(|(id, _)| id)
        .collect();
    motors.sort();
    // `NoPhysicsWorld` on tick 0 is expected.
    let _ = ludion_engine::move_character(world, motors[0], [0.1, -0.1, 0.0]);
}

fn motor_pair(name: &str) -> Engine {
    let mut engine = game(name)
        .system("push_motors", push_motors)
        .build()
        .expect("boots");
    engine.load_inline_scene(MOTOR_PAIR).unwrap();
    engine
}

/// The restored solver carries its bodies' collision groups, but the layer table that
/// `move_character` filters by must be back before the first game system runs, not only after the
/// next physics write.
#[test]
fn a_motor_moved_on_the_first_tick_after_a_restore_still_honours_the_layers() {
    let mut uninterrupted = motor_pair("layers-reference");
    for _ in 0..4 {
        uninterrupted.advance_live();
    }
    let mut first = motor_pair("layers-first");
    for _ in 0..4 {
        first.advance_live();
    }
    let document = snapshot_of(&mut first);
    let mut resumed = motor_pair("layers-resumed");
    restore(&mut resumed, document);

    uninterrupted.advance_live();
    resumed.advance_live();

    let x_of = |engine: &mut Engine| {
        let mut motors: Vec<_> = engine
            .world_mut()
            .query_ids::<(&ludion_physics::CharacterMotor,)>()
            .map(|(id, _)| id)
            .collect();
        motors.sort();
        engine
            .world()
            .get::<ludion_engine::Transform>(motors[0])
            .unwrap()
            .unwrap()
            .translation
    };
    assert_eq!(x_of(&mut resumed), x_of(&mut uninterrupted));
    assert_eq!(
        resumed.world_hash().unwrap().as_u64(),
        uninterrupted.world_hash().unwrap().as_u64()
    );
}
