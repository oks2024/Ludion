//! `Plan/physics.md` W3: one test per [`JointKind`] proving the rapier mapping holds the shape the
//! plan promises, plus break, missing-body and snapshot-continuation coverage for the joint table
//! itself.

mod common;

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, World};
use ludion_physics::{
    DEFAULT_SOLVER_ITERATIONS, PhysicsSettings, PhysicsSpace, PhysicsWorld, install,
};
use ludion_spatial::{Body, Collider, Joint, JointKind, RigidBody, Shape, Velocity};

fn static_body(shape: Shape) -> Collider {
    Collider {
        shape,
        body: Body::new_static(),
        layer: None,
    }
}

fn rigid(shape: Shape, body: RigidBody) -> Collider {
    Collider {
        shape,
        body: Body::Rigid(body),
        layer: None,
    }
}

fn small_sphere() -> Shape {
    Shape::Sphere { radius: 0.1 }
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn joint_world() -> (World, Schedule) {
    joint_world_with_iterations(DEFAULT_SOLVER_ITERATIONS)
}

/// [`joint_world`], with `solver_iterations` set explicitly — fix 2's cross-iteration-count test
/// needs the same scene at more than one substep count.
fn joint_world_with_iterations(solver_iterations: u32) -> (World, Schedule) {
    let mut world = World::new();
    let anchor = world.spawn();
    world
        .insert(anchor, TestPose::at([0.0; 3]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(anchor, static_body(small_sphere()))
        .expect("a fresh entity accepts a collider");
    let mut schedule = Schedule::new();
    let settings = PhysicsSettings {
        solver_iterations,
        ..PhysicsSettings::new(PhysicsSpace::Free)
    };
    install::<TestPose>(&mut schedule, settings);
    (world, schedule)
}

/// Spawns `bob`, a rigid sphere carrying `kind`'s joint back to `anchor` (entity 0, spawned by
/// [`joint_world`]), and returns its id.
fn spawn_linked(world: &mut World, anchor: EntityId, at: [f32; 3], kind: JointKind) -> EntityId {
    let bob = world.spawn();
    world
        .insert(bob, TestPose::at(at))
        .expect("a fresh entity accepts a pose");
    world
        .insert(bob, rigid(small_sphere(), RigidBody::default()))
        .expect("a fresh entity accepts a collider");
    world
        .insert(bob, Velocity::default())
        .expect("a fresh entity accepts a velocity");
    world
        .insert(
            bob,
            Joint {
                a: anchor,
                b: bob,
                kind,
                anchor: [0.0; 3],
                break_impulse: None,
                collide: false,
            },
        )
        .expect("a fresh entity accepts a joint");
    bob
}

/// A hinge pendulum swinging about `z`, one unit from a fixed anchor at the origin, never leaves
/// that radius and never swings past its authored limit.
#[test]
fn a_hinge_pendulum_keeps_its_anchor_distance_and_stays_inside_its_limits() {
    let (mut world, mut schedule) = joint_world();
    let anchor = world
        .entities()
        .next()
        .expect("joint_world spawns an anchor");
    let bob = spawn_linked(
        &mut world,
        anchor,
        [1.0, 0.0, 0.0],
        JointKind::Hinge {
            axis: [0.0, 0.0, 1.0],
            limits: Some([-90.0, 90.0]),
        },
    );

    for frame in 0..150 {
        tick(&mut world, &mut schedule, frame);
        let pos = pose_of(&world, bob).translation;
        let dist = distance(pos, [0.0; 3]);
        assert!(
            (dist - 1.0).abs() <= 1e-2,
            "tick {frame}: anchor distance drifted to {dist} ({pos:?})"
        );
        let angle = ludion_core::math::acos((pos[0] / dist).clamp(-1.0, 1.0)).to_degrees();
        assert!(
            angle <= 91.0,
            "tick {frame}: swung past its 90 degree limit to {angle} ({pos:?})"
        );
    }
}

/// A ball joint is a hinge with every rotation free: the same one-unit anchor distance holds, with
/// no limit to respect.
#[test]
fn a_ball_joint_keeps_its_anchor_distance() {
    let (mut world, mut schedule) = joint_world();
    let anchor = world
        .entities()
        .next()
        .expect("joint_world spawns an anchor");
    let bob = spawn_linked(
        &mut world,
        anchor,
        [1.0, 0.0, 0.0],
        JointKind::Ball { limits: None },
    );

    for frame in 0..150 {
        tick(&mut world, &mut schedule, frame);
        let dist = distance(pose_of(&world, bob).translation, [0.0; 3]);
        assert!(
            (dist - 1.0).abs() <= 1e-2,
            "tick {frame}: anchor distance drifted to {dist}"
        );
    }
}

/// A vertical slider under gravity settles at its lower limit, never drifting off its axis.
#[test]
fn a_slider_keeps_b_on_axis_and_inside_its_limits() {
    let (mut world, mut schedule) = joint_world();
    let anchor = world
        .entities()
        .next()
        .expect("joint_world spawns an anchor");
    let rider = spawn_linked(
        &mut world,
        anchor,
        [0.0, -0.5, 0.0],
        JointKind::Slider {
            axis: [0.0, 1.0, 0.0],
            limits: Some([-1.5, 0.0]),
        },
    );

    for frame in 0..150 {
        tick(&mut world, &mut schedule, frame);
        let pos = pose_of(&world, rider).translation;
        assert!(
            pos[0].abs() <= 1e-2,
            "tick {frame}: left the axis on x: {pos:?}"
        );
        assert!(
            pos[2].abs() <= 1e-2,
            "tick {frame}: left the axis on z: {pos:?}"
        );
        assert!(
            (-2.0 - 0.05..=-0.5 + 0.05).contains(&pos[1]),
            "tick {frame}: outside its limits: {pos:?}"
        );
    }
}

/// A fixed joint welds `bob` to a static anchor: its pose must not drift under gravity at all.
#[test]
fn a_fixed_joint_keeps_the_relative_pose_under_gravity() {
    let (mut world, mut schedule) = joint_world();
    let anchor = world
        .entities()
        .next()
        .expect("joint_world spawns an anchor");
    let initial = [0.0, -1.0, 0.0];
    let bob = spawn_linked(&mut world, anchor, initial, JointKind::Fixed {});

    for frame in 0..90 {
        tick(&mut world, &mut schedule, frame);
        let drift = distance(pose_of(&world, bob).translation, initial);
        assert!(
            drift <= 1e-2,
            "tick {frame}: a fixed joint must hold its pose, drifted {drift}"
        );
    }
}

/// A fixed joint between a static anchor and a falling body, with `break_impulse` set. Holding
/// `bob` still against gravity settles to a steady impulse magnitude (measured empirically at
/// ~0.041 for this mass under `TickRate::DEFAULT`), so a threshold below that breaks quickly and
/// one far above it never does.
fn fixed_joint_world(break_impulse: Option<f32>) -> (World, EntityId, EntityId, Schedule) {
    fixed_joint_world_with_iterations(break_impulse, DEFAULT_SOLVER_ITERATIONS)
}

/// [`fixed_joint_world`], with `solver_iterations` set explicitly.
fn fixed_joint_world_with_iterations(
    break_impulse: Option<f32>,
    solver_iterations: u32,
) -> (World, EntityId, EntityId, Schedule) {
    let (mut world, schedule) = joint_world_with_iterations(solver_iterations);
    let anchor = world
        .entities()
        .next()
        .expect("joint_world spawns an anchor");
    let bob = world.spawn();
    world
        .insert(bob, TestPose::at([0.0, -1.0, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(bob, rigid(small_sphere(), RigidBody::default()))
        .expect("a fresh entity accepts a collider");
    world
        .insert(bob, Velocity::default())
        .expect("a fresh entity accepts a velocity");
    world
        .insert(
            bob,
            Joint {
                a: anchor,
                b: bob,
                kind: JointKind::Fixed {},
                anchor: [0.0; 3],
                break_impulse,
                collide: false,
            },
        )
        .expect("a fresh entity accepts a joint");
    (world, anchor, bob, schedule)
}

#[test]
fn a_low_break_impulse_breaks_the_joint_and_the_body_then_falls_freely() {
    let (mut world, anchor, bob, mut schedule) = fixed_joint_world(Some(0.02));

    let mut broke_at_y = None;
    for frame in 0..30 {
        tick(&mut world, &mut schedule, frame);
        let events = world
            .resource::<PhysicsWorld>()
            .expect("a claimed body installs the world")
            .joint_events()
            .to_vec();
        if events.is_empty() {
            continue;
        }
        assert_eq!(events.len(), 1, "tick {frame}: {events:?}");
        let event = events[0];
        assert_eq!(event.joint, bob, "the event must name its carrier");
        assert_eq!(event.a, anchor);
        assert_eq!(event.b, bob);
        assert!(
            event.impulse > 0.02,
            "the reported impulse must exceed the threshold"
        );
        assert!(
            world.get::<Joint>(bob).expect("a live entity").is_none(),
            "the mirror must remove the broken joint's component the same tick"
        );
        broke_at_y = Some(pose_of(&world, bob).translation[1]);
        break;
    }
    let y_at_break = broke_at_y.expect("a low break_impulse must break within 30 ticks");

    for frame in 1..40 {
        tick(&mut world, &mut schedule, 1000 + frame);
    }
    let y_later = pose_of(&world, bob).translation[1];
    assert!(
        y_later < y_at_break - 0.1,
        "a broken joint's body must then fall freely: {y_at_break} -> {y_later}"
    );
}

/// A `Joint` re-added to a carrier whose joint just broke must form on the next reconcile, not
/// stay inert forever.
#[test]
fn a_joint_re_added_to_a_broken_carrier_forms() {
    let (mut world, anchor, bob, mut schedule) = fixed_joint_world(Some(0.02));

    let mut broke = false;
    for frame in 0..30 {
        tick(&mut world, &mut schedule, frame);
        if !world
            .resource::<PhysicsWorld>()
            .expect("a claimed body installs the world")
            .joint_events()
            .is_empty()
        {
            broke = true;
            break;
        }
    }
    assert!(broke, "the low break_impulse must break within 30 ticks");
    assert!(
        world.get::<Joint>(bob).expect("a live entity").is_none(),
        "the mirror removes the broken joint's component the same tick"
    );

    world
        .insert(
            bob,
            Joint {
                a: anchor,
                b: bob,
                kind: JointKind::Fixed {},
                anchor: [0.0; 3],
                break_impulse: None,
                collide: false,
            },
        )
        .expect("a live entity accepts a joint");

    tick(&mut world, &mut schedule, 1000);
    assert_eq!(
        world
            .resource::<PhysicsWorld>()
            .expect("still installed")
            .is_joint_active(bob),
        Some(true),
        "a joint re-added to a broken carrier must form instead of staying inert forever"
    );
}

/// `break_impulse` is a per-*step* quantity. The oracle is m·g·dt: the impulse that holds the 1 kg
/// bob against gravity for one tick. The raw per-substep magnitude the solver writes back is about
/// `1/substeps` of that, so an unscaled report misses it at both 4 and 8 iterations.
#[test]
fn break_impulse_is_the_same_per_step_quantity_at_4_and_8_solver_iterations() {
    let reported_impulse = |solver_iterations: u32| -> f32 {
        let (mut world, _anchor, bob, mut schedule) =
            fixed_joint_world_with_iterations(Some(0.02), solver_iterations);
        for frame in 0..30 {
            tick(&mut world, &mut schedule, frame);
            let events = world
                .resource::<PhysicsWorld>()
                .expect("a claimed body installs the world")
                .joint_events()
                .to_vec();
            if let [event] = events.as_slice() {
                assert_eq!(event.b, bob, "the event must name its carrier");
                return event.impulse;
            }
        }
        panic!(
            "solver_iterations {solver_iterations}: the low break_impulse must break within 30 ticks"
        );
    };

    let at_4 = reported_impulse(4);
    let at_8 = reported_impulse(8);
    let expected = 9.81 / 60.0;
    for (iterations, impulse) in [(4, at_4), (8, at_8)] {
        assert!(
            (impulse - expected).abs() <= expected * 0.15,
            "break_impulse at {iterations} solver iterations must be the per-step m*g*dt \
             ({expected}) within 15%: reported {impulse}"
        );
    }
}

/// Fix 2: a slider's own free axis carries no locked-DOF impulse, so its end-stop limit impulse
/// must count toward the break on its own, or a slider slamming its limit can never break.
#[test]
fn a_slider_breaks_when_it_slams_its_limit() {
    let (mut world, mut schedule) = joint_world();
    let anchor = world
        .entities()
        .next()
        .expect("joint_world spawns an anchor");
    let rider = spawn_linked(
        &mut world,
        anchor,
        [0.0, -0.5, 0.0],
        JointKind::Slider {
            axis: [0.0, 1.0, 0.0],
            limits: Some([-0.5, 0.0]),
        },
    );
    world
        .get_mut::<Joint>(rider)
        .expect("a live entity")
        .expect("just linked")
        .break_impulse = Some(0.02);

    let mut broke = false;
    for frame in 0..60 {
        tick(&mut world, &mut schedule, frame);
        if !world
            .resource::<PhysicsWorld>()
            .expect("a claimed body installs the world")
            .joint_events()
            .is_empty()
        {
            broke = true;
            break;
        }
    }
    assert!(
        broke,
        "a slider slamming its limit under a low break_impulse must break within 60 ticks"
    );
}

#[test]
fn a_high_break_impulse_holds() {
    let (mut world, _anchor, bob, mut schedule) = fixed_joint_world(Some(1000.0));
    for frame in 0..120 {
        tick(&mut world, &mut schedule, frame);
        assert!(
            world.get::<Joint>(bob).expect("a live entity").is_some(),
            "tick {frame}: a high break_impulse must hold"
        );
    }
}

/// `b` despawning leaves the joint inert rather than deleted, and it is queued on
/// `take_missing_joints` exactly on the transition into inert — no panic anywhere along the way.
#[test]
fn a_missing_body_leaves_the_joint_inert_and_is_reported() {
    let (mut world, mut schedule) = joint_world();
    let anchor = world
        .entities()
        .next()
        .expect("joint_world spawns an anchor");
    let bob = world.spawn();
    world
        .insert(bob, TestPose::at([1.0, 0.0, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(bob, rigid(small_sphere(), RigidBody::default()))
        .expect("a fresh entity accepts a collider");
    world
        .insert(bob, Velocity::default())
        .expect("a fresh entity accepts a velocity");

    // A dedicated carrier, distinct from `a`/`b` — the ragdoll/runtime-grab convention
    // (`Plan/physics.md` design decision 2) — so despawning `b` cannot also remove the `Joint`.
    let carrier = world.spawn();
    world
        .insert(
            carrier,
            Joint {
                a: anchor,
                b: bob,
                kind: JointKind::Fixed {},
                anchor: [0.0; 3],
                break_impulse: None,
                collide: false,
            },
        )
        .expect("a fresh entity accepts a joint");

    tick(&mut world, &mut schedule, 0);
    assert_eq!(
        world
            .resource::<PhysicsWorld>()
            .expect("a claimed body installs the world")
            .is_joint_active(carrier),
        Some(true)
    );

    world.despawn(bob).expect("a live entity despawns");
    tick(&mut world, &mut schedule, 1);

    {
        let physics = world
            .resource_mut::<PhysicsWorld>()
            .expect("still installed");
        assert_eq!(
            physics.is_joint_active(carrier),
            Some(false),
            "a missing body leaves the joint inert, not deleted"
        );
        assert_eq!(physics.take_missing_joints(), vec![carrier]);
    }

    // No panic on further ticks, and — since nothing transitioned — the carrier is not queued
    // again while `b` stays gone.
    tick(&mut world, &mut schedule, 2);
    assert_eq!(
        world
            .resource_mut::<PhysicsWorld>()
            .expect("still installed")
            .take_missing_joints(),
        Vec::<EntityId>::new(),
        "a standing-missing carrier must not be re-queued every tick"
    );
}

/// A bare host (`render`, `sweep`) never calls `take_missing_joints`, so a carrier that stays
/// inert must be queued once — on the transition — never again, or the queue grows without bound
/// for the life of the session.
#[test]
fn an_inert_joint_queues_its_carrier_exactly_once_over_ten_ticks() {
    let (mut world, mut schedule) = joint_world();
    let anchor = world
        .entities()
        .next()
        .expect("joint_world spawns an anchor");
    let bob = world.spawn();
    world
        .insert(bob, TestPose::at([1.0, 0.0, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(bob, rigid(small_sphere(), RigidBody::default()))
        .expect("a fresh entity accepts a collider");
    world
        .insert(bob, Velocity::default())
        .expect("a fresh entity accepts a velocity");

    let carrier = world.spawn();
    world
        .insert(
            carrier,
            Joint {
                a: anchor,
                b: bob,
                kind: JointKind::Fixed {},
                anchor: [0.0; 3],
                break_impulse: None,
                collide: false,
            },
        )
        .expect("a fresh entity accepts a joint");

    tick(&mut world, &mut schedule, 0);
    world.despawn(bob).expect("a live entity despawns");

    let mut queued = Vec::new();
    for frame in 1..11 {
        tick(&mut world, &mut schedule, frame);
        queued.extend(
            world
                .resource_mut::<PhysicsWorld>()
                .expect("still installed")
                .take_missing_joints(),
        );
    }
    assert_eq!(
        queued,
        vec![carrier],
        "an inert joint must be queued exactly once across ten ticks of staying missing"
    );
}

const CHAIN_TICKS: u64 = 150;
const CHAIN_SNAPSHOT_TICK: u64 = 80;

/// A static anchor and two hinge-linked rigid spheres, each carrying its own joint back to its
/// parent — the ragdoll convention (`Plan/physics.md` design decision 2). Returns
/// `(anchor, link1, link2)`.
fn build_hinge_chain(world: &mut World) -> (EntityId, EntityId, EntityId) {
    let anchor = world.spawn();
    world
        .insert(anchor, TestPose::at([0.0; 3]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(anchor, static_body(small_sphere()))
        .expect("a fresh entity accepts a collider");

    let link1 = spawn_linked(
        world,
        anchor,
        [1.0, 0.0, 0.0],
        JointKind::Hinge {
            axis: [0.0, 0.0, 1.0],
            limits: None,
        },
    );
    let link2 = spawn_linked(
        world,
        link1,
        [2.0, 0.0, 0.0],
        JointKind::Hinge {
            axis: [0.0, 0.0, 1.0],
            limits: None,
        },
    );
    (anchor, link1, link2)
}

fn chain_schedule() -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose>(&mut schedule, PhysicsSpace::Free);
    schedule
}

fn chain_trace() -> Vec<(TestPose, TestPose)> {
    let mut world = World::new();
    let (_, link1, link2) = build_hinge_chain(&mut world);
    let mut schedule = chain_schedule();
    (0..CHAIN_TICKS)
        .map(|frame| {
            tick(&mut world, &mut schedule, frame);
            (pose_of(&world, link1), pose_of(&world, link2))
        })
        .collect()
}

/// Everything a session needs to be rebuilt from nothing, the `pile.rs` `Capture` precedent
/// extended with each entity's `Joint`.
struct Capture {
    entities: Vec<EntityId>,
    colliders: Vec<Option<Collider>>,
    poses: Vec<TestPose>,
    velocities: Vec<Option<Velocity>>,
    joints: Vec<Option<Joint>>,
    bytes: Vec<u8>,
}

fn capture(world: &World) -> Capture {
    let entities: Vec<EntityId> = world.entities().collect();
    Capture {
        colliders: entities
            .iter()
            .map(|&entity| {
                world
                    .get::<Collider>(entity)
                    .expect("a live entity")
                    .cloned()
            })
            .collect(),
        poses: entities
            .iter()
            .map(|&entity| pose_of(world, entity))
            .collect(),
        velocities: entities
            .iter()
            .map(|&entity| {
                world
                    .get::<Velocity>(entity)
                    .expect("a live entity")
                    .copied()
            })
            .collect(),
        joints: entities
            .iter()
            .map(|&entity| world.get::<Joint>(entity).expect("a live entity").cloned())
            .collect(),
        bytes: world
            .resource::<PhysicsWorld>()
            .expect("the write system installed a world")
            .to_bytes()
            .expect("the solver state encodes"),
        entities,
    }
}

fn restore(captured: &Capture) -> World {
    let mut world = World::new();
    world
        .restore_entities(&captured.entities)
        .expect("captured live set is valid");
    for (index, &entity) in captured.entities.iter().enumerate() {
        if let Some(collider) = &captured.colliders[index] {
            world
                .insert(entity, collider.clone())
                .expect("a restored entity accepts a collider");
        }
        world
            .insert(entity, captured.poses[index])
            .expect("a restored entity accepts a pose");
        if let Some(velocity) = captured.velocities[index] {
            world
                .insert(entity, velocity)
                .expect("a restored entity accepts a velocity");
        }
        if let Some(joint) = &captured.joints[index] {
            world
                .insert(entity, joint.clone())
                .expect("a restored entity accepts a joint");
        }
    }
    world.insert_resource(
        PhysicsWorld::from_bytes(&captured.bytes, TickRate::DEFAULT)
            .expect("the bytes this run just wrote decode"),
    );
    world
}

#[test]
fn a_snapshot_resumed_mid_swing_continues_a_hinge_chain_bit_identically() {
    let reference = chain_trace();

    let mut world = World::new();
    let (_, link1, link2) = build_hinge_chain(&mut world);
    let mut schedule = chain_schedule();
    for frame in 0..CHAIN_SNAPSHOT_TICK {
        tick(&mut world, &mut schedule, frame);
    }
    let captured = capture(&world);
    drop(world);
    drop(schedule);

    let mut world = restore(&captured);
    let mut schedule = chain_schedule();
    for frame in CHAIN_SNAPSHOT_TICK..CHAIN_TICKS {
        tick(&mut world, &mut schedule, frame);
        assert_eq!(
            (pose_of(&world, link1), pose_of(&world, link2)),
            reference[frame as usize],
            "the resumed chain diverges at tick {frame}"
        );
    }
}

#[test]
fn joint_events_survive_a_snapshot_round_trip() {
    let (mut world, _anchor, _bob, mut schedule) = fixed_joint_world(Some(0.02));

    let mut broke = false;
    for frame in 0..30 {
        tick(&mut world, &mut schedule, frame);
        if !world
            .resource::<PhysicsWorld>()
            .expect("a claimed body installs the world")
            .joint_events()
            .is_empty()
        {
            broke = true;
            break;
        }
    }
    assert!(broke, "the low break_impulse must break within 30 ticks");

    let physics = world.resource::<PhysicsWorld>().expect("still installed");
    let events_before = physics.joint_events().to_vec();
    assert_eq!(events_before.len(), 1, "exactly one joint broke");
    let bytes = physics.to_bytes().expect("the solver state encodes");

    let restored = PhysicsWorld::from_bytes(&bytes, TickRate::DEFAULT).expect("its own bytes");
    assert_eq!(
        restored.joint_events(),
        events_before.as_slice(),
        "joint_events must survive to_bytes/from_bytes"
    );
}
