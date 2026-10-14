//! Font documents: `*.font.yaml` — the glyph table, metrics and atlas geometry of one face
//! (`Plan/font-assets.md`, decision 3).
//!
//! A `kind: font` document describes an atlas image: for each glyph, its advance and its rect in
//! that image, plus the face's vertical metrics, sparse kerning and character aliases. Metrics are
//! integers in font units (`units_per_em`); atlas rects are texels. The document reads no file —
//! `image` names the PNG a loader resolves — so the whole format is a pure text model, in the
//! crate's usual split: [`parse`] checks structure/shape, [`validate`] checks semantics, and
//! [`emit`] writes canonical text, all under `L1250`.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use serde_json::Value;

use crate::envelope;
use crate::scalar::scalar_string;
use crate::yaml::{self, MapEntry, Node, NodeValue, pointer_token};

/// The font document format version this crate reads and writes.
pub const FONT_FORMAT_VERSION: u64 = 1;

const FONT_KEYS: [&str; 15] = [
    "format",
    "kind",
    "generated_from",
    "image",
    "mode",
    "units_per_em",
    "ascender",
    "descender",
    "line_gap",
    "atlas",
    "notdef",
    "glyphs",
    "aliases",
    "kerning",
    "missing",
];
const ATLAS_KEYS: [&str; 4] = ["width", "height", "em_px", "spread_px"];
const NOTDEF_KEYS: [&str; 3] = ["advance", "bounds", "rect"];
const GLYPH_KEYS: [&str; 4] = ["char", "advance", "bounds", "rect"];

/// How a face's atlas encodes coverage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FontMode {
    /// A signed distance field: one atlas serves every size; needs `spread_px`.
    Sdf,
    /// Plain coverage at the face's native pixel size.
    #[default]
    Bitmap,
}

impl FontMode {
    /// The mode's document keyword.
    pub fn keyword(self) -> &'static str {
        match self {
            FontMode::Sdf => "sdf",
            FontMode::Bitmap => "bitmap",
        }
    }
}

/// The atlas image's geometry.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FontAtlas {
    /// Image width in texels.
    pub width: u32,
    /// Image height in texels.
    pub height: u32,
    /// The pixel size the glyphs were baked at.
    pub em_px: u32,
    /// The SDF falloff in texels; `Some` iff the face is [`FontMode::Sdf`].
    pub spread_px: Option<u32>,
}

/// The fallback drawn for a char with no glyph: an advance and, optionally, an image.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FontNotdef {
    /// Horizontal advance in font units.
    pub advance: i32,
    /// Ink bounds `[x0, y0, x1, y1]` in font units, set together with `rect`.
    pub bounds: Option<[i32; 4]>,
    /// Atlas rect `[x, y, w, h]` in texels, set together with `bounds`.
    pub rect: Option<[u32; 4]>,
}

/// One glyph: its char, advance, and (for an inked glyph) bounds and atlas rect.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FontGlyph {
    /// The character this glyph draws.
    pub ch: char,
    /// Horizontal advance in font units.
    pub advance: i32,
    /// Ink bounds `[x0, y0, x1, y1]` in font units, set together with `rect`.
    pub bounds: Option<[i32; 4]>,
    /// Atlas rect `[x, y, w, h]` in texels, set together with `bounds`.
    pub rect: Option<[u32; 4]>,
}

/// A parsed `kind: font` document.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FontDocument {
    /// The source document a bake derived this from; absent on an authored face.
    pub generated_from: Option<String>,
    /// The atlas PNG, relative to this document.
    pub image: String,
    /// How the atlas encodes coverage.
    pub mode: FontMode,
    /// Font units per em; every metric below is in these units.
    pub units_per_em: u32,
    /// Distance from the baseline up to the line top.
    pub ascender: i32,
    /// Distance from the baseline to the line bottom (zero or negative).
    pub descender: i32,
    /// Extra space between lines.
    pub line_gap: i32,
    /// The atlas geometry.
    pub atlas: FontAtlas,
    /// The fallback for an unmapped char.
    pub notdef: FontNotdef,
    /// Glyphs, strictly ascending by code point.
    pub glyphs: Vec<FontGlyph>,
    /// `(from, to)` char aliases, sorted by `from`.
    pub aliases: Vec<(char, char)>,
    /// `(left, right, adjust)` kerning in font units, sorted by `(left, right)`.
    pub kerning: Vec<(char, char, i32)>,
    /// Requested chars the face lacks, ascending.
    pub missing: Vec<char>,
}

// --- Parse --------------------------------------------------------------------------------------

fn diag(pointer: &str, message: String, help: &str) -> Diagnostic {
    Diagnostic::new(code::FONT_DOCUMENT_STRUCTURE, message)
        .with_path(pointer)
        .with_help(help)
}

fn expect_mapping<'a>(
    node: &'a Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<&'a [MapEntry]> {
    match &node.value {
        NodeValue::Mapping(entries) => Some(entries),
        other => {
            diagnostics.push(diag(
                pointer,
                format!(
                    "{what} must be a mapping, found {} (line {})",
                    other.type_phrase(),
                    node.line
                ),
                "write it as indented `key: value` lines",
            ));
            None
        }
    }
}

fn expect_int(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<i64> {
    match &node.value {
        NodeValue::Scalar(Value::Number(n)) if n.as_i64().is_some() => n.as_i64(),
        other => {
            diagnostics.push(diag(
                pointer,
                format!(
                    "{what} must be an integer, found {} (line {})",
                    other.type_phrase(),
                    node.line
                ),
                "font metrics and atlas geometry are whole numbers",
            ));
            None
        }
    }
}

fn expect_i32(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<i32> {
    let value = expect_int(node, pointer, what, diagnostics)?;
    match i32::try_from(value) {
        Ok(v) => Some(v),
        Err(_) => {
            diagnostics.push(diag(
                pointer,
                format!("{what} is {value}, outside the 32-bit range"),
                "use a smaller value",
            ));
            None
        }
    }
}

fn expect_u32(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<u32> {
    let value = expect_int(node, pointer, what, diagnostics)?;
    match u32::try_from(value) {
        Ok(v) => Some(v),
        Err(_) => {
            diagnostics.push(diag(
                pointer,
                format!("{what} is {value}; it must be a non-negative 32-bit integer"),
                "use a value of zero or more",
            ));
            None
        }
    }
}

/// A single-char string; a bare `1` resolves as a number, so digits must be quoted.
fn expect_char(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<char> {
    let text = envelope::expect_string(
        node,
        pointer,
        what,
        code::FONT_DOCUMENT_STRUCTURE,
        diagnostics,
    )?;
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c),
        _ => {
            diagnostics.push(diag(
                pointer,
                format!(
                    "{what} must be exactly one character, found {text:?} (line {})",
                    node.line
                ),
                "write one character, quoted if it is a digit or punctuation",
            ));
            None
        }
    }
}

fn expect_array<const N: usize>(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<[i64; N]> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(diag(
            pointer,
            format!(
                "{what} must be a sequence of {N} integers, found {} (line {})",
                node.value.type_phrase(),
                node.line
            ),
            "write one `- <integer>` line per component",
        ));
        return None;
    };
    if items.len() != N {
        diagnostics.push(diag(
            pointer,
            format!(
                "{what} must have {N} components, found {} (line {})",
                items.len(),
                node.line
            ),
            "write one `- <integer>` line per component",
        ));
        return None;
    }
    let mut out = [0i64; N];
    let mut ok = true;
    for (i, item) in items.iter().enumerate() {
        match expect_int(item, &format!("{pointer}/{i}"), what, diagnostics) {
            Some(v) => out[i] = v,
            None => ok = false,
        }
    }
    ok.then_some(out)
}

fn expect_bounds(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<[i32; 4]> {
    let raw = expect_array::<4>(node, pointer, "`bounds`", diagnostics)?;
    let mut out = [0i32; 4];
    for (slot, value) in out.iter_mut().zip(raw) {
        match i32::try_from(value) {
            Ok(v) => *slot = v,
            Err(_) => {
                diagnostics.push(diag(
                    pointer,
                    format!("`bounds` component {value} is outside the 32-bit range"),
                    "use a smaller value",
                ));
                return None;
            }
        }
    }
    Some(out)
}

fn expect_rect(node: &Node, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<[u32; 4]> {
    let raw = expect_array::<4>(node, pointer, "`rect`", diagnostics)?;
    let mut out = [0u32; 4];
    for (slot, value) in out.iter_mut().zip(raw) {
        match u32::try_from(value) {
            Ok(v) => *slot = v,
            Err(_) => {
                diagnostics.push(diag(
                    pointer,
                    format!("`rect` component {value} must be a non-negative 32-bit integer"),
                    "a rect is `[x, y, w, h]` in texels",
                ));
                return None;
            }
        }
    }
    Some(out)
}

/// Parses strict-subset YAML into a [`FontDocument`], checking structure/shape only (`L1250`).
pub(crate) fn parse(source: &str) -> Result<FontDocument, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![diag(
            "",
            "the font document is empty; a font needs at least `format: 1`, `kind: font`, \
                 `image`, `mode`, the metrics, `atlas`, `notdef` and `glyphs`"
                .to_string(),
            "see `ludion docs` for a minimal font document",
        )]);
    };
    let NodeValue::Mapping(entries) = root.value else {
        return Err(vec![diag(
            "",
            format!(
                "the font document root must be a mapping, found {} (line {})",
                root.value.type_phrase(),
                root.line
            ),
            "start the file with the top-level keys `format:` and `kind: font`",
        )]);
    };

    let mut diagnostics = Vec::new();
    let mut doc = FontDocument::default();
    let mut seen = [false; FONT_KEYS.len()];
    for entry in &entries {
        if let Some(i) = FONT_KEYS.iter().position(|k| *k == entry.key) {
            seen[i] = true;
        }
        match entry.key.as_str() {
            "format" => envelope::check_envelope(
                entry,
                code::FONT_DOCUMENT_STRUCTURE,
                code::FONT_DOCUMENT_STRUCTURE,
                FONT_FORMAT_VERSION,
                "the font document",
                "font",
                "font",
                &mut diagnostics,
            ),
            "kind" => envelope::check_kind(
                entry,
                code::FONT_DOCUMENT_STRUCTURE,
                "font",
                "a font document",
                &mut diagnostics,
            ),
            "generated_from" => {
                doc.generated_from = envelope::expect_string(
                    &entry.node,
                    "/generated_from",
                    "`generated_from`",
                    code::FONT_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "image" => {
                doc.image = envelope::expect_string(
                    &entry.node,
                    "/image",
                    "`image`",
                    code::FONT_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                )
                .unwrap_or_default();
            }
            "mode" => parse_mode(entry, &mut doc, &mut diagnostics),
            "units_per_em" => {
                doc.units_per_em = expect_u32(
                    &entry.node,
                    "/units_per_em",
                    "`units_per_em`",
                    &mut diagnostics,
                )
                .unwrap_or_default();
            }
            "ascender" => {
                doc.ascender = expect_i32(&entry.node, "/ascender", "`ascender`", &mut diagnostics)
                    .unwrap_or_default();
            }
            "descender" => {
                doc.descender =
                    expect_i32(&entry.node, "/descender", "`descender`", &mut diagnostics)
                        .unwrap_or_default();
            }
            "line_gap" => {
                doc.line_gap = expect_i32(&entry.node, "/line_gap", "`line_gap`", &mut diagnostics)
                    .unwrap_or_default();
            }
            "atlas" => parse_atlas(entry, &mut doc, &mut diagnostics),
            "notdef" => parse_notdef(entry, &mut doc, &mut diagnostics),
            "glyphs" => parse_glyphs(entry, &mut doc, &mut diagnostics),
            "aliases" => parse_aliases(entry, &mut doc, &mut diagnostics),
            "kerning" => parse_kerning(entry, &mut doc, &mut diagnostics),
            "missing" => parse_missing(entry, &mut doc, &mut diagnostics),
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "",
                "a font document",
                FONT_KEYS,
                code::FONT_DOCUMENT_STRUCTURE,
            )),
        }
    }

    const REQUIRED: [(&str, &str); 10] = [
        ("format", "add `format: 1` as the first line"),
        ("kind", "add `kind: font`"),
        ("image", "add `image:` naming the atlas PNG"),
        ("mode", "add `mode: sdf` or `mode: bitmap`"),
        ("units_per_em", "add `units_per_em:`, e.g. `2048`"),
        ("ascender", "add `ascender:` in font units"),
        (
            "descender",
            "add `descender:` in font units (zero or negative)",
        ),
        ("line_gap", "add `line_gap:` in font units (zero if none)"),
        (
            "atlas",
            "add an `atlas` mapping with `width`, `height` and `em_px`",
        ),
        ("notdef", "add a `notdef` mapping with an `advance`"),
    ];
    for (key, help) in REQUIRED {
        let i = FONT_KEYS
            .iter()
            .position(|k| *k == key)
            .expect("a font key");
        if !seen[i] {
            diagnostics.push(envelope::missing_key(
                key,
                help,
                code::FONT_DOCUMENT_STRUCTURE,
                "font document",
            ));
        }
    }
    if !seen[FONT_KEYS
        .iter()
        .position(|k| *k == "glyphs")
        .expect("a font key")]
    {
        diagnostics.push(envelope::missing_key(
            "glyphs",
            "add a `glyphs` list (`glyphs: []` for none)",
            code::FONT_DOCUMENT_STRUCTURE,
            "font document",
        ));
    }

    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

fn parse_mode(entry: &MapEntry, doc: &mut FontDocument, diagnostics: &mut Vec<Diagnostic>) {
    let Some(text) = envelope::expect_string(
        &entry.node,
        "/mode",
        "`mode`",
        code::FONT_DOCUMENT_STRUCTURE,
        diagnostics,
    ) else {
        return;
    };
    match text.as_str() {
        "sdf" => doc.mode = FontMode::Sdf,
        "bitmap" => doc.mode = FontMode::Bitmap,
        other => diagnostics.push(diag(
            "/mode",
            format!("unknown font mode `{other}` (line {})", entry.node.line),
            "`mode` is `sdf` (a vector face) or `bitmap` (a pixel face)",
        )),
    }
}

fn parse_atlas(entry: &MapEntry, doc: &mut FontDocument, diagnostics: &mut Vec<Diagnostic>) {
    let Some(entries) = expect_mapping(&entry.node, "/atlas", "`atlas`", diagnostics) else {
        return;
    };
    let (mut width, mut height, mut em_px) = (false, false, false);
    for e in entries {
        let pointer = format!("/atlas/{}", pointer_token(&e.key));
        match e.key.as_str() {
            "width" => {
                width = true;
                doc.atlas.width =
                    expect_u32(&e.node, &pointer, "`width`", diagnostics).unwrap_or_default();
            }
            "height" => {
                height = true;
                doc.atlas.height =
                    expect_u32(&e.node, &pointer, "`height`", diagnostics).unwrap_or_default();
            }
            "em_px" => {
                em_px = true;
                doc.atlas.em_px =
                    expect_u32(&e.node, &pointer, "`em_px`", diagnostics).unwrap_or_default();
            }
            "spread_px" => {
                doc.atlas.spread_px = expect_u32(&e.node, &pointer, "`spread_px`", diagnostics);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                e,
                "/atlas",
                "the atlas mapping",
                ATLAS_KEYS,
                code::FONT_DOCUMENT_STRUCTURE,
            )),
        }
    }
    for (seen, key) in [(width, "width"), (height, "height"), (em_px, "em_px")] {
        if !seen {
            diagnostics.push(diag(
                "/atlas",
                format!("the atlas mapping is missing the required key `{key}`"),
                &format!("add `{key}:` to `atlas`"),
            ));
        }
    }
}

/// The fields a `notdef` and a glyph entry share, each `None` when absent or malformed.
type ParsedCell = (
    Option<char>,
    Option<i32>,
    Option<[i32; 4]>,
    Option<[u32; 4]>,
);

/// Parses the keys a `notdef` and a glyph entry share; `char` is the glyph's own.
fn parse_cell(
    entries: &[MapEntry],
    pointer: &str,
    glyph: bool,
    diagnostics: &mut Vec<Diagnostic>,
) -> ParsedCell {
    let (mut ch, mut advance, mut bounds, mut rect) = (None, None, None, None);
    for e in entries {
        let p = format!("{pointer}/{}", pointer_token(&e.key));
        match e.key.as_str() {
            "char" if glyph => ch = expect_char(&e.node, &p, "`char`", diagnostics),
            "advance" => advance = expect_i32(&e.node, &p, "`advance`", diagnostics),
            "bounds" => bounds = expect_bounds(&e.node, &p, diagnostics),
            "rect" => rect = expect_rect(&e.node, &p, diagnostics),
            unknown if glyph => diagnostics.push(envelope::unknown_key(
                unknown,
                e,
                pointer,
                "a glyph entry",
                GLYPH_KEYS,
                code::FONT_DOCUMENT_STRUCTURE,
            )),
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                e,
                pointer,
                "the notdef mapping",
                NOTDEF_KEYS,
                code::FONT_DOCUMENT_STRUCTURE,
            )),
        }
    }
    (ch, advance, bounds, rect)
}

fn bounds_rect_together(
    entries: &[MapEntry],
    pointer: &str,
    node_line: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let has = |k: &str| entries.iter().any(|e| e.key == k);
    if has("bounds") != has("rect") {
        diagnostics.push(diag(
            pointer,
            format!("`bounds` and `rect` must be set together (line {node_line})"),
            "an inked glyph has both; a blank one (a space) has neither",
        ));
    }
}

fn parse_notdef(entry: &MapEntry, doc: &mut FontDocument, diagnostics: &mut Vec<Diagnostic>) {
    let Some(entries) = expect_mapping(&entry.node, "/notdef", "`notdef`", diagnostics) else {
        return;
    };
    let (_, advance, bounds, rect) = parse_cell(entries, "/notdef", false, diagnostics);
    bounds_rect_together(entries, "/notdef", entry.node.line, diagnostics);
    match advance {
        Some(advance) => {
            doc.notdef = FontNotdef {
                advance,
                bounds,
                rect,
            }
        }
        None if !entries.iter().any(|e| e.key == "advance") => diagnostics.push(diag(
            "/notdef",
            "the notdef mapping is missing the required key `advance`".to_string(),
            "add `advance:` in font units",
        )),
        None => {}
    }
}

fn parse_glyphs(entry: &MapEntry, doc: &mut FontDocument, diagnostics: &mut Vec<Diagnostic>) {
    let NodeValue::Sequence(items) = &entry.node.value else {
        diagnostics.push(diag(
            "/glyphs",
            format!(
                "`glyphs` must be a list, found {} (line {})",
                entry.node.value.type_phrase(),
                entry.node.line
            ),
            "write one `- char: …` item per glyph",
        ));
        return;
    };
    for (i, item) in items.iter().enumerate() {
        let pointer = format!("/glyphs/{i}");
        let Some(entries) = expect_mapping(item, &pointer, "a glyph entry", diagnostics) else {
            continue;
        };
        let (ch, advance, bounds, rect) = parse_cell(entries, &pointer, true, diagnostics);
        bounds_rect_together(entries, &pointer, item.line, diagnostics);
        for key in ["char", "advance"] {
            if !entries.iter().any(|e| e.key == key) {
                diagnostics.push(diag(
                    &pointer,
                    format!(
                        "a glyph entry is missing the required key `{key}` (line {})",
                        item.line
                    ),
                    &format!("add `{key}:` to the glyph"),
                ));
            }
        }
        if let (Some(ch), Some(advance)) = (ch, advance) {
            doc.glyphs.push(FontGlyph {
                ch,
                advance,
                bounds,
                rect,
            });
        }
    }
}

fn parse_aliases(entry: &MapEntry, doc: &mut FontDocument, diagnostics: &mut Vec<Diagnostic>) {
    let Some(entries) = expect_mapping(&entry.node, "/aliases", "`aliases`", diagnostics) else {
        return;
    };
    for e in entries {
        let pointer = format!("/aliases/{}", pointer_token(&e.key));
        let mut chars = e.key.chars();
        let (Some(from), None) = (chars.next(), chars.next()) else {
            diagnostics.push(diag(
                &pointer,
                format!(
                    "alias key {:?} must be exactly one character (line {})",
                    e.key, e.key_line
                ),
                "alias keys and values are single characters",
            ));
            continue;
        };
        if let Some(to) = expect_char(&e.node, &pointer, "an alias target", diagnostics) {
            doc.aliases.push((from, to));
        }
    }
    doc.aliases.sort_unstable();
}

fn parse_kerning(entry: &MapEntry, doc: &mut FontDocument, diagnostics: &mut Vec<Diagnostic>) {
    let Some(lefts) = expect_mapping(&entry.node, "/kerning", "`kerning`", diagnostics) else {
        return;
    };
    for left in lefts {
        let lp = format!("/kerning/{}", pointer_token(&left.key));
        let mut chars = left.key.chars();
        let (Some(l), None) = (chars.next(), chars.next()) else {
            diagnostics.push(diag(
                &lp,
                format!(
                    "kerning key {:?} must be exactly one character (line {})",
                    left.key, left.key_line
                ),
                "kerning is `left: { right: adjust }` over single characters",
            ));
            continue;
        };
        let Some(rights) = expect_mapping(&left.node, &lp, "a kerning row", diagnostics) else {
            continue;
        };
        for right in rights {
            let rp = format!("{lp}/{}", pointer_token(&right.key));
            let mut chars = right.key.chars();
            let (Some(r), None) = (chars.next(), chars.next()) else {
                diagnostics.push(diag(
                    &rp,
                    format!(
                        "kerning key {:?} must be exactly one character (line {})",
                        right.key, right.key_line
                    ),
                    "kerning is `left: { right: adjust }` over single characters",
                ));
                continue;
            };
            if let Some(adjust) = expect_i32(&right.node, &rp, "a kerning adjustment", diagnostics)
            {
                doc.kerning.push((l, r, adjust));
            }
        }
    }
    doc.kerning.sort_unstable();
}

fn parse_missing(entry: &MapEntry, doc: &mut FontDocument, diagnostics: &mut Vec<Diagnostic>) {
    let NodeValue::Sequence(items) = &entry.node.value else {
        diagnostics.push(diag(
            "/missing",
            format!(
                "`missing` must be a list, found {} (line {})",
                entry.node.value.type_phrase(),
                entry.node.line
            ),
            "write one `- \"<char>\"` item per missing character",
        ));
        return;
    };
    for (i, item) in items.iter().enumerate() {
        if let Some(c) = expect_char(
            item,
            &format!("/missing/{i}"),
            "a missing char",
            diagnostics,
        ) {
            doc.missing.push(c);
        }
    }
    doc.missing.sort_unstable();
    doc.missing.dedup();
}

// --- Validate -----------------------------------------------------------------------------------

/// The bound on every font-unit integer. Four such values sum without overflowing `i32` (a line's
/// `ascender - descender + line_gap`), and an advance plus a kerning adjust stays far inside it, so
/// the layout's `i32` arithmetic and its `i64` line sums cannot overflow.
const MAX_FONT_UNITS: i32 = 1 << 20;
/// The largest `units_per_em`; the embedded face uses 8, TrueType allows 16384.
const MAX_UNITS_PER_EM: u32 = 16384;

/// Validates semantic invariants of a [`FontDocument`] (`L1250`).
pub(crate) fn validate(doc: &FontDocument) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if let Some(refusal) = ludion_core::paths::relative_refusal(&doc.image) {
        diagnostics.push(diag(
            "/image",
            format!("the image path {}", refusal.describe()),
            "set `image:` to the PNG's path relative to this document, e.g. `inter.png`",
        ));
    }
    if doc.units_per_em == 0 || doc.units_per_em > MAX_UNITS_PER_EM {
        diagnostics.push(diag(
            "/units_per_em",
            format!(
                "`units_per_em` is {}; it must be in 1..={MAX_UNITS_PER_EM}",
                doc.units_per_em
            ),
            "set `units_per_em` to e.g. 2048, or the pixel size of a bitmap face",
        ));
    }
    let check_units = |pointer: &str, what: &str, v: i32, diagnostics: &mut Vec<Diagnostic>| {
        if !(-MAX_FONT_UNITS..=MAX_FONT_UNITS).contains(&v) {
            diagnostics.push(diag(
                pointer,
                format!("{what} is {v}; font units must lie in ±{MAX_FONT_UNITS}"),
                "a face's metrics are a few thousand font units; check the document for a typo",
            ));
        }
    };
    check_units("/ascender", "`ascender`", doc.ascender, &mut diagnostics);
    check_units("/descender", "`descender`", doc.descender, &mut diagnostics);
    check_units("/line_gap", "`line_gap`", doc.line_gap, &mut diagnostics);
    check_units(
        "/notdef/advance",
        "the notdef advance",
        doc.notdef.advance,
        &mut diagnostics,
    );
    for (pointer, bounds) in std::iter::once(("/notdef/bounds".to_string(), doc.notdef.bounds))
        .chain(
            doc.glyphs
                .iter()
                .enumerate()
                .map(|(i, g)| (format!("/glyphs/{i}/bounds"), g.bounds)),
        )
    {
        for v in bounds.into_iter().flatten() {
            check_units(&pointer, "a bounds value", v, &mut diagnostics);
        }
    }
    for (i, glyph) in doc.glyphs.iter().enumerate() {
        check_units(
            &format!("/glyphs/{i}/advance"),
            &format!("the advance of {:?}", glyph.ch),
            glyph.advance,
            &mut diagnostics,
        );
    }
    for (left, right, adjust) in &doc.kerning {
        check_units(
            &format!(
                "/kerning/{}/{}",
                pointer_token(&left.to_string()),
                pointer_token(&right.to_string())
            ),
            "a kerning adjust",
            *adjust,
            &mut diagnostics,
        );
    }
    let line = i64::from(doc.ascender) - i64::from(doc.descender) + i64::from(doc.line_gap);
    if line <= 0 {
        diagnostics.push(diag(
            "/ascender",
            format!("`ascender - descender + line_gap` is {line}; a line needs a positive height"),
            "the descender is zero or negative, so `ascender` should be positive",
        ));
    }
    match (doc.mode, doc.atlas.spread_px) {
        (FontMode::Sdf, None) => diagnostics.push(diag(
            "/atlas/spread_px",
            "an `sdf` face needs `atlas.spread_px`".to_string(),
            "add `spread_px:` — the distance-field falloff in texels",
        )),
        (FontMode::Bitmap, Some(_)) => diagnostics.push(diag(
            "/atlas/spread_px",
            "a `bitmap` face has no distance field; `atlas.spread_px` is not allowed".to_string(),
            "remove `spread_px`, or set `mode: sdf`",
        )),
        _ => {}
    }

    let atlas = &doc.atlas;
    let check_rect = |pointer: &str, rect: [u32; 4], diagnostics: &mut Vec<Diagnostic>| {
        let [x, y, w, h] = rect;
        if w == 0 || h == 0 {
            diagnostics.push(diag(
                pointer,
                format!("rect {rect:?} has no area"),
                "a rect is `[x, y, w, h]` with `w` and `h` above zero",
            ));
        } else if u64::from(x) + u64::from(w) > u64::from(atlas.width)
            || u64::from(y) + u64::from(h) > u64::from(atlas.height)
        {
            diagnostics.push(diag(
                pointer,
                format!(
                    "rect {rect:?} lies outside the {}×{} atlas",
                    atlas.width, atlas.height
                ),
                "move or shrink the rect so it fits the image",
            ));
        }
    };
    let check_bounds = |pointer: &str, b: [i32; 4], diagnostics: &mut Vec<Diagnostic>| {
        if b[0] > b[2] || b[1] > b[3] {
            diagnostics.push(diag(
                pointer,
                format!("bounds {b:?} are inverted"),
                "bounds are `[x0, y0, x1, y1]` with `x0 <= x1` and `y0 <= y1`",
            ));
        }
    };

    if doc.notdef.advance < 0 {
        diagnostics.push(diag(
            "/notdef/advance",
            format!(
                "the notdef advance is {}; it must not be negative",
                doc.notdef.advance
            ),
            "advances are zero or more",
        ));
    }
    if let Some(b) = doc.notdef.bounds {
        check_bounds("/notdef/bounds", b, &mut diagnostics);
    }
    if let Some(r) = doc.notdef.rect {
        check_rect("/notdef/rect", r, &mut diagnostics);
    }
    let mut previous: Option<char> = None;
    for (i, glyph) in doc.glyphs.iter().enumerate() {
        let pointer = format!("/glyphs/{i}");
        if let Some(prev) = previous
            && glyph.ch <= prev
        {
            let what = if glyph.ch == prev {
                "repeats"
            } else {
                "is out of order after"
            };
            diagnostics.push(diag(
                &format!("{pointer}/char"),
                format!(
                    "glyph {:?} {what} {prev:?}; glyphs are strictly ascending by code point",
                    glyph.ch
                ),
                "sort the glyph list by code point and drop duplicates",
            ));
        }
        previous = Some(glyph.ch);
        if glyph.advance < 0 {
            diagnostics.push(diag(
                &format!("{pointer}/advance"),
                format!(
                    "glyph {:?} has advance {}; it must not be negative",
                    glyph.ch, glyph.advance
                ),
                "advances are zero or more",
            ));
        }
        if let Some(b) = glyph.bounds {
            check_bounds(&format!("{pointer}/bounds"), b, &mut diagnostics);
        }
        if let Some(r) = glyph.rect {
            check_rect(&format!("{pointer}/rect"), r, &mut diagnostics);
        }
    }

    let has_glyph = |c: char| doc.glyphs.binary_search_by(|g| g.ch.cmp(&c)).is_ok();
    for (from, to) in &doc.aliases {
        let pointer = format!("/aliases/{}", pointer_token(&from.to_string()));
        if has_glyph(*from) {
            diagnostics.push(diag(
                &pointer,
                format!("alias source {from:?} is itself a glyph"),
                "remove the alias, or the glyph it shadows",
            ));
        }
        if !has_glyph(*to) {
            diagnostics.push(diag(
                &pointer,
                format!("alias {from:?} targets {to:?}, which has no glyph"),
                "point the alias at a char in `glyphs`",
            ));
        }
    }
    for (left, right, _) in &doc.kerning {
        for c in [left, right] {
            if !has_glyph(*c) {
                diagnostics.push(diag(
                    &format!(
                        "/kerning/{}/{}",
                        pointer_token(&left.to_string()),
                        pointer_token(&right.to_string())
                    ),
                    format!("kerning names {c:?}, which has no glyph"),
                    "kerning pairs are over chars in `glyphs`",
                ));
            }
        }
    }
    for (i, c) in doc.missing.iter().enumerate() {
        if has_glyph(*c) {
            diagnostics.push(diag(
                &format!("/missing/{i}"),
                format!("{c:?} is listed as missing but has a glyph"),
                "remove it from `missing`",
            ));
        }
    }
    diagnostics
}

// --- Emit ---------------------------------------------------------------------------------------

fn push_int_seq<T: ToString>(out: &mut String, indent: &str, items: &[T]) {
    for item in items {
        out.push_str(indent);
        out.push_str("- ");
        out.push_str(&item.to_string());
        out.push('\n');
    }
}

fn emit_cell(out: &mut String, indent: &str, bounds: Option<[i32; 4]>, rect: Option<[u32; 4]>) {
    if let Some(b) = bounds {
        out.push_str(indent);
        out.push_str("bounds:\n");
        push_int_seq(out, &format!("{indent}  "), &b);
    }
    if let Some(r) = rect {
        out.push_str(indent);
        out.push_str("rect:\n");
        push_int_seq(out, &format!("{indent}  "), &r);
    }
}

/// Writes `doc` in canonical form: the keys in the order of [`FONT_KEYS`], optional sections only
/// when non-empty, integers as decimal, LF endings. Byte-idempotent on any document and a fixed
/// point on canonical input.
pub(crate) fn emit(doc: &FontDocument) -> String {
    let mut out = String::from("format: 1\nkind: font\n");
    if let Some(from) = &doc.generated_from {
        out.push_str(&format!("generated_from: {}\n", scalar_string(from)));
    }
    out.push_str(&format!("image: {}\n", scalar_string(&doc.image)));
    out.push_str(&format!("mode: {}\n", doc.mode.keyword()));
    out.push_str(&format!("units_per_em: {}\n", doc.units_per_em));
    out.push_str(&format!("ascender: {}\n", doc.ascender));
    out.push_str(&format!("descender: {}\n", doc.descender));
    out.push_str(&format!("line_gap: {}\n", doc.line_gap));
    out.push_str("atlas:\n");
    out.push_str(&format!("  width: {}\n", doc.atlas.width));
    out.push_str(&format!("  height: {}\n", doc.atlas.height));
    out.push_str(&format!("  em_px: {}\n", doc.atlas.em_px));
    if let Some(spread) = doc.atlas.spread_px {
        out.push_str(&format!("  spread_px: {spread}\n"));
    }
    out.push_str("notdef:\n");
    out.push_str(&format!("  advance: {}\n", doc.notdef.advance));
    emit_cell(&mut out, "  ", doc.notdef.bounds, doc.notdef.rect);

    if doc.glyphs.is_empty() {
        out.push_str("glyphs: []\n");
    } else {
        out.push_str("glyphs:\n");
        for glyph in &doc.glyphs {
            out.push_str(&format!(
                "  - char: {}\n",
                scalar_string(&glyph.ch.to_string())
            ));
            out.push_str(&format!("    advance: {}\n", glyph.advance));
            emit_cell(&mut out, "    ", glyph.bounds, glyph.rect);
        }
    }
    if !doc.aliases.is_empty() {
        out.push_str("aliases:\n");
        for (from, to) in &doc.aliases {
            out.push_str(&format!(
                "  {}: {}\n",
                scalar_string(&from.to_string()),
                scalar_string(&to.to_string())
            ));
        }
    }
    if !doc.kerning.is_empty() {
        out.push_str("kerning:\n");
        let mut current: Option<char> = None;
        for (left, right, adjust) in &doc.kerning {
            if current != Some(*left) {
                out.push_str(&format!("  {}:\n", scalar_string(&left.to_string())));
                current = Some(*left);
            }
            out.push_str(&format!(
                "    {}: {adjust}\n",
                scalar_string(&right.to_string())
            ));
        }
    }
    if !doc.missing.is_empty() {
        out.push_str("missing:\n");
        for c in &doc.missing {
            out.push_str(&format!("  - {}\n", scalar_string(&c.to_string())));
        }
    }
    out
}

#[cfg(test)]
#[path = "font_tests.rs"]
mod tests;
