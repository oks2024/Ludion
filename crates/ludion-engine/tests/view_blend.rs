//! Presenter interpolation of secondary views: `BlendData.views` keys each view camera, a layered
//! frame's `BlendData.layered` keys its unfiltered source, and `blend` moves both with the main frame
//! (`Plan/secondary-views.md` D34-D36). None needs a GPU: the acceptance is the extracted data.

use ludion_core::ids::EntityId;
use ludion_ecs::World;
use ludion_engine::{Engine, Name, NoInterpolate, Transform};
use ludion_renderer::{BlendData, RenderScene, blend};

fn gallery_engine(file: &str) -> Engine {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gallery")
        .join(file);
    let document = std::fs::read_to_string(path).expect("the gallery scene reads");
    let mut engine = Engine::builder().build().expect("the game builds");
    engine
        .load_inline_scene(&document)
        .expect("the scene loads");
    engine
}

fn extract(engine: &mut Engine) -> (RenderScene, BlendData) {
    engine.advance_live();
    engine.extract_present_scene().expect("extracts")
}

fn named(world: &World, wanted: &str) -> EntityId {
    world
        .entities()
        .find(|&id| {
            world
                .get::<Name>(id)
                .ok()
                .flatten()
                .is_some_and(|name| name.0 == wanted)
        })
        .unwrap_or_else(|| panic!("the scene has a `{wanted}`"))
}

fn move_x(engine: &mut Engine, entity: &str, dx: f32) {
    let id = named(engine.world(), entity);
    let world = engine.world_mut();
    let transform = world.get_mut::<Transform>(id).unwrap().unwrap();
    transform.translation[0] += dx;
}

#[test]
fn a_view_camera_is_keyed_and_matches_the_drawn_camera() {
    let mut engine = gallery_engine("render-view-radar.yaml");
    let (scene, data) = extract(&mut engine);
    assert_eq!(scene.views.len(), 1);
    assert_eq!(data.views.len(), scene.views.len());
    let radar = named(engine.world(), "radar-cam");
    let camera = data.views[0].expect("the radar camera is keyed");
    assert_eq!(camera.key, radar);
    assert!(
        ludion_core::math::mat4_inverse(camera.model).abs_diff_eq(scene.views[0].camera.view, 1e-5),
        "the keyed model is the drawn view camera's pose"
    );
    assert_eq!(
        blend(&scene, &data, &scene, &data, 1.0),
        scene,
        "alpha 1 is the newer tick"
    );
}

#[test]
fn a_moved_view_camera_lands_halfway() {
    let mut engine = gallery_engine("render-view-radar.yaml");
    let (prev, prev_data) = extract(&mut engine);
    move_x(&mut engine, "radar-cam", 2.0);
    let (next, next_data) = extract(&mut engine);
    assert_ne!(prev.views[0].camera, next.views[0].camera);
    let out = blend(&prev, &prev_data, &next, &next_data, 0.5);
    let (a, b) = (prev.views[0].camera.position, next.views[0].camera.position);
    assert!(
        out.views[0]
            .camera
            .position
            .abs_diff_eq((a + b) * 0.5, 1e-5),
        "{:?} is not halfway between {a:?} and {b:?}",
        out.views[0].camera.position
    );
    assert_eq!(out.camera, next.camera, "the main camera did not move");
}

#[test]
fn no_interpolate_cuts_a_view_camera() {
    let mut engine = gallery_engine("render-view-radar.yaml");
    let (prev, prev_data) = extract(&mut engine);
    let radar = named(engine.world(), "radar-cam");
    // The marker lives for one tick: a tick clears the last one's before it runs.
    engine.advance_live();
    engine.world_mut().insert(radar, NoInterpolate).unwrap();
    move_x(&mut engine, "radar-cam", 2.0);
    let (next, next_data) = engine.extract_present_scene().unwrap();
    assert!(next_data.snaps.contains(&radar));
    let out = blend(&prev, &prev_data, &next, &next_data, 0.5);
    assert_eq!(out.views[0].camera, next.views[0].camera);
}

#[test]
fn a_layered_frame_keys_its_source_lists() {
    let mut engine = gallery_engine("render-view-layers.yaml");
    let (scene, data) = extract(&mut engine);
    let source = scene.layered.as_ref().expect("the frame is layered");
    let keys = data.layered.as_ref().expect("the source is keyed");
    let scene_of = &source.scene;
    assert_eq!(keys.meshes.len(), scene_of.meshes.len());
    assert_eq!(keys.ground_markers.len(), scene_of.ground_markers.len());
    assert_eq!(keys.sprites.len(), scene_of.sprites.len());
    assert!(
        scene_of.ground_markers.len() > scene.ground_markers.len(),
        "the radar-only marker is on the source only"
    );
    let marker = named(engine.world(), "radar-only-marker");
    assert!(keys.ground_markers.contains(&Some(marker)));
    assert!(
        data.poses
            .binary_search_by_key(&marker, |(id, _)| *id)
            .is_ok(),
        "a view-only layer's item is in the pose table"
    );
    assert!(!data.ground_markers.contains(&Some(marker)));
}

#[test]
fn a_view_only_layer_item_lands_halfway_in_the_source() {
    let mut engine = gallery_engine("render-view-layers.yaml");
    let marker = named(engine.world(), "radar-only-marker");
    let (prev, prev_data) = extract(&mut engine);
    move_x(&mut engine, "radar-only-marker", 2.0);
    let (next, next_data) = extract(&mut engine);
    let at = |scene: &RenderScene| {
        let source = scene.layered.as_ref().unwrap();
        let keys = next_data.layered.as_ref().unwrap();
        let index = keys
            .ground_markers
            .iter()
            .position(|k| *k == Some(marker))
            .unwrap();
        source.scene.ground_markers[index].model.w_axis.x
    };
    let out = blend(&prev, &prev_data, &next, &next_data, 0.5);
    let (a, b) = (at(&prev), at(&next));
    assert!((b - a - 2.0).abs() < 1e-5, "the marker moved 2 m");
    assert!((at(&out) - (a + b) * 0.5).abs() < 1e-5, "halfway");
    assert_eq!(
        out.ground_markers, next.ground_markers,
        "the main list is untouched"
    );
}
