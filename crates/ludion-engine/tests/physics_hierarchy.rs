//! Physics and the spatial index under a `Parent` (`Plan/transform-hierarchy-and-prefabs.md`
//! §Rulings 3–4): a kinematic child rides its parent's world pose through `physics_write`, a rigid or
//! static-solver child gets no body, and the 2D tier resolves a parented collider's world position.

use ludion_core::ids::EntityId;
use ludion_core::time::{FrameIndex, TickRate};
use ludion_ecs::{Parent, Schedule, SystemCtx, World};
use ludion_engine::{Transform, world_transform};
use ludion_physics::{PhysicsSpace, PhysicsWorld, QueryFilter};
use ludion_spatial::{Body, Collider, RigidBody, Shape, spatial};

fn box_collider(half_extents: [f32; 3], body: Body) -> Collider {
    Collider {
        shape: Shape::Box { half_extents },
        body,
        layer: None,
    }
}

fn spawn(world: &mut World, transform: Transform, collider: Collider) -> EntityId {
    let id = world.spawn();
    world.insert(id, transform).unwrap();
    world.insert(id, collider).unwrap();
    id
}

fn at(translation: [f32; 3]) -> Transform {
    Transform {
        translation,
        ..Transform::default()
    }
}

fn run(schedule: &mut Schedule, world: &mut World, ticks: u64) {
    for frame in 0..ticks {
        schedule.run(
            world,
            &SystemCtx::new(FrameIndex::new(frame), TickRate::DEFAULT),
        );
    }
}

fn has_body(world: &World, entity: EntityId) -> bool {
    world
        .resource::<PhysicsWorld>()
        .is_some_and(|physics| physics.entities().any(|e| e == entity))
}

/// Moves the root platform +x each tick; registered before `physics_write`.
fn move_platform(world: &mut World, _ctx: &SystemCtx) {
    let ids: Vec<EntityId> = world
        .query_ids::<(&Collider, &Transform)>()
        .filter(|(_, (collider, _))| matches!(collider.body, Body::Kinematic(_)))
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        if world.has::<Parent>(id) {
            continue;
        }
        world.get_mut::<Transform>(id).unwrap().unwrap().translation[0] += 0.1;
    }
}

#[test]
fn a_kinematic_child_tracks_its_moving_parents_world_pose() {
    let mut world = World::new();
    let platform = spawn(
        &mut world,
        Transform {
            rotation: [0.0, 90.0, 0.0],
            ..Transform::default()
        },
        box_collider([1.0, 0.25, 1.0], Body::new_kinematic()),
    );
    let child = spawn(
        &mut world,
        at([2.0, 1.0, 0.0]),
        box_collider([0.5, 0.5, 0.5], Body::new_kinematic()),
    );
    world.insert(child, Parent(platform)).unwrap();

    let mut schedule = Schedule::new();
    schedule.add_system("move_platform", move_platform);
    ludion_physics::install::<Transform>(&mut schedule, PhysicsSpace::Free);
    run(&mut schedule, &mut world, 10);

    let world_pose = world_transform(&world, child).unwrap().trs.translation;
    assert!(
        (world_pose[1] - 1.0).abs() < 1e-4 && (world_pose[2] + 2.0).abs() < 1e-4,
        "a 90° yaw turns the local +x offset onto -z: {world_pose:?}"
    );
    let physics = world.resource::<PhysicsWorld>().unwrap();
    let down = |x: f32, z: f32| {
        physics
            .ray_cast([x, 10.0, z], [0.0, -1.0, 0.0], 20.0, &QueryFilter::new())
            .unwrap()
    };
    let hit = down(world_pose[0], world_pose[2])
        .into_iter()
        .find(|hit| hit.entity == child)
        .expect("a ray above the child's world position hits it");
    assert!(
        (hit.distance - 8.5).abs() < 1e-3,
        "distance {}",
        hit.distance
    );
    assert!(
        down(world_pose[0] + 2.0, 0.0)
            .iter()
            .all(|hit| hit.entity != child),
        "the child's local position is not where its body is"
    );
}

#[test]
fn a_rigid_or_static_solver_child_gets_no_body_until_unparented() {
    let mut world = World::new();
    let parent = spawn(
        &mut world,
        at([0.0, 0.0, 0.0]),
        box_collider([1.0, 1.0, 1.0], Body::new_kinematic()),
    );
    let rigid = spawn(
        &mut world,
        at([0.0, 5.0, 0.0]),
        box_collider([0.5; 3], Body::Rigid(RigidBody::default())),
    );
    let fixed = spawn(
        &mut world,
        at([4.0, 0.0, 0.0]),
        box_collider([0.5; 3], Body::new_static()),
    );
    world.insert(rigid, Parent(parent)).unwrap();
    world.insert(fixed, Parent(parent)).unwrap();

    let mut schedule = Schedule::new();
    ludion_physics::install::<Transform>(&mut schedule, PhysicsSpace::Free);
    run(&mut schedule, &mut world, 3);
    assert!(has_body(&world, parent));
    assert!(!has_body(&world, rigid));
    assert!(!has_body(&world, fixed));

    world.remove::<Parent>(rigid).unwrap();
    run(&mut schedule, &mut world, 1);
    assert!(
        has_body(&world, rigid),
        "an un-parented rigid body is created"
    );
    assert!(!has_body(&world, fixed));

    world.insert(rigid, Parent(parent)).unwrap();
    run(&mut schedule, &mut world, 1);
    assert!(!has_body(&world, rigid), "re-parenting retires its body");
}

#[test]
fn a_sheared_kinematic_child_still_has_a_body() {
    let mut world = World::new();
    let parent = spawn(
        &mut world,
        Transform {
            scale: [2.0, 1.0, 1.0],
            ..Transform::default()
        },
        box_collider([1.0; 3], Body::new_kinematic()),
    );
    let child = spawn(
        &mut world,
        Transform {
            translation: [0.0, 2.0, 0.0],
            rotation: [0.0, 0.0, 30.0],
            ..Transform::default()
        },
        box_collider([0.5; 3], Body::new_kinematic()),
    );
    world.insert(child, Parent(parent)).unwrap();

    let mut schedule = Schedule::new();
    ludion_physics::install::<Transform>(&mut schedule, PhysicsSpace::Free);
    run(&mut schedule, &mut world, 3);
    assert!(has_body(&world, child));
}

#[test]
fn the_2d_index_files_a_parented_collider_at_its_world_position() {
    let mut world = World::new();
    let parent = world.spawn();
    world.insert(parent, at([10.0, 0.0, 5.0])).unwrap();
    let child = spawn(
        &mut world,
        at([1.0, 0.0, 1.0]),
        Collider {
            shape: Shape::Aabb {
                half_extents: [0.5, 0.5],
            },
            body: Body::new_static(),
            layer: None,
        },
    );
    world.insert(child, Parent(parent)).unwrap();

    spatial::rebuild::<Transform>(
        &mut world,
        &SystemCtx::new(FrameIndex::ZERO, TickRate::DEFAULT),
    );
    let probe =
        |center: [f32; 2]| spatial::overlaps(&world, Shape::Circle { radius: 0.1 }, center, None);
    assert_eq!(probe([11.0, 6.0]), vec![child]);
    assert!(probe([1.0, 1.0]).is_empty());
}
