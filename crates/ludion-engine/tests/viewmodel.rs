//! The first-person viewmodel packet: a flagged mesh extracts as a camera-composed item with its own
//! lens, no shadow and no blend key, and a world without the flag is untouched. Every case extracts
//! through `Engine::extract_present_scene` and none needs a GPU.

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::math::glam::Mat4;
use ludion_ecs::World;
use ludion_engine::{Diagnostics, Engine, Name, dispatch};
use ludion_protocol::command::{RenderViewParams, ViewPose};
use ludion_protocol::{Command, ResponseBody};
use ludion_renderer::{BlendData, RenderScene, latch_camera};

const RIG: &str = "      Transform: {}
      Camera: {}
      CameraRig:
        target:
          index: 1
          generation: 0
        rig:
          mode:
            first_person: {}
";

const WORLD: &str = "  - name: hero
    components:
      Transform:
        translation:
          - 1.0
          - 0.0
          - -2.0
  - name: wall
    components:
      Transform:
        translation:
          - 3.0
          - 0.0
          - 0.0
      Mesh:
        shape: cube
";

const GUN: &str = "  - name: gun
    presentation: true
    components:
      Parent:
        index: 0
        generation: 0
      Transform: {}
      Mesh:
        shape: cube
";

fn scene(camera_extra: &str, flag: Option<&str>, extra: &str) -> String {
    let flag = flag.map_or(String::new(), |f| format!("      {f}\n"));
    format!(
        "format: 1\nseed: 3\nentities:\n  - name: cam\n    components:\n{RIG}{camera_extra}{WORLD}{GUN}{flag}{extra}"
    )
}

fn engine_for(document: &str) -> Engine {
    let mut engine = Engine::builder().build().expect("the game builds");
    engine.load_inline_scene(document).expect("the scene loads");
    engine
}

fn extract_both(engine: &mut Engine) -> (RenderScene, BlendData) {
    engine.advance_live();
    engine.extract_present_scene().expect("extracts")
}

fn extract(engine: &mut Engine) -> RenderScene {
    extract_both(engine).0
}

fn named(world: &World, wanted: &str) -> EntityId {
    world
        .entities()
        .chain(world.presentation_entities())
        .find(|&id| {
            world
                .get::<Name>(id)
                .ok()
                .flatten()
                .is_some_and(|name| name.0 == wanted)
        })
        .unwrap_or_else(|| panic!("the scene has a `{wanted}`"))
}

fn reported(engine: &Engine, code: ludion_core::diagnostics::DiagnosticCode) -> Vec<String> {
    engine
        .world()
        .resource::<Diagnostics>()
        .map(|d| d.reported(code).map(str::to_string).collect())
        .unwrap_or_default()
}

#[allow(clippy::disallowed_methods)] // test-side: compares GPU matrices, never hashed
fn inverse(m: Mat4) -> Mat4 {
    m.inverse()
}

const FLAG: Option<&str> = Some("Viewmodel: {}");

/// The one viewmodel item of a scene extracted with the flag.
fn gun(scene: &RenderScene) -> &ludion_renderer::MeshInstance {
    let local = &scene.viewmodel.as_ref().expect("a viewmodel packet").locals;
    assert_eq!(local.len(), 1);
    &scene.meshes[local[0].mesh as usize]
}

#[test]
fn the_flag_does_not_move_the_world_hash() {
    let mut with = engine_for(&scene("", FLAG, ""));
    let mut without = engine_for(&scene("", None, ""));
    for tick in 0..10 {
        with.advance_live();
        without.advance_live();
        assert_eq!(
            with.world_hash().unwrap(),
            without.world_hash().unwrap(),
            "tick {tick}"
        );
    }
}

#[test]
fn flagged_items_cast_no_shadow_and_leave_the_world_alone() {
    let (flagged, blend) = extract_both(&mut engine_for(&scene("", FLAG, "")));
    let gun_item = gun(&flagged);
    assert!(gun_item.viewmodel && !gun_item.cast_shadows);
    let casters = |scene: &RenderScene, blend: &BlendData| -> Vec<(Option<EntityId>, String)> {
        scene
            .meshes
            .iter()
            .zip(&blend.meshes)
            .filter(|(mesh, _)| mesh.cast_shadows)
            .map(|(mesh, key)| (*key, format!("{:?}", mesh.model)))
            .collect()
    };
    // The scene without the gun entity at all.
    let bare = scene("", None, "").replace(GUN, "");
    let (plain, plain_blend) = extract_both(&mut engine_for(&bare));
    assert!(plain.viewmodel.is_none());
    assert_eq!(casters(&flagged, &blend), casters(&plain, &plain_blend));
    let gun_at = flagged.meshes.iter().position(|m| m.viewmodel).unwrap();
    assert_eq!(
        blend.meshes[gun_at], None,
        "a viewmodel item has no blend key"
    );
}

#[test]
fn the_unlatched_gun_sits_on_the_drawn_camera() {
    let scene = extract(&mut engine_for(&scene("", FLAG, "")));
    let item = gun(&scene);
    assert!(
        item.model.abs_diff_eq(inverse(scene.camera.view), 1e-5),
        "{:?} vs {:?}",
        item.model,
        inverse(scene.camera.view)
    );
    let data = scene.viewmodel.as_ref().unwrap();
    assert_eq!(
        (data.fov_y_deg, data.near, data.depth_slice),
        (55.0, 0.01, 0.01)
    );
    assert!(data.locals[0].model.abs_diff_eq(Mat4::IDENTITY, 1e-6));
}

#[test]
fn the_latched_gun_follows_the_latched_camera() {
    let mut engine = engine_for(&scene("", FLAG, ""));
    for _ in 0..3 {
        engine.advance_live();
    }
    let mut packet = engine.extract_present_scene().expect("extracts").0;
    let before = gun(&packet).model;
    let latch = engine.present_camera_latch().expect("a first-person rig");
    let tick = latch.resolve([0.0; 2]).expect("resolves");
    let latched = latch.resolve([15.0, 4.0]).expect("resolves");
    latch_camera(&mut packet, &tick, &latched);
    let item = gun(&packet).model;
    assert!(item.abs_diff_eq(inverse(packet.camera.view), 1e-5));
    assert_ne!(item, before);
}

#[test]
fn a_camera_offset_moves_the_gun_but_not_the_lens() {
    let base = extract(&mut engine_for(&scene("", FLAG, "")));
    let offset = "  - name: intro
    presentation: true
    components:
      CameraOffset:
        translation:
          - 0.5
          - -0.25
          - 4.0
        fov_delta: -10.0
";
    let moved = extract(&mut engine_for(&scene("", FLAG, offset)));
    assert_ne!(moved.camera.view, base.camera.view);
    assert!(
        gun(&moved)
            .model
            .abs_diff_eq(inverse(moved.camera.view), 1e-5)
    );
    assert_eq!(
        moved.viewmodel.as_ref().unwrap().projection,
        base.viewmodel.as_ref().unwrap().projection
    );
}

#[test]
fn a_track_rig_draws_the_gun_as_a_world_mesh_and_reports_once() {
    let track = scene("", FLAG, "").replace("first_person: {}", "track: {}");
    let mut engine = engine_for(&track);
    let mut last = None;
    for _ in 0..3 {
        last = Some(extract(&mut engine));
    }
    let scene = last.unwrap();
    assert!(scene.viewmodel.is_none());
    assert!(scene.meshes.iter().all(|m| !m.viewmodel && m.cast_shadows));
    assert_eq!(reported(&engine, code::VIEWMODEL_NO_CAMERA).len(), 1);
}

#[test]
fn a_lens_off_a_camera_is_l1337() {
    let hero_lens = scene("", FLAG, "").replace(
        "      Transform:\n        translation:\n          - 1.0\n          - 0.0\n          - -2.0\n",
        "      Transform:\n        translation:\n          - 1.0\n          - 0.0\n          - -2.0\n      ViewmodelLens: {}\n",
    );
    let mut engine = engine_for(&hero_lens);
    extract(&mut engine);
    let hero = named(engine.world(), "hero");
    assert!(engine.world().has::<ludion_engine::ViewmodelLens>(hero));
    assert_eq!(
        reported(&engine, code::VIEWMODEL_LENS_NOT_ON_CAMERA).len(),
        1
    );
}

#[test]
fn the_cameras_lens_reaches_the_packet() {
    let lens = "      ViewmodelLens:\n        fov_y_deg: 40.0\n";
    let packet = extract(&mut engine_for(&scene(lens, FLAG, "")));
    assert_eq!(packet.viewmodel.as_ref().unwrap().fov_y_deg, 40.0);
}

#[test]
fn a_view_override_draws_world_meshes_silently() {
    let mut engine = engine_for(&scene("", FLAG, ""));
    let pose = ViewPose {
        translation: [0.0, 3.0, 8.0],
        rotation: [0.0, 0.0, 0.0],
        fov_y_deg: None,
    };
    match dispatch(
        &mut engine,
        Command::RenderView(RenderViewParams {
            pose: Some(Some(pose)),
        }),
    )
    .unwrap()
    .body
    {
        ResponseBody::RenderView { .. } => {}
        other => panic!("{other:?}"),
    }
    let packet = extract(&mut engine);
    assert!(packet.viewmodel.is_none());
    assert!(packet.meshes.iter().all(|m| !m.viewmodel));
    assert!(reported(&engine, code::VIEWMODEL_NO_CAMERA).is_empty());
}

#[test]
fn the_gi_grid_does_not_see_the_gun() {
    let gi = "  - name: gi
    components:
      GiVolume:
        cell_size: 0.5
        probe_spacing: 2.0
";
    let flagged = extract(&mut engine_for(&scene("", FLAG, gi)));
    let no_gun = extract(&mut engine_for(&format!(
        "format: 1\nseed: 3\nentities:\n  - name: cam\n    components:\n{RIG}{WORLD}{gi}"
    )));
    let grid = no_gun.voxel.as_ref().expect("the world builds a grid");
    assert_eq!(flagged.voxel.as_ref(), Some(grid));
}
