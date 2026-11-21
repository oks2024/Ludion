//! Render layers (`RenderLayers`, `Camera.layers`, `RenderView.layers`): what the main frame and each view
//! keep. None needs a GPU: the acceptance is the extracted `RenderScene` and its blend lists.

use ludion_engine::{Engine, builtin_registry};
use ludion_protocol::command::ViewPose;
use ludion_renderer::{RenderScene, view_scene};

type Layers<'a> = Option<&'a [&'a str]>;

/// A `layers:` field at a component's field indent, in block style (a scene has no non-empty flow lists).
fn layers_field(names: Layers) -> String {
    match names {
        None => String::new(),
        Some([]) => "\n        layers: []".to_string(),
        Some(names) => names
            .iter()
            .fold("\n        layers:".to_string(), |out, n| {
                format!("{out}\n          - {n}")
            }),
    }
}

/// The `RenderLayers` component lines, or none.
fn render_layers(names: Layers) -> String {
    names.map_or(String::new(), |_| {
        format!("      RenderLayers:{}\n", layers_field(names))
    })
}

fn camera(layers: Layers) -> String {
    let layers = layers_field(layers);
    format!(
        "  - name: cam
    components:
      Transform:
        translation:
          - 0.0
          - 1.0
          - 8.0
      Camera:
        near: 0.1{layers}
"
    )
}

fn view_entity(layers: Layers) -> String {
    let layers = layers_field(layers);
    format!(
        "  - name: radar-cam
    presentation: true
    components:
      Transform:
        translation:
          - 0.0
          - 9.0
          - 0.0
      Camera: {{}}
      RenderView:
        name: radar
        size:
          - 64
          - 32{layers}
"
    )
}

/// A cube; `layers` is a `RenderLayers` list, `parent` the index of its `Parent`.
fn cube(name: &str, layers: Layers, parent: Option<usize>) -> String {
    let layers = render_layers(layers);
    let parent = parent.map_or(String::new(), |p| {
        format!("      Parent:\n        index: {p}\n        generation: 0\n")
    });
    format!(
        "  - name: {name}
    components:
      Transform: {{}}
      Mesh:
        shape: cube
{layers}{parent}"
    )
}

fn marker(name: &str, layers: Layers) -> String {
    let layers = render_layers(layers);
    format!(
        "  - name: {name}
    components:
      Transform: {{}}
      GroundMarker:
        shape:
          type: disc
          radius: 1.0
{layers}"
    )
}

fn document(entities: &[String]) -> String {
    format!("format: 1\nseed: 3\nentities:\n{}", entities.concat())
}

fn engine_for(document: &str) -> Engine {
    let mut engine = Engine::builder().build().expect("the game builds");
    engine.load_inline_scene(document).expect("the scene loads");
    engine
}

fn extract(engine: &mut Engine) -> RenderScene {
    engine.advance_live();
    engine.extract_present_scene().expect("extracts").0
}

#[test]
fn an_unlayered_scene_takes_no_layer_path() {
    let mut engine = engine_for(&document(&[
        camera(None),
        view_entity(None),
        cube("a", None, None),
    ]));
    let scene = extract(&mut engine);
    assert!(scene.layered.is_none());
    assert_eq!(scene.views[0].layers, u32::MAX);
    assert_eq!(scene.meshes.len(), 1);
}

#[test]
fn a_child_inherits_its_roots_layer_and_its_own_wins() {
    // 0 camera; 1 root (radar); 2 child, no layers; 3 child in `default`; 4 a plain cube.
    let mut engine = engine_for(&document(&[
        camera(Some(&["default"])),
        cube("root", Some(&["radar"]), None),
        cube("inherits", None, Some(1)),
        cube("own", Some(&["default"]), Some(1)),
        cube("plain", None, None),
    ]));
    let scene = extract(&mut engine);
    assert_eq!(
        scene.meshes.len(),
        2,
        "own and plain stay in the main frame"
    );
    let source = scene.layered.as_deref().expect("a layered frame");
    assert_eq!(source.scene.meshes.len(), 4);
    assert_eq!(source.masks.meshes.len(), 4);
    let radar = source.masks.meshes[0];
    assert_ne!(radar, ludion_renderer::DEFAULT_LAYER_BIT);
    assert_eq!(source.masks.meshes[1], radar, "the child inherits");
    assert_eq!(source.masks.meshes[2], ludion_renderer::DEFAULT_LAYER_BIT);
    assert_eq!(source.masks.meshes[3], ludion_renderer::DEFAULT_LAYER_BIT);
}

fn radar_world() -> String {
    document(&[
        camera(Some(&["default"])),
        view_entity(Some(&["radar"])),
        cube("world", None, None),
        cube("radar-mesh", Some(&["radar"]), None).replace(
            "Transform: {}",
            "Transform:
        translation:
          - 5.0
          - 0.0
          - 0.0",
        ),
        marker("world-marker", None),
        marker("radar-marker", Some(&["radar"])),
    ])
}

#[test]
fn the_main_camera_drops_other_layers_and_the_source_keeps_them() {
    let scene = extract(&mut engine_for(&radar_world()));
    assert_eq!(scene.meshes.len(), 1);
    assert_eq!(scene.ground_markers.len(), 1);
    let source = scene.layered.as_deref().expect("a layered frame");
    assert_eq!(source.scene.meshes.len(), 2);
    assert_eq!(source.scene.ground_markers.len(), 2);
}

#[test]
fn a_view_keeps_only_its_layers() {
    let scene = extract(&mut engine_for(&radar_world()));
    assert_ne!(scene.views[0].layers, u32::MAX);
    let view = view_scene(&scene, 0);
    assert_eq!(view.meshes.len(), 1);
    assert_eq!(view.ground_markers.len(), 1);
    assert!(view.layered.is_none());
    assert_eq!(
        view.meshes[0].model.w_axis.x, 5.0,
        "the view draws the radar cube"
    );
    assert_eq!(
        scene.meshes[0].model.w_axis.x, 0.0,
        "the main frame the world one"
    );
}

#[test]
fn thirty_two_names_are_l2375() {
    let mut entities = vec![camera(None)];
    entities
        .extend((0..32).map(|i| cube(&format!("c{i}"), Some(&[format!("l{i}").as_str()]), None)));
    let mut engine = engine_for(&document(&entities));
    engine.advance_live();
    let error = engine.extract_present_scene().expect_err("too many layers");
    assert_eq!(error.code.as_str(), "L2375");
    // 31 names are the most one frame has bits for.
    let mut entities = vec![camera(None)];
    entities
        .extend((0..31).map(|i| cube(&format!("c{i}"), Some(&[format!("l{i}").as_str()]), None)));
    let scene = extract(&mut engine_for(&document(&entities)));
    assert_eq!(scene.layered.as_deref().unwrap().scene.meshes.len(), 31);
}

#[test]
fn the_editor_view_override_draws_every_layer() {
    let mut engine = engine_for(&radar_world());
    engine.set_view_override(Some(ViewPose {
        translation: [0.0, 4.0, 10.0],
        rotation: [-10.0, 0.0, 0.0],
        fov_y_deg: None,
    }));
    let scene = extract(&mut engine);
    assert_eq!(scene.meshes.len(), 2);
    assert_eq!(scene.ground_markers.len(), 2);
    assert!(scene.layered.is_some(), "the views still filter from it");
}

#[test]
fn blend_lists_stay_parallel_to_the_filtered_lists() {
    let mut engine = engine_for(&radar_world());
    engine.advance_live();
    let (scene, blend) = engine.extract_present_scene().expect("extracts");
    assert_eq!(scene.meshes.len(), 1);
    assert_eq!(blend.meshes.len(), scene.meshes.len());
    assert_eq!(blend.ground_markers.len(), scene.ground_markers.len());
    assert_eq!(blend.sprites.len(), scene.sprites.len());
    assert_eq!(blend.texts.len(), scene.texts.len());
    assert_eq!(blend.decals.len(), scene.decals.len());
    assert_eq!(blend.drop_shadows.len(), scene.drop_shadows.len());
    // The kept marker is the world one, whichever entity the full list held first.
    let kept = blend.ground_markers[0].expect("keyed");
    let full = scene.layered.as_deref().unwrap();
    assert_eq!(full.scene.ground_markers.len(), 2);
    assert_eq!(
        engine
            .world()
            .get::<ludion_engine::Name>(kept)
            .unwrap()
            .unwrap()
            .0,
        "world-marker"
    );
}

fn check(document: &str) -> Vec<(String, Option<String>)> {
    match ludion_scene::check(document, &builtin_registry()) {
        Ok(_) => Vec::new(),
        Err(diagnostics) => diagnostics
            .into_iter()
            .map(|d| (d.code.as_str().to_string(), d.path))
            .collect(),
    }
}

#[test]
fn check_scene_reports_l2375_at_its_pointer() {
    let empty = document(&[cube("a", Some(&[]), None)]);
    assert_eq!(
        check(&empty),
        [(
            "L2375".to_string(),
            Some("/entities/0/components/RenderLayers/layers".to_string())
        )]
    );
    let duplicate = document(&[camera(Some(&["a", "b", "a"]))]);
    assert_eq!(
        check(&duplicate),
        [(
            "L2375".to_string(),
            Some("/entities/0/components/Camera/layers/2".to_string())
        )]
    );
}
