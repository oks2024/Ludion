//! The checkpoint cost bench (`Plan/checkpoints.md` Wave 4b): what one capture and one restore cost on
//! a 5 000-entity slice greybox with physics (Neon Shroud request 0012 §6: capture <= 4 ms, restore
//! <= 50 ms).
//!
//! The scene: a static floor, 400 resting rigid boxes, 600 static box colliders and 4 000 non-physics
//! props, each carrying a `Guard` (a squad/AI blackboard stand-in) that a system mutates every tick.
//!
//! A `Checkpoints` request made during tick N is drained in the same `advance_live()` call, so the bench
//! times whole `advance_live()` calls from outside the engine and subtracts the frame before: on a cycle
//! of 8 frames a director captures at offset 2 and restores at offset 6, so
//! `capture = t(2) - t(1)` and `restore = t(6) - t(5)`. No engine hook, wire field or profiler key.
//!
//! Ignored by default so CI stays fast; run it explicitly, in release:
//!
//! ```text
//! cargo test -p ludion-engine --release --test it -- checkpoint_bench --ignored --nocapture
//! ```
//!
//! **Budget file.** It gates the medians against `budgets/checkpoints-<os>.toml` (own schema, the
//! request's fixed targets) when the file exists. The wall-clock spans are observation-only and never
//! feed sim state.

use std::path::Path;
use std::time::{Duration, Instant};

use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{CheckpointConfig, Checkpoints, Engine, RestoreOutcome};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const THREADS: usize = 8;
const CYCLE: u64 = 8;
const CAPTURE_AT: u64 = 2;
const RESTORE_AT: u64 = 6;
const WARMUP_CYCLES: usize = 3;
const CYCLES: usize = 40;
const BODIES: usize = 400;
const STATICS: usize = 600;
const PROPS: usize = 4000;

/// The per-entity state a squad/AI blackboard would hold.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct Guard {
    hp: f32,
    state: u8,
    timer: u32,
    target: Option<[f32; 3]>,
}

#[derive(Clone, Copy)]
struct Scale {
    bodies: usize,
    statics: usize,
    props: usize,
}

const FULL: Scale = Scale {
    bodies: BODIES,
    statics: STATICS,
    props: PROPS,
};

const SMALL: Scale = Scale {
    bodies: 8,
    statics: 8,
    props: 40,
};

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

fn translation(pad: &str, at: [f32; 3]) -> String {
    format!(
        "{pad}Transform:\n{pad}  translation:\n{pad}    - {:?}\n{pad}    - {:?}\n{pad}    - {:?}\n",
        at[0], at[1], at[2]
    )
}

fn collider(half: [f32; 3], body: &str) -> String {
    format!(
        "      Collider:\n        shape:\n          box:\n            half_extents:\n              - {:?}\n              - {:?}\n              - {:?}\n        body:{body}\n",
        half[0], half[1], half[2]
    )
}

fn scene(scale: Scale) -> String {
    let mut out = String::from("format: 1\nseed: 1\nphysics:\n  space: free\nentities:\n");
    let mut add = |name: String, components: String| {
        out += &format!("  - name: {name}\n    components:\n{components}");
    };
    add(
        "floor".into(),
        translation("      ", [0.0, -0.5, 0.0]) + &collider([400.0, 0.5, 400.0], " static"),
    );
    let side = scale.bodies.isqrt().max(1);
    for i in 0..scale.bodies {
        let at = [(i % side) as f32 * 1.5, 0.5, (i / side) as f32 * 1.5];
        add(
            format!("body{i}"),
            translation("      ", at)
                + &collider([0.5, 0.5, 0.5], "\n          rigid:\n            mass: 1.0"),
        );
    }
    let side = scale.statics.isqrt().max(1);
    for i in 0..scale.statics {
        let at = [(i % side) as f32 * 2.0, 1.0, 60.0 + (i / side) as f32 * 2.0];
        add(
            format!("wall{i}"),
            translation("      ", at) + &collider([0.75, 1.0, 0.75], " static"),
        );
    }
    let side = scale.props.isqrt().max(1);
    for i in 0..scale.props {
        let at = [
            (i % side) as f32 * 1.0,
            0.0,
            -60.0 - (i / side) as f32 * 1.0,
        ];
        add(
            format!("prop{i}"),
            translation("      ", at)
                + &format!(
                    "      Guard:\n        hp: 100.0\n        state: {}\n        timer: {i}\n        target:\n          - 1.0\n          - 0.0\n          - {:?}\n",
                    i % 4,
                    i as f32
                ),
        );
    }
    out
}

fn guard_tick(world: &mut World, _ctx: &SystemCtx) {
    for (g,) in world.query::<(&mut Guard,)>() {
        g.timer = g.timer.wrapping_add(1);
    }
}

/// Captures at offset `CAPTURE_AT` of every `CYCLE` frames and restores at `RESTORE_AT`.
fn director(world: &mut World, ctx: &SystemCtx) {
    let checkpoints = world.resource_mut::<Checkpoints>().unwrap();
    match ctx.frame.as_u64() % CYCLE {
        CAPTURE_AT => checkpoints.capture("cp"),
        RESTORE_AT => checkpoints.restore("cp"),
        _ => {}
    }
}

fn engine(scale: Scale) -> Engine {
    let mut engine = Engine::builder()
        .checkpoints(CheckpointConfig::default())
        .components(|r| r.register::<Guard>("Guard"))
        .system("guard_tick", guard_tick)
        .system("director", director)
        .build()
        .expect("boots");
    engine.load_inline_scene(&scene(scale)).expect("loads");
    engine
}

/// One cycle's frame durations and the hashes right after the capture and restore frames.
struct Cycle {
    frames: [Duration; CYCLE as usize],
    captured: u64,
    restored: u64,
}

fn run_cycle(engine: &mut Engine) -> Cycle {
    let mut frames = [Duration::ZERO; CYCLE as usize];
    let (mut captured, mut restored) = (0, 0);
    for (offset, slot) in frames.iter_mut().enumerate() {
        let start = now();
        engine.advance_live();
        *slot = lap(start);
        match offset as u64 {
            CAPTURE_AT => {
                assert!(
                    engine
                        .world()
                        .resource::<Checkpoints>()
                        .unwrap()
                        .slot("cp")
                        .is_some(),
                    "the slot exists after the capture frame"
                );
                captured = engine.world_hash().unwrap().as_u64();
            }
            RESTORE_AT => {
                assert!(
                    matches!(
                        engine.world().resource::<Checkpoints>().unwrap().last_restore(),
                        Some(RestoreOutcome::Restored { name, .. }) if name == "cp"
                    ),
                    "the restore ran"
                );
                restored = engine.world_hash().unwrap().as_u64();
            }
            _ => {}
        }
    }
    Cycle {
        frames,
        captured,
        restored,
    }
}

/// Every cycle's hash pair after `cycles` cycles at `threads`.
fn hash_trace(scale: Scale, threads: usize, cycles: usize) -> Vec<(u64, u64)> {
    par::with_threads(threads, || {
        let mut engine = engine(scale);
        (0..cycles)
            .map(|_| {
                let c = run_cycle(&mut engine);
                assert_eq!(c.captured, c.restored, "a restore changed the world");
                (c.captured, c.restored)
            })
            .collect()
    })
}

#[test]
fn checkpoint_bench_smoke() {
    let one = hash_trace(SMALL, 1, 3);
    let eight = hash_trace(SMALL, THREADS, 3);
    assert_eq!(one, eight, "1 and {THREADS} threads trace the same hashes");
}

/// The gate's own schema: the request's fixed targets, read from `budgets/checkpoints-<os>.toml`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointBudget {
    threads: usize,
    capture_ms: f64,
    restore_ms: f64,
}

#[test]
#[ignore = "release-mode benchmark; run explicitly (see the module docs)"]
fn checkpoint_bench() {
    let (capture, restore, plain, entities) = par::with_threads(THREADS, || {
        let mut engine = engine(FULL);
        let entities = engine.world().entities().count();
        for _ in 0..WARMUP_CYCLES {
            run_cycle(&mut engine);
        }
        let (mut capture, mut restore, mut plain) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..CYCLES {
            let c = run_cycle(&mut engine);
            assert_eq!(c.captured, c.restored, "a restore changed the world");
            let at = |i: u64| c.frames[i as usize];
            capture.push(at(CAPTURE_AT).saturating_sub(at(CAPTURE_AT - 1)));
            restore.push(at(RESTORE_AT).saturating_sub(at(RESTORE_AT - 1)));
            plain.push(at(CAPTURE_AT - 1));
        }
        (capture, restore, plain, entities)
    });
    println!(
        "checkpoint bench: {entities} entities ({BODIES} rigid, {STATICS} static, {PROPS} props), {THREADS} threads, {CYCLES} cycles (indicative unless run on a pinned box)"
    );
    println!(
        "  capture: median {:.3} ms, max {:.3} ms, n {}",
        median_ms(&capture),
        max_ms(&capture),
        capture.len()
    );
    println!(
        "  restore: median {:.3} ms, max {:.3} ms, n {}",
        median_ms(&restore),
        max_ms(&restore),
        restore.len()
    );
    println!("  plain frame: median {:.3} ms", median_ms(&plain));

    let Ok(platform) = ludion_testkit::platform_dir() else {
        println!("no pinned platform here; the budget check is skipped");
        return;
    };
    let file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../budgets")
        .join(format!("checkpoints-{platform}.toml"));
    let Ok(text) = std::fs::read_to_string(&file) else {
        println!("no {} (owed: the pinned run measures it)", file.display());
        return;
    };
    let budget: CheckpointBudget = toml::from_str(&text).expect("the budget file parses");
    assert_eq!(
        budget.threads, THREADS,
        "the budget was calibrated at another thread count"
    );
    let (capture, restore) = (median_ms(&capture), median_ms(&restore));
    assert!(
        capture <= budget.capture_ms,
        "capture median {capture:.3} ms is over {} ms ({})",
        budget.capture_ms,
        file.display()
    );
    assert!(
        restore <= budget.restore_ms,
        "restore median {restore:.3} ms is over {} ms ({})",
        budget.restore_ms,
        file.display()
    );
}
