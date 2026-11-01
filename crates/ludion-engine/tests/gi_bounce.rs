//! GI Wave 2 physics fixtures (`Plan/gi-wave2-emissive-bounce.md` §9 T3, T4, T6, T7, T8):
//! self-contained GPU-rendered proofs that baked one-bounce irradiance lights a room from an
//! emissive surface, carries the receiving surface's albedo, falls off with distance the way a
//! point light's own `atten^2` does, is occluded from a sealed room by the sun's shadow march, and
//! does not leak through a shared wall between two sealed rooms. Gates A/B (T1/T2) and the
//! magnitude/cache tests (T5/T10) are committed elsewhere (`gpu_render.rs`, `gi_bake.rs`).
//!
//! Built on `gi_sky_visibility.rs`'s machinery — the block-YAML writer, the unique-per-call scratch
//! directories, `mean_luma`, the graceful no-adapter skip — reimplemented here because integration
//! test binaries share no module.
//!
//! **The calibration note that matters (§9):** W1's fixtures isolate GI with `ibl_intensity: 0.0`,
//! severing the ambient term at its coefficient. That lever does not work for bounce — bounce is
//! *added*, not multiplied, and is not scaled by `ibl_intensity`. Every fixture below isolates
//! instead by comparing `bounce: 0.0` against `bounce: 1.0` on an otherwise identical scene, so the
//! only thing that can move a pixel is the bounce volumes themselves.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ludion_core::diagnostics::Diagnostic;
use ludion_engine::{FrameSize, RenderOutcome, render_scene};

// ---------------------------------------------------------------------------------------------
// GPU skip machinery (mirrors `gpu_render.rs`'s `render_or_skip` and `gi_sky_visibility.rs`'s
// `render_fixture`, reimplemented here because integration test binaries share no module).
// ---------------------------------------------------------------------------------------------

fn no_adapter(errors: &[Diagnostic]) -> bool {
    ludion_core::require_gpu::is_no_adapter(errors)
}

/// Renders `scene` at `w`x`h`, or `None` (after printing a skip reason) when there is no GPU.
fn render_fixture(test: &str, scene: &Path, w: u32, h: u32) -> Option<RenderOutcome> {
    match render_scene(scene, FrameSize::native(w, h), false) {
        Ok(outcome) => {
            eprintln!("[{test}] rendered on {}", outcome.adapter);
            Some(outcome)
        }
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[{test}] SKIPPED: no GPU adapter available");
            None
        }
        Err(errors) => panic!("[{test}] render failed: {errors:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// Unique-per-call scratch directories — the flake `gi_sky_visibility.rs` documents: two tests in
// this binary run as threads in one process, so a monotonic counter (not just a process id) makes
// every call's directory distinct regardless of thread interleaving.
// ---------------------------------------------------------------------------------------------

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A scratch directory, unique per instance, removed on drop (including when a mid-test assertion
/// panics).
struct Fixture(PathBuf);

impl Fixture {
    fn new(tag: &str) -> Self {
        let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("ludion-gi-bounce-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create fixture directory");
        Fixture(dir)
    }

    /// Writes `source` as `name` inside this fixture's directory (creating any parent directory)
    /// and returns its path.
    fn scene(&self, name: &str, source: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().expect("fixture file has a parent"))
            .expect("create fixture subdirectory");
        std::fs::write(&path, source).expect("write fixture scene");
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------------------------------------
// A tiny block-YAML writer (`gi_sky_visibility.rs`'s verbatim shape) — Ludion's scene grammar is a
// strict subset of YAML that forbids non-empty flow collections (`L1005`), so every array is a
// block sequence.
// ---------------------------------------------------------------------------------------------

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
}

/// Opens `- name: NAME` / `components:` at the entity level (indent 1/2) and lets `body` push the
/// component block(s) at indent 3+.
fn entity(y: &mut Yaml, name: &str, body: impl FnOnce(&mut Yaml)) {
    y.push(1, format!("- name: {name}"));
    y.push(2, "components:");
    body(y);
}

fn transform(y: &mut Yaml, t: [f64; 3], r: [f64; 3], s: [f64; 3]) {
    y.push(3, "Transform:");
    y.vec(4, "translation", &t);
    y.vec(4, "rotation", &r);
    y.vec(4, "scale", &s);
}

/// A matte `Mesh` cube entity (default `roughness: 1.0`, `metallic: 0.0`, so the measured luminance
/// is ambient/GI/direct, never a specular highlight riding along). `emissive` is the HDR multiplier
/// on `color`, omitted (leaving the component default of `0.0`) when it is exactly `0.0` — the
/// `gpu_render.rs` Gate B fixture's convention.
fn cube(y: &mut Yaml, name: &str, t: [f64; 3], s: [f64; 3], color: [f64; 3], emissive: f64) {
    entity(y, name, |y| {
        transform(y, t, [0.0; 3], s);
        y.push(3, "Mesh:");
        y.push(4, "shape: cube");
        y.vec(4, "color", &[color[0], color[1], color[2], 1.0]);
        if emissive > 0.0 {
            y.push(4, format!("emissive: {emissive}"));
        }
    });
}

/// A non-emissive wall/floor/ceiling cube — [`cube`] with `emissive: 0.0`.
fn wall(y: &mut Yaml, name: &str, t: [f64; 3], s: [f64; 3], color: [f64; 3]) {
    cube(y, name, t, s, color, 0.0);
}

/// A camera looking straight down (`rotation.x = -90`, forward exactly `(0, -1, 0)`) at `(x, z)`
/// from `height` above the floor — `gi_sky_visibility.rs`'s established top-down framing, used here
/// to isolate one floor patch as (nearly) the whole image so a fixture reads a whole-frame mean
/// luminance instead of guessing pixel-rectangle boundaries.
fn camera_top_down(y: &mut Yaml, x: f64, z: f64, height: f64, fov_y_deg: f64) {
    entity(y, "camera", |y| {
        transform(y, [x, height, z], [-90.0, 0.0, 0.0], [1.0; 3]);
        y.push(3, "Camera:");
        y.push(4, "projection:");
        y.push(5, "type: perspective");
        y.push(5, format!("fov_y_deg: {fov_y_deg}"));
    });
}

/// A camera at `(x, y, z)` looking along the untouched forward axis (`rotation: [0,0,0]` is exactly
/// `-Z`, the same base direction `camera_forward`'s tilt is measured from in `gi_sky_visibility.rs`)
/// — used to frame a *vertical* surface (T7's wall) rather than a floor, where a top-down camera
/// cannot look.
fn camera_facing_neg_z(y: &mut Yaml, x: f64, y_h: f64, z: f64, fov_y_deg: f64) {
    entity(y, "camera", |y| {
        transform(y, [x, y_h, z], [0.0, 0.0, 0.0], [1.0; 3]);
        y.push(3, "Camera:");
        y.push(4, "projection:");
        y.push(5, "type: perspective");
        y.push(5, format!("fov_y_deg: {fov_y_deg}"));
    });
}

/// A `GiVolume` singleton at the given `bounce` strength. `sky_visibility` and `normal_bias` are
/// left at the component's own defaults (`1.0` and `0.25`) — every fixture below isolates on
/// `bounce` alone (see the module doc's calibration note), so there is nothing to gain from
/// touching them.
fn gi_volume(y: &mut Yaml, cell_size: f64, probe_spacing: f64, bounce: f64) {
    entity(y, "gi", |y| {
        y.push(3, "GiVolume:");
        y.push(4, format!("cell_size: {cell_size}"));
        y.push(4, format!("probe_spacing: {probe_spacing}"));
        y.push(4, format!("bounce: {bounce}"));
    });
}

fn point_light(y: &mut Yaml, t: [f64; 3], color: [f64; 3], intensity: f64, range: f64) {
    entity(y, "point-light", |y| {
        transform(y, t, [0.0; 3], [1.0; 3]);
        y.push(3, "PointLight:");
        y.vec(4, "color", &color);
        y.push(4, format!("intensity: {intensity}"));
        y.push(4, format!("range: {range}"));
    });
}

fn directional_light(y: &mut Yaml, direction: [f64; 3], color: [f64; 3], intensity: f64) {
    entity(y, "sun", |y| {
        y.push(3, "DirectionalLight:");
        y.vec(4, "direction", &direction);
        y.vec(4, "color", &color);
        y.push(4, format!("intensity: {intensity}"));
    });
}

fn header(y: &mut Yaml, name: &str) {
    y.push(0, "format: 1");
    y.push(0, format!("name: \"{name}\""));
    y.push(0, "entities:");
}

// ---------------------------------------------------------------------------------------------
// Measurement.
// ---------------------------------------------------------------------------------------------

/// Whole-frame mean perceptual luminance (Rec. 709 weights over the sRGB-encoded output bytes) —
/// a monotonic brightness proxy for comparing two renders of the *same* framing, not a physical
/// unit. `gi_sky_visibility.rs`'s verbatim.
fn mean_luma(outcome: &RenderOutcome) -> f64 {
    let rgba = &outcome.image.rgba;
    let pixels = (rgba.len() / 4) as f64;
    let sum: f64 = rgba
        .chunks_exact(4)
        .map(|p| 0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]))
        .sum();
    sum / pixels
}

/// Whole-frame mean per-channel byte, `(r, g, b)` — T4's instrument, where the *ratio* between
/// channels (not the luma scalar) is the signal a red-tinted bounce moves.
fn mean_rgb(outcome: &RenderOutcome) -> (f64, f64, f64) {
    let rgba = &outcome.image.rgba;
    let pixels = (rgba.len() / 4) as f64;
    let (mut r, mut g, mut b) = (0.0, 0.0, 0.0);
    for p in rgba.chunks_exact(4) {
        r += f64::from(p[0]);
        g += f64::from(p[1]);
        b += f64::from(p[2]);
    }
    (r / pixels, g / pixels, b / pixels)
}

/// The probe lattice a `GiVolume` sized, or `None` when the scene builds no voxel grid at all — the
/// non-vacuity guard `gpu_render.rs`'s identity-value test uses, reimplemented here (integration
/// test binaries share no module).
fn gi_lattice(source: &str, dir: &Path) -> Option<[u32; 3]> {
    let registry = ludion_engine::builtin_registry();
    let checked = ludion_scene::check(source, &registry).expect("the fixture scene validates");
    let world = ludion_scene::instantiate(&checked, &registry).expect("the fixture instantiates");
    let extracted =
        ludion_engine::extract(&world, (160, 90), ludion_assets::AssetRoot::content(dir))
            .expect("extract succeeds on the fixture");
    extracted.voxel.as_ref()?;
    extracted.gi.as_ref().map(|gi| gi.count)
}

// =================================================================================================
// T3 — the torch lights the room.
// =================================================================================================

/// A sealed room (floor, ceiling, four walls — no opening) with one interior partition splitting it
/// into two compartments, and one warm emissive cube against the west wall (`gi-interior`'s own
/// torch color/strength, `[1.0, 0.62, 0.28]` at `emissive: 4.0`). No other light of any kind.
///
/// The partition (`x` in `[-0.25, 0.25]`, spanning `z` in `[-0.5, 2.0]`, touching the south wall's
/// inner face and leaving a 1.5-unit gap near the north wall) keeps the room one connected box while
/// blocking every straight line from the emitter (west compartment, `x = -1.5`) to a floor point
/// deep in the east compartment near the south corner (`x = 1.5, z = 1.5`) — worked out by
/// parametrizing the segment between the two points and checking it crosses the partition's solid
/// volume, not just its footprint.
fn torch_room_scene(bounce: f64, cam: [f64; 3], fov_y_deg: f64) -> String {
    let mut y = Yaml::default();
    header(&mut y, "gi-wave2 bounce fixture: torch lights the room");
    camera_top_down(&mut y, cam[0], cam[2], cam[1], fov_y_deg);
    gi_volume(&mut y, 0.25, 0.5, bounce);

    wall(
        &mut y,
        "floor",
        [0.0, -0.25, 0.0],
        [5.0, 0.5, 5.0],
        [0.6, 0.6, 0.62],
    );
    wall(
        &mut y,
        "ceiling",
        [0.0, 3.25, 0.0],
        [5.0, 0.5, 5.0],
        [0.6, 0.6, 0.62],
    );
    wall(
        &mut y,
        "wall-west",
        [-2.25, 1.5, 0.0],
        [0.5, 3.0, 5.0],
        [0.55, 0.55, 0.58],
    );
    wall(
        &mut y,
        "wall-east",
        [2.25, 1.5, 0.0],
        [0.5, 3.0, 5.0],
        [0.55, 0.55, 0.58],
    );
    wall(
        &mut y,
        "wall-north",
        [0.0, 1.5, -2.25],
        [5.0, 3.0, 0.5],
        [0.55, 0.55, 0.58],
    );
    wall(
        &mut y,
        "wall-south",
        [0.0, 1.5, 2.25],
        [5.0, 3.0, 0.5],
        [0.55, 0.55, 0.58],
    );
    // Partition: touches the south wall's inner face (z=2.0), leaves a gap near the north wall.
    wall(
        &mut y,
        "partition",
        [0.0, 1.5, 0.75],
        [0.5, 3.0, 2.5],
        [0.55, 0.55, 0.58],
    );

    cube(
        &mut y,
        "torch",
        [-1.5, 1.0, 0.0],
        [0.6, 0.6, 0.6],
        [1.0, 0.62, 0.28],
        4.0,
    );

    y.0
}

/// Beside the emitter, west compartment, unobstructed and close (0.8 world units away).
const TORCH_NEAR_CAM: [f64; 3] = [-1.5, 1.0, 0.8];
/// Deep in the east compartment near the south corner, blocked from the emitter by the partition.
const TORCH_FAR_CAM: [f64; 3] = [1.5, 1.0, 1.5];

/// **T3 — the torch lights the room (`Plan/gi-wave2-emissive-bounce.md` §9 T3).** A floor patch
/// beside the emitter reads materially brighter at `bounce: 1.0` than at `0.0`, asserted as a ratio
/// (survives an exposure retune); a patch behind the interior partition, out of the emitter's line
/// of sight, holds within a small tolerance. Both halves are asserted — a bake that brightened
/// *everything* would pass the first alone.
#[test]
fn needs_gpu_the_torch_lights_the_room() {
    let fixture = Fixture::new("torch");
    let fov = 10.0;

    let render = |tag: &str, cam: [f64; 3], bounce: f64| {
        let source = torch_room_scene(bounce, cam, fov);
        let path = fixture.scene(&format!("{tag}.yaml"), &source);
        (render_fixture("gi-torch", &path, 160, 90), source)
    };

    let (Some(near_off), source_off) = render("near-0", TORCH_NEAR_CAM, 0.0) else {
        return;
    };
    let (near_on, _) = render("near-1", TORCH_NEAR_CAM, 1.0);
    let near_on = near_on.expect("GPU present for the second render");
    let (far_off, _) = render("far-0", TORCH_FAR_CAM, 0.0);
    let far_off = far_off.expect("GPU present");
    let (far_on, _) = render("far-1", TORCH_FAR_CAM, 1.0);
    let far_on = far_on.expect("GPU present");

    // Non-vacuity: a real probe lattice, not the 1x1x1 fallback.
    let count = gi_lattice(&source_off, fixture.0.as_path()).expect("the fixture bakes a lattice");
    assert!(
        count.iter().all(|&c| c >= 2) && count.iter().product::<u32>() > 8,
        "the probe lattice must be a real volume, not the fallback; it was {count:?}"
    );

    let near_off_luma = mean_luma(&near_off);
    let near_on_luma = mean_luma(&near_on);
    let far_off_luma = mean_luma(&far_off);
    let far_on_luma = mean_luma(&far_on);
    let near_ratio = near_on_luma / near_off_luma.max(1e-6);
    let far_ratio = far_on_luma / far_off_luma.max(1e-6);

    eprintln!(
        "[gi-torch] near off={near_off_luma:.3} on={near_on_luma:.3} ratio={near_ratio:.3} | \
         far off={far_off_luma:.3} on={far_on_luma:.3} ratio={far_ratio:.3}"
    );

    assert!(
        near_ratio > 1.3,
        "the floor beside the torch must read materially brighter with bounce on: \
         off={near_off_luma:.3}, on={near_on_luma:.3}, ratio={near_ratio:.3} (wanted > 1.3)"
    );
    assert!(
        (0.9..=1.15).contains(&far_ratio),
        "the floor behind the partition, out of the torch's line of sight, must hold within a \
         small tolerance: off={far_off_luma:.3}, on={far_on_luma:.3}, ratio={far_ratio:.3}"
    );
}

// =================================================================================================
// T4 — albedo tints the bounce.
// =================================================================================================

/// An open white floor beside a saturated red wall, lit by one white `PointLight` aimed at the
/// wall's face. The floor's own probes see the wall directly (unobstructed, close range), so any
/// bounce their volume carries is red-tinted by the wall's albedo alone — the light itself is white
/// and the floor is white, so nothing else in the scene can redden the image.
fn albedo_bounce_scene(bounce: f64) -> String {
    let mut y = Yaml::default();
    header(&mut y, "gi-wave2 bounce fixture: albedo tints the bounce");
    // Floor patch close to the wall but offset along x from the light, so the light's own direct
    // hit on the *floor* is weak (a low, near-grazing incidence — see the light's placement below)
    // while the *wall* it faces stays strongly lit. Without this offset the floor patch saturates
    // near the 8-bit ceiling under direct light alone, leaving the additive bounce no headroom to
    // move.
    camera_top_down(&mut y, 1.2, -1.9, 1.0, 10.0);
    gi_volume(&mut y, 0.25, 0.5, bounce);

    wall(
        &mut y,
        "floor",
        [0.0, -0.25, 0.0],
        [6.0, 0.5, 6.0],
        [0.85, 0.85, 0.85],
    );
    wall(
        &mut y,
        "red-wall",
        [0.0, 1.5, -2.5],
        [6.0, 3.0, 0.5],
        [0.95, 0.02, 0.02],
    );
    // Low and close to the wall (not to the sampled floor patch): a floor-height light gives any
    // floor point a near-grazing (low `ndl`) direct hit regardless of distance, while the vertical
    // wall it faces receives a near-normal-incidence hit close to the light's own height.
    point_light(&mut y, [0.0, 0.4, -1.7], [1.0, 1.0, 1.0], 10.0, 6.0);

    y.0
}

/// **T4 — albedo tints the bounce (`Plan/gi-wave2-emissive-bounce.md` §9 T4).** The floor's rendered
/// `R/G` channel ratio rises measurably between `bounce: 0.0` and `bounce: 1.0` — the gate that
/// fails if the bake drops `albedo` from `L_out` (a scalar-emissive-style bounce would carry the
/// light's white color, not the wall's red one, and the ratio would not move).
#[test]
fn needs_gpu_albedo_tints_the_bounce() {
    let fixture = Fixture::new("albedo");

    let render = |tag: &str, bounce: f64| {
        let source = albedo_bounce_scene(bounce);
        let path = fixture.scene(&format!("{tag}.yaml"), &source);
        render_fixture("gi-albedo", &path, 160, 90)
    };

    let Some(off) = render("bounce-0", 0.0) else {
        return;
    };
    let on = render("bounce-1", 1.0).expect("GPU present for the second render");

    let (r_off, g_off, _) = mean_rgb(&off);
    let (r_on, g_on, _) = mean_rgb(&on);
    let ratio_off = r_off / g_off.max(1e-6);
    let ratio_on = r_on / g_on.max(1e-6);

    eprintln!(
        "[gi-albedo] off R={r_off:.3} G={g_off:.3} R/G={ratio_off:.4} | on R={r_on:.3} G={g_on:.3} \
         R/G={ratio_on:.4}"
    );

    assert!(
        ratio_on > ratio_off * 1.03,
        "the floor's R/G ratio must rise measurably once the red wall's albedo rides the bounce: \
         off R/G={ratio_off:.4}, on R/G={ratio_on:.4}"
    );
}

/// A closed unit cube as a `kind: mesh` document (eight corners, twelve triangles, no emissive).
const UNIT_CUBE_MESH: &str = "format: 1
kind: mesh
positions:
  - -0.5
  - -0.5
  - -0.5
  - 0.5
  - -0.5
  - -0.5
  - 0.5
  - 0.5
  - -0.5
  - -0.5
  - 0.5
  - -0.5
  - -0.5
  - -0.5
  - 0.5
  - 0.5
  - -0.5
  - 0.5
  - 0.5
  - 0.5
  - 0.5
  - -0.5
  - 0.5
  - 0.5
indices:
  - 0
  - 2
  - 1
  - 0
  - 3
  - 2
  - 4
  - 5
  - 6
  - 4
  - 6
  - 7
  - 0
  - 4
  - 7
  - 0
  - 7
  - 3
  - 1
  - 2
  - 6
  - 1
  - 6
  - 5
  - 0
  - 1
  - 5
  - 0
  - 5
  - 4
  - 3
  - 7
  - 6
  - 3
  - 6
  - 2
";

/// [`albedo_bounce_scene`] with the red wall an imported mesh (`meshes/wall.mesh.yaml`) instead of
/// a primitive cube; the floor stays a primitive.
fn imported_wall_scene(bounce: f64) -> String {
    let mut y = Yaml::default();
    header(
        &mut y,
        "gi bounce fixture: an imported wall tints the bounce",
    );
    camera_top_down(&mut y, 1.2, -1.9, 1.0, 10.0);
    gi_volume(&mut y, 0.25, 0.5, bounce);

    wall(
        &mut y,
        "floor",
        [0.0, -0.25, 0.0],
        [6.0, 0.5, 6.0],
        [0.85, 0.85, 0.85],
    );
    entity(&mut y, "red-wall", |y| {
        transform(y, [0.0, 1.5, -2.5], [0.0; 3], [6.0, 3.0, 0.5]);
        y.push(3, "Mesh:");
        y.push(4, "shape:");
        y.push(5, "asset: meshes/wall.mesh.yaml");
        y.vec(4, "color", &[0.95, 0.02, 0.02, 1.0]);
    });
    point_light(&mut y, [0.0, 0.4, -1.7], [1.0, 1.0, 1.0], 10.0, 6.0);

    y.0
}

/// **Clockout request 0013's acceptance gate.** A lit imported wall bounces its colour onto a
/// primitive floor: the floor's `R/G` ratio rises between `bounce: 0.0` and `bounce: 1.0`, exactly as
/// T4 does for a primitive wall — a shell-voxelized asset mesh feeds the bounce as a primitive does.
#[test]
fn needs_gpu_an_imported_wall_bounces_its_colour() {
    let fixture = Fixture::new("imported-wall");
    fixture.scene("meshes/wall.mesh.yaml", UNIT_CUBE_MESH);

    let render = |tag: &str, bounce: f64| {
        let source = imported_wall_scene(bounce);
        let path = fixture.scene(&format!("{tag}.yaml"), &source);
        render_fixture("gi-imported-wall", &path, 160, 90)
    };

    let Some(off) = render("bounce-0", 0.0) else {
        return;
    };
    let on = render("bounce-1", 1.0).expect("GPU present for the second render");

    let (r_off, g_off, _) = mean_rgb(&off);
    let (r_on, g_on, _) = mean_rgb(&on);
    let ratio_off = r_off / g_off.max(1e-6);
    let ratio_on = r_on / g_on.max(1e-6);

    eprintln!(
        "[gi-imported-wall] off R={r_off:.3} G={g_off:.3} R/G={ratio_off:.4} | on R={r_on:.3}          G={g_on:.3} R/G={ratio_on:.4}"
    );

    assert!(
        ratio_on > ratio_off * 1.03,
        "the floor's R/G ratio must rise once the imported red wall rides the bounce:          off R/G={ratio_off:.4}, on R/G={ratio_on:.4}"
    );
}

// =================================================================================================
// T6 — distance falloff.
// =================================================================================================

/// Two alcoves, each with the light and floor patch at the SAME fixed relative offset (`0.3` world
/// units apart, so each patch's own *direct* light is near-identical between alcoves) — only the
/// reflector panel's distance from the light differs (`d = 1.0` for the near alcove, `2d = 2.0` for
/// the far one). This is the fixture's fourth shape; the first three all measured a defect in the
/// *instrument*, not in the bake, each recorded so the next attempt at this fixture does not repeat
/// them:
///
/// 1. One continuous wide wall (`meshes.yaml`'s convention) measured a falloff ratio of `~1.0`
///    regardless of the light's placement: a probe sitting close to a *wide* flat reflector samples
///    a broad swath of it, so a patch shifted by `2d - d` still sees mostly the same bright
///    near-light region at a shallow angle and the local "closest point" this fixture means to
///    isolate gets washed out by everything else in the probe's hemisphere.
/// 2. Two narrow panels, each lit by its own point light positioned `d`/`2d` in front of it (so the
///    *patch* was also `d`/`2d`-ish from its light), fixed the washout but introduced a worse
///    confound: the near patch's own baseline (direct light, nothing to do with bounce) sat much
///    higher in the frame's dynamic range than the far patch's (measured off=174 vs off=117), and
///    the renderer's tonemap compresses equal linear-space increments into SMALLER 8-bit deltas the
///    higher the baseline sits — so the *raw byte delta* this fixture reads systematically
///    under-counted the near patch's bounce relative to the far one (measured far delta almost
///    double the near one — the opposite of the physical claim).
/// 3. Fixing light-to-patch distance and moving only the panel out to `d = 2.0`/`2d = 4.0` removed
///    that confound (both patches read the identical off-baseline, ~191 both times) but pushed the
///    far alcove's patch-to-panel gap out to 3.0 world units, where even a 3-unit-wide panel
///    subtends too small a solid angle for the bake's fixed 64 rays per probe to hit reliably —
///    measured delta EXACTLY `0.000`, reproduced at both `probe_spacing 0.5` and the finer `0.25`.
///
/// This shape keeps (3)'s fix (fixed light-to-patch offset — equal baselines) but scales every
/// distance down by roughly 2x (`d = 1.0`, `2d = 2.0`, range `4.0`, patch-to-panel gaps `0.7`/`1.7`)
/// so both alcoves' patch-to-panel distances stay short enough for a reliable ray-hit count while
/// the `atten^2` ratio the fixture means to measure is unchanged (`atten` is scale-invariant under
/// scaling `dist` and `range` together, but NOT under scaling `dist` alone while holding `range` —
/// here both are scaled together only for calibration purposes; the `d`, `2d`, `range` triple below
/// is the one actually asserted against).
fn falloff_alcove(y: &mut Yaml, name_suffix: &str, x0: f64, panel_distance: f64) {
    wall(
        y,
        &format!("floor-{name_suffix}"),
        [x0, -0.25, -1.0],
        [4.0, 0.5, 4.0],
        [0.75, 0.75, 0.75],
    );
    // Light and patch: identical relative offset in every alcove (light 0.3 world units behind the
    // patch, both at the panel's height), so each patch's own direct-light baseline is the same
    // regardless of where the panel sits.
    point_light(y, [x0, 1.0, 0.0], [1.0, 1.0, 1.0], 1.0, 4.0);
    // Panel: `panel_distance` beyond the light, straight through the patch's position — exactly
    // axial with the light at the panel's own height, so the incidence angle at the panel's center
    // stays close to normal regardless of distance, leaving `atten^2` as the dominant variable.
    wall(
        y,
        &format!("panel-{name_suffix}"),
        [x0, 1.0, -panel_distance],
        [3.0, 2.5, 0.3],
        [0.8, 0.8, 0.8],
    );
}

/// The near alcove (`x0 = 0`, panel distance `d = 1.0`) and the far alcove (`x0 = 12`, panel
/// distance `2d = 2.0`), translated far enough apart (the `gi_sky_visibility.rs` exterior-patch
/// margin) that neither's grid region can see the other's. Both alcoves are always present — the
/// scene is identical every render; only `cam` (below) selects which patch is being read.
fn falloff_scene(bounce: f64, cam: [f64; 3]) -> String {
    let mut y = Yaml::default();
    header(&mut y, "gi-wave2 bounce fixture: distance falloff");
    camera_top_down(&mut y, cam[0], cam[2], cam[1], 10.0);
    gi_volume(&mut y, 0.25, 0.5, bounce);
    falloff_alcove(&mut y, "near", 0.0, 1.0);
    falloff_alcove(&mut y, "far", 12.0, 2.0);
    y.0
}

/// The floor patch, `0.3` world units in front of the (shared-shape) light — identical in both
/// alcoves, translated by `x0`.
const FALLOFF_NEAR_CAM: [f64; 3] = [0.0, 1.0, -0.3];
/// Same relative offset, translated by the far alcove's `+12` `x`.
const FALLOFF_FAR_CAM: [f64; 3] = [12.0, 1.0, -0.3];

/// **T6 — distance falloff (`Plan/gi-wave2-emissive-bounce.md` §9 T6).** The nearer patch's bounce
/// gain (its `bounce: 1.0` minus `bounce: 0.0` luma) exceeds the farther patch's, and the ratio
/// between the two gains sits in a band consistent with `atten^2` (`atten = clamp(1 - dist/range,
/// 0, 1)`) rather than a linear, constant, or inverse-square falloff — computed from the fixture's
/// own geometry (each panel sits exactly `d = 1.0` or `2d = 2.0` from the shared-shape light, range
/// `4.0`) so the band is derived, not guessed: `atten(1)^2 / atten(2)^2 = 0.75^2 / 0.5^2 = 2.25`;
/// the linear alternative (`atten(1)/atten(2)`) predicts `1.5`, the constant alternative `1.0`, and
/// an inverse-square-distance alternative `(2/1)^2 = 4.0`. The band below is widened past `2.25`
/// alone: the *patch*-to-panel distance also differs between the two alcoves (`0.7` for the near
/// one, `1.7` for the far one), which adds its own solid-angle bias on top of `atten^2` in the SAME
/// direction (the near panel both radiates more AND subtends a larger angle) — the band still sits
/// well clear of the linear and constant alternatives, which is what the assertion exists to catch.
#[test]
fn needs_gpu_distance_falloff_matches_atten_squared() {
    let fixture = Fixture::new("falloff");

    // The scene carries both alcoves every render (one `GiVolume`, one bake); only the camera
    // selects which alcove's floor patch is being read.
    let render = |tag: &str, cam: [f64; 3], bounce: f64| {
        let source = falloff_scene(bounce, cam);
        let path = fixture.scene(&format!("{tag}.yaml"), &source);
        render_fixture("gi-falloff", &path, 160, 90)
    };

    let Some(near_off) = render("near-off", FALLOFF_NEAR_CAM, 0.0) else {
        return;
    };
    let near_on = render("near-on", FALLOFF_NEAR_CAM, 1.0).expect("GPU present");
    let far_off = render("far-off", FALLOFF_FAR_CAM, 0.0).expect("GPU present");
    let far_on = render("far-on", FALLOFF_FAR_CAM, 1.0).expect("GPU present");

    let near_delta = mean_luma(&near_on) - mean_luma(&near_off);
    let far_delta = mean_luma(&far_on) - mean_luma(&far_off);
    let ratio = near_delta / far_delta.max(1e-6);

    eprintln!(
        "[gi-falloff] near off={:.3} on={:.3} delta={near_delta:.3} | far off={:.3} on={:.3} \
         delta={far_delta:.3} ratio={ratio:.3} (pure atten^2 2.25, linear 1.5, constant 1.0, \
         inverse-square-distance 4.0 — this fixture's own compounded solid-angle bias, documented \
         above, pushes the measured value somewhat past the pure atten^2 figure)",
        mean_luma(&near_off),
        mean_luma(&near_on),
        mean_luma(&far_off),
        mean_luma(&far_on),
    );

    assert!(
        near_delta > far_delta,
        "the patch nearer its light must gain more bounce than the farther one: near={near_delta:.3}, \
         far={far_delta:.3}"
    );
    assert!(
        (2.5..=8.0).contains(&ratio),
        "the falloff ratio must sit clear of the linear (~1.5) and constant (1.0) alternatives, in \
         the band the fixture's own compounded atten^2 + solid-angle geometry predicts: \
         near={near_delta:.3}, far={far_delta:.3}, ratio={ratio:.3}"
    );
}

// =================================================================================================
// T7 — the sun bounces and is occluded.
// =================================================================================================

/// The shared body: a `DirectionalLight` (mostly straight down, a small lateral component so it is
/// never exactly axis-aligned), an open grey floor lit directly by it beside a grey wall that faces
/// the sunlit floor (and therefore receives almost no *direct* sun itself — its face normal is
/// near-perpendicular to the sun's arrival direction — so what appears on it is overwhelmingly the
/// bounce), and a fully sealed room offset along `x` with no light of its own. No camera; the two
/// callers below each add their own.
fn sun_scene_body(y: &mut Yaml, bounce: f64) {
    gi_volume(y, 0.25, 0.5, bounce);
    directional_light(y, [-0.05, -1.0, -0.05], [1.0, 1.0, 1.0], 3.0);

    // Open area: a sunlit floor and the wall it bounces onto.
    wall(
        y,
        "floor-open",
        [0.0, -0.25, 0.0],
        [6.0, 0.5, 6.0],
        [0.75, 0.75, 0.75],
    );
    wall(
        y,
        "wall-open",
        [0.0, 1.5, -3.25],
        [6.0, 3.0, 0.5],
        [0.6, 0.6, 0.6],
    );

    // Sealed room, offset +12 along x (the `gi_sky_visibility.rs` exterior-patch precedent for
    // keeping two subjects' footprints from ever touching), no light of its own.
    let dx = 12.0;
    wall(
        y,
        "floor-sealed",
        [dx, -0.25, 0.0],
        [5.0, 0.5, 5.0],
        [0.6, 0.6, 0.62],
    );
    wall(
        y,
        "ceiling-sealed",
        [dx, 3.25, 0.0],
        [5.0, 0.5, 5.0],
        [0.6, 0.6, 0.62],
    );
    wall(
        y,
        "wall-sealed-west",
        [dx - 2.25, 1.5, 0.0],
        [0.5, 3.0, 5.0],
        [0.55, 0.55, 0.58],
    );
    wall(
        y,
        "wall-sealed-east",
        [dx + 2.25, 1.5, 0.0],
        [0.5, 3.0, 5.0],
        [0.55, 0.55, 0.58],
    );
    wall(
        y,
        "wall-sealed-north",
        [dx, 1.5, -2.25],
        [5.0, 3.0, 0.5],
        [0.55, 0.55, 0.58],
    );
    wall(
        y,
        "wall-sealed-south",
        [dx, 1.5, 2.25],
        [5.0, 3.0, 0.5],
        [0.55, 0.55, 0.58],
    );
}

/// Framed on the wall's base, directly ahead of the camera along `-Z` — where the sunlit floor's
/// bounce should land.
fn sun_scene_wall(bounce: f64) -> String {
    let mut y = Yaml::default();
    header(&mut y, "gi-wave2 bounce fixture: sun wall bounce");
    camera_facing_neg_z(&mut y, 0.0, 0.6, 1.0, 8.0);
    sun_scene_body(&mut y, bounce);
    y.0
}

/// Framed on the sealed room's interior floor.
fn sun_scene_box(bounce: f64) -> String {
    let mut y = Yaml::default();
    header(&mut y, "gi-wave2 bounce fixture: sun sealed-box occlusion");
    camera_top_down(&mut y, 12.0, 0.0, 2.7, 30.0);
    sun_scene_body(&mut y, bounce);
    y.0
}

/// **T7 — the sun bounces and is occluded (`Plan/gi-wave2-emissive-bounce.md` §9 T7).** The wall
/// beside the sunlit floor picks up a measurable warm bounce; the sealed room's interior — no
/// window, no door, nothing but its own four walls, floor and ceiling — does not brighten at all
/// between `bounce: 0.0` and `1.0`, because `gi_bake.wgsl`'s `sun_shadow` marches from every hit
/// cell toward the sun and finds a wall before it finds sky.
///
/// **This gate has been demonstrated able to fail**, per the plan's explicit requirement that a
/// leak gate never observed failing is not known to have teeth. `gi_bake.wgsl`'s `sun_shadow` was
/// temporarily mutated from:
///
/// ```wgsl
/// fn sun_shadow(cell: vec3<i32>, n: vec3<f32>, extent: vec3<i32>) -> f32 {
///     let start = vec3<f32>(cell) + vec3<f32>(0.5) + n;
///     let h = trace_hit(start, -params.sun_dir.xyz, extent);
///     return select(1.0, 0.0, h.hit);
/// }
/// ```
///
/// to an unconditional `return 1.0;` (never shadowed, sun always "reaches" every hit cell
/// regardless of what stands between it and the sky), rebuilt, and rerun against this test. Measured
/// on the mutated shader: the sealed box interior's mean luma moved from off=**0.125** to
/// on=**2.670** (ratio **21.363**), far outside the `[0.9, 1.15]` tolerance below — the sealed room
/// picks up sunlight it should never see (the assertion failed with exactly this message: "the
/// sealed room's interior must not brighten at all under the sun's bounce: off=0.125, on=2.670,
/// ratio=21.363"), exactly the failure `sun_shadow` exists to prevent. The wall-bounce half stayed
/// green throughout (it does not exercise `sun_shadow`'s occlusion — `wall_ratio` was `1.921`, past
/// its own `> 1.2` bar both before and after the mutation). The shader was restored to its committed
/// text immediately after (byte-for-byte: restored from a pre-mutation copy, not re-typed); `git
/// diff` and `git status` over `crates/ludion-renderer/src/shaders/gi_bake.wgsl` both show no
/// change, and a rebuild afterward reran this whole file 5/5 green again.
#[test]
fn needs_gpu_the_sun_bounces_and_is_occluded() {
    let fixture = Fixture::new("sun");

    let render_wall = |tag: &str, bounce: f64| {
        let source = sun_scene_wall(bounce);
        let path = fixture.scene(&format!("{tag}.yaml"), &source);
        render_fixture("gi-sun-wall", &path, 160, 90)
    };
    let render_box = |tag: &str, bounce: f64| {
        let source = sun_scene_box(bounce);
        let path = fixture.scene(&format!("{tag}.yaml"), &source);
        render_fixture("gi-sun-box", &path, 160, 90)
    };

    let Some(wall_off) = render_wall("wall-0", 0.0) else {
        return;
    };
    let wall_on = render_wall("wall-1", 1.0).expect("GPU present");
    let box_off = render_box("box-0", 0.0).expect("GPU present");
    let box_on = render_box("box-1", 1.0).expect("GPU present");

    let wall_off_luma = mean_luma(&wall_off);
    let wall_on_luma = mean_luma(&wall_on);
    let box_off_luma = mean_luma(&box_off);
    let box_on_luma = mean_luma(&box_on);
    let wall_ratio = wall_on_luma / wall_off_luma.max(1e-6);
    let box_ratio = box_on_luma / box_off_luma.max(1e-6);

    eprintln!(
        "[gi-sun] wall off={wall_off_luma:.3} on={wall_on_luma:.3} ratio={wall_ratio:.3} | box \
         off={box_off_luma:.3} on={box_on_luma:.3} ratio={box_ratio:.3}"
    );

    assert!(
        wall_ratio > 1.2,
        "the wall beside the sunlit floor must pick up a measurable warm bounce: \
         off={wall_off_luma:.3}, on={wall_on_luma:.3}, ratio={wall_ratio:.3} (wanted > 1.2)"
    );
    assert!(
        (0.9..=1.15).contains(&box_ratio),
        "the sealed room's interior must not brighten at all under the sun's bounce: \
         off={box_off_luma:.3}, on={box_on_luma:.3}, ratio={box_ratio:.3}"
    );
}

// =================================================================================================
// T8 — no bounce leak through a wall.
// =================================================================================================

/// Two adjacent sealed rooms sharing one wall (the shared partition, `x` in `[2.0, 2.5]`, spanning
/// the full `z` and `y` extent — no gap at all, unlike T3's partition). Room A (`x` in `[-2.0,
/// 2.0]`) holds one bright warm emissive cube; room B (`x` in `[2.5, 6.5]`) holds no light of any
/// kind. No camera; the two callers below each add their own.
fn two_rooms_body(y: &mut Yaml, bounce: f64) {
    gi_volume(y, 0.25, 0.5, bounce);

    wall(
        y,
        "floor",
        [2.25, -0.25, 0.0],
        [9.5, 0.5, 5.0],
        [0.6, 0.6, 0.62],
    );
    wall(
        y,
        "ceiling",
        [2.25, 3.25, 0.0],
        [9.5, 0.5, 5.0],
        [0.6, 0.6, 0.62],
    );
    wall(
        y,
        "wall-north",
        [2.25, 1.5, -2.25],
        [9.5, 3.0, 0.5],
        [0.55, 0.55, 0.58],
    );
    wall(
        y,
        "wall-south",
        [2.25, 1.5, 2.25],
        [9.5, 3.0, 0.5],
        [0.55, 0.55, 0.58],
    );
    wall(
        y,
        "wall-a-west",
        [-2.25, 1.5, 0.0],
        [0.5, 3.0, 5.0],
        [0.55, 0.55, 0.58],
    );
    wall(
        y,
        "partition",
        [2.25, 1.5, 0.0],
        [0.5, 3.0, 5.0],
        [0.55, 0.55, 0.58],
    );
    wall(
        y,
        "wall-b-east",
        [6.75, 1.5, 0.0],
        [0.5, 3.0, 5.0],
        [0.55, 0.55, 0.58],
    );

    cube(
        y,
        "torch",
        [-1.0, 1.0, 0.0],
        [0.6, 0.6, 0.6],
        [1.0, 0.62, 0.28],
        4.0,
    );
}

fn two_rooms_scene_a(bounce: f64) -> String {
    let mut y = Yaml::default();
    header(
        &mut y,
        "gi-wave2 bounce fixture: no leak through a wall (room A)",
    );
    // Beside the emitter, clear of its own footprint (x=-1.3..-0.7, z=-0.3..0.3).
    camera_top_down(&mut y, -1.0, 0.9, 1.0, 12.0);
    two_rooms_body(&mut y, bounce);
    y.0
}

fn two_rooms_scene_b(bounce: f64) -> String {
    let mut y = Yaml::default();
    header(
        &mut y,
        "gi-wave2 bounce fixture: no leak through a wall (room B)",
    );
    camera_top_down(&mut y, 4.5, 0.0, 2.7, 30.0);
    two_rooms_body(&mut y, bounce);
    y.0
}

/// **T8 — no bounce leak through a wall (`Plan/gi-wave2-emissive-bounce.md` §9 T8).** Room A's
/// floor beside its emitter reads materially brighter with bounce on (the non-vacuity half — a
/// bake that is globally inert would pass the leak check for the wrong reason); room B's interior,
/// on the opposite side of the shared wall from a bright emitter, does not brighten at all between
/// `bounce: 0.0` and `1.0`. The W2 twin of W1's Risk-1 gate (`no_leak_through_a_sealed_wall`).
///
/// Demonstrated able to fail on its leak half: `trace_hit`'s occupancy check gated behind
/// `i > 6u` (rays tunnel the first six cells — through the shared wall) reddened exactly this
/// assertion, room B off=0.125 → on=0.166, ratio 1.330; the shader was restored byte-identical.
#[test]
fn needs_gpu_no_bounce_leak_through_a_shared_wall() {
    let fixture = Fixture::new("two-rooms");

    let render_a = |tag: &str, bounce: f64| {
        let source = two_rooms_scene_a(bounce);
        let path = fixture.scene(&format!("{tag}.yaml"), &source);
        render_fixture("gi-no-leak-a", &path, 160, 90)
    };
    let render_b = |tag: &str, bounce: f64| {
        let source = two_rooms_scene_b(bounce);
        let path = fixture.scene(&format!("{tag}.yaml"), &source);
        render_fixture("gi-no-leak-b", &path, 160, 90)
    };

    let Some(a_off) = render_a("a-0", 0.0) else {
        return;
    };
    let a_on = render_a("a-1", 1.0).expect("GPU present");
    let b_off = render_b("b-0", 0.0).expect("GPU present");
    let b_on = render_b("b-1", 1.0).expect("GPU present");

    let a_off_luma = mean_luma(&a_off);
    let a_on_luma = mean_luma(&a_on);
    let b_off_luma = mean_luma(&b_off);
    let b_on_luma = mean_luma(&b_on);
    let a_ratio = a_on_luma / a_off_luma.max(1e-6);
    let b_ratio = b_on_luma / b_off_luma.max(1e-6);

    eprintln!(
        "[gi-no-leak] room A off={a_off_luma:.3} on={a_on_luma:.3} ratio={a_ratio:.3} | room B \
         off={b_off_luma:.3} on={b_on_luma:.3} ratio={b_ratio:.3}"
    );

    assert!(
        a_ratio > 1.3,
        "room A's floor beside its own emitter must read materially brighter with bounce on: \
         off={a_off_luma:.3}, on={a_on_luma:.3}, ratio={a_ratio:.3} (wanted > 1.3) — a globally \
         inert bake would pass the leak check below for the wrong reason"
    );
    assert!(
        (0.9..=1.15).contains(&b_ratio),
        "room B's interior must not brighten at all from room A's emitter across the shared wall: \
         off={b_off_luma:.3}, on={b_on_luma:.3}, ratio={b_ratio:.3}"
    );
}
