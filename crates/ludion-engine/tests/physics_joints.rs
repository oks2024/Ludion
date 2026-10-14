//! `Plan/physics.md` W3 at the engine level: a scene's `Joint` components resume from an engine
//! snapshot bit-identically, the `physics_snapshot.rs` proof extended to a joint chain instead of a
//! falling pile.
//!
//! The shape of the proof is the same: run a static anchor plus a four-link hinge/ball chain 200
//! ticks uninterrupted, recording the `WorldHash` after each; run a second engine 80 ticks, snapshot
//! it, restore that document into a *third* engine built fresh from the same scene, and run ticks
//! 81..200 there. Every hash from tick 81 on must match the uninterrupted trace, and the trace must
//! be varied — a chain that never swung would hash identically on both sides and prove nothing.

use ludion_core::ids::EntityId;
use ludion_engine::{Engine, EngineBuilder, dispatch};
use ludion_physics::PhysicsWorld;
use ludion_protocol::command::{
    ComponentGetParams, EngineRestoreParams, EntityDestroyParams, EntitySpawnParams,
};
use ludion_protocol::{Command, ResponseBody};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

const TICKS: usize = 200;
const SNAPSHOT_AT: usize = 80;
const MIN_DISTINCT: usize = 100;

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-physics-joints-snapshot-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn game(name: &str) -> EngineBuilder {
    Engine::builder().project_root(fresh_dir(name))
}

fn vec3(indent: &str, key: &str, value: [f32; 3]) -> String {
    let mut text = format!("{indent}{key}:\n");
    for component in value {
        text.push_str(&format!("{indent}  - {component:?}\n"));
    }
    text
}

/// A static anchor at the origin (document position 0) plus a four-link chain, each link a small
/// rigid sphere carrying its own `Joint` back to the previous link — the ragdoll convention
/// (`Plan/physics.md` design decision 2), alternating `hinge` and `ball` kinds so the chain exercises
/// both mappings at once. Every joint's `a`/`b` name document positions with generation 0.
fn scene() -> String {
    let mut text = String::from("format: 1\nseed: 5\nphysics:\n  space: free\nentities:\n");
    text.push_str(
        "  - name: anchor\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - 0.0\n          - 0.0\n      Collider:\n        shape:\n          sphere:\n            radius: 0.1\n        body: static\n",
    );

    let positions = [
        [1.0, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        [3.0, 0.0, 0.0],
        [4.0, 0.0, 0.0],
    ];
    let kinds = [
        "          hinge:\n            axis:\n              - 0.0\n              - 0.0\n              - 1.0\n",
        "          ball: {}\n",
        "          hinge:\n            axis:\n              - 0.0\n              - 0.0\n              - 1.0\n",
        "          ball: {}\n",
    ];
    for (i, (position, kind)) in positions.iter().zip(kinds.iter()).enumerate() {
        let (parent, index) = (i, i + 1);
        text.push_str(&format!(
            "  - name: link{i}\n    components:\n      Transform:\n"
        ));
        text.push_str(&vec3("        ", "translation", *position));
        text.push_str(
            "      Collider:\n        shape:\n          sphere:\n            radius: 0.1\n        body:\n          rigid:\n            mass: 1.0\n      Velocity: {}\n      Joint:\n",
        );
        text.push_str(&format!(
            "        a:\n          index: {parent}\n          generation: 0\n        b:\n          index: {index}\n          generation: 0\n        kind:\n{kind}"
        ));
        text.push_str(
            "        anchor:\n          - 0.0\n          - 0.0\n          - 0.0\n        collide: false\n",
        );
    }
    text
}

/// A static anchor at the origin plus two rigid boxes, all at document positions 0/1/2 (generation
/// 0), carrying no `Joint` — the runtime-created-joint tests author the chain via `entity.spawn`
/// instead of scene text, so the wire ids are known up front.
fn unjoined_scene() -> String {
    let mut text = String::from("format: 1\nseed: 5\nphysics:\n  space: free\nentities:\n");
    text.push_str(
        "  - name: anchor\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - 0.0\n          - 0.0\n      Collider:\n        shape:\n          sphere:\n            radius: 0.1\n        body: static\n",
    );
    for (i, position) in [[1.0, 0.0, 0.0], [2.0, 0.0, 0.0]].iter().enumerate() {
        text.push_str(&format!(
            "  - name: box{i}\n    components:\n      Transform:\n"
        ));
        text.push_str(&vec3("        ", "translation", *position));
        text.push_str(
            "      Collider:\n        shape:\n          sphere:\n            radius: 0.1\n        body:\n          rigid:\n            mass: 1.0\n      Velocity: {}\n",
        );
    }
    text
}

/// The `entity.spawn` params for a carrier whose sole component is a `Joint` between `a` and `b`
/// (both document positions at generation 0), of the given `kind`.
fn joint_carrier(a: u32, b: u32, kind: Value) -> EntitySpawnParams {
    let mut components = BTreeMap::new();
    components.insert(
        "Joint".to_string(),
        json!({
            "a": {"index": a, "generation": 0},
            "b": {"index": b, "generation": 0},
            "kind": kind,
            "anchor": [0.0, 0.0, 0.0],
            "collide": false,
        }),
    );
    EntitySpawnParams {
        name: None,
        components,
    }
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

fn advance_trace(engine: &mut Engine, n: usize) -> Vec<u64> {
    (0..n)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

#[test]
fn a_restored_joint_chain_reproduces_the_uninterrupted_trace() {
    let text = scene();

    let mut uninterrupted = game("uninterrupted").build().expect("boots");
    uninterrupted.load_inline_scene(&text).unwrap();
    let reference = advance_trace(&mut uninterrupted, TICKS);

    let distinct: BTreeSet<u64> = reference.iter().copied().collect();
    assert!(
        distinct.len() > MIN_DISTINCT,
        "the reference trace holds {} distinct hashes of {TICKS}; a trace this flat means the \
         chain never swung",
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
        "a session with a claimed joint chain carries the solver state as the `physics` field"
    );

    let mut resumed = game("resumed").build().expect("boots");
    resumed.load_inline_scene(&text).unwrap();
    restore(&mut resumed, document);
    assert_eq!(
        resumed.world_hash().unwrap().as_u64(),
        reference[SNAPSHOT_AT - 1],
        "the restore boundary itself matches"
    );

    let second_half = advance_trace(&mut resumed, TICKS - SNAPSHOT_AT);
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

/// A wire `entity.spawn` of a semantically invalid component is rejected loudly, and — because
/// `spawn` validates before it mutates the world — leaves no half-built entity behind. Covers both
/// the `Joint` gap and a non-`Joint` component, to pin the fix as generic rather
/// than joint-specific.
#[test]
fn entity_spawn_rejects_a_semantically_invalid_component_and_spawns_nothing() {
    let mut engine = Engine::new();
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .unwrap();
    let before = engine.world().entities().count();

    let mut joint_components = BTreeMap::new();
    joint_components.insert(
        "Joint".to_string(),
        json!({
            "a": {"index": 0, "generation": 0},
            "b": {"index": 0, "generation": 0},
            "kind": {"fixed": {}},
        }),
    );
    let error = dispatch(
        &mut engine,
        Command::EntitySpawn(EntitySpawnParams {
            name: None,
            components: joint_components,
        }),
    )
    .expect_err("a self-joined Joint must be rejected");
    assert_eq!(error.code.as_str(), "L1183");
    assert_eq!(
        engine.world().entities().count(),
        before,
        "a rejected entity.spawn must not have added an entity"
    );

    let mut tilemap_components = BTreeMap::new();
    tilemap_components.insert(
        "Tilemap".to_string(),
        json!({
            "cell_size": 1.0,
            "legend": { "#": {} },
            "rows": ["##", "#"],
        }),
    );
    let error = dispatch(
        &mut engine,
        Command::EntitySpawn(EntitySpawnParams {
            name: None,
            components: tilemap_components,
        }),
    )
    .expect_err("a ragged Tilemap must be rejected");
    assert_eq!(error.code.as_str(), "L1019");
    assert_eq!(
        engine.world().entities().count(),
        before,
        "a rejected entity.spawn must not have added an entity"
    );
}

/// How many distinct hashes [`a_command_created_joint_chain_survives_restore_and_hashes_identically`]
/// requires of its 200-tick trace — the chain must actually swing, not settle flat.
const COMMAND_MIN_DISTINCT: usize = 150;

/// The tick (of the total run) at which the two joints are created by command, after the bodies have
/// fallen freely and unconstrained for that many ticks.
const JOINTS_CREATED_AT: usize = 10;

/// Advances `engine` `upto` ticks, creating the hinge/ball chain by `entity.spawn` right after tick
/// [`JOINTS_CREATED_AT`] (i.e. before `create_at + 1`'s step), and returns the `WorldHash` trace.
fn advance_with_joints_created_at(engine: &mut Engine, upto: usize, create_at: usize) -> Vec<u64> {
    (1..=upto)
        .map(|tick| {
            if tick == create_at + 1 {
                engine
                    .execute(Command::EntitySpawn(joint_carrier(
                        0,
                        1,
                        json!({"hinge": {"axis": [0.0, 0.0, 1.0]}}),
                    )))
                    .expect("the anchor-box0 hinge must spawn");
                engine
                    .execute(Command::EntitySpawn(joint_carrier(
                        1,
                        2,
                        json!({"ball": {}}),
                    )))
                    .expect("the box0-box1 ball joint must spawn");
            }
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

/// `Transform.translation` of the entity at `(index, 0)`, read through `component.get` — the wire
/// path, not a direct ECS peek.
fn translation_of(engine: &mut Engine, index: u32) -> [f32; 3] {
    match dispatch(
        engine,
        Command::ComponentGet(ComponentGetParams {
            entity: EntityId::new(index, 0),
            component: "Transform".to_string(),
        }),
    )
    .unwrap()
    .body
    {
        ResponseBody::ComponentValue { value } => {
            let translation = value["translation"]
                .as_array()
                .expect("Transform always carries a translation array");
            let axis = |i: usize| translation[i].as_f64().expect("finite coordinate") as f32;
            [axis(0), axis(1), axis(2)]
        }
        other => panic!("expected a component value, got {other:?}"),
    }
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// The plan-B proof: no dedicated joint commands exist, so a client forms and tears down a joint with
/// the commands `entity.spawn`/`entity.destroy`/`component.*` already have. A chain created entirely
/// at runtime — not authored in the scene — must resume from a snapshot exactly like the
/// scene-authored chain above.
#[test]
fn a_command_created_joint_chain_survives_restore_and_hashes_identically() {
    let text = unjoined_scene();

    let mut uninterrupted = game("command-uninterrupted").build().expect("boots");
    uninterrupted.load_inline_scene(&text).unwrap();
    let reference = advance_with_joints_created_at(&mut uninterrupted, TICKS, JOINTS_CREATED_AT);

    let distinct: BTreeSet<u64> = reference.iter().copied().collect();
    assert!(
        distinct.len() > COMMAND_MIN_DISTINCT,
        "the reference trace holds {} distinct hashes of {TICKS}; a trace this flat means the \
         chain never swung",
        distinct.len()
    );

    let mut snapshotted = game("command-snapshotted").build().expect("boots");
    snapshotted.load_inline_scene(&text).unwrap();
    let first_part =
        advance_with_joints_created_at(&mut snapshotted, SNAPSHOT_AT, JOINTS_CREATED_AT);
    assert_eq!(
        first_part,
        reference[..SNAPSHOT_AT],
        "two runs of one scenario"
    );
    let document = snapshot_of(&mut snapshotted);
    assert!(
        document.get("physics").and_then(Value::as_str).is_some(),
        "a session with a command-created joint chain carries the solver state as the `physics` field"
    );

    let mut resumed = game("command-resumed").build().expect("boots");
    resumed.load_inline_scene(&text).unwrap();
    restore(&mut resumed, document);
    assert_eq!(
        resumed.world_hash().unwrap().as_u64(),
        reference[SNAPSHOT_AT - 1],
        "the restore boundary itself matches"
    );

    let second_half = advance_trace(&mut resumed, TICKS - SNAPSHOT_AT);
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

    // The joint is live after restore, not silently dropped: box0 stays pinned near the anchor
    // instead of resuming free fall (which would put it many units away by tick 200).
    let anchor_position = translation_of(&mut resumed, 0);
    let box0_position = translation_of(&mut resumed, 1);
    assert!(
        distance(anchor_position, box0_position) < 3.0,
        "box0 drifted to {box0_position:?} (anchor at {anchor_position:?}) after restore; the \
         hinge joint looks dropped"
    );
}

/// Destroying a joint's carrier entity retires the joint from the solver — the destroy half of the
/// plan-B proof (create is covered above).
#[test]
fn destroying_the_carrier_retires_the_joint_and_the_body_falls_away() {
    let mut engine = game("command-destroy").build().expect("boots");
    engine.load_inline_scene(&unjoined_scene()).unwrap();

    let carrier = match engine
        .execute(Command::EntitySpawn(joint_carrier(
            0,
            1,
            json!({"hinge": {"axis": [0.0, 0.0, 1.0]}}),
        )))
        .unwrap()
        .body
    {
        ResponseBody::Spawned { entity } => entity,
        other => panic!("expected Spawned, got {other:?}"),
    };

    engine.advance_live();
    assert_eq!(
        engine
            .world()
            .resource::<PhysicsWorld>()
            .expect("a claimed rigid body installs the solver")
            .joint_count(),
        1,
        "the carrier's Joint attaches on the first tick after it is spawned"
    );
    let pinned_position = translation_of(&mut engine, 1);

    engine
        .execute(Command::EntityDestroy(EntityDestroyParams {
            entity: carrier,
        }))
        .unwrap();
    engine.advance_live();
    assert_eq!(
        engine
            .world()
            .resource::<PhysicsWorld>()
            .unwrap()
            .joint_count(),
        0,
        "destroying the carrier retires its joint from the solver"
    );

    for _ in 0..30 {
        engine.advance_live();
    }
    let fallen_position = translation_of(&mut engine, 1);
    assert!(
        distance(pinned_position, fallen_position) > 1.0,
        "box0 stayed at {fallen_position:?} (was {pinned_position:?}); a retired joint must let \
         it fall away instead of holding it pinned"
    );
}
