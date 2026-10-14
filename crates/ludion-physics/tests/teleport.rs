//! Writing a rigid body's pose is a teleport: the body keeps its handle, its velocity, its joints'
//! captured zeros and its contacts' bookkeeping, and the solver sees the new pose at the next
//! step. One test per claim in `ludion docs physics` §Teleports.

mod common;

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_ecs::{Schedule, World};
use ludion_physics::{
    Blast, ContactPhase, Falloff, PhysicsSettings, PhysicsSpace, PhysicsWorld, QueryFilter, install,
};
use ludion_spatial::{Body, Collider, Joint, JointKind, RigidBody, Shape, Velocity};

fn schedule_with(gravity: [f32; 3]) -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose>(
        &mut schedule,
        PhysicsSettings {
            gravity,
            ..PhysicsSettings::new(PhysicsSpace::Free)
        },
    );
    schedule
}

fn physics(world: &mut World) -> &mut PhysicsWorld {
    world
        .resource_mut::<PhysicsWorld>()
        .expect("the write system installed a world")
}

fn sphere() -> Shape {
    Shape::Sphere { radius: 0.5 }
}

fn rigid(shape: Shape, sleep_after_ticks: Option<u32>, gravity_scale: f32) -> Collider {
    Collider {
        shape,
        body: Body::Rigid(RigidBody {
            sleep_after_ticks,
            gravity_scale,
            ..RigidBody::default()
        }),
        layer: None,
    }
}

fn fixed(shape: Shape) -> Collider {
    Collider {
        shape,
        body: Body::new_static(),
        layer: None,
    }
}

fn spawn(world: &mut World, at: [f32; 3], collider: Collider) -> EntityId {
    let entity = world.spawn();
    world.insert(entity, TestPose::at(at)).expect("a pose");
    world.insert(entity, collider).expect("a collider");
    world
        .insert(entity, Velocity::default())
        .expect("a velocity");
    entity
}

fn floor(world: &mut World) -> EntityId {
    let half_extents = [20.0, 0.5, 20.0];
    spawn(world, [0.0, -0.5, 0.0], fixed(Shape::Box { half_extents }))
}

/// What a game system does: write the `Transform` (here the test pose).
fn teleport(world: &mut World, entity: EntityId, to: [f32; 3]) {
    world
        .get_mut::<TestPose>(entity)
        .expect("a live entity")
        .expect("its pose")
        .translation = to;
}

fn velocity_of(world: &World, entity: EntityId) -> Velocity {
    *world
        .get::<Velocity>(entity)
        .expect("a live entity")
        .expect("its velocity")
}

fn run(world: &mut World, schedule: &mut Schedule, frames: std::ops::Range<u64>) {
    for frame in frames {
        tick(world, schedule, frame);
    }
}

/// A teleport leaves the velocity alone; writing `Velocity` alongside sets it.
#[test]
fn a_teleport_keeps_the_velocity_unless_the_game_also_writes_it() {
    let mut world = World::new();
    let moved = spawn(&mut world, [0.0, 100.0, 0.0], rigid(sphere(), None, 1.0));
    let twin = spawn(&mut world, [5.0, 100.0, 0.0], rigid(sphere(), None, 1.0));
    let mut schedule = schedule_with([0.0, -9.81, 0.0]);
    run(&mut world, &mut schedule, 0..20);

    teleport(&mut world, moved, [0.0, 300.0, 0.0]);
    tick(&mut world, &mut schedule, 20);

    assert_eq!(
        velocity_of(&world, moved).linear.map(f32::to_bits),
        velocity_of(&world, twin).linear.map(f32::to_bits),
        "the teleported body's velocity matches an untouched twin's"
    );
    assert!(velocity_of(&world, moved).linear[1] < -3.0);

    teleport(&mut world, moved, [0.0, 400.0, 0.0]);
    world
        .get_mut::<Velocity>(moved)
        .expect("a live entity")
        .expect("its velocity")
        .linear = [0.0, 5.0, 0.0];
    tick(&mut world, &mut schedule, 21);
    let written = velocity_of(&world, moved).linear[1];
    assert!(
        (written - (5.0 - 9.81 / 60.0)).abs() < 1e-3,
        "the written velocity governs the step: {written}"
    );
}

/// A sleeping body that is teleported wakes, and its sleep countdown starts over.
#[test]
fn a_teleport_wakes_a_sleeping_body_and_restarts_its_sleep_countdown() {
    let mut world = World::new();
    let drifter = spawn(&mut world, [0.0, 10.0, 0.0], rigid(sphere(), Some(10), 0.0));
    let mut schedule = schedule_with([0.0, -9.81, 0.0]);
    tick(&mut world, &mut schedule, 0);
    let mut frame = 1;
    while physics(&mut world).is_sleeping(drifter) != Some(true) {
        assert!(frame < 300, "the weightless body never fell asleep");
        tick(&mut world, &mut schedule, frame);
        frame += 1;
    }

    teleport(&mut world, drifter, [0.0, 20.0, 0.0]);
    tick(&mut world, &mut schedule, frame);
    assert_eq!(physics(&mut world).is_sleeping(drifter), Some(false));
    assert_eq!(pose_of(&world, drifter).translation[1], 20.0);

    run(&mut world, &mut schedule, frame + 1..frame + 6);
    assert_eq!(
        physics(&mut world).is_sleeping(drifter),
        Some(false),
        "the countdown restarted: it is still awake well inside `sleep_after_ticks`"
    );
    run(&mut world, &mut schedule, frame + 6..frame + 60);
    assert_eq!(physics(&mut world).is_sleeping(drifter), Some(true));
}

/// Two weightless rigid parts, an anchor at the origin and a bob hinged to it one unit out at +x.
fn hinged_pair() -> (World, Schedule, EntityId, EntityId) {
    let mut world = World::new();
    let anchor = spawn(&mut world, [0.0; 3], rigid(sphere(), None, 0.0));
    let bob = spawn(&mut world, [1.0, 0.0, 0.0], rigid(sphere(), None, 0.0));
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
        .expect("a joint");
    let mut schedule = schedule_with([0.0, -9.81, 0.0]);
    run(&mut world, &mut schedule, 0..120);
    (world, schedule, anchor, bob)
}

fn offset(world: &World, anchor: EntityId, bob: EntityId) -> [f32; 3] {
    let (a, b) = (pose_of(world, anchor), pose_of(world, bob));
    std::array::from_fn(|i| b.translation[i] - a.translation[i])
}

/// Moving every body of a jointed figure by the same translation in one tick keeps the joint: the
/// offset between the bodies is unchanged and nothing drifts, because the joint zeros were
/// captured once and are not re-captured.
#[test]
fn teleporting_every_body_by_the_same_translation_keeps_the_joint() {
    let (mut world, mut schedule, anchor, bob) = hinged_pair();
    let before = offset(&world, anchor, bob);
    let (a, b) = (
        pose_of(&world, anchor).translation,
        pose_of(&world, bob).translation,
    );
    let shift = [30.0, 7.0, -4.0];

    teleport(&mut world, anchor, std::array::from_fn(|i| a[i] + shift[i]));
    teleport(&mut world, bob, std::array::from_fn(|i| b[i] + shift[i]));
    run(&mut world, &mut schedule, 120..240);

    let after = offset(&world, anchor, bob);
    for i in 0..3 {
        assert!(
            (after[i] - before[i]).abs() < 0.02,
            "the joint offset holds: {before:?} -> {after:?}"
        );
    }
    assert_eq!(physics(&mut world).is_joint_active(bob), Some(true));
}

/// Moving one body of a jointed pair does not move the joint's zero: the joint pulls it back.
#[test]
fn teleporting_one_body_of_a_jointed_pair_is_pulled_back() {
    let (mut world, mut schedule, anchor, bob) = hinged_pair();
    let b = pose_of(&world, bob).translation;
    teleport(&mut world, bob, [b[0], b[1] + 5.0, b[2]]);
    let distance = |world: &World| {
        let o = offset(world, anchor, bob);
        (o[0] * o[0] + o[1] * o[1] + o[2] * o[2]).sqrt()
    };

    let far = distance(&world);
    run(&mut world, &mut schedule, 120..240);

    assert!(far > 5.0, "the body starts far away: {far}");
    assert!(
        distance(&world) < 2.0,
        "the joint pulls it back toward the other part: {far} -> {}",
        distance(&world)
    );
}

/// A teleport out of contact emits `Stopped` on that tick, and into contact `Started`.
#[test]
fn a_teleport_out_of_contact_stops_and_into_contact_starts_the_pair() {
    let mut world = World::new();
    let ground = floor(&mut world);
    let ball = spawn(&mut world, [0.0, 0.5, 0.0], rigid(sphere(), None, 1.0));
    let mut schedule = schedule_with([0.0, -9.81, 0.0]);
    run(&mut world, &mut schedule, 0..60);

    teleport(&mut world, ball, [0.0, 8.0, 0.0]);
    tick(&mut world, &mut schedule, 60);
    let events = physics(&mut world).contact_events().to_vec();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!((events[0].a, events[0].b), (ground, ball));
    assert_eq!(events[0].phase, ContactPhase::Stopped);

    teleport(&mut world, ball, [0.0, 0.49, 0.0]);
    tick(&mut world, &mut schedule, 61);
    let events = physics(&mut world).contact_events().to_vec();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].phase, ContactPhase::Started);
}

/// A body resting on the teleported one is woken with it (`wake_up(true)` wakes the touching
/// island) and falls, rather than floating where its carrier was.
#[test]
fn a_sleeping_rider_wakes_and_falls_when_its_carrier_is_teleported_away() {
    let mut world = World::new();
    floor(&mut world);
    let half_extents = [1.0, 0.5, 1.0];
    let carrier = spawn(
        &mut world,
        [0.0, 0.5, 0.0],
        rigid(Shape::Box { half_extents }, Some(5), 1.0),
    );
    let rider = spawn(&mut world, [0.0, 1.5, 0.0], rigid(sphere(), Some(5), 1.0));
    let mut schedule = schedule_with([0.0, -9.81, 0.0]);
    tick(&mut world, &mut schedule, 0);
    let mut frame = 1;
    while physics(&mut world).is_sleeping(rider) != Some(true)
        || physics(&mut world).is_sleeping(carrier) != Some(true)
    {
        assert!(frame < 600, "the stack never fell asleep");
        tick(&mut world, &mut schedule, frame);
        frame += 1;
    }
    let resting = pose_of(&world, rider).translation;

    teleport(&mut world, carrier, [10.0, 0.5, 0.0]);
    tick(&mut world, &mut schedule, frame);
    assert_eq!(
        physics(&mut world).is_sleeping(rider),
        Some(false),
        "the rider is woken on the teleport tick"
    );
    run(&mut world, &mut schedule, frame + 1..frame + 40);

    let landed = pose_of(&world, rider).translation[1];
    assert!(
        landed < resting[1] - 0.5,
        "the rider fell: {resting:?} -> {landed}"
    );
}

/// A query made between a pose write and the next step reads the last completed step: the body
/// is not at the new position until the tick runs.
#[test]
fn a_query_does_not_see_a_teleport_until_the_next_step() {
    let mut world = World::new();
    let ball = spawn(&mut world, [0.0, 5.0, 0.0], rigid(sphere(), None, 0.0));
    let mut schedule = schedule_with([0.0; 3]);
    run(&mut world, &mut schedule, 0..3);
    let through_new_position = |world: &mut World| {
        physics(world)
            .ray_cast(
                [10.0, 5.0, 10.0],
                [0.0, 0.0, -1.0],
                20.0,
                &QueryFilter::new(),
            )
            .expect("a valid ray")
    };

    teleport(&mut world, ball, [10.0, 5.0, 0.0]);
    assert!(through_new_position(&mut world).is_empty());

    tick(&mut world, &mut schedule, 3);
    let hits = through_new_position(&mut world);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].entity, ball);
}

/// A blast queued on the tick a body is teleported selects against the pre-teleport collider pose
/// (the query pipeline only syncs in the step), while its impulse lands on the moved body.
#[test]
fn a_blast_on_the_teleport_tick_selects_the_body_at_its_old_position() {
    let mut world = World::new();
    let ball = spawn(&mut world, [0.0, 0.0, 0.0], rigid(sphere(), None, 0.0));
    let mut schedule = schedule_with([0.0; 3]);
    tick(&mut world, &mut schedule, 0);

    teleport(&mut world, ball, [10.0, 0.0, 0.0]);
    physics(&mut world)
        .radial_impulse(Blast {
            center: [0.0, -1.0, 0.0],
            radius: 3.0,
            impulse: 6.0,
            falloff: Falloff::Constant,
            filter: QueryFilter::new(),
        })
        .expect("a valid blast queues");
    tick(&mut world, &mut schedule, 1);

    let hits = physics(&mut world).blast_hits().to_vec();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].entity, ball);
    assert!(
        velocity_of(&world, ball).linear[1] > 1.0,
        "the impulse lands on the already-moved body"
    );
    assert_eq!(pose_of(&world, ball).translation[0], 10.0);
}

/// A static body's pose is re-read every tick: writing its `Transform` moves it in the solver.
#[test]
fn a_static_body_follows_a_pose_write() {
    let mut world = World::new();
    let wall = spawn(&mut world, [0.0; 3], fixed(sphere()));
    let mut schedule = schedule_with([0.0; 3]);
    run(&mut world, &mut schedule, 0..3);

    teleport(&mut world, wall, [10.0, 0.0, 0.0]);
    tick(&mut world, &mut schedule, 3);

    let at = |world: &mut World, x: f32| {
        physics(world)
            .ray_cast([x, 0.0, 10.0], [0.0, 0.0, -1.0], 20.0, &QueryFilter::new())
            .expect("a valid ray")
    };
    assert!(at(&mut world, 0.0).is_empty(), "the old place is empty");
    assert_eq!(at(&mut world, 10.0).len(), 1, "the collider follows");
}
