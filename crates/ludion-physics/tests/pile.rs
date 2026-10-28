//! The determinism gates: a chaotic pile traces identically run to run and against a committed
//! reference, and a run resumed from snapshot bytes continues byte-identically.

mod common;

use std::path::PathBuf;

use common::{TestPose, tick};
use ludion_core::ids::EntityId;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, SystemCtx, World};
use ludion_physics::{ContactEvent, ContactPhase, PhysicsSpace, PhysicsWorld, install};
use ludion_spatial::{Body, Collider, RigidBody, Shape, Velocity};
use xxhash_rust::xxh3::Xxh3;

/// One trace line: the frame index and an XXH3 digest of every body's pose, read in the handle
/// table's creation order — the only order this crate iterates.
///
/// Hashing the mirrored *components* rather than the solver's own state is deliberate: the
/// components are what reaches the world hash, so this trace fails exactly where a real scene's
/// hash trace would.
fn trace_line(world: &World, frame: u64) -> String {
    let physics = world
        .resource::<PhysicsWorld>()
        .expect("the write system installs the world on the first tick");
    let mut digest = Xxh3::new();
    for entity in physics.entities() {
        let pose = common::pose_of(world, entity);
        for value in pose.translation {
            digest.update(&value.to_bits().to_le_bytes());
        }
        for value in pose.rotation {
            digest.update(&value.to_bits().to_le_bytes());
        }
    }
    format!("{frame:04} {:016x}", digest.digest())
}

/// Boxes dropped on the floor. Enough for the pile to interlock and keep resolving contacts, few
/// enough that 300 ticks stay well under a second in a debug build.
const BOXES: usize = 100;
/// Ticks traced. Long enough that a pile left alone would sleep several times over, which is what
/// the kicks exist to prevent.
const TICKS: u64 = 300;
/// The tick the resume gate captures at: past the first landing, inside a live contact graph, with
/// warm-start impulses and island membership that a naive snapshot would drop.
const SNAPSHOT_TICK: u64 = 150;
/// Ticks between kicks. Every kick wakes the whole pile, so the trace never flattens into the one
/// repeated line a sleeping pile would produce.
const KICK_PERIOD: u64 = 25;

/// The committed reference trace (`sort -u` over it is the variety check the gate needs).
fn reference_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/traces/pile.txt")
}

/// A box's drop position: a 5×5×4 lattice with an integer-derived jitter, so the pile is
/// asymmetric (and therefore chaotic) without a random number generator anywhere.
fn drop_position(index: usize) -> [f32; 3] {
    let column = (index % 5) as f32;
    let row = ((index / 5) % 5) as f32;
    let layer = (index / 25) as f32;
    let jitter = ((index * 37) % 13) as f32 * 0.01;
    [
        column * 1.1 - 2.2 + jitter,
        layer * 1.3 + 0.6,
        row * 1.1 - 2.2 - jitter,
    ]
}

/// A static floor plus [`BOXES`] rigid boxes, spawned in a fixed order.
fn build(world: &mut World) {
    let floor = world.spawn();
    world
        .insert(floor, TestPose::at([0.0, -0.5, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            floor,
            Collider {
                shape: Shape::Box {
                    half_extents: [20.0, 0.5, 20.0],
                },
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .expect("a fresh entity accepts a collider");

    for index in 0..BOXES {
        let entity = world.spawn();
        world
            .insert(entity, TestPose::at(drop_position(index)))
            .expect("a fresh entity accepts a pose");
        world
            .insert(
                entity,
                Collider {
                    shape: Shape::Box {
                        half_extents: [0.5; 3],
                    },
                    body: Body::Rigid(RigidBody {
                        mass: 1.0,
                        friction: 0.5,
                        restitution: 0.1,
                        sleep_after_ticks: Some(30),
                        ..RigidBody::default()
                    }),
                    layer: None,
                    sensor: false,
                },
            )
            .expect("a fresh entity accepts a collider");
        world
            .insert(entity, Velocity::default())
            .expect("a fresh entity accepts a velocity");
    }
}

/// Game code: every [`KICK_PERIOD`] ticks, add an integer-derived impulse to each box's velocity.
///
/// This is the whole authored write path — a game touches `Velocity` and nothing else — and it is
/// also the wake path: the added term makes the component differ from the bits the mirror last
/// wrote, which is exactly the condition the write system pushes on.
fn kick(world: &mut World, ctx: &SystemCtx) {
    let frame = ctx.frame.as_u64();
    if frame == 0 || !frame.is_multiple_of(KICK_PERIOD) {
        return;
    }
    let round = (frame / KICK_PERIOD) as u32;
    for (nth, (velocity,)) in world.query::<(&mut Velocity,)>().enumerate() {
        let mix = (nth as u32)
            .wrapping_mul(2_654_435_761)
            .wrapping_add(round.wrapping_mul(40_503))
            % 1000;
        velocity.linear[0] += (mix % 7) as f32 - 3.0;
        velocity.linear[1] += (mix % 5) as f32 * 0.8 + 2.5;
        velocity.linear[2] += (mix % 11) as f32 * 0.5 - 2.5;
        velocity.angular[1] += (mix % 3) as f32 - 1.0;
    }
}

/// The schedule every run in this file uses: game code first, then the physics triple.
fn pile_schedule() -> Schedule {
    let mut schedule = Schedule::new();
    schedule.add_system("kick", kick);
    // The pile falls and tumbles on every axis, and this world holds no `Plane` anyway.
    install::<TestPose>(&mut schedule, PhysicsSpace::Free);
    schedule
}

/// Runs the pile from scratch for [`TICKS`] ticks and returns the trace.
fn run() -> Vec<String> {
    let mut world = World::new();
    build(&mut world);
    let mut schedule = pile_schedule();
    let mut trace = Vec::with_capacity(TICKS as usize);
    for frame in 0..TICKS {
        tick(&mut world, &mut schedule, frame);
        trace.push(trace_line(&world, frame));
    }
    trace
}

/// Runs the pile from scratch for [`TICKS`] ticks and returns each tick's [`ContactEvent`] list.
fn run_events() -> Vec<Vec<ContactEvent>> {
    let mut world = World::new();
    build(&mut world);
    let mut schedule = pile_schedule();
    let mut events = Vec::with_capacity(TICKS as usize);
    for frame in 0..TICKS {
        tick(&mut world, &mut schedule, frame);
        events.push(
            world
                .resource::<PhysicsWorld>()
                .expect("the write system installs the world on the first tick")
                .contact_events()
                .to_vec(),
        );
    }
    events
}

/// Everything a session needs to be rebuilt from nothing: the ECS half by hand, the solver half as
/// snapshot bytes. This is what the engine's snapshot document carries, in miniature.
struct Capture {
    entities: Vec<EntityId>,
    colliders: Vec<Collider>,
    poses: Vec<TestPose>,
    velocities: Vec<Option<Velocity>>,
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
                    .expect("every body carries a collider")
                    .clone()
            })
            .collect(),
        poses: entities
            .iter()
            .map(|&entity| common::pose_of(world, entity))
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
        bytes: world
            .resource::<PhysicsWorld>()
            .expect("the write system installed a world")
            .to_bytes()
            .expect("the solver state encodes"),
        entities,
    }
}

fn restore(capture: &Capture) -> World {
    let mut world = World::new();
    world
        .restore_entities(&capture.entities)
        .expect("captured live set is valid");
    for (index, &entity) in capture.entities.iter().enumerate() {
        world
            .insert(entity, capture.colliders[index].clone())
            .expect("a restored entity accepts a collider");
        world
            .insert(entity, capture.poses[index])
            .expect("a restored entity accepts a pose");
        if let Some(velocity) = capture.velocities[index] {
            world
                .insert(entity, velocity)
                .expect("a restored entity accepts a velocity");
        }
    }
    world.insert_resource(
        PhysicsWorld::from_bytes(&capture.bytes, TickRate::DEFAULT)
            .expect("the bytes this run just wrote decode"),
    );
    world
}

#[test]
fn the_pile_traces_identically_twice() {
    assert_eq!(run(), run());
}

#[test]
fn the_pile_matches_the_committed_trace() {
    let trace = run();
    let path = reference_path();
    if ludion_core::bless::bless_requested() {
        let header = "\
# Per-tick digest of every body's mirrored pose, in handle-table order (tests/pile.rs).\n\
# Recorded with: LUDION_BLESS=1 cargo test -p ludion-physics --test pile\n\
# An empty diff only passes if the trace is varied: the digest canonicalizes nothing, but a\n\
# sleeping pile repeats one line, so the distinct-line count is the second half of the gate.\n";
        std::fs::write(&path, format!("{header}{}\n", trace.join("\n")))
            .expect("the trace directory is committed");
        return;
    }
    let committed: Vec<String> = std::fs::read_to_string(&path)
        .expect("the committed reference trace")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(str::to_owned)
        .collect();
    assert_eq!(committed.len(), TICKS as usize);
    for (frame, (actual, expected)) in trace.iter().zip(&committed).enumerate() {
        assert_eq!(actual, expected, "the trace diverges at tick {frame}");
    }
}

#[test]
fn the_trace_is_varied() {
    let trace = run();
    let mut distinct: Vec<&String> = trace.iter().collect();
    distinct.sort_unstable();
    distinct.dedup();
    // A pile that fell asleep would repeat one line forever, and an empty diff against the
    // reference would then prove nothing; the distinct-line count is what rules that out.
    assert!(
        distinct.len() > 250,
        "only {} distinct lines of {TICKS}",
        distinct.len()
    );
}

#[test]
fn a_resumed_run_continues_byte_identically() {
    let reference = run();

    let mut world = World::new();
    build(&mut world);
    let mut schedule = pile_schedule();
    for frame in 0..SNAPSHOT_TICK {
        tick(&mut world, &mut schedule, frame);
    }
    let capture = capture(&world);
    drop(world);
    drop(schedule);

    let mut world = restore(&capture);
    let mut schedule = pile_schedule();
    for frame in SNAPSHOT_TICK..TICKS {
        tick(&mut world, &mut schedule, frame);
        assert_eq!(
            trace_line(&world, frame),
            reference[frame as usize],
            "the resumed run diverges at tick {frame}"
        );
    }
}

#[test]
fn snapshot_bytes_round_trip_to_the_same_bytes() {
    let mut world = World::new();
    build(&mut world);
    let mut schedule = pile_schedule();
    for frame in 0..SNAPSHOT_TICK {
        tick(&mut world, &mut schedule, frame);
    }
    let bytes = world
        .resource::<PhysicsWorld>()
        .expect("a world")
        .to_bytes()
        .expect("the solver state encodes");
    let restored = PhysicsWorld::from_bytes(&bytes, TickRate::DEFAULT).expect("its own bytes");
    assert_eq!(
        restored.to_bytes().expect("the restored state encodes"),
        bytes,
        "a restore that loses stored layout shows up here before it shows up in a trace"
    );
    assert_eq!(restored.body_count(), BOXES + 1);
}

#[test]
fn the_pile_events_are_canonical_and_deterministic() {
    let first = run_events();
    let second = run_events();
    assert_eq!(first.len(), TICKS as usize);
    for (frame, (a, b)) in first.iter().zip(&second).enumerate() {
        assert_eq!(a.len(), b.len(), "tick {frame} differs in event count");
        for (event_a, event_b) in a.iter().zip(b) {
            assert_eq!(
                (event_a.a, event_a.b),
                (event_b.a, event_b.b),
                "tick {frame}"
            );
            assert_eq!(event_a.phase, event_b.phase, "tick {frame}");
            assert_eq!(
                event_a.impulse.to_bits(),
                event_b.impulse.to_bits(),
                "tick {frame}"
            );
            assert_eq!(
                event_a.point.map(f32::to_bits),
                event_b.point.map(f32::to_bits),
                "tick {frame}"
            );
            assert_eq!(
                event_a.normal.map(f32::to_bits),
                event_b.normal.map(f32::to_bits),
                "tick {frame}"
            );
        }
    }

    let mut saw_started = false;
    let mut saw_stopped = false;
    for (frame, tick_events) in first.iter().enumerate() {
        let mut previous: Option<(EntityId, EntityId)> = None;
        for event in tick_events {
            assert!(
                event.a < event.b,
                "tick {frame}: {event:?} is not canonical"
            );
            let key = (event.a, event.b);
            if let Some(prev) = previous {
                assert!(
                    prev < key,
                    "tick {frame}: {tick_events:?} is not strictly ascending"
                );
            }
            previous = Some(key);
            match event.phase {
                ContactPhase::Started => {
                    saw_started = true;
                    let [x, y, z] = event.normal;
                    let length = ludion_core::math::sqrt(x * x + y * y + z * z);
                    assert!((length - 1.0).abs() < 1e-4, "tick {frame}: {event:?}");
                    assert!(
                        event.point.iter().all(|c| c.is_finite()),
                        "tick {frame}: {event:?}"
                    );
                }
                ContactPhase::Stopped => {
                    saw_stopped = true;
                    assert_eq!(event.point, [0.0; 3], "tick {frame}: {event:?}");
                    assert_eq!(event.normal, [0.0; 3], "tick {frame}: {event:?}");
                }
            }
        }
    }
    assert!(
        saw_started,
        "the pile must produce at least one Started event"
    );
    assert!(
        saw_stopped,
        "the pile must produce at least one Stopped event"
    );
}

#[test]
fn a_resumed_run_continues_contact_events_too() {
    let reference = run_events();

    let mut world = World::new();
    build(&mut world);
    let mut schedule = pile_schedule();
    for frame in 0..SNAPSHOT_TICK {
        tick(&mut world, &mut schedule, frame);
    }
    let capture = capture(&world);
    let events_before_restore = world
        .resource::<PhysicsWorld>()
        .expect("a world")
        .contact_events()
        .to_vec();
    assert_eq!(
        events_before_restore,
        reference[(SNAPSHOT_TICK - 1) as usize],
        "the captured world's own events must match the uninterrupted run's last tick"
    );
    drop(world);
    drop(schedule);

    let restored = restore(&capture);
    assert_eq!(
        restored
            .resource::<PhysicsWorld>()
            .expect("the restored world")
            .contact_events(),
        events_before_restore.as_slice(),
        "contact_events must survive to_bytes/from_bytes"
    );

    let mut world = restored;
    let mut schedule = pile_schedule();
    for frame in SNAPSHOT_TICK..TICKS {
        tick(&mut world, &mut schedule, frame);
        assert_eq!(
            world
                .resource::<PhysicsWorld>()
                .expect("a world")
                .contact_events(),
            reference[frame as usize].as_slice(),
            "the resumed run's events diverge at tick {frame}"
        );
    }
}
