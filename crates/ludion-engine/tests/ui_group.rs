//! `UiGroup` (`Plan/ui-hud.md` decisions 18-24): a box other UI elements anchor inside, with its scale,
//! opacity and visibility composed onto them, resolved in extract. Every case extracts through
//! `Engine::extract_present_scene`; none needs a GPU.

use ludion_components::ui::{
    UiAnchorBehind, UiAnchorShow, UiClock, UiEase, UiGroup, UiTween, UiTweenField, UiTweenRepeat,
    UiTweenTrack, UiWorldAnchor,
};
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_engine::components::{Anchor, UiList, UiListItem, UiRect, UiText};
use ludion_engine::ui::resolve_ui_rect;
use ludion_engine::{Diagnostics, Engine};
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
            UiInstance::Text { .. } => None,
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
