//! The engine fills the display-rate blend data on handover: the pose table, the per-item keys, the
//! tick's snap set, the load epoch and the camera. Every case drives a small inline scene through
//! `advance_live` and reads `Engine::extract_present_scene` after each tick; none needs a GPU.

use std::path::PathBuf;

use ludion_core::ids::EntityId;
use ludion_core::time::{SimTime, TickRate};
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    Engine, Mesh, Name, NoInterpolate, QualityParams, Transform, Visibility, advance_volleys,
    extract_at, play_patterns, teleport_character,
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
