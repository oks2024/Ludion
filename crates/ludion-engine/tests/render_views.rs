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

/// A radar view entity with `extra` lines (indented 8) added to its `RenderView`.
fn look_view(name: &str, extra: &str) -> String {
    format!(
        "  - name: {name}-cam
    presentation: true
    components:
      Camera: {{}}
      RenderView:
        name: {name}
{extra}"
    )
}

/// A presentation entity carrying a `PostProfile` and a `PostProcess` with `extra` lines (indented 8).
fn profile_entity(name: &str, extra: &str) -> String {
    format!(
        "  - name: {name}-look
    presentation: true
    components:
      PostProfile:
        name: {name}
      PostProcess:
{extra}"
    )
}

#[test]
fn a_view_takes_the_background_shading_and_fog_it_authors() {
    let view = look_view(
        "radar",
        "        background:\n          color:\n            - 0.0\n            - 0.5\n            - 0.25\n        shading: unlit\n        fog: false\n",
    );
    let mut engine = engine_for(&document(&[view, look_view("plain", "")]));
    let scene = extract(&mut engine);
    let [radar, plain] = &scene.views[..] else {
        panic!("two views");
    };
    assert_eq!(radar.background, Some([0.0, 0.5, 0.25]));
    assert!(radar.unlit && !radar.fog);
    assert_eq!(radar.post, ludion_renderer::ViewPost::Scene);
    assert_eq!(plain.background, None);
    assert!(!plain.unlit && plain.fog);
}

#[test]
fn a_profile_resolves_to_its_grade_and_leaves_the_scene_post_alone() {
    let scene_post = "  - name: world\n    components:\n      PostProcess:\n        contrast: 1.25\n        fog_density: 0.05\n";
    let mut engine = engine_for(&document(&[
        look_view("radar", "        post: feed\n"),
        profile_entity("feed", "        contrast: 2.0\n        tonemap: none\n"),
        scene_post.to_string(),
    ]));
    let scene = extract(&mut engine);
    let post = scene.post.expect("the scene post is the unprofiled one");
    assert_eq!((post.contrast, post.fog_density), (1.25, 0.05));
    let ludion_renderer::ViewPost::Profile { post, lut } = &scene.views[0].post else {
        panic!("a profile: {:?}", scene.views[0].post);
    };
    assert_eq!(post.contrast, 2.0);
    assert_eq!(post.tonemap, ludion_renderer::TonemapOperator::None);
    assert!(lut.is_none());
}

#[test]
fn a_scene_whose_only_post_process_is_a_profile_has_no_scene_post() {
    let mut engine = engine_for(&document(&[
        look_view("radar", "        post: feed\n"),
        profile_entity("feed", "        contrast: 2.0\n"),
    ]));
    let scene = extract(&mut engine);
    assert!(scene.post.is_none() && scene.lut.is_none());
    assert!(matches!(
        scene.views[0].post,
        ludion_renderer::ViewPost::Profile { .. }
    ));
}

#[test]
fn post_none_is_passthrough() {
    let mut engine = engine_for(&document(&[look_view("radar", "        post: none\n")]));
    let scene = extract(&mut engine);
    assert_eq!(scene.views[0].post, ludion_renderer::ViewPost::Passthrough);
}

#[test]
fn an_unknown_profile_is_l2378_at_the_view() {
    let mut engine = engine_for(&document(&[
        look_view("radar", "        post: fede\n"),
        profile_entity("feed", "        contrast: 2.0\n"),
    ]));
    engine.advance_live();
    let error = engine
        .extract_present_scene()
        .expect_err("an unknown profile");
    assert_eq!(error.code.as_str(), "L2378");
    assert_eq!(
        error.path.as_deref(),
        Some("/entities/2/components/RenderView/post")
    );
    assert!(error.help.as_deref().unwrap_or("").contains("`feed`"));
}

#[test]
fn check_scene_reports_each_post_profile_mistake_at_its_pointer() {
    let orphan =
        "  - name: orphan\n    components:\n      PostProfile:\n        name: lone\n".to_string();
    assert_eq!(
        static_codes(&document(&[orphan])),
        [(
            "L2378".to_string(),
            Some("/entities/2/components/PostProfile".to_string())
        )]
    );
    let found = static_codes(&document(&[
        profile_entity("feed", "        contrast: 2.0\n"),
        profile_entity("feed", "        contrast: 3.0\n"),
    ]));
    assert_eq!(
        found,
        [(
            "L2378".to_string(),
            Some("/entities/3/components/PostProfile/name".to_string())
        )]
    );
    let found = static_codes(&document(&[
        look_view("radar", "        post: fede\n"),
        profile_entity("feed", "        contrast: 2.0\n"),
    ]));
    assert_eq!(
        found,
        [(
            "L2378".to_string(),
            Some("/entities/2/components/RenderView/post".to_string())
        )]
    );
}

#[test]
fn a_profile_a_view_names_warns_for_every_field_the_view_ignores() {
    let profile = profile_entity(
        "feed",
        "        ssao: 1.0\n        ssr: 0.5\n        dof: 0.2\n        shafts: 1.0\n        fog_density: 0.1\n        contrast: 2.0\n",
    );
    let found = static_codes(&document(&[
        look_view("radar", "        post: feed\n"),
        profile.clone(),
    ]));
    let fields: Vec<_> = found
        .iter()
        .map(|(code, path)| (code.as_str(), path.clone().unwrap()))
        .collect();
    assert_eq!(fields.len(), 5);
    assert!(
        fields.iter().all(|(code, path)| *code == "L2377"
            && path.starts_with("/entities/3/components/PostProcess/"))
    );
    // A profile no view names says nothing.
    assert!(static_codes(&document(&[profile])).is_empty());
}

#[test]
fn a_clean_look_scene_has_no_diagnostics() {
    let found = static_codes(&document(&[
        look_view("radar", "        post: feed\n        shading: unlit\n"),
        look_view("flat", "        post: none\n"),
        profile_entity("feed", "        contrast: 2.0\n        tonemap: none\n"),
    ]));
    assert!(found.is_empty(), "{found:?}");
}

// ---- Rendering a view alone (`render_scene_views`, `Engine::render_view_png`, L2379) ----

fn gallery(scene: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gallery")
        .join(scene)
}

/// The offline views render of the radar scene, or `None` (after printing why) with no GPU.
fn radar_views() -> Option<ludion_engine::ViewsOutcome> {
    let result = ludion_engine::render_scene_views(
        &builtin_registry(),
        &gallery("render-view-radar.yaml"),
        ludion_engine::FrameSize::native(320, 180),
        &ludion_engine::QualitySetting::full(),
        &[],
        0,
        None,
        ludion_renderer::AntiAliasing::Off,
    );
    match result {
        Ok(outcome) => Some(outcome),
        Err(errors) if ludion_core::require_gpu::is_no_adapter(&errors) => {
            eprintln!("SKIPPED: no GPU adapter available");
            None
        }
        Err(errors) => panic!("views render failed: {errors:?}"),
    }
}

#[test]
fn needs_gpu_render_scene_views_returns_the_view_at_its_size_and_the_frame_unchanged() {
    let Some(outcome) = radar_views() else {
        return;
    };
    assert_eq!(outcome.views.len(), 1);
    let (name, image) = &outcome.views[0];
    assert_eq!(name, "radar");
    assert_eq!((image.width, image.height), (256, 256));
    assert_eq!(outcome.view("radar").unwrap(), image);

    let frame = ludion_engine::render_scene_with_registry(
        &builtin_registry(),
        &gallery("render-view-radar.yaml"),
        ludion_engine::FrameSize::native(320, 180),
        &ludion_engine::QualitySetting::full(),
        false,
        &[],
        0,
        &ludion_engine::DebugCategories::None,
        None,
        ludion_renderer::AntiAliasing::Off,
        None,
    )
    .expect("the frame renders");
    assert_eq!(outcome.frame, frame.image, "the frame half is untouched");
}

#[test]
fn needs_gpu_render_view_png_through_an_engine_equals_the_offline_view() {
    let Some(outcome) = radar_views() else {
        return;
    };
    let mut engine = Engine::builder()
        .load_scene_file(&gallery("render-view-radar.yaml"))
        .expect("the scene loads");
    engine.set_quality(ludion_engine::QualitySetting::full());
    let png = match engine.render_view_png("radar") {
        Ok(png) => png,
        Err(error) if error.code.as_str() == "L2003" => return,
        Err(error) => panic!("{error:?}"),
    };
    let offline = outcome.view("radar").unwrap().encode_png().unwrap();
    assert_eq!(png, offline);
}

#[test]
fn an_unknown_view_is_l2379_with_the_rendered_names_and_the_nearest_first() {
    let mut engine = engine_for(&document(&[view_entity("radar", 0, true)]));
    engine.advance_live();
    let error = engine.render_view_png("radr").expect_err("no such view");
    assert_eq!(error.code.as_str(), "L2379");
    let help = error.help.expect("help");
    assert!(help.contains("did you mean `radar`?"), "{help}");
    assert!(help.contains("rendered views: radar"), "{help}");
}

#[test]
fn a_disabled_view_and_a_fifth_view_are_l2379() {
    let mut entities: Vec<String> = ["a", "b", "c", "d", "e"]
        .iter()
        .enumerate()
        .map(|(order, name)| view_entity(name, order as i32, true))
        .collect();
    entities.push(view_entity("off", 9, false));
    let mut engine = engine_for(&document(&entities));
    engine.advance_live();
    for name in ["off", "e"] {
        let error = engine.render_view_png(name).expect_err(name);
        assert_eq!(error.code.as_str(), "L2379", "{name}");
        assert!(error.help.unwrap().contains("rendered views: a, b, c, d"));
    }
}

#[test]
fn a_scene_with_no_view_says_so() {
    let mut engine = engine_for(&document(&[]));
    engine.advance_live();
    let error = engine.render_view_png("radar").expect_err("no views");
    assert_eq!(error.code.as_str(), "L2379");
    assert!(error.help.unwrap().contains("renders no view"));
}

#[test]
fn the_views_sheet_is_the_frame_and_the_views_with_gutters() {
    let cell = |width: u32, height: u32| ludion_renderer::RenderedImage {
        width,
        height,
        rgba: vec![200; (width * height * 4) as usize],
    };
    let sheet = ludion_engine::views_sheet(&cell(320, 180), &[cell(256, 256), cell(64, 32)]);
    assert_eq!((sheet.width, sheet.height), (320 + 256 + 64 + 16, 256));
}

#[test]
fn needs_gpu_profile_scene_times_each_view_after_the_main_passes() {
    let profile = |scene: &str| {
        ludion_engine::profile_scene(
            &gallery(scene),
            3,
            0,
            None,
            None,
            ludion_engine::QualitySetting::full(),
        )
        .expect("the scene profiles")
    };
    let report = profile("render-view-radar.yaml");
    if !report.gpu.available {
        eprintln!("SKIPPED: no GPU adapter available");
        return;
    }
    let names: Vec<&str> = report.gpu.passes.iter().map(|p| p.name.as_str()).collect();
    let span = names
        .iter()
        .position(|n| *n == "view:radar")
        .expect("a span row");
    let first_view_pass = names
        .iter()
        .position(|n| *n == "view:radar/scene")
        .expect("a pass row");
    let last_main = names.iter().rposition(|n| !n.starts_with("view:")).unwrap();
    assert!(last_main < span && span < first_view_pass, "{names:?}");

    let plain = profile("meshes.yaml");
    assert!(
        plain
            .gpu
            .passes
            .iter()
            .all(|p| !p.name.starts_with("view:"))
    );
}
