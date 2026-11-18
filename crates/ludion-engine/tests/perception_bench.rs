//! The perception benchmark (`Plan/perception.md` Wave 4b): 30 perceivers in a ring, two zones each,
//! watching 12 targets of 3 points each clustered at the ring's centre, with a few occluders so the
//! rays hit. It times `perception_update` at 1 and 8 threads and reports the median, the p95 and the
//! rays cast.
//!
//! Ignored by default so CI stays fast; run it explicitly, in release:
//!
//! ```text
//! cargo test -p ludion-engine --release --test it -- perception_bench --ignored --nocapture
//! ```
//!
//! It reports and does not gate: there is no `budgets/*.toml` entry and no wall-clock assertion. The
//! 0.5 ms target stays Proposed (`Plan/perception.md`). The wall-clock spans are observation-only and
//! never feed sim state; the 1-thread and 8-thread `WorldHash` traces are asserted equal.

use std::f32::consts::TAU;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ludion_core::math;
use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Engine, PerceptionConfig, PerceptionResults, perception_update};

const PERCEIVERS: usize = 30;
const TARGETS: usize = 12;
const WARMUP: usize = 10;
const MEASURED: usize = 120;
const RING_RADIUS: f32 = 20.0;

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

fn p95_ms(samples: &[Duration]) -> f64 {
    let mut s = samples.to_vec();
    s.sort_unstable();
    ms(s[(s.len() * 95 / 100).min(s.len() - 1)])
}

fn entity(name: &str, at: [f32; 3], rotation: Option<[f32; 3]>, components: &str) -> String {
    let rotation = rotation.map_or(String::new(), |r| {
        format!(
            "        rotation:\n          - {:?}\n          - {:?}\n          - {:?}\n",
            r[0], r[1], r[2]
        )
    });
    format!(
        "  - name: {name}\n    components:\n      Transform:\n        translation:\n          - {:?}\n          - {:?}\n          - {:?}\n{rotation}{components}",
        at[0], at[1], at[2]
    )
}

const GUARD: &str = "      Perceiver:\n        eye:\n          - 0.0\n          - 1.6\n          - 0.0\n        sight:\n          - name: inner\n            range: 12.0\n            half_angle_deg: 50.0\n            detects:\n              - player\n          - name: outer\n            range: 30.0\n            half_angle_deg: 40.0\n            detects:\n              - player\n";

const TARGET: &str = "      SightTarget:\n        class: player\n        points:\n          - offset:\n              - 0.0\n              - 1.6\n              - 0.0\n            blocked_by:\n              - cover\n          - offset:\n              - 0.0\n              - 1.0\n              - 0.0\n            blocked_by:\n              - cover\n          - offset:\n              - 0.0\n              - 0.4\n              - 0.0\n            blocked_by:\n              - cover\n";

const BLOCK: &str = "      Collider:\n        shape:\n          box:\n            half_extents:\n              - 0.6\n              - 1.0\n              - 0.6\n        body: static\n        layer: cover\n";

fn bench_scene() -> String {
    let mut out = String::from(
        "format: 1\nseed: 1\nphysics:\n  space: free\n  layers:\n    - name: cover\nentities:\n",
    );
    for i in 0..PERCEIVERS {
        let angle = TAU * i as f32 / PERCEIVERS as f32;
        let yaw = 90.0 - angle.to_degrees();
        out += &entity(
            &format!("guard{i}"),
            [
                RING_RADIUS * math::cos(angle),
                0.0,
                RING_RADIUS * math::sin(angle),
            ],
            Some([0.0, yaw, 0.0]),
            GUARD,
        );
    }
    for i in 0..TARGETS {
        let angle = TAU * i as f32 / TARGETS as f32;
        out += &entity(
            &format!("target{i}"),
            [2.0 * math::cos(angle), 0.0, 2.0 * math::sin(angle)],
            None,
            TARGET,
        );
    }
    for i in 0..6 {
        let angle = TAU * (i as f32 + 0.5) / 6.0;
        out += &entity(
            &format!("cover{i}"),
            [8.0 * math::cos(angle), 1.0, 8.0 * math::sin(angle)],
            None,
            BLOCK,
        );
    }
    out
}

type Laps = Arc<Mutex<Vec<Duration>>>;

struct Arm {
    hashes: Vec<u64>,
    update: Vec<Duration>,
    rays: u32,
}

fn run_arm(threads: usize) -> Arm {
    par::with_threads(threads, || {
        let laps = Laps::default();
        let sink = Arc::clone(&laps);
        let timed = move |world: &mut World, ctx: &SystemCtx| {
            let t = now();
            perception_update(world, ctx);
            sink.lock().unwrap().push(lap(t));
        };
        let mut engine = Engine::builder()
            .perception(PerceptionConfig {
                classes: vec!["player".into()],
                ..Default::default()
            })
            .system("perception_update", timed)
            .build()
            .expect("boots");
        engine.load_inline_scene(&bench_scene()).unwrap();
        let mut hashes = Vec::new();
        for _ in 0..WARMUP + MEASURED {
            engine.advance_live();
            hashes.push(engine.world_hash().unwrap().as_u64());
        }
        let rays = engine
            .world()
            .resource::<PerceptionResults>()
            .unwrap()
            .rays_cast();
        let update = laps.lock().unwrap()[WARMUP..].to_vec();
        Arm {
            hashes,
            update,
            rays,
        }
    })
}

#[test]
#[ignore = "benchmark: run explicitly in release with --ignored --nocapture"]
fn perception_update_30_perceivers() {
    let one = run_arm(1);
    let eight = run_arm(8);
    for (threads, arm) in [(1, &one), (8, &eight)] {
        println!(
            "perception_update: {PERCEIVERS} perceivers x 2 zones x {TARGETS} targets x 3 points, {threads} thread(s): median {:.3} ms, p95 {:.3} ms, {} rays/tick",
            median_ms(&arm.update),
            p95_ms(&arm.update),
            arm.rays
        );
    }
    println!("target 0.5 ms (Proposed)");
    assert!(one.rays > 0, "the rays are really cast");
    assert_eq!(
        one.hashes, eight.hashes,
        "1 and 8 threads trace the same hashes"
    );
}
