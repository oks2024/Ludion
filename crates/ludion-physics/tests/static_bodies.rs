//! `PhysicsWorld::static_bodies`: only statics, the posed AABB, and a revision that sees a turn.

mod common;

use common::{TestPose, pose_of, tick};
use ludion_core::math::euler_xyz_degrees_to_quat;
use ludion_ecs::{Schedule, World};
use ludion_physics::{NoParent, PhysicsSpace, PhysicsWorld, StaticBody, install};
use ludion_spatial::{Body, Collider, RigidBody, Shape};

fn spawn(world: &mut World, at: [f32; 3], body: Body) -> ludion_core::ids::EntityId {
    let entity = world.spawn();
    world.insert(entity, TestPose::at(at)).unwrap();
    let collider = Collider {
        shape: Shape::Box {
            half_extents: [0.5; 3],
        },
        body,
        layer: None,
        sensor: false,
    };
    world.insert(entity, collider).unwrap();
    entity
}

fn statics(world: &World) -> Vec<StaticBody> {
    world
        .resource::<PhysicsWorld>()
        .unwrap()
        .static_bodies()
        .collect()
}

#[test]
fn lists_statics_only_and_sees_moves_and_turns() {
    let mut world = World::new();
    let wall = spawn(&mut world, [0.0, 0.0, 0.0], Body::new_static());
    spawn(
        &mut world,
        [5.0, 0.0, 0.0],
        Body::Rigid(RigidBody::default()),
    );
    let mut schedule = Schedule::new();
    install::<TestPose, NoParent>(&mut schedule, PhysicsSpace::Free);
    tick(&mut world, &mut schedule, 0);

    assert_eq!(pose_of(&world, wall).translation, [0.0; 3]);
    let first = statics(&world);
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(first[0].entity, wall);
    assert_eq!(first[0].min, [-0.5; 3]);

    world.get_mut::<TestPose>(wall).unwrap().unwrap().rotation =
        euler_xyz_degrees_to_quat([0.0, 90.0, 0.0]);
    tick(&mut world, &mut schedule, 1);
    let turned = statics(&world);
    assert_ne!(turned[0].revision, first[0].revision, "a turned cube");

    world
        .get_mut::<TestPose>(wall)
        .unwrap()
        .unwrap()
        .translation = [3.0, 0.0, 0.0];
    tick(&mut world, &mut schedule, 2);
    let moved = statics(&world);
    assert_eq!(moved[0].min[0], 2.5, "{moved:?}");
    assert_ne!(moved[0].revision, turned[0].revision);
}
