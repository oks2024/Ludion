//! The physics settle bench: a town of resting, jointed bodies, measured on its first
//! tick with and without the load-time prime. A scene load builds the physics world and runs its first
//! collision detection at commit; tick 1 adopts that work, so the one-time settle cost moves from the
//! first frame to the load. The bench compares the two arms at 1,300 and 2,000 rigid bodies and gates the
//! primed 2,000-body arm: tick-1 `physics_step` under 12 ms, the whole tick-1 frame under 16.667 ms.
//!
//! The scene is `physics_prime.rs`'s, scaled: 40-body walls (32 bricks glued by fixed joints, 8 two-part
//! compound tiles resting on them, so a fifth of the bodies are compounds) spread over a static floor,
//! with 0.6 joints per body (an external game's 792 / 1,295) taken evenly from the adjacent-brick candidates. Every
//! body rests exactly touching its neighbours, so tick 1 has live contacts.
//!
//! Ignored by default so CI stays fast; run it explicitly, in release, with rapier's profiler timers on
//! (without the feature the stage table is skipped):
//!
//! ```text
//! cargo test -p ludion-engine --release --features physics-profiler --test it -- settle_bench --ignored --nocapture
//! ```
//!
//! **Arms.** The primed arm is an ordinary load; the cold arm removes the `PrimedPhysics` resource right
//! after the load, so tick 1 builds and detects as before. Each arm runs 3 fresh engines at 8 threads,
//! `profile_engine(.., 60, 0)`: tick 1 is the first sample of each column, and the printed tick-1 numbers
//! are the median of the 3. Steady medians come from the last 100 frames of a 300-frame primed run.
//!
//! **Prime cost.** Per N, five fresh loads; each drops the prime the load made and times a direct
//! `ludion_physics::prime` call on the loaded world, which is exactly what commit now pays on top of the
//! load. It is printed in ms and as a share of the load's wall time (which includes that prime).
//!
//! **Budget file.** The bench checks `budgets/pinned-<box>-physics-settle.toml` when it exists
//! (`apple-m4` on macOS, `rtx3090` on Windows) against the last primed 2,000-body report.
//!
//! Determinism: both arms must produce the same `WorldHash` on every tick, at 1 and at 8 threads.

use std::path::Path;
use std::time::Instant;

use ludion_core::par;
use ludion_core::time::{FrameIndex, TickRate};
use ludion_engine::{
    Engine, ProfileReport, Transform, TransformHierarchy, check_budget, profile_engine,
};
use ludion_physics::{PhysicsSettings, PhysicsSpace, PhysicsWorld, PrimedPhysics, STEP, WRITE};

use crate::bench_rows::platform_budget;

const THREADS: usize = 8;
const MEASURED: u64 = 60;
const STEADY_WINDOW: u64 = 300;
const STEADY_TAIL: usize = 100;
const REPEATS: usize = 3;
const PRIME_REPEATS: usize = 5;
const FRAME_BUDGET_MS: f64 = 16.667;
const STEP_TARGET_MS: f64 = 12.0;
/// Bodies per wall: 32 bricks and 8 two-part compound tiles.
const WALL_BODIES: usize = 40;
const COLS: usize = 8;
const ROWS: usize = 4;
/// An external game's joint ratio, 792 joints over 1,295 bodies, as a fraction of the bodies.
const JOINTS_PER_BODY: f64 = 0.6;

/// A 3-vector as the block-style list the scene reader requires.
fn list(pad: &str, key: &str, v: [f32; 3]) -> String {
    format!(
        "{pad}{key}:\n{pad}  - {:?}\n{pad}  - {:?}\n{pad}  - {:?}\n",
        v[0], v[1], v[2]
    )
}

fn cuboid(pad: &str, half: [f32; 3]) -> String {
    format!(
        "{pad}box:\n{}",
        list(&format!("{pad}  "), "half_extents", half)
    )
}

/// The scene text for about `bodies` rigid bodies: whole 40-body walls, so the count rounds to a
/// multiple of 40. Returns it with the rigid-body and joint counts it holds.
fn scene(bodies: usize) -> (String, usize, usize) {
    let walls = bodies.div_ceil(WALL_BODIES).max(1);
    let mut entities = String::new();
    let mut count = 0usize;
    let mut add = |name: String, components: String| {
        entities.push_str(&format!("  - name: {name}\n    components:\n{components}"));
        count += 1;
        count - 1
    };
    let floor = format!(
        "      Transform:\n{}      Collider:\n        shape:\n{}        body: static\n",
        list("        ", "translation", [0.0, -0.5, 0.0]),
        cuboid("          ", [200.0, 0.5, 200.0]),
    );
    add("floor".into(), floor);

    let rigid = |at: [f32; 3], shape: String| {
        format!(
            "      Transform:\n{}      Collider:\n        shape:\n{shape}        body:\n          rigid:\n            mass: 1.0\n      Velocity: {{}}\n",
            list("        ", "translation", at),
        )
    };
    let mut candidates: Vec<(usize, usize)> = Vec::new();
    let columns = 7usize;
    for wall in 0..walls {
        let ox = (wall % columns) as f32 * 10.0 - 30.0;
        let oz = (wall / columns) as f32 * 3.0 - 12.0;
        let mut bricks = [[0usize; COLS]; ROWS];
        for (row, line) in bricks.iter_mut().enumerate() {
            for (col, slot) in line.iter_mut().enumerate() {
                let at = [ox + col as f32, 0.25 + row as f32 * 0.5, oz];
                *slot = add(
                    format!("brick-{wall}-{row}-{col}"),
                    rigid(at, cuboid("          ", [0.5, 0.25, 0.25])),
                );
            }
        }
        for row in 0..ROWS {
            for col in 0..COLS {
                if col > 0 {
                    candidates.push((bricks[row][col - 1], bricks[row][col]));
                }
                if row > 0 {
                    candidates.push((bricks[row - 1][col], bricks[row][col]));
                }
            }
        }
        for col in 0..COLS {
            let part = |x: f32| {
                format!(
                    "              - shape:\n{}                translation:\n                  - {x:?}\n                  - 0.0\n                  - 0.0\n",
                    cuboid("                  ", [0.25, 0.05, 0.5])
                )
            };
            let tile = format!(
                "          compound:\n            parts:\n{}{}",
                part(-0.25),
                part(0.25)
            );
            add(
                format!("tile-{wall}-{col}"),
                rigid([ox + col as f32, 2.05, oz], tile),
            );
        }
    }

    // An even pick of `target` joints from the candidates, in candidate order.
    let rigid_bodies = walls * WALL_BODIES;
    let target = (rigid_bodies as f64 * JOINTS_PER_BODY).round() as usize;
    let total = candidates.len();
    let mut joints = 0usize;
    for (at, (a, b)) in candidates.into_iter().enumerate() {
        if (at + 1) * target / total == at * target / total {
            continue;
        }
        let components = format!(
            "      Joint:\n        a:\n          index: {a}\n          generation: 0\n        b:\n          index: {b}\n          generation: 0\n        kind:\n          fixed: {{}}\n{}        collide: false\n",
            list("        ", "anchor", [0.0; 3]),
        );
        add(format!("glue-{at}"), components);
        joints += 1;
    }
    let text = format!(
        "format: 1\nseed: 3\nphysics:\n  space: free\n  solver_iterations: 8\nentities:\n{entities}"
    );
    (text, rigid_bodies, joints)
}

/// A freshly loaded engine, with its prime dropped for the cold arm.
fn loaded(text: &str, primed: bool) -> Engine {
    let mut engine = Engine::builder().build().expect("boots");
    engine.load_inline_scene(text).expect("the scene loads");
    let prime = engine.world_mut().remove_resource::<PrimedPhysics>();
    assert!(prime.is_some(), "a load primes");
    if primed {
        engine.world_mut().insert_resource(prime.unwrap());
    }
    engine
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// One profiled arm: the report of the last repeat, and tick 1's column values (median over the
/// repeats) for the three gated rows.
struct Arm {
    report: ProfileReport,
    write: f64,
    step: f64,
    frame: f64,
}

fn column<'a>(report: &'a ProfileReport, name: &str) -> &'a [f64] {
    let samples = report
        .samples
        .as_ref()
        .expect("offline reports carry samples");
    let at = samples
        .system_names
        .iter()
        .position(|n| n == name)
        .unwrap_or_else(|| panic!("no column `{name}`"));
    &samples.system_ms[at]
}

fn run_arm(text: &str, primed: bool) -> Arm {
    par::with_threads(THREADS, || {
        let (mut writes, mut steps, mut frames) = (Vec::new(), Vec::new(), Vec::new());
        let mut last = None;
        for _ in 0..REPEATS {
            let mut engine = loaded(text, primed);
            let report = profile_engine(&mut engine, "physics-settle".into(), MEASURED, 0)
                .expect("profiles");
            writes.push(column(&report, WRITE)[0]);
            steps.push(column(&report, STEP)[0]);
            frames.push(report.samples.as_ref().unwrap().frame_total_ms[0]);
            last = Some(report);
        }
        Arm {
            report: last.unwrap(),
            write: median(&mut writes),
            step: median(&mut steps),
            frame: median(&mut frames),
        }
    })
}

/// Medians of the last `STEADY_TAIL` frames of a `STEADY_WINDOW`-frame primed run.
fn steady(text: &str) -> (f64, f64, f64, f64) {
    par::with_threads(THREADS, || {
        let mut engine = loaded(text, true);
        let report = profile_engine(&mut engine, "physics-settle".into(), STEADY_WINDOW, 0)
            .expect("profiles");
        let tail = |values: &[f64]| median(&mut values[values.len() - STEADY_TAIL..].to_vec());
        let awake = engine
            .world()
            .resource::<PhysicsWorld>()
            .and_then(PhysicsWorld::step_breakdown)
            .map_or(f64::NAN, |s| s.awake_bodies as f64);
        (
            tail(column(&report, WRITE)),
            tail(column(&report, STEP)),
            tail(&report.samples.as_ref().unwrap().frame_total_ms),
            awake,
        )
    })
}

/// The prime's cost at load (wall clock: a bench measures, nothing here is sim): `(prime ms, load wall ms)`, medians over `PRIME_REPEATS` fresh loads.
#[allow(clippy::disallowed_methods)]
fn prime_cost(text: &str) -> (f64, f64) {
    par::with_threads(THREADS, || {
        let (mut primes, mut loads) = (Vec::new(), Vec::new());
        for _ in 0..PRIME_REPEATS {
            let mut engine = Engine::builder().build().expect("boots");
            let start = Instant::now();
            engine.load_inline_scene(text).expect("the scene loads");
            loads.push(start.elapsed().as_secs_f64() * 1000.0);
            drop(engine.world_mut().remove_resource::<PrimedPhysics>());
            let mut settings = PhysicsSettings::new(PhysicsSpace::Free);
            settings.solver_iterations = 8;
            let start = Instant::now();
            ludion_physics::prime::<Transform, TransformHierarchy>(
                engine.world_mut(),
                settings,
                TickRate::DEFAULT,
                FrameIndex(0),
            );
            primes.push(start.elapsed().as_secs_f64() * 1000.0);
            assert!(
                engine.world().resource::<PrimedPhysics>().is_some(),
                "the direct prime built a world"
            );
        }
        (median(&mut primes), median(&mut loads))
    })
}

/// Advances `ticks` and returns every tick's hash, plus whether tick 1 adopted the prime.
fn hashes(text: &str, primed: bool, threads: usize, ticks: usize) -> (Vec<u64>, bool) {
    par::with_threads(threads, || {
        let mut engine = loaded(text, primed);
        let mut out = Vec::new();
        let mut adopted = false;
        for tick in 0..ticks {
            engine.advance_live();
            if tick == 0 {
                adopted = engine
                    .world()
                    .resource::<PhysicsWorld>()
                    .expect("physics runs")
                    .last_step_primed();
            }
            out.push(engine.world_hash().unwrap().as_u64());
        }
        (out, adopted)
    })
}

/// Tick 1 and tick 2 of one arm, as rapier's stage timers saw them.
fn tick_stages(text: &str, primed: bool) -> Vec<ludion_physics::StepBreakdown> {
    par::with_threads(THREADS, || {
        let mut engine = loaded(text, primed);
        (0..2)
            .filter_map(|_| {
                engine.advance_live();
                engine
                    .world()
                    .resource::<PhysicsWorld>()
                    .and_then(PhysicsWorld::step_breakdown)
            })
            .collect()
    })
}

#[test]
fn settle_scene_builds_and_primes() {
    let (text, bodies, joints) = scene(80);
    assert_eq!(bodies, 80);
    assert_eq!(joints, 48);
    let (primed, adopted) = hashes(&text, true, 1, 6);
    assert!(adopted, "tick 1 adopts the prime");
    let (cold, cold_adopted) = hashes(&text, false, 1, 6);
    assert!(!cold_adopted);
    assert_eq!(primed, cold, "a prime changed what the scene computes");
}

#[test]
#[ignore = "release-mode benchmark; run explicitly (see the module docs)"]
fn settle_bench() {
    println!(
        "settle bench: {THREADS} threads, {REPEATS} fresh engines per arm (tick-1 = median), \
         {MEASURED} measured frames, steady = median of the last {STEADY_TAIL} of {STEADY_WINDOW} \
         (indicative unless run on a pinned box)"
    );
    println!(
        "\n{:>6} {:>6} {:<7} | {:>9} {:>9} {:>9} | {:>9} {:>9} {:>9} | {:>9}",
        "bodies",
        "joints",
        "arm",
        "t1 step",
        "t1 write",
        "t1 frame",
        "st step",
        "st write",
        "st frame",
        "awake"
    );
    let mut gated = None;
    let mut missed = Vec::new();
    for target in [1300, 2000] {
        let (text, bodies, joints) = scene(target);

        for threads in [1, THREADS] {
            let (primed, adopted) = hashes(&text, true, threads, 60);
            let (cold, cold_adopted) = hashes(&text, false, threads, 60);
            assert!(
                adopted && !cold_adopted,
                "tick 1 primed only in the primed arm"
            );
            assert_eq!(
                primed, cold,
                "primed and cold WorldHash differ at {threads} threads ({bodies} bodies)"
            );
        }
        let (one, _) = hashes(&text, true, 1, 60);
        let (eight, _) = hashes(&text, true, THREADS, 60);
        assert_eq!(
            one, eight,
            "WorldHash differs between 1 and {THREADS} threads"
        );

        let (st_write, st_step, st_frame, awake) = steady(&text);
        for (label, primed) in [("primed", true), ("cold", false)] {
            let arm = run_arm(&text, primed);
            println!(
                "{bodies:>6} {joints:>6} {label:<7} | {:>9.3} {:>9.3} {:>9.3} | {st_step:>9.3} {st_write:>9.3} {st_frame:>9.3} | {awake:>9.0}",
                arm.step, arm.write, arm.frame
            );
            if primed && bodies >= 2000 {
                if arm.step >= STEP_TARGET_MS {
                    missed.push(format!(
                        "tick-1 physics_step {:.3} ms >= {STEP_TARGET_MS}",
                        arm.step
                    ));
                }
                if arm.frame >= FRAME_BUDGET_MS {
                    missed.push(format!(
                        "tick-1 frame_total {:.3} ms >= {FRAME_BUDGET_MS}",
                        arm.frame
                    ));
                }
                gated = Some(arm.report);
            }
        }

        let (prime_ms, load_ms) = prime_cost(&text);
        println!(
            "       prime at load, {bodies} bodies: {prime_ms:.2} ms of a {load_ms:.2} ms load ({:.1}%)",
            100.0 * prime_ms / load_ms
        );

        let stages = [(true, "primed"), (false, "cold")]
            .map(|(primed, label)| (label, tick_stages(&text, primed)));
        if stages.iter().all(|(_, s)| s.is_empty()) {
            println!("build with --features physics-profiler for the stage breakdown");
        } else {
            println!(
                "       {:<7} {:>5} | {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
                "stage ms",
                "tick",
                "step",
                "usr_chg",
                "broad",
                "narrow",
                "islands",
                "solver",
                "contacts"
            );
            for (label, ticks) in &stages {
                for (i, s) in ticks.iter().enumerate() {
                    println!(
                        "       {label:<8} {:>5} | {:>8.3} {:>8.3} {:>8.3} {:>8.3} {:>8.3} {:>8.3} {:>8}",
                        i + 1,
                        s.step_ms,
                        s.user_changes_ms,
                        s.broad_phase_ms,
                        s.narrow_phase_ms,
                        s.island_construction_ms,
                        s.solver_ms,
                        s.contact_pairs
                    );
                }
            }
        }
    }
    let report = gated.expect("the 2,000-body primed arm ran");
    println!(
        "\n2,000-body primed report ({MEASURED} frames): frame_total median {:.3} ms, worst {:.3} ms",
        report.sim.frame_total_median_ms,
        report
            .sim
            .frame_total_tail
            .as_ref()
            .map_or(0.0, |t| t.max_ms)
    );
    for system in report
        .sim
        .systems
        .iter()
        .filter(|s| s.name.starts_with("physics_"))
    {
        println!(
            "  {:<16} median {:.3} ms, worst {:.3} ms",
            system.name,
            system.median_ms,
            system.tail.as_ref().map_or(0.0, |t| t.max_ms)
        );
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../budgets");
    match ludion_testkit::platform_dir() {
        Ok(platform) => match platform_budget(&dir, platform, "physics-settle") {
            Some(budget) => {
                assert_eq!(
                    report.sim.threads, budget.threads,
                    "the budget was calibrated at another thread count"
                );
                let over = check_budget(&report, &budget);
                assert!(over.is_empty(), "over the physics-settle budget: {over:?}");
            }
            None => {
                println!("no pinned-<box>-physics-settle.toml (owed: the pinned run writes it)")
            }
        },
        Err(_) => println!("no pinned platform here; the budget check is skipped"),
    }
    assert!(missed.is_empty(), "settle targets missed: {missed:?}");
}
