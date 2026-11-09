//! The contact-pair census: rapier's broad phase never tracks kinematic-kinematic or
//! kinematic-static pairs, a rigid body on a kinematic box is tracked and touching, and a landed
//! rigid body touches the static floor.

#[allow(dead_code)] // shared helpers; each test binary uses a subset
mod common;

use common::{TestPose, tick};
use ludion_core::ids::EntityId;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, World};
use ludion_physics::{ContactPairCensus, NoParent, PairCount, PhysicsSpace, PhysicsWorld, install};
use ludion_spatial::{Body, Collider, RigidBody, Shape, Velocity};

fn spawn(world: &mut World, at: [f32; 3], shape: Shape, body: Body) -> EntityId {
    let entity = world.spawn();
    world.insert(entity, TestPose::at(at)).expect("a pose");
    world
        .insert(
            entity,
            Collider {
                shape,
                body,
                layer: None,
                sensor: false,
            },
        )
        .expect("a collider");
    entity
}

fn cuboid(half: f32) -> Shape {
    Shape::Box {
        half_extents: [half; 3],
    }
}

#[test]
fn kinematic_pairs_are_never_tracked_and_rigid_pairs_touch() {
    let mut world = World::new();
    spawn(
        &mut world,
        [0.0, -0.5, 0.0],
        Shape::Box {
            half_extents: [20.0, 0.5, 20.0],
        },
        Body::new_static(),
    );
    spawn(
        &mut world,
        [-5.0, 0.5, 0.0],
        cuboid(0.5),
        Body::new_kinematic(),
    );
    spawn(
        &mut world,
        [-4.6, 0.5, 0.0],
        cuboid(0.5),
        Body::new_kinematic(),
    );
    for x in [5.0, -4.8] {
        let ball = spawn(
            &mut world,
            [x, 2.0, 0.0],
            Shape::Sphere { radius: 0.3 },
            Body::Rigid(RigidBody {
                mass: 1.0,
                restitution: 0.0,
                ..RigidBody::default()
            }),
        );
        world.insert(ball, Velocity::default()).expect("a velocity");
    }

    let mut schedule = Schedule::new();
    install::<TestPose, NoParent>(&mut schedule, PhysicsSpace::Free);
    for frame in 0..90 {
        tick(&mut world, &mut schedule, frame);
    }
    let census = world
        .resource::<PhysicsWorld>()
        .expect("the physics world")
        .contact_pair_census();
    // Rapier's broad phase prefilters pairs by `active_collision_types`, so these never exist.
    assert_eq!(
        census.kinematic_kinematic,
        PairCount::default(),
        "{census:?}"
    );
    assert_eq!(census.kinematic_static, PairCount::default(), "{census:?}");
    assert!(census.rigid_static.touching >= 1, "{census:?}");
    assert!(census.rigid_kinematic.tracked >= 1, "{census:?}");
    assert!(census.rigid_kinematic.touching >= 1, "{census:?}");
}

#[test]
fn an_empty_world_has_an_empty_census() {
    let world = PhysicsWorld::new(TickRate::default());
    assert_eq!(world.contact_pair_census(), ContactPairCensus::default());
}
