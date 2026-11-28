//! A skinned assembly as `Mesh.shape`: the gallery mannequin loads with its four clips and
//! effective overlay values, extracts to its skinned mesh, and every load
//! failure is pointed at the referencing `Mesh` component.

use std::path::{Path, PathBuf};

use ludion_core::codes::code;
use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{Engine, QualityParams, extract_at, load_character};
use ludion_renderer::MeshShape;
use ludion_scene::{MeshDocument, emit_mesh};

const CHARACTER: &str = "characters/mannequin.assembly.yaml";
const POINTER: &str = "/entities/0/components/Mesh/shape/assembly";

fn gallery() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

/// A scratch project holding copies of the gallery mannequin's character, mesh and clip files.
fn fresh_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-character-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for sub in ["characters", "meshes", "clips"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    let files = [
        CHARACTER,
        "meshes/skin-mannequin.mesh.yaml",
        "clips/idle.anim.yaml",
        "clips/jump.anim.yaml",
        "clips/run.anim.yaml",
        "clips/spin.anim.yaml",
    ];
    for file in files {
        std::fs::copy(gallery().join(file), dir.join(file)).unwrap();
    }
    dir
}

const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: hero
    components:
      Mesh:
        shape:
          assembly: characters/mannequin.assembly.yaml
";

/// Loads `scene` at `root` and extracts it, returning the extract result.
fn extract(
    root: &Path,
    scene: &str,
) -> Result<ludion_renderer::RenderScene, Vec<ludion_core::diagnostics::Diagnostic>> {
    let mut engine = Engine::with_project_root(root);
    engine.load_inline_scene(scene).expect("the scene loads");
    extract_at(
        engine.world(),
        (320, 180),
        ludion_assets::AssetRoot::content(root),
        SimTime::from_ticks(0, TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )
}

#[test]
fn the_gallery_mannequin_resolves_four_clips_with_effective_overlays() {
    let data = load_character(ludion_assets::AssetRoot::content(&gallery()), CHARACTER)
        .expect("the gallery character loads");
    assert_eq!(data.mesh, "meshes/skin-mannequin.mesh.yaml");
    assert_eq!(data.clips.len(), 4);
    let run = data.clip("run").expect("`run` is bound");
    assert_eq!(run.speed, 1.2);
    assert_eq!(run.events.len(), 2);
    assert_eq!(run.events[0].name, "foot_l");
    assert_eq!(run.clip.duration, 0.8);
    assert_eq!(data.clip("idle").unwrap().speed, 1.0);
    assert!(data.clip("idle").unwrap().events.is_empty());
    assert!(data.clip("dance").is_none());
}

#[test]
fn a_character_extracts_to_its_skinned_mesh() {
    let root = fresh_project("extract");
    let scene = extract(&root, SCENE).expect("the character extracts");
    assert_eq!(scene.meshes.len(), 1);
    assert!(matches!(scene.meshes[0].shape, MeshShape::Asset(_)));
    assert!(
        scene.meshes[0].bone_offset > 0,
        "a skinned mesh gets a palette slice"
    );
    assert!(!scene.bone_palette.is_empty());
}

#[test]
fn a_character_and_a_plain_asset_of_the_same_mesh_share_one_upload() {
    let root = fresh_project("shared");
    let scene_text = format!(
        "{SCENE}  - name: plain\n    components:\n      Mesh:\n        shape:\n          asset: meshes/skin-mannequin.mesh.yaml\n"
    );
    let scene = extract(&root, &scene_text).expect("both extract");
    assert_eq!(scene.meshes.len(), 2);
    assert_eq!(scene.mesh_assets.len(), 1);
}

#[test]
fn a_missing_clip_file_is_l1101_at_the_character() {
    let root = fresh_project("missing-clip");
    std::fs::remove_file(root.join("clips/spin.anim.yaml")).unwrap();
    let errors = extract(&root, SCENE).expect_err("the clip is gone");
    assert!(
        errors.iter().any(|d| d.code == code::ASSET_FILE_IO),
        "{errors:?}"
    );
    assert!(
        errors.iter().all(|d| d.path.as_deref() == Some(POINTER)),
        "{errors:?}"
    );
}

/// A hand-authored clip (no `generated_from`) with its own events, bound by a character that also
/// overlays events on it.
#[test]
fn overlay_events_on_a_clip_with_its_own_are_l1200() {
    let root = fresh_project("overlay");
    let idle = std::fs::read_to_string(root.join("clips/run.anim.yaml")).unwrap();
    let without_source: String = idle
        .lines()
        .filter(|line| !line.starts_with("generated_from"))
        .map(|line| format!("{line}\n"))
        .collect::<String>()
        .replace(
            "loop: true\n",
            "loop: true\nevents:\n  - t: 0.2\n    name: own\n",
        );
    std::fs::write(root.join("clips/run.anim.yaml"), without_source).unwrap();
    let errors = extract(&root, SCENE).expect_err("the overlay is redundant");
    assert!(
        errors
            .iter()
            .any(|d| d.code == code::ASSEMBLY_DATA_CONSISTENCY),
        "{errors:?}"
    );
    assert!(
        errors.iter().all(|d| d.path.as_deref() == Some(POINTER)),
        "{errors:?}"
    );
}

#[test]
fn a_track_naming_an_unknown_joint_is_l1200() {
    let root = fresh_project("unknown-joint");
    let clip = "format: 1\nkind: animation\nname: spin\nduration: 0.4\nloop: false\ntracks:\n  tail:\n    rotation:\n      - t: 0.0\n        v:\n          - 0.0\n          - 0.0\n          - 0.0\n      - t: 0.4\n        v:\n          - 0.0\n          - 90.0\n          - 0.0\n";
    std::fs::write(root.join("clips/spin.anim.yaml"), clip).unwrap();
    let errors = extract(&root, SCENE).expect_err("`tail` is not a joint");
    assert!(
        errors
            .iter()
            .any(|d| d.code == code::ASSEMBLY_DATA_CONSISTENCY),
        "{errors:?}"
    );
    assert!(
        errors.iter().all(|d| d.path.as_deref() == Some(POINTER)),
        "{errors:?}"
    );
}

#[test]
fn an_unskinned_mesh_is_l1200() {
    let root = fresh_project("unskinned");
    let flat = MeshDocument {
        positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        indices: vec![0, 1, 2],
        ..MeshDocument::default()
    };
    std::fs::write(
        root.join("meshes/skin-mannequin.mesh.yaml"),
        emit_mesh(&flat),
    )
    .unwrap();
    let errors = extract(&root, SCENE).expect_err("the mesh has no skin");
    assert!(
        errors
            .iter()
            .any(|d| d.code == code::ASSEMBLY_DATA_CONSISTENCY),
        "{errors:?}"
    );
    assert!(
        errors.iter().all(|d| d.path.as_deref() == Some(POINTER)),
        "{errors:?}"
    );
}

/// A `SkinBone` part resolves its skin's joints through a character's mesh, and `L1197` still
/// fires when that skin also plays an `Animation`.
#[test]
fn skin_bone_follows_a_characters_skin() {
    let root = fresh_project("skin-bone");
    let part = "  - name: hand\n    components:\n      SkinBone:\n        skin:\n          index: 0\n          generation: 0\n        bone: lower_arm_l\n";
    let mut engine = Engine::with_project_root(&root);
    engine
        .load_inline_scene(&format!("{SCENE}{part}"))
        .expect("the part resolves its bone through the character");

    let animated = SCENE.replace(
        "      Mesh:",
        "      Animation:\n        clip: run\n      Mesh:",
    );
    let mut engine = Engine::with_project_root(&root);
    let errors = engine
        .load_inline_scene(&format!("{animated}{part}"))
        .expect_err("an animated skin has one pose source");
    assert!(
        errors
            .iter()
            .any(|d| d.code == code::SKIN_BONE_ANIMATED_SKIN),
        "{errors:?}"
    );
}

/// The committed gallery character is in canonical form.
#[test]
fn the_gallery_character_is_canonical() {
    let text = std::fs::read_to_string(gallery().join(CHARACTER)).unwrap();
    let doc = ludion_scene::check_assembly(&text).unwrap();
    assert_eq!(ludion_scene::emit_assembly(&doc), text);
}

/// The retired `{ character: … }` shape, as a scene's component and as a prefab body's.
const RETIRED_SHAPE: &str = "\
format: 1
entities:
  - name: hero
    components:
      Mesh:
        shape:
          character: characters/mannequin.assembly.yaml
";

fn assert_retired(diagnostics: &[ludion_core::diagnostics::Diagnostic], pointer: &str) {
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].code, code::CHARACTER_MERGED);
    assert_eq!(diagnostics[0].path.as_deref(), Some(pointer));
    assert_eq!(
        diagnostics[0].help.as_deref(),
        Some(
            "write `assembly:` for `character:` — `{ assembly: <path> }` draws a skinned assembly \
             and a voxel one alike"
        )
    );
}

#[test]
fn the_retired_character_shape_is_l1246_in_a_scene() {
    let errors = ludion_scene::check(RETIRED_SHAPE, &ludion_engine::builtin_registry())
        .expect_err("the spelling is retired");
    assert_retired(&errors, "/entities/0/components/Mesh/shape/character");
}

#[test]
fn the_retired_character_shape_is_l1246_over_the_wire() {
    let value = serde_json::json!({ "shape": { "character": CHARACTER } });
    let errors = ludion_scene::check_component("Mesh", &value, &ludion_engine::builtin_registry());
    assert_retired(&errors, "/shape/character");
}

#[test]
fn the_retired_character_shape_is_l1246_in_a_prefab() {
    let prefab = RETIRED_SHAPE.replace("format: 1\n", "format: 1\nkind: prefab\nname: hero\n");
    let errors = ludion_scene::check_prefab(&prefab, &ludion_engine::builtin_registry())
        .expect_err("the spelling is retired");
    assert_retired(&errors, "/entities/0/components/Mesh/shape/character");
}
