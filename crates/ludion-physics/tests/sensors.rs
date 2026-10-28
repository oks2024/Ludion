//! Sensor colliders: built as rapier sensors that never enter the contact graph, skipped by every
//! query unless the filter opts in, and refused on a rigid body or a surface-only shape.

mod common;

use common::{TestPose, pose_of, tick};
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_ecs::{Schedule, World};
use ludion_physics::{PhysicsSpace, PhysicsWorld, QueryFilter, install};
use ludion_spatial::{Body, Collider, RigidBody, Shape};

const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

fn schedule() -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose>(&mut schedule, PhysicsSpace::Free);
    schedule
}

fn cube(half: f32) -> Shape {
    Shape::Box {
        half_extents: [half; 3],
    }
}

fn collider(shape: Shape, body: Body, sensor: bool) -> Collider {
    Collider {
        shape,
        body,
        layer: None,
        sensor,
    }
}

fn spawn(world: &mut World, at: [f32; 3], collider: Collider) -> EntityId {
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at(at))
        .expect("a fresh entity accepts a pose");
    world
        .insert(entity, collider)
        .expect("a fresh entity accepts a collider");
    entity
}

fn physics(world: &World) -> &PhysicsWorld {
    world
        .resource::<PhysicsWorld>()
        .expect("a body claimed the world")
}

fn set_sensor(world: &mut World, entity: EntityId, sensor: bool) {
    world
        .get_mut::<Collider>(entity)
        .expect("a live entity")
        .expect("its collider")
        .sensor = sensor;
}

/// A rigid ball dropped from above a static slab, with a sensor box around its path or without one.
fn drop_through(with_sensor: bool) -> (World, Option<EntityId>, EntityId) {
    let mut world = World::new();
    let mut sensor = None;
    if with_sensor {
        sensor = Some(spawn(
            &mut world,
            [0.0, 2.0, 0.0],
            collider(cube(1.0), Body::new_static(), true),
        ));
    }
    let ball = spawn(
        &mut world,
        [0.0, 5.0, 0.0],
        collider(
            Shape::Sphere { radius: 0.25 },
            Body::Rigid(RigidBody::default()),
            false,
        ),
    );
    (world, sensor, ball)
}

#[test]
fn a_rigid_ball_falls_through_a_sensor_as_if_it_were_not_there() {
    let (mut with, sensor, with_ball) = drop_through(true);
    let (mut without, _, without_ball) = drop_through(false);
    let sensor = sensor.expect("the sensor was spawned");
    let mut with_schedule = schedule();
    let mut without_schedule = schedule();
    for frame in 0..90 {
        tick(&mut with, &mut with_schedule, frame);
        tick(&mut without, &mut without_schedule, frame);
        let touched = physics(&with)
            .contact_events()
            .iter()
            .any(|event| event.a == sensor || event.b == sensor);
        assert!(!touched, "tick {frame}: a sensor entered the contact graph");
    }
    let (a, b) = (pose_of(&with, with_ball), pose_of(&without, without_ball));
    assert_eq!(a.translation, b.translation);
    assert_eq!(a.rotation, b.rotation);
    assert!(
        a.translation[1] < 1.0,
        "the ball must have passed the sensor"
    );
}

/// A sensor box at x = 2 between the origin and a solid box at x = 6.
fn lined_up() -> (World, Schedule, EntityId, EntityId) {
    let mut world = World::new();
    let sensor = spawn(
        &mut world,
        [2.0, 0.0, 0.0],
        collider(cube(0.5), Body::new_static(), true),
    );
    let solid = spawn(
        &mut world,
        [6.0, 0.0, 0.0],
        collider(cube(0.5), Body::new_static(), false),
    );
    let mut schedule = schedule();
    tick(&mut world, &mut schedule, 0);
    (world, schedule, sensor, solid)
}

#[test]
fn queries_skip_sensors_unless_the_filter_includes_them() {
    let (world, _, sensor, solid) = lined_up();
    let physics = physics(&world);
    let plain = QueryFilter::new();
    let opted_in = QueryFilter::new().include_sensors();
    let probe = cube(0.25);

    let ray = |filter: &QueryFilter| -> Vec<EntityId> {
        physics
            .ray_cast([0.0; 3], [1.0, 0.0, 0.0], 100.0, filter)
            .expect("a finite ray")
            .iter()
            .map(|hit| hit.entity)
            .collect()
    };
    assert_eq!(ray(&plain), vec![solid]);
    assert_eq!(ray(&opted_in), vec![sensor, solid]);

    let cast = |filter: &QueryFilter| -> Vec<EntityId> {
        physics
            .shape_cast(&probe, [0.0; 3], IDENTITY, [1.0, 0.0, 0.0], 100.0, filter)
            .expect("a finite cast")
            .iter()
            .map(|hit| hit.entity)
            .collect()
    };
    assert_eq!(cast(&plain), vec![solid]);
    assert_eq!(cast(&opted_in), vec![sensor, solid]);

    let wide = cube(10.0);
    let overlap = |filter: &QueryFilter| -> Vec<EntityId> {
        let mut found = physics
            .overlap(&wide, [4.0, 0.0, 0.0], IDENTITY, filter)
            .expect("a finite probe");
        found.sort();
        found
    };
    assert_eq!(overlap(&plain), vec![solid]);
    let mut both = vec![sensor, solid];
    both.sort();
    assert_eq!(overlap(&opted_in), both);
}

#[test]
fn a_rigid_sensor_or_a_heightfield_sensor_gets_no_body() {
    let mut world = World::new();
    let rigid = spawn(
        &mut world,
        [0.0; 3],
        collider(cube(0.5), Body::Rigid(RigidBody::default()), true),
    );
    let heightfield = spawn(
        &mut world,
        [5.0, 0.0, 0.0],
        collider(
            Shape::Heightfield {
                rows: 2,
                cols: 2,
                heights: vec![0.0; 4],
                scale: [1.0; 3],
            },
            Body::new_static(),
            true,
        ),
    );
    let ok = spawn(
        &mut world,
        [10.0, 0.0, 0.0],
        collider(cube(0.5), Body::new_static(), true),
    );
    let mut schedule = schedule();
    tick(&mut world, &mut schedule, 0);

    let physics = world
        .resource_mut::<PhysicsWorld>()
        .expect("the valid sensor claimed the world");
    assert!(physics.shape_kind(rigid).is_none());
    assert!(physics.shape_kind(heightfield).is_none());
    assert!(physics.shape_kind(ok).is_some());
    let rejections = physics.take_rejections();
    let codes: Vec<_> = rejections.iter().map(|r| (r.entity, r.code)).collect();
    assert_eq!(
        codes,
        vec![
            (rigid, code::COLLIDER_SENSOR_ON_RIGID),
            (heightfield, code::COLLIDER_SENSOR_NO_INTERIOR),
        ],
        "{rejections:?}"
    );
}

#[test]
fn toggling_sensor_on_a_live_body_rebuilds_it() {
    let mut world = World::new();
    let crate_ = spawn(
        &mut world,
        [3.0, 0.0, 0.0],
        collider(cube(0.5), Body::new_static(), false),
    );
    let mut schedule = schedule();
    tick(&mut world, &mut schedule, 0);
    let hits = |world: &World| {
        physics(world)
            .ray_cast([0.0; 3], [1.0, 0.0, 0.0], 100.0, &QueryFilter::new())
            .expect("a finite ray")
            .len()
    };
    assert_eq!(hits(&world), 1);

    set_sensor(&mut world, crate_, true);
    tick(&mut world, &mut schedule, 1);
    assert_eq!(hits(&world), 0, "the rebuilt body is a sensor");

    set_sensor(&mut world, crate_, false);
    tick(&mut world, &mut schedule, 2);
    assert_eq!(hits(&world), 1, "toggling back restores the hit");
}
