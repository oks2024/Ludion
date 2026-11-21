//! GPU integration tests: real headless rendering of gallery scenes on this machine's adapter.
//!
//! Each test skips gracefully (printing why) when no adapter is available (an `L2003` in the error
//! list), so CI without a GPU stays green. The headline tests are the Phase 2 determinism criterion
//! (rendering the same scene twice yields byte-identical pixels) and the non-triviality guard
//! (a gallery render is not a single flat color — catches all-black regressions without goldens).

use std::path::{Path, PathBuf};

use ludion_core::diagnostics::Diagnostic;
use ludion_engine::{
    Engine, FrameSize, QualitySetting, RenderOutcome, builtin_registry, render_scene,
    render_scene_with_builder, render_scene_with_registry,
};
use ludion_scene::SceneOverride;

fn gallery(scene: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gallery")
        .join(scene)
}

fn no_adapter(errors: &[Diagnostic]) -> bool {
    ludion_core::require_gpu::is_no_adapter(errors)
}

/// Renders a gallery scene, or `None` (after printing a skip reason) when there is no GPU.
fn render_or_skip(test: &str, scene: &str, w: u32, h: u32) -> Option<RenderOutcome> {
    render_or_skip_timed(test, scene, w, h, false)
}

/// As [`render_or_skip`], optionally collecting per-pass GPU timings.
fn render_or_skip_timed(
    test: &str,
    scene: &str,
    w: u32,
    h: u32,
    timings: bool,
) -> Option<RenderOutcome> {
    match render_scene(&gallery(scene), FrameSize::native(w, h), timings) {
        Ok(outcome) => {
            eprintln!("[{test}] {scene} on {}", outcome.adapter);
            Some(outcome)
        }
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[{test}] SKIPPED: no GPU adapter available");
            None
        }
        Err(errors) => panic!("[{test}] render failed: {errors:?}"),
    }
}

/// **Phase 2 determinism criterion.** Rendering the same scene twice in one process is byte-identical.
#[test]
fn needs_gpu_two_renders_of_a_scene_are_byte_identical() {
    let Some(a) = render_or_skip("determinism", "lights.yaml", 160, 90) else {
        return;
    };
    let b = render_scene(&gallery("lights.yaml"), FrameSize::native(160, 90), false).unwrap();
    assert_eq!(a.image, b.image, "two renders of the same scene must match");
}

/// `--dump-timings` payload: well-shaped JSON with one entry per pass, in scheduled order.
/// Timing values are asserted finite and non-negative (their magnitudes are wall-clock GPU
/// measurements and are not asserted).
#[test]
fn needs_gpu_dump_timings_json_has_one_entry_per_pass() {
    let Some(outcome) = render_or_skip_timed("timings", "lights.yaml", 160, 90, true) else {
        return;
    };
    let adapter = outcome.adapter.clone();
    let json = outcome.timings_json.expect("timings were requested");
    let value: serde_json::Value = serde_json::from_str(&json).expect("timings output is JSON");

    let supported = value
        .get("timestamps_supported")
        .and_then(|v| v.as_bool())
        .expect("timestamps_supported is a bool");

    let passes = value
        .get("passes")
        .and_then(|v| v.as_array())
        .expect("passes is an array");
    let names: Vec<&str> = passes
        .iter()
        .map(|p| p.get("name").and_then(|n| n.as_str()).expect("named pass"))
        .collect();
    assert_eq!(
        names,
        [
            "cull",
            "scene",
            "reflections",
            "transparents",
            "bloom_threshold",
            "bloom_down_1",
            "bloom_down_2",
            "bloom_down_3",
            "bloom_down_4",
            "bloom_down_5",
            "bloom_up_4",
            "bloom_up_3",
            "bloom_up_2",
            "bloom_up_1",
            "bloom_up_0",
            "composite"
        ],
        "one entry per scheduled pass, in scheduled order",
    );

    if supported {
        assert!(value.get("timestamp_period_ns").is_some());
        for pass in passes {
            let ms = pass
                .get("gpu_time_ms")
                .and_then(|v| v.as_f64())
                .expect("timing-capable adapters report gpu_time_ms per pass");
            assert!(
                ms.is_finite() && ms >= 0.0,
                "gpu_time_ms must be finite and non-negative, got {ms}",
            );
        }
        // The frame span is reported on exactly the backends that prove one queue timeline.
        let proved = ["(Vulkan)", "(Dx12)", "(Metal)"]
            .iter()
            .any(|backend| adapter.ends_with(backend));
        if proved {
            let span = value
                .get("frame_gpu_time_ms")
                .and_then(|v| v.as_f64())
                .expect("a proving backend reports the frame span");
            assert!(span > 0.0, "the frame span must be positive, got {span}");
        } else {
            assert!(value.get("frame_gpu_time_ms").is_none());
        }
    }
}

/// The bloom pyramid is deterministic like every other path, and its eleven passes genuinely reach
/// the image: the same scene at `bloom_strength: 0.0` differs from the scene as authored. The
/// zero-strength render is the non-vacuity guard — without it a chain that wrote nothing at all
/// would still pass the determinism half. Driven through the override seam so no scene file has to
/// carry a second strength.
#[test]
fn needs_gpu_the_bloom_pyramid_is_deterministic_and_reaches_the_image() {
    let scene = gallery("night-sky.yaml");
    let unlit = [SceneOverride {
        pointer: "/entities/0/components/PostProcess/bloom_strength".into(),
        value: serde_json::json!(0.0),
    }];
    let render = |overrides: &[SceneOverride]| {
        ludion_engine::render_scene_with_registry(
            &ludion_engine::builtin_registry(),
            &scene,
            FrameSize::native(160, 90),
            &ludion_engine::QualitySetting::full(),
            false,
            overrides,
            0,
            &ludion_engine::DebugCategories::None,
            None,
            ludion_engine::AntiAliasing::Off,
            None,
        )
    };

    let first = match render(&[]) {
        Ok(outcome) => outcome,
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[bloom-pyramid] SKIPPED: no GPU adapter available");
            return;
        }
        Err(errors) => panic!("[bloom-pyramid] render failed: {errors:?}"),
    };
    let second = render(&[]).unwrap();
    assert_eq!(
        first.image, second.image,
        "two renders of the same scene must match"
    );

    let no_bloom = render(&unlit).unwrap();
    assert_ne!(
        first.image, no_bloom.image,
        "the pyramid's output must reach the composite — at strength 0 the image has to differ"
    );
}

/// A scratch scene file, removed on drop — including when a mid-test assertion panics — so a failed
/// run never leaves a stray fixture behind.
struct TempScene(PathBuf);

impl TempScene {
    /// Writes `source` to a fresh path under the OS temp directory tagged with `tag` and this
    /// process's id (parallel test runs cannot collide), owning its cleanup.
    fn write(tag: &str, source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "ludion-gpu-render-{tag}-{}.yaml",
            std::process::id()
        ));
        std::fs::write(&path, source).expect("temp scene writes");
        TempScene(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempScene {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// `source` with a `GiVolume` singleton entity appended at the given `cell_size` and
/// `sky_visibility`, and `bounce` when `Some` (`None` leaves the field unauthored, exercising its
/// `0.0` default) — the GI neutrality fixtures' injection point. Appending a final entity is
/// well-formed under the strict-subset scene grammar regardless of what the source scene otherwise
/// contains.
fn with_gi_volume(
    source: &str,
    cell_size: f64,
    sky_visibility: f64,
    bounce: Option<f64>,
) -> String {
    let mut out = source.to_string();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!(
        "  - components:\n      GiVolume:\n        cell_size: {cell_size}\n        \
         sky_visibility: {sky_visibility}\n"
    ));
    if let Some(bounce) = bounce {
        out.push_str(&format!("        bounce: {bounce}\n"));
    }
    out
}

/// The lattice the appended `GiVolume` sizes over `source`, or `None` when the scene builds no
/// voxel grid at all — the shape both GI neutrality assertions guard themselves with.
fn gi_lattice(source: &str, dir: &Path) -> Option<[u32; 3]> {
    let registry = ludion_engine::builtin_registry();
    let checked = ludion_scene::check(source, &registry).expect("the augmented scene validates");
    let world = ludion_scene::instantiate(&checked, &registry).expect("the scene instantiates");
    let extracted =
        ludion_engine::extract(&world, (160, 90), ludion_assets::AssetRoot::content(dir))
            .expect("extract succeeds on the scene");
    extracted.voxel.as_ref()?;
    extracted.gi.as_ref().map(|gi| gi.count)
}

/// **Identity-value neutrality (`Plan/gi-wave1-sky-visibility.md` §11 test 1, the GI arc gate).** A
/// `GiVolume` authored at `sky_visibility: 0.0` renders byte-identical to no `GiVolume` at all,
/// while the same volume at `1.0` visibly does not.
///
/// The equality half is the load-bearing one: it exercises the new group-3 binding, the grown
/// `EnvParams` layout and the new shader branch, and passes only because `mix(1, v, 0)` is exactly
/// `1.0` and `acc * 1.0` is bit-preserving in IEEE-754. The inequality half is its non-vacuity
/// guard — a bug that zeroed the whole feature (an unbound texture read as zero, a bake that never
/// dispatched, a flag stuck off) would satisfy the equality alone. Byte-exact, not SSIM.
#[test]
fn needs_gpu_a_gi_volume_at_zero_strength_moves_no_pixel_and_at_full_strength_moves_many() {
    let Some(without) = render_or_skip("gi-neutrality", "meshes.yaml", 160, 90) else {
        return;
    };

    let source = std::fs::read_to_string(gallery("meshes.yaml")).expect("meshes.yaml reads");
    let render = |tag: &str, sky_visibility: f64| {
        let augmented = with_gi_volume(&source, 1.0, sky_visibility, None);
        let temp = TempScene::write(tag, &augmented);
        let outcome =
            render_scene(temp.path(), FrameSize::native(160, 90), false).unwrap_or_else(|errors| {
                panic!("rendering the GiVolume-augmented scene failed: {errors:?}")
            });
        // Non-vacuity guard: the appended GiVolume must have built a grid AND sized a real probe
        // lattice (more than the 1×1×1 fallback the absent case binds), or neither assertion below
        // says anything about the bake.
        let count = gi_lattice(
            &augmented,
            temp.path().parent().expect("a parent directory"),
        )
        .expect("the appended GiVolume builds a grid and a probe lattice");
        assert!(
            count.iter().all(|&c| c >= 2) && count.iter().product::<u32>() > 8,
            "the probe lattice must be a real volume, not the fallback; it was {count:?}"
        );
        outcome
    };

    let neutral = render("meshes-gi-0", 0.0);
    assert_eq!(
        without.image, neutral.image,
        "a GiVolume at sky_visibility 0.0 must be byte-identical to no GiVolume at all"
    );

    let full = render("meshes-gi-1", 1.0);
    assert_ne!(
        without.image, full.image,
        "sky visibility must reach the image — at full strength the ambient term is occluded"
    );
}

/// **Gate A — skip neutrality (`Plan/gi-wave2-emissive-bounce.md` §9 T1).** A `GiVolume` authored at
/// `sky_visibility: 0.0, bounce: 0.0` renders byte-identical to no `GiVolume` at all.
///
/// The W2 twin of the test above, and what keeps every W1 fixture and every committed golden green
/// **by construction**: `bounce: 0.0` is the field's default, so every already-authored `GiVolume`
/// is exactly this case. It measures the thing §2 says must be measured rather than assumed — that
/// three more group-3 bindings, a grown `EnvParams` (64 → 80 bytes) and a nested branch do not
/// perturb the codegen of the path that does not take them. Byte-exact, not SSIM.
#[test]
fn needs_gpu_a_gi_volume_at_zero_sky_visibility_and_zero_bounce_moves_no_pixel() {
    let Some(without) = render_or_skip("gi-bounce-skip", "meshes.yaml", 160, 90) else {
        return;
    };

    let source = std::fs::read_to_string(gallery("meshes.yaml")).expect("meshes.yaml reads");
    let augmented = with_gi_volume(&source, 1.0, 0.0, Some(0.0));
    let temp = TempScene::write("meshes-gi-bounce-0", &augmented);
    let neutral =
        render_scene(temp.path(), FrameSize::native(160, 90), false).unwrap_or_else(|errors| {
            panic!("rendering the GiVolume-augmented scene failed: {errors:?}")
        });

    // Non-vacuity: the appended GiVolume must have built a grid AND sized a real probe lattice, or
    // the equality below is a statement about the fallback path and says nothing about the feature.
    let count = gi_lattice(
        &augmented,
        temp.path().parent().expect("a parent directory"),
    )
    .expect("the appended GiVolume builds a grid and a probe lattice");
    assert!(
        count.iter().all(|&c| c >= 2) && count.iter().product::<u32>() > 8,
        "the probe lattice must be a real volume, not the fallback; it was {count:?}"
    );

    assert_eq!(
        without.image, neutral.image,
        "a GiVolume at sky_visibility 0.0 and bounce 0.0 must be byte-identical to no GiVolume at all"
    );
}

// ---------------------------------------------------------------------------------------------
// Gate B's fixture: an open floor under a bright sky with one wall, and optionally one emissive
// panel standing in front of that wall. Authored here as YAML (the `gi_sky_visibility.rs` style,
// reimplemented because integration test binaries share no module) rather than borrowed from
// `examples/gallery/`, so no committed scene has to carry a second `bounce` for a test's sake.
// ---------------------------------------------------------------------------------------------

/// A block-YAML line accumulator — the two-space-per-level indentation every scene in this repo uses.
#[derive(Default)]
struct Yaml(String);

impl Yaml {
    fn push(&mut self, indent: usize, line: impl std::fmt::Display) {
        for _ in 0..indent {
            self.0.push_str("  ");
        }
        self.0.push_str(&line.to_string());
        self.0.push('\n');
    }

    fn vec(&mut self, indent: usize, key: &str, values: &[f64]) {
        self.push(indent, format!("{key}:"));
        for v in values {
            self.push(indent + 1, format!("- {v}"));
        }
    }

    /// A matte `Mesh` cube entity at `t` with half-extents `s`. `emissive` is the HDR multiplier on
    /// `color` — `0.0` authors no emission at all, which is what makes the grid emitter-free.
    fn cube(&mut self, name: &str, t: [f64; 3], s: [f64; 3], color: [f64; 3], emissive: f64) {
        self.push(1, format!("- name: {name}"));
        self.push(2, "components:");
        self.push(3, "Transform:");
        self.vec(4, "translation", &t);
        self.vec(4, "rotation", &[0.0, 0.0, 0.0]);
        self.vec(4, "scale", &s);
        self.push(3, "Mesh:");
        self.push(4, "shape: cube");
        self.vec(4, "color", &[color[0], color[1], color[2], 1.0]);
        if emissive > 0.0 {
            self.push(4, format!("emissive: {emissive}"));
        }
    }
}

/// The Gate B fixture. `emissive` decides whether the grid contains an emissive cell at all — with
/// it `false` the scene has **no emissive cell, no `PointLight` and no `DirectionalLight`**, which
/// is exactly the §9 T2 condition: the bake writes an all-zero bounce volume and the shader adds
/// exactly `vec3(0.0)`.
fn bounce_fixture(bounce: f64, emissive: bool) -> String {
    let mut y = Yaml::default();
    y.push(0, "format: 1");
    y.push(0, "name: \"gi-wave2 bounce consumption fixture\"");
    y.push(0, "entities:");

    y.push(1, "- name: environment");
    y.push(2, "components:");
    y.push(3, "Environment:");
    y.vec(4, "sky_zenith", &[0.24, 0.30, 0.42]);
    y.vec(4, "sky_horizon", &[0.30, 0.34, 0.44]);
    y.vec(4, "sky_ground", &[0.08, 0.08, 0.09]);
    y.push(4, "ibl_intensity: 1.0");
    y.push(3, "GiVolume:");
    y.push(4, "cell_size: 0.25");
    y.push(4, "probe_spacing: 0.5");
    y.push(4, "sky_visibility: 1.0");
    y.push(4, format!("bounce: {bounce}"));

    // Straight down (`rotation.x = -90` ⇒ forward is exactly (0, -1, 0)) over the floor strip that
    // sits between the camera and the wall — the surface a bounce off the panel lands on.
    y.push(1, "- name: camera");
    y.push(2, "components:");
    y.push(3, "Transform:");
    y.vec(4, "translation", &[0.0, 3.0, 1.0]);
    y.vec(4, "rotation", &[-90.0, 0.0, 0.0]);
    y.vec(4, "scale", &[1.0, 1.0, 1.0]);
    y.push(3, "Camera:");
    y.push(4, "projection:");
    y.push(5, "type: perspective");
    y.push(5, "fov_y_deg: 60.0");

    y.cube(
        "floor",
        [0.0, -0.25, 0.0],
        [8.0, 0.5, 8.0],
        [0.85, 0.85, 0.85],
        0.0,
    );
    y.cube(
        "wall",
        [0.0, 1.0, -1.8],
        [4.0, 2.0, 0.4],
        [0.70, 0.70, 0.72],
        0.0,
    );
    if emissive {
        y.cube(
            "panel",
            [0.0, 0.5, -1.2],
            [3.0, 1.0, 0.2],
            [1.0, 0.62, 0.28],
            6.0,
        );
    }

    y.0
}

/// Whole-frame mean perceptual luminance (Rec. 709 over the sRGB output bytes) — a monotonic
/// brightness proxy for comparing two renders of the *same* framing.
fn mean_luma(outcome: &RenderOutcome) -> f64 {
    let rgba = &outcome.image.rgba;
    let pixels = (rgba.len() / 4) as f64;
    let sum: f64 = rgba
        .chunks_exact(4)
        .map(|p| 0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]))
        .sum();
    sum / pixels
}

/// **Gate B — the additive identity (`Plan/gi-wave2-emissive-bounce.md` §9 T2, the gate that
/// matters).** A scene whose grid contains no emissive cell, no `PointLight` and no
/// `DirectionalLight` bakes an all-zero bounce volume; at `bounce: 1.0` the shader binds it, samples
/// all three channels, takes the branch and adds exactly `vec3(0.0)` — and the image is **byte-
/// identical** to the same scene at `bounce: 0.0`.
///
/// Gate A proves the skip; this proves the **arithmetic**, because it is the only test that
/// exercises the three bindings, the three fetches, the SH-L1 reconstruction and the add. It stands
/// on §2's bit-exactness argument: `x + (+0.0) == x` in IEEE-754 for every `x` except `−0.0`, and
/// `acc` at the add site is a sum of products of non-negative quantities.
///
/// Two non-vacuity guards, because a silently dead bounce term would pass the equality wearing its
/// costume: the probe lattice must be a real volume (not the 1×1×1 fallback), and **the same fixture
/// with one emissive panel added must move pixels** between `bounce: 0.0` and `1.0`. The second is
/// the complement assertion — the bindings, the bake and the add are demonstrably live on this exact
/// geometry, so the emitter-free equality is a statement about the arithmetic and not about a
/// feature that never ran.
#[test]
fn needs_gpu_the_bounce_term_adds_exactly_zero_when_the_grid_holds_no_emitter() {
    let render = |tag: &str, bounce: f64, emissive: bool| {
        let source = bounce_fixture(bounce, emissive);
        let temp = TempScene::write(tag, &source);
        let outcome = render_scene(temp.path(), FrameSize::native(160, 90), false);
        let dir = temp.path().parent().expect("a parent directory").to_owned();
        (outcome, source, dir)
    };

    let (dark, source, dir) = render("bounce-gate-b-0", 0.0, false);
    let dark = match dark {
        Ok(outcome) => outcome,
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[gi-bounce-gate-b] SKIPPED: no GPU adapter available");
            return;
        }
        Err(errors) => panic!("[gi-bounce-gate-b] render failed: {errors:?}"),
    };
    let count = gi_lattice(&source, &dir).expect("the fixture builds a grid and a probe lattice");
    assert!(
        count.iter().all(|&c| c >= 2) && count.iter().product::<u32>() > 8,
        "the probe lattice must be a real volume, not the fallback; it was {count:?}"
    );
    let first = &dark.image.rgba[0..4];
    assert!(
        dark.image.rgba.chunks_exact(4).any(|px| px != first),
        "the fixture must render real, varied pixels — byte-equality over a flat frame proves nothing"
    );

    let (lit, ..) = render("bounce-gate-b-1", 1.0, false);
    let lit = lit.expect("the bounce-enabled fixture renders");
    assert_eq!(
        dark.image, lit.image,
        "with no emissive cell, no PointLight and no DirectionalLight the baked bounce is exactly \
         zero, so bounce 1.0 must be byte-identical to bounce 0.0"
    );

    // The complement: the same geometry with one emissive panel. If the bindings, the bake or the
    // add were dead, this could not move.
    let (emitter_off, ..) = render("bounce-emitter-0", 0.0, true);
    let (emitter_on, ..) = render("bounce-emitter-1", 1.0, true);
    let emitter_off = emitter_off.expect("the emissive fixture renders at bounce 0.0");
    let emitter_on = emitter_on.expect("the emissive fixture renders at bounce 1.0");
    let (before, after) = (mean_luma(&emitter_off), mean_luma(&emitter_on));
    eprintln!("[gi-bounce-gate-b] emitter mean luma {before:.3} -> {after:.3}");
    assert_ne!(
        emitter_off.image, emitter_on.image,
        "an emissive cell in the grid must reach the image through the bounce volumes"
    );
    assert!(
        after > before * 1.05,
        "the bounce must be light, not noise: mean luma {before:.3} -> {after:.3}"
    );
}

/// A flat ground under vigil's sun at its shadow defaults, optionally with a `1 × 0.02 × 1` plate
/// whose underside hangs 0.12 above the origin, seen from straight above (`rotation.x = -90`: image
/// up is −Z, so +Z is down).
fn thin_plate_fixture(with_plate: bool) -> String {
    let mut y = Yaml::default();
    y.push(0, "format: 1");
    y.push(0, "name: \"shadow-bias thin plate fixture\"");
    y.push(0, "entities:");

    y.push(1, "- name: camera");
    y.push(2, "components:");
    y.push(3, "Transform:");
    y.vec(4, "translation", &[0.0, 4.0, 0.0]);
    y.vec(4, "rotation", &[-90.0, 0.0, 0.0]);
    y.vec(4, "scale", &[1.0, 1.0, 1.0]);
    y.push(3, "Camera:");
    y.push(4, "projection:");
    y.push(5, "type: perspective");
    y.push(5, "fov_y_deg: 60.0");

    y.push(1, "- name: sun");
    y.push(2, "components:");
    y.push(3, "DirectionalLight:");
    y.vec(4, "direction", &[0.32, -0.45, 0.83]);
    y.push(4, "intensity: 2.0");
    y.push(4, "shadow_extent: 10.0");

    y.cube(
        "ground",
        [0.0, -0.25, 0.0],
        [40.0, 0.5, 40.0],
        [0.85, 0.85, 0.85],
        0.0,
    );
    if with_plate {
        y.cube(
            "plate",
            [0.0, 0.13, 0.0],
            [1.0, 0.02, 1.0],
            [0.6, 0.4, 0.2],
            0.0,
        );
    }
    y.0
}

/// **Shadow bias is world units (`Plan/audits/2026-10-03-lighting.md` § O2).** A plate hanging 0.12
/// above the ground casts its footprint shifted downwind by `0.12 · |d_xz| / |d_y| ≈ 0.24`; the
/// occluder-to-ground gap along the light is ≈0.29 world, under the 0.40 the old light-NDC bias
/// swallowed. The ground at (0.1, 0.62) lies past the plate's +Z edge (0.5) but inside that shifted
/// shadow, and the straight-down camera sees it unobstructed.
#[test]
fn needs_gpu_a_thin_plate_near_the_ground_casts_its_shadow() {
    const SIZE: (u32, u32) = (640, 360);
    let render = |tag: &str, with_plate: bool| {
        let temp = TempScene::write(tag, &thin_plate_fixture(with_plate));
        render_scene(temp.path(), FrameSize::native(SIZE.0, SIZE.1), false)
    };
    let without = match render("thin-plate-without", false) {
        Ok(outcome) => outcome,
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[thin-plate-shadow] SKIPPED: no GPU adapter available");
            return;
        }
        Err(errors) => panic!("[thin-plate-shadow] render failed: {errors:?}"),
    };
    let with = render("thin-plate-with", true).expect("the plated fixture renders");

    // Camera 4 above, fov_y 60: world units per pixel at the ground is 2·4·tan(30°) / height.
    let (gx, gz) = (0.1f32, 0.62f32);
    let per_pixel = 2.0 * 4.0 * ludion_core::math::tan(30.0f32.to_radians()) / SIZE.1 as f32;
    let px = (SIZE.0 as f32 / 2.0 + gx / per_pixel).round() as u32;
    let py = (SIZE.1 as f32 / 2.0 + gz / per_pixel).round() as u32;

    let luma = |outcome: &RenderOutcome| {
        let mut sum = 0.0f64;
        for dy in 0..3u32 {
            for dx in 0..3u32 {
                let i = (((py + dy - 1) * outcome.image.width + px + dx - 1) * 4) as usize;
                let p = &outcome.image.rgba[i..i + 3];
                sum +=
                    0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]);
            }
        }
        sum / 9.0
    };
    let (lit, shadowed) = (luma(&without), luma(&with));
    eprintln!("[thin-plate-shadow] probe ({px},{py}) luma {lit:.1} -> {shadowed:.1}");
    assert!(
        lit - shadowed > 30.0,
        "the ground past the plate's edge must be in its shadow: luma {lit:.1} -> {shadowed:.1}"
    );
}

/// A flat ground under a steep sun at a wide shadow extent, optionally with a `4 × 2 × 0.02` wall
/// standing on it (its +Z face at z = 0.01, downwind of the light), seen from straight above at
/// `z = 0.5` so the ground past the face is unobstructed.
fn wall_fixture(with_wall: bool) -> String {
    let mut y = Yaml::default();
    y.push(0, "format: 1");
    y.push(0, "name: \"shadow-lift wall fixture\"");
    y.push(0, "entities:");

    y.push(1, "- name: camera");
    y.push(2, "components:");
    y.push(3, "Transform:");
    y.vec(4, "translation", &[0.0, 4.0, 0.5]);
    y.vec(4, "rotation", &[-90.0, 0.0, 0.0]);
    y.vec(4, "scale", &[1.0, 1.0, 1.0]);
    y.push(3, "Camera:");
    y.push(4, "projection:");
    y.push(5, "type: perspective");
    y.push(5, "fov_y_deg: 60.0");

    y.push(1, "- name: sun");
    y.push(2, "components:");
    y.push(3, "DirectionalLight:");
    y.vec(4, "direction", &[0.0, -1.0, 0.364]);
    y.push(4, "intensity: 2.0");
    y.push(4, "shadow_extent: 96.0");

    y.cube(
        "ground",
        [0.0, -0.25, 0.0],
        [200.0, 0.5, 200.0],
        [0.85, 0.85, 0.85],
        0.0,
    );
    if with_wall {
        y.cube(
            "wall",
            [0.0, 1.0, 0.0],
            [4.0, 2.0, 0.02],
            [0.6, 0.4, 0.2],
            0.0,
        );
    }
    y.0
}

/// **A vertical caster under a high sun at a wide extent shadows its base
/// (`Plan/audits/2026-10-03-lighting.md` § O2).** The slope-scaled
/// shadow bias grows with the light-space texel, so the wall's shadow starts a little downwind of
/// its face; this measures that lift and bounds it.
///
/// The shadow begins where the depth along the light, from the light-facing face, exceeds the bias:
/// `gap ≈ (b_c + k·tanθ·texel)·cos(e) − t`, with `b_c` the 0.05 constant bias, `k` the renderer's
/// slope factor 1.5, `θ` the angle between the face normal and the light (70°, the sun's elevation
/// `e`: the face is vertical), `texel` the world size of a shadow texel (2·96/2048), and `t` the
/// wall's 0.02 thickness (the lit span is shortened by the depth the wall itself covers) — about
/// 0.13. The assertion allows one ground-projected texel (`texel / sin e`) on top, plus the
/// probe's own pixel pitch.
#[test]
fn needs_gpu_a_wall_under_a_high_sun_shadows_its_base_at_a_wide_extent() {
    const SIZE: (u32, u32) = (1280, 720);
    let render = |tag: &str, with_wall: bool| {
        let temp = TempScene::write(tag, &wall_fixture(with_wall));
        render_scene(temp.path(), FrameSize::native(SIZE.0, SIZE.1), false)
    };
    let without = match render("wall-without", false) {
        Ok(outcome) => outcome,
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[wall-shadow-lift] SKIPPED: no GPU adapter available");
            return;
        }
        Err(errors) => panic!("[wall-shadow-lift] render failed: {errors:?}"),
    };
    let with = render("wall-with", true).expect("the walled fixture renders");

    // Camera 4 above the ground at z = 0.5, fov_y 60: image down is +Z.
    let per_pixel = 2.0 * 4.0 * ludion_core::math::tan(30.0f32.to_radians()) / SIZE.1 as f32;
    let px = SIZE.0 / 2;
    let luma = |outcome: &RenderOutcome, z: f32| {
        let py = (SIZE.1 as f32 / 2.0 + (z - 0.5) / per_pixel).round() as u32;
        let mut sum = 0.0f64;
        for dx in 0..3u32 {
            let i = ((py * outcome.image.width + px + dx - 1) * 4) as usize;
            let p = &outcome.image.rgba[i..i + 3];
            sum += 0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]);
        }
        sum / 3.0
    };

    // Walk outward from the face until the walled render is darker than the bare one.
    let face = 0.01f32;
    let mut z = face + per_pixel;
    let gap = loop {
        assert!(
            z < 2.0,
            "the wall's shadow never starts within 2 world units"
        );
        let (lit, shadowed) = (luma(&without, z), luma(&with, z));
        if lit - shadowed > 30.0 {
            break z - face;
        }
        z += per_pixel;
    };

    let elevation = 70.0f32.to_radians();
    let texel = 2.0 * 96.0 / 2048.0;
    let predicted = (0.05 + 1.5 * ludion_core::math::tan(elevation) * texel)
        * ludion_core::math::cos(elevation)
        - 0.02;
    let allowed = predicted + texel / ludion_core::math::sin(elevation) + per_pixel;
    eprintln!(
        "[wall-shadow-lift] measured gap {gap:.3}, predicted {predicted:.3}, allowed {allowed:.3}"
    );
    assert!(
        gap <= allowed,
        "the wall's shadow starts {gap:.3} from its face; the bias model allows {allowed:.3}"
    );
}

/// **Non-triviality guard.** Every gallery render has real pixel variance (not one flat color).
#[test]
fn needs_gpu_gallery_scenes_render_non_trivial_images() {
    for scene in [
        "sprites.yaml",
        "billboards.yaml",
        "meshes.yaml",
        "mesh-asset.yaml",
        "lights.yaml",
        "text.yaml",
        "breakout-mock.yaml",
    ] {
        let Some(outcome) = render_or_skip("non-trivial", scene, 160, 90) else {
            return;
        };
        let rgba = &outcome.image.rgba;
        let first = &rgba[0..4];
        assert!(
            rgba.chunks_exact(4).any(|px| px != first),
            "{scene} rendered a single flat color — likely an all-black regression",
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The bare render path applies the same `SkinBone` spawn pose as an `Engine` load, so a part with
// no authored `Transform` lands on the same pixels through either one.
// ---------------------------------------------------------------------------------------------

/// A scratch project directory under the OS temp dir, with `meshes/skin-mannequin.mesh.yaml`
/// copied in from the gallery fixture — removed (recursively) on drop, including on a mid-test
/// panic.
struct TempProject(PathBuf);

impl TempProject {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("ludion-gpu-render-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("meshes")).expect("the temp project dir creates");
        std::fs::copy(
            gallery("meshes/skin-mannequin.mesh.yaml"),
            dir.join("meshes/skin-mannequin.mesh.yaml"),
        )
        .expect("the mannequin mesh copies into the temp project");
        TempProject(dir)
    }

    /// Writes `source` as this project's `scene.yaml` and returns its path.
    fn scene(&self, source: &str) -> PathBuf {
        let path = self.0.join("scene.yaml");
        std::fs::write(&path, source).expect("the scene fixture writes");
        path
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A camera, a light, a translated and yaw-rotated `skin` entity (scale 1) wearing the mannequin
/// mesh, and three `SkinBone` parts (`pelvis`, `torso`, `upper_arm_l`) — each a bare unit `Mesh:
/// cube` with **no authored `Transform`**, so the only thing that can place it is the spawn pose.
/// No `physics:` block, so nothing moves once spawned.
const SKIN_BONE_BARE_PATH_SCENE: &str = r#"format: 1
name: "SkinBone bare-path spawn-pose fixture"
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.5
          - 1.6
          - 4.0
        rotation:
          - -10.0
          - 0.0
          - 0.0
      Camera:
        projection:
          type: perspective
          fov_y_deg: 50.0
  - name: key-light
    components:
      Transform:
        translation:
          - 1.0
          - 3.0
          - 2.0
      PointLight:
        color:
          - 1.0
          - 1.0
          - 1.0
        intensity: 5.0
        range: 20.0
  - name: skin
    components:
      Transform:
        translation:
          - 0.5
          - 0.2
          - -0.3
        rotation:
          - 0.0
          - 35.0
          - 0.0
      Mesh:
        shape:
          asset: "meshes/skin-mannequin.mesh.yaml"
        color:
          - 0.6
          - 0.45
          - 0.35
          - 1.0
        roughness: 0.8
  - name: pelvis
    components:
      SkinBone:
        skin:
          index: 2
          generation: 0
        bone: pelvis
      Mesh:
        shape: cube
        color:
          - 1.0
          - 0.2
          - 0.2
          - 1.0
  - name: torso
    components:
      SkinBone:
        skin:
          index: 2
          generation: 0
        bone: torso
      Mesh:
        shape: cube
        color:
          - 0.2
          - 1.0
          - 0.2
          - 1.0
  - name: upper_arm_l
    components:
      SkinBone:
        skin:
          index: 2
          generation: 0
        bone: upper_arm_l
      Mesh:
        shape: cube
        color:
          - 0.2
          - 0.2
          - 1.0
          - 1.0
"#;

/// Decodes PNG bytes to RGBA8 `(width, height, bytes)` — `ludion-cli/tests/render.rs`'s own
/// `decode_png`, restated over an in-memory buffer: `render_scene_with_builder` returns PNG bytes
/// directly, and `ludion-testkit` (whose `compare_png_bytes` would otherwise do this) is not a
/// dev-dependency of this crate.
fn decode_png_bytes(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().expect("valid PNG");
    let mut buf = vec![0u8; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).expect("frame decodes");
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

/// The bare render path (`render_scene_with_registry`, `crate::render::extract_scene_file`)
/// applies the same `SkinBone` spawn pose as an `Engine` load
/// (`render_scene_with_builder`/`instantiate_named_world`): three parts with no authored
/// `Transform` land on the same pixels through either one.
///
/// Reasoned rather than re-demonstrated here, per the plan's allowance: before this fix,
/// `extract_scene_file` never called `skin_bone::apply_spawn_poses`, so each part's cube rendered
/// at `Transform::default()` — the world origin — on the bare path, while the `Engine` path posed
/// it at the skin's transform times its bone's rest transform. The fixture's skin sits well off
/// the origin (translated, yaw-rotated) with three differently-colored part cubes at three
/// distinct bones, so that mismatch necessarily moves real, on-screen pixels — reverting the
/// `apply_spawn_poses` call this test guards would fail it.
#[test]
fn needs_gpu_the_bare_render_path_applies_the_same_skin_bone_spawn_pose_as_an_engine_load() {
    let project = TempProject::new("skin-bone-spawn-pose");
    let path = project.scene(SKIN_BONE_BARE_PATH_SCENE);
    let size = FrameSize::native(320, 180);

    let bare = match render_scene_with_registry(
        &builtin_registry(),
        &path,
        size,
        &QualitySetting::full(),
        false,
        &[],
        0,
        &ludion_engine::DebugCategories::None,
        None,
        ludion_engine::AntiAliasing::Off,
        None,
    ) {
        Ok(outcome) => outcome,
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[skin-bone-spawn-pose] SKIPPED: no GPU adapter available");
            return;
        }
        Err(errors) => panic!("[skin-bone-spawn-pose] bare render failed: {errors:?}"),
    };

    let builder_outcome = render_scene_with_builder(
        Engine::builder(),
        &path,
        Some(320),
        Some(180),
        &QualitySetting::full(),
        0,
        &ludion_engine::DebugCategories::None,
    )
    .unwrap_or_else(|errors| panic!("[skin-bone-spawn-pose] Engine render failed: {errors:?}"));

    let (bw, bh, engine_rgba) = decode_png_bytes(&builder_outcome.png);
    assert_eq!(
        (bw, bh),
        (bare.image.width, bare.image.height),
        "both paths must render the same extent"
    );
    assert_eq!(
        bare.image.rgba, engine_rgba,
        "a SkinBone part with no authored Transform must land at the same pixels on the bare \
         render path and an Engine load"
    );
}

// --- Debug draw ------------------------------------------------------------------------------------

/// Draws one long bright horizontal line through the middle of the view, in category `probe`.
fn probe_lines(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
    ludion_engine::debug_draw(world, "probe").line(
        [-3.0, 0.0, 0.0],
        [3.0, 0.0, 0.0],
        [5.0, 0.0, 5.0, 1.0],
    );
}

fn probe_game() -> ludion_engine::EngineBuilder {
    Engine::builder().system("probe_lines", probe_lines)
}

/// A camera looking down −Z at the origin, and — when `occluder` — an opaque slab between it and the
/// line the probe system draws.
fn probe_scene(occluder: bool) -> String {
    let slab = if occluder {
        "  - name: slab
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 3.0
        scale:
          - 12.0
          - 12.0
          - 1.0
      Mesh:
        shape: cube
        color:
          - 0.2
          - 0.6
          - 0.2
          - 1.0
"
    } else {
        ""
    };
    format!(
        "format: 1
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 10.0
      Camera: {{}}
{slab}"
    )
}

/// One probe frame (one tick, so the system has drawn) as RGBA8, or `None` without a GPU.
fn probe_frame(
    project: &TempProject,
    occluder: bool,
    debug: &ludion_engine::DebugCategories,
) -> Option<Vec<u8>> {
    let path = project.scene(&probe_scene(occluder));
    match render_scene_with_builder(
        probe_game(),
        &path,
        Some(160),
        Some(90),
        &QualitySetting::full(),
        1,
        debug,
    ) {
        Ok(outcome) => Some(decode_png_bytes(&outcome.png).2),
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[debug-draw] SKIPPED: no GPU adapter available");
            None
        }
        Err(errors) => panic!("[debug-draw] render failed: {errors:?}"),
    }
}

/// Total absolute channel difference between two frames.
fn channel_delta(a: &[u8], b: &[u8]) -> u64 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(x, y)| x.abs_diff(*y) as u64).sum()
}

/// A category the game drew and the host enabled changes the frame; disabled, it changes nothing —
/// and a frame with no debug draw is byte-identical to an engine that never heard of it.
#[test]
fn needs_gpu_debug_lines_appear_only_when_their_category_is_enabled() {
    use ludion_engine::DebugCategories;
    let project = TempProject::new("debug-draw-on-off");
    let Some(plain) = probe_frame(&project, false, &DebugCategories::None) else {
        return;
    };
    let all = probe_frame(&project, false, &DebugCategories::All).unwrap();
    let named = probe_frame(&project, false, &DebugCategories::parse("probe")).unwrap();
    let other = probe_frame(&project, false, &DebugCategories::parse("motor")).unwrap();
    assert_ne!(plain, all, "an enabled category draws its lines");
    assert_eq!(
        all, named,
        "`all` and the named category show the same lines"
    );
    assert_eq!(
        plain, other,
        "a category nothing drew under changes nothing"
    );

    // (b) `None` is the frame an engine renders without ever calling `set_debug_draw`.
    let path = project.scene(&probe_scene(false));
    let mut engine =
        ludion_engine::engine_at_frame(probe_game(), &path, 1).expect("the probe game loads");
    engine.set_quality(QualitySetting::full());
    let untouched = decode_png_bytes(&engine.render_png(160, 90).unwrap()).2;
    assert_eq!(
        plain, untouched,
        "no debug draw leaves the frame byte-identical"
    );
}

/// A replay render draws the debug lines of the categories asked for, `None` renders as an engine that
/// never heard of debug draw, and turning debug draw on moves no `WorldHash`.
#[test]
fn needs_gpu_a_replay_renders_debug_lines_without_moving_its_hashes() {
    use ludion_engine::{DebugCategories, ReplayFramesRequest};
    use ludion_protocol::command::{Command, SimStepParams};
    use std::ops::ControlFlow;

    let project = TempProject::new("debug-draw-replay");
    let mut live = probe_game()
        .project_root(&project.0)
        .build()
        .expect("the probe game builds");
    live.load_inline_scene(&probe_scene(false))
        .expect("the probe scene loads");
    live.set_recording(std::fs::File::create(project.0.join("run.lrp")).unwrap())
        .unwrap();
    ludion_engine::drive_hashes(
        &mut live,
        vec![Command::SimStep(SimStepParams {
            ticks: 3,
            hash: false,
        })],
    )
    .unwrap();
    live.stop_recording().unwrap();
    let bytes = std::fs::read(project.0.join("run.lrp")).unwrap();

    let render = |debug: DebugCategories| {
        let mut frames: Vec<(u64, Vec<u8>)> = Vec::new();
        let request = ReplayFramesRequest {
            from: 1,
            to: Some(3),
            width: 160,
            height: 90,
            debug,
        };
        let outcome = ludion_engine::replay_render_frames(
            probe_game(),
            &bytes,
            &project.0,
            request,
            |frame, png| {
                frames.push((frame, png.to_vec()));
                Ok(())
            },
        );
        (frames, outcome)
    };

    let (plain, outcome) = render(DebugCategories::None);
    let plain_outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) if no_adapter(std::slice::from_ref(&error)) => {
            eprintln!("[debug-draw] SKIPPED: no GPU adapter available");
            return;
        }
        Err(error) => panic!("[debug-draw] replay render failed: {error}"),
    };
    assert_eq!(plain_outcome.rendered, 3, "the window [1, 3] is 3 frames");
    let (probe, probe_outcome) = render(DebugCategories::parse("probe"));
    let probe_outcome = probe_outcome.unwrap();
    let (motor, motor_outcome) = render(DebugCategories::parse("motor"));
    let motor_outcome = motor_outcome.unwrap();

    assert_ne!(plain, probe, "an enabled category draws its lines");
    assert_eq!(
        plain, motor,
        "a category nothing drew under changes nothing"
    );
    assert!(
        probe_outcome.debug_drawn.contains(&"probe"),
        "{:?}",
        probe_outcome.debug_drawn
    );
    let summary = DebugCategories::parse("motor")
        .summary(&motor_outcome.debug_drawn)
        .expect("a named category has a summary");
    assert!(summary.contains("motor"), "{summary}");

    // `None` is the frames a replay renders without ever calling `set_debug_draw`.
    let mut untouched: Vec<(u64, Vec<u8>)> = Vec::new();
    ludion_engine::replay_with(probe_game(), &bytes, &project.0, |engine, frame| {
        let frame = frame.as_u64();
        if (1..=3).contains(&frame) && untouched.last().is_none_or(|(last, _)| *last != frame) {
            untouched.push((frame, engine.render_png(160, 90)?));
        }
        Ok(ControlFlow::Continue(()))
    })
    .unwrap();
    assert_eq!(
        plain, untouched,
        "no debug draw leaves the frames identical"
    );

    // Debug draw is host state: no hash moves with it.
    let hashes = |debug_on: bool| {
        let mut hashes = Vec::new();
        let mut first = true;
        ludion_engine::replay_with(probe_game(), &bytes, &project.0, |engine, frame| {
            if first && debug_on {
                engine.set_debug_draw(DebugCategories::All);
            }
            first = false;
            hashes.push((frame, engine.world_hash()?));
            Ok(ControlFlow::Continue(()))
        })
        .unwrap();
        hashes
    };
    assert_eq!(hashes(false), hashes(true), "debug draw moves no hash");
}

/// A line behind an opaque mesh still shows (dimmed) — x-ray — but less than the same line in the open.
#[test]
fn needs_gpu_a_line_behind_geometry_shows_dimmed() {
    use ludion_engine::DebugCategories;
    let project = TempProject::new("debug-draw-xray");
    let Some(open_plain) = probe_frame(&project, false, &DebugCategories::None) else {
        return;
    };
    let open_lines = probe_frame(&project, false, &DebugCategories::parse("probe")).unwrap();
    let hidden_plain = probe_frame(&project, true, &DebugCategories::None).unwrap();
    let hidden_lines = probe_frame(&project, true, &DebugCategories::parse("probe")).unwrap();
    let open = channel_delta(&open_plain, &open_lines);
    let hidden = channel_delta(&hidden_plain, &hidden_lines);
    assert!(hidden > 0, "x-ray: the occluded line is still drawn");
    assert!(
        hidden < open,
        "the occluded line is dimmer than the open one: {hidden} vs {open}"
    );
}

/// Writes a `w × h` RGBA8 PNG of one repeated `texel`.
fn write_flat_png(path: &Path, w: u32, h: u32, texel: [u8; 4]) {
    let file = std::fs::File::create(path).expect("png file creates");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header writes");
    let data: Vec<u8> = texel
        .iter()
        .copied()
        .cycle()
        .take((w * h * 4) as usize)
        .collect();
    writer.write_image_data(&data).expect("png data writes");
}

/// **A lit decal's partial coverage leaves the projector a full-strength normal.** Half-covering
/// alpha paint with an 80°-tilted normal map blends the target's normal with the receiver's into one
/// shorter than 1 (`y ≈ 0.59`, below the multiply decal's `angle_fade` start of 45° ≈ 0.71, though the
/// direction is 40° off up). Read raw, the multiply scorch over it would lose about half its weight;
/// normalized, it blackens the floor.
#[test]
fn needs_gpu_a_multiply_decal_over_half_covered_normal_mapped_paint_keeps_full_strength() {
    let dir = std::env::temp_dir().join(format!(
        "ludion-gpu-render-decal-norm-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir creates");
    write_flat_png(&dir.join("half.png"), 4, 4, [255, 255, 255, 128]);
    write_flat_png(&dir.join("solid.png"), 4, 4, [255, 255, 255, 255]);
    write_flat_png(&dir.join("tilt.png"), 4, 4, [253, 128, 150, 255]);

    let mut y = Yaml::default();
    y.push(0, "format: 1");
    y.push(0, "name: \"decal normal normalization fixture\"");
    y.push(0, "entities:");
    y.push(1, "- name: camera");
    y.push(2, "components:");
    y.push(3, "Transform:");
    y.vec(4, "translation", &[0.0, 4.0, 0.0]);
    y.vec(4, "rotation", &[-90.0, 0.0, 0.0]);
    y.push(3, "Camera:");
    y.push(4, "projection:");
    y.push(5, "type: perspective");
    y.push(5, "fov_y_deg: 60.0");
    y.push(1, "- name: sun");
    y.push(2, "components:");
    y.push(3, "DirectionalLight:");
    y.vec(4, "direction", &[0.0, -1.0, 0.0]);
    y.push(4, "intensity: 2.0");
    y.cube(
        "ground",
        [0.0, -0.25, 0.0],
        [40.0, 0.5, 40.0],
        [0.85, 0.85, 0.85],
        0.0,
    );
    y.push(1, "- name: paint");
    y.push(2, "components:");
    y.push(3, "Transform:");
    y.vec(4, "translation", &[0.0, 0.2, 0.0]);
    y.push(3, "Decal:");
    y.vec(4, "size", &[3.0, 1.0, 3.0]);
    y.push(4, "base_color_texture:");
    y.push(5, "path: half.png");
    y.push(4, "normal_texture:");
    y.push(5, "path: tilt.png");
    y.push(1, "- name: scorch");
    y.push(2, "components:");
    y.push(3, "Transform:");
    y.vec(4, "translation", &[0.0, 0.2, 0.0]);
    y.push(3, "Decal:");
    y.vec(4, "size", &[2.0, 1.0, 2.0]);
    y.vec(4, "color", &[0.0, 0.0, 0.0, 1.0]);
    y.push(4, "base_color_texture:");
    y.push(5, "path: solid.png");
    y.vec(4, "angle_fade", &[45.0, 60.0]);
    y.push(4, "blend: multiply");
    let scene = dir.join("scene.yaml");
    std::fs::write(&scene, y.0).expect("scene writes");

    let outcome = match render_scene(&scene, FrameSize::native(1920, 1080), false) {
        Ok(outcome) => outcome,
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[decal-normalize] SKIPPED: no GPU adapter available");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        Err(errors) => panic!("[decal-normalize] render failed: {errors:?}"),
    };
    let _ = std::fs::remove_dir_all(&dir);

    let (cx, cy) = (outcome.image.width / 2, outcome.image.height / 2);
    let i = ((cy * outcome.image.width + cx) * 4) as usize;
    let p = &outcome.image.rgba[i..i + 3];
    eprintln!("[decal-normalize] centre pixel {p:?}");
    assert!(
        p.iter().all(|&c| c <= 8),
        "a black multiply decal over half-covered paint must blacken the floor: {p:?}"
    );
}

const VISIBILITY_BASE: &str = r#"format: 1
name: "visibility-proof"
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 5.0
          - 9.0
        rotation:
          - -28.0
          - 0.0
          - 0.0
      Camera:
        projection:
          type: perspective
          fov_y_deg: 50.0
  - name: sun
    components:
      Transform: {}
      DirectionalLight:
        direction:
          - 0.5
          - -0.8
          - -0.3
        intensity: 2.0
  - name: floor
    components:
      Transform:
        scale:
          - 20.0
          - 1.0
          - 20.0
      Mesh:
        shape: plane
        color:
          - 0.5
          - 0.5
          - 0.5
          - 1.0
  - name: prop
    components:
      Transform:
        translation:
          - -3.0
          - 0.6
          - 0.0
        scale:
          - 1.2
          - 1.2
          - 1.2
      Mesh:
        shape: cube
        color:
          - 0.8
          - 0.6
          - 0.4
          - 1.0
"#;

/// The tree, appended last so no other entity index moves: a root (index 4), two meshes and a
/// shadowed point light. `{visible}` is the root's `Visibility.visible`.
const VISIBILITY_TREE: &str = r#"  - name: tree
    components:
      Transform:
        translation:
          - 2.5
          - 0.0
          - 0.0
      Visibility:
        visible: {visible}
  - name: tree-pillar
    components:
      Parent:
        index: 4
        generation: 0
      Transform:
        translation:
          - 0.0
          - 1.0
          - 0.0
        scale:
          - 0.8
          - 2.0
          - 0.8
      Mesh:
        shape: cube
        color:
          - 0.3
          - 0.6
          - 0.9
          - 1.0
  - name: tree-ball
    components:
      Parent:
        index: 4
        generation: 0
      Transform:
        translation:
          - 1.4
          - 0.5
          - 0.5
      Mesh:
        shape: sphere
        color:
          - 0.9
          - 0.3
          - 0.3
          - 1.0
  - name: tree-lamp
    components:
      Parent:
        index: 4
        generation: 0
      Transform:
        translation:
          - 0.0
          - 3.0
          - 1.5
      PointLight:
        color:
          - 1.0
          - 0.8
          - 0.5
        intensity: 12.0
        range: 9.0
        cast_shadows: true
"#;

/// **Visibility acceptance.** A frame with a hidden tree (meshes plus a shadowed point light, under a
/// sun) is byte-identical to the frame with the tree despawned; shown, the same tree changes the frame.
#[test]
fn needs_gpu_a_hidden_tree_renders_exactly_like_a_despawned_one() {
    let dir = std::env::temp_dir().join(format!("ludion-visibility-proof-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let render = |name: &str, visible: Option<bool>| {
        let path = dir.join(name);
        let mut doc = VISIBILITY_BASE.to_string();
        if let Some(visible) = visible {
            doc.push_str(&VISIBILITY_TREE.replace("{visible}", &visible.to_string()));
        }
        std::fs::write(&path, doc).unwrap();
        render_scene(&path, FrameSize::native(240, 135), false)
    };
    let despawned = match render("despawned.yaml", None) {
        Ok(outcome) => outcome,
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[visibility] SKIPPED: no GPU adapter available");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        Err(errors) => panic!("[visibility] render failed: {errors:?}"),
    };
    let hidden = render("hidden.yaml", Some(false)).unwrap();
    let shown = render("shown.yaml", Some(true)).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        hidden.image, despawned.image,
        "a hidden tree must leave no pixel, shadow or light pool"
    );
    assert_ne!(
        shown.image, despawned.image,
        "the same tree, shown, must draw and cast"
    );
}

/// A cube under a sun, seen by a camera at `eye` with a `fov`; `extra` appends entities.
fn camera_offset_scene(eye: [f32; 3], fov: f32, extra: &str) -> String {
    format!(
        "format: 1
seed: 3
entities:
  - name: cam
    components:
      Transform:
        translation:
          - {:?}
          - {:?}
          - {:?}
      Camera:
        projection:
          type: perspective
          fov_y_deg: {fov:?}
  - name: sun
    components:
      DirectionalLight:
        direction:
          - -0.4
          - -1.0
          - -0.6
  - name: crate
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      Mesh:
        shape: cube
{extra}",
        eye[0], eye[1], eye[2]
    )
}

/// **CameraOffset acceptance.** A scene whose presentation offset pulls the camera in renders the
/// same bytes as the scene with the camera moved by hand, and a different frame than the unoffset
/// one.
#[test]
fn needs_gpu_a_camera_offset_renders_like_a_hand_moved_camera() {
    let dir =
        std::env::temp_dir().join(format!("ludion-camera-offset-proof-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let render = |name: &str, document: String| {
        let path = dir.join(name);
        std::fs::write(&path, document).unwrap();
        render_scene(&path, FrameSize::native(240, 135), false)
    };
    let offset = "  - name: intro
    presentation: true
    components:
      CameraOffset:
        translation:
          - 0.5
          - 0.25
          - -3.0
        fov_delta: -10.0
";
    let plain = match render("plain.yaml", camera_offset_scene([0.0, 1.0, 8.0], 60.0, "")) {
        Ok(outcome) => outcome,
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[camera-offset] SKIPPED: no GPU adapter available");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        Err(errors) => panic!("[camera-offset] render failed: {errors:?}"),
    };
    let with_offset = render(
        "offset.yaml",
        camera_offset_scene([0.0, 1.0, 8.0], 60.0, offset),
    )
    .unwrap();
    let moved = render(
        "moved.yaml",
        camera_offset_scene([0.5, 1.25, 5.0], 50.0, ""),
    )
    .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        with_offset.image, moved.image,
        "the offset frame is the hand-moved frame"
    );
    assert_ne!(
        with_offset.image, plain.image,
        "and the offset moved the view"
    );
}
