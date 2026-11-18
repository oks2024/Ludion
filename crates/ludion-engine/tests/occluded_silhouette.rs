//! `OccludedSilhouette`: which meshes reach the render scene's silhouette table and with what style.
//! Inheritance down `Parent` (including a voxel assembly's parts), the `Visibility` and `Viewmodel`
//! exclusions with `L2351`, the eight-style limit with `L2352`, the absent component's empty table and
//! the hash, none of which needs a GPU.

use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::DiagnosticCode;
use ludion_engine::{Diagnostics, Engine};
use ludion_renderer::RenderScene;

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

const CUBE: &str = "      Mesh:\n        shape: cube\n";

fn parent_of(index: u32) -> String {
    format!("      Parent:\n        index: {index}\n        generation: 0\n")
}

/// A block-style colour field (scene files refuse a non-empty flow sequence), at `OccludedSilhouette`'s depth.
fn color(c: [f32; 4]) -> String {
    let lanes: String = c
        .iter()
        .map(|v| {
            format!(
                "          - {v:?}
"
            )
        })
        .collect();
    format!(
        "        color:
{lanes}"
    )
}

/// A cube named `name` carrying `extra` component lines.
fn cube(name: &str, extra: &str) -> String {
    format!("  - name: {name}\n    components:\n      Transform: {{}}\n{CUBE}{extra}")
}

#[test]
fn a_world_without_the_component_has_an_empty_table() {
    let mut engine = engine_for(&document(&cube("a", "")));
    let scene = extract(&mut engine);
    assert!(!scene.meshes.is_empty());
    assert!(scene.silhouettes.meshes.is_empty());
    assert!(scene.silhouettes.styles.is_empty());
}

#[test]
fn a_style_reaches_the_table_with_its_fields() {
    let own = format!(
        "      OccludedSilhouette:\n{}        outline:\n          px: 3.0\n          alpha: 0.5\n        when: always\n        bias: 0.25\n",
        color([0.0, 1.0, 0.0, 0.5])
    );
    let mut engine = engine_for(&document(&cube("a", &own)));
    let scene = extract(&mut engine);
    assert_eq!(scene.silhouettes.styles.len(), 1);
    let style = scene.silhouettes.styles[0];
    assert_eq!(style.color, [0.0, 1.0, 0.0, 0.5]);
    assert_eq!((style.outline_px, style.outline_alpha), (3.0, 0.5));
    assert_eq!(style.bias, 0.25);
    assert!(style.always);
    assert_eq!(scene.silhouettes.meshes.len(), 1);
    assert_eq!(scene.silhouettes.meshes[0].style, 1);
}

#[test]
fn a_root_silhouette_reaches_its_descendants_and_a_nearer_one_wins() {
    let root =
        "  - name: root\n    components:\n      Transform: {}\n      OccludedSilhouette: {}\n";
    let child = format!(
        "  - name: child\n    components:\n      Transform: {{}}\n{CUBE}{}",
        parent_of(0)
    );
    let grandchild = format!(
        "  - name: grandchild\n    components:\n      Transform: {{}}\n{CUBE}{}",
        parent_of(1)
    );
    let own = format!(
        "{}      OccludedSilhouette:\n{}",
        parent_of(0),
        color([1.0, 0.0, 0.0, 1.0])
    );
    let override_child = cube("own", &own);
    let mut engine = engine_for(&document(&format!(
        "{root}{child}{grandchild}{override_child}"
    )));
    let scene = extract(&mut engine);
    assert_eq!(scene.meshes.len(), 3);
    assert_eq!(scene.silhouettes.meshes.len(), 3, "every mesh resolves one");
    assert_eq!(scene.silhouettes.styles.len(), 2);
    let styles: Vec<u8> = scene.silhouettes.meshes.iter().map(|m| m.style).collect();
    assert_eq!(styles.iter().filter(|&&s| s == 1).count(), 2, "{styles:?}");
    assert_eq!(styles.iter().filter(|&&s| s == 2).count(), 1, "{styles:?}");
}

#[test]
fn every_part_of_an_assembly_reaches_the_table() {
    let text = document(
        "  - name: figure\n    components:\n      Transform: {}\n      Mesh:\n        shape:\n          assembly: voxels/figure.assembly.yaml\n      OccludedSilhouette: {}\n",
    );
    let mut engine = gallery_engine(&text);
    let scene = extract(&mut engine);
    assert!(scene.meshes.len() > 1, "the assembly places several parts");
    let tagged: Vec<u32> = scene.silhouettes.meshes.iter().map(|m| m.mesh).collect();
    let all: Vec<u32> = (0..scene.meshes.len() as u32).collect();
    assert_eq!(tagged, all);
}

#[test]
fn a_hidden_entity_leaves_the_table() {
    let hidden = "      Visibility:\n        visible: false\n      OccludedSilhouette: {}\n";
    let shown = "      OccludedSilhouette: {}\n";
    let mut engine = engine_for(&document(&format!(
        "{}{}",
        cube("hidden", hidden),
        cube("shown", shown)
    )));
    let scene = extract(&mut engine);
    assert_eq!(scene.meshes.len(), 1);
    assert_eq!(scene.silhouettes.meshes.len(), 1);
    assert_eq!(scene.silhouettes.meshes[0].mesh, 0);
}

const CAMERA_RIG: &str = "  - name: cam
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
      Viewmodel: {}
      OccludedSilhouette: {}
";

#[test]
fn a_viewmodel_is_never_silhouetted_and_warns_once() {
    let wall = cube("wall", "      OccludedSilhouette: {}\n");
    let mut engine = engine_for(&document(&format!("{CAMERA_RIG}{wall}")));
    let scene = extract(&mut engine);
    let gun = scene
        .meshes
        .iter()
        .position(|m| m.viewmodel)
        .expect("the gun extracts as a viewmodel mesh");
    assert!(
        scene
            .silhouettes
            .meshes
            .iter()
            .all(|m| m.mesh as usize != gun),
        "the viewmodel is not in the table"
    );
    assert_eq!(scene.silhouettes.meshes.len(), 1, "the wall still is");
    assert_eq!(reported(&engine, code::OCCLUDED_SILHOUETTE_ON_VIEWMODEL), 1);
    extract_again(&mut engine);
    assert_eq!(reported(&engine, code::OCCLUDED_SILHOUETTE_ON_VIEWMODEL), 1);
}

fn extract_again(engine: &mut Engine) {
    let _ = extract(engine);
}

#[test]
fn a_ninth_style_is_not_drawn_and_warns_once() {
    let mut entities = String::new();
    for i in 0..9 {
        let extra = format!(
            "      OccludedSilhouette:\n{}",
            color([i as f32 / 16.0, 0.0, 0.0, 1.0])
        );
        entities += &cube(&format!("c{i}"), &extra);
    }
    let mut engine = engine_for(&document(&entities));
    let scene = extract(&mut engine);
    assert_eq!(scene.meshes.len(), 9);
    assert_eq!(scene.silhouettes.styles.len(), 8);
    assert_eq!(scene.silhouettes.meshes.len(), 8, "the ninth gets none");
    assert_eq!(reported(&engine, code::OCCLUDED_SILHOUETTE_STYLE_LIMIT), 1);
}

#[test]
fn identical_styles_share_a_slot() {
    let mut entities = String::new();
    for i in 0..12 {
        entities += &cube(&format!("c{i}"), "      OccludedSilhouette: {}\n");
    }
    let mut engine = engine_for(&document(&entities));
    let scene = extract(&mut engine);
    assert_eq!(scene.silhouettes.styles.len(), 1);
    assert_eq!(scene.silhouettes.meshes.len(), 12);
    assert_eq!(reported(&engine, code::OCCLUDED_SILHOUETTE_STYLE_LIMIT), 0);
}

#[test]
fn a_presentation_silhouette_does_not_move_the_world_hash() {
    let with = |component: bool| {
        let extra = if component {
            "      OccludedSilhouette: {}\n"
        } else {
            ""
        };
        document(&format!(
            "{}  - name: fx\n    presentation: true\n    components:\n      Transform: {{}}\n{CUBE}{extra}",
            cube("a", "")
        ))
    };
    let mut on = engine_for(&with(true));
    let mut off = engine_for(&with(false));
    for tick in 0..10 {
        on.advance_live();
        off.advance_live();
        assert_eq!(
            on.world_hash().unwrap(),
            off.world_hash().unwrap(),
            "tick {tick}"
        );
    }
}
