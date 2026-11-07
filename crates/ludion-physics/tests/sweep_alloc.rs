//! `first_hit` / `first_hits` allocate nothing at steady state. A separate test binary so the
//! counting global allocator stays local to it.

#[allow(dead_code)] // shared helpers; each test binary uses a subset
mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use common::{TestPose, tick};
use ludion_core::par;
use ludion_ecs::{Schedule, World};
use ludion_physics::{NoParent, PhysicsSpace, PhysicsWorld, QueryFilter, Sweep, install};
use ludion_spatial::{Body, Collider, Shape};

struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards every call to `System` unchanged; it only counts.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

#[test]
fn sweeps_do_not_allocate() {
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

    ALLOCS.store(0, Ordering::Relaxed);
    par::with_threads(1, || {
        for probe in &probes {
            let _ = physics.first_hit(probe, &filter);
        }
        physics.first_hits(&probes, &filter, &mut out);
    });
    assert_eq!(ALLOCS.load(Ordering::Relaxed), 0);
}
