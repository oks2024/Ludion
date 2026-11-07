//! `spatial3d` and `WorldHash`: the index resource is not hashed, and its results do not depend on
//! the thread count.

use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_core::time::{FrameIndex, TickRate};
use ludion_ecs::{Registry, Schedule, SystemCtx, World};
use ludion_sim::WorldHash;
use ludion_spatial::spatial3d::{self, Filter, Index3d, SpatialVolume, VolumeShape};
use ludion_spatial::{NoParent, WorldPosition};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
struct Pos {
    xyz: [f32; 3],
}

impl WorldPosition for Pos {
    fn world_position(&self) -> [f32; 3] {
        self.xyz
    }
}

/// `n` carriers on a deterministic scatter, alternating spheres and boxes.
fn world_of(n: u32) -> World {
    let mut world = World::new();
    for i in 0..n {
        let e = world.spawn();
        let f = i as f32;
        world
            .insert(
                e,
                Pos {
                    xyz: [(f * 7.31) % 61.0, (f * 3.77) % 23.0, (f * 5.13) % 59.0],
                },
            )
            .unwrap();
        let shape = if i % 2 == 0 {
            VolumeShape::Sphere { radius: 0.6 }
        } else {
            VolumeShape::Aabb {
                half_extents: [0.4, 0.8, 0.5],
            }
        };
        world
            .insert(
                e,
                SpatialVolume {
                    shape,
                    layers: 1 << (i % 3),
                },
            )
            .unwrap();
    }
    world
}

fn drift(world: &mut World, ctx: &SystemCtx) {
    let frame = ctx.frame.as_u64() as f32;
    let ids: Vec<EntityId> = world.entities().collect();
    for e in ids {
        let step = 0.05 * ((e.index() % 5) as f32 - 2.0) * (1.0 + frame * 0.1);
        world.get_mut::<Pos>(e).unwrap().unwrap().xyz[0] += step;
    }
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    registry.register::<Pos>("Pos").unwrap();
    registry.register::<SpatialVolume>("SpatialVolume").unwrap();
    registry
}

fn hash_trace(rebuild: bool) -> Vec<String> {
    let mut world = world_of(40);
    let registry = registry();
    let mut schedule = Schedule::new();
    if rebuild {
        schedule.add_system("spatial3d_rebuild", spatial3d::rebuild::<Pos, NoParent>);
    }
    schedule.add_system("drift", drift);
    (0..20u64)
        .map(|frame| {
            let ctx = SystemCtx::new(FrameIndex::new(frame), TickRate::DEFAULT);
            schedule.run(&mut world, &ctx);
            WorldHash::compute(&world, &registry).unwrap().to_string()
        })
        .collect()
}

#[test]
fn the_index_resource_is_outside_the_world_hash() {
    let without = hash_trace(false);
    let with = hash_trace(true);
    assert_eq!(
        with, without,
        "scheduling the rebuild must not move the hash"
    );
    assert_eq!(
        hash_trace(true),
        with,
        "two runs with the rebuild are identical"
    );
    assert!(
        with.windows(2).any(|w| w[0] != w[1]),
        "the world must change across frames for the trace to be meaningful"
    );
}

/// Overlap, k-nearest and cast results over 20 ticks, enough items that the build's key pass runs
/// in several parallel chunks.
fn query_trace(threads: usize) -> Vec<String> {
    par::with_threads(threads, || {
        let mut world = world_of(3000);
        let mut schedule = Schedule::new();
        schedule.add_system("spatial3d_rebuild", spatial3d::rebuild::<Pos, NoParent>);
        schedule.add_system("drift", drift);
        let filter = Filter::new(0b011);
        let (mut handles, mut near) = (Vec::new(), Vec::new());
        let mut trace = Vec::new();
        for frame in 0..20u64 {
            let ctx = SystemCtx::new(FrameIndex::new(frame), TickRate::DEFAULT);
            schedule.run(&mut world, &ctx);
            let index = world.resource::<Index3d>().expect("rebuilt");
            index.overlap_sphere([30.0, 10.0, 30.0], 6.0, &filter, &mut handles);
            index.k_nearest([20.0, 10.0, 20.0], 8, 50.0, &filter, &mut near);
            let hit = index.cast([0.0, 10.0, 0.0], [60.0, 0.0, 60.0], 0.25, &filter);
            trace.push(format!("{handles:?}|{near:?}|{hit:?}"));
        }
        trace
    })
}

#[test]
fn queries_match_across_thread_counts() {
    let one = query_trace(1);
    assert!(
        one.iter().any(|t| t.starts_with("[Handle")),
        "the probe finds items"
    );
    assert_eq!(query_trace(8), one);
}
