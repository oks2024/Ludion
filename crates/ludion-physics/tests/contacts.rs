//! The contact event stream: which pairs started or stopped touching, in a pinned order, across
//! landing, resting, despawn and collider removal.

mod common;

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_ecs::{Schedule, SystemCtx, World};
use ludion_physics::{
    ContactEvent, ContactPhase, PhysicsSettings, PhysicsSpace, PhysicsWorld, install,
};
use ludion_spatial::{Body, Collider, RigidBody, Shape, Velocity};

fn floor_collider() -> Collider {
    Collider {
        shape: Shape::Box {
            half_extents: [20.0, 0.5, 20.0],
        },
        body: Body::new_static(),
        layer: None,
    }
}

fn ball_collider(sleep_after_ticks: u32) -> Collider {
    Collider {
        shape: Shape::Sphere { radius: 0.5 },
        body: Body::Rigid(RigidBody {
            mass: 1.0,
            restitution: 0.0,
            sleep_after_ticks: Some(sleep_after_ticks),
            ..RigidBody::default()
        }),
        layer: None,
    }
}

fn spawn_floor(world: &mut World) -> EntityId {
    let floor = world.spawn();
    world
        .insert(floor, TestPose::at([0.0, -0.5, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(floor, floor_collider())
        .expect("a fresh entity accepts a collider");
    floor
}

fn spawn_ball(world: &mut World, x: f32, y: f32, sleep_after_ticks: u32) -> EntityId {
    spawn_ball_at(world, [x, y, 0.0], sleep_after_ticks)
}

fn spawn_ball_at(world: &mut World, at: [f32; 3], sleep_after_ticks: u32) -> EntityId {
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at(at))
        .expect("a fresh entity accepts a pose");
    world
        .insert(entity, ball_collider(sleep_after_ticks))
        .expect("a fresh entity accepts a collider");
    world
        .insert(entity, Velocity::default())
        .expect("a fresh entity accepts a velocity");
    entity
}

/// `(a, b)` in `EntityId` order, matching [`ContactEvent`]'s own canonicalization.
fn canonical(a: EntityId, b: EntityId) -> (EntityId, EntityId) {
    if a < b { (a, b) } else { (b, a) }
}

fn free_schedule() -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose>(&mut schedule, PhysicsSpace::Free);
    schedule
}

fn events_of(world: &World) -> Vec<ContactEvent> {
    world
        .resource::<PhysicsWorld>()
        .expect("the write system installs the world once a body claims it")
        .contact_events()
        .to_vec()
}

#[test]
fn simultaneous_starts_are_sorted_by_entity() {
    let mut world = World::new();
    let b0 = spawn_ball(&mut world, -6.0, 3.0, 10);
    let b1 = spawn_ball(&mut world, -3.0, 3.0, 10);
    let b2 = spawn_ball(&mut world, 0.0, 3.0, 10);
    let floor = spawn_floor(&mut world);
    let b3 = spawn_ball(&mut world, 3.0, 3.0, 10);
    let mut schedule = free_schedule();

    let mut events = Vec::new();
    for frame in 0..150 {
        tick(&mut world, &mut schedule, frame);
        events = events_of(&world);
        if !events.is_empty() {
            break;
        }
    }
    assert!(!events.is_empty(), "no ball ever touched the floor");

    let expected_pairs = [(b0, floor), (b1, floor), (b2, floor), (floor, b3)];
    assert_eq!(events.len(), expected_pairs.len(), "{events:?}");
    for (event, &(a, b)) in events.iter().zip(&expected_pairs) {
        assert_eq!((event.a, event.b), (a, b), "{events:?}");
        assert_eq!(event.phase, ContactPhase::Started);
        assert!(event.impulse > 0.0, "{event:?}");
    }
}

/// The despawn's `Stopped` literal below also pins the zero point and normal a stop carries.
#[test]
fn a_resting_contact_emits_nothing_more_and_a_despawn_stops_it() {
    let mut world = World::new();
    let floor = spawn_floor(&mut world);
    let ball = spawn_ball(&mut world, 0.0, 2.0, 5);
    let mut schedule = free_schedule();
    let pair = canonical(ball, floor);

    let mut started = false;
    let mut frame = 0;
    loop {
        tick(&mut world, &mut schedule, frame);
        let events = events_of(&world);
        if started {
            assert!(
                events.is_empty(),
                "a resting contact must emit no further event at tick {frame}: {events:?}"
            );
        } else if !events.is_empty() {
            assert_eq!(events.len(), 1, "{events:?}");
            assert_eq!((events[0].a, events[0].b), pair);
            assert_eq!(events[0].phase, ContactPhase::Started);
            assert!(events[0].impulse > 0.0, "{events:?}");
            started = true;
        }
        let asleep = world
            .resource::<PhysicsWorld>()
            .expect("claimed")
            .is_sleeping(ball)
            == Some(true);
        frame += 1;
        if asleep {
            break;
        }
        assert!(frame < 300, "the ball never fell asleep");
    }
    assert!(started, "the ball never touched the floor");
    let height = pose_of(&world, ball).translation[1];
    assert!(
        (height - 0.5).abs() < 0.05,
        "a resting ball should sit at floor height + radius: {height}"
    );

    world.despawn(ball).expect("a live entity despawns");
    tick(&mut world, &mut schedule, frame);
    frame += 1;
    assert_eq!(
        events_of(&world),
        vec![ContactEvent {
            a: pair.0,
            b: pair.1,
            phase: ContactPhase::Stopped,
            impulse: 0.0,
            point: [0.0; 3],
            normal: [0.0; 3],
        }]
    );

    tick(&mut world, &mut schedule, frame);
    let events = events_of(&world);
    assert!(events.is_empty(), "one step's events only: {events:?}");
}

#[test]
fn removing_a_collider_stops_its_pairs_once() {
    let mut world = World::new();
    let floor = spawn_floor(&mut world);
    let ball = spawn_ball(&mut world, 0.0, 2.0, 10);
    let mut schedule = free_schedule();
    let pair = canonical(ball, floor);

    let mut frame = 0;
    loop {
        tick(&mut world, &mut schedule, frame);
        let events = events_of(&world);
        frame += 1;
        if !events.is_empty() {
            break;
        }
        assert!(frame < 200, "the ball never touched the floor");
    }

    world
        .remove::<Collider>(ball)
        .expect("a live entity accepts a component removal");
    tick(&mut world, &mut schedule, frame);
    frame += 1;
    assert_eq!(
        events_of(&world),
        vec![ContactEvent {
            a: pair.0,
            b: pair.1,
            phase: ContactPhase::Stopped,
            impulse: 0.0,
            point: [0.0; 3],
            normal: [0.0; 3],
        }],
        "the entity stays alive with no collider, so `write` drops it from `live` and `retire` \
         removes its body"
    );

    tick(&mut world, &mut schedule, frame);
    frame += 1;
    assert!(
        events_of(&world).is_empty(),
        "no stale pair is left to re-stop"
    );

    world
        .insert(ball, ball_collider(10))
        .expect("a live entity accepts a fresh collider");
    tick(&mut world, &mut schedule, frame);
    let events = events_of(&world);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!((events[0].a, events[0].b), pair);
    assert_eq!(
        events[0].phase,
        ContactPhase::Started,
        "no stale entry in `touching` suppressed the fresh start: {events:?}"
    );
}

/// A game system registered before [`install`] copies [`contact_events`](PhysicsWorld::contact_events)
/// into a log resource every tick; the log at frame `F` must equal what the resource held right
/// after tick `F - 1`, since the physics triple runs last.
fn record_events(world: &mut World, _ctx: &SystemCtx) {
    let events = world
        .resource::<PhysicsWorld>()
        .map(|physics| physics.contact_events().to_vec())
        .unwrap_or_default();
    let mut log = world.remove_resource::<EventLog>().unwrap_or_default();
    log.0.push(events);
    world.insert_resource(log);
}

#[derive(Default)]
struct EventLog(Vec<Vec<ContactEvent>>);

#[test]
fn a_reader_sees_last_ticks_events() {
    const TICKS: u64 = 120;

    let mut world = World::new();
    let _floor = spawn_floor(&mut world);
    let _ball = spawn_ball(&mut world, 0.0, 2.0, 10);

    let mut schedule = Schedule::new();
    schedule.add_system("record_events", record_events);
    install::<TestPose>(&mut schedule, PhysicsSpace::Free);

    let mut after_tick = Vec::with_capacity(TICKS as usize);
    for frame in 0..TICKS {
        tick(&mut world, &mut schedule, frame);
        after_tick.push(events_of(&world));
    }

    let log = &world
        .resource::<EventLog>()
        .expect("record_events always inserts one")
        .0;
    assert_eq!(log.len(), TICKS as usize);
    assert!(
        log[0].is_empty(),
        "no PhysicsWorld exists before the first tick's write"
    );
    for frame in 1..TICKS as usize {
        assert_eq!(
            log[frame],
            after_tick[frame - 1],
            "frame {frame}'s reader must see what tick {}'s step produced",
            frame - 1
        );
    }
    assert!(
        after_tick.iter().any(|events| !events.is_empty()),
        "the ball must land during the run"
    );
}

/// The first `Started` event of a ball dropped onto the floor, under `solver_iterations`.
fn first_contact(solver_iterations: u32) -> ContactEvent {
    let mut world = World::new();
    spawn_floor(&mut world);
    spawn_ball(&mut world, 0.0, 2.0, 10);
    let mut schedule = Schedule::new();
    let settings = PhysicsSettings {
        solver_iterations,
        ..PhysicsSettings::new(PhysicsSpace::Free)
    };
    install::<TestPose>(&mut schedule, settings);
    for frame in 0..150 {
        tick(&mut world, &mut schedule, frame);
        if let Some(event) = events_of(&world)
            .iter()
            .find(|event| event.phase == ContactPhase::Started)
        {
            return *event;
        }
    }
    panic!("solver_iterations {solver_iterations}: the ball never touched the floor");
}

/// A contact's impulse is the step's, not a substep's: the same drop reports the same magnitude
/// whether the solver runs 1, 4 or 8 substeps.
#[test]
fn contact_impulse_is_the_same_per_step_quantity_at_1_4_and_8_solver_iterations() {
    let impulses = [1, 4, 8].map(|iterations| first_contact(iterations).impulse);
    for impulse in impulses {
        assert!(impulse > 0.0, "{impulses:?}");
        let relative = (impulse - impulses[1]).abs() / impulses[1];
        assert!(relative < 0.01, "{impulses:?}");
    }
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ludion_core::math::sqrt(
        (a[0] - b[0]) * (a[0] - b[0])
            + (a[1] - b[1]) * (a[1] - b[1])
            + (a[2] - b[2]) * (a[2] - b[2]),
    )
}

fn length(v: [f32; 3]) -> f32 {
    distance(v, [0.0; 3])
}

fn close(a: [f32; 3], b: [f32; 3], tolerance: f32) -> bool {
    distance(a, b) <= tolerance
}

fn bits(v: [f32; 3]) -> [u32; 3] {
    v.map(f32::to_bits)
}

/// The first tick's events, stepping until any appear.
fn first_starts(world: &mut World, schedule: &mut Schedule) -> Vec<ContactEvent> {
    for frame in 0..150 {
        tick(world, schedule, frame);
        let events = events_of(world);
        if !events.is_empty() {
            return events;
        }
    }
    panic!("nothing ever touched");
}

/// A start's point sits on the ball's foot on the floor and its normal points from `a` to `b`
/// whichever side the floor took in the canonical pair.
#[test]
fn a_start_carries_the_point_and_an_a_to_b_normal() {
    let mut world = World::new();
    let ball_low = spawn_ball_at(&mut world, [-2.0, 1.0, 1.0], 10);
    let floor = spawn_floor(&mut world);
    let ball_high = spawn_ball_at(&mut world, [1.5, 1.0, -0.75], 10);
    let mut schedule = free_schedule();

    let events = first_starts(&mut world, &mut schedule);
    assert_eq!(events.len(), 2, "{events:?}");
    for event in &events {
        assert_eq!(event.phase, ContactPhase::Started);
        assert!((length(event.normal) - 1.0).abs() < 1e-4, "{event:?}");
        let (ball, up) = if event.a == floor {
            (event.b, [0.0, 1.0, 0.0])
        } else {
            (event.a, [0.0, -1.0, 0.0])
        };
        assert!(ball == ball_low || ball == ball_high, "{event:?}");
        assert!(close(event.normal, up, 1e-4), "{event:?}");
        let at = pose_of(&world, ball).translation;
        assert!(
            close(event.point, [at[0], 0.0, at[2]], 0.05),
            "{event:?} vs ball at {at:?}"
        );
    }
    assert!(
        events.iter().any(|event| event.a == floor) && events.iter().any(|event| event.b == floor),
        "one ball on each side of the floor's canonical order: {events:?}"
    );
}

/// A gravity-free rigid box at pose `(0, y, 0)` with `rotation`, above the floor.
fn boxed_start(rotation: [f32; 4], y: f32) -> ContactEvent {
    let mut world = World::new();
    spawn_floor(&mut world);
    let boxed = world.spawn();
    world
        .insert(
            boxed,
            TestPose {
                translation: [0.0, y, 0.0],
                rotation,
            },
        )
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            boxed,
            Collider {
                shape: Shape::Box {
                    half_extents: [0.5, 0.5, 0.5],
                },
                body: Body::Rigid(RigidBody::default()),
                layer: None,
            },
        )
        .expect("a fresh entity accepts a collider");
    world
        .insert(boxed, Velocity::default())
        .expect("a fresh entity accepts a velocity");
    let mut schedule = Schedule::new();
    let settings = PhysicsSettings {
        gravity: [0.0; 3],
        ..PhysicsSettings::new(PhysicsSpace::Free)
    };
    install::<TestPose>(&mut schedule, settings);
    let events = first_starts(&mut world, &mut schedule);
    assert_eq!(events.len(), 1, "{events:?}");
    events[0]
}

/// Touching flush, or with the low edge just inside, and no impulse yet, the pick resolves by depth,
/// then by the point, never by solver order.
#[test]
fn a_zero_impulse_start_picks_a_deterministic_corner() {
    let flat = boxed_start([0.0, 0.0, 0.0, 1.0], 0.5);
    assert_eq!(flat.phase, ContactPhase::Started);
    assert_eq!(flat.impulse, 0.0, "{flat:?}");
    let corners = [-0.5_f32, 0.5]
        .into_iter()
        .flat_map(|x| [-0.5_f32, 0.5].map(|z| [x, 0.0, z]));
    assert!(
        corners
            .into_iter()
            .any(|corner| close(flat.point, corner, 0.02)),
        "{flat:?}"
    );
    let again = boxed_start([0.0, 0.0, 0.0, 1.0], 0.5);
    assert_eq!(bits(flat.point), bits(again.point));
    assert_eq!(bits(flat.normal), bits(again.normal));

    // Tilted about z by 0.05 rad so the -x edge is the low one, just inside the floor.
    let tilted = boxed_start([0.0, 0.0, 0.024_997_4, 0.999_687_5], 0.523_865);
    assert_eq!(tilted.impulse, 0.0, "{tilted:?}");
    assert!(
        tilted.point[0] < 0.0,
        "depth decides before the point: {tilted:?}"
    );
    let again = boxed_start([0.0, 0.0, 0.024_997_4, 0.999_687_5], 0.523_865);
    assert_eq!(bits(tilted.point), bits(again.point));
    assert_eq!(bits(tilted.normal), bits(again.normal));
}

/// A box dropped flat on a heightfield: the composite path, where solver contacts come from
/// clustered manifolds. The heightfield takes the lower `EntityId` when `ground_first`.
fn heightfield_start(ground_first: bool) -> ContactEvent {
    let mut world = World::new();
    let (ground, boxed) = if ground_first {
        let ground = world.spawn();
        (ground, world.spawn())
    } else {
        let boxed = world.spawn();
        (world.spawn(), boxed)
    };
    world
        .insert(ground, TestPose::at([0.0, 0.0, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            ground,
            Collider {
                shape: Shape::Heightfield {
                    rows: 5,
                    cols: 5,
                    heights: vec![0.0; 25],
                    scale: [20.0, 1.0, 20.0],
                },
                body: Body::new_static(),
                layer: None,
            },
        )
        .expect("a fresh entity accepts a collider");
    world
        .insert(boxed, TestPose::at([0.3, 1.0, -0.2]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            boxed,
            Collider {
                shape: Shape::Box {
                    half_extents: [0.5, 0.5, 0.5],
                },
                body: Body::Rigid(RigidBody::default()),
                layer: None,
            },
        )
        .expect("a fresh entity accepts a collider");
    world
        .insert(boxed, Velocity::default())
        .expect("a fresh entity accepts a velocity");
    let mut schedule = free_schedule();
    let events = first_starts(&mut world, &mut schedule);
    assert_eq!(events.len(), 1, "{events:?}");
    let event = events[0];
    assert_eq!(
        event.a == ground,
        ground_first,
        "the heightfield is `a` exactly when it was spawned first: {event:?}"
    );
    event
}

/// The composite path flips like the primitive one: with the box the lower `EntityId` the pair
/// swaps against the collider order, and the normal still points from `a` (the box) to `b` (the
/// heightfield), so down.
#[test]
fn a_heightfield_start_flips_its_normal_when_the_box_is_the_lower_entity() {
    let box_first = heightfield_start(false);
    let ground_first = heightfield_start(true);
    assert!(
        close(box_first.normal, [0.0, -1.0, 0.0], 1e-3),
        "the box is `a`, so the normal points down onto the ground: {box_first:?}"
    );
    assert!(
        close(ground_first.normal, [0.0, 1.0, 0.0], 1e-3),
        "{ground_first:?}"
    );
    assert!(box_first.point[1].abs() < 0.05, "{box_first:?}");
}

/// A ball dropped on a static box floor rotated `angle` about z, the ramp taking the lower
/// `EntityId` when `ramp_first`. Returns the start and the ramp's world up normal.
fn ramp_start(ramp_first: bool, angle: f32) -> (ContactEvent, [f32; 3]) {
    let mut world = World::new();
    let (ramp, ball) = if ramp_first {
        let ramp = world.spawn();
        (ramp, world.spawn())
    } else {
        let ball = world.spawn();
        (world.spawn(), ball)
    };
    let half = angle * 0.5;
    world
        .insert(
            ramp,
            TestPose {
                translation: [0.0; 3],
                rotation: [
                    0.0,
                    0.0,
                    ludion_core::math::sin(half),
                    ludion_core::math::cos(half),
                ],
            },
        )
        .expect("a fresh entity accepts a pose");
    world
        .insert(ramp, floor_collider())
        .expect("a fresh entity accepts a collider");
    world
        .insert(ball, TestPose::at([0.0, 2.0, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(ball, ball_collider(10))
        .expect("a fresh entity accepts a collider");
    world
        .insert(ball, Velocity::default())
        .expect("a fresh entity accepts a velocity");
    let mut schedule = free_schedule();
    let events = first_starts(&mut world, &mut schedule);
    assert_eq!(events.len(), 1, "{events:?}");
    let event = events[0];
    assert_eq!(event.a == ramp, ramp_first, "{event:?}");
    let up = [
        -ludion_core::math::sin(angle),
        ludion_core::math::cos(angle),
        0.0,
    ];
    (event, up)
}

/// A rotated first collider: the normal is the ramp's world normal, signed by which side the ramp
/// took in the canonical pair, and the point lies on the ramp's surface plane.
#[test]
fn a_start_on_a_rotated_ramp_carries_the_ramps_world_normal() {
    for ramp_first in [true, false] {
        let (event, up) = ramp_start(ramp_first, 0.3);
        let expected = if ramp_first { up } else { up.map(|c| -c) };
        assert!(
            close(event.normal, expected, 1e-3),
            "ramp_first {ramp_first}: {event:?} vs {expected:?}"
        );
        // The ramp's top face is the plane `up . p = 0.5` (half-height 0.5, centred on the origin).
        let along = event.point[0] * up[0] + event.point[1] * up[1] + event.point[2] * up[2];
        assert!(
            (along - 0.5).abs() < 0.05,
            "ramp_first {ramp_first}: the point is off the ramp surface: {event:?}"
        );
    }
}

#[test]
fn a_face_contact_on_a_heightfield_is_deterministic() {
    let first = heightfield_start(true);
    let second = heightfield_start(true);
    assert_eq!(first.phase, ContactPhase::Started);
    assert_eq!(bits(first.point), bits(second.point));
    assert_eq!(bits(first.normal), bits(second.normal));
    assert_eq!(first.impulse.to_bits(), second.impulse.to_bits());
    assert!(first.point[1].abs() < 0.05, "{first:?}");
    assert!(
        close(first.normal, [0.0, 1.0, 0.0], 1e-3),
        "the heightfield spawned first, so the normal points up: {first:?}"
    );
}

/// A single-contact start (a sphere) picks the same point and normal at any solver iteration count
/// to tolerance; a multi-point start makes no such promise, since the count redistributes the
/// impulses the pick ranks by.
#[test]
fn contact_point_and_normal_are_per_step_at_1_4_and_8_solver_iterations() {
    let events = [1, 4, 8].map(first_contact);
    for event in events {
        assert!(close(event.point, events[1].point, 0.02), "{events:?}");
        assert!(close(event.normal, events[1].normal, 1e-4), "{events:?}");
    }
}
