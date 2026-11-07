//! The extract cost of drawing 5,000 live projectiles: `extract_present_scene` with the pool full
//! against the same scene with it empty. CPU-only; indicative, so it prints and asserts nothing.
//!
//! Ignored by default; run it explicitly, in release:
//!
//! ```text
//! cargo test -p ludion-engine --release --test it -- projectiles_extract_bench --ignored --nocapture
//! ```
//!
//! The wall-clock spans are observation-only and never feed sim state.

use std::time::{Duration, Instant};

use ludion_core::rng::DeterministicRng;
use ludion_engine::{Engine, ProjectileConfig, Projectiles};
use ludion_projectiles::ProjectileDesc;

const WARMUP: usize = 10;
const MEASURED: usize = 60;
const LIVE: usize = 5_000;

const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 10.0
      Camera: {}
";

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

fn extract_median(engine: &mut Engine) -> f64 {
    let mut samples = Vec::with_capacity(MEASURED);
    for i in 0..WARMUP + MEASURED {
        let start = now();
        engine.extract_present_scene().unwrap();
        let took = lap(start);
        if i >= WARMUP {
            samples.push(took);
        }
    }
    median_ms(&mut samples)
}

#[test]
#[ignore = "release-mode benchmark; run explicitly (see the module docs)"]
fn extract_cost_of_5000_shots() {
    let mut engine = Engine::builder()
        .projectiles(ProjectileConfig {
            capacity: LIVE as u32,
            ..ProjectileConfig::default()
        })
        .build()
        .expect("boots");
    engine.load_inline_scene(SCENE).unwrap();

    let empty = extract_median(&mut engine);

    let mut rng = DeterministicRng::new(7);
    let pool = engine.world_mut().resource_mut::<Projectiles>().unwrap();
    for _ in 0..LIVE {
        let f = |rng: &mut DeterministicRng| rng.next_f32() * 2.0 - 1.0;
        let position = [f(&mut rng) * 20.0, f(&mut rng) * 5.0, f(&mut rng) * 20.0];
        let velocity = [f(&mut rng) * 60.0, f(&mut rng) * 60.0, f(&mut rng) * 60.0];
        pool.spawn(ProjectileDesc::new(position, velocity)).unwrap();
    }
    let full = extract_median(&mut engine);

    println!("extract, empty pool: {empty:.3} ms");
    println!("extract, {LIVE} shots: {full:.3} ms");
    println!(
        "per-frame extract cost of {LIVE} shots: {:.3} ms",
        full - empty
    );
}
