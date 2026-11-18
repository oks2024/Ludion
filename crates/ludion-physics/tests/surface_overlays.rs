//! `SurfaceOverlay`: a sensor volume that overrides the surface of every hit whose point lies inside
//! it (highest priority, ties to the lowest entity); `surface_at` and `surface_under` give one shared
//! answer; an undeclared name is one `L1690` and an inert overlay; a restore answers before any write.

mod common;

use std::sync::{Arc, Mutex};

use common::{TestPose, pose_of, tick};
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, SystemCtx, World};
use ludion_physics::{
    CharacterMotor, NoParent, PhysicsError, PhysicsSettings, PhysicsSpace, PhysicsWorld,
    QueryFilter, ShapeRejection, SurfaceId, SurfaceOverlay, SurfaceTable, SurfaceTag, install,
    move_character,
};
use ludion_spatial::{Body, Collider, Shape};

const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const CONCRETE: SurfaceId = SurfaceId(0);
const PUDDLE: SurfaceId = SurfaceId(1);
const CARPET: SurfaceId = SurfaceId(2);

/// Each step's x position and the ground surface the motor reported there.
type SurfaceLog = Arc<Mutex<Vec<(f32, Option<SurfaceId>)>>>;

fn table() -> SurfaceTable {
    SurfaceTable::new(
        vec!["concrete".into(), "puddle".into(), "carpet".into()],
        None,
    )
}

fn settings() -> PhysicsSettings {
    PhysicsSettings {
        surfaces: Some(table()),
        ..PhysicsSettings::new(PhysicsSpace::Free)
    }
}

fn stepped() -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose, NoParent>(&mut schedule, settings());
    schedule
}

fn run(world: &mut World, schedule: &mut Schedule, frames: std::ops::Range<u64>) {
    for frame in frames {
        tick(world, schedule, frame);
    }
}

fn physics(world: &World) -> &PhysicsWorld {
    world.resource::<PhysicsWorld>().unwrap()
}

fn rejections(world: &mut World) -> Vec<ShapeRejection> {
    world
        .resource_mut::<PhysicsWorld>()
        .unwrap()
        .take_rejections()
}

/// A concrete floor strip, `x` in `[0, 8]`, top at `y = 0`.
fn floor(world: &mut World) -> EntityId {
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at([4.0, -0.5, 0.0]))
        .unwrap();
    world
        .insert(
            entity,
            Collider {
                shape: Shape::Box {
                    half_extents: [4.0, 0.5, 2.0],
                },
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .unwrap();
    world
        .insert(
            entity,
            SurfaceTag {
                surface: "concrete".to_string(),
            },
        )
        .unwrap();
    entity
}

/// A sensor cylinder (`radius`, half height 0.05) centred on the floor's top face at `x`.
fn overlay(
    world: &mut World,
    x: f32,
    radius: f32,
    surface: &str,
    priority: i32,
    body: Body,
) -> EntityId {
    let entity = world.spawn();
    world.insert(entity, TestPose::at([x, 0.0, 0.0])).unwrap();
    world
        .insert(
            entity,
            Collider {
                shape: Shape::Cylinder {
                    half_height: 0.05,
                    radius,
                },
                body,
                layer: None,
                sensor: true,
            },
        )
        .unwrap();
    world
        .insert(
            entity,
            SurfaceOverlay {
                surface: surface.to_string(),
                priority,
            },
        )
        .unwrap();
    entity
}

fn down(physics: &PhysicsWorld, x: f32) -> ludion_physics::QueryHit {
    physics
        .ray_cast([x, 2.0, 0.0], [0.0, -1.0, 0.0], 5.0, &QueryFilter::new())
        .unwrap()
        .remove(0)
}

#[test]
fn a_motor_walking_across_a_puddle_reports_concrete_puddle_concrete() {
    let mut world = World::new();
    floor(&mut world);
    overlay(&mut world, 4.0, 1.0, "puddle", 0, Body::new_static());
    let hero = world.spawn();
    world.insert(hero, TestPose::at([0.5, 0.0, 0.0])).unwrap();
    world
        .insert(
            hero,
            CharacterMotor {
                radius: 0.28,
                height: 0.9,
                slope_limit_deg: 45.0,
                step_height: 0.25,
                skin_width: 0.01,
                ..CharacterMotor::default()
            },
        )
        .unwrap();
    let log: SurfaceLog = Arc::default();
    let sink = Arc::clone(&log);
    let mut schedule = Schedule::new();
    schedule.add_system_shared(
        "walk",
        Arc::new(move |world: &mut World, _: &SystemCtx| {
            if let Ok(report) = move_character::<TestPose, NoParent>(world, hero, [0.1, -0.1, 0.0])
            {
                let x = pose_of(world, hero).translation[0];
                sink.lock()
                    .unwrap()
                    .push((x, report.ground.map(|g| g.surface).unwrap_or(None)));
            }
        }),
    );
    install::<TestPose, NoParent>(&mut schedule, settings());
    run(&mut world, &mut schedule, 0..70);

    let log = log.lock().unwrap();
    let mut seen = Vec::new();
    for &(x, surface) in log.iter() {
        // Near an edge the capsule's footprint reaches both patches: skip, the rest must agree.
        if [3.0, 5.0].iter().any(|edge| (x - edge).abs() < 0.4) {
            continue;
        }
        let want = if (3.0..5.0).contains(&x) {
            PUDDLE
        } else {
            CONCRETE
        };
        assert_eq!(surface, Some(want), "at x = {x}");
        if seen.last() != Some(&surface) {
            seen.push(surface);
        }
    }
    assert_eq!(seen, [Some(CONCRETE), Some(PUDDLE), Some(CONCRETE)]);
}

#[test]
fn overlapping_overlays_resolve_by_priority_then_lowest_entity() {
    let mut world = World::new();
    floor(&mut world);
    let first = overlay(&mut world, 4.0, 1.0, "puddle", 0, Body::new_static());
    let second = overlay(&mut world, 4.0, 1.0, "carpet", 0, Body::new_static());
    let mut schedule = stepped();
    run(&mut world, &mut schedule, 0..2);
    assert_eq!(
        down(physics(&world), 4.0).surface,
        Some(PUDDLE),
        "equal priority: the lower entity wins"
    );

    world
        .get_mut::<SurfaceOverlay>(second)
        .unwrap()
        .unwrap()
        .priority = 1;
    run(&mut world, &mut schedule, 2..3);
    assert_eq!(down(physics(&world), 4.0).surface, Some(CARPET));

    world
        .get_mut::<SurfaceOverlay>(first)
        .unwrap()
        .unwrap()
        .priority = 2;
    run(&mut world, &mut schedule, 3..4);
    assert_eq!(down(physics(&world), 4.0).surface, Some(PUDDLE));

    assert_eq!(
        down(physics(&world), 6.5).surface,
        Some(CONCRETE),
        "outside both overlays the floor's tag"
    );
}

#[test]
fn a_moved_kinematic_overlay_is_seen_from_the_next_step() {
    let mut world = World::new();
    floor(&mut world);
    let patch = overlay(&mut world, 20.0, 1.0, "puddle", 0, Body::new_kinematic());
    let mut schedule = stepped();
    run(&mut world, &mut schedule, 0..2);
    assert_eq!(down(physics(&world), 4.0).surface, Some(CONCRETE));

    world
        .get_mut::<TestPose>(patch)
        .unwrap()
        .unwrap()
        .translation = [4.0, 0.0, 0.0];
    assert_eq!(
        down(physics(&world), 4.0).surface,
        Some(CONCRETE),
        "the edit alone moves nothing: a pose is the last completed step's"
    );
    run(&mut world, &mut schedule, 2..4);
    assert_eq!(down(physics(&world), 4.0).surface, Some(PUDDLE));
}

#[test]
fn every_query_reports_the_overlay_and_the_sensor_is_no_obstacle() {
    let mut world = World::new();
    let ground = floor(&mut world);
    overlay(&mut world, 4.0, 1.0, "puddle", 0, Body::new_static());
    let mut schedule = stepped();
    run(&mut world, &mut schedule, 0..2);
    let physics = physics(&world);
    let filter = QueryFilter::new();

    let ray = down(physics, 4.0);
    assert_eq!(ray.entity, ground, "the ray passes the sensor to the floor");
    assert_eq!(ray.surface, Some(PUDDLE));

    let cast = physics
        .shape_cast(
            &Shape::Sphere { radius: 0.1 },
            [4.0, 2.0, 0.0],
            IDENTITY,
            [0.0, -1.0, 0.0],
            5.0,
            &filter,
        )
        .unwrap();
    assert_eq!(cast[0].entity, ground);
    assert_eq!(cast[0].surface, Some(PUDDLE));

    let closest = physics
        .closest_points([4.0, 0.3, 0.0], 1.0, &filter)
        .unwrap();
    assert_eq!(closest[0].entity, ground);
    assert_eq!(closest[0].surface, Some(PUDDLE));

    assert_eq!(physics.surface_at(ground, [4.0, 0.0, 0.0]), Some(PUDDLE));
    assert_eq!(physics.surface_at(ground, [7.0, 0.0, 0.0]), Some(CONCRETE));
    assert_eq!(
        physics.surface_of(ground),
        Some(CONCRETE),
        "contacts keep the tag"
    );
}

#[test]
fn surface_under_answers_overlay_ray_and_nothing() {
    let mut world = World::new();
    floor(&mut world);
    overlay(&mut world, 4.0, 1.0, "puddle", 0, Body::new_static());
    let mut schedule = stepped();
    run(&mut world, &mut schedule, 0..2);
    let physics = physics(&world);
    let filter = QueryFilter::new();

    assert_eq!(
        physics
            .surface_under([4.0, 0.0, 0.0], 1.0, &filter)
            .unwrap(),
        Some(PUDDLE),
        "inside an overlay"
    );
    assert_eq!(
        physics
            .surface_under([1.0, 1.0, 0.0], 2.0, &filter)
            .unwrap(),
        Some(CONCRETE),
        "above the floor, out of any overlay"
    );
    assert_eq!(
        physics
            .surface_under([50.0, 1.0, 0.0], 2.0, &filter)
            .unwrap(),
        None,
        "nothing under"
    );
    for (point, reach) in [
        ([f32::NAN, 0.0, 0.0], 1.0),
        ([0.0, 1.0, 0.0], -1.0),
        ([0.0, 1.0, 0.0], f32::INFINITY),
    ] {
        assert!(
            matches!(
                physics.surface_under(point, reach, &filter),
                Err(PhysicsError::QueryInput(_))
            ),
            "{point:?} {reach}"
        );
    }
}

#[test]
fn an_undeclared_overlay_is_inert_and_rejects_once() {
    let mut world = World::new();
    let ground = floor(&mut world);
    let bad = overlay(&mut world, 4.0, 1.0, "pudle", 0, Body::new_static());
    let mut schedule = stepped();
    run(&mut world, &mut schedule, 0..4);
    let found = rejections(&mut world);
    assert_eq!(found.len(), 1, "one L1690 for a standing fault: {found:?}");
    assert_eq!(found[0].code, code::SURFACE_UNKNOWN);
    assert_eq!(found[0].entity, bad);
    assert!(
        found[0].reason.contains("SurfaceOverlay"),
        "{}",
        found[0].reason
    );
    assert!(found[0].reason.contains("puddle"), "{}", found[0].reason);
    assert_eq!(physics(&world).body_count(), 2, "the body is kept");
    let hit = down(physics(&world), 4.0);
    assert_eq!(hit.entity, ground);
    assert_eq!(hit.surface, Some(CONCRETE), "the overlay is inert");

    world
        .get_mut::<SurfaceOverlay>(bad)
        .unwrap()
        .unwrap()
        .surface = "puddle".to_string();
    run(&mut world, &mut schedule, 4..5);
    assert!(rejections(&mut world).is_empty(), "the fix reports nothing");
    assert_eq!(down(physics(&world), 4.0).surface, Some(PUDDLE));
}

#[test]
fn a_restore_answers_overlays_before_any_write() {
    let mut world = World::new();
    let ground = floor(&mut world);
    overlay(&mut world, 4.0, 1.0, "puddle", 0, Body::new_static());
    let mut schedule = stepped();
    run(&mut world, &mut schedule, 0..2);
    let blob = physics(&world).to_bytes().unwrap();
    let mut restored = PhysicsWorld::from_bytes(&blob, TickRate::DEFAULT).unwrap();
    restored.sync_surface_tags(&world);
    restored.apply_settings(&settings());
    assert_eq!(restored.surface_at(ground, [4.0, 0.0, 0.0]), Some(PUDDLE));
    let hit = restored
        .ray_cast([4.0, 2.0, 0.0], [0.0, -1.0, 0.0], 5.0, &QueryFilter::new())
        .unwrap();
    assert_eq!(hit[0].surface, Some(PUDDLE));
    assert!(restored.take_rejections().is_empty());
}
