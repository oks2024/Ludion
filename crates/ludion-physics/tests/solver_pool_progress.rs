//! Forward progress of the solver pool: a step never blocks a worker on a task only that worker can
//! run.
//!
//! The broad phase hands its deferred BVH optimize to the pool with `rayon::spawn`, which from a
//! worker lands on that worker's own deque with no guarantee a sibling wakes for it. The step's
//! join then waited on a plain channel, so a pool whose other workers were busy or asleep left the
//! task stranded behind its own waiter: `step()` never returned, at 0% CPU.

mod common;

use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::{TestPose, tick};
use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, World};
use ludion_physics::{NoParent, PhysicsSpace, PhysicsWorld, install};
use ludion_spatial::{Body, Collider, RigidBody, Shape, Velocity};
use rapier3d::prelude::*;

/// A static ground and a few dynamic boxes that never sleep, at staggered heights so most move every
/// step. Few enough that the narrow phase stays under its broadcast threshold (64 update
/// candidates), which needs the pinned worker and would hang for an unrelated reason.
fn few_boxes() -> rapier3d::prelude::PhysicsWorld {
    let mut world = rapier3d::prelude::PhysicsWorld::new();
    world.insert(
        RigidBodyBuilder::fixed().translation(Vector::new(0.0, -1.0, 0.0)),
        ColliderBuilder::cuboid(50.0, 1.0, 50.0),
    );
    for index in 0..8 {
        world.insert(
            RigidBodyBuilder::dynamic()
                .translation(Vector::new(
                    index as f32 * 1.5 - 6.0,
                    2.0 + index as f32 * 1.7,
                    0.0,
                ))
                .can_sleep(false),
            ColliderBuilder::cuboid(0.5, 0.5, 0.5),
        );
    }
    world
}

/// Steps `world` [`STEPS`] times on a spawned thread; the harness times it out instead of hanging.
const STEPS: usize = 300;

#[allow(clippy::disallowed_methods)] // watchdog thread: a hang must fail the test, not hang it
fn step_under_watchdog(mut world: rapier3d::prelude::PhysicsWorld, hang_message: &str) {
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        for _ in 0..STEPS {
            world.step();
        }
        let _ = done_tx.send(());
    });
    if done_rx.recv_timeout(Duration::from_secs(30)).is_err() {
        panic!("{hang_message}");
    }
}

#[test]
#[allow(clippy::disallowed_methods)] // watchdog thread: a hang must fail the test, not hang it
fn a_step_completes_when_its_spawned_optimize_lands_on_the_only_free_worker() {
    let mut world = few_boxes();
    world
        .configure_thread_pool(2)
        .expect("a two-worker pool builds");
    let pool = world.thread_pool().expect("the pool was configured");

    // One worker is held by a blocker, so the step runs on the other: the optimize it spawns has
    // nobody to steal it, and only its own waiter can run it.
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    pool.spawn(move || {
        let _ = started_tx.send(());
        let _ = release_rx.recv_timeout(Duration::from_secs(60));
    });
    started_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the blocker starts on a pool worker");

    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        for _ in 0..STEPS {
            world.step();
        }
        let _ = done_tx.send(());
    });
    let outcome = done_rx.recv_timeout(Duration::from_secs(30));
    let _ = release_tx.send(());
    assert!(
        outcome.is_ok(),
        "step() never returned with one pool worker busy: join_deferred_bvh_optimize is blocking \
         the worker that holds the spawned optimize on its own deque"
    );
}

#[test]
fn a_one_worker_pool_steps_without_spawning() {
    let mut world = few_boxes();
    world
        .configure_thread_pool(1)
        .expect("a one-worker pool builds");
    step_under_watchdog(
        world,
        "step() never returned on a one-worker pool, where the optimize runs inline",
    );
}

const ROOMS: usize = 4;
const FALLERS: usize = 40;

fn box_collider() -> Collider {
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
    }
}

fn spawn_box(world: &mut World, position: [f32; 3]) -> EntityId {
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at(position))
        .expect("a fresh entity accepts a pose");
    world
        .insert(entity, box_collider())
        .expect("a fresh entity accepts a collider");
    world
        .insert(entity, Velocity::default())
        .expect("a fresh entity accepts a velocity");
    entity
}

/// A deterministic lattice position for the `index`th box of a room.
fn lattice(room: usize, index: usize) -> [f32; 3] {
    let jitter = ((index * 37 + room * 11) % 13) as f32 * 0.01;
    [
        (index % 5) as f32 * 1.1 - 2.2 + jitter,
        (index / 25) as f32 * 1.3 + 0.6 + ((index / 5) % 5) as f32 * 1.2,
        ((index / 5) % 5) as f32 * 1.1 - 2.2 - jitter,
    ]
}

fn build_room(world: &mut World, room: usize) {
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
    for index in 0..FALLERS {
        spawn_box(world, lattice(room, index));
    }
}

fn schedule() -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose, NoParent>(&mut schedule, PhysicsSpace::Free);
    schedule
}

/// The ECS half by hand, the solver half as snapshot bytes — as `pile.rs` does.
fn capture_restore(world: &World) -> World {
    let entities: Vec<EntityId> = world.entities().collect();
    let mut restored = World::new();
    restored
        .restore_entities(&entities)
        .expect("captured live set is valid");
    for &entity in &entities {
        let collider = world
            .get::<Collider>(entity)
            .expect("a live entity")
            .expect("every body carries a collider")
            .clone();
        restored
            .insert(entity, collider)
            .expect("a restored entity accepts a collider");
        restored
            .insert(entity, common::pose_of(world, entity))
            .expect("a restored entity accepts a pose");
        if let Some(velocity) = world.get::<Velocity>(entity).expect("a live entity") {
            restored
                .insert(entity, *velocity)
                .expect("a restored entity accepts a velocity");
        }
    }
    let bytes = world
        .resource::<PhysicsWorld>()
        .expect("the write system installed a world")
        .to_bytes()
        .expect("the solver state encodes");
    restored.insert_resource(
        PhysicsWorld::from_bytes(&bytes, TickRate::DEFAULT).expect("its own bytes decode"),
    );
    restored
}

fn run_ticks(world: &mut World, schedule: &mut Schedule, frame: &mut u64, count: u64) {
    for _ in 0..count {
        tick(world, schedule, *frame);
        *frame += 1;
    }
}

/// Every body's translation as bits, in entity order.
fn trace(world: &World) -> Vec<[u32; 3]> {
    world
        .entities()
        .map(|entity| common::pose_of(world, entity).translation.map(f32::to_bits))
        .collect()
}

/// Four rooms of build, tick, snapshot-restore, tick, churn and tick, plus one pool rebuild
/// from a nested thread-count change; returns the final pose trace.
fn soak(threads: usize) -> Vec<[u32; 3]> {
    par::with_threads(threads, || {
        let mut frame = 0;
        let mut last = Vec::new();
        for room in 0..ROOMS {
            let mut world = World::new();
            build_room(&mut world, room);
            let mut schedule = schedule();
            run_ticks(&mut world, &mut schedule, &mut frame, 120);

            let mut world = capture_restore(&world);
            let mut schedule = self::schedule();
            run_ticks(&mut world, &mut schedule, &mut frame, 60);

            let boxes: Vec<EntityId> = world.entities().skip(1).collect();
            let half = boxes.len() / 2;
            for &entity in &boxes[..half] {
                world.despawn(entity).expect("a live box despawns");
            }
            for index in 0..half {
                spawn_box(&mut world, lattice(room + 1, index + 3));
            }
            run_ticks(&mut world, &mut schedule, &mut frame, 60);

            if room == 0 {
                let other = if threads == 2 { 8 } else { 2 };
                par::with_threads(other, || {
                    run_ticks(&mut world, &mut schedule, &mut frame, 30)
                });
            }
            last = trace(&world);
        }
        last
    })
}

#[test]
#[allow(clippy::disallowed_methods)] // watchdog thread and wall-clock bound: a hang must fail the test, not hang it
fn long_runs_with_reloads_restores_and_thread_changes_never_hang() {
    let started = Instant::now();
    let traces: Vec<Vec<[u32; 3]>> = [1usize, 2, 8]
        .into_iter()
        .map(|threads| {
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(soak(threads));
            });
            rx.recv_timeout(Duration::from_secs(120)).unwrap_or_else(|_| {
                panic!(
                    "the {threads}-thread soak made no progress for 120 s: a step is waiting on \
                     a task that cannot run (join_deferred_bvh_optimize)"
                )
            })
        })
        .collect();
    eprintln!("soak took {:?}", started.elapsed());
    assert_eq!(
        traces[0], traces[1],
        "the 1-thread and 2-thread soaks diverge"
    );
    assert_eq!(
        traces[0], traces[2],
        "the 1-thread and 8-thread soaks diverge"
    );
}
