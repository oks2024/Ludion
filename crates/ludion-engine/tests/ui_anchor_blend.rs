//! A world-anchored UI element under `present_interpolation`: extract records the anchors in
//! `BlendData`, and `blend` re-places the keyed rows against the blended camera and target point
//! (`Plan/ui-hud.md` decisions 102-106). Every case drives a small inline scene through
//! `advance_live` and `Engine::extract_present_scene`; none needs a GPU.

use ludion_components::ui::{UiAnchorBehind, UiAnchorShow, UiGroup, UiWorldAnchor};
use ludion_core::ids::EntityId;
use ludion_core::time::{SimTime, TickRate};
use ludion_ecs::{SystemCtx, World};
use ludion_engine::components::{Anchor, UiRect};
use ludion_engine::ui::place_world_anchor;
use ludion_engine::{Engine, Name, NoInterpolate, Transform, extract_at};
use ludion_renderer::{BlendData, QualityParams, RenderScene, UiInstance, blend};

const SCENE: &str = "\
format: 1
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
  - name: target
    components:
      Transform:
        translation:
          - 2.0
          - 0.0
          - 0.0
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

fn nudge(world: &mut World, wanted: &str, dx: f32) {
    let id = named(world, wanted);
    world.get_mut::<Transform>(id).unwrap().unwrap().translation[0] += dx;
}

fn slide_target(world: &mut World, _ctx: &SystemCtx) {
    nudge(world, "target", 1.0);
}

fn slide_camera(world: &mut World, _ctx: &SystemCtx) {
    nudge(world, "cam", 1.0);
}

fn mark_target(world: &mut World, ctx: &SystemCtx) {
    nudge(world, "target", 1.0);
    if ctx.frame.as_u64() == 1 {
        let id = named(world, "target");
        world.insert(id, NoInterpolate).unwrap();
    }
}

fn engine_with(system: fn(&mut World, &SystemCtx)) -> Engine {
    let mut engine = Engine::builder()
        .system("move", system)
        .build()
        .expect("the game builds");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    engine
}

fn anchor_to(target: EntityId) -> UiWorldAnchor {
    UiWorldAnchor {
        show: UiAnchorShow::Always,
        behind: UiAnchorBehind::Clamp,
        margin: 48.0,
        fade: None,
        ..UiWorldAnchor::new(target)
    }
}

fn spawn<C: ludion_ecs::Component>(engine: &mut Engine, component: C) -> EntityId {
    let id = engine.world_mut().spawn_presentation();
    engine.world_mut().insert(id, component).unwrap();
    id
}

fn marker_rect() -> UiRect {
    UiRect {
        anchor: Anchor::Center,
        size: [40.0, 20.0],
        ..UiRect::default()
    }
}

fn tick(engine: &mut Engine) -> (RenderScene, BlendData) {
    engine.advance_live();
    engine.extract_present_scene().expect("the scene extracts")
}

fn rect_x(scene: &RenderScene, index: usize) -> f32 {
    match &scene.ui[index] {
        UiInstance::Rect { rect_px, .. } => rect_px[0],
        other => panic!("not a rect: {other:?}"),
    }
}

/// Where a centred marker's left edge lands for `point` seen from `scene`'s camera.
fn expected_x(engine: &Engine, scene: &RenderScene, point: [f32; 3]) -> f32 {
    let hit = place_world_anchor(
        scene.camera.view,
        scene.camera.projection,
        point,
        engine.render_size(),
        48.0,
    )
    .expect("the point places");
    let ps = engine.render_size().1 as f32 / 1080.0;
    hit.pixel[0] - 20.0 * ps
}

fn lerp_x(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, b[1], b[2]]
}

struct Anchored {
    engine: Engine,
    marker: EntityId,
    prev_scene: RenderScene,
    prev: BlendData,
    next: RenderScene,
    data: BlendData,
}

/// A marker pinned to the target, two ticks in.
fn anchored(system: fn(&mut World, &SystemCtx)) -> Anchored {
    let mut engine = engine_with(system);
    let target = named(engine.world(), "target");
    let marker = spawn(&mut engine, marker_rect());
    engine
        .world_mut()
        .insert(marker, anchor_to(target))
        .unwrap();
    let (prev_scene, prev) = tick(&mut engine);
    let (next, data) = tick(&mut engine);
    Anchored {
        engine,
        marker,
        prev_scene,
        prev,
        next,
        data,
    }
}

#[test]
fn extract_keys_the_anchored_rect_and_records_where_it_drew() {
    let mut engine = engine_with(slide_target);
    let target = named(engine.world(), "target");
    let marker = spawn(&mut engine, marker_rect());
    engine
        .world_mut()
        .insert(marker, anchor_to(target))
        .unwrap();
    spawn(&mut engine, marker_rect());
    let (scene, data) = tick(&mut engine);
    assert_eq!(scene.ui.len(), 2);
    assert_eq!(data.ui.len(), scene.ui.len());
    assert_eq!(data.ui.iter().filter(|key| key.is_some()).count(), 1);
    assert!(data.ui.contains(&Some(marker)));
    assert_eq!(data.ui_anchors.len(), 1);
    let record = data.ui_anchors[0];
    assert_eq!(
        (record.anchor, record.target, record.view),
        (marker, target, None)
    );
    assert_eq!(record.point, [3.0, 0.0, 0.0]);
    assert_eq!(
        record.box_px,
        [
            0.0,
            0.0,
            engine.render_size().0 as f32,
            engine.render_size().1 as f32
        ]
    );
}

#[test]
fn a_moving_target_blends_the_rect_between_the_two_ticks() {
    let a = anchored(slide_target);
    let mid = blend(&a.prev_scene, &a.prev, &a.next, &a.data, 0.5);
    let point = lerp_x(a.prev.ui_anchors[0].point, a.data.ui_anchors[0].point, 0.5);
    let x = rect_x(&mid, 0);
    assert!((x - expected_x(&a.engine, &a.next, point)).abs() < 1e-3);
    assert!(
        rect_x(&a.prev_scene, 0) < x && x < rect_x(&a.next, 0),
        "{x}"
    );
    assert_eq!(blend(&a.prev_scene, &a.prev, &a.next, &a.data, 1.0), a.next);
    assert_eq!(a.data.ui_anchors[0].anchor, a.marker);
}

#[test]
fn an_anchored_group_moves_every_child_by_the_same_delta() {
    let mut engine = engine_with(slide_target);
    let target = named(engine.world(), "target");
    let group = spawn(
        &mut engine,
        UiGroup {
            anchor: Anchor::Center,
            size: [40.0, 20.0],
            ..UiGroup::default()
        },
    );
    engine.world_mut().insert(group, anchor_to(target)).unwrap();
    for _ in 0..2 {
        spawn(
            &mut engine,
            UiRect {
                group: Some(group),
                ..marker_rect()
            },
        );
    }
    let (prev_scene, prev) = tick(&mut engine);
    let (next, data) = tick(&mut engine);
    assert_eq!(data.ui, vec![Some(group), Some(group)]);
    let mid = blend(&prev_scene, &prev, &next, &data, 0.5);
    let moved = [
        rect_x(&mid, 0) - rect_x(&next, 0),
        rect_x(&mid, 1) - rect_x(&next, 1),
    ];
    assert!(
        moved[0] < -1.0,
        "the group moved back toward the previous tick: {moved:?}"
    );
    assert!((moved[0] - moved[1]).abs() < 1e-3);
}

#[test]
fn a_no_interpolate_target_keeps_the_newer_row() {
    let mut engine = engine_with(mark_target);
    let target = named(engine.world(), "target");
    let marker = spawn(&mut engine, marker_rect());
    engine
        .world_mut()
        .insert(marker, anchor_to(target))
        .unwrap();
    let (prev_scene, prev) = tick(&mut engine);
    let (next, data) = tick(&mut engine);
    assert!(data.snaps.contains(&target));
    assert_eq!(blend(&prev_scene, &prev, &next, &data, 0.5), next);
}

#[test]
fn a_moving_camera_moves_the_row_with_a_static_target() {
    let a = anchored(slide_camera);
    let mid = blend(&a.prev_scene, &a.prev, &a.next, &a.data, 0.5);
    let drawn = rect_x(&mid, 0);
    let newer = rect_x(&a.next, 0);
    assert!(
        drawn > newer,
        "the camera is half a metre behind: {drawn} {newer}"
    );
    assert_eq!(mid.ui.len(), a.next.ui.len());
}

#[test]
fn blend_data_collection_leaves_the_scene_unchanged() {
    let mut engine = engine_with(slide_target);
    let target = named(engine.world(), "target");
    let marker = spawn(&mut engine, marker_rect());
    engine
        .world_mut()
        .insert(marker, anchor_to(target))
        .unwrap();
    engine.advance_live();
    let (with_blend, data) = engine.extract_present_scene().expect("extracts");
    assert!(!data.ui_anchors.is_empty());
    let plain = extract_at(
        engine.world(),
        engine.render_size(),
        ludion_assets::AssetRoot::content(engine.project_root()),
        SimTime::from_ticks(0, TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )
    .expect("extracts");
    assert_eq!(plain.ui, with_blend.ui);
    assert_eq!(plain.camera, with_blend.camera);
}
