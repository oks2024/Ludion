//! `Mesh.blend`: blended instances reach `RenderScene::blended`, never `meshes`; assemblies share one
//! group; a blended mesh casts no shadow; a `Viewmodel` mesh stays opaque with `L2360`; a `Static`
//! blended entity takes the dynamic path with `L1341`; a world without `blend` is unchanged. None of
//! it needs a GPU.

use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::DiagnosticCode;
use ludion_engine::{Diagnostics, Engine, Mesh, MeshBlend};
use ludion_renderer::{BlendMode, RenderScene};

fn engine_for(text: &str) -> Engine {
    let mut engine = Engine::builder().build().expect("the game builds");
    engine.load_inline_scene(text).expect("the scene loads");
    engine
}

fn gallery_engine(text: &str) -> Engine {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery");
    let mut engine = Engine::with_project_root(&root);
    engine.load_inline_scene(text).expect("the scene loads");
    engine
}

fn extract(engine: &mut Engine) -> RenderScene {
    engine.advance_live();
    engine.extract_present_scene().expect("extracts").0
}

fn reported(engine: &Engine, code: DiagnosticCode) -> usize {
    engine
        .world()
        .resource::<Diagnostics>()
        .map_or(0, |d| d.reported(code).count())
}

fn document(entities: &str) -> String {
    format!("format: 1\nseed: 3\nentities:\n{entities}")
}

fn cube(name: &str, extra: &str) -> String {
    format!(
        "  - name: {name}\n    components:\n      Transform: {{}}\n      Mesh:\n        shape: cube\n{extra}"
    )
}

#[test]
fn a_blended_mesh_is_in_the_side_list_and_not_in_meshes() {
    let mut engine = engine_for(&document(&format!(
        "{}{}{}",
        cube("solid", ""),
        cube("glass", "        blend: alpha\n"),
        cube("glow", "        blend: additive\n"),
    )));
    let scene = extract(&mut engine);
    assert_eq!(scene.meshes.len(), 1);
    assert_eq!(scene.blended.len(), 2);
    assert_eq!(scene.blended[0].mode, BlendMode::Alpha);
    assert_eq!(scene.blended[1].mode, BlendMode::Additive);
    assert_eq!(
        (scene.blended[0].group, scene.blended[1].group),
        (0, 1),
        "one group per entity, in extract order"
    );
}

#[test]
fn a_blended_mesh_never_casts_a_shadow() {
    let mut engine = engine_for(&document(&cube("glass", "        blend: alpha\n")));
    let scene = extract(&mut engine);
    assert_eq!(scene.blended.len(), 1);
    assert!(!scene.blended[0].instance.cast_shadows);
}

#[test]
fn every_part_of_a_blended_assembly_shares_a_group() {
    let text = document(
        "  - name: figure\n    components:\n      Transform: {}\n      Mesh:\n        shape:\n          assembly: voxels/figure.assembly.yaml\n        blend: alpha\n",
    ) + &cube("other", "        blend: alpha\n");
    let mut engine = gallery_engine(&text);
    let scene = extract(&mut engine);
    assert!(scene.meshes.is_empty(), "no part is opaque");
    assert!(scene.blended.len() > 2, "the assembly places several parts");
    let last = scene.blended.len() - 1;
    assert!(
        scene.blended[..last].iter().all(|b| b.group == 0),
        "the parts share group 0"
    );
    assert_eq!(scene.blended[last].group, 1);
}

#[test]
fn a_viewmodel_mesh_stays_opaque_and_warns_once() {
    let rig = "  - name: cam
    components:
      Transform: {}
      Camera: {}
      CameraRig:
        target:
          index: 1
          generation: 0
        rig:
          mode:
            first_person: {}
  - name: hero
    components:
      Transform: {}
  - name: gun
    presentation: true
    components:
      Parent:
        index: 0
        generation: 0
      Transform: {}
      Mesh:
        shape: cube
        blend: alpha
      Viewmodel: {}
";
    let mut engine = engine_for(&document(rig));
    let scene = extract(&mut engine);
    assert!(scene.blended.is_empty());
    assert!(scene.meshes.iter().any(|m| m.viewmodel));
    assert_eq!(reported(&engine, code::MESH_BLEND_ON_VIEWMODEL), 1);
    let _ = extract(&mut engine);
    assert_eq!(reported(&engine, code::MESH_BLEND_ON_VIEWMODEL), 1);
}

#[test]
fn a_static_blended_mesh_reports_l1341_and_draws_blended() {
    let mut engine = engine_for(&document(&cube(
        "glass",
        "        blend: alpha\n      Static: {}\n",
    )));
    let scene = extract(&mut engine);
    assert_eq!(scene.blended.len(), 1);
    assert!(scene.meshes.is_empty());
    assert_eq!(reported(&engine, code::STATIC_INELIGIBLE), 1);
}

#[test]
fn a_world_without_blend_has_an_empty_side_list() {
    let mut engine = engine_for(&document(&cube("a", "")));
    let scene = extract(&mut engine);
    assert_eq!(scene.meshes.len(), 1);
    assert!(scene.blended.is_empty());
}

#[test]
fn the_canonical_form_omits_an_absent_blend_and_round_trips_a_set_one() {
    let plain = serde_json::to_value(Mesh::default()).unwrap();
    assert!(plain.get("blend").is_none());
    let glass = Mesh {
        blend: Some(MeshBlend::Additive),
        ..Mesh::default()
    };
    let value = serde_json::to_value(&glass).unwrap();
    assert_eq!(value["blend"], "additive");
    let back: Mesh = serde_json::from_value(value).unwrap();
    assert_eq!(back.blend, Some(MeshBlend::Additive));
}
