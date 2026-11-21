//! GI Wave 3′ dynamic-injection physics fixtures (`Plan/gi-wave3-dynamic-injection.md` §11 T4, T6,
//! T7 — WP-9): self-contained GPU-rendered proofs that the per-frame injection lights only the room
//! its marked light sits in (T4), that the injected term's energy agrees with the static bake's own
//! estimator of the same physical light (T6), and that the injected term never steps as a marked
//! light crosses a bin-cell boundary (T7). Gates A–C (T1–T3), the double-count gate (T5), the bin
//! oracle (T8) and the mesh-side twin (T13) are committed elsewhere, in
//! `crates/ludion-renderer/src/gi_inject.rs`'s own test module and `crates/ludion-renderer/src/
//! gi_bake.rs`'s.
//!
//! Two distinct fixture idioms, one per altitude:
//!
//! - **T4 is engine-level** (extract → render): a YAML scene on disk, driven through
//!   `render_scene_with_registry` at two [`Quality`] settings — the block-YAML writer and the
//!   sealed-two-rooms leak geometry are `gi_bounce.rs`'s own machinery, restated here because
//!   integration test binaries share no module (that file's own header states the same rule).
//! - **T6 and T7 are renderer-level**: a [`RenderScene`] built directly (no YAML, no `extract`),
//!   mirroring `ludion-renderer::gi_inject`'s own test module's `scene_over`/`room_grid` idiom —
//!   restated here because, beyond sharing no module, those items are `pub(crate)` to
//!   `ludion-renderer` and unreachable across the crate boundary from an integration test. Only
//!   `ludion_renderer`'s public surface is used: [`RenderScene`], [`WorldVoxelGrid`], [`GiData`],
//!   [`PointLightData`], [`MeshInstance`], [`Renderer`], [`Quality`].
//!
//! **The isolation technique both T6 and T7 share.** Neither test has access to the raw probe
//! buffer the injection writes (that readback is `pub(crate)` too — WP-8's `--dump-gi` instrument is
//! a different work package). Both instead isolate the injected term by **subtraction**: rendering
//! the identical scene at [`Quality::FULL`] and at a quality that turns `gi_dynamic` off alone
//! differs by exactly the injection's own contribution (Gate B/C's proven identity — the switch
//! changes nothing else a pixel can see), so `mean_luma(on) - mean_luma(off)` is the injected term's
//! effect with the scene's direct light, ambient and every other term cancelled exactly. T6 uses the
//! same trick on the *static* side (`bounce: 0.0` vs `bounce: 1.0`, quality held at `FULL`) so both
//! halves of the energy comparison are measured the same way.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ludion_core::diagnostics::Diagnostic;
use ludion_core::math::glam::{Mat3, Mat4, Vec3};
use ludion_ecs::Registry;
use ludion_engine::{
    FrameSize, Quality, QualityParams, QualitySetting, RenderOutcome, builtin_registry,
    render_scene_with_registry,
};
use ludion_renderer::{
    GiData, MeshInstance, MeshShape, PointLightData, RenderCamera, RenderScene, RenderedImage,
    Renderer, WorldVoxelGrid, aspect_ratio,
};

// =================================================================================================
// GPU skip machinery — `gi_bounce.rs`'s and `gi_inject.rs`'s own idioms, restated.
// =================================================================================================

fn no_adapter(errors: &[Diagnostic]) -> bool {
    ludion_core::require_gpu::is_no_adapter(errors)
}

/// Renders `scene` at `path` through the lockstep engine path, or `None` (after printing a skip
/// reason) when there is no GPU.
fn render_or_skip(
    test: &str,
    registry: &Registry,
    path: &Path,
    size: FrameSize,
    quality: Quality,
) -> Option<RenderOutcome> {
    // The parameters stay at FULL: what this fixture varies is which effects run, never how hard.
    let setting = QualitySetting {
        quality,
        params: QualityParams::FULL,
        label: QualitySetting::CUSTOM_LABEL.to_string(),
    };
    match render_scene_with_registry(
        registry,
        path,
        size,
        &setting,
        false,
        &[],
        0,
        &ludion_engine::DebugCategories::None,
        None,
        ludion_engine::AntiAliasing::Off,
    ) {
        Ok(outcome) => Some(outcome),
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[{test}] SKIPPED: no GPU adapter available");
            None
        }
        Err(errors) => panic!("[{test}] render failed: {errors:?}"),
    }
}

/// A live [`Renderer`], or `None` (after printing a skip reason) when there is no GPU —
/// `gi_inject.rs`'s own `skip_or_renderer`, restated across the crate boundary.
fn skip_or_renderer(name: &str) -> Option<Renderer> {
    match Renderer::new() {
        Ok(r) => Some(r),
        Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
            eprintln!("[{name}] SKIPPED: no GPU adapter");
            None
        }
        Err(d) => panic!("[{name}] renderer init failed: {d:?}"),
    }
}

// =================================================================================================
// T4's machinery: the block-YAML writer and the sealed-two-rooms leak geometry, `gi_bounce.rs`'s
// own idioms restated (that file's header states the same "integration binaries share no module"
// rule this one is following).
// =================================================================================================

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A scratch directory, unique per instance, removed on drop (including when a mid-test assertion
/// panics).
struct Fixture(PathBuf);

impl Fixture {
    fn new(tag: &str) -> Self {
        let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "ludion-gi-dynamic-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create fixture directory");
        Fixture(dir)
    }

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

fn wall(y: &mut Yaml, name: &str, t: [f64; 3], s: [f64; 3], color: [f64; 3]) {
    entity(y, name, |y| {
        transform(y, t, [0.0; 3], s);
        y.push(3, "Mesh:");
        y.push(4, "shape: cube");
        y.vec(4, "color", &[color[0], color[1], color[2], 1.0]);
    });
}

fn camera_top_down_yaml(y: &mut Yaml, x: f64, z: f64, height: f64, fov_y_deg: f64) {
    entity(y, "camera", |y| {
        transform(y, [x, height, z], [-90.0, 0.0, 0.0], [1.0; 3]);
        y.push(3, "Camera:");
        y.push(4, "projection:");
        y.push(5, "type: perspective");
        y.push(5, format!("fov_y_deg: {fov_y_deg}"));
    });
}

fn gi_volume(y: &mut Yaml, cell_size: f64, probe_spacing: f64, bounce: f64) {
    entity(y, "gi", |y| {
        y.push(3, "GiVolume:");
        y.push(4, format!("cell_size: {cell_size}"));
        y.push(4, format!("probe_spacing: {probe_spacing}"));
        y.push(4, format!("bounce: {bounce}"));
    });
}

/// A `PointLight` entity carrying the `GiDynamic` marker: `Plan/gi-wave3-dynamic-injection.md` §3's
/// opt-in marker, injected per frame instead of baked.
fn point_light_marked(
    y: &mut Yaml,
    name: &str,
    t: [f64; 3],
    color: [f64; 3],
    intensity: f64,
    range: f64,
) {
    entity(y, name, |y| {
        transform(y, t, [0.0; 3], [1.0; 3]);
        y.push(3, "PointLight:");
        y.vec(4, "color", &color);
        y.push(4, format!("intensity: {intensity}"));
        y.push(4, format!("range: {range}"));
        y.push(3, "GiDynamic: {}");
    });
}

fn header(y: &mut Yaml, name: &str) {
    y.push(0, "format: 1");
    y.push(0, format!("name: \"{name}\""));
    y.push(0, "entities:");
}

/// Two sealed rooms sharing one wall — `gi_bounce.rs`'s T8 leak geometry (`two_rooms_body`),
/// restated verbatim except the emitter: a bare emissive cube cannot be marked `GiDynamic` (only a
/// `PointLight`'s injection is covered), so room A's light is a marked `PointLight` in
/// its place. Room A (`x` in `[-2.0, 2.0]`) holds the marked lamp; room B (`x` in `[2.5, 6.5]`) holds
/// no light of any kind; the shared partition (`x` in `[2.0, 2.5]`) has no gap at all.
fn two_rooms_body(y: &mut Yaml) {
    gi_volume(y, 0.25, 0.5, 1.0);

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

    // Low and offset in z from the read patch (`TORCH_NEAR_CAM`'s spirit, `albedo_bounce_scene`'s
    // grazing-incidence trick): a floor-height light gives the *floor* patch a near-grazing (low
    // `ndl`) direct hit, so the read patch's brightening is dominated by the bounce/injection rather
    // than an already-saturated direct term.
    point_light_marked(y, "lamp", [-1.0, 0.15, -1.3], [1.0, 0.62, 0.28], 10.0, 3.5);
}

fn two_rooms_scene_a() -> String {
    let mut y = Yaml::default();
    header(&mut y, "gi-wave3 dynamic fixture: T4 room A (lit)");
    camera_top_down_yaml(&mut y, -1.0, 0.9, 1.0, 12.0);
    two_rooms_body(&mut y);
    y.0
}

fn two_rooms_scene_b() -> String {
    let mut y = Yaml::default();
    header(
        &mut y,
        "gi-wave3 dynamic fixture: T4 room B (sealed, no light)",
    );
    camera_top_down_yaml(&mut y, 4.5, 0.0, 2.7, 30.0);
    two_rooms_body(&mut y);
    y.0
}

/// Whole-frame mean perceptual luminance (Rec. 709 weights over the sRGB-encoded output bytes) —
/// `gi_bounce.rs`'s verbatim, a monotonic brightness proxy for comparing two renders of the *same*
/// framing.
fn mean_luma_bytes(rgba: &[u8]) -> f64 {
    let pixels = (rgba.len() / 4) as f64;
    let sum: f64 = rgba
        .chunks_exact(4)
        .map(|p| 0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]))
        .sum();
    sum / pixels
}

fn mean_luma_outcome(outcome: &RenderOutcome) -> f64 {
    mean_luma_bytes(&outcome.image.rgba)
}

fn mean_luma_image(image: &RenderedImage) -> f64 {
    mean_luma_bytes(&image.rgba)
}

/// The quality that differs from `FULL` in the injection switch **alone** — `gi_inject.rs`'s own
/// `NO_INJECTION` const, restated: `LOW` would also turn off `ssao`/`shafts`/`dof`, which is not
/// what any of these three fixtures mean to isolate.
const NO_INJECTION: Quality = Quality {
    gi_dynamic: false,
    ..Quality::FULL
};

// =================================================================================================
// T4 — the injection lights the room, and only the room.
// =================================================================================================

/// **T4 — the injection lights the room, and only the room
/// (`Plan/gi-wave3-dynamic-injection.md` §11 T4).** A sealed two-room fixture with a marked
/// `PointLight` in room A: room A's floor beside the lamp reads materially brighter with the
/// machine's `gi_dynamic` switch on than off (the injection reaching it), and room B — sealed behind
/// a gapless shared wall, with no light of its own — holds within a small tolerance. This is W2 T8's
/// twin (`no_bounce_leak_through_a_shared_wall` in `gi_bounce.rs`) and the charter's "leaks are
/// inherited-solved" claim made falsifiable for the per-frame path, exactly as that fixture made it
/// for the bake.
///
/// **Demonstrated able to fail.** `gi_inject.wgsl`'s `inject` entry point was temporarily mutated
/// from the committed hit-cache replay to accumulate each marked light's term **directly at the
/// probe's own position**, bypassing `hit_cache` (and therefore the bake's march, the sealed
/// partition, every wall) entirely:
///
/// ```wgsl
/// @compute @workgroup_size(4, 4, 4)
/// fn inject(@builtin(global_invocation_id) gid: vec3<u32>) {
///     if (any(gid >= params.counts.xyz)) {
///         return;
///     }
///     // No visibility at all: the probe's own world position, not a replayed hit. (Dummy reads of
///     // `albedo`/`hit_cache` keep both bindings in the auto-derived layout, which the Rust-side
///     // bind group still populates.)
///     let _dummy_albedo = textureLoad(albedo, vec3<i32>(0, 0, 0), 0);
///     let _dummy_hit = hit_cache[0];
///     let world = params.grid.xyz + vec3<f32>(gid) * params.grid.w;
///     var acc = vec3<f32>(0.0);
///     let slice = bin_table[bin_cell_of(world)];
///     for (var i = 0u; i < slice.y; i = i + 1u) {
///         let light = light_list[bin_indices[slice.x + i]];
///         let to_light = light.position.xyz - world;
///         let dist = length(to_light);
///         let range = max(light.position.w, 1e-3);
///         let atten = clamp(1.0 - dist / range, 0.0, 1.0);
///         acc = acc + light.color.rgb * atten * atten;
///     }
///     textureStore(dyn_out, vec3<i32>(gid), vec4<f32>(acc, 0.0));
/// }
/// ```
///
/// (`params.grid.w` is the voxel cell size, `0.25` in this fixture, not the authored `probe_spacing`
/// of `0.5` — so `gid * params.grid.w` does not land on the lattice's real probe positions, but that
/// approximation is immaterial to what the mutation demonstrates: it queries the world-space bin from
/// *some* point in every room with no visibility test at all, which is the leak.)
///
/// Measured on the mutated shader against this exact committed fixture: room B's `on/off` ratio moved
/// from **1.000** (green, within `[0.9, 1.15]`) to **1520.194** (off=0.125, on=190.024) — the bin's
/// world-space lookup has no notion of the shared partition, so a probe on room B's side reads room
/// A's lamp as if the wall did not exist. Room A's own ratio stayed above its `> 1.3` bar throughout
/// (**1.514** unmutated, **3.434** mutated — the leak inflates room B, it does not break room A's own
/// check). The shader was restored byte-identical immediately after (`cp` from a pre-mutation copy,
/// not re-typed); `git status`/`git diff` over `crates/ludion-renderer/src/shaders/gi_inject.wgsl`
/// showed no change, and a rebuild afterward reran this file green again at the original numbers
/// (off=68.287, on=103.400, ratio=1.514 for room A; off=0.125, on=0.125, ratio=1.000 for room B).
#[test]
fn needs_gpu_the_injection_lights_the_room_and_only_the_room() {
    let registry = builtin_registry();
    let fixture = Fixture::new("t4");
    let size = FrameSize::native(160, 90);

    let render = |tag: &str, source: &str, quality: Quality| {
        let path = fixture.scene(&format!("{tag}.yaml"), source);
        render_or_skip("gi-dynamic-t4", &registry, &path, size, quality)
    };

    let scene_a = two_rooms_scene_a();
    let scene_b = two_rooms_scene_b();

    let Some(a_off) = render("a-off", &scene_a, NO_INJECTION) else {
        return;
    };
    let a_on = render("a-on", &scene_a, Quality::FULL).expect("GPU present for the second render");
    let b_off = render("b-off", &scene_b, NO_INJECTION).expect("GPU present");
    let b_on = render("b-on", &scene_b, Quality::FULL).expect("GPU present");

    let a_off_luma = mean_luma_outcome(&a_off);
    let a_on_luma = mean_luma_outcome(&a_on);
    let b_off_luma = mean_luma_outcome(&b_off);
    let b_on_luma = mean_luma_outcome(&b_on);
    let a_ratio = a_on_luma / a_off_luma.max(1e-6);
    let b_ratio = b_on_luma / b_off_luma.max(1e-6);

    eprintln!(
        "[gi-dynamic-t4] room A off={a_off_luma:.3} on={a_on_luma:.3} ratio={a_ratio:.3} | room B \
         off={b_off_luma:.3} on={b_on_luma:.3} ratio={b_ratio:.3}"
    );

    assert!(
        a_ratio > 1.3,
        "room A's floor beside the marked lamp must read materially brighter with injection on: \
         off={a_off_luma:.3}, on={a_on_luma:.3}, ratio={a_ratio:.3} (wanted > 1.3) — a globally \
         inert injection would pass the leak check below for the wrong reason"
    );
    assert!(
        (0.9..=1.15).contains(&b_ratio),
        "room B's interior must not brighten at all from room A's marked lamp across the shared \
         wall: off={b_off_luma:.3}, on={b_on_luma:.3}, ratio={b_ratio:.3}"
    );
}

// =================================================================================================
// T6/T7 shared machinery — direct `RenderScene` construction, `ludion-renderer::gi_inject`'s own
// test module's idiom restated across the crate boundary (only the public surface is reachable).
// =================================================================================================

/// A closed `16 × 12 × 16` unit-cell room centred on the origin — `x, z ∈ [-8, 8]`, `y ∈ [-1, 11]` —
/// with a one-cell solid skin and a hollow, white-walled interior. `gi_inject.rs`'s own `room_grid`,
/// restated: the "enclosing shell" `Plan/gi-wave3-dynamic-injection.md` §11 T6 asks for, where every
/// one of the bake's 16 strided cached rays hits something.
fn room_grid() -> WorldVoxelGrid {
    let extent = [16u32, 12, 16];
    let total = (extent[0] * extent[1] * extent[2]) as usize;
    let mut occupancy = vec![false; total];
    for z in 0..extent[2] {
        for y in 0..extent[1] {
            for x in 0..extent[0] {
                let skin = x == 0
                    || y == 0
                    || z == 0
                    || x == extent[0] - 1
                    || y == extent[1] - 1
                    || z == extent[2] - 1;
                let flat = x + extent[0] * (y + extent[1] * z);
                occupancy[flat as usize] = skin;
            }
        }
    }
    WorldVoxelGrid {
        origin: Vec3::new(-8.0, -1.0, -8.0),
        cell_size: 1.0,
        extent,
        occupancy,
        albedo: vec![[1.0, 1.0, 1.0, 1.0]; total],
        emissive: vec![0.0; total],
        hash: 0,
    }
    .sealed()
}

/// The lattice `ludion-engine` would size over `grid` at `probe_spacing` — `gi_inject.rs`'s own
/// `gi_data`, restated so this module needs no engine-internal dependency.
fn gi_data(grid: &WorldVoxelGrid, probe_spacing: f32, bounce: f32) -> GiData {
    let count = std::array::from_fn(|axis| {
        let span = grid.extent[axis] as f32 * grid.cell_size;
        (span / probe_spacing).ceil().max(1.0) as u32 + 1
    });
    GiData {
        probe_spacing,
        normal_bias: 0.25,
        sky_visibility: 1.0,
        bounce,
        count,
        origin: grid.origin,
        span: Vec3::new(
            (count[0] - 1) as f32 * probe_spacing,
            (count[1] - 1) as f32 * probe_spacing,
            (count[2] - 1) as f32 * probe_spacing,
        ),
    }
}

fn point_light(
    position: Vec3,
    color: Vec3,
    intensity: f32,
    range: f32,
    dynamic: bool,
) -> PointLightData {
    PointLightData {
        position,
        color,
        intensity,
        range,
        dynamic,
        shadow: None,
        scatter: 0.0,
    }
}

/// A matte white floor plane at `y = 0` — the top face of `room_grid`'s solid floor skin (cell 0
/// spans `y ∈ [-1, 0]`), the only geometry the camera actually sees: the voxel grid feeds the GI
/// bake/injection alone and draws nothing by itself (`gi_inject.rs`'s `scene_over` establishes the
/// same split).
fn floor_instance() -> MeshInstance {
    MeshInstance {
        medium: 1.0,
        porosity: ludion_renderer::DEFAULT_POROSITY,
        shape: MeshShape::Plane,
        model: Mat4::from_scale(Vec3::new(7.0, 1.0, 7.0)),
        normal_matrix: Mat3::IDENTITY,
        color: [0.85, 0.85, 0.85, 1.0],
        material: [1.0, 0.0, 0.0],
        use_vertex_material: false,
        fade: 1.0,
        fade_height_min: -f32::MAX,
        flash: 0.0,
        flash_color: [1.0; 3],
        wrap: 0.0,
        scatter_color: [1.0; 3],
        bone_offset: 0,
        cast_shadows: true,
        viewmodel: false,
        rim: [0.0; 4],
    }
}

/// A camera looking straight down at `(x, z)` from `height` above the floor — the same top-down
/// framing `gi_bounce.rs` establishes at the YAML level, built directly here since T6/T7 skip
/// `extract` entirely.
fn camera_top_down(
    x: f32,
    z: f32,
    height: f32,
    fov_y_deg: f32,
    viewport: (u32, u32),
) -> RenderCamera {
    let position = Vec3::new(x, height, z);
    let view = Mat4::look_at_rh(position, Vec3::new(x, 0.0, z), Vec3::Z);
    let projection =
        Mat4::perspective_rh(fov_y_deg.to_radians(), aspect_ratio(viewport), 0.1, 1000.0);
    RenderCamera {
        view,
        projection,
        position,
    }
}

/// `room_grid()` plus its floor plane, under `camera`, with `lights` and `gi` — the one scene shape
/// T6 and T7 both render repeatedly.
fn room_scene(
    grid: &WorldVoxelGrid,
    gi: Option<GiData>,
    lights: Vec<PointLightData>,
    camera: RenderCamera,
    viewport: (u32, u32),
) -> RenderScene {
    let mut scene = RenderScene::empty(viewport);
    scene.camera = camera;
    scene.meshes.push(floor_instance());
    scene.lights = lights;
    if gi.is_some() {
        scene.voxel = Some(Arc::new(grid.clone()));
    }
    scene.gi = gi;
    scene
}

// =================================================================================================
// T6 — energy agreement.
// =================================================================================================

/// **T6 — energy agreement (`Plan/gi-wave3-dynamic-injection.md` §11 T6).** One `PointLight`, same
/// position/intensity/range, measured two ways at the same floor patch: baked (unmarked, `bounce:
/// 0.0` vs `1.0`, quality held at `FULL`) and injected (marked, quality `gi_dynamic` off vs on,
/// `bounce` held at `1.0`). Both halves isolate their term by subtraction (this file's module doc),
/// so the comparison is `static_gain` — the DC the static estimator (64 rays, W2's shipped
/// convention) puts at this patch — against `injected_gain`, the same physical light integrated
/// through the strided 16-ray cache replay (`Plan/gi-wave3-dynamic-injection.md` §5.2).
///
/// **The tolerance band.** `room_grid()` is the "enclosing-shell" arrangement §11 T6 asks for: a
/// light anywhere in the hollow interior has nearby wall on every side, so both the bake's 64 rays
/// and the injection's cached 16 sample a similarly-shaped set of nearby hits rather than a handful
/// of directions dominating — the K = 16 vs 64 sampling variance this band exists to absorb is
/// therefore modest, not the dominant term. What the band must absorb instead, honestly stated: this
/// measurement runs through the full render (tonemap, exposure, 8-bit quantization) rather than a raw
/// probe-buffer readback (`ludion-renderer::gi_inject`'s `pub(crate)` readback path is unreachable
/// across the crate boundary from here — this file's module doc), so `±40%` is wide enough to cover
/// that reconstruction noise while remaining far tighter than the drifts it must still catch: a
/// missing `1/K` normalization is a 16× swing, a missing premultiply moves the color channels apart
/// rather than the total magnitude, and a wrong albedo factor scales linearly with `room_grid`'s
/// authored `1.0` (i.e. would show as a near-exact factor-of-`albedo` miss, nowhere near `40%`).
///
/// Fails if the `1/K` normalization, the premultiply, or the albedo factor drifts between the two
/// shaders' convergent-but-independent implementations of "the same physical light's outgoing
/// radiance at a hit."
#[test]
fn needs_gpu_the_injected_energy_agrees_with_the_baked_energy() {
    let Some(mut renderer) = skip_or_renderer("gi-dynamic-t6") else {
        return;
    };
    let grid = room_grid();
    let viewport = (160, 90);
    let camera = camera_top_down(0.0, 0.0, 1.5, 10.0, viewport);
    let size = FrameSize::native(viewport.0, viewport.1);
    let position = Vec3::new(3.0, 4.0, 2.0);
    let color = Vec3::new(1.0, 1.0, 1.0);
    let intensity = 6.0;
    let range = 8.0;

    let render = |dynamic: bool, bounce: f32, quality: Quality, renderer: &mut Renderer| {
        let lights = vec![point_light(position, color, intensity, range, dynamic)];
        let gi = gi_data(&grid, 4.0, bounce);
        let scene = room_scene(&grid, Some(gi), lights, camera, viewport);
        renderer.set_quality(quality, QualityParams::FULL);
        renderer.render(&scene, size).expect("render succeeds")
    };

    // Static half: unmarked light, bounce off vs on, quality held at FULL (the injection never
    // schedules — nothing is marked).
    let static_off = render(false, 0.0, Quality::FULL, &mut renderer);
    let static_on = render(false, 1.0, Quality::FULL, &mut renderer);
    let static_gain = mean_luma_image(&static_on) - mean_luma_image(&static_off);

    // Injected half: marked light, bounce always 1.0 (the injected term IS the bounce term — R2),
    // quality toggles the injection pass alone.
    let injected_off = render(true, 1.0, NO_INJECTION, &mut renderer);
    let injected_on = render(true, 1.0, Quality::FULL, &mut renderer);
    let injected_gain = mean_luma_image(&injected_on) - mean_luma_image(&injected_off);

    let ratio = injected_gain / static_gain.max(1e-6);
    eprintln!(
        "[gi-dynamic-t6] static gain={static_gain:.4} injected gain={injected_gain:.4} \
         ratio={ratio:.4} (wanted in [0.6, 1.4])"
    );

    assert!(
        static_gain > 0.05 && injected_gain > 0.05,
        "both terms must actually deposit light, or the ratio compares two near-zero noise floors: \
         static={static_gain:.4}, injected={injected_gain:.4}"
    );
    assert!(
        (0.6..=1.4).contains(&ratio),
        "the injected term's energy must agree with the baked term's for the same physical light \
         within the stated band: static gain={static_gain:.4}, injected gain={injected_gain:.4}, \
         ratio={ratio:.4} (wanted in [0.6, 1.4])"
    );
}

// =================================================================================================
// T7 — no step at a bin boundary.
// =================================================================================================

/// **T7 — no step at a bin-cell boundary (`Plan/gi-wave3-dynamic-injection.md` §11 T7, §5.4).**
///
/// `room_grid()`'s own dimensions place a bin-cell boundary at a convenient, exactly-known
/// coordinate: `gi_bin.rs`'s `BIN_DIMS = [32, 16, 32]` bins a fixed count of cells over the grid's
/// world span (restated here from that module, since it is `pub(crate)`), so with this grid's 16-unit
/// x-span the per-axis cell size is `max(16/32, 1 voxel cell) = max(0.5, 1.0) = 1.0` world unit (the
/// one-voxel-cell floor wins) — bin cells align exactly with this grid's own voxel cells, and with
/// `origin.x = -8.0` a boundary sits at every integer world `x`, `x = 0.0` among them (the room's own
/// centre).
///
/// A marked light sweeps `x` from `-0.5` to `0.5` in `0.05`-unit steps (ten sub-cell steps either
/// side of the boundary), embedded just above the floor slab's own top (`y = -0.2`, inside the
/// floor's bin cell but on the side that still gives a hit at the floor's `+Y` face a positive
/// `ndl`) and centred in `z`, directly under the fixed floor-centre camera patch the injected term
/// is read at (isolated by subtraction, this file's module doc). The lattice is dense
/// (`probe_spacing = 0.5`, matching the bin's own cell size) so a probe sits exactly at the origin,
/// its own near-floor cached hit landing close enough to the swept region for the series to carry a
/// real signal rather than reading a probe too far away to be affected at all — the failure mode a
/// first attempt at this fixture hit (light placed off in open space, away from any probe whose
/// cached hits could ever reach it: uniformly zero at every step, recorded here so the next attempt
/// does not repeat it).
///
/// **The bound, derived from `atten`'s own slope.** `atten = clamp(1 - dist/range, 0, 1)` is C¹, and
/// `d(atten²)/d(dist) = -2·atten/range`, maximal in magnitude at `atten = 1` (`dist = 0`): `2/range`.
/// Moving the light by a step `δ` changes any *fixed* hit's distance by at most `δ` (triangle
/// inequality), so the per-hit change in `atten²` is bounded by `(2/range)·δ` regardless of which
/// side of any boundary the light sits on — conservative filing (`gi_bin.rs`'s own claim, §4.3)
/// guarantees the *set* of hits reachable does not change discontinuously either, so the injected DC
/// as a whole inherits the same per-step bound. This fixture's own smooth region (every step that
/// does not cross `x = 0.0`) is where that bound is measured concretely — through the render's own
/// reconstruction, not a raw analytic pixel count — and the boundary step is asserted against it: a
/// discontinuity is a delta at `x = 0.0` that stands out from its neighbours, not one merely present
/// (GPU float noise gives every step *some* nonzero delta).
///
/// **Demonstrated able to fail.** `gi_bin.rs`'s `footprint_of` was temporarily mutated from the
/// committed range-sphere-plus-margin filing to file a light into **only the one bin cell containing
/// its exact position**, ignoring `reach` entirely:
///
/// ```rust,ignore
/// fn footprint_of(params: &GiBinParams, light: &Light, _margin: f32) -> Footprint {
///     let center = [light.position[0], light.position[1], light.position[2]];
///     if !center.iter().all(|v| v.is_finite()) {
///         return Footprint::everything(params);
///     }
///     let cell = std::array::from_fn(|axis| axis_index(params, axis, center[axis]));
///     Footprint { lo: cell, hi: cell }
/// }
/// ```
///
/// Measured on the mutated bin against this fixture: the smooth-region per-step deltas rose to
/// **max 2.329, mean 1.386** (larger than the unmutated run's own **max 0.735, mean 0.325** — the
/// centre-cell-only bin is noisier everywhere, since the light now depends on exactly which single
/// cell each probe's hit falls in rather than a wide, forgiving window), but the step crossing
/// `x = 0.0` jumped to **17.165** — roughly **7.4×** the mutated run's own noisiest smooth step, and
/// **47×** the unmutated run's boundary step (0.361) — comfortably past the bound
/// (`max(4·max_other, 8·mean_other) = 11.086`) the assertion computed from the mutated run's own
/// smooth region, which is why the test failed with exactly that message. The discontinuity is the
/// mutation's signature: every hit cell in the light's *previous* bin cell loses its membership in
/// one discrete step at the boundary instead of fading through the margin. The shader-free nature of
/// this mutation (pure Rust) meant no rebuild of any `.wgsl`; `cargo test` alone reran it. `gi_bin.rs`
/// was restored byte-identical immediately after (`cp` from a pre-mutation copy, not re-typed);
/// `git status`/`git diff` showed no change, and a rerun of this file was green again at the original
/// numbers (boundary delta 0.361, smooth-region max 0.735, mean 0.325).
#[test]
fn needs_gpu_no_step_at_a_bin_cell_boundary() {
    let Some(mut renderer) = skip_or_renderer("gi-dynamic-t7") else {
        return;
    };
    let grid = room_grid();
    let viewport = (160, 90);
    let camera = camera_top_down(0.0, 0.0, 1.5, 12.0, viewport);
    let size = FrameSize::native(viewport.0, viewport.1);
    let color = Vec3::new(1.0, 1.0, 1.0);
    let intensity = 6.0;
    let range = 6.0;
    // Embedded just above the floor slab's own top face (the floor's bin cell spans world
    // y ∈ [-1, 0), and `y = -0.2` sits inside it on the side that still gives the floor's `+Y`-facing
    // hits a positive `ndl`), directly under the read patch — what makes the able-to-fail
    // demonstration below meaningful. Two earlier placements were tried and rejected, recorded so the
    // next attempt at this fixture does not repeat them: (1) deep in the hollow interior (`y = 3`,
    // `z = 3`) shares a bin cell with no geometry at all — a centre-cell-only bin then files the light
    // somewhere no hit cache ever reaches, measured as uniformly zero injected energy at every step,
    // whichever side of `x = 0.0` the light sits on, a real but uninteresting failure with no boundary
    // to speak of; (2) close to a side wall but 7+ units from the read patch (`y = 3`, `z = 7.3`)
    // shares a bin cell with real geometry, but that geometry's hit lands in a *different* probe's
    // cache than the ones the read patch interpolates from — GI volumes do not diffuse between
    // probes, so a signal that never reaches a nearby probe never reaches the patch either, and this
    // also measured uniformly zero. Embedding the light in the floor right under the patch solves
    // both: the floor is occupied at every `(x, z)`, so the light's own bin cell is always real
    // geometry, and the probe sitting at the origin (this fixture's dense `probe_spacing = 0.5`
    // guarantees one) is close enough for its near-floor cached hit to matter to the patch.
    let (y, z) = (-0.2, 0.0);

    // Ten sub-cell steps either side of the boundary at x = 0.0 — `BIN_DIMS`/`room_grid`'s own
    // arithmetic, restated in this test's doc comment.
    let xs: Vec<f32> = (-10..=10).map(|i| (i as f32) * 0.05).collect();
    assert!(
        xs.contains(&0.0),
        "the sweep must include the boundary itself"
    );

    let gain_at = |x: f32, renderer: &mut Renderer| -> f64 {
        let lights = vec![point_light(
            Vec3::new(x, y, z),
            color,
            intensity,
            range,
            true,
        )];
        // A dense lattice: under the centre-cell-only mutation only a probe whose *own* cached hit
        // lands in the light's exact evolving bin cell carries any signal at all, so the sweep needs
        // enough probes that some hit realistically lands there on both sides of the boundary.
        let gi = gi_data(&grid, 0.5, 1.0);
        let scene = room_scene(&grid, Some(gi), lights, camera, viewport);
        renderer.set_quality(Quality::FULL, QualityParams::FULL);
        let on = renderer.render(&scene, size).expect("render succeeds");
        renderer.set_quality(NO_INJECTION, QualityParams::FULL);
        let off = renderer.render(&scene, size).expect("render succeeds");
        mean_luma_image(&on) - mean_luma_image(&off)
    };

    let gains: Vec<(f32, f64)> = xs.iter().map(|&x| (x, gain_at(x, &mut renderer))).collect();
    assert!(
        gains.iter().any(|&(_, g)| g > 0.02),
        "the sweep must actually deposit measurable light somewhere, or every delta below is noise: \
         {gains:?}"
    );

    // Per-step deltas, tagged by the step's midpoint x (so the boundary-crossing step is easy to
    // find: it is the one whose interval contains 0.0).
    let deltas: Vec<(f32, f64)> = gains
        .windows(2)
        .map(|w| {
            let ((x0, g0), (x1, g1)) = (w[0], w[1]);
            ((x0 + x1) / 2.0, (g1 - g0).abs())
        })
        .collect();

    let (boundary_x, boundary_delta) = deltas
        .iter()
        .copied()
        .find(|&(mid, _)| mid.abs() < 0.03)
        .expect("one step interval must straddle x = 0.0 at this step size");
    let others: Vec<f64> = deltas
        .iter()
        .filter(|&&(mid, _)| mid.abs() >= 0.03)
        .map(|&(_, d)| d)
        .collect();
    let max_other = others.iter().copied().fold(0.0f64, f64::max);
    let mean_other = others.iter().sum::<f64>() / others.len() as f64;

    eprintln!(
        "[gi-dynamic-t7] boundary step (mid x={boundary_x:.3}) delta={boundary_delta:.4} | \
         smooth-region max={max_other:.4} mean={mean_other:.4} | full series: {gains:?}"
    );

    // The bound: the boundary step's delta must not stand out from the sweep's own smooth-region
    // deltas by more than a generous safety margin over the noisiest smooth step measured — the
    // analytic argument says it should be OF THE SAME ORDER, not merely "not infinite".
    let bound = (max_other * 4.0).max(mean_other * 8.0).max(0.01);
    assert!(
        boundary_delta <= bound,
        "the injected term must not step at the bin-cell boundary: boundary delta={boundary_delta:.4} \
         at x≈{boundary_x:.3}, smooth-region max={max_other:.4} mean={mean_other:.4}, bound={bound:.4} \
         — a delta this far past its neighbours is a discontinuity, not sampling noise"
    );
}
