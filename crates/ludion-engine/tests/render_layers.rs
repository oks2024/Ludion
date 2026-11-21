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

fn l2376(document: &str) -> Vec<(Option<String>, Option<String>)> {
    let scene = ludion_scene::check(document, &builtin_registry()).expect("the scene checks");
    ludion_engine::scene_render_layer_diagnostics(&scene)
        .into_iter()
        .map(|d| {
            assert_eq!(d.code.as_str(), "L2376");
            (d.path, d.help)
        })
        .collect()
}

fn view_with_lights(layers: Layers, lights: Layers) -> String {
    let lights = lights.map_or(String::new(), |names| {
        names
            .iter()
            .fold("\n        lights:".to_string(), |out, n| {
                format!("{out}\n          - {n}")
            })
    });
    view_entity(layers).replace(
        "size:\n          - 64\n          - 32",
        &format!("size:\n          - 64\n          - 32{lights}"),
    )
}

#[test]
fn l2376_names_each_undeclared_field_with_a_nearest_match() {
    let cam = camera(Some(&["default", "radr"]));
    let cam = format!("{cam}        lights:\n          - radr\n");
    let doc = format!(
        "format: 1\nrender_layers:\n  - name: radar\nentities:\n{cam}{}{}",
        cube("a", Some(&["radr"]), None),
        view_with_lights(Some(&["radr"]), Some(&["zzz"])),
    );
    let found = l2376(&doc);
    let pointers: Vec<&str> = found.iter().map(|(p, _)| p.as_deref().unwrap()).collect();
    assert_eq!(
        pointers,
        [
            "/entities/0/components/Camera/layers/1",
            "/entities/0/components/Camera/lights/0",
            "/entities/1/components/RenderLayers/layers/0",
            "/entities/2/components/RenderView/layers/0",
            "/entities/2/components/RenderView/lights/0",
        ]
    );
    assert_eq!(found[0].1.as_deref(), Some("nearest match is `radar`"));
    assert_eq!(
        found[4].1.as_deref(),
        Some("declare it under `render_layers:`")
    );
}

#[test]
fn declared_names_and_no_names_are_clean() {
    let declared = format!(
        "format: 1\nrender_layers:\n  - name: radar\nentities:\n{}{}",
        camera(Some(&["default", "radar"])),
        cube("a", Some(&["radar"]), None)
    );
    assert!(l2376(&declared).is_empty());
    assert!(l2376(&document(&[camera(None), cube("a", None, None)])).is_empty());
}

#[test]
fn scene_save_keeps_the_render_layers_block() {
    use ludion_protocol::command::SceneSaveParams;
    use ludion_protocol::{Command, ResponseBody};
    let root =
        std::env::temp_dir().join(format!("ludion-render-layers-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut engine = Engine::with_project_root(&root);
    engine
        .load_inline_scene(&format!(
            "format: 1\nseed: 3\nrender_layers:\n  - name: radar\n  - name: world\nentities:\n{}",
            cube("a", Some(&["radar"]), None)
        ))
        .unwrap();
    let body = engine
        .execute(Command::SceneSave(SceneSaveParams {
            path: "out.yaml".into(),
            force: false,
        }))
        .unwrap()
        .body;
    assert!(matches!(body, ResponseBody::SceneSaved { .. }), "{body:?}");
    let saved = std::fs::read_to_string(root.join("out.yaml")).unwrap();
    assert!(
        saved.contains("render_layers:\n  - name: radar\n  - name: world\n"),
        "{saved}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A `PointLight` at x on `layers`, optionally a child of entity `parent`; `SpotLight` when `spot`.
fn light(name: &str, x: f32, layers: Layers, parent: Option<usize>, spot: bool) -> String {
    let layers = render_layers(layers);
    let parent = parent.map_or(String::new(), |p| {
        format!("      Parent:\n        index: {p}\n        generation: 0\n")
    });
    let kind = if spot { "SpotLight" } else { "PointLight" };
    format!(
        "  - name: {name}
    components:
      Transform:
        translation:
          - {x}
          - 2.0
          - 0.0
      {kind}: {{}}
{layers}{parent}"
    )
}

fn camera_with_lights(layers: Layers, lights: Layers) -> String {
    let lights = lights.map_or(String::new(), |names| {
        names.iter().fold("        lights:".to_string(), |out, n| {
            format!("{out}\n          - {n}")
        }) + "\n"
    });
    format!("{}{lights}", camera(layers))
}

fn lights_world(cam: String) -> String {
    document(&[
        cam,
        light("key", 1.0, Some(&["portrait"]), None, false),
        light("lamp", 2.0, None, None, false),
        light("spot-key", 3.0, Some(&["portrait"]), None, true),
        light("spot-lamp", 4.0, None, None, true),
        cube("figure", Some(&["portrait"]), None),
    ])
}

fn xs(scene: &RenderScene) -> (Vec<f32>, Vec<f32>) {
    (
        scene.lights.iter().map(|l| l.position.x).collect(),
        scene.spot_lights.iter().map(|l| l.position.x).collect(),
    )
}

#[test]
fn a_portrait_light_stays_out_of_a_main_camera_on_default() {
    let scene = extract(&mut engine_for(&lights_world(camera(Some(&["default"])))));
    assert_eq!(xs(&scene), (vec![2.0], vec![4.0]));
    let source = scene.layered.as_deref().expect("layered");
    assert_eq!(
        source.scene.lights.len(),
        2,
        "the full set rides on the source"
    );
    assert_eq!(source.scene.spot_lights.len(), 2);
}

#[test]
fn camera_lights_choose_the_lights_apart_from_the_layers() {
    let both = camera_with_lights(Some(&["default"]), Some(&["default", "portrait"]));
    let scene = extract(&mut engine_for(&lights_world(both)));
    assert_eq!(xs(&scene), (vec![1.0, 2.0], vec![3.0, 4.0]));
    assert_eq!(scene.meshes.len(), 0, "the figure is still not drawn");
    let only = camera_with_lights(Some(&["default"]), Some(&["portrait"]));
    let scene = extract(&mut engine_for(&lights_world(only)));
    assert_eq!(xs(&scene), (vec![1.0], vec![3.0]));
}

#[test]
fn an_unlayered_camera_takes_every_light() {
    let scene = extract(&mut engine_for(&lights_world(camera(None))));
    assert_eq!(xs(&scene), (vec![1.0, 2.0], vec![3.0, 4.0]));
}

#[test]
fn a_child_light_inherits_its_parents_layer() {
    // 0 camera; 1 rig on `portrait`; 2 child light with no layers.
    let world = document(&[
        camera(Some(&["default"])),
        cube("rig", Some(&["portrait"]), None),
        light("child", 1.0, None, Some(1), false),
        light("own", 2.0, Some(&["default"]), Some(1), false),
    ]);
    let scene = extract(&mut engine_for(&world));
    assert_eq!(xs(&scene).0, vec![2.0]);
}

#[test]
fn a_view_lights_by_its_own_list_and_defaults_to_its_layers() {
    let view = |lights: &str| {
        view_entity(Some(&["portrait"])).replace(
            "layers:\n          - portrait",
            &format!("layers:\n          - portrait{lights}"),
        )
    };
    let world = |view: String| {
        document(&[
            camera(Some(&["default"])),
            view,
            light("key", 1.0, Some(&["portrait"]), None, false),
            light("lamp", 2.0, None, None, false),
        ])
    };
    let scene = extract(&mut engine_for(&world(view(""))));
    assert_eq!(
        view_scene(&scene, 0).lights.len(),
        1,
        "lights default to layers"
    );
    assert_eq!(view_scene(&scene, 0).lights[0].position.x, 1.0);
    let with = view("\n        lights:\n          - default\n          - portrait");
    let scene = extract(&mut engine_for(&world(with)));
    assert_eq!(view_scene(&scene, 0).lights.len(), 2);
}

#[test]
fn view_sun_false_drops_the_sun_even_in_an_unlayered_frame() {
    let sun = "  - name: sun
    components:
      Transform: {}
      DirectionalLight: {}
";
    let world = |view: String| document(&[camera(None), view, sun.to_string()]);
    let off = view_entity(None).replace("size:", "sun: false\n        size:");
    let scene = extract(&mut engine_for(&world(off)));
    assert!(scene.layered.is_none());
    assert!(scene.sun.is_some());
    assert!(!scene.views[0].sun);
    assert!(view_scene(&scene, 0).sun.is_none());
    let scene = extract(&mut engine_for(&world(view_entity(None))));
    assert!(scene.views[0].sun && view_scene(&scene, 0).sun.is_some());
}

#[test]
fn blend_light_lists_follow_the_filtered_lights() {
    let mut engine = engine_for(&lights_world(camera(Some(&["default"]))));
    engine.advance_live();
    let (scene, blend) = engine.extract_present_scene().expect("extracts");
    assert_eq!(blend.lights.len(), scene.lights.len());
    assert_eq!(blend.spot_lights.len(), scene.spot_lights.len());
    assert_eq!(blend.lights.len(), 1);
}
