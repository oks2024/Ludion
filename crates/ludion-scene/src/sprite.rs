//! Sprite documents: `sprites/*.sprite.yaml` — one strict-YAML document per sprite
//! (`Plan/sprite-engine.md`, "Data and bake").
//!
//! A sprite document is the 2D sibling of [`crate::voxel`]: a `size` in texels, a `palette` mapping
//! single printable-ASCII glyphs to linear RGBA colors, and `frames` — an *ordered* mapping of frame
//! name to a grid of row strings, every frame the document's one `size`. A differently sized pose is
//! a different document. `.` is transparent and can never name a palette entry
//! ([`SPRITE_EMPTY_GLYPH`]).
//!
//! On top of the grid sits `effects:`, an ordered list of enumerated ops (`gradient`, `outline`,
//! `shade`, `shadow`, `dither`, `noise`, `blur`, `glow`) baked on the CPU in list order. The op set is
//! closed by design — no expressions, no node graph (`engine-pillars.md`: Rust is the only
//! Turing-complete layer) — so an unrecognized op is a diagnostic, never a skipped entry. Nothing
//! here bakes: this module owns the *text*, and the pixels are a separate crate's job against these
//! types.
//!
//! The split is the crate's usual one: [`parse`] checks structure and shape (`L1153`), [`validate`]
//! checks semantics (`L1153` for the document, `L1154` for anything under `/effects`), and [`emit`]
//! writes canonical text. **Canonical means minimal**: a field whose value equals its default is not
//! written, so `emit(parse(text)) == text` holds for canonical input and
//! `emit(parse(emit(d))) == emit(d)` holds for every document.

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};
use ludion_core::suggest;
use serde_json::Value;

use crate::envelope;
use crate::fields::parse_number;
use crate::scalar::{format_float, scalar_string};
use crate::yaml::{self, MapEntry, Node, NodeValue, pointer_token};

/// The sprite document format version this crate reads and writes.
pub const SPRITE_FORMAT_VERSION: u64 = 1;

/// The reserved glyph meaning "transparent texel". It can never name a palette entry — the
/// `crate::voxel` convention, so a sprite grid reads exactly like a voxel slice.
pub const SPRITE_EMPTY_GLYPH: char = '.';

/// The largest `size` component a sprite document may declare, per axis. A sprite is one region of
/// a packed atlas, and a frame wider than this stops being a sprite and becomes a texture.
pub const SPRITE_MAX_SIZE: u32 = 1024;

/// How many distinct colours a sprite document can name: printable ASCII `0x21..=0x7E` (94
/// characters) minus the reserved [`SPRITE_EMPTY_GLYPH`].
///
/// This is the ceiling the PNG → sprite-document importer measures a source image against: past it
/// the import refuses with [`code::SPRITE_IMPORT_NEEDS_QUANTIZE`] rather than inventing a second
/// glyph class, and a `--quantize` run that merges colours to fit reports the loss as
/// [`code::SPRITE_IMPORT_QUANTIZED`].
pub const SPRITE_GLYPH_BUDGET: usize = 93;

const SPRITE_KEYS: [&str; 8] = [
    "format", "kind", "name", "size", "seed", "palette", "frames", "effects",
];

/// The palette-entry keys this format recognizes, in the order suggestions should prefer.
const PALETTE_ENTRY_KEYS: [&str; 2] = ["color", "emissive"];

/// The keys a `palette:` include block recognizes: the referenced palette document and the local
/// overrides applied on top of it.
const PALETTE_INCLUDE_KEYS: [&str; 2] = ["from", "overrides"];

/// The frame keys this format recognizes, in the order suggestions should prefer.
const FRAME_KEYS: [&str; 5] = ["base", "mirror", "patches", "pivot", "rows"];
const PATCH_KEYS: [&str; 2] = ["at", "rows"];

/// Every effect `op` name, in the order the format documents them.
const EFFECT_OPS: [&str; 8] = [
    "gradient", "outline", "shade", "shadow", "dither", "noise", "blur", "glow",
];

const GRADIENT_KEYS: [&str; 7] = ["op", "glyphs", "frames", "from", "to", "direction", "blend"];
const OUTLINE_KEYS: [&str; 5] = ["op", "glyphs", "frames", "color", "side"];
const SHADE_KEYS: [&str; 5] = ["op", "glyphs", "frames", "light", "strength"];
const SHADOW_KEYS: [&str; 5] = ["op", "glyphs", "frames", "offset", "color"];
const DITHER_KEYS: [&str; 4] = ["op", "glyphs", "frames", "density"];
const NOISE_KEYS: [&str; 4] = ["op", "glyphs", "frames", "amount"];
const BLUR_KEYS: [&str; 4] = ["op", "glyphs", "frames", "radius"];
const GLOW_KEYS: [&str; 6] = ["op", "glyphs", "frames", "color", "radius_px", "strength"];

/// The `gradient` op's default `direction`, omitted by [`emit`] when it is the authored value.
const GRADIENT_DIRECTION_DEFAULT: GradientDirection = GradientDirection::Vertical;
/// The `gradient` op's default `blend`.
const GRADIENT_BLEND_DEFAULT: GradientBlend = GradientBlend::Replace;
/// The `outline` op's default `side`.
const OUTLINE_SIDE_DEFAULT: OutlineSide = OutlineSide::Outside;
/// The `shade` op's default light direction, in texel space (`+x` right, `+y` down).
const SHADE_LIGHT_DEFAULT: [f64; 2] = [-1.0, -1.0];
/// The `shade` op's default strength.
const SHADE_STRENGTH_DEFAULT: f64 = 0.25;
/// The `shadow` op's default texel offset.
const SHADOW_OFFSET_DEFAULT: [i32; 2] = [1, 1];
/// The `shadow` op's default color: half-opaque black.
const SHADOW_COLOR_DEFAULT: [f64; 4] = [0.0, 0.0, 0.0, 0.5];
/// The `dither` op's default density.
const DITHER_DENSITY_DEFAULT: f64 = 0.5;
/// The inclusive `blur` radius range, in texels.
const BLUR_RADIUS_RANGE: (u32, u32) = (1, 8);
/// The largest `glow` halo radius, in texels. Fractional and unbounded below by an integer floor —
/// the point of the op — but capped so a halo wider than a large sprite is authored as its own
/// sprite rather than a runaway radius, the same rule `blur` states for its integer ceiling.
const GLOW_RADIUS_MAX: f64 = 32.0;

/// One palette entry: a glyph and the linear RGBA color it paints.
///
/// `glyph` is stored exactly as authored — a single printable-ASCII character in a valid document,
/// but held as a `String` so `validate` can diagnose a multi-character, non-ASCII, or reserved key
/// rather than silently dropping it (the [`crate::voxel::PaletteEntry`] convention). Colors are
/// `f64` because text round-trips losslessly at that width; the renderer narrows to `f32`.
#[derive(Clone, Debug, PartialEq)]
pub struct SpritePaletteEntry {
    /// The palette key exactly as authored. Valid keys are one printable-ASCII glyph
    /// (`0x21..=0x7E`, excluding the reserved [`SPRITE_EMPTY_GLYPH`]); `validate` reports anything
    /// else as `L1153`.
    pub glyph: String,
    /// Linear RGBA color, four finite components in `0..=1`.
    pub color: [f64; 4],
    /// HDR emissive intensity, an optional finite multiplier (`>= 0`) on this entry's own color.
    /// `None` means the texel is not emissive. It lives in the palette rather than on a frame so an
    /// emissive map can bake from the same source the color map does.
    pub emissive: Option<f64>,
}

/// A shared-palette include: a reference to a standalone `kind: palette` document, plus the local
/// overrides applied on top of it.
///
/// A sprite document authors *either* an inline `palette:` map (the entries live in
/// [`SpriteDocument::palette`]) *or* an include (`palette: { from, overrides }`, held here with
/// [`SpriteDocument::palette`] left empty). The include is resolved at bake/generate time — the
/// referenced palette with these [`overrides`](Self::overrides) applied on top produces exactly the
/// palette an equivalent inline document would, so the two bake to byte-identical images. See
/// [`crate::resolve_sprite_include`].
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SpritePaletteInclude {
    /// The referenced `kind: palette` document, as a path relative to this sprite document's own
    /// directory. This crate does no IO — the caller reads it and hands the text to
    /// [`crate::resolve_sprite_include`].
    pub from: String,
    /// The palette entries applied on top of the referenced palette, in canonical (glyph-sorted)
    /// order: a glyph the referenced palette defines is replaced, a glyph it does not is added. Empty
    /// when the include takes the referenced palette unchanged.
    pub overrides: Vec<SpritePaletteEntry>,
}

/// One named frame: a grid of row strings, one row per texel line, top-down.
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteFrame {
    /// The frame's name, unique within the document and free of `/` — an atlas region is named
    /// `<document name>/<frame name>`, so a `/` here would make the region name ambiguous.
    pub name: String,
    /// The rotation/anchor point in texels from the frame's top-left, `[px, py]`. `None` means the
    /// frame's center, [`SpriteDocument::default_pivot`] — never written by `emit` when absent, so
    /// "centered" stays one spelling.
    pub pivot: Option<[f64; 2]>,
    /// How the frame's grid is authored; [`SpriteDocument::frame_rows`] resolves it to the full grid.
    pub grid: SpriteFrameGrid,
}

/// How a frame authors its glyph grid.
#[derive(Clone, Debug, PartialEq)]
pub enum SpriteFrameGrid {
    /// Authored rows. Without `mirror`: exactly `size[1]` rows of exactly `size[0]` characters, each
    /// [`SPRITE_EMPTY_GLYPH`] or a palette glyph.
    Rows {
        /// One string per texel row, top-down. With `mirror`, only the kept half: the top rows
        /// and/or the left columns, the centre line included when the extent is odd.
        rows: Vec<String>,
        /// The axes the authored half is reflected across to make the full grid.
        mirror: Option<SpriteMirror>,
    },
    /// A grid made by writing patches over another frame's resolved grid.
    Patched {
        /// The name of a [`SpriteFrameGrid::Rows`] frame; patches never chain.
        base: String,
        /// The patches, applied in order over the base's full (expanded) grid.
        patches: Vec<SpritePatch>,
    },
}

/// The axes a `mirror:` frame reflects its authored half across.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpriteMirror {
    /// Left-right: each row is the left half, `ceil(width / 2)` characters.
    X,
    /// Top-bottom: the rows are the top half, `ceil(height / 2)` rows.
    Y,
    /// Both axes: the top-left quarter.
    Xy,
}

impl SpriteMirror {
    /// The keyword the document spells this axis set as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::X => "x",
            Self::Y => "y",
            Self::Xy => "xy",
        }
    }

    fn flips_x(self) -> bool {
        matches!(self, Self::X | Self::Xy)
    }

    fn flips_y(self) -> bool {
        matches!(self, Self::Y | Self::Xy)
    }
}

/// One rectangle of glyphs written over a base frame.
#[derive(Clone, Debug, PartialEq)]
pub struct SpritePatch {
    /// The patch's top-left texel in the base's full coordinates, `[x, y]`.
    pub at: [u32; 2],
    /// The patch rows: a space keeps the base texel, `.` writes transparent, any other glyph writes
    /// that glyph.
    pub rows: Vec<String>,
}

/// `half ++ reverse(half[..len - odd])`: the reflection that shares the centre line when the full
/// extent is odd. Keyed on the document's size parity, so a 1-wide `x` mirror is the identity.
fn reflect<T: Clone>(half: &[T], odd: bool) -> Vec<T> {
    let keep = half.len().saturating_sub(usize::from(odd));
    let mut out = half.to_vec();
    out.extend(half[..keep].iter().rev().cloned());
    out
}

fn expand_mirror(rows: &[String], mirror: SpriteMirror, size: [u32; 2]) -> Vec<String> {
    let mut rows: Vec<String> = rows.to_vec();
    if mirror.flips_x() {
        let odd = size[0] % 2 == 1;
        rows = rows
            .iter()
            .map(|row| {
                reflect(&row.chars().collect::<Vec<_>>(), odd)
                    .into_iter()
                    .collect()
            })
            .collect();
    }
    if mirror.flips_y() {
        rows = reflect(&rows, size[1] % 2 == 1);
    }
    rows
}

/// Writes each patch over `rows` by character index; a space keeps the texel, and anything that
/// would land outside `size` is skipped.
fn apply_patches(rows: Vec<String>, patches: &[SpritePatch], size: [u32; 2]) -> Vec<String> {
    let mut grid: Vec<Vec<char>> = rows.iter().map(|row| row.chars().collect()).collect();
    for patch in patches {
        for (j, patch_row) in patch.rows.iter().enumerate() {
            let y = patch.at[1] as usize + j;
            if y >= size[1] as usize {
                break;
            }
            let Some(row) = grid.get_mut(y) else { continue };
            for (i, ch) in patch_row.chars().enumerate() {
                let x = patch.at[0] as usize + i;
                if x >= size[0] as usize {
                    break;
                }
                if ch != ' ' && x < row.len() {
                    row[x] = ch;
                }
            }
        }
    }
    grid.into_iter()
        .map(|row| row.into_iter().collect())
        .collect()
}

/// Which way a `gradient` op ramps across a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GradientDirection {
    /// Top to bottom (`from` at `y = 0`).
    #[default]
    Vertical,
    /// Left to right (`from` at `x = 0`).
    Horizontal,
    /// Center outward (`from` at the frame's center).
    Radial,
}

/// How a `gradient` op combines with the texels it covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GradientBlend {
    /// The ramp replaces the covered texel's color.
    #[default]
    Replace,
    /// The ramp multiplies the covered texel's color.
    Multiply,
}

/// Which side of the silhouette an `outline` op paints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OutlineSide {
    /// One texel out from the silhouette, into transparent texels.
    #[default]
    Outside,
    /// One texel in from the silhouette, over the sprite's own edge texels.
    Inside,
}

/// One effect op and its parameters. The set is closed: a document naming anything else is `L1154`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SpriteEffectOp {
    /// A linear or radial ramp between two linear RGBA colors, in texel coordinates.
    Gradient {
        /// The color at the ramp's start.
        from: [f64; 4],
        /// The color at the ramp's end.
        to: [f64; 4],
        /// Which way the ramp runs.
        direction: GradientDirection,
        /// How the ramp combines with what it covers.
        blend: GradientBlend,
    },
    /// A one-texel border along the silhouette.
    Outline {
        /// The border's linear RGBA color.
        color: [f64; 4],
        /// Which side of the silhouette the border sits on.
        side: OutlineSide,
    },
    /// Darkening and lightening derived from the silhouette, as if lit from one direction.
    Shade {
        /// The light direction in texel space (`+x` right, `+y` down); never `[0, 0]`.
        light: [f64; 2],
        /// How far the shading pushes, in `0..=1`.
        strength: f64,
    },
    /// An offset copy of the silhouette painted underneath it.
    Shadow {
        /// The whole-texel offset the copy sits at; never `[0, 0]`.
        offset: [i32; 2],
        /// The copy's linear RGBA color.
        color: [f64; 4],
    },
    /// An ordered two-tone checker between the effect's exactly-two glyphs.
    Dither {
        /// The share of texels taking the second glyph's tone, in `0..=1`.
        density: f64,
    },
    /// Seeded per-texel color jitter, deterministic from the document's `seed`.
    Noise {
        /// How far a texel may jitter, in `0..=1`.
        amount: f64,
    },
    /// A box blur — the glow primitive.
    Blur {
        /// The box radius in texels, in `1..=8`.
        radius: u32,
    },
    /// A soft halo grown *outside* the silhouette, leaving the opaque core untouched.
    ///
    /// Every transparent texel within `radius_px` of an opaque source texel gains alpha from a
    /// parabolic falloff, so a small bright sprite reads as a bright core wrapped in a soft ring —
    /// what `blur`'s integer `1` floor destroys on a tiny frame by dissolving the core.
    Glow {
        /// The halo tint. `None` derives it per texel from the contributing source texels' own
        /// colours (a red core glows red); `Some` forces one linear RGBA tint on the whole halo.
        color: Option<[f64; 4]>,
        /// The halo reach in texels — a finite `f64` in `0 < radius_px <= 32`, fractional by design
        /// so a halo can be smaller than `blur`'s one-texel box.
        radius_px: f64,
        /// The multiplier on the accumulated halo coverage (`>= 0`, finite). The resulting alpha is
        /// clamped to `0..=1`, so a value above 1 pushes the near-core halo toward opaque.
        strength: f64,
    },
}

/// One entry of a document's `effects:` list: an op plus the glyph and frame filters that scope it.
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteEffect {
    /// The op and its parameters.
    pub op: SpriteEffectOp,
    /// The palette glyphs this op touches. `None` means the whole frame. Required, and exactly two
    /// entries, for [`SpriteEffectOp::Dither`].
    pub glyphs: Option<Vec<String>>,
    /// The frames this op applies to. `None` means every frame.
    pub frames: Option<Vec<String>>,
}

/// A parsed sprite document.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SpriteDocument {
    /// Optional human-readable sprite name. The atlas bake falls back to the file stem when it is
    /// absent, which is why this stays an `Option` rather than being resolved here — this crate
    /// does no IO and does not know the file's name.
    pub name: Option<String>,
    /// The frame extent in texels: `[width, height]`. Every frame is exactly this size.
    pub size: [u32; 2],
    /// The seed the `noise` op draws from (`dither` is an ordered Bayer pattern and takes none).
    /// `None` bakes as `0` — the deterministic default, so an unseeded document still bakes to one
    /// fixed image.
    pub seed: Option<u64>,
    /// The inline palette, sorted by glyph (the canonical order `emit` writes). Empty when the
    /// document authors a [`palette_include`](Self::palette_include) instead; after that include is
    /// resolved ([`crate::resolve_sprite_include`]) this holds the merged palette and the include is
    /// cleared, so a resolved document is an ordinary inline one the bake consumes unchanged.
    pub palette: Vec<SpritePaletteEntry>,
    /// A shared-palette include: `Some` when the document authors `palette: { from, overrides }`
    /// rather than an inline palette map, `None` for an inline palette or an already-resolved include.
    /// Mutually exclusive with a non-empty [`palette`](Self::palette).
    pub palette_include: Option<SpritePaletteInclude>,
    /// The frames in authored order, which is the order the atlas bake mints region names in.
    pub frames: Vec<SpriteFrame>,
    /// The effect list in authored order, which is application order at bake.
    pub effects: Vec<SpriteEffect>,
}

impl SpriteDocument {
    /// The frame named `name`, or `None`.
    ///
    /// # Example
    /// ```
    /// let source = "format: 1\nkind: sprite\nsize:\n  - 2\n  - 1\npalette:\n  A:\n    color:\n\
    ///               \x20     - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\nframes:\n  idle:\n    \
    ///               rows:\n      - \"A.\"\n";
    /// let sprite = ludion_scene::check_sprite(source).expect("the sprite is valid");
    /// assert!(sprite.frame("idle").is_some());
    /// assert!(sprite.frame("walk").is_none());
    /// ```
    pub fn frame(&self, name: &str) -> Option<&SpriteFrame> {
        self.frames.iter().find(|frame| frame.name == name)
    }

    /// The frame's resolved glyph grid as the bake reads it, one string per texel row, top-down:
    /// a `mirror` is expanded and `patches` are applied over their base. Total — a patched frame
    /// whose base is missing or itself patched resolves to no rows (validation reports it).
    ///
    /// # Example
    /// ```
    /// let source = "format: 1\nkind: sprite\nsize:\n  - 2\n  - 1\npalette:\n  A:\n    color:\n\
    ///               \x20     - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\nframes:\n  idle:\n    \
    ///               rows:\n      - \"A.\"\n";
    /// let sprite = ludion_scene::check_sprite(source).expect("the sprite is valid");
    /// let frame = sprite.frame("idle").expect("the frame exists");
    /// assert_eq!(sprite.frame_rows(frame).as_ref(), &["A.".to_string()][..]);
    /// ```
    ///
    /// A mirrored frame authors the kept half:
    /// ```
    /// let source = "format: 1\nkind: sprite\nsize:\n  - 3\n  - 1\npalette:\n  A:\n    color:\n\
    ///               \x20     - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\nframes:\n  idle:\n    \
    ///               mirror: x\n    rows:\n      - \"A.\"\n";
    /// let sprite = ludion_scene::check_sprite(source).expect("the sprite is valid");
    /// let frame = sprite.frame("idle").expect("the frame exists");
    /// assert_eq!(sprite.frame_rows(frame).as_ref(), &["A.A".to_string()][..]);
    /// ```
    pub fn frame_rows<'a>(&'a self, frame: &'a SpriteFrame) -> std::borrow::Cow<'a, [String]> {
        match &frame.grid {
            SpriteFrameGrid::Rows { rows, mirror: None } => std::borrow::Cow::Borrowed(rows),
            SpriteFrameGrid::Rows {
                rows,
                mirror: Some(mirror),
            } => std::borrow::Cow::Owned(expand_mirror(rows, *mirror, self.size)),
            SpriteFrameGrid::Patched { base, patches } => {
                let base_rows = match self.frame(base).map(|frame| &frame.grid) {
                    Some(SpriteFrameGrid::Rows { rows, mirror }) => match mirror {
                        Some(mirror) => expand_mirror(rows, *mirror, self.size),
                        None => rows.clone(),
                    },
                    _ => return std::borrow::Cow::Owned(Vec::new()),
                };
                std::borrow::Cow::Owned(apply_patches(base_rows, patches, self.size))
            }
        }
    }

    /// The frame's grid with a column ruler above (hundreds and tens lines only when the width
    /// needs them, then units) and the row index right-aligned down the left. Every line ends in
    /// `\n` and none ends in a space.
    pub fn render_frame_rows(&self, frame: &SpriteFrame) -> String {
        let [width, height] = self.size;
        let label_w = (height.saturating_sub(1).to_string().len()).max(1);
        let pad = " ".repeat(label_w + 2);
        let digit = |value: u32| char::from_digit(value % 10, 10).unwrap_or('?');
        let ruler = |show: bool, cell: &dyn Fn(u32) -> char| -> String {
            if !show {
                return String::new();
            }
            let line: String = (0..width).map(cell).collect();
            format!("{pad}{}\n", line.trim_end())
        };
        let mut out = String::new();
        out.push_str(&ruler(width > 100, &|c| {
            if c >= 100 { digit(c / 100) } else { ' ' }
        }));
        out.push_str(&ruler(width > 10, &|c| {
            if c >= 10 { digit(c / 10) } else { ' ' }
        }));
        out.push_str(&ruler(true, &|c| digit(c)));
        for (y, row) in self.frame_rows(frame).iter().enumerate() {
            out.push_str(&format!("{y:>label_w$}  {row}\n"));
        }
        out
    }

    /// The palette entry `glyph` names, or `None` for the transparent glyph and for any glyph the
    /// palette does not define.
    pub fn palette_entry(&self, glyph: char) -> Option<&SpritePaletteEntry> {
        self.palette
            .iter()
            .find(|entry| single_glyph(&entry.glyph) == Some(glyph))
    }

    /// The pivot a frame with no authored `pivot` takes: the frame's center, in texels from its
    /// top-left.
    pub fn default_pivot(&self) -> [f64; 2] {
        [f64::from(self.size[0]) / 2.0, f64::from(self.size[1]) / 2.0]
    }
}

/// The single character `key` holds, or `None` when it is empty or longer than one character.
fn single_glyph(key: &str) -> Option<char> {
    let mut chars = key.chars();
    chars.next().filter(|_| chars.next().is_none())
}

/// Whether `ch` may name a palette entry: printable ASCII, excluding the reserved empty glyph.
fn is_palette_glyph(ch: char) -> bool {
    ('\u{21}'..='\u{7e}').contains(&ch) && ch != SPRITE_EMPTY_GLYPH
}

// --- Parse (structure only, `L1153` / `L1154`) --------------------------------------------------

/// Parses strict-subset YAML into a [`SpriteDocument`], checking structure/shape only.
pub(crate) fn parse(source: &str) -> Result<SpriteDocument, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                "the sprite document is empty; a sprite needs at least `format: 1`, \
                 `kind: sprite`, `size`, `palette`, and `frames`",
            )
            .with_path("")
            .with_help(
                "a minimal sprite is:\n  format: 1\n  kind: sprite\n  size:\n    - 1\n    - 1\n  \
                 palette:\n    A:\n      color:\n        - 1.0\n        - 1.0\n        - 1.0\n\
                 \x20       - 1.0\n  frames:\n    idle:\n      rows:\n        - \"A\"",
            ),
        ]);
    };
    let NodeValue::Mapping(entries) = root.value else {
        return Err(vec![
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "the sprite document root must be a mapping, found {} (line {})",
                    root.value.type_phrase(),
                    root.line
                ),
            )
            .with_path("")
            .with_help("start the file with the top-level keys `format:` and `kind: sprite`"),
        ]);
    };

    let mut diagnostics = Vec::new();
    let mut doc = SpriteDocument::default();
    let mut format_seen = false;
    let mut kind_seen = false;
    let mut size_seen = false;
    let mut palette_seen = false;
    let mut frames_seen = false;

    for entry in &entries {
        match entry.key.as_str() {
            "format" => {
                format_seen = true;
                envelope::check_envelope(
                    entry,
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    SPRITE_FORMAT_VERSION,
                    "the sprite document",
                    "sprite",
                    "sprite",
                    &mut diagnostics,
                );
            }
            "kind" => {
                kind_seen = true;
                envelope::check_kind(
                    entry,
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    "sprite",
                    "a sprite document",
                    &mut diagnostics,
                );
            }
            "name" => {
                doc.name = envelope::expect_string(
                    &entry.node,
                    "/name",
                    "the sprite name",
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "size" => {
                size_seen = true;
                if let Some(size) = parse_size(entry, &mut diagnostics) {
                    doc.size = size;
                }
            }
            "seed" => {
                doc.seed = parse_seed(entry, &mut diagnostics);
            }
            "palette" => {
                palette_seen = true;
                parse_palette_field(entry, &mut doc, &mut diagnostics);
            }
            "frames" => {
                frames_seen = true;
                doc.frames = parse_frames(entry, &mut diagnostics);
            }
            "effects" => {
                doc.effects = parse_effects(entry, &mut diagnostics);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "",
                "a sprite document",
                SPRITE_KEYS,
                code::SPRITE_DOCUMENT_STRUCTURE,
            )),
        }
    }

    if !format_seen {
        diagnostics.push(missing_key("format", "add `format: 1` as the first line"));
    }
    if !kind_seen {
        diagnostics.push(missing_key("kind", "add `kind: sprite`"));
    }
    if !size_seen {
        diagnostics.push(missing_key(
            "size",
            "add a `size` of two positive integers: width and height in texels",
        ));
    }
    if !palette_seen {
        diagnostics.push(missing_key(
            "palette",
            "add a `palette` mapping each glyph to a `color`",
        ));
    }
    if !frames_seen {
        diagnostics.push(missing_key(
            "frames",
            "add `frames`: one named frame per pose, each with `rows`",
        ));
    }

    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

fn missing_key(key: &str, help: &str) -> Diagnostic {
    envelope::missing_key(
        key,
        help,
        code::SPRITE_DOCUMENT_STRUCTURE,
        "sprite document",
    )
}

/// Parses `size`: a sequence of exactly two positive integers that fit in `u32`. The upper bound
/// [`SPRITE_MAX_SIZE`] is a semantic concern, checked later by [`validate`].
fn parse_size(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Option<[u32; 2]> {
    let NodeValue::Sequence(items) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "`size` must be a sequence of two positive integers, found {} (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/size")
            .with_help("write `size` as two integers:\n  size:\n    - 16\n    - 12"),
        );
        return None;
    };
    if items.len() != 2 {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "`size` must have exactly 2 entries (width, height), found {}",
                    items.len()
                ),
            )
            .with_path("/size")
            .with_help("supply exactly two positive integers: width and height in texels"),
        );
        return None;
    }
    let mut out = [0u32; 2];
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        let value = match &item.value {
            NodeValue::Scalar(Value::Number(n)) => {
                n.as_u64().filter(|v| *v >= 1 && *v <= u64::from(u32::MAX))
            }
            _ => None,
        };
        match value {
            Some(v) => out[index] = v as u32,
            None => {
                diagnostics.push(
                    Diagnostic::new(
                        code::SPRITE_DOCUMENT_STRUCTURE,
                        format!(
                            "`size[{index}]` must be a positive integer (at least 1), found {} \
                             (line {})",
                            item.value.type_phrase(),
                            item.line
                        ),
                    )
                    .with_path(format!("/size/{index}"))
                    .with_help("each sprite dimension is a positive whole number of texels"),
                );
                ok = false;
            }
        }
    }
    ok.then_some(out)
}

/// Parses `seed`: a non-negative integer that fits in `u64`.
fn parse_seed(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Option<u64> {
    match &entry.node.value {
        NodeValue::Scalar(Value::Number(n)) if n.as_u64().is_some() => n.as_u64(),
        other => {
            diagnostics.push(
                Diagnostic::new(
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    format!(
                        "`seed` must be a non-negative integer, found {} (line {})",
                        other.type_phrase(),
                        entry.node.line
                    ),
                )
                .with_path("/seed")
                .with_help(
                    "write `seed` as one whole number, e.g. `seed: 7`; remove the key to bake the \
                     noise and dither ops from seed 0",
                ),
            );
            None
        }
    }
}

/// Parses the `palette:` field, which is one of two forms: an inline map of glyph keys to
/// `{color, emissive?}` entries (today), or a shared-palette include (`{ from, overrides? }`). The
/// two are told apart by a `from` key — a palette glyph is exactly one printable-ASCII character, so
/// no inline palette can carry a key spelled `from`, and the disambiguation is unambiguous.
fn parse_palette_field(
    entry: &MapEntry,
    doc: &mut SpriteDocument,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let NodeValue::Mapping(entries) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "`palette` must be a mapping of glyphs to colors (or a `from` include), found {} \
                     (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/palette")
            .with_help(
                "write `palette` as glyph keys, each mapping to a `color`:\n  palette:\n    \"#\":\n\
                 \x20     color: \"#ff0000\"\n  or \
                 reference a shared `kind: palette` document:\n  palette:\n    from: shared.palette.yaml",
            ),
        );
        return;
    };
    if entries.iter().any(|sub| sub.key == "from") {
        doc.palette_include = parse_palette_include(entries, diagnostics);
    } else {
        doc.palette = parse_palette_entries_map(
            entries,
            "/palette",
            code::SPRITE_DOCUMENT_STRUCTURE,
            diagnostics,
        );
    }
}

/// Parses a palette mapping — glyph keys to `{color, emissive?}` entries — rooted at `base`, tagging
/// every diagnostic with `code`. Returns the entries sorted by glyph (the canonical order). Shared by
/// a sprite document's inline `palette:` (`L1153`), a `palette:` include's `overrides` (`L1147`), and
/// a standalone `kind: palette` document (`L1147`, [`crate::palette`]).
pub(crate) fn parse_palette_entries_map(
    entries: &[MapEntry],
    base: &str,
    code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<SpritePaletteEntry> {
    let mut out = Vec::with_capacity(entries.len());
    for sub in entries {
        let pointer = format!("{base}/{}", pointer_token(&sub.key));
        if let Some(parsed) = parse_palette_entry(&sub.node, &pointer, &sub.key, code, diagnostics)
        {
            out.push(parsed);
        }
    }
    out.sort_by(|a, b| a.glyph.cmp(&b.glyph));
    out
}

/// Parses a `palette:` include block (`{ from, overrides? }`). All of an include's own defects are
/// `L1147` — the shared-palette reference is one concern, distinct from the sprite document's inline
/// palette (`L1153`). A `from` key is present by construction (it is what routed the caller here); a
/// wrong-typed one is reported and yields no include, which is why the return is an `Option`.
fn parse_palette_include(
    entries: &[MapEntry],
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<SpritePaletteInclude> {
    let mut from = None;
    let mut overrides = Vec::new();
    for sub in entries {
        match sub.key.as_str() {
            "from" => {
                from = envelope::expect_string(
                    &sub.node,
                    "/palette/from",
                    "a palette include's `from`",
                    code::SPRITE_PALETTE_STRUCTURE,
                    diagnostics,
                );
            }
            "overrides" => {
                let NodeValue::Mapping(over_entries) = &sub.node.value else {
                    diagnostics.push(
                        Diagnostic::new(
                            code::SPRITE_PALETTE_STRUCTURE,
                            format!(
                                "`palette.overrides` must be a mapping of glyphs to colors, found {} \
                                 (line {})",
                                sub.node.value.type_phrase(),
                                sub.node.line
                            ),
                        )
                        .with_path("/palette/overrides")
                        .with_help(
                            "write `overrides` as glyph keys, each mapping to a `color`, exactly like \
                             an inline palette",
                        ),
                    );
                    continue;
                };
                overrides = parse_palette_entries_map(
                    over_entries,
                    "/palette/overrides",
                    code::SPRITE_PALETTE_STRUCTURE,
                    diagnostics,
                );
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                sub,
                "/palette",
                "a palette include",
                PALETTE_INCLUDE_KEYS,
                code::SPRITE_PALETTE_STRUCTURE,
            )),
        }
    }
    Some(SpritePaletteInclude {
        from: from?,
        overrides,
    })
}

/// Parses one palette entry: a mapping with a required `color` and an optional `emissive`. `code`
/// tags every diagnostic, so the same entry shape serves both palette families.
pub(crate) fn parse_palette_entry(
    node: &Node,
    pointer: &str,
    glyph: &str,
    code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<SpritePaletteEntry> {
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code,
                format!(
                    "a palette entry must be a mapping with a `color`, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("give the glyph a color:\n      color: \"#ffffff\""),
        );
        return None;
    };
    let mut color = None;
    let mut color_seen = false;
    let mut emissive = None;
    for sub in entries {
        match sub.key.as_str() {
            "color" => {
                color_seen = true;
                color = parse_color(&sub.node, &format!("{pointer}/color"), code, diagnostics);
            }
            "emissive" => {
                emissive = parse_number(
                    &sub.node,
                    &format!("{pointer}/emissive"),
                    "emissive",
                    code,
                    diagnostics,
                );
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                sub,
                pointer,
                "a palette entry",
                PALETTE_ENTRY_KEYS,
                code,
            )),
        }
    }
    if !color_seen {
        diagnostics.push(
            Diagnostic::new(code, "a palette entry is missing its required `color`")
                .with_path(pointer.to_string())
                .with_help("add a `color`, e.g. `color: \"#ff8800\"`"),
        );
    }
    color.map(|color| SpritePaletteEntry {
        glyph: glyph.to_string(),
        color,
        emissive,
    })
}

/// Parses `frames`: a mapping of frame name to `{pivot?, rows}`, kept in authored order.
fn parse_frames(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Vec<SpriteFrame> {
    let NodeValue::Mapping(entries) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "`frames` must be a mapping of frame names to grids, found {} (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/frames")
            .with_help(
                "write one named frame per pose:\n  frames:\n    idle:\n      rows:\n        \
                 - \"..##..\"",
            ),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(entries.len());
    for sub in entries {
        let pointer = format!("/frames/{}", pointer_token(&sub.key));
        if let Some(frame) = parse_frame(&sub.node, &pointer, &sub.key, diagnostics) {
            out.push(frame);
        }
    }
    out
}

/// Parses one frame: a mapping with `rows` (and an optional `mirror`) or `base` + `patches`, and an
/// optional `pivot`.
fn parse_frame(
    node: &Node,
    pointer: &str,
    name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<SpriteFrame> {
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "a frame must be a mapping with `rows`, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("give the frame its grid:\n      rows:\n        - \"..##..\""),
        );
        return None;
    };
    let mut pivot = None;
    let mut rows = None;
    let mut mirror = None;
    let mut base = None;
    let mut patches = None;
    for sub in entries {
        match sub.key.as_str() {
            "pivot" => {
                pivot = parse_vec2_f64(
                    &sub.node,
                    &format!("{pointer}/pivot"),
                    "pivot",
                    "px, py",
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    diagnostics,
                );
            }
            "rows" => {
                rows = Some(parse_rows(
                    &sub.node,
                    &format!("{pointer}/rows"),
                    diagnostics,
                ));
            }
            "mirror" => {
                mirror = parse_choice(
                    &sub.node,
                    &format!("{pointer}/mirror"),
                    "mirror",
                    &["x", "y", "xy"],
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    diagnostics,
                )
                .map(|text| match text {
                    "x" => SpriteMirror::X,
                    "y" => SpriteMirror::Y,
                    _ => SpriteMirror::Xy,
                });
            }
            "base" => match &sub.node.value {
                NodeValue::Scalar(Value::String(text)) => base = Some(text.clone()),
                other => diagnostics.push(
                    Diagnostic::new(
                        code::SPRITE_DOCUMENT_STRUCTURE,
                        format!(
                            "`base` must be a frame name, found {} (line {})",
                            other.type_phrase(),
                            sub.node.line
                        ),
                    )
                    .with_path(format!("{pointer}/base"))
                    .with_help("name the `rows:` frame the patches apply to"),
                ),
            },
            "patches" => {
                patches = Some(parse_patches(
                    &sub.node,
                    &format!("{pointer}/patches"),
                    diagnostics,
                ));
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                sub,
                pointer,
                "a frame",
                FRAME_KEYS,
                code::SPRITE_DOCUMENT_STRUCTURE,
            )),
        }
    }
    let structural = |message: &str, help: &str| {
        Diagnostic::new(code::SPRITE_DOCUMENT_STRUCTURE, message.to_string())
            .with_path(pointer.to_string())
            .with_help(help.to_string())
    };
    let grid = match (rows, base, patches) {
        (Some(_), Some(_), _) => {
            diagnostics.push(structural(
                "a frame authors either `rows` or `base` with `patches`, not both",
                "keep `rows` for a frame drawn by hand, or `base` + `patches` for a variant of \
                 another frame",
            ));
            return None;
        }
        (None, Some(_), _) if mirror.is_some() => {
            diagnostics.push(structural(
                "a patched frame never authors `mirror`",
                "mirror its base; the patch applies to the expanded base",
            ));
            return None;
        }
        (Some(rows), None, None) => SpriteFrameGrid::Rows { rows, mirror },
        (Some(_), None, Some(_)) => {
            diagnostics.push(structural(
                "`patches` needs a `base` frame to patch, not `rows`",
                "replace `rows` with `base: <frame>`, or drop `patches`",
            ));
            return None;
        }
        (None, Some(base), Some(patches)) => SpriteFrameGrid::Patched { base, patches },
        (None, Some(_), None) => {
            diagnostics.push(structural(
                "a frame with a `base` is missing its `patches`",
                "add `patches`, a list of patches:\n  patches:\n    - at:\n        - <x>\n        - <y>\n      rows:\n        - \"...\"",
            ));
            return None;
        }
        (None, None, Some(_)) => {
            diagnostics.push(structural(
                "`patches` needs a `base` frame to patch",
                "add `base: <frame>` naming a frame that authors `rows:`",
            ));
            return None;
        }
        (None, None, None) => {
            diagnostics.push(structural(
                "a frame is missing its required `rows`",
                "add `rows`: one quoted string per texel row, top-down, `.` for a transparent \
                 texel; or `base:` + `patches:` to vary another frame",
            ));
            return None;
        }
    };
    Some(SpriteFrame {
        name: name.to_string(),
        pivot,
        grid,
    })
}

/// Parses a frame's `patches`: a sequence of `{at, rows}` mappings.
fn parse_patches(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<SpritePatch> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "`patches` must be a sequence of patches, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(
                "write each patch as:\n  - at:\n      - <x>\n      - <y>\n    rows:\n      - \"...\"",
            ),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let item_pointer = format!("{pointer}/{index}");
        let NodeValue::Mapping(entries) = &item.value else {
            diagnostics.push(
                Diagnostic::new(
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    format!(
                        "patch {index} must be a mapping with `at` and `rows`, found {} (line {})",
                        item.value.type_phrase(),
                        item.line
                    ),
                )
                .with_path(item_pointer)
                .with_help(
                    "write each patch as:\n  - at:\n      - <x>\n      - <y>\n    rows:\n      - \"...\"",
                ),
            );
            continue;
        };
        let mut at = None;
        let mut rows = None;
        for sub in entries {
            match sub.key.as_str() {
                "at" => at = parse_at(&sub.node, &format!("{item_pointer}/at"), diagnostics),
                "rows" => {
                    rows = Some(parse_rows(
                        &sub.node,
                        &format!("{item_pointer}/rows"),
                        diagnostics,
                    ));
                }
                unknown => diagnostics.push(envelope::unknown_key(
                    unknown,
                    sub,
                    &item_pointer,
                    "a patch",
                    PATCH_KEYS,
                    code::SPRITE_DOCUMENT_STRUCTURE,
                )),
            }
        }
        match (at, rows) {
            (Some(at), Some(rows)) => out.push(SpritePatch { at, rows }),
            (at, rows) => {
                if at.is_none() && !entries.iter().any(|sub| sub.key == "at") {
                    diagnostics.push(
                        Diagnostic::new(
                            code::SPRITE_DOCUMENT_STRUCTURE,
                            format!("patch {index} is missing its required `at`"),
                        )
                        .with_path(item_pointer.clone())
                        .with_help(
                            "add the patch's top-left texel:\n  at:\n    - <x>\n    - <y>",
                        ),
                    );
                }
                if rows.is_none() {
                    diagnostics.push(
                        Diagnostic::new(
                            code::SPRITE_DOCUMENT_STRUCTURE,
                            format!("patch {index} is missing its required `rows`"),
                        )
                        .with_path(item_pointer)
                        .with_help("add `rows`: one quoted string per patch row"),
                    );
                }
            }
        }
    }
    out
}

/// Parses a patch's `at`: two non-negative whole numbers.
fn parse_at(node: &Node, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<[u32; 2]> {
    let help = "write `at` as two non-negative integers, the patch's top-left texel:\n  at:\n    - <x>\n    - <y>";
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "`at` must be a sequence of two whole numbers, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(help),
        );
        return None;
    };
    if items.len() != 2 {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "`at` must have exactly 2 entries (x, y), found {}",
                    items.len()
                ),
            )
            .with_path(pointer.to_string())
            .with_help(help),
        );
        return None;
    }
    let mut out = [0u32; 2];
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        let value = match &item.value {
            NodeValue::Scalar(Value::Number(n)) => n.as_u64().and_then(|v| u32::try_from(v).ok()),
            _ => None,
        };
        match value {
            Some(v) => out[index] = v,
            None => {
                diagnostics.push(
                    Diagnostic::new(
                        code::SPRITE_DOCUMENT_STRUCTURE,
                        format!(
                            "`at[{index}]` must be a non-negative whole number of texels, found \
                             {} (line {})",
                            item.value.type_phrase(),
                            item.line
                        ),
                    )
                    .with_path(format!("{pointer}/{index}"))
                    .with_help(help),
                );
                ok = false;
            }
        }
    }
    ok.then_some(out)
}

/// Parses a frame's `rows`: a sequence of strings.
fn parse_rows(node: &Node, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Vec<String> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "`rows` must be a sequence of row strings, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("write one quoted string per texel row, top-down"),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        match &item.value {
            NodeValue::Scalar(Value::String(s)) => out.push(s.clone()),
            other => diagnostics.push(
                Diagnostic::new(
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    format!(
                        "row {index} must be a string, found {} (line {})",
                        other.type_phrase(),
                        item.line
                    ),
                )
                .with_path(format!("{pointer}/{index}"))
                .with_help("quote the row so a numeric- or boolean-looking line stays a string"),
            ),
        }
    }
    out
}

fn bad_hex_colour(
    text: &str,
    line: usize,
    pointer: &str,
    code: ludion_core::diagnostics::DiagnosticCode,
) -> Diagnostic {
    let short = text
        .strip_prefix('#')
        .filter(|d| matches!(d.len(), 3 | 4) && d.bytes().all(|b| b.is_ascii_hexdigit()));
    let help = match short {
        Some(digits) => {
            let expanded: String = digits.chars().flat_map(|c| [c, c]).collect();
            format!("write all digits out: `\"#{expanded}\"` for `{text}`")
        }
        None => "write the colour as `\"#rrggbb\"` or `\"#rrggbbaa\"`, e.g. `\"#ff8800\"`".into(),
    };
    Diagnostic::new(
        code,
        format!(
            "a colour is \"#rrggbb\" or \"#rrggbbaa\" (sRGB hex), found `{text}` (line {line})"
        ),
    )
    .with_path(pointer.to_string())
    .with_help(help)
}

/// Parses a colour: an sRGB hex string (`"#rrggbb"` / `"#rrggbbaa"`) or the legacy sequence of four
/// linear RGBA numbers.
fn parse_color(
    node: &Node,
    pointer: &str,
    structure_code: ludion_core::diagnostics::DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<[f64; 4]> {
    if let NodeValue::Scalar(Value::String(text)) = &node.value {
        let parsed = ludion_core::color::parse_hex(text);
        if parsed.is_none() {
            diagnostics.push(bad_hex_colour(text, node.line, pointer, structure_code));
        }
        return parsed.map(|c| c.map(f64::from));
    }
    let NodeValue::Sequence(items) = &node.value else {
        let help = if matches!(node.value, NodeValue::Scalar(Value::Null)) {
            "a `#` starts a YAML comment, so an unquoted hex colour reads as empty — quote it: \
             `color: \"#ff0000\"`"
        } else {
            "write the colour as a quoted hex string, e.g. `color: \"#ff8800\"` (\"#rrggbbaa\" adds alpha)"
        };
        diagnostics.push(
            Diagnostic::new(
                structure_code,
                format!(
                    "a color must be a hex string such as \"#ff8800\", found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(help),
        );
        return None;
    };
    if items.len() != 4 {
        diagnostics.push(
            Diagnostic::new(
                structure_code,
                format!(
                    "a color must have exactly 4 components (red, green, blue, alpha), found {}",
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
                            "color component {index} must be a number, found {} (line {})",
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

/// Parses a two-number pair. `components` names the pair in the help ("px, py", "x, y").
fn parse_vec2_f64(
    node: &Node,
    pointer: &str,
    what: &str,
    components: &str,
    structure_code: ludion_core::diagnostics::DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<[f64; 2]> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                structure_code,
                format!(
                    "`{what}` must be a sequence of two numbers, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(format!("write `{what}` as two numbers ({components})")),
        );
        return None;
    };
    if items.len() != 2 {
        diagnostics.push(
            Diagnostic::new(
                structure_code,
                format!(
                    "`{what}` must have exactly 2 entries ({components}), found {}",
                    items.len()
                ),
            )
            .with_path(pointer.to_string())
            .with_help(format!("supply exactly two numbers ({components})")),
        );
        return None;
    }
    let mut out = [0.0f64; 2];
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match &item.value {
            NodeValue::Scalar(Value::Number(n)) => out[index] = crate::fields::number(n),
            other => {
                diagnostics.push(
                    Diagnostic::new(
                        structure_code,
                        format!(
                            "`{what}[{index}]` must be a number, found {} (line {})",
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

/// Parses a two-integer whole-texel pair (`shadow`'s `offset`).
fn parse_vec2_i32(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<[i32; 2]> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_EFFECT_INVALID,
                format!(
                    "`{what}` must be a sequence of two whole numbers, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(format!("write `{what}` as two integers (x, y) in texels")),
        );
        return None;
    };
    if items.len() != 2 {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_EFFECT_INVALID,
                format!(
                    "`{what}` must have exactly 2 entries (x, y), found {}",
                    items.len()
                ),
            )
            .with_path(pointer.to_string())
            .with_help(format!("supply exactly two integers for `{what}`")),
        );
        return None;
    }
    let mut out = [0i32; 2];
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        let value = match &item.value {
            NodeValue::Scalar(Value::Number(n)) => n
                .as_i64()
                .filter(|v| i32::try_from(*v).is_ok())
                .map(|v| v as i32),
            _ => None,
        };
        match value {
            Some(v) => out[index] = v,
            None => {
                diagnostics.push(
                    Diagnostic::new(
                        code::SPRITE_EFFECT_INVALID,
                        format!(
                            "`{what}[{index}]` must be a whole number of texels, found {} \
                             (line {})",
                            item.value.type_phrase(),
                            item.line
                        ),
                    )
                    .with_path(format!("{pointer}/{index}"))
                    .with_help("a texel offset is an integer; fractional offsets do not exist"),
                );
                ok = false;
            }
        }
    }
    ok.then_some(out)
}

/// Parses a sequence of names (`glyphs:`, an effect's `frames:`).
fn parse_name_list(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Vec<String>> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_EFFECT_INVALID,
                format!(
                    "`{what}` must be a sequence, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(format!(
                "write `{what}` as one quoted entry per line; remove the key to apply the effect \
                 everywhere"
            )),
        );
        return None;
    };
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        match &item.value {
            NodeValue::Scalar(Value::String(s)) => out.push(s.clone()),
            other => diagnostics.push(
                Diagnostic::new(
                    code::SPRITE_EFFECT_INVALID,
                    format!(
                        "`{what}[{index}]` must be a string, found {} (line {})",
                        other.type_phrase(),
                        item.line
                    ),
                )
                .with_path(format!("{pointer}/{index}"))
                .with_help("quote the entry so a numeric- or boolean-looking name stays a string"),
            ),
        }
    }
    Some(out)
}

/// Parses one of a fixed set of keyword strings, with a nearest-match suggestion on a miss.
fn parse_choice(
    node: &Node,
    pointer: &str,
    what: &str,
    valid: &[&'static str],
    code: ludion_core::diagnostics::DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<&'static str> {
    let NodeValue::Scalar(Value::String(text)) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code,
                format!(
                    "`{what}` must be a string, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(format!("`{what}` is one of: {}", valid.join(", "))),
        );
        return None;
    };
    if let Some(found) = valid.iter().find(|candidate| *candidate == text) {
        return Some(found);
    }
    let mut diagnostic = Diagnostic::new(
        code,
        format!("`{text}` is not a valid `{what}` (line {})", node.line),
    )
    .with_path(pointer.to_string());
    diagnostic = match suggest::nearest(text, valid.iter().copied()) {
        Some(candidate) => diagnostic.with_help(format!("nearest match is `{candidate}`")),
        None => diagnostic.with_help(format!("`{what}` is one of: {}", valid.join(", "))),
    };
    diagnostics.push(diagnostic);
    None
}

/// Parses `effects`: a sequence of effect entries. The key itself being the wrong shape is a
/// document-structure defect (`L1153`); everything under `/effects/<i>` is `L1154`.
fn parse_effects(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Vec<SpriteEffect> {
    let NodeValue::Sequence(items) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "`effects` must be a sequence of effect entries, found {} (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/effects")
            .with_help(
                "write one entry per op, in application order:\n  effects:\n    - op: outline\n\
                 \x20     color: \"#000000\"",
            ),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        if let Some(effect) = parse_effect(item, index, diagnostics) {
            out.push(effect);
        }
    }
    out
}

/// Every field an effect entry can carry, collected before the op decides which are meaningful.
#[derive(Default)]
struct EffectFields {
    glyphs: Option<Vec<String>>,
    frames: Option<Vec<String>>,
    from: Option<[f64; 4]>,
    to: Option<[f64; 4]>,
    direction: Option<GradientDirection>,
    blend: Option<GradientBlend>,
    color: Option<[f64; 4]>,
    side: Option<OutlineSide>,
    light: Option<[f64; 2]>,
    strength: Option<f64>,
    offset: Option<[i32; 2]>,
    density: Option<f64>,
    amount: Option<f64>,
    radius: Option<u32>,
    radius_px: Option<f64>,
}

/// Parses one effect entry. `op` is read first, because it decides the entry's whole key set — a
/// `radius` on a `gradient` is an unknown key, not a stray field.
fn parse_effect(
    node: &Node,
    index: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<SpriteEffect> {
    let pointer = format!("/effects/{index}");
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_EFFECT_INVALID,
                format!(
                    "an effect entry must be a mapping with an `op`, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer)
            .with_help("write the entry as keys under one `-`:\n    - op: blur\n      radius: 1"),
        );
        return None;
    };
    let Some(op_entry) = entries.iter().find(|sub| sub.key == "op") else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_EFFECT_INVALID,
                format!(
                    "an effect entry is missing its required `op` (line {})",
                    node.line
                ),
            )
            .with_path(pointer)
            .with_help(format!("`op` is one of: {}", EFFECT_OPS.join(", "))),
        );
        return None;
    };
    let op_name = parse_choice(
        &op_entry.node,
        &format!("{pointer}/op"),
        "op",
        &EFFECT_OPS,
        code::SPRITE_EFFECT_INVALID,
        diagnostics,
    )?;

    let valid: &[&'static str] = match op_name {
        "gradient" => &GRADIENT_KEYS,
        "outline" => &OUTLINE_KEYS,
        "shade" => &SHADE_KEYS,
        "shadow" => &SHADOW_KEYS,
        "dither" => &DITHER_KEYS,
        "noise" => &NOISE_KEYS,
        "glow" => &GLOW_KEYS,
        _ => &BLUR_KEYS,
    };
    let fields = parse_effect_fields(entries, &pointer, op_name, valid, diagnostics);

    let op = match op_name {
        "gradient" => {
            if fields.from.is_none() {
                diagnostics.push(missing_effect_field(
                    &pointer,
                    "gradient",
                    "from",
                    "the color the ramp starts at, a hex string",
                ));
            }
            if fields.to.is_none() {
                diagnostics.push(missing_effect_field(
                    &pointer,
                    "gradient",
                    "to",
                    "the color the ramp ends at, a hex string",
                ));
            }
            SpriteEffectOp::Gradient {
                from: fields.from?,
                to: fields.to?,
                direction: fields.direction.unwrap_or(GRADIENT_DIRECTION_DEFAULT),
                blend: fields.blend.unwrap_or(GRADIENT_BLEND_DEFAULT),
            }
        }
        "outline" => {
            if fields.color.is_none() {
                diagnostics.push(missing_effect_field(
                    &pointer,
                    "outline",
                    "color",
                    "the border's color, a hex string",
                ));
            }
            SpriteEffectOp::Outline {
                color: fields.color?,
                side: fields.side.unwrap_or(OUTLINE_SIDE_DEFAULT),
            }
        }
        "shade" => SpriteEffectOp::Shade {
            light: fields.light.unwrap_or(SHADE_LIGHT_DEFAULT),
            strength: fields.strength.unwrap_or(SHADE_STRENGTH_DEFAULT),
        },
        "shadow" => SpriteEffectOp::Shadow {
            offset: fields.offset.unwrap_or(SHADOW_OFFSET_DEFAULT),
            color: fields.color.unwrap_or(SHADOW_COLOR_DEFAULT),
        },
        "dither" => SpriteEffectOp::Dither {
            density: fields.density.unwrap_or(DITHER_DENSITY_DEFAULT),
        },
        "noise" => {
            if fields.amount.is_none() {
                diagnostics.push(missing_effect_field(
                    &pointer,
                    "noise",
                    "amount",
                    "how far a texel may jitter, 0..=1",
                ));
            }
            SpriteEffectOp::Noise {
                amount: fields.amount?,
            }
        }
        "glow" => {
            if fields.radius_px.is_none() {
                diagnostics.push(missing_effect_field(
                    &pointer,
                    "glow",
                    "radius_px",
                    "the halo reach in texels, a number in 0 < radius_px <= 32",
                ));
            }
            if fields.strength.is_none() {
                diagnostics.push(missing_effect_field(
                    &pointer,
                    "glow",
                    "strength",
                    "how strongly the halo builds, a number >= 0",
                ));
            }
            SpriteEffectOp::Glow {
                color: fields.color,
                radius_px: fields.radius_px?,
                strength: fields.strength?,
            }
        }
        _ => {
            if fields.radius.is_none() {
                diagnostics.push(missing_effect_field(
                    &pointer,
                    "blur",
                    "radius",
                    "the box radius in texels, 1..=8",
                ));
            }
            SpriteEffectOp::Blur {
                radius: fields.radius?,
            }
        }
    };

    Some(SpriteEffect {
        op,
        glyphs: fields.glyphs,
        frames: fields.frames,
    })
}

/// The "an effect is missing a required field" diagnostic.
fn missing_effect_field(pointer: &str, op: &str, field: &str, help: &str) -> Diagnostic {
    Diagnostic::new(
        code::SPRITE_EFFECT_INVALID,
        format!("the `{op}` effect is missing its required `{field}`"),
    )
    .with_path(pointer.to_string())
    .with_help(format!("add `{field}`: {help}"))
}

/// Reads every key of one effect entry that `valid` admits, diagnosing the rest with a nearest match
/// over the op's own key set.
fn parse_effect_fields(
    entries: &[MapEntry],
    pointer: &str,
    op: &str,
    valid: &[&'static str],
    diagnostics: &mut Vec<Diagnostic>,
) -> EffectFields {
    let mut fields = EffectFields::default();
    for sub in entries {
        let key = sub.key.as_str();
        if !valid.contains(&key) {
            let mut diagnostic = Diagnostic::new(
                code::SPRITE_EFFECT_INVALID,
                format!(
                    "the `{op}` effect does not have a key named `{key}` (line {})",
                    sub.key_line
                ),
            )
            .with_path(format!("{pointer}/{}", pointer_token(key)));
            diagnostic = match suggest::nearest(key, valid.iter().copied()) {
                Some(candidate) => diagnostic.with_help(format!("nearest match is `{candidate}`")),
                None => diagnostic.with_help(format!(
                    "the `{op}` effect's keys are: {}",
                    valid.join(", ")
                )),
            };
            diagnostics.push(diagnostic);
            continue;
        }
        let field_pointer = format!("{pointer}/{key}");
        match key {
            "op" => {}
            "glyphs" => {
                fields.glyphs = parse_name_list(&sub.node, &field_pointer, "glyphs", diagnostics);
            }
            "frames" => {
                fields.frames = parse_name_list(&sub.node, &field_pointer, "frames", diagnostics);
            }
            "from" => {
                fields.from = parse_color(
                    &sub.node,
                    &field_pointer,
                    code::SPRITE_EFFECT_INVALID,
                    diagnostics,
                );
            }
            "to" => {
                fields.to = parse_color(
                    &sub.node,
                    &field_pointer,
                    code::SPRITE_EFFECT_INVALID,
                    diagnostics,
                );
            }
            "color" => {
                fields.color = parse_color(
                    &sub.node,
                    &field_pointer,
                    code::SPRITE_EFFECT_INVALID,
                    diagnostics,
                );
            }
            "direction" => {
                fields.direction = parse_choice(
                    &sub.node,
                    &field_pointer,
                    "direction",
                    &["vertical", "horizontal", "radial"],
                    code::SPRITE_EFFECT_INVALID,
                    diagnostics,
                )
                .map(|text| match text {
                    "horizontal" => GradientDirection::Horizontal,
                    "radial" => GradientDirection::Radial,
                    _ => GradientDirection::Vertical,
                });
            }
            "blend" => {
                fields.blend = parse_choice(
                    &sub.node,
                    &field_pointer,
                    "blend",
                    &["replace", "multiply"],
                    code::SPRITE_EFFECT_INVALID,
                    diagnostics,
                )
                .map(|text| match text {
                    "multiply" => GradientBlend::Multiply,
                    _ => GradientBlend::Replace,
                });
            }
            "side" => {
                fields.side = parse_choice(
                    &sub.node,
                    &field_pointer,
                    "side",
                    &["outside", "inside"],
                    code::SPRITE_EFFECT_INVALID,
                    diagnostics,
                )
                .map(|text| match text {
                    "inside" => OutlineSide::Inside,
                    _ => OutlineSide::Outside,
                });
            }
            "light" => {
                fields.light = parse_vec2_f64(
                    &sub.node,
                    &field_pointer,
                    "light",
                    "x, y",
                    code::SPRITE_EFFECT_INVALID,
                    diagnostics,
                );
            }
            "offset" => {
                fields.offset = parse_vec2_i32(&sub.node, &field_pointer, "offset", diagnostics);
            }
            "strength" => {
                fields.strength = parse_number(
                    &sub.node,
                    &field_pointer,
                    "strength",
                    code::SPRITE_EFFECT_INVALID,
                    diagnostics,
                );
            }
            "density" => {
                fields.density = parse_number(
                    &sub.node,
                    &field_pointer,
                    "density",
                    code::SPRITE_EFFECT_INVALID,
                    diagnostics,
                );
            }
            "amount" => {
                fields.amount = parse_number(
                    &sub.node,
                    &field_pointer,
                    "amount",
                    code::SPRITE_EFFECT_INVALID,
                    diagnostics,
                );
            }
            "radius_px" => {
                fields.radius_px = parse_number(
                    &sub.node,
                    &field_pointer,
                    "radius_px",
                    code::SPRITE_EFFECT_INVALID,
                    diagnostics,
                );
            }
            _ => {
                fields.radius = parse_radius(&sub.node, &field_pointer, diagnostics);
            }
        }
    }
    fields
}

/// Parses `blur`'s `radius`: a non-negative integer. The `1..=8` range is checked by [`validate`].
fn parse_radius(node: &Node, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<u32> {
    let value = match &node.value {
        NodeValue::Scalar(Value::Number(n)) => n.as_u64().filter(|v| *v <= u64::from(u32::MAX)),
        _ => None,
    };
    match value {
        Some(v) => Some(v as u32),
        None => {
            diagnostics.push(
                Diagnostic::new(
                    code::SPRITE_EFFECT_INVALID,
                    format!(
                        "`radius` must be a non-negative whole number, found {} (line {})",
                        node.value.type_phrase(),
                        node.line
                    ),
                )
                .with_path(pointer.to_string())
                .with_help("a blur radius is a whole number of texels, 1..=8"),
            );
            None
        }
    }
}

// --- Validate (semantics, `L1153` / `L1154`) ----------------------------------------------------

/// Validates semantic invariants of an already-parsed [`SpriteDocument`]: `size` in range, palette
/// keys single printable-ASCII glyphs with in-range colors, frame names unique and `/`-free, every
/// frame's grid matching `size` with known glyphs, pivots inside the frame, and every effect's
/// parameters in range and resolving against this document's own palette and frames.
///
/// Document-level defects are `L1153`; anything under `/effects` is `L1154`. Independent of how the
/// document was built — hand-authored via [`parse`] or assembled programmatically — so this is the
/// one place those invariants are enforced.
pub(crate) fn validate(doc: &SpriteDocument) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    validate_size(doc, &mut diagnostics);
    // With an inline palette the known-glyph set is in hand, so a frame or effect glyph is checked
    // against it. With an unresolved include the palette is not yet known, so those membership checks
    // wait for [`crate::resolve_sprite_include`] — after which the resolved (inline) document faces
    // this same validator with a real set. The include's own shape is checked here regardless.
    let known = match &doc.palette_include {
        None => Some(validate_palette(doc, &mut diagnostics)),
        Some(include) => {
            validate_palette_include(include, &mut diagnostics);
            None
        }
    };
    validate_frames(doc, known.as_deref(), &mut diagnostics);
    validate_effects(doc, known.as_deref(), &mut diagnostics);
    diagnostics
}

fn validate_size(doc: &SpriteDocument, diagnostics: &mut Vec<Diagnostic>) {
    for (index, axis) in ["width", "height"].into_iter().enumerate() {
        let value = doc.size[index];
        if !(1..=SPRITE_MAX_SIZE).contains(&value) {
            diagnostics.push(
                Diagnostic::new(
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    format!(
                        "`size` {axis} is {value} texels, outside the valid range \
                         1..={SPRITE_MAX_SIZE}"
                    ),
                )
                .with_path(format!("/size/{index}"))
                .with_help(
                    "a sprite frame is one region of a packed atlas; anything larger than \
                     1024 texels on an axis belongs in its own texture, not an atlas",
                ),
            );
        }
    }
}

/// Validates the palette, returning every accepted glyph once — the membership set the frame grids
/// and the effects' `glyphs:` filters are tested against.
fn validate_palette(doc: &SpriteDocument, diagnostics: &mut Vec<Diagnostic>) -> Vec<char> {
    if doc.palette.is_empty() {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                "the palette is empty; a sprite must name at least one color",
            )
            .with_path("/palette")
            .with_help(
                "add one glyph with a `color`; `.` is always transparent and never a palette entry",
            ),
        );
    }
    validate_palette_entries(
        &doc.palette,
        "/palette",
        code::SPRITE_DOCUMENT_STRUCTURE,
        diagnostics,
    )
}

/// Validates a `palette:` include's own shape: a non-empty `from`, and override entries that are
/// well-formed palette entries. Whether the referenced file resolves, and whether the merged palette
/// covers every glyph the frames use, are the resolve/bake seam's checks — this validator sees only
/// the sprite document's own text.
fn validate_palette_include(include: &SpritePaletteInclude, diagnostics: &mut Vec<Diagnostic>) {
    if include.from.trim().is_empty() {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_PALETTE_STRUCTURE,
                "`palette.from` is empty; it must name a `kind: palette` document",
            )
            .with_path("/palette/from")
            .with_help(
                "point `from` at a palette document, relative to this sprite document's own directory",
            ),
        );
    }
    validate_palette_entries(
        &include.overrides,
        "/palette/overrides",
        code::SPRITE_PALETTE_STRUCTURE,
        diagnostics,
    );
}

/// Validates a palette's entries — glyph keys single printable-ASCII and unique, colors in `0..=1`,
/// emissive finite and `>= 0` — rooted at `base`, tagging every diagnostic with `code`. Returns each
/// accepted glyph once: the membership set frame grids and effect `glyphs:` filters test against.
/// Shared by a sprite's inline palette, a `palette:` include's overrides, and a `kind: palette`
/// document.
pub(crate) fn validate_palette_entries(
    palette: &[SpritePaletteEntry],
    base: &str,
    code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<char> {
    let mut known: Vec<char> = Vec::new();
    for entry in palette {
        let pointer = format!("{base}/{}", pointer_token(&entry.glyph));
        match single_glyph(&entry.glyph) {
            None => diagnostics.push(
                Diagnostic::new(
                    code,
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
            Some(SPRITE_EMPTY_GLYPH) => diagnostics.push(
                Diagnostic::new(
                    code,
                    "the glyph `.` is reserved for transparent texels and cannot name a palette \
                     entry",
                )
                .with_path(pointer.clone())
                .with_help(
                    "choose any other printable-ASCII glyph; `.` always means a transparent texel",
                ),
            ),
            Some(ch) if !is_palette_glyph(ch) => diagnostics.push(
                Diagnostic::new(
                    code,
                    format!(
                        "the palette glyph `{ch}` is not a printable ASCII character (0x21..=0x7E)"
                    ),
                )
                .with_path(pointer.clone())
                .with_help(
                    "sprite glyphs must be printable ASCII so texel columns stay aligned; pick a \
                     character from `!` to `~`",
                ),
            ),
            Some(ch) if known.contains(&ch) => diagnostics.push(
                Diagnostic::new(
                    code,
                    format!("the palette glyph `{ch}` is defined more than once"),
                )
                .with_path(pointer.clone())
                .with_help("each glyph may appear once; remove or rename the duplicate entry"),
            ),
            Some(ch) => known.push(ch),
        }

        for (index, &component) in entry.color.iter().enumerate() {
            if !(component.is_finite() && (0.0..=1.0).contains(&component)) {
                diagnostics.push(
                    Diagnostic::new(
                        code,
                        format!(
                            "palette glyph `{}` has a color component `{component}` at index \
                             {index}, outside the valid range [0, 1]",
                            entry.glyph
                        ),
                    )
                    .with_path(format!("{pointer}/color/{index}"))
                    .with_help(
                        "sprite colors are linear RGBA in 0..=1; HDR brightness belongs in \
                         `emissive`, which is unbounded above",
                    ),
                );
            }
        }

        if let Some(emissive) = entry.emissive
            && !(emissive.is_finite() && emissive >= 0.0)
        {
            diagnostics.push(
                Diagnostic::new(
                    code,
                    format!(
                        "palette glyph `{}` has an `emissive` of {emissive}; it must be finite and \
                         at least 0",
                        entry.glyph
                    ),
                )
                .with_path(format!("{pointer}/emissive"))
                .with_help("remove the key for a non-emissive glyph, or give it a value >= 0"),
            );
        }
    }
    known
}

/// Checks an authored `rows:` grid against the extents its `mirror` (if any) leaves it: the full
/// size, or the kept half.
fn validate_rows(
    frame: &SpriteFrame,
    rows: &[String],
    mirror: Option<SpriteMirror>,
    [width, height]: [u32; 2],
    known: Option<&[char]>,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let half_rows = mirror.is_some_and(SpriteMirror::flips_y);
    let half_width = mirror.is_some_and(SpriteMirror::flips_x);
    let want_rows = if half_rows {
        height.div_ceil(2)
    } else {
        height
    };
    let want_width = if half_width { width.div_ceil(2) } else { width };
    if rows.len() as u64 != u64::from(want_rows) {
        let message = if half_rows {
            format!(
                "frame `{}` has {} rows but `mirror: {}` keeps the top half: expected {want_rows} \
                 (ceil({height}/2), the centre row included)",
                frame.name,
                rows.len(),
                mirror.map_or("", SpriteMirror::as_str)
            )
        } else {
            format!(
                "frame `{}` has {} rows but `size` declares height {height}",
                frame.name,
                rows.len()
            )
        };
        diagnostics.push(
            Diagnostic::new(code::SPRITE_DOCUMENT_STRUCTURE, message)
                .with_path(format!("{pointer}/rows"))
                .with_help(
                    "every frame in a document is the same size; a differently sized pose is a \
                     different sprite document",
                ),
        );
    }
    for (index, row) in rows.iter().enumerate() {
        let row_pointer = format!("{pointer}/rows/{index}");
        let row_width = row.chars().count();
        if row_width as u64 != u64::from(want_width) {
            let message = if half_width {
                format!(
                    "row {index} of frame `{}` is {row_width} characters wide but `mirror: {}` \
                     keeps the left half: expected {want_width} (ceil({width}/2), the centre \
                     column included)",
                    frame.name,
                    mirror.map_or("", SpriteMirror::as_str)
                )
            } else {
                format!(
                    "row {index} of frame `{}` is {row_width} characters wide but `size` \
                     declares width {width}",
                    frame.name
                )
            };
            diagnostics.push(
                Diagnostic::new(code::SPRITE_DOCUMENT_STRUCTURE, message)
                    .with_path(row_pointer.clone())
                    .with_help(if half_width {
                        "author only the kept half of each row; the rest is the reflection"
                    } else {
                        "pad transparent texels with `.` so every row is exactly `size[0]`"
                    }),
            );
        }
        // Glyph membership is only checkable with a resolved palette; an unresolved include skips
        // it here and faces it again once resolved (see [`validate`]).
        if let Some(known) = known {
            report_unknown_glyphs(
                row,
                known,
                &[],
                &format!("row {index} of frame `{}`", frame.name),
                &row_pointer,
                diagnostics,
            );
        }
    }
}

/// Reports each palette-less glyph of `row` once; `allowed` are extra accepted characters.
fn report_unknown_glyphs(
    row: &str,
    known: &[char],
    allowed: &[char],
    place: &str,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut reported: Vec<char> = Vec::new();
    for ch in row.chars() {
        if ch != SPRITE_EMPTY_GLYPH
            && !allowed.contains(&ch)
            && !known.contains(&ch)
            && !reported.contains(&ch)
        {
            reported.push(ch);
            diagnostics.push(
                Diagnostic::new(
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    format!("the glyph `{ch}` in {place} names no palette entry"),
                )
                .with_path(pointer.to_string())
                .with_help(format!(
                    "add `{ch}` to the palette, or use `.` for a transparent texel"
                )),
            );
        }
    }
}

/// Checks a `base` + `patches` frame: the base, then each patch's origin, extent and glyphs.
fn validate_patched(
    doc: &SpriteDocument,
    frame: &SpriteFrame,
    base: &str,
    patches: &[SpritePatch],
    known: Option<&[char]>,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let [width, height] = doc.size;
    match doc.frame(base).map(|found| &found.grid) {
        None => {
            let mut diagnostic = Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "frame `{}` has a `base` of `{base}`, which names no frame",
                    frame.name
                ),
            )
            .with_path(format!("{pointer}/base"));
            let names = doc.frames.iter().map(|other| other.name.as_str());
            diagnostic = match suggest::nearest(base, names) {
                Some(candidate) => diagnostic.with_help(format!("nearest match is `{candidate}`")),
                None => diagnostic.with_help("name a frame of this document that authors `rows:`"),
            };
            diagnostics.push(diagnostic);
        }
        Some(SpriteFrameGrid::Patched { .. }) => diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "frame `{}` has a `base` of `{base}`, which is itself a patched frame",
                    frame.name
                ),
            )
            .with_path(format!("{pointer}/base"))
            .with_help(
                "a base must author `rows:`; patch that frame's own base instead — patches do \
                 not chain",
            ),
        ),
        Some(SpriteFrameGrid::Rows { .. }) => {}
    }
    if patches.is_empty() {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "frame `{}` has no patches; a patched frame must write something",
                    frame.name
                ),
            )
            .with_path(format!("{pointer}/patches"))
            .with_help("add a patch, or author the frame with `rows:` instead"),
        );
    }
    for (k, patch) in patches.iter().enumerate() {
        let patch_pointer = format!("{pointer}/patches/{k}");
        let [x, y] = patch.at;
        if x >= width || y >= height {
            diagnostics.push(
                Diagnostic::new(
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    format!(
                        "patch {k} of frame `{}` starts at [{x}, {y}], outside the {width}x{height} \
                         frame",
                        frame.name
                    ),
                )
                .with_path(format!("{patch_pointer}/at"))
                .with_help("`at` is the patch's top-left texel in the base's full coordinates"),
            );
        }
        if patch.rows.is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    format!(
                        "patch {k} of frame `{}` is a patch with nothing to write",
                        frame.name
                    ),
                )
                .with_path(format!("{patch_pointer}/rows"))
                .with_help("add one string per patch row, or remove the patch"),
            );
        }
        for (j, row) in patch.rows.iter().enumerate() {
            let row_pointer = format!("{patch_pointer}/rows/{j}");
            let chars = row.chars().count() as u64;
            if u64::from(x) + chars > u64::from(width)
                || u64::from(y) + j as u64 >= u64::from(height)
            {
                diagnostics.push(
                    Diagnostic::new(
                        code::SPRITE_DOCUMENT_STRUCTURE,
                        format!(
                            "row {j} of patch {k} in frame `{}` extends past the {width}x{height} \
                             frame from [{x}, {y}]",
                            frame.name
                        ),
                    )
                    .with_path(row_pointer.clone())
                    .with_help("shorten the row or move `at` so the whole patch fits the frame"),
                );
            }
            if let Some(known) = known {
                report_unknown_glyphs(
                    row,
                    known,
                    &[' '],
                    &format!("row {j} of patch {k} in frame `{}`", frame.name),
                    &row_pointer,
                    diagnostics,
                );
            }
        }
    }
}

fn validate_frames(
    doc: &SpriteDocument,
    known: Option<&[char]>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if doc.frames.is_empty() {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                "the document has no frames; a sprite must draw at least one",
            )
            .with_path("/frames")
            .with_help("add one named frame with `rows`, e.g. `idle`"),
        );
    }
    let [width, height] = doc.size;
    let mut seen: Vec<&str> = Vec::new();
    for frame in &doc.frames {
        let pointer = format!("/frames/{}", pointer_token(&frame.name));
        if frame.name.is_empty() {
            diagnostics.push(
                Diagnostic::new(code::SPRITE_DOCUMENT_STRUCTURE, "a frame name is empty")
                    .with_path(pointer.clone())
                    .with_help(
                        "name the frame after the pose it draws, e.g. `idle` or `bank-left`",
                    ),
            );
        } else if frame.name.contains('/') {
            diagnostics.push(
                Diagnostic::new(
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    format!("the frame name `{}` contains a `/`", frame.name),
                )
                .with_path(pointer.clone())
                .with_help(
                    "an atlas region is named `<document name>/<frame name>`, so a `/` in a frame \
                     name would make the region name ambiguous; use `-` instead",
                ),
            );
        }
        if seen.contains(&frame.name.as_str()) {
            diagnostics.push(
                Diagnostic::new(
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    format!("the frame name `{}` is used more than once", frame.name),
                )
                .with_path(pointer.clone())
                .with_help("each frame mints one atlas region, so its name must be unique"),
            );
        } else {
            seen.push(&frame.name);
        }

        match &frame.grid {
            SpriteFrameGrid::Rows { rows, mirror } => {
                validate_rows(frame, rows, *mirror, doc.size, known, &pointer, diagnostics);
            }
            SpriteFrameGrid::Patched { base, patches } => {
                validate_patched(doc, frame, base, patches, known, &pointer, diagnostics);
            }
        }

        if let Some([px, py]) = frame.pivot {
            let bounds = [f64::from(width), f64::from(height)];
            for (index, (value, limit)) in [px, py].into_iter().zip(bounds).enumerate() {
                if !(value.is_finite() && (0.0..=limit).contains(&value)) {
                    diagnostics.push(
                        Diagnostic::new(
                            code::SPRITE_DOCUMENT_STRUCTURE,
                            format!(
                                "frame `{}` has a pivot component {value} at index {index}, \
                                 outside the frame's 0..={limit} texels",
                                frame.name
                            ),
                        )
                        .with_path(format!("{pointer}/pivot/{index}"))
                        .with_help(
                            "a pivot is measured in texels from the frame's top-left and must lie \
                             on or inside the frame; remove the key to pivot about the center",
                        ),
                    );
                }
            }
        }
    }
}

fn validate_effects(
    doc: &SpriteDocument,
    known: Option<&[char]>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (index, effect) in doc.effects.iter().enumerate() {
        let pointer = format!("/effects/{index}");
        // Glyph membership needs a resolved palette; an unresolved include skips it here and faces it
        // again once resolved (see [`validate`]). `dither`'s exactly-two-glyphs count check below does
        // not need the palette, so it still runs.
        if let (Some(known), Some(glyphs)) = (known, &effect.glyphs) {
            for (slot, glyph) in glyphs.iter().enumerate() {
                let resolved = single_glyph(glyph).filter(|ch| known.contains(ch));
                if resolved.is_none() {
                    diagnostics.push(
                        Diagnostic::new(
                            code::SPRITE_EFFECT_INVALID,
                            format!("`{glyph}` names no palette glyph of this document"),
                        )
                        .with_path(format!("{pointer}/glyphs/{slot}"))
                        .with_help(
                            "each entry is one glyph the `palette` defines; remove the `glyphs` \
                             key to apply the effect to the whole frame",
                        ),
                    );
                }
            }
        }
        if let Some(frames) = &effect.frames {
            for (slot, name) in frames.iter().enumerate() {
                if doc.frame(name).is_none() {
                    let mut diagnostic = Diagnostic::new(
                        code::SPRITE_EFFECT_INVALID,
                        format!("`{name}` names no frame of this document"),
                    )
                    .with_path(format!("{pointer}/frames/{slot}"));
                    let names = doc.frames.iter().map(|frame| frame.name.as_str());
                    diagnostic = match suggest::nearest(name, names) {
                        Some(candidate) => {
                            diagnostic.with_help(format!("nearest match is `{candidate}`"))
                        }
                        None => diagnostic.with_help(
                            "each entry names a frame under `frames:`; remove the `frames` key to \
                             apply the effect to every frame",
                        ),
                    };
                    diagnostics.push(diagnostic);
                }
            }
        }
        validate_effect_op(effect, &pointer, diagnostics);
    }
}

fn validate_effect_op(effect: &SpriteEffect, pointer: &str, diagnostics: &mut Vec<Diagnostic>) {
    match effect.op {
        SpriteEffectOp::Gradient { from, to, .. } => {
            validate_effect_color(from, pointer, "from", diagnostics);
            validate_effect_color(to, pointer, "to", diagnostics);
        }
        SpriteEffectOp::Outline { color, .. } => {
            validate_effect_color(color, pointer, "color", diagnostics);
        }
        SpriteEffectOp::Shade { light, strength } => {
            if !light.iter().all(|v| v.is_finite()) || light == [0.0, 0.0] {
                diagnostics.push(
                    Diagnostic::new(
                        code::SPRITE_EFFECT_INVALID,
                        format!(
                            "the `shade` light direction [{}, {}] is not a direction",
                            light[0], light[1]
                        ),
                    )
                    .with_path(format!("{pointer}/light"))
                    .with_help(
                        "give two finite numbers that are not both zero — the direction the light \
                         comes from in texel space (+x right, +y down)",
                    ),
                );
            }
            validate_unit_range(strength, pointer, "strength", diagnostics);
        }
        SpriteEffectOp::Shadow { offset, color } => {
            if offset == [0, 0] {
                diagnostics.push(
                    Diagnostic::new(
                        code::SPRITE_EFFECT_INVALID,
                        "the `shadow` offset is [0, 0], so the copy would sit exactly under the \
                         sprite and never be seen",
                    )
                    .with_path(format!("{pointer}/offset"))
                    .with_help("offset the copy by at least one texel on one axis"),
                );
            }
            validate_effect_color(color, pointer, "color", diagnostics);
        }
        SpriteEffectOp::Dither { density } => {
            validate_unit_range(density, pointer, "density", diagnostics);
            let count = effect.glyphs.as_ref().map_or(0, Vec::len);
            if count != 2 {
                diagnostics.push(
                    Diagnostic::new(
                        code::SPRITE_EFFECT_INVALID,
                        format!(
                            "the `dither` effect needs exactly two glyphs to alternate between, \
                             found {count}"
                        ),
                    )
                    .with_path(format!("{pointer}/glyphs"))
                    .with_help(
                        "dither is a two-tone checker, so `glyphs` is required here and names the \
                         two tones in order",
                    ),
                );
            }
        }
        SpriteEffectOp::Noise { amount } => {
            validate_unit_range(amount, pointer, "amount", diagnostics);
        }
        SpriteEffectOp::Blur { radius } => {
            let (lo, hi) = BLUR_RADIUS_RANGE;
            if radius < lo || radius > hi {
                diagnostics.push(
                    Diagnostic::new(
                        code::SPRITE_EFFECT_INVALID,
                        format!(
                            "the `blur` radius is {radius} texels, outside the valid range \
                             {lo}..={hi}"
                        ),
                    )
                    .with_path(format!("{pointer}/radius"))
                    .with_help(
                        "a blur wider than a sprite's own frame is a glow to author as a second \
                         sprite, not a larger radius",
                    ),
                );
            }
        }
        SpriteEffectOp::Glow {
            color,
            radius_px,
            strength,
        } => {
            if let Some(color) = color {
                validate_effect_color(color, pointer, "color", diagnostics);
            }
            if !(radius_px.is_finite() && radius_px > 0.0 && radius_px <= GLOW_RADIUS_MAX) {
                diagnostics.push(
                    Diagnostic::new(
                        code::SPRITE_EFFECT_INVALID,
                        format!(
                            "the `glow` radius_px is {radius_px}, outside the valid range \
                             0 < radius_px <= {GLOW_RADIUS_MAX}"
                        ),
                    )
                    .with_path(format!("{pointer}/radius_px"))
                    .with_help(
                        "a glow reach is a positive number of texels — fractional for a small halo; \
                         a halo wider than a large sprite is a second sprite, not a bigger radius",
                    ),
                );
            }
            if !(strength.is_finite() && strength >= 0.0) {
                diagnostics.push(
                    Diagnostic::new(
                        code::SPRITE_EFFECT_INVALID,
                        format!(
                            "the `glow` strength is {strength}; it must be finite and at least 0"
                        ),
                    )
                    .with_path(format!("{pointer}/strength"))
                    .with_help(
                        "strength scales the halo's alpha; the result is clamped to 0..=1, so a \
                         value above 1 pushes the near-core halo toward opaque",
                    ),
                );
            }
        }
    }
}

fn validate_effect_color(
    color: [f64; 4],
    pointer: &str,
    field: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (index, &component) in color.iter().enumerate() {
        if !(component.is_finite() && (0.0..=1.0).contains(&component)) {
            diagnostics.push(
                Diagnostic::new(
                    code::SPRITE_EFFECT_INVALID,
                    format!(
                        "`{field}` component {index} is {component}, outside the valid range [0, 1]"
                    ),
                )
                .with_path(format!("{pointer}/{field}/{index}"))
                .with_help("effect colors are linear RGBA in 0..=1, like the palette's"),
            );
        }
    }
}

fn validate_unit_range(value: f64, pointer: &str, field: &str, diagnostics: &mut Vec<Diagnostic>) {
    if !(value.is_finite() && (0.0..=1.0).contains(&value)) {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_EFFECT_INVALID,
                format!("`{field}` is {value}, outside the valid range [0, 1]"),
            )
            .with_path(format!("{pointer}/{field}"))
            .with_help(format!("give `{field}` a finite value between 0 and 1")),
        );
    }
}

// --- Emit (canonical, minimal) ------------------------------------------------------------------

/// Writes `doc` in canonical form: `format`, `kind`, `name` (optional), `size`, `seed` (optional),
/// `palette` (sorted by glyph), `frames` (authored order), `effects` (authored order), ryu
/// shortest-round-trip floats — the sprite analog of [`crate::voxel::emit`].
///
/// **Minimal by rule:** an effect field whose value equals its default is not written, and an empty
/// `effects` list is written as nothing at all. So a document that spells a default out loud is not
/// canonical and re-emits shorter; emitting an already-emitted document is byte-identical. A colour is
/// written as quoted `"#rrggbb[aa]"` when it is exactly a hex colour, else as four floats.
pub(crate) fn emit(doc: &SpriteDocument) -> String {
    let mut out = String::new();
    out.push_str("format: 1\n");
    out.push_str("kind: sprite\n");
    if let Some(name) = &doc.name {
        out.push_str("name: ");
        out.push_str(&scalar_string(name));
        out.push('\n');
    }

    out.push_str("size:\n");
    for dim in doc.size {
        out.push_str("  - ");
        out.push_str(&dim.to_string());
        out.push('\n');
    }

    if let Some(seed) = doc.seed {
        out.push_str("seed: ");
        out.push_str(&seed.to_string());
        out.push('\n');
    }

    match &doc.palette_include {
        Some(include) => emit_palette_include(&mut out, include),
        None => emit_palette(&mut out, &doc.palette),
    }
    emit_frames(&mut out, &doc.frames);
    emit_effects(&mut out, &doc.effects);
    out
}

/// Writes a `palette:` include in canonical form: `from`, then `overrides` (glyph-sorted, one nesting
/// level below an inline palette) — omitted entirely when there are none.
fn emit_palette_include(out: &mut String, include: &SpritePaletteInclude) {
    out.push_str("palette:\n  from: ");
    out.push_str(&scalar_string(&include.from));
    out.push('\n');
    if include.overrides.is_empty() {
        return;
    }
    out.push_str("  overrides:\n");
    let mut overrides = include.overrides.clone();
    overrides.sort_by(|a, b| a.glyph.cmp(&b.glyph));
    for entry in &overrides {
        out.push_str("    ");
        out.push_str(&scalar_string(&entry.glyph));
        out.push_str(":\n");
        emit_color_value(out, "      ", "color", entry.color);
        if let Some(emissive) = entry.emissive {
            out.push_str("      emissive: ");
            out.push_str(&format_float(emissive));
            out.push('\n');
        }
    }
}

pub(crate) fn emit_palette(out: &mut String, palette: &[SpritePaletteEntry]) {
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
        out.push_str(":\n");
        emit_color_value(out, "    ", "color", entry.color);
        if let Some(emissive) = entry.emissive {
            out.push_str("    emissive: ");
            out.push_str(&format_float(emissive));
            out.push('\n');
        }
    }
}

fn emit_frames(out: &mut String, frames: &[SpriteFrame]) {
    if frames.is_empty() {
        out.push_str("frames: {}\n");
        return;
    }
    out.push_str("frames:\n");
    for frame in frames {
        out.push_str("  ");
        out.push_str(&scalar_string(&frame.name));
        out.push_str(":\n");
        if let Some(pivot) = frame.pivot {
            out.push_str("    pivot:\n");
            for component in pivot {
                out.push_str("      - ");
                out.push_str(&format_float(component));
                out.push('\n');
            }
        }
        match &frame.grid {
            SpriteFrameGrid::Rows { rows, mirror } => {
                if let Some(mirror) = mirror {
                    out.push_str("    mirror: ");
                    out.push_str(mirror.as_str());
                    out.push('\n');
                }
                emit_rows(out, "    ", rows);
            }
            SpriteFrameGrid::Patched { base, patches } => {
                out.push_str("    base: ");
                out.push_str(&scalar_string(base));
                out.push('\n');
                if patches.is_empty() {
                    out.push_str("    patches: []\n");
                } else {
                    out.push_str("    patches:\n");
                    for patch in patches {
                        out.push_str("      - at:\n");
                        for component in patch.at {
                            out.push_str(&format!("          - {component}\n"));
                        }
                        emit_rows(out, "        ", &patch.rows);
                    }
                }
            }
        }
    }
}

fn emit_rows(out: &mut String, indent: &str, rows: &[String]) {
    if rows.is_empty() {
        out.push_str(&format!("{indent}rows: []\n"));
    } else {
        out.push_str(&format!("{indent}rows:\n"));
        for row in rows {
            out.push_str(&format!("{indent}  - "));
            out.push_str(&scalar_string(row));
            out.push('\n');
        }
    }
}

fn emit_effects(out: &mut String, effects: &[SpriteEffect]) {
    if effects.is_empty() {
        return;
    }
    out.push_str("effects:\n");
    for effect in effects {
        out.push_str("  - op: ");
        out.push_str(op_name(&effect.op));
        out.push('\n');
        if let Some(glyphs) = &effect.glyphs {
            emit_name_list(out, "glyphs", glyphs);
        }
        if let Some(frames) = &effect.frames {
            emit_name_list(out, "frames", frames);
        }
        match effect.op {
            SpriteEffectOp::Gradient {
                from,
                to,
                direction,
                blend,
            } => {
                emit_color(out, "from", from);
                emit_color(out, "to", to);
                if direction != GRADIENT_DIRECTION_DEFAULT {
                    out.push_str("    direction: ");
                    out.push_str(match direction {
                        GradientDirection::Vertical => "vertical",
                        GradientDirection::Horizontal => "horizontal",
                        GradientDirection::Radial => "radial",
                    });
                    out.push('\n');
                }
                if blend != GRADIENT_BLEND_DEFAULT {
                    out.push_str("    blend: ");
                    out.push_str(match blend {
                        GradientBlend::Replace => "replace",
                        GradientBlend::Multiply => "multiply",
                    });
                    out.push('\n');
                }
            }
            SpriteEffectOp::Outline { color, side } => {
                emit_color(out, "color", color);
                if side != OUTLINE_SIDE_DEFAULT {
                    out.push_str("    side: ");
                    out.push_str(match side {
                        OutlineSide::Outside => "outside",
                        OutlineSide::Inside => "inside",
                    });
                    out.push('\n');
                }
            }
            SpriteEffectOp::Shade { light, strength } => {
                if light != SHADE_LIGHT_DEFAULT {
                    out.push_str("    light:\n");
                    for component in light {
                        out.push_str("      - ");
                        out.push_str(&format_float(component));
                        out.push('\n');
                    }
                }
                if strength != SHADE_STRENGTH_DEFAULT {
                    out.push_str("    strength: ");
                    out.push_str(&format_float(strength));
                    out.push('\n');
                }
            }
            SpriteEffectOp::Shadow { offset, color } => {
                if offset != SHADOW_OFFSET_DEFAULT {
                    out.push_str("    offset:\n");
                    for component in offset {
                        out.push_str("      - ");
                        out.push_str(&component.to_string());
                        out.push('\n');
                    }
                }
                if color != SHADOW_COLOR_DEFAULT {
                    emit_color(out, "color", color);
                }
            }
            SpriteEffectOp::Dither { density } => {
                if density != DITHER_DENSITY_DEFAULT {
                    out.push_str("    density: ");
                    out.push_str(&format_float(density));
                    out.push('\n');
                }
            }
            SpriteEffectOp::Noise { amount } => {
                out.push_str("    amount: ");
                out.push_str(&format_float(amount));
                out.push('\n');
            }
            SpriteEffectOp::Blur { radius } => {
                out.push_str("    radius: ");
                out.push_str(&radius.to_string());
                out.push('\n');
            }
            SpriteEffectOp::Glow {
                color,
                radius_px,
                strength,
            } => {
                if let Some(color) = color {
                    emit_color(out, "color", color);
                }
                out.push_str("    radius_px: ");
                out.push_str(&format_float(radius_px));
                out.push('\n');
                out.push_str("    strength: ");
                out.push_str(&format_float(strength));
                out.push('\n');
            }
        }
    }
}

/// The `op:` keyword one variant is written as — the inverse of [`parse_effect`]'s dispatch.
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

fn emit_name_list(out: &mut String, key: &str, names: &[String]) {
    if names.is_empty() {
        out.push_str("    ");
        out.push_str(key);
        out.push_str(": []\n");
        return;
    }
    out.push_str("    ");
    out.push_str(key);
    out.push_str(":\n");
    for name in names {
        out.push_str("      - ");
        out.push_str(&scalar_string(name));
        out.push('\n');
    }
}

fn emit_color(out: &mut String, key: &str, color: [f64; 4]) {
    emit_color_value(out, "    ", key, color);
}

/// Writes one colour field: the quoted lowercase hex when the colour is hex-exact, else the legacy
/// four-float sequence (so the writer never quantizes).
fn emit_color_value(out: &mut String, indent: &str, key: &str, color: [f64; 4]) {
    out.push_str(indent);
    out.push_str(key);
    if let Some(hex) = ludion_core::color::hex_of_linear(color) {
        out.push_str(": \"");
        out.push_str(&hex);
        out.push_str("\"\n");
        return;
    }
    out.push_str(":\n");
    for component in color {
        out.push_str(indent);
        out.push_str("  - ");
        out.push_str(&format_float(component));
        out.push('\n');
    }
}

#[cfg(test)]
#[path = "sprite_tests.rs"]
mod tests;
