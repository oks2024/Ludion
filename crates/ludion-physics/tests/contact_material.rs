//! The contact material reaches the solver for every body kind: a static or kinematic collider's
//! `friction`, `restitution` and combine rules are built into the rapier collider, and an edit of
//! them is applied in place (`tests/live_edit.rs` covers the rigid scalars).

mod common;

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_ecs::{Schedule, World};
use ludion_physics::{PhysicsSettings, PhysicsSpace, PhysicsWorld, install};
use ludion_spatial::{
    Body, Collider, CombineRule, ContactMaterial, Joint, JointKind, RigidBody, Shape, Velocity,
};

const GRAVITY: [f32; 3] = [0.0, -9.81, 0.0];

fn physics_schedule() -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose>(
        &mut schedule,
        PhysicsSettings {
            gravity: GRAVITY,
            ..PhysicsSettings::new(PhysicsSpace::Free)
        },
    );
    schedule
}

fn physics(world: &World) -> &PhysicsWorld {
    world
        .resource::<PhysicsWorld>()
        .expect("the write system installed a world")
}

fn sphere() -> Shape {
    Shape::Sphere { radius: 0.5 }
}

fn ball(restitution: f32, restitution_combine: CombineRule) -> Collider {
    let body = RigidBody {
        restitution,
        restitution_combine,
        ..RigidBody::default()
    };
    Collider {
        shape: sphere(),
        body: Body::Rigid(body),
        layer: None,
    }
}

fn spawn_body(world: &mut World, at: [f32; 3], collider: Collider) -> EntityId {
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at(at))
        .expect("a fresh entity accepts a pose");
    world
        .insert(entity, collider)
        .expect("a fresh entity accepts a collider");
    world
        .insert(entity, Velocity::default())
        .expect("a fresh entity accepts a velocity");
    entity
}

fn collider_of(world: &World, entity: EntityId) -> Collider {
    world
        .get::<Collider>(entity)
        .expect("a live entity")
        .expect("its collider")
        .clone()
}

fn set_collider(world: &mut World, entity: EntityId, collider: Collider) {
    *world
        .get_mut::<Collider>(entity)
        .expect("a live entity")
        .expect("its collider") = collider;
}

fn velocity_of(world: &World, entity: EntityId) -> Velocity {
    *world
        .get::<Velocity>(entity)
        .expect("a live entity")
        .expect("the test gives it a Velocity")
}

fn run(world: &mut World, schedule: &mut Schedule, frames: std::ops::Range<u64>) {
    for frame in frames {
        tick(world, schedule, frame);
    }
}

/// A floor whose top face is at `y = 0`.
fn floor(world: &mut World, body: Body) -> EntityId {
    spawn_body(
        world,
        [0.0, -0.5, 0.0],
        Collider {
            shape: Shape::Box {
                half_extents: [20.0, 0.5, 20.0],
            },
            body,
            layer: None,
        },
    )
}

/// Runs `frames` and returns the highest `y` the body reaches after it first rebounds upward.
fn rebound_apex(
    world: &mut World,
    schedule: &mut Schedule,
    body: EntityId,
    frames: std::ops::Range<u64>,
) -> f32 {
    let (mut rebounded, mut apex) = (false, 0.0f32);
    for frame in frames {
        tick(world, schedule, frame);
        rebounded |= velocity_of(world, body).linear[1] > 0.5;
        if rebounded {
            apex = apex.max(pose_of(world, body).translation[1]);
        }
    }
    apex
}

/// A static anchor at the origin (entity 0) and a rigid bob one unit out at +x, hinged about z with
/// limits `[-10, 10]` degrees around the pose the joint captured.
fn hinged_bob(anchor_body: Body) -> (World, EntityId, EntityId) {
    let mut world = World::new();
    let anchor = spawn_body(
        &mut world,
        [0.0; 3],
        Collider {
            shape: Shape::Sphere { radius: 0.1 },
            body: anchor_body,
            layer: None,
        },
    );
    let bob = spawn_body(&mut world, [1.0, 0.0, 0.0], ball(0.0, CombineRule::Average));
    world
        .insert(
            bob,
            Joint {
                a: anchor,
                b: bob,
                kind: JointKind::Hinge {
                    axis: [0.0, 0.0, 1.0],
                    limits: Some([-10.0, 10.0]),
                },
                anchor: [0.0; 3],
                break_impulse: None,
                collide: false,
            },
        )
        .expect("a fresh entity accepts a joint");
    (world, anchor, bob)
}

/// The bob's angle about z around the origin, in degrees.
fn angle_z(world: &World, entity: EntityId) -> f32 {
    let [x, y, _] = pose_of(world, entity).translation;
    ludion_core::math::atan2(y, x).to_degrees()
}

/// Flips the bob's gravity so it swings to the upper limit around the original zero.
fn flip_gravity(world: &mut World, bob: EntityId) {
    let mut collider = collider_of(world, bob);
    let Body::Rigid(body) = &mut collider.body else {
        panic!("the bob is not rigid");
    };
    body.gravity_scale = -1.0;
    set_collider(world, bob, collider);
}

fn assert_zero_kept(world: &World, bob: EntityId) {
    let after = angle_z(world, bob);
    assert!(
        (after - 10.0).abs() < 2.0,
        "the bob settles at the upper limit around the original zero: {after}"
    );
    assert_eq!(physics(world).is_joint_active(bob), Some(true));
}

#[test]
fn a_static_ground_material_reaches_the_solver() {
    let mut world = World::new();
    let springy = ContactMaterial {
        restitution: 0.6,
        ..ContactMaterial::default()
    };
    let bouncy_floor = floor(&mut world, Body::Static(springy));
    let dropped = spawn_body(&mut world, [0.0, 6.0, 0.0], ball(0.0, CombineRule::Average));
    let mut schedule = physics_schedule();
    let apex = rebound_apex(&mut world, &mut schedule, dropped, 0..150);

    assert_eq!(physics(&world).restitution(bouncy_floor), Some(0.6));
    assert!(apex > 0.8, "the ball rebounds off the bouncy floor: {apex}");

    let mut dead_world = World::new();
    floor(&mut dead_world, Body::new_static());
    let twin = spawn_body(
        &mut dead_world,
        [0.0, 6.0, 0.0],
        ball(0.0, CombineRule::Average),
    );
    let mut dead_schedule = physics_schedule();
    let dead_apex = rebound_apex(&mut dead_world, &mut dead_schedule, twin, 0..150);
    assert!(
        dead_apex < 0.1,
        "a default floor does not rebound it: {dead_apex}"
    );
}

#[test]
fn max_beats_average_on_a_dead_ground() {
    let mut world = World::new();
    floor(&mut world, Body::new_static());
    let averaged = spawn_body(
        &mut world,
        [-3.0, 3.0, 0.0],
        ball(0.8, CombineRule::Average),
    );
    let maxed = spawn_body(&mut world, [3.0, 3.0, 0.0], ball(0.8, CombineRule::Max));
    let mut schedule = physics_schedule();

    let (mut averaged_apex, mut maxed_apex) = (0.0f32, 0.0f32);
    let (mut averaged_up, mut maxed_up) = (false, false);
    for frame in 0..150 {
        tick(&mut world, &mut schedule, frame);
        averaged_up |= velocity_of(&world, averaged).linear[1] > 0.5;
        maxed_up |= velocity_of(&world, maxed).linear[1] > 0.5;
        if averaged_up {
            averaged_apex = averaged_apex.max(pose_of(&world, averaged).translation[1]);
        }
        if maxed_up {
            maxed_apex = maxed_apex.max(pose_of(&world, maxed).translation[1]);
        }
    }

    // Restitution 0.8 against the floor's 0.0: average gives 0.4, max gives 0.8.
    let (average_rise, max_rise) = (averaged_apex - 0.5, maxed_apex - 0.5);
    assert!(
        max_rise > 2.0 * average_rise,
        "Max rebounds much higher: {max_rise} vs {average_rise}"
    );
}

#[test]
fn a_default_material_builds_rapiers_defaults() {
    let mut world = World::new();
    let ground = floor(&mut world, Body::new_static());
    let mut schedule = physics_schedule();
    run(&mut world, &mut schedule, 0..2);

    let physics = physics(&world);
    assert_eq!(physics.friction(ground), Some(0.5));
    assert_eq!(physics.restitution(ground), Some(0.0));
    assert_eq!(physics.friction_combine(ground), Some(CombineRule::Average));
    assert_eq!(
        physics.restitution_combine(ground),
        Some(CombineRule::Average)
    );
}

#[test]
fn a_static_material_edit_applies_in_place_and_wakes_the_resting_body() {
    let mut world = World::new();
    let ground = floor(&mut world, Body::new_static());
    let body = RigidBody {
        sleep_after_ticks: Some(5),
        ..RigidBody::default()
    };
    let resting = spawn_body(
        &mut world,
        [0.0, 0.5, 0.0],
        Collider {
            shape: sphere(),
            body: Body::Rigid(body),
            layer: None,
        },
    );
    let mut schedule = physics_schedule();
    tick(&mut world, &mut schedule, 0);
    let mut frame = 1;
    while physics(&world).is_sleeping(resting) != Some(true) {
        assert!(frame < 300, "the ball never settled asleep");
        tick(&mut world, &mut schedule, frame);
        frame += 1;
    }

    let edited = ContactMaterial {
        restitution: 0.7,
        restitution_combine: CombineRule::Max,
        ..ContactMaterial::default()
    };
    let mut collider = collider_of(&world, ground);
    collider.body = Body::Static(edited);
    set_collider(&mut world, ground, collider);
    tick(&mut world, &mut schedule, frame);

    let physics = physics(&world);
    assert_eq!(physics.is_sleeping(resting), Some(false));
    assert_eq!(physics.restitution(ground), Some(0.7));
    assert_eq!(physics.restitution_combine(ground), Some(CombineRule::Max));
}

#[test]
fn a_kinematic_anchor_material_edit_keeps_the_joint_zero() {
    let (mut world, anchor, bob) = hinged_bob(Body::new_kinematic());
    let mut schedule = physics_schedule();

    run(&mut world, &mut schedule, 0..120);
    let edited = ContactMaterial {
        friction: 0.2,
        restitution_combine: CombineRule::Max,
        ..ContactMaterial::default()
    };
    let mut collider = collider_of(&world, anchor);
    collider.body = Body::Kinematic(edited);
    set_collider(&mut world, anchor, collider);
    run(&mut world, &mut schedule, 120..130);
    flip_gravity(&mut world, bob);
    run(&mut world, &mut schedule, 130..400);

    assert_zero_kept(&world, bob);
    assert_eq!(physics(&world).friction(anchor), Some(0.2));
    assert_eq!(
        physics(&world).restitution_combine(anchor),
        Some(CombineRule::Max)
    );
}

#[test]
fn a_combine_edit_on_a_rigid_body_applies_in_place() {
    let (mut world, _, bob) = hinged_bob(Body::new_static());
    let mut schedule = physics_schedule();

    run(&mut world, &mut schedule, 0..120);
    let mut collider = collider_of(&world, bob);
    let Body::Rigid(body) = &mut collider.body else {
        panic!("the bob is not rigid");
    };
    body.restitution_combine = CombineRule::Max;
    set_collider(&mut world, bob, collider);
    run(&mut world, &mut schedule, 120..130);
    flip_gravity(&mut world, bob);
    run(&mut world, &mut schedule, 130..400);

    assert_zero_kept(&world, bob);
    assert_eq!(
        physics(&world).restitution_combine(bob),
        Some(CombineRule::Max)
    );
}
