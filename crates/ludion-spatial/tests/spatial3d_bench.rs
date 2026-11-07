//! The `spatial3d` microbenchmark: the 0005 query mix at 500 enemies / 5,000 projectiles.
//!
//! Ignored by default so CI stays fast; run it explicitly, in release:
//!
//! ```text
//! cargo test -p ludion-spatial --release --test spatial3d_bench -- --ignored --nocapture
//! ```
//!
//! There is deliberately no `budgets/*.toml` entry: those files gate `ProfileReport` keys produced
//! by `ludion profile` or a game's `--profile`, and a crate microbench produces none (the CLI links
//! no game systems). The precedent is `e6_bench.rs`: an inline asserted sub-budget,
//! [`SPATIAL3D_BUDGET_MS`]. The wall-clock spans are observation-only and never feed sim state.

use std::time::{Duration, Instant};

use ludion_core::par;
use ludion_core::rng::DeterministicRng;
use ludion_spatial::spatial3d::{CastHit, Filter, Handle, Index3d, Item, Neighbor, Volume};

/// Per-frame sub-budget, in milliseconds, for the serial full-index frame: rebuild + 5,000 casts +
/// 500 `k_nearest` + 50 splash overlaps. About 3x the release median sum (3.57 ms: rebuild 0.29 ms
/// at 1 thread / 0.34 ms at 8, casts 2.55, `k_nearest` 0.64, splash 0.04) on an unpinned M5
/// laptop (not authoritative), 2026-11-07, rounded up. Two levers cut the casts, which
/// dominate: `par_chunks(256)` over the projectiles (0.48 ms at 8 threads) and a separate index
/// holding only enemies and walls (1.59 ms serial). Pinned M4 / RTX 3090 figures are owed.
const SPATIAL3D_BUDGET_MS: f64 = 11.0;

const WARMUP: usize = 10;
const MEASURED: usize = 60;
const ENEMIES: usize = 500;
const PROJECTILES: usize = 5_000;
const WALLS: usize = 40;
const ROOM: [f32; 3] = [120.0, 12.0, 120.0];
const DT: f32 = 1.0 / 60.0;

const ENEMY: u32 = 1;
const PROJECTILE: u32 = 2;
const WALL: u32 = 4;

#[allow(clippy::disallowed_methods)]
fn now() -> Instant {
    Instant::now()
}

#[allow(clippy::disallowed_methods)]
fn lap(start: Instant) -> Duration {
    start.elapsed()
}

fn median_ms(samples: &mut [Duration]) -> f64 {
    samples.sort_unstable();
    let mid = samples.len() / 2;
    let ms = |d: Duration| d.as_secs_f64() * 1_000.0;
    if samples.len() % 2 == 1 {
        ms(samples[mid])
    } else {
        (ms(samples[mid - 1]) + ms(samples[mid])) / 2.0
    }
}

fn range(rng: &mut DeterministicRng, lo: f32, hi: f32) -> f32 {
    lo + rng.next_f32() * (hi - lo)
}

struct Mover {
    pos: [f32; 3],
    vel: [f32; 3],
    radius: f32,
}

struct World {
    enemies: Vec<Mover>,
    projectiles: Vec<Mover>,
    walls: Vec<Item>,
}

fn point(rng: &mut DeterministicRng) -> [f32; 3] {
    [
        range(rng, 0.0, ROOM[0]),
        range(rng, 0.0, ROOM[1]),
        range(rng, 0.0, ROOM[2]),
    ]
}

fn dir(rng: &mut DeterministicRng) -> [f32; 3] {
    loop {
        let v = [
            range(rng, -1.0, 1.0),
            range(rng, -1.0, 1.0),
            range(rng, -1.0, 1.0),
        ];
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if len > 0.1 {
            return [v[0] / len, v[1] / len, v[2] / len];
        }
    }
}

fn build_world() -> World {
    let mut rng = DeterministicRng::new(0x3D);
    let enemies = (0..ENEMIES)
        .map(|_| {
            let mut pos = point(&mut rng);
            pos[1] = range(&mut rng, 0.0, 8.0);
            let d = dir(&mut rng);
            Mover {
                pos,
                // About 0.1 m of drift per tick.
                vel: [d[0] * 0.1, d[1] * 0.1, d[2] * 0.1],
                radius: range(&mut rng, 0.4, 1.2),
            }
        })
        .collect();
    let projectiles = (0..PROJECTILES)
        .map(|_| {
            let d = dir(&mut rng);
            let speed = range(&mut rng, 5.0, 120.0) * DT;
            Mover {
                pos: point(&mut rng),
                vel: [d[0] * speed, d[1] * speed, d[2] * speed],
                radius: range(&mut rng, 0.05, 0.2),
            }
        })
        .collect();
    let walls = (0..WALLS)
        .map(|i| {
            let c = point(&mut rng);
            // A few long walls, the rest props.
            let h = if i < 6 {
                [range(&mut rng, 20.0, 50.0), 3.0, 0.5]
            } else {
                [range(&mut rng, 0.5, 3.0), 2.0, range(&mut rng, 0.5, 3.0)]
            };
            Item {
                handle: Handle::slot(i as u32, 0),
                volume: Volume::Aabb {
                    min: [c[0] - h[0], c[1] - h[1], c[2] - h[2]],
                    max: [c[0] + h[0], c[1] + h[1], c[2] + h[2]],
                },
                layers: WALL,
            }
        })
        .collect();
    World {
        enemies,
        projectiles,
        walls,
    }
}

fn advance(world: &mut World) {
    for m in world.enemies.iter_mut().chain(world.projectiles.iter_mut()) {
        for (a, extent) in ROOM.iter().enumerate() {
            m.pos[a] += m.vel[a];
            if m.pos[a] > *extent {
                m.pos[a] -= extent;
            } else if m.pos[a] < 0.0 {
                m.pos[a] += extent;
            }
        }
    }
}

const ENEMY_BASE: u32 = 1_000;
const PROJECTILE_BASE: u32 = 10_000;
const SHOOTER_SLOT: u32 = 99_999;

fn rebuild(index: &mut Index3d, world: &World, projectiles: bool) {
    index.clear();
    for w in &world.walls {
        index.push(*w);
    }
    for (i, m) in world.enemies.iter().enumerate() {
        index.push(Item {
            handle: Handle::slot(ENEMY_BASE + i as u32, 0),
            volume: Volume::Sphere {
                center: m.pos,
                radius: m.radius,
            },
            layers: ENEMY,
        });
    }
    for (i, m) in world.projectiles.iter().enumerate().filter(|_| projectiles) {
        index.push(Item {
            handle: Handle::slot(PROJECTILE_BASE + i as u32, 0),
            volume: Volume::Sphere {
                center: m.pos,
                radius: m.radius,
            },
            layers: PROJECTILE,
        });
    }
    index.build();
}

const CAST_CHUNK: usize = 256;

fn cast_one(index: &Index3d, i: usize, p: &Mover) -> Option<CastHit> {
    let filter = Filter::new(ENEMY | WALL)
        .excluding(Handle::slot(PROJECTILE_BASE + i as u32, 0))
        .owned_by(Handle::slot(SHOOTER_SLOT, 0));
    index.cast(p.pos, p.vel, p.radius, &filter)
}

fn casts_serial(index: &Index3d, world: &World, hits: &mut [Option<CastHit>]) {
    for (i, (p, hit)) in world.projectiles.iter().zip(hits.iter_mut()).enumerate() {
        *hit = cast_one(index, i, p);
    }
}

fn casts_parallel(index: &Index3d, world: &World, hits: &mut [Option<CastHit>]) {
    par::par_chunks(hits, CAST_CHUNK, |chunk, out| {
        let base = chunk * CAST_CHUNK;
        for (k, hit) in out.iter_mut().enumerate() {
            *hit = cast_one(index, base + k, &world.projectiles[base + k]);
        }
    });
}

/// Scratch buffers, one per query kind, reused across ticks.
#[derive(Default)]
struct Scratch {
    hits: Vec<Option<CastHit>>,
    hits_parallel: Vec<Option<CastHit>>,
    hits_separate: Vec<Option<CastHit>>,
    near: Vec<Neighbor>,
    overlap: Vec<Handle>,
}

/// Every query result of one tick, folded to a comparable digest.
fn queries(
    index: &Index3d,
    world: &World,
    scratch: &mut Scratch,
    times: Option<&mut Times>,
) -> u64 {
    let mut digest = 0u64;
    let mut mix = |v: u64| digest = digest.rotate_left(5) ^ v.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut times = times;

    // The casts are timed by `casts`; their hits join the digest there.
    let t = now();
    for (i, m) in world.enemies.iter().enumerate() {
        let filter = Filter::new(ENEMY).excluding(Handle::slot(ENEMY_BASE + i as u32, 0));
        index.k_nearest(m.pos, 8, 4.0, &filter, &mut scratch.near);
        for n in &scratch.near {
            mix(u64::from(n.distance_sq.to_bits()) ^ u64::from(n.handle.index));
        }
    }
    let knn = lap(t);

    let t = now();
    for m in world.enemies.iter().take(50) {
        index.overlap_sphere(m.pos, 3.0, &Filter::new(ENEMY), &mut scratch.overlap);
        for h in &scratch.overlap {
            mix(u64::from(h.index));
        }
    }
    let splash = lap(t);

    if let Some(times) = times.as_mut() {
        times.knn.push(knn);
        times.splash.push(splash);
    }
    digest
}

#[derive(Default)]
struct Times {
    rebuild: Vec<Duration>,
    cast_serial: Vec<Duration>,
    cast_parallel: Vec<Duration>,
    cast_separate: Vec<Duration>,
    knn: Vec<Duration>,
    splash: Vec<Duration>,
}

/// Runs the tick loop with the rebuild under `threads`, returning the medians and the digest trace.
fn run(threads: usize) -> (Times, Vec<u64>) {
    let mut world = build_world();
    let mut index = Index3d::new();
    let mut casters = Index3d::new();
    let mut scratch = Scratch::default();
    for hits in [
        &mut scratch.hits,
        &mut scratch.hits_parallel,
        &mut scratch.hits_separate,
    ] {
        hits.resize(PROJECTILES, None);
    }
    let mut times = Times::default();
    let mut trace = Vec::new();
    for tick in 0..WARMUP + MEASURED {
        advance(&mut world);
        let t = now();
        par::with_threads(threads, || rebuild(&mut index, &world, true));
        let rebuild_time = lap(t);
        // Enemies and walls only: the index the casts actually need.
        par::with_threads(threads, || rebuild(&mut casters, &world, false));

        let t = now();
        casts_serial(&index, &world, &mut scratch.hits);
        let serial = lap(t);
        let t = now();
        par::with_threads(threads, || {
            casts_parallel(&index, &world, &mut scratch.hits_parallel);
        });
        let parallel = lap(t);
        let t = now();
        casts_serial(&casters, &world, &mut scratch.hits_separate);
        let separate = lap(t);
        assert_eq!(scratch.hits, scratch.hits_parallel, "parallel casts differ");
        assert_eq!(
            scratch.hits, scratch.hits_separate,
            "separate-index casts differ"
        );

        let mut digest = queries(
            &index,
            &world,
            &mut scratch,
            (tick >= WARMUP).then_some(&mut times),
        );
        for hit in scratch.hits.iter().flatten() {
            digest =
                digest.rotate_left(5) ^ u64::from(hit.t.to_bits()) ^ u64::from(hit.handle.index);
        }
        if tick >= WARMUP {
            times.rebuild.push(rebuild_time);
            times.cast_serial.push(serial);
            times.cast_parallel.push(parallel);
            times.cast_separate.push(separate);
        }
        trace.push(digest);
    }
    (times, trace)
}

#[test]
#[ignore = "benchmark: run explicitly in release with --ignored --nocapture"]
fn spatial3d_mix_fits_the_sub_budget() {
    let auto = par::auto_threads();
    let (mut one, trace_one) = run(1);
    let (mut many, trace_many) = run(auto);
    assert_eq!(
        trace_one, trace_many,
        "query results differ between 1 and {auto} threads"
    );

    let r1 = median_ms(&mut one.rebuild);
    let rn = median_ms(&mut many.rebuild);
    let cast = median_ms(&mut many.cast_serial);
    let cast_par = median_ms(&mut many.cast_parallel);
    let cast_sep = median_ms(&mut many.cast_separate);
    let knn = median_ms(&mut many.knn);
    let splash = median_ms(&mut many.splash);
    println!("rebuild ({ENEMIES} enemies + {PROJECTILES} projectiles + {WALLS} walls):");
    println!("  1 thread        {r1:.3} ms");
    println!("  {auto} threads (auto) {rn:.3} ms");
    println!("{PROJECTILES} swept casts, serial, full index   {cast:.3} ms");
    println!("{PROJECTILES} swept casts, par_chunks({CAST_CHUNK}), full {cast_par:.3} ms");
    println!("{PROJECTILES} swept casts, serial, separate index {cast_sep:.3} ms");
    println!("{ENEMIES} k_nearest (k=8)    {knn:.3} ms");
    println!("50 splash overlaps    {splash:.3} ms");
    let frame = rn + cast + knn + splash;
    println!("frame sum (serial, full index) {frame:.3} ms (budget {SPATIAL3D_BUDGET_MS} ms)");
    assert!(
        frame < SPATIAL3D_BUDGET_MS,
        "spatial3d frame sum {frame:.3} ms exceeds the {SPATIAL3D_BUDGET_MS} ms sub-budget"
    );
}
