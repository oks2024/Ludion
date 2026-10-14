//! The E6-mix microbenchmark (Wave 2) — measures the pinned-order spatial design against the
//! first-game numbers **before** the API freezes (Wave 3).
//!
//! Ignored by default so CI stays fast; run it explicitly, in release:
//!
//! ```text
//! cargo test -p ludion-spatial --release --test e6_bench -- --ignored --nocapture
//! ```
//!
//! This reuses the `ludion-engine` `profile.rs` measurement *pattern* — median over N frames of a
//! wall-clock span taken in a scoped `#[allow(clippy::disallowed_methods)]` — not criterion (the
//! repo has no `benches/`) and not `ludion profile` (the CLI runs no game systems, so it cannot see
//! `spatial::rebuild`). It constructs a synthetic world at the E6 entity mix (~500 dynamic seekers,
//! 300 projectiles, 200 pickups, 300 random static AABBs, plus a 4-wall production-scale merged
//! perimeter) in a deterministic layout, then times, per simulated frame: one `rebuild`, 500
//! `k_nearest` (separation/targeting-shaped), 300 `overlaps` (projectile-hit-shaped), and 500
//! `move_and_slide` (each seeker sweeping its per-frame velocity against the statics — added by
//! Wave 4, completing the sample set the plan's Wave 2 text named), asserting their medians' sum
//! fits the recorded spatial sub-budget.

use std::time::{Duration, Instant};

use ludion_core::ids::EntityId;
use ludion_core::rng::DeterministicRng;
use ludion_core::time::{FrameIndex, TickRate};
use ludion_ecs::{SystemCtx, World};
use ludion_spatial::{Body, Collider, GroundPosition, Plane, Shape, spatial};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The spatial sub-budget, in milliseconds per frame, carved from the 16.6 ms frame for the whole
/// per-frame spatial workload (one rebuild + 500 k_nearest + 300 overlaps + 500 move_and_slide at
/// the E6 mix). Recorded in `Plan/phase-12-spatial.md` §Status (Wave 2, slide sample added by
/// Wave 4); set ~2× above the pinned-machine measurement at the tuned cell size (~0.66 ms median
/// sum in release with the slide sample and the perimeter walls) and under the plan's ≤ ~2 ms
/// guidance, so a real regression trips it but machine noise does not.
const SUB_BUDGET_MS: f64 = 1.5;

const WARMUP_FRAMES: usize = 10;
const MEASURED_FRAMES: usize = 60;

/// Ground half-extent of the square arena the mix is scattered over (world units).
const ARENA_HALF: f32 = 60.0;

const SEEKERS: usize = 500;
const PROJECTILES: usize = 300;
const PICKUPS: usize = 200;
const STATICS: usize = 300;

/// A minimal position component (the bench must not depend on `ludion-engine`'s `Transform`).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
struct Pos {
    xz: [f32; 2],
}

impl GroundPosition for Pos {
    fn ground_position(&self, _plane: Plane) -> [f32; 2] {
        self.xz
    }
}

/// Sanctioned wall-clock site (the `profile.rs` pattern): measurement is pure observation — the
/// spans are output-only and never feed sim state, so determinism is provably unaffected.
#[allow(clippy::disallowed_methods)]
fn now() -> Instant {
    Instant::now()
}

/// Closes a span opened by [`now`] — the same sanctioned wall-clock site.
#[allow(clippy::disallowed_methods)]
fn lap(start: Instant) -> Duration {
    start.elapsed()
}

/// Median of a duration sample set in milliseconds (the `profile.rs` `median_ms` shape).
fn median_ms(samples: &mut [Duration]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.sort_unstable();
    let mid = samples.len() / 2;
    let ms = |d: Duration| d.as_secs_f64() * 1_000.0;
    if samples.len() % 2 == 1 {
        ms(samples[mid])
    } else {
        (ms(samples[mid - 1]) + ms(samples[mid])) / 2.0
    }
}

fn coord(rng: &mut DeterministicRng) -> f32 {
    (rng.next_f32() - 0.5) * 2.0 * ARENA_HALF
}

/// One dynamic mover: its entity, plus a fixed per-entity drift velocity (units/frame) so the
/// index genuinely changes every frame, as it would under a real horde.
struct Mover {
    entity: EntityId,
    velocity: [f32; 2],
}

fn spawn(world: &mut World, rng: &mut DeterministicRng, shape: Shape, body: Body) -> EntityId {
    let e = world.spawn();
    let xz = [coord(rng), coord(rng)];
    world
        .insert(
            e,
            Collider {
                shape,
                body,
                layer: None,
            },
        )
        .unwrap();
    world.insert(e, Pos { xz }).unwrap();
    e
}

fn drift(world: &mut World, movers: &[Mover]) {
    for mover in movers {
        let pos = world.get_mut::<Pos>(mover.entity).unwrap().unwrap();
        for axis in 0..2 {
            pos.xz[axis] += mover.velocity[axis];
            // Wrap at the arena edge so density stays constant over the run.
            if pos.xz[axis] > ARENA_HALF {
                pos.xz[axis] -= 2.0 * ARENA_HALF;
            } else if pos.xz[axis] < -ARENA_HALF {
                pos.xz[axis] += 2.0 * ARENA_HALF;
            }
        }
    }
}

#[test]
#[ignore = "benchmark: run explicitly in release with --ignored --nocapture"]
fn e6_mix_fits_the_spatial_sub_budget() {
    let mut world = World::new();
    let mut rng = DeterministicRng::new(0xE6);

    let mut movers: Vec<Mover> = Vec::new();
    let mover = |entity: EntityId, rng: &mut DeterministicRng, speed: f32| Mover {
        entity,
        velocity: [
            (rng.next_f32() - 0.5) * 2.0 * speed,
            (rng.next_f32() - 0.5) * 2.0 * speed,
        ],
    };

    // ~500 seeking enemies: dynamic circles at walk speed.
    let mut seekers = Vec::with_capacity(SEEKERS);
    for _ in 0..SEEKERS {
        let e = spawn(
            &mut world,
            &mut rng,
            Shape::Circle { radius: 0.4 },
            Body::new_kinematic(),
        );
        movers.push(mover(e, &mut rng, 0.05));
        seekers.push(e);
    }
    // 300 projectiles: small, fast dynamic circles.
    let mut projectiles = Vec::with_capacity(PROJECTILES);
    for _ in 0..PROJECTILES {
        let e = spawn(
            &mut world,
            &mut rng,
            Shape::Circle { radius: 0.15 },
            Body::new_kinematic(),
        );
        movers.push(mover(e, &mut rng, 0.3));
        projectiles.push(e);
    }
    // 200 pickups: static circles (auras find them via k_nearest).
    for _ in 0..PICKUPS {
        spawn(
            &mut world,
            &mut rng,
            Shape::Circle { radius: 0.5 },
            Body::new_static(),
        );
    }
    // A few hundred statics: walls/props as AABBs.
    for _ in 0..STATICS {
        let half_extents = [0.5 + rng.next_f32() * 2.0, 0.5 + rng.next_f32() * 2.0];
        spawn(
            &mut world,
            &mut rng,
            Shape::Aabb { half_extents },
            Body::new_static(),
        );
    }

    // The production perimeter-wall workload class: four merged static AABBs enclosing the arena,
    // sized and placed like `generate-map`'s merged perimeter output (side walls `half_extents
    // [2.0, H + 4.0]` at `x = ±(H + 2.0)`, H the arena half-extent) — the exact degenerate shape
    // whose statics-side `max_reach` is arena-scale, the case the random ~0.5–2.5 statics above
    // never exercise.
    let wall_half = ARENA_HALF + 4.0;
    let wall_offset = ARENA_HALF + 2.0;
    let walls = [
        ([wall_half, 2.0], [0.0, wall_offset]),
        ([wall_half, 2.0], [0.0, -wall_offset]),
        ([2.0, wall_half], [wall_offset, 0.0]),
        ([2.0, wall_half], [-wall_offset, 0.0]),
    ];
    for (half_extents, xz) in walls {
        let e = world.spawn();
        world
            .insert(
                e,
                Collider {
                    shape: Shape::Aabb { half_extents },
                    body: Body::new_static(),
                    layer: None,
                },
            )
            .unwrap();
        world.insert(e, Pos { xz }).unwrap();
    }
    // Measured in release with the walls above (3 runs): frame sum 0.6562 / 0.6592 / 0.6909 ms,
    // inside SUB_BUDGET_MS (1.5 ms) — rebuild ~0.077, k_nearest ~0.245, overlaps ~0.16,
    // move_and_slide ~0.18. Each wall's reach exceeds one cell, so the index files it in its half's
    // oversize grid, where it is scanned by every query of that half but keeps the half's
    // `max_reach` at a prop's: no probe rectangle is inflated by a map. Against 0.5256 ms with the
    // walls removed and everything else unchanged, the residual ~0.13 ms is that unpruned scan
    // (four entries per probe, ~2300 probes a frame) plus the wider merge — the price of the walls
    // being *findable*, not of pruning failing. Filing them in the cell grid instead costs
    // 2.16–2.22 ms, over budget, with the whole excess on the two queries that admit the statics
    // half (`overlaps`'s `None` filter, `move_and_slide`'s statics-only design).

    let ctx = SystemCtx::new(FrameIndex::ZERO, TickRate::DEFAULT);

    let mut rebuild_spans = Vec::with_capacity(MEASURED_FRAMES);
    let mut k_nearest_spans = Vec::with_capacity(MEASURED_FRAMES);
    let mut overlaps_spans = Vec::with_capacity(MEASURED_FRAMES);
    let mut slide_spans = Vec::with_capacity(MEASURED_FRAMES);

    for frame in 0..(WARMUP_FRAMES + MEASURED_FRAMES) {
        drift(&mut world, &movers); // sim work, outside every timed span

        let start = now();
        spatial::rebuild::<Pos>(&mut world, &ctx);
        let rebuild_span = lap(start);

        // 500 k_nearest: separation/targeting-shaped — small k, modest radius, movers only.
        let seeker_centers: Vec<[f32; 2]> = seekers
            .iter()
            .map(|&e| world.get::<Pos>(e).unwrap().unwrap().xz)
            .collect();
        let start = now();
        let mut k_hits = 0usize;
        for &center in &seeker_centers {
            k_hits += spatial::k_nearest(&world, center, 8, 4.0, Some(Body::new_kinematic())).len();
        }
        let k_nearest_span = lap(start);

        // 300 overlaps: projectile-hit-shaped — the projectile's own circle against everything.
        let projectile_centers: Vec<[f32; 2]> = projectiles
            .iter()
            .map(|&e| world.get::<Pos>(e).unwrap().unwrap().xz)
            .collect();
        let start = now();
        let mut o_hits = 0usize;
        for &center in &projectile_centers {
            o_hits += spatial::overlaps(&world, Shape::Circle { radius: 0.15 }, center, None).len();
        }
        let overlaps_span = lap(start);

        // 500 move_and_slide: each seeker sweeps its per-frame velocity against the statics
        // (pure reads — the drift above stays the one position write, outside the spans).
        let start = now();
        let mut slid = [0.0f32; 2];
        for (i, &seeker) in seekers.iter().enumerate() {
            let resolved = spatial::move_and_slide::<Pos>(&world, seeker, movers[i].velocity);
            slid[0] += resolved[0];
            slid[1] += resolved[1];
        }
        let slide_span = lap(start);

        std::hint::black_box((k_hits, o_hits, slid));
        if frame >= WARMUP_FRAMES {
            rebuild_spans.push(rebuild_span);
            k_nearest_spans.push(k_nearest_span);
            overlaps_spans.push(overlaps_span);
            slide_spans.push(slide_span);
        }
    }

    let rebuild_ms = median_ms(&mut rebuild_spans);
    let k_nearest_ms = median_ms(&mut k_nearest_spans);
    let overlaps_ms = median_ms(&mut overlaps_spans);
    let slide_ms = median_ms(&mut slide_spans);
    let sum_ms = rebuild_ms + k_nearest_ms + overlaps_ms + slide_ms;

    println!("e6 spatial microbench (cell size {}):", spatial::CELL_SIZE);
    println!("  rebuild            median {rebuild_ms:.4} ms");
    println!("  k_nearest x{SEEKERS}     median {k_nearest_ms:.4} ms");
    println!("  overlaps  x{PROJECTILES}     median {overlaps_ms:.4} ms");
    println!("  move_and_slide x{SEEKERS} median {slide_ms:.4} ms");
    println!("  frame sum          {sum_ms:.4} ms  (sub-budget {SUB_BUDGET_MS} ms)");

    assert!(
        sum_ms <= SUB_BUDGET_MS,
        "per-frame spatial total {sum_ms:.4} ms exceeds the recorded sub-budget {SUB_BUDGET_MS} ms \
         (rebuild {rebuild_ms:.4} + k_nearest {k_nearest_ms:.4} + overlaps {overlaps_ms:.4} + \
         move_and_slide {slide_ms:.4})"
    );
}
