//! The bulk-projectile benchmarks, both on a walled room with ~40 pillars and a 500-target
//! `spatial3d` index, stepped at one thread and at the machine's thread count:
//! `bulk_projectiles_bench` holds 5,000 live shots; `bulk_gibs_bench` adds 1,000 settling,
//! spinning gibs beside them.
//!
//! Ignored by default so CI stays fast; run it explicitly, in release:
//!
//! ```text
//! cargo test -p ludion-projectiles --release --test bulk_bench -- --ignored --nocapture
//! ```
//!
//! There is deliberately no `budgets/*.toml` entry: those files gate `ProfileReport` keys produced
//! by `ludion profile` or a game's `--profile`, and a crate microbench produces none. The precedent
//! is `spatial3d_bench.rs`: an inline asserted sub-budget, [`BULK_PROJECTILES_BUDGET_MS`]. The
//! wall-clock spans are observation-only and never feed sim state.

mod common;

use std::time::{Duration, Instant};

use common::*;
use ludion_core::par;
use ludion_core::rng::DeterministicRng;
use ludion_projectiles::{ProjectileDesc, ProjectilePool, Response, StepEvents, step};
use ludion_spatial::spatial3d::Handle;

/// Per-frame sub-budget, in milliseconds, for the serial arm: `step` + `digest` with 5,000 live
/// shots. About 3x the release median sum (1.87 ms: step 1.79, digest 0.08; 0.46 ms at 8 threads) on an unpinned M5 laptop
/// (not authoritative), 2026-11-07, rounded up. Pinned M4 / RTX 3090 figures are owed.
const BULK_PROJECTILES_BUDGET_MS: f64 = 6.0;

/// Per-frame sub-budget, in milliseconds, for the serial gib arm: `step` + `digest` with 5,000
/// shots and 1,000 gibs. About 3x the release median sum (1.92 ms: step 1.82, digest 0.10; 0.51 ms
/// at 8 threads; ~389 of 1,000 gibs parked) on an unpinned M5 laptop (not authoritative),
/// 2026-11-08, rounded up. Pinned M4 / RTX 3090 figures are owed.
const BULK_GIBS_BUDGET_MS: f64 = 6.0;

const WARMUP: usize = 10;
const MEASURED: usize = 60;
const LIVE: usize = 5_000;
const GIBS: usize = 1_000;
const GIB_TAG: u32 = 1;
const PREROLL: usize = 300;
const TARGETS: usize = 500;
const PILLARS: usize = 40;
const ROOM: [f32; 3] = [120.0, 12.0, 120.0];

#[allow(clippy::disallowed_methods)]
fn now() -> Instant {
    Instant::now()
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

fn dir(rng: &mut DeterministicRng) -> [f32; 3] {
    loop {
        let v = [
            range(rng, -1.0, 1.0),
            range(rng, -1.0, 1.0),
            range(rng, -1.0, 1.0),
        ];
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if (0.1..=1.0).contains(&len) {
            return [v[0] / len, v[1] / len, v[2] / len];
        }
    }
}

fn shot(rng: &mut DeterministicRng) -> ProjectileDesc {
    let d = dir(rng);
    let speed = range(rng, 30.0, 120.0);
    let mut desc = ProjectileDesc::new(
        [
            range(rng, 2.0, ROOM[0] - 2.0),
            range(rng, 1.0, ROOM[1] - 1.0),
            range(rng, 2.0, ROOM[2] - 2.0),
        ],
        [d[0] * speed, d[1] * speed, d[2] * speed],
    );
    desc.radius = if rng.next_f32() < 0.5 {
        0.0
    } else {
        range(rng, 0.02, 0.1)
    };
    desc.lifetime = range(rng, 1.0, 3.0);
    let r = rng.next_f32();
    desc.response = if r < 0.7 {
        Response::Despawn
    } else if r < 0.9 {
        Response::Reflect {
            restitution: 0.6,
            max_bounces: 3,
        }
    } else {
        Response::Pierce { max: 2 }
    };
    desc
}

fn gib(rng: &mut DeterministicRng) -> ProjectileDesc {
    let mut desc = ProjectileDesc::new(
        [
            range(rng, 2.0, ROOM[0] - 2.0),
            range(rng, 0.5, 4.0),
            range(rng, 2.0, ROOM[2] - 2.0),
        ],
        [
            range(rng, -6.0, 6.0),
            range(rng, 2.0, 8.0),
            range(rng, -6.0, 6.0),
        ],
    );
    desc.gravity_scale = 1.0;
    desc.radius = range(rng, 0.05, 0.15);
    desc.response = Response::settle(0.35, 0.4, 0.6);
    desc.spin = [
        range(rng, -10.0, 10.0),
        range(rng, -10.0, 10.0),
        range(rng, -10.0, 10.0),
    ];
    desc.targets = Some(0);
    desc.lifetime = range(rng, 1.5, 6.0);
    desc.tag = GIB_TAG;
    desc
}

struct Arm {
    digests: Vec<u64>,
    step: Vec<Duration>,
    digest: Vec<Duration>,
    sum: Vec<Duration>,
    live: usize,
    parked: usize,
}

fn run_arm(threads: usize, gibs: usize) -> Arm {
    let mut rng = DeterministicRng::new(0xB01C);
    let mut gib_rng = DeterministicRng::new(0x61B5);
    let w = ROOM;
    let mut boxes = vec![
        (
            [w[0] / 2.0, -1.0, w[2] / 2.0],
            [w[0] / 2.0, 1.0, w[2] / 2.0],
        ),
        (
            [w[0] / 2.0, w[1] + 1.0, w[2] / 2.0],
            [w[0] / 2.0, 1.0, w[2] / 2.0],
        ),
        (
            [-1.0, w[1] / 2.0, w[2] / 2.0],
            [1.0, w[1] / 2.0, w[2] / 2.0],
        ),
        (
            [w[0] + 1.0, w[1] / 2.0, w[2] / 2.0],
            [1.0, w[1] / 2.0, w[2] / 2.0],
        ),
        (
            [w[0] / 2.0, w[1] / 2.0, -1.0],
            [w[0] / 2.0, w[1] / 2.0, 1.0],
        ),
        (
            [w[0] / 2.0, w[1] / 2.0, w[2] + 1.0],
            [w[0] / 2.0, w[1] / 2.0, 1.0],
        ),
    ];
    for _ in 0..PILLARS {
        let half = [
            range(&mut rng, 0.5, 2.0),
            w[1] / 2.0,
            range(&mut rng, 0.5, 2.0),
        ];
        boxes.push((
            [
                range(&mut rng, 5.0, w[0] - 5.0),
                w[1] / 2.0,
                range(&mut rng, 5.0, w[2] - 5.0),
            ],
            half,
        ));
    }
    let (world, _) = box_world(&boxes);
    let physics = physics_ref(&world);

    let items: Vec<_> = (0..TARGETS as u32)
        .map(|i| {
            sphere(
                Handle::slot(10_000 + i, 0),
                [
                    range(&mut rng, 2.0, w[0] - 2.0),
                    range(&mut rng, 1.0, w[1] - 1.0),
                    range(&mut rng, 2.0, w[2] - 2.0),
                ],
                range(&mut rng, 0.4, 1.2),
                1,
            )
        })
        .collect();
    let index = index_of(&items);
    let mut cfg = config(physics);
    cfg.targets = 1;
    if gibs > 0 {
        cfg.gravity = [0.0, -9.81, 0.0];
    }

    let mut pool = ProjectilePool::with_capacity(LIVE + gibs);
    let mut events = StepEvents::with_capacity((LIVE + gibs) * 2, LIVE + gibs);
    let mut arm = Arm {
        digests: Vec::new(),
        step: Vec::new(),
        digest: Vec::new(),
        sum: Vec::new(),
        live: 0,
        parked: 0,
    };
    par::with_threads(threads, || {
        let preroll = if gibs > 0 { PREROLL } else { 0 };
        for tick in 0..preroll + WARMUP + MEASURED {
            if gibs == 0 {
                while pool.len() < LIVE {
                    let _ = pool.spawn(shot(&mut rng));
                }
            } else {
                let gib_count = pool
                    .ids()
                    .filter(|&id| pool.tag(id) == Some(GIB_TAG))
                    .count();
                for _ in pool.len() - gib_count..LIVE {
                    let _ = pool.spawn(shot(&mut rng));
                }
                for _ in gib_count..gibs {
                    let _ = pool.spawn(gib(&mut gib_rng));
                }
            }
            let t0 = now();
            step(&mut pool, DT, physics, &index, &cfg, &mut events);
            let t1 = now();
            let digest = pool.digest();
            let t2 = now();
            arm.digests.push(digest);
            if tick >= preroll + WARMUP {
                if gibs > 0 {
                    arm.parked += pool
                        .ids()
                        .filter(|&id| {
                            pool.tag(id) == Some(GIB_TAG) && pool.is_parked(id) == Some(true)
                        })
                        .count();
                }
                arm.step.push(t1.duration_since(t0));
                arm.digest.push(t2.duration_since(t1));
                arm.sum.push(t2.duration_since(t0));
                arm.live += pool.len();
            }
        }
    });
    arm
}

#[test]
#[ignore = "release-mode benchmark; run explicitly (see the module docs)"]
fn bulk_projectiles_bench() {
    let n = par::auto_threads();
    let mut serial = run_arm(1, 0);
    let mut wide = run_arm(n, 0);
    assert_eq!(
        serial.digests, wide.digests,
        "the pool digest trace differs between 1 and {n} threads"
    );
    let sum1 = median_ms(&mut serial.sum);
    for (label, arm) in [
        ("1 thread", &mut serial),
        (&*format!("{n} threads"), &mut wide),
    ] {
        println!(
            "{label:>10}: step {:.3} ms, digest {:.3} ms, sum {:.3} ms, live after step ~{}",
            median_ms(&mut arm.step),
            median_ms(&mut arm.digest),
            median_ms(&mut arm.sum),
            arm.live / MEASURED,
        );
    }
    assert!(
        sum1 < BULK_PROJECTILES_BUDGET_MS,
        "serial step + digest {sum1:.3} ms is over the {BULK_PROJECTILES_BUDGET_MS} ms budget"
    );
}

#[test]
#[ignore = "release-mode benchmark; run explicitly (see the module docs)"]
fn bulk_gibs_bench() {
    let n = par::auto_threads();
    let mut serial = run_arm(1, GIBS);
    let mut wide = run_arm(n, GIBS);
    assert_eq!(
        serial.digests, wide.digests,
        "the pool digest trace differs between 1 and {n} threads"
    );
    let sum1 = median_ms(&mut serial.sum);
    for (label, arm) in [
        ("1 thread", &mut serial),
        (&*format!("{n} threads"), &mut wide),
    ] {
        println!(
            "{label:>10}: 5000 shots + 1000 gibs: step {:.3} ms, digest {:.3} ms, sum {:.3} ms, live after step ~{}, parked ~{} of {GIBS}",
            median_ms(&mut arm.step),
            median_ms(&mut arm.digest),
            median_ms(&mut arm.sum),
            arm.live / MEASURED,
            arm.parked / MEASURED,
        );
    }
    assert!(
        sum1 < BULK_GIBS_BUDGET_MS,
        "serial step + digest {sum1:.3} ms is over the {BULK_GIBS_BUDGET_MS} ms budget"
    );
}
