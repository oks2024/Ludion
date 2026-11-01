//! `ParentJoint`: the two diagnostics (`L1318`, `L1319`), both through `load_inline_scene` and
//! directly through `scene_parent_joint_diagnostics` / `scene_engine_diagnostics`, and the
//! `scene_text()` round trip of a sim entity carrying it.

use std::path::{Path, PathBuf};

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::math::euler_xyz_degrees_to_quat;
use ludion_core::math::glam::{DMat4, DQuat, DVec3};
use ludion_engine::{Engine, Transform};
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};
use ludion_scene::{JointRest, MeshDocument, MeshJoint, MeshSkin};

fn fresh_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-parent-joint-{}-{name}", std::process::id()));
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

/// Writes `meshes/rig.mesh.yaml`: a two-joint skin, `root` then `child`.
fn write_rig(dir: &Path) {
    let half = 22.5f64.to_radians();
    let joint =
        |name: &str, parent: Option<u32>, translation: [f64; 3], rotation: [f64; 4]| MeshJoint {
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
                joint("root", None, [0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]),
                joint(
                    "child",
                    Some(0),
                    [0.0, 1.0, 0.0],
                    [
                        0.0,
                        0.0,
                        ludion_core::math::f64::sin(half),
                        ludion_core::math::f64::cos(half),
                    ],
                ),
            ],
            weights: vec![1.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            bone_indices: vec![0, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0],
        }),
        indices: vec![0, 1, 2],
        ..MeshDocument::default()
    };
    std::fs::write(
        dir.join("meshes/rig.mesh.yaml"),
        ludion_scene::emit_mesh(&doc),
    )
    .unwrap();
}

const SKIN_MESH: &str = "      Mesh:\n        shape:\n          asset: \"meshes/rig.mesh.yaml\"\n";
const PARENT_OF_SKIN: &str = "      Parent:\n        index: 0\n        generation: 0\n";

fn parent_joint(joint: &str) -> String {
    format!("      ParentJoint:\n        joint: {joint}\n")
}

/// A `skin` entity with exactly `skin_components` and a `part` entity with exactly `part_components`.
fn scene(skin_components: &str, part_components: &str) -> String {
    let mut text = String::from("format: 1\nseed: 3\nphysics:\n  space: free\nentities:\n");
    text.push_str("  - name: skin\n    components:\n");
    text.push_str(skin_components);
    text.push_str("  - name: part\n    components:\n");
    text.push_str(part_components);
    text
}

fn engine_with_rig(name: &str) -> (Engine, PathBuf) {
    let dir = fresh_dir(name);
    write_rig(&dir);
    (Engine::with_project_root(&dir), dir)
}

/// The scene fails to load with `expected`, and both check entry points raise it too.
fn assert_raises(scene_text: &str, dir: &Path, expected: ludion_core::diagnostics::DiagnosticCode) {
    let mut engine = Engine::with_project_root(dir);
    let errors = engine
        .load_inline_scene(scene_text)
        .expect_err("the malformed ParentJoint scene must fail to load");
    assert!(
        errors.iter().any(|d| d.code == expected),
        "expected {expected:?} among {errors:?}"
    );

    let checked = ludion_scene::check(scene_text, &ludion_engine::builtin_registry())
        .unwrap_or_else(|errors| panic!("the fixture is schema-valid: {errors:?}"));
    let direct = ludion_engine::scene_parent_joint_diagnostics(dir, &checked);
    assert!(
        direct.iter().any(|d| d.code == expected),
        "expected {expected:?} among {direct:?}"
    );
    let engine_wide = ludion_engine::scene_engine_diagnostics(dir, &checked);
    assert!(
        engine_wide.iter().any(|d| d.code == expected),
        "expected {expected:?} among {engine_wide:?}"
    );
}

/// `L1318`: the parent carries no `Mesh`.
#[test]
fn a_parent_with_no_mesh_is_l1318() {
    let (_engine, dir) = engine_with_rig("not-skinned");
    let part = format!("{PARENT_OF_SKIN}{}", parent_joint("child"));
    assert_raises(
        &scene(&skin_without_mesh(), &part),
        &dir,
        code::PARENT_JOINT_NOT_SKINNED,
    );
}

/// A skin entity that is a plain transform-only parent.
fn skin_without_mesh() -> String {
    "      Transform:\n        translation:\n          - 0.0\n          - 0.0\n          - 0.0\n"
        .to_string()
}

/// `L1318`: `ParentJoint` with no `Parent` at all.
#[test]
fn a_parent_joint_without_a_parent_is_l1318() {
    let (_engine, dir) = engine_with_rig("no-parent");
    assert_raises(
        &scene(SKIN_MESH, &parent_joint("child")),
        &dir,
        code::PARENT_JOINT_NOT_SKINNED,
    );
}

/// `L1319`: `joint` names no joint of the parent's mesh document; the help carries the nearest name.
#[test]
fn an_unknown_joint_is_l1319_with_the_nearest_name() {
    let (_engine, dir) = engine_with_rig("unknown-joint");
    let part = format!("{PARENT_OF_SKIN}{}", parent_joint("chlid"));
    let text = scene(SKIN_MESH, &part);
    assert_raises(&text, &dir, code::PARENT_JOINT_UNKNOWN_JOINT);

    let mut engine = Engine::with_project_root(&dir);
    let errors = engine
        .load_inline_scene(&text)
        .expect_err("an unknown joint must fail to load");
    let found = errors
        .iter()
        .find(|d| d.code == code::PARENT_JOINT_UNKNOWN_JOINT)
        .expect("L1319 among the errors");
    assert!(
        found
            .help
            .as_deref()
            .is_some_and(|help| help.contains("child")),
        "help should name the nearest joint: {found:?}"
    );
}

/// A sim `ParentJoint` child under the skin loads, `scene_text()` carries the component, and the
/// saved text reloads to the same `WorldHash`.
#[test]
fn a_parent_joint_child_loads_and_round_trips() {
    let (mut engine, dir) = engine_with_rig("round-trip");
    let part = format!("{PARENT_OF_SKIN}{}", parent_joint("child"));
    engine
        .load_inline_scene(&scene(SKIN_MESH, &part))
        .expect("a well-formed ParentJoint scene loads");
    let hash = engine.world_hash().unwrap();
    let text = engine.scene_text().unwrap();
    assert!(text.contains("ParentJoint"), "{text}");

    let mut reloaded = Engine::with_project_root(&dir);
    reloaded
        .load_inline_scene(&text)
        .expect("the saved text re-loads");
    assert_eq!(reloaded.world_hash().unwrap(), hash);
}

const SKIN_TRANSFORM: &str = "      Transform:
        translation:
          - 2.0
          - 0.0
          - 0.0
        rotation:
          - 0.0
          - 90.0
          - 0.0
        scale:
          - 2.0
          - 2.0
          - 2.0
";
const CHILD_LOCAL: &str = "      Transform:
        translation:
          - 0.0
          - 0.5
          - 0.25
        rotation:
          - 0.0
          - 30.0
          - 0.0
";
const GRANDCHILD_LOCAL: &str = "      Transform:
        translation:
          - 0.5
          - 0.0
          - 0.0
        rotation:
          - 0.0
          - 20.0
          - 0.0
";

fn local_model(translation: [f64; 3], euler_degrees: [f32; 3]) -> DMat4 {
    let q = euler_xyz_degrees_to_quat(euler_degrees);
    DMat4::from_rotation_translation(
        DQuat::from_xyzw(q[0] as f64, q[1] as f64, q[2] as f64, q[3] as f64),
        DVec3::from_array(translation),
    )
}

fn assert_pose(pose: ludion_engine::WorldPose, expected: DMat4, what: &str) {
    let (scale, rotation, translation) = expected.to_scale_rotation_translation();
    for axis in 0..3 {
        assert!(
            (pose.trs.translation[axis] as f64 - translation[axis]).abs() < 1e-5,
            "{what} translation {:?} vs {translation:?}",
            pose.trs.translation
        );
        assert!(
            (pose.trs.scale[axis] as f64 - scale[axis]).abs() < 1e-5,
            "{what} scale {:?} vs {scale:?}",
            pose.trs.scale
        );
    }
    let q = pose.trs.rotation;
    let dot = DQuat::from_xyzw(q[0] as f64, q[1] as f64, q[2] as f64, q[3] as f64)
        .dot(rotation)
        .abs();
    assert!((dot - 1.0).abs() < 1e-4, "{what} rotation dot {dot}");
}

/// A skin, its `ParentJoint` child (index 1) and the child's own child (index 2).
fn compose_scene() -> String {
    let child = format!("{PARENT_OF_SKIN}{}{CHILD_LOCAL}", parent_joint("child"));
    let mut text = scene(&format!("{SKIN_MESH}{SKIN_TRANSFORM}"), &child);
    text.push_str(
        "  - name: grandchild
    components:
      Parent:
        index: 1
        generation: 0
",
    );
    text.push_str(GRANDCHILD_LOCAL);
    text
}

/// A child under a skin with `ParentJoint` lands at `skin x joint rest x local`, and a grandchild
/// composes on top; `world_affine` agrees.
#[test]
fn a_parent_joint_child_composes_the_joint_rest() {
    let (mut engine, _dir) = engine_with_rig("compose");
    engine
        .load_inline_scene(&compose_scene())
        .expect("the scene loads");

    let skin_model = DMat4::from_scale_rotation_translation(
        DVec3::splat(2.0),
        {
            let q = euler_xyz_degrees_to_quat([0.0, 90.0, 0.0]);
            DQuat::from_xyzw(q[0] as f64, q[1] as f64, q[2] as f64, q[3] as f64)
        },
        DVec3::new(2.0, 0.0, 0.0),
    );
    let half = 22.5f64.to_radians();
    let bone_model = DMat4::from_rotation_translation(
        DQuat::from_xyzw(
            0.0,
            0.0,
            ludion_core::math::f64::sin(half),
            ludion_core::math::f64::cos(half),
        ),
        DVec3::new(0.0, 1.0, 0.0),
    );
    let child_model = skin_model * bone_model * local_model([0.0, 0.5, 0.25], [0.0, 30.0, 0.0]);
    let grand_model = child_model * local_model([0.5, 0.0, 0.0], [0.0, 20.0, 0.0]);

    let world = engine.world();
    let child_id = ludion_core::ids::EntityId::new(1, 0);
    let grand_id = ludion_core::ids::EntityId::new(2, 0);
    assert_pose(
        ludion_engine::world_transform(world, child_id).unwrap(),
        child_model,
        "child",
    );
    assert_pose(
        ludion_engine::world_transform(world, grand_id).unwrap(),
        grand_model,
        "grandchild",
    );
    for (id, model) in [(child_id, child_model), (grand_id, grand_model)] {
        let affine = ludion_engine::world_affine(world, id).unwrap();
        let cols = model.to_cols_array();
        for (c, col) in affine.cols.iter().enumerate() {
            for (r, v) in col.iter().enumerate() {
                assert!(
                    (*v as f64 - cols[c * 4 + r]).abs() < 1e-4,
                    "{id} affine {affine:?}"
                );
            }
        }
    }
}

/// A presentation `ParentJoint` child is never hashed: the scene with and without it hashes equal.
#[test]
fn a_presentation_parent_joint_child_does_not_move_the_hash() {
    let (mut alone, _dir) = engine_with_rig("hash-alone");
    let skin = format!("{SKIN_MESH}{SKIN_TRANSFORM}");
    alone.load_inline_scene(&scene(&skin, CHILD_LOCAL)).unwrap();
    let (mut with, _dir2) = engine_with_rig("hash-with");
    let mut text = scene(&skin, CHILD_LOCAL);
    text.push_str(
        "  - name: fx
    presentation: true
    components:
      Parent:
        index: 0
        generation: 0
",
    );
    text.push_str(&parent_joint("child"));
    with.load_inline_scene(&text).expect("the scene loads");
    assert_eq!(with.world_hash().unwrap(), alone.world_hash().unwrap());
}

/// Reads `world_transform` of the socket child and its child without declaring `Parent`,
/// `ParentJoint` or `Mesh`.
fn read_world(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
    for index in [1, 2] {
        ludion_engine::world_transform(world, EntityId::new(index, 0)).unwrap();
    }
}

/// The hierarchy walk and the joint rest read record no `Parent`, `ParentJoint` or `Mesh` access.
#[test]
fn world_transform_on_a_socket_child_passes_the_query_drift_gate() {
    let dir = fresh_dir("drift");
    write_rig(&dir);
    let mut engine = Engine::builder()
        .project_root(&dir)
        .system("read_world", read_world)
        .query::<(&Transform,)>()
        .build()
        .expect("the game builds");
    engine
        .load_inline_scene(&compose_scene())
        .expect("the scene loads");
    let report = ludion_engine::check_query_drift_with(&mut engine, |e| {
        e.execute(Command::SimStep(SimStepParams {
            ticks: 2,
            hash: false,
        }))
        .map(|_| ())
    })
    .unwrap();
    assert!(report.is_clean(), "{report}");
}

/// The rest pose comes back from a snapshot restored into a fresh engine, bit for bit.
#[test]
fn a_restored_socket_child_composes_the_same_world_transform() {
    let (mut engine, dir) = engine_with_rig("restore");
    engine
        .load_inline_scene(&compose_scene())
        .expect("the scene loads");
    let child = EntityId::new(1, 0);
    let before = ludion_engine::world_transform(engine.world(), child).unwrap();
    let snapshot = match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    let mut fresh = Engine::with_project_root(&dir);
    match fresh
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    let after = ludion_engine::world_transform(fresh.world(), child).unwrap();
    assert_eq!(before, after);
}
