//! The local-light scatter bench: a 30 x 30 m room with a medium and N scattering lights on a jittered
//! grid in front of the camera, at 1920x1080. Every arm is paired with an identical twin whose lights
//! all have `scatter: 0` (or, for the GI arm, a `GiVolume` at zero strengths), so the scatter pass is
//! not scheduled: 1, 16 and 64 scattering point lights, 16 shadow-casting spots (the twin keeps the
//! shadow maps, isolating scatter from them), and a `GiVolume` with no scattering light.
//!
//! The headline is the **delta of `gpu.frame_median_ms`** against the twin: the GPU frame span is the
//! number to read for frame cost. Per-pass medians overlap on adjacent passes (the M5 especially), so
//! the table prints `medium_scatter` and `medium_scatter_blur` for orientation but never sums them.
//! A delta that comes out negative or within noise is printed as measured, not clamped.
//!
//! Ignored by default so CI stays fast; run it explicitly, in release:
//!
//! ```text
//! cargo test -p ludion-engine --release --test it -- scatter_bench --ignored --nocapture
//! ```
//!
//! There is deliberately no `budgets/*.toml` entry: pinned figures are owed, and the target
//! (a frame delta `<= 0.5 ms` at 16 lights) is reported as `OVER TARGET`, never failed on. The
//! precedent is `culling_bench.rs`: inline budgets, each arm keeps its fastest of five rounds, and a
//! box with no adapter prints and skips.

use std::fmt::Write as _;

use ludion_core::rng::DeterministicRng;
use ludion_engine::{Engine, ProfileReport, profile_engine};
use ludion_renderer::graph::pass;

/// The frame delta at 16 lights, in milliseconds: the request's target.
const TARGET_MS: f64 = 0.5;
/// Frame delta at 16 lights, in milliseconds. About 3x the release delta on an unpinned M5 laptop
/// (not authoritative), 2026-11-08, rounded up. Pinned figures are owed.
const SIXTEEN_BUDGET_MS: f64 = 1.0;
/// Frame delta at 64 lights, same provenance.
const SIXTY_FOUR_BUDGET_MS: f64 = 5.5;

const WARMUP: u64 = 10;
const MEASURED: u64 = 60;
/// The M5 laptop GPU flips power states mid-run; the fastest round is the repeatable one.
const ROUNDS: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Point,
    Spot,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Gi {
    None,
    On,
    /// The same volume at zero strengths: everything else identical, the GI term off.
    Zero,
}

#[derive(Clone, Copy)]
struct Arm {
    label: &'static str,
    lights: usize,
    kind: Kind,
    scatter: f32,
    shadows: bool,
    gi: Gi,
}

/// An arm and its scatter-off twin.
struct Pair {
    on: Arm,
    twin: Arm,
}

impl Arm {
    const fn point(label: &'static str, lights: usize, scatter: f32) -> Arm {
        Arm {
            label,
            lights,
            kind: Kind::Point,
            scatter,
            shadows: false,
            gi: Gi::None,
        }
    }

    const fn spots(label: &'static str, scatter: f32) -> Arm {
        Arm {
            label,
            lights: 16,
            kind: Kind::Spot,
            scatter,
            shadows: true,
            gi: Gi::None,
        }
    }

    const fn gi(label: &'static str, gi: Gi) -> Arm {
        Arm {
            label,
            lights: 0,
            kind: Kind::Point,
            scatter: 0.0,
            shadows: false,
            gi,
        }
    }
}

const PAIRS: [Pair; 5] = [
    Pair {
        on: Arm::point("1", 1, 1.0),
        twin: Arm::point("off-1", 1, 0.0),
    },
    Pair {
        on: Arm::point("16", 16, 1.0),
        twin: Arm::point("off-16", 16, 0.0),
    },
    Pair {
        on: Arm::point("64", 64, 1.0),
        twin: Arm::point("off-64", 64, 0.0),
    },
    Pair {
        on: Arm::spots("16-shadowed", 1.0),
        twin: Arm::spots("16-shadowed-off", 0.0),
    },
    Pair {
        on: Arm::gi("gi-only", Gi::On),
        twin: Arm::gi("gi-off", Gi::Zero),
    },
];

fn vec3(indent: usize, key: &str, v: [f32; 3]) -> String {
    let pad = " ".repeat(indent);
    let mut out = format!("{pad}{key}:\n");
    for x in v {
        let _ = writeln!(out, "{pad}  - {x:?}");
    }
    out
}

fn entity(text: &mut String, name: &str, components: &str) {
    let _ = writeln!(text, "  - name: {name}\n    components:");
    text.push_str(components);
}

fn wall(text: &mut String, name: &str, at: [f32; 3], scale: [f32; 3]) {
    let mut body = String::from("      Transform:\n");
    body.push_str(&vec3(8, "translation", at));
    body.push_str(&vec3(8, "scale", scale));
    body.push_str("      Mesh:\n        shape: cube\n");
    body.push_str(
        "        color:\n          - 0.3\n          - 0.3\n          - 0.32\n          - 1.0\n",
    );
    entity(text, name, &body);
}

/// The room, the medium and the arm's lights, as canonical scene text.
fn scene(arm: Arm) -> String {
    let mut text = String::from("format: 1\nseed: 1\nentities:\n");
    let mut camera = String::from("      Transform:\n");
    camera.push_str(&vec3(8, "translation", [0.0, 1.7, 14.0]));
    camera.push_str(
        "      Camera:\n        projection:\n          type: perspective\n          fov_y_deg: 60.0\n",
    );
    camera.push_str("      PostProcess:\n        fog_density: 0.06\n");
    camera.push_str(&vec3(8, "fog_color", [0.02, 0.016, 0.014]));
    camera.push_str("        fog_albedo: 1.0\n        fog_anisotropy: 0.5\n");
    entity(&mut text, "camera", &camera);
    wall(&mut text, "floor", [0.0, -0.2, 0.0], [30.0, 0.4, 30.0]);
    wall(&mut text, "ceiling", [0.0, 4.2, 0.0], [30.0, 0.4, 30.0]);
    wall(&mut text, "wall-n", [0.0, 2.0, -15.0], [30.0, 4.0, 0.4]);
    wall(&mut text, "wall-s", [0.0, 2.0, 15.0], [30.0, 4.0, 0.4]);
    wall(&mut text, "wall-w", [-15.0, 2.0, 0.0], [0.4, 4.0, 30.0]);
    wall(&mut text, "wall-e", [15.0, 2.0, 0.0], [0.4, 4.0, 30.0]);
    if arm.gi != Gi::None {
        let strength = if arm.gi == Gi::On {
            "        sky_visibility: 1.0\n"
        } else {
            "        sky_visibility: 0.0\n        bounce: 0.0\n"
        };
        entity(
            &mut text,
            "gi-volume",
            &format!(
                "      GiVolume:\n        cell_size: 0.5\n        probe_spacing: 1.0\n{strength}"
            ),
        );
    }

    // A jittered grid over the half of the room the camera faces.
    let mut rng = DeterministicRng::new(5);
    let cols = (arm.lights as f32).sqrt().ceil().max(1.0) as usize;
    let rows = arm.lights.div_ceil(cols.max(1)).max(1);
    for i in 0..arm.lights {
        let (col, row) = ((i % cols) as f32, (i / cols) as f32);
        let x = -13.0 + (col + rng.next_f32()) * 26.0 / cols as f32;
        let z = -13.0 + (row + rng.next_f32()) * 22.0 / rows as f32;
        let y = 1.0 + rng.next_f32() * 2.0;
        let mut body = String::from("      Transform:\n");
        body.push_str(&vec3(8, "translation", [x, y, z]));
        let shadows = if arm.shadows {
            "        cast_shadows: true\n"
        } else {
            ""
        };
        let tail = format!(
            "        intensity: 2.0\n        range: 8.0\n{shadows}        scatter: {:?}\n",
            arm.scatter
        );
        match arm.kind {
            Kind::Point => {
                body.push_str("      PointLight:\n");
                body.push_str(&vec3(8, "color", [1.0, 0.7, 0.4]));
            }
            Kind::Spot => {
                body.push_str("      SpotLight:\n");
                body.push_str(&vec3(8, "color", [1.0, 0.7, 0.4]));
                body.push_str(&vec3(8, "direction", [0.0, -1.0, 0.0]));
                body.push_str("        inner_angle: 30.0\n        outer_angle: 50.0\n");
            }
        }
        body.push_str(&tail);
        entity(&mut text, &format!("light-{i}"), &body);
    }
    text
}

struct Measured {
    arm: Arm,
    report: ProfileReport,
}

impl Measured {
    fn pass(&self, name: &str) -> Option<f64> {
        self.report
            .gpu
            .passes
            .iter()
            .find(|p| p.name == name)
            .and_then(|p| p.median_ms)
    }

    fn frame(&self) -> f64 {
        self.report.gpu.frame_median_ms.unwrap_or(f64::INFINITY)
    }
}

fn measure(arm: Arm) -> Measured {
    let mut engine = Engine::builder().build().expect("boots");
    engine
        .load_inline_scene(&scene(arm))
        .expect("the bench scene loads");
    let mut best: Option<ProfileReport> = None;
    for _ in 0..ROUNDS {
        let report =
            profile_engine(&mut engine, arm.label.to_string(), MEASURED, WARMUP).expect("profiles");
        let frame = |r: &ProfileReport| r.gpu.frame_median_ms.unwrap_or(f64::INFINITY);
        if best.as_ref().is_none_or(|b| frame(&report) < frame(b)) {
            best = Some(report);
        }
    }
    Measured {
        arm,
        report: best.expect("at least one round"),
    }
}

#[test]
fn bench_scenes_load() {
    for arm in PAIRS.iter().flat_map(|p| [p.on, p.twin]) {
        let mut engine = Engine::builder().build().expect("boots");
        engine
            .load_inline_scene(&scene(arm))
            .expect("the bench scene loads");
        let expected = 1 + 6 + usize::from(arm.gi != Gi::None) + arm.lights;
        assert_eq!(engine.world().entity_count(), expected, "{}", arm.label);
    }
}

#[test]
#[ignore = "release-mode benchmark; run explicitly (see the module docs)"]
fn needs_gpu_scatter_bench() {
    let pairs: Vec<(Measured, Measured)> = PAIRS
        .into_iter()
        .map(|p| (measure(p.on), measure(p.twin)))
        .collect();
    if ludion_core::require_gpu::skip_no_adapter(!pairs[0].0.report.gpu.available) {
        return;
    }
    let show = |v: Option<f64>| v.map_or("     n/a".to_string(), |v| format!("{v:>8.3}"));
    println!(
        "\n{:<12} {:>8} {:>10} {:>8}   pass medians (overlap; not additive)",
        "arm", "frame", "twin frame", "delta"
    );
    println!(
        "{:<12} {:>8} {:>10} {:>8}   {:>8} {:>8}",
        "", "ms", "ms", "ms", "scatter", "blur"
    );
    for (on, twin) in &pairs {
        println!(
            "{:<12} {:>8.3} {:>10.3} {:>8.3}   {} {}",
            on.arm.label,
            on.frame(),
            twin.frame(),
            on.frame() - twin.frame(),
            show(on.pass(pass::MEDIUM_SCATTER)),
            show(on.pass(pass::MEDIUM_SCATTER_BLUR)),
        );
    }
    println!("deltas are printed as measured: negative or near-zero means within noise");
    for (on, twin) in &pairs {
        let (present, twin_present) = (
            on.pass(pass::MEDIUM_SCATTER).is_some(),
            twin.pass(pass::MEDIUM_SCATTER).is_some(),
        );
        assert!(
            !twin_present,
            "{}: medium_scatter ran in the scatter-off twin",
            twin.arm.label
        );
        match on.arm.label {
            "gi-only" => println!("gi-only: medium_scatter present = {present}"),
            label => assert!(present, "{label}: medium_scatter missing"),
        }
    }
    let delta = |i: usize| pairs[i].0.frame() - pairs[i].1.frame();
    let sixteen = delta(1);
    if sixteen > TARGET_MS {
        println!(
            "OVER TARGET: 16 lights cost {sixteen:.3} ms of frame against the {TARGET_MS} ms target"
        );
    } else {
        println!("16 lights cost {sixteen:.3} ms of frame, inside the {TARGET_MS} ms target");
    }
    assert!(
        sixteen <= SIXTEEN_BUDGET_MS,
        "16 lights: {sixteen:.3} ms over {SIXTEEN_BUDGET_MS} ms"
    );
    let sixty_four = delta(2);
    assert!(
        sixty_four <= SIXTY_FOUR_BUDGET_MS,
        "64 lights: {sixty_four:.3} ms over {SIXTY_FOUR_BUDGET_MS} ms"
    );
}
