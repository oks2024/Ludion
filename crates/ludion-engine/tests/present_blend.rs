//! The engine fills the display-rate blend data on handover: the pose table, the per-item keys, the
//! tick's snap set, the load epoch and the camera. Every case drives a small inline scene through
//! `advance_live` and reads `Engine::extract_present_scene` after each tick; none needs a GPU.

use ludion_core::ids::EntityId;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Engine, Mesh, Name, NoInterpolate, Transform, teleport_character};
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
    let (_, prev) = tick(&mut engine);
    let (next, next_data) = tick(&mut engine);
    let mid = blend(&prev, &next, &next_data, 0.5);
    assert!((box_x(&next, &next_data, id) - 2.0).abs() < 1e-5);
    assert!(
        (box_x(&mid, &next_data, id) - 1.5).abs() < 1e-5,
        "x lands between 1 and 2: {}",
        box_x(&mid, &next_data, id)
    );
    assert_eq!(blend(&prev, &next, &next_data, 1.0), next);
    assert!(next_data.snaps.is_empty());
}

#[test]
fn an_entity_spawned_this_tick_takes_its_new_pose() {
    let mut engine = engine_with(&[("spawn", spawn_late)]);
    let (_, prev) = tick(&mut engine);
    let (next, next_data) = tick(&mut engine);
    assert!(
        mesh_xs(&next).contains(&10.0),
        "the late mesh is in the scene"
    );
    let mid = blend(&prev, &next, &next_data, 0.5);
    assert!(
        mesh_xs(&mid).contains(&10.0),
        "no previous pose: the spawn is not dragged from the origin"
    );
}

#[test]
fn a_game_marker_snaps_the_entity_and_its_child_for_one_tick() {
    let mut engine = engine_with(&[("mark", mark_box)]);
    let parent = named(engine.world(), "box");
    let child = named(engine.world(), "child");
    let (_, quiet) = tick(&mut engine);
    assert!(quiet.snaps.is_empty());
    let (next, marked) = tick(&mut engine);
    assert!(marked.snaps.contains(&parent));
    assert!(
        marked.snaps.contains(&child),
        "the child rides its parent's cut"
    );
    assert!(marked.snaps.windows(2).all(|pair| pair[0] < pair[1]));
    let mid = blend(&quiet, &next, &marked, 0.5);
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
    let (_, before) = tick(&mut engine);
    engine.load_inline_scene(SCENE).expect("the scene reloads");
    let (next, after) = tick(&mut engine);
    assert_ne!(before.epoch, after.epoch);
    assert_eq!(blend(&before, &next, &after, 0.5), next);
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
