//! Voxel model documents: `voxels/*.voxel.yaml` — one strict-YAML document per model.
//!
//! A voxel document is a textual, palette-indexed description of a small 3D grid: `size` (the
//! `[x, y, z]` extent in cells), a `palette` mapping single printable-ASCII glyphs to colors, and
//! `slices` — one row-string layer per y level, bottom-up (y = 0 first), each layer a sequence of
//! `size.z` rows of exactly `size.x` characters (row `r` is `z = r`, column `c` is `x = c`). It is
//! the most LLM-friendly 3D authoring representation there is: discrete, spatially readable as
//! text, and diffable (see `Plan/phase-10-voxel-models.md`).
//!
//! This module is a sibling of [`crate::mesh`], structured identically. It owns voxel-document
//! *structure* (shape/type checking, `L1113`) and *data consistency* (slice dimensions match
//! `size`, every glyph is `.` or a palette entry, palette keys are single printable-ASCII glyphs,
//! colors are finite; `L1114`), plus the non-fatal cell-count style budget (`L1115`). It reuses the
//! crate's shared strict-YAML layer ([`crate::yaml`]) and the same canonical-writer primitives
//! ([`crate::scalar`]) the scene and mesh emitters use, so a voxel document is ryu-float,
//! byte-idempotent canonical text for the same reasons.
//!
//! The row-string encoding (not one block scalar per layer) is deliberate: block scalars are
//! rejected by the crate's own strict subset (`L1006`), and equal-width row-string sequences are
//! the proven `Tilemap.rows` shape. ASCII-only glyphs keep column alignment exact — a multi-byte
//! glyph would break the one property the format exists to provide.

use std::collections::VecDeque;

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};
use ludion_core::suggest;
use serde_json::Value;

use crate::envelope;
use crate::overrides::{AppliedOverride, SceneOverride, unescape};
use crate::scalar::{format_float, scalar_string};
use crate::voxel_edit::VoxelComments;
use crate::yaml::{self, MapEntry, Node, NodeValue, pointer_token};

/// The voxel document format version this crate reads and writes.
pub const VOXEL_FORMAT_VERSION: u64 = 1;

/// Total cell count (`size.x * size.y * size.z`) above which [`crate::voxel_budget_warning`] warns
/// (`L1115`) — a style guardrail, not a hard limit. 64³ is generous; author-scale models live in
/// the 16³–48³ range, and world-scale content belongs in chunks, not one huge file.
pub const VOXEL_CELL_BUDGET: u64 = 262_144;

/// The most cells (`size.x * size.y * size.z`) any grid a voxel verb or an assembly builds may hold
/// (256³) — a hard limit, where [`VOXEL_CELL_BUDGET`] is a style warning. It keeps a `grow`, an
/// `upscale`, an assembly `size` or `voxel new --size` from allocating without bound, and matches
/// the sculpt lattice's cell ceiling.
pub const VOXEL_MAX_CELLS: u64 = 16_777_216;

/// The reserved glyph meaning "empty cell". It can never name a palette entry.
const EMPTY_GLYPH: char = '.';

const VOXEL_KEYS: [&str; 9] = [
    "format",
    "kind",
    "name",
    "size",
    "scale",
    "generated_from",
    "budget_exempt",
    "palette",
    "slices",
];

/// One palette entry: a glyph and its RGBA color.
///
/// `glyph` is stored exactly as authored — a single printable-ASCII character in a valid document,
/// but held as a `String` so [`crate::validate_voxel`] can diagnose a multi-character, non-ASCII,
/// or reserved key rather than silently dropping it. Colors are `f64` (text round-trips losslessly; the
/// renderer narrows to `f32`), so this crate never depends on a GPU type.
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteEntry {
    /// The palette key exactly as authored. Valid keys are one printable-ASCII glyph (`0x21..=0x7E`,
    /// excluding the reserved `.`); [`crate::validate_voxel`] reports anything else as `L1114`.
    pub glyph: String,
    /// Linear RGBA color, four finite components (same convention as `Mesh.color`).
    pub color: [f64; 4],
    /// HDR emissive intensity, an optional finite multiplier (`>= 0`) on the entry's own color —
    /// `None` means the quad takes the default material's emissive (`0.0`) at mesh time.
    pub emissive: Option<f64>,
    /// Optional metallic factor in `[0, 1]` (finite); `None` defers to the default material.
    pub metallic: Option<f64>,
    /// Optional perceptual roughness in `[0, 1]` (finite); `None` defers to the default material.
    pub roughness: Option<f64>,
}

/// One connected patch of X-mirror asymmetry: a 6-connected group of low-x cells whose glyph differs
/// from its mirror on the high-x side (`x' = width - 1 - x`). Reported by
/// [`VoxelDocument::mirror_x_regions`].
///
/// A region is the unit the question "is my asymmetry confined to the features I intended" is
/// answered in: two tight regions read as deliberate at a glance, forty scattered ones read as a bug.
/// A per-cell list answers neither, because one feature is thousands of cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MirrorRegion {
    /// Differing low-x cells in this region.
    pub cells: u64,
    /// Inclusive lower corner of the region's bounding box, in **low-half** coordinates
    /// (`x < width / 2` always, since a pair is reported from its low side).
    pub min: [u32; 3],
    /// Inclusive upper corner of the region's bounding box, in low-half coordinates.
    pub max: [u32; 3],
    /// Every glyph involved, sorted and deduped: both the low cell's glyph and its mirror's, with
    /// `.` (empty) included — "these cells are `G` against nothing" is the usual asymmetry.
    pub glyphs: Vec<char>,
}

/// One glyph's share of an [`ExposureReport`]: how many cells it fills, and how many of those are
/// fully enclosed — surrounded on all six faces, so no renderer will ever show them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphExposure {
    /// The palette glyph.
    pub glyph: char,
    /// Filled cells carrying this glyph.
    pub cells: u64,
    /// How many of [`Self::cells`] are fully enclosed.
    pub enclosed: u64,
    /// Up to five enclosed cells `[x, y, z]`, in (y, z, x) scan order — enough to jump to the
    /// problem without printing a wall of coordinates.
    pub enclosed_samples: Vec<[u32; 3]>,
    /// Enclosed cells beyond [`Self::enclosed_samples`] (`0` when every one is listed).
    pub enclosed_overflow: u64,
}

/// Which way one emissive glyph's visible faces point — the text answer to "is this lightbar
/// glowing where I meant it to?", which otherwise costs a GPU render.
#[derive(Clone, Debug, PartialEq)]
pub struct EmissiveFaces {
    /// The palette glyph.
    pub glyph: char,
    /// The glyph's authored `emissive` intensity (always `> 0` — a non-emissive glyph is absent).
    pub emissive: f64,
    /// Exposed-face counts in the fixed order of [`Self::FACE_LABELS`]. A *fixed* axis order, never
    /// sorted by count: reading two models against each other only works when the columns line up.
    pub faces: [u64; 6],
    /// The same faces judged against the gameplay camera, when the report was asked for under
    /// [`ExposureView::Game`]; `None` under [`ExposureView::Any`].
    pub game: Option<GameViewFaces>,
}

/// One emissive glyph's exposed faces measured against the gameplay camera: how many point at it at
/// all, and how many of those the model's own geometry hides.
///
/// "Exposed" only says light can leave the cell. A recessed panel can score perfectly on the
/// direction histogram and still render as two slivers, because at the camera's 58° pitch its own
/// lintel is in the way — the class of defect this pair of numbers catches as text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct GameViewFaces {
    /// Exposed faces whose outward normal points toward the camera (`dot(normal, to_camera) > 0`).
    /// The other exposed faces point away and could never be seen from this view regardless.
    pub facing: u64,
    /// How many of [`Self::facing`] are blocked by a filled cell of this same model, found by
    /// marching a ray from the face toward the camera until it leaves the grid.
    pub occluded: u64,
}

/// Which viewpoint an [`ExposureReport`]'s emissive faces are judged against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ExposureView {
    /// Count exposed faces by direction only — no viewpoint, no ray casting.
    #[default]
    Any,
    /// Additionally ray-test each emissive glyph's camera-facing faces against the gameplay camera,
    /// filling [`EmissiveFaces::game`].
    Game,
}

impl EmissiveFaces {
    /// The face directions of [`Self::faces`], in its order.
    pub const FACE_LABELS: [&'static str; 6] = ["+X", "-X", "+Y", "-Y", "+Z", "-Z"];

    /// Total exposed faces across all six directions.
    pub fn total(&self) -> u64 {
        self.faces.iter().sum()
    }
}

/// What of a voxel model is actually *visible*: the split between surface and fully-enclosed cells,
/// per glyph, plus where each emissive glyph's light escapes. Produced by
/// [`VoxelDocument::exposure`].
///
/// A model's interior is conventionally solid, so a large enclosed count on the structural glyph is
/// normal and says nothing. The signal is a *detail* or *emissive* glyph with enclosed cells: work
/// that was authored and can never be seen.
#[derive(Clone, Debug, PartialEq)]
pub struct ExposureReport {
    /// Filled cells in the model (matches [`VoxelDocument::filled_count`]).
    pub filled: u64,
    /// Filled cells with at least one exposed face.
    pub surface: u64,
    /// Filled cells with no exposed face at all (`filled == surface + enclosed`).
    pub enclosed: u64,
    /// One entry per glyph *present in the model*, sorted by glyph — a declared-but-unused palette
    /// entry has no row.
    pub glyphs: Vec<GlyphExposure>,
    /// One entry per emissive glyph present in the model, sorted by glyph.
    pub emissive: Vec<EmissiveFaces>,
}

/// A parsed, structurally valid voxel model document.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct VoxelDocument {
    /// Optional human-readable model name.
    pub name: Option<String>,
    /// The grid extent in cells: `[x, y, z]` (width, height, depth).
    pub size: [u32; 3],
    /// The model's *declared intended world scale* — metadata, never engine-applied.
    ///
    /// The document is authoritative as **intent** ("this model was drawn to stand 2.16 units
    /// tall"); a scene's `Transform.scale` is authoritative as **fact**. Placement code (an editor
    /// stage, a kit shelf) stamps this number into the Transform it authors; the mesher and the
    /// render path never read it, so `Transform.scale` keeps exactly one meaning everywhere.
    /// `None` means the model declares no intent. Finite and strictly positive when present
    /// (`L1114`); `f64` like every other float in this crate, so the canonical text is
    /// shortest-round-trip and the crate never depends on a GPU type.
    pub scale: Option<f64>,
    /// The generator script that owns this model, as a path relative to the model file — metadata,
    /// never engine-applied (the `scale` precedent).
    ///
    /// Its presence is the marker that the *script* is the source of truth and the file is
    /// write-only: regenerating must reproduce the committed bytes, the way a `generated_from:`
    /// scene relates to its `.map.yaml`. A model hand-sculpted after a one-shot scaffold carries no
    /// such key — the file is then authoritative. Non-empty when present (`L1113`); no existence
    /// check happens here, so this crate stays IO-free.
    pub generated_from: Option<String>,
    /// The explicit opt-out from the [`VOXEL_CELL_BUDGET`] style warning (`L1115`).
    ///
    /// `Some(true)` — the only valid authored value — declares that this model is deliberately over
    /// budget, and [`crate::voxel_budget_warning`] stays silent for it. `None` (the usual case)
    /// leaves the warning armed. `Some(false)` is an `L1114` error rather than a no-op: a key whose
    /// only meaning is "suppress" says nothing when set to `false`, and silently accepting it would
    /// invite the reading that the budget had been *asserted*.
    pub budget_exempt: Option<bool>,
    /// Palette entries, in sorted-glyph order (the canonical order [`crate::emit_voxel`] writes).
    pub palette: Vec<PaletteEntry>,
    /// Slice layers, bottom-up (`slices[y]`), each a sequence of row strings (`slices[y][z]`); the
    /// character at column `c` of a row is the cell at `x = c`.
    pub slices: Vec<Vec<String>>,
}

impl VoxelDocument {
    /// The total number of grid cells (`size.x * size.y * size.z`), computed in `u64` and
    /// saturating at `u64::MAX`, so a huge declared `size` stays over any cap instead of wrapping
    /// under it.
    pub fn cell_count(&self) -> u64 {
        u64::from(self.size[0])
            .saturating_mul(u64::from(self.size[1]))
            .saturating_mul(u64::from(self.size[2]))
    }

    /// The palette index of the cell at `(x, y, z)`, or `None` for an empty cell (`.`), an
    /// out-of-range coordinate, or a glyph that names no palette entry.
    ///
    /// Out-of-range reads as empty deliberately. Call on a [`crate::validate_voxel`]-clean document
    /// for a total, dimension-consistent grid.
    pub fn palette_index(&self, x: u32, y: u32, z: u32) -> Option<u8> {
        if x >= self.size[0] || y >= self.size[1] || z >= self.size[2] {
            return None;
        }
        let row = self.slices.get(y as usize)?.get(z as usize)?;
        let glyph = row.chars().nth(x as usize)?;
        if glyph == EMPTY_GLYPH {
            return None;
        }
        self.palette
            .iter()
            .position(|entry| single_glyph(&entry.glyph) == Some(glyph))
            .map(|index| index as u8)
    }

    /// Visits every in-bounds cell exactly once, in `(y, z, x)` scan order — the order the row
    /// strings are authored in — passing `(x, y, z)` and that cell's palette index (`None` for an
    /// empty cell, exactly as [`Self::palette_index`] answers).
    ///
    /// The whole-grid form of [`Self::palette_index`], and what any sweep over every cell must use:
    /// each row string is decoded once, left to right, through a glyph lookup built once from the
    /// palette, so a sweep costs O(cells). A per-cell `palette_index` loop costs O(cells × width)
    /// instead — its `chars().nth(x)` restarts the row at every column.
    ///
    /// # Example
    /// ```
    /// let source = "format: 1\nkind: voxel\nsize:\n  - 2\n  - 1\n  - 1\npalette:\n  A:\n    \
    ///               color:\n      - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\nslices:\n  -\n    \
    ///               - \"A.\"\n";
    /// let model = ludion_scene::check_voxel(source).expect("the model is valid");
    /// let mut cells = Vec::new();
    /// model.for_each_cell(|x, y, z, index| cells.push(([x, y, z], index)));
    /// assert_eq!(cells, [([0, 0, 0], Some(0)), ([1, 0, 0], None)]);
    /// ```
    pub fn for_each_cell(&self, mut visit: impl FnMut(u32, u32, u32, Option<u8>)) {
        let lookup = glyph_lookup(&self.palette);
        let [sx, sy, sz] = self.size;
        for y in 0..sy {
            for z in 0..sz {
                let row = self
                    .slices
                    .get(y as usize)
                    .and_then(|layer| layer.get(z as usize))
                    .map_or("", String::as_str);
                let mut glyphs = row.chars();
                for x in 0..sx {
                    // A missing layer, a missing row, or a row shorter than `size.x` reads as empty
                    // past its end — what `palette_index`'s `nth` answers on the same document.
                    let index = glyphs.next().and_then(|glyph| decode_glyph(&lookup, glyph));
                    visit(x, y, z, index);
                }
            }
        }
    }

    /// The number of non-empty (filled) cells — every in-bounds cell whose glyph names a palette
    /// entry.
    ///
    /// One [`Self::for_each_cell`] sweep: O(cells), with neither a per-cell row scan nor a per-glyph
    /// palette scan.
    pub fn filled_count(&self) -> u64 {
        let mut count = 0;
        self.for_each_cell(|_, _, _, index| {
            if index.is_some() {
                count += 1;
            }
        });
        count
    }

    /// The number of 6-connected components its filled cells form (two cells connect only across a
    /// shared face — never an edge or a corner). `0` for an empty model, `1` for a fully-connected
    /// one, more when a piece floats free of the main body. Call on a [`crate::validate_voxel`]-clean
    /// document (dimensions consistent with `size`).
    pub fn component_count(&self) -> u32 {
        filled_components(self).len() as u32
    }

    /// The component count ([`Self::component_count`]) and the connectivity warning
    /// ([`crate::voxel_connectivity_warning`]) together, from **one** grid decode and **one** flood
    /// fill — what a caller reporting both should ask for.
    ///
    /// Those two are the single-answer wrappers over the same sweep, so asking each in turn walks
    /// the whole grid twice; `ludion check-scene` prints both and calls this instead.
    ///
    /// # Example
    /// ```
    /// let source = "format: 1\nkind: voxel\nsize:\n  - 3\n  - 1\n  - 1\npalette:\n  A:\n    \
    ///               color:\n      - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\nslices:\n  -\n    \
    ///               - \"A.A\"\n";
    /// let model = ludion_scene::check_voxel(source).expect("the model is valid");
    /// let (components, warning) = model.connectivity();
    /// assert_eq!(components, 2, "the two filled cells do not touch");
    /// assert_eq!(warning.expect("L1129").code.as_str(), "L1129");
    /// ```
    pub fn connectivity(&self) -> (u32, Option<Diagnostic>) {
        let occupancy = Occupancy::of(self);
        let components = components_of(&occupancy);
        (components.len() as u32, warning_of(&occupancy, &components))
    }

    /// The raw authored glyph at `(x, y, z)`, or `.` (empty) for any out-of-range coordinate or a
    /// row shorter than `x + 1`. Unlike [`Self::palette_index`] this never consults the palette — it
    /// is a pure text read, for the mirror/slice/column authoring views.
    fn glyph_at(&self, x: u32, y: u32, z: u32) -> char {
        self.slices
            .get(y as usize)
            .and_then(|layer| layer.get(z as usize))
            .and_then(|row| row.chars().nth(x as usize))
            .unwrap_or(EMPTY_GLYPH)
    }

    /// Every X-mirror asymmetry, clustered into 6-connected regions: for each `(y, z)` and each
    /// low-x column `x < width / 2`, the cell whose glyph differs from its mirror at
    /// `(width - 1 - x, y, z)` is collected, and the differing cells are flood-filled into regions.
    ///
    /// An informational diff, not a lint — an asymmetric model (a censer in one hand) is legitimate.
    /// Each mirror pair is counted once, from the low-x side, so every reported coordinate is a
    /// low-half one; on an odd width the centre column (`x == width - 1 - x`) is self-mirroring and
    /// skipped (the `x < width / 2` bound excludes it). Regions come out **largest first** — the
    /// order a reader wants, and the reverse of the flood fill's own smallest-first order, which
    /// `L1129` depends on. An empty result means the model is X-symmetric.
    ///
    /// Reads through the same decode as every other whole-grid sweep, so a glyph naming no palette
    /// entry compares as empty; on a [`crate::validate_voxel`]-clean document (where every glyph is
    /// either `.` or a palette key) that is the authored character.
    pub fn mirror_x_regions(&self) -> Vec<MirrorRegion> {
        let occupancy = Occupancy::of(self);
        let [sx, sy, sz] = self.size;
        // The differing low-half cells as a grid of their own, so the shared flood fill clusters
        // them: the marker glyph only has to be "not empty" for `is_filled` to see it.
        let mut diff = Occupancy {
            sx: occupancy.sx,
            sy: occupancy.sy,
            sz: occupancy.sz,
            glyphs: vec![EMPTY_GLYPH; occupancy.glyphs.len()],
        };
        const DIFFERS: char = '#';
        for y in 0..sy as usize {
            for z in 0..sz as usize {
                for x in 0..(sx / 2) as usize {
                    let xm = (sx - 1) as usize - x;
                    if occupancy.glyphs[occupancy.index([x, y, z])]
                        != occupancy.glyphs[occupancy.index([xm, y, z])]
                    {
                        let index = diff.index([x, y, z]);
                        diff.glyphs[index] = DIFFERS;
                    }
                }
            }
        }
        let mut regions: Vec<MirrorRegion> = components_of(&diff)
            .into_iter()
            .map(|component| {
                let mut min = [u32::MAX; 3];
                let mut max = [0u32; 3];
                let mut glyphs: Vec<char> = Vec::new();
                for cell in &component.cells {
                    for axis in 0..3 {
                        min[axis] = min[axis].min(cell[axis]);
                        max[axis] = max[axis].max(cell[axis]);
                    }
                    let [x, y, z] = *cell;
                    let low =
                        occupancy.glyphs[occupancy.index([x as usize, y as usize, z as usize])];
                    let high = occupancy.glyphs
                        [occupancy.index([(sx - 1 - x) as usize, y as usize, z as usize])];
                    // A region's glyph set is a handful of characters, so a linear membership test
                    // beats any index — and keeps the result a plain sorted `Vec<char>`.
                    for glyph in [low, high] {
                        if !glyphs.contains(&glyph) {
                            glyphs.push(glyph);
                        }
                    }
                }
                glyphs.sort_unstable();
                MirrorRegion {
                    cells: component.cells.len() as u64,
                    min,
                    max,
                    glyphs,
                }
            })
            .collect();
        // Largest first, then by lower corner — a total order, so the table is deterministic.
        regions.sort_by(|a, b| b.cells.cmp(&a.cells).then(a.min.cmp(&b.min)));
        regions
    }

    /// Renders layer `y` as its authored rows annotated with a column ruler (a units line, plus a
    /// tens line when `width > 10`), a right-aligned `z` index down the left, and
    /// `z = 0 (front, world -Z)` / `z = <max> (back)` markers — so an author reads one slice without
    /// counting columns by hand. The marker names the *world* axis the authored front meshes at, not
    /// the authoring-space nickname: "front +Z" reads as a world axis and is 180° from the truth.
    ///
    /// # Errors
    /// `L1268`, naming the valid `y` range, when `y` is out of range.
    pub fn render_slice(&self, y: u32) -> Result<String, Diagnostic> {
        let [sx, sy, sz] = self.size;
        if y >= sy {
            return Err(Diagnostic::new(
                code::VOXEL_VIEW_OUT_OF_RANGE,
                format!("slice y={y} is out of range (this model's height is {sy})"),
            )
            .with_help(format!("valid y is 0..={}", sy.saturating_sub(1))));
        }
        let zlabel_w = ((sz.saturating_sub(1)).to_string().len()).max(1);
        let gutter = zlabel_w + 2;
        let pad = " ".repeat(gutter);

        let mut out = String::new();
        if sx > 10 {
            out.push_str(&pad);
            for c in 0..sx {
                if c >= 10 {
                    out.push(char::from_digit((c / 10) % 10, 10).unwrap_or('?'));
                } else {
                    out.push(' ');
                }
            }
            out.push('\n');
        }
        out.push_str(&pad);
        for c in 0..sx {
            out.push(char::from_digit(c % 10, 10).unwrap_or('?'));
        }
        out.push('\n');

        out.push_str(&pad);
        out.push_str("z=0 (front, world -Z)\n");
        for z in 0..sz {
            let row = self
                .slices
                .get(y as usize)
                .and_then(|layer| layer.get(z as usize))
                .cloned()
                .unwrap_or_default();
            out.push_str(&format!("{z:>zlabel_w$}  {row}\n"));
        }
        out.push_str(&pad);
        out.push_str(&format!("z={} (back)\n", sz.saturating_sub(1)));
        Ok(out)
    }

    /// Renders the vertical profile at column `(x, z)` bottom-up: one `y=N: 'G'` (or `y=N: .`) line
    /// per y level, then a compact `filled y:` run-length summary of the filled levels (e.g. `2-7,9`,
    /// or `none`) — so an author reasons about what stacks above/below a cell without holding every
    /// layer in mind.
    ///
    /// # Errors
    /// `L1268`, naming the valid `x`/`z` ranges, when either is out of range.
    pub fn render_column(&self, x: u32, z: u32) -> Result<String, Diagnostic> {
        let [sx, sy, sz] = self.size;
        if x >= sx || z >= sz {
            return Err(Diagnostic::new(
                code::VOXEL_VIEW_OUT_OF_RANGE,
                format!("column (x={x}, z={z}) is out of range (size {sx}x{sy}x{sz})"),
            )
            .with_help(format!(
                "valid x is 0..={}, z is 0..={}",
                sx.saturating_sub(1),
                sz.saturating_sub(1)
            )));
        }
        let mut out = format!("column (x={x}, z={z}):\n");
        for y in 0..sy {
            let glyph = self.glyph_at(x, y, z);
            if glyph == EMPTY_GLYPH {
                out.push_str(&format!("y={y}: .\n"));
            } else {
                out.push_str(&format!("y={y}: '{glyph}'\n"));
            }
        }
        let filled: Vec<u32> = (0..sy)
            .filter(|&y| self.glyph_at(x, y, z) != EMPTY_GLYPH)
            .collect();
        out.push_str(&format!("filled y: {}\n", run_length_ranges(&filled)));
        Ok(out)
    }

    /// What of this model can be seen: the surface/fully-enclosed split overall and per glyph, plus
    /// each emissive glyph's exposed faces by direction. See [`ExposureReport`].
    ///
    /// One 6-neighbour sweep over the occupancy grid. A face is exposed when its neighbour is empty
    /// or outside the grid (the model's own boundary is visible); a cell is fully enclosed when all
    /// six neighbours are filled. Call on a [`crate::validate_voxel`]-clean document.
    ///
    /// Under [`ExposureView::Game`] each emissive glyph additionally gets [`GameViewFaces`]: the
    /// exposed faces that point at the gameplay camera, and how many of those the model's own
    /// geometry hides. That costs one ray march per camera-facing emissive face and nothing at all
    /// for the rest of the model, so it stays opt-in rather than free.
    pub fn exposure(&self, view: ExposureView) -> ExposureReport {
        let occupancy = Occupancy::of(self);
        // Palette glyphs, sorted and deduped: the report's row order, and the accumulator index.
        let mut glyphs: Vec<char> = self
            .palette
            .iter()
            .filter_map(|entry| single_glyph(&entry.glyph))
            .collect();
        glyphs.sort_unstable();
        glyphs.dedup();
        let emissive: Vec<Option<f64>> = glyphs
            .iter()
            .map(|&glyph| {
                self.palette
                    .iter()
                    .find(|entry| single_glyph(&entry.glyph) == Some(glyph))
                    .and_then(|entry| entry.emissive)
                    .filter(|intensity| *intensity > 0.0)
            })
            .collect();

        let mut cells = vec![0u64; glyphs.len()];
        let mut enclosed = vec![0u64; glyphs.len()];
        let mut samples: Vec<Vec<[u32; 3]>> = vec![Vec::new(); glyphs.len()];
        let mut faces = vec![[0u64; 6]; glyphs.len()];
        let mut game = vec![GameViewFaces::default(); glyphs.len()];
        let (mut filled, mut surface) = (0u64, 0u64);

        // (y, z, x) scan order, so each glyph's enclosed samples come out in it for free.
        for y in 0..occupancy.sy {
            for z in 0..occupancy.sz {
                for x in 0..occupancy.sx {
                    let cell = [x, y, z];
                    let glyph = occupancy.glyphs[occupancy.index(cell)];
                    let Some(slot) = glyphs.iter().position(|&known| known == glyph) else {
                        continue;
                    };
                    filled += 1;
                    cells[slot] += 1;
                    let mut exposed = 0u32;
                    for (face, step) in NEIGHBORS.into_iter().enumerate() {
                        let open = match occupancy.step(cell, step) {
                            Some(neighbor) => !occupancy.is_filled(neighbor),
                            None => true,
                        };
                        if open {
                            exposed += 1;
                            if emissive[slot].is_some() {
                                faces[slot][face] += 1;
                                if view == ExposureView::Game {
                                    tally_game_view(&occupancy, cell, face, &mut game[slot]);
                                }
                            }
                        }
                    }
                    if exposed == 0 {
                        enclosed[slot] += 1;
                        if samples[slot].len() < MAX_ENCLOSED_SAMPLES {
                            samples[slot].push([x as u32, y as u32, z as u32]);
                        }
                    } else {
                        surface += 1;
                    }
                }
            }
        }

        let rows: Vec<GlyphExposure> = (0..glyphs.len())
            .filter(|&slot| cells[slot] > 0)
            .map(|slot| GlyphExposure {
                glyph: glyphs[slot],
                cells: cells[slot],
                enclosed: enclosed[slot],
                enclosed_overflow: enclosed[slot] - samples[slot].len() as u64,
                enclosed_samples: std::mem::take(&mut samples[slot]),
            })
            .collect();
        let lights: Vec<EmissiveFaces> = (0..glyphs.len())
            .filter(|&slot| cells[slot] > 0)
            .filter_map(|slot| {
                emissive[slot].map(|intensity| EmissiveFaces {
                    glyph: glyphs[slot],
                    emissive: intensity,
                    faces: faces[slot],
                    game: (view == ExposureView::Game).then_some(game[slot]),
                })
            })
            .collect();
        ExposureReport {
            filled,
            surface,
            enclosed: filled - surface,
            glyphs: rows,
            emissive: lights,
        }
    }
}

/// How many enclosed cells [`GlyphExposure::enclosed_samples`] lists before it starts counting.
const MAX_ENCLOSED_SAMPLES: usize = 5;

/// The gameplay camera's pitch, in degrees — reported so a caller can name the view it measured.
pub const GAME_VIEW_PITCH_DEG: f64 = 58.0;

/// The unit vector **from the model toward the gameplay camera**, in authored grid space.
///
/// Source of truth: `ModelView::Game` in `crates/ludion-engine/src/model_sheet.rs` — the vigil
/// district camera, pitched [`GAME_VIEW_PITCH_DEG`] down with its horizontal aim at
/// `normalize(0.45, 0, -1)` (the authored front, with a slight `+X` three-quarter). Duplicated here
/// rather than imported because `ludion-scene` must never depend on `ludion-engine` (crate graph);
/// the two must be changed together.
///
/// **No axis flip is needed.** `ludion_engine::voxel_mesh` maps grid space to model space by
/// translation alone (bottom-center origin), so grid `+X/+Y/+Z` *are* world `+X/+Y/+Z`; the authored
/// front (`z = 0`) meshes at world −Z because it is the lowest `z`, not because the axis is
/// mirrored. A ray marched in grid coordinates is therefore already in world direction.
///
/// Written as literals rather than computed: the direction is compile-time known, and a constant
/// keeps this sweep free of runtime transcendentals. Derivation —
/// `normalize(0.45, 0, -1) * cos(58°) + (0, 1, 0) * sin(58°)`.
const GAME_VIEW_TO_CAMERA: [f64; 3] = [
    0.217_460_147_877_373_1,
    0.848_048_096_156_426_1,
    -0.483_244_773_060_829_1,
];

/// How far off the surface a ray starts, in cells: enough to land inside the (empty) neighbour cell
/// rather than exactly on the shared face, where the grid walk's cell would be a coin toss.
const RAY_NUDGE: f64 = 0.5 + 1e-6;

/// Judges one exposed face of `cell` against the gameplay camera and folds the answer into `tally`:
/// whether it points at the camera at all, and — if it does — whether the model's own geometry
/// stands between it and the camera.
fn tally_game_view(
    occupancy: &Occupancy,
    cell: [usize; 3],
    face: usize,
    tally: &mut GameViewFaces,
) {
    let step = NEIGHBORS[face];
    let normal = [step[0] as f64, step[1] as f64, step[2] as f64];
    let toward = GAME_VIEW_TO_CAMERA;
    let facing = normal[0] * toward[0] + normal[1] * toward[1] + normal[2] * toward[2];
    if facing <= 0.0 {
        // The face points away from the camera; no ray can change that.
        return;
    }
    tally.facing += 1;
    let origin = [
        cell[0] as f64 + 0.5 + normal[0] * RAY_NUDGE,
        cell[1] as f64 + 0.5 + normal[1] * RAY_NUDGE,
        cell[2] as f64 + 0.5 + normal[2] * RAY_NUDGE,
    ];
    if ray_hits_filled(occupancy, origin, toward) {
        tally.occluded += 1;
    }
}

/// Marches a ray from `origin` (in cell units, so cell `(x, y, z)` spans `[x, x+1)` on each axis)
/// along the unit `direction` and reports whether it enters a filled cell before leaving the grid.
///
/// Amanatides–Woo: track, per axis, the ray parameter at which the next cell boundary is crossed
/// (`t_max`) and the parameter cost of one whole cell (`t_delta`); each iteration steps the axis
/// whose boundary comes first. Exact integer cell indices throughout — the floats only order the
/// crossings.
///
/// A ray starting outside the grid never enters it here, and does not need to: every caller starts
/// on a face that points toward the camera, so an origin already past a grid boundary is one whose
/// ray only travels further out.
fn ray_hits_filled(occupancy: &Occupancy, origin: [f64; 3], direction: [f64; 3]) -> bool {
    let size = [
        occupancy.sx as i64,
        occupancy.sy as i64,
        occupancy.sz as i64,
    ];
    let mut cell = [0i64; 3];
    let mut step = [0i64; 3];
    let mut t_max = [f64::INFINITY; 3];
    let mut t_delta = [f64::INFINITY; 3];
    for axis in 0..3 {
        cell[axis] = origin[axis].floor() as i64;
        if cell[axis] < 0 || cell[axis] >= size[axis] {
            return false;
        }
        if direction[axis] > 0.0 {
            step[axis] = 1;
            t_max[axis] = ((cell[axis] + 1) as f64 - origin[axis]) / direction[axis];
            t_delta[axis] = 1.0 / direction[axis];
        } else if direction[axis] < 0.0 {
            step[axis] = -1;
            t_max[axis] = (cell[axis] as f64 - origin[axis]) / direction[axis];
            t_delta[axis] = -1.0 / direction[axis];
        }
    }
    loop {
        // The axis whose next boundary is nearest; ties break toward the lowest axis, which only
        // reorders two steps that both happen.
        let axis = if t_max[0] <= t_max[1] && t_max[0] <= t_max[2] {
            0
        } else if t_max[1] <= t_max[2] {
            1
        } else {
            2
        };
        if step[axis] == 0 {
            // Every remaining axis is parallel to a face the ray never crosses: it leaves through
            // whichever boundary it was already heading for, and it is not heading for any.
            return false;
        }
        cell[axis] += step[axis];
        if cell[axis] < 0 || cell[axis] >= size[axis] {
            return false;
        }
        t_max[axis] += t_delta[axis];
        if occupancy.is_filled([cell[0] as usize, cell[1] as usize, cell[2] as usize]) {
            return true;
        }
    }
}

/// Collapses an ascending list of integers into comma-joined run-length ranges (`[2,3,4,5,6,7,9]` →
/// `"2-7,9"`), or `"none"` when empty. Used by [`VoxelDocument::render_column`].
fn run_length_ranges(values: &[u32]) -> String {
    let Some(&first) = values.first() else {
        return "none".to_string();
    };
    let mut parts = Vec::new();
    let mut start = first;
    let mut prev = first;
    for &value in &values[1..] {
        if value == prev + 1 {
            prev = value;
        } else {
            parts.push(fmt_range(start, prev));
            start = value;
            prev = value;
        }
    }
    parts.push(fmt_range(start, prev));
    parts.join(",")
}

fn fmt_range(a: u32, b: u32) -> String {
    if a == b {
        format!("{a}")
    } else {
        format!("{a}-{b}")
    }
}

/// The single glyph character of a palette key, or `None` if the key is not exactly one character.
pub(crate) fn single_glyph(key: &str) -> Option<char> {
    let mut chars = key.chars();
    let ch = chars.next()?;
    chars.next().is_none().then_some(ch)
}

/// Whether `ch` is a printable ASCII character usable as a palette glyph (`0x21..=0x7E`).
fn is_palette_glyph(ch: char) -> bool {
    ('\u{21}'..='\u{7e}').contains(&ch)
}

/// Slots in a glyph lookup: one per ASCII code point, which covers every character the format
/// admits as a glyph (`0x21..=0x7E`).
const GLYPH_SLOTS: usize = 128;

/// The glyph → palette-index lookup for `palette`: slot `ch as usize` holds the index of the first
/// entry keyed by exactly `ch`, and `None` wherever no entry claims the slot.
///
/// This array is what keeps a whole-grid sweep linear — matching each glyph by scanning the palette
/// instead costs O(cells × palette). It is deliberately an array and not a map: the glyph domain is
/// 128 values wide, so the lookup is one indexed read per cell, and this crate's determinism
/// denylist bans a std hash map anyway.
///
/// [`EMPTY_GLYPH`] claims no slot (it is reserved, and `validate` rejects it as a palette key), and
/// a non-ASCII key claims none either — both decode as empty. On a [`validate`]-clean document every
/// key is a printable-ASCII glyph, so decoding through this array answers exactly as
/// [`VoxelDocument::palette_index`] does, cell for cell.
fn glyph_lookup(palette: &[PaletteEntry]) -> [Option<u8>; GLYPH_SLOTS] {
    let mut lookup = [None; GLYPH_SLOTS];
    for (index, entry) in palette.iter().enumerate() {
        let Some(glyph) = single_glyph(&entry.glyph) else {
            continue;
        };
        let slot = glyph as usize;
        if glyph == EMPTY_GLYPH || slot >= GLYPH_SLOTS {
            continue;
        }
        // The first entry claiming a glyph wins, as `palette_index`'s `position` scan does on a
        // duplicate the palette should not have.
        if lookup[slot].is_none() {
            lookup[slot] = Some(index as u8);
        }
    }
    lookup
}

/// The palette index `glyph` decodes to under `lookup`, or `None` for an empty cell, a glyph no
/// palette entry claims, or a non-ASCII character (out of the lookup's domain).
fn decode_glyph(lookup: &[Option<u8>; GLYPH_SLOTS], glyph: char) -> Option<u8> {
    *lookup.get(glyph as usize)?
}

/// Parses strict-subset YAML into a [`VoxelDocument`], checking structure/shape only (`L1113`).
pub(crate) fn parse(source: &str) -> Result<VoxelDocument, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![
            Diagnostic::new(
                code::VOXEL_DOCUMENT_STRUCTURE,
                "the voxel document is empty; a voxel model needs at least `format: 1`, \
                 `kind: voxel`, `size`, `palette`, and `slices`",
            )
            .with_path("")
            .with_help(
                "a minimal voxel model is:\n  format: 1\n  kind: voxel\n  size:\n    - 1\n    \
                 - 1\n    - 1\n  palette:\n    A:\n      color:\n        - 1.0\n        - 1.0\n\
                 \x20       - 1.0\n        - 1.0\n  slices:\n    -\n      - \"A\"",
            ),
        ]);
    };
    let NodeValue::Mapping(entries) = root.value else {
        return Err(vec![
            Diagnostic::new(
                code::VOXEL_DOCUMENT_STRUCTURE,
                format!(
                    "the voxel document root must be a mapping, found {} (line {})",
                    root.value.type_phrase(),
                    root.line
                ),
            )
            .with_path("")
            .with_help("start the file with the top-level keys `format:` and `kind: voxel`"),
        ]);
    };

    let mut diagnostics = Vec::new();
    let mut doc = VoxelDocument::default();
    let mut format_seen = false;
    let mut kind_seen = false;
    let mut size_seen = false;
    let mut palette_seen = false;
    let mut slices_seen = false;

    for entry in &entries {
        match entry.key.as_str() {
            "format" => {
                format_seen = true;
                envelope::check_envelope(
                    entry,
                    code::VOXEL_DOCUMENT_STRUCTURE,
                    code::VOXEL_DOCUMENT_STRUCTURE,
                    VOXEL_FORMAT_VERSION,
                    "the voxel document",
                    "model",
                    "voxel",
                    &mut diagnostics,
                );
            }
            "kind" => {
                kind_seen = true;
                envelope::check_kind(
                    entry,
                    code::VOXEL_DOCUMENT_STRUCTURE,
                    "voxel",
                    "a voxel model document",
                    &mut diagnostics,
                );
            }
            "name" => {
                doc.name = envelope::expect_string(
                    &entry.node,
                    "/name",
                    "the voxel model name",
                    code::VOXEL_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "size" => {
                size_seen = true;
                if let Some(size) = crate::fields::parse_size3(
                    entry,
                    code::VOXEL_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                ) {
                    doc.size = size;
                }
            }
            "scale" => {
                doc.scale = parse_scale(entry, &mut diagnostics);
            }
            "generated_from" => {
                doc.generated_from = parse_generated_from(entry, &mut diagnostics);
            }
            "budget_exempt" => {
                doc.budget_exempt = parse_budget_exempt(entry, &mut diagnostics);
            }
            "palette" => {
                palette_seen = true;
                doc.palette =
                    parse_palette(entry, code::VOXEL_DOCUMENT_STRUCTURE, &mut diagnostics);
            }
            "slices" => {
                slices_seen = true;
                doc.slices = parse_slices(entry, &mut diagnostics);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "",
                "a voxel document",
                VOXEL_KEYS,
                code::VOXEL_DOCUMENT_STRUCTURE,
            )),
        }
    }

    if !format_seen {
        diagnostics.push(missing_key("format", "add `format: 1` as the first line"));
    }
    if !kind_seen {
        diagnostics.push(missing_key("kind", "add `kind: voxel`"));
    }
    if !size_seen {
        diagnostics.push(missing_key(
            "size",
            "add a `size` of three positive integers: width (x), height (y), depth (z)",
        ));
    }
    if !palette_seen {
        diagnostics.push(missing_key(
            "palette",
            "add a `palette` mapping each glyph to a `color`",
        ));
    }
    if !slices_seen {
        diagnostics.push(missing_key(
            "slices",
            "add `slices`: one row-string layer per y level, bottom-up",
        ));
    }

    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Parses `scale`: the declared intended world scale, a single number. Finiteness and positivity
/// are `L1114` data-consistency concerns, checked later by [`validate`].
fn parse_scale(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Option<f64> {
    match &entry.node.value {
        NodeValue::Scalar(Value::Number(n)) => Some(crate::fields::number(n)),
        other => {
            diagnostics.push(
                Diagnostic::new(
                    code::VOXEL_DOCUMENT_STRUCTURE,
                    format!(
                        "`scale` must be a number, found {} (line {})",
                        other.type_phrase(),
                        entry.node.line
                    ),
                )
                .with_path("/scale")
                .with_help(
                    "write `scale` as one positive number — the world scale this model was drawn \
                     for, e.g. `scale: 0.24`",
                ),
            );
            None
        }
    }
}

/// Parses `generated_from`: the owning generator script's path, a non-empty string. Emptiness is
/// caught here rather than in [`validate`] because an empty path names no file at all — there is no
/// consistency question left to ask about it.
fn parse_generated_from(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Option<String> {
    let value = envelope::expect_string(
        &entry.node,
        "/generated_from",
        "the generator script path",
        code::VOXEL_DOCUMENT_STRUCTURE,
        diagnostics,
    )?;
    if value.is_empty() {
        diagnostics.push(
            Diagnostic::new(
                code::VOXEL_DOCUMENT_STRUCTURE,
                format!(
                    "`generated_from` is empty; it must name the generator script that owns this \
                     model (line {})",
                    entry.node.line
                ),
            )
            .with_path("/generated_from")
            .with_help(
                "write the script's path relative to this file, e.g. \
                 `generated_from: brazier.voxel.gen.mjs`; remove the key if the file itself is the \
                 source of truth",
            ),
        );
        return None;
    }
    Some(value)
}

/// Parses `budget_exempt`: a boolean. `false` is a `L1114` data-consistency concern (checked later
/// by [`validate`]), not a structure error — the shape is right, the value is not meaningful.
fn parse_budget_exempt(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Option<bool> {
    match &entry.node.value {
        NodeValue::Scalar(Value::Bool(value)) => Some(*value),
        other => {
            diagnostics.push(
                Diagnostic::new(
                    code::VOXEL_DOCUMENT_STRUCTURE,
                    format!(
                        "`budget_exempt` must be a boolean, found {} (line {})",
                        other.type_phrase(),
                        entry.node.line
                    ),
                )
                .with_path("/budget_exempt")
                .with_help(
                    "write `budget_exempt: true` to declare this model deliberately over the cell \
                     budget, or remove the key",
                ),
            );
            None
        }
    }
}

/// Parses `palette`: a mapping of glyph keys (raw, validated later by [`validate`]) to
/// `{color: [r, g, b, a]}` entries. Returns the entries in sorted-glyph order.
///
/// `structure_code` tags every shape/type diagnostic this parse raises, so the
/// [`crate::assembly`] family — whose palette is *exactly* this one — reports under its own code
/// while the traversal that produces the diagnostic stays single-sourced here (the
/// [`crate::envelope`] pattern). The pointer prefix is `/palette` for both families.
pub(crate) fn parse_palette(
    entry: &MapEntry,
    structure_code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<PaletteEntry> {
    let NodeValue::Mapping(entries) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                structure_code,
                format!(
                    "`palette` must be a mapping of glyphs to colors, found {} (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/palette")
            .with_help(
                "write `palette` as glyph keys, each mapping to a `color`:\n  palette:\n    A:\n\
                 \x20     color:\n        - 1.0\n        - 0.0\n        - 0.0\n        - 1.0",
            ),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(entries.len());
    for sub in entries {
        let pointer = format!("/palette/{}", pointer_token(&sub.key));
        if let Some(entry) =
            parse_palette_entry(&sub.node, &pointer, &sub.key, structure_code, diagnostics)
        {
            out.push(entry);
        }
    }
    out.sort_by(|a, b| a.glyph.cmp(&b.glyph));
    out
}

/// The palette-entry keys this format recognizes, in the order suggestions should prefer.
const PALETTE_ENTRY_KEYS: [&str; 4] = ["color", "emissive", "metallic", "roughness"];

/// Parses one palette entry — a mapping with a required `color` and optional `emissive`,
/// `metallic`, `roughness` material scalars.
fn parse_palette_entry(
    node: &Node,
    pointer: &str,
    glyph: &str,
    structure_code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<PaletteEntry> {
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                structure_code,
                format!(
                    "a palette entry must be a mapping with a `color`, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(
                "give the glyph a color:\n      color:\n        - 1.0\n        - 1.0\n        \
                 - 1.0\n        - 1.0",
            ),
        );
        return None;
    };
    let mut color = None;
    let mut color_seen = false;
    let mut emissive = None;
    let mut metallic = None;
    let mut roughness = None;
    for sub in entries {
        match sub.key.as_str() {
            "color" => {
                color_seen = true;
                color = parse_color(
                    &sub.node,
                    &format!("{pointer}/color"),
                    structure_code,
                    diagnostics,
                );
            }
            "emissive" => {
                emissive = parse_material_scalar(
                    &sub.node,
                    &format!("{pointer}/emissive"),
                    "emissive",
                    structure_code,
                    diagnostics,
                );
            }
            "metallic" => {
                metallic = parse_material_scalar(
                    &sub.node,
                    &format!("{pointer}/metallic"),
                    "metallic",
                    structure_code,
                    diagnostics,
                );
            }
            "roughness" => {
                roughness = parse_material_scalar(
                    &sub.node,
                    &format!("{pointer}/roughness"),
                    "roughness",
                    structure_code,
                    diagnostics,
                );
            }
            unknown => {
                let mut diagnostic = Diagnostic::new(
                    structure_code,
                    format!(
                        "a palette entry does not have a key named `{unknown}` (line {})",
                        sub.key_line
                    ),
                )
                .with_path(format!("{pointer}/{}", pointer_token(unknown)));
                diagnostic = match suggest::nearest(unknown, PALETTE_ENTRY_KEYS) {
                    Some(candidate) => {
                        diagnostic.with_help(format!("nearest match is `{candidate}`"))
                    }
                    None => diagnostic.with_help(format!(
                        "the palette-entry keys are: {}",
                        PALETTE_ENTRY_KEYS.join(", ")
                    )),
                };
                diagnostics.push(diagnostic);
            }
        }
    }
    if !color_seen {
        diagnostics.push(
            Diagnostic::new(
                structure_code,
                "a palette entry is missing its required `color`",
            )
            .with_path(pointer.to_string())
            .with_help("add a `color` with four numbers: red, green, blue, alpha"),
        );
    }
    color.map(|color| PaletteEntry {
        glyph: glyph.to_string(),
        color,
        emissive,
        metallic,
        roughness,
    })
}

/// Parses a palette-entry material scalar (`emissive`, `metallic`, `roughness`): any JSON number.
/// Finiteness and range are `L1114` data-consistency concerns, checked later by [`validate`].
fn parse_material_scalar(
    node: &Node,
    pointer: &str,
    what: &str,
    structure_code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<f64> {
    match &node.value {
        NodeValue::Scalar(Value::Number(n)) => Some(crate::fields::number(n)),
        other => {
            diagnostics.push(
                Diagnostic::new(
                    structure_code,
                    format!(
                        "`{what}` must be a number, found {} (line {})",
                        other.type_phrase(),
                        node.line
                    ),
                )
                .with_path(pointer.to_string())
                .with_help(format!(
                    "write `{what}` as a single number, e.g. `{what}: 0.5`"
                )),
            );
            None
        }
    }
}

/// Parses a `color`: a sequence of exactly four numbers.
fn parse_color(
    node: &Node,
    pointer: &str,
    structure_code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<[f64; 4]> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                structure_code,
                format!(
                    "`color` must be a sequence of four numbers, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("write four numbers (red, green, blue, alpha), one per line"),
        );
        return None;
    };
    if items.len() != 4 {
        diagnostics.push(
            Diagnostic::new(
                structure_code,
                format!(
                    "`color` must have exactly 4 components (red, green, blue, alpha), found {}",
                    items.len()
                ),
            )
            .with_path(pointer.to_string())
            .with_help("supply exactly four numbers"),
        );
        return None;
    }
    let mut out = [0.0f64; 4];
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match &item.value {
            NodeValue::Scalar(Value::Number(n)) => out[index] = crate::fields::number(n),
            other => {
                diagnostics.push(
                    Diagnostic::new(
                        structure_code,
                        format!(
                            "`color[{index}]` must be a number, found {} (line {})",
                            other.type_phrase(),
                            item.line
                        ),
                    )
                    .with_path(format!("{pointer}/{index}")),
                );
                ok = false;
            }
        }
    }
    ok.then_some(out)
}

/// Parses `slices`: a sequence of layers, each a sequence of row strings.
fn parse_slices(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Vec<Vec<String>> {
    let NodeValue::Sequence(layers) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::VOXEL_DOCUMENT_STRUCTURE,
                format!(
                    "`slices` must be a sequence of layers, found {} (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/slices")
            .with_help(
                "write `slices` as one layer per y level, each a sequence of row strings:\n  \
                 slices:\n    -\n      - \"AA\"\n      - \"AA\"",
            ),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(layers.len());
    for (y, layer) in layers.iter().enumerate() {
        let NodeValue::Sequence(rows) = &layer.value else {
            diagnostics.push(
                Diagnostic::new(
                    code::VOXEL_DOCUMENT_STRUCTURE,
                    format!(
                        "slice layer {y} must be a sequence of row strings, found {} (line {})",
                        layer.value.type_phrase(),
                        layer.line
                    ),
                )
                .with_path(format!("/slices/{y}"))
                .with_help("each layer is one row string per z, top-to-bottom in the file"),
            );
            out.push(Vec::new());
            continue;
        };
        let mut row_strings = Vec::with_capacity(rows.len());
        for (z, row) in rows.iter().enumerate() {
            match &row.value {
                NodeValue::Scalar(Value::String(s)) => row_strings.push(s.clone()),
                other => diagnostics.push(
                    Diagnostic::new(
                        code::VOXEL_DOCUMENT_STRUCTURE,
                        format!(
                            "slice row (layer {y}, row {z}) must be a string, found {} (line {})",
                            other.type_phrase(),
                            row.line
                        ),
                    )
                    .with_path(format!("/slices/{y}/{z}"))
                    .with_help(
                        "quote the row so a numeric- or boolean-looking line stays a string",
                    ),
                ),
            }
        }
        out.push(row_strings);
    }
    out
}

fn missing_key(key: &str, help: &str) -> Diagnostic {
    envelope::missing_key(key, help, code::VOXEL_DOCUMENT_STRUCTURE, "voxel document")
}

/// Validates data-consistency invariants of an already-parsed [`VoxelDocument`] (`L1114`): palette
/// keys are single printable-ASCII glyphs and unique, colors are finite, authored material scalars
/// (`emissive`, `metallic`, `roughness`) are finite and in range, the slice layers/rows/columns
/// match `size`, and every slice glyph is `.` or a palette entry.
///
/// Independent of how the document was built — a hand-authored one via [`parse`], or one assembled
/// programmatically — so this is the one place those invariants are enforced.
pub(crate) fn validate(doc: &VoxelDocument) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    // The declared intended world scale: metadata, but a zero/negative/non-finite one is a typo,
    // and every reader of it (placement code) would produce a degenerate transform.
    if let Some(scale) = doc.scale
        && !(scale.is_finite() && scale > 0.0)
    {
        diagnostics.push(
            Diagnostic::new(
                code::VOXEL_DATA_CONSISTENCY,
                format!("`scale` is {scale}; a model's declared scale must be finite and greater than zero"),
            )
            .with_path("/scale")
            .with_help(
                "`scale` records the world scale this model was drawn for (grid height x scale = \
                 world height), so it must be a positive number, e.g. `scale: 0.24`; remove the \
                 key to declare no intended scale",
            ),
        );
    }

    // `budget_exempt` exists only to suppress `L1115`; `false` suppresses nothing, so it is a
    // no-op key that reads like an assertion the budget was honoured. Say so instead.
    if doc.budget_exempt == Some(false) {
        diagnostics.push(
            Diagnostic::new(
                code::VOXEL_DATA_CONSISTENCY,
                "`budget_exempt` is `false`; the only meaningful value is `true` (the key exists \
                 solely to opt out of the cell-budget warning)",
            )
            .with_path("/budget_exempt")
            .with_help(
                "remove the `budget_exempt` key — its absence already means the budget warning \
                 applies",
            ),
        );
    }

    let known = validate_palette(
        &doc.palette,
        code::VOXEL_DATA_CONSISTENCY,
        "voxel",
        &mut diagnostics,
    );

    // Slice dimensions and glyphs against `size`. `known` is re-shaped into an ASCII membership
    // array first: the slice scan tests every character of every row, and a linear `Vec` scan there
    // is O(cells × palette). Every accepted glyph is printable ASCII (the arms above), so a
    // character outside the array's domain is by construction not in `known`.
    let mut accepted = [false; GLYPH_SLOTS];
    for &glyph in &known {
        accepted[glyph as usize] = true;
    }
    let is_known = |ch: char| accepted.get(ch as usize).copied().unwrap_or(false);
    let [sx, sy, sz] = doc.size;
    if doc.slices.len() as u64 != u64::from(sy) {
        diagnostics.push(
            Diagnostic::new(
                code::VOXEL_DATA_CONSISTENCY,
                format!(
                    "`slices` has {} layers but `size` declares height y = {sy}",
                    doc.slices.len()
                ),
            )
            .with_path("/slices")
            .with_help("supply exactly one slice layer per y level, bottom-up (y = 0 first)"),
        );
    }
    for (y, layer) in doc.slices.iter().enumerate() {
        if layer.len() as u64 != u64::from(sz) {
            diagnostics.push(
                Diagnostic::new(
                    code::VOXEL_DATA_CONSISTENCY,
                    format!(
                        "slice layer y = {y} has {} rows but `size` declares depth z = {sz}",
                        layer.len()
                    ),
                )
                .with_path(format!("/slices/{y}"))
                .with_help("each layer needs exactly one row per z (row r maps to z = r)"),
            );
        }
        for (z, row) in layer.iter().enumerate() {
            let width = row.chars().count();
            if width as u64 != u64::from(sx) {
                diagnostics.push(
                    Diagnostic::new(
                        code::VOXEL_DATA_CONSISTENCY,
                        format!(
                            "row z = {z} in layer y = {y} is {width} characters wide but `size` \
                             declares width x = {sx}"
                        ),
                    )
                    .with_path(format!("/slices/{y}/{z}"))
                    .with_help(
                        "every row must be exactly `size.x` characters (column c maps to x = c); \
                         pad empty cells with `.`",
                    ),
                );
            }
            let mut reported: Vec<char> = Vec::new();
            for ch in row.chars() {
                if ch != EMPTY_GLYPH && !is_known(ch) && !reported.contains(&ch) {
                    reported.push(ch);
                    diagnostics.push(
                        Diagnostic::new(
                            code::VOXEL_DATA_CONSISTENCY,
                            format!(
                                "the glyph `{ch}` in layer y = {y}, row z = {z} names no palette \
                                 entry"
                            ),
                        )
                        .with_path(format!("/slices/{y}/{z}"))
                        .with_help(format!(
                            "add `{ch}` to the palette, or use `.` for an empty cell"
                        )),
                    );
                }
            }
        }
    }

    diagnostics
}

/// Validates a palette's keys, colors, and material scalars, returning every accepted glyph once
/// (the membership set a slice scan tests against).
///
/// The value half of the palette contract, shared with the [`crate::assembly`] family whose palette
/// is exactly this one: `data_code` tags every diagnostic (`L1114` for a voxel document, `L1138`
/// for an assembly) and `glyph_noun` names the family in the printable-ASCII help, so each family
/// speaks its own vocabulary while the checks stay single-sourced. Pointers are `/palette/<glyph>`
/// in both.
pub(crate) fn validate_palette(
    palette: &[PaletteEntry],
    data_code: DiagnosticCode,
    glyph_noun: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<char> {
    let mut known: Vec<char> = Vec::new();
    for entry in palette {
        let pointer = format!("/palette/{}", pointer_token(&entry.glyph));
        match single_glyph(&entry.glyph) {
            None => diagnostics.push(
                Diagnostic::new(
                    data_code,
                    format!(
                        "the palette key `{}` is {} characters long; a palette glyph must be \
                         exactly one character",
                        entry.glyph,
                        entry.glyph.chars().count()
                    ),
                )
                .with_path(pointer.clone())
                .with_help("use a single printable-ASCII glyph, e.g. `A`, `#`, or `1`"),
            ),
            Some(EMPTY_GLYPH) => diagnostics.push(
                Diagnostic::new(
                    data_code,
                    "the glyph `.` is reserved for empty cells and cannot name a palette entry",
                )
                .with_path(pointer.clone())
                .with_help(
                    "choose any other printable-ASCII glyph; `.` always means an empty cell",
                ),
            ),
            Some(ch) if !is_palette_glyph(ch) => diagnostics.push(
                Diagnostic::new(
                    data_code,
                    format!(
                        "the palette glyph `{ch}` is not a printable ASCII character (0x21..=0x7E)"
                    ),
                )
                .with_path(pointer.clone())
                .with_help(format!(
                    "{glyph_noun} glyphs must be printable ASCII so columns stay aligned; pick a \
                     character from `!` to `~`"
                )),
            ),
            Some(ch) if known.contains(&ch) => diagnostics.push(
                Diagnostic::new(
                    data_code,
                    format!("the palette glyph `{ch}` is defined more than once"),
                )
                .with_path(pointer.clone())
                .with_help("each glyph may appear once; remove or rename the duplicate entry"),
            ),
            Some(ch) => known.push(ch),
        }

        for (index, &component) in entry.color.iter().enumerate() {
            if !component.is_finite() {
                diagnostics.push(
                    Diagnostic::new(
                        data_code,
                        format!(
                            "palette glyph `{}` has a non-finite color component `{component}` at \
                             index {index}",
                            entry.glyph
                        ),
                    )
                    .with_path(format!("{pointer}/color/{index}"))
                    .with_help("color components must be finite numbers (typically 0.0..=1.0)"),
                );
            }
        }

        validate_material_scalar(
            entry.emissive,
            "emissive",
            &entry.glyph,
            &pointer,
            0.0,
            f64::from(f32::MAX),
            data_code,
            diagnostics,
        );
        validate_material_scalar(
            entry.metallic,
            "metallic",
            &entry.glyph,
            &pointer,
            0.0,
            1.0,
            data_code,
            diagnostics,
        );
        validate_material_scalar(
            entry.roughness,
            "roughness",
            &entry.glyph,
            &pointer,
            0.0,
            1.0,
            data_code,
            diagnostics,
        );
    }
    known
}

/// Validates one optional palette-entry material scalar (`L1114`/`L1138`): non-finite is always an
/// error; out-of-range against `[lo, hi]` is an error whenever finite (pass `f64::INFINITY` for
/// `hi` to mean "no upper bound"; `emissive` is capped at the f32 range). `None` (unauthored) is
/// always valid — the mesh path substitutes the default material's value.
#[allow(clippy::too_many_arguments)] // the range bounds and the per-family code are all load-bearing
fn validate_material_scalar(
    value: Option<f64>,
    field: &str,
    glyph: &str,
    pointer: &str,
    lo: f64,
    hi: f64,
    data_code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(value) = value else {
        return;
    };
    let field_pointer = format!("{pointer}/{field}");
    if !value.is_finite() {
        diagnostics.push(
            Diagnostic::new(
                data_code,
                format!("palette glyph `{glyph}` has a non-finite `{field}` value `{value}`"),
            )
            .with_path(field_pointer)
            .with_help(format!("`{field}` must be a finite number")),
        );
        return;
    }
    if value < lo || value > hi {
        let range = if hi.is_finite() {
            format!("[{lo}, {hi}]")
        } else {
            format!(">= {lo}")
        };
        diagnostics.push(
            Diagnostic::new(
                data_code,
                format!(
                    "palette glyph `{glyph}`'s `{field}` is {value}, outside the valid range \
                     {range}"
                ),
            )
            .with_path(field_pointer)
            .with_help(format!("`{field}` must be in the range {range}")),
        );
    }
}

/// The non-fatal style-budget warning (`L1115`) for `doc`, if its cell count exceeds
/// [`VOXEL_CELL_BUDGET`]. `None` when under budget.
pub(crate) fn budget_warning(doc: &VoxelDocument) -> Option<Diagnostic> {
    // The authored opt-out. Checked before the count so an exempt model costs nothing to ask about.
    if doc.budget_exempt == Some(true) {
        return None;
    }
    let cells = doc.cell_count();
    if cells <= VOXEL_CELL_BUDGET {
        return None;
    }
    Some(
        Diagnostic::new(
            code::VOXEL_SIZE_BUDGET_EXCEEDED,
            format!(
                "this voxel model has {cells} cells ({}x{}x{}), above the {VOXEL_CELL_BUDGET}-cell \
                 (64^3) style budget",
                doc.size[0], doc.size[1], doc.size[2]
            ),
        )
        .with_path("/size")
        .with_help(
            "author-scale models stay well under 64^3; split large content into several models \
             rather than one huge file — this is a warning, not an error, and does not block \
             validation",
        ),
    )
}

/// The six face-adjacent neighbour steps, in the fixed order `+X, -X, +Y, -Y, +Z, -Z`.
///
/// One order for every sweep in this module: the flood fill, the gap BFS, and — visibly —
/// [`EmissiveFaces::faces`], whose printed columns are these directions.
const NEIGHBORS: [[i64; 3]; 6] = [
    [1, 0, 0],
    [-1, 0, 0],
    [0, 1, 0],
    [0, -1, 0],
    [0, 0, 1],
    [0, 0, -1],
];

/// A voxel document's cells resolved to a flat grid: the glyph at each cell, or [`EMPTY_GLYPH`] for
/// an empty one. The shared front half of every 6-neighbour sweep (connectivity, gaps, exposure), so
/// they all agree on what "filled" means without re-walking the row strings.
///
/// Cells are indexed `(y * sz + z) * sx + x` — the (y, z, x) order the row strings are authored in,
/// so ascending index *is* scan order.
struct Occupancy {
    sx: usize,
    sy: usize,
    sz: usize,
    glyphs: Vec<char>,
}

impl Occupancy {
    /// One pass over the row strings, each decoded left to right through [`glyph_lookup`] — the
    /// same linear decode [`VoxelDocument::for_each_cell`] runs, keeping "filled" one definition.
    /// Call on a [`validate`]-clean document (dimensions consistent with `size`).
    fn of(doc: &VoxelDocument) -> Self {
        let [sx, sy, sz] = doc.size;
        let (sx, sy, sz) = (sx as usize, sy as usize, sz as usize);
        let lookup = glyph_lookup(&doc.palette);
        let mut glyphs = vec![EMPTY_GLYPH; sx * sy * sz];
        for (y, layer) in doc.slices.iter().enumerate().take(sy) {
            for (z, row) in layer.iter().enumerate().take(sz) {
                for (x, glyph) in row.chars().enumerate().take(sx) {
                    if decode_glyph(&lookup, glyph).is_some() {
                        glyphs[(y * sz + z) * sx + x] = glyph;
                    }
                }
            }
        }
        Self { sx, sy, sz, glyphs }
    }

    fn index(&self, [x, y, z]: [usize; 3]) -> usize {
        (y * self.sz + z) * self.sx + x
    }

    /// The inverse of [`Self::index`].
    fn cell(&self, index: usize) -> [usize; 3] {
        let x = index % self.sx;
        let rest = index / self.sx;
        [x, rest / self.sz, rest % self.sz]
    }

    fn is_filled(&self, cell: [usize; 3]) -> bool {
        self.glyphs[self.index(cell)] != EMPTY_GLYPH
    }

    /// The in-bounds cell one `step` from `cell`, or `None` past a grid face.
    fn step(&self, [x, y, z]: [usize; 3], [dx, dy, dz]: [i64; 3]) -> Option<[usize; 3]> {
        let (nx, ny, nz) = (x as i64 + dx, y as i64 + dy, z as i64 + dz);
        let inside = nx >= 0
            && ny >= 0
            && nz >= 0
            && nx < self.sx as i64
            && ny < self.sy as i64
            && nz < self.sz as i64;
        inside.then_some([nx as usize, ny as usize, nz as usize])
    }
}

/// One 6-connected component of filled cells: a sample cell, plus every cell it spans.
///
/// The cell list is what lets the connectivity warning answer *where the gap is* rather than only
/// *that there is one*. It is bounded by the grid (a budget-sized model costs ~3 MB) and transient —
/// built for one warning and dropped.
struct VoxelComponent {
    sample: [u32; 3],
    cells: Vec<[u32; 3]>,
}

/// The 6-connected components of `doc`'s filled cells, sorted by cell count ascending then by sample
/// coordinate — so the smallest (likeliest-stray) piece is first and the order is deterministic. Two
/// cells connect only across a shared face, never a diagonal edge or corner.
fn filled_components(doc: &VoxelDocument) -> Vec<VoxelComponent> {
    components_of(&Occupancy::of(doc))
}

/// [`filled_components`] over an already-built occupancy grid — a flood fill from each unvisited
/// filled cell (a `Vec<bool>` visited grid, not a `HashSet`: determinism-clean and simpler).
fn components_of(occupancy: &Occupancy) -> Vec<VoxelComponent> {
    let mut visited = vec![false; occupancy.glyphs.len()];
    let mut components = Vec::new();
    let mut stack: Vec<[usize; 3]> = Vec::new();
    // Scan in (y, z, x) order so a component's sample is its first cell in that stable order.
    for y in 0..occupancy.sy {
        for z in 0..occupancy.sz {
            for x in 0..occupancy.sx {
                let start = occupancy.index([x, y, z]);
                if !occupancy.is_filled([x, y, z]) || visited[start] {
                    continue;
                }
                visited[start] = true;
                stack.push([x, y, z]);
                let sample = [x as u32, y as u32, z as u32];
                let mut cells = Vec::new();
                while let Some(cell) = stack.pop() {
                    cells.push([cell[0] as u32, cell[1] as u32, cell[2] as u32]);
                    for step in NEIGHBORS {
                        let Some(neighbor) = occupancy.step(cell, step) else {
                            continue;
                        };
                        let n = occupancy.index(neighbor);
                        if occupancy.is_filled(neighbor) && !visited[n] {
                            visited[n] = true;
                            stack.push(neighbor);
                        }
                    }
                }
                components.push(VoxelComponent { sample, cells });
            }
        }
    }
    components.sort_by(|a, b| {
        a.cells
            .len()
            .cmp(&b.cells.len())
            .then(a.sample.cmp(&b.sample))
    });
    components
}

/// The nearest-cell pair between one stray component and the main body: which cell of each, and the
/// 6-connected step distance between them (on a grid, that is their Manhattan distance).
#[derive(Clone, Copy, Debug)]
struct ComponentGap {
    stray: [u32; 3],
    main: [u32; 3],
    distance: u32,
}

/// The gap from each stray component to the main body — one entry per element of
/// `components[..len - 1]`, in that order. `None` for a component the sweep could not reach, which
/// only an empty main body can produce.
///
/// **One** multi-source BFS, never a pairwise cell scan: seeded with every main-body cell at once
/// (in scan order), it traverses *all* grid cells — empty ones included — so the distance field it
/// leaves behind is each cell's step distance to the nearest main-body cell, and the answer for a
/// stray piece is a minimum over its own cells. Cost is O(cells), not O(main × stray).
///
/// Deterministic by construction: fixed seed order, fixed [`NEIGHBORS`] order, FIFO queue — and the
/// per-component minimum is taken on the explicit key `(distance, stray cell)`, so a tie between two
/// equidistant cells resolves lexicographically rather than by traversal order.
fn component_gaps(
    occupancy: &Occupancy,
    components: &[VoxelComponent],
) -> Vec<Option<ComponentGap>> {
    const UNREACHED: u32 = u32::MAX;
    let Some(main) = components.last() else {
        return Vec::new();
    };

    let mut distance = vec![UNREACHED; occupancy.glyphs.len()];
    // The main-body cell each shortest path started from, as a linear index.
    let mut source = vec![0u32; occupancy.glyphs.len()];
    let mut seeds: Vec<usize> = main
        .cells
        .iter()
        .map(|&[x, y, z]| occupancy.index([x as usize, y as usize, z as usize]))
        .collect();
    seeds.sort_unstable();
    let mut queue: VecDeque<[usize; 3]> = VecDeque::with_capacity(seeds.len());
    for seed in seeds {
        distance[seed] = 0;
        source[seed] = seed as u32;
        queue.push_back(occupancy.cell(seed));
    }
    while let Some(cell) = queue.pop_front() {
        let here = occupancy.index(cell);
        let (next, from) = (distance[here] + 1, source[here]);
        for step in NEIGHBORS {
            let Some(neighbor) = occupancy.step(cell, step) else {
                continue;
            };
            let n = occupancy.index(neighbor);
            if distance[n] == UNREACHED {
                distance[n] = next;
                source[n] = from;
                queue.push_back(neighbor);
            }
        }
    }

    components[..components.len() - 1]
        .iter()
        .map(|component| {
            component
                .cells
                .iter()
                .filter_map(|&stray| {
                    let index =
                        occupancy.index([stray[0] as usize, stray[1] as usize, stray[2] as usize]);
                    (distance[index] != UNREACHED).then_some((
                        distance[index],
                        stray,
                        source[index],
                    ))
                })
                .min_by_key(|&(distance, stray, _)| (distance, stray))
                .map(|(distance, stray, from)| {
                    let [x, y, z] = occupancy.cell(from as usize);
                    ComponentGap {
                        stray,
                        main: [x as u32, y as u32, z as u32],
                        distance,
                    }
                })
        })
        .collect()
}

/// What closing a gap of `distance` steps costs, in cells: the two nearest cells have `distance - 1`
/// cells between them.
fn bridge_phrase(distance: u32) -> String {
    match distance.saturating_sub(1) {
        0 => "the two cells already touch".to_string(),
        1 => "1 bridge cell to connect".to_string(),
        bridge => format!("{bridge} bridge cells to connect"),
    }
}

/// The non-fatal connectivity warning (`L1129`) for `doc`, if its filled cells split into more than
/// one 6-connected component. `None` when the model is empty or fully connected.
///
/// The first line names the component count and the main body's size; one indented line per stray
/// piece follows, smallest first, each carrying its cell count, a sample cell, and — the point of the
/// report — the *nearest pair* of cells between it and the main body plus the gap between them. A
/// sample cell alone locates the piece, not the place to fix it: a stray shoulder's scan-order first
/// cell can be a dozen layers away from the one gap that severed it.
pub(crate) fn connectivity_warning(doc: &VoxelDocument) -> Option<Diagnostic> {
    let occupancy = Occupancy::of(doc);
    warning_of(&occupancy, &components_of(&occupancy))
}

/// [`connectivity_warning`] over an already-built occupancy and its components — the shared form, so
/// a caller that also wants the component count ([`VoxelDocument::connectivity`]) pays for one grid
/// decode and one flood fill, not two of each.
fn warning_of(occupancy: &Occupancy, components: &[VoxelComponent]) -> Option<Diagnostic> {
    if components.len() <= 1 {
        return None;
    }
    let count = components.len();
    // Sorted by cell count ascending, so the last element is the main body and every earlier one is
    // a stray piece.
    let main = components[count - 1].cells.len();
    let gaps = component_gaps(occupancy, components);
    let mut message = format!(
        "this voxel model's filled cells split into {count} disconnected components (main body \
         {main} cells); each stray piece, smallest first:"
    );
    for (component, gap) in components[..count - 1].iter().zip(&gaps) {
        let cells = component.cells.len();
        let [x, y, z] = component.sample;
        message.push_str(&format!(
            "\n  {cells} cell{}, sample ({x}, {y}, {z})",
            if cells == 1 { "" } else { "s" }
        ));
        if let Some(gap) = gap {
            let [sx, sy, sz] = gap.stray;
            let [mx, my, mz] = gap.main;
            message.push_str(&format!(
                "; nearest cells to the main body: stray ({sx}, {sy}, {sz}) vs main ({mx}, {my}, \
                 {mz}), gap {} ({})",
                gap.distance,
                bridge_phrase(gap.distance)
            ));
        }
    }
    Some(
        Diagnostic::new(code::VOXEL_DISCONNECTED_COMPONENTS, message)
            .with_path("/slices")
            .with_help(
                "if a piece was meant to attach, fill the cells between the nearest pair named \
                 above (6-connectivity — a purely diagonal touch does not connect); note that a \
                 `line` op with a diagonal step draws exactly such a chain of edge-touching cells, \
                 so bridge with face-adjacent `set` steps instead; if the detachment is \
                 intentional, ignore this warning (it does not block validation)",
            ),
    )
}

/// The nearest-cell pair from one stray [`ConnectivityComponent`] to the main body, structured — see
/// [`ConnectivityComponent::gap`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectivityGap {
    /// The stray component's cell nearest the main body.
    pub stray: [u32; 3],
    /// The main body's cell nearest that stray cell.
    pub main: [u32; 3],
    /// The 6-connected step distance between them (`bridge cells = distance - 1`).
    pub distance: u32,
}

/// One 6-connected component of a [`VoxelDocument`]'s filled cells, structured — the data behind
/// [`VoxelDocument::connectivity`]'s rendered `L1129` diagnostic, for a caller that needs the actual
/// cells rather than the message text (an assembly's per-part attribution, `Plan/voxel-animation.md`
/// decision 11). Produced by [`VoxelDocument::connectivity_components`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectivityComponent {
    /// Every filled cell this component spans.
    pub cells: Vec<[u32; 3]>,
    /// This component's first cell in `(y, z, x)` scan order — the same sample
    /// [`VoxelDocument::connectivity`]'s diagnostic names.
    pub sample: [u32; 3],
    /// The nearest-pair gap to the main body, or `None` for the main body itself (the last, largest
    /// entry [`VoxelDocument::connectivity_components`] returns).
    pub gap: Option<ConnectivityGap>,
}

impl VoxelDocument {
    /// [`Self::connectivity`]'s structured form: every 6-connected component of filled cells, smallest
    /// first (matching the diagnostic's stray-piece order), the main body last with `gap: None`.
    ///
    /// Reuses the exact same occupancy decode, flood fill, and multi-source BFS
    /// [`Self::connectivity`]'s diagnostic renders from — nothing here is a second implementation of
    /// that analysis, only a structured view of its result.
    pub fn connectivity_components(&self) -> Vec<ConnectivityComponent> {
        let occupancy = Occupancy::of(self);
        let components = components_of(&occupancy);
        let gaps = component_gaps(&occupancy, &components);
        let count = components.len();
        components
            .into_iter()
            .enumerate()
            .map(|(index, component)| ConnectivityComponent {
                cells: component.cells,
                sample: component.sample,
                gap: if index + 1 == count {
                    None
                } else {
                    gaps[index].map(|gap| ConnectivityGap {
                        stray: gap.stray,
                        main: gap.main,
                        distance: gap.distance,
                    })
                },
            })
            .collect()
    }
}

/// Writes `doc` in canonical form: `format`, `kind`, `name` (optional), `size`, `scale`
/// (optional), `generated_from` (optional), `budget_exempt` (optional), `palette` (sorted by
/// glyph), `slices` (one row per line), ryu shortest-round-trip floats — the voxel analog of
/// [`crate::emit`].
pub(crate) fn emit(doc: &VoxelDocument) -> String {
    emit_with_comments(doc, &VoxelComments::default())
}

/// [`emit`] with authoring comments replayed into it: the header block between the envelope keys
/// and `size:`, and each layer's anatomy line above its `-` inside `slices:`.
///
/// Comment-free input reproduces [`emit`] byte for byte — the one property that lets the canonical
/// writer stay a single function while comments survive a grid reshape.
pub(crate) fn emit_with_comments(doc: &VoxelDocument, comments: &VoxelComments) -> String {
    let mut out = String::new();
    out.push_str("format: 1\n");
    out.push_str("kind: voxel\n");
    if let Some(name) = &doc.name {
        out.push_str("name: ");
        out.push_str(&scalar_string(name));
        out.push('\n');
    }
    for line in &comments.header {
        out.push_str(&comment_line("", line));
    }

    out.push_str("size:\n");
    for dim in doc.size {
        out.push_str("  - ");
        out.push_str(&dim.to_string());
        out.push('\n');
    }

    if let Some(scale) = doc.scale {
        out.push_str("scale: ");
        out.push_str(&format_float(scale));
        out.push('\n');
    }

    if let Some(source) = &doc.generated_from {
        out.push_str("generated_from: ");
        out.push_str(&scalar_string(source));
        out.push('\n');
    }

    if let Some(exempt) = doc.budget_exempt {
        out.push_str(if exempt {
            "budget_exempt: true\n"
        } else {
            "budget_exempt: false\n"
        });
    }

    emit_palette(&mut out, &doc.palette);

    if doc.slices.is_empty() {
        out.push_str("slices: []\n");
    } else {
        out.push_str("slices:\n");
        for (y, layer) in doc.slices.iter().enumerate() {
            if let Some(text) = comments.layer(y as u32) {
                for line in text.lines() {
                    out.push_str(&comment_line("  ", line));
                }
            }
            if layer.is_empty() {
                out.push_str("  - []\n");
            } else {
                out.push_str("  -\n");
                for row in layer {
                    out.push_str("    - ");
                    // Rows are grid data, always quoted, so a re-emit never churns untouched rows.
                    out.push_str(&crate::scalar::quote_double(row));
                    out.push('\n');
                }
            }
        }
    }

    out
}

/// Appends a canonical `palette:` block to `out`: entries sorted by glyph, `color` first, then each
/// authored material scalar; an empty palette is the flow empty `palette: {}`.
///
/// Shared with the [`crate::assembly`] family, whose palette is exactly a voxel document's, so the
/// two families cannot drift into two spellings of the same block.
pub(crate) fn emit_palette(out: &mut String, palette: &[PaletteEntry]) {
    if palette.is_empty() {
        out.push_str("palette: {}\n");
        return;
    }
    out.push_str("palette:\n");
    let mut palette = palette.to_vec();
    palette.sort_by(|a, b| a.glyph.cmp(&b.glyph));
    for entry in &palette {
        out.push_str("  ");
        out.push_str(&scalar_string(&entry.glyph));
        out.push_str(":\n    color:\n");
        for component in entry.color {
            out.push_str("      - ");
            out.push_str(&format_float(component));
            out.push('\n');
        }
        if let Some(emissive) = entry.emissive {
            out.push_str("    emissive: ");
            out.push_str(&format_float(emissive));
            out.push('\n');
        }
        if let Some(metallic) = entry.metallic {
            out.push_str("    metallic: ");
            out.push_str(&format_float(metallic));
            out.push('\n');
        }
        if let Some(roughness) = entry.roughness {
            out.push_str("    roughness: ");
            out.push_str(&format_float(roughness));
            out.push('\n');
        }
    }
}

/// One comment line at `indent`: `# text`, or a bare `#` for an empty line (no trailing space,
/// which an editor or a formatter would strip and turn into a spurious diff).
pub(crate) fn comment_line(indent: &str, text: &str) -> String {
    if text.is_empty() {
        format!("{indent}#\n")
    } else {
        format!("{indent}# {text}\n")
    }
}

// --- Render-time palette overrides ------------------------------------------------------------

/// The pointer shapes a model override may address, spelled out for every `L4008` this module
/// raises — a rejected pointer is nearly always a guess at the domain, so the help states it whole.
const PALETTE_POINTER_SHAPES: &str = "a model override addresses one palette field: \
     `/palette/<glyph>/color/<0..3>`, `/palette/<glyph>/emissive`, `/palette/<glyph>/metallic`, or \
     `/palette/<glyph>/roughness`";

/// Applies render-time overrides to a parsed voxel document, in order, returning what each changed.
///
/// The model-document counterpart of [`crate::apply_overrides`]: it is what `ludion render --model
/// --set` and `--sweep` write, so a palette can be calibrated without touching the file. The domain
/// is the palette alone — `size` and `slices` are the model's *shape*, not a value to tune, and a
/// pointer that could rewrite them would let a typo render a model nobody authored.
///
/// Writing a scalar the entry left unset creates it, and [`AppliedOverride::from`] comes back `None`
/// so a caller can echo `unset -> <value>`. Range and finiteness are not checked entry by entry:
/// once every override is written the document is validated, so an out-of-range value comes back
/// as the ordinary `L1114`, exactly as the same edit to the file would.
///
/// # Errors
/// `L4008` per override whose pointer is not one of the supported shapes, names a glyph the palette
/// does not have, or carries a non-numeric value. Failures are collected, so a batch reports every
/// bad pointer in one pass. When every override applies, the document's own error codes (`L1114`
/// for a palette value out of range) come back instead. On failure `doc` is left partially
/// overridden and the caller must discard it.
///
/// # Example
/// ```
/// use ludion_scene::{SceneOverride, apply_palette_overrides};
/// use serde_json::json;
///
/// let source = "format: 1\nkind: voxel\nsize:\n  - 1\n  - 1\n  - 1\npalette:\n  G:\n    \
///               color:\n      - 0.1\n      - 0.2\n      - 0.3\n      - 1.0\nslices:\n  -\n    - G\n";
/// let mut model = ludion_scene::check_voxel(source).unwrap();
/// let overrides = [
///     SceneOverride::parse("/palette/G/color/1=0.75").unwrap(),
///     // A scalar the entry left unset is created, not refused.
///     SceneOverride::parse("/palette/G/emissive=2.2").unwrap(),
/// ];
///
/// let applied = apply_palette_overrides(&mut model, &overrides).unwrap();
/// assert_eq!(applied[0].from, Some(json!(0.2)));
/// assert_eq!(applied[1].from, None, "`emissive` was not in the file");
/// assert_eq!(model.palette[0].color[1], 0.75);
/// assert_eq!(model.palette[0].emissive, Some(2.2));
/// ```
pub fn apply_palette_overrides(
    doc: &mut VoxelDocument,
    overrides: &[SceneOverride],
) -> Result<Vec<AppliedOverride>, Vec<Diagnostic>> {
    let applied = apply_palette(&mut doc.palette, overrides)?;
    into_result(applied, validate(doc))
}

/// `applied`, unless the overridden document now carries an error-severity diagnostic (a lint such
/// as `L1115` never fails an override).
pub(crate) fn into_result(
    applied: Vec<AppliedOverride>,
    diagnostics: Vec<Diagnostic>,
) -> Result<Vec<AppliedOverride>, Vec<Diagnostic>> {
    let errors: Vec<Diagnostic> = diagnostics
        .into_iter()
        .filter(|d| d.severity.is_error())
        .collect();
    if errors.is_empty() {
        Ok(applied)
    } else {
        Err(errors)
    }
}

/// Applies render-time overrides to a parsed assembly document, in order, returning what each
/// changed.
///
/// The assembly counterpart of [`apply_palette_overrides`], and literally the same code over the
/// same [`PaletteEntry`] list: an assembly owns the one palette every part it places resolves its
/// glyphs against, so the pointer domain, the `L4008` shapes and the [`AppliedOverride`] echo are
/// the voxel document's, described there.
///
/// # Errors
/// `L4008`, per override, on exactly the conditions [`apply_palette_overrides`] documents; once
/// every override applies, the assembly's own error codes (`L1138` for a palette value out of
/// range) come back instead.
///
/// # Example
/// ```
/// use ludion_scene::{SceneOverride, apply_assembly_palette_overrides};
///
/// let source = "format: 1\nkind: assembly\nsize:\n  - 1\n  - 1\n  - 1\npalette:\n  G:\n    \
///               color:\n      - 0.1\n      - 0.2\n      - 0.3\n      - 1.0\nparts:\n  - name: \
///               body\n    file: parts/body.part.yaml\n    place:\n      - 0\n      - 0\n      \
///               - 0\n";
/// let mut model = ludion_scene::parse_assembly(source).unwrap();
/// let overrides = [SceneOverride::parse("/palette/G/emissive=2.2").unwrap()];
///
/// let applied = apply_assembly_palette_overrides(&mut model, &overrides).unwrap();
/// assert_eq!(applied[0].from, None, "`emissive` was not in the file");
/// assert_eq!(model.palette[0].emissive, Some(2.2));
/// ```
pub fn apply_assembly_palette_overrides(
    doc: &mut crate::assembly::AssemblyDocument,
    overrides: &[SceneOverride],
) -> Result<Vec<AppliedOverride>, Vec<Diagnostic>> {
    let applied = apply_palette(&mut doc.palette, overrides)?;
    into_result(applied, crate::assembly::validate(doc))
}

/// The applier both document kinds share: a palette is a palette, whichever document owns it.
///
/// Failures are collected rather than short-circuited, so a batch reports every bad pointer in one
/// pass and the caller discards the partially overridden palette.
fn apply_palette(
    palette: &mut [PaletteEntry],
    overrides: &[SceneOverride],
) -> Result<Vec<AppliedOverride>, Vec<Diagnostic>> {
    let mut applied = Vec::with_capacity(overrides.len());
    let mut diagnostics = Vec::new();
    for over in overrides {
        match apply_palette_one(palette, over) {
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

pub(crate) fn apply_palette_one(
    palette: &mut [PaletteEntry],
    over: &SceneOverride,
) -> Result<AppliedOverride, Diagnostic> {
    let pointer = over.pointer.as_str();
    let segments: Vec<String> = pointer.split('/').skip(1).map(unescape).collect();
    if segments.first().map(String::as_str) != Some("palette") {
        return Err(unsupported_pointer(pointer));
    }
    let Some(glyph) = segments.get(1) else {
        return Err(unsupported_pointer(pointer));
    };
    let Some(index) = palette.iter().position(|entry| entry.glyph == *glyph) else {
        return Err(unknown_glyph(pointer, glyph, palette));
    };
    let entry = &mut palette[index];

    let from = match (segments.get(2).map(String::as_str), segments.len()) {
        (Some("color"), 4) => {
            let raw = &segments[3];
            let Some(channel) = raw.parse::<usize>().ok().filter(|i| *i < entry.color.len()) else {
                return Err(bad_color_channel(pointer, raw));
            };
            let value = as_number(pointer, &over.value)?;
            let from = Value::from(entry.color[channel]);
            entry.color[channel] = value;
            Some(from)
        }
        (Some(name @ ("emissive" | "metallic" | "roughness")), 3) => {
            let value = as_number(pointer, &over.value)?;
            let slot = match name {
                "emissive" => &mut entry.emissive,
                "metallic" => &mut entry.metallic,
                _ => &mut entry.roughness,
            };
            let from = slot.map(Value::from);
            *slot = Some(value);
            from
        }
        _ => return Err(unsupported_pointer(pointer)),
    };

    Ok(AppliedOverride {
        pointer: pointer.to_string(),
        from,
        to: over.value.clone(),
    })
}

/// A palette field takes a number, and only a number: every one of them is a float in the document.
fn as_number(pointer: &str, value: &Value) -> Result<f64, Diagnostic> {
    value
        .as_f64()
        .map(ludion_core::number::engine_number)
        .ok_or_else(|| {
            Diagnostic::new(
                code::SCENE_OVERRIDE_INVALID,
                format!(
                    "the override at `{pointer}` expects a number, but the value is {}",
                    ludion_core::diagnostics::value_phrase(value)
                ),
            )
            .with_path(pointer.to_string())
            .with_help("every palette field is a number, e.g. `/palette/G/emissive=2.2`")
        })
}

fn unsupported_pointer(pointer: &str) -> Diagnostic {
    Diagnostic::new(
        code::SCENE_OVERRIDE_INVALID,
        format!("override pointer `{pointer}` does not address a palette field of this model"),
    )
    .with_path(pointer.to_string())
    .with_help(PALETTE_POINTER_SHAPES)
}

fn unknown_glyph(pointer: &str, glyph: &str, palette: &[PaletteEntry]) -> Diagnostic {
    let keys: Vec<&str> = palette.iter().map(|entry| entry.glyph.as_str()).collect();
    let held = if keys.is_empty() {
        "this model's palette is empty".to_string()
    } else {
        format!("this model's palette holds `{}`", keys.join("`, `"))
    };
    // Only a case difference is worth calling a "nearest match": glyphs are single characters, so
    // edit distance makes every one of them equally near and the suggestion would be noise.
    let help = match keys
        .iter()
        .find(|key| key.eq_ignore_ascii_case(glyph) && **key != glyph)
    {
        Some(candidate) => format!("{held} — did you mean `{candidate}`?"),
        None => held,
    };
    Diagnostic::new(
        code::SCENE_OVERRIDE_INVALID,
        format!(
            "override pointer `{pointer}` names glyph `{glyph}`, which the palette does not have"
        ),
    )
    .with_path(pointer.to_string())
    .with_help(help)
}

fn bad_color_channel(pointer: &str, segment: &str) -> Diagnostic {
    Diagnostic::new(
        code::SCENE_OVERRIDE_INVALID,
        format!("override pointer `{pointer}` addresses color channel `{segment}`, but a color is four channels"),
    )
    .with_path(pointer.to_string())
    .with_help("`0` is red, `1` green, `2` blue, `3` alpha — e.g. `/palette/S/color/0=0.12`")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_count_saturates_instead_of_wrapping() {
        let doc = |size| VoxelDocument {
            size,
            ..Default::default()
        };
        assert_eq!(doc([u32::MAX; 3]).cell_count(), u64::MAX);
        assert_eq!(doc([2, 3, 4]).cell_count(), 24);
    }

    const GEM: &str = "\
format: 1
kind: voxel
size:
  - 2
  - 1
  - 2
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
slices:
  -
    - \"A.\"
    - \".A\"
";

    #[test]
    fn parses_a_small_model() {
        let doc = parse(GEM).expect("voxel should parse");
        assert_eq!(doc.size, [2, 1, 2]);
        assert_eq!(doc.palette.len(), 1);
        assert_eq!(doc.palette[0].glyph, "A");
        assert!(validate(&doc).is_empty());
        // (0,0,0) is 'A' → index 0; (1,0,0) is '.' → empty; (1,0,1) is 'A'.
        assert_eq!(doc.palette_index(0, 0, 0), Some(0));
        assert_eq!(doc.palette_index(1, 0, 0), None);
        assert_eq!(doc.palette_index(1, 0, 1), Some(0));
        // Out of range reads as empty.
        assert_eq!(doc.palette_index(9, 0, 0), None);
        assert_eq!(doc.filled_count(), 2);
    }

    #[test]
    fn emit_is_a_fixed_point() {
        let doc = parse(GEM).expect("voxel should parse");
        assert_eq!(emit(&doc), GEM);
    }

    #[test]
    fn missing_slices_is_l1113() {
        let diagnostics =
            parse("format: 1\nkind: voxel\nsize:\n  - 1\n  - 1\n  - 1\npalette: {}\n").unwrap_err();
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == code::VOXEL_DOCUMENT_STRUCTURE && d.message.contains("slices"))
        );
    }

    #[test]
    fn bad_kind_is_l1113() {
        let diagnostics =
            parse("format: 1\nkind: mesh\nsize:\n  - 1\n  - 1\n  - 1\npalette: {}\nslices: []\n")
                .unwrap_err();
        assert_eq!(diagnostics[0].code, code::VOXEL_DOCUMENT_STRUCTURE);
        assert_eq!(diagnostics[0].path.as_deref(), Some("/kind"));
    }

    #[test]
    fn unknown_key_suggests_nearest() {
        let diagnostics = parse(
            "format: 1\nkind: voxel\nsize:\n  - 1\n  - 1\n  - 1\npalate: {}\npalette: {}\n\
             slices: []\n",
        )
        .unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/palate"))
            .expect("unknown key diagnostic");
        assert_eq!(d.code, code::VOXEL_DOCUMENT_STRUCTURE);
        assert_eq!(d.help.as_deref(), Some("nearest match is `palette`"));
    }

    /// `scale` parses, validates, and emits between `size` and `palette`; a typo'd key rides the
    /// same nearest-suggestion path the other keys do.
    #[test]
    fn scale_round_trips_in_canonical_position() {
        let source = GEM.replace("palette:", "scale: 0.24\npalette:");
        let doc = parse(&source).expect("voxel with a scale should parse");
        assert_eq!(doc.scale, Some(0.24));
        assert!(validate(&doc).is_empty());
        assert_eq!(emit(&doc), source);
    }

    #[test]
    fn non_positive_scale_is_l1114() {
        let doc = parse(&GEM.replace("palette:", "scale: 0.0\npalette:")).expect("parses");
        let diagnostics = validate(&doc);
        assert_eq!(diagnostics[0].code, code::VOXEL_DATA_CONSISTENCY);
        assert_eq!(diagnostics[0].path.as_deref(), Some("/scale"));
    }

    #[test]
    fn non_number_scale_is_l1113() {
        let diagnostics = parse(&GEM.replace("palette:", "scale: big\npalette:")).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/scale"))
            .expect("a /scale diagnostic");
        assert_eq!(d.code, code::VOXEL_DOCUMENT_STRUCTURE);
    }

    #[test]
    fn misspelled_scale_suggests_scale() {
        let diagnostics = parse(&GEM.replace("palette:", "scal: 0.24\npalette:")).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/scal"))
            .expect("unknown key diagnostic");
        assert_eq!(d.help.as_deref(), Some("nearest match is `scale`"));
    }

    #[test]
    fn peek_kind_finds_voxel() {
        assert_eq!(crate::kinds::peek_kind(GEM).as_deref(), Some("voxel"));
    }

    /// A fully face-connected model is one component — no warning.
    #[test]
    fn single_component_has_no_warning() {
        // A 2×1×2 solid slab: all four cells share faces.
        let doc = parse(
            "format: 1\nkind: voxel\nsize:\n  - 2\n  - 1\n  - 2\npalette:\n  A:\n    color:\n      \
             - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\nslices:\n  -\n    - AA\n    - AA\n",
        )
        .expect("voxel parses");
        assert!(validate(&doc).is_empty());
        assert_eq!(doc.component_count(), 1);
        assert!(connectivity_warning(&doc).is_none());
    }

    /// Two cells separated by a gap are two components — warning names both the count and a sample of
    /// the smaller (here both are size 1; the lexicographically-first sample leads).
    #[test]
    fn two_components_warn_with_sample() {
        // A 3×1×1 row: cells at x=0 and x=2, empty at x=1 — two separate single-cell components.
        let doc = parse(
            "format: 1\nkind: voxel\nsize:\n  - 3\n  - 1\n  - 1\npalette:\n  A:\n    color:\n      \
             - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\nslices:\n  -\n    - A.A\n",
        )
        .expect("voxel parses");
        assert!(validate(&doc).is_empty());
        assert_eq!(doc.component_count(), 2);
        let warning = connectivity_warning(&doc).expect("two components warn");
        assert_eq!(warning.code, code::VOXEL_DISCONNECTED_COMPONENTS);
        assert!(warning.message.contains("2 disconnected components"));
        // The stray piece reported is the first component in scan order, cell (0, 0, 0).
        assert!(
            warning.message.contains("(0, 0, 0)"),
            "expected a sample coordinate: {}",
            warning.message
        );
        // And the gap report: the nearest pair and what it costs to close.
        assert!(
            warning.message.contains(
                "nearest cells to the main body: stray (0, 0, 0) vs main (2, 0, 0), gap 2 (1 \
                 bridge cell to connect)"
            ),
            "expected a gap line: {}",
            warning.message
        );
    }

    /// [`VoxelDocument::connectivity_components`] is the structured form of the same warning
    /// [`two_components_warn_with_sample`] renders as text: same components, same gap, main body last
    /// with `gap: None`.
    #[test]
    fn connectivity_components_structures_the_same_gap() {
        let doc = parse(
            "format: 1\nkind: voxel\nsize:\n  - 3\n  - 1\n  - 1\npalette:\n  A:\n    color:\n      \
             - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\nslices:\n  -\n    - A.A\n",
        )
        .expect("voxel parses");
        let components = doc.connectivity_components();
        assert_eq!(components.len(), 2);
        let stray = &components[0];
        assert_eq!(stray.sample, [0, 0, 0]);
        assert_eq!(stray.cells, vec![[0, 0, 0]]);
        let gap = stray
            .gap
            .expect("the stray component has a gap to the main body");
        assert_eq!(gap.stray, [0, 0, 0]);
        assert_eq!(gap.main, [2, 0, 0]);
        assert_eq!(gap.distance, 2);
        let main = &components[1];
        assert_eq!(main.sample, [2, 0, 0]);
        assert_eq!(main.cells, vec![[2, 0, 0]]);
        assert!(main.gap.is_none(), "the main body's own gap is always None");
    }

    /// A two-cell model, its palette, and its slices — the shorthand every connectivity/exposure
    /// fixture below is built from, so a test says what its *shape* is and nothing else.
    fn model(size: [u32; 3], palette: &[(char, Option<f64>)], slices: &[&[&str]]) -> VoxelDocument {
        let doc = VoxelDocument {
            size,
            palette: palette
                .iter()
                .map(|&(glyph, emissive)| PaletteEntry {
                    glyph: glyph.to_string(),
                    color: [1.0, 1.0, 1.0, 1.0],
                    emissive,
                    metallic: None,
                    roughness: None,
                })
                .collect(),
            slices: slices
                .iter()
                .map(|layer| layer.iter().map(|row| (*row).to_string()).collect())
                .collect(),
            ..Default::default()
        };
        assert!(validate(&doc).is_empty(), "fixture is not a valid model");
        doc
    }

    /// The strays are listed smallest first, each with its own nearest-pair line — the ordering an
    /// author reads top-down when several pieces broke off at once.
    #[test]
    fn strays_are_reported_smallest_first_each_with_its_own_gap() {
        // A 4-cell main body at x=0..1, a 2-cell stray at x=3 (y=0..1), a 1-cell stray at (3,3,0).
        let doc = model(
            [4, 4, 1],
            &[('A', None)],
            &[&["AA.A"], &["AA.A"], &["AA.."], &["...A"]],
        );
        assert_eq!(doc.component_count(), 3);
        let message = connectivity_warning(&doc)
            .expect("three components warn")
            .message;
        let lines: Vec<&str> = message.lines().collect();
        assert!(
            lines[0].contains("3 disconnected components (main body 6 cells)"),
            "{message}"
        );
        assert!(
            lines[1].starts_with("  1 cell, sample (3, 3, 0);"),
            "smallest stray leads: {message}"
        );
        assert!(
            lines[1]
                .contains("stray (3, 3, 0) vs main (1, 2, 0), gap 3 (2 bridge cells to connect)"),
            "{message}"
        );
        assert!(
            lines[2].starts_with("  2 cells, sample (3, 0, 0);"),
            "{message}"
        );
        assert!(
            lines[2]
                .contains("stray (3, 0, 0) vs main (1, 0, 0), gap 2 (1 bridge cell to connect)"),
            "{message}"
        );
    }

    /// When several of a stray's cells sit the same distance from the main body, the reported pair
    /// is chosen by the explicit `(distance, cell)` key — never by BFS scheduling — so the message
    /// is byte-identical across runs.
    #[test]
    fn equidistant_cells_tie_break_lexicographically() {
        // Two parallel 3-cell columns at x=0 and x=2: every cell of one is exactly 2 steps from its
        // counterpart in the other.
        let doc = model([3, 3, 1], &[('A', None)], &[&["A.A"], &["A.A"], &["A.A"]]);
        let first = connectivity_warning(&doc)
            .expect("two components warn")
            .message;
        assert!(
            first.contains("stray (0, 0, 0) vs main (2, 0, 0), gap 2 (1 bridge cell to connect)"),
            "{first}"
        );
        let second = connectivity_warning(&doc)
            .expect("two components warn")
            .message;
        assert_eq!(first, second, "the warning must not vary between runs");
    }

    /// Two equal halves of a 41³ grid: 68,921 cells, 33,620 per component. The gap report is one
    /// multi-source BFS over the grid, so this is instant; a pairwise nearest-pair scan would be
    /// ~10^9 comparisons and this test would not finish.
    #[test]
    fn a_large_two_halves_model_reports_its_gap_cheaply() {
        const EXTENT: usize = 41;
        let row: String = (0..EXTENT)
            .map(|x| if x == EXTENT / 2 { '.' } else { 'A' })
            .collect();
        let doc = VoxelDocument {
            size: [EXTENT as u32; 3],
            palette: vec![PaletteEntry {
                glyph: "A".to_string(),
                color: [1.0, 1.0, 1.0, 1.0],
                emissive: None,
                metallic: None,
                roughness: None,
            }],
            slices: vec![vec![row.clone(); EXTENT]; EXTENT],
            ..Default::default()
        };
        assert!(validate(&doc).is_empty());
        assert_eq!(doc.component_count(), 2);
        let message = connectivity_warning(&doc).expect("two halves warn").message;
        // Both halves are 20 columns wide; the sort tie-breaks on sample, so the low-x half strays.
        assert!(
            message.contains("2 disconnected components (main body 33620 cells)"),
            "{message}"
        );
        assert!(
            message
                .contains("stray (19, 0, 0) vs main (21, 0, 0), gap 2 (1 bridge cell to connect)"),
            "{message}"
        );
    }

    /// A solid 3×3×3 block has exactly one invisible cell — its centre — and the report names it.
    #[test]
    fn exposure_finds_the_enclosed_centre_of_a_solid_block() {
        let doc = model(
            [3, 3, 3],
            &[('A', None)],
            &[
                &["AAA", "AAA", "AAA"],
                &["AAA", "AAA", "AAA"],
                &["AAA", "AAA", "AAA"],
            ],
        );
        let report = doc.exposure(ExposureView::Any);
        assert_eq!(report.filled, 27);
        assert_eq!(report.surface, 26);
        assert_eq!(report.enclosed, 1);
        assert_eq!(report.glyphs.len(), 1);
        assert_eq!(report.glyphs[0].glyph, 'A');
        assert_eq!(report.glyphs[0].cells, 27);
        assert_eq!(report.glyphs[0].enclosed, 1);
        assert_eq!(report.glyphs[0].enclosed_samples, vec![[1, 1, 1]]);
        assert_eq!(report.glyphs[0].enclosed_overflow, 0);
        assert!(report.emissive.is_empty());
    }

    /// A hollow shell has no enclosed cells at all — every cell of it is surface.
    #[test]
    fn exposure_reports_a_hollow_shell_as_all_surface() {
        let doc = model(
            [3, 3, 3],
            &[('A', None)],
            &[
                &["AAA", "AAA", "AAA"],
                &["AAA", "A.A", "AAA"],
                &["AAA", "AAA", "AAA"],
            ],
        );
        let report = doc.exposure(ExposureView::Any);
        assert_eq!(report.filled, 26);
        assert_eq!(report.surface, 26);
        assert_eq!(report.enclosed, 0);
        assert_eq!(report.glyphs[0].enclosed_samples, Vec::<[u32; 3]>::new());
    }

    /// Only the first five enclosed cells are sampled; the rest are counted. Samples come out in
    /// (y, z, x) scan order, which is the order an author reads the slice prints in.
    #[test]
    fn exposure_samples_cap_at_five_and_count_the_rest() {
        // A 3×3×9 solid: the whole z = 1..7 middle column of each layer's centre is enclosed.
        let layer: &[&str] = &["AAA", "AAA", "AAA"];
        let doc = model([3, 9, 3], &[('A', None)], &[layer; 9]);
        let report = doc.exposure(ExposureView::Any);
        assert_eq!(report.enclosed, 7);
        let glyph = &report.glyphs[0];
        assert_eq!(glyph.enclosed_samples.len(), 5);
        assert_eq!(glyph.enclosed_overflow, 2);
        assert_eq!(glyph.enclosed_samples[0], [1, 1, 1]);
        assert_eq!(glyph.enclosed_samples[4], [1, 5, 1]);
    }

    /// The emissive histogram is what catches a "lightbar" whose glow all escapes one way: this
    /// buried `E` cell has exactly one open face, and it points +Y.
    #[test]
    fn exposure_reports_emissive_faces_in_fixed_axis_order() {
        let doc = model(
            [3, 2, 3],
            &[('A', None), ('E', Some(4.5))],
            &[&["AAA", "AAA", "AAA"], &["AAA", "AEA", "AAA"]],
        );
        let report = doc.exposure(ExposureView::Any);
        assert_eq!(report.emissive.len(), 1);
        let light = &report.emissive[0];
        assert_eq!(light.glyph, 'E');
        assert_eq!(light.emissive, 4.5);
        // +X, -X, +Y, -Y, +Z, -Z — the grid's top face is the only one open.
        assert_eq!(light.faces, [0, 0, 1, 0, 0, 0]);
        assert_eq!(light.total(), 1);
        assert_eq!(EmissiveFaces::FACE_LABELS[2], "+Y");
        // The glyph rows carry both glyphs, sorted, and `E` is fully surrounded but not enclosed:
        // its +Y face is the grid boundary, which is visible.
        assert_eq!(
            report.glyphs.iter().map(|g| g.glyph).collect::<Vec<_>>(),
            vec!['A', 'E']
        );
        assert_eq!(report.glyphs[1].enclosed, 0);
    }

    /// A palette entry declaring `emissive: 0.0`, or one no cell uses, is not a light and gets no
    /// row — the histogram lists what actually glows.
    #[test]
    fn exposure_skips_unused_and_zero_emissive_glyphs() {
        let doc = model(
            [3, 1, 1],
            &[('A', None), ('D', Some(0.0)), ('U', Some(2.0))],
            &[&["ADA"]],
        );
        let report = doc.exposure(ExposureView::Any);
        assert!(report.emissive.is_empty());
        assert_eq!(
            report.glyphs.iter().map(|g| g.glyph).collect::<Vec<_>>(),
            vec!['A', 'D'],
            "the unused `U` gets no row"
        );
    }

    /// The exposure report is a pure function of the document: same input, byte-identical rendering.
    #[test]
    fn exposure_is_stable_across_runs() {
        let doc = model(
            [3, 2, 3],
            &[('A', None), ('E', Some(4.5))],
            &[&["AAA", "AAA", "AAA"], &["AAA", "AEA", "AAA"]],
        );
        assert_eq!(
            format!("{:?}", doc.exposure(ExposureView::Any)),
            format!("{:?}", doc.exposure(ExposureView::Any))
        );
    }

    /// A left/right-symmetric model reports no X-mirror asymmetries at all.
    #[test]
    fn mirror_x_symmetric_model_has_no_regions() {
        // 3×1×1, odd width: 'A.A' mirrors exactly; the centre column (x=1) self-matches.
        let doc = parse(
            "format: 1\nkind: voxel\nsize:\n  - 3\n  - 1\n  - 1\npalette:\n  A:\n    color:\n      \
             - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\nslices:\n  -\n    - A.A\n",
        )
        .expect("voxel parses");
        assert!(validate(&doc).is_empty());
        assert!(doc.mirror_x_regions().is_empty());
    }

    /// An asymmetric model reports the differing cell from the low-x side, with both the low glyph
    /// and its mirror (`.` for the empty one) in the region's glyph set.
    #[test]
    fn mirror_x_reports_one_cell_as_a_degenerate_region() {
        // 4×1×1, even width: 'AB.A'. x=0 ('A') vs x=3 ('A') match; x=1 ('B') vs x=2 ('.') differ.
        let doc = parse(
            "format: 1\nkind: voxel\nsize:\n  - 4\n  - 1\n  - 1\npalette:\n  A:\n    color:\n      \
             - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\n  B:\n    color:\n      - 0.0\n      - 1.0\n\
             \x20     - 0.0\n      - 1.0\nslices:\n  -\n    - \"AB.A\"\n",
        )
        .expect("voxel parses");
        assert!(validate(&doc).is_empty());
        let regions = doc.mirror_x_regions();
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].cells, 1);
        assert_eq!(regions[0].min, [1, 0, 0]);
        assert_eq!(regions[0].max, [1, 0, 0]);
        assert_eq!(regions[0].glyphs, ['.', 'B']);
    }

    /// The odd-width centre column is self-mirroring and never reported, even when its neighbours are
    /// asymmetric.
    #[test]
    fn mirror_x_skips_the_centre_column() {
        // 3×1×1, 'A.B': centre x=1 ('.') is skipped; x=0 ('A') vs x=2 ('B') differ.
        let doc = parse(
            "format: 1\nkind: voxel\nsize:\n  - 3\n  - 1\n  - 1\npalette:\n  A:\n    color:\n      \
             - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\n  B:\n    color:\n      - 0.0\n      - 1.0\n\
             \x20     - 0.0\n      - 1.0\nslices:\n  -\n    - \"A.B\"\n",
        )
        .expect("voxel parses");
        let regions = doc.mirror_x_regions();
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].min, [0, 0, 0]);
        assert_eq!(regions[0].max, [0, 0, 0]);
        assert_eq!(regions[0].glyphs, ['A', 'B']);
    }

    /// Two deliberate asymmetric features cluster into exactly two regions — the question
    /// `--mirror-x` exists to answer — each with its own bounding box, cell count, and glyph set,
    /// and the larger one first.
    #[test]
    fn mirror_x_clusters_two_features_into_two_regions() {
        // 10 wide (low half x < 5), 4 tall, 2 deep. The mirror of x is 9 - x.
        // Feature 1 (larger): a 2×2×2 block of 'B' at x=0..1, y=0..1, z=0..1 whose mirror is empty.
        // Feature 2: a single 'C' at (3, 3, 0), whose mirror (6, 3, 0) holds 'A'.
        // Everything else is a symmetric 'A' fill, so only the two features differ.
        let doc = model(
            [10, 4, 2],
            &[('A', None), ('B', None), ('C', None)],
            &[
                &["BBAAAAAA..", "BBAAAAAA.."], // y = 0
                &["BBAAAAAA..", "BBAAAAAA.."], // y = 1
                &["AAAAAAAAAA", "AAAAAAAAAA"], // y = 2
                &["AAACAAAAAA", "AAAAAAAAAA"], // y = 3: (3,3,0) 'C' vs (6,3,0) 'A'
            ],
        );
        let regions = doc.mirror_x_regions();
        assert_eq!(regions.len(), 2, "two features, two regions: {regions:?}");
        // Largest first: the 2×2×2 block's eight low-half cells.
        assert_eq!(regions[0].cells, 8);
        assert_eq!(regions[0].min, [0, 0, 0]);
        assert_eq!(regions[0].max, [1, 1, 1]);
        // Both sides of every pair: 'B' on the low side, '.' on the mirrored high side.
        assert_eq!(regions[0].glyphs, ['.', 'B']);
        assert_eq!(regions[1].cells, 1);
        assert_eq!(regions[1].min, [3, 3, 0]);
        assert_eq!(regions[1].max, [3, 3, 0]);
        assert_eq!(regions[1].glyphs, ['A', 'C']);
    }

    /// Cells that only touch across a corner are two regions, not one — the region count is a
    /// 6-connected count, the same rule `L1129` uses.
    #[test]
    fn mirror_x_regions_are_six_connected() {
        // 8 wide (low half x < 4): differing cells at (0,0,0) and (1,1,0) touch only at an edge.
        let doc = model(
            [8, 2, 1],
            &[('A', None), ('B', None)],
            &[&["BAAAAAAA"], &["ABAAAAAA"]],
        );
        let regions = doc.mirror_x_regions();
        assert_eq!(regions.len(), 2, "diagonal neighbours do not connect");
        // A tie on cell count breaks on the lower corner, so the order is total and deterministic.
        assert_eq!(regions[0].min, [0, 0, 0]);
        assert_eq!(regions[1].min, [1, 1, 0]);
    }

    /// The direction-only report carries no viewpoint answer at all — `--from` is what asks for one.
    #[test]
    fn exposure_without_a_view_has_no_game_answer() {
        let doc = model([1, 1, 1], &[('E', Some(2.0))], &[&["E"]]);
        let report = doc.exposure(ExposureView::Any);
        assert_eq!(report.emissive[0].game, None);
    }

    /// A lone emissive cell exposes all six faces, but only the three whose normals point at the
    /// camera are counted — and none of them is occluded, because every ray leaves the 1×1×1 grid
    /// on its first step (the DDA's immediate-exit case, from a face on the grid boundary).
    #[test]
    fn game_view_counts_only_camera_facing_faces() {
        let doc = model([1, 1, 1], &[('E', Some(2.0))], &[&["E"]]);
        let report = doc.exposure(ExposureView::Game);
        let entry = &report.emissive[0];
        assert_eq!(entry.total(), 6, "a lone cell exposes every face");
        let game = entry.game.expect("the game view was requested");
        // +X, +Y and -Z face a camera that sits up, forward of the authored front, and slightly +X.
        assert_eq!(game.facing, 3);
        assert_eq!(game.occluded, 0);
    }

    /// A recessed emissive panel: exposed, facing the camera, and hidden behind its own lintel at
    /// the gameplay pitch — the defect the direction histogram scores as perfect. Shallowing the
    /// recess is what fixes it, and the report says so.
    #[test]
    fn game_view_sees_a_deep_recess_as_occluded_and_a_shallow_one_as_clear() {
        let deep = recess_model(4).exposure(ExposureView::Game);
        let deep = deep.emissive[0].game.expect("the game view was requested");
        assert_eq!(
            deep.facing, 1,
            "only the -Z face of the panel is exposed and facing"
        );
        assert_eq!(
            deep.occluded, 1,
            "four cells deep, the lintel is in the way"
        );

        let shallow = recess_model(1).exposure(ExposureView::Game);
        let shallow = shallow.emissive[0]
            .game
            .expect("the game view was requested");
        assert_eq!(shallow.facing, 1, "the same face, the same direction");
        assert_eq!(
            shallow.occluded, 0,
            "one cell deep, the ray clears the lintel"
        );
    }

    /// A wall with a window opening, an emissive panel recessed `depth` cells behind it, and the
    /// lintel above: `z = depth` is the solid back wall carrying the panel at its foot, `z < depth`
    /// is open below `OPENING` and solid above it.
    fn recess_model(depth: u32) -> VoxelDocument {
        const SX: u32 = 6;
        const SY: u32 = 10;
        /// The window opening's height in cells; the lintel starts here.
        const OPENING: u32 = 6;
        let sz = depth + 1;
        let slices = (0..SY)
            .map(|y| {
                (0..sz)
                    .map(|z| {
                        (0..SX)
                            .map(|x| match () {
                                _ if z == sz - 1 => {
                                    if x == 2 && y == 0 {
                                        'E'
                                    } else {
                                        'S'
                                    }
                                }
                                _ if y >= OPENING => 'S',
                                _ => EMPTY_GLYPH,
                            })
                            .collect()
                    })
                    .collect()
            })
            .collect();
        let doc = VoxelDocument {
            size: [SX, SY, sz],
            palette: vec![
                PaletteEntry {
                    glyph: 'E'.to_string(),
                    color: [1.0, 1.0, 1.0, 1.0],
                    emissive: Some(2.0),
                    metallic: None,
                    roughness: None,
                },
                PaletteEntry {
                    glyph: 'S'.to_string(),
                    color: [1.0, 1.0, 1.0, 1.0],
                    emissive: None,
                    metallic: None,
                    roughness: None,
                },
            ],
            slices,
            ..Default::default()
        };
        assert!(validate(&doc).is_empty(), "fixture is not a valid model");
        doc
    }

    /// A slice renders the authored rows with a units ruler, z indices, and front/back markers.
    #[test]
    fn render_slice_annotates_a_narrow_layer() {
        // 3×1×2: one layer, two z rows.
        let doc = parse(
            "format: 1\nkind: voxel\nsize:\n  - 3\n  - 1\n  - 2\npalette:\n  A:\n    color:\n      \
             - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\nslices:\n  -\n    - AAA\n    - A.A\n",
        )
        .expect("voxel parses");
        let slice = doc.render_slice(0).expect("y=0 is in range");
        assert!(slice.contains("012"), "units ruler: {slice}");
        assert!(
            slice.contains("z=0 (front, world -Z)"),
            "front marker: {slice}"
        );
        assert!(slice.contains("z=1 (back)"), "back marker: {slice}");
        assert!(slice.contains("0  AAA"), "row 0: {slice}");
        assert!(slice.contains("1  A.A"), "row 1: {slice}");
        // A width-3 model has no tens ruler line — the very first line is the units ruler.
        assert_eq!(slice.lines().next().unwrap().trim(), "012");
    }

    /// A slice wider than ten columns gains a tens ruler line above the units line.
    #[test]
    fn render_slice_adds_a_tens_ruler_over_wide_layers() {
        // 12×1×1: columns 10 and 11 need the tens digit.
        let doc = parse(
            "format: 1\nkind: voxel\nsize:\n  - 12\n  - 1\n  - 1\npalette:\n  A:\n    color:\n      \
             - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\nslices:\n  -\n    - AAAAAAAAAAAA\n",
        )
        .expect("voxel parses");
        let slice = doc.render_slice(0).expect("y=0 is in range");
        let lines: Vec<&str> = slice.lines().collect();
        // First line is the tens ruler (blank cols 0-9, then '1' at 10 and 11); second is the units.
        assert!(
            lines[0].trim_end().ends_with("11"),
            "tens ruler: {:?}",
            lines[0]
        );
        assert!(
            lines[1].contains("012345678901"),
            "units ruler: {:?}",
            lines[1]
        );
    }

    /// An out-of-range slice y is a readable error naming the valid range.
    #[test]
    fn render_slice_rejects_out_of_range_y() {
        let doc = parse(
            "format: 1\nkind: voxel\nsize:\n  - 1\n  - 2\n  - 1\npalette:\n  A:\n    color:\n      \
             - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\nslices:\n  -\n    - A\n  -\n    - A\n",
        )
        .expect("voxel parses");
        let error = doc.render_slice(5).expect_err("y=5 is out of range");
        assert_eq!(error.code, code::VOXEL_VIEW_OUT_OF_RANGE);
        let help = error.help.as_deref().unwrap();
        assert!(help.contains("0..=1"), "help names the range: {help}");
    }

    /// A column profile lists each y bottom-up and summarizes the filled levels as run-length ranges.
    #[test]
    fn render_column_profiles_with_gaps() {
        // 1×5×1 column: filled at y=0,1,2 and y=4 (gap at y=3).
        let doc = parse(
            "format: 1\nkind: voxel\nsize:\n  - 1\n  - 5\n  - 1\npalette:\n  G:\n    color:\n      \
             - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\nslices:\n  -\n    - G\n  -\n    - G\n  \
             -\n    - G\n  -\n    - .\n  -\n    - G\n",
        )
        .expect("voxel parses");
        let column = doc.render_column(0, 0).expect("(0,0) is in range");
        assert!(column.contains("y=0: 'G'"), "{column}");
        assert!(column.contains("y=3: ."), "{column}");
        assert!(column.contains("y=4: 'G'"), "{column}");
        assert!(
            column.contains("filled y: 0-2,4"),
            "run-length summary: {column}"
        );
    }

    /// An empty column summarizes as `none`; an out-of-range column is a readable error.
    #[test]
    fn render_column_empty_and_out_of_range() {
        let doc = parse(
            "format: 1\nkind: voxel\nsize:\n  - 2\n  - 1\n  - 1\npalette:\n  A:\n    color:\n      \
             - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\nslices:\n  -\n    - A.\n",
        )
        .expect("voxel parses");
        // x=1 is empty in the only layer.
        let column = doc.render_column(1, 0).expect("(1,0) is in range");
        assert!(column.contains("filled y: none"), "{column}");
        let error = doc.render_column(9, 0).expect_err("x=9 is out of range");
        assert_eq!(error.code, code::VOXEL_VIEW_OUT_OF_RANGE);
        let help = error.help.as_deref().unwrap();
        assert!(help.contains("0..=1"), "help names x range: {help}");
    }

    /// Cells that touch only diagonally (share an edge/corner, not a face) are separate components —
    /// 6-connectivity, not 26-connectivity.
    #[test]
    fn diagonal_touch_is_two_components() {
        // A 2×1×2 model with A at (0,0,0) and (1,0,1): they touch only at a corner.
        let doc = parse(
            "format: 1\nkind: voxel\nsize:\n  - 2\n  - 1\n  - 2\npalette:\n  A:\n    color:\n      \
             - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\nslices:\n  -\n    - \"A.\"\n    - \".A\"\n",
        )
        .expect("voxel parses");
        assert!(validate(&doc).is_empty());
        assert_eq!(doc.component_count(), 2);
        assert!(connectivity_warning(&doc).is_some());
    }

    // --- Render-time palette overrides ---------------------------------------------------------

    /// A two-glyph model whose `S` entry authors every material scalar and whose `G` entry authors
    /// none — so both the replace and the create path have a subject.
    const PALETTE_MODEL: &str = "\
format: 1
kind: voxel
size:
  - 2
  - 1
  - 1
palette:
  G:
    color:
      - 0.1
      - 0.2
      - 0.3
      - 1.0
  S:
    color:
      - 0.4
      - 0.5
      - 0.6
      - 1.0
    emissive: 1.0
    metallic: 0.25
    roughness: 0.75
slices:
  -
    - GS
";

    /// Applies one override spec to [`PALETTE_MODEL`].
    fn palette_set(spec: &str) -> Result<(VoxelDocument, Vec<AppliedOverride>), Vec<Diagnostic>> {
        let mut doc = parse(PALETTE_MODEL).expect("the palette model parses");
        let over = SceneOverride::parse(spec).map_err(|d| vec![d])?;
        let applied = apply_palette_overrides(&mut doc, &[over])?;
        Ok((doc, applied))
    }

    /// Every supported pointer shape writes its own typed field and echoes what it replaced.
    #[test]
    fn every_palette_pointer_shape_writes_its_field() {
        let (doc, applied) = palette_set("/palette/G/color/1=0.75").unwrap();
        assert_eq!(applied[0].from, Some(serde_json::json!(0.2)));
        assert_eq!(doc.palette[0].color, [0.1, 0.75, 0.3, 1.0]);

        let (doc, applied) = palette_set("/palette/S/color/3=0.5").unwrap();
        assert_eq!(applied[0].from, Some(serde_json::json!(1.0)));
        assert_eq!(doc.palette[1].color[3], 0.5);

        for (spec, expected) in [
            ("/palette/S/emissive=2.2", 2.2),
            ("/palette/S/metallic=0.0", 0.0),
            ("/palette/S/roughness=1", 1.0),
        ] {
            let (doc, applied) = palette_set(spec).unwrap();
            assert!(
                applied[0].from.is_some(),
                "{spec} replaced an authored value"
            );
            let entry = &doc.palette[1];
            let written = match spec {
                s if s.contains("emissive") => entry.emissive,
                s if s.contains("metallic") => entry.metallic,
                _ => entry.roughness,
            };
            assert_eq!(written, Some(expected), "{spec}");
        }
    }

    /// A scalar the entry left unset is created, and the echo reports `from: None` so a caller can
    /// print `unset -> <value>` — the same contract the scene override layer offers.
    #[test]
    fn an_unset_scalar_is_created() {
        let (doc, applied) = palette_set("/palette/G/emissive=2.2").unwrap();
        assert_eq!(applied[0].from, None);
        assert_eq!(applied[0].to, serde_json::json!(2.2));
        assert_eq!(doc.palette[0].emissive, Some(2.2));
        assert_eq!(
            doc.palette[0].color,
            [0.1, 0.2, 0.3, 1.0],
            "creating one field leaves the rest of the entry alone",
        );
    }

    /// An unknown glyph is an `L4008` that lists the palette, and calls out a mere case difference.
    #[test]
    fn an_unknown_glyph_is_rejected_with_the_palette() {
        let diagnostics = palette_set("/palette/Q/emissive=1.0").unwrap_err();
        assert_eq!(diagnostics[0].code.as_str(), "L4008");
        let help = diagnostics[0].help.as_deref().unwrap();
        assert!(help.contains('G') && help.contains('S'), "{help}");
        assert!(
            !help.contains("did you mean"),
            "no near miss to name: {help}"
        );

        let near = palette_set("/palette/g/emissive=1.0").unwrap_err();
        let help = near[0].help.as_deref().unwrap();
        assert!(help.contains("did you mean `G`"), "{help}");
    }

    /// A color has four channels, so `4` addresses nothing.
    #[test]
    fn a_color_channel_past_three_is_rejected() {
        let diagnostics = palette_set("/palette/G/color/4=0.5").unwrap_err();
        assert_eq!(diagnostics[0].code.as_str(), "L4008");
        assert!(
            diagnostics[0].message.contains("four channels"),
            "{}",
            diagnostics[0].message
        );
    }

    /// Everything outside the palette domain is refused, and the help states the whole domain.
    #[test]
    fn pointers_outside_the_palette_are_refused() {
        for spec in [
            "/size/0=4",
            "/slices/0/0=..",
            "/name=other",
            "/palette/G=1",
            "/palette/G/color=1",
            "/palette/G/color/0/0=1",
            "/palette/G/opacity=1",
        ] {
            let diagnostics = palette_set(spec).unwrap_err();
            assert_eq!(diagnostics[0].code.as_str(), "L4008", "{spec}");
        }
        let help = palette_set("/size/0=4").unwrap_err()[0]
            .help
            .clone()
            .unwrap();
        assert!(help.contains("/palette/<glyph>/emissive"), "{help}");
    }

    /// A palette field is a number; anything else is refused rather than coerced.
    #[test]
    fn a_non_numeric_value_is_refused() {
        for spec in [
            "/palette/G/emissive=bright",
            "/palette/G/color/0=null",
            "/palette/G/metallic=true",
        ] {
            let diagnostics = palette_set(spec).unwrap_err();
            assert_eq!(diagnostics[0].code.as_str(), "L4008", "{spec}");
            assert!(
                diagnostics[0].message.contains("expects a number"),
                "{spec}: {}",
                diagnostics[0].message
            );
        }
    }

    /// A glyph that RFC 6901 escapes (`/` and `~` are both legal palette glyphs) round-trips.
    #[test]
    fn escaped_glyphs_resolve() {
        let source = PALETTE_MODEL
            .replace("  G:", "  /:")
            .replace("    - GS", "    - /S");
        let mut doc = parse(&source).expect("the escaped-glyph model parses");
        assert!(validate(&doc).is_empty());
        let over = SceneOverride::parse("/palette/~1/emissive=3.0").unwrap();
        apply_palette_overrides(&mut doc, &[over]).unwrap();
        let entry = doc.palette.iter().find(|e| e.glyph == "/").unwrap();
        assert_eq!(entry.emissive, Some(3.0));
    }

    /// Every failure comes back in one pass, and later overrides see earlier ones.
    #[test]
    fn overrides_batch_and_compose() {
        let mut doc = parse(PALETTE_MODEL).expect("the palette model parses");
        let bad = [
            SceneOverride::parse("/palette/Q/emissive=1").unwrap(),
            SceneOverride::parse("/size/0=9").unwrap(),
        ];
        assert_eq!(
            apply_palette_overrides(&mut doc, &bad).unwrap_err().len(),
            2
        );

        let chained = [
            SceneOverride::parse("/palette/G/emissive=1.0").unwrap(),
            SceneOverride::parse("/palette/G/emissive=2.0").unwrap(),
        ];
        let applied = apply_palette_overrides(&mut doc, &chained).unwrap();
        assert_eq!(applied[1].from, Some(serde_json::json!(1.0)));
        assert_eq!(doc.palette[0].emissive, Some(2.0));
    }

    /// The same two-glyph palette as [`PALETTE_MODEL`], owned by an assembly instead: the applier
    /// is shared, so the assembly tests only have to prove the palette it reaches is this one.
    const PALETTE_ASSEMBLY: &str = "\
format: 1
kind: assembly
size:
  - 2
  - 1
  - 1
palette:
  G:
    color:
      - 0.1
      - 0.2
      - 0.3
      - 1.0
  S:
    color:
      - 0.4
      - 0.5
      - 0.6
      - 1.0
    emissive: 1.0
parts:
  - name: body
    file: parts/body.part.yaml
    place:
      - 0
      - 0
      - 0
";

    /// Applies one override spec to [`PALETTE_ASSEMBLY`].
    fn assembly_palette_set(
        spec: &str,
    ) -> Result<(crate::assembly::AssemblyDocument, Vec<AppliedOverride>), Vec<Diagnostic>> {
        let mut doc =
            crate::assembly::parse(PALETTE_ASSEMBLY).expect("the palette assembly parses");
        let over = SceneOverride::parse(spec).map_err(|d| vec![d])?;
        let applied = apply_assembly_palette_overrides(&mut doc, &[over])?;
        Ok((doc, applied))
    }

    /// An assembly's palette takes the same writes as a voxel document's, and echoes them the same.
    #[test]
    fn an_assembly_palette_takes_the_same_writes() {
        let (doc, applied) = assembly_palette_set("/palette/G/color/1=0.75").unwrap();
        assert_eq!(applied[0].from, Some(serde_json::json!(0.2)));
        assert_eq!(doc.palette[0].color, [0.1, 0.75, 0.3, 1.0]);

        let (doc, applied) = assembly_palette_set("/palette/S/emissive=2.2").unwrap();
        assert_eq!(applied[0].from, Some(serde_json::json!(1.0)));
        assert_eq!(doc.palette[1].emissive, Some(2.2));

        let (doc, applied) = assembly_palette_set("/palette/G/emissive=2.2").unwrap();
        assert_eq!(applied[0].from, None, "`emissive` was not in the file");
        assert_eq!(doc.palette[0].emissive, Some(2.2));
        assert_eq!(
            doc.parts.len(),
            1,
            "an override touches the palette and nothing else",
        );
    }

    /// A glyph the assembly's palette does not have is the same `L4008`, listing the same palette.
    #[test]
    fn an_unknown_assembly_glyph_is_rejected_with_the_palette() {
        let diagnostics = assembly_palette_set("/palette/Q/emissive=1.0").unwrap_err();
        assert_eq!(diagnostics[0].code.as_str(), "L4008");
        let help = diagnostics[0].help.as_deref().unwrap();
        assert!(help.contains('G') && help.contains('S'), "{help}");
    }

    /// A value the kind's validator rejects comes back as that kind's own error, at the field.
    #[test]
    fn an_out_of_range_palette_override_is_the_kinds_data_error() {
        let diagnostics = palette_set("/palette/G/roughness=2").unwrap_err();
        assert_eq!(diagnostics[0].code.as_str(), "L1114");
        assert_eq!(diagnostics[0].path.as_deref(), Some("/palette/G/roughness"));

        let diagnostics = assembly_palette_set("/palette/G/roughness=2").unwrap_err();
        assert_eq!(diagnostics[0].code.as_str(), "L1138");
        assert_eq!(diagnostics[0].path.as_deref(), Some("/palette/G/roughness"));
    }

    /// An assembly's own keys are outside the domain too: `parts` is the model's *shape*.
    #[test]
    fn pointers_outside_an_assembly_palette_are_refused() {
        for spec in ["/parts/0/place/0=4", "/size/0=4", "/palette/G/pivot=1"] {
            let diagnostics = assembly_palette_set(spec).unwrap_err();
            assert_eq!(diagnostics[0].code.as_str(), "L4008", "{spec}");
            let help = diagnostics[0].help.as_deref().unwrap();
            assert!(help.contains("/palette/<glyph>/emissive"), "{spec}: {help}");
        }
    }
}
