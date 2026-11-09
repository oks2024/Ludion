//! `switch_skin_to_ragdoll`: parts land at the bone rest pose where a scene load puts them, carry the
//! skin's velocity, the skin sheds its animation and collider, every refusal leaves the world
//! untouched, and a switch from a sim system is deterministic.

use std::path::{Path, PathBuf};

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Animation, Engine, Mesh, SkinBone, Transform, switch_skin_to_ragdoll};
use ludion_protocol::Command;
use ludion_protocol::command::SimStepParams;
use ludion_scene::{JointRest, MeshDocument, MeshJoint, MeshSkin};
use ludion_spatial::{Collider, Velocity};

const PARTS: &str = "parts.prefab.yaml";

fn fresh_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-skin-ragdoll-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("meshes")).unwrap();
    dir
}

fn identity16() -> [f64; 16] {
    let mut m = [0.0; 16];
    for i in 0..4 {
        m[i * 5] = 1.0;
    }
    m
}

/// A quaternion `[x, y, z, w]` for a rotation of `degrees` about `+Z` (pure-Rust libm, never `f64::sin`).
fn quat_deg_z(degrees: f64) -> [f64; 4] {
    use ludion_core::math::f64::{cos, sin};
    let half = degrees.to_radians() / 2.0;
    [0.0, 0.0, sin(half), cos(half)]
}

/// Writes `meshes/<file>`: a two-joint skin whose child rest carries a translation and a rotation.
fn write_rig(dir: &Path, file: &str) {
    let joint = |name: &str, parent: Option<u32>, translation, rotation| MeshJoint {
        name: name.to_string(),
        parent,
        rest: JointRest {
            translation,
            rotation,
            scale: [1.0, 1.0, 1.0],
        },
        inverse_bind: identity16(),
    };
    let doc = MeshDocument {
        positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        skin: Some(MeshSkin {
            joints: vec![
                joint("root", None, [0.0; 3], [0.0, 0.0, 0.0, 1.0]),
                joint("child", Some(0), [0.0, 1.0, 0.0], quat_deg_z(45.0)),
            ],
            weights: vec![1.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            bone_indices: vec![0, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0],
        }),
        indices: vec![0, 1, 2],
        ..MeshDocument::default()
    };
    std::fs::write(dir.join("meshes").join(file), ludion_scene::emit_mesh(&doc)).unwrap();
}

/// A fixture root with two identical rigs and the plain parts prefab.
fn fixture(name: &str) -> PathBuf {
    let dir = fresh_dir(name);
    write_rig(&dir, "rig.mesh.yaml");
    write_rig(&dir, "other.mesh.yaml");
    let plain = MeshDocument {
        positions: vec![0.0; 3],
        ..MeshDocument::default()
    };
    std::fs::write(
        dir.join("meshes/plain.mesh.yaml"),
        ludion_scene::emit_mesh(&plain),
    )
    .unwrap();
    std::fs::write(dir.join(PARTS), PARTS_TEXT).unwrap();
    dir
}

const COLLIDER: &str = "      Collider:\n        shape:\n          sphere:\n            radius: 0.4\n        body:\n          rigid:\n            mass: 1.0\n";

/// The parts prefab: `root`, `child` (jointed to root) and `$skin` last.
const PARTS_TEXT: &str = "\
format: 1
kind: prefab
name: parts
entities:
  - name: root
    components:
      SkinBone:
        skin:
          index: 2
          generation: 0
        bone: root
      Collider:
        shape:
          sphere:
            radius: 0.4
        body:
          rigid:
            mass: 1.0
  - name: child
    components:
      SkinBone:
        skin:
          index: 2
          generation: 0
        bone: child
      Collider:
        shape:
          sphere:
            radius: 0.4
        body:
          rigid:
            mass: 1.0
      Joint:
        a:
          index: 0
          generation: 0
        b:
          index: 1
          generation: 0
        kind:
          fixed: {}
  - name: $skin
    components:
      Mesh:
        shape:
          asset: \"meshes/rig.mesh.yaml\"
";

const MESH: &str = "      Mesh:\n        shape:\n          asset: \"meshes/rig.mesh.yaml\"\n";
const TRANSFORM: &str = "      Transform:\n        translation:\n          - 2.0\n          - 0.0\n          - 0.0\n        rotation:\n          - 0.0\n          - 90.0\n          - 0.0\n        scale:\n          - 2.0\n          - 2.0\n          - 2.0\n";
const UNIT_TRANSFORM: &str = "      Transform:\n        translation:\n          - 2.0\n          - 0.0\n          - 0.0\n        rotation:\n          - 0.0\n          - 90.0\n          - 0.0\n";

fn skin_scene(components: &str) -> String {
    format!(
        "format: 1\nseed: 3\nphysics:\n  space: free\nentities:\n  - name: enemy\n    components:\n{components}"
    )
}

fn engine_with(dir: &Path, scene: &str) -> Engine {
    let mut engine = Engine::builder()
        .project_root(dir)
        .build()
        .expect("the game builds");
    engine.load_inline_scene(scene).expect("the scene loads");
    engine
}

fn skin_id() -> EntityId {
    EntityId::new(0, 0)
}

fn transform(engine: &Engine, id: EntityId) -> Transform {
    *engine
        .world()
        .get::<Transform>(id)
        .unwrap()
        .expect("a Transform")
}

#[test]
fn parts_land_where_a_scene_load_puts_them() {
    let dir = fixture("placement");
    let mut engine = engine_with(&dir, &skin_scene(&format!("{MESH}{TRANSFORM}")));
    let ids = switch_skin_to_ragdoll(engine.world_mut(), skin_id(), PARTS).expect("switches");
    assert_eq!(ids.len(), 2, "`$skin` spawns nothing");

    let inline = format!(
        "{}  - name: root\n    components:\n      SkinBone:\n        skin:\n          index: 0\n          generation: 0\n        bone: root\n  - name: child\n    components:\n      SkinBone:\n        skin:\n          index: 0\n          generation: 0\n        bone: child\n",
        skin_scene(&format!("{MESH}{TRANSFORM}"))
    );
    let reference = engine_with(&dir, &inline);
    for (id, expected) in ids.iter().zip([1u32, 2]) {
        let got = transform(&engine, *id);
        let want = transform(&reference, EntityId::new(expected, 0));
        for axis in 0..3 {
            assert!(
                (got.translation[axis] - want.translation[axis]).abs() < 1e-4,
                "{got:?} vs {want:?}"
            );
            assert!(
                (got.rotation[axis] - want.rotation[axis]).abs() < 1e-3,
                "{got:?} vs {want:?}"
            );
            assert!(
                (got.scale[axis] - want.scale[axis]).abs() < 1e-4,
                "{got:?} vs {want:?}"
            );
        }
    }

    for (id, bone) in ids.iter().zip(["root", "child"]) {
        let link = engine
            .world()
            .get::<SkinBone>(*id)
            .unwrap()
            .expect("a SkinBone");
        assert_eq!(link.skin, skin_id());
        assert_eq!(link.bone, bone);
    }
    let names: Vec<_> = ids
        .iter()
        .map(|id| {
            engine
                .world()
                .get::<ludion_engine::Name>(*id)
                .unwrap()
                .unwrap()
                .0
                .clone()
        })
        .collect();
    assert_eq!(names, ["enemy/root", "enemy/child"]);
}

#[test]
fn the_skin_keeps_its_mesh_and_sheds_its_animation_collider_and_velocity() {
    let dir = fixture("shed");
    let components = format!(
        "{MESH}{UNIT_TRANSFORM}{COLLIDER}      Velocity:\n        linear:\n          - 1.0\n          - 0.0\n          - 0.0\n"
    );
    let mut engine = engine_with(&dir, &skin_scene(&components));
    let animation: Animation = serde_json::from_value(serde_json::json!({})).unwrap();
    engine.world_mut().insert(skin_id(), animation).unwrap();

    switch_skin_to_ragdoll(engine.world_mut(), skin_id(), PARTS).expect("switches");
    let world = engine.world();
    assert!(world.is_alive(skin_id()));
    assert!(world.get::<Mesh>(skin_id()).unwrap().is_some());
    assert!(world.get::<Animation>(skin_id()).unwrap().is_none());
    assert!(world.get::<Collider>(skin_id()).unwrap().is_none());
    assert!(world.get::<Velocity>(skin_id()).unwrap().is_none());
    engine
        .execute(Command::SimStep(SimStepParams {
            ticks: 3,
            hash: false,
        }))
        .expect("the ragdoll steps");
}

#[test]
fn rigid_parts_inherit_the_skin_velocity_at_their_own_point() {
    let dir = fixture("velocity");
    let moving = format!(
        "{MESH}{UNIT_TRANSFORM}      Velocity:\n        linear:\n          - 1.0\n          - 2.0\n          - 3.0\n        angular:\n          - 0.0\n          - 4.0\n          - 0.0\n"
    );
    let mut engine = engine_with(&dir, &skin_scene(&moving));
    let skin_at = transform(&engine, skin_id()).translation;
    let ids = switch_skin_to_ragdoll(engine.world_mut(), skin_id(), PARTS).expect("switches");
    for id in ids {
        let at = transform(&engine, id).translation;
        let velocity = engine
            .world()
            .get::<Velocity>(id)
            .unwrap()
            .expect("a Velocity");
        let r = [at[0] - skin_at[0], at[1] - skin_at[1], at[2] - skin_at[2]];
        let omega = [0.0f32, 4.0, 0.0];
        let cross = [
            omega[1] * r[2] - omega[2] * r[1],
            omega[2] * r[0] - omega[0] * r[2],
            omega[0] * r[1] - omega[1] * r[0],
        ];
        let want = [1.0 + cross[0], 2.0 + cross[1], 3.0 + cross[2]];
        for axis in 0..3 {
            assert!(
                (velocity.linear[axis] - want[axis]).abs() < 1e-4,
                "{velocity:?} vs {want:?}"
            );
        }
        assert_eq!(velocity.angular, omega);
    }

    let mut still = engine_with(&dir, &skin_scene(&format!("{MESH}{UNIT_TRANSFORM}")));
    for id in switch_skin_to_ragdoll(still.world_mut(), skin_id(), PARTS).expect("switches") {
        let velocity = still
            .world()
            .get::<Velocity>(id)
            .unwrap()
            .expect("a Velocity");
        assert_eq!((velocity.linear, velocity.angular), ([0.0; 3], [0.0; 3]));
    }
}

/// Switches under `scene` with the parts text `parts` (written to `parts.prefab.yaml`) and returns
/// the diagnostic codes of a refusal, asserting the world did not move.
fn refused(tag: &str, scene: &str, parts: &str, path: &str, kill_skin: bool) -> Vec<String> {
    let dir = fixture(tag);
    std::fs::write(dir.join(PARTS), parts).unwrap();
    let mut engine = engine_with(&dir, scene);
    if kill_skin {
        engine.world_mut().despawn(skin_id()).unwrap();
    }
    let hash = engine.world_hash().unwrap();
    let count = engine.world().entity_count();
    let Err(errors) = switch_skin_to_ragdoll(engine.world_mut(), skin_id(), path) else {
        panic!("{tag}: the switch must be refused");
    };
    assert_eq!(engine.world_hash().unwrap(), hash, "{tag}: the hash moved");
    assert_eq!(
        engine.world().entity_count(),
        count,
        "{tag}: entities spawned"
    );
    errors.iter().map(|d| d.code.as_str().to_owned()).collect()
}

#[test]
fn every_refusal_mutates_nothing() {
    let good = skin_scene(&format!("{MESH}{UNIT_TRANSFORM}"));
    let l3036 = [code::RAGDOLL_SWITCH_INVALID.as_str().to_owned()];
    let check = |tag: &str, scene: &str, parts: &str, path: &str, kill: bool, want: &[String]| {
        let got = refused(tag, scene, parts, path, kill);
        assert_eq!(got, want, "{tag}");
    };

    check("dead", &good, PARTS_TEXT, PARTS, true, &l3036);
    check(
        "no-transform",
        &skin_scene(MESH),
        PARTS_TEXT,
        PARTS,
        false,
        &l3036,
    );
    let skewed =
        "      Transform:\n        scale:\n          - 1.0\n          - 2.0\n          - 1.0\n";
    check(
        "skewed",
        &skin_scene(&format!("{MESH}{skewed}")),
        PARTS_TEXT,
        PARTS,
        false,
        &l3036,
    );
    let zero =
        "      Transform:\n        scale:\n          - 0.0\n          - 0.0\n          - 0.0\n";
    check(
        "zero",
        &skin_scene(&format!("{MESH}{zero}")),
        PARTS_TEXT,
        PARTS,
        false,
        &l3036,
    );
    let cube = "      Mesh:\n        shape: cube\n";
    check(
        "cube",
        &skin_scene(&format!("{cube}{UNIT_TRANSFORM}")),
        PARTS_TEXT,
        PARTS,
        false,
        &l3036,
    );
    let plain = "      Mesh:\n        shape:\n          asset: \"meshes/plain.mesh.yaml\"\n";
    check(
        "no-joints",
        &skin_scene(&format!("{plain}{UNIT_TRANSFORM}")),
        PARTS_TEXT,
        PARTS,
        false,
        &l3036,
    );

    check(
        "no-skin-part",
        &good,
        &PARTS_TEXT.replace("name: $skin", "name: tail"),
        PARTS,
        false,
        &l3036,
    );
    let moved = format!("{PARTS_TEXT}  - name: extra\n    components: {{}}\n");
    check("skin-not-last", &good, &moved, PARTS, false, &l3036);
    let other = PARTS_TEXT.replace("meshes/rig.mesh.yaml", "meshes/other.mesh.yaml");
    check("mesh-mismatch", &good, &other, PARTS, false, &l3036);
    let joint_to_skin = PARTS_TEXT.replace(
        "        b:\n          index: 1",
        "        b:\n          index: 2",
    );
    check(
        "joint-names-skin",
        &good,
        &joint_to_skin,
        PARTS,
        false,
        &l3036,
    );

    let unknown = PARTS_TEXT.replace("bone: child", "bone: chld");
    check(
        "unknown-bone",
        &good,
        &unknown,
        PARTS,
        false,
        &[code::SKIN_BONE_UNKNOWN_BONE.as_str().to_owned()],
    );
    check(
        "missing-file",
        &good,
        PARTS_TEXT,
        "missing.prefab.yaml",
        false,
        &[code::PREFAB_UNRESOLVED.as_str().to_owned()],
    );
}

/// Switches at frame 2 from a sim system.
fn switcher(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 2 {
        switch_skin_to_ragdoll(world, skin_id(), PARTS).expect("switches");
    }
}

fn trace(tag: &str) -> Vec<String> {
    let dir = fixture(tag);
    let mut engine = Engine::builder()
        .project_root(&dir)
        .system("switcher", switcher)
        .build()
        .expect("the game builds");
    engine
        .load_inline_scene(&skin_scene(&format!("{MESH}{UNIT_TRANSFORM}{COLLIDER}")))
        .expect("the scene loads");
    let mut hashes = Vec::new();
    for _ in 0..60 {
        engine
            .execute(Command::SimStep(SimStepParams {
                ticks: 1,
                hash: false,
            }))
            .expect("the sim steps");
        hashes.push(format!("{:?}", engine.world_hash().unwrap()));
    }
    assert_eq!(
        engine.world().entity_count(),
        3,
        "the switch spawned two parts"
    );
    hashes
}

#[test]
fn a_switch_from_a_sim_system_is_deterministic() {
    assert_eq!(trace("sim-a"), trace("sim-b"));
}

/// `scene.save` of a switched skin is refused with `L1234`: the parts are prefab-instance members and
/// the save's prefab re-expansion cannot drop `$skin`. The refusal writes no file and leaves the world
/// untouched; `engine.snapshot` is how a switched skin persists.
#[test]
fn saving_a_switched_skin_is_refused_with_l1234_and_changes_nothing() {
    let dir = fixture("save");
    let mut engine = engine_with(
        &dir,
        &skin_scene(&format!("{MESH}{UNIT_TRANSFORM}{COLLIDER}")),
    );
    switch_skin_to_ragdoll(engine.world_mut(), skin_id(), PARTS).expect("switches");
    let hash = engine.world_hash().unwrap();
    let entities = engine.world().entity_count();

    let refusal = engine
        .execute(Command::SceneSave(
            ludion_protocol::command::SceneSaveParams {
                path: "switched.yaml".into(),
                force: false,
            },
        ))
        .expect_err("a switched skin does not save");
    assert_eq!(refusal.code, code::PREFAB_INSTANCE_SAVE);
    assert!(!dir.join("switched.yaml").exists(), "nothing is written");
    assert_eq!(engine.world().entity_count(), entities);
    assert_eq!(engine.world_hash().unwrap(), hash);
}
