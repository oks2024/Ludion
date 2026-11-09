//! The room-assembly benchmark: a ~340-entity generated room prefab spawned and torn down from a sim
//! system under nav, with 100 walkers heading for it. It times the sim tick around each event: the
//! spawn tick, the tick after it, the teardown tick and the one after it.
//!
//! The room is invisible to nav: kit colliders are 2D `aabb`/`circle`, so in a 3D physics world a
//! generated room has no solver colliders. The nav systems run, but no nav rebuild is in these ticks.
//!
//! Ignored by default so CI stays fast; run it explicitly, in release:
//!
//! ```text
//! cargo test -p ludion-cli --release --test it -- room_assembly_bench --ignored --nocapture
//! ```
//!
//! There is deliberately no `budgets/*.toml` entry: those files gate `ProfileReport` keys, and these
//! ticks appear in no report. The precedent is `nav_bench.rs`: inline asserted sub-budgets. The
//! wall-clock spans are observation-only and never feed sim state; the 1-thread and 8-thread
//! `WorldHash` traces are asserted equal.

use std::path::Path;
use std::time::{Duration, Instant};

use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    Engine, EngineBuilder, Nav, NavSteering, PrefabInstance, Transform, TransformHierarchy,
    despawn_instance, nav_steer, nav_update, spawn_prefab,
};
use ludion_spatial::spatial3d;

use crate::room_assembly::{KIT, Spawned, generate, nav_config};

/// Median tick time, in milliseconds, of the tick that spawns the room, at one thread. About 3x the
/// release median (0.65 ms, 343 entities) on an unpinned M5, 2026-11-09, not authoritative; pinned
/// figures owed.
const ROOM_SPAWN_TICK_BUDGET_MS: f64 = 2.0;
/// Median tick time, in milliseconds, of the tick after the spawn, at one thread. About 3x the
/// release median (0.115 ms) on an unpinned M5, 2026-11-09, not authoritative; pinned figures owed.
/// The room's 2D colliders do not reach the nav grid, so no region rebuild is in this figure.
const ROOM_REBUILD_TICK_BUDGET_MS: f64 = 0.4;

const CYCLES: u64 = 20;
const PERIOD: u64 = 40;
const WALKERS: usize = 100;
const MAP_W: usize = 20;
const MAP_H: usize = 14;
const PREFAB: &str = "rooms/big.prefab.yaml";
const PLACEMENT: [f32; 3] = [20.0, 0.0, 0.0];

#[allow(clippy::disallowed_methods)]
fn now() -> Instant {
    Instant::now()
}

#[allow(clippy::disallowed_methods)]
fn lap(start: Instant) -> Duration {
    start.elapsed()
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1_000.0
}

fn median_ms(samples: &[Duration]) -> f64 {
    let mut s = samples.to_vec();
    s.sort_unstable();
    let mid = s.len() / 2;
    if s.len() % 2 == 1 {
        ms(s[mid])
    } else {
        (ms(s[mid - 1]) + ms(s[mid])) / 2.0
    }
}

fn max_ms(samples: &[Duration]) -> f64 {
    samples.iter().copied().max().map_or(0.0, ms)
}

/// A 20 x 14 map: a `#` perimeter ring with a door gap in the west wall, `.` inside.
fn big_map() -> String {
    let mut out = String::from(
        "format: 1\nkind: map\nname: big\nkit: kit.kit.yaml\ncell_size: 2.0\norigin:\n  - 0.0\n  - 0.0\nrows:\n",
    );
    for row in 0..MAP_H {
        let line: String = (0..MAP_W)
            .map(|col| {
                let edge = row == 0 || row == MAP_H - 1 || col == 0 || col == MAP_W - 1;
                if edge && !(col == 0 && row == MAP_H / 2) {
                    '#'
                } else {
                    '.'
                }
            })
            .collect();
        out += &format!("  - \"{line}\"\n");
    }
    out
}

fn seq(v: [f32; 3], indent: usize) -> String {
    v.iter()
        .map(|c| format!("{:indent$}- {c:?}\n", ""))
        .collect()
}

/// Room cell (row 7, col 10) at the placement.
const TARGET: [f32; 3] = [41.0, 0.0, 15.0];

fn scene() -> String {
    let mut out = String::from(
        "format: 1\nseed: 1\nphysics:\n  space: free\npreload:\n  - rooms/big.prefab.yaml\nentities:\n  \
         - name: ground\n    components:\n      Transform:\n        translation:\n",
    );
    out += &seq([10.0, -0.5, 14.0], 10);
    out += "      Collider:\n        shape:\n          box:\n            half_extents:\n";
    out += &seq([10.0, 0.5, 14.0], 14);
    out += "        body: static\n";
    for n in 0..WALKERS {
        let at = [1.0 + 1.5 * (n % 6) as f32, 0.0, 1.0 + 1.2 * (n / 6) as f32];
        out +=
            &format!("  - name: a{n}\n    components:\n      Transform:\n        translation:\n");
        out += &seq(at, 10);
        out += "      SpatialVolume:\n        layers: 2\n      NavAgent:\n";
        out += "        speed: 6.0\n        neighbor_layers: 2\n        target:\n";
        out += &seq(TARGET, 10);
    }
    out
}

fn spawner(world: &mut World, ctx: &SystemCtx) {
    let frame = ctx.frame.as_u64();
    if frame < 10 || (frame - 10) / PERIOD >= CYCLES {
        return;
    }
    match (frame - 10) % PERIOD {
        0 => {
            let placement = Transform {
                translation: PLACEMENT,
                ..Transform::default()
            };
            if let Ok(ids) = spawn_prefab(world, PREFAB, Some(placement), Some("r"), false) {
                let mut spawned = world.remove_resource::<Spawned>().unwrap_or_default();
                spawned.0.push(ids);
                world.insert_resource(spawned);
            }
        }
        20 => {
            let root = world
                .resource::<Spawned>()
                .and_then(|s| s.0.last())
                .map(|ids| ids[0]);
            if let Some(root) = root {
                despawn_instance(world, root).expect("the room's root is alive");
            }
        }
        _ => {}
    }
}

fn mover(world: &mut World, ctx: &SystemCtx) {
    let moves: Vec<_> = world
        .query_ids::<(&Transform, &NavSteering)>()
        .map(|(id, (t, s))| (id, t.translation, s.velocity))
        .collect();
    for (id, at, v) in moves {
        let mut next = [at[0] + v[0] * ctx.dt, at[1], at[2] + v[2] * ctx.dt];
        if let Some(grid) = world.resource::<Nav>().and_then(Nav::grid)
            && let Some(node) = grid.node_at([next[0], at[1] + 0.6, next[2]])
        {
            next[1] = grid.node_pos(node)[1];
        }
        world.get_mut::<Transform>(id).unwrap().unwrap().translation = next;
    }
}

fn builder(root: &Path) -> EngineBuilder {
    let mut config = nav_config();
    config.nx = 60;
    config.nz = 28;
    Engine::builder()
        .project_root(root)
        .nav(config)
        .system("spawner", spawner)
        .system(
            "spatial3d_rebuild",
            spatial3d::rebuild::<Transform, TransformHierarchy>,
        )
        .system("nav_update", nav_update)
        .system("nav_steer", nav_steer)
        .system("mover", mover)
        .query::<(&PrefabInstance,)>()
}

struct Run {
    hashes: Vec<u64>,
    ticks: Vec<Duration>,
    entities: usize,
}

fn run(root: &Path, threads: usize) -> Run {
    par::with_threads(threads, || {
        let mut engine = builder(root).build().expect("boots");
        engine.load_inline_scene(&scene()).unwrap();
        let total = (10 + CYCLES * PERIOD) as usize;
        let mut hashes = Vec::new();
        let mut ticks = Vec::new();
        let mut entities = 0;
        for tick in 0..total {
            let t = now();
            engine.advance_live();
            ticks.push(lap(t));
            hashes.push(engine.world_hash().unwrap().as_u64());
            if tick == 11 {
                entities = engine
                    .world()
                    .resource::<Spawned>()
                    .map_or(0, |s| s.0[0].len());
            }
        }
        Run {
            hashes,
            ticks,
            entities,
        }
    })
}

/// The ticks at offset `off` from the spawn in each cycle, optionally skipping cycle 0.
fn at(run: &Run, off: u64, skip_first: bool) -> Vec<Duration> {
    // `advance_live` number `n` (0-based) runs the spawner with frame `n`.
    (u64::from(skip_first)..CYCLES)
        .map(|k| run.ticks[(10 + k * PERIOD + off) as usize])
        .collect()
}

fn idle(run: &Run) -> Vec<Duration> {
    (0..CYCLES)
        .flat_map(|k| (5..15).chain(30..38).map(move |o| (k, o)))
        .map(|(k, o)| run.ticks[(10 + k * PERIOD + o) as usize])
        .collect()
}

#[test]
#[ignore = "benchmark: run explicitly in release with --ignored --nocapture"]
fn room_spawn_and_teardown_under_nav() {
    let root = crate::room_assembly::room_root("room_assembly_bench");
    std::fs::write(root.join("kit.kit.yaml"), KIT).unwrap();
    std::fs::write(root.join("big.map.yaml"), big_map()).unwrap();
    generate(&root, "big.map.yaml", PREFAB);

    let one = run(&root, 1);
    let many = run(&root, 8);
    assert_eq!(
        one.hashes, many.hashes,
        "WorldHash traces differ between 1 and 8 threads"
    );
    assert!(
        (280..=360).contains(&one.entities),
        "room entity count {}",
        one.entities
    );

    println!(
        "room of {} entities, {CYCLES} cycles, {WALKERS} walkers",
        one.entities
    );
    for (label, r) in [("1 thread", &one), ("8 threads", &many)] {
        for (name, off) in [
            ("spawn tick", 0),
            ("spawn + 1 (nav rebuild)", 1),
            ("teardown tick", 20),
            ("teardown + 1", 21),
        ] {
            let samples = at(r, off, true);
            println!(
                "{label:>9} {name:<24} median {:.3} ms (max {:.3}), cycle 0 {:.3} ms",
                median_ms(&samples),
                max_ms(&samples),
                ms(r.ticks[(10 + off) as usize]),
            );
        }
        let idle = idle(r);
        println!(
            "{label:>9} {:<24} median {:.3} ms (max {:.3})",
            "idle ticks",
            median_ms(&idle),
            max_ms(&idle)
        );
    }

    let spawn = median_ms(&at(&one, 0, true));
    let rebuild = median_ms(&at(&one, 1, true));
    assert!(
        spawn < ROOM_SPAWN_TICK_BUDGET_MS,
        "spawn tick {spawn:.3} ms exceeds the {ROOM_SPAWN_TICK_BUDGET_MS} ms sub-budget"
    );
    assert!(
        rebuild < ROOM_REBUILD_TICK_BUDGET_MS,
        "rebuild tick {rebuild:.3} ms exceeds the {ROOM_REBUILD_TICK_BUDGET_MS} ms sub-budget"
    );
}
