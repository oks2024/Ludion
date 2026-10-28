//! A `CharacterMotor` gets one kinematic capsule body whose origin is the character's feet:
//! rebuilt when the height or layer changes, refused beside a `Collider` or with a bad field,
//! driving rigid bodies like any kinematic body, and honouring its collision layer.

mod common;

use common::{TestPose, pose_of, tick};
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_ecs::{Schedule, World};
use ludion_physics::{
    CharacterMotor, CharacterMotorState, CollisionLayers, PhysicsSettings, PhysicsSpace,
    PhysicsWorld, QueryFilter, ShapeKind, ShapeRejection, install,
};
use ludion_spatial::{Body, Collider, RigidBody, Shape, Velocity};

fn schedule(layers: Option<CollisionLayers>) -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose>(
        &mut schedule,
        PhysicsSettings {
            layers,
            ..PhysicsSettings::new(PhysicsSpace::Free)
        },
    );
    schedule
}

fn spawn_motor(world: &mut World, feet: [f32; 3], motor: CharacterMotor) -> EntityId {
    let entity = world.spawn();
    world.insert(entity, TestPose::at(feet)).unwrap();
    world.insert(entity, motor).unwrap();
    entity
}

fn physics(world: &World) -> &PhysicsWorld {
    world
        .resource::<PhysicsWorld>()
        .expect("a body claimed the world")
}

fn take_rejections(world: &mut World) -> Vec<ShapeRejection> {
    world
        .resource_mut::<PhysicsWorld>()
        .expect("a world exists")
        .take_rejections()
}

/// The `y` where a ray dropped from above meets the topmost collider at `x = 0`.
fn top_at_origin(world: &World) -> f32 {
    let hits = physics(world)
        .ray_cast(
            [0.0, 10.0, 0.0],
            [0.0, -1.0, 0.0],
            20.0,
            &QueryFilter::new(),
        )
        .unwrap();
    hits.first().expect("a hit").point[1]
}

#[test]
fn a_motor_builds_one_kinematic_capsule_with_its_origin_at_the_feet() {
    let mut world = World::new();
    let mut schedule = schedule(None);
    let motor = spawn_motor(&mut world, [0.0, 1.0, 0.0], CharacterMotor::default());
    tick(&mut world, &mut schedule, 0);
    assert_eq!(physics(&world).body_count(), 1);
    assert_eq!(physics(&world).shape_kind(motor), Some(ShapeKind::Capsule));
    assert!((top_at_origin(&world) - 2.6).abs() < 1e-3);
    // The capsule does not move the feet: kinematic bodies are not mirrored back.
    assert_eq!(pose_of(&world, motor).translation, [0.0, 1.0, 0.0]);
}

#[test]
fn the_state_height_rebuilds_the_capsule() {
    let mut world = World::new();
    let mut schedule = schedule(None);
    let motor = spawn_motor(&mut world, [0.0, 1.0, 0.0], CharacterMotor::default());
    tick(&mut world, &mut schedule, 0);
    world
        .insert(
            motor,
            CharacterMotorState {
                height: Some(1.0),
                ..Default::default()
            },
        )
        .unwrap();
    tick(&mut world, &mut schedule, 1);
    tick(&mut world, &mut schedule, 2);
    assert_eq!(physics(&world).body_count(), 1);
    assert!((top_at_origin(&world) - 2.0).abs() < 1e-3);
}

#[test]
fn a_motor_beside_a_collider_keeps_the_capsule_and_reports_l1295() {
    let mut world = World::new();
    let mut schedule = schedule(None);
    let motor = spawn_motor(&mut world, [0.0, 0.0, 0.0], CharacterMotor::default());
    world
        .insert(
            motor,
            Collider {
                shape: Shape::Box {
                    half_extents: [0.5; 3],
                },
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .unwrap();
    tick(&mut world, &mut schedule, 0);
    assert_eq!(physics(&world).body_count(), 1);
    assert_eq!(physics(&world).shape_kind(motor), Some(ShapeKind::Capsule));
    let rejections = take_rejections(&mut world);
    assert_eq!(rejections.len(), 1);
    assert_eq!(rejections[0].entity, motor);
    assert_eq!(rejections[0].code, code::CHARACTER_MOTOR_WITH_COLLIDER);
}

#[test]
fn an_invalid_motor_built_in_rust_gets_no_body_and_reports_l1294() {
    let mut world = World::new();
    let mut schedule = schedule(None);
    let motor = spawn_motor(
        &mut world,
        [0.0, 0.0, 0.0],
        CharacterMotor {
            radius: 0.0,
            ..CharacterMotor::default()
        },
    );
    for frame in 0..3 {
        tick(&mut world, &mut schedule, frame);
    }
    assert_eq!(physics(&world).body_count(), 0);
    let rejections = take_rejections(&mut world);
    assert_eq!(rejections.len(), 1);
    assert_eq!(rejections[0].entity, motor);
    assert_eq!(rejections[0].code, code::CHARACTER_MOTOR_INVALID);
}

/// A floor, a resting box at `x = 1`, and a motor at the origin on `motor_layer`.
fn push_scene(
    motor_layer: Option<&str>,
    box_layer: Option<&str>,
    layers: Option<CollisionLayers>,
) -> (World, Schedule, EntityId, EntityId) {
    let mut world = World::new();
    let floor = world.spawn();
    world.insert(floor, TestPose::at([0.0, -0.5, 0.0])).unwrap();
    world
        .insert(
            floor,
            Collider {
                shape: Shape::Box {
                    half_extents: [20.0, 0.5, 20.0],
                },
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .unwrap();
    let block = world.spawn();
    world.insert(block, TestPose::at([1.0, 0.5, 0.0])).unwrap();
    world
        .insert(
            block,
            Collider {
                shape: Shape::Box {
                    half_extents: [0.5; 3],
                },
                body: Body::Rigid(RigidBody::default()),
                layer: box_layer.map(str::to_string),
                sensor: false,
            },
        )
        .unwrap();
    world.insert(block, Velocity::default()).unwrap();
    let motor = spawn_motor(
        &mut world,
        [0.0, 0.0, 0.0],
        CharacterMotor {
            layer: motor_layer.map(str::to_string),
            ..CharacterMotor::default()
        },
    );
    (world, schedule(layers), motor, block)
}

fn walk_into_the_block(world: &mut World, schedule: &mut Schedule, motor: EntityId) {
    for frame in 0..5 {
        tick(world, schedule, frame);
    }
    world
        .get_mut::<TestPose>(motor)
        .unwrap()
        .unwrap()
        .translation = [0.5, 0.0, 0.0];
    for frame in 5..30 {
        tick(world, schedule, frame);
    }
}

#[test]
fn a_moving_motor_pushes_a_resting_box() {
    let (mut world, mut schedule, motor, block) = push_scene(None, None, None);
    walk_into_the_block(&mut world, &mut schedule, motor);
    assert!(pose_of(&world, block).translation[0] > 1.1);
}

#[test]
fn the_motor_layer_rides_through_to_the_solver() {
    let layers = CollisionLayers::new(
        vec!["default".into(), "a".into(), "b".into()],
        vec![0b000, 0b100, 0b010],
    );
    let (mut world, mut schedule, motor, block) = push_scene(Some("a"), Some("b"), Some(layers));
    walk_into_the_block(&mut world, &mut schedule, motor);
    assert!((pose_of(&world, block).translation[0] - 1.0).abs() < 1e-3);
}
