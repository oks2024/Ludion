//! Part documents: `parts/*.part.yaml` — one body part of a multi-part `kind: assembly`
//! (`Plan/voxel-animation.md` decisions 3-4).
//!
//! A part is a lean voxel-slices-only document: `size` (the `[x, y, z]` extent in cells) and
//! `slices` — one row-string layer per y level, bottom-up, identical in shape to
//! [`crate::voxel::VoxelDocument`]'s grid — but **no palette**. A part's glyphs are resolved later
//! against the owning assembly's palette, so any printable-ASCII glyph other than `.` (reserved for
//! empty) is structurally legal here; a glyph the assembly's palette does not define is that
//! assembly's lint to raise at composition time, not this module's.
//!
//! This module is a structural sibling of [`crate::voxel`], trimmed to the fields a part needs. It
//! owns part-document *structure* (shape/type checking, `L1139`) and *data consistency* (slice
//! dimensions match `size`, every glyph is `.` or printable ASCII; `L1140`) — the `L1113`/`L1114`
//! pair, renumbered for the part family. It reuses the crate's shared strict-YAML layer
//! ([`crate::yaml`]) and the same canonical-writer primitives ([`crate::scalar`]) every other
//! document family uses, so a part document is byte-idempotent canonical text for the same reasons.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use serde_json::Value;

use crate::envelope;
use crate::scalar::scalar_string;
use crate::yaml::{self, MapEntry, NodeValue};

/// The part document format version this crate reads and writes.
pub const PART_FORMAT_VERSION: u64 = 1;

/// The reserved glyph meaning "empty cell" — the [`crate::voxel`] convention, shared so a part's
/// glyphs read identically to a voxel model's.
const EMPTY_GLYPH: char = '.';

const PART_KEYS: [&str; 5] = ["format", "kind", "name", "size", "slices"];

/// A parsed, structurally valid part document.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PartDocument {
    /// Optional human-readable part name.
    pub name: Option<String>,
    /// The grid extent in cells: `[x, y, z]` (width, height, depth) — same convention as
    /// [`crate::voxel::VoxelDocument::size`].
    pub size: [u32; 3],
    /// Slice layers, bottom-up (`slices[y]`), each a sequence of row strings (`slices[y][z]`); the
    /// character at column `c` of a row is the cell at `x = c`. Unlike a voxel document's slices, a
    /// glyph here names no palette entry of its own — it resolves against the owning assembly's
    /// palette.
    pub slices: Vec<Vec<String>>,
}

impl PartDocument {
    /// Visits every in-bounds cell exactly once, in `(y, z, x)` scan order — the order the row
    /// strings are authored in — passing `(x, y, z)` and that cell's raw glyph (`None` for `.`/an
    /// empty cell).
    ///
    /// The part analog of [`crate::voxel::VoxelDocument::for_each_cell`], trimmed to what a part
    /// can answer without a palette: the character itself, not a palette index. Downstream assembly
    /// composition resolves each glyph against the assembly's palette.
    ///
    /// # Example
    /// ```
    /// let source = "format: 1\nkind: part\nsize:\n  - 2\n  - 1\n  - 1\nslices:\n  -\n    \
    ///               - \"A.\"\n";
    /// let part = ludion_scene::check_part(source).expect("the part is valid");
    /// let mut cells = Vec::new();
    /// part.for_each_cell(|x, y, z, glyph| cells.push(([x, y, z], glyph)));
    /// assert_eq!(cells, [([0, 0, 0], Some('A')), ([1, 0, 0], None)]);
    /// ```
    pub fn for_each_cell(&self, mut visit: impl FnMut(u32, u32, u32, Option<char>)) {
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
                    // past its end — the same convention `VoxelDocument::for_each_cell` uses.
                    let glyph = glyphs.next().filter(|&ch| ch != EMPTY_GLYPH);
                    visit(x, y, z, glyph);
                }
            }
        }
    }
}

/// Parses strict-subset YAML into a [`PartDocument`], checking structure/shape only (`L1139`).
pub(crate) fn parse(source: &str) -> Result<PartDocument, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![
            Diagnostic::new(
                code::PART_DOCUMENT_STRUCTURE,
                "the part document is empty; a part needs at least `format: 1`, `kind: part`, \
                 `size`, and `slices`",
            )
            .with_path("")
            .with_help(
                "a minimal part is:\n  format: 1\n  kind: part\n  size:\n    - 1\n    - 1\n    \
                 - 1\n  slices:\n    -\n      - \"A\"",
            ),
        ]);
    };
    let NodeValue::Mapping(entries) = root.value else {
        return Err(vec![
            Diagnostic::new(
                code::PART_DOCUMENT_STRUCTURE,
                format!(
                    "the part document root must be a mapping, found {} (line {})",
                    root.value.type_phrase(),
                    root.line
                ),
            )
            .with_path("")
            .with_help("start the file with the top-level keys `format:` and `kind: part`"),
        ]);
    };

    let mut diagnostics = Vec::new();
    let mut doc = PartDocument::default();
    let mut format_seen = false;
    let mut kind_seen = false;
    let mut size_seen = false;
    let mut slices_seen = false;

    for entry in &entries {
        match entry.key.as_str() {
            "format" => {
                format_seen = true;
                envelope::check_envelope(
                    entry,
                    code::PART_DOCUMENT_STRUCTURE,
                    code::PART_DOCUMENT_STRUCTURE,
                    PART_FORMAT_VERSION,
                    "the part document",
                    "part",
                    "part",
                    &mut diagnostics,
                );
            }
            "kind" => {
                kind_seen = true;
                envelope::check_kind(
                    entry,
                    code::PART_DOCUMENT_STRUCTURE,
                    "part",
                    "a part document",
                    &mut diagnostics,
                );
            }
            "name" => {
                doc.name = envelope::expect_string(
                    &entry.node,
                    "/name",
                    "the part name",
                    code::PART_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "size" => {
                size_seen = true;
                if let Some(size) = crate::fields::parse_size3(
                    entry,
                    code::PART_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                ) {
                    doc.size = size;
                }
            }
            "slices" => {
                slices_seen = true;
                doc.slices = parse_slices(entry, &mut diagnostics);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "",
                "a part document",
                PART_KEYS,
                code::PART_DOCUMENT_STRUCTURE,
            )),
        }
    }

    if !format_seen {
        diagnostics.push(missing_key("format", "add `format: 1` as the first line"));
    }
    if !kind_seen {
        diagnostics.push(missing_key("kind", "add `kind: part`"));
    }
    if !size_seen {
        diagnostics.push(missing_key(
            "size",
            "add a `size` of three positive integers: width (x), height (y), depth (z)",
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

/// Parses `slices`: a sequence of layers, each a sequence of row strings.
fn parse_slices(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Vec<Vec<String>> {
    let NodeValue::Sequence(layers) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::PART_DOCUMENT_STRUCTURE,
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
                    code::PART_DOCUMENT_STRUCTURE,
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
                        code::PART_DOCUMENT_STRUCTURE,
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
    envelope::missing_key(key, help, code::PART_DOCUMENT_STRUCTURE, "part document")
}

/// Validates data-consistency invariants of an already-parsed [`PartDocument`] (`L1140`): the slice
/// layers/rows/columns match `size`, and every slice glyph is `.` or printable ASCII
/// (`0x21..=0x7E`).
///
/// Unlike [`crate::voxel::validate`] there is no palette to check a glyph against — a part has
/// none — so a glyph is illegal here only when it is not printable ASCII at all; whether an
/// assembly's palette actually claims it is that assembly's concern, checked at composition time.
///
/// Independent of how the document was built — a hand-authored one via [`parse`], or one assembled
/// programmatically — so this is the one place those invariants are enforced.
pub(crate) fn validate(doc: &PartDocument) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let [sx, sy, sz] = doc.size;
    if doc.slices.len() as u64 != u64::from(sy) {
        diagnostics.push(
            Diagnostic::new(
                code::PART_DATA_CONSISTENCY,
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
                    code::PART_DATA_CONSISTENCY,
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
                        code::PART_DATA_CONSISTENCY,
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
                if ch != EMPTY_GLYPH && !is_part_glyph(ch) && !reported.contains(&ch) {
                    reported.push(ch);
                    diagnostics.push(
                        Diagnostic::new(
                            code::PART_DATA_CONSISTENCY,
                            format!(
                                "the glyph `{ch}` in layer y = {y}, row z = {z} is not a \
                                 printable-ASCII character (0x21..=0x7E)"
                            ),
                        )
                        .with_path(format!("/slices/{y}/{z}"))
                        .with_help(
                            "a part's glyphs resolve against the owning assembly's palette, so \
                             any printable-ASCII glyph other than `.` is legal here; pick a \
                             character from `!` to `~`, or use `.` for an empty cell",
                        ),
                    );
                }
            }
        }
    }
    diagnostics
}

/// Whether `ch` is a printable ASCII character usable as a part glyph (`0x21..=0x7E`) — the
/// [`crate::voxel`] convention, minus the palette-membership check a part has no palette to run.
fn is_part_glyph(ch: char) -> bool {
    ('\u{21}'..='\u{7e}').contains(&ch)
}

/// Writes `doc` in canonical form: `format`, `kind`, `name` (optional), `size`, `slices` — the part
/// analog of [`crate::voxel::emit`], with no palette to write.
pub(crate) fn emit(doc: &PartDocument) -> String {
    let mut out = String::new();
    out.push_str("format: 1\n");
    out.push_str("kind: part\n");
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

    if doc.slices.is_empty() {
        out.push_str("slices: []\n");
    } else {
        out.push_str("slices:\n");
        for layer in &doc.slices {
            if layer.is_empty() {
                out.push_str("  - []\n");
            } else {
                out.push_str("  -\n");
                for row in layer {
                    out.push_str("    - ");
                    out.push_str(&scalar_string(row));
                    out.push('\n');
                }
            }
        }
    }

    out
}
