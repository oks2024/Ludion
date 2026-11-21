//! The assembly animation bench: 32 grounded voxel humanoids, each on a 1D locomotion blend with two
//! layers (an override `carry` on `upper_body` and an additive `lean`), a look-at and a hand IK goal,
//! with ground feet over a field of steps and ramps — measured as the median of the `extract`
//! sequential timing, which is where assembly poses are composed (`Plan/animation-layers.md`
//! decisions 11 and 54).
//!
//! Ignored by default so CI stays fast; run it explicitly, in release, on a quiet box:
//!
//! ```text
//! cargo test -p ludion-engine --release --test it -- assembly_anim_bench --ignored --nocapture
//! ```
//!
//! Five arms pose the same 32 placements, so each row's delta over `empty` is the cost of one more
//! feature: `empty` (floor, ground features and camera), `clip` (`walk` only), `layers` (the blend and
//! both layers), `ik` (`layers` plus `IkTargets`) and `full` (the grounded assembly, ground feet on).
//! Each arm prints the extract median, the p99 and the delta over `empty` at 1 and 8 threads.
//!
//! **Gates.** The `full` arm at one thread stays under `EXTRACT_BUDGET_MS`, the owner-accepted budget.
//! A debug build skips the wall-clock asserts. The bench also checks `budgets/pinned-<box>-assembly-anim.toml`
//! when it exists (`apple-m4` on macOS, `rtx3090` on Windows) and skips the check when it does not.
//! Calibrate it from the `full` arm: a `threads = 1` line above any table and `extract_ms` at about
//! 2x the pinned median, never above the accepted budget (`dev-docs/budget-files.md`).
//!
//! Determinism: extract never feeds the sim, so the `full` arm's `WorldHash` after the measured window
//! is asserted equal at 1 and 8 threads. Positions, yaws and clip starts derive from indices.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use ludion_core::math::glam::Mat4;
use ludion_core::par;
use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{
    Engine, ProfileReport, QualityParams, check_budget, extract_at, profile_engine,
};

use crate::bench_rows::{platform_budget, sequential};

const FIGURES: usize = 32;
const COLUMNS: usize = 4;
const SPACING: f32 = 1.2;
const PARTS: usize = 10;
const LOWER_LEG_L: usize = 8;
const LOWER_LEG_R: usize = 9;
const MEASURED: u64 = 120;
const WARMUP: u64 = 10;
const THREADS: usize = 8;
/// Decision 11's extract budget for 32 realistic characters, accepted by the owner on 2026-11-19.
const EXTRACT_BUDGET_MS: f64 = 0.5;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Arm {
    Empty,
    Clip,
    Layers,
    Ik,
    Full,
}

const ARMS: [(Arm, &str); 5] = [
    (Arm::Empty, "empty"),
    (Arm::Clip, "clip"),
    (Arm::Layers, "layers"),
    (Arm::Ik, "ik"),
    (Arm::Full, "full"),
];

fn gallery_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

/// A static box collider entity with no mesh: `centre` and `half` in world metres, rotated about X by
/// `pitch_deg`.
fn block(name: &str, centre: [f32; 3], half: [f32; 3], pitch_deg: f32) -> String {
    format!(
        "  - name: {name}
    components:
      Transform:
        translation:
          - {:?}
          - {:?}
          - {:?}
        rotation:
          - {pitch_deg:?}
          - 0.0
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - {:?}
              - {:?}
              - {:?}
        body: static
",
        centre[0], centre[1], centre[2], half[0], half[1], half[2]
    )
}

/// The figure grid: four columns, eight rows. Returns the world position of figure `i`.
fn place(i: usize) -> [f32; 3] {
    let col = (i % COLUMNS) as f32 - (COLUMNS as f32 - 1.0) / 2.0;
    [col * SPACING, 0.0, (i / COLUMNS) as f32 * SPACING]
}

/// One ground feature per figure row: a step (one or two voxels tall) whose edge runs through the
/// row's figures, or a ramp pitched 15 degrees about X through it, so both feet stand over a changing
/// surface.
fn features() -> String {
    let rows = FIGURES / COLUMNS;
    let half_x = COLUMNS as f32 * SPACING / 2.0;
    let voxel = 0.125;
    let mut out = String::new();
    for row in 0..rows {
        let z = row as f32 * SPACING;
        if row % 2 == 0 {
            let top = voxel * (1 + (row / 2) % 2) as f32;
            out += &block(
                &format!("step{row}"),
                [0.0, top - 0.125, z + 0.5],
                [half_x, 0.125, 0.5],
                0.0,
            );
        } else {
            out += &block(
                &format!("ramp{row}"),
                [0.0, -0.1, z],
                [half_x, 0.1, 0.6],
                15.0,
            );
        }
    }
    out
}

fn figure(i: usize, arm: Arm) -> String {
    let [x, y, z] = place(i);
    let yaw = (i * 37 % 360) as f32;
    let assembly = if arm == Arm::Full {
        "voxels/humanoid-grounded.assembly.yaml"
    } else {
        "voxels/humanoid.assembly.yaml"
    };
    let mut out = format!(
        "  - name: figure{i}
    components:
      Transform:
        translation:
          - {x:?}
          - {y:?}
          - {z:?}
        rotation:
          - 0.0
          - {yaw:?}
          - 0.0
        scale:
          - 0.125
          - 0.125
          - 0.125
      Mesh:
        shape:
          assembly: {assembly}
"
    );
    if arm == Arm::Clip {
        let _ = writeln!(
            out,
            "      Animation:\n        clip: walk\n        started: {i}"
        );
        return out;
    }
    let speed = 0.5 + (i % 8) as f32 * 0.6;
    let _ = write!(
        out,
        "      Animation:
        clip: locomotion
        params:
          - {speed:?}
          - 0.0
        started: {i}
      AnimationLayers:
        layers:
          - name: carry
            clip: carry
            mask: upper_body
            mode: override
            weight: 1.0
          - name: lean
            clip: lean
            mode: additive
            weight: 0.5
"
    );
    if matches!(arm, Arm::Ik | Arm::Full) {
        let _ = write!(
            out,
            "      IkTargets:
        targets:
          look:
            direction:
              yaw_deg: {:?}
          hand_r:
            point:
              - {:?}
              - 1.0
              - {:?}
            weight: 1.0
",
            (i * 53 % 120) as f32 - 60.0,
            x + 0.2,
            z + 0.3
        );
    }
    out
}

/// The bench scene: a camera, the floor, the ground features (unless `ground` is off), and the arm's
/// figures.
fn bench_scene(arm: Arm, ground: bool) -> String {
    let mut out = String::from(
        "format: 1
seed: 1
physics:
  space: free
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 5.0
          - 14.0
      Camera:
        projection:
          type: perspective
          fov_y_deg: 45.0
",
    );
    out += &block("floor", [0.0, -0.5, 4.2], [6.0, 0.5, 7.0], 0.0);
    if ground {
        out += &features();
    }
    if arm != Arm::Empty {
        for i in 0..FIGURES {
            out += &figure(i, arm);
        }
    }
    out
}

fn load(arm: Arm, ground: bool) -> Engine {
    let mut engine = Engine::with_project_root(gallery_root());
    engine
        .load_inline_scene(&bench_scene(arm, ground))
        .expect("the bench scene loads");
    engine
}

struct ArmResult {
    median: f64,
    p99: f64,
    hash: u64,
    report: ProfileReport,
}

fn run_arm(arm: Arm, label: &str, threads: usize) -> ArmResult {
    par::with_threads(threads, || {
        let mut engine = load(arm, true);
        let report = profile_engine(
            &mut engine,
            format!("assembly-anim-{label}"),
            MEASURED,
            WARMUP,
        )
        .expect("profiles");
        let p99 = report
            .sim
            .sequential
            .iter()
            .find(|s| s.name == "extract")
            .and_then(|s| s.tail.as_ref())
            .map_or(0.0, |t| t.p99_ms);
        ArmResult {
            median: sequential(&report, "extract"),
            p99,
            hash: engine.world_hash().expect("hashes").as_u64(),
            report,
        }
    })
}

fn part_models(engine: &Engine, tick: u64) -> Vec<Mat4> {
    let scene = extract_at(
        engine.world(),
        (320, 180),
        ludion_assets::AssetRoot::content(engine.project_root()),
        SimTime::from_ticks(tick, TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )
    .expect("the bench world extracts");
    assert_eq!(scene.meshes.len(), FIGURES * PARTS);
    scene.meshes.iter().map(|m| m.model).collect()
}

fn advanced(arm: Arm, ground: bool, threads: usize) -> (Engine, u64) {
    par::with_threads(threads, || {
        let mut engine = load(arm, ground);
        for _ in 0..20 {
            engine.advance_live();
        }
        let hash = engine.world_hash().expect("hashes").as_u64();
        (engine, hash)
    })
}

#[test]
fn assembly_anim_bench_scene_poses() {
    let (full, one) = advanced(Arm::Full, true, 1);
    let (_, eight) = advanced(Arm::Full, true, THREADS);
    assert_eq!(
        one, eight,
        "WorldHash differs between 1 and {THREADS} threads"
    );

    let (clip, _) = advanced(Arm::Clip, true, 1);
    let full_models = part_models(&full, 20);
    let clip_models = part_models(&clip, 20);
    assert_ne!(
        full_models[..PARTS],
        clip_models[..PARTS],
        "layers, IK and ground pose figure 0 differently from its bare walk clip"
    );

    // The same figures over the floor alone: a foot the ground feature moved is a cast that did work.
    let (flat, _) = advanced(Arm::Full, false, 1);
    let flat_models = part_models(&flat, 20);
    let grounded = (0..FIGURES)
        .filter(|i| {
            [LOWER_LEG_L, LOWER_LEG_R]
                .iter()
                .any(|part| full_models[i * PARTS + part] != flat_models[i * PARTS + part])
        })
        .count();
    assert!(
        grounded >= FIGURES / 2,
        "only {grounded} of {FIGURES} figures had a foot adjusted by the ground"
    );
}

#[test]
#[ignore = "release-mode benchmark; run explicitly (see the module docs)"]
fn assembly_anim_bench() {
    println!(
        "assembly animation bench: {FIGURES} figures, {MEASURED} measured ticks per arm \
         (indicative unless run on a pinned box)"
    );
    println!(
        "{:<7} {:>7} | {:>10} {:>10} {:>10}",
        "arm", "threads", "extract med", "p99", "delta"
    );
    let mut baseline = [0.0; 2];
    let mut gate: Option<ArmResult> = None;
    let mut full_eight_hash = 0;
    for (arm, label) in ARMS {
        for (slot, threads) in [1, THREADS].into_iter().enumerate() {
            let result = run_arm(arm, label, threads);
            if arm == Arm::Empty {
                baseline[slot] = result.median;
            }
            println!(
                "{label:<7} {threads:>7} | {:>10.4} {:>10.4} {:>+10.4}",
                result.median,
                result.p99,
                result.median - baseline[slot]
            );
            if arm == Arm::Full {
                if threads == 1 {
                    gate = Some(result);
                } else {
                    full_eight_hash = result.hash;
                }
            }
        }
    }
    let gate = gate.expect("the full arm ran at one thread");
    assert_eq!(
        gate.hash, full_eight_hash,
        "full: WorldHash differs between 1 and {THREADS} threads"
    );

    if cfg!(debug_assertions) {
        println!("debug build: wall-clock gate skipped");
        return;
    }
    assert!(
        gate.median < EXTRACT_BUDGET_MS,
        "full arm: extract {:.4} ms exceeds the {EXTRACT_BUDGET_MS} ms budget",
        gate.median
    );

    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../budgets");
    let budget = match ludion_testkit::platform_dir() {
        Ok(platform) => platform_budget(&dir, platform, "assembly-anim"),
        Err(_) => {
            println!("no pinned platform here; the budget check is skipped");
            return;
        }
    };
    match budget {
        Some(budget) => {
            assert_eq!(
                gate.report.sim.threads, budget.threads,
                "the budget was calibrated at another thread count"
            );
            let over = check_budget(&gate.report, &budget);
            assert!(over.is_empty(), "over the assembly-anim budget: {over:?}");
        }
        None => println!("no pinned-<box>-assembly-anim.toml (owed: the pinned run writes it)"),
    }
}
