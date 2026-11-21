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
use ludion_engine::components::{Anchor, UiRect, UiShape, UiShapeType, UiStroke};
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
