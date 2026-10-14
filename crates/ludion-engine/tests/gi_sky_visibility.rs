//! GI Wave 1 physics fixtures (`Plan/gi-wave1-sky-visibility.md` §11 tests 2-5): self-contained
//! GPU-rendered proofs that baked sky visibility darkens interiors while leaving open sky alone,
//! respects a doorway, does not leak through a sealed wall, and leaves direct/emissive light
//! untouched. Tests 1, 7 and 8 are committed elsewhere (`gpu_render.rs`, `voxelizer.rs`,
//! `components.rs`); test 6 (cross-process bake stability) lives in `ludion-cli::render_gi`, where
//! `CARGO_BIN_EXE_ludion` hands it a cargo-managed binary — spawning `cargo build` from inside a
//! test here raced the parallel suite for the `ludion.exe` link target.
//!
//! Every fixture is authored **in this file** as a YAML string, written to a scratch scene — no
//! dependency on `examples/gallery/gi-interior.yaml` (a different work package building in
//! parallel). Skips gracefully (printing why) when no GPU adapter is available, matching
//! `gpu_render.rs`'s established style.
//!
//! Calibration follows the plan's guidance: a **bright** `Environment` sky and no direct light in
//! the ambient-comparison fixtures (tests 2-4), so the sky-visibility delta sits well above 8-bit
//! quantization noise. Every temp scene lives under its own uniquely tagged directory (test name +
//! process id + a monotonic counter) — a shared temp path between two tests in one binary caused an
//! intermittent P0 flake in `model_sheet`; this file never repeats that mistake.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ludion_core::diagnostics::Diagnostic;
use ludion_engine::{FrameSize, RenderOutcome, render_scene};

// ---------------------------------------------------------------------------------------------
// GPU skip machinery (mirrors `gpu_render.rs`'s `render_or_skip`, reimplemented here because
// integration test binaries share no module).
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
// Unique-per-call scratch directories. The flake this guards against: two tests in this binary
// run as threads in one process, so tagging a temp path by process id alone (as an earlier
// `model_sheet` fixture did) is not unique — two threads racing the same path intermittently
// stepped on each other's scene file. A monotonic counter makes every call's directory distinct
// regardless of thread interleaving.
// ---------------------------------------------------------------------------------------------

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A scratch directory, unique per instance, removed on drop (including when a mid-test assertion
/// panics).
struct Fixture(PathBuf);

impl Fixture {
    fn new(tag: &str) -> Self {
        let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("ludion-gi-sky-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create fixture directory");
        Fixture(dir)
    }

    /// Writes `source` as `name` inside this fixture's directory and returns its path.
    fn scene(&self, name: &str, source: &str) -> PathBuf {
        let path = self.0.join(name);
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
// A tiny block-YAML writer. Ludion's scene grammar is a *strict subset* of YAML that forbids
// non-empty flow collections (`L1005`) — every array must be a block sequence, exactly the shape
// every gallery fixture in this repo already uses. This keeps fixture assembly declarative
// instead of hand-aligning `format!` strings with the wrong indentation.
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

/// A matte `Mesh` cube entity — every wall, floor and ceiling in these fixtures. Matte (default
/// `roughness: 1.0`, default `metallic: 0.0`) so the measured luminance is ambient/GI, not a
/// specular highlight riding along for the ride.
fn wall(y: &mut Yaml, name: &str, t: [f64; 3], s: [f64; 3], color: [f64; 3]) {
    entity(y, name, |y| {
        transform(y, t, [0.0; 3], s);
        y.push(3, "Mesh:");
        y.push(4, "shape: cube");
        y.vec(4, "color", &[color[0], color[1], color[2], 1.0]);
    });
}

/// A camera looking straight down (`rotation.x = -90`, so its forward is exactly `(0, -1, 0)` —
/// `build_camera` builds the view matrix from the transform's inverse, not `look_at`, so there is
/// no degenerate-up-vector case to dodge) at `(x, z)` from `height` above the floor. Used to frame
/// one floor patch as (nearly) the whole image, so a fixture's assertions read a full-frame mean
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

/// A forward-looking camera, angled slightly down (the `meshes.yaml` convention) — used by the
/// untouched-by-GI fixtures, where framing precision does not matter (the assertion is whole-image
/// byte equality between two `sky_visibility` settings of the *same* framing, not a luminance
/// measurement).
fn camera_forward(y: &mut Yaml, t: [f64; 3]) {
    entity(y, "camera", |y| {
        transform(y, t, [-8.0, 0.0, 0.0], [1.0; 3]);
        y.push(3, "Camera:");
        y.push(4, "projection:");
        y.push(5, "type: perspective");
        y.push(5, "fov_y_deg: 45.0");
    });
}

/// A bright sky `Environment` — the plan's calibration guidance: the ambient term GI attenuates is
/// small in a dark scene (a durable finding: SSAO was structurally invisible at night), so these
/// fixtures light the sky hard enough that the sky-visibility delta sits well above 8-bit noise.
fn bright_environment(y: &mut Yaml, sky: [f64; 3], ibl_intensity: f64) {
    entity(y, "environment", |y| {
        y.push(3, "Environment:");
        y.vec(4, "sky_zenith", &sky);
        y.vec(4, "sky_horizon", &sky);
        y.vec(4, "sky_ground", &[sky[0] * 0.3, sky[1] * 0.3, sky[2] * 0.3]);
        y.push(4, format!("ibl_intensity: {ibl_intensity}"));
    });
}

/// An `Environment` whose ambient/IBL term contributes **exactly zero** to every pixel:
/// `acc = (diffuse_ibl + specular_ibl) * ibl_intensity`, and `0.0 * finite = 0.0` in IEEE-754
/// regardless of what the sky or the bake computed. This is `render.rs`'s own established
/// isolation technique (`a_zero_sky_blend_never_reads_the_environments_colours`,
/// `without_an_environment_the_sky_blend_does_not_exist`): the route the fixture wants to *remove*
/// is severed at its coefficient, not by hoping every sampled point reads a coincidentally-neutral
/// baked value.
fn zero_ambient_environment(y: &mut Yaml) {
    entity(y, "environment", |y| {
        y.push(3, "Environment:");
        y.push(4, "ibl_intensity: 0.0");
    });
}

fn gi_volume(y: &mut Yaml, cell_size: f64, probe_spacing: f64, sky_visibility: f64) {
    entity(y, "gi", |y| {
        y.push(3, "GiVolume:");
        y.push(4, format!("cell_size: {cell_size}"));
        y.push(4, format!("probe_spacing: {probe_spacing}"));
        y.push(4, format!("sky_visibility: {sky_visibility}"));
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
/// unit. Every fixture below frames a single floor patch as (nearly) the entire image, so a
/// whole-frame mean stands in for a region measurement without pixel-rectangle bookkeeping.
fn mean_luma(outcome: &RenderOutcome) -> f64 {
    let rgba = &outcome.image.rgba;
    let pixels = (rgba.len() / 4) as f64;
    let sum: f64 = rgba
        .chunks_exact(4)
        .map(|p| 0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]))
        .sum();
    sum / pixels
}

/// The probe lattice a `GiVolume` sized, or `None` when the scene builds no voxel grid at all —
/// the non-vacuity guard `gpu_render.rs`'s identity-value test uses, reimplemented here (a
/// scratch-directory helper the same shape as that file's `gi_lattice`, since integration test
/// binaries share no module).
fn gi_lattice(source: &str, dir: &Path) -> Option<[u32; 3]> {
    let registry = ludion_engine::builtin_registry();
    let checked = ludion_scene::check(source, &registry).expect("the fixture scene validates");
    let world = ludion_scene::instantiate(&checked, &registry).expect("the fixture instantiates");
    let extracted =
        ludion_engine::extract(&world, (160, 90), dir).expect("extract succeeds on the fixture");
    extracted.voxel.as_ref()?;
    extracted.gi.as_ref().map(|gi| gi.count)
}

// ---------------------------------------------------------------------------------------------
// Fixture: a sealed room (four walls, floor, ceiling, no opening at all) plus an unrelated
// exterior floor patch far away, unobstructed above. Shared by tests 2 and 4, which both need a
// fully enclosed interior and a way to vary either the camera spot or the sky's brightness while
// keeping the same bake. Room interior clear space is 4x3x4 world units (walls 0.5 thick); the
// exterior patch sits 12 units away so its own bounds and the room's never touch.
// ---------------------------------------------------------------------------------------------

fn sealed_room_scene(
    sky: [f64; 3],
    ibl_intensity: f64,
    sky_visibility: f64,
    cam: [f64; 3],
    fov_y_deg: f64,
    probe_spacing: f64,
) -> String {
    let mut y = Yaml::default();
    header(&mut y, "gi-sky-visibility fixture: sealed room");
    camera_top_down(&mut y, cam[0], cam[2], cam[1], fov_y_deg);
    bright_environment(&mut y, sky, ibl_intensity);
    gi_volume(&mut y, 0.25, probe_spacing, sky_visibility);

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

    // Unobstructed exterior patch, far enough away that its own footprint never overlaps the
    // room's — the "open sky" surface tests 2 and 4 hold up against.
    wall(
        &mut y,
        "exterior-floor",
        [12.0, -0.25, 0.0],
        [8.0, 0.5, 8.0],
        [0.6, 0.6, 0.62],
    );

    y.0
}

/// Camera pose that frames the room's interior floor, centred, as (nearly) the whole image:
/// straight down from just under the ceiling. At `height = 2.7` and `fov_y_deg = 30`, the
/// half-footprint is `2.7 * tan(15deg) ~= 0.72` (vertical) and `~1.29` (horizontal, at 160:90) —
/// comfortably inside the room's 4-unit interior clearance (walls' inner faces at +-2.0), so every
/// pixel is floor, no wall or ceiling intrudes.
const INTERIOR_CAM: [f64; 3] = [0.0, 2.7, 0.0];

/// Camera pose that frames the exterior floor patch, centred, as (nearly) the whole image.
const EXTERIOR_CAM: [f64; 3] = [12.0, 5.0, 0.0];

/// Camera pose that frames a tight patch of floor just inside the room's west wall — the
/// leak-gate test's sample point (see [`no_leak_through_a_sealed_wall`]), close enough to the wall
/// that trilinear interpolation still weights the wall-embedded probe there.
const NO_LEAK_CAM: [f64; 3] = [-1.75, 0.6, 0.0];

/// **Test 2 — the interior darkens while the exterior holds
/// (`Plan/gi-wave1-sky-visibility.md` §11).** On the sealed-room fixture, the interior floor's
/// mean luminance is materially lower at `sky_visibility: 1.0` than at `0.0`, while the unrelated
/// exterior floor patch — open to the sky, nothing between it and every ray a probe there casts —
/// holds within a small tolerance. Both halves are asserted: a bake that darkened *everything*
/// (an unbound texture read as zero, a sign flipped in the mix) would pass the first half alone.
#[test]
fn needs_gpu_the_interior_darkens_while_the_exterior_holds() {
    let fixture = Fixture::new("interior-exterior");
    let sky = [2.4, 2.5, 2.7];

    let render = |tag: &str, cam: [f64; 3], sky_visibility: f64| {
        let source = sealed_room_scene(sky, 1.8, sky_visibility, cam, 30.0, 1.0);
        let path = fixture.scene(&format!("{tag}.yaml"), &source);
        render_fixture("gi-interior-exterior", &path, 160, 90).map(|o| (o, source))
    };

    let Some((interior_off, source_off)) = render("interior-0", INTERIOR_CAM, 0.0) else {
        return;
    };
    let (interior_on, source_on) = render("interior-1", INTERIOR_CAM, 1.0).expect("GPU present");
    let (exterior_off, _) = render("exterior-0", EXTERIOR_CAM, 0.0).expect("GPU present");
    let (exterior_on, _) = render("exterior-1", EXTERIOR_CAM, 1.0).expect("GPU present");

    // Non-vacuity: the bake actually built a real probe lattice (not the 1x1x1 fallback), on both
    // authored strengths — the same grid either way, since `sky_visibility` never changes the
    // voxelization signature.
    let count = gi_lattice(&source_off, fixture.0.as_path()).expect("the fixture bakes a lattice");
    assert!(
        count.iter().all(|&c| c >= 2) && count.iter().product::<u32>() > 8,
        "the probe lattice must be a real volume, not the fallback; it was {count:?}"
    );
    let _ = &source_on; // same geometry, same lattice; kept for symmetry with the render closure

    let interior_off_luma = mean_luma(&interior_off);
    let interior_on_luma = mean_luma(&interior_on);
    let exterior_off_luma = mean_luma(&exterior_off);
    let exterior_on_luma = mean_luma(&exterior_on);

    eprintln!(
        "[gi-interior-exterior] interior off={interior_off_luma:.3} on={interior_on_luma:.3} \
         exterior off={exterior_off_luma:.3} on={exterior_on_luma:.3}"
    );

    assert!(
        interior_on_luma < interior_off_luma * 0.5,
        "the sealed room's interior must darken materially under sky visibility: off={interior_off_luma:.3}, \
         on={interior_on_luma:.3} (wanted on < 50% of off)"
    );
    let exterior_ratio = exterior_on_luma / exterior_off_luma.max(1e-6);
    assert!(
        (0.9..=1.1).contains(&exterior_ratio),
        "the open exterior surface must hold within a small tolerance under sky visibility: \
         off={exterior_off_luma:.3}, on={exterior_on_luma:.3}, ratio={exterior_ratio:.3}"
    );
}

/// **Test 4 — no leak through a wall, the Risk-1 gate
/// (`Plan/gi-wave1-sky-visibility.md` §11).** The sealed room has no opening at all, so a floor
/// sample just inside its west wall — close enough that trilinear interpolation still weights the
/// probe sitting *in* that wall (the lattice, aligned to the grid's own origin, lands a probe
/// exactly on the wall's outer face) — must read near-darkness, and raising the *exterior* sky's
/// brightness by 4x must not brighten it: every ray a nearby probe casts has to hit a wall before
/// it can find that exterior brightness at all.
///
/// **This gate has been demonstrated able to fail**, per the plan's explicit requirement that a
/// leak gate never observed failing is not known to have teeth. `gi_bake.wgsl`'s `push_out` (the
/// probe-in-solid push-out, the 3^3->5^3 nearest-free-cell scan) was temporarily mutated to a
/// no-op that claims success unconditionally without ever checking whether the cell it names is
/// actually free or even in bounds (`*free = cell + vec3<i32>(-1, -1, -1); return true;` as its
/// first two statements, the rest of the function made dead code by the early return). Note that a
/// no-op which *disables* push-out (`return false;`, always falling through to the buried-probe
/// darkness fallback) is not the demonstration: `trace()`'s very first loop iteration already
/// returns `0.0` for a start whose own cell is occupied, before it ever looks at ray direction, so
/// every buried probe already darkens correctly with push-out removed outright — the fallback
/// darkness and "push-out never ran" are the same bit pattern. The mutation that has teeth is the
/// one above: it hands `bake()` a `start` that can land *outside the grid* (unchecked bounds), so
/// roughly half of a buried probe's 64 rays exit the box immediately and read sky. Rerunning this
/// test against that mutation reddened both assertions below (dim=158.003, bright=214.733 — far off
/// the near-darkness baseline of 0.125 this same fixture reads under the committed shader, and the
/// ~57 unit swing between them blew past the delta tolerance too). The shader was restored to its
/// committed text immediately after — `git diff` over
/// `crates/ludion-renderer/src/shaders/gi_bake.wgsl` shows no change.
#[test]
fn needs_gpu_no_leak_through_a_sealed_wall() {
    let fixture = Fixture::new("no-leak");
    let dim_sky = [0.6, 0.65, 0.7];
    let bright_sky = [2.4, 2.6, 3.0];

    let render = |tag: &str, sky: [f64; 3]| {
        let source = sealed_room_scene(sky, 1.8, 1.0, NO_LEAK_CAM, 10.0, 1.0);
        let path = fixture.scene(&format!("{tag}.yaml"), &source);
        render_fixture("gi-no-leak", &path, 160, 90)
    };

    let Some(dim) = render("dim-sky", dim_sky) else {
        return;
    };
    let bright = render("bright-sky", bright_sky).expect("GPU present for the second render");

    let dim_luma = mean_luma(&dim);
    let bright_luma = mean_luma(&bright);
    eprintln!(
        "[gi-no-leak] dim-exterior interior luma={dim_luma:.3} bright-exterior interior luma={bright_luma:.3}"
    );

    // "At or near zero": well below a fully-exposed patch under this Environment (the exterior
    // measurement in the interior/exterior test reads comfortably above 40 at this tonemap/
    // exposure setting), so a real leak would be unmistakable against this ceiling.
    assert!(
        dim_luma < 12.0 && bright_luma < 12.0,
        "a fully sealed room's interior must read near-darkness regardless of exterior sky \
         brightness: dim={dim_luma:.3}, bright={bright_luma:.3}"
    );
    let delta = (bright_luma - dim_luma).abs();
    assert!(
        delta < 3.0,
        "raising the exterior sky's brightness ~4x must not measurably brighten a sealed \
         interior: dim={dim_luma:.3}, bright={bright_luma:.3}, delta={delta:.3}"
    );
}

// ---------------------------------------------------------------------------------------------
// Fixture: a room with a doorway gap in its north wall (test 3).
// ---------------------------------------------------------------------------------------------

/// Same footprint as [`sealed_room_scene`]'s room, but the north wall is split into two segments
/// leaving a 1.5-unit gap centred at `x = 0` — a doorway with no lintel (open to the room's own
/// ceiling), and nothing placed beyond it, so a ray that exits through the gap leaves the grid and
/// reads sky.
fn doorway_room_scene(sky_visibility: f64, cam: [f64; 3], fov_y_deg: f64) -> String {
    let mut y = Yaml::default();
    header(&mut y, "gi-sky-visibility fixture: doorway room");
    camera_top_down(&mut y, cam[0], cam[2], cam[1], fov_y_deg);
    bright_environment(&mut y, [2.4, 2.5, 2.7], 1.8);
    gi_volume(&mut y, 0.25, 0.5, sky_visibility);

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
        "wall-south",
        [0.0, 1.5, 2.25],
        [5.0, 3.0, 0.5],
        [0.55, 0.55, 0.58],
    );
    // North wall, split around a 1.5-unit-wide doorway centred at x=0 (gap: -0.75..0.75).
    wall(
        &mut y,
        "wall-north-left",
        [-1.625, 1.5, -2.25],
        [1.75, 3.0, 0.5],
        [0.55, 0.55, 0.58],
    );
    wall(
        &mut y,
        "wall-north-right",
        [1.625, 1.5, -2.25],
        [1.75, 3.0, 0.5],
        [0.55, 0.55, 0.58],
    );

    y.0
}

/// **Test 3 — the doorway wedge (`Plan/gi-wave1-sky-visibility.md` §11).** The floor just inside
/// the doorway reads measurably brighter than the room's back corner, asserted as a **ratio** —
/// per the plan, so the claim survives an exposure retune rather than pinning absolute luma
/// values.
#[test]
fn needs_gpu_the_doorway_wedge_is_a_ratio_that_survives_exposure() {
    let fixture = Fixture::new("doorway-wedge");
    let fov = 16.0;

    let render = |tag: &str, x: f64, z: f64| {
        let source = doorway_room_scene(1.0, [x, 1.0, z], fov);
        let path = fixture.scene(&format!("{tag}.yaml"), &source);
        render_fixture("gi-doorway-wedge", &path, 160, 90)
    };

    // Just inside the doorway (door gap is x in -0.75..0.75, at z = -2.25).
    let Some(near_door) = render("near-door", 0.0, -1.3) else {
        return;
    };
    // The opposite corner, doubly shielded by two solid walls.
    let back_corner = render("back-corner", -1.3, 1.3).expect("GPU present for the second render");

    let door_luma = mean_luma(&near_door);
    let corner_luma = mean_luma(&back_corner);
    let ratio = door_luma / corner_luma.max(1e-6);
    eprintln!("[gi-doorway-wedge] door={door_luma:.3} corner={corner_luma:.3} ratio={ratio:.3}");

    assert!(
        ratio > 1.5,
        "the floor just inside the doorway must read measurably brighter than the room's back \
         corner: door={door_luma:.3}, corner={corner_luma:.3}, ratio={ratio:.3} (wanted > 1.5)"
    );
}

// ---------------------------------------------------------------------------------------------
// Fixture: an emissive-only subject and a point-lit subject, both under an `Environment` whose
// `ibl_intensity` is zeroed — the ambient/IBL term GI multiplies contributes exactly nothing, so
// flipping `sky_visibility` cannot move a single byte unless the code touches direct light or
// emissive too.
// ---------------------------------------------------------------------------------------------

fn untouched_scene(sky_visibility: f64, emissive: bool) -> String {
    let mut y = Yaml::default();
    header(
        &mut y,
        if emissive {
            "gi-sky-visibility fixture: emissive untouched"
        } else {
            "gi-sky-visibility fixture: point-lit untouched"
        },
    );
    camera_forward(&mut y, [0.0, 1.6, 5.0]);
    zero_ambient_environment(&mut y);
    gi_volume(&mut y, 0.25, 1.0, sky_visibility);

    wall(
        &mut y,
        "floor",
        [0.0, -0.25, 0.0],
        [6.0, 0.5, 6.0],
        [0.5, 0.5, 0.5],
    );
    wall(
        &mut y,
        "occluder",
        [-2.0, 1.0, 0.0],
        [1.0, 2.0, 1.0],
        [0.5, 0.5, 0.5],
    );

    if emissive {
        entity(&mut y, "subject", |y| {
            transform(y, [0.0, 1.0, 0.0], [0.0, 0.0, 0.0], [1.2, 1.2, 1.2]);
            y.push(3, "Mesh:");
            y.push(4, "shape: cube");
            y.vec(4, "color", &[1.0, 0.7, 0.3, 1.0]);
            y.push(4, "emissive: 4.0");
        });
    } else {
        entity(&mut y, "subject", |y| {
            transform(y, [0.0, 1.0, 0.0], [0.0, 0.0, 0.0], [1.2, 1.2, 1.2]);
            y.push(3, "Mesh:");
            y.push(4, "shape: cube");
            y.vec(4, "color", &[0.8, 0.8, 0.85, 1.0]);
        });
        point_light(&mut y, [1.5, 2.5, 3.0], [1.0, 0.95, 0.85], 6.0, 15.0);
    }

    y.0
}

/// **Test 5 — emissive and direct light untouched (`Plan/gi-wave1-sky-visibility.md` §11).**
/// Whole-image **byte** equality of an emissive-only fixture, and separately a point-lit fixture,
/// rendered at `sky_visibility: 0.0` and `1.0` — the `render.rs` `ibl_intensity: 0` isolation
/// technique, so the only route GI has into the image is severed at its coefficient rather than
/// hoped away by geometry. `shade()` multiplies only the ambient/IBL accumulator by the GI factor;
/// emissive is added outside `shade()` entirely and direct light is summed into `acc` afterward —
/// so with the ambient term pinned to exactly zero, nothing left for `sky_visibility` to move.
#[test]
fn needs_gpu_emissive_and_direct_light_are_untouched_by_sky_visibility() {
    let fixture = Fixture::new("untouched");

    let render = |tag: &str, emissive: bool, sky_visibility: f64| {
        let source = untouched_scene(sky_visibility, emissive);
        let path = fixture.scene(&format!("{tag}.yaml"), &source);
        render_fixture("gi-untouched", &path, 160, 90)
    };

    let Some(emissive_off) = render("emissive-0", true, 0.0) else {
        return;
    };
    let emissive_on = render("emissive-1", true, 1.0).expect("GPU present");
    assert_eq!(
        emissive_off.image.rgba, emissive_on.image.rgba,
        "an emissive-only fixture must render byte-identically at sky_visibility 0.0 and 1.0"
    );

    let lit_off = render("lit-0", false, 0.0).expect("GPU present");
    let lit_on = render("lit-1", false, 1.0).expect("GPU present");
    assert_eq!(
        lit_off.image.rgba, lit_on.image.rgba,
        "a point-lit fixture must render byte-identically at sky_visibility 0.0 and 1.0"
    );

    // Non-vacuity: both fixtures actually show something (not an all-black frame that would
    // satisfy byte-equality by having nothing to move in the first place).
    for (name, outcome) in [("emissive", &emissive_off), ("point-lit", &lit_off)] {
        let first = &outcome.image.rgba[0..4];
        assert!(
            outcome.image.rgba.chunks_exact(4).any(|px| px != first),
            "{name} fixture rendered a single flat color — likely an all-black regression"
        );
    }
}
