//! `ParentJoint`: the two diagnostics (`L1318`, `L1319`), both through `load_inline_scene` and
//! directly through `scene_parent_joint_diagnostics` / `scene_engine_diagnostics`, and the
//! `scene_text()` round trip of a sim entity carrying it.

use std::path::{Path, PathBuf};

use ludion_core::codes::code;
use ludion_engine::Engine;
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
    let joint = |name: &str, parent: Option<u32>, translation: [f64; 3]| MeshJoint {
        name: name.to_string(),
        parent,
        rest: JointRest {
            translation,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        },
        inverse_bind: identity16(),
    };
    let doc = MeshDocument {
        positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        skin: Some(MeshSkin {
            joints: vec![
                joint("root", None, [0.0, 0.0, 0.0]),
                joint("child", Some(0), [0.0, 1.0, 0.0]),
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
