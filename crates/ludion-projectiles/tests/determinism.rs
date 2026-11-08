//! Thread-count invariance and run-to-run identity, with spawn/despawn churn.

mod common;

use common::*;
use ludion_core::par;
use ludion_projectiles::{HitOutcome, ProjectileDesc, ProjectilePool, Response, StepEvents, step};
use ludion_spatial::spatial3d::Handle;

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
    fn unit(&mut self) -> f32 {
        (self.next() % 10_000) as f32 / 10_000.0
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.unit() * (hi - lo)
    }
}

fn desc(rng: &mut Lcg) -> ProjectileDesc {
    let mut d = ProjectileDesc::new(
        [
            rng.range(0.0, 60.0),
            rng.range(0.5, 6.0),
            rng.range(0.0, 60.0),
        ],
        [
            rng.range(-40.0, 40.0),
            rng.range(-5.0, 5.0),
            rng.range(-40.0, 40.0),
        ],
    );
    d.gravity_scale = rng.unit();
    d.drag = rng.unit() * 0.2;
    d.radius = if rng.next().is_multiple_of(2) {
        0.0
    } else {
        0.1
    };
    d.lifetime = rng.range(0.2, 2.0);
    d.tag = rng.next();
    d.response = match rng.next() % 4 {
        0 => Response::Despawn,
        1 => Response::Reflect {
            restitution: 0.75,
            max_bounces: 3,
        },
        2 => Response::Pierce { max: 3 },
        _ => Response::Stop,
    };
    d
}

fn gib_desc(rng: &mut Lcg) -> ProjectileDesc {
    let mut d = desc(rng);
    if rng.next().is_multiple_of(3) {
        d.response = Response::settle(0.5, 0.4, 0.8);
        d.spin = [
            rng.range(-6.0, 6.0),
            rng.range(-6.0, 6.0),
            rng.range(-6.0, 6.0),
        ];
        d.orientation = [
            rng.range(-1.0, 1.0),
            rng.range(-1.0, 1.0),
            rng.range(-1.0, 1.0),
            1.0,
        ];
        d.targets = Some(0);
        d.gravity_scale = 1.0;
    }
    d
}

fn trace() -> (Vec<u64>, Vec<u32>) {
    trace_with(desc).0
}

fn trace_gibs() -> ((Vec<u64>, Vec<u32>), usize) {
    trace_with(gib_desc)
}

/// The traces plus how many shots were parked on the last tick.
fn trace_with(desc: fn(&mut Lcg) -> ProjectileDesc) -> ((Vec<u64>, Vec<u32>), usize) {
    let mut boxes = Vec::new();
    for i in 0..5 {
        for j in 0..5 {
            boxes.push((
                [6.0 + 12.0 * i as f32, 3.0, 6.0 + 12.0 * j as f32],
                [1.5, 3.0, 1.5],
            ));
        }
    }
    let (world, _) = box_world(&boxes);
    let physics = physics_ref(&world);
    let mut items = Vec::new();
    for i in 0..5u32 {
        for j in 0..5u32 {
            items.push(sphere(
                Handle::slot(10_000 + i * 5 + j, 0),
                [12.0 * i as f32, 2.0, 12.0 * j as f32],
                1.25,
                1,
            ));
        }
    }
    let index = index_of(&items);
    let mut cfg = config(physics);
    cfg.gravity = [0.0, -9.8, 0.0];
    cfg.targets = 1;

    let mut rng = Lcg(7);
    let mut pool = ProjectilePool::with_capacity(2_600);
    for _ in 0..2_000 {
        pool.spawn(desc(&mut rng)).unwrap();
    }
    let mut events = StepEvents::with_capacity(8_192, 2_600);
    let (mut digests, mut bits) = (Vec::new(), Vec::new());
    for tick in 0..30 {
        step(&mut pool, DT, physics, &index, &cfg, &mut events);
        for h in &events.hits {
            bits.extend([
                h.id.index,
                h.id.generation,
                h.handle.index,
                h.handle.generation,
            ]);
            bits.extend(h.point.map(f32::to_bits));
            bits.extend(h.normal.map(f32::to_bits));
            bits.extend(h.velocity.map(f32::to_bits));
            bits.extend([h.t.to_bits(), h.tag, h.outcome as u32]);
        }
        for e in &events.expired {
            bits.extend([e.id.index, e.id.generation, e.tag]);
            bits.extend(e.position.map(f32::to_bits));
        }
        // Churn: despawn every 7th live id and respawn a handful.
        let ids: Vec<_> = pool.ids().collect();
        for id in ids.iter().step_by(7) {
            pool.despawn(*id);
        }
        for _ in 0..40 {
            let _ = pool.spawn(desc(&mut rng));
        }
        digests.push(pool.digest());
        bits.push(tick);
    }
    assert!(
        events.hits.len() + bits.len() > 1_000,
        "the scene must exercise the step"
    );
    let parked = pool
        .ids()
        .filter(|&id| pool.is_parked(id) == Some(true))
        .count();
    ((digests, bits), parked)
}

#[test]
fn digest_and_events_are_identical_at_any_thread_count_and_across_runs() {
    let base = par::with_threads(1, trace);
    let outcomes = base.1.len();
    assert!(
        outcomes > 2_000,
        "too few events to mean anything: {outcomes}"
    );
    for n in [3, 8] {
        assert_eq!(par::with_threads(n, trace), base, "{n} threads");
    }
    assert_eq!(par::with_threads(1, trace), base, "second run");
}

#[test]
fn gib_digest_and_events_are_identical_at_any_thread_count_and_across_runs() {
    let base = par::with_threads(1, trace_gibs);
    assert!(base.1 > 0, "some shots end parked");
    for n in [3, 8] {
        assert_eq!(par::with_threads(n, trace_gibs), base, "{n} threads");
    }
    assert_eq!(par::with_threads(1, trace_gibs), base, "second run");
}

#[test]
fn every_outcome_kind_occurs() {
    let mut boxes = Vec::new();
    for i in 0..5 {
        for j in 0..5 {
            boxes.push((
                [6.0 + 12.0 * i as f32, 3.0, 6.0 + 12.0 * j as f32],
                [1.5, 3.0, 1.5],
            ));
        }
    }
    let (world, _) = box_world(&boxes);
    let physics = physics_ref(&world);
    let mut cfg = config(physics);
    cfg.gravity = [0.0, -9.8, 0.0];
    let index = index_of(&[]);
    let mut rng = Lcg(7);
    let mut pool = ProjectilePool::with_capacity(2_000);
    for _ in 0..2_000 {
        pool.spawn(desc(&mut rng)).unwrap();
    }
    let mut events = StepEvents::with_capacity(8_192, 2_000);
    let mut seen = [false; 4];
    for _ in 0..30 {
        step(&mut pool, DT, physics, &index, &cfg, &mut events);
        for h in &events.hits {
            seen[match h.outcome {
                HitOutcome::Despawned => 0,
                HitOutcome::Reflected => 1,
                HitOutcome::Pierced => 2,
                HitOutcome::Stopped => 3,
            }] = true;
        }
    }
    // Pierce needs index targets, so the world-only scene never pierces.
    assert!(seen[0] && seen[1] && seen[3], "{seen:?}");
}
