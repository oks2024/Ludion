//! `UiShape` through `extract` and `ui_hit` (`Plan/ui-hud.md` decisions 53-61, Neon Shroud request 0020
//! W3b): one `UiInstance::Shape` per visible shape in `(order, EntityId)` order, tweens and groups
//! composed about the shape's origin, and a hit test that follows the draw. None of it needs a GPU; the
//! pixel agreement lives in `ui_clip_draw.rs`.

use ludion_components::ui::{
    UiClipShape, UiClock, UiEase, UiGroup, UiTween, UiTweenField, UiTweenRepeat, UiTweenTrack,
};
use ludion_core::ids::EntityId;
use ludion_core::time::{SimTime, TickRate};
use ludion_engine::Engine;
use ludion_engine::components::{
    Anchor, UiDash, UiLineCap, UiLineJoin, UiRect, UiShape, UiShapeType, UiStroke,
};
use ludion_engine::ui::{ui_hit, ui_tween_sample};
use ludion_protocol::Command;
use ludion_protocol::command::SimStepParams;
use ludion_renderer::{RenderScene, UiClip, UiInstance, UiRotation, UiShapeGeometry};

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
";

fn engine() -> Engine {
    let mut engine = Engine::builder().build().expect("the engine builds");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    engine
}

fn extract(engine: &mut Engine) -> RenderScene {
    engine.extract_present_scene().expect("extracts").0
}

fn step(engine: &mut Engine, ticks: u64) {
    engine
        .execute(Command::SimStep(SimStepParams { ticks, hash: false }))
        .expect("the sim steps");
}

fn disc(radius: f32, order: i32) -> UiShape {
    UiShape {
        anchor: Anchor::Center,
        shape: UiShapeType::Circle { radius },
        fill: Some([1.0; 4]),
        order,
        ..UiShape::default()
    }
}

fn ring() -> UiShape {
    UiShape {
        anchor: Anchor::Center,
        shape: UiShapeType::Ring {
            radius: 100.0,
            thickness: 40.0,
            start_deg: 0.0,
            sweep_deg: 360.0,
        },
        stroke: Some(UiStroke {
            width: 8.0,
            color: [1.0; 4],
        }),
        ..UiShape::default()
    }
}

fn spawn(engine: &mut Engine, shape: UiShape) -> EntityId {
    let id = engine.world_mut().spawn_presentation();
    engine.world_mut().insert(id, shape).unwrap();
    id
}

fn track(field: UiTweenField, from: &[f32], to: &[f32]) -> UiTween {
    UiTween {
        tracks: vec![UiTweenTrack {
            field,
            from: from.to_vec(),
            to: to.to_vec(),
            start: 0,
            duration: 1.0,
            ease: UiEase::Linear,
            repeat: UiTweenRepeat::Once,
            clock: UiClock::Real,
        }],
    }
}

fn shapes(scene: &RenderScene) -> Vec<&UiInstance> {
    scene
        .ui
        .iter()
        .filter(|instance| matches!(instance, UiInstance::Shape { .. }))
        .collect()
}

#[test]
fn one_shape_per_visible_shape_in_order_between_rects() {
    let mut engine = engine();
    spawn(
        &mut engine,
        UiShape {
            visible: false,
            ..disc(10.0, 0)
        },
    );
    spawn(&mut engine, disc(30.0, 5));
    spawn(&mut engine, disc(20.0, 1));
    let behind = engine.world_mut().spawn_presentation();
    engine
        .world_mut()
        .insert(
            behind,
            UiRect {
                order: 0,
                ..UiRect::default()
            },
        )
        .unwrap();
    let above = engine.world_mut().spawn_presentation();
    engine
        .world_mut()
        .insert(
            above,
            UiRect {
                order: 3,
                ..UiRect::default()
            },
        )
        .unwrap();
    // A shape with neither fill nor stroke draws nothing, silently.
    spawn(
        &mut engine,
        UiShape {
            fill: None,
            ..disc(5.0, 2)
        },
    );

    let scene = extract(&mut engine);
    let kinds: Vec<(&str, f32)> = scene
        .ui
        .iter()
        .map(|instance| match instance {
            UiInstance::Rect { .. } => ("rect", 0.0),
            UiInstance::Shape {
                geometry: UiShapeGeometry::Sector { radius_outer, .. },
                ..
            } => ("shape", *radius_outer),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    let ps = engine.render_size().1 as f32 / 1080.0;
    assert_eq!(
        kinds,
        vec![
            ("rect", 0.0),
            ("shape", 20.0 * ps),
            ("rect", 0.0),
            ("shape", 30.0 * ps),
        ]
    );
}

#[test]
fn a_scale_tween_scales_the_radius_about_the_origin() {
    let mut engine = engine();
    let id = spawn(&mut engine, disc(50.0, 0));
    let tween = track(UiTweenField::Scale, &[1.0], &[3.0]);
    engine.world_mut().insert(id, tween.clone()).unwrap();
    step(&mut engine, 30);
    let frame = engine.frame().as_u64();
    let k = ui_tween_sample(&tween, UiTweenField::Scale, frame, frame, TickRate::DEFAULT)
        .expect("the track is live")[0];
    assert!(k > 1.0 && k < 3.0, "mid-way: {k}");
    let scene = extract(&mut engine);
    let ps = engine.render_size().1 as f32 / 1080.0;
    let (w, h) = engine.render_size();
    let [
        UiInstance::Shape {
            origin_px,
            geometry: UiShapeGeometry::Sector { radius_outer, .. },
            ..
        },
    ] = shapes(&scene)[..]
    else {
        panic!("one disc");
    };
    assert_eq!(*origin_px, [w as f32 * 0.5, h as f32 * 0.5]);
    assert!(
        (radius_outer - 50.0 * ps * k).abs() < 1e-2,
        "{radius_outer}"
    );
}

#[test]
fn a_rotation_tween_adds_to_the_authored_rotation_about_the_origin() {
    let mut engine = engine();
    let id = spawn(
        &mut engine,
        UiShape {
            rotation: 30.0,
            ..disc(50.0, 0)
        },
    );
    let tween = track(UiTweenField::Rotation, &[0.0], &[60.0]);
    engine.world_mut().insert(id, tween.clone()).unwrap();
    step(&mut engine, 30);
    let frame = engine.frame().as_u64();
    let turn = ui_tween_sample(
        &tween,
        UiTweenField::Rotation,
        frame,
        frame,
        TickRate::DEFAULT,
    )
    .expect("the track is live")[0];
    let scene = extract(&mut engine);
    let [
        UiInstance::Shape {
            origin_px,
            rotation,
            ..
        },
    ] = shapes(&scene)[..]
    else {
        panic!("one disc");
    };
    assert!((rotation.radians - (30.0 + turn).to_radians()).abs() < 1e-5);
    assert_eq!(rotation.pivot_px, *origin_px);
}

#[test]
fn a_grouped_shape_takes_the_groups_clip() {
    let mut engine = engine();
    let group = engine.world_mut().spawn_presentation();
    engine
        .world_mut()
        .insert(
            group,
            UiGroup {
                offset: [100.0, 100.0],
                size: [200.0, 200.0],
                clip: UiClipShape::Circle,
                ..UiGroup::default()
            },
        )
        .unwrap();
    spawn(
        &mut engine,
        UiShape {
            group: Some(group),
            ..disc(60.0, 0)
        },
    );
    let scene = extract(&mut engine);
    let [UiInstance::Shape { clip, rotation, .. }] = shapes(&scene)[..] else {
        panic!("one disc");
    };
    assert_ne!(*clip, UiClip::NONE);
    assert!(clip.boxes[0].is_some());
    assert_eq!(*rotation, UiRotation::NONE);
}

#[test]
fn ui_hit_is_true_on_a_stroked_rings_band_and_false_at_its_centre() {
    let mut engine = engine();
    let id = spawn(&mut engine, ring());
    let (w, h) = engine.render_size();
    let ps = h as f32 / 1080.0;
    let centre = [w as f32 * 0.5, h as f32 * 0.5];
    let now = SimTime::from_ticks(0, TickRate::DEFAULT);
    let at = |dx: f32| [centre[0] + dx * ps, centre[1]];
    // The stroke is centred on the outer edge (radius 100) and the inner edge (radius 60).
    assert!(ui_hit(
        engine.world(),
        id,
        engine.render_size(),
        at(100.0),
        now
    ));
    assert!(ui_hit(
        engine.world(),
        id,
        engine.render_size(),
        at(60.0),
        now
    ));
    assert!(!ui_hit(
        engine.world(),
        id,
        engine.render_size(),
        at(80.0),
        now
    ));
    assert!(!ui_hit(
        engine.world(),
        id,
        engine.render_size(),
        centre,
        now
    ));
}

#[test]
fn ui_hit_follows_a_filled_shape_through_its_group_and_clip() {
    let mut engine = engine();
    let group = engine.world_mut().spawn_presentation();
    engine
        .world_mut()
        .insert(
            group,
            UiGroup {
                offset: [100.0, 100.0],
                size: [200.0, 200.0],
                clip: UiClipShape::Circle,
                ..UiGroup::default()
            },
        )
        .unwrap();
    let id = spawn(
        &mut engine,
        UiShape {
            group: Some(group),
            anchor: Anchor::TopLeft,
            ..disc(150.0, 0)
        },
    );
    let viewport = engine.render_size();
    let ps = viewport.1 as f32 / 1080.0;
    let now = SimTime::from_ticks(0, TickRate::DEFAULT);
    // The disc is centred on the group's top-left corner; the group's circle clip keeps a quadrant of it.
    let inside_both = [(150.0) * ps, 150.0 * ps];
    let outside_clip = [(100.0 + 5.0) * ps, 105.0 * ps];
    assert!(ui_hit(engine.world(), id, viewport, inside_both, now));
    assert!(!ui_hit(engine.world(), id, viewport, outside_clip, now));
}

#[test]
fn a_world_without_shapes_extracts_only_what_it_always_did() {
    let mut engine = engine();
    let id = engine.world_mut().spawn_presentation();
    engine
        .world_mut()
        .insert(
            id,
            UiRect {
                offset: [10.0, 20.0],
                size: [30.0, 40.0],
                color: [0.2, 0.4, 0.6, 0.8],
                ..UiRect::default()
            },
        )
        .unwrap();
    let scene = extract(&mut engine);
    let ps = engine.render_size().1 as f32 / 1080.0;
    assert_eq!(
        scene.ui,
        vec![UiInstance::Rect {
            rect_px: [10.0 * ps, 20.0 * ps, 30.0 * ps, 40.0 * ps],
            texture: None,
            color: [0.2, 0.4, 0.6, 0.8],
            uv: [0.0, 0.0, 1.0, 1.0],
            clip: UiClip::NONE,
            view: None,
            rotation: UiRotation::NONE,
        }]
    );
}

// --- polygons (`Plan/ui-hud.md` decisions 67-73) ---

fn polygon(points: &[[f32; 2]], order: i32) -> UiShape {
    UiShape {
        shape: UiShapeType::Polygon {
            points: points.to_vec(),
            join: UiLineJoin::default(),
        },
        fill: Some([1.0; 4]),
        offset: [100.0, 100.0],
        order,
        ..UiShape::default()
    }
}

/// A "U": the notch between its arms is inside the convex hull but outside the polygon.
const NOTCHED: [[f32; 2]; 8] = [
    [0.0, 0.0],
    [100.0, 0.0],
    [100.0, 200.0],
    [70.0, 200.0],
    [70.0, 50.0],
    [30.0, 50.0],
    [30.0, 200.0],
    [0.0, 200.0],
];

/// A bow-tie whose edges cross and whose signed area is not zero.
const BOW_TIE: [[f32; 2]; 4] = [[0.0, 0.0], [100.0, 100.0], [100.0, 0.0], [0.0, 120.0]];

fn polygons(scene: &RenderScene) -> Vec<&UiInstance> {
    scene
        .ui
        .iter()
        .filter(|instance| matches!(instance, UiInstance::Polygon { .. }))
        .collect()
}

#[test]
fn a_polygon_emits_one_instance_in_order_and_scales_its_points() {
    let mut engine = engine();
    spawn(&mut engine, polygon(&NOTCHED, 4));
    spawn(&mut engine, disc(10.0, 1));
    let scene = extract(&mut engine);
    let ps = engine.render_size().1 as f32 / 1080.0;
    assert!(matches!(scene.ui[0], UiInstance::Shape { .. }));
    let UiInstance::Polygon {
        origin_px,
        points,
        triangles,
        ..
    } = &scene.ui[1]
    else {
        panic!("the polygon draws after the lower-order disc");
    };
    assert_eq!(*origin_px, [100.0 * ps, 100.0 * ps]);
    assert_eq!(points.len(), NOTCHED.len());
    assert_eq!(points[1], [100.0 * ps, 0.0]);
    assert_eq!(triangles.len(), NOTCHED.len() - 2);
    assert_eq!(polygons(&scene).len(), 1);
}

#[test]
fn a_bow_tie_draws_nothing_and_warns_once() {
    let mut engine = engine();
    spawn(&mut engine, polygon(&BOW_TIE, 0));
    for _ in 0..3 {
        let scene = extract(&mut engine);
        assert!(polygons(&scene).is_empty());
        assert!(scene.ui.is_empty());
    }
    let reported = engine
        .world()
        .resource::<ludion_engine::Diagnostics>()
        .map_or(0, |d| {
            d.reported(ludion_core::codes::code::UI_POLYGON_DEGENERATE)
                .count()
        });
    assert_eq!(reported, 1);
}

#[test]
fn ui_hit_is_true_inside_a_concave_polygon_and_false_in_its_notch() {
    let mut engine = engine();
    let id = spawn(&mut engine, polygon(&NOTCHED, 0));
    let ps = engine.render_size().1 as f32 / 1080.0;
    let now = SimTime::from_ticks(0, TickRate::DEFAULT);
    let at = |x: f32, y: f32| [(100.0 + x) * ps, (100.0 + y) * ps];
    let hit = |p: [f32; 2]| ui_hit(engine.world(), id, engine.render_size(), p, now);
    assert!(hit(at(10.0, 100.0)), "the left arm");
    assert!(hit(at(50.0, 20.0)), "the bridge");
    assert!(!hit(at(50.0, 120.0)), "the notch");
    assert!(!hit(at(-10.0, 100.0)), "outside");
}

#[test]
fn a_degenerate_polygon_never_hits() {
    let mut engine = engine();
    let id = spawn(&mut engine, polygon(&BOW_TIE, 0));
    let ps = engine.render_size().1 as f32 / 1080.0;
    let now = SimTime::from_ticks(0, TickRate::DEFAULT);
    assert!(!ui_hit(
        engine.world(),
        id,
        engine.render_size(),
        [(100.0 + 90.0) * ps, (100.0 + 20.0) * ps],
        now
    ));
}

#[test]
fn a_polygon_follows_its_groups_scale_and_rotation() {
    let mut engine = engine();
    let group = engine.world_mut().spawn_presentation();
    engine
        .world_mut()
        .insert(
            group,
            UiGroup {
                size: [400.0, 400.0],
                scale: 2.0,
                rotation: 30.0,
                ..UiGroup::default()
            },
        )
        .unwrap();
    let mut shape = polygon(&NOTCHED, 0);
    shape.group = Some(group);
    spawn(&mut engine, shape);
    let scene = extract(&mut engine);
    let ps = engine.render_size().1 as f32 / 1080.0;
    let UiInstance::Polygon {
        points, rotation, ..
    } = &scene.ui[0]
    else {
        panic!("a grouped polygon draws");
    };
    assert_eq!(points[1], [100.0 * ps * 2.0, 0.0]);
    assert!((rotation.radians - 30.0f32.to_radians()).abs() < 1e-5);
}

// --- strokes and polylines (`Plan/ui-hud.md` decisions 74-80) ---

const ZIGZAG: [[f32; 2]; 4] = [[0.0, 0.0], [60.0, 0.0], [60.0, 60.0], [120.0, 60.0]];

fn polyline(points: &[[f32; 2]], width: f32) -> UiShape {
    UiShape {
        shape: UiShapeType::Polyline {
            points: points.to_vec(),
            closed: false,
            join: UiLineJoin::default(),
            cap: UiLineCap::Butt,
        },
        stroke: Some(UiStroke {
            width,
            color: [1.0, 0.5, 0.25, 1.0],
        }),
        offset: [100.0, 100.0],
        ..UiShape::default()
    }
}

fn reported_degenerate(engine: &Engine) -> usize {
    engine
        .world()
        .resource::<ludion_engine::Diagnostics>()
        .map_or(0, |d| {
            d.reported(ludion_core::codes::code::UI_POLYGON_DEGENERATE)
                .count()
        })
}

#[test]
fn a_polyline_emits_one_stroke_only_open_instance() {
    let mut engine = engine();
    spawn(&mut engine, polyline(&ZIGZAG, 10.0));
    let scene = extract(&mut engine);
    let ps = engine.render_size().1 as f32 / 1080.0;
    let UiInstance::Polygon {
        points,
        closed,
        triangles,
        fill,
        stroke,
        ..
    } = &scene.ui[0]
    else {
        panic!("a polyline draws as a path");
    };
    assert_eq!(points.len(), ZIGZAG.len());
    assert!(!closed);
    assert!(triangles.is_empty());
    assert!(fill.is_none());
    assert_eq!(stroke.map(|s| s.width_px), Some(10.0 * ps));
}

#[test]
fn a_polygon_with_a_stroke_keeps_its_fill_triangles_and_a_stroke_only_one_has_none() {
    let mut engine = engine();
    let mut both = polygon(&NOTCHED, 0);
    both.stroke = Some(UiStroke {
        width: 6.0,
        color: [0.0, 1.0, 0.0, 1.0],
    });
    let mut outline = both.clone();
    outline.fill = None;
    outline.order = 1;
    spawn(&mut engine, both);
    spawn(&mut engine, outline);
    let scene = extract(&mut engine);
    let paths = polygons(&scene);
    assert_eq!(paths.len(), 2);
    let UiInstance::Polygon {
        triangles,
        fill,
        stroke,
        closed,
        ..
    } = paths[0]
    else {
        unreachable!()
    };
    assert_eq!(triangles.len(), NOTCHED.len() - 2);
    assert!(fill.is_some() && stroke.is_some() && *closed);
    let UiInstance::Polygon {
        triangles, fill, ..
    } = paths[1]
    else {
        unreachable!()
    };
    assert!(triangles.is_empty() && fill.is_none());
}

#[test]
fn a_one_point_polyline_draws_nothing_and_warns_once() {
    let mut engine = engine();
    spawn(&mut engine, polyline(&[[5.0, 5.0], [5.0, 5.0]], 4.0));
    for _ in 0..3 {
        let scene = extract(&mut engine);
        assert!(scene.ui.is_empty());
    }
    assert_eq!(reported_degenerate(&engine), 1);
}

#[test]
fn a_stroked_bow_tie_still_warns_once() {
    let mut engine = engine();
    let mut bow = polygon(&BOW_TIE, 0);
    bow.fill = None;
    bow.stroke = Some(UiStroke {
        width: 4.0,
        color: [1.0; 4],
    });
    spawn(&mut engine, bow);
    for _ in 0..3 {
        assert!(extract(&mut engine).ui.is_empty());
    }
    assert_eq!(reported_degenerate(&engine), 1);
}

#[test]
fn ui_hit_follows_a_polyline_band_and_a_group_scales_its_width() {
    let mut engine = engine();
    let id = spawn(&mut engine, polyline(&ZIGZAG, 10.0));
    let ps = engine.render_size().1 as f32 / 1080.0;
    let now = SimTime::from_ticks(0, TickRate::DEFAULT);
    let at = |x: f32, y: f32| [(100.0 + x) * ps, (100.0 + y) * ps];
    let hit = |p: [f32; 2]| ui_hit(engine.world(), id, engine.render_size(), p, now);
    assert!(hit(at(30.0, 3.0)), "on the first leg");
    assert!(hit(at(60.0, 30.0)), "on the riser");
    assert!(!hit(at(30.0, 12.0)), "beside the band");
    assert!(!hit(at(30.0, 30.0)), "inside the zigzag");

    let group = engine.world_mut().spawn_presentation();
    engine
        .world_mut()
        .insert(
            group,
            UiGroup {
                size: [400.0, 400.0],
                scale: 2.0,
                ..UiGroup::default()
            },
        )
        .unwrap();
    let mut shape = polyline(&ZIGZAG, 10.0);
    shape.group = Some(group);
    spawn(&mut engine, shape);
    let scene = extract(&mut engine);
    let stroke = scene
        .ui
        .iter()
        .find_map(|i| match i {
            UiInstance::Polygon { stroke, .. }
                if stroke.is_some_and(|s| s.width_px > 10.0 * ps * 1.5) =>
            {
                *stroke
            }
            _ => None,
        })
        .expect("the grouped polyline's stroke doubles");
    assert!((stroke.width_px - 20.0 * ps).abs() < 1e-3);
}

// --- dashes (`Plan/ui-hud.md` decisions 81-87) ---

fn reported_dense(engine: &Engine) -> usize {
    engine
        .world()
        .resource::<ludion_engine::Diagnostics>()
        .map_or(0, |d| {
            d.reported(ludion_core::codes::code::UI_DASH_TOO_DENSE)
                .count()
        })
}

#[test]
fn a_dashed_polyline_emits_one_path_with_its_dash_in_pixels() {
    let mut engine = engine();
    let mut dashed = polyline(&ZIGZAG, 10.0);
    dashed.dash = Some(UiDash { on: 12.0, off: 8.0 });
    spawn(&mut engine, dashed);
    let scene = extract(&mut engine);
    let ps = engine.render_size().1 as f32 / 1080.0;
    assert_eq!(scene.ui.len(), 1);
    let UiInstance::Polygon { stroke, .. } = &scene.ui[0] else {
        panic!("a polyline draws as a path");
    };
    assert_eq!(stroke.and_then(|s| s.dash), Some([12.0 * ps, 8.0 * ps]));
}

#[test]
fn a_dashed_ring_emits_its_outlines_under_one_order() {
    let mut engine = engine();
    let mut dashed = ring();
    dashed.fill = Some([0.0, 1.0, 0.0, 1.0]);
    dashed.dash = Some(UiDash { on: 10.0, off: 6.0 });
    spawn(&mut engine, dashed);
    let scene = extract(&mut engine);
    let kinds: Vec<&str> = scene
        .ui
        .iter()
        .map(|row| match row {
            UiInstance::Shape { stroke, .. } => {
                assert!(stroke.is_none(), "the fill row carries no stroke");
                "shape"
            }
            UiInstance::Polygon { stroke, closed, .. } => {
                assert!(*closed && stroke.is_some_and(|s| s.dash.is_some()));
                "outline"
            }
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["shape", "outline", "outline"]);
}

#[test]
fn a_too_dense_dash_draws_solid_and_warns_once() {
    let mut engine = engine();
    let mut dense = ring();
    dense.dash = Some(UiDash { on: 0.1, off: 0.1 });
    spawn(&mut engine, dense);
    for _ in 0..3 {
        let scene = extract(&mut engine);
        assert!(matches!(
            scene.ui.as_slice(),
            [UiInstance::Shape {
                stroke: Some(_),
                ..
            }]
        ));
    }
    assert_eq!(reported_dense(&engine), 1);
}

#[test]
fn ui_hit_misses_in_a_dash_gap() {
    let mut engine = engine();
    let mut dashed = polyline(&[[0.0, 0.0], [200.0, 0.0]], 10.0);
    dashed.dash = Some(UiDash {
        on: 20.0,
        off: 20.0,
    });
    let id = spawn(&mut engine, dashed);
    let ps = engine.render_size().1 as f32 / 1080.0;
    let now = SimTime::from_ticks(0, TickRate::DEFAULT);
    let hit = |x: f32| {
        ui_hit(
            engine.world(),
            id,
            engine.render_size(),
            [(100.0 + x) * ps, 100.0 * ps],
            now,
        )
    };
    assert!(hit(10.0), "on a dash");
    assert!(!hit(30.0), "in a gap");
    assert!(hit(50.0), "on the next dash");
}
