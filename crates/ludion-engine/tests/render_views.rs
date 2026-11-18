//! Secondary views (`RenderView`, `UiRect.view`): which camera is the main one, which views render,
//! and what a rect naming a view draws. None needs a GPU: the acceptance is the extracted `RenderScene`.

use ludion_core::codes::code;
use ludion_engine::{Diagnostics, Engine, builtin_registry, scene_render_view_diagnostics};
use ludion_renderer::{RenderScene, UiInstance};

const MAIN: &str = "  - name: cam
    components:
      Transform:
        translation:
          - 0.0
          - 1.0
          - 8.0
      Camera: {}
  - name: crate
    components:
      Transform: {}
      Mesh:
        shape: cube
";

fn view_entity(name: &str, order: i32, enabled: bool) -> String {
    format!(
        "  - name: {name}-cam
    presentation: true
    components:
      Transform:
        translation:
          - 0.0
          - 9.0
          - 0.0
      Camera: {{}}
      RenderView:
        name: {name}
        size:
          - 64
          - 32
        order: {order}
        enabled: {enabled}
"
    )
}

fn rect_entity(view: &str) -> String {
    format!("  - name: show-{view}\n    components:\n      UiRect:\n        view: {view}\n")
}

fn document(entities: &[String]) -> String {
    format!("format: 1\nseed: 3\nentities:\n{MAIN}{}", entities.concat())
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

fn view_rects(scene: &RenderScene) -> Vec<u32> {
    scene
        .ui
        .iter()
        .filter_map(|row| match row {
            UiInstance::Rect { view, .. } => *view,
            _ => None,
        })
        .collect()
}

fn reported(engine: &Engine, code: ludion_core::diagnostics::DiagnosticCode) -> usize {
    engine
        .world()
        .resource::<Diagnostics>()
        .map_or(0, |d| d.reported(code).count())
}

#[test]
fn a_camera_beside_a_render_view_is_not_the_main_camera() {
    let plain = extract(&mut engine_for(&document(&[])));
    let mut engine = engine_for(&document(&[view_entity("radar", 0, true)]));
    let scene = extract(&mut engine);
    assert_eq!(
        scene.camera, plain.camera,
        "the plain camera is the main one"
    );
    assert_eq!(scene.views.len(), 1);
    assert_eq!(scene.views[0].name, "radar");
    assert_eq!(scene.views[0].size, (64, 32));
    assert_ne!(scene.views[0].camera, scene.camera);
    assert!(plain.views.is_empty());
}

#[test]
fn two_plain_cameras_are_still_l2101() {
    let second = "  - name: other\n    components:\n      Camera: {}\n".to_string();
    let mut engine = engine_for(&document(&[second]));
    engine.advance_live();
    let error = engine
        .extract_present_scene()
        .expect_err("two main cameras");
    assert_eq!(error.code.as_str(), "L2101");
}

#[test]
fn a_view_rect_draws_the_view_and_a_disabled_one_draws_nothing() {
    let mut engine = engine_for(&document(&[
        view_entity("radar", 0, true),
        view_entity("feed", 1, false),
        rect_entity("radar"),
        rect_entity("feed"),
    ]));
    let scene = extract(&mut engine);
    assert_eq!(scene.views.len(), 1, "the disabled view is absent");
    assert_eq!(
        view_rects(&scene),
        [0],
        "only the enabled view's rect draws"
    );
    assert_eq!(
        scene.ui.len(),
        1,
        "the disabled view's rect is skipped, not drawn plain"
    );
    let UiInstance::Rect { texture, .. } = &scene.ui[0] else {
        panic!("a rect");
    };
    assert_eq!(*texture, None);
}

#[test]
fn views_are_ordered_and_capped_at_four() {
    let names = ["e", "d", "c", "b", "a"];
    let mut entities: Vec<String> = names
        .iter()
        .enumerate()
        .map(|(i, name)| view_entity(name, 10 - i as i32, true))
        .collect();
    entities.extend(names.iter().map(|name| rect_entity(name)));
    let mut engine = engine_for(&document(&entities));
    let scene = extract(&mut engine);
    let order: Vec<&str> = scene.views.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(
        order,
        ["a", "b", "c", "d"],
        "lowest order first, the fifth dropped"
    );
    assert_eq!(
        view_rects(&scene).len(),
        4,
        "the fifth view's rect is skipped"
    );
    assert_eq!(reported(&engine, code::RENDER_VIEW_BUDGET), 1);
}

#[test]
fn equal_orders_fall_back_to_the_entity() {
    let mut engine = engine_for(&document(&[
        view_entity("first", 0, true),
        view_entity("second", 0, true),
    ]));
    let scene = extract(&mut engine);
    let order: Vec<&str> = scene.views.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(order, ["first", "second"]);
}

#[test]
fn an_unknown_view_name_is_l2372_at_extract() {
    let mut engine = engine_for(&document(&[
        view_entity("radar", 0, true),
        rect_entity("rada"),
    ]));
    engine.advance_live();
    let error = engine
        .extract_present_scene()
        .expect_err("an unresolved view");
    assert_eq!(error.code.as_str(), "L2372");
}

fn static_codes(document: &str) -> Vec<(String, Option<String>)> {
    let scene = ludion_scene::check(document, &builtin_registry()).expect("the scene checks");
    scene_render_view_diagnostics(&scene)
        .into_iter()
        .map(|d| (d.code.as_str().to_string(), d.path))
        .collect()
}

#[test]
fn check_scene_reports_each_mistake_at_its_pointer() {
    let no_camera = "  - name: lone\n    components:\n      RenderView:\n        name: lone\n";
    assert_eq!(
        static_codes(&document(&[no_camera.to_string()])),
        [(
            "L2370".to_string(),
            Some("/entities/2/components/RenderView".to_string())
        )]
    );
    let found = static_codes(&document(&[
        view_entity("radar", 0, true),
        view_entity("radar", 1, true),
    ]));
    assert_eq!(
        found,
        [(
            "L2371".to_string(),
            Some("/entities/3/components/RenderView/name".to_string())
        )]
    );
    let found = static_codes(&document(&[rect_entity("nowhere")]));
    assert_eq!(
        found,
        [(
            "L2372".to_string(),
            Some("/entities/2/components/UiRect/view".to_string())
        )]
    );
}

#[test]
fn a_view_beside_a_texture_source_is_l2373() {
    let text = format!(
        "{}  - name: bad\n    components:\n      UiRect:\n        view: radar\n        texture: a.png\n",
        document(&[view_entity("radar", 0, true)])
    );
    let errors = ludion_scene::check(&text, &builtin_registry()).expect_err("a conflict");
    let found: Vec<_> = errors
        .iter()
        .filter(|d| d.code.as_str() == "L2373")
        .map(|d| d.path.as_deref())
        .collect();
    assert_eq!(found, [Some("/entities/3/components/UiRect/view")]);
}

#[test]
fn a_presentation_view_does_not_move_the_world_hash() {
    let mut bare = engine_for(&document(&[]));
    let mut viewed = engine_for(&document(&[view_entity("radar", 0, true)]));
    assert_eq!(bare.world_hash(), viewed.world_hash());
}

#[test]
fn a_hidden_view_rect_leaves_the_entitys_text_drawn() {
    let both = "  - name: both\n    components:\n      UiRect:\n        view: feed\n      UiText:\n        text: hi\n";
    let mut engine = engine_for(&document(&[
        view_entity("feed", 0, false),
        both.to_string(),
    ]));
    let scene = extract(&mut engine);
    assert!(view_rects(&scene).is_empty());
    assert!(
        scene
            .ui
            .iter()
            .any(|row| matches!(row, UiInstance::Text { .. })),
        "the text still draws"
    );
    assert!(
        !scene
            .ui
            .iter()
            .any(|row| matches!(row, UiInstance::Rect { .. })),
        "the hidden view's rect does not"
    );
}
