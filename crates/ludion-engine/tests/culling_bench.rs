//! The culling torture scene, cull off by default (`CULLING_BENCH_CULL=on` schedules the GPU cull):
//! a 40 x 40 m room of 500 animated enemies, 100 SkinBone voxel-ragdoll corpses, 60 decals, 200
//! sprites, 16 particle emitters and 5,000 frozen shots, with SSAO on, measured with the camera facing a wall and facing the crowd. Every number is the
//! "before" for the GPU cull: extract median and its per-phase breakdown, GPU `scene`, `prepass` and
//! frame total, and the cost of sprites, particles, shots and decals by ablation.
//!
//! Ignored by default so CI stays fast; run it explicitly, in release:
//!
//! ```text
//! cargo test -p ludion-engine --release --test it -- culling_bench --ignored --nocapture
//! ```
//!
//! `CULLING_BENCH_CULL=on` measures the same arms with the cull scheduled; combine it with
//! `WGPU_VALIDATION_INDIRECT_CALL=0` to see the indirect-validation cost. Without the variable the
//! arms are the cull-off "before" figures.
//!
//! `CULLING_BENCH_LOD=on` swaps the 500 enemies to the three-level `mannequin-lod` character, so the
//! geometry passes (`shadow`, `prepass`, `scene`, `cull`) read chain off (default) against chain on.
//! The chain draws through the cull pass, so `CULLING_BENCH_CULL` does not change which levels draw.
//!
//! There is deliberately no `budgets/*.toml` entry: those files gate `ProfileReport` keys, and the
//! per-phase extract breakdown is bench-only (it rides `ExtractObserver`, not the wire). The
//! precedent is `nav_bench.rs`: inline budgets. The wall-clock spans are observation-only and never
//! feed sim state. Each arm keeps its fastest of five rounds (extract and GPU independently). Shots are spawned but `projectiles_step` is not scheduled, so every arm draws the
//! same frozen pool.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ludion_core::rng::DeterministicRng;
use ludion_engine::extract_observer::{ExtractObserver, ExtractPhase};
use ludion_engine::{
    Engine, ProfileReport, ProjectileConfig, ProjectileLook, ProjectileVisual, ProjectileVisuals,
    Projectiles, profile_engine,
};
use ludion_projectiles::ProjectileDesc;
use ludion_renderer::graph::pass;

/// Median extract time, in milliseconds, facing the wall. About 3x the release median (0.92 ms) on
/// an unpinned M5 laptop (not authoritative), 2026-11-07, rounded up. Pinned M4 / RTX 3090 figures
/// are owed.
const WALL_EXTRACT_BUDGET_MS: f64 = 3.0;
/// Median GPU frame time, in milliseconds, facing the wall. About 3x the release median (2.8 to
/// 5.9 ms: the laptop GPU flips power state between runs) on the same unpinned M5, rounded up.
const WALL_GPU_BUDGET_MS: f64 = 18.0;
/// Median extract time, in milliseconds, facing the crowd. About 3x the release median (0.94 ms),
/// same provenance.
const CROWD_EXTRACT_BUDGET_MS: f64 = 3.0;
/// Median GPU frame time, in milliseconds, facing the crowd. About 3x the release median (3.1 to
/// 6.3 ms, power-state flips), same provenance.
const CROWD_GPU_BUDGET_MS: f64 = 19.0;

const WARMUP: usize = 10;
const MEASURED: usize = 60;
const SHOTS: usize = 5_000;
/// Each arm is measured this many times and keeps its fastest round: the M5 laptop GPU flips
/// between power states mid-run (the same arm reads 3.4 or 6.3 ms), and the fastest round is the
/// repeatable one.
const ROUNDS: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Facing {
    Wall,
    Crowd,
}

#[derive(Clone, Copy)]
struct Mix {
    sprites: bool,
    particles: bool,
    decals: bool,
}

impl Mix {
    const FULL: Mix = Mix {
        sprites: true,
        particles: true,
        decals: true,
    };
}

const ENEMIES: usize = 500;
const CORPSES: usize = 100;
const CORPSE_BONES: [&str; 11] = [
    "pelvis",
    "torso",
    "head",
    "upper_arm_l",
    "upper_arm_r",
    "lower_arm_l",
    "lower_arm_r",
    "upper_leg_l",
    "upper_leg_r",
    "lower_leg_l",
    "lower_leg_r",
];
/// The voxel ragdoll part drawn for each bone; the pelvis has none.
const CORPSE_PARTS: [Option<&str>; 11] = [
    None,
    Some("torso"),
    Some("head"),
    Some("upper-arm-l"),
    Some("upper-arm-r"),
    Some("lower-arm-l"),
    Some("lower-arm-r"),
    Some("upper-leg-l"),
    Some("upper-leg-r"),
    Some("lower-leg-l"),
    Some("lower-leg-r"),
];
const DECALS: usize = 60;
const SPRITES: usize = 200;
const EMITTERS: usize = 16;
const POINT_LIGHTS: usize = 4;

/// The entity count `scene(_, mix)` declares.
fn expected_entities(mix: Mix) -> usize {
    // camera, sun, lights, floor, four walls, enemies, corpse skins and parts.
    let base = 1 + 1 + POINT_LIGHTS + 1 + 4 + ENEMIES + CORPSES * (1 + CORPSE_BONES.len());
    base + if mix.decals { DECALS } else { 0 }
        + if mix.sprites { SPRITES } else { 0 }
        + if mix.particles { EMITTERS } else { 0 }
}

/// Writes `name:` + the components as the canonical block YAML (floats round-trip via `{:?}`).
struct Doc {
    text: String,
    entities: usize,
}

impl Doc {
    fn entity(&mut self, name: &str, components: &str) {
        let _ = writeln!(self.text, "  - name: {name}\n    components:");
        self.text.push_str(components);
        self.entities += 1;
    }
}

fn vec3(indent: usize, key: &str, v: [f32; 3]) -> String {
    let pad = " ".repeat(indent);
    let mut out = format!("{pad}{key}:\n");
    for x in v {
        let _ = writeln!(out, "{pad}  - {x:?}");
    }
    out
}

fn color(indent: usize, key: &str, c: [f32; 4]) -> String {
    let pad = " ".repeat(indent);
    let mut out = format!("{pad}{key}:\n");
    for x in c {
        let _ = writeln!(out, "{pad}  - {x:?}");
    }
    out
}

fn transform(translation: [f32; 3], rotation: [f32; 3], scale: Option<[f32; 3]>) -> String {
    let mut out = String::from("      Transform:\n");
    out.push_str(&vec3(8, "translation", translation));
    out.push_str(&vec3(8, "rotation", rotation));
    if let Some(scale) = scale {
        out.push_str(&vec3(8, "scale", scale));
    }
    out
}

fn cube(tint: [f32; 4]) -> String {
    format!(
        "      Mesh:\n        shape: cube\n{}",
        color(8, "color", tint)
    )
}

fn scene(facing: Facing, mix: Mix) -> String {
    let mut rng = DeterministicRng::new(11);
    let mut doc = Doc {
        text: String::from("format: 1\nseed: 1\nentities:\n"),
        entities: 0,
    };
    let yaw = match facing {
        Facing::Crowd => 0.0,
        Facing::Wall => 180.0,
    };
    let mut camera = transform([0.0, 1.6, 16.0], [0.0, yaw, 0.0], None);
    camera.push_str(
        "      Camera:\n        projection:\n          type: perspective\n          fov_y_deg: 70.0\n",
    );
    camera.push_str("      PostProcess:\n        ssao: 1.0\n        ssao_radius: 0.5\n");
    doc.entity("camera", &camera);

    let mut sun = transform([0.0, 10.0, 0.0], [0.0, 0.0, 0.0], None);
    sun.push_str("      DirectionalLight:\n");
    sun.push_str(&vec3(8, "direction", [0.3, -0.8, -0.5]));
    sun.push_str("        intensity: 1.0\n        shadow_extent: 24.0\n");
    doc.entity("sun", &sun);
    for i in 0..POINT_LIGHTS {
        let x = if i % 2 == 0 { -10.0 } else { 10.0 };
        let z = if i < 2 { -8.0 } else { 8.0 };
        let mut light = transform([x, 3.5, z], [0.0, 0.0, 0.0], None);
        light.push_str("      PointLight:\n");
        light.push_str(&vec3(8, "color", [1.0, 0.9, 0.8]));
        light.push_str("        intensity: 6.0\n        range: 18.0\n");
        doc.entity(&format!("light-{i}"), &light);
    }

    let grey = [0.55, 0.57, 0.6, 1.0];
    let mut floor = transform([0.0, -0.5, 0.0], [0.0, 0.0, 0.0], Some([40.0, 1.0, 40.0]));
    floor.push_str(&cube(grey));
    doc.entity("floor", &floor);
    for (name, at, scale) in [
        ("wall-n", [0.0, 1.5, -20.0], [40.0, 3.0, 0.4]),
        ("wall-s", [0.0, 1.5, 20.0], [40.0, 3.0, 0.4]),
        ("wall-w", [-20.0, 1.5, 0.0], [0.4, 3.0, 40.0]),
        ("wall-e", [20.0, 1.5, 0.0], [0.4, 3.0, 40.0]),
    ] {
        let mut wall = transform(at, [0.0; 3], Some(scale));
        wall.push_str(&cube([0.5, 0.5, 0.55, 1.0]));
        doc.entity(name, &wall);
    }

    for i in 0..ENEMIES {
        let (col, row) = ((i % 25) as f32, (i / 25) as f32);
        let at = [(col - 12.0) * 1.2, 0.0, (row - 9.5) * 1.2];
        let mut enemy = transform(at, [0.0, rng.next_f32() * 360.0, 0.0], None);
        enemy.push_str(&format!(
            "      Mesh:\n        shape:\n          character: characters/{}.character.yaml\n",
            if lod_on() {
                "mannequin-lod"
            } else {
                "mannequin"
            }
        ));
        enemy.push_str(&color(8, "color", [0.6, 0.45, 0.35, 1.0]));
        enemy.push_str("        roughness: 0.8\n      Animation:\n        clip: idle\n");
        doc.entity(&format!("enemy-{i}"), &enemy);
    }

    for i in 0..CORPSES {
        let (col, row) = ((i % 50) as f32, (i / 50) as f32);
        let at = [(col - 24.5) * 0.72, 0.0, -16.0 - row * 2.0];
        let skin_index = doc.entities;
        let mut skin = transform(at, [0.0, rng.next_f32() * 360.0, 0.0], None);
        skin.push_str(
            "      Mesh:\n        shape:\n          asset: meshes/skin-mannequin.mesh.yaml\n",
        );
        skin.push_str(&color(8, "color", [0.6, 0.45, 0.35, 1.0]));
        skin.push_str("        roughness: 0.8\n");
        doc.entity(&format!("corpse-{i}"), &skin);
        for (bone, part) in CORPSE_BONES.iter().zip(CORPSE_PARTS) {
            let mut body = format!(
                "      SkinBone:\n        skin:\n          index: {skin_index}\n          generation: 0\n        bone: {bone}\n"
            );
            if let Some(part) = part {
                // No `Transform`: the part spawns at its skin's pose times its bone's rest.
                body.push_str(&format!(
                    "      Mesh:\n        shape:\n          assembly_part:\n            assembly: voxels/ragdoll.assembly.yaml\n            part: {part}\n"
                ));
                body.push_str(&color(8, "color", [1.0, 1.0, 1.0, 1.0]));
            }
            doc.entity(&format!("corpse-{i}/{bone}"), &body);
        }
    }

    if mix.decals {
        for i in 0..DECALS {
            let textured = i % 2 == 0;
            let (at, rot) = if i % 3 == 0 {
                (
                    [
                        (rng.next_f32() - 0.5) * 36.0,
                        0.2,
                        (rng.next_f32() - 0.5) * 36.0,
                    ],
                    [0.0; 3],
                )
            } else if i % 3 == 1 {
                ([(rng.next_f32() - 0.5) * 36.0, 1.2, 19.6], [90.0, 0.0, 0.0])
            } else {
                (
                    [(rng.next_f32() - 0.5) * 36.0, 1.2, -19.6],
                    [90.0, 0.0, 0.0],
                )
            };
            let mut body = transform(at, rot, None);
            body.push_str("      Decal:\n");
            body.push_str(&vec3(8, "size", [2.5, 1.0, 1.5]));
            if textured {
                body.push_str(
                    "        base_color_texture:\n          path: assets/decal-hazard-stripes.png\n          wrap: clamp\n",
                );
            } else {
                body.push_str(&color(8, "color", [0.15, 0.45, 0.85, 1.0]));
            }
            doc.entity(&format!("decal-{i}"), &body);
        }
    }
    if mix.sprites {
        for i in 0..SPRITES {
            let at = [
                (rng.next_f32() - 0.5) * 36.0,
                0.6 + rng.next_f32() * 2.0,
                (rng.next_f32() - 0.5) * 36.0,
            ];
            let mut body = transform(at, [0.0; 3], None);
            body.push_str(
                "      Sprite:\n        texture: assets/checker.png\n        size:\n          - 1.0\n          - 1.0\n        billboard: true\n        lit: true\n",
            );
            doc.entity(&format!("sprite-{i}"), &body);
        }
    }
    if mix.particles {
        for i in 0..EMITTERS {
            let at = [
                (rng.next_f32() - 0.5) * 30.0,
                0.2,
                (rng.next_f32() - 0.5) * 30.0,
            ];
            let mut body = transform(at, [0.0; 3], None);
            body.push_str("      ParticleEmitter:\n        shape: sphere\n");
            body.push_str(&vec3(8, "extent", [0.35, 0.35, 0.35]));
            body.push_str("        rate: 18.0\n        life:\n          - 2.0\n          - 2.8\n        speed:\n          - 0.5\n          - 1.0\n        spread_deg: 14.0\n        gravity: -0.9\n        drag: 0.35\n        start_size: 0.9\n        end_size: 3.0\n");
            body.push_str(&color(8, "start_color", [0.7, 0.7, 0.72, 0.5]));
            body.push_str(&color(8, "end_color", [0.5, 0.5, 0.55, 0.0]));
            let _ = writeln!(body, "        softness: 0.85\n        seed: {}", 11 + i);
            doc.entity(&format!("smoke-{i}"), &body);
        }
    }
    debug_assert_eq!(doc.entities, expected_entities(mix));
    doc.text
}

fn visuals() -> ProjectileVisuals {
    let mut visuals = ProjectileVisuals::default();
    visuals.tags.insert(
        0,
        ProjectileVisual {
            look: ProjectileLook::Billboard { softness: 1.0 },
            size: 0.1,
            ..ProjectileVisual::default()
        },
    );
    visuals.tags.insert(
        1,
        ProjectileVisual {
            look: ProjectileLook::Cube,
            size: 0.1,
            ..ProjectileVisual::default()
        },
    );
    visuals
}

fn gallery() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

/// `CULLING_BENCH_CULL=on` measures with the GPU cull scheduled; anything else (the default) is the
/// cull-off "before" arm. Bench-only: no sim reads it.
fn cull_on() -> bool {
    std::env::var("CULLING_BENCH_CULL").is_ok_and(|v| v == "on")
}

/// `CULLING_BENCH_LOD=on` draws the enemies through the LOD chain. Bench-only.
fn lod_on() -> bool {
    std::env::var("CULLING_BENCH_LOD").is_ok_and(|v| v == "on")
}

fn engine(facing: Facing, mix: Mix, shots: bool) -> Engine {
    let mut engine = Engine::builder()
        .project_root(gallery())
        .projectiles(ProjectileConfig {
            capacity: SHOTS as u32,
            ..ProjectileConfig::default()
        })
        .projectile_visuals(visuals())
        .build()
        .expect("boots");
    engine.set_frustum_culling(cull_on());
    engine
        .load_inline_scene(&scene(facing, mix))
        .expect("the torture scene loads");
    if shots {
        let mut rng = DeterministicRng::new(7);
        let pool = engine.world_mut().resource_mut::<Projectiles>().unwrap();
        for i in 0..SHOTS {
            let position = [
                (rng.next_f32() - 0.5) * 36.0,
                0.2 + rng.next_f32() * 2.8,
                (rng.next_f32() - 0.5) * 36.0,
            ];
            let mut desc = ProjectileDesc::new(position, [0.0, 0.0, 1.0]);
            desc.tag = (i % 2) as u32;
            desc.lifetime = 1_000.0;
            pool.spawn(desc).unwrap();
        }
    }
    engine
}

#[derive(Default)]
struct PhaseTotals {
    open: Option<(ExtractPhase, Instant)>,
    call: BTreeMap<ExtractPhase, Duration>,
}

/// The clock site: sums each phase's spans for the current `extract_present_scene` call.
#[derive(Clone, Default)]
struct PhaseClock(Arc<Mutex<PhaseTotals>>);

#[allow(clippy::disallowed_methods)]
fn now() -> Instant {
    Instant::now()
}

#[allow(clippy::disallowed_methods)]
fn lap(start: Instant) -> Duration {
    start.elapsed()
}

impl ExtractObserver for PhaseClock {
    fn begin(&mut self, phase: ExtractPhase) {
        self.0.lock().unwrap().open = Some((phase, now()));
    }

    fn end(&mut self, phase: ExtractPhase) {
        let mut totals = self.0.lock().unwrap();
        if let Some((open, start)) = totals.open.take() {
            assert_eq!(open, phase, "extract phases do not nest");
            *totals.call.entry(phase).or_default() += lap(start);
        }
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1_000.0
}

fn median(samples: &mut [f64]) -> f64 {
    samples.sort_by(f64::total_cmp);
    let mid = samples.len() / 2;
    if samples.len() % 2 == 1 {
        samples[mid]
    } else {
        (samples[mid - 1] + samples[mid]) / 2.0
    }
}

struct Arm {
    label: String,
    extract_total_ms: f64,
    phases: Vec<(ExtractPhase, f64)>,
    report: ProfileReport,
}

impl Arm {
    fn pass(&self, name: &str) -> Option<f64> {
        self.report
            .gpu
            .passes
            .iter()
            .find(|p| p.name == name)
            .and_then(|p| p.median_ms)
    }

    fn gpu_ms(&self) -> Option<f64> {
        self.report.gpu.frame_median_ms
    }

    fn sim_extract_ms(&self) -> f64 {
        self.report
            .sim
            .sequential
            .iter()
            .find(|s| s.name == "extract")
            .map_or(f64::NAN, |s| s.median_ms)
    }
}

fn measure(label: &str, facing: Facing, mix: Mix, shots: bool) -> Arm {
    let mut engine = engine(facing, mix, shots);
    let clock = PhaseClock::default();
    let mut best: Option<(f64, Vec<(ExtractPhase, f64)>)> = None;
    let mut best_report: Option<ProfileReport> = None;
    for _ in 0..ROUNDS {
        engine.set_extract_observer(Some(Box::new(clock.clone())));
        let mut totals = Vec::new();
        let mut by_phase: BTreeMap<ExtractPhase, Vec<f64>> = BTreeMap::new();
        for i in 0..WARMUP + MEASURED {
            let start = now();
            engine.extract_present_scene().expect("extracts");
            let took = lap(start);
            let call = std::mem::take(&mut clock.0.lock().unwrap().call);
            if i >= WARMUP {
                totals.push(ms(took));
                for phase in ExtractPhase::ALL {
                    by_phase
                        .entry(phase)
                        .or_default()
                        .push(ms(call.get(&phase).copied().unwrap_or_default()));
                }
            }
        }
        engine.set_extract_observer(None);
        let total = median(&mut totals);
        if best.as_ref().is_none_or(|(b, _)| total < *b) {
            let phases = by_phase
                .into_iter()
                .map(|(phase, mut samples)| (phase, median(&mut samples)))
                .collect();
            best = Some((total, phases));
        }
        let report = profile_engine(
            &mut engine,
            label.to_string(),
            MEASURED as u64,
            WARMUP as u64,
        )
        .expect("profiles");
        let frame = |r: &ProfileReport| r.gpu.frame_median_ms.unwrap_or(f64::INFINITY);
        if best_report
            .as_ref()
            .is_none_or(|b| frame(&report) < frame(b))
        {
            best_report = Some(report);
        }
    }
    let (extract_total_ms, phases) = best.expect("at least one round");
    Arm {
        label: label.to_string(),
        extract_total_ms,
        phases,
        report: best_report.expect("at least one round"),
    }
}

fn print_arm(arm: &Arm) {
    println!("\n== {} ==", arm.label);
    println!(
        "extract (observed total) {:.3} ms | sim `extract` span {:.3} ms",
        arm.extract_total_ms,
        arm.sim_extract_ms()
    );
    let mut phases = arm.phases.clone();
    phases.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (phase, took) in phases {
        println!("  {phase:<12?} {took:>8.3} ms");
    }
    if !arm.report.gpu.available {
        println!("gpu: unavailable (no adapter)");
        return;
    }
    let show = |name: &str| {
        arm.pass(name)
            .map_or("n/a".to_string(), |v| format!("{v:.3} ms"))
    };
    println!(
        "gpu: cull {} | shadow {} | scene {} | prepass {} | ssao {} | transparents {} | decals_lit {} | projector {} | frame {}",
        show(pass::CULL),
        show(pass::SHADOW),
        show(pass::SCENE),
        show(pass::PREPASS),
        show(pass::SSAO),
        show(pass::TRANSPARENTS),
        show(pass::DECALS_LIT),
        show(pass::PROJECTOR),
        arm.gpu_ms()
            .map_or("n/a".to_string(), |v| format!("{v:.3} ms")),
    );
}

fn print_ablation(what: &str, full: &Arm, without: &Arm) {
    let gpu = match (full.gpu_ms(), without.gpu_ms()) {
        (Some(a), Some(b)) => format!("{:.3} ms", a - b),
        _ => "n/a".to_string(),
    };
    println!(
        "{what:<14} extract {:>7.3} ms | gpu frame {gpu}",
        full.extract_total_ms - without.extract_total_ms
    );
}

#[test]
fn torture_scene_matches_its_counts() {
    for mix in [
        Mix::FULL,
        Mix {
            sprites: false,
            ..Mix::FULL
        },
        Mix {
            particles: false,
            ..Mix::FULL
        },
        Mix {
            decals: false,
            ..Mix::FULL
        },
    ] {
        let mut engine = Engine::builder()
            .project_root(gallery())
            .build()
            .expect("boots");
        engine
            .load_inline_scene(&scene(Facing::Crowd, mix))
            .expect("the torture scene loads");
        assert_eq!(engine.world().entity_count(), expected_entities(mix));
        engine.extract_present_scene().expect("extracts");
    }
}

#[derive(Clone, Default)]
struct Counting(Arc<Mutex<(Vec<ExtractPhase>, Vec<ExtractPhase>)>>);

impl ExtractObserver for Counting {
    fn begin(&mut self, phase: ExtractPhase) {
        self.0.lock().unwrap().0.push(phase);
    }

    fn end(&mut self, phase: ExtractPhase) {
        self.0.lock().unwrap().1.push(phase);
    }
}

#[test]
fn extract_observer_sees_matched_phases_and_leaves_the_hash_alone() {
    let text = scene(Facing::Crowd, Mix::FULL);
    let mut plain = Engine::builder().project_root(gallery()).build().unwrap();
    plain.load_inline_scene(&text).unwrap();
    plain.extract_present_scene().unwrap();
    let plain_hash = plain.world_hash().unwrap();

    let mut observed = Engine::builder().project_root(gallery()).build().unwrap();
    observed.load_inline_scene(&text).unwrap();
    let counting = Counting::default();
    observed.set_extract_observer(Some(Box::new(counting.clone())));
    observed.extract_present_scene().unwrap();
    let observed_hash = observed.world_hash().unwrap();

    let (begins, ends) = counting.0.lock().unwrap().clone();
    assert_eq!(begins, ends, "every begin is matched by an end, in order");
    for phase in ExtractPhase::ALL {
        assert!(begins.contains(&phase), "{phase:?} never reported");
    }
    assert_eq!(plain_hash, observed_hash);
}

#[test]
fn frustum_culling_defaults_on_and_survives_a_reload() {
    let mut engine = Engine::builder().build().unwrap();
    assert!(engine.frustum_culling());
    engine.set_frustum_culling(false);
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .unwrap();
    assert!(!engine.frustum_culling());
}

#[test]
#[ignore = "release-mode benchmark; run explicitly (see the module docs)"]
fn needs_gpu_culling_torture_bench() {
    let arms = [
        measure("Wall / full", Facing::Wall, Mix::FULL, true),
        measure("Crowd / full", Facing::Crowd, Mix::FULL, true),
        measure(
            "Crowd / no sprites",
            Facing::Crowd,
            Mix {
                sprites: false,
                ..Mix::FULL
            },
            true,
        ),
        measure(
            "Crowd / no particles",
            Facing::Crowd,
            Mix {
                particles: false,
                ..Mix::FULL
            },
            true,
        ),
        measure("Crowd / no shots", Facing::Crowd, Mix::FULL, false),
        measure(
            "Crowd / no decals",
            Facing::Crowd,
            Mix {
                decals: false,
                ..Mix::FULL
            },
            true,
        ),
    ];
    for arm in &arms {
        print_arm(arm);
    }
    println!("\n== ablation: full minus ablated (Crowd) ==");
    print_ablation("sprites", &arms[1], &arms[2]);
    print_ablation("particles", &arms[1], &arms[3]);
    print_ablation("billboard shots", &arms[1], &arms[4]);
    print_ablation("decals", &arms[1], &arms[5]);

    let gpu = !ludion_core::require_gpu::skip_no_adapter(!arms[0].report.gpu.available);
    if gpu {
        for arm in &arms {
            assert!(arm.report.quality.as_ref().is_some_and(|q| q.ssao));
            assert!(
                arm.report
                    .gpu
                    .passes
                    .iter()
                    .any(|p| p.name == pass::PREPASS)
            );
        }
    }
    for (arm, extract_budget, gpu_budget) in [
        (&arms[0], WALL_EXTRACT_BUDGET_MS, WALL_GPU_BUDGET_MS),
        (&arms[1], CROWD_EXTRACT_BUDGET_MS, CROWD_GPU_BUDGET_MS),
    ] {
        assert!(
            arm.extract_total_ms <= extract_budget,
            "{}: extract {:.3} ms over the {extract_budget} ms budget",
            arm.label,
            arm.extract_total_ms
        );
        if let Some(frame) = arm.gpu_ms().filter(|_| gpu) {
            assert!(
                frame <= gpu_budget,
                "{}: gpu frame {frame:.3} ms over the {gpu_budget} ms budget",
                arm.label
            );
        }
    }
}
