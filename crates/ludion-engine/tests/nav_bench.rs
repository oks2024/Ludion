//! The navigation benchmarks: 100 and 500 agents chasing a target across a 64 x 64 m room with a
//! ramp, one flow-field compute, the grid's full and region rebuilds, and 100 / 500
//! `CharacterMotor`s fed by `NavSteering`.
//!
//! Ignored by default so CI stays fast; run it explicitly, in release:
//!
//! ```text
//! cargo test -p ludion-engine --release --test it -- nav_bench --ignored --nocapture
//! ```
//!
//! There is deliberately no `budgets/*.toml` entry: those files gate `ProfileReport` keys produced
//! by `ludion profile` or a game's `--profile`, and these systems appear in neither. The precedent
//! is `spatial3d_bench.rs`: inline asserted sub-budgets. The wall-clock spans are observation-only
//! and never feed sim state; the 1-thread and N-thread `WorldHash` traces are asserted equal.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    Engine, EngineBuilder, Nav, NavAgent, NavConfig, NavSteering, Transform, TransformHierarchy,
    move_character, nav_steer, nav_update,
};
use ludion_nav::FlowField;
use ludion_spatial::{Body, Collider, Shape, spatial3d};

use crate::nav::{Block, agent, block, mover, scene, scene_with_layers, target_yaml};

/// Median `nav_steer` time per tick, in milliseconds, for 500 agents at one thread. About 3x the
/// release median (0.169 ms at 1 thread, 0.102 ms at 8; 100 agents 0.029 / 0.041 ms) on an unpinned
/// M5 laptop (not authoritative), 2026-11-07, rounded up. Pinned M4 / RTX 3090 figures are owed.
const NAV_STEER_BUDGET_MS: f64 = 0.6;
/// Median flow-field compute, in milliseconds, over 60 targets on the 64 x 64 room. About 3x the
/// release median (0.109 ms, 2,912 nodes reached) on an unpinned M5 laptop (not authoritative),
/// 2026-11-07, rounded up. Pinned figures are owed.
const NAV_FIELD_BUDGET_MS: f64 = 0.4;
/// Median full grid build, in milliseconds, for 64 x 64 columns. About 3x the release median on an
/// unpinned M5 laptop (not authoritative), 2026-11-07, rounded up: full build 0.800 ms, region
/// rebuild 0.162 ms. Pinned figures are owed.
const NAV_REBUILD_BUDGET_MS: f64 = 2.5;
/// Median `motor_mover` time per tick, in milliseconds, for 500 motors at one thread. About 3x the
/// release median (3.705 ms at 1 thread, 3.680 at 8; 100 motors 0.692 / 0.687 ms) on an unpinned M5
/// laptop (not authoritative), 2026-11-07, rounded up. The motor is serial. Pinned figures are owed.
const MOTOR_BUDGET_MS: f64 = 12.0;

const WARMUP: usize = 10;
const MEASURED: usize = 120;

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

/// A 64 x 64 m low floor, a 2 m deck from x = 44 and a 30-degree ramp 8 m wide joining them, with
/// pillars at x ~ 37.
fn bench_room() -> Vec<Block> {
    let mut blocks = vec![
        block([32.0, -0.5, 32.0], [32.0, 0.5, 32.0]),
        block([54.0, 1.5, 32.0], [10.0, 0.5, 32.0]),
        Block {
            at: [42.0, 0.913, 32.0],
            half: [2.0, 0.1, 4.0],
            rot: [0.0, 0.0, 30.0],
        },
    ];
    for z in [6.0, 14.0, 22.0, 42.0, 50.0, 58.0] {
        blocks.push(block([37.0, 1.5, z], [0.5, 1.5, 0.5]));
    }
    blocks
}

fn config() -> NavConfig {
    NavConfig {
        nx: 64,
        nz: 64,
        y_min: -2.0,
        y_max: 8.0,
        ..NavConfig::default()
    }
}

fn target_at(frame: u64) -> [f32; 3] {
    [54.5, 2.0, 20.5 + ((frame / 30) % 24) as f32]
}

/// Writes every agent's target: it crosses one cell every 30 ticks, so fields recompute as in play.
fn chase_target(world: &mut World, ctx: &SystemCtx) {
    let target = target_at(ctx.frame.0);
    let ids: Vec<EntityId> = world
        .query_ids::<(&NavAgent,)>()
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        world.get_mut::<NavAgent>(id).unwrap().unwrap().target = target;
    }
}

/// Agents in `EntityId` order, each moved by its steering through the character motor.
fn motor_mover(world: &mut World, ctx: &SystemCtx) {
    let mut moves: Vec<_> = world
        .query_ids::<(&NavSteering,)>()
        .map(|(id, (s,))| (id, s.velocity))
        .collect();
    moves.sort_by_key(|(id, _)| *id);
    for (id, v) in moves {
        // `NoPhysicsWorld` on tick 0 is expected.
        let _ = move_character(world, id, [v[0] * ctx.dt, -0.1, v[2] * ctx.dt]);
    }
}

type Laps = Arc<Mutex<Vec<Duration>>>;

fn timed(
    laps: &Laps,
    system: impl Fn(&mut World, &SystemCtx) + Send + Sync + 'static,
) -> impl Fn(&mut World, &SystemCtx) + Send + Sync + 'static {
    let laps = Arc::clone(laps);
    move |world, ctx| {
        let t = now();
        system(world, ctx);
        laps.lock().unwrap().push(lap(t));
    }
}

fn crowd_scene(n: usize, motors: bool) -> String {
    let layers = if motors {
        "  layers:\n    - name: agents\n      ignores:\n        - agents\n"
    } else {
        ""
    };
    let mut out = scene_with_layers(&bench_room(), layers);
    let fields = format!(
        "        speed: 6.0\n        neighbor_layers: 2\n{}",
        target_yaml(target_at(0))
    );
    for i in 0..n {
        let at = [
            2.0 + 1.2 * (i % 25) as f32,
            0.0,
            8.0 + 1.2 * (i / 25) as f32,
        ];
        out += &agent(i, at, &fields);
        if motors {
            out += "      CharacterMotor:\n        layer: agents\n";
        }
    }
    out
}

struct Arm {
    hashes: Vec<u64>,
    system: Vec<Duration>,
    ticks: Vec<Duration>,
    computes: u64,
    near_target: usize,
}

fn computes(engine: &mut Engine) -> u64 {
    engine
        .world_mut()
        .resource_mut::<Nav>()
        .unwrap()
        .cache_mut()
        .computes()
}

fn run_arm(n: usize, threads: usize, motors: bool) -> Arm {
    par::with_threads(threads, || {
        let laps = Laps::default();
        let mut config = config();
        if motors {
            config.ignore_layers = vec!["agents".to_string()];
        }
        let builder: EngineBuilder = Engine::builder()
            .system(
                "spatial3d_rebuild",
                spatial3d::rebuild::<Transform, TransformHierarchy>,
            )
            .nav(config)
            .system("nav_update", nav_update)
            .system("chase_target", chase_target);
        let builder = if motors {
            builder
                .system("nav_steer", nav_steer)
                .system("motor_mover", timed(&laps, motor_mover))
        } else {
            builder
                .system("nav_steer", timed(&laps, nav_steer))
                .system("mover", mover)
        };
        let mut engine = builder.build().expect("boots");
        engine.load_inline_scene(&crowd_scene(n, motors)).unwrap();

        let mut hashes = Vec::new();
        let mut ticks = Vec::new();
        let mut computes_before = 0;
        for tick in 0..WARMUP + MEASURED {
            if tick == WARMUP {
                computes_before = computes(&mut engine);
            }
            let t = now();
            engine.advance_live();
            let took = lap(t);
            hashes.push(engine.world_hash().unwrap().as_u64());
            if tick >= WARMUP {
                ticks.push(took);
            }
        }
        let computes = computes(&mut engine) - computes_before;
        let target = target_at((WARMUP + MEASURED - 1) as u64);
        let near_target = engine
            .world_mut()
            .query_ids::<(&Transform, &NavAgent)>()
            .filter(|(_, (t, _))| {
                let d = [t.translation[0] - target[0], t.translation[2] - target[2]];
                (d[0] * d[0] + d[1] * d[1]).sqrt() < 2.0
            })
            .count();
        let system = laps.lock().unwrap()[WARMUP..].to_vec();
        Arm {
            hashes,
            system,
            ticks,
            computes,
            near_target,
        }
    })
}

/// Runs both thread counts for 100 and 500 agents; returns the 1-thread 500-agent system median.
fn crowd(motors: bool, name: &str) -> f64 {
    let auto = par::auto_threads();
    let mut gate = 0.0;
    for n in [100, 500] {
        let one = run_arm(n, 1, motors);
        let many = run_arm(n, auto, motors);
        assert_eq!(
            one.hashes, many.hashes,
            "{name}: WorldHash traces differ between 1 and {auto} threads at {n} agents"
        );
        for (label, arm) in [
            ("1 thread".to_string(), &one),
            (format!("{auto} threads"), &many),
        ] {
            println!(
                "{name} {n:>3} agents, {label:>10}: system median {:.3} ms (max {:.3}), whole tick \
                 median {:.3} ms, {} field computes, {} within 2 m of the target",
                median_ms(&arm.system),
                max_ms(&arm.system),
                median_ms(&arm.ticks),
                arm.computes,
                arm.near_target,
            );
        }
        if n == 500 {
            gate = median_ms(&one.system);
        }
    }
    gate
}

#[test]
#[ignore = "benchmark: run explicitly in release with --ignored --nocapture"]
fn crowd_steering() {
    let median = crowd(false, "nav_steer");
    assert!(
        median < NAV_STEER_BUDGET_MS,
        "nav_steer {median:.3} ms exceeds the {NAV_STEER_BUDGET_MS} ms sub-budget"
    );
}

#[test]
#[ignore = "benchmark: run explicitly in release with --ignored --nocapture"]
fn flow_field_compute() {
    let mut engine = Engine::builder()
        .nav(config())
        .system("nav_update", nav_update)
        .build()
        .expect("boots");
    engine.load_inline_scene(&scene(&bench_room())).unwrap();
    for _ in 0..2 {
        engine.advance_live();
    }
    let grid = engine
        .world()
        .resource::<Nav>()
        .unwrap()
        .grid()
        .expect("the grid is built")
        .clone();
    let count = grid.node_count();
    let targets: Vec<_> = (0..60)
        .map(|i| ludion_nav::NodeId((i * count / 60) as u32))
        .collect();
    let mut field = FlowField::default();
    for t in targets.iter().cycle().take(WARMUP) {
        field.compute(&grid, *t, 48.0);
    }
    let mut laps = Vec::new();
    let mut reached = Vec::new();
    for t in &targets {
        let start = now();
        field.compute(&grid, *t, 48.0);
        laps.push(lap(start));
        reached.push(field.reached().len());
    }
    reached.sort_unstable();
    let median = median_ms(&laps);
    println!(
        "flow field over {count} nodes: median {median:.3} ms (max {:.3}), median {} nodes reached",
        max_ms(&laps),
        reached[reached.len() / 2]
    );
    assert!(
        median < NAV_FIELD_BUDGET_MS,
        "flow field {median:.3} ms exceeds the {NAV_FIELD_BUDGET_MS} ms sub-budget"
    );
}

fn add_wall(engine: &mut Engine, at: [f32; 3]) -> EntityId {
    let world: &mut World = engine.world_mut();
    let id = world.spawn();
    world
        .insert(
            id,
            Transform {
                translation: at,
                ..Transform::default()
            },
        )
        .unwrap();
    let collider = Collider {
        shape: Shape::Box {
            half_extents: [0.5, 1.5, 4.0],
        },
        body: Body::new_static(),
        layer: None,
        sensor: false,
    };
    world.insert(id, collider).unwrap();
    id
}

fn rebuilds(engine: &Engine) -> u64 {
    engine.world().resource::<Nav>().unwrap().rebuilds()
}

#[test]
#[ignore = "benchmark: run explicitly in release with --ignored --nocapture"]
fn grid_rebuild() {
    let laps = Laps::default();
    let mut engine = Engine::builder()
        .nav(config())
        .system("nav_update", timed(&laps, nav_update))
        .build()
        .expect("boots");
    engine.load_inline_scene(&scene(&bench_room())).unwrap();
    for _ in 0..2 {
        engine.advance_live();
    }

    let mut full = Vec::new();
    for _ in 0..30 {
        engine
            .world_mut()
            .resource_mut::<Nav>()
            .unwrap()
            .request_full_rebuild();
        let before = rebuilds(&engine);
        engine.advance_live();
        assert!(rebuilds(&engine) > before, "the full rebuild ran");
        full.push(*laps.lock().unwrap().last().unwrap());
    }

    let mut region = Vec::new();
    let mut wall: Option<EntityId> = None;
    for _ in 0..30 {
        match wall.take() {
            Some(id) => engine.world_mut().despawn(id).unwrap(),
            None => wall = Some(add_wall(&mut engine, [20.5, 1.5, 45.0])),
        }
        for _ in 0..2 {
            let before = rebuilds(&engine);
            engine.advance_live();
            if rebuilds(&engine) > before {
                region.push(*laps.lock().unwrap().last().unwrap());
            }
        }
    }
    assert!(
        !region.is_empty(),
        "the wall edits triggered region rebuilds"
    );
    let median = median_ms(&full);
    println!(
        "grid 64 x 64 columns: full build median {median:.3} ms (max {:.3}), region rebuild \
         median {:.3} ms (max {:.3}, {} samples)",
        max_ms(&full),
        median_ms(&region),
        max_ms(&region),
        region.len()
    );
    assert!(
        median < NAV_REBUILD_BUDGET_MS,
        "full grid build {median:.3} ms exceeds the {NAV_REBUILD_BUDGET_MS} ms sub-budget"
    );
}

#[test]
#[ignore = "benchmark: run explicitly in release with --ignored --nocapture"]
fn motor_crowd() {
    let median = crowd(true, "motor_mover");
    assert!(
        median < MOTOR_BUDGET_MS,
        "motor_mover {median:.3} ms exceeds the {MOTOR_BUDGET_MS} ms sub-budget"
    );
}
