//! A `PostProcess.lut` reaches `RenderScene.lut` through the asset cache, and a bad file is named at
//! the component's pointer by both `extract` and the check-time pass.

use std::path::{Path, PathBuf};

use ludion_assets::AssetRoot;
use ludion_assets::cube::identity;
use ludion_engine::{builtin_registry, extract, scene_engine_diagnostics, scene_lut_diagnostics};

const POINTER: &str = "/entities/0/components/PostProcess/lut/path";

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-post-lut-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn cube_text(size: u32) -> String {
    let rows: String = identity(size)
        .rgb
        .iter()
        .map(|r| format!("{} {} {}\n", r[0], r[1], r[2]))
        .collect();
    format!("TITLE \"identity\"\nLUT_3D_SIZE {size}\n{rows}")
}

fn scene_text(lut: &str) -> String {
    format!("format: 1\nentities:\n  - components:\n      PostProcess:\n{lut}")
}

fn world_of(text: &str) -> ludion_ecs::World {
    let registry = builtin_registry();
    let scene = ludion_scene::check(text, &registry).expect("scene checks");
    ludion_scene::instantiate(&scene, &registry).expect("scene instantiates")
}

fn extract_in(
    root: &Path,
    text: &str,
) -> Result<ludion_renderer::RenderScene, Vec<ludion_core::diagnostics::Diagnostic>> {
    extract(&world_of(text), (320, 180), AssetRoot::content(root))
}

#[test]
fn extract_carries_the_lut_and_its_strength() {
    let dir = scratch("carry");
    std::fs::write(dir.join("grade.cube"), cube_text(17)).unwrap();
    let text = scene_text("        lut:\n          path: grade.cube\n          strength: 0.5\n");
    let scene = extract_in(&dir, &text).expect("extracts");
    let lut = scene.lut.expect("the lut is carried");
    assert_eq!(lut.strength, 0.5);
    assert_eq!(lut.lut.size, 17);
    assert_ne!(lut.lut.hash, 0, "the hash is the file's content hash");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_post_process_without_a_lut_extracts_none() {
    let dir = scratch("none");
    let scene = extract_in(&dir, &scene_text("        exposure: 1.2\n")).expect("extracts");
    assert!(scene.lut.is_none());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_missing_file_is_l1101_at_the_pointer_from_extract_and_check() {
    let dir = scratch("missing");
    let text = scene_text("        lut:\n          path: nope.cube\n");
    let errors = extract_in(&dir, &text).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|d| d.code.as_str() == "L1101" && d.path.as_deref() == Some(POINTER)),
        "{errors:?}"
    );
    let registry = builtin_registry();
    let scene = ludion_scene::check(&text, &registry).unwrap();
    for found in [
        scene_lut_diagnostics(&dir, &scene),
        scene_engine_diagnostics(&dir, &scene),
    ] {
        assert!(
            found
                .iter()
                .any(|d| d.code.as_str() == "L1101" && d.path.as_deref() == Some(POINTER)),
            "{found:?}"
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_one_d_file_is_l2391() {
    let dir = scratch("oned");
    std::fs::write(dir.join("one.cube"), "LUT_1D_SIZE 16\n").unwrap();
    let text = scene_text("        lut:\n          path: one.cube\n");
    let errors = extract_in(&dir, &text).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|d| d.code.as_str() == "L2391" && d.path.as_deref() == Some(POINTER)),
        "{errors:?}"
    );
    let scene = ludion_scene::check(&text, &builtin_registry()).unwrap();
    let found = scene_lut_diagnostics(&dir, &scene);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code.as_str(), "L2391");
    std::fs::remove_dir_all(&dir).ok();
}
