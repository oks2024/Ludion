//! A warm `step` allocates nothing. A separate test binary so the counting global allocator stays
//! local to it.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use common::*;
use ludion_core::par;
use ludion_projectiles::{ProjectileDesc, ProjectilePool, Response, StepEvents, step};
use ludion_spatial::spatial3d::Handle;

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
fn a_warm_step_does_not_allocate() {
    let boxes: Vec<_> = (0..16)
        .map(|i| {
            (
                [4.0 + 8.0 * (i % 4) as f32, 2.0, 4.0 + 8.0 * (i / 4) as f32],
                [1.0; 3],
            )
        })
        .collect();
    let (world, _) = box_world(&boxes);
    let physics = physics_ref(&world);
    let items: Vec<_> = (0..16u32)
        .map(|i| {
            sphere(
                Handle::slot(900 + i, 0),
                [8.0 * (i % 4) as f32, 2.0, 8.0 * (i / 4) as f32],
                1.0,
                1,
            )
        })
        .collect();
    let index = index_of(&items);
    let mut cfg = config(physics);
    cfg.gravity = [0.0, -9.8, 0.0];
    cfg.targets = 1;

    let mut pool = ProjectilePool::with_capacity(1_024);
    let mut events = StepEvents::with_capacity(4_096, 1_024);
    let mut seed = 1u32;
    let mut spawn = |pool: &mut ProjectilePool| {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let f = |s: u32| (s % 1000) as f32 / 1000.0;
        // Spawn in the gap between box columns: a ball cast that starts inside a collider
        // allocates in parry, which is a physics cost, not the step's.
        let mut d = ProjectileDesc::new(
            [
                6.0 + f(seed) * 4.0,
                1.0 + f(seed >> 8) * 3.0,
                6.0 + f(seed >> 4) * 4.0,
            ],
            [f(seed >> 12) * 80.0 - 40.0, 0.0, f(seed >> 6) * 80.0 - 40.0],
        );
        d.radius = 0.05;
        d.lifetime = 0.5;
        d.response = match seed % 3 {
            0 => Response::Reflect {
                restitution: 0.8,
                max_bounces: 4,
            },
            1 => Response::Pierce { max: 3 },
            _ => Response::Despawn,
        };
        let _ = pool.spawn(d);
    };
    par::with_threads(1, || {
        // Long enough that rapier's one-off lazy buffers (a ball cast that grazes a box) have grown.
        for _ in 0..30 {
            for _ in 0..200 {
                spawn(&mut pool);
            }
            step(&mut pool, DT, physics, &index, &cfg, &mut events);
        }
        ALLOCS.store(0, Ordering::Relaxed);
        for _ in 0..100 {
            for _ in 0..20 {
                spawn(&mut pool);
            }
            step(&mut pool, DT, physics, &index, &cfg, &mut events);
        }
        assert_eq!(ALLOCS.load(Ordering::Relaxed), 0);
    });
    assert!(!pool.is_empty());
}
