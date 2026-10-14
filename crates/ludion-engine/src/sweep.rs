//! The sweep harness: one scene rendered N times, one value per cell, stitched into one labelled
//! **contact sheet**.
//!
//! [`render_sweep_sheet`] takes a scene file, the fixed `--set` overrides that apply to every cell,
//! one swept JSON Pointer, and the values to try there. Each value produces one render of the same
//! scene with that value written in memory (never to disk), and the cells are stitched into a single
//! RGBA8 [`RenderedImage`] with the value that produced each burned into it.
//!
//! It exists for the calls a number cannot make. `ludion image-stats` gates *defects* — banding,
//! aliasing, clipping — and a sweep sheet answers *taste*: which exposure, which bloom strength,
//! which grade. One image, N candidates, side by side, so the question is a comparison rather than a
//! memory of the last render.
//!
//! Everything here is a deterministic pure function of the inputs: the same scene, overrides,
//! values, and sheet size produce byte-identical PNG bytes on the same machine. All CPU work (read,
//! parse, override, validate, instantiate, tick 0, extract — once per cell) happens **before** any
//! GPU adapter is requested, matching [`render_scene`](crate::render_scene)'s discipline, so an
//! unresolvable pointer or an ill-typed value fails with its diagnostics on an adapterless machine.
//! One [`Renderer`] then serves every cell: a sweep costs one adapter, not N.
//!
//! # Sheet layout
//!
//! The grid is near-square — `cols = ceil(sqrt(N))`, `rows = ceil(N / cols)` — because cell *area*
//! is what a taste call needs: five candidates in one row of a 1920-wide sheet are 384 px across and
//! useless, while the same five at 3×2 are 640 px. `cell_w = sheet_width / cols` and
//! `cell_h = sheet_height / rows` (integer division, so the sheet may come back slightly smaller
//! than requested). Values fill row-major in the order given; a leftover cell (`N = 3`, `5`, `7`, …)
//! is a solid `UNUSED_CELL` fill. The stitch is a plain CPU RGBA8 row-copy — no wall clock, no
//! threads — so it is deterministic. At most [`MAX_SWEEP_CELLS`] values are accepted; past that the
//! cells are too small to judge, which is the only thing a sheet is for.
//!
//! # The label, and why a sheet is not a golden
//!
//! Each cell's label is a screen-space `UiText` entity appended to that cell's scene — the engine has
//! no CPU font rasterizer and needs none. The appended entity is a direct scene mutation, not an
//! override: [`apply_overrides`](ludion_scene::apply_overrides) deliberately cannot create an entity,
//! and that restriction is worth keeping intact.
//!
//! The consequence is worth stating plainly: a `UiText` makes the frame graph schedule its overlay
//! pass. That pass runs *after* all post-processing, so it can never affect the imaging being judged
//! — but it does mean **a sweep cell is not byte-identical to the same `render --set` invocation**.
//! A contact sheet is a judging instrument, not a golden.

use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::Registry;
use ludion_renderer::{
    FrameSize, QualityParams, RenderScale, RenderScene, RenderedImage, Renderer,
};
use ludion_scene::{AppliedOverride, Scene, SceneEntity, SceneOverride};
use serde_json::{Value, json};

use crate::builtin_registry;
use crate::extract::{extract, extract_at};

/// The most values one sweep may carry. Past sixteen the near-square grid puts cells below a quarter
/// of the sheet's linear size in each axis, which is too small for the comparison the sheet exists to
/// support — split the range into several sheets instead.
///
/// Shared with the model sheet's own palette sweep (`crate::model_sheet`): the cap is a property of
/// what a human can compare in one image, not of what is being swept, so the two must agree.
pub const MAX_SWEEP_CELLS: usize = 16;

/// Solid fill for a grid cell with no value (any `N` that is not a perfect fit), a deliberately dark
/// neutral gray — the same neutral the model sheet uses, so the two sheets read as one family.
const UNUSED_CELL: [u8; 4] = [24, 26, 30, 255];
/// Label glyph cell height, in the 1080-tall UI canvas units the overlay measures in (`crate::ui`).
/// Legible at the default sheet size without swamping the image being judged.
const LABEL_SIZE: f32 = 52.0;
/// Label inset from the cell's top-left corner, in canvas units (both axes).
const LABEL_OFFSET: f32 = 8.0;
/// Warm yellow: reads over both a night scene and a blown-out one.
const LABEL_COLOR: [f32; 4] = [1.0, 0.9, 0.25, 1.0];

/// What one cell of the sheet rendered.
#[derive(Clone, Debug, PartialEq)]
pub struct SweepCell {
    /// The value written at the swept pointer for this cell.
    pub value: Value,
    /// The text burned into the cell (the value, then the pointer's leaf segment).
    pub label: String,
    /// The cell's row in the grid, zero-based.
    pub row: u32,
    /// The cell's column in the grid, zero-based.
    pub col: u32,
}

/// The result of a sweep: the stitched contact sheet plus everything a caller must echo, since an
/// override silently disagreeing with the file on disk is a footgun.
#[derive(Debug)]
pub struct SweepOutcome {
    /// The stitched contact-sheet image (RGBA8).
    pub image: RenderedImage,
    /// A human-readable adapter description, e.g. `"NVIDIA … (Vulkan)"`.
    pub adapter: String,
    /// The pointer that was swept.
    pub pointer: String,
    /// The value the scene held at `pointer` before the sweep wrote over it (after the fixed
    /// overrides, which is what every cell actually started from), or `None` if the scene did not
    /// author that field at all and each cell created it.
    pub original: Option<Value>,
    /// What each fixed override changed, in the order they were given.
    pub applied_base: Vec<AppliedOverride>,
    /// The grid's column count.
    pub cols: u32,
    /// The grid's row count.
    pub rows: u32,
    /// What each cell rendered, in sheet order (row-major).
    pub cells: Vec<SweepCell>,
}

/// Renders one contact sheet of the scene at `path`, one cell per entry in `values`, each written at
/// `pointer` over the scene in memory.
///
/// `base` is applied to every cell before the swept value, so a sweep of one knob at a fixed setting
/// of another — an exposure sweep at a chosen bloom strength — is the ordinary case. The file on disk
/// is never written.
///
/// The sheet is at most `width` × `height`; integer cell division may shrink it slightly. Each cell
/// carries a burned-in label naming the value it rendered, which schedules the overlay pass — see the
/// module doc: **a sweep cell is not byte-identical to the same `render --set` invocation**, and a
/// sheet is a judging instrument, never a golden.
///
/// `scale` supersamples every cell. A sweep is the arc's taste instrument, and a taste call should
/// not be made on an aliased image, so the flag applies here exactly as it does to a plain render —
/// per *cell*, since that is the frame being judged.
///
/// # Errors
/// Every failure is a `Vec<Diagnostic>`:
/// - `L4008` if `values` is empty or longer than [`MAX_SWEEP_CELLS`], or if an override's pointer
///   (fixed or swept) does not resolve in the scene.
/// - `L1015` if the file cannot be read; `L1001`–`L1014` for parse/validation problems — a swept
///   value the schema rejects is the ordinary `L1014`, exactly as the same edit to the file would be.
/// - `L0004`/`L0006` for instantiation problems; `L1101`/`L1102`/`L2101` from [`extract`].
/// - `L2003`/`L2004`/`L2005` from the GPU (adapter, device, or readback).
///
/// The first cell whose scene fails CPU validation reports its diagnostics and the sweep stops; the
/// GPU is never touched.
pub fn render_sweep_sheet(
    path: &Path,
    base: &[SceneOverride],
    pointer: &str,
    values: &[Value],
    width: u32,
    height: u32,
    scale: RenderScale,
) -> Result<SweepOutcome, Vec<Diagnostic>> {
    if values.is_empty() {
        return Err(vec![sweep_size_error(
            pointer,
            "a sweep needs at least one value",
            "give `--sweep <pointer>=<v,v,…>` at least one value to try",
        )]);
    }
    if values.len() > MAX_SWEEP_CELLS {
        return Err(vec![sweep_size_error(
            pointer,
            format!(
                "a sweep of {} values does not fit a contact sheet; at most {MAX_SWEEP_CELLS} cells \
                 stay large enough to judge",
                values.len()
            ),
            "split the range across several sheets, or sweep fewer candidates at a time",
        )]);
    }

    let layout = SheetLayout::compute(values.len() as u32, width, height);

    // --- All CPU-side work happens before the GPU is requested. ---
    let source = std::fs::read_to_string(path).map_err(|error| {
        vec![
            Diagnostic::new(
                code::SCENE_IO,
                format!("could not read `{}`: {error}", path.display()),
            )
            .with_help("check the path (relative to the current directory) and permissions"),
        ]
    })?;
    let project_root = path.parent().unwrap_or(Path::new("."));

    let context = CellContext {
        registry: builtin_registry(),
        source: &source,
        base,
        pointer,
        project_root,
        layout: &layout,
    };
    let mut render_scenes = Vec::with_capacity(values.len());
    let mut cells = Vec::with_capacity(values.len());
    let mut applied_base = Vec::new();
    let mut original = None;
    for (index, value) in values.iter().enumerate() {
        let label = cell_label(pointer, value);
        let prepared = prepare_cell(&context, value, &label)?;
        if index == 0 {
            applied_base = prepared.applied_base;
            original = prepared.original;
        }
        render_scenes.push(prepared.scene);
        cells.push(SweepCell {
            value: value.clone(),
            label,
            row: index as u32 / layout.cols,
            col: index as u32 % layout.cols,
        });
    }

    // --- GPU work: one renderer, reused across every cell. ---
    let renderer = Renderer::new().map_err(|d| vec![d])?;
    let adapter = format!(
        "{} ({})",
        renderer.adapter_name(),
        renderer.adapter_backend()
    );

    let mut sheet = vec![0u8; layout.sheet_w as usize * layout.sheet_h as usize * 4];
    fill(&mut sheet, UNUSED_CELL);
    for (cell, scene) in cells.iter().zip(&render_scenes) {
        let pixels = renderer
            .render(
                scene,
                FrameSize::scaled(layout.cell_w, layout.cell_h, scale),
            )
            .map_err(|d| vec![d])?;
        blit_cell(&mut sheet, &layout, &pixels, cell.row, cell.col);
    }

    Ok(SweepOutcome {
        image: RenderedImage {
            width: layout.sheet_w,
            height: layout.sheet_h,
            rgba: sheet,
        },
        adapter,
        pointer: pointer.to_string(),
        original,
        applied_base,
        cols: layout.cols,
        rows: layout.rows,
        cells,
    })
}

/// What one cell of a `--frames a,b,c` time strip rendered.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameCell {
    /// The tick count the world was run to before this cell was extracted.
    pub frame: u64,
    /// The text burned into the cell (`frame N`).
    pub label: String,
}

/// The result of a time strip: the stitched image plus everything a caller must echo.
#[derive(Debug)]
pub struct FrameStripOutcome {
    /// The stitched strip image (RGBA8).
    pub image: RenderedImage,
    /// A human-readable adapter description, e.g. `"NVIDIA … (Vulkan)"`.
    pub adapter: String,
    /// What each fixed override changed, in the order they were given.
    pub applied_base: Vec<AppliedOverride>,
    /// The strip's column count — one per entry in `frames`, left to right.
    pub cols: u32,
    /// What each cell rendered, in strip order.
    pub cells: Vec<FrameCell>,
}

/// Renders one labelled horizontal strip of the scene at `path`, one cell per entry in `frames`: the
/// world ticked that many times over the empty built-in schedule (no game systems run — see the
/// module header on `crate::render`) before the cell is extracted at the sim time that reaches.
/// `ludion render --frames a,b,c`, the instrument for judging motion — a burst particle emitter, an
/// assembly clip — without writing a test, since a burst is invisible at tick 0.
///
/// Reuses the sweep sheet's own grid/label/blit machinery (`SheetLayout`, `push_label_entity`,
/// `blit_cell`) with the grid forced to one row: a time strip reads left to right in ascending
/// frame order, never wrapped, so — unlike [`render_sweep_sheet`]'s near-square grid — every column
/// is a requested frame and `UNUSED_CELL` never appears.
///
/// `base` (the `--set` overrides) applies to every cell, exactly as in [`render_sweep_sheet`]. The
/// file on disk is never written. `frames`' ascending order is the caller's responsibility
/// (`ludion render`'s clap parsing validates it); the count is bounded here, as every cell
/// re-simulates the world from its start.
///
/// # Errors
/// As [`render_sweep_sheet`], minus the swept-pointer cases (there is no swept pointer here):
/// `L4008` for more than [`MAX_SWEEP_CELLS`] frames, `L1015`, `L1001`–`L1014` (read/parse/validate), `L0004`/`L0006` (instantiate),
/// `L1101`/`L1102`/`L2101` (extract), `L2003`/`L2004`/`L2005` (the GPU).
pub fn render_frame_strip(
    path: &Path,
    base: &[SceneOverride],
    frames: &[u64],
    width: u32,
    height: u32,
    scale: RenderScale,
) -> Result<FrameStripOutcome, Vec<Diagnostic>> {
    if frames.len() > MAX_SWEEP_CELLS {
        return Err(vec![
            Diagnostic::new(
                code::SCENE_OVERRIDE_INVALID,
                format!(
                    "a strip of {} frames does not fit a contact sheet; at most {MAX_SWEEP_CELLS}                      cells stay large enough to judge",
                    frames.len()
                ),
            )
            .with_help("split the frames across several strips"),
        ]);
    }
    let layout = SheetLayout::row(frames.len() as u32, width, height);

    // --- All CPU-side work happens before the GPU is requested. ---
    let source = std::fs::read_to_string(path).map_err(|error| {
        vec![
            Diagnostic::new(
                code::SCENE_IO,
                format!("could not read `{}`: {error}", path.display()),
            )
            .with_help("check the path (relative to the current directory) and permissions"),
        ]
    })?;
    let project_root = path.parent().unwrap_or(Path::new("."));
    let registry = builtin_registry();

    let mut render_scenes = Vec::with_capacity(frames.len());
    let mut cells = Vec::with_capacity(frames.len());
    let mut applied_base = Vec::new();
    for (index, &frame) in frames.iter().enumerate() {
        let label = format!("frame {frame}");
        let mut scene = ludion_scene::parse(&source)?;
        let applied = ludion_scene::apply_overrides(&mut scene, base)?;
        if index == 0 {
            applied_base = applied;
        }
        push_label_entity(&mut scene, &label);

        let diagnostics = ludion_scene::validate(&scene, &registry);
        if !diagnostics.is_empty() {
            return Err(diagnostics);
        }
        let (scene, world) = crate::render::expanded_world(project_root, &registry, &scene)?;
        let mut driver = crate::scene_physics::bare_driver(world, &scene, project_root);
        driver.run_ticks(frame);
        if let Some(d) = driver.take_system_panic() {
            return Err(vec![d]);
        }

        render_scenes.push(extract_at(
            driver.world(),
            (layout.cell_w, layout.cell_h),
            project_root,
            driver.time(),
            QualityParams::FULL.particle_density,
        )?);
        cells.push(FrameCell { frame, label });
    }

    // --- GPU work: one renderer, reused across every cell. ---
    let renderer = Renderer::new().map_err(|d| vec![d])?;
    let adapter = format!(
        "{} ({})",
        renderer.adapter_name(),
        renderer.adapter_backend()
    );

    let mut strip = vec![0u8; layout.sheet_w as usize * layout.sheet_h as usize * 4];
    for (col, scene) in render_scenes.iter().enumerate() {
        let pixels = renderer
            .render(
                scene,
                FrameSize::scaled(layout.cell_w, layout.cell_h, scale),
            )
            .map_err(|d| vec![d])?;
        blit_cell(&mut strip, &layout, &pixels, 0, col as u32);
    }

    Ok(FrameStripOutcome {
        image: RenderedImage {
            width: layout.sheet_w,
            height: layout.sheet_h,
            rgba: strip,
        },
        adapter,
        applied_base,
        cols: layout.cols,
        cells,
    })
}

/// One cell's CPU result: the extracted scene plus what the overrides changed on the way there.
struct PreparedCell {
    scene: RenderScene,
    applied_base: Vec<AppliedOverride>,
    original: Option<Value>,
}

/// Everything a cell needs that is the same for all of them. The registry in particular is built
/// once: it generates every component's schema, which is far too much work to repeat per cell.
struct CellContext<'a> {
    registry: Registry,
    source: &'a str,
    base: &'a [SceneOverride],
    pointer: &'a str,
    project_root: &'a Path,
    layout: &'a SheetLayout,
}

/// The CPU half for one cell: parse → fixed overrides → the swept value → append the label entity →
/// validate → instantiate → tick 0 → extract.
///
/// Overrides land between parse and validation, as everywhere: the schema then rejects an ill-typed
/// swept value with the `L1014` it would give the same value written into the file.
fn prepare_cell(
    context: &CellContext,
    value: &Value,
    label: &str,
) -> Result<PreparedCell, Vec<Diagnostic>> {
    let CellContext {
        registry,
        source,
        base,
        pointer,
        project_root,
        layout,
    } = context;
    let mut scene = ludion_scene::parse(source)?;

    let applied_base = ludion_scene::apply_overrides(&mut scene, base)?;
    let swept = [SceneOverride {
        pointer: pointer.to_string(),
        value: value.clone(),
    }];
    let original = ludion_scene::apply_overrides(&mut scene, &swept)?
        .pop()
        .and_then(|change| change.from);

    push_label_entity(&mut scene, label);

    let diagnostics = ludion_scene::validate(&scene, registry);
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let (scene, world) = crate::render::expanded_world(project_root, registry, &scene)?;
    let mut driver = crate::scene_physics::bare_driver(world, &scene, project_root);
    driver.run_ticks(0);

    let scene = extract(driver.world(), (layout.cell_w, layout.cell_h), project_root)?;
    Ok(PreparedCell {
        scene,
        applied_base,
        original,
    })
}

/// Appends the burned-in label as a screen-space `UiText` entity.
///
/// Appended last so every `/entities/N` pointer in the caller's overrides still addresses the entity
/// the file put there, and so the lowest-`EntityId` singleton rules (`PostProcess`, `Environment`)
/// pick the authored component, never this one. `order` is the maximum so the label draws over any
/// UI the scene itself carries.
fn push_label_entity(scene: &mut Scene, label: &str) {
    scene.entities.push(SceneEntity {
        presentation: false,
        name: Some("sweep-label".to_string()),
        components: vec![(
            "UiText".to_string(),
            json!({
                "anchor": "top_left",
                "offset": [LABEL_OFFSET, LABEL_OFFSET],
                "text": label,
                "size": LABEL_SIZE,
                "color": LABEL_COLOR,
                "order": i32::MAX,
            }),
        )],
        instance: None,
    });
}

/// The label for one cell: the value, then the pointer's leaf segment on a second line.
///
/// Two lines rather than `leaf = value` because a short longest-line keeps a long pointer inside a
/// narrow cell. The **value goes first**: it is the one thing that differs between cells, and the
/// top line is the least likely to land on content.
fn cell_label(pointer: &str, value: &Value) -> String {
    let leaf = pointer.rsplit('/').next().unwrap_or(pointer);
    let value = match value {
        // A JSON string renders without its quotes: they add width and no information.
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    format!("{value}\n{leaf}")
}

/// `L4008` for a value list that cannot become a sheet.
fn sweep_size_error(pointer: &str, message: impl Into<String>, help: &str) -> Diagnostic {
    Diagnostic::new(code::SCENE_OVERRIDE_INVALID, message.into())
        .with_path(pointer.to_string())
        .with_help(help.to_string())
}

/// The grid geometry of the sheet, derived from the value count and requested size.
struct SheetLayout {
    cols: u32,
    rows: u32,
    cell_w: u32,
    cell_h: u32,
    sheet_w: u32,
    sheet_h: u32,
}

impl SheetLayout {
    /// Near-square: `cols = ceil(sqrt(N))`, `rows = ceil(N / cols)`. Cells divide the requested size
    /// (integer), and the final sheet is exactly `cell_w·cols × cell_h·rows`.
    fn compute(n: u32, sheet_width: u32, sheet_height: u32) -> SheetLayout {
        let n = n.max(1);
        // Integer ceil(sqrt(n)) without transcendentals: the smallest `c` with `c*c >= n`.
        let mut cols = 1;
        while cols * cols < n {
            cols += 1;
        }
        let rows = n.div_ceil(cols);
        let cell_w = (sheet_width / cols).max(1);
        let cell_h = (sheet_height / rows).max(1);
        SheetLayout {
            cols,
            rows,
            cell_w,
            cell_h,
            sheet_w: cell_w * cols,
            sheet_h: cell_h * rows,
        }
    }

    /// One row, `cols = n`: [`render_frame_strip`]'s layout — a time strip reads left to right and
    /// never wraps, so unlike [`compute`](Self::compute)'s near-square grid, no cell goes unused and
    /// no count is ever padded.
    fn row(n: u32, sheet_width: u32, sheet_height: u32) -> SheetLayout {
        let cols = n.max(1);
        let cell_w = (sheet_width / cols).max(1);
        let cell_h = sheet_height.max(1);
        SheetLayout {
            cols,
            rows: 1,
            cell_w,
            cell_h,
            sheet_w: cell_w * cols,
            sheet_h: cell_h,
        }
    }
}

/// Fills an RGBA8 buffer with one color.
fn fill(buffer: &mut [u8], color: [u8; 4]) {
    for pixel in buffer.chunks_exact_mut(4) {
        pixel.copy_from_slice(&color);
    }
}

/// Row-copies one rendered cell into the sheet at grid position `(row, col)`.
fn blit_cell(sheet: &mut [u8], layout: &SheetLayout, cell: &RenderedImage, row: u32, col: u32) {
    let row_bytes = (layout.cell_w * 4) as usize;
    let sheet_stride = (layout.sheet_w * 4) as usize;
    let x_offset = (col * layout.cell_w * 4) as usize;
    for r in 0..layout.cell_h as usize {
        let src = &cell.rgba[r * row_bytes..r * row_bytes + row_bytes];
        let dst_y = (row * layout.cell_h) as usize + r;
        let dst = dst_y * sheet_stride + x_offset;
        sheet[dst..dst + row_bytes].copy_from_slice(src);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The near-square grid rule, over every accepted cell count.
    #[test]
    fn grid_is_near_square() {
        let cases = [
            (1u32, 1u32, 1u32),
            (2, 2, 1),
            (3, 2, 2),
            (4, 2, 2),
            (5, 3, 2),
            (6, 3, 2),
            (7, 3, 3),
            (9, 3, 3),
            (10, 4, 3),
            (12, 4, 3),
            (13, 4, 4),
            (16, 4, 4),
        ];
        for (n, cols, rows) in cases {
            let layout = SheetLayout::compute(n, 1920, 1080);
            assert_eq!((layout.cols, layout.rows), (cols, rows), "N={n}");
            assert!(layout.cols * layout.rows >= n, "N={n} has too few cells");
        }
    }

    /// Cells divide the requested size and the sheet is exactly the cell grid — never larger than
    /// asked for.
    #[test]
    fn cell_and_sheet_dimensions() {
        // N=5 → 3×2; 1920/3 = 640, 1080/2 = 540 — the case a single row would render useless.
        let layout = SheetLayout::compute(5, 1920, 1080);
        assert_eq!((layout.cell_w, layout.cell_h), (640, 540));
        assert_eq!((layout.sheet_w, layout.sheet_h), (1920, 1080));

        // Integer division may shrink the sheet: 1000/3 = 333.
        let ragged = SheetLayout::compute(7, 1000, 1000);
        assert_eq!((ragged.cell_w, ragged.cell_h), (333, 333));
        assert_eq!((ragged.sheet_w, ragged.sheet_h), (999, 999));
    }

    /// A degenerate sheet size still produces at least one pixel per cell.
    #[test]
    fn tiny_sheets_do_not_collapse_to_zero() {
        let layout = SheetLayout::compute(9, 1, 1);
        assert_eq!((layout.cell_w, layout.cell_h), (1, 1));
        assert_eq!((layout.sheet_w, layout.sheet_h), (3, 3));
    }

    /// The label is the value then the pointer's leaf, with strings unquoted (quotes add width and
    /// no information).
    #[test]
    fn labels_name_the_value_and_the_leaf() {
        assert_eq!(
            cell_label(
                "/entities/0/components/PostProcess/bloom_strength",
                &json!(0.8)
            ),
            "0.8\nbloom_strength",
        );
        assert_eq!(cell_label("/name", &json!("night")), "night\nname");
        assert_eq!(cell_label("/seed", &Value::Null), "null\nseed");
    }

    /// The label rides on an appended entity, never an override — `apply_overrides` cannot create
    /// one, and appending keeps every authored `/entities/N` pointer addressing what the file put
    /// there.
    #[test]
    fn the_label_entity_is_appended_last() {
        let mut scene =
            ludion_scene::parse("format: 1\nentities:\n  - name: hero\n    components: {}\n")
                .unwrap();
        push_label_entity(&mut scene, "x\n1");
        assert_eq!(scene.entities.len(), 2);
        assert_eq!(scene.entities[0].name.as_deref(), Some("hero"));
        let (name, value) = &scene.entities[1].components[0];
        assert_eq!(name, "UiText");
        assert_eq!(value["text"], json!("x\n1"));
    }

    /// An empty or oversized value list is an `L4008`, refused before any file is read.
    #[test]
    fn value_counts_outside_the_sheet_are_rejected() {
        let missing = Path::new("does-not-exist.yaml");
        let empty = render_sweep_sheet(missing, &[], "/seed", &[], 64, 64, RenderScale::NATIVE)
            .unwrap_err();
        assert_eq!(empty[0].code.as_str(), "L4008");

        let too_many: Vec<Value> = (0..=MAX_SWEEP_CELLS).map(|i| json!(i)).collect();
        let over = render_sweep_sheet(
            missing,
            &[],
            "/seed",
            &too_many,
            64,
            64,
            RenderScale::NATIVE,
        )
        .unwrap_err();
        assert_eq!(over[0].code.as_str(), "L4008");
        assert!(over[0].message.contains(&MAX_SWEEP_CELLS.to_string()));
    }

    /// A frame strip over the cell cap is an `L4008`, refused before any file is read.
    #[test]
    fn a_frame_strip_over_the_cell_cap_is_rejected() {
        let missing = Path::new("does-not-exist.yaml");
        let frames: Vec<u64> = (0..=MAX_SWEEP_CELLS as u64).collect();
        let over =
            render_frame_strip(missing, &[], &frames, 64, 64, RenderScale::NATIVE).unwrap_err();
        assert_eq!(over[0].code.as_str(), "L4008");
        assert!(over[0].message.contains(&MAX_SWEEP_CELLS.to_string()));
    }
}
