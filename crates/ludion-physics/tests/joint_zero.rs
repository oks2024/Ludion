//! `JointZero` on the wrapper: a stored zero replaces the zero captured from the bodies' pose at
//! build, so a motor target and the limits are measured from it; it survives a body rebuild and a
//! solver snapshot, and adding one to a live joint rebuilds the joint.

mod common;

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, World};
use ludion_physics::{NoParent, PhysicsSettings, PhysicsSpace, PhysicsWorld, install};
use ludion_spatial::{
    Body, Collider, Joint, JointKind, JointMotor, JointZero, RigidBody, Shape, Velocity,
};

const LINK_RADIUS: f32 = 0.5;
/// A link's inertia about its hinge at one metre: `m·r²` plus the sphere's own `2/5·m·R²`.
const LINK_INERTIA: f32 = 1.0 + 0.4 * LINK_RADIUS * LINK_RADIUS;
/// How far past the rest pose the link starts.
const START_DEGREES: f32 = 40.0;

fn firm_drive(target: f32) -> JointMotor {
    let omega = std::f32::consts::TAU * 10.0;
    JointMotor {
        target: vec![target],
        stiffness: LINK_INERTIA * omega * omega,
        damping: 2.0 * LINK_INERTIA * omega,
        max_force: None,
    }
}

fn schedule() -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose, NoParent>(
        &mut schedule,
        PhysicsSettings {
            gravity: [0.0; 3],
            ..PhysicsSettings::new(PhysicsSpace::Free)
        },
    );
    schedule
}

/// A static anchor at the origin and a link hinged to it about z, rotated `START_DEGREES` about z
/// at the same place on the orbit, so its pose relative to the anchor is `START_DEGREES` off the
/// pose the test calls rest.
fn world_at_40(
    limits: Option<[f32; 2]>,
    zero: Option<JointZero>,
    motor: Option<f32>,
) -> (World, EntityId) {
    let mut world = World::new();
    let anchor = world.spawn();
    world.insert(anchor, TestPose::at([0.0; 3])).unwrap();
    world
        .insert(
            anchor,
            Collider {
                shape: Shape::Sphere { radius: 0.1 },
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .unwrap();
    let bob = world.spawn();
    let half = START_DEGREES.to_radians() / 2.0;
    world
        .insert(
            bob,
            TestPose {
                translation: [
                    ludion_core::math::cos(START_DEGREES.to_radians()),
                    ludion_core::math::sin(START_DEGREES.to_radians()),
                    0.0,
                ],
                rotation: [
                    0.0,
                    0.0,
                    ludion_core::math::sin(half),
                    ludion_core::math::cos(half),
                ],
            },
        )
        .unwrap();
    world
        .insert(
            bob,
            Collider {
                shape: Shape::Sphere {
                    radius: LINK_RADIUS,
                },
                body: Body::Rigid(RigidBody::default()),
                layer: None,
                sensor: false,
            },
        )
        .unwrap();
    world.insert(bob, Velocity::default()).unwrap();
    world
        .insert(
            bob,
            Joint {
                a: anchor,
                b: bob,
                kind: JointKind::Hinge {
                    axis: [0.0, 0.0, 1.0],
                    limits,
                },
                anchor: [0.0; 3],
                break_impulse: None,
                collide: false,
            },
        )
        .unwrap();
    if let Some(zero) = zero {
        world.insert(bob, zero).unwrap();
    }
    if let Some(target) = motor {
        world.insert(bob, firm_drive(target)).unwrap();
    }
    (world, bob)
}

fn angle(world: &World, bob: EntityId) -> f32 {
    let [x, y, _] = pose_of(world, bob).translation;
    ludion_core::math::atan2(y, x).to_degrees()
}

fn run(world: &mut World, schedule: &mut Schedule, frames: std::ops::Range<u64>) {
    for frame in frames {
        tick(world, schedule, frame);
    }
}

#[test]
fn a_motor_target_is_measured_from_the_stored_zero() {
    let (mut world, bob) = world_at_40(Some([-30.0, 30.0]), Some(JointZero::default()), Some(0.0));
    let mut sched = schedule();
    run(&mut world, &mut sched, 0..300);
    let a = angle(&world, bob);
    assert!(a.abs() <= 1.5, "drove to {a} degrees, expected about 0");
}

#[test]
fn without_a_zero_the_same_motor_holds_the_start_pose() {
    let (mut world, bob) = world_at_40(Some([-30.0, 30.0]), None, Some(0.0));
    let mut sched = schedule();
    run(&mut world, &mut sched, 0..300);
    let a = angle(&world, bob);
    assert!((a - START_DEGREES).abs() <= 1.5, "held {a} degrees");
}

/// A switch pose outside the limits measured from the zero is pulled back to the limit, not refused.
#[test]
fn a_start_outside_the_limits_from_the_zero_is_clamped() {
    let (mut world, bob) = world_at_40(Some([-30.0, 30.0]), Some(JointZero::default()), None);
    let mut sched = schedule();
    run(&mut world, &mut sched, 0..300);
    let a = angle(&world, bob);
    assert!((28.0..=31.5).contains(&a), "clamped to {a} degrees");
}

fn bump_mass(world: &mut World, bob: EntityId) {
    let collider = world
        .get_mut::<Collider>(bob)
        .unwrap()
        .expect("the link carries a collider");
    if let Body::Rigid(body) = &mut collider.body {
        body.mass += 1.0;
    }
}

#[test]
fn a_body_rebuild_keeps_the_stored_zero() {
    let (mut world, bob) = world_at_40(Some([-30.0, 30.0]), Some(JointZero::default()), None);
    let mut sched = schedule();
    run(&mut world, &mut sched, 0..120);
    let before = angle(&world, bob);
    assert!(before <= 31.5, "clamped before the rebuild: {before}");
    bump_mass(&mut world, bob);
    run(&mut world, &mut sched, 120..180);
    // Without the stored zero the rebuild would re-capture it near 30 degrees and the motor below
    // would hold there.
    world.insert(bob, firm_drive(0.0)).unwrap();
    run(&mut world, &mut sched, 180..480);
    let a = angle(&world, bob);
    assert!(a.abs() <= 1.5, "drove to {a} degrees after the rebuild");
}

#[test]
fn adding_a_zero_to_a_live_joint_rebuilds_it() {
    let (mut world, bob) = world_at_40(Some([-30.0, 30.0]), None, Some(0.0));
    let mut sched = schedule();
    run(&mut world, &mut sched, 0..120);
    let held = angle(&world, bob);
    assert!((held - START_DEGREES).abs() <= 1.5, "held {held}");
    world.insert(bob, JointZero::default()).unwrap();
    run(&mut world, &mut sched, 120..480);
    let a = angle(&world, bob);
    assert!(
        a.abs() <= 1.5,
        "drove to {a} degrees after the zero arrived"
    );
}

#[test]
fn a_degenerate_zero_is_treated_as_absent() {
    let zero = JointZero {
        rotation: [0.0, 0.0, 0.0, 0.0],
    };
    let (mut world, bob) = world_at_40(None, Some(zero), Some(0.0));
    let mut sched = schedule();
    run(&mut world, &mut sched, 0..300);
    let a = angle(&world, bob);
    assert!((a - START_DEGREES).abs() <= 1.5, "held {a} degrees");
}

fn trace(
    world: &mut World,
    sched: &mut Schedule,
    frames: std::ops::Range<u64>,
    bob: EntityId,
) -> Vec<[u32; 4]> {
    frames
        .map(|frame| {
            tick(world, sched, frame);
            pose_of(world, bob).rotation.map(f32::to_bits)
        })
        .collect()
}

/// The solver blob round-trips with a zero, and a world resumed from it continues bit-identically.
#[test]
fn the_solver_blob_round_trips_with_a_zero() {
    let build = || world_at_40(Some([-30.0, 30.0]), Some(JointZero::default()), Some(0.0));
    let (mut reference, bob) = build();
    let mut sched = schedule();
    let full = trace(&mut reference, &mut sched, 0..200, bob);

    let (mut world, bob) = build();
    let mut sched = schedule();
    let mut head = trace(&mut world, &mut sched, 0..100, bob);
    let bytes = world
        .resource::<PhysicsWorld>()
        .unwrap()
        .to_bytes()
        .unwrap();
    let restored = PhysicsWorld::from_bytes(&bytes, TickRate::DEFAULT).unwrap();
    assert_eq!(restored.to_bytes().unwrap(), bytes, "a re-encode is stable");
    world.insert_resource(restored);
    head.extend(trace(&mut world, &mut sched, 100..200, bob));
    assert_eq!(
        head, full,
        "a resumed run differs from the uninterrupted one"
    );
}

#[test]
fn the_solver_blob_round_trips_without_a_zero() {
    let (mut world, bob) = world_at_40(Some([-30.0, 30.0]), None, Some(0.0));
    let mut sched = schedule();
    run(&mut world, &mut sched, 0..50);
    let bytes = world
        .resource::<PhysicsWorld>()
        .unwrap()
        .to_bytes()
        .unwrap();
    let restored = PhysicsWorld::from_bytes(&bytes, TickRate::DEFAULT).unwrap();
    assert_eq!(restored.to_bytes().unwrap(), bytes);
    world.insert_resource(restored);
    run(&mut world, &mut sched, 50..300);
    let a = angle(&world, bob);
    assert!((a - START_DEGREES).abs() <= 1.5, "held {a} degrees");
}
