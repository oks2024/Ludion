//! `UiGroup` (`Plan/ui-hud.md` decisions 18-24): a box other UI elements anchor inside, with its scale,
//! opacity and visibility composed onto them, resolved in extract. Every case extracts through
//! `Engine::extract_present_scene`; none needs a GPU.

use ludion_components::ui::{
    UiAnchorBehind, UiAnchorShow, UiClipShape, UiClock, UiEase, UiGroup, UiTween, UiTweenField,
    UiTweenRepeat, UiTweenTrack, UiWorldAnchor,
};
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::time::{SimTime, TickRate};
use ludion_engine::components::{Anchor, UiList, UiListItem, UiRect, UiText};
use ludion_engine::ui::{resolve_ui_rect, ui_hit};
use ludion_engine::{Diagnostics, Engine, extract_at};
use ludion_renderer::{QualityParams, RenderScene, UiClip, UiInstance, UiRotation};

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

fn engine() -> Engine {
    let mut engine = Engine::builder().build().expect("the game builds");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    engine
}

fn extract(engine: &mut Engine) -> RenderScene {
    engine.extract_present_scene().expect("extracts").0
}

fn spawn<C: ludion_ecs::Component>(engine: &mut Engine, component: C) -> EntityId {
    let id = engine.world_mut().spawn_presentation();
    engine.world_mut().insert(id, component).unwrap();
    id
}

fn add<C: ludion_ecs::Component>(engine: &mut Engine, id: EntityId, component: C) {
    engine.world_mut().insert(id, component).unwrap();
}

fn group(engine: &mut Engine, group: UiGroup) -> EntityId {
    spawn(engine, group)
}

fn child(anchor: Anchor, size: [f32; 2], group: EntityId) -> UiRect {
    UiRect {
        anchor,
        size,
        group: Some(group),
        ..UiRect::default()
    }
}

fn scale(engine: &Engine) -> f32 {
    engine.render_size().1 as f32 / 1080.0
}

fn rects(scene: &RenderScene) -> Vec<([f32; 4], [f32; 4])> {
    scene
        .ui
        .iter()
        .filter_map(|instance| match instance {
            UiInstance::Rect { rect_px, color, .. } => Some((*rect_px, *color)),
            UiInstance::Text { .. } | UiInstance::Shape { .. } => None,
        })
        .collect()
}

fn near(a: [f32; 4], b: [f32; 4]) {
    for (x, y) in a.iter().zip(b) {
        assert!((x - y).abs() < 1e-2, "{a:?} != {b:?}");
    }
}

fn scaled(r: [f32; 4], s: f32) -> [f32; 4] {
    [r[0] * s, r[1] * s, r[2] * s, r[3] * s]
}

fn constant(field: UiTweenField, value: &[f32]) -> UiTween {
    UiTween {
        tracks: vec![UiTweenTrack {
            field,
            from: value.to_vec(),
            to: value.to_vec(),
            start: 0,
            duration: 1.0,
            ease: UiEase::Linear,
            repeat: UiTweenRepeat::Once,
            clock: UiClock::Real,
        }],
    }
}

fn reported(engine: &Engine, which: ludion_core::diagnostics::DiagnosticCode) -> usize {
    engine
        .world()
        .resource::<Diagnostics>()
        .map_or(0, |d| d.reported(which).count())
}

fn top_right_group() -> UiGroup {
    UiGroup {
        anchor: Anchor::TopRight,
        offset: [-24.0, 24.0],
        size: [360.0, 360.0],
        ..UiGroup::default()
    }
}

#[test]
fn a_centred_child_sits_in_the_middle_of_the_group_box() {
    let mut engine = engine();
    let g = group(&mut engine, top_right_group());
    spawn(&mut engine, child(Anchor::Center, [40.0, 40.0], g));
    let ps = scale(&engine);
    let viewport = engine.render_size();
    let b = resolve_ui_rect(Anchor::TopRight, [-24.0, 24.0], [360.0, 360.0], viewport);
    let rows = rects(&extract(&mut engine));
    assert_eq!(rows.len(), 1);
    near(
        rows[0].0,
        [
            b[0] + b[2] * 0.5 - 20.0 * ps,
            b[1] + b[3] * 0.5 - 20.0 * ps,
            40.0 * ps,
            40.0 * ps,
        ],
    );
}

#[test]
fn fill_group_covers_the_box() {
    let mut engine = engine();
    let g = group(&mut engine, top_right_group());
    spawn(
        &mut engine,
        UiRect {
            group: Some(g),
            fill_group: true,
            ..UiRect::default()
        },
    );
    let viewport = engine.render_size();
    let b = resolve_ui_rect(Anchor::TopRight, [-24.0, 24.0], [360.0, 360.0], viewport);
    near(rects(&extract(&mut engine))[0].0, b);
}

fn scaled_child_expectation(engine: &Engine, s: f32) -> [f32; 4] {
    let b = resolve_ui_rect(
        Anchor::TopRight,
        [-24.0, 24.0],
        [360.0, 360.0],
        engine.render_size(),
    );
    let c = [b[0] + b[2] * 0.5, b[1] + b[3] * 0.5];
    // A top-left child of size 100 sits on the box's top-left corner.
    let ps = scale(engine);
    [
        c[0] + (b[0] - c[0]) * s,
        c[1] + (b[1] - c[1]) * s,
        100.0 * ps * s,
        100.0 * ps * s,
    ]
}

#[test]
fn scale_shrinks_children_about_the_box_centre() {
    let mut engine = engine();
    let g = group(
        &mut engine,
        UiGroup {
            scale: 0.5,
            ..top_right_group()
        },
    );
    spawn(&mut engine, child(Anchor::TopLeft, [100.0, 100.0], g));
    let expected = scaled_child_expectation(&engine, 0.5);
    near(rects(&extract(&mut engine))[0].0, expected);
}

#[test]
fn a_tween_scale_on_the_group_moves_every_child() {
    let mut engine = engine();
    let g = group(&mut engine, top_right_group());
    add(&mut engine, g, constant(UiTweenField::Scale, &[0.5]));
    spawn(&mut engine, child(Anchor::TopLeft, [100.0, 100.0], g));
    spawn(&mut engine, child(Anchor::TopLeft, [100.0, 100.0], g));
    let expected = scaled_child_expectation(&engine, 0.5);
    let rows = rects(&extract(&mut engine));
    assert_eq!(rows.len(), 2);
    for (rect, _) in rows {
        near(rect, expected);
    }
}

#[test]
fn opacity_halves_child_alpha_and_visible_false_hides_them() {
    let mut engine = engine();
    let g = group(
        &mut engine,
        UiGroup {
            opacity: 0.5,
            ..top_right_group()
        },
    );
    spawn(&mut engine, child(Anchor::Center, [10.0, 10.0], g));
    assert_eq!(rects(&extract(&mut engine))[0].1[3], 0.5);
    add(
        &mut engine,
        g,
        UiGroup {
            visible: false,
            ..top_right_group()
        },
    );
    assert!(extract(&mut engine).ui.is_empty());
}

#[test]
fn nested_groups_compose_scale_opacity_and_boxes() {
    let mut engine = engine();
    let outer = group(
        &mut engine,
        UiGroup {
            size: [400.0, 200.0],
            scale: 0.5,
            opacity: 0.5,
            ..UiGroup::default()
        },
    );
    let inner = group(
        &mut engine,
        UiGroup {
            anchor: Anchor::Center,
            size: [100.0, 100.0],
            scale: 4.0,
            opacity: 0.5,
            group: Some(outer),
            ..UiGroup::default()
        },
    );
    spawn(&mut engine, child(Anchor::TopLeft, [100.0, 100.0], inner));
    let ps = scale(&engine);
    let rows = rects(&extract(&mut engine));
    // The inner box is (150, 50, 100, 100) in the outer box; scale 0.5 * 4 = 2 about the nested
    // centres leaves the child at (100, 0) with size 200.
    near(rows[0].0, scaled([100.0, 0.0, 200.0, 200.0], ps));
    assert_eq!(rows[0].1[3], 0.25);
}

#[test]
fn a_world_anchor_on_the_group_moves_the_group() {
    let mut engine = engine();
    let target = engine
        .world()
        .entities()
        .nth(1)
        .expect("the scene has a target");
    let anchor = || UiWorldAnchor {
        target,
        world_offset: [0.0; 3],
        show: UiAnchorShow::Always,
        behind: UiAnchorBehind::Clamp,
        margin: 48.0,
        fade: None,
    };
    let g = group(
        &mut engine,
        UiGroup {
            anchor: Anchor::Center,
            size: [40.0, 20.0],
            ..UiGroup::default()
        },
    );
    add(&mut engine, g, anchor());
    spawn(
        &mut engine,
        UiRect {
            group: Some(g),
            fill_group: true,
            order: 1,
            ..UiRect::default()
        },
    );
    let reference = spawn(
        &mut engine,
        UiRect {
            anchor: Anchor::Center,
            size: [40.0, 20.0],
            ..UiRect::default()
        },
    );
    add(&mut engine, reference, anchor());
    let rows = rects(&extract(&mut engine));
    assert_eq!(rows.len(), 2);
    near(rows[0].0, rows[1].0);
    let centre = resolve_ui_rect(Anchor::Center, [0.0; 2], [40.0, 20.0], engine.render_size());
    assert!(rows[0].0[0] > centre[0], "the group followed the target");
}

#[test]
fn a_grouped_element_own_world_anchor_is_l2300() {
    let mut engine = engine();
    let target = engine.world().entities().nth(1).unwrap();
    let g = group(&mut engine, top_right_group());
    let id = spawn(&mut engine, child(Anchor::Center, [40.0, 40.0], g));
    add(
        &mut engine,
        id,
        UiWorldAnchor {
            target,
            world_offset: [0.0; 3],
            show: UiAnchorShow::Always,
            behind: UiAnchorBehind::Clamp,
            margin: 48.0,
            fade: None,
        },
    );
    let b = resolve_ui_rect(
        Anchor::TopRight,
        [-24.0, 24.0],
        [360.0, 360.0],
        engine.render_size(),
    );
    let rows = rects(&extract(&mut engine));
    let ps = scale(&engine);
    near(
        rows[0].0,
        [
            b[0] + b[2] * 0.5 - 20.0 * ps,
            b[1] + b[3] * 0.5 - 20.0 * ps,
            40.0 * ps,
            40.0 * ps,
        ],
    );
    assert_eq!(reported(&engine, code::UI_MOTION_UNSUPPORTED), 1);
}

#[test]
fn a_missing_group_is_l2302_and_draws_ungrouped() {
    let mut engine = engine();
    let plain = engine.world_mut().spawn_presentation();
    spawn(&mut engine, child(Anchor::Center, [40.0, 40.0], plain));
    let rows = rects(&extract(&mut engine));
    near(
        rows[0].0,
        resolve_ui_rect(Anchor::Center, [0.0; 2], [40.0, 40.0], engine.render_size()),
    );
    assert_eq!(reported(&engine, code::UI_GROUP_UNRESOLVED), 1);
}

#[test]
fn a_world_without_groups_extracts_as_before() {
    let mut engine = engine();
    spawn(
        &mut engine,
        UiRect {
            anchor: Anchor::BottomRight,
            offset: [-8.0, -8.0],
            size: [64.0, 32.0],
            ..UiRect::default()
        },
    );
    let rows = rects(&extract(&mut engine));
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].0,
        resolve_ui_rect(
            Anchor::BottomRight,
            [-8.0, -8.0],
            [64.0, 32.0],
            engine.render_size()
        )
    );
    assert_eq!(reported(&engine, code::UI_GROUP_UNRESOLVED), 0);
}

fn first_quad(scene: &RenderScene, index: usize) -> [f32; 4] {
    let mut seen = 0;
    for instance in &scene.ui {
        if let UiInstance::Text { quads, .. } = instance {
            if seen == index {
                return quads[0].pos;
            }
            seen += 1;
        }
    }
    panic!("no text instance {index}");
}

#[test]
fn text_and_list_children_move_with_the_group() {
    let mut engine = engine();
    let g = group(
        &mut engine,
        UiGroup {
            offset: [100.0, 50.0],
            ..UiGroup::default()
        },
    );
    let text = |group: Option<EntityId>, order: i32| UiText {
        text: "A".into(),
        order,
        group,
        ..UiText::default()
    };
    let list = |group: Option<EntityId>, order: i32| UiList {
        items: vec![UiListItem {
            text: "B".into(),
            enabled: true,
        }],
        order,
        group,
        ..UiList::default()
    };
    spawn(&mut engine, text(None, 0));
    spawn(&mut engine, list(None, 1));
    spawn(&mut engine, text(Some(g), 2));
    spawn(&mut engine, list(Some(g), 3));
    let ps = scale(&engine);
    let scene = extract(&mut engine);
    for (plain, grouped) in [(0, 2), (1, 3)] {
        let (a, b) = (first_quad(&scene, plain), first_quad(&scene, grouped));
        near(
            b,
            [
                a[0] + 100.0 * ps,
                a[1] + 50.0 * ps,
                a[2] + 100.0 * ps,
                a[3] + 50.0 * ps,
            ],
        );
    }
}

#[test]
fn a_deep_chain_and_a_cycle_are_refused_by_check_scene() {
    let entity = |group: Option<usize>, component: &str| match group {
        None => format!("  - components:\n      {component}: {{}}\n"),
        Some(i) => format!(
            "  - components:\n      {component}:\n        group:\n          index: {i}\n          generation: 0\n"
        ),
    };
    let diagnostics = |body: String| {
        let scene = ludion_scene::check(
            &format!("format: 1\nentities:\n{body}"),
            &ludion_engine::builtin_registry(),
        )
        .expect("the scene parses");
        ludion_engine::scene_engine_diagnostics(std::path::Path::new("."), &scene)
    };
    let mut deep = entity(None, "UiGroup");
    for i in 1..9 {
        deep.push_str(&entity(Some(i - 1), "UiGroup"));
    }
    deep.push_str(&entity(Some(8), "UiRect"));
    let found = diagnostics(deep);
    assert!(found.iter().any(|d| d.code == code::UI_GROUP_TOO_DEEP));

    let cycle = format!(
        "{}{}",
        entity(Some(1), "UiGroup"),
        entity(Some(0), "UiGroup")
    );
    let found = diagnostics(cycle);
    assert!(found.iter().any(|d| d.code == code::UI_GROUP_CYCLE));
}

fn clips(scene: &RenderScene) -> Vec<UiClip> {
    scene
        .ui
        .iter()
        .map(|instance| match instance {
            UiInstance::Rect { clip, .. }
            | UiInstance::Text { clip, .. }
            | UiInstance::Shape { clip, .. } => *clip,
        })
        .collect()
}

fn clipping(shape: UiClipShape, group: Option<EntityId>) -> UiGroup {
    UiGroup {
        size: [100.0, 100.0],
        clip: shape,
        group,
        ..UiGroup::default()
    }
}

fn now(tick: u64) -> SimTime {
    SimTime::from_ticks(tick, TickRate::DEFAULT)
}

fn hit(engine: &Engine, id: EntityId, cursor: [f32; 2]) -> bool {
    ui_hit(engine.world(), id, engine.render_size(), cursor, now(0))
}

/// The centre is hit, and a point one pixel outside each side is not.
fn assert_box(engine: &Engine, id: EntityId, rect: [f32; 4]) {
    let (cx, cy) = (rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5);
    assert!(hit(engine, id, [cx, cy]), "centre of {rect:?}");
    assert!(
        hit(engine, id, [rect[0] + 0.5, cy]),
        "inside the left edge of {rect:?}"
    );
    for outside in [
        [rect[0] - 1.0, cy],
        [rect[0] + rect[2] + 1.0, cy],
        [cx, rect[1] - 1.0],
        [cx, rect[1] + rect[3] + 1.0],
    ] {
        assert!(!hit(engine, id, outside), "{outside:?} is outside {rect:?}");
    }
}

#[test]
fn a_clipped_groups_children_carry_its_clip_and_the_rest_carry_none() {
    let mut engine = engine();
    let clipped = group(&mut engine, clipping(UiClipShape::Rect, None));
    let plain = group(&mut engine, clipping(UiClipShape::None, None));
    spawn(
        &mut engine,
        UiRect {
            order: 0,
            ..child(Anchor::TopLeft, [50.0, 50.0], clipped)
        },
    );
    spawn(
        &mut engine,
        UiRect {
            order: 1,
            ..child(Anchor::TopLeft, [50.0, 50.0], plain)
        },
    );
    spawn(
        &mut engine,
        UiRect {
            order: 2,
            ..UiRect::default()
        },
    );
    spawn(
        &mut engine,
        UiText {
            text: "A".into(),
            order: 3,
            group: Some(clipped),
            ..UiText::default()
        },
    );
    let ps = scale(&engine);
    let found = clips(&extract(&mut engine));
    assert_eq!(found.len(), 4);
    let own = found[0].boxes[0].expect("the clipped child carries the box");
    assert_eq!(
        (own.rect_px, own.radius_px),
        (scaled([0.0, 0.0, 100.0, 100.0], ps), 0.0)
    );
    assert_eq!(found[0].boxes[1], None);
    assert_eq!(found[1], UiClip::NONE);
    assert_eq!(found[2], UiClip::NONE);
    assert_eq!(found[3], found[0], "a text child carries the same clip");
}

#[test]
fn a_rect_in_a_circle_in_a_rect_carries_both_boxes_inner_first() {
    let mut engine = engine();
    let outer = group(&mut engine, clipping(UiClipShape::Rect, None));
    let inner = group(&mut engine, clipping(UiClipShape::Circle, Some(outer)));
    spawn(&mut engine, child(Anchor::TopLeft, [200.0, 200.0], inner));
    let found = clips(&extract(&mut engine));
    let [inner_box, outer_box] = found[0].boxes.map(|b| b.expect("both boxes"));
    assert_eq!(inner_box.radius_px, 50.0 * scale(&engine));
    assert_eq!(outer_box.radius_px, 0.0);
}

#[test]
fn ui_hit_follows_a_group_scale_tween_at_mid_tween() {
    let mut engine = engine();
    let g = group(&mut engine, top_right_group());
    add(
        &mut engine,
        g,
        UiTween {
            tracks: vec![UiTweenTrack {
                field: UiTweenField::Scale,
                from: vec![1.0],
                to: vec![0.5],
                start: 0,
                duration: 1.0,
                ease: UiEase::Linear,
                repeat: UiTweenRepeat::Once,
                clock: UiClock::Real,
            }],
        },
    );
    let id = spawn(&mut engine, child(Anchor::TopLeft, [100.0, 100.0], g));
    let at = now(30);
    let drawn = extract_at(
        engine.world(),
        engine.render_size(),
        ludion_assets::AssetRoot::content(engine.project_root()),
        at,
        QualityParams::FULL.particle_density,
    )
    .expect("extracts");
    let rect = rects(&drawn)[0].0;
    let full = scaled_child_expectation(&engine, 1.0);
    assert!(
        rect[2] < full[2] && rect[2] > full[2] * 0.5,
        "mid-tween: {rect:?} vs {full:?}"
    );
    let probe = |cursor: [f32; 2]| ui_hit(engine.world(), id, engine.render_size(), cursor, at);
    let (cx, cy) = (rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5);
    assert!(probe([cx, cy]));
    assert!(probe([rect[0] + 0.5, rect[1] + 0.5]));
    assert!(!probe([rect[0] - 1.0, cy]));
    assert!(!probe([rect[0] + rect[2] + 1.0, cy]));
    assert!(!probe([cx, rect[1] + rect[3] + 1.0]));
    // At tick 0 the child is still full size and reaches further up and left than mid-tween.
    let past = [rect[0] - 2.0, cy];
    assert!(ui_hit(
        engine.world(),
        id,
        engine.render_size(),
        past,
        now(0)
    ));
}

#[test]
fn ui_hit_follows_a_top_level_world_anchor() {
    let mut engine = engine();
    let target = engine
        .world()
        .entities()
        .nth(1)
        .expect("the scene has a target");
    let g = group(
        &mut engine,
        UiGroup {
            anchor: Anchor::Center,
            size: [40.0, 20.0],
            ..UiGroup::default()
        },
    );
    add(
        &mut engine,
        g,
        UiWorldAnchor {
            target,
            world_offset: [0.0; 3],
            show: UiAnchorShow::Always,
            behind: UiAnchorBehind::Clamp,
            margin: 48.0,
            fade: None,
        },
    );
    let id = spawn(
        &mut engine,
        UiRect {
            group: Some(g),
            fill_group: true,
            ..UiRect::default()
        },
    );
    let rect = rects(&extract(&mut engine))[0].0;
    let canvas = resolve_ui_rect(Anchor::Center, [0.0; 2], [40.0, 20.0], engine.render_size());
    assert!(rect[0] > canvas[0], "the group followed the target");
    assert_box(&engine, id, rect);
}

#[test]
fn ui_hit_is_false_for_an_invisible_group_dead_entity_and_bad_cursor() {
    let mut engine = engine();
    let g = group(
        &mut engine,
        UiGroup {
            visible: false,
            ..top_right_group()
        },
    );
    let id = spawn(&mut engine, child(Anchor::TopLeft, [100.0, 100.0], g));
    let b = resolve_ui_rect(
        Anchor::TopRight,
        [-24.0, 24.0],
        [360.0, 360.0],
        engine.render_size(),
    );
    let inside = [b[0] + 5.0, b[1] + 5.0];
    assert!(!hit(&engine, id, inside));
    add(&mut engine, g, top_right_group());
    assert!(hit(&engine, id, inside));
    assert!(!hit(&engine, id, [f32::NAN, 5.0]));
    engine.world_mut().despawn(id).unwrap();
    assert!(!hit(&engine, id, inside));
}

#[test]
fn ui_hit_respects_the_clip_for_a_rect_text_and_list() {
    let mut engine = engine();
    let g = group(&mut engine, clipping(UiClipShape::Circle, None));
    let ps = scale(&engine);
    // A rect covering the box: its corner is clipped away, its centre is not.
    let rect = spawn(
        &mut engine,
        UiRect {
            group: Some(g),
            fill_group: true,
            ..UiRect::default()
        },
    );
    assert!(hit(&engine, rect, [50.0 * ps, 50.0 * ps]));
    assert!(
        !hit(&engine, rect, [2.0 * ps, 2.0 * ps]),
        "the box corner is outside the circle"
    );
    let text = spawn(
        &mut engine,
        UiText {
            text: "WWWW".into(),
            size: 20.0,
            anchor: Anchor::Center,
            group: Some(g),
            ..UiText::default()
        },
    );
    assert!(hit(&engine, text, [50.0 * ps, 50.0 * ps]));
    assert!(!hit(&engine, text, [500.0 * ps, 500.0 * ps]));
    let list = spawn(
        &mut engine,
        UiList {
            items: vec![UiListItem {
                text: "B".into(),
                enabled: true,
            }],
            anchor: Anchor::Center,
            group: Some(g),
            ..UiList::default()
        },
    );
    assert!(hit(&engine, list, [50.0 * ps, 50.0 * ps]));
    assert!(!hit(&engine, list, [500.0 * ps, 500.0 * ps]));
}

#[test]
fn three_clipping_levels_report_l2303_statically_and_at_runtime() {
    let entity = |group: Option<usize>, component: &str, clip: bool| {
        if !clip && group.is_none() {
            return format!("  - components:\n      {component}: {{}}\n");
        }
        let mut out = format!("  - components:\n      {component}:\n");
        if clip {
            out.push_str("        clip: rect\n");
        }
        if let Some(i) = group {
            out.push_str(&format!(
                "        group:\n          index: {i}\n          generation: 0\n"
            ));
        }
        out
    };
    let body = format!(
        "{}{}{}{}",
        entity(None, "UiGroup", true),
        entity(Some(0), "UiGroup", true),
        entity(Some(1), "UiGroup", true),
        entity(Some(2), "UiRect", false),
    );
    let scene = ludion_scene::check(
        &format!("format: 1\nentities:\n{body}"),
        &ludion_engine::builtin_registry(),
    )
    .expect("the scene parses");
    let found = ludion_engine::scene_engine_diagnostics(std::path::Path::new("."), &scene);
    let paths: Vec<_> = found
        .iter()
        .filter(|d| d.code == code::UI_CLIP_NESTING)
        .map(|d| d.path.as_deref().unwrap_or(""))
        .collect();
    assert_eq!(
        paths,
        [
            "/entities/2/components/UiGroup/group",
            "/entities/3/components/UiRect/group"
        ]
    );

    let mut engine = engine();
    let a = group(&mut engine, clipping(UiClipShape::Rect, None));
    let b = group(&mut engine, clipping(UiClipShape::Rect, Some(a)));
    let c = group(&mut engine, clipping(UiClipShape::Rect, Some(b)));
    spawn(&mut engine, child(Anchor::TopLeft, [10.0, 10.0], c));
    let found = clips(&extract(&mut engine));
    assert!(found[0].boxes.iter().all(Option::is_some));
    assert_eq!(reported(&engine, code::UI_CLIP_NESTING), 1);
}

// --- rotation (`Plan/ui-hud.md` decisions 45-52) ---

fn rotations(scene: &RenderScene) -> Vec<UiRotation> {
    scene
        .ui
        .iter()
        .map(|instance| match instance {
            UiInstance::Rect { rotation, .. }
            | UiInstance::Text { rotation, .. }
            | UiInstance::Shape { rotation, .. } => *rotation,
        })
        .collect()
}

fn extract_when(engine: &Engine, tick: u64) -> RenderScene {
    extract_at(
        engine.world(),
        engine.render_size(),
        ludion_assets::AssetRoot::content(engine.project_root()),
        now(tick),
        QualityParams::FULL.particle_density,
    )
    .expect("extracts")
}

#[test]
fn rotated_elements_carry_their_rotation_and_unrotated_ones_carry_none() {
    let mut engine = engine();
    spawn(
        &mut engine,
        UiRect {
            offset: [100.0, 100.0],
            size: [40.0, 20.0],
            rotation: 30.0,
            ..UiRect::default()
        },
    );
    spawn(
        &mut engine,
        UiText {
            text: "HI".into(),
            rotation: -15.0,
            order: 1,
            ..UiText::default()
        },
    );
    spawn(
        &mut engine,
        UiRect {
            order: 2,
            ..UiRect::default()
        },
    );
    let scene = extract(&mut engine);
    let turned = rotations(&scene);
    let s = scale(&engine);
    assert_eq!(turned[0].radians, 30.0f32.to_radians());
    assert!((turned[0].pivot_px[0] - 120.0 * s).abs() < 1e-2);
    assert!((turned[0].pivot_px[1] - 110.0 * s).abs() < 1e-2);
    assert_eq!(turned[1].radians, (-15.0f32).to_radians());
    assert_eq!(turned[2], UiRotation::NONE);
}

#[test]
fn a_rotation_tween_adds_to_the_authored_rotation() {
    let mut engine = engine();
    let id = spawn(
        &mut engine,
        UiRect {
            rotation: 10.0,
            ..UiRect::default()
        },
    );
    add(
        &mut engine,
        id,
        UiTween {
            tracks: vec![UiTweenTrack {
                field: UiTweenField::Rotation,
                from: vec![0.0],
                to: vec![40.0],
                start: 0,
                duration: 1.0,
                ease: UiEase::Linear,
                repeat: UiTweenRepeat::Once,
                clock: UiClock::Real,
            }],
        },
    );
    let at = rotations(&extract_when(&engine, 30))[0].radians;
    assert!(
        (at - 30.0f32.to_radians()).abs() < 1e-3,
        "10 authored + 20 at mid-tween, got {at}"
    );
}

#[test]
fn a_rotated_fill_group_child_under_a_rotated_group_sums_the_radians() {
    let mut engine = engine();
    let g = group(
        &mut engine,
        UiGroup {
            rotation: 20.0,
            ..top_right_group()
        },
    );
    spawn(
        &mut engine,
        UiRect {
            fill_group: true,
            group: Some(g),
            rotation: 10.0,
            ..UiRect::default()
        },
    );
    let turned = rotations(&extract(&mut engine));
    assert!((turned[0].radians - 30.0f32.to_radians()).abs() < 1e-5);
}

#[test]
fn ui_hit_follows_a_rotated_rect() {
    let mut engine = engine();
    let id = spawn(
        &mut engine,
        UiRect {
            anchor: Anchor::Center,
            size: [200.0, 20.0],
            rotation: 90.0,
            ..UiRect::default()
        },
    );
    let s = scale(&engine);
    let (w, h) = engine.render_size();
    let (cx, cy) = (w as f32 * 0.5, h as f32 * 0.5);
    assert!(
        hit(&engine, id, [cx, cy - 50.0 * s]),
        "on the turned long axis"
    );
    assert!(
        !hit(&engine, id, [cx + 50.0 * s, cy]),
        "where the unrotated rect would be"
    );
    assert!(!hit(&engine, id, [cx + 90.0 * s, cy]));
}

#[test]
fn ui_hit_misses_a_child_cut_by_a_rotated_clip() {
    let mut engine = engine();
    let g = group(
        &mut engine,
        UiGroup {
            anchor: Anchor::Center,
            size: [200.0, 200.0],
            clip: UiClipShape::Rect,
            rotation: 45.0,
            ..UiGroup::default()
        },
    );
    let id = spawn(&mut engine, child(Anchor::Center, [400.0, 400.0], g));
    let s = scale(&engine);
    let (w, h) = engine.render_size();
    let (cx, cy) = (w as f32 * 0.5, h as f32 * 0.5);
    assert!(
        hit(&engine, id, [cx - 60.0 * s, cy - 60.0 * s]),
        "inside the diamond"
    );
    assert!(
        !hit(&engine, id, [cx - 95.0 * s, cy - 95.0 * s]),
        "inside the unturned box, outside the turned one"
    );
}

fn filled_wheel(group: EntityId) -> ludion_engine::components::UiRadial {
    ludion_engine::components::UiRadial {
        anchor: Anchor::Center,
        group: Some(group),
        items: vec![ludion_engine::components::UiRadialItem::default(); 4],
        fill: Some([0.2, 0.4, 0.6, 0.8]),
        ..Default::default()
    }
}

fn wedges(scene: &RenderScene) -> Vec<&UiInstance> {
    scene
        .ui
        .iter()
        .filter(|i| matches!(i, UiInstance::Shape { .. }))
        .collect()
}

#[test]
fn a_filled_wheel_in_a_rotated_group_turns_every_wedge_about_the_wheel_centre() {
    let mut engine = engine();
    let g = group(
        &mut engine,
        UiGroup {
            rotation: 30.0,
            ..top_right_group()
        },
    );
    spawn(&mut engine, filled_wheel(g));
    let scene = extract(&mut engine);
    let b = resolve_ui_rect(
        Anchor::TopRight,
        [-24.0, 24.0],
        [360.0, 360.0],
        engine.render_size(),
    );
    let centre = [b[0] + b[2] * 0.5, b[1] + b[3] * 0.5];
    let drawn = wedges(&scene);
    assert_eq!(drawn.len(), 4);
    for wedge in drawn {
        let UiInstance::Shape {
            origin_px,
            rotation,
            ..
        } = wedge
        else {
            unreachable!()
        };
        assert!((rotation.radians - 30.0f32.to_radians()).abs() < 1e-5);
        assert_eq!(rotation.pivot_px, *origin_px);
        assert!((origin_px[0] - centre[0]).abs() < 1e-2 && (origin_px[1] - centre[1]).abs() < 1e-2);
    }
}

#[test]
fn a_group_opacity_tween_multiplies_the_wedge_fill_alpha() {
    let mut engine = engine();
    let g = group(&mut engine, top_right_group());
    add(&mut engine, g, constant(UiTweenField::Opacity, &[0.5]));
    spawn(&mut engine, filled_wheel(g));
    let scene = extract(&mut engine);
    let drawn = wedges(&scene);
    assert_eq!(drawn.len(), 4);
    for wedge in drawn {
        let UiInstance::Shape { fill, .. } = wedge else {
            unreachable!()
        };
        assert_eq!(fill.map(|f| f[3]), Some(0.4));
    }
}
