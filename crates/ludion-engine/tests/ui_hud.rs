//! `UiTween` and `UiWorldAnchor` resolved in extract: a presentation HUD element follows a sim cube
//! through the camera as drawn, clamps to the edge, hides and fades, and tweens on the real or the
//! game clock. Every case extracts through `Engine::extract_present_scene`; none needs a GPU.

use ludion_components::ui::{
    UiAnchorBehind, UiAnchorShow, UiClock, UiEase, UiTween, UiTweenField, UiTweenRepeat,
    UiTweenTrack, UiWorldAnchor,
};
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::time::{GameClock, TickRate};
use ludion_ecs::{SystemCtx, World};
use ludion_engine::components::{Anchor, UiRect, UiText};
use ludion_engine::ui::{place_world_anchor, resolve_ui_rect, ui_tween_sample};
use ludion_engine::{Diagnostics, Engine, Name, Transform};
use ludion_protocol::Command;
use ludion_protocol::command::SimStepParams;
use ludion_renderer::{RenderScene, UiInstance};

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

const HOLD_FROM: u64 = 10;
const HOLD_TO: u64 = 29;

fn freeze_window(world: &mut World, ctx: &SystemCtx) {
    if (HOLD_FROM..=HOLD_TO).contains(&ctx.frame.as_u64()) {
        world.resource_mut::<GameClock>().unwrap().hold();
    }
}

fn engine() -> Engine {
    let mut engine = Engine::builder()
        .system("freeze_window", freeze_window)
        .build()
        .expect("the game builds");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    engine
}

fn step(engine: &mut Engine, ticks: u64) {
    engine
        .execute(Command::SimStep(SimStepParams { ticks, hash: false }))
        .expect("the sim steps");
}

fn extract(engine: &mut Engine) -> RenderScene {
    engine.extract_present_scene().expect("extracts").0
}

fn named(engine: &Engine, wanted: &str) -> EntityId {
    let world = engine.world();
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

fn move_target(engine: &mut Engine, to: [f32; 3]) {
    let id = named(engine, "target");
    engine
        .world_mut()
        .get_mut::<Transform>(id)
        .unwrap()
        .unwrap()
        .translation = to;
}

fn anchor(target: EntityId) -> UiWorldAnchor {
    UiWorldAnchor {
        target,
        world_offset: [0.0; 3],
        show: UiAnchorShow::Always,
        behind: UiAnchorBehind::Clamp,
        margin: 48.0,
        fade: None,
        ..UiWorldAnchor::new(target)
    }
}

/// A centred 40 x 20 marker pinned to the target.
fn marker(engine: &mut Engine, world_anchor: UiWorldAnchor) -> EntityId {
    let id = engine.world_mut().spawn_presentation();
    let world = engine.world_mut();
    world
        .insert(
            id,
            UiRect {
                anchor: Anchor::Center,
                size: [40.0, 20.0],
                ..UiRect::default()
            },
        )
        .unwrap();
    world.insert(id, world_anchor).unwrap();
    id
}

fn pixel_scale(engine: &Engine) -> f32 {
    engine.render_size().1 as f32 / 1080.0
}

fn rects(scene: &RenderScene) -> Vec<[f32; 4]> {
    scene
        .ui
        .iter()
        .filter_map(|instance| match instance {
            UiInstance::Rect { rect_px, .. } => Some(*rect_px),
            UiInstance::Text { .. } | UiInstance::Shape { .. } | UiInstance::Polygon { .. } => None,
        })
        .collect()
}

fn centre(rect: [f32; 4]) -> [f32; 2] {
    [rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5]
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-2
}

#[test]
fn a_marker_on_screen_sits_on_the_projected_target() {
    let mut engine = engine();
    let target = named(&engine, "target");
    marker(&mut engine, anchor(target));
    step(&mut engine, 1);
    let scene = extract(&mut engine);
    let expected = place_world_anchor(
        scene.camera.view,
        scene.camera.projection,
        [2.0, 0.0, 0.0],
        engine.render_size(),
        48.0,
    )
    .unwrap();
    assert!(expected.on_screen);
    let drawn = rects(&scene);
    assert_eq!(drawn.len(), 1);
    let c = centre(drawn[0]);
    assert!(
        close(c[0], expected.pixel[0]) && close(c[1], expected.pixel[1]),
        "{c:?}"
    );
    assert!(close(drawn[0][2], 40.0 * pixel_scale(&engine)));
}

#[test]
fn a_marker_off_the_right_edge_clamps_to_the_margin() {
    let mut engine = engine();
    let target = named(&engine, "target");
    marker(&mut engine, anchor(target));
    move_target(&mut engine, [100.0, 0.0, 0.0]);
    step(&mut engine, 1);
    let drawn = rects(&extract(&mut engine));
    let (w, h) = engine.render_size();
    let c = centre(drawn[0]);
    assert!(close(c[0], w as f32 - 48.0 * pixel_scale(&engine)), "{c:?}");
    assert!(close(c[1], h as f32 * 0.5));
}

#[test]
fn a_marker_behind_the_camera_goes_to_the_edge_of_its_side() {
    let mut engine = engine();
    let target = named(&engine, "target");
    marker(&mut engine, anchor(target));
    move_target(&mut engine, [-3.0, 0.0, 20.0]);
    step(&mut engine, 1);
    let c = centre(rects(&extract(&mut engine))[0]);
    assert!(
        close(c[0], 48.0 * pixel_scale(&engine)),
        "never mirrored: {c:?}"
    );
}

#[test]
fn show_on_screen_hides_an_off_screen_marker_and_behind_hide_removes_it() {
    let mut engine = engine();
    let target = named(&engine, "target");
    let id = marker(&mut engine, anchor(target));
    engine
        .world_mut()
        .get_mut::<UiWorldAnchor>(id)
        .unwrap()
        .unwrap()
        .show = UiAnchorShow::OnScreen;
    step(&mut engine, 1);
    assert_eq!(rects(&extract(&mut engine)).len(), 1);
    move_target(&mut engine, [100.0, 0.0, 0.0]);
    assert!(rects(&extract(&mut engine)).is_empty());

    let a = engine
        .world_mut()
        .get_mut::<UiWorldAnchor>(id)
        .unwrap()
        .unwrap();
    a.show = UiAnchorShow::Always;
    a.behind = UiAnchorBehind::Hide;
    move_target(&mut engine, [0.0, 0.0, 20.0]);
    assert!(rects(&extract(&mut engine)).is_empty());
}

#[test]
fn a_fade_midpoint_halves_the_alpha() {
    let mut engine = engine();
    let target = named(&engine, "target");
    let mut with_fade = anchor(target);
    with_fade.fade = Some([0.0, 20.0]);
    marker(&mut engine, with_fade);
    move_target(&mut engine, [0.0, 0.0, 0.0]);
    step(&mut engine, 1);
    let scene = extract(&mut engine);
    let UiInstance::Rect { color, .. } = &scene.ui[0] else {
        panic!("a rect")
    };
    assert!((color[3] - 0.5).abs() < 1e-5, "{color:?}");
}

#[test]
fn a_dead_target_draws_nothing() {
    let mut engine = engine();
    let target = named(&engine, "target");
    marker(&mut engine, anchor(target));
    engine.world_mut().despawn(target).unwrap();
    step(&mut engine, 1);
    assert!(extract(&mut engine).ui.is_empty());
}

#[test]
fn elements_without_the_new_components_extract_as_before() {
    let mut engine = engine();
    let id = engine.world_mut().spawn_presentation();
    let rect = UiRect {
        anchor: Anchor::BottomRight,
        offset: [-10.0, -10.0],
        size: [64.0, 32.0],
        color: [0.2, 0.4, 0.6, 0.8],
        ..UiRect::default()
    };
    engine.world_mut().insert(id, rect.clone()).unwrap();
    step(&mut engine, 1);
    let drawn = rects(&extract(&mut engine));
    assert_eq!(
        drawn,
        vec![resolve_ui_rect(
            rect.anchor,
            rect.offset,
            rect.size,
            engine.render_size()
        )]
    );
}

fn track(
    field: UiTweenField,
    from: &[f32],
    to: &[f32],
    duration: f32,
    clock: UiClock,
) -> UiTweenTrack {
    UiTweenTrack {
        field,
        from: from.to_vec(),
        to: to.to_vec(),
        start: 0,
        duration,
        ease: UiEase::Linear,
        repeat: UiTweenRepeat::Once,
        clock,
    }
}

#[test]
fn the_real_clock_runs_through_a_hold_and_the_game_clock_stands_still() {
    let mut engine = engine();
    let id = engine.world_mut().spawn_presentation();
    let tween = UiTween {
        tracks: vec![
            track(
                UiTweenField::Offset,
                &[0.0, 0.0],
                &[100.0, 0.0],
                1.0,
                UiClock::Real,
            ),
            track(
                UiTweenField::Size,
                &[100.0, 100.0],
                &[200.0, 100.0],
                1.0,
                UiClock::Game,
            ),
        ],
    };
    engine.world_mut().insert(id, UiRect::default()).unwrap();
    engine.world_mut().insert(id, tween.clone()).unwrap();

    step(&mut engine, 15);
    let early = rects(&extract(&mut engine))[0];
    let frame_early = engine.frame().as_u64();
    step(&mut engine, 10);
    let late = rects(&extract(&mut engine))[0];
    let frame_late = engine.frame().as_u64();
    assert!(HOLD_FROM < frame_early && frame_late <= HOLD_TO);

    let scale = pixel_scale(&engine);
    assert_eq!(
        early[2], late[2],
        "the game clock held, so the size stood still"
    );
    assert!(
        late[0] > early[0],
        "the real clock ran: x {} to {}",
        early[0],
        late[0]
    );
    let rate = TickRate::DEFAULT;
    let offset = ui_tween_sample(&tween, UiTweenField::Offset, frame_late, 0, rate).unwrap();
    assert!(close(late[0], offset[0] * scale));
}

#[test]
fn a_scale_track_scales_text_glyphs_about_the_block_centre() {
    let mut engine = engine();
    let id = engine.world_mut().spawn_presentation();
    engine
        .world_mut()
        .insert(
            id,
            UiText {
                anchor: Anchor::Center,
                text: "HELLO".into(),
                ..UiText::default()
            },
        )
        .unwrap();
    step(&mut engine, 1);
    let quads = |scene: &RenderScene| match &scene.ui[0] {
        UiInstance::Text { quads, .. } => quads.iter().map(|q| q.pos).collect::<Vec<_>>(),
        other => panic!("expected text, got {other:?}"),
    };
    let base = quads(&extract(&mut engine));
    let bounds = base
        .iter()
        .fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, q| {
            [
                b[0].min(q[0]),
                b[1].min(q[1]),
                b[2].max(q[2]),
                b[3].max(q[3]),
            ]
        });
    let c = [(bounds[0] + bounds[2]) * 0.5, (bounds[1] + bounds[3]) * 0.5];
    engine
        .world_mut()
        .insert(
            id,
            UiTween {
                tracks: vec![track(
                    UiTweenField::Scale,
                    &[2.0],
                    &[2.0],
                    0.0,
                    UiClock::Real,
                )],
            },
        )
        .unwrap();
    let scaled = quads(&extract(&mut engine));
    assert_eq!(base.len(), scaled.len());
    for (b, s) in base.iter().zip(&scaled) {
        let expected = [
            (b[0] - c[0]) * 2.0 + c[0],
            (b[1] - c[1]) * 2.0 + c[1],
            (b[2] - c[0]) * 2.0 + c[0],
            (b[3] - c[1]) * 2.0 + c[1],
        ];
        for (e, g) in expected.iter().zip(s) {
            assert!(close(*e, *g), "{expected:?} vs {s:?}");
        }
    }
}

#[test]
fn a_size_track_on_text_is_skipped_with_l2300() {
    let mut engine = engine();
    let id = engine.world_mut().spawn_presentation();
    let text = UiText {
        text: "HI".into(),
        ..UiText::default()
    };
    engine.world_mut().insert(id, text).unwrap();
    step(&mut engine, 1);
    let base = extract(&mut engine).ui;
    engine
        .world_mut()
        .insert(
            id,
            UiTween {
                tracks: vec![track(
                    UiTweenField::Size,
                    &[1.0, 1.0],
                    &[2.0, 2.0],
                    0.0,
                    UiClock::Real,
                )],
            },
        )
        .unwrap();
    assert_eq!(extract(&mut engine).ui, base);
    extract(&mut engine);
    let reported: Vec<String> = engine
        .world()
        .resource::<Diagnostics>()
        .map(|d| {
            d.reported(code::UI_MOTION_UNSUPPORTED)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(reported.len(), 1, "{reported:?}");
}

fn rotated(scene: &RenderScene) -> Vec<([f32; 4], f32, [f32; 2])> {
    scene
        .ui
        .iter()
        .filter_map(|instance| match instance {
            UiInstance::Rect {
                rect_px, rotation, ..
            } => Some((*rect_px, rotation.radians, rotation.pivot_px)),
            _ => None,
        })
        .collect()
}

#[test]
fn rotate_toward_turns_an_edge_marker_about_its_centre_and_leaves_an_on_screen_one() {
    let mut engine = engine();
    let target = named(&engine, "target");
    let mut turning = anchor(target);
    turning.rotate_toward = true;
    marker(&mut engine, turning);
    move_target(&mut engine, [100.0, 0.0, 0.0]);
    step(&mut engine, 1);
    let rows = rotated(&extract(&mut engine));
    let (rect, radians, pivot) = rows[0];
    assert!(
        (radians - std::f32::consts::FRAC_PI_2).abs() < 1e-3,
        "{radians}"
    );
    let c = centre(rect);
    assert!(
        close(pivot[0], c[0]) && close(pivot[1], c[1]),
        "{pivot:?} {c:?}"
    );

    move_target(&mut engine, [2.0, 0.0, 0.0]);
    let rows = rotated(&extract(&mut engine));
    assert_eq!(rows[0].1, 0.0, "on screen: no turn");
}

#[test]
fn distance_scale_shrinks_about_the_projected_point() {
    let mut engine = engine();
    let target = named(&engine, "target");
    let mut scaled = anchor(target);
    scaled.distance_scale = Some([0.0, 20.0, 1.0, 0.5]);
    let id = engine.world_mut().spawn_presentation();
    let world = engine.world_mut();
    world
        .insert(
            id,
            UiRect {
                anchor: Anchor::BottomCenter,
                size: [40.0, 20.0],
                ..UiRect::default()
            },
        )
        .unwrap();
    world.insert(id, scaled).unwrap();
    move_target(&mut engine, [0.0, 0.0, 0.0]);
    step(&mut engine, 1);
    let scene = extract(&mut engine);
    let expected = place_world_anchor(
        scene.camera.view,
        scene.camera.projection,
        [0.0; 3],
        engine.render_size(),
        48.0,
    )
    .unwrap();
    assert!(close(expected.distance, 10.0));
    let rect = rects(&scene)[0];
    let ps = pixel_scale(&engine);
    assert!(
        close(rect[2], 30.0 * ps) && close(rect[3], 15.0 * ps),
        "{rect:?}"
    );
    assert!(
        close(rect[0] + rect[2] * 0.5, expected.pixel[0]),
        "{rect:?}"
    );
    assert!(close(rect[1] + rect[3], expected.pixel[1]), "{rect:?}");
}

const RADAR: &str = "  - name: radar-cam
    presentation: true
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 10.0
      Camera: {}
      RenderView:
        name: radar
        size:
          - 200
          - 100
  - name: radar-rect
    components:
      UiRect:
        anchor: top_left
        offset:
          - 100.0
          - 50.0
        size:
          - 200.0
          - 100.0
        view: radar
";

fn radar_engine() -> Engine {
    let mut engine = Engine::builder().build().expect("the game builds");
    engine
        .load_inline_scene(&format!("{SCENE}{RADAR}"))
        .expect("the scene loads");
    engine
}

fn reported_l2308(engine: &Engine) -> usize {
    engine
        .world()
        .resource::<Diagnostics>()
        .map_or(0, |d| d.reported(code::UI_ANCHOR_CAMERA_UNRESOLVED).count())
}

#[test]
fn a_camera_anchor_places_the_marker_in_the_box_of_the_rect_that_shows_the_view() {
    let mut engine = radar_engine();
    let target = named(&engine, "target");
    let mut blip = anchor(target);
    blip.camera = Some(named(&engine, "radar-cam"));
    blip.margin = 4.0;
    marker(&mut engine, blip);
    step(&mut engine, 1);
    let scene = extract(&mut engine);
    let ps = pixel_scale(&engine);
    let view = &scene.views[0].camera;
    let expected = ludion_engine::ui::place_world_anchor_in(
        view.view,
        view.projection,
        [2.0, 0.0, 0.0],
        resolve_ui_rect(
            Anchor::TopLeft,
            [100.0, 50.0],
            [200.0, 100.0],
            engine.render_size(),
        ),
        4.0 * ps,
    )
    .unwrap();
    let blips: Vec<[f32; 4]> = scene
        .ui
        .iter()
        .filter_map(|instance| match instance {
            UiInstance::Rect {
                rect_px,
                view: None,
                ..
            } => Some(*rect_px),
            _ => None,
        })
        .collect();
    assert_eq!(blips.len(), 1);
    let c = centre(blips[0]);
    assert!(
        close(c[0], expected.pixel[0]) && close(c[1], expected.pixel[1]),
        "{c:?} {expected:?}"
    );
    assert_eq!(reported_l2308(&engine), 0);
}

#[test]
fn an_unresolved_camera_draws_nothing_and_is_l2308() {
    let mut engine = engine();
    let target = named(&engine, "target");
    let mut blip = anchor(target);
    blip.camera = Some(named(&engine, "cam"));
    marker(&mut engine, blip);
    step(&mut engine, 1);
    assert!(extract(&mut engine).ui.is_empty());
    extract(&mut engine);
    assert_eq!(reported_l2308(&engine), 1);
}

#[test]
fn rotate_toward_on_a_list_is_l2300() {
    let mut engine = engine();
    let target = named(&engine, "target");
    let id = engine.world_mut().spawn_presentation();
    let world = engine.world_mut();
    world
        .insert(
            id,
            ludion_engine::components::UiList {
                items: vec![ludion_engine::components::UiListItem {
                    text: "A".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .unwrap();
    let mut turning = anchor(target);
    turning.rotate_toward = true;
    world.insert(id, turning).unwrap();
    move_target(&mut engine, [100.0, 0.0, 0.0]);
    step(&mut engine, 1);
    extract(&mut engine);
    extract(&mut engine);
    let reported = engine
        .world()
        .resource::<Diagnostics>()
        .map_or(0, |d| d.reported(code::UI_MOTION_UNSUPPORTED).count());
    assert_eq!(reported, 1);
}
