//! The physics-horde bench: `examples/gallery/physics-horde.yaml` under `horde_driver` (500 walking
//! `CharacterMotor`s, 120 jointed `SkinBone` ragdolls, 5,000 pool projectiles), measured to answer
//! whether the solver, the narrow phase or the motor casts dominate. It reports the per-system
//! rows at 1 and 8 threads, rapier's step stages, the contact-pair census by body kind, and the
//! awake part count.
//!
//! A converged arm (`motor_converge`) walks every motor to one point so the crowd piles up, to
//! measure what overlapping motor queries cost against the dispersed walk.
//!
//! Ignored by default so CI stays fast; run it explicitly, in release, with rapier's profiler
//! timers on (without the feature the stage table is skipped):
//!
//! ```text
//! cargo test -p ludion-engine --release --features physics-profiler --test it -- horde_bench --ignored --nocapture
//! ```
//!
//! **Budget file.** The bench checks `budgets/pinned-<box>-physics-horde.toml` when it exists
//! (`apple-m4` on macOS, `rtx3090` on Windows) against the 8-thread report, and skips the check
//! when it does not. It carries no in-file ceilings: it diagnoses, and `ragdoll_bench` holds the
//! step ceilings.
//!
//! Determinism: every system is a sim system driven by the tick and sorted ids; the stage timers
//! and the census are read-only measurement. The `WorldHash` at 1 and 8 threads is asserted equal.

use std::path::Path;

use ludion_core::par;
use ludion_engine::{Engine, ProfileReport, SkinBone, Transform, check_budget, profile_engine};
use ludion_physics::{
    CharacterMotor, ContactPairCensus, MIRROR, PairCount, PhysicsWorld, STEP, StepBreakdown, WRITE,
};

use crate::bench_rows::{Row, platform_budget, row};
use crate::horde_driver::{
    CONVERGE_POINT, MOTORS, RAGDOLLS, RETHROW_EVERY, SHOTS, build, build_converged,
};

/// Ticks before measuring: past the tick-0 switch and the first landing.
const WARMUP: u64 = 120;
/// Ticks the converged arm walks before measuring: the ~26 m motor half at 0.1 m a tick, plus slack.
const CONVERGE_WARMUP: u64 = 360;
/// Eight re-throw rounds.
const MEASURED: u64 = 240;
const THREADS: usize = 8;
const SYSTEMS: [&str; 8] = [
    WRITE,
    STEP,
    MIRROR,
    "motor_walk",
    "horde_ragdolls",
    "spatial3d_rebuild",
    "horde_shots",
    "projectiles_step",
];

type StageField = fn(&StepBreakdown) -> f64;
type KindField = fn(&ContactPairCensus) -> PairCount;

struct Arm {
    report: Option<ProfileReport>,
    rows: Vec<(&'static str, Row)>,
    stages: Vec<StepBreakdown>,
    census: Vec<ContactPairCensus>,
    min_awake: usize,
    mean_awake: f64,
    hash: u64,
}

/// Mean and p99 (`sorted[ceil(0.99 n) - 1]`) of `values`.
fn stats(values: &[f64]) -> (f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0);
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let p99 = sorted[(0.99 * sorted.len() as f64).ceil() as usize - 1];
    (values.iter().sum::<f64>() / values.len() as f64, p99)
}

/// `profile` adds the `profile_engine` pass, which needs a GPU device; the smoke test skips it.
fn run_arm(threads: usize, warmup: u64, measured: u64, profile: bool, converged: bool) -> Arm {
    par::with_threads(threads, || {
        let mut engine = if converged {
            build_converged()
        } else {
            build()
        };
        for _ in 0..warmup {
            engine.advance_live();
        }
        if converged && warmup >= CONVERGE_WARMUP {
            let median = median_motor_distance(&mut engine);
            assert!(
                median < 3.0,
                "the converged arm measures a dispersed crowd: median motor distance {median:.2} m"
            );
        }
        let report = profile.then(|| {
            profile_engine(&mut engine, "physics-horde".into(), measured, 0).expect("profiles")
        });
        let rows = report
            .iter()
            .flat_map(|report| SYSTEMS.iter().map(|&n| (n, row(report, n))))
            .collect();
        let (mut stages, mut census, mut awake_counts) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..measured {
            engine.advance_live();
            let parts: Vec<_> = engine
                .world_mut()
                .query_ids::<(&SkinBone,)>()
                .map(|(id, _)| id)
                .collect();
            let physics = engine.world().resource::<PhysicsWorld>().expect("physics");
            stages.extend(physics.step_breakdown());
            census.push(physics.contact_pair_census());
            let awake = parts
                .iter()
                .filter(|part| physics.is_sleeping(**part) == Some(false))
                .count();
            assert!(awake > 0, "every ragdoll part is asleep");
            awake_counts.push(awake);
        }
        Arm {
            report,
            rows,
            stages,
            census,
            min_awake: awake_counts.iter().copied().min().unwrap_or(0),
            mean_awake: awake_counts.iter().sum::<usize>() as f64
                / awake_counts.len().max(1) as f64,
            hash: engine.world_hash().unwrap().as_u64(),
        }
    })
}

/// Median horizontal distance of the motors to the convergence point.
fn median_motor_distance(engine: &mut Engine) -> f32 {
    let mut distances: Vec<f32> = engine
        .world_mut()
        .query_ids::<(&CharacterMotor, &Transform)>()
        .map(|(_, (_, at))| {
            let dx = at.translation[0] - CONVERGE_POINT[0];
            let dz = at.translation[2] - CONVERGE_POINT[2];
            ludion_core::math::sqrt(dx * dx + dz * dz)
        })
        .collect();
    distances.sort_by(f32::total_cmp);
    distances[distances.len() / 2]
}

fn motor_row(arm: &Arm) -> &Row {
    &arm.rows
        .iter()
        .find(|(n, _)| *n == "motor_walk")
        .expect("the motor row")
        .1
}

fn mean_pair(
    census: &[ContactPairCensus],
    pick: fn(&ContactPairCensus) -> PairCount,
) -> (f64, f64) {
    let n = census.len().max(1) as f64;
    (
        census.iter().map(|c| pick(c).tracked).sum::<usize>() as f64 / n,
        census.iter().map(|c| pick(c).touching).sum::<usize>() as f64 / n,
    )
}

#[test]
fn horde_bench_runs() {
    let arm = run_arm(1, 2, 2, false, false);
    assert_eq!(arm.census.len(), 2);
    assert_eq!(run_arm(1, 2, 2, false, true).census.len(), 2);
    let tracked: usize = arm
        .census
        .iter()
        .map(|c| {
            [
                c.static_static,
                c.kinematic_static,
                c.kinematic_kinematic,
                c.rigid_static,
                c.rigid_kinematic,
                c.rigid_rigid,
            ]
            .iter()
            .map(|p| p.tracked)
            .sum::<usize>()
        })
        .sum();
    assert!(tracked > 0, "the horde has contact pairs");
}

#[test]
#[ignore = "release-mode benchmark; run explicitly (see the module docs)"]
fn horde_bench() {
    let one = run_arm(1, WARMUP, MEASURED, true, false);
    let eight = run_arm(THREADS, WARMUP, MEASURED, true, false);
    assert_eq!(
        one.hash, eight.hash,
        "WorldHash differs between 1 and {THREADS} threads"
    );
    let converged_one = run_arm(1, CONVERGE_WARMUP, MEASURED, true, true);
    let converged_eight = run_arm(THREADS, CONVERGE_WARMUP, MEASURED, true, true);
    assert_eq!(
        converged_one.hash, converged_eight.hash,
        "the converged WorldHash differs between 1 and {THREADS} threads"
    );
    println!(
        "horde bench: {MOTORS} motors, {RAGDOLLS} ragdolls, {SHOTS} shots, re-throw every \
         {RETHROW_EVERY} ticks, {MEASURED} measured ticks after {WARMUP} warmup \
         (indicative unless run on a pinned box)"
    );

    println!("\n(a) systems, ms per tick");
    println!(
        "{:<20} | {:>8} {:>8} | {:>8} {:>8} | {:>7}",
        "system", "1t mean", "p99", "8t mean", "p99", "speedup"
    );
    for ((name, a), (_, b)) in one.rows.iter().zip(&eight.rows) {
        println!(
            "{name:<20} | {:>8.3} {:>8.3} | {:>8.3} {:>8.3} | {:>6.2}x",
            a.mean,
            a.p99,
            b.mean,
            b.p99,
            a.mean / b.mean.max(f64::MIN_POSITIVE)
        );
    }
    let motor = |arm: &Arm| {
        arm.rows
            .iter()
            .find(|(n, _)| *n == "motor_walk")
            .unwrap()
            .1
            .mean
    };
    println!(
        "motor_walk per motor (us): 1t {:.2}, 8t {:.2}",
        motor(&one) * 1000.0 / MOTORS as f64,
        motor(&eight) * 1000.0 / MOTORS as f64
    );

    println!("\n(b) step stages at {THREADS}t, ms");
    if eight.stages.is_empty() {
        println!("build with --features physics-profiler for the stage breakdown");
    } else {
        let fields: [(&str, StageField); 15] = [
            ("step", |s| s.step_ms),
            ("user_changes", |s| s.user_changes_ms),
            ("collision_detection", |s| s.collision_detection_ms),
            ("broad_phase", |s| s.broad_phase_ms),
            ("narrow_phase", |s| s.narrow_phase_ms),
            ("final_broad_phase", |s| s.final_broad_phase_ms),
            ("island_construction", |s| s.island_construction_ms),
            ("constraints_collection", |s| s.constraints_collection_ms),
            ("update", |s| s.update_ms),
            ("solver", |s| s.solver_ms),
            ("velocity_assembly", |s| s.velocity_assembly_ms),
            ("velocity_resolution", |s| s.velocity_resolution_ms),
            ("velocity_update", |s| s.velocity_update_ms),
            ("velocity_writeback", |s| s.velocity_writeback_ms),
            ("ccd", |s| s.ccd_ms),
        ];
        println!("{:<24} | {:>8} {:>8}", "stage", "mean", "p99");
        for (name, get) in fields {
            let values: Vec<f64> = eight.stages.iter().map(get).collect();
            let (mean, p99) = stats(&values);
            println!("{name:<24} | {mean:>8.3} {p99:>8.3}");
        }
        let n = eight.stages.len() as f64;
        let avg = |get: fn(&StepBreakdown) -> usize| {
            eight.stages.iter().map(get).sum::<usize>() as f64 / n
        };
        println!(
            "mean bodies {:.0}, awake bodies {:.0}, contact_pairs {:.0}, impulse joints {:.0}",
            avg(|s| s.bodies),
            avg(|s| s.awake_bodies),
            avg(|s| s.contact_pairs),
            avg(|s| s.impulse_joints)
        );
    }

    println!("\n(c) pair census at {THREADS}t, mean per tick");
    println!("{:<20} | {:>9} {:>9}", "kind", "tracked", "touching");
    let kinds: [(&str, KindField); 6] = [
        ("static-static", |c| c.static_static),
        ("kinematic-static", |c| c.kinematic_static),
        ("kinematic-kinematic", |c| c.kinematic_kinematic),
        ("rigid-static", |c| c.rigid_static),
        ("rigid-kinematic", |c| c.rigid_kinematic),
        ("rigid-rigid", |c| c.rigid_rigid),
    ];
    for (name, pick) in kinds {
        let (tracked, touching) = mean_pair(&eight.census, pick);
        println!("{name:<20} | {tracked:>9.1} {touching:>9.1}");
    }

    println!(
        "\n(d) motor_walk, dispersed vs converged (after {CONVERGE_WARMUP} warmup ticks), ms per tick"
    );
    println!(
        "{:<10} | {:>8} {:>8} | {:>8} {:>8} | {:>9} {:>9}",
        "arm", "1t mean", "p99", "8t mean", "p99", "1t us/mtr", "8t us/mtr"
    );
    for (name, a, b) in [
        ("dispersed", &one, &eight),
        ("converged", &converged_one, &converged_eight),
    ] {
        let (ra, rb) = (motor_row(a), motor_row(b));
        println!(
            "{name:<10} | {:>8.3} {:>8.3} | {:>8.3} {:>8.3} | {:>9.2} {:>9.2}",
            ra.mean,
            ra.p99,
            rb.mean,
            rb.p99,
            ra.mean * 1000.0 / MOTORS as f64,
            rb.mean * 1000.0 / MOTORS as f64
        );
    }

    println!(
        "\n(e) awake ragdoll parts at {THREADS}t: min {}, mean {:.1}",
        eight.min_awake, eight.mean_awake
    );

    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../budgets");
    let budget = match ludion_testkit::platform_dir() {
        Ok(platform) => platform_budget(&dir, platform, "physics-horde"),
        Err(_) => {
            println!("no pinned platform here; the budget check is skipped");
            return;
        }
    };
    match budget {
        Some(budget) => {
            assert_eq!(
                eight.report.as_ref().unwrap().sim.threads,
                budget.threads,
                "the budget was calibrated at another thread count"
            );
            let over = check_budget(eight.report.as_ref().unwrap(), &budget);
            assert!(over.is_empty(), "over the physics-horde budget: {over:?}");
        }
        None => println!("no pinned-<box>-physics-horde.toml (owed: the pinned run writes it)"),
    }
}
