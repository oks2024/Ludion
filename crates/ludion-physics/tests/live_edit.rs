//! Live `Collider` edits: an edit of only `friction`, `restitution`, the combine rules, damping,
//! `gravity_scale` or `sleep_after_ticks` is applied in place — the body handle survives, so a
//! `Joint`'s captured zero, its contacts and its sleep state are not thrown away by a rebuild;
//! static and kinematic materials too (`tests/contact_material.rs`). Any other edit still
//! rebuilds.

mod common;

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, World};
use ludion_physics::{NoParent, PhysicsSettings, PhysicsSpace, PhysicsWorld, install};
use ludion_spatial::{Body, Collider, Joint, JointKind, RigidBody, Shape, Velocity};

const GRAVITY: [f32; 3] = [0.0, -9.81, 0.0];

fn physics_schedule() -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose, NoParent>(
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

fn scaled(scale: f32) -> RigidBody {
    RigidBody {
        gravity_scale: scale,
        ..RigidBody::default()
    }
}

fn rigid(shape: Shape, body: RigidBody) -> Collider {
    Collider {
        shape,
        body: Body::Rigid(body),
        layer: None,
        sensor: false,
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

/// Edits only `gravity_scale` on `entity`'s rigid collider.
fn set_scale(world: &mut World, entity: EntityId, scale: f32) {
    let mut collider = collider_of(world, entity);
    let Body::Rigid(body) = &mut collider.body else {
        panic!("the entity is not rigid");
    };
    body.gravity_scale = scale;
    set_collider(world, entity, collider);
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

/// A static anchor at the origin (entity 0) and a rigid bob one unit out at +x, hinged about z with
/// limits `[-10, 10]` degrees around the pose the joint captured.
fn hinged_bob() -> (World, EntityId) {
    let mut world = World::new();
    let anchor = spawn_body(
        &mut world,
        [0.0; 3],
        Collider {
            shape: Shape::Sphere { radius: 0.1 },
            body: Body::new_static(),
            layer: None,
            sensor: false,
        },
    );
    let bob = spawn_body(&mut world, [1.0, 0.0, 0.0], rigid(sphere(), scaled(1.0)));
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
    (world, bob)
}

/// The bob's angle about z around the origin, in degrees.
fn angle_z(world: &World, entity: EntityId) -> f32 {
    let [x, y, _] = pose_of(world, entity).translation;
    ludion_core::math::atan2(y, x).to_degrees()
}

#[test]
fn an_authored_scale_floats_or_raises_a_body() {
    let mut world = World::new();
    let floater = spawn_body(&mut world, [-3.0, 5.0, 0.0], rigid(sphere(), scaled(0.0)));
    let riser = spawn_body(&mut world, [0.0, 5.0, 0.0], rigid(sphere(), scaled(-1.0)));
    let faller = spawn_body(&mut world, [3.0, 5.0, 0.0], rigid(sphere(), scaled(1.0)));
    let mut schedule = physics_schedule();

    run(&mut world, &mut schedule, 0..60);

    assert_eq!(
        pose_of(&world, floater).translation[1],
        5.0,
        "a zero scale keeps the body exactly where it started"
    );
    assert!(
        pose_of(&world, riser).translation[1] > 6.0,
        "a negative scale rises: {:?}",
        pose_of(&world, riser)
    );
    assert!(
        pose_of(&world, faller).translation[1] < 4.0,
        "the default scale falls: {:?}",
        pose_of(&world, faller)
    );
    assert_eq!(physics(&world).gravity_scale(floater), Some(0.0));
    assert_eq!(physics(&world).gravity_scale(riser), Some(-1.0));
    assert_eq!(physics(&world).gravity_scale(faller), Some(1.0));
}

/// A hinge with limits `[-10, 10]` holds a horizontal bob at about -10° under gravity. Flipping
/// its scale to `-1` must swing it to about +10°: the joint's zero stays where it was captured.
/// A rebuild would have re-captured the zero at the -10° pose, so the same limits would then span
/// `[-20, 0]` and the bob would stop near 0°.
#[test]
fn a_scale_only_edit_keeps_the_joint_zero() {
    let (mut world, bob) = hinged_bob();
    let mut schedule = physics_schedule();

    run(&mut world, &mut schedule, 0..120);
    let before = angle_z(&world, bob);
    assert!(
        (before + 10.0).abs() < 2.0,
        "the bob rests at its lower limit first: {before}"
    );

    set_scale(&mut world, bob, -1.0);
    run(&mut world, &mut schedule, 120..360);

    let after = angle_z(&world, bob);
    assert!(
        (after - 10.0).abs() < 2.0,
        "the bob settles at the upper limit around the original zero: {after}"
    );
    assert_eq!(physics(&world).is_joint_active(bob), Some(true));
    assert_eq!(physics(&world).gravity_scale(bob), Some(-1.0));
}

#[test]
fn a_scale_edit_mid_flight_flips_the_fall() {
    let mut world = World::new();
    let ball = spawn_body(&mut world, [0.0, 50.0, 0.0], rigid(sphere(), scaled(1.0)));
    let mut schedule = physics_schedule();

    run(&mut world, &mut schedule, 0..20);
    let falling = velocity_of(&world, ball);
    assert!(falling.linear[1] < -1.0, "the ball is falling: {falling:?}");

    set_scale(&mut world, ball, 0.0);
    tick(&mut world, &mut schedule, 20);
    let first = velocity_of(&world, ball);
    tick(&mut world, &mut schedule, 21);
    let second = velocity_of(&world, ball);

    assert_eq!(
        first.linear.map(f32::to_bits),
        falling.linear.map(f32::to_bits),
        "no gravity acts once the scale is zero, so the velocity is unchanged"
    );
    assert_eq!(
        second.linear.map(f32::to_bits),
        first.linear.map(f32::to_bits),
        "and stays unchanged on the next tick"
    );
}

#[test]
fn a_scale_only_edit_wakes_a_sleeping_body() {
    let mut world = World::new();
    spawn_body(
        &mut world,
        [0.0, -0.5, 0.0],
        Collider {
            shape: Shape::Box {
                half_extents: [20.0, 0.5, 20.0],
            },
            body: Body::new_static(),
            layer: None,
            sensor: false,
        },
    );
    let ball = spawn_body(
        &mut world,
        [0.0, 0.5, 0.0],
        rigid(
            sphere(),
            RigidBody {
                sleep_after_ticks: Some(5),
                ..RigidBody::default()
            },
        ),
    );
    let mut schedule = physics_schedule();

    tick(&mut world, &mut schedule, 0);
    let mut frame = 1;
    while physics(&world).is_sleeping(ball) != Some(true) {
        assert!(frame < 300, "the resting ball never fell asleep");
        tick(&mut world, &mut schedule, frame);
        frame += 1;
    }
    let resting_y = pose_of(&world, ball).translation[1];

    set_scale(&mut world, ball, -1.0);
    tick(&mut world, &mut schedule, frame);

    assert_eq!(
        physics(&world).is_sleeping(ball),
        Some(false),
        "an in-place scale edit wakes the body"
    );
    run(&mut world, &mut schedule, frame + 1..frame + 20);
    assert!(
        pose_of(&world, ball).translation[1] > resting_y + 0.05,
        "the woken body rises: {resting_y} -> {:?}",
        pose_of(&world, ball)
    );
}

/// Edits `entity`'s rigid collider through `edit`, leaving everything else as it was.
fn edit_rigid(world: &mut World, entity: EntityId, edit: impl FnOnce(&mut RigidBody)) {
    let mut collider = collider_of(world, entity);
    let Body::Rigid(body) = &mut collider.body else {
        panic!("the entity is not rigid");
    };
    edit(body);
    set_collider(world, entity, collider);
}

fn floor(world: &mut World) {
    spawn_body(
        world,
        [0.0, -0.5, 0.0],
        Collider {
            shape: Shape::Box {
                half_extents: [20.0, 0.5, 20.0],
            },
            body: Body::new_static(),
            layer: None,
            sensor: false,
        },
    );
}

/// Two identical balls fall on a static floor; one gets `restitution: 0.9` while still in the air.
/// After the impact the edited one rebounds and the other stays down.
#[test]
fn a_restitution_edit_mid_flight_bounces() {
    let mut world = World::new();
    floor(&mut world);
    let edited = spawn_body(&mut world, [-3.0, 10.0, 0.0], rigid(sphere(), scaled(1.0)));
    let twin = spawn_body(&mut world, [3.0, 10.0, 0.0], rigid(sphere(), scaled(1.0)));
    let mut schedule = physics_schedule();

    run(&mut world, &mut schedule, 0..20);
    edit_rigid(&mut world, edited, |body| body.restitution = 0.9);
    let (mut edited_peak, mut twin_peak) = (0.0f32, 0.0f32);
    for frame in 20..140 {
        tick(&mut world, &mut schedule, frame);
        edited_peak = edited_peak.max(velocity_of(&world, edited).linear[1]);
        twin_peak = twin_peak.max(velocity_of(&world, twin).linear[1]);
    }

    assert_eq!(physics(&world).restitution(edited), Some(0.9));
    assert!(edited_peak > 1.0, "the edited ball rebounds: {edited_peak}");
    assert!(twin_peak < 0.5, "the twin does not: {twin_peak}");
}

/// A box rests on a 30-degree static ramp at high friction, asleep. Editing the friction to zero
/// wakes it (a sleeping contact is not re-solved) and it slides down the slope.
#[test]
fn a_friction_edit_lets_a_resting_box_slide() {
    let angle = 30.0f32.to_radians();
    let (sin, cos) = (ludion_core::math::sin(angle), ludion_core::math::cos(angle));
    let tilt = [
        0.0,
        0.0,
        ludion_core::math::sin(angle / 2.0),
        ludion_core::math::cos(angle / 2.0),
    ];
    let mut world = World::new();
    let ramp = world.spawn();
    let on_ramp = |translation: [f32; 3]| TestPose {
        translation,
        rotation: tilt,
    };
    world.insert(ramp, on_ramp([0.0; 3])).expect("a pose");
    let half_extents = [10.0, 0.5, 10.0];
    let slab = Collider {
        shape: Shape::Box { half_extents },
        body: Body::new_static(),
        layer: None,
        sensor: false,
    };
    world.insert(ramp, slab).expect("a collider");
    let crate_body = RigidBody {
        friction: 2.0,
        sleep_after_ticks: Some(5),
        ..RigidBody::default()
    };
    let cube = Shape::Box {
        half_extents: [0.5; 3],
    };
    let resting = world.spawn();
    world
        .insert(resting, on_ramp([-sin, cos, 0.0]))
        .expect("a pose");
    world
        .insert(resting, rigid(cube, crate_body))
        .expect("a collider");
    world
        .insert(resting, Velocity::default())
        .expect("a velocity");
    let mut schedule = physics_schedule();

    tick(&mut world, &mut schedule, 0);
    let mut frame = 1;
    while physics(&world).is_sleeping(resting) != Some(true) {
        assert!(frame < 300, "the box never settled asleep on the ramp");
        tick(&mut world, &mut schedule, frame);
        frame += 1;
    }
    let held = pose_of(&world, resting).translation;

    edit_rigid(&mut world, resting, |body| body.friction = 0.0);
    run(&mut world, &mut schedule, frame..frame + 60);

    assert_eq!(physics(&world).friction(resting), Some(0.0));
    let slid = pose_of(&world, resting).translation;
    let down_slope = (held[0] - slid[0]) * cos + (held[1] - slid[1]) * sin;
    assert!(
        down_slope > 0.5,
        "the box slides down the slope: {down_slope}"
    );
}

/// Like the scale-only edit, a material and damping edit keeps the joint's captured zero: the bob
/// rests at -10, and only after the material edit is the scale flipped, yet it still swings to +10.
#[test]
fn a_material_edit_keeps_the_joint_zero() {
    let (mut world, bob) = hinged_bob();
    let mut schedule = physics_schedule();

    run(&mut world, &mut schedule, 0..120);
    edit_rigid(&mut world, bob, |body| {
        body.friction = 0.2;
        body.restitution = 0.1;
        body.linear_damping = 0.05;
        body.angular_damping = 0.05;
    });
    run(&mut world, &mut schedule, 120..130);
    set_scale(&mut world, bob, -1.0);
    run(&mut world, &mut schedule, 130..400);

    let after = angle_z(&world, bob);
    assert!(
        (after - 10.0).abs() < 2.0,
        "the bob settles at the upper limit around the original zero: {after}"
    );
    assert_eq!(physics(&world).is_joint_active(bob), Some(true));
    assert_eq!(physics(&world).friction(bob), Some(0.2));
}

/// An edit of `sleep_after_ticks` alone is also in place: the body keeps its handle-backed state
/// and the new threshold governs its next rest.
#[test]
fn a_sleep_threshold_edit_applies_in_place() {
    let mut world = World::new();
    floor(&mut world);
    let ball = spawn_body(&mut world, [0.0, 0.5, 0.0], rigid(sphere(), scaled(1.0)));
    let mut schedule = physics_schedule();
    run(&mut world, &mut schedule, 0..5);

    edit_rigid(&mut world, ball, |body| body.sleep_after_ticks = Some(2));
    run(&mut world, &mut schedule, 5..40);

    assert_eq!(physics(&world).is_sleeping(ball), Some(true));
}

/// Only an in-place-tunables difference takes the in-place path: changing the mass alongside the
/// scale rebuilds, and the rebuilt body carries both.
#[test]
fn a_mass_edit_still_rebuilds() {
    let mut world = World::new();
    let ball = spawn_body(&mut world, [0.0, 5.0, 0.0], rigid(sphere(), scaled(1.0)));
    let mut schedule = physics_schedule();
    run(&mut world, &mut schedule, 0..5);

    set_collider(
        &mut world,
        ball,
        rigid(
            sphere(),
            RigidBody {
                mass: 4.0,
                gravity_scale: 0.5,
                ..RigidBody::default()
            },
        ),
    );
    tick(&mut world, &mut schedule, 5);

    assert_eq!(physics(&world).mass(ball), Some(4.0));
    assert_eq!(physics(&world).gravity_scale(ball), Some(0.5));
}

/// The scripted live edits: a scale flip at 20 and 40, with the material changed alongside.
fn apply_edit(world: &mut World, bob: EntityId, frame: u64) {
    let scale = match frame {
        20 => 0.0,
        40 => -1.0,
        _ => return,
    };
    let mut collider = collider_of(world, bob);
    let Body::Rigid(body) = &mut collider.body else {
        panic!("the entity is not rigid");
    };
    body.gravity_scale = scale;
    body.restitution = 0.25 + scale.abs() * 0.5;
    body.friction = 0.1 + scale.abs();
    set_collider(world, bob, collider);
}

/// The hinged bob's pose and velocity bits, the trace's comparison unit.
type Trace = ([u32; 3], [u32; 4], [u32; 3], [u32; 3]);

fn trace_of(world: &World, bob: EntityId) -> Trace {
    let pose = pose_of(world, bob);
    let velocity = velocity_of(world, bob);
    (
        pose.translation.map(f32::to_bits),
        pose.rotation.map(f32::to_bits),
        velocity.linear.map(f32::to_bits),
        velocity.angular.map(f32::to_bits),
    )
}

#[test]
fn a_live_edit_survives_a_snapshot_bit_exact() {
    // Uninterrupted reference: both scale edits in one continuous run.
    let (mut reference, bob) = hinged_bob();
    let mut reference_schedule = physics_schedule();
    let mut reference_trace = Vec::with_capacity(70);
    for frame in 0..70u64 {
        apply_edit(&mut reference, bob, frame);
        tick(&mut reference, &mut reference_schedule, frame);
        reference_trace.push(trace_of(&reference, bob));
    }

    // Captured: identical to tick 30 (one edit in), then snapshot the solver bytes and the ECS
    // state a real restore would carry alongside them.
    let (mut captured, captured_bob) = hinged_bob();
    let mut captured_schedule = physics_schedule();
    for frame in 0..30u64 {
        apply_edit(&mut captured, captured_bob, frame);
        tick(&mut captured, &mut captured_schedule, frame);
    }
    let bytes = physics(&captured).to_bytes().expect("the solver encodes");
    let pose = pose_of(&captured, captured_bob);
    let velocity = velocity_of(&captured, captured_bob);
    let collider = collider_of(&captured, captured_bob);

    // Restored: a fresh world with the same entities, carrying the captured ECS state, then the
    // remaining ticks including the second edit.
    let (mut restored, restored_bob) = hinged_bob();
    assert_eq!(restored_bob, captured_bob);
    *restored
        .get_mut::<TestPose>(restored_bob)
        .expect("a live entity")
        .expect("its pose") = pose;
    *restored
        .get_mut::<Velocity>(restored_bob)
        .expect("a live entity")
        .expect("its velocity") = velocity;
    set_collider(&mut restored, restored_bob, collider);
    restored.insert_resource(
        PhysicsWorld::from_bytes(&bytes, TickRate::DEFAULT).expect("the captured bytes decode"),
    );
    let mut restored_schedule = physics_schedule();
    let mut restored_trace = Vec::with_capacity(40);
    for frame in 30..70u64 {
        apply_edit(&mut restored, restored_bob, frame);
        tick(&mut restored, &mut restored_schedule, frame);
        restored_trace.push(trace_of(&restored, restored_bob));
    }

    assert_eq!(
        restored_trace,
        reference_trace[30..],
        "a live edit after a restore must match the uninterrupted trace bit-for-bit"
    );
}

/// A static body's `Transform` is read every tick, not only at creation: moving the box moves its
/// geometry in the solver.
#[test]
fn a_moved_static_body_follows_its_transform() {
    let mut world = World::new();
    let slab = world.spawn();
    world.insert(slab, TestPose::at([0.0; 3])).unwrap();
    world
        .insert(
            slab,
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
    let mut schedule = physics_schedule();
    run(&mut world, &mut schedule, 0..2);

    let hits_at = |world: &World, x: f32| {
        physics(world)
            .ray_cast(
                [x, 5.0, 0.0],
                [0.0, -1.0, 0.0],
                10.0,
                &ludion_physics::QueryFilter::new(),
            )
            .expect("a valid ray")
            .len()
    };
    assert_eq!(hits_at(&world, 0.0), 1);
    assert_eq!(hits_at(&world, 10.0), 0);

    world
        .get_mut::<TestPose>(slab)
        .unwrap()
        .unwrap()
        .translation = [10.0, 0.0, 0.0];
    run(&mut world, &mut schedule, 2..4);

    assert_eq!(hits_at(&world, 0.0), 0, "the old place must be empty");
    assert_eq!(hits_at(&world, 10.0), 1, "the new place must be solid");
}
