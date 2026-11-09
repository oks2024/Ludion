//! The ragdoll physics bench: N = 60, 120 and 240 mannequin ragdolls (11 rigid `SkinBone` parts and
//! 10 joints each) in a walled room beside 500 walking `CharacterMotor`s, measured in four phases —
//! all awake, settled, a blast waking 40, and switch-in plus despawn churn at 25 per second — as
//! the physics step, write and mirror medians, the churn and motor systems, and the asleep fraction.
//!
//! Ignored by default so CI stays fast; run it explicitly, in release:
//!
//! ```text
//! cargo test -p ludion-engine --release --test it -- ragdoll_bench --ignored --nocapture
//! ```
//!
//! The rig is the mannequin parts prefab `examples/gallery/prefabs/mannequin-ragdoll.prefab.yaml`,
//! switched in by `switch_skin_to_ragdoll`. That call reads and validates the prefab once and then
//! reuses the cached copy (`PrefabLibrary::ready`), so the figures carry the per-switch part spawn
//! and body build but not a per-switch file read. Skins carry no `Animation`: the switch drops it.
//!
//! **Budget file.** The bench checks `budgets/pinned-<box>-ragdoll-stress.toml` when it exists
//! (`apple-m4` on macOS, `rtx3090` on Windows) and skips the check when it does not; the in-file
//! ceilings stand until one exists. Calibrate it at N = 120, phase (d) churn, beside the 500
//! motors, at `threads = 8`: a `threads = 8` line above the first table, `frame_total_ms`, and
//! `[systems]` keys `physics_write`, `physics_step`, `physics_mirror`, `ragdoll_churn` and
//! `motor_walk`, each about 2x the pinned median (`dev-docs/budget-files.md`). The same run command
//! prints the medians to copy.
//!
//! Determinism: the systems here are sim systems, so positions, directions and the churn cadence
//! derive from indices and the tick. Wall-clock reads live only inside `profile_engine`. The phase
//! `WorldHash`es at 1 and 8 threads are asserted equal.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    Budget, Engine, Mesh, MeshSource, ProfileReport, SkinBone, check_budget, load_budget,
    profile_engine, switch_skin_to_ragdoll,
};
use ludion_physics::{CharacterMotor, MIRROR, PhysicsWorld, STEP, WRITE};

use crate::horde_driver::motor_walk;

const PARTS: &str = "prefabs/mannequin-ragdoll.prefab.yaml";
const MOTORS: usize = 500;
const MOTOR_COLUMNS: usize = 25;
const RESERVE: usize = 64;
const THREADS: usize = 8;
const MEASURED: u64 = 120;
const SETTLE_CAP: usize = 1800;
const BLASTED: usize = 40;
/// Switch-ins per second of churn; the sim runs at 60 ticks per second.
const CHURN_PER_SECOND: u64 = 25;
const TICKS_PER_SECOND: u64 = 60;
/// X offset of the motor slab: at least 10 m clear of the ragdoll room.
const MOTOR_SLAB_X: f32 = 80.0;

/// Median `physics_step` per tick, in milliseconds, for N = 120 ragdolls beside 500 motors at 8
/// threads, phase (a) all awake: about 3x the release median on an unpinned M5 laptop (not
/// authoritative), 2026-11-09, rounded up. A pinned budget file overrides it when present.
const STEP_AWAKE_BUDGET_MS: f64 = 5.0;
/// Phase (b) settled, N = 120: about 3x the release median on an unpinned M5 laptop (not
/// authoritative), 2026-11-09, rounded up. A pinned budget file overrides it when present.
const STEP_SETTLED_BUDGET_MS: f64 = 1.0;
/// Phase (c) blast, N = 120: about 3x the release median on an unpinned M5 laptop (not
/// authoritative), 2026-11-09, rounded up. A pinned budget file overrides it when present.
const STEP_BLAST_BUDGET_MS: f64 = 3.0;
/// Phase (d) churn, N = 120: about 3x the release median on an unpinned M5 laptop (not
/// authoritative), 2026-11-09, rounded up. A pinned budget file overrides it when present.
const STEP_CHURN_BUDGET_MS: f64 = 2.5;

fn entity(name: &str, at: [f32; 3], rest: &str) -> String {
    format!(
        "  - name: {name}\n    components:\n      Transform:\n        translation:\n          - {:?}\n          - {:?}\n          - {:?}\n{rest}",
        at[0], at[1], at[2]
    )
}

fn static_box(name: &str, at: [f32; 3], half: [f32; 3]) -> String {
    entity(
        name,
        at,
        &format!(
            "      Collider:\n        shape:\n          box:\n            half_extents:\n              - {:?}\n              - {:?}\n              - {:?}\n        body: static\n",
            half[0], half[1], half[2]
        ),
    )
}

const SKIN_MESH: &str =
    "      Mesh:\n        shape:\n          character: characters/mannequin.character.yaml\n";

/// The bench scene: a camera, a light, the ragdoll room with `n` skins on a grid, `reserve` more
/// skins on a row above it, and a separate slab carrying 500 motors. Entities are ordered ragdoll
/// skins, reserve skins, then motors.
fn bench_scene(n: usize, reserve: usize) -> String {
    let mut out = String::from(
        "format: 1\nseed: 1\nphysics:\n  space: free\n  layers:\n    - name: agents\n      ignores:\n        - agents\nentities:\n",
    );
    out += &entity(
        "camera",
        [0.0, 6.0, 14.0],
        "      Camera:\n        projection:\n          type: perspective\n          fov_y_deg: 45.0\n",
    );
    out += &entity(
        "key-light",
        [0.0, 8.0, 0.0],
        "      PointLight:\n        color:\n          - 1.0\n          - 1.0\n          - 1.0\n        intensity: 6.0\n        range: 30.0\n",
    );
    let cols = (n as f32).sqrt().ceil() as usize;
    let rows = n.div_ceil(cols);
    let half_x = cols as f32 / 2.0 + 1.0;
    let half_z = rows as f32 / 2.0 + 1.0;
    out += &static_box("floor", [0.0, -0.5, 0.0], [half_x + 1.0, 0.5, half_z + 1.0]);
    out += &static_box(
        "wall-east",
        [half_x + 1.0, 1.0, 0.0],
        [0.5, 1.5, half_z + 1.0],
    );
    out += &static_box(
        "wall-west",
        [-half_x - 1.0, 1.0, 0.0],
        [0.5, 1.5, half_z + 1.0],
    );
    out += &static_box(
        "wall-north",
        [0.0, 1.0, half_z + 1.0],
        [half_x + 1.0, 1.5, 0.5],
    );
    out += &static_box(
        "wall-south",
        [0.0, 1.0, -half_z - 1.0],
        [half_x + 1.0, 1.5, 0.5],
    );
    for i in 0..n {
        let at = [
            (i % cols) as f32 - (cols as f32 - 1.0) / 2.0,
            0.3,
            (i / cols) as f32 - (rows as f32 - 1.0) / 2.0,
        ];
        out += &entity(&format!("ragdoll{i}"), at, SKIN_MESH);
    }
    for i in 0..reserve {
        let at = [
            (i % cols) as f32 - (cols as f32 - 1.0) / 2.0,
            3.0,
            (i / cols) as f32 * 1.0 + half_z + 4.0,
        ];
        out += &entity(&format!("reserve{i}"), at, SKIN_MESH);
    }
    out += &static_box(
        "motor-slab",
        [MOTOR_SLAB_X + 15.0, -0.5, 12.0],
        [17.0, 0.5, 14.0],
    );
    for i in 0..MOTORS {
        let at = [
            MOTOR_SLAB_X + 1.2 * (i % MOTOR_COLUMNS) as f32,
            0.8,
            1.2 * (i / MOTOR_COLUMNS) as f32,
        ];
        out += &entity(
            &format!("motor{i}"),
            at,
            "      CharacterMotor:\n        radius: 0.28\n        height: 0.9\n        layer: agents\n",
        );
    }
    out
}

/// What the systems and the driver share: the groups live now, the skins still to admit, and the
/// churn switch.
#[derive(Default)]
struct Churn {
    active: bool,
    pending_initial: Vec<EntityId>,
    reserve: Vec<EntityId>,
    next_reserve: usize,
    live: VecDeque<(EntityId, Vec<EntityId>)>,
    admitted: u64,
}

type Shared = Arc<Mutex<Churn>>;

/// The ids the loaded scene gave its skins and motors, found by component, not by position.
struct Layout {
    skins: Vec<EntityId>,
    motors: Vec<EntityId>,
}

fn layout(world: &mut World) -> Layout {
    let mut skins: Vec<EntityId> = world
        .query_ids::<(&Mesh,)>()
        .filter(|(_, (mesh,))| matches!(mesh.shape, MeshSource::Character(_)))
        .map(|(id, _)| id)
        .collect();
    skins.sort();
    let mut motors: Vec<EntityId> = world
        .query_ids::<(&CharacterMotor,)>()
        .map(|(id, _)| id)
        .collect();
    motors.sort();
    Layout { skins, motors }
}

fn switch(world: &mut World, skin: EntityId) -> (EntityId, Vec<EntityId>) {
    let parts = switch_skin_to_ragdoll(world, skin, PARTS).expect("the mannequin switches");
    (skin, parts)
}

/// Switches every initial skin on the first tick, then, once active, admits 25 reserve skins per
/// 60 ticks and despawns the oldest group for each, keeping the live count constant.
fn ragdoll_churn(world: &mut World, ctx: &SystemCtx, shared: &Mutex<Churn>) {
    let mut churn = shared.lock().unwrap();
    for skin in std::mem::take(&mut churn.pending_initial) {
        let group = switch(world, skin);
        churn.live.push_back(group);
    }
    let tick = ctx.frame.as_u64();
    if !churn.active || tick == 0 {
        return;
    }
    let due = |t: u64| t * CHURN_PER_SECOND / TICKS_PER_SECOND;
    if due(tick) == due(tick - 1) {
        return;
    }
    let skin = churn.reserve[churn.next_reserve];
    churn.next_reserve += 1;
    let group = switch(world, skin);
    churn.live.push_back(group);
    churn.admitted += 1;
    let (old_skin, mut doomed) = churn.live.pop_front().unwrap();
    doomed.push(old_skin);
    let errors = world.despawn_many(&doomed);
    assert!(errors.is_empty(), "the oldest group despawns: {errors:?}");
}

fn builder(shared: &Shared) -> ludion_engine::EngineBuilder {
    let shared = Arc::clone(shared);
    Engine::builder()
        .project_root(gallery_root())
        .system("motor_walk", motor_walk)
        .system(
            "ragdoll_churn",
            move |world: &mut World, ctx: &SystemCtx| ragdoll_churn(world, ctx, &shared),
        )
}

fn gallery_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn load(n: usize, reserve: usize) -> (Engine, Shared, Layout) {
    let shared = Shared::default();
    let mut engine = builder(&shared).build().expect("boots");
    engine
        .load_inline_scene(&bench_scene(n, reserve))
        .expect("the bench scene loads");
    let layout = layout(engine.world_mut());
    assert_eq!(layout.skins.len(), n + reserve);
    assert_eq!(layout.motors.len(), MOTORS);
    {
        let mut churn = shared.lock().unwrap();
        churn.pending_initial = layout.skins[..n].to_vec();
        churn.reserve = layout.skins[n..].to_vec();
    }
    (engine, shared, layout)
}

#[derive(Clone, Copy, Default)]
struct Row {
    median: f64,
    mean: f64,
    p99: f64,
}

#[derive(Clone, Default)]
struct Phase {
    write: Row,
    step: Row,
    mirror: Row,
    churn: Row,
    motors: Row,
    awake_fraction: f64,
    hash: u64,
    settle_ticks: usize,
}

fn row(report: &ProfileReport, name: &str) -> Row {
    let timing = report
        .sim
        .systems
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no timing for `{name}`"));
    let samples = report
        .samples
        .as_ref()
        .expect("offline reports carry samples");
    let at = samples
        .system_names
        .iter()
        .position(|s| s == name)
        .expect("a sampled system");
    let column = &samples.system_ms[at];
    Row {
        median: timing.median_ms,
        mean: column.iter().sum::<f64>() / column.len() as f64,
        p99: timing.tail.as_ref().map_or(0.0, |t| t.p99_ms),
    }
}

/// The fraction of every live part whose body is awake.
fn awake_fraction(engine: &Engine, shared: &Shared) -> f64 {
    let physics = engine.world().resource::<PhysicsWorld>().expect("physics");
    let churn = shared.lock().unwrap();
    let (mut awake, mut total) = (0usize, 0usize);
    for (_, parts) in &churn.live {
        for part in parts {
            total += 1;
            if physics.is_sleeping(*part) == Some(false) {
                awake += 1;
            }
        }
    }
    awake as f64 / total.max(1) as f64
}

fn measure(
    engine: &mut Engine,
    shared: &Shared,
    label: &str,
    warmup: u64,
) -> (Phase, ProfileReport) {
    let report = profile_engine(engine, label.to_string(), MEASURED, warmup).expect("profiles");
    let phase = Phase {
        write: row(&report, WRITE),
        step: row(&report, STEP),
        mirror: row(&report, MIRROR),
        churn: row(&report, "ragdoll_churn"),
        motors: row(&report, "motor_walk"),
        awake_fraction: awake_fraction(engine, shared),
        hash: engine.world_hash().unwrap().as_u64(),
        settle_ticks: 0,
    };
    (phase, report)
}

/// Advances until every live part sleeps or [`SETTLE_CAP`] ticks pass; returns the ticks taken.
fn settle(engine: &mut Engine, shared: &Shared) -> usize {
    for tick in 0..SETTLE_CAP {
        if awake_fraction(engine, shared) == 0.0 {
            return tick;
        }
        engine.advance_live();
    }
    SETTLE_CAP
}

/// Impulses on every part of `BLASTED` groups spread evenly over the live list; index-derived
/// directions, about 1 m/s of upward velocity per part (0.08 N·s on 0.05–0.09 kg parts).
fn blast(engine: &mut Engine, shared: &Shared) {
    let parts: Vec<Vec<EntityId>> = {
        let churn = shared.lock().unwrap();
        let len = churn.live.len();
        (0..BLASTED.min(len))
            .map(|k| churn.live[k * len / BLASTED.min(len)].1.clone())
            .collect()
    };
    let physics = engine
        .world_mut()
        .resource_mut::<PhysicsWorld>()
        .expect("physics");
    for (g, group) in parts.iter().enumerate() {
        for (j, part) in group.iter().enumerate() {
            let sx = if (g + j) % 2 == 0 { 1.0 } else { -1.0 };
            let sz = if (g + 2 * j) % 3 == 0 { 1.0 } else { -1.0 };
            physics
                .apply_impulse(*part, [0.03 * sx, 0.08, 0.03 * sz])
                .expect("queues");
        }
    }
}

struct Arm {
    phases: [Phase; 4],
    churn_report: ProfileReport,
    admitted: u64,
}

fn run_arm(n: usize, threads: usize) -> Arm {
    par::with_threads(threads, || {
        let (mut engine, shared, _) = load(n, RESERVE);
        let (a, _) = measure(&mut engine, &shared, "ragdoll-awake", 1);
        let ticks = settle(&mut engine, &shared);
        let (mut b, _) = measure(&mut engine, &shared, "ragdoll-settled", 10);
        b.settle_ticks = ticks;
        blast(&mut engine, &shared);
        let (c, _) = measure(&mut engine, &shared, "ragdoll-blast", 0);
        let ticks = settle(&mut engine, &shared);
        shared.lock().unwrap().active = true;
        let (mut d, churn_report) = measure(&mut engine, &shared, "ragdoll-churn", 30);
        d.settle_ticks = ticks;
        let admitted = shared.lock().unwrap().admitted;
        Arm {
            phases: [a, b, c, d],
            churn_report,
            admitted,
        }
    })
}

/// The pinned box name the budget file carries, `None` where no pinned box exists.
fn pinned_box(platform: &str) -> Option<&'static str> {
    match platform {
        "macos" => Some("apple-m4"),
        "windows" => Some("rtx3090"),
        _ => None,
    }
}

/// `dir/pinned-<box>-ragdoll-stress.toml` when it exists; a present but broken file panics.
fn platform_budget(dir: &Path, platform: &str) -> Option<Budget> {
    let name = pinned_box(platform)?;
    let path = dir.join(format!("pinned-{name}-ragdoll-stress.toml"));
    if !path.exists() {
        return None;
    }
    Some(load_budget(&path).unwrap_or_else(|d| panic!("{}: {}", path.display(), d.message)))
}

#[test]
fn ragdoll_bench_scene_switches() {
    let (mut engine, shared, layout) = load(4, 4);
    for _ in 0..5 {
        engine.advance_live();
    }
    let parts: usize = shared.lock().unwrap().live.iter().map(|g| g.1.len()).sum();
    assert_eq!(parts, 44, "four mannequins of eleven parts");
    assert_eq!(shared.lock().unwrap().live.len(), 4);
    let bones = engine.world_mut().query_ids::<(&SkinBone,)>().count();
    assert_eq!(bones, 44);
    assert_eq!(layout.motors.len(), MOTORS);

    shared.lock().unwrap().active = true;
    for _ in 0..6 {
        engine.advance_live();
    }
    let churn = shared.lock().unwrap();
    assert!(churn.admitted >= 2, "admitted {}", churn.admitted);
    assert_eq!(churn.live.len(), 4);
}

#[test]
fn platform_budget_skips_absent_and_reads_present() {
    let dir = std::env::temp_dir().join(format!("ludion-ragdoll-budget-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    assert!(platform_budget(&dir, "macos").is_none());
    assert!(platform_budget(&dir, "windows").is_none());
    std::fs::write(
        dir.join("pinned-apple-m4-ragdoll-stress.toml"),
        "threads = 8\n\n[systems]\nphysics_step = 1.0\n",
    )
    .unwrap();
    let budget = platform_budget(&dir, "macos").expect("the present file is read");
    assert_eq!(budget.threads, Some(8));
    assert!(platform_budget(&dir, "windows").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "release-mode benchmark; run explicitly (see the module docs)"]
fn ragdoll_bench() {
    const PHASES: [&str; 4] = ["awake", "settled", "blast", "churn"];
    println!(
        "ragdoll bench at {THREADS} threads, {MOTORS} motors, {MEASURED} measured ticks per phase \
         (indicative unless run on a pinned box)"
    );
    println!(
        "{:>4} {:<8} | {:>8} {:>8} {:>8} | {:>8} {:>8} | {:>8} {:>8} {:>8} {:>8} | {:>6} {:>6}",
        "N",
        "phase",
        "step med",
        "mean",
        "p99",
        "write",
        "mirror",
        "churn med",
        "mean",
        "p99",
        "motors",
        "awake",
        "settle"
    );
    let mut gate: Option<Arm> = None;
    for n in [60, 120, 240] {
        let arm = run_arm(n, THREADS);
        for (label, p) in PHASES.iter().zip(&arm.phases) {
            println!(
                "{n:>4} {label:<8} | {:>8.3} {:>8.3} {:>8.3} | {:>8.3} {:>8.3} | {:>8.3} {:>8.3} {:>8.3} {:>8.3} | {:>6.3} {:>6}",
                p.step.median,
                p.step.mean,
                p.step.p99,
                p.write.median,
                p.mirror.median,
                p.churn.median,
                p.churn.mean,
                p.churn.p99,
                p.motors.median,
                p.awake_fraction,
                p.settle_ticks,
            );
        }
        println!(
            "      churn admitted {} switch-ins in the last phase's run",
            arm.admitted
        );
        if n == 120 {
            gate = Some(arm);
        }
    }
    let gate = gate.expect("the N = 120 arm ran");

    let one = run_arm(60, 1);
    let eight = run_arm(60, THREADS);
    for (i, label) in PHASES.iter().enumerate() {
        assert_eq!(
            one.phases[i].hash, eight.phases[i].hash,
            "{label}: WorldHash differs between 1 and {THREADS} threads"
        );
    }

    let ceilings = [
        STEP_AWAKE_BUDGET_MS,
        STEP_SETTLED_BUDGET_MS,
        STEP_BLAST_BUDGET_MS,
        STEP_CHURN_BUDGET_MS,
    ];
    for ((label, p), ceiling) in PHASES.iter().zip(&gate.phases).zip(ceilings) {
        assert!(
            p.step.median < ceiling,
            "N = 120 {label}: physics_step {:.3} ms exceeds the {ceiling} ms sub-budget",
            p.step.median
        );
    }

    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../budgets");
    let budget = match ludion_testkit::platform_dir() {
        Ok(platform) => platform_budget(&dir, platform),
        Err(_) => {
            println!("no pinned platform here; the budget check is skipped");
            return;
        }
    };
    match budget {
        Some(budget) => {
            assert_eq!(
                gate.churn_report.sim.threads, budget.threads,
                "the budget was calibrated at another thread count"
            );
            let over = check_budget(&gate.churn_report, &budget);
            assert!(over.is_empty(), "over the ragdoll-stress budget: {over:?}");
        }
        None => println!("no pinned-<box>-ragdoll-stress.toml (owed: the pinned run writes it)"),
    }
}
