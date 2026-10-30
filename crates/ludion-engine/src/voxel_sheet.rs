//! `ludion render --dump-voxels` — the GI voxelizer's world-space grid rendered as one stitched,
//! labeled PNG.
//!
//! [`render_voxel_sheet`] is the "zero pixels" instrument the voxelizer wave needs
//! (`Plan/gi-wave0-voxelizer.md` §11): a scene's [`WorldVoxelGrid`], if extraction built one, laid
//! out as one horizontal (XZ) slice per Y layer — an **albedo** block ("did the walls fill?") beside
//! a parallel **emissive** block ("did the torches register?"), each tiled near-square. Every byte is
//! a pure CPU row-copy of the grid's own buffers: no GPU adapter is ever requested and no wall clock
//! is read, so two runs over the same scene write byte-identical bytes, and a scene with no
//! `GiVolume` — the voxelizer's opt-in switch — reports a readable error rather than an empty image.
//!
//! # Why the labels are stamped on the CPU
//!
//! `model_sheet` burns its orientation labels by placing a `UiText` in the synthesized scene and
//! letting the ordinary GPU overlay pass draw it — that is a real render. `--dump-voxels` must stay
//! usable with **no GPU adapter at all** when only the dump was asked for (`render --dump-voxels`
//! with no `--out`), so labels here are stamped straight into the RGBA8 sheet buffer from the
//! embedded 8×8 face (`ludion_font::stamp_label` over the decoded `EMBEDDED_PNG`) — the same glyphs
//! the GPU text path draws, with no adapter and no renderer type involved.
//!
//! # Layout
//!
//! For a grid of extent `[ex, ey, ez]`: `ey` tiles, one per Y layer, packed into a near-square grid
//! (`cols = ceil(sqrt(ey))`, matching `model_sheet`'s sweep grid). Each tile is `ex` × `ez` cells,
//! upscaled by an integer factor so a small grid's tile is still legible — a district-scale grid
//! (hundreds of cells per axis) already renders at native 1:1 and is left alone
//! ([`upscale_factor`]). A cell's albedo tile pixel is its sRGB-encoded linear albedo when occupied,
//! the sheet background otherwise; its emissive tile pixel lerps from a dim occupied-but-dark gray to
//! a warm color by the cell's emissive strength as a fraction of the **grid's own peak** — so the
//! sheet self-calibrates to whatever emissive scale the scene actually authored, and stays background
//! where nothing is occupied at all. A one-pixel gap between neighbouring tiles (baked into each
//! tile's reserved cell size, not drawn) keeps slices visually separated with no extra draw pass.
//!
//! # `--dump-gi`
//!
//! [`render_gi_sheet`] is this instrument's sibling for the baked SH-L1 probe volume
//! (`Plan/gi-wave1-sky-visibility.md` §10, grown a third block by
//! `Plan/gi-wave2-emissive-bounce.md` §8 and a fourth by `Plan/gi-wave3-dynamic-injection.md` §10):
//! the same one-tile-per-Y-layer, blocks-side-by-side shape, a **VISIBILITY** block (the DC term,
//! greyscale) beside a **BENT NORMAL** block (the L1 vector remapped to RGB via `0.5 + 0.5*v`)
//! beside a **BOUNCE** block (the baked one-bounce DC radiance, tone-mapped — see [`bounce_color`])
//! beside a **DYNAMIC** block (the per-frame injected DC irradiance from `GiDynamic`-marked lights,
//! tone-mapped at the same fixed exposure as BOUNCE — one exposure convention across both radiance
//! blocks, so a sheet reads as one instrument). A scene that authored `bounce: 0.0` baked no bounce
//! volume at all, so the BOUNCE block draws no data and its title says so ("NOT BAKED") rather than
//! drawing an indistinguishable-from-dark black block; the DYNAMIC block does the same when the scene
//! schedules no injection, and **states which of the three content reasons it is** — no `GiVolume`,
//! `bounce: 0.0`, or no `GiDynamic`-marked light — never a bare "off". Above the blocks, a header
//! strip carries the split census (`lights: N baked + M injected`) and the static bake's own
//! cache-hit status — the one-glance "is something thrashing the bake" answer
//! (`Plan/gi-wave3-dynamic-injection.md` §3.1). Unlike [`render_voxel_sheet`] it does need a GPU
//! adapter: the probe volume exists only as a baked GPU texture (`ludion-renderer`'s `gi_bake.rs`),
//! so dumping it means baking (or hitting the renderer's own cache) and reading the result back --
//! the one honest difference from its CPU-only sibling.

use std::path::Path;
use std::sync::OnceLock;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::math;
use ludion_ecs::Registry;
use ludion_renderer::{
    FrameSize, GiData, GiReadback, QualityParams, RenderedImage, Renderer, WorldVoxelGrid,
};
use ludion_scene::{AppliedOverride, SceneOverride};

use crate::debug_draw::DebugCategories;
use crate::extract::{extract_at, gi_volume_skips};
use crate::render::scene_file_driver;
use crate::voxelizer::SkipReport;

/// The result of a voxel-sheet dump: the stitched image plus the dumped grid's own shape, for the
/// CLI's summary line.
#[derive(Debug)]
pub struct VoxelSheetOutcome {
    /// The stitched sheet (RGBA8): an albedo block beside a parallel emissive block, one tile per Y
    /// layer.
    pub image: RenderedImage,
    /// The dumped grid's cell extent, `[ex, ey, ez]`.
    pub extent: [u32; 3],
    /// How many of the grid's cells are occupied — a non-zero number is the "the walls filled"
    /// signal at a glance, before a human or agent even opens the image.
    pub occupied_cells: usize,
    /// What each `--set` override changed, in the order given — echoed by the caller exactly as
    /// every other render mode echoes its overrides.
    pub applied_overrides: Vec<AppliedOverride>,
}

/// Renders the [`WorldVoxelGrid`] the scene at `path` builds, as one stitched slice sheet — the
/// `--dump-voxels` payload.
///
/// CPU-only start to finish: the scene is read, overridden, validated, instantiated, and extracted
/// exactly as [`crate::render_scene_with_registry`] does (`extract_scene_file`'s shared prologue),
/// and the sheet is a pure row-copy of the resulting grid's buffers — no GPU adapter is ever
/// requested, so this answers on a machine with none.
///
/// # Errors
/// - As `extract_scene_file`'s CPU half: `L1015`/`L1001`–`L1014` (read/parse/validate),
///   `L0004`/`L0006` (instantiate), `L1101`/`L1102`/`L2101`/`L2103` (extract, including the
///   voxelizer's own cell-budget guard), `L4008` (an unresolvable `--set` pointer).
/// - `L2104` if the scene authors no `GiVolume` — extraction then builds no grid, and there is
///   nothing to dump. `--set` cannot create the component (it never conjures structure, by design);
///   author one in the scene, or `--set` a `cell_size` on a `GiVolume` the scene already has.
/// - `L2111` if the scene authors a `GiVolume` but nothing placed in it voxelizes.
pub fn render_voxel_sheet(
    registry: &Registry,
    path: &Path,
    size: FrameSize,
    overrides: &[SceneOverride],
) -> Result<VoxelSheetOutcome, Vec<Diagnostic>> {
    let (driver, applied_overrides) =
        scene_file_driver(registry, path, overrides, 0, &DebugCategories::None)?;
    let project_root = path.parent().unwrap_or(Path::new("."));
    let render_scene = extract_at(
        driver.world(),
        size.output,
        project_root,
        driver.time(),
        QualityParams::FULL.particle_density,
    )?;
    let Some(grid) = render_scene.voxel.as_ref() else {
        return Err(empty_grid(
            path,
            driver.world(),
            &render_scene.camera,
            project_root,
            driver.time(),
        ));
    };
    Ok(VoxelSheetOutcome {
        image: build_sheet(grid),
        extent: grid.extent,
        occupied_cells: grid.occupancy.iter().filter(|&&occupied| occupied).count(),
        applied_overrides,
    })
}

/// The result of a GI-sheet dump: the stitched image plus the dumped probe lattice's own shape, for
/// the CLI's summary line.
#[derive(Debug)]
pub struct GiSheetOutcome {
    /// The stitched sheet (RGBA8): a VISIBILITY block, a BENT NORMAL block, a BOUNCE block, and a
    /// DYNAMIC block, one tile per probe Y layer, with a header strip above carrying the split
    /// census and the bake-cache status.
    pub image: RenderedImage,
    /// The dumped probe lattice's shape, `[count.x, count.y, count.z]`.
    pub count: [u32; 3],
    /// Whether the scene baked a bounce volume at all (the authored `GiVolume.bounce > 0.0`) — the
    /// BOUNCE block draws real data when `true` and carries the "NOT BAKED" title when `false`. The
    /// CLI's summary line echoes this so the not-baked case is visible without opening the PNG.
    pub bounce_baked: bool,
    /// How many of the scene's lights are **static** — unmarked, feeding the baked bounce and
    /// re-baking whenever a system moves one — versus **injected**: carrying the `GiDynamic` marker,
    /// gathered per frame, free to move (`Plan/gi-wave3-dynamic-injection.md` §3, §10's split
    /// census). `lights_baked + lights_injected` is the scene's whole light count.
    pub lights_baked: usize,
    /// How many of the scene's lights are `GiDynamic`-marked (injected). See
    /// [`lights_baked`](GiSheetOutcome::lights_baked).
    pub lights_injected: usize,
    /// Whether this dump's own static bake was a cache hit (`true`) or paid for a fresh bake
    /// (`false`) — [`GiReadback::bake_cached`] verbatim, the "is something thrashing the bake"
    /// signal a forgotten `GiDynamic` marker shows up as.
    pub bake_cached: bool,
    /// `None` when the scene's DYNAMIC term was actually injected this dump; `Some(reason)` when it
    /// was not — one of `"no GiVolume"`, `"bounce: 0.0"`, `"no GiDynamic lights"`
    /// (`dynamic_absent_reason`). The machine's own quality switch can never be the reason here:
    /// `--dump-gi` always reads back at this renderer's `FULL` quality.
    pub dynamic_absent: Option<&'static str>,
    /// What each `--set` override changed, in the order given — echoed by the caller exactly as
    /// every other render mode echoes its overrides.
    pub applied_overrides: Vec<AppliedOverride>,
}

/// Renders the baked SH-L1 probe volume the scene at `path` builds, as one stitched slice sheet — the
/// `--dump-gi` payload (`Plan/gi-wave1-sky-visibility.md` §10).
///
/// The CPU prologue is shared with [`render_voxel_sheet`] (`extract_scene_file`), so a scene that
/// fails to parse, validate, or that authors no `GiVolume` errors out before an adapter is ever
/// requested. Once that passes, **unlike [`render_voxel_sheet`] this does request a GPU adapter**: the
/// probe volume this dumps exists only as a baked GPU texture
/// (`crates/ludion-renderer/src/gi_bake.rs`), so producing the sheet means baking it (or hitting the
/// renderer's own cache) and reading the result back.
///
/// # Errors
/// - As [`render_voxel_sheet`]'s CPU half.
/// - `L2104` if the scene authors no `GiVolume` (extraction then builds no probe lattice to dump),
///   `L2111` if it authors one but nothing placed in it voxelizes — the same codes and messages
///   [`render_voxel_sheet`] uses for its own missing-grid case.
/// - `L2003`/`L2004` if no GPU adapter/device is available.
/// - `L2005` if the GPU readback fails.
pub fn render_gi_sheet(
    registry: &Registry,
    path: &Path,
    size: FrameSize,
    overrides: &[SceneOverride],
) -> Result<GiSheetOutcome, Vec<Diagnostic>> {
    let (driver, applied_overrides) =
        scene_file_driver(registry, path, overrides, 0, &DebugCategories::None)?;
    let project_root = path.parent().unwrap_or(Path::new("."));
    let render_scene = extract_at(
        driver.world(),
        size.output,
        project_root,
        driver.time(),
        QualityParams::FULL.particle_density,
    )?;
    let Some(gi) = render_scene.gi else {
        return Err(empty_grid(
            path,
            driver.world(),
            &render_scene.camera,
            project_root,
            driver.time(),
        ));
    };

    let renderer = Renderer::new().map_err(|d| vec![d])?;
    let readback = renderer
        .read_gi_probes(&render_scene)
        .map_err(|d| vec![d])?
        .ok_or_else(|| vec![no_gi_volume(path)])?;

    let lights_injected = render_scene
        .lights
        .iter()
        .filter(|light| light.dynamic)
        .count();
    let lights_baked = render_scene.lights.len() - lights_injected;
    let dynamic_absent = readback
        .dynamic
        .is_none()
        .then(|| dynamic_absent_reason(Some(&gi), lights_injected > 0));

    Ok(GiSheetOutcome {
        image: build_gi_sheet(
            gi.count,
            &readback,
            lights_baked,
            lights_injected,
            readback.bake_cached,
            dynamic_absent,
        ),
        count: gi.count,
        bounce_baked: readback.bounce.is_some(),
        lights_baked,
        lights_injected,
        bake_cached: readback.bake_cached,
        dynamic_absent,
        applied_overrides,
    })
}

/// Why the DYNAMIC block carries no data, mirroring `gi_inject::scheduled_lattice`'s three content
/// gates in `ludion-renderer` — the machine's own `gi_dynamic` quality switch is deliberately not a
/// fourth arm here: `Renderer::read_gi_probes` always reads back at this renderer's own `FULL`
/// quality, so that reason cannot occur through this path, and this function does not fabricate it
/// (`Plan/gi-wave3-dynamic-injection.md` §10).
///
/// `gi: None` stands for "no GiVolume": unreachable through [`render_gi_sheet`] today (extraction
/// reports `L2104` before this function is ever called, since the sheet needs a resolved lattice to
/// size itself from), kept as a real arm rather than an `unreachable!` so the reason stays directly
/// unit-testable instead of merely assumed dead code.
fn dynamic_absent_reason(gi: Option<&GiData>, any_marked: bool) -> &'static str {
    let Some(gi) = gi else {
        return "no GiVolume";
    };
    if gi.bounce <= 0.0 {
        return "bounce: 0.0";
    }
    if !any_marked {
        return "no GiDynamic lights";
    }
    // Given the caller's invariant (asked only when the readback's dynamic term came back `None`),
    // reaching here would mean `scheduled_lattice` disagreed with what was actually read back — a
    // defensive fallback that states the likeliest cause rather than panicking over it, never
    // expected to fire.
    "no GiDynamic lights"
}

/// Why extraction built no grid for a scene that reached the dump: `L2104` when it authors no
/// `GiVolume`, otherwise `L2111` with what the voxelizer skipped. The diagnosis re-walks placement,
/// which is fine on an error path.
fn empty_grid(
    path: &Path,
    world: &ludion_ecs::World,
    camera: &ludion_renderer::RenderCamera,
    project_root: &Path,
    now: ludion_core::time::SimTime,
) -> Vec<Diagnostic> {
    match gi_volume_skips(world, camera, project_root, now) {
        Ok(None) => vec![no_gi_volume(path)],
        Ok(Some((index, skipped))) => vec![gi_volume_empty(path, index, &skipped)],
        Err(diagnostics) => diagnostics,
    }
}

/// `L2111` — the scene's `GiVolume` covers no voxelizable geometry, so extraction built no grid to
/// dump; the message counts what the voxelizer skipped, by reason, and the help points at the first
/// entity of each.
fn gi_volume_empty(path: &Path, index: usize, skipped: &SkipReport) -> Diagnostic {
    let reasons = [
        (
            &skipped.dynamic,
            "`GiDynamic`",
            "drop `GiDynamic` from a mesh that should occlude",
        ),
        (
            &skipped.skinned,
            "skinned",
            "a skinned mesh never voxelizes",
        ),
        (
            &skipped.non_finite,
            "non-finite",
            "fix the non-finite transform",
        ),
        (
            &skipped.unsupported,
            "sculpt, assembly or empty mesh",
            "sculpts, assemblies and meshes with no triangles are not voxelized",
        ),
        (
            &skipped.cutout,
            "cutout",
            "an `alpha_mode: mask` mesh lets light through and is not voxelized",
        ),
    ];
    let counts: Vec<String> = reasons
        .iter()
        .filter(|(skip, ..)| skip.count > 0)
        .map(|(skip, label, _)| format!("{} {label}", skip.count))
        .collect();
    let (detail, help) = if counts.is_empty() {
        (
            "the scene places no mesh".to_string(),
            "place a cube, plane, sphere, voxel model or asset mesh inside the scene".to_string(),
        )
    } else {
        let hints: Vec<String> = reasons
            .iter()
            .filter_map(|(skip, _, fix)| {
                skip.first.map(|first| format!("/entities/{first}: {fix}"))
            })
            .collect();
        (format!("skipped: {}", counts.join(", ")), hints.join("; "))
    };
    Diagnostic::new(
        code::GI_VOLUME_EMPTY,
        format!(
            "`{}`'s `GiVolume` covers no voxelizable geometry, so extraction built no \
             global-illumination grid to dump; {detail}",
            path.display()
        ),
    )
    .with_path(format!("/entities/{index}/components/GiVolume"))
    .with_help(help)
}

/// `L2104` — the scene authors no `GiVolume`, so extraction built no grid to dump.
fn no_gi_volume(path: &Path) -> Diagnostic {
    Diagnostic::new(
        code::GI_VOLUME_MISSING,
        format!(
            "`{}` authors no `GiVolume`, so extraction built no global-illumination grid to dump",
            path.display()
        ),
    )
    .with_help(
        "add a `GiVolume` entity to the scene to opt it into voxelization; `--set` can tune a \
         `cell_size` on a `GiVolume` the scene already authors, but it never creates a component \
         the scene does not have",
    )
}

// --- Sheet geometry ---

/// A slice tile's larger side is upscaled at least to this many pixels before the integer scale
/// factor is clamped by [`MAX_UPSCALE`] — small enough that a district-scale grid (hundreds of cells
/// per axis) needs no upscale at all and renders 1:1.
const MIN_TILE_PX: u32 = 128;
/// The largest integer upscale a tile takes. Past this a tiny grid's tile would dwarf its
/// neighbours' labels for no further legibility gain.
const MAX_UPSCALE: u32 = 64;
/// Sheet background, and the color an unoccupied cell renders as on both blocks — the same neutral
/// dark `model_sheet` fills an unused contact-sheet cell with.
pub(crate) const BACKGROUND: [u8; 4] = [24, 26, 30, 255];
/// Gap between the albedo block and the emissive block.
pub(crate) const BLOCK_GAP: u32 = 24;
/// Header strip above each block, carrying its name.
pub(crate) const BLOCK_TITLE_H: u32 = 28;
/// The GI sheet's own header strip, above every block: two lines at [`HEADER_SCALE`] — the split
/// census (`lights: N baked + M injected`) and the static bake's cache status
/// (`Plan/gi-wave3-dynamic-injection.md` §10). [`build_sheet`]'s voxel dump has no equivalent
/// (there is no bake to report a cache status for), so this is [`build_gi_sheet`]'s alone.
pub(crate) const SHEET_HEADER_H: u32 = 40;
/// Glyph pixel scale for the GI sheet's header lines — matches [`LABEL_SCALE`], since both are
/// secondary text beside a block's own [`TITLE_SCALE`] name.
pub(crate) const HEADER_SCALE: u32 = 2;
/// Glyph pixel scale for a block title — sized to exactly fill [`BLOCK_TITLE_H`] with its margin.
pub(crate) const TITLE_SCALE: u32 = 3;
/// Header strip above each tile, carrying its Y-layer label.
pub(crate) const TILE_LABEL_H: u32 = 20;
/// Glyph pixel scale for a tile's Y-layer label — sized to exactly fill [`TILE_LABEL_H`] with its
/// margin.
pub(crate) const LABEL_SCALE: u32 = 2;
/// Margin, in pixels, a burned label leaves from the top-left of its strip.
pub(crate) const LABEL_MARGIN: u32 = 2;
/// The color an occupied-but-non-emissive cell renders as on the emissive block — dim rather than
/// background, so a wall's silhouette stays visible next to the cells that are actually lit.
const EMISSIVE_FLOOR: [f32; 3] = [0.2, 0.2, 0.22];
/// The color an emissive cell is lerped toward as its share of the grid's own peak emissive value
/// grows — a warm, torch-like tone chosen for contrast against [`EMISSIVE_FLOOR`] and
/// [`BACKGROUND`].
const EMISSIVE_HOT: [f32; 3] = [1.0, 0.78, 0.42];

/// Builds the stitched slice sheet for `grid` — the pure function [`render_voxel_sheet`] wraps with
/// scene I/O. Exercised directly in tests against a hand-built grid, with no scene file involved.
fn build_sheet(grid: &WorldVoxelGrid) -> RenderedImage {
    let [ex, ey, ez] = grid.extent;
    let peak_emissive = grid.emissive.iter().copied().fold(0.0_f32, f32::max);
    let (cols, rows) = near_square(ey);
    let scale = upscale_factor(ex, ez);

    let tile_w = ex * scale;
    let tile_h = ez * scale;
    // `+ 1` on each axis is a deliberately undrawn pixel: the gap between neighbouring tiles (and
    // around the outer edge of the block), for free, with no separate line-drawing pass.
    let cell_w = tile_w + 1;
    let cell_h = TILE_LABEL_H + tile_h + 1;
    let block_w = cols * cell_w + 1;
    let block_h = BLOCK_TITLE_H + rows * cell_h + 1;

    let width = block_w * 2 + BLOCK_GAP;
    let height = block_h;
    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    fill(&mut rgba, width, height, BACKGROUND);

    let albedo_x0 = 0;
    let emissive_x0 = block_w + BLOCK_GAP;
    stamp_text(
        &mut rgba,
        width,
        height,
        albedo_x0 + LABEL_MARGIN,
        LABEL_MARGIN,
        "ALBEDO",
        TITLE_SCALE,
        [255, 255, 255, 255],
    );
    stamp_text(
        &mut rgba,
        width,
        height,
        emissive_x0 + LABEL_MARGIN,
        LABEL_MARGIN,
        "EMISSIVE",
        TITLE_SCALE,
        [255, 255, 255, 255],
    );

    for y in 0..ey {
        let row = y / cols;
        let col = y % cols;
        let tile_x = col * cell_w;
        let tile_y = BLOCK_TITLE_H + row * cell_h;

        let label = format!("Y{y}");
        stamp_text(
            &mut rgba,
            width,
            height,
            albedo_x0 + tile_x + LABEL_MARGIN,
            tile_y + LABEL_MARGIN,
            &label,
            LABEL_SCALE,
            [200, 200, 208, 255],
        );
        stamp_text(
            &mut rgba,
            width,
            height,
            emissive_x0 + tile_x + LABEL_MARGIN,
            tile_y + LABEL_MARGIN,
            &label,
            LABEL_SCALE,
            [200, 200, 208, 255],
        );

        let slice_y = tile_y + TILE_LABEL_H;
        for z in 0..ez {
            for x in 0..ex {
                let occupied = grid.occupancy[grid.index([x, y, z])];
                let albedo_color = if occupied {
                    srgb8(grid.albedo[grid.index([x, y, z])])
                } else {
                    BACKGROUND
                };
                let emissive_color = if occupied {
                    emissive8(grid.emissive[grid.index([x, y, z])], peak_emissive)
                } else {
                    BACKGROUND
                };
                fill_block(
                    &mut rgba,
                    width,
                    height,
                    albedo_x0 + tile_x + x * scale,
                    slice_y + z * scale,
                    scale,
                    scale,
                    albedo_color,
                );
                fill_block(
                    &mut rgba,
                    width,
                    height,
                    emissive_x0 + tile_x + x * scale,
                    slice_y + z * scale,
                    scale,
                    scale,
                    emissive_color,
                );
            }
        }
    }

    RenderedImage {
        width,
        height,
        rgba,
    }
}

/// Builds the stitched GI slice sheet for a `count`-shaped probe lattice — the pure function
/// [`render_gi_sheet`] wraps with scene I/O and the GPU bake/readback. `readback.visibility`,
/// `readback.bounce` and `readback.dynamic` are flat-indexed `x + count.x·(y + count.y·z)`,
/// [`WorldVoxelGrid`]'s own flat order (the shape `Renderer::read_gi_probes` returns). Shares
/// [`build_sheet`]'s layout arithmetic and pixel-writing helpers; only the per-cell colors, the four
/// block labels, and the header strip differ.
///
/// `lights_baked`/`lights_injected`/`bake_cached` are [`render_gi_sheet`]'s own census and
/// [`GiReadback::bake_cached`], stamped into the header rather than recomputed here — this function
/// draws what it is told, so a unit test can hand it a made-up census with no scene or GPU involved.
/// `dynamic_absent` is [`dynamic_absent_reason`]'s output when `readback.dynamic` is `None`, `None`
/// otherwise; passed in rather than recomputed because this function has no `GiData` to derive it
/// from.
#[allow(clippy::too_many_arguments)]
fn build_gi_sheet(
    count: [u32; 3],
    readback: &GiReadback,
    lights_baked: usize,
    lights_injected: usize,
    bake_cached: bool,
    dynamic_absent: Option<&str>,
) -> RenderedImage {
    let [cx, cy, cz] = count;
    let (cols, rows) = near_square(cy);
    let scale = upscale_factor(cx, cz);

    let tile_w = cx * scale;
    let tile_h = cz * scale;
    let cell_w = tile_w + 1;
    let cell_h = TILE_LABEL_H + tile_h + 1;
    let block_w = cols * cell_w + 1;
    let block_h = BLOCK_TITLE_H + rows * cell_h + 1;

    let width = block_w * 4 + BLOCK_GAP * 3;
    let height = SHEET_HEADER_H + block_h;
    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    fill(&mut rgba, width, height, BACKGROUND);

    // The sheet-wide header: the split census and the static bake's cache status, stamped once above
    // every block — the "is something thrashing the bake" answer a forgotten `GiDynamic` marker
    // needs (`Plan/gi-wave3-dynamic-injection.md` §3.1, §10).
    stamp_text(
        &mut rgba,
        width,
        height,
        LABEL_MARGIN,
        LABEL_MARGIN,
        &format!("LIGHTS: {lights_baked} BAKED + {lights_injected} INJECTED"),
        HEADER_SCALE,
        [255, 255, 255, 255],
    );
    stamp_text(
        &mut rgba,
        width,
        height,
        LABEL_MARGIN,
        LABEL_MARGIN + 8 * HEADER_SCALE + 4,
        &format!("BAKE: {}", if bake_cached { "HIT" } else { "MISS" }),
        HEADER_SCALE,
        [200, 200, 208, 255],
    );

    let vis_x0 = 0;
    let normal_x0 = block_w + BLOCK_GAP;
    let bounce_x0 = 2 * (block_w + BLOCK_GAP);
    let dynamic_x0 = 3 * (block_w + BLOCK_GAP);
    let title_y = SHEET_HEADER_H + LABEL_MARGIN;
    stamp_text(
        &mut rgba,
        width,
        height,
        vis_x0 + LABEL_MARGIN,
        title_y,
        "VISIBILITY",
        TITLE_SCALE,
        [255, 255, 255, 255],
    );
    stamp_text(
        &mut rgba,
        width,
        height,
        normal_x0 + LABEL_MARGIN,
        title_y,
        "BENT NORMAL",
        TITLE_SCALE,
        [255, 255, 255, 255],
    );
    // The scene's own bake decides whether there is anything to plot: `bounce: 0.0` (Gate A's
    // default) bakes no bounce volume at all, and the title says so rather than leaving a block that
    // reads as "baked, and just dark" (§8). When baked, the volume's own peak DC component rides in
    // the title beside the fixed exposure, so two sheets stay honestly comparable and a saturated bake
    // is visible as a stated number, not just a guess from the picture.
    let bounce_title = match &readback.bounce {
        Some(dc) => {
            let peak = dc
                .iter()
                .flat_map(|c| c.iter().copied())
                .fold(0.0_f32, f32::max);
            format!("BOUNCE E{BOUNCE_EXPOSURE:.1} M{peak:.1}")
        }
        None => "BOUNCE NOT BAKED".to_string(),
    };
    stamp_text(
        &mut rgba,
        width,
        height,
        bounce_x0 + LABEL_MARGIN,
        title_y,
        &bounce_title,
        TITLE_SCALE,
        [255, 255, 255, 255],
    );
    // DYNAMIC rides the exact same fixed-exposure Reinhard as BOUNCE (`BOUNCE_EXPOSURE`, shared, not
    // a second constant) — one exposure convention across both radiance blocks, so a sheet reads as
    // one instrument (`Plan/gi-wave3-dynamic-injection.md` §10, §15). Absent draws no data and states
    // which of the three content reasons applies, never a bare "off" and never a computed color
    // standing in for nothing.
    let dynamic_title = match &readback.dynamic {
        Some(dc) => {
            let peak = dc
                .iter()
                .flat_map(|c| c.iter().copied())
                .fold(0.0_f32, f32::max);
            format!("DYNAMIC E{BOUNCE_EXPOSURE:.1} M{peak:.1}")
        }
        None => format!(
            "DYNAMIC NOT INJECTED: {}",
            dynamic_absent.unwrap_or("?").to_uppercase()
        ),
    };
    stamp_text(
        &mut rgba,
        width,
        height,
        dynamic_x0 + LABEL_MARGIN,
        title_y,
        &dynamic_title,
        TITLE_SCALE,
        [255, 255, 255, 255],
    );

    for y in 0..cy {
        let row = y / cols;
        let col = y % cols;
        let tile_x = col * cell_w;
        let tile_y = SHEET_HEADER_H + BLOCK_TITLE_H + row * cell_h;

        let label = format!("Y{y}");
        for x0 in [vis_x0, normal_x0, bounce_x0, dynamic_x0] {
            stamp_text(
                &mut rgba,
                width,
                height,
                x0 + tile_x + LABEL_MARGIN,
                tile_y + LABEL_MARGIN,
                &label,
                LABEL_SCALE,
                [200, 200, 208, 255],
            );
        }

        let slice_y = tile_y + TILE_LABEL_H;
        for z in 0..cz {
            for x in 0..cx {
                let idx = (x + cx * (y + cy * z)) as usize;
                let probe = readback.visibility[idx];
                let vis_color = visibility_color(probe[0]);
                let normal_color = bent_normal_color([probe[1], probe[2], probe[3]]);
                // No bounce/dynamic volume at all leaves every cell at the sheet background — the
                // same "nothing to show here" color the VISIBILITY/BENT NORMAL blocks use for an
                // out-of-lattice cell, never a computed color standing in for absent data.
                let bounce_px = match &readback.bounce {
                    Some(dc) => bounce_color(dc[idx]),
                    None => BACKGROUND,
                };
                let dynamic_px = match &readback.dynamic {
                    Some(dc) => bounce_color(dc[idx]),
                    None => BACKGROUND,
                };
                fill_block(
                    &mut rgba,
                    width,
                    height,
                    vis_x0 + tile_x + x * scale,
                    slice_y + z * scale,
                    scale,
                    scale,
                    vis_color,
                );
                fill_block(
                    &mut rgba,
                    width,
                    height,
                    normal_x0 + tile_x + x * scale,
                    slice_y + z * scale,
                    scale,
                    scale,
                    normal_color,
                );
                fill_block(
                    &mut rgba,
                    width,
                    height,
                    bounce_x0 + tile_x + x * scale,
                    slice_y + z * scale,
                    scale,
                    scale,
                    bounce_px,
                );
                fill_block(
                    &mut rgba,
                    width,
                    height,
                    dynamic_x0 + tile_x + x * scale,
                    slice_y + z * scale,
                    scale,
                    scale,
                    dynamic_px,
                );
            }
        }
    }

    RenderedImage {
        width,
        height,
        rgba,
    }
}

/// The SH-L1 DC visibility term (`[0, 1]`) as a greyscale pixel — a direct linear scale, no gamma:
/// this is a debug plot of a normalized scalar, not display color.
fn visibility_color(v: f32) -> [u8; 4] {
    let c = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [c, c, c, 255]
}

/// The L1 "bent normal" vector remapped to RGB, `0.5 + 0.5*v` per channel, clamped to `[0, 1]` —
/// `Plan/gi-wave1-sky-visibility.md` §10's own remap.
fn bent_normal_color(v: [f32; 3]) -> [u8; 4] {
    let encode = |c: f32| ((0.5 + 0.5 * c).clamp(0.0, 1.0) * 255.0).round() as u8;
    [encode(v[0]), encode(v[1]), encode(v[2]), 255]
}

/// The fixed exposure the BOUNCE block's tone map multiplies the baked DC radiance by, before the
/// Reinhard compression — burned into the block's title (`Plan/gi-wave2-emissive-bounce.md` §8) so a
/// future retune cannot make the label lie. `1.0`: no boost, letting the curve's own compression carry
/// the comparability the spec asks for.
const BOUNCE_EXPOSURE: f32 = 1.0;

/// The BOUNCE block's tone map: a fixed-exposure Reinhard curve, `x/(1+x)` after multiplying by
/// [`BOUNCE_EXPOSURE`], then sRGB-encoded via [`srgb8`]'s own gamma. Reinhard rather than
/// per-scene max-normalization is the one that keeps two sheets from two different bakes
/// comparable at a glance (§8): a brighter bake visibly clips toward white instead of silently
/// rescaling to fill the same `[0, 1]` every time, the way a per-scene peak-normalize would. Unlike
/// [`visibility_color`]/[`bent_normal_color`] (a direct, ungammaed plot of a normalized scalar or
/// unit vector), this block draws an actual radiance color, so it earns the same gamma the voxel
/// sheet's albedo tile ([`srgb8`]) gets.
fn bounce_color(dc: [f32; 3]) -> [u8; 4] {
    let encode = |c: f32| -> u8 {
        let exposed = c.max(0.0) * BOUNCE_EXPOSURE;
        let mapped = exposed / (1.0 + exposed);
        (math::powf(mapped, 1.0 / 2.2) * 255.0).round() as u8
    };
    [encode(dc[0]), encode(dc[1]), encode(dc[2]), 255]
}

/// The near-square packing of `n` tiles: `cols = ceil(sqrt(n))`, `rows = ceil(n/cols)` — matching
/// `model_sheet::Grid::near_square`, since cell *area* is what a reading needs (the sweep-sheet
/// precedent this dump follows for the same reason).
pub(crate) fn near_square(n: u32) -> (u32, u32) {
    let n = n.max(1);
    let mut cols = 1;
    while cols * cols < n {
        cols += 1;
    }
    (cols, n.div_ceil(cols))
}

/// The integer upscale a `w`×`h` (cells) tile takes so its larger side reaches at least
/// [`MIN_TILE_PX`], clamped to [`MAX_UPSCALE`]. `1` (no upscale) once the tile is already that large
/// on its own — the district-scale case the spec calls out as already legible at native size.
pub(crate) fn upscale_factor(w: u32, h: u32) -> u32 {
    let largest = w.max(h).max(1);
    if largest >= MIN_TILE_PX {
        1
    } else {
        MIN_TILE_PX.div_ceil(largest).min(MAX_UPSCALE)
    }
}

/// Linear RGB (the grid's own storage) to sRGB8, alpha forced opaque — a debug sheet has no use for
/// translucency, and every committed content path treats voxel/primitive albedo as opaque already.
/// Routed through `ludion_core::math::powf` (pure-Rust `libm`) rather than `f32::powf`, matching the
/// engine-wide rule that a cross-platform-deterministic path never calls the platform transcendental.
fn srgb8(linear: [f32; 4]) -> [u8; 4] {
    let encode = |c: f32| -> u8 {
        let c = c.clamp(0.0, 1.0);
        (math::powf(c, 1.0 / 2.2) * 255.0).round() as u8
    };
    [encode(linear[0]), encode(linear[1]), encode(linear[2]), 255]
}

/// An occupied cell's emissive tile color: [`EMISSIVE_FLOOR`] lerped toward [`EMISSIVE_HOT`] by its
/// emissive strength as a fraction of `peak` (the grid's own maximum, so the sheet self-calibrates to
/// whatever absolute scale the scene authored — a torch registers as hot relative to a wall's zero
/// regardless of whether the scene's emissive units run 1s or 10s). `peak <= 0` (no emissive geometry
/// at all) reports every occupied cell at the floor color, never dividing by zero.
fn emissive8(value: f32, peak: f32) -> [u8; 4] {
    let t = if peak > 0.0 {
        (value / peak).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    [
        (lerp(EMISSIVE_FLOOR[0], EMISSIVE_HOT[0]) * 255.0).round() as u8,
        (lerp(EMISSIVE_FLOOR[1], EMISSIVE_HOT[1]) * 255.0).round() as u8,
        (lerp(EMISSIVE_FLOOR[2], EMISSIVE_HOT[2]) * 255.0).round() as u8,
        255,
    ]
}

// --- Pixel writing (bounds-checked: a label that overruns a small sheet's edge is silently clipped
// rather than panicking — a tiny test grid's block title is the one case this can happen to, and a
// truncated caption is a cosmetic footnote, never a crash). ---

/// Fills the whole buffer with one color.
pub(crate) fn fill(buffer: &mut [u8], width: u32, height: u32, color: [u8; 4]) {
    fill_block(buffer, width, height, 0, 0, width, height, color);
}

/// Fills the `w`×`h` rect at `(x0, y0)` with one color, clipped to the buffer's own bounds.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fill_block(
    buffer: &mut [u8],
    width: u32,
    height: u32,
    x0: u32,
    y0: u32,
    w: u32,
    h: u32,
    color: [u8; 4],
) {
    let stride = width as usize * 4;
    let x_end = (x0 + w).min(width);
    let y_end = (y0 + h).min(height);
    for y in y0..y_end.max(y0) {
        if y >= height {
            break;
        }
        let row = y as usize * stride;
        for x in x0..x_end.max(x0) {
            let idx = row + x as usize * 4;
            buffer[idx..idx + 4].copy_from_slice(&color);
        }
    }
}
// --- The label stamp ---

/// The decoded embedded atlas, once per process.
fn embedded_atlas() -> &'static [u8] {
    static ATLAS: OnceLock<Vec<u8>> = OnceLock::new();
    ATLAS.get_or_init(|| {
        ludion_assets::decode_png(ludion_font::EMBEDDED_PNG, "default.png")
            .expect("the embedded face's atlas is a valid PNG")
            .2
    })
}

/// Stamps `text` into the buffer at `(x0, y0)`, one glyph cell (8 + 1px gap, both times `scale`) per
/// character, left to right, in the embedded face. A character the face lacks draws as blank space
/// rather than erroring — a debug label losing one glyph is not worth failing the whole dump over.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stamp_text(
    buffer: &mut [u8],
    width: u32,
    height: u32,
    x0: u32,
    y0: u32,
    text: &str,
    scale: u32,
    color: [u8; 4],
) {
    ludion_font::stamp_label(
        embedded_atlas(),
        buffer,
        [width, height],
        [x0, y0],
        text,
        scale,
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×1×2 grid, upscaled well past [`MIN_TILE_PX`], with one occupied cell carrying both
    /// albedo and emissive so both blocks have something to show.
    fn tiny_grid() -> WorldVoxelGrid {
        let extent = [2u32, 1, 2];
        let total = 4;
        let mut occupancy = vec![false; total];
        let mut albedo = vec![[0.0; 4]; total];
        let mut emissive = vec![0.0; total];
        // Flat index of (x=1, y=0, z=0) in a `[2, 1, 2]`-extent grid: `x + ex*(y + ey*z)`.
        let index = 1;
        occupancy[index] = true;
        albedo[index] = [0.5, 0.25, 0.125, 1.0];
        emissive[index] = 3.0;
        WorldVoxelGrid {
            origin: ludion_core::math::glam::Vec3::ZERO,
            cell_size: 1.0,
            extent,
            occupancy,
            albedo,
            emissive,
            hash: 0,
        }
        .sealed()
    }

    /// The sheet's pixel size follows exactly from the layout formula: one Y layer means `cols =
    /// rows = 1`, so each block is one tile plus its label strip and border.
    #[test]
    fn sheet_dimensions_match_the_layout_formula() {
        let grid = tiny_grid();
        let image = build_sheet(&grid);

        let scale = upscale_factor(2, 2);
        assert!(scale > 1, "a 2-cell tile must be upscaled for legibility");
        let tile = 2 * scale;
        let cell_w = tile + 1;
        let cell_h = TILE_LABEL_H + tile + 1;
        let block_w = cell_w + 1; // cols = 1
        let block_h = BLOCK_TITLE_H + cell_h + 1; // rows = 1
        assert_eq!(image.width, block_w * 2 + BLOCK_GAP);
        assert_eq!(image.height, block_h);
        assert_eq!(image.rgba.len(), (image.width * image.height * 4) as usize);
    }

    /// The one occupied cell paints its scaled block on both the albedo block (its sRGB-encoded
    /// color) and the emissive block (lerped toward the hot color, since it is the grid's own peak).
    /// A neighbouring empty cell stays background on both.
    #[test]
    fn occupied_cell_paints_both_blocks_and_empty_cells_stay_background() {
        let grid = tiny_grid();
        let image = build_sheet(&grid);
        let scale = upscale_factor(2, 2);

        let albedo_x0 = 0u32;
        let emissive_x0 = (2 * scale + 1) + 1 + BLOCK_GAP;
        let slice_y = BLOCK_TITLE_H + TILE_LABEL_H;

        let pixel = |x: u32, y: u32| -> [u8; 4] {
            let stride = image.width as usize * 4;
            let idx = y as usize * stride + x as usize * 4;
            image.rgba[idx..idx + 4].try_into().unwrap()
        };

        // Cell (x=1, z=0) is occupied: its albedo pixel is the sRGB encode of [0.5, 0.25, 0.125].
        let occupied_albedo = pixel(albedo_x0 + scale, slice_y);
        assert_eq!(occupied_albedo, srgb8([0.5, 0.25, 0.125, 1.0]));

        // Its emissive pixel is the grid's own peak (t = 1.0), i.e. exactly EMISSIVE_HOT.
        let occupied_emissive = pixel(emissive_x0 + scale, slice_y);
        assert_eq!(occupied_emissive, emissive8(3.0, 3.0));
        assert_ne!(occupied_emissive, BACKGROUND);

        // Cell (x=0, z=0) is empty: background on both blocks.
        assert_eq!(pixel(albedo_x0, slice_y), BACKGROUND);
        assert_eq!(pixel(emissive_x0, slice_y), BACKGROUND);
    }

    /// Two builds from the same grid are byte-identical — the pure-CPU determinism the "zero pixels"
    /// contract needs (analogous to the voxelizer's own `double_run_is_bit_identical`).
    #[test]
    fn double_build_is_bit_identical() {
        let grid = tiny_grid();
        let a = build_sheet(&grid);
        let b = build_sheet(&grid);
        assert_eq!(a.width, b.width);
        assert_eq!(a.height, b.height);
        assert_eq!(a.rgba, b.rgba);
    }

    /// A grid with no emissive geometry at all (`peak == 0`) reports every occupied cell at the
    /// floor color rather than dividing by zero.
    #[test]
    fn zero_peak_emissive_uses_the_floor_color_everywhere() {
        let color = emissive8(0.0, 0.0);
        let expected = [
            (EMISSIVE_FLOOR[0] * 255.0).round() as u8,
            (EMISSIVE_FLOOR[1] * 255.0).round() as u8,
            (EMISSIVE_FLOOR[2] * 255.0).round() as u8,
            255,
        ];
        assert_eq!(color, expected);
    }

    /// A grid already at or past [`MIN_TILE_PX`] on its larger axis is left at native 1:1 — the
    /// district-scale case the spec calls "already legible".
    #[test]
    fn a_large_grid_is_not_upscaled() {
        assert_eq!(upscale_factor(264, 288), 1);
    }

    /// The near-square packing matches `model_sheet`'s sweep grid rule.
    #[test]
    fn near_square_packs_as_expected() {
        assert_eq!(near_square(1), (1, 1));
        assert_eq!(near_square(4), (2, 2));
        assert_eq!(near_square(5), (3, 2));
        assert_eq!(near_square(16), (4, 4));
    }

    // --- `Plan/gi-wave3-dynamic-injection.md` §11 T11: `dynamic_absent_reason`'s three cases ---

    /// A minimal, otherwise-arbitrary `GiData` — every field but `bounce` is inert to
    /// [`dynamic_absent_reason`], so a `1.0` filler stands in for the ones this test does not vary.
    fn gi_data(bounce: f32) -> GiData {
        GiData {
            probe_spacing: 1.0,
            normal_bias: 0.25,
            sky_visibility: 1.0,
            bounce,
            count: [2, 2, 2],
            origin: ludion_core::math::glam::Vec3::ZERO,
            span: ludion_core::math::glam::Vec3::ONE,
        }
    }

    /// **No `GiVolume`** — unreachable through [`render_gi_sheet`] today (see the function's own
    /// doc), exercised directly so the reason is proven correct rather than merely assumed.
    #[test]
    fn dynamic_absent_reason_names_a_missing_gi_volume() {
        assert_eq!(dynamic_absent_reason(None, true), "no GiVolume");
        // `any_marked` cannot rescue a missing `GiVolume` — there is nowhere to inject into.
        assert_eq!(dynamic_absent_reason(None, false), "no GiVolume");
    }

    /// **`bounce: 0.0`** takes precedence over the marked-lights question, matching
    /// `gi_inject::scheduled_lattice`'s own conjunction order: the injected term *is* the bounce
    /// term, so an unbounced `GiVolume` never injects, marked lights or not.
    #[test]
    fn dynamic_absent_reason_names_a_zero_bounce_even_when_marked() {
        assert_eq!(
            dynamic_absent_reason(Some(&gi_data(0.0)), true),
            "bounce: 0.0"
        );
        assert_eq!(
            dynamic_absent_reason(Some(&gi_data(0.0)), false),
            "bounce: 0.0"
        );
    }

    /// **No `GiDynamic`-marked light** — a bouncing `GiVolume` with nothing marked.
    #[test]
    fn dynamic_absent_reason_names_no_marked_lights() {
        assert_eq!(
            dynamic_absent_reason(Some(&gi_data(1.0)), false),
            "no GiDynamic lights"
        );
    }
}
