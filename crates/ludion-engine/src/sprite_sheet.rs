//! `ludion render --sprite` and `--atlas` — the two CPU-composed sprite instruments: a sprite
//! document's **contact sheet** and a packed atlas's **overview**.
//!
//! Both are the "zero pixels" shape [`crate::voxel_sheet`] established: every byte is written by
//! integer arithmetic into an RGBA8 buffer, no GPU adapter is ever requested, no wall clock is read,
//! and no scene is instantiated — so two runs over the same inputs write byte-identical bytes and
//! either sheet answers on a machine with no adapter at all. They reuse that module's 8×8 bitmap
//! font and blit helpers rather than growing a third copy of either.
//!
//! # The contact sheet ([`render_sprite_sheet`])
//!
//! One image answering "what did this document bake to, and what did the effect list do to it":
//!
//! - a **header** naming the document, its texel size, its frame count and its seed;
//! - **block A**, every frame nearest-upscaled to legibility ([`crate::voxel_sheet::upscale_factor`])
//!   and laid out near-square, each labelled, each carrying its pivot as a cross — and the first one
//!   carrying a **texel ruler** along its top and left edges, so texels can be *counted* on screen
//!   rather than estimated; a checkerboard shows behind every transparent texel, and at an upscale
//!   of [`GRID_MIN_UPSCALE`] or more a texel grid (every eighth line darker) separates the texels;
//! - **block B**, every frame at the authored game scale in one row — the "is it readable at the
//!   size it ships at" half, which no amount of upscaling can answer;
//! - a **footer** echoing the effect list that was applied, in application order, and what each
//!   `--set` override changed (`Plan/sprite-engine.md`, "Data and bake": a gradient, glow, shadow or
//!   shading painted into glyphs is a defect, so the sheet must say out loud which ops it ran).
//!
//! With a `sweep` the sheet becomes a near-square grid of *candidates* instead: one cell per swept
//! value, each cell that document's frames in a row, labelled with the value. Same footer, same
//! header, one baked document per cell.
//!
//! With `steps` the sheet becomes a **steps sheet** instead: one row per frame, the authored paint
//! first and then the image after each effect that applies to it, cumulatively, each cell captioned
//! with that effect's echo line — "which op made this change".
//!
//! # The atlas overview ([`render_atlas_sheet`])
//!
//! The packed image beside its own index: the committed PNG composited over the sheet background,
//! nearest-upscaled, with every region outlined and named and every pivot crossed. It answers "did
//! the packer put the regions where the document says" without opening two files side by side.
//!
//! # The flipbook instruments ([`render_atlas_clip`])
//!
//! The same atlas read through one of its `flipbooks:` instead of through its packing — three
//! images over one clip, one flag apart:
//!
//! - **one instant**: the entry showing at a given second, ruled and labelled with the time and the
//!   region name — "what is on screen at t";
//! - **a strip**: `n` evenly spaced phases in a row, spaced by the clip's own `loop:` — a one-shot
//!   exactly as `render --model --strip` spaces an assembly clip's, a loop sampled at the *centre* of
//!   each phase's span rather than its opening instant (see [`clip_phase_times`]) — "does the whole
//!   cycle read";
//! - **an onion skin**: every entry of the clip overlaid in one cell, aligned by the regions' own
//!   pivots and ghosted behind the one showing — "does the motion arc, or does it wobble".
//!
//! All three go through [`ludion_scene::flipbook_frame`], the one evaluator a drawn sprite uses, so
//! the instrument cannot disagree with the game about which entry an instant shows.
//!
//! # Why the labels are burned in
//!
//! Exactly [`crate::voxel_sheet`]'s reason: the alternative is a `UiText` entity and a real GPU
//! overlay pass, and these two instruments must work with no adapter. The face is the embedded 8×8
//! one, stamped through [`crate::voxel_sheet::stamp_text`]. It is an uppercase face (lowercase
//! aliases to uppercase), so a lowercase palette glyph (`o`) is burned as `O` while the
//! [`SpriteSheetOutcome::effects`] strings keep the authored spelling.

use std::path::{Path, PathBuf};

use ludion_assets::Image;
use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, value_phrase};
use ludion_scene::{
    AppliedOverride, AtlasDocument, AtlasRegion, Flipbook, GradientBlend, GradientDirection,
    OutlineSide, SceneOverride, SpriteDocument, SpriteEffect, SpriteEffectOp,
};
use ludion_sprite::BakedFrame;
use serde_json::Value;

use crate::voxel_sheet::{
    BACKGROUND, BLOCK_GAP, BLOCK_TITLE_H, HEADER_SCALE, LABEL_MARGIN, LABEL_SCALE, SHEET_HEADER_H,
    TILE_LABEL_H, TITLE_SCALE, fill, fill_block, near_square, stamp_text, upscale_factor,
};

// --- Layout constants ---------------------------------------------------------------------------

/// The band reserved on a block-A tile's top and left edges for the texel ruler. Wide enough for a
/// two-digit index at [`RULER_SCALE`] (`2 × 9 = 18` px), and reserved on **every** cell so the grid
/// stays regular even though only the first tile draws ticks.
const RULER_BAND: u32 = 20;
/// A ruler tick's length, in pixels, for a texel that is not a multiple of four.
const RULER_TICK: u32 = 3;
/// A ruler tick's length for every fourth texel — the coarse count an eye can follow.
const RULER_LONG_TICK: u32 = 6;
/// Glyph scale for a ruler's stamped indices: native 8×8, the smallest text on the sheet, because a
/// ruler index must not be wider than the texels it counts.
const RULER_SCALE: u32 = 1;
/// The texel interval a ruler index is stamped at, on an axis longer than [`RULER_FINE_EXTENT`].
const RULER_INDEX_STEP: u32 = 8;
/// The texel interval a ruler index is stamped at on a short axis — a frame this small would carry
/// one lone index at [`RULER_INDEX_STEP`], which counts nothing.
const RULER_FINE_STEP: u32 = 4;
/// An axis at or below this many texels uses [`RULER_FINE_STEP`].
const RULER_FINE_EXTENT: u32 = 16;
/// The one-pixel separator between neighbouring cells: reserved in each cell's size and never drawn,
/// so the gap costs no pass ([`crate::voxel_sheet`]'s `+ 1`).
const CELL_GAP: u32 = 1;
/// One footer or cell-label line's height: the glyph box at [`LABEL_SCALE`] plus leading.
const FOOTER_LINE_H: u32 = 8 * LABEL_SCALE + 4;
/// Neither sheet exceeds this on either axis: past it an image viewer starts down-sampling, which
/// defeats a sheet whose whole point is that one texel is countable. Upscales are clamped to hold
/// it — see [`fit_scales`].
const MAX_SHEET_PX: u32 = 4096;

/// Colour of a block title and of the header line.
const TITLE_COLOR: [u8; 4] = [255, 255, 255, 255];
/// Colour of a tile label, a footer line and a cell label — dimmer than a title, still legible.
const LABEL_COLOR: [u8; 4] = [200, 200, 208, 255];
/// Colour of the texel ruler's ticks and indices.
const RULER_COLOR: [u8; 4] = [120, 124, 134, 255];
/// Colour of the pivot cross: a saturated magenta no palette is likely to author, so the marker is
/// never mistaken for art.
const PIVOT_COLOR: [u8; 4] = [255, 48, 200, 255];
/// A pivot cross's arm length in pixels, measured from its centre.
const PIVOT_ARM: u32 = 3;

/// The six colours an atlas region outline cycles through, in region (name) order — enough to tell
/// neighbouring rectangles apart, few enough that each stays distinguishable from the others.
const REGION_COLORS: [[u8; 4]; 6] = [
    [255, 96, 96, 255],
    [96, 220, 120, 255],
    [110, 160, 255, 255],
    [255, 200, 80, 255],
    [220, 120, 255, 255],
    [96, 224, 224, 255],
];
/// Glyph scale for a region's name on the atlas overview: native 8×8, the smallest text the font
/// draws. Region rectangles are small and adjacent by construction, so the label is kept as narrow
/// as it can be — on a densely packed atlas neighbouring names still overlap, and the outline
/// colours are then what tell the rectangles apart.
const REGION_LABEL_SCALE: u32 = 1;
/// The atlas overview is at least this wide: the upscale cap ([`ATLAS_MAX_UPSCALE`]) can leave a
/// tiny atlas narrower than its own header line, and padding with background is the honest answer —
/// stretching further would misreport the texel scale the ruler-free overview implies.
const ATLAS_MIN_WIDTH: u32 = 512;
/// The width the atlas overview upscales toward before the cap applies.
const ATLAS_TARGET_WIDTH: u32 = 512;
/// The largest integer upscale the atlas overview takes.
const ATLAS_MAX_UPSCALE: u32 = 16;

/// The most values one sprite sweep may carry, matching [`crate::sweep::MAX_SWEEP_CELLS`]: the cap
/// is a property of what can be compared in one image, not of what is being swept.
const MAX_SPRITE_SWEEP_CELLS: usize = crate::sweep::MAX_SWEEP_CELLS;

/// Block A draws a texel grid only at an upscale of at least this many sheet pixels per texel: below
/// it the grid lines would eat the texels they separate.
const GRID_MIN_UPSCALE: u32 = 6;
/// Every this-many'th grid line is drawn darker, so texels can be counted in blocks.
const GRID_MAJOR_EVERY: u32 = 8;
/// The checkerboard squares behind a transparent texel, both distinct from [`BACKGROUND`].
const CHECKER_DARK: [u8; 4] = [44, 46, 52, 255];
const CHECKER_LIGHT: [u8; 4] = [62, 65, 72, 255];

/// The fewest phases one flipbook strip lays out. One phase is the plain instant a `--time` alone
/// already renders, spelled the long way.
const MIN_CLIP_PHASES: u32 = 2;
/// The most phases one flipbook strip lays out — the sweep sheet's cap, for its reason: past
/// sixteen cells the frames are too small to judge, which is the only thing a strip is for.
const MAX_CLIP_PHASES: u32 = MAX_SPRITE_SWEEP_CELLS as u32;
/// Glyph scale for a phase caption. Native 8×8, unlike the contact sheet's tile labels: a caption
/// here carries an instant *and* a region name on one line, and at the larger scale a sixteen-phase
/// strip would be sized by its captions rather than by its art.
const CLIP_LABEL_SCALE: u32 = 1;
/// The strip a phase caption is stamped in: one line at [`CLIP_LABEL_SCALE`] plus its margins.
const CLIP_LABEL_H: u32 = 8 * CLIP_LABEL_SCALE + 2 * LABEL_MARGIN;
/// The fraction of its own weight an onion-skinned entry keeps when it is not the one showing, as a
/// rational so the composite is the same byte on every platform. The texels are premultiplied, so
/// scaling all four channels by it is exactly "the same art, fainter".
const ONION_GHOST: (u32, u32) = (35, 100);

// --- Public API ---------------------------------------------------------------------------------

/// What to draw a sprite contact sheet of: the document, the game scale block B renders at, the
/// `--set` overrides to apply first, and optionally the pointer to sweep.
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteSheetRequest {
    /// The `kind: sprite` document to bake.
    pub doc: PathBuf,
    /// The authored game scale block B draws every frame at: the integer number of sheet pixels one
    /// texel occupies, so `1` is one texel per pixel. Clamped up to `1`, and down if the sheet would
    /// otherwise pass the 4096 px sheet bound.
    pub scale: u32,
    /// Overrides applied to the document before it bakes, in order — the palette, pivot, seed and
    /// effect-parameter pointers `apply_sprite_overrides` documents (`SPRITE_POINTER_SHAPES` lists them
    /// verbatim in every refusal).
    pub set: Vec<SceneOverride>,
    /// A pointer and the values to try at it, one cell per value. `None` renders the ordinary
    /// two-block contact sheet; `Some` renders the candidate grid instead.
    pub sweep: Option<(String, Vec<Value>)>,
    /// Render the steps sheet (paint, then the image after each effect) instead of the contact
    /// sheet. Refused together with a [`sweep`](Self::sweep).
    pub steps: Option<SpriteSteps>,
}

/// What a steps sheet draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpriteSteps {
    /// Draw only this frame's row; `None` draws every frame.
    pub frame: Option<String>,
}

impl SpriteSheetRequest {
    /// A request for `doc` at scale `1`, with no overrides and no sweep.
    pub fn new(doc: impl Into<PathBuf>) -> Self {
        Self {
            doc: doc.into(),
            scale: 1,
            set: Vec::new(),
            sweep: None,
            steps: None,
        }
    }
}

/// The result of a sprite contact sheet: the image plus everything the caller must echo.
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteSheetOutcome {
    /// The composed sheet: RGBA8, straight alpha, fully opaque — every frame is composited over the
    /// sheet background, so nothing here is transparent.
    pub image: Image,
    /// What each `--set` override changed, in the order given. A sweep's own per-cell value is not
    /// in this list: it differs per cell and rides [`SpriteSheetRequest::sweep`] instead.
    pub applied: Vec<AppliedOverride>,
    /// One echo line per entry of the document's `effects:` list, in application order — the same
    /// text the footer burns in, but in the authored letter case. Empty when the document has no
    /// effects (the footer then says so).
    pub effects: Vec<String>,
    /// In steps mode, one line per drawn row — `<frame>: paint, 0, 1` (effect indices); empty
    /// otherwise.
    pub steps: Vec<String>,
}

/// Bakes the sprite document at [`SpriteSheetRequest::doc`] and composes its contact sheet.
///
/// The whole path is CPU-only and deterministic: read, parse, override, validate, bake, compose. A
/// document that cannot be read, parsed, validated *or* that an override breaks fails before a
/// single pixel is written, with its own ordinary diagnostic.
///
/// # Errors
/// - `L1015` if the document cannot be read.
/// - `L1153`/`L1154` for a sprite document that does not parse or validate — including one that only
///   *became* invalid because an override wrote an out-of-range value, which is deliberate: the
///   override applier checks shape, and the document's own validator checks meaning.
/// - `L4008` for an override pointer outside the supported shapes (the `SPRITE_POINTER_SHAPES` list,
///   echoed in every such diagnostic's help),
///   and for a sweep of more than [`crate::sweep::MAX_SWEEP_CELLS`] values or of none; for a sweep
///   together with steps, a `steps` frame the document does not carry, and a steps sheet of more
///   than [`crate::sweep::MAX_SWEEP_CELLS`] cells.
pub fn render_sprite_sheet(
    request: &SpriteSheetRequest,
) -> Result<SpriteSheetOutcome, Vec<Diagnostic>> {
    let source = read_document(&request.doc, "sprite")?;
    let base = ludion_scene::check_sprite(&source)?;
    // A `palette: { from }` include is resolved into a concrete inline palette first, so the contact
    // sheet bakes and every `--set`/`--sweep` `/palette/<glyph>/…` pointer addresses the resolved
    // palette (an inherited glyph or a local override alike). The shared palette file is never
    // written — the edit lives only in this in-memory document.
    let base = resolve_sprite_palette(&request.doc, base)?;

    let mut doc = base.clone();
    let applied = apply_sprite_overrides(&mut doc, &request.set)?;
    let effects = effect_lines(&doc.effects);
    let title = document_title(&doc, &request.doc);

    let mut step_lines = Vec::new();
    let image = match (&request.sweep, &request.steps) {
        (Some(_), Some(_)) => {
            return Err(vec![clip_refusal(
                "`steps` and a sweep are two different sheets".to_string(),
                "render one or the other: drop the sweep to see the steps, or drop the steps",
            )]);
        }
        (None, Some(steps)) => {
            let rows = bake_steps_checked(&doc, steps)?;
            step_lines = rows.iter().map(step_line).collect();
            compose_steps_sheet(&title, &doc, &rows, &effects, &applied)
        }
        (None, None) => {
            let frames = bake_checked(&doc)?;
            compose_contact_sheet(&title, &doc, &frames, request.scale, &effects, &applied)
        }
        (Some((pointer, values)), None) => {
            let cells = bake_sweep(&doc, pointer, values)?;
            compose_sweep_sheet(&title, &doc, pointer, &cells, &effects, &applied)
        }
    };

    Ok(SpriteSheetOutcome {
        image,
        applied,
        effects,
        steps: step_lines,
    })
}

/// Composes the overview of the packed atlas document at `atlas`: its own PNG, upscaled, with every
/// region outlined and named and every pivot crossed.
///
/// The colour map is resolved beside the atlas document, exactly as a scene resolves it, and it is
/// **premultiplied** (that is the committed convention), so it is composited with the premultiplied
/// `over` rather than the straight one a baked frame gets.
///
/// # Errors
/// - `L1015` if the atlas document cannot be read.
/// - Every diagnostic [`ludion_scene::check_atlas`] reports.
/// - `L1101`/`L1102` if the colour map beside it cannot be read or decoded.
pub fn render_atlas_sheet(atlas: &Path) -> Result<Image, Vec<Diagnostic>> {
    let source = read_document(atlas, "atlas")?;
    let doc = ludion_scene::check_atlas(&source)?;
    let root = atlas.parent().unwrap_or_else(|| Path::new("."));
    let packed = ludion_assets::load_png(root, &doc.color_map).map_err(|d| vec![d])?;
    Ok(compose_atlas_sheet(atlas, &doc, &packed))
}

/// What to draw of one of an atlas's flipbooks: the document, the clip, and which of the three
/// instruments to compose.
#[derive(Clone, Debug, PartialEq)]
pub struct AtlasClipRequest {
    /// The packed `kind: atlas` document carrying the flipbook.
    pub atlas: PathBuf,
    /// The flipbook's name, as the document's `flipbooks:` spells it.
    pub clip: String,
    /// The instant to show, in seconds; `None` is `0.0`. A [`strip`](Self::strip) supersedes it
    /// with its own evenly spaced phases; under [`onion`](Self::onion) it picks the opaque entry.
    pub time: Option<f64>,
    /// Lay `n` evenly spaced phases out in one row instead of showing one instant.
    pub strip: Option<u32>,
    /// Overlay the clip's entries in one cell, ghosting every one behind the one showing.
    pub onion: bool,
    /// Whole sheet pixels per texel, `1..=16`. Reduced if the image would otherwise pass the sheet
    /// bound, and the header then says `(clamped)`.
    pub scale: u32,
}

impl AtlasClipRequest {
    /// A request for `clip` of `atlas` at one texel per pixel: the instant at `0.0`, no strip, no
    /// onion skin.
    pub fn new(atlas: impl Into<PathBuf>, clip: impl Into<String>) -> Self {
        Self {
            atlas: atlas.into(),
            clip: clip.into(),
            time: None,
            strip: None,
            onion: false,
            scale: 1,
        }
    }
}

/// The result of a flipbook instrument: the image plus everything the caller must echo.
#[derive(Clone, Debug, PartialEq)]
pub struct AtlasClipOutcome {
    /// The composed image: RGBA8, straight alpha, fully opaque — every entry is composited over the
    /// sheet background.
    pub image: Image,
    /// The clip's name, as the document spells it.
    pub clip: String,
    /// The clip's authored frames per second.
    pub fps: f64,
    /// How many entries the clip's `frames:` list carries.
    pub entries: usize,
    /// One cycle's length in seconds — [`ludion_scene::Flipbook::duration`], `hold:` counts
    /// included.
    pub duration: f64,
    /// Whether the clip repeats.
    pub looped: bool,
    /// One echo line per drawn phase (a strip or an instant) or per overlaid entry (an onion skin),
    /// in draw order — the same text the image burns in, for a session that never opens it.
    pub phases: Vec<String>,
}

/// Checks the flipbook flags' shape before an atlas is read: `--time`/`--strip`/`--onion` are
/// modifiers of `--clip` and mean nothing without it, a strip and an onion skin are two answers to
/// one question, and a strip of one phase is an instant spelled the long way.
///
/// Split out so a caller can refuse an orphaned modifier — the one check that must happen where
/// there is no clip to build an [`AtlasClipRequest`] around. [`render_atlas_clip`] re-checks the
/// half that survives into a request, so the seam is safe called on its own.
///
/// # Errors
/// `L4008` for each of the three misshapes, with the spelling that works in its help.
pub fn check_atlas_clip_flags(
    clip: Option<&str>,
    time: Option<f64>,
    strip: Option<u32>,
    onion: bool,
) -> Result<(), Vec<Diagnostic>> {
    if clip.is_none() {
        let orphan = match (time, strip, onion) {
            (Some(_), _, _) => "`--time`",
            (_, Some(_), _) => "`--strip`",
            (_, _, true) => "`--onion`",
            _ => return Ok(()),
        };
        return Err(vec![clip_refusal(
            format!("{orphan} needs `--clip`: there is no flipbook to take an instant of"),
            "name the clip too, e.g. `--clip ship/bank --time 0.25`; `check-scene <name>.atlas.yaml` \
             lists the flipbooks an atlas carries",
        )]);
    }
    check_clip_shape(strip, onion)
}

/// Composes one of the three flipbook instruments over the atlas at
/// [`AtlasClipRequest::atlas`]: an instant, a strip of phases, or an onion skin.
///
/// CPU-only and deterministic, exactly like [`render_atlas_sheet`]: read, parse, resolve the clip,
/// decode the packed image, compose. The packed image is **premultiplied** (the bake's convention),
/// so every entry is composited with the premultiplied `over`.
///
/// # Errors
/// - `L1015` if the atlas document cannot be read.
/// - Every diagnostic [`ludion_scene::check_atlas`] reports.
/// - `L1101`/`L1102` if the colour map beside it cannot be read or decoded.
/// - `L4008` for a strip alongside an onion skin, a phase count outside
///   `MIN_CLIP_PHASES..=MAX_CLIP_PHASES`, or a clip name the document carries no flipbook for (the
///   help lists the names it does carry, or says it carries none).
pub fn render_atlas_clip(request: &AtlasClipRequest) -> Result<AtlasClipOutcome, Vec<Diagnostic>> {
    check_clip_shape(request.strip, request.onion)?;

    let source = read_document(&request.atlas, "atlas")?;
    let doc = ludion_scene::check_atlas(&source)?;
    let flipbook = doc
        .flipbook(&request.clip)
        .ok_or_else(|| vec![unknown_clip(&request.clip, &doc)])?;
    let root = request.atlas.parent().unwrap_or_else(|| Path::new("."));
    let packed = ludion_assets::load_png(root, &doc.color_map).map_err(|d| vec![d])?;

    let title = atlas_title(&request.atlas);
    let (image, phases) = if request.onion {
        let entries = onion_entries(&doc, flipbook, request.time);
        compose_clip_onion(&title, flipbook, &entries, &packed, request.scale)
    } else {
        let phases = clip_phases(&doc, flipbook, request.strip, request.time);
        compose_clip_strip(&title, flipbook, &phases, &packed, request.scale)
    };

    Ok(AtlasClipOutcome {
        image,
        clip: flipbook.name.clone(),
        fps: flipbook.fps,
        entries: flipbook.frames.len(),
        duration: flipbook.duration(),
        looped: flipbook.looped,
        phases,
    })
}

// --- Reading ------------------------------------------------------------------------------------

/// Resolves a sprite document's shared-palette include (`palette: { from }`) into a concrete inline
/// document, reading the referenced `kind: palette` document relative to the sprite file's own
/// directory. A sprite with no include is returned unchanged, so a caller may run every sprite it
/// reads through this.
///
/// This is the one IO half of the include: `ludion_scene::resolve_sprite_include` merges two
/// documents and reads nothing, and everything that turns a sprite file into pixels — the contact
/// sheet here, `check-scene` and `generate-atlas` in the CLI — comes through this function, so one
/// unreadable palette reads the same everywhere.
///
/// # Errors
/// `L1147` if the palette file cannot be read, or if it is not a well-formed `kind: palette` document.
pub fn resolve_sprite_palette(
    sprite_path: &Path,
    doc: SpriteDocument,
) -> Result<SpriteDocument, Vec<Diagnostic>> {
    let Some(include) = &doc.palette_include else {
        return Ok(doc);
    };
    let dir = sprite_path.parent().unwrap_or_else(|| Path::new("."));
    let palette_path = dir.join(&include.from);
    let palette_source = std::fs::read_to_string(&palette_path).map_err(|error| {
        vec![
            Diagnostic::new(
                code::SPRITE_PALETTE_STRUCTURE,
                format!(
                    "could not read the palette `{}` that `palette.from` names: {error}",
                    palette_path.display()
                ),
            )
            .with_path("/palette/from")
            .with_help(
                "`palette.from` resolves relative to this sprite document's own directory; point it \
                 at a readable `kind: palette` document",
            ),
        ]
    })?;
    ludion_scene::resolve_sprite_include(&doc, &palette_source).map_err(|diagnostics| {
        diagnostics
            .into_iter()
            .map(|diagnostic| palette_defect(&palette_path, diagnostic))
            .collect()
    })
}

/// Re-points a defect of the *palette* document at the sprite's own `palette.from`, moving the
/// palette's path and the pointer inside it into the message.
///
/// The file a report is headed with is the sprite, so a bare `/kind` pointer would name a key the
/// sprite does not have and send an author looking in the wrong file. The pointer that is true of the
/// file under the header is `/palette/from`; where the defect actually sits is prose.
fn palette_defect(palette_path: &Path, mut diagnostic: Diagnostic) -> Diagnostic {
    let at = match diagnostic.path.take() {
        Some(pointer) if !pointer.is_empty() => format!(" at `{pointer}`"),
        _ => String::new(),
    };
    diagnostic.message = format!(
        "the shared palette `{}`{at}: {}",
        palette_path.display(),
        diagnostic.message
    );
    diagnostic.path = Some("/palette/from".to_string());
    diagnostic
}

/// Reads a document file, reporting an unreadable path as the workspace's ordinary `L1015`.
fn read_document(path: &Path, kind: &str) -> Result<String, Vec<Diagnostic>> {
    std::fs::read_to_string(path).map_err(|error| {
        vec![
            Diagnostic::new(
                code::SCENE_IO,
                format!(
                    "could not read {kind} document `{}`: {error}",
                    path.display()
                ),
            )
            .with_help(format!(
                "pass a readable `kind: {kind}` document (check the path and permissions)"
            )),
        ]
    })
}

/// The name the header carries: the document's own `name:`, else its file stem.
fn document_title(doc: &SpriteDocument, path: &Path) -> String {
    doc.name.clone().unwrap_or_else(|| {
        path.file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string())
    })
}

/// Bakes a document, forwarding the bake's own diagnostics.
fn bake_checked(doc: &SpriteDocument) -> Result<Vec<BakedFrame>, Vec<Diagnostic>> {
    ludion_sprite::bake(doc)
}

/// Bakes the stepped frames, narrows them to the requested frame and enforces the cell cap.
fn bake_steps_checked(
    doc: &SpriteDocument,
    steps: &SpriteSteps,
) -> Result<Vec<ludion_sprite::FrameSteps>, Vec<Diagnostic>> {
    let mut rows = ludion_sprite::bake_steps(doc)?;
    let names: Vec<String> = rows.iter().map(|row| row.name.clone()).collect();
    if let Some(wanted) = &steps.frame {
        rows.retain(|row| &row.name == wanted);
        if rows.is_empty() {
            return Err(vec![clip_refusal(
                format!("`--frame {wanted}` names a frame this document does not carry"),
                &format!("this document carries `{}`", names.join("`, `")),
            )]);
        }
    }
    let cells: usize = rows.iter().map(|row| 1 + row.steps.len()).sum();
    if cells > MAX_SPRITE_SWEEP_CELLS {
        let one_fits = rows
            .iter()
            .all(|row| row.steps.len() < MAX_SPRITE_SWEEP_CELLS);
        let help = if rows.len() > 1 && one_fits {
            format!(
                "draw one frame per sheet with `--frame <name>`; this document carries `{}`",
                names.join("`, `")
            )
        } else {
            "split the effect list: past sixteen cells the frames are too small to judge, which \
             is the only thing a sheet is for"
                .to_string()
        };
        return Err(vec![clip_refusal(
            format!(
                "a steps sheet of {cells} cells does not fit; at most {MAX_SPRITE_SWEEP_CELLS} are \
                 drawn"
            ),
            &help,
        )]);
    }
    Ok(rows)
}

/// One steps row's echo: `<frame>: paint, 0, 1` — the effect indices that produced a cell.
fn step_line(row: &ludion_sprite::FrameSteps) -> String {
    let mut line = format!("{}: paint", row.name);
    for step in &row.steps {
        line.push_str(&format!(", {}", step.effect));
    }
    line
}

/// One sweep cell: the value that produced it and the frames it baked to.
struct SweepBake {
    value: Value,
    frames: Vec<BakedFrame>,
}

/// Bakes one document per swept value, each with that one override written over the already
/// `--set`-overridden document.
fn bake_sweep(
    doc: &SpriteDocument,
    pointer: &str,
    values: &[Value],
) -> Result<Vec<SweepBake>, Vec<Diagnostic>> {
    if values.is_empty() {
        return Err(vec![sweep_size_error(
            pointer,
            "a sweep needs at least one value",
            "pass `--sweep <pointer>=<v,v,…>` with one or more comma-separated values",
        )]);
    }
    if values.len() > MAX_SPRITE_SWEEP_CELLS {
        return Err(vec![sweep_size_error(
            pointer,
            format!(
                "a sweep of {} values does not fit a contact sheet; at most \
                 {MAX_SPRITE_SWEEP_CELLS} cells are drawn",
                values.len()
            ),
            "split the range into several sheets — past sixteen cells the frames are too small to \
             judge, which is the only thing a sheet is for",
        )]);
    }

    let mut cells = Vec::with_capacity(values.len());
    for value in values {
        let mut candidate = doc.clone();
        let over = SceneOverride {
            pointer: pointer.to_string(),
            value: value.clone(),
        };
        apply_sprite_overrides(&mut candidate, std::slice::from_ref(&over))?;
        cells.push(SweepBake {
            value: value.clone(),
            frames: bake_checked(&candidate)?,
        });
    }
    Ok(cells)
}

/// `L4008` for a value list that cannot become a sheet — [`crate::sweep`]'s own shape.
fn sweep_size_error(pointer: &str, message: impl Into<String>, help: &str) -> Diagnostic {
    Diagnostic::new(code::SCENE_OVERRIDE_INVALID, message.into())
        .with_path(pointer.to_string())
        .with_help(help.to_string())
}

// --- Overrides ----------------------------------------------------------------------------------

/// The legal pointer shapes, listed verbatim in every `L4008` this applier raises.
const SPRITE_POINTER_SHAPES: &str = "a sprite override addresses one of `/seed`, \
     `/palette/<glyph>/color/<0..3>`, `/palette/<glyph>/emissive`, `/frames/<name>/pivot/<0|1>`, \
     `/effects/<i>/<param>` (a scalar or an enum keyword) or `/effects/<i>/<param>/<j>` (one \
     component of `from`, `to`, `light`, `offset` or `color`) — e.g. \
     `/palette/A/color/0=0.12`, `/effects/1/strength=0.4`, `/effects/0/from/3=0.5`";

/// Applies render-time overrides to a parsed sprite document, in order, returning what each changed.
///
/// The sprite counterpart of [`ludion_scene::apply_palette_overrides`], and it follows that
/// applier's two rules exactly. **The domain is the tunable values alone** — `size`, `frames/<name>/
/// rows` and an effect's `op`/`glyphs`/`frames` are the document's *shape*, not a knob, and a
/// pointer that could rewrite them would let a typo bake a sprite nobody authored. **Range is not
/// checked here**: the overridden document is re-validated before it bakes, so an out-of-range
/// colour comes back as the ordinary `L1153`/`L1154`, exactly as the same edit to the file would.
///
/// Writing a `pivot` component on a frame that authored none first materializes the document's
/// default pivot (the frame's centre) and reports [`AppliedOverride::from`] as `None`, so a caller
/// can echo `unset -> <value>`; the same holds for an unset `emissive`.
///
/// # Errors
/// `L4008` per override whose pointer is not one of the supported shapes, names a glyph, frame or
/// effect the document does not have, or carries a value of the wrong kind. Failures are collected,
/// so a batch reports every bad pointer in one pass; on failure `doc` is left partially overridden
/// and the caller must discard it.
fn apply_sprite_overrides(
    doc: &mut SpriteDocument,
    overrides: &[SceneOverride],
) -> Result<Vec<AppliedOverride>, Vec<Diagnostic>> {
    let mut applied = Vec::with_capacity(overrides.len());
    let mut diagnostics = Vec::new();
    for over in overrides {
        match apply_one(doc, over) {
            Ok(change) => applied.push(change),
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }
    if diagnostics.is_empty() {
        Ok(applied)
    } else {
        Err(diagnostics)
    }
}

/// Reverses RFC 6901's escaping (`~1` → `/`, `~0` → `~`, in that order) — a palette glyph may
/// legitimately *be* `/` or `~`, so a sprite pointer needs exactly this unescaping.
fn unescape(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

fn apply_one(
    doc: &mut SpriteDocument,
    over: &SceneOverride,
) -> Result<AppliedOverride, Diagnostic> {
    let pointer = over.pointer.as_str();
    let segments: Vec<String> = pointer.split('/').skip(1).map(unescape).collect();
    let head = segments.first().map(String::as_str);
    let from = match (head, segments.len()) {
        (Some("seed"), 1) => {
            let from = doc.seed.map_or(Value::Null, Value::from);
            match &over.value {
                Value::Null => doc.seed = None,
                Value::Number(number) => match number.as_u64() {
                    Some(seed) => doc.seed = Some(seed),
                    None => {
                        return Err(wrong_value(pointer, "a non-negative integer", &over.value));
                    }
                },
                _ => return Err(wrong_value(pointer, "a non-negative integer", &over.value)),
            }
            Some(from)
        }
        (Some("palette"), 3 | 4) => write_palette(doc, &segments, pointer, &over.value)?,
        (Some("frames"), 4) => write_pivot(doc, &segments, pointer, &over.value)?,
        (Some("effects"), 3 | 4) => write_effect(doc, &segments, pointer, &over.value)?,
        _ => return Err(unsupported_pointer(pointer)),
    };
    Ok(AppliedOverride {
        pointer: pointer.to_string(),
        from,
        to: over.value.clone(),
    })
}

/// `/palette/<glyph>/color/<0..3>` and `/palette/<glyph>/emissive`.
fn write_palette(
    doc: &mut SpriteDocument,
    segments: &[String],
    pointer: &str,
    value: &Value,
) -> Result<Option<Value>, Diagnostic> {
    let key = &segments[1];
    let Some(index) = doc.palette.iter().position(|entry| entry.glyph == *key) else {
        return Err(unknown_name(
            pointer,
            "glyph",
            key,
            doc.palette.iter().map(|entry| entry.glyph.as_str()),
        ));
    };
    let entry = &mut doc.palette[index];
    match (segments[2].as_str(), segments.len()) {
        ("color", 4) => {
            let channel = segments[3]
                .parse::<usize>()
                .ok()
                .filter(|i| *i < entry.color.len())
                .ok_or_else(|| bad_index(pointer, &segments[3], "a colour is four channels"))?;
            let number = as_number(pointer, value)?;
            let from = Value::from(entry.color[channel]);
            entry.color[channel] = number;
            Ok(Some(from))
        }
        ("emissive", 3) => {
            let number = as_number(pointer, value)?;
            let from = entry.emissive.map(Value::from);
            entry.emissive = Some(number);
            Ok(from)
        }
        _ => Err(unsupported_pointer(pointer)),
    }
}

/// `/frames/<name>/pivot/<0|1>` — the one frame field that is a value rather than shape.
fn write_pivot(
    doc: &mut SpriteDocument,
    segments: &[String],
    pointer: &str,
    value: &Value,
) -> Result<Option<Value>, Diagnostic> {
    if segments[2] != "pivot" {
        return Err(unsupported_pointer(pointer));
    }
    let default = doc.default_pivot();
    let key = &segments[1];
    let Some(index) = doc.frames.iter().position(|frame| frame.name == *key) else {
        return Err(unknown_name(
            pointer,
            "frame",
            key,
            doc.frames.iter().map(|frame| frame.name.as_str()),
        ));
    };
    let axis = segments[3]
        .parse::<usize>()
        .ok()
        .filter(|i| *i < 2)
        .ok_or_else(|| bad_index(pointer, &segments[3], "a pivot is `0` (x) and `1` (y)"))?;
    let number = as_number(pointer, value)?;
    let frame = &mut doc.frames[index];
    // An unauthored pivot is the document's default; materializing it keeps the *other* axis at the
    // value the document already meant rather than at zero.
    let authored = frame.pivot;
    let pivot = frame.pivot.get_or_insert(default);
    let from = authored.map(|held| Value::from(held[axis]));
    pivot[axis] = number;
    Ok(from)
}

/// `/effects/<i>/<param>` and `/effects/<i>/<param>/<j>`.
fn write_effect(
    doc: &mut SpriteDocument,
    segments: &[String],
    pointer: &str,
    value: &Value,
) -> Result<Option<Value>, Diagnostic> {
    let count = doc.effects.len();
    let index = segments[1]
        .parse::<usize>()
        .ok()
        .filter(|i| *i < count)
        .ok_or_else(|| {
            unresolved(
                pointer,
                format!(
                    "`/effects` resolves to {count} entr{}, so `{}` is not one of them",
                    if count == 1 { "y" } else { "ies" },
                    segments[1]
                ),
            )
        })?;
    let param = segments[2].as_str();
    let component = match segments.get(3) {
        None => None,
        Some(raw) => Some(
            raw.parse::<usize>()
                .map_err(|_| bad_index(pointer, raw, "an array component is an index"))?,
        ),
    };
    let effect = &mut doc.effects[index];
    let slot = effect_slot(&mut effect.op, param, component)
        .ok_or_else(|| unsupported_effect_param(pointer, param))?;
    write_slot(slot, pointer, value)
}

/// One writable effect parameter, borrowed out of its op.
enum EffectSlot<'a> {
    /// Any of the `f64` parameters: a colour channel, a light component, a strength, a density, an
    /// amount.
    Number(&'a mut f64),
    /// A `shadow` offset component: whole texels, so it takes an integer.
    Offset(&'a mut i32),
    /// A `blur` radius: whole texels, non-negative.
    Radius(&'a mut u32),
    /// A `gradient` direction keyword.
    Direction(&'a mut GradientDirection),
    /// A `gradient` blend keyword.
    Blend(&'a mut GradientBlend),
    /// An `outline` side keyword.
    Side(&'a mut OutlineSide),
}

/// The parameter `param` (with `component` for an array one) names on `op`, or `None` when this op
/// has no such parameter — an array parameter addressed with no component, or a scalar addressed
/// with one, is deliberately `None` too: the pointer must match the shape of the value it writes.
fn effect_slot<'a>(
    op: &'a mut SpriteEffectOp,
    param: &str,
    component: Option<usize>,
) -> Option<EffectSlot<'a>> {
    match op {
        SpriteEffectOp::Gradient {
            from,
            to,
            direction,
            blend,
        } => match (param, component) {
            ("from", Some(i)) => from.get_mut(i).map(EffectSlot::Number),
            ("to", Some(i)) => to.get_mut(i).map(EffectSlot::Number),
            ("direction", None) => Some(EffectSlot::Direction(direction)),
            ("blend", None) => Some(EffectSlot::Blend(blend)),
            _ => None,
        },
        SpriteEffectOp::Outline { color, side } => match (param, component) {
            ("color", Some(i)) => color.get_mut(i).map(EffectSlot::Number),
            ("side", None) => Some(EffectSlot::Side(side)),
            _ => None,
        },
        SpriteEffectOp::Shade { light, strength } => match (param, component) {
            ("light", Some(i)) => light.get_mut(i).map(EffectSlot::Number),
            ("strength", None) => Some(EffectSlot::Number(strength)),
            _ => None,
        },
        SpriteEffectOp::Shadow { offset, color } => match (param, component) {
            ("offset", Some(i)) => offset.get_mut(i).map(EffectSlot::Offset),
            ("color", Some(i)) => color.get_mut(i).map(EffectSlot::Number),
            _ => None,
        },
        SpriteEffectOp::Dither { density } => match (param, component) {
            ("density", None) => Some(EffectSlot::Number(density)),
            _ => None,
        },
        SpriteEffectOp::Noise { amount } => match (param, component) {
            ("amount", None) => Some(EffectSlot::Number(amount)),
            _ => None,
        },
        SpriteEffectOp::Blur { radius } => match (param, component) {
            ("radius", None) => Some(EffectSlot::Radius(radius)),
            _ => None,
        },
        SpriteEffectOp::Glow {
            color,
            radius_px,
            strength,
        } => match (param, component) {
            // A colour component is writable only when the halo has an explicit tint to write into;
            // an untinted glow derives its colour, so there is no channel to set.
            ("color", Some(i)) => color
                .as_mut()
                .and_then(|c| c.get_mut(i))
                .map(EffectSlot::Number),
            ("radius_px", None) => Some(EffectSlot::Number(radius_px)),
            ("strength", None) => Some(EffectSlot::Number(strength)),
            _ => None,
        },
    }
}

/// Writes `value` into one borrowed parameter, returning what it held.
fn write_slot(
    slot: EffectSlot<'_>,
    pointer: &str,
    value: &Value,
) -> Result<Option<Value>, Diagnostic> {
    let from = match slot {
        EffectSlot::Number(target) => {
            let number = as_number(pointer, value)?;
            let from = Value::from(*target);
            *target = number;
            from
        }
        EffectSlot::Offset(target) => {
            let number = value
                .as_i64()
                .and_then(|raw| i32::try_from(raw).ok())
                .ok_or_else(|| wrong_value(pointer, "a whole number of texels", value))?;
            let from = Value::from(*target);
            *target = number;
            from
        }
        EffectSlot::Radius(target) => {
            let number = value
                .as_u64()
                .and_then(|raw| u32::try_from(raw).ok())
                .ok_or_else(|| wrong_value(pointer, "a non-negative whole number", value))?;
            let from = Value::from(*target);
            *target = number;
            from
        }
        EffectSlot::Direction(target) => {
            let word = as_keyword(pointer, value, &["vertical", "horizontal", "radial"])?;
            let from = Value::from(direction_keyword(*target));
            *target = match word.as_str() {
                "horizontal" => GradientDirection::Horizontal,
                "radial" => GradientDirection::Radial,
                _ => GradientDirection::Vertical,
            };
            from
        }
        EffectSlot::Blend(target) => {
            let word = as_keyword(pointer, value, &["replace", "multiply"])?;
            let from = Value::from(blend_keyword(*target));
            *target = match word.as_str() {
                "multiply" => GradientBlend::Multiply,
                _ => GradientBlend::Replace,
            };
            from
        }
        EffectSlot::Side(target) => {
            let word = as_keyword(pointer, value, &["outside", "inside"])?;
            let from = Value::from(side_keyword(*target));
            *target = match word.as_str() {
                "inside" => OutlineSide::Inside,
                _ => OutlineSide::Outside,
            };
            from
        }
    };
    Ok(Some(from))
}

/// The `f64` an override must carry for a numeric parameter.
fn as_number(pointer: &str, value: &Value) -> Result<f64, Diagnostic> {
    value
        .as_f64()
        .ok_or_else(|| wrong_value(pointer, "a number", value))
}

/// The enum keyword an override must carry, checked against the op's own choice list.
fn as_keyword(pointer: &str, value: &Value, choices: &[&str]) -> Result<String, Diagnostic> {
    let text = value
        .as_str()
        .ok_or_else(|| wrong_value(pointer, "a keyword", value))?;
    if choices.contains(&text) {
        return Ok(text.to_string());
    }
    Err(Diagnostic::new(
        code::SCENE_OVERRIDE_INVALID,
        format!(
            "the override at `{pointer}` names `{text}`, which is not one of this op's keywords"
        ),
    )
    .with_path(pointer.to_string())
    .with_help(format!("this parameter takes `{}`", choices.join("`, `"))))
}

fn unsupported_pointer(pointer: &str) -> Diagnostic {
    Diagnostic::new(
        code::SCENE_OVERRIDE_INVALID,
        format!("override pointer `{pointer}` does not address a tunable field of this sprite"),
    )
    .with_path(pointer.to_string())
    .with_help(SPRITE_POINTER_SHAPES)
}

fn unsupported_effect_param(pointer: &str, param: &str) -> Diagnostic {
    Diagnostic::new(
        code::SCENE_OVERRIDE_INVALID,
        format!("override pointer `{pointer}` names parameter `{param}`, which this effect's op does not take at that shape"),
    )
    .with_path(pointer.to_string())
    .with_help(SPRITE_POINTER_SHAPES)
}

fn unresolved(pointer: &str, detail: String) -> Diagnostic {
    Diagnostic::new(
        code::SCENE_OVERRIDE_INVALID,
        format!("override pointer `{pointer}` does not resolve in this sprite"),
    )
    .with_path(pointer.to_string())
    .with_help(detail)
}

/// An unknown palette glyph or frame name, listing what the document does hold.
fn unknown_name<'a>(
    pointer: &str,
    what: &str,
    name: &str,
    held: impl Iterator<Item = &'a str>,
) -> Diagnostic {
    let keys: Vec<&str> = held.collect();
    let detail = if keys.is_empty() {
        format!("this sprite has no {what}s at all")
    } else {
        format!("this sprite holds `{}`", keys.join("`, `"))
    };
    Diagnostic::new(
        code::SCENE_OVERRIDE_INVALID,
        format!(
            "override pointer `{pointer}` names {what} `{name}`, which the document does not have"
        ),
    )
    .with_path(pointer.to_string())
    .with_help(detail)
}

fn bad_index(pointer: &str, segment: &str, detail: &str) -> Diagnostic {
    Diagnostic::new(
        code::SCENE_OVERRIDE_INVALID,
        format!("override pointer `{pointer}` addresses component `{segment}`, but {detail}"),
    )
    .with_path(pointer.to_string())
    .with_help(SPRITE_POINTER_SHAPES)
}

fn wrong_value(pointer: &str, expected: &str, value: &Value) -> Diagnostic {
    Diagnostic::new(
        code::SCENE_OVERRIDE_INVALID,
        format!(
            "the override at `{pointer}` expects {expected}, but the value is {}",
            value_phrase(value)
        ),
    )
    .with_path(pointer.to_string())
    .with_help(SPRITE_POINTER_SHAPES)
}

// --- The effect echo ----------------------------------------------------------------------------

/// The op keyword, as the document spells it.
fn op_name(op: &SpriteEffectOp) -> &'static str {
    match op {
        SpriteEffectOp::Gradient { .. } => "gradient",
        SpriteEffectOp::Outline { .. } => "outline",
        SpriteEffectOp::Shade { .. } => "shade",
        SpriteEffectOp::Shadow { .. } => "shadow",
        SpriteEffectOp::Dither { .. } => "dither",
        SpriteEffectOp::Noise { .. } => "noise",
        SpriteEffectOp::Blur { .. } => "blur",
        SpriteEffectOp::Glow { .. } => "glow",
    }
}

/// The `direction:` keyword this variant is written as.
fn direction_keyword(direction: GradientDirection) -> &'static str {
    match direction {
        GradientDirection::Vertical => "vertical",
        GradientDirection::Horizontal => "horizontal",
        GradientDirection::Radial => "radial",
    }
}

/// The `blend:` keyword this variant is written as.
fn blend_keyword(blend: GradientBlend) -> &'static str {
    match blend {
        GradientBlend::Replace => "replace",
        GradientBlend::Multiply => "multiply",
    }
}

/// The `side:` keyword this variant is written as.
fn side_keyword(side: OutlineSide) -> &'static str {
    match side {
        OutlineSide::Outside => "outside",
        OutlineSide::Inside => "inside",
    }
}

/// One `key=value` list for an op's own parameters, arrays as `[a, b, …]`.
fn op_params(op: &SpriteEffectOp) -> String {
    match op {
        SpriteEffectOp::Gradient {
            from,
            to,
            direction,
            blend,
        } => format!(
            "from={} to={} direction={} blend={}",
            number_list(from),
            number_list(to),
            direction_keyword(*direction),
            blend_keyword(*blend)
        ),
        SpriteEffectOp::Outline { color, side } => {
            format!("color={} side={}", number_list(color), side_keyword(*side))
        }
        SpriteEffectOp::Shade { light, strength } => {
            format!("light={} strength={strength}", number_list(light))
        }
        SpriteEffectOp::Shadow { offset, color } => format!(
            "offset=[{}, {}] color={}",
            offset[0],
            offset[1],
            number_list(color)
        ),
        SpriteEffectOp::Dither { density } => format!("density={density}"),
        SpriteEffectOp::Noise { amount } => format!("amount={amount}"),
        SpriteEffectOp::Blur { radius } => format!("radius={radius}"),
        SpriteEffectOp::Glow {
            color,
            radius_px,
            strength,
        } => format!(
            "color={} radius_px={radius_px} strength={strength}",
            color.map_or_else(|| "source".to_string(), |c| number_list(&c))
        ),
    }
}

/// `[a, b, …]` over an array parameter, each component in Rust's shortest round-tripping form.
fn number_list(values: &[f64]) -> String {
    let text: Vec<String> = values.iter().map(f64::to_string).collect();
    format!("[{}]", text.join(", "))
}

/// A glyph or frame filter as the echo names it: the list, or `all` for an absent filter.
fn list_or_all(names: Option<&[String]>) -> String {
    match names {
        None => "all".to_string(),
        Some(names) => names.join(", "),
    }
}

/// One echo line per effect, numbered by its **`/effects/<i>` index** rather than from one: the
/// number an author reads off the sheet is the number they then write into a `--set` pointer.
fn effect_lines(effects: &[SpriteEffect]) -> Vec<String> {
    effects
        .iter()
        .enumerate()
        .map(|(index, effect)| {
            format!(
                "{index}. {}  glyphs: {}  frames: {}  {}",
                op_name(&effect.op),
                list_or_all(effect.glyphs.as_deref()),
                list_or_all(effect.frames.as_deref()),
                op_params(&effect.op)
            )
        })
        .collect()
}

/// One line per applied override: `<pointer>: <from> -> <to>`, an absent `from` spelled `unset`.
fn override_lines(applied: &[AppliedOverride]) -> Vec<String> {
    applied
        .iter()
        .map(|change| {
            let from = match &change.from {
                None => "unset".to_string(),
                Some(value) => plain(value),
            };
            format!("{}: {from} -> {}", change.pointer, plain(&change.to))
        })
        .collect()
}

/// A JSON value as burnable text: a string without its quotes, everything else as JSON.
fn plain(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The footer's lines: the effect echo (or the explicit "none"), then the override echo.
fn footer_lines(effects: &[String], applied: &[AppliedOverride]) -> Vec<String> {
    let mut lines = if effects.is_empty() {
        vec!["effects: none".to_string()]
    } else {
        effects.to_vec()
    };
    lines.extend(override_lines(applied));
    lines
}

/// One cell's label: the value, then the pointer's leaf segment on a second line —
/// [`crate::sweep`]'s own rule, and for its own reason (a short longest line keeps a long pointer
/// inside a narrow cell).
fn cell_label(pointer: &str, value: &Value) -> (String, String) {
    let segments: Vec<&str> = pointer.rsplit('/').take(2).collect();
    let leaf = match segments.as_slice() {
        // An array component's own leaf is a bare index (`…/color/0`), which names nothing on its
        // own — the parameter it indexes rides along.
        [last, parent] if last.chars().all(|ch| ch.is_ascii_digit()) => format!("{parent}/{last}"),
        [last, ..] => (*last).to_string(),
        [] => pointer.to_string(),
    };
    (plain(value), leaf)
}

// --- Contact sheet composition ------------------------------------------------------------------

/// Block A's and block B's geometry for one document, after clamping.
struct ContactLayout {
    /// Block A's per-frame upscale.
    upscale: u32,
    /// Block B's game scale, as honoured.
    scale: u32,
    /// Whether either factor was reduced from what was asked for.
    clamped: bool,
    /// The near-square grid's column count — the one part of the packing composition needs, since
    /// a row is just the index divided by it.
    cols: u32,
    /// Block A cell size, ruler band and label strip included.
    cell_a: (u32, u32),
    /// Block B cell size, label strip included.
    cell_b: (u32, u32),
    width: u32,
    height: u32,
    /// Block A's own origin (y), and block B's, and the footer's.
    block_a_y: u32,
    block_b_y: u32,
    footer_y: u32,
}

/// Computes the contact sheet's geometry at one pair of scales — the pure arithmetic
/// [`fit_scales`] searches over.
#[allow(clippy::too_many_arguments)]
fn contact_layout(
    size: [u32; 2],
    frames: u32,
    upscale: u32,
    scale: u32,
    label_px: u32,
    text_px: u32,
    footer: u32,
) -> ContactLayout {
    let [fw, fh] = size;
    let (cols, rows) = near_square(frames.max(1));
    // A cell is never narrower than the frame name written under it: at the shipping scale a tile
    // is a few pixels wide and the label is what would otherwise run into its neighbour's.
    let cell_a = (
        (RULER_BAND + fw * upscale + CELL_GAP).max(label_px),
        RULER_BAND + fh * upscale + TILE_LABEL_H + CELL_GAP,
    );
    let cell_b = (
        (fw * scale + CELL_GAP).max(label_px),
        fh * scale + TILE_LABEL_H + CELL_GAP,
    );
    let block_a_w = cols * cell_a.0 + CELL_GAP;
    let block_a_h = BLOCK_TITLE_H + rows * cell_a.1 + CELL_GAP;
    let block_b_w = frames.max(1) * cell_b.0 + CELL_GAP;
    let block_b_h = BLOCK_TITLE_H + cell_b.1 + CELL_GAP;
    let footer_h = footer * FOOTER_LINE_H + 2 * LABEL_MARGIN;

    let block_a_y = SHEET_HEADER_H;
    let block_b_y = block_a_y + block_a_h + BLOCK_GAP;
    let footer_y = block_b_y + block_b_h + BLOCK_GAP;
    ContactLayout {
        upscale,
        scale,
        clamped: false,
        cols,
        cell_a,
        cell_b,
        width: block_a_w.max(block_b_w).max(text_px).min(MAX_SHEET_PX),
        height: footer_y + footer_h,
        block_a_y,
        block_b_y,
        footer_y,
    }
}

/// Reduces the two scale factors until the sheet fits [`MAX_SHEET_PX`], always shrinking the larger
/// factor first so the two stay comparable.
///
/// A document whose frames do not fit even at one pixel per texel is drawn at that floor and the
/// header still says `(clamped)` — there is no honest smaller step than 1:1, and cropping the sheet
/// would hide content rather than shrink it.
#[allow(clippy::too_many_arguments)]
fn fit_scales(
    size: [u32; 2],
    frames: u32,
    upscale: u32,
    scale: u32,
    label_px: u32,
    text_px: u32,
    footer: u32,
) -> ContactLayout {
    let (mut u, mut s) = (upscale.max(1), scale.clamp(1, MAX_SHEET_PX));
    loop {
        let layout = contact_layout(size, frames, u, s, label_px, text_px, footer);
        if (layout.width <= MAX_SHEET_PX && layout.height <= MAX_SHEET_PX) || (u == 1 && s == 1) {
            let clamped = u < upscale.max(1) || s < scale.max(1);
            return ContactLayout { clamped, ..layout };
        }
        if s > u {
            s -= 1;
        } else if u > 1 {
            u -= 1;
        } else {
            s -= 1;
        }
    }
}

/// Composes the two-block contact sheet.
fn compose_contact_sheet(
    title: &str,
    doc: &SpriteDocument,
    frames: &[BakedFrame],
    scale: u32,
    effects: &[String],
    applied: &[AppliedOverride],
) -> Image {
    let footer = footer_lines(effects, applied);
    let text_width_px = text_width(&footer, LABEL_SCALE);
    let count = frames.len() as u32;
    // One blank glyph cell past the longest name, so two neighbouring labels never read as one word.
    let label_px = frames
        .iter()
        .map(|frame| text_px(&frame.name, LABEL_SCALE) + ludion_font::LABEL_PITCH * LABEL_SCALE)
        .max()
        .unwrap_or(0);
    let layout = fit_scales(
        doc.size,
        count,
        upscale_factor(doc.size[0], doc.size[1]),
        scale,
        label_px,
        text_width_px,
        footer.len() as u32,
    );

    let (width, height) = (layout.width, layout.height);
    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    fill(&mut rgba, width, height, BACKGROUND);

    let header = format!(
        "{title}  {}x{} texels  {count} frames  seed {}{}",
        doc.size[0],
        doc.size[1],
        doc.seed.unwrap_or(0),
        if layout.clamped { "  (clamped)" } else { "" }
    );
    stamp_text(
        &mut rgba,
        width,
        height,
        LABEL_MARGIN,
        LABEL_MARGIN,
        &header,
        HEADER_SCALE,
        TITLE_COLOR,
    );

    // Block A: legibility. Every frame upscaled, near-square, the first one ruled.
    stamp_text(
        &mut rgba,
        width,
        height,
        LABEL_MARGIN,
        layout.block_a_y + LABEL_MARGIN,
        &format!("texels x{}", layout.upscale),
        TITLE_SCALE,
        TITLE_COLOR,
    );
    for (index, frame) in frames.iter().enumerate() {
        let cell_x = (index as u32 % layout.cols) * layout.cell_a.0;
        let cell_y =
            layout.block_a_y + BLOCK_TITLE_H + (index as u32 / layout.cols) * layout.cell_a.1;
        let tile = (cell_x + RULER_BAND, cell_y + RULER_BAND);
        draw_legible_tile(&mut rgba, width, height, tile, &frame.image, layout.upscale);
        draw_pivot(&mut rgba, width, height, tile, frame, layout.upscale);
        if index == 0 {
            draw_ruler(&mut rgba, width, height, tile, doc.size, layout.upscale);
        }
        stamp_text(
            &mut rgba,
            width,
            height,
            tile.0,
            tile.1 + doc.size[1] * layout.upscale + LABEL_MARGIN,
            &frame.name,
            LABEL_SCALE,
            LABEL_COLOR,
        );
    }

    // Block B: the shipping size. One row, no ruler — a texel here is meant to be uncountable.
    stamp_text(
        &mut rgba,
        width,
        height,
        LABEL_MARGIN,
        layout.block_b_y + LABEL_MARGIN,
        &format!("scale x{}", layout.scale),
        TITLE_SCALE,
        TITLE_COLOR,
    );
    for (index, frame) in frames.iter().enumerate() {
        let tile = (
            index as u32 * layout.cell_b.0,
            layout.block_b_y + BLOCK_TITLE_H,
        );
        draw_frame(&mut rgba, width, height, tile, frame, layout.scale);
        stamp_text(
            &mut rgba,
            width,
            height,
            tile.0,
            tile.1 + doc.size[1] * layout.scale + LABEL_MARGIN,
            &frame.name,
            LABEL_SCALE,
            LABEL_COLOR,
        );
    }

    draw_footer(&mut rgba, width, height, layout.footer_y, &footer);
    finish(width, height, rgba)
}

/// Composes the sweep sheet: one cell per candidate value, near-square.
fn compose_sweep_sheet(
    title: &str,
    doc: &SpriteDocument,
    pointer: &str,
    cells: &[SweepBake],
    effects: &[String],
    applied: &[AppliedOverride],
) -> Image {
    let footer = footer_lines(effects, applied);
    let labels: Vec<(String, String)> = cells
        .iter()
        .map(|cell| cell_label(pointer, &cell.value))
        .collect();
    let label_px = labels
        .iter()
        .map(|(value, leaf)| text_px(value, LABEL_SCALE).max(text_px(leaf, LABEL_SCALE)))
        .max()
        .unwrap_or(0);

    let frames = doc.frames.len().max(1) as u32;
    let (cols, rows) = near_square(cells.len().max(1) as u32);
    let footer_px = text_width(&footer, LABEL_SCALE);
    let mut upscale = upscale_factor(doc.size[0], doc.size[1]);
    // The same shrink-to-fit rule the contact sheet uses, over the one factor a sweep has.
    let (cell_w, cell_h, width, height) = loop {
        let cell_w = (frames * (doc.size[0] * upscale + CELL_GAP) + CELL_GAP).max(label_px);
        let cell_h = doc.size[1] * upscale + 2 * FOOTER_LINE_H + CELL_GAP;
        let width = (cols * cell_w + CELL_GAP).max(footer_px).min(MAX_SHEET_PX);
        let height = SHEET_HEADER_H
            + rows * cell_h
            + CELL_GAP
            + BLOCK_GAP
            + footer.len() as u32 * FOOTER_LINE_H
            + 2 * LABEL_MARGIN;
        if (width <= MAX_SHEET_PX && height <= MAX_SHEET_PX) || upscale == 1 {
            break (cell_w, cell_h, width, height);
        }
        upscale -= 1;
    };
    let clamped = upscale < upscale_factor(doc.size[0], doc.size[1]);

    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    fill(&mut rgba, width, height, BACKGROUND);
    let header = format!(
        "{title}  {}x{} texels  {} frames  seed {}  sweep {pointer}{}",
        doc.size[0],
        doc.size[1],
        doc.frames.len(),
        doc.seed.unwrap_or(0),
        if clamped { "  (clamped)" } else { "" }
    );
    stamp_text(
        &mut rgba,
        width,
        height,
        LABEL_MARGIN,
        LABEL_MARGIN,
        &header,
        HEADER_SCALE,
        TITLE_COLOR,
    );

    for (index, cell) in cells.iter().enumerate() {
        let cell_x = (index as u32 % cols) * cell_w;
        let cell_y = SHEET_HEADER_H + (index as u32 / cols) * cell_h;
        for (slot, frame) in cell.frames.iter().enumerate() {
            let tile = (
                cell_x + slot as u32 * (doc.size[0] * upscale + CELL_GAP),
                cell_y,
            );
            draw_frame(&mut rgba, width, height, tile, frame, upscale);
        }
        let (value, leaf) = &labels[index];
        let label_y = cell_y + doc.size[1] * upscale + LABEL_MARGIN;
        stamp_text(
            &mut rgba,
            width,
            height,
            cell_x,
            label_y,
            value,
            LABEL_SCALE,
            TITLE_COLOR,
        );
        stamp_text(
            &mut rgba,
            width,
            height,
            cell_x,
            label_y + FOOTER_LINE_H,
            leaf,
            LABEL_SCALE,
            LABEL_COLOR,
        );
    }

    let footer_y = SHEET_HEADER_H + rows * cell_h + CELL_GAP + BLOCK_GAP;
    draw_footer(&mut rgba, width, height, footer_y, &footer);
    finish(width, height, rgba)
}

/// Wraps a cell caption to `cell_w` pixels: fields (split on the echo's two-space separator) fill
/// greedily, and a field still too wide breaks on single spaces.
fn wrap_caption(caption: &str, cell_w: u32) -> Vec<String> {
    let fits = |text: &str| text_px(text, CLIP_LABEL_SCALE) <= cell_w;
    let mut lines = Vec::new();
    let mut current = String::new();
    for field in caption.split("  ").filter(|field| !field.is_empty()) {
        let joined = if current.is_empty() {
            field.to_string()
        } else {
            format!("{current}  {field}")
        };
        if fits(&joined) {
            current = joined;
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        if fits(field) {
            current = field.to_string();
            continue;
        }
        for word in field.split(' ').filter(|word| !word.is_empty()) {
            let joined = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if fits(&joined) {
                current = joined;
            } else {
                if !current.is_empty() {
                    lines.push(std::mem::take(&mut current));
                }
                current = word.to_string();
            }
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Composes the steps sheet: one row per frame, the paint then each applicable effect's result.
fn compose_steps_sheet(
    title: &str,
    doc: &SpriteDocument,
    rows: &[ludion_sprite::FrameSteps],
    effects: &[String],
    applied: &[AppliedOverride],
) -> Image {
    let footer = footer_lines(effects, applied);
    let footer_px = text_width(&footer, LABEL_SCALE);
    let [fw, fh] = doc.size;

    // Each row's captions, in cell order.
    let captions: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            std::iter::once(format!("{}  paint", row.name))
                .chain(row.steps.iter().map(|step| effects[step.effect].clone()))
                .collect()
        })
        .collect();
    let widest_token = captions
        .iter()
        .flatten()
        .flat_map(|caption| caption.split_whitespace())
        .map(|token| text_px(token, CLIP_LABEL_SCALE))
        .max()
        .unwrap_or(0);
    let cols = captions.iter().map(Vec::len).max().unwrap_or(1) as u32;
    let row_count = rows.len().max(1) as u32;

    let top = upscale_factor(fw, fh);
    let mut upscale = top;
    let (cell_w, cell_h, width, height) = loop {
        let cell_w = (fw * upscale + CELL_GAP).max(widest_token);
        let max_lines = captions
            .iter()
            .flatten()
            .map(|caption| wrap_caption(caption, cell_w).len() as u32)
            .max()
            .unwrap_or(1);
        let cell_h = fh * upscale + LABEL_MARGIN + max_lines * CLIP_LABEL_H + CELL_GAP;
        let width = (cols * cell_w + CELL_GAP).max(footer_px).min(MAX_SHEET_PX);
        let height = SHEET_HEADER_H
            + row_count * cell_h
            + CELL_GAP
            + BLOCK_GAP
            + footer.len() as u32 * FOOTER_LINE_H
            + 2 * LABEL_MARGIN;
        if (width <= MAX_SHEET_PX && height <= MAX_SHEET_PX) || upscale == 1 {
            break (cell_w, cell_h, width, height);
        }
        upscale -= 1;
    };
    let clamped = upscale < top;

    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    fill(&mut rgba, width, height, BACKGROUND);
    let header = format!(
        "{title}  {fw}x{fh} texels  {} frames  seed {}  steps{}",
        doc.frames.len(),
        doc.seed.unwrap_or(0),
        if clamped { "  (clamped)" } else { "" }
    );
    stamp_text(
        &mut rgba,
        width,
        height,
        LABEL_MARGIN,
        LABEL_MARGIN,
        &header,
        HEADER_SCALE,
        TITLE_COLOR,
    );

    for (row_index, row) in rows.iter().enumerate() {
        let cell_y = SHEET_HEADER_H + row_index as u32 * cell_h;
        let images = std::iter::once(&row.paint).chain(row.steps.iter().map(|step| &step.image));
        for (col, image) in images.enumerate() {
            let cell_x = col as u32 * cell_w;
            draw_legible_tile(&mut rgba, width, height, (cell_x, cell_y), image, upscale);
            let label_y = cell_y + fh * upscale + LABEL_MARGIN;
            for (line_index, line) in wrap_caption(&captions[row_index][col], cell_w)
                .iter()
                .enumerate()
            {
                stamp_text(
                    &mut rgba,
                    width,
                    height,
                    cell_x,
                    label_y + line_index as u32 * CLIP_LABEL_H,
                    line,
                    CLIP_LABEL_SCALE,
                    LABEL_COLOR,
                );
            }
        }
    }

    let footer_y = SHEET_HEADER_H + row_count * cell_h + CELL_GAP + BLOCK_GAP;
    draw_footer(&mut rgba, width, height, footer_y, &footer);
    finish(width, height, rgba)
}

/// Stamps the footer block, one line per entry.
fn draw_footer(buffer: &mut [u8], width: u32, height: u32, y0: u32, lines: &[String]) {
    for (index, line) in lines.iter().enumerate() {
        stamp_text(
            buffer,
            width,
            height,
            LABEL_MARGIN,
            y0 + LABEL_MARGIN + index as u32 * FOOTER_LINE_H,
            line,
            LABEL_SCALE,
            LABEL_COLOR,
        );
    }
}

/// The width, in pixels, the widest of `lines` needs at `scale`.
fn text_width(lines: &[String], scale: u32) -> u32 {
    lines
        .iter()
        .map(|line| text_px(line, scale))
        .max()
        .unwrap_or(0)
}

/// The width, in pixels, one line needs at `scale` — one 8×8 glyph box plus a one-pixel gap per
/// character, plus the margin it is stamped at.
fn text_px(line: &str, scale: u32) -> u32 {
    line.chars().count() as u32 * ludion_font::LABEL_PITCH * scale + 2 * LABEL_MARGIN
}

/// Wraps a finished buffer as an [`Image`], with the content hash of the pixels as its identity — a
/// composed sheet has no source file to take one from.
fn finish(width: u32, height: u32, rgba: Vec<u8>) -> Image {
    Image {
        width,
        height,
        hash: ludion_assets::content_hash(&rgba),
        rgba,
    }
}

// --- Drawing ------------------------------------------------------------------------------------

/// Draws one baked frame at `(x0, y0)`, nearest-upscaled by `scale`, composited over the sheet
/// background with the straight-alpha `over`.
fn draw_frame(
    buffer: &mut [u8],
    width: u32,
    height: u32,
    origin: (u32, u32),
    frame: &BakedFrame,
    scale: u32,
) {
    let image = frame.image.to_srgb8_straight();
    for y in 0..image.height {
        for x in 0..image.width {
            let index = ((y * image.width + x) * 4) as usize;
            let texel: [u8; 4] = image.rgba[index..index + 4]
                .try_into()
                .expect("a four-byte RGBA texel");
            fill_block(
                buffer,
                width,
                height,
                origin.0 + x * scale,
                origin.1 + y * scale,
                scale,
                scale,
                over_straight(texel, BACKGROUND),
            );
        }
    }
}

/// Draws a frame image for reading texels: each texel is composited over a checkerboard instead of
/// the flat background, so a transparent texel is visibly *not* a texel; at an upscale of
/// [`GRID_MIN_UPSCALE`] or more a one-pixel grid darkens every texel boundary (every
/// [`GRID_MAJOR_EVERY`]th one more).
///
/// Checker squares are `(upscale / 2).max(1)` px with their phase anchored at `origin`, so every
/// transparent texel shows a 2×2 checker. The grid darkens the pixels already drawn — integer
/// arithmetic, alpha untouched — so the art's hue stays readable under a line.
fn draw_legible_tile(
    buffer: &mut [u8],
    width: u32,
    height: u32,
    origin: (u32, u32),
    image: &ludion_sprite::FrameImage,
    upscale: u32,
) {
    let image = image.to_srgb8_straight();
    let square = (upscale / 2).max(1);
    for y in 0..image.height {
        for x in 0..image.width {
            let index = ((y * image.width + x) * 4) as usize;
            let texel: [u8; 4] = image.rgba[index..index + 4]
                .try_into()
                .expect("a four-byte RGBA texel");
            for dy in 0..upscale {
                for dx in 0..upscale {
                    let checker = if ((x * upscale + dx) / square + (y * upscale + dy) / square)
                        .is_multiple_of(2)
                    {
                        CHECKER_DARK
                    } else {
                        CHECKER_LIGHT
                    };
                    fill_block(
                        buffer,
                        width,
                        height,
                        origin.0 + x * upscale + dx,
                        origin.1 + y * upscale + dy,
                        1,
                        1,
                        over_straight(texel, checker),
                    );
                }
            }
        }
    }
    if upscale < GRID_MIN_UPSCALE {
        return;
    }
    let darken = |buffer: &mut [u8], x: u32, y: u32, major: bool| {
        let mut pixel = pixel_at(buffer, width, height, x, y);
        for channel in &mut pixel[..3] {
            *channel = if major {
                *channel / 2
            } else {
                (u16::from(*channel) * 3 / 4) as u8
            };
        }
        fill_block(buffer, width, height, x, y, 1, 1, pixel);
    };
    let (span_w, span_h) = (image.width * upscale, image.height * upscale);
    for k in 0..=image.width {
        let major = k % GRID_MAJOR_EVERY == 0;
        for y in 0..span_h {
            darken(buffer, origin.0 + k * upscale, origin.1 + y, major);
        }
    }
    for k in 0..=image.height {
        let major = k % GRID_MAJOR_EVERY == 0;
        for x in 0..span_w {
            darken(buffer, origin.0 + x, origin.1 + k * upscale, major);
        }
    }
}

/// Marks a frame's pivot with a one-pixel cross. The pivot is authored in texels, so it is floored
/// to a texel first and *then* multiplied by the integer upscale — the same value on every platform.
fn draw_pivot(
    buffer: &mut [u8],
    width: u32,
    height: u32,
    origin: (u32, u32),
    frame: &BakedFrame,
    scale: u32,
) {
    let texel = |value: f64, extent: u32| -> u32 {
        let floored = value.floor();
        if floored <= 0.0 {
            0
        } else {
            (floored as u32).min(extent.saturating_sub(1))
        }
    };
    let cx = origin.0 + texel(frame.pivot[0], frame.image.width) * scale;
    let cy = origin.1 + texel(frame.pivot[1], frame.image.height) * scale;
    let arm = PIVOT_ARM;
    fill_block(
        buffer,
        width,
        height,
        cx.saturating_sub(arm),
        cy,
        arm * 2 + 1,
        1,
        PIVOT_COLOR,
    );
    fill_block(
        buffer,
        width,
        height,
        cx,
        cy.saturating_sub(arm),
        1,
        arm * 2 + 1,
        PIVOT_COLOR,
    );
}

/// Draws the texel ruler along a tile's top and left edges: one tick per texel, a long tick every
/// four, and a stamped index every [`RULER_INDEX_STEP`] — or every [`RULER_FINE_STEP`] on an axis of
/// at most [`RULER_FINE_EXTENT`] texels, where the coarse interval would print one lone `0`.
fn draw_ruler(
    buffer: &mut [u8],
    width: u32,
    height: u32,
    origin: (u32, u32),
    size: [u32; 2],
    scale: u32,
) {
    let [fw, fh] = size;
    let step = |extent: u32| {
        if extent <= RULER_FINE_EXTENT {
            RULER_FINE_STEP
        } else {
            RULER_INDEX_STEP
        }
    };

    let step_x = step(fw);
    for texel in 0..fw {
        let x = origin.0 + texel * scale;
        let length = if texel % 4 == 0 {
            RULER_LONG_TICK
        } else {
            RULER_TICK
        };
        fill_block(
            buffer,
            width,
            height,
            x,
            origin.1 - length,
            1,
            length,
            RULER_COLOR,
        );
        if texel % step_x == 0 {
            stamp_text(
                buffer,
                width,
                height,
                x + 1,
                origin.1 - RULER_BAND,
                &texel.to_string(),
                RULER_SCALE,
                RULER_COLOR,
            );
        }
    }

    let step_y = step(fh);
    for texel in 0..fh {
        let y = origin.1 + texel * scale;
        let length = if texel % 4 == 0 {
            RULER_LONG_TICK
        } else {
            RULER_TICK
        };
        fill_block(
            buffer,
            width,
            height,
            origin.0 - length,
            y,
            length,
            1,
            RULER_COLOR,
        );
        if texel % step_y == 0 {
            stamp_text(
                buffer,
                width,
                height,
                origin.0 - RULER_BAND,
                y + 1,
                &texel.to_string(),
                RULER_SCALE,
                RULER_COLOR,
            );
        }
    }
}

/// Straight-alpha `over`: `out = (src·a + dst·(255 − a) + 127) / 255`, in 8-bit integers so the
/// composite is the same byte on every platform. Alpha comes out opaque — the sheet's background is.
fn over_straight(src: [u8; 4], dst: [u8; 4]) -> [u8; 4] {
    let alpha = u32::from(src[3]);
    let mix = |s: u8, d: u8| -> u8 {
        let value = (u32::from(s) * alpha + u32::from(d) * (255 - alpha) + 127) / 255;
        value as u8
    };
    [
        mix(src[0], dst[0]),
        mix(src[1], dst[1]),
        mix(src[2], dst[2]),
        255,
    ]
}

/// Premultiplied `over`: `out = src + dst·(255 − a) / 255`. The atlas PNG is premultiplied by the
/// bake's own convention, so its colour is already weighted by its coverage.
fn over_premultiplied(src: [u8; 4], dst: [u8; 4]) -> [u8; 4] {
    let inverse = 255 - u32::from(src[3]);
    let mix = |s: u8, d: u8| -> u8 {
        let value = u32::from(s) + (u32::from(d) * inverse + 127) / 255;
        value.min(255) as u8
    };
    [
        mix(src[0], dst[0]),
        mix(src[1], dst[1]),
        mix(src[2], dst[2]),
        255,
    ]
}

// --- Atlas overview -----------------------------------------------------------------------------

/// Composes the atlas overview from an already-loaded document and its packed image.
///
/// Split from [`render_atlas_sheet`] so the composition is exercised without touching the
/// filesystem; `path` only names the atlas in the header.
fn compose_atlas_sheet(path: &Path, doc: &AtlasDocument, packed: &Image) -> Image {
    let upscale = atlas_upscale(packed.width, packed.height);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let header = format!(
        "{name}  {}x{} texels  {} regions  ppu {}  filter {}",
        packed.width,
        packed.height,
        doc.regions.len(),
        doc.pixels_per_unit,
        match doc.filter {
            ludion_scene::AtlasFilter::Nearest => "nearest",
            ludion_scene::AtlasFilter::Linear => "linear",
        }
    );
    let width = (packed.width * upscale)
        .max(ATLAS_MIN_WIDTH)
        .max(text_px(&header, HEADER_SCALE))
        .min(MAX_SHEET_PX);
    let height = (SHEET_HEADER_H + packed.height * upscale).min(MAX_SHEET_PX);
    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    fill(&mut rgba, width, height, BACKGROUND);
    stamp_text(
        &mut rgba,
        width,
        height,
        LABEL_MARGIN,
        LABEL_MARGIN,
        &header,
        HEADER_SCALE,
        TITLE_COLOR,
    );

    for y in 0..packed.height {
        for x in 0..packed.width {
            let index = ((y * packed.width + x) * 4) as usize;
            let texel: [u8; 4] = packed.rgba[index..index + 4]
                .try_into()
                .expect("a four-byte RGBA texel");
            fill_block(
                &mut rgba,
                width,
                height,
                x * upscale,
                SHEET_HEADER_H + y * upscale,
                upscale,
                upscale,
                over_premultiplied(texel, BACKGROUND),
            );
        }
    }

    // `regions` is a `BTreeMap`, so this walk is name order — the same colour lands on the same
    // region every run.
    for (index, (name, region)) in doc.regions.iter().enumerate() {
        let color = REGION_COLORS[index % REGION_COLORS.len()];
        let [rx, ry, rw, rh] = region.rect;
        let x0 = rx * upscale;
        let y0 = SHEET_HEADER_H + ry * upscale;
        let w = rw * upscale;
        let h = rh * upscale;
        // A one-texel frame, so the outline reads at the same weight the art does.
        fill_block(&mut rgba, width, height, x0, y0, w, upscale, color);
        fill_block(
            &mut rgba,
            width,
            height,
            x0,
            y0 + h.saturating_sub(upscale),
            w,
            upscale,
            color,
        );
        fill_block(&mut rgba, width, height, x0, y0, upscale, h, color);
        fill_block(
            &mut rgba,
            width,
            height,
            x0 + w.saturating_sub(upscale),
            y0,
            upscale,
            h,
            color,
        );

        // The name sits just above its rectangle — or just *inside* it when the region is packed
        // against the top of the atlas, where above would mean over the header or off the image.
        // Either way it never lands on the outline it names.
        let label_h = 8 * REGION_LABEL_SCALE;
        let label_y = if y0 > SHEET_HEADER_H + label_h {
            y0 - label_h - 1
        } else {
            y0 + upscale + 1
        };
        stamp_text(
            &mut rgba,
            width,
            height,
            x0,
            label_y,
            name,
            REGION_LABEL_SCALE,
            color,
        );

        let px = x0 + (region.pivot[0].max(0.0).floor() as u32).min(rw.saturating_sub(1)) * upscale;
        let py = y0 + (region.pivot[1].max(0.0).floor() as u32).min(rh.saturating_sub(1)) * upscale;
        fill_block(
            &mut rgba,
            width,
            height,
            px.saturating_sub(PIVOT_ARM),
            py,
            PIVOT_ARM * 2 + 1,
            1,
            PIVOT_COLOR,
        );
        fill_block(
            &mut rgba,
            width,
            height,
            px,
            py.saturating_sub(PIVOT_ARM),
            1,
            PIVOT_ARM * 2 + 1,
            PIVOT_COLOR,
        );
    }

    finish(width, height, rgba)
}

/// The integer upscale the overview draws a `w`×`h` atlas at: enough to reach
/// [`ATLAS_TARGET_WIDTH`], capped at [`ATLAS_MAX_UPSCALE`], and reduced further if the result would
/// pass [`MAX_SHEET_PX`] on either axis.
fn atlas_upscale(w: u32, h: u32) -> u32 {
    let w = w.max(1);
    let h = h.max(1);
    let mut upscale = if w >= ATLAS_TARGET_WIDTH {
        1
    } else {
        ATLAS_TARGET_WIDTH.div_ceil(w).min(ATLAS_MAX_UPSCALE)
    };
    while upscale > 1 && (w * upscale > MAX_SHEET_PX || SHEET_HEADER_H + h * upscale > MAX_SHEET_PX)
    {
        upscale -= 1;
    }
    upscale
}

// --- Flipbook instruments -----------------------------------------------------------------------

/// `L4008` for a flipbook request the instruments cannot serve, in the wording the model sheet's
/// clip refusals use: what was asked, why it does not apply, and the spelling that does.
fn clip_refusal(message: String, help: &str) -> Diagnostic {
    Diagnostic::new(code::SCENE_OVERRIDE_INVALID, message).with_help(help.to_string())
}

/// The half of the flag shape that survives into an [`AtlasClipRequest`]: a strip and an onion skin
/// are two answers to the same question, and a strip has a range.
fn check_clip_shape(strip: Option<u32>, onion: bool) -> Result<(), Vec<Diagnostic>> {
    if strip.is_some() && onion {
        return Err(vec![clip_refusal(
            "`--strip` and `--onion` are two ways to see one clip at once, and an image shows one \
             of them"
                .to_string(),
            "ask for the phases side by side (`--strip 6`) or stacked (`--onion`), then run the \
             other into a second `--out`",
        )]);
    }
    if let Some(phases) = strip
        && !(MIN_CLIP_PHASES..=MAX_CLIP_PHASES).contains(&phases)
    {
        return Err(vec![clip_refusal(
            format!(
                "`--strip {phases}` is out of range: a strip lays out {MIN_CLIP_PHASES} to \
                 {MAX_CLIP_PHASES} phases"
            ),
            "ask for at least two phases (one phase is `--clip <name> --time <t>`) and at most \
             sixteen, past which a phase is too narrow to read the art in",
        )]);
    }
    Ok(())
}

/// `L4008` for a clip name the atlas carries no flipbook for, listing the names it does carry.
fn unknown_clip(name: &str, doc: &AtlasDocument) -> Diagnostic {
    let held: Vec<&str> = doc
        .flipbooks
        .iter()
        .map(|flipbook| flipbook.name.as_str())
        .collect();
    let detail = if held.is_empty() {
        "this atlas carries no flipbooks at all; declare one under `flipbooks:` in its \
         `kind: atlas-source` document and re-run `generate-atlas`"
            .to_string()
    } else {
        format!("this atlas carries `{}`", held.join("`, `"))
    };
    clip_refusal(
        format!("`--clip {name}` names a flipbook this atlas does not carry"),
        &detail,
    )
}

/// The name a flipbook instrument's header carries: the atlas document's own file name, exactly as
/// the overview's header takes it.
fn atlas_title(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// One drawn phase: the instant, the entry showing at it, and where that entry sits in the atlas.
struct ClipPhase {
    time: f64,
    frame: String,
    region: AtlasRegion,
}

/// One overlaid entry of an onion skin, in `frames:` order.
struct OnionEntry {
    frame: String,
    region: AtlasRegion,
    /// Whether this is the entry showing — the one drawn at full weight, above the ghosts.
    opaque: bool,
}

/// The region `name` resolves to. `check_atlas` resolves every flipbook frame against the region
/// map before this runs, so a miss here is impossible rather than handled.
fn clip_region(doc: &AtlasDocument, name: &str) -> AtlasRegion {
    *doc.region(name)
        .expect("check_atlas resolves every flipbook frame against the region map")
}

/// The phases a strip (or a lone instant) draws, in draw order. The instants come from
/// [`clip_phase_times`]; each is resolved to an entry through [`ludion_scene::flipbook_frame`], the
/// evaluator a drawn sprite uses.
fn clip_phases(
    doc: &AtlasDocument,
    flipbook: &Flipbook,
    strip: Option<u32>,
    time: Option<f64>,
) -> Vec<ClipPhase> {
    clip_phase_times(flipbook, strip, time)
        .into_iter()
        .map(|time| {
            let frame = flipbook.frames[ludion_scene::flipbook_frame(flipbook, time, 1.0)].clone();
            let region = clip_region(doc, &frame);
            ClipPhase {
                time,
                frame,
                region,
            }
        })
        .collect()
}

/// The instants a strip (or a lone instant) samples a flipbook clip at.
///
/// A one-shot strip keeps [`crate::model_sheet::phase_times`]' own spacing — `n − 1` phases across
/// the duration, both endpoints shown — the rule shared with `render --model --strip`.
///
/// A **looping** strip diverges from it on purpose: `phase_times`' `t_i = i × duration / n` samples
/// each phase's *opening* instant, which for `n` equal to the clip's frame count lands every phase
/// exactly on a `flipbook_frame` boundary. `flipbook_frame` resolves a boundary to the frame it
/// *opens* (`floor(elapsed × fps)`), but `t_i` is a divided float, not an exact fraction, so rounding
/// noise routinely lands it a hair below the boundary instead — onto the frame the boundary *closes*.
/// At `n = 3` on a 3-frame loop every phase does this, so the strip shows frame 0 three times and
/// never reaches frames 1 or 2. Sampling the *centre* of each phase's span instead —
/// `t_i = (i + 0.5) × duration / n` — sits strictly inside the phase's frame, so no rounding
/// direction can carry it across a neighbouring boundary.
fn clip_phase_times(flipbook: &Flipbook, strip: Option<u32>, time: Option<f64>) -> Vec<f64> {
    let Some(phases) = strip else {
        return vec![time.unwrap_or(0.0)];
    };
    if !flipbook.looped {
        return crate::model_sheet::phase_times(flipbook.duration(), flipbook.looped, strip, time);
    }
    let n = f64::from(phases);
    (0..phases)
        .map(|index| (f64::from(index) + 0.5) * flipbook.duration() / n)
        .collect()
}

/// Which entry of `frames:` an onion skin draws opaque: the one showing at `time`, or the clip's
/// last entry when no instant was asked for.
///
/// Every entry *before* it is ghosted and every entry after it is left out — an onion skin reads
/// forward from the start of the clip to the pose being judged, which is what makes the ghosts a
/// history rather than a blur.
fn onion_opaque(flipbook: &Flipbook, time: Option<f64>) -> usize {
    match time {
        Some(time) => ludion_scene::flipbook_frame(flipbook, time, 1.0),
        None => flipbook.frames.len().saturating_sub(1),
    }
}

/// The entries an onion skin overlays, in `frames:` order, up to and including the opaque one.
fn onion_entries(doc: &AtlasDocument, flipbook: &Flipbook, time: Option<f64>) -> Vec<OnionEntry> {
    let opaque = onion_opaque(flipbook, time);
    flipbook.frames[..=opaque.min(flipbook.frames.len().saturating_sub(1))]
        .iter()
        .enumerate()
        .map(|(index, frame)| OnionEntry {
            frame: frame.clone(),
            region: clip_region(doc, frame),
            opaque: index == opaque,
        })
        .collect()
}

/// Reduces `scale` until `extent` fits [`MAX_SHEET_PX`] on both axes, never below one texel per
/// pixel — [`fit_scales`]' rule, over the one factor a flipbook instrument has. Returns the scale
/// and whether it was reduced, so the header can say `(clamped)`.
fn fit_clip_scale(scale: u32, extent: impl Fn(u32) -> (u32, u32)) -> (u32, bool) {
    let asked = scale.max(1);
    let mut scale = asked;
    while scale > 1 {
        let (width, height) = extent(scale);
        if width <= MAX_SHEET_PX && height <= MAX_SHEET_PX {
            break;
        }
        scale -= 1;
    }
    (scale, scale < asked)
}

/// The header line every flipbook instrument carries: the atlas, the clip, and the clip's own shape.
fn clip_header(title: &str, flipbook: &Flipbook, scale: u32, clamped: bool) -> String {
    format!(
        "{title}  clip {}  {} frames  {} fps  {:.3}s  {}  x{scale}{}",
        flipbook.name,
        flipbook.frames.len(),
        flipbook.fps,
        flipbook.duration(),
        if flipbook.looped { "loop" } else { "one-shot" },
        if clamped { "  (clamped)" } else { "" }
    )
}

/// Composes a row of phases: one cell each, the first ruled, every one captioned with its instant
/// and the entry showing at it. A lone instant is the one-cell case of the same layout.
fn compose_clip_strip(
    title: &str,
    flipbook: &Flipbook,
    phases: &[ClipPhase],
    packed: &Image,
    scale: u32,
) -> (Image, Vec<String>) {
    let labels: Vec<String> = phases
        .iter()
        .map(|phase| format!("t={:.3}s  {}", phase.time, phase.frame))
        .collect();
    let label_px = labels
        .iter()
        .map(|label| text_px(label, CLIP_LABEL_SCALE))
        .max()
        .unwrap_or(0);
    let cols = phases.len().max(1) as u32;
    let tile_w = phases
        .iter()
        .map(|phase| phase.region.rect[2])
        .max()
        .unwrap_or(1);
    let tile_h = phases
        .iter()
        .map(|phase| phase.region.rect[3])
        .max()
        .unwrap_or(1);

    let cell_width = |scale: u32| (RULER_BAND + tile_w * scale + CELL_GAP).max(label_px);
    let (scale, clamped) = fit_clip_scale(scale, |scale| {
        (
            cols * cell_width(scale) + CELL_GAP,
            SHEET_HEADER_H + RULER_BAND + tile_h * scale + CLIP_LABEL_H + CELL_GAP,
        )
    });

    let cell_w = cell_width(scale);
    let header = clip_header(title, flipbook, scale, clamped);
    let width = (cols * cell_w + CELL_GAP)
        .max(text_px(&header, HEADER_SCALE))
        .min(MAX_SHEET_PX);
    let height =
        (SHEET_HEADER_H + RULER_BAND + tile_h * scale + CLIP_LABEL_H + CELL_GAP).min(MAX_SHEET_PX);

    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    fill(&mut rgba, width, height, BACKGROUND);
    stamp_text(
        &mut rgba,
        width,
        height,
        LABEL_MARGIN,
        LABEL_MARGIN,
        &header,
        HEADER_SCALE,
        TITLE_COLOR,
    );

    let tile_y = SHEET_HEADER_H + RULER_BAND;
    for (index, phase) in phases.iter().enumerate() {
        let cell_x = index as u32 * cell_w;
        let tile = (cell_x + RULER_BAND, tile_y);
        draw_region(
            &mut rgba,
            width,
            height,
            tile,
            packed,
            phase.region.rect,
            scale,
            false,
        );
        // One ruler, on the first cell, exactly as the contact sheet rules only its first tile: the
        // cells share a scale, so a second ruler would count the same texels again.
        if index == 0 {
            draw_ruler(
                &mut rgba,
                width,
                height,
                tile,
                [phase.region.rect[2], phase.region.rect[3]],
                scale,
            );
        }
        stamp_text(
            &mut rgba,
            width,
            height,
            cell_x,
            tile_y + tile_h * scale + LABEL_MARGIN,
            &labels[index],
            CLIP_LABEL_SCALE,
            LABEL_COLOR,
        );
    }

    let echo = labels
        .iter()
        .enumerate()
        .map(|(index, label)| format!("phase {index}  {label}"))
        .collect();
    (finish(width, height, rgba), echo)
}

/// Composes the onion skin: every entry up to the one showing, overlaid in one cell and aligned by
/// the regions' own pivots, the ghosts first and the opaque entry last so it reads on top.
///
/// A region's pivot defaults to its centre at bake time, so a clip whose frames author no pivot
/// aligns by centre — the sensible default falls out of the same arithmetic rather than a branch.
fn compose_clip_onion(
    title: &str,
    flipbook: &Flipbook,
    entries: &[OnionEntry],
    packed: &Image,
    scale: u32,
) -> (Image, Vec<String>) {
    let pivots: Vec<(u32, u32)> = entries
        .iter()
        .map(|entry| {
            (
                pivot_texel(entry.region.pivot[0], entry.region.rect[2]),
                pivot_texel(entry.region.pivot[1], entry.region.rect[3]),
            )
        })
        .collect();
    // The cell reaches from the furthest any entry extends before its pivot to the furthest any
    // extends after it, so every entry fits with its pivot on one shared anchor.
    let anchor = (
        pivots.iter().map(|pivot| pivot.0).max().unwrap_or(0),
        pivots.iter().map(|pivot| pivot.1).max().unwrap_or(0),
    );
    let reach = |axis: usize| {
        entries
            .iter()
            .zip(&pivots)
            .map(|(entry, pivot)| {
                let extent = entry.region.rect[2 + axis];
                let ahead = if axis == 0 { pivot.0 } else { pivot.1 };
                extent.saturating_sub(ahead)
            })
            .max()
            .unwrap_or(1)
    };
    let tile_w = anchor.0 + reach(0);
    let tile_h = anchor.1 + reach(1);

    let footer: Vec<String> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            format!(
                "{index}  {}  {}",
                entry.frame,
                if entry.opaque { "showing" } else { "ghost" }
            )
        })
        .collect();
    let footer_px = text_width(&footer, LABEL_SCALE);
    let footer_h = footer.len() as u32 * FOOTER_LINE_H + 2 * LABEL_MARGIN;

    let cell_height =
        |scale: u32| SHEET_HEADER_H + RULER_BAND + tile_h * scale + CELL_GAP + BLOCK_GAP;
    let (scale, clamped) = fit_clip_scale(scale, |scale| {
        (
            RULER_BAND + tile_w * scale + CELL_GAP,
            cell_height(scale) + footer_h,
        )
    });

    let header = clip_header(title, flipbook, scale, clamped);
    let width = (RULER_BAND + tile_w * scale + CELL_GAP)
        .max(text_px(&header, HEADER_SCALE))
        .max(footer_px)
        .min(MAX_SHEET_PX);
    let footer_y = cell_height(scale);
    let height = (footer_y + footer_h).min(MAX_SHEET_PX);

    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    fill(&mut rgba, width, height, BACKGROUND);
    stamp_text(
        &mut rgba,
        width,
        height,
        LABEL_MARGIN,
        LABEL_MARGIN,
        &header,
        HEADER_SCALE,
        TITLE_COLOR,
    );

    let tile = (RULER_BAND, SHEET_HEADER_H + RULER_BAND);
    draw_ruler(&mut rgba, width, height, tile, [tile_w, tile_h], scale);
    for (entry, pivot) in entries.iter().zip(&pivots) {
        let origin = (
            tile.0 + (anchor.0 - pivot.0) * scale,
            tile.1 + (anchor.1 - pivot.1) * scale,
        );
        draw_region(
            &mut rgba,
            width,
            height,
            origin,
            packed,
            entry.region.rect,
            scale,
            !entry.opaque,
        );
    }
    // The one point every entry was stacked on: without it the alignment is a claim the image makes
    // and cannot show.
    draw_cross(
        &mut rgba,
        width,
        height,
        (tile.0 + anchor.0 * scale, tile.1 + anchor.1 * scale),
    );

    draw_footer(&mut rgba, width, height, footer_y, &footer);

    let echo = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            format!(
                "onion {index}  {}  {}",
                entry.frame,
                if entry.opaque { "showing" } else { "ghost" }
            )
        })
        .collect();
    (finish(width, height, rgba), echo)
}

/// A pivot's whole-texel position inside a region of `extent` texels: floored, so the marker lands
/// on the texel the pivot is in, and clamped inside the region.
fn pivot_texel(value: f64, extent: u32) -> u32 {
    let floored = value.max(0.0).floor();
    if floored <= 0.0 {
        0
    } else {
        (floored as u32).min(extent.saturating_sub(1))
    }
}

/// Draws one region of the packed atlas at `origin`, nearest-upscaled by `scale` and composited
/// **over whatever is already there** — which is the sheet background for every instrument but the
/// onion skin, where it is the ghosts already drawn.
///
/// A `ghosted` entry keeps [`ONION_GHOST`] of its own weight. The texels are premultiplied, so that
/// is one multiply on all four channels and the result is still premultiplied.
#[allow(clippy::too_many_arguments)]
fn draw_region(
    buffer: &mut [u8],
    width: u32,
    height: u32,
    origin: (u32, u32),
    packed: &Image,
    rect: [u32; 4],
    scale: u32,
    ghosted: bool,
) {
    let [rx, ry, rw, rh] = rect;
    for y in 0..rh {
        for x in 0..rw {
            let (sx, sy) = (rx + x, ry + y);
            if sx >= packed.width || sy >= packed.height {
                continue;
            }
            let index = ((sy * packed.width + sx) * 4) as usize;
            let texel: [u8; 4] = packed.rgba[index..index + 4]
                .try_into()
                .expect("a four-byte RGBA texel");
            let texel = if ghosted { ghost(texel) } else { texel };
            let (px, py) = (origin.0 + x * scale, origin.1 + y * scale);
            let under = pixel_at(buffer, width, height, px, py);
            fill_block(
                buffer,
                width,
                height,
                px,
                py,
                scale,
                scale,
                over_premultiplied(texel, under),
            );
        }
    }
}

/// Scales a premultiplied texel down to [`ONION_GHOST`] of its own weight, in integers.
fn ghost(texel: [u8; 4]) -> [u8; 4] {
    let (numerator, denominator) = ONION_GHOST;
    let faint = |channel: u8| -> u8 {
        ((u32::from(channel) * numerator + denominator / 2) / denominator) as u8
    };
    [
        faint(texel[0]),
        faint(texel[1]),
        faint(texel[2]),
        faint(texel[3]),
    ]
}

/// The pixel already at `(x, y)`, or the sheet background when that is outside the buffer — the
/// destination a composite reads.
fn pixel_at(buffer: &[u8], width: u32, height: u32, x: u32, y: u32) -> [u8; 4] {
    if x >= width || y >= height {
        return BACKGROUND;
    }
    let index = ((y * width + x) * 4) as usize;
    buffer[index..index + 4]
        .try_into()
        .expect("a four-byte RGBA pixel")
}

/// Marks one point with the pivot cross the other sprite instruments use.
fn draw_cross(buffer: &mut [u8], width: u32, height: u32, at: (u32, u32)) {
    fill_block(
        buffer,
        width,
        height,
        at.0.saturating_sub(PIVOT_ARM),
        at.1,
        PIVOT_ARM * 2 + 1,
        1,
        PIVOT_COLOR,
    );
    fill_block(
        buffer,
        width,
        height,
        at.0,
        at.1.saturating_sub(PIVOT_ARM),
        1,
        PIVOT_ARM * 2 + 1,
        PIVOT_COLOR,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPRITE: &str = "\
format: 1
kind: sprite
size:
  - 4
  - 3
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
frames:
  idle:
    rows:
      - \"A...\"
      - \".A..\"
      - \"..A.\"
effects:
  - op: outline
    color:
      - 0.0
      - 0.0
      - 0.0
      - 1.0
  - op: noise
    amount: 0.1
";

    fn doc() -> SpriteDocument {
        ludion_scene::check_sprite(SPRITE).expect("the fixture is a valid sprite")
    }

    fn over(spec: &str) -> SceneOverride {
        SceneOverride::parse(spec).expect("the fixture override parses")
    }

    #[test]
    fn the_effect_echo_names_every_op_in_application_order() {
        let lines = effect_lines(&doc().effects);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("0. outline"), "{}", lines[0]);
        assert!(lines[0].contains("glyphs: all"), "{}", lines[0]);
        assert!(lines[0].contains("frames: all"), "{}", lines[0]);
        assert!(lines[0].contains("color=[0, 0, 0, 1]"), "{}", lines[0]);
        assert!(lines[1].starts_with("1. noise"), "{}", lines[1]);
        assert!(lines[1].contains("amount=0.1"), "{}", lines[1]);
    }

    /// An empty list says so out loud rather than leaving the footer blank — "no effects ran" and
    /// "the sheet forgot to echo them" must not look the same.
    #[test]
    fn an_empty_effect_list_footers_as_none() {
        assert_eq!(footer_lines(&[], &[]), vec!["effects: none".to_string()]);
    }

    #[test]
    fn every_supported_pointer_shape_writes_its_field() {
        let mut sprite = doc();
        let applied = apply_sprite_overrides(
            &mut sprite,
            &[
                over("/seed=9"),
                over("/palette/A/color/0=0.25"),
                over("/palette/A/emissive=2.0"),
                over("/frames/idle/pivot/1=0.0"),
                over("/effects/0/color/3=0.5"),
                over("/effects/0/side=inside"),
                over("/effects/1/amount=0.4"),
            ],
        )
        .expect("every pointer is a supported shape");

        assert_eq!(sprite.seed, Some(9));
        assert_eq!(sprite.palette[0].color[0], 0.25);
        assert_eq!(sprite.palette[0].emissive, Some(2.0));
        assert_eq!(sprite.frames[0].pivot, Some([2.0, 0.0]));
        assert!(matches!(
            sprite.effects[0].op,
            SpriteEffectOp::Outline {
                side: OutlineSide::Inside,
                ..
            }
        ));
        assert!(matches!(
            sprite.effects[1].op,
            SpriteEffectOp::Noise { amount } if amount == 0.4
        ));

        assert_eq!(applied[1].from, Some(Value::from(1.0)));
        assert_eq!(applied[2].from, None, "`emissive` was not in the file");
        assert_eq!(applied[3].from, None, "`pivot` was not in the file");
        assert_eq!(applied[5].from, Some(Value::from("outside")));
    }

    /// The shape half of the applier's contract: a pointer at the wrong depth, into a field that is
    /// document *shape*, or naming something the document does not hold, is `L4008`.
    #[test]
    fn unsupported_pointers_are_refused() {
        for spec in [
            "/size/0=8",
            "/frames/idle/rows/0=AAAA",
            "/palette/Z/color/0=1.0",
            "/palette/A/color/4=1.0",
            "/frames/walk/pivot/0=1.0",
            "/effects/9/amount=0.2",
            "/effects/1/radius=2",
            "/effects/0/color=1.0",
            "/effects/1/amount/0=0.2",
        ] {
            let mut sprite = doc();
            let diagnostics = apply_sprite_overrides(&mut sprite, &[over(spec)])
                .expect_err("the pointer is out of the sprite override domain");
            assert_eq!(diagnostics[0].code.as_str(), "L4008", "{spec}");
        }
    }

    /// Range is the document validator's job, not the applier's — so an in-shape, out-of-range
    /// write is accepted here and rejected there, exactly as the same edit to the file would be.
    #[test]
    fn an_out_of_range_value_is_left_to_the_documents_own_validator() {
        let mut sprite = doc();
        apply_sprite_overrides(&mut sprite, &[over("/palette/A/color/0=4.0")])
            .expect("the shape is legal");
        assert!(!ludion_scene::validate_sprite(&sprite).is_empty());
    }

    /// A keyword parameter takes only its op's own keywords.
    #[test]
    fn an_unknown_keyword_is_refused() {
        let mut sprite = doc();
        let diagnostics = apply_sprite_overrides(&mut sprite, &[over("/effects/0/side=sideways")])
            .expect_err("`sideways` is not an outline side");
        assert_eq!(diagnostics[0].code.as_str(), "L4008");
        let help = diagnostics[0].help.as_deref().unwrap();
        assert!(help.contains("outside"), "{help}");
    }

    /// A palette glyph may itself be `/` or `~`, so the pointer's segment is RFC 6901-unescaped.
    #[test]
    fn an_escaped_glyph_segment_resolves() {
        let source = SPRITE.replace('A', "/").replace("  /:", "  \"/\":");
        let mut sprite =
            ludion_scene::check_sprite(&source).expect("a `/` glyph is a legal palette key");
        apply_sprite_overrides(&mut sprite, &[over("/palette/~1/color/1=0.5")])
            .expect("the escaped segment resolves");
        assert_eq!(sprite.palette[0].color[1], 0.5);
    }

    /// The near-square grid and the reserved ruler band follow from the layout formula alone.
    #[test]
    fn the_contact_layout_follows_the_formula() {
        let layout = fit_scales([4, 3], 2, 32, 1, 0, 0, 1);
        assert_eq!((layout.cols, near_square(2)), (2, (2, 1)));
        assert_eq!(layout.cell_a.0, RULER_BAND + 4 * 32 + CELL_GAP);
        assert_eq!(
            layout.cell_a.1,
            RULER_BAND + 3 * 32 + TILE_LABEL_H + CELL_GAP
        );
        assert!(!layout.clamped);
    }

    /// A scale that would blow past [`MAX_SHEET_PX`] is reduced, and the caller is told it was.
    #[test]
    fn an_oversized_scale_is_clamped() {
        let layout = fit_scales([64, 64], 8, 2, 4096, 0, 0, 1);
        assert!(layout.clamped);
        assert!(layout.width <= MAX_SHEET_PX, "{}", layout.width);
        assert!(layout.height <= MAX_SHEET_PX, "{}", layout.height);
    }

    /// A tiny atlas is upscaled toward the target width but never past the cap, and a large one is
    /// left at 1:1.
    #[test]
    fn the_atlas_upscale_is_bounded_at_both_ends() {
        assert_eq!(atlas_upscale(8, 8), ATLAS_MAX_UPSCALE);
        assert_eq!(atlas_upscale(64, 64), 8);
        assert_eq!(atlas_upscale(1024, 1024), 1);
        assert_eq!(atlas_upscale(4096, 4096), 1);
    }

    /// The two composites are the ones their conventions call for: a straight-alpha texel weights
    /// its own colour, a premultiplied one is already weighted.
    #[test]
    fn the_composites_match_their_alpha_conventions() {
        assert_eq!(
            over_straight([255, 0, 0, 128], [0, 0, 0, 255]),
            [128, 0, 0, 255]
        );
        assert_eq!(
            over_straight([255, 0, 0, 255], [10, 10, 10, 255]),
            [255, 0, 0, 255]
        );
        assert_eq!(over_straight([9, 9, 9, 0], BACKGROUND), BACKGROUND);
        assert_eq!(
            over_premultiplied([128, 0, 0, 128], [0, 0, 0, 255]),
            [128, 0, 0, 255]
        );
        assert_eq!(over_premultiplied([0, 0, 0, 0], BACKGROUND), BACKGROUND);
    }

    // --- Flipbook instruments ---

    /// Two 2×2 regions side by side, with two clips over them: one looping, one not. The pivots
    /// differ on purpose — `ship/a` is centred and `ship/b` is anchored on its left edge — so the
    /// onion skin's alignment has something to align.
    const ATLAS: &str = "\
format: 1
kind: atlas
generated_from: ship.atlas-source.yaml
pixels_per_unit: 8.0
maps:
  color: ship.png
regions:
  ship/a:
    rect:
      - 0
      - 0
      - 2
      - 2
    pivot:
      - 1.0
      - 1.0
  ship/b:
    rect:
      - 2
      - 0
      - 2
      - 2
    pivot:
      - 0.0
      - 1.0
flipbooks:
  - name: loop
    frames:
      - ship/a
      - ship/b
    fps: 4.0
  - name: once
    frames:
      - ship/a
      - ship/b
    fps: 4.0
    loop: false
";

    fn atlas() -> AtlasDocument {
        ludion_scene::check_atlas(ATLAS).expect("the fixture is a valid atlas")
    }

    /// The 4×2 packed image the fixture's rects address. Every texel is opaque, so the premultiplied
    /// `over` is the identity and an expected pixel is the texel itself.
    fn packed() -> Image {
        let mut rgba = Vec::with_capacity(4 * 2 * 4);
        for y in 0..2u8 {
            for x in 0..4u8 {
                rgba.extend_from_slice(&[10 + x * 40, 20 + y * 60, 30, 255]);
            }
        }
        Image {
            width: 4,
            height: 2,
            hash: ludion_assets::content_hash(&rgba),
            rgba,
        }
    }

    fn pixel(image: &Image, x: u32, y: u32) -> [u8; 4] {
        let index = ((y * image.width + x) * 4) as usize;
        image.rgba[index..index + 4]
            .try_into()
            .expect("a four-byte RGBA pixel")
    }

    /// A looping clip divides its cycle by `n`, sampled at each phase's *centre*
    /// (`t_i = (i + 0.5) × duration / n`, not its opening instant — see [`clip_phase_times`]), and
    /// every phase names the entry showing at it through the same evaluator a drawn sprite uses.
    #[test]
    fn a_looping_strip_spaces_its_phases_across_one_cycle() {
        let doc = atlas();
        let flipbook = doc.flipbook("loop").expect("the fixture binds `loop`");
        assert_eq!(flipbook.duration(), 0.5);

        let phases = clip_phases(&doc, flipbook, Some(4), None);
        let times: Vec<f64> = phases.iter().map(|phase| phase.time).collect();
        assert_eq!(times, vec![0.0625, 0.1875, 0.3125, 0.4375]);
        let frames: Vec<&str> = phases.iter().map(|phase| phase.frame.as_str()).collect();
        assert_eq!(frames, vec!["ship/a", "ship/a", "ship/b", "ship/b"]);
    }

    /// The regression this centre-sampling fix exists for: a strip whose phase count equals a
    /// looping clip's own frame count must show every frame exactly once, in order — not the first
    /// frame twice and the last not at all (the bug: opening-instant sampling landed every phase
    /// exactly on a `flipbook_frame` boundary, and division rounding carried it back onto the
    /// *previous* frame).
    #[test]
    fn a_strip_at_the_frame_count_shows_every_frame_once() {
        const IDLE_ATLAS: &str = "\
format: 1
kind: atlas
generated_from: idle.atlas-source.yaml
pixels_per_unit: 8.0
maps:
  color: idle.png
regions:
  idle-0:
    rect:
      - 0
      - 0
      - 1
      - 1
    pivot:
      - 0.0
      - 0.0
  idle-1:
    rect:
      - 1
      - 0
      - 1
      - 1
    pivot:
      - 0.0
      - 0.0
  idle-2:
    rect:
      - 2
      - 0
      - 1
      - 1
    pivot:
      - 0.0
      - 0.0
flipbooks:
  - name: idle
    frames:
      - idle-0
      - idle-1
      - idle-2
    fps: 10.0
";
        let atlas = ludion_scene::check_atlas(IDLE_ATLAS).expect("the fixture is a valid atlas");
        let flipbook = atlas.flipbook("idle").expect("the fixture binds `idle`");
        assert_eq!(flipbook.duration(), 0.3);
        assert!(flipbook.looped, "loop defaults to true");

        let phases = clip_phases(&atlas, flipbook, Some(3), None);
        let frames: Vec<&str> = phases.iter().map(|phase| phase.frame.as_str()).collect();
        assert_eq!(
            frames,
            vec!["idle-0", "idle-1", "idle-2"],
            "one phase per frame, in order — no repeat, no skip"
        );
    }

    /// A one-shot divides by `n − 1` instead, so both endpoints are drawn — and its final phase
    /// lands on the entry the clip exists to reach.
    #[test]
    fn a_one_shot_strip_shows_both_endpoints() {
        let doc = atlas();
        let flipbook = doc.flipbook("once").expect("the fixture binds `once`");
        let phases = clip_phases(&doc, flipbook, Some(3), None);
        let times: Vec<f64> = phases.iter().map(|phase| phase.time).collect();
        assert_eq!(times, vec![0.0, 0.25, 0.5]);
        assert_eq!(
            phases.last().map(|phase| phase.frame.as_str()),
            flipbook.frames.last().map(String::as_str)
        );
    }

    /// Without a strip there is one phase: the requested instant, or zero.
    #[test]
    fn a_lone_instant_is_the_one_phase_case() {
        let doc = atlas();
        let flipbook = doc.flipbook("loop").expect("the fixture binds `loop`");
        let phases = clip_phases(&doc, flipbook, None, Some(0.3));
        assert_eq!(phases.len(), 1);
        assert_eq!(phases[0].time, 0.3);
        assert_eq!(phases[0].frame, "ship/b");
        assert_eq!(clip_phases(&doc, flipbook, None, None)[0].time, 0.0);
    }

    /// An onion skin reads forward to the entry showing: everything before it ghosts, everything
    /// after it is left out, and an absent `--time` means the clip's last entry.
    #[test]
    fn the_onion_stacks_up_to_the_entry_showing() {
        let doc = atlas();
        let flipbook = doc.flipbook("loop").expect("the fixture binds `loop`");

        let whole = onion_entries(&doc, flipbook, None);
        assert_eq!(whole.len(), 2);
        assert!(!whole[0].opaque && whole[1].opaque);

        let early = onion_entries(&doc, flipbook, Some(0.0));
        assert_eq!(
            early.len(),
            1,
            "the entries after the one showing are omitted"
        );
        assert!(early[0].opaque);

        let once = doc.flipbook("once").expect("the fixture binds `once`");
        assert_eq!(
            onion_opaque(once, Some(99.0)),
            1,
            "a one-shot holds its last entry however far past its end the instant sits"
        );
    }

    /// One cell per phase, in phase order, each showing the entry its caption names: the first cell
    /// holds `ship/a`'s texels and the last holds `ship/b`'s, at one pixel per texel.
    #[test]
    fn a_strip_draws_one_cell_per_phase_in_order() {
        let doc = atlas();
        let flipbook = doc.flipbook("loop").expect("the fixture binds `loop`");
        let phases = clip_phases(&doc, flipbook, Some(4), None);
        let packed = packed();
        let (image, echo) = compose_clip_strip("ship.atlas.yaml", flipbook, &phases, &packed, 1);

        assert_eq!(echo.len(), 4);
        assert!(
            echo[3].contains("t=0.438s") && echo[3].contains("ship/b"),
            "{}",
            echo[3]
        );

        let label_px = phases
            .iter()
            .map(|phase| {
                text_px(
                    &format!("t={:.3}s  {}", phase.time, phase.frame),
                    CLIP_LABEL_SCALE,
                )
            })
            .max()
            .expect("four phases");
        let cell_w = (RULER_BAND + 2 + CELL_GAP).max(label_px);
        let tile_y = SHEET_HEADER_H + RULER_BAND;
        // Cell 0 shows `ship/a`, whose rect starts at the packed image's origin; cell 3 shows
        // `ship/b`, whose rect starts two texels along. Both are opaque, so the composite over the
        // sheet background is the texel itself.
        assert_eq!(pixel(&image, RULER_BAND, tile_y), pixel(&packed, 0, 0));
        assert_eq!(
            pixel(&image, 3 * cell_w + RULER_BAND, tile_y),
            pixel(&packed, 2, 0)
        );
    }

    /// The onion skin stacks its entries on one anchor: `ship/b`'s pivot sits on its left edge and
    /// `ship/a`'s in its middle, so `b` is drawn one texel right of `a` and both pivots land on the
    /// same column. The ghosts keep only their fraction of their own weight.
    #[test]
    fn the_onion_aligns_its_entries_on_one_pivot() {
        let doc = atlas();
        let flipbook = doc.flipbook("loop").expect("the fixture binds `loop`");
        let entries = onion_entries(&doc, flipbook, None);
        let packed = packed();
        // Four pixels a texel, so the probes below sit clear of the anchor cross drawn over the
        // stack's own pivot column.
        let (image, echo) = compose_clip_onion("ship.atlas.yaml", flipbook, &entries, &packed, 4);

        assert_eq!(echo.len(), 2);
        assert!(echo[0].contains("ghost") && echo[1].contains("showing"));

        let tile = (RULER_BAND, SHEET_HEADER_H + RULER_BAND);
        // The anchor is the largest pivot, `ship/a`'s x of 1, so `a` draws at the tile origin and
        // `b` — pivoted at 0 — draws one texel to its right.
        assert_eq!(
            pixel(&image, tile.0 + 1, tile.1),
            over_premultiplied(ghost(pixel(&packed, 0, 0)), BACKGROUND)
        );
        assert_eq!(pixel(&image, tile.0 + 5, tile.1), pixel(&packed, 2, 0));
    }

    /// A ghost is the same premultiplied art at a fraction of its weight, never a different colour.
    #[test]
    fn a_ghost_keeps_its_fraction_of_every_channel() {
        assert_eq!(ghost([100, 200, 0, 200]), [35, 70, 0, 70]);
        assert_eq!(ghost([0, 0, 0, 0]), [0, 0, 0, 0]);
    }

    /// The three flag misshapes are `L4008` with the spelling that works, and an unmodified
    /// `--atlas` passes through.
    #[test]
    fn the_clip_flags_refuse_what_they_cannot_serve() {
        assert!(check_atlas_clip_flags(None, None, None, false).is_ok());
        for (time, strip, onion) in [
            (Some(0.5), None, false),
            (None, Some(4), false),
            (None, None, true),
        ] {
            let diagnostics = check_atlas_clip_flags(None, time, strip, onion)
                .expect_err("a modifier with no clip to modify is refused");
            assert_eq!(diagnostics[0].code.as_str(), "L4008");
            assert!(
                diagnostics[0].message.contains("`--clip`"),
                "{}",
                diagnostics[0].message
            );
        }

        let both = check_atlas_clip_flags(Some("loop"), None, Some(4), true)
            .expect_err("a strip and an onion skin are two answers to one question");
        assert_eq!(both[0].code.as_str(), "L4008");

        for phases in [1, MAX_CLIP_PHASES + 1] {
            let out_of_range = check_atlas_clip_flags(Some("loop"), None, Some(phases), false)
                .expect_err("the phase count is bounded at both ends");
            assert_eq!(out_of_range[0].code.as_str(), "L4008");
        }
    }

    /// An unknown clip name lists what the atlas does carry — or says out loud that it carries
    /// nothing, which is a different mistake with a different fix.
    #[test]
    fn an_unknown_clip_names_the_flipbooks_the_atlas_carries() {
        let doc = atlas();
        let diagnostic = unknown_clip("bank", &doc);
        assert_eq!(diagnostic.code.as_str(), "L4008");
        let help = diagnostic.help.as_deref().expect("a refusal carries help");
        assert!(help.contains("loop") && help.contains("once"), "{help}");

        let bare = AtlasDocument::default();
        let help = unknown_clip("bank", &bare)
            .help
            .expect("a refusal carries help");
        assert!(help.contains("no flipbooks"), "{help}");
    }

    /// Every character the sheets stamp has a glyph — a missing one is silent at runtime, so the
    /// alphabet is pinned here instead.
    #[test]
    fn the_font_covers_the_label_alphabet() {
        for ch in "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 .,:;=-_/\\[]()<>#*~!?%@'\"+".chars() {
            assert!(
                ludion_font::FontFace::embedded().glyph(ch).is_some(),
                "no glyph for `{ch}`"
            );
        }
    }
}
