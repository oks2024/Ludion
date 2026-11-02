//! The engine fills the display-rate blend data on handover: the pose table, the per-item keys, the
//! tick's snap set, the load epoch and the camera. Every case drives a small inline scene through
//! `advance_live` and reads `Engine::extract_present_scene` after each tick; none needs a GPU.

use std::path::PathBuf;

use ludion_core::ids::EntityId;
use ludion_core::time::{SimTime, TickRate};
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    CameraOffset, Engine, Mesh, Name, NoInterpolate, QualityParams, Transform, Visibility,
    advance_volleys, extract_at, play_patterns, teleport_character,
};
use ludion_protocol::{Command, ResponseBody};
use ludion_renderer::{BlendData, RenderScene, blend};

type Systems<'a> = &'a [(&'static str, fn(&mut World, &SystemCtx))];

const SCENE: &str = "format: 1
seed: 3
physics:
  space: free
entities:
  - name: box
    components:
      Transform: {}
      Mesh:
        shape: cube
  - name: child
    components:
      Parent:
        index: 0
        generation: 0
      Transform:
        translation:
          - 1.0
          - 0.0
          - 0.0
      Mesh:
        shape: cube
  - name: floor
    components:
      Transform:
        translation:
          - 0.0
          - -0.5
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 60.0
              - 0.5
              - 60.0
        body: static
  - name: hero
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 5.0
      CharacterMotor:
        radius: 0.28
        height: 0.9
        slope_limit_deg: 45.0
        step_height: 0.25
      Mesh:
        shape: cube
";

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

fn engine_with(systems: Systems) -> Engine {
    let mut builder = Engine::builder();
    for &(name, system) in systems {
        builder = builder.system(name, system);
    }
    let mut engine = builder.build().expect("the game builds");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    engine
}

fn nudge(world: &mut World, wanted: &str, dx: f32) {
    let id = named(world, wanted);
    world.get_mut::<Transform>(id).unwrap().unwrap().translation[0] += dx;
}

/// Moves the box +1 m in x every tick.
fn slide(world: &mut World, _ctx: &SystemCtx) {
    nudge(world, "box", 1.0);
}

/// Moves the box +1 m every tick; on frame 1 (`marked`) also marks it `NoInterpolate`, and on frame
/// 2 teleports the hero.
fn session(world: &mut World, ctx: &SystemCtx, marked: bool) {
    nudge(world, "box", 1.0);
    match ctx.frame.as_u64() {
        1 if marked => {
            let id = named(world, "box");
            world.insert(id, NoInterpolate).unwrap();
        }
        2 => {
            let hero = named(world, "hero");
            teleport_character(world, hero, [8.0, 0.0, 5.0], 0.0).unwrap();
        }
        _ => {}
    }
}

fn session_marked(world: &mut World, ctx: &SystemCtx) {
    session(world, ctx, true);
}

fn session_plain(world: &mut World, ctx: &SystemCtx) {
    session(world, ctx, false);
}

/// Marks the box on frame 1 and nothing else; the scene's own entities stay put.
fn mark_box(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 1 {
        let id = named(world, "box");
        nudge(world, "box", 10.0);
        world.insert(id, NoInterpolate).unwrap();
    }
}

fn teleport_hero(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 1 {
        let hero = named(world, "hero");
        teleport_character(world, hero, [8.0, 0.0, 5.0], 0.0).unwrap();
    }
}

fn spawn_late(world: &mut World, ctx: &SystemCtx) {
    nudge(world, "box", 1.0);
    if ctx.frame.as_u64() == 1 {
        let late = world.spawn();
        let at = Transform {
            translation: [10.0, 0.0, 0.0],
            ..Transform::default()
        };
        world.insert(late, at).unwrap();
        let mesh: Mesh = serde_json::from_value(serde_json::json!({ "shape": "cube" })).unwrap();
        world.insert(late, mesh).unwrap();
    }
}

fn tick(engine: &mut Engine) -> (RenderScene, BlendData) {
    engine.advance_live();
    engine.extract_present_scene().expect("the scene extracts")
}

/// The world x of every mesh, ascending.
fn mesh_xs(scene: &RenderScene) -> Vec<f32> {
    let mut xs: Vec<f32> = scene
        .meshes
        .iter()
        .map(|mesh| mesh.model.w_axis.x)
        .collect();
    xs.sort_by(f32::total_cmp);
    xs
}

fn box_x(scene: &RenderScene, data: &BlendData, id: EntityId) -> f32 {
    let index = data
        .meshes
        .iter()
        .position(|key| *key == Some(id))
        .expect("the box is keyed");
    scene.meshes[index].model.w_axis.x
}

#[test]
fn a_moving_mesh_blends_to_the_midpoint_of_its_two_ticks() {
    let mut engine = engine_with(&[("slide", slide)]);
    let id = named(engine.world(), "box");
    let (prev_scene, prev) = tick(&mut engine);
    let (next, next_data) = tick(&mut engine);
    let mid = blend(&prev_scene, &prev, &next, &next_data, 0.5);
    assert!((box_x(&next, &next_data, id) - 2.0).abs() < 1e-5);
    assert!(
        (box_x(&mid, &next_data, id) - 1.5).abs() < 1e-5,
        "x lands between 1 and 2: {}",
        box_x(&mid, &next_data, id)
    );
    assert_eq!(blend(&prev_scene, &prev, &next, &next_data, 1.0), next);
    assert!(next_data.snaps.is_empty());
}

#[test]
fn an_entity_spawned_this_tick_takes_its_new_pose() {
    let mut engine = engine_with(&[("spawn", spawn_late)]);
    let (prev_scene, prev) = tick(&mut engine);
    let (next, next_data) = tick(&mut engine);
    assert!(
        mesh_xs(&next).contains(&10.0),
        "the late mesh is in the scene"
    );
    let mid = blend(&prev_scene, &prev, &next, &next_data, 0.5);
    assert!(
        mesh_xs(&mid).contains(&10.0),
        "no previous pose: the spawn is not dragged from the origin"
    );
}

/// Moves the box +1 m every tick; hides it on frame 0 and shows it again on frame 1.
fn hide_then_show(world: &mut World, ctx: &SystemCtx) {
    nudge(world, "box", 1.0);
    let id = named(world, "box");
    match ctx.frame.as_u64() {
        0 => {
            let hidden = Visibility {
                visible: false,
                ..Visibility::default()
            };
            world.insert(id, hidden).unwrap();
        }
        1 => {
            world.insert(id, Visibility::default()).unwrap();
        }
        _ => {}
    }
}

/// A hidden-to-shown entity has no key in the previous tick, so it snaps to its new pose instead of
/// blending from where it was last drawn.
#[test]
fn an_entity_shown_this_tick_takes_its_new_pose() {
    let mut engine = engine_with(&[("toggle", hide_then_show)]);
    let id = named(engine.world(), "box");
    let (prev_scene, prev) = tick(&mut engine);
    assert!(
        !prev.meshes.contains(&Some(id)),
        "the hidden box is not in the previous tick"
    );
    let (next, next_data) = tick(&mut engine);
    let mid = blend(&prev_scene, &prev, &next, &next_data, 0.5);
    assert_eq!(
        box_x(&mid, &next_data, id),
        box_x(&next, &next_data, id),
        "no previous pose: the box takes its new pose, with no midpoint"
    );
}

#[test]
fn a_game_marker_snaps_the_entity_and_its_child_for_one_tick() {
    let mut engine = engine_with(&[("mark", mark_box)]);
    let parent = named(engine.world(), "box");
    let child = named(engine.world(), "child");
    let (quiet_scene, quiet) = tick(&mut engine);
    assert!(quiet.snaps.is_empty());
    let (next, marked) = tick(&mut engine);
    assert!(marked.snaps.contains(&parent));
    assert!(
        marked.snaps.contains(&child),
        "the child rides its parent's cut"
    );
    assert!(marked.snaps.windows(2).all(|pair| pair[0] < pair[1]));
    let mid = blend(&quiet_scene, &quiet, &next, &marked, 0.5);
    assert_eq!(mid, next, "a snapped pair draws the newer tick unchanged");
    let (_, after) = tick(&mut engine);
    assert!(
        after.snaps.is_empty(),
        "the marker is cleared before the next tick"
    );
}

#[test]
fn a_teleport_snaps_the_character() {
    let mut engine = engine_with(&[("teleport", teleport_hero)]);
    let hero = named(engine.world(), "hero");
    let (_, quiet) = tick(&mut engine);
    assert!(quiet.snaps.is_empty());
    let (_, cut) = tick(&mut engine);
    assert_eq!(cut.snaps, vec![hero]);
    let (_, after) = tick(&mut engine);
    assert!(after.snaps.is_empty());
}

#[test]
fn a_scene_reload_bumps_the_epoch_and_blend_returns_the_new_scene() {
    let mut engine = engine_with(&[("slide", slide)]);
    let (before_scene, before) = tick(&mut engine);
    engine.load_inline_scene(SCENE).expect("the scene reloads");
    let (next, after) = tick(&mut engine);
    assert_ne!(before.epoch, after.epoch);
    assert_eq!(blend(&before_scene, &before, &next, &after, 0.5), next);
}

#[test]
fn collecting_blend_data_and_marking_move_nothing_that_is_hashed() {
    let mut collecting = engine_with(&[("session", session_marked)]);
    let mut plain = engine_with(&[("session", session_plain)]);
    let mut saw_marker = false;
    for frame in 0..6 {
        collecting.advance_live();
        collecting.extract_present_scene().unwrap();
        plain.advance_live();
        assert_eq!(
            collecting.world_hash().unwrap(),
            plain.world_hash().unwrap(),
            "frame {frame}: the marker and the blend collection are off-hash"
        );
        let marked = collecting
            .world()
            .has::<NoInterpolate>(named(collecting.world(), "box"));
        if marked && !saw_marker {
            saw_marker = true;
            let ResponseBody::Snapshot { snapshot } =
                collecting.execute(Command::EngineSnapshot).unwrap().body
            else {
                panic!("expected a snapshot");
            };
            assert!(
                !snapshot.to_string().contains("NoInterpolate"),
                "the marker never reaches a snapshot"
            );
        }
    }
    assert!(saw_marker, "the marker was in the world after its tick");
}

const PARTICLE_SCENE: &str = "format: 1
seed: 3
entities:
  - name: brazier
    components:
      Transform: {}
      ParticleEmitter:
        rate: 30.0
        life:
          - 1.0
          - 2.0
        speed:
          - 0.5
          - 1.5
        gravity: -1.0
        seed: 11
";

/// Same-key pairs between two ticks' scenes: `(prev, next)` particle centres.
fn paired(
    prev: &(RenderScene, BlendData),
    next: &(RenderScene, BlendData),
) -> Vec<(ludion_renderer::ParticleInstance, usize)> {
    let mut out = Vec::new();
    for (j, key) in next.1.particles.iter().enumerate() {
        if let Some(i) = prev.1.particles.iter().position(|k| k == key) {
            out.push((prev.0.particles[i], j));
        }
    }
    out
}

#[test]
fn a_particle_present_in_both_ticks_blends_to_the_midpoint() {
    let mut engine = Engine::builder().build().expect("the game builds");
    engine
        .load_inline_scene(PARTICLE_SCENE)
        .expect("the scene loads");
    let prev = tick(&mut engine);
    let next = tick(&mut engine);
    assert_eq!(next.1.particles.len(), next.0.particles.len());
    assert!(next.1.particles.windows(2).all(|pair| pair[0] < pair[1]));
    let pairs = paired(&prev, &next);
    assert!(pairs.len() > 10, "an endless plume persists across ticks");
    let mid = blend(&prev.0, &prev.1, &next.0, &next.1, 0.5);
    for (before, j) in pairs {
        let after = next.0.particles[j].center;
        let want = (before.center + after) * 0.5;
        assert!(
            mid.particles[j].center.abs_diff_eq(want, 1e-5),
            "particle {j} lands at the midpoint"
        );
    }
    assert_eq!(blend(&prev.0, &prev.1, &next.0, &next.1, 1.0), next.0);
}

#[test]
fn key_collection_does_not_change_the_extracted_scene() {
    let mut engine = Engine::builder().build().expect("the game builds");
    engine
        .load_inline_scene(PARTICLE_SCENE)
        .expect("the scene loads");
    let (collected, data) = tick(&mut engine);
    assert!(!data.particles.is_empty());
    let plain = extract_at(
        engine.world(),
        engine.render_size(),
        ludion_assets::AssetRoot::content(engine.project_root()),
        SimTime::from_ticks(engine.frame().0, TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )
    .expect("the scene extracts");
    assert_eq!(collected.particles, plain.particles);
    assert_eq!(collected.sprites, plain.sprites);
}

const PATTERN: &str = "format: 1
kind: pattern
name: probe
bullets:
  pellet:
    life: 600
timeline:
  - at: 0
    bullet: pellet
    count: 3
    speed: 2.0
";

const PATTERN_SCENE: &str = "format: 1
seed: 5
entities:
  - name: turret
    components:
      Transform: {}
      PatternPlayer:
        pattern: patterns/probe.yaml
        started: 0
";

#[test]
fn a_bullet_blends_to_the_midpoint_of_its_two_ticks() {
    let dir: PathBuf =
        std::env::temp_dir().join(format!("ludion-present-blend-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("patterns")).unwrap();
    std::fs::write(dir.join("patterns/probe.yaml"), PATTERN).unwrap();
    let mut engine = Engine::builder()
        .patterns()
        .system("play_patterns", play_patterns)
        .system("advance_volleys", advance_volleys)
        .project_root(&dir)
        .build()
        .expect("the pattern validates");
    engine
        .load_inline_scene(PATTERN_SCENE)
        .expect("the scene loads");
    let prev = tick(&mut engine);
    let next = tick(&mut engine);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(next.1.bullets.len(), 3);
    assert!(next.1.bullets.windows(2).all(|pair| pair[0].0 < pair[1].0));
    let mid = blend(&prev.0, &prev.1, &next.0, &next.1, 0.5);
    for &(key, j) in &next.1.bullets {
        let &(_, i) = prev
            .1
            .bullets
            .iter()
            .find(|(k, _)| *k == key)
            .expect("the bullet lives in both ticks");
        let want = (prev.0.sprites[i as usize].center + next.0.sprites[j as usize].center) * 0.5;
        assert!(mid.sprites[j as usize].center.abs_diff_eq(want, 1e-5));
        assert_ne!(
            mid.sprites[j as usize].center,
            next.0.sprites[j as usize].center
        );
    }
}

// ---- the presentation camera offset ----

const OFFSET_SCENE: &str = "format: 1
seed: 3
entities:
  - name: cam
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 10.0
      Camera: {}
  - name: box
    components:
      Transform: {}
      Mesh:
        shape: cube
";

type PresentationSystems<'a> = &'a [(&'static str, fn(&mut World, &SystemCtx))];

/// The camera's z and fov delta a tick sets, so every tick's composed pose differs.
fn offset_for(frame: u64) -> CameraOffset {
    CameraOffset {
        translation: [0.0, 0.0, 2.0 * frame as f32],
        fov_delta: -(frame as f32),
        ..CameraOffset::default()
    }
}

fn drive(world: &mut World, ctx: &SystemCtx) {
    let existing = world.presentation_entities().next();
    let id = existing.unwrap_or_else(|| world.spawn_presentation());
    world.insert(id, offset_for(ctx.frame.as_u64())).unwrap();
}

/// Drives the offset until frame 2, then despawns it.
fn drive_then_despawn(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() >= 2 {
        let gone: Vec<EntityId> = world.presentation_entities().collect();
        for id in gone {
            world.despawn(id).unwrap();
        }
    } else {
        drive(world, ctx);
    }
}

/// Drives the offset, and on frame 1 marks the offset entity `NoInterpolate`.
fn drive_and_mark_offset(world: &mut World, ctx: &SystemCtx) {
    drive(world, ctx);
    if ctx.frame.as_u64() == 1 {
        let id = world.presentation_entities().next().unwrap();
        world.insert(id, NoInterpolate).unwrap();
    }
}

/// Marks the camera `NoInterpolate` on frame 1 (the cut a `snap_rig` makes).
fn cut_camera(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 1 {
        let cam = named(world, "cam");
        world.insert(cam, NoInterpolate).unwrap();
    }
}

fn offset_engine(scene: &str, systems: Systems, presentation: PresentationSystems) -> Engine {
    let mut builder = Engine::builder();
    for &(name, system) in systems {
        builder = builder.system(name, system);
    }
    for &(name, system) in presentation {
        builder = builder.presentation_system(name, system);
    }
    let mut engine = builder.build().expect("the game builds");
    engine.load_inline_scene(scene).expect("the scene loads");
    engine
}

fn eye(scene: &RenderScene) -> [f32; 3] {
    scene.camera.position.to_array()
}

fn y_scale(scene: &RenderScene) -> f32 {
    scene.camera.projection.y_axis.y
}

/// A changing offset blends to the midpoint of the two composed poses and projections, and
/// `alpha 1` is exactly `next`.
#[test]
fn a_changing_offset_blends_between_its_two_composed_cameras() {
    let mut engine = offset_engine(OFFSET_SCENE, &[], &[("drive", drive)]);
    let prev = tick(&mut engine);
    let next = tick(&mut engine);
    let mid = blend(&prev.0, &prev.1, &next.0, &next.1, 0.5);
    assert!(eye(&next.0)[2] > eye(&prev.0)[2], "the offset pulled back");
    let expected = (eye(&prev.0)[2] + eye(&next.0)[2]) / 2.0;
    assert!((eye(&mid)[2] - expected).abs() < 1e-4, "{:?}", eye(&mid));
    let expected_fov = {
        let fov = |scene: &RenderScene| 2.0 * ludion_core::math::atan(1.0 / y_scale(scene));
        (fov(&prev.0) + fov(&next.0)) / 2.0
    };
    assert!((y_scale(&mid) - 1.0 / ludion_core::math::tan(expected_fov / 2.0)).abs() < 1e-4);
    assert_eq!(blend(&prev.0, &prev.1, &next.0, &next.1, 1.0), next.0);
    let model = next.1.camera.expect("the camera blends").model;
    let view = next.0.camera.view;
    assert!(
        (ludion_core::math::mat4_inverse(model) - view)
            .to_cols_array()
            .iter()
            .all(|v| v.abs() < 1e-4),
        "the blend camera inverts the drawn view"
    );
}

/// An offset that is gone on the next tick slides over that tick, and ends on the rig.
#[test]
fn an_offset_despawned_slides_to_the_rig() {
    let mut engine = offset_engine(OFFSET_SCENE, &[], &[("drive", drive_then_despawn)]);
    let mut ticks = Vec::new();
    for _ in 0..4 {
        ticks.push(tick(&mut engine));
    }
    let (with, without) = (&ticks[1], &ticks[2]);
    assert!(eye(&with.0)[2] > 10.5, "the offset was in force");
    assert!((eye(&without.0)[2] - 10.0).abs() < 1e-5, "and is gone");
    let mid = blend(&with.0, &with.1, &without.0, &without.1, 0.5);
    let expected = (eye(&with.0)[2] + 10.0) / 2.0;
    assert!((eye(&mid)[2] - expected).abs() < 1e-4, "{:?}", eye(&mid));
    assert!(without.1.snaps.is_empty());
}

/// A cut camera with an offset on: `blend` takes `next`, whose camera still holds the offset.
#[test]
fn a_cut_camera_keeps_the_offset() {
    let mut engine = offset_engine(OFFSET_SCENE, &[("cut", cut_camera)], &[("drive", drive)]);
    let mut cut = None;
    let mut previous = tick(&mut engine);
    for _ in 0..3 {
        let next = tick(&mut engine);
        let key = next.1.camera.as_ref().map(|camera| camera.key);
        if key.is_some_and(|key| next.1.snaps.contains(&key)) {
            cut = Some((previous.clone(), next.clone()));
        }
        previous = next;
    }
    let (prev, next) = cut.expect("one tick cut the camera");
    assert_eq!(blend(&prev.0, &prev.1, &next.0, &next.1, 0.5), next.0);
    assert!(eye(&next.0)[2] > 10.0, "the offset is in the cut camera");
}

/// `NoInterpolate` on the offset entity cuts the camera for that tick only, and the marker is gone
/// the tick after.
#[test]
fn a_marked_offset_entity_cuts_the_camera_for_one_tick() {
    let mut engine = offset_engine(OFFSET_SCENE, &[], &[("drive", drive_and_mark_offset)]);
    let mut cuts = Vec::new();
    let mut previous = tick(&mut engine);
    for _ in 0..4 {
        let next = tick(&mut engine);
        let key = next.1.camera.as_ref().map(|camera| camera.key).unwrap();
        cuts.push(next.1.snaps.contains(&key));
        if cuts.last() == Some(&true) {
            assert_eq!(
                blend(&previous.0, &previous.1, &next.0, &next.1, 0.5),
                next.0
            );
        }
        previous = next;
    }
    assert_eq!(cuts.iter().filter(|&&cut| cut).count(), 1, "{cuts:?}");
    let marker_left = engine
        .world()
        .presentation_entities()
        .any(|id| engine.world().has::<NoInterpolate>(id));
    assert!(!marker_left, "the cut was cleared");
}

/// A pixel-snapped orthographic camera does not blend; its view holds the offset.
#[test]
fn a_pixel_snapped_camera_does_not_blend_and_holds_the_offset() {
    let root = std::env::temp_dir().join(format!("ludion-offset-snap-{}", std::process::id()));
    std::fs::create_dir_all(root.join("art")).unwrap();
    let rgba = vec![200u8; 8 * 8 * 4];
    std::fs::write(
        root.join("art/fixture.png"),
        ludion_assets::encode_png(8, 8, &rgba).unwrap(),
    )
    .unwrap();
    let mut regions = std::collections::BTreeMap::new();
    regions.insert(
        "mid".to_string(),
        ludion_scene::AtlasRegion {
            rect: [0, 0, 4, 4],
            pivot: [2.0, 2.0],
        },
    );
    let atlas = ludion_scene::AtlasDocument {
        generated_from: "fixture.atlas-source.yaml".to_string(),
        pixels_per_unit: 4.0,
        color_map: "fixture.png".to_string(),
        filter: ludion_scene::AtlasFilter::Nearest,
        regions,
        flipbooks: Vec::new(),
    };
    std::fs::write(
        root.join("art/fixture.atlas.yaml"),
        ludion_scene::emit_atlas(&atlas),
    )
    .unwrap();
    let scene = "format: 1
seed: 1
entities:
  - name: cam
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 10.0
      Camera:
        projection:
          type: orthographic
          height: 8.0
          pixel_snap: true
  - name: sprite
    components:
      Transform: {}
      Sprite:
        atlas: art/fixture.atlas.yaml
        frame: mid
";
    let run = |presentation: PresentationSystems| {
        let mut builder = Engine::builder().project_root(&root);
        for &(name, system) in presentation {
            builder = builder.presentation_system(name, system);
        }
        let mut engine = builder.build().unwrap();
        engine.load_inline_scene(scene).unwrap();
        tick(&mut engine);
        tick(&mut engine)
    };
    let (plain, _) = run(&[]);
    let (offset, data) = run(&[("drive", drive)]);
    assert!(data.camera.is_none(), "a snapped camera takes no blend");
    assert_ne!(
        offset.camera.view, plain.camera.view,
        "the view holds the offset"
    );
    assert_eq!(
        offset.camera.projection, plain.camera.projection,
        "ortho ignores fov_delta"
    );
    let _ = std::fs::remove_dir_all(&root);
}
