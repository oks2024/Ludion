//! `JointMotor` on the wrapper: each kind drives to its target in the limits' sign and units, a
//! per-tick retarget mutates the live joint (never re-captures its zero), a change wakes sleepers,
//! a passive motor is bit-identical to none, `max_force` caps, a mismatch leaves the joint passive
//! and reports once, and the applied motor rides the snapshot.

mod common;

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, World};
use ludion_physics::{NoParent, PhysicsSettings, PhysicsSpace, PhysicsWorld, install};
use ludion_spatial::{Body, Collider, Joint, JointKind, JointMotor, RigidBody, Shape, Velocity};

/// A link's radius, and its mass (`RigidBody::default`); it hangs `LEVER` from its anchor.
const LINK_RADIUS: f32 = 0.5;
const LINK_MASS: f32 = 1.0;
const LEVER: f32 = 1.0;
/// A link's inertia about its hinge: `m·r²` plus the sphere's own `2/5·m·R²`.
const LINK_INERTIA: f32 = LINK_MASS * LEVER * LEVER + 0.4 * LINK_MASS * LINK_RADIUS * LINK_RADIUS;

/// A critically damped drive of response frequency `hz` for a part of the given inertia (mass, on a
/// slider): `stiffness = I·ω²`, `damping = 2·I·ω`, `ω = 2π·hz`.
fn tuned(target: &[f32], inertia: f32, hz: f32) -> JointMotor {
    let omega = std::f32::consts::TAU * hz;
    motor(target, inertia * omega * omega, 2.0 * inertia * omega)
}

/// A drive that settles a link in a second or two.
fn drive(target: &[f32]) -> JointMotor {
    tuned(target, LINK_INERTIA, 2.0)
}

/// A drive stiff enough to hold a link near its target against gravity (`m·g·r` = 9.81 N·m sags
/// it about 0.1° at this stiffness).
fn firm_drive(target: &[f32]) -> JointMotor {
    tuned(target, LINK_INERTIA, 10.0)
}

fn small_sphere() -> Shape {
    Shape::Sphere { radius: 0.1 }
}

fn hinge_z(limits: Option<[f32; 2]>) -> JointKind {
    JointKind::Hinge {
        axis: [0.0, 0.0, 1.0],
        limits,
    }
}

fn motor(target: &[f32], stiffness: f32, damping: f32) -> JointMotor {
    JointMotor {
        target: target.to_vec(),
        stiffness,
        damping,
        max_force: None,
    }
}

/// A world with a static anchor at the origin (entity 0) and the physics tick under `gravity`.
fn motor_world(gravity: [f32; 3]) -> (World, Schedule, EntityId) {
    let mut world = World::new();
    let anchor = world.spawn();
    world
        .insert(anchor, TestPose::at([0.0; 3]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            anchor,
            Collider {
                shape: small_sphere(),
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .expect("a fresh entity accepts a collider");
    (world, physics_schedule(gravity), anchor)
}

fn physics_schedule(gravity: [f32; 3]) -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose, NoParent>(
        &mut schedule,
        PhysicsSettings {
            gravity,
            ..PhysicsSettings::new(PhysicsSpace::Free)
        },
    );
    schedule
}

/// Spawns a rigid sphere at `at` carrying `kind`'s joint back to `parent`, plus `drive` if given.
fn link(
    world: &mut World,
    parent: EntityId,
    at: [f32; 3],
    kind: JointKind,
    drive: Option<JointMotor>,
) -> EntityId {
    shaped_link(
        world,
        parent,
        at,
        kind,
        drive,
        Shape::Sphere {
            radius: LINK_RADIUS,
        },
        LINK_MASS,
    )
}

/// [`link`] with its own collider `shape` and `mass`.
fn shaped_link(
    world: &mut World,
    parent: EntityId,
    at: [f32; 3],
    kind: JointKind,
    drive: Option<JointMotor>,
    shape: Shape,
    mass: f32,
) -> EntityId {
    let bob = world.spawn();
    world
        .insert(bob, TestPose::at(at))
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            bob,
            Collider {
                shape,
                body: Body::Rigid(RigidBody {
                    mass,
                    ..RigidBody::default()
                }),
                layer: None,
                sensor: false,
            },
        )
        .expect("a fresh entity accepts a collider");
    world
        .insert(bob, Velocity::default())
        .expect("a fresh entity accepts a velocity");
    world
        .insert(
            bob,
            Joint {
                a: parent,
                b: bob,
                kind,
                anchor: [0.0; 3],
                break_impulse: None,
                collide: false,
            },
        )
        .expect("a fresh entity accepts a joint");
    if let Some(drive) = drive {
        world
            .insert(bob, drive)
            .expect("a fresh entity accepts a motor");
    }
    bob
}

/// The link's angle about z around the origin, in degrees (a hinge at the origin with the link at +x).
fn angle_z(world: &World, entity: EntityId) -> f32 {
    let [x, y, _] = pose_of(world, entity).translation;
    ludion_core::math::atan2(y, x).to_degrees()
}

fn set_target(world: &mut World, entity: EntityId, target: &[f32]) {
    world
        .get_mut::<JointMotor>(entity)
        .expect("a live entity")
        .expect("the entity carries a motor")
        .target = target.to_vec();
}

fn run(world: &mut World, schedule: &mut Schedule, frames: std::ops::Range<u64>) {
    for frame in frames {
        tick(world, schedule, frame);
    }
}

fn physics(world: &World) -> &PhysicsWorld {
    world
        .resource::<PhysicsWorld>()
        .expect("the write system installed a world")
}

fn take_mismatches(world: &mut World) -> Vec<ludion_physics::MotorMismatch> {
    world
        .resource_mut::<PhysicsWorld>()
        .expect("the write system installed a world")
        .take_motor_mismatches()
}

#[test]
fn a_hinge_motor_drives_to_its_target() {
    let (mut world, mut schedule, anchor) = motor_world([0.0; 3]);
    let bob = link(
        &mut world,
        anchor,
        [1.0, 0.0, 0.0],
        hinge_z(None),
        Some(drive(&[30.0])),
    );
    run(&mut world, &mut schedule, 0..300);
    let angle = angle_z(&world, bob);
    assert!((angle - 30.0).abs() <= 1.0, "settled at {angle} degrees");
}

/// The motor's sign and units are the limits' (`limits` are `[min, max]` degrees about the same
/// axis): a target past the upper limit rests at the limit, on the positive side.
#[test]
fn a_motor_target_shares_the_limits_sign() {
    let (mut world, mut schedule, anchor) = motor_world([0.0; 3]);
    let bob = link(
        &mut world,
        anchor,
        [1.0, 0.0, 0.0],
        hinge_z(Some([0.0, 20.0])),
        Some(drive(&[30.0])),
    );
    run(&mut world, &mut schedule, 0..300);
    let angle = angle_z(&world, bob);
    assert!(
        (angle - 20.0).abs() <= 1.5,
        "rests at {angle} degrees, not against the 20 degree limit"
    );
}

#[test]
fn a_slider_motor_drives_to_its_target() {
    let (mut world, mut schedule, anchor) = motor_world([0.0; 3]);
    let rider = link(
        &mut world,
        anchor,
        [1.0, 0.0, 0.0],
        JointKind::Slider {
            axis: [1.0, 0.0, 0.0],
            limits: None,
        },
        Some(tuned(&[0.5], LINK_MASS, 2.0)),
    );
    run(&mut world, &mut schedule, 0..300);
    let pos = pose_of(&world, rider).translation;
    assert!(
        (pos[0] - 1.5).abs() <= 0.02,
        "settled {} along the axis from its start",
        pos[0] - 1.0
    );
    assert!(pos[1].abs() <= 1e-2 && pos[2].abs() <= 1e-2, "{pos:?}");
}

#[test]
fn a_ball_motor_drives_its_axes_to_the_target() {
    let (mut world, mut schedule, anchor) = motor_world([0.0; 3]);
    let bob = link(
        &mut world,
        anchor,
        [1.0, 0.0, 0.0],
        JointKind::Ball { limits: None },
        Some(drive(&[0.0, 0.0, 30.0])),
    );
    run(&mut world, &mut schedule, 0..300);
    let angle = angle_z(&world, bob);
    assert!((angle - 30.0).abs() <= 1.5, "settled at {angle} degrees");
}

/// A target rewritten every tick mutates the live joint: a re-captured zero would leave the link
/// at the last target instead of returning to the start.
#[test]
fn per_tick_edits_keep_the_captured_zero() {
    let (mut world, mut schedule, anchor) = motor_world([0.0; 3]);
    let bob = link(
        &mut world,
        anchor,
        [1.0, 0.0, 0.0],
        hinge_z(None),
        Some(drive(&[0.0])),
    );
    for frame in 0..120u64 {
        set_target(&mut world, bob, &[40.0 * frame as f32 / 120.0]);
        tick(&mut world, &mut schedule, frame);
        assert_eq!(physics(&world).is_joint_active(bob), Some(true));
    }
    let raised = angle_z(&world, bob);
    assert!(raised > 20.0, "the ramp moved the link to {raised}");
    set_target(&mut world, bob, &[0.0]);
    for frame in 120..420u64 {
        tick(&mut world, &mut schedule, frame);
        assert_eq!(physics(&world).is_joint_active(bob), Some(true));
    }
    let angle = angle_z(&world, bob);
    assert!(angle.abs() <= 1.0, "returned to {angle}, not the zero");
}

#[test]
fn a_retarget_wakes_a_sleeping_joint() {
    let (mut world, mut schedule, anchor) = motor_world([0.0; 3]);
    let bob = link(
        &mut world,
        anchor,
        [1.0, 0.0, 0.0],
        hinge_z(None),
        Some(drive(&[0.0])),
    );
    run(&mut world, &mut schedule, 0..200);
    assert_eq!(
        physics(&world).is_sleeping(bob),
        Some(true),
        "a link held at rest goes to sleep"
    );
    set_target(&mut world, bob, &[30.0]);
    run(&mut world, &mut schedule, 200..500);
    let angle = angle_z(&world, bob);
    assert!((angle - 30.0).abs() <= 1.0, "settled at {angle} degrees");
}

/// Runs `bob` for 300 ticks; returns its highest angle about z and its angle at the end, asserting
/// every pose stays finite.
fn peak_and_final(
    world: &mut World,
    schedule: &mut Schedule,
    bob: EntityId,
    label: &str,
) -> (f32, f32) {
    let mut peak = f32::MIN;
    for frame in 0..300 {
        tick(world, schedule, frame);
        let pose = pose_of(world, bob);
        assert!(
            pose.translation.iter().all(|c| c.is_finite()),
            "NaN pose at tick {frame} ({label})"
        );
        peak = peak.max(angle_z(world, bob));
    }
    (peak, angle_z(world, bob))
}

/// A 1 kg rod limb 1.0 long hinged at one end (its centre 0.5 from the anchor): `I = m·L²/3`, about
/// 4x the limb's own inertia. The rule of thumb holds at 2, 20 and 60 Hz: finite, no overshoot past
/// target + 10 degrees, settled within 1 degree — and at 20 Hz under gravity with the force uncapped.
#[test]
fn a_rod_limb_meets_the_rule_of_thumb() {
    let inertia = LINK_MASS * 1.0 * 1.0 / 3.0;
    let runs = [
        ([0.0; 3], 2.0),
        ([0.0; 3], 20.0),
        ([0.0; 3], 60.0),
        ([0.0, -9.81, 0.0], 20.0),
    ];
    for (gravity, hz) in runs {
        let (mut world, mut schedule, anchor) = motor_world(gravity);
        let rod = shaped_link(
            &mut world,
            anchor,
            [0.5, 0.0, 0.0],
            hinge_z(None),
            Some(tuned(&[30.0], inertia, hz)),
            Shape::Box {
                half_extents: [0.5, 0.05, 0.05],
            },
            LINK_MASS,
        );
        let label = format!("rod at {hz} Hz, gravity {gravity:?}");
        let (peak, angle) = peak_and_final(&mut world, &mut schedule, rod, &label);
        assert!(peak <= 40.0, "{label} overshot to {peak} degrees");
        assert!(
            (angle - 30.0).abs() <= 1.0,
            "{label} settled at {angle} degrees"
        );
    }
}

/// The limit of the rule: a 0.2 kg, 0.1 radius sphere on a 1.0 lever has 250x more swing inertia
/// than its own, and rapier's angular motor row sees only the own. The drive overshoots and rings
/// but stays finite, bounded and settles.
#[test]
fn an_extreme_lever_stays_bounded() {
    for hz in [2.0, 20.0, 60.0] {
        let (mut world, mut schedule, anchor) = motor_world([0.0; 3]);
        let bob = shaped_link(
            &mut world,
            anchor,
            [LEVER, 0.0, 0.0],
            hinge_z(None),
            Some(tuned(&[30.0], 0.2, hz)),
            Shape::Sphere { radius: 0.1 },
            0.2,
        );
        let label = format!("extreme lever at {hz} Hz");
        let (peak, angle) = peak_and_final(&mut world, &mut schedule, bob, &label);
        assert!(peak < 60.0, "{label} overshot to {peak} degrees");
        assert!(
            (angle - 30.0).abs() <= 1.0,
            "{label} settled at {angle} degrees"
        );
    }
}

/// A light part on a long lever: 0.2 kg, 0.1 radius, 1.0 from the anchor.
fn light_lever(gravity: [f32; 3], drive: JointMotor) -> (World, Schedule, EntityId) {
    let (mut world, schedule, anchor) = motor_world(gravity);
    let bob = shaped_link(
        &mut world,
        anchor,
        [LEVER, 0.0, 0.0],
        hinge_z(None),
        Some(drive),
        Shape::Sphere { radius: 0.1 },
        0.2,
    );
    (world, schedule, bob)
}

/// A weak, steady, never-retargeted drive (overdamped: it creeps at under 0.3 rad/s) puts a light
/// link to sleep short of its target — 25.4 degrees, asleep from tick 300 on, with the keep-awake
/// rule disabled. The rule keeps it moving until it is within epsilon of the target, and then it
/// may sleep.
#[test]
fn a_steady_weak_drive_reaches_its_target() {
    let (mut world, mut schedule, bob) = light_lever([0.0; 3], motor(&[30.0], 4.0, 8.0));
    run(&mut world, &mut schedule, 0..1200);
    let angle = angle_z(&world, bob);
    assert!((angle - 30.0).abs() <= 1.0, "stalled at {angle} degrees");
    run(&mut world, &mut schedule, 1200..2400);
    assert_eq!(
        physics(&world).is_sleeping(bob),
        Some(true),
        "a link settled within epsilon of its target sleeps"
    );
    let angle = angle_z(&world, bob);
    assert!((angle - 30.0).abs() <= 0.5, "asleep at {angle} degrees");
}

fn pendulum_trace(drive: Option<JointMotor>) -> Vec<TestPose> {
    let (mut world, mut schedule, anchor) = motor_world([0.0, -9.81, 0.0]);
    let bob = link(&mut world, anchor, [1.0, 0.0, 0.0], hinge_z(None), drive);
    (0..200)
        .map(|frame| {
            tick(&mut world, &mut schedule, frame);
            pose_of(&world, bob)
        })
        .collect()
}

#[test]
fn a_passive_motor_is_bit_identical_to_none() {
    assert_eq!(
        pendulum_trace(Some(motor(&[30.0], 0.0, 0.0))),
        pendulum_trace(None)
    );
}

#[test]
fn removing_a_motor_stops_the_drive() {
    let (mut world, mut schedule, anchor) = motor_world([0.0, -9.81, 0.0]);
    let bob = link(
        &mut world,
        anchor,
        [1.0, 0.0, 0.0],
        hinge_z(None),
        Some(firm_drive(&[30.0])),
    );
    run(&mut world, &mut schedule, 0..200);
    let held = angle_z(&world, bob);
    assert!((held - 30.0).abs() <= 4.0, "held at {held} degrees");
    world
        .remove::<JointMotor>(bob)
        .expect("a live entity")
        .expect("the motor was there");
    run(&mut world, &mut schedule, 200..260);
    let released = angle_z(&world, bob);
    assert!(released < 0.0, "gravity took the link to {released}");
}

#[test]
fn max_force_caps_the_drive() {
    let capped = {
        let (mut world, mut schedule, anchor) = motor_world([0.0, -9.81, 0.0]);
        let bob = link(
            &mut world,
            anchor,
            [1.0, 0.0, 0.0],
            hinge_z(None),
            Some(JointMotor {
                max_force: Some(0.01),
                ..firm_drive(&[30.0])
            }),
        );
        run(&mut world, &mut schedule, 0..200);
        angle_z(&world, bob)
    };
    let uncapped = {
        let (mut world, mut schedule, anchor) = motor_world([0.0, -9.81, 0.0]);
        let bob = link(
            &mut world,
            anchor,
            [1.0, 0.0, 0.0],
            hinge_z(None),
            Some(firm_drive(&[30.0])),
        );
        run(&mut world, &mut schedule, 0..200);
        angle_z(&world, bob)
    };
    assert!(capped < -20.0, "the capped drive sagged only to {capped}");
    assert!(
        (uncapped - 30.0).abs() <= 5.0,
        "the uncapped drive reached {uncapped}"
    );
}

fn plain_trace(kind: JointKind, drive: Option<JointMotor>) -> Vec<TestPose> {
    let (mut world, mut schedule, anchor) = motor_world([0.0, -9.81, 0.0]);
    let bob = link(&mut world, anchor, [1.0, 0.0, 0.0], kind, drive);
    (0..10)
        .map(|frame| {
            tick(&mut world, &mut schedule, frame);
            pose_of(&world, bob)
        })
        .collect()
}

#[test]
fn a_mismatched_motor_leaves_the_joint_passive_and_reports_once() {
    let bad_motors = [
        (JointKind::Fixed {}, drive(&[30.0])),
        (hinge_z(None), drive(&[1.0, 2.0, 3.0])),
    ];
    for (kind, bad) in bad_motors {
        assert_eq!(
            plain_trace(kind.clone(), Some(bad.clone())),
            plain_trace(kind.clone(), None),
            "a mismatched motor must not move the joint ({kind:?})"
        );

        let (mut world, mut schedule, anchor) = motor_world([0.0, -9.81, 0.0]);
        let bob = link(&mut world, anchor, [1.0, 0.0, 0.0], kind, Some(bad));
        let mut queued = Vec::new();
        for frame in 0..10 {
            tick(&mut world, &mut schedule, frame);
            queued.extend(take_mismatches(&mut world));
        }
        assert_eq!(queued.len(), 1, "one report across ten ticks: {queued:?}");
        assert_eq!(queued[0].carrier, bob);
        assert!(!queued[0].reason.is_empty());
    }
}

#[test]
fn fixing_a_mismatched_motor_clears_it_and_drives() {
    let (mut world, mut schedule, anchor) = motor_world([0.0; 3]);
    let bob = link(
        &mut world,
        anchor,
        [1.0, 0.0, 0.0],
        hinge_z(None),
        Some(drive(&[30.0, 0.0, 0.0])),
    );
    run(&mut world, &mut schedule, 0..10);
    assert_eq!(take_mismatches(&mut world).len(), 1);
    assert!(angle_z(&world, bob).abs() <= 1e-3, "passive until fixed");

    set_target(&mut world, bob, &[30.0]);
    run(&mut world, &mut schedule, 10..310);
    assert!(take_mismatches(&mut world).is_empty());
    let angle = angle_z(&world, bob);
    assert!((angle - 30.0).abs() <= 1.0, "settled at {angle} degrees");

    set_target(&mut world, bob, &[1.0, 2.0]);
    run(&mut world, &mut schedule, 310..320);
    assert_eq!(
        take_mismatches(&mut world).len(),
        1,
        "breaking it again reports again"
    );
}

const TICKS: u64 = 150;
const SNAPSHOT_TICK: u64 = 80;

/// A two-link hinge chain, each link driven by a triangle ramp of the tick index — a target that
/// changes every tick, so the resumed run must re-apply the same retargets.
fn build_chain(world: &mut World) -> (EntityId, EntityId) {
    let anchor = world.spawn();
    world
        .insert(anchor, TestPose::at([0.0; 3]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            anchor,
            Collider {
                shape: small_sphere(),
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .expect("a fresh entity accepts a collider");
    let first = link(
        world,
        anchor,
        [1.0, 0.0, 0.0],
        hinge_z(None),
        Some(drive(&[0.0])),
    );
    let second = link(
        world,
        first,
        [2.0, 0.0, 0.0],
        hinge_z(None),
        Some(drive(&[0.0])),
    );
    (first, second)
}

fn drive_chain(world: &mut World, links: (EntityId, EntityId), frame: u64) {
    let ramp = (frame % 60) as f32;
    set_target(world, links.0, &[ramp * 0.5]);
    set_target(world, links.1, &[30.0 - ramp * 0.25]);
}

fn chain_trace() -> Vec<(TestPose, TestPose)> {
    let mut world = World::new();
    let links = build_chain(&mut world);
    let mut schedule = physics_schedule([0.0, -9.81, 0.0]);
    (0..TICKS)
        .map(|frame| {
            drive_chain(&mut world, links, frame);
            tick(&mut world, &mut schedule, frame);
            (pose_of(&world, links.0), pose_of(&world, links.1))
        })
        .collect()
}

struct Capture {
    entities: Vec<EntityId>,
    colliders: Vec<Option<Collider>>,
    poses: Vec<TestPose>,
    velocities: Vec<Option<Velocity>>,
    joints: Vec<Option<Joint>>,
    motors: Vec<Option<JointMotor>>,
    bytes: Vec<u8>,
}

fn capture(world: &World) -> Capture {
    let entities: Vec<EntityId> = world.entities().collect();
    Capture {
        colliders: entities
            .iter()
            .map(|&e| world.get::<Collider>(e).expect("a live entity").cloned())
            .collect(),
        poses: entities.iter().map(|&e| pose_of(world, e)).collect(),
        velocities: entities
            .iter()
            .map(|&e| world.get::<Velocity>(e).expect("a live entity").copied())
            .collect(),
        joints: entities
            .iter()
            .map(|&e| world.get::<Joint>(e).expect("a live entity").cloned())
            .collect(),
        motors: entities
            .iter()
            .map(|&e| world.get::<JointMotor>(e).expect("a live entity").cloned())
            .collect(),
        bytes: physics(world).to_bytes().expect("the solver state encodes"),
        entities,
    }
}

fn restore(captured: &Capture) -> World {
    let mut world = World::new();
    world
        .restore_entities(&captured.entities)
        .expect("captured live set is valid");
    for (i, &entity) in captured.entities.iter().enumerate() {
        if let Some(collider) = &captured.colliders[i] {
            world.insert(entity, collider.clone()).expect("collider");
        }
        world.insert(entity, captured.poses[i]).expect("pose");
        if let Some(velocity) = captured.velocities[i] {
            world.insert(entity, velocity).expect("velocity");
        }
        if let Some(joint) = &captured.joints[i] {
            world.insert(entity, joint.clone()).expect("joint");
        }
        if let Some(drive) = &captured.motors[i] {
            world.insert(entity, drive.clone()).expect("motor");
        }
    }
    world.insert_resource(
        PhysicsWorld::from_bytes(&captured.bytes, TickRate::DEFAULT)
            .expect("the bytes this run just wrote decode"),
    );
    world
}

#[test]
fn a_snapshot_resumed_mid_drive_continues_bit_identically() {
    let reference = chain_trace();
    let distinct = reference
        .iter()
        .map(|(a, _)| a.translation.map(f32::to_bits))
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    assert!(distinct > 100, "the driven chain moved: {distinct} poses");

    let mut world = World::new();
    let links = build_chain(&mut world);
    let mut sched = physics_schedule([0.0, -9.81, 0.0]);
    for frame in 0..SNAPSHOT_TICK {
        drive_chain(&mut world, links, frame);
        tick(&mut world, &mut sched, frame);
    }
    let captured = capture(&world);
    drop(world);
    drop(sched);

    let mut world = restore(&captured);
    let mut sched = physics_schedule([0.0, -9.81, 0.0]);
    for frame in SNAPSHOT_TICK..TICKS {
        drive_chain(&mut world, links, frame);
        tick(&mut world, &mut sched, frame);
        assert_eq!(
            (pose_of(&world, links.0), pose_of(&world, links.1)),
            reference[frame as usize],
            "the resumed chain diverges at tick {frame}"
        );
    }
}
