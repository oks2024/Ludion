//! `first_hit` / `first_hits` allocate nothing at steady state. A separate test binary so the
//! counting global allocator stays local to it.

#[allow(dead_code)] // shared helpers; each test binary uses a subset
mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::{TestPose, tick};
use ludion_core::par;
use ludion_ecs::{Schedule, World};
use ludion_physics::{NoParent, PhysicsSpace, PhysicsWorld, QueryFilter, Sweep, install};
use ludion_spatial::{Body, Collider, Shape};

struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
/// The counter is global, so the tests in this binary take turns.
static SERIAL: Mutex<()> = Mutex::new(());

thread_local! {
    /// Set on the test thread around a measured window: allocations from the harness or from
    /// other threads never count.
    static COUNTING: Cell<bool> = const { Cell::new(false) };
}

fn counting() -> bool {
    // `try_with`: the allocator runs during TLS teardown, when `with` would panic.
    COUNTING.try_with(Cell::get).unwrap_or(false)
}

/// Runs `f` with this thread's allocations counted, returning how many there were.
fn count_allocs(f: impl FnOnce()) -> usize {
    ALLOCS.store(0, Ordering::Relaxed);
    COUNTING.with(|c| c.set(true));
    f();
    COUNTING.with(|c| c.set(false));
    ALLOCS.load(Ordering::Relaxed)
}

// SAFETY: forwards every call to `System` unchanged; it only counts.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if counting() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if counting() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

#[test]
fn sweeps_do_not_allocate() {
    let _turn = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut world = World::new();
    for i in 0..30 {
        let entity = world.spawn();
        world
            .insert(
                entity,
                TestPose::at([(i % 6) as f32 * 3.0, 0.0, (i / 6) as f32 * 3.0]),
            )
            .unwrap();
        world
            .insert(
                entity,
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
    }
    let mut schedule = Schedule::new();
    install::<TestPose, NoParent>(&mut schedule, PhysicsSpace::Free);
    tick(&mut world, &mut schedule, 0);
    let physics = world.resource::<PhysicsWorld>().expect("bodies claimed");
    let filter = physics.sweep_filter(&QueryFilter::new()).unwrap();
    let probes: Vec<Sweep> = (0..1000)
        .map(|i| {
            let r = if i % 2 == 0 { 0.0 } else { 0.2 };
            Sweep::new(
                [-2.0, 0.0, (i % 15) as f32],
                [20.0, 0.1 * (i % 3) as f32, 0.0],
                r,
            )
        })
        .collect();
    let mut out = vec![None; probes.len()];

    // Warm up, then count on one thread: multi-threaded `par` spawns scoped threads, which
    // allocate, and that is the scheduler's cost rather than the query's.
    let _ = physics.first_hit(&probes[0], &filter);
    par::with_threads(1, || physics.first_hits(&probes, &filter, &mut out));
    assert!(out.iter().any(Option::is_some));

    let allocs = count_allocs(|| {
        par::with_threads(1, || {
            for probe in &probes {
                let _ = physics.first_hit(probe, &filter);
            }
            physics.first_hits(&probes, &filter, &mut out);
        });
    });
    assert_eq!(allocs, 0);
}

#[test]
fn sweeps_starting_in_contact_do_not_allocate() {
    let _turn = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let shapes = [
        Shape::Compound {
            parts: vec![ludion_spatial::CompoundPart {
                shape: Shape::Box {
                    half_extents: [0.5; 3],
                },
                translation: [0.0; 3],
                rotation: [0.0; 3],
            }],
        },
        Shape::Box {
            half_extents: [0.5; 3],
        },
        Shape::Sphere { radius: 0.5 },
        Shape::Capsule {
            half_height: 0.5,
            radius: 0.5,
        },
    ];
    let mut world = World::new();
    for (i, shape) in shapes.into_iter().enumerate() {
        let entity = world.spawn();
        world
            .insert(entity, TestPose::at([i as f32 * 10.0, 0.0, 0.0]))
            .unwrap();
        world
            .insert(
                entity,
                Collider {
                    shape,
                    body: Body::new_static(),
                    layer: None,
                    sensor: false,
                },
            )
            .unwrap();
    }
    let mut schedule = Schedule::new();
    install::<TestPose, NoParent>(&mut schedule, PhysicsSpace::Free);
    tick(&mut world, &mut schedule, 0);
    let physics = world.resource::<PhysicsWorld>().expect("bodies claimed");
    let filter = physics.sweep_filter(&QueryFilter::new()).unwrap();
    // Per collider: centre inside, then centre outside but overlapping (gap 0.1 < radius 0.2).
    let probes: Vec<Sweep> = (0..4)
        .flat_map(|i| {
            let x = i as f32 * 10.0;
            [0.1, 0.6].map(|dx| Sweep::new([x + dx, 0.0, 0.0], [-4.0, 0.0, 0.0], 0.2))
        })
        .collect();
    let _ = physics.first_hit(&probes[0], &filter);
    let allocs = count_allocs(|| {
        par::with_threads(1, || {
            for probe in &probes {
                let got = physics.first_hit(probe, &filter).expect("in contact");
                assert_eq!(got.t, 0.0);
            }
        });
    });
    assert_eq!(allocs, 0);
}
