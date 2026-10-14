//! Coordinate-addressed editing of voxel model documents: the authoring comment model, the typed
//! ops document (`kind: voxel-ops`), and the executor that turns a list of ops into new file text.
//!
//! This module exists because hand-editing a model above roughly 32³ fails: an edit means counting
//! columns inside fixed-width quoted rows across dozens of layers, and a shape-sized edit is
//! hundreds of cells (`Plan/voxel-authoring-tools.md`). Ops are addressed in **voxel coordinates**
//! — never a line and column — so the same op file reads the same before and after the grid moves.
//!
//! Two contracts govern everything here:
//!
//! - **Comments are load-bearing.** A model's header docstring and its per-layer anatomy lines are
//!   how the next agent re-reads intent, so an edit that erases them is broken. Cell-level edits are
//!   therefore **surgical**: the original text is modified only at the changed cell characters, and
//!   every other byte survives. Only a grid *reshape* (`grow`/`crop`) or a y-shift re-emits canonically,
//!   and then the extracted comments are replayed — layer comments follow their content, and one
//!   whose layer leaves the grid is reported.
//! - **Validate before write.** [`apply`] builds the whole new text in memory and re-checks it
//!   ([`crate::check_voxel`]) before returning it; the caller writes temp-then-rename. A failed op
//!   leaves the file byte-identical, which matters because the editor's watcher reloads on save — a
//!   half-applied write would blank the model in a viewport.
//!
//! The read side (`check-scene --slice/--column/--mirror-x`, `L1129` connectivity) is untouched:
//! it is how an author finds the coordinates the ops then name.

use std::collections::BTreeMap;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::suggest;
use serde_json::Value;

use crate::envelope;
use crate::voxel::{VOXEL_MAX_CELLS, VoxelDocument, comment_line};
use crate::yaml::{self, MapEntry, Node, NodeValue};

/// The ops document format version this crate reads.
pub const VOXEL_OPS_FORMAT_VERSION: u64 = 1;

/// The reserved glyph meaning "empty cell" — writing it carves.
const EMPTY_GLYPH: char = '.';

// --- Comments ----------------------------------------------------------------------------------

/// The authoring comments of a voxel document: the header block and the per-layer anatomy lines.
///
/// These are the only two comment positions the canonical writer can reproduce, so they are the
/// only two `extract` classifies; anything else comes back as a [`StrayComment`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VoxelComments {
    /// The header block, one entry per line, written between the envelope keys and `size:`. Each
    /// entry is the comment's text with its `#` and one following space stripped.
    pub header: Vec<String>,
    /// One comment per slice layer, keyed by y. A value may itself be several lines (`\n`-joined),
    /// which emit as consecutive comment lines above the layer. A `BTreeMap` rather than a hash
    /// map: iteration order is part of the output.
    pub layers: BTreeMap<u32, String>,
}

impl VoxelComments {
    /// Layer `y`'s comment text, or `None` when it has none.
    pub fn layer(&self, y: u32) -> Option<&str> {
        self.layers.get(&y).map(String::as_str)
    }

    /// Sets (or replaces) layer `y`'s comment text.
    pub fn set_layer(&mut self, y: u32, text: impl Into<String>) {
        self.layers.insert(y, text.into());
    }

    /// Whether there is nothing to replay — the state in which
    /// [`crate::emit_voxel_with_comments`] reproduces [`crate::emit_voxel`] byte for byte.
    pub fn is_empty(&self) -> bool {
        self.header.is_empty() && self.layers.is_empty()
    }

    /// Rewrites every layer key through `remap` — how a reshape or y-shift carries anatomy
    /// comments onto their new y levels. A moved text whose first line opens with the `y<old>`
    /// anchor is relabelled `y<new>`. Returns the layers `remap` mapped to `None` (old y, text),
    /// in ascending old y.
    fn remap_layers(&mut self, remap: impl Fn(u32) -> Option<u32>) -> Vec<(u32, String)> {
        let mut next = BTreeMap::new();
        let mut dropped = Vec::new();
        for (&y, text) in &self.layers {
            match remap(y) {
                Some(moved) => {
                    next.insert(moved, relabel_anchor(text, y, moved, &remap));
                }
                None => dropped.push((y, text.clone())),
            }
        }
        self.layers = next;
        dropped
    }
}

/// Rewrites a leading `y<old>` anchor of `text` to `y<new>`, and the upper bound of a leading range
/// anchor `y<old>-y<B>` through `remap`; any other text is returned as is. A range whose upper bound
/// `remap` drops is left alone.
fn relabel_anchor(text: &str, old: u32, new: u32, remap: &impl Fn(u32) -> Option<u32>) -> String {
    if old == new {
        return text.to_string();
    }
    let Some(rest) = text.strip_prefix(&format!("y{old}")) else {
        return text.to_string();
    };
    if let Some(range) = rest.strip_prefix("-y") {
        let digits = range.len() - range.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let tail = &range[digits..];
        if digits > 0
            && let Ok(upper) = range[..digits].parse::<u32>()
        {
            if tail.starts_with(|c: char| c.is_ascii_alphanumeric()) {
                return text.to_string();
            }
            return match remap(upper) {
                Some(moved) => format!("y{new}-y{moved}{tail}"),
                None => text.to_string(),
            };
        }
    }
    if rest.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        return text.to_string();
    }
    format!("y{new}{rest}")
}

/// A comment `extract` found somewhere the canonical writer cannot put it back (inside the
/// palette, between slice rows, at the top of the file above `format:`).
///
/// A surgical edit preserves it like every other byte; a re-emit would silently drop it, so `apply`
/// reports the list and the CLI warns. A layer comment whose layer leaves the grid is reported the
/// same way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrayComment {
    /// The 1-based source line the comment was found on; `0` when a `comment` op in this run set
    /// it.
    pub line: usize,
    /// The comment line's text, trimmed, `#` included.
    pub text: String,
}

/// What `extract` found: the replayable comments, plus the ones that could not be classified.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtractedComments {
    /// The header block and per-layer comments, ready for [`crate::emit_voxel_with_comments`].
    pub comments: VoxelComments,
    /// Comments in positions the canonical writer cannot reproduce, in source order.
    pub stray: Vec<StrayComment>,
    /// The 1-based source line of the first line of each layer comment block, keyed by y.
    pub layer_lines: BTreeMap<u32, usize>,
}

/// Extracts the authoring comments of a voxel document from its raw source text.
///
/// Purely textual — it never parses the document, so it works on a file that does not validate.
/// The header is the contiguous comment block immediately above `size:`; a layer's comment is the
/// contiguous block immediately above that layer's `-` inside `slices:`. Everything else is stray.
pub fn extract(source: &str) -> ExtractedComments {
    let mut out = ExtractedComments::default();
    let mut pending: Vec<(usize, String)> = Vec::new();
    let mut in_slices = false;
    let mut layer = 0u32;

    for (index, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            pending.push((index + 1, comment_text(trimmed)));
            continue;
        }
        if trimmed.is_empty() {
            // A blank line breaks contiguity: whatever came before it anchors to nothing.
            stray_out(&mut pending, &mut out.stray);
            continue;
        }
        if !in_slices && trimmed.starts_with("size:") {
            out.comments.header = pending.drain(..).map(|(_, text)| text).collect();
            continue;
        }
        if !in_slices && !line.starts_with(char::is_whitespace) && trimmed.starts_with("slices:") {
            in_slices = true;
            stray_out(&mut pending, &mut out.stray);
            continue;
        }
        if in_slices && is_layer_marker(trimmed) {
            if !pending.is_empty() {
                out.layer_lines.insert(layer, pending[0].0);
                let text = pending
                    .drain(..)
                    .map(|(_, text)| text)
                    .collect::<Vec<_>>()
                    .join("\n");
                out.comments.set_layer(layer, text);
            }
            layer += 1;
            continue;
        }
        stray_out(&mut pending, &mut out.stray);
    }
    stray_out(&mut pending, &mut out.stray);
    out
}

/// A slice layer's own list item: `-` alone (rows follow, indented) or the empty `- []`. A row is
/// never either — it always carries `size.x >= 1` characters of content.
fn is_layer_marker(trimmed: &str) -> bool {
    trimmed == "-" || trimmed == "- []"
}

/// One comment line's text: the `#` and one following space stripped, trailing whitespace dropped.
fn comment_text(trimmed: &str) -> String {
    let body = trimmed.strip_prefix('#').unwrap_or(trimmed);
    body.strip_prefix(' ')
        .unwrap_or(body)
        .trim_end()
        .to_string()
}

fn stray_out(pending: &mut Vec<(usize, String)>, stray: &mut Vec<StrayComment>) {
    for (line, text) in pending.drain(..) {
        stray.push(StrayComment {
            line,
            text: if text.is_empty() {
                "#".to_string()
            } else {
                format!("# {text}")
            },
        });
    }
}

// --- Ops ---------------------------------------------------------------------------------------

/// A cardinal grid axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoxelAxis {
    /// Width (columns within a row).
    X,
    /// Height (slice layers).
    Y,
    /// Depth (rows within a layer).
    Z,
}

impl VoxelAxis {
    fn index(self) -> usize {
        match self {
            VoxelAxis::X => 0,
            VoxelAxis::Y => 1,
            VoxelAxis::Z => 2,
        }
    }
}

/// How a shape op writes into the grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WriteMode {
    /// Write every cell the shape covers (the default).
    #[default]
    Place,
    /// Write only cells that are already filled — recolouring detail onto a surface without
    /// changing the silhouette.
    Paint,
}

/// Which half of a [`VoxelOp::Mirror`] is the source that gets copied over the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MirrorKeep {
    /// Keep the low-index half (x < width / 2 for the x axis).
    Low,
    /// Keep the high-index half.
    High,
}

/// One editing operation, addressed in voxel coordinates.
///
/// Volume ops (`Box`, `Ellipsoid`, `Cylinder`, `Line`) clip to the grid silently — a shape that
/// pokes out of the model is a normal way to sculpt. Point ops (`Set`) do not: an out-of-range cell
/// is a mistake worth an `L1132`.
#[derive(Clone, Debug, PartialEq)]
pub enum VoxelOp {
    /// Write one cell.
    Set {
        /// The `x,y,z` cell.
        at: [i64; 3],
        /// The glyph to write (`.` erases).
        glyph: char,
        /// Whether to write regardless, or only over an already-filled cell.
        mode: WriteMode,
    },
    /// Write every cell of an inclusive box.
    Box {
        /// One corner.
        from: [i64; 3],
        /// The opposite corner (inclusive; either corner may be the lower one).
        to: [i64; 3],
        /// The glyph to write (`.` erases).
        glyph: char,
        /// Whether to write anywhere or only over filled cells.
        mode: WriteMode,
    },
    /// Write every cell inside an axis-aligned ellipsoid.
    Ellipsoid {
        /// The centre, in cell coordinates (decimals allowed — a half-cell centre is how an even
        /// diameter lands symmetrically).
        center: [f64; 3],
        /// The three semi-axes, each strictly positive.
        radii: [f64; 3],
        /// The glyph to write (`.` erases).
        glyph: char,
        /// Whether to write anywhere or only over filled cells.
        mode: WriteMode,
    },
    /// Write every cell inside a right circular cylinder.
    Cylinder {
        /// The centre of the base disc.
        base: [f64; 3],
        /// The axis the cylinder extends along, in the positive direction.
        axis: VoxelAxis,
        /// The disc radius, strictly positive.
        radius: f64,
        /// The extent along `axis`, strictly positive.
        height: f64,
        /// The glyph to write (`.` erases).
        glyph: char,
        /// Whether to write anywhere or only over filled cells.
        mode: WriteMode,
    },
    /// Write every cell on the 3D Bresenham line between two cells (both endpoints included).
    Line {
        /// The first endpoint.
        from: [i64; 3],
        /// The second endpoint.
        to: [i64; 3],
        /// The glyph to write (`.` erases).
        glyph: char,
        /// Whether to write anywhere or only over filled cells.
        mode: WriteMode,
    },
    /// Copy one half of the grid onto the other across an axis, making symmetry true rather than
    /// merely measured. On an odd extent the centre slice is self-mirroring and untouched.
    Mirror {
        /// The axis to mirror across.
        axis: VoxelAxis,
        /// Which half is the source.
        keep: MirrorKeep,
    },
    /// Translate every cell by a signed offset. Cells shifted out are dropped, vacated cells become
    /// empty, and the grid dimensions do not change.
    Shift {
        /// The signed `dx,dy,dz` offset.
        by: [i64; 3],
    },
    /// Enlarge the grid by adding empty cells on each side.
    Grow {
        /// Cells added below `x,y,z` = 0 (every existing cell moves up by this much).
        low: [u32; 3],
        /// Cells added past the far side.
        high: [u32; 3],
    },
    /// Shrink the grid to an inclusive sub-box, discarding everything outside it.
    Crop {
        /// The low corner, kept.
        from: [u32; 3],
        /// The high corner, kept.
        to: [u32; 3],
    },
    /// Set (or replace) the anatomy comment above a slice layer.
    Comment {
        /// The layer.
        y: u32,
        /// The comment text, without its `#`.
        text: String,
    },
}

impl VoxelOp {
    /// The op's name as it is spelled in an ops document.
    pub fn name(&self) -> &'static str {
        match self {
            VoxelOp::Set { .. } => "set",
            VoxelOp::Box { .. } => "box",
            VoxelOp::Ellipsoid { .. } => "ellipsoid",
            VoxelOp::Cylinder { .. } => "cylinder",
            VoxelOp::Line { .. } => "line",
            VoxelOp::Mirror { .. } => "mirror",
            VoxelOp::Shift { .. } => "shift",
            VoxelOp::Grow { .. } => "grow",
            VoxelOp::Crop { .. } => "crop",
            VoxelOp::Comment { .. } => "comment",
        }
    }

    /// Whether the op forces a canonical re-emit instead of a surgical text patch: it changes the
    /// grid dimensions or moves layers along y.
    pub fn reshapes(&self) -> bool {
        match self {
            VoxelOp::Grow { .. } | VoxelOp::Crop { .. } => true,
            VoxelOp::Shift { by } => by[1] != 0,
            _ => false,
        }
    }
}

// --- Ops document parsing ----------------------------------------------------------------------

const OPS_KEYS: [&str; 3] = ["format", "kind", "ops"];

const OP_NAMES: [&str; 10] = [
    "set",
    "box",
    "ellipsoid",
    "cylinder",
    "line",
    "mirror",
    "shift",
    "grow",
    "crop",
    "comment",
];

/// The recognized keys of each op, `op` included so a typo of it suggests itself.
fn op_keys(name: &str) -> Option<&'static [&'static str]> {
    Some(match name {
        "set" => &["op", "at", "glyph", "mode"],
        "box" => &["op", "from", "to", "glyph", "mode"],
        "ellipsoid" => &["op", "center", "radii", "glyph", "mode"],
        "cylinder" => &["op", "base", "axis", "radius", "height", "glyph", "mode"],
        "line" => &["op", "from", "to", "glyph", "mode"],
        "mirror" => &["op", "axis", "keep"],
        "shift" => &["op", "by"],
        "grow" => &["op", "low", "high"],
        "crop" => &["op", "from", "to"],
        "comment" => &["op", "y", "text"],
        _ => return None,
    })
}

/// Parses a strict-subset YAML ops document (`kind: voxel-ops`) into its op list (`L1131`).
pub(crate) fn parse_ops(source: &str) -> Result<Vec<VoxelOp>, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![
            ops_error("", "the ops document is empty; it needs `format: 1`, `kind: voxel-ops`, and an `ops` sequence")
                .with_help(OPS_SKELETON),
        ]);
    };
    let NodeValue::Mapping(entries) = root.value else {
        return Err(vec![
            ops_error(
                "",
                format!(
                    "the ops document root must be a mapping, found {} (line {})",
                    root.value.type_phrase(),
                    root.line
                ),
            )
            .with_help(OPS_SKELETON),
        ]);
    };

    let mut diagnostics = Vec::new();
    let mut format_seen = false;
    let mut kind_seen = false;
    let mut ops_seen = false;
    let mut ops = Vec::new();

    for entry in &entries {
        match entry.key.as_str() {
            "format" => {
                format_seen = true;
                envelope::check_envelope(
                    entry,
                    code::VOXEL_OPS_DOCUMENT_STRUCTURE,
                    code::VOXEL_OPS_DOCUMENT_STRUCTURE,
                    VOXEL_OPS_FORMAT_VERSION,
                    "the ops document",
                    "ops document",
                    "voxel-ops",
                    &mut diagnostics,
                );
            }
            "kind" => {
                kind_seen = true;
                envelope::check_kind(
                    entry,
                    code::VOXEL_OPS_DOCUMENT_STRUCTURE,
                    "voxel-ops",
                    "a voxel ops document",
                    &mut diagnostics,
                );
            }
            "ops" => {
                ops_seen = true;
                ops = parse_op_list(entry, &mut diagnostics);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "",
                "an ops document",
                OPS_KEYS,
                code::VOXEL_OPS_DOCUMENT_STRUCTURE,
            )),
        }
    }

    if !format_seen {
        diagnostics.push(missing_top_level(
            "format",
            "add `format: 1` as the first line",
        ));
    }
    if !kind_seen {
        diagnostics.push(missing_top_level("kind", "add `kind: voxel-ops`"));
    }
    if !ops_seen {
        diagnostics.push(missing_top_level(
            "ops",
            "add an `ops` sequence — one mapping per operation, applied in order",
        ));
    }

    if diagnostics.is_empty() {
        Ok(ops)
    } else {
        Err(diagnostics)
    }
}

const OPS_SKELETON: &str = "a minimal ops document is:\n  format: 1\n  kind: voxel-ops\n  ops:\n    - op: set\n      at: \"3,10,4\"\n      glyph: H";

fn ops_error(pointer: &str, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(code::VOXEL_OPS_DOCUMENT_STRUCTURE, message).with_path(pointer.to_string())
}

fn missing_top_level(key: &str, help: &str) -> Diagnostic {
    envelope::missing_key(
        key,
        help,
        code::VOXEL_OPS_DOCUMENT_STRUCTURE,
        "ops document",
    )
}

fn parse_op_list(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Vec<VoxelOp> {
    let NodeValue::Sequence(items) = &entry.node.value else {
        diagnostics.push(
            ops_error(
                "/ops",
                format!(
                    "`ops` must be a sequence of operations, found {} (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_help(OPS_SKELETON),
        );
        return Vec::new();
    };
    let mut ops = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        if let Some(op) = parse_op(index, item, diagnostics) {
            ops.push(op);
        }
    }
    ops
}

fn parse_op(index: usize, node: &Node, diagnostics: &mut Vec<Diagnostic>) -> Option<VoxelOp> {
    let base = format!("/ops/{index}");
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            ops_error(
                &base,
                format!(
                    "each entry of `ops` must be a mapping with an `op` key, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_help(OPS_SKELETON),
        );
        return None;
    };

    let Some(op_entry) = entries.iter().find(|entry| entry.key == "op") else {
        diagnostics.push(
            ops_error(
                &base,
                format!("op {index} has no `op` key naming the operation"),
            )
            .with_help(format!("add one of: {}", OP_NAMES.join(", "))),
        );
        return None;
    };
    let name = envelope::expect_string(
        &op_entry.node,
        &format!("{base}/op"),
        "an operation name",
        code::VOXEL_OPS_DOCUMENT_STRUCTURE,
        diagnostics,
    )?;
    let Some(keys) = op_keys(&name) else {
        let mut diagnostic = ops_error(
            &format!("{base}/op"),
            format!(
                "`{name}` is not a voxel operation (line {})",
                op_entry.node.line
            ),
        );
        diagnostic = match suggest::nearest(&name, OP_NAMES) {
            Some(candidate) => diagnostic.with_help(format!("nearest match is `{candidate}`")),
            None => diagnostic.with_help(format!("the operations are: {}", OP_NAMES.join(", "))),
        };
        diagnostics.push(diagnostic);
        return None;
    };

    let fields = Fields {
        index,
        op: &name,
        base: &base,
        entries,
    };
    for entry in entries {
        if !keys.contains(&entry.key.as_str()) {
            let mut diagnostic = ops_error(
                &format!("{base}/{}", yaml::pointer_token(&entry.key)),
                format!(
                    "op `{name}` does not have a key named `{}` (line {})",
                    entry.key, entry.key_line
                ),
            );
            diagnostic = match suggest::nearest(&entry.key, keys.iter().copied()) {
                Some(candidate) => diagnostic.with_help(format!("nearest match is `{candidate}`")),
                None => diagnostic.with_help(format!(
                    "op `{name}` takes: {}",
                    keys.iter()
                        .filter(|k| **k != "op")
                        .copied()
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            };
            diagnostics.push(diagnostic);
        }
    }

    match name.as_str() {
        "set" => Some(VoxelOp::Set {
            at: fields.coord_int("at", diagnostics)?,
            glyph: fields.glyph(diagnostics)?,
            mode: fields.mode(diagnostics)?,
        }),
        "box" => Some(VoxelOp::Box {
            from: fields.coord_int("from", diagnostics)?,
            to: fields.coord_int("to", diagnostics)?,
            glyph: fields.glyph(diagnostics)?,
            mode: fields.mode(diagnostics)?,
        }),
        "ellipsoid" => Some(VoxelOp::Ellipsoid {
            center: fields.coord_float("center", diagnostics)?,
            radii: fields.radii(diagnostics)?,
            glyph: fields.glyph(diagnostics)?,
            mode: fields.mode(diagnostics)?,
        }),
        "cylinder" => Some(VoxelOp::Cylinder {
            base: fields.coord_float("base", diagnostics)?,
            axis: fields.axis(diagnostics)?,
            radius: fields.positive("radius", diagnostics)?,
            height: fields.positive("height", diagnostics)?,
            glyph: fields.glyph(diagnostics)?,
            mode: fields.mode(diagnostics)?,
        }),
        "line" => Some(VoxelOp::Line {
            from: fields.coord_int("from", diagnostics)?,
            to: fields.coord_int("to", diagnostics)?,
            glyph: fields.glyph(diagnostics)?,
            mode: fields.mode(diagnostics)?,
        }),
        "mirror" => Some(VoxelOp::Mirror {
            axis: fields.axis(diagnostics)?,
            keep: fields.keep(diagnostics)?,
        }),
        "shift" => Some(VoxelOp::Shift {
            by: fields.coord_int("by", diagnostics)?,
        }),
        "grow" => Some(VoxelOp::Grow {
            low: fields.coord_extent("low", diagnostics)?,
            high: fields.coord_extent("high", diagnostics)?,
        }),
        "crop" => Some(VoxelOp::Crop {
            from: fields.coord_extent("from", diagnostics)?,
            to: fields.coord_extent("to", diagnostics)?,
        }),
        "comment" => Some(VoxelOp::Comment {
            y: fields.layer_index(diagnostics)?,
            text: fields.text(diagnostics)?,
        }),
        _ => None,
    }
}

/// One op's entries, with the per-key readers every op shares. Each reader pushes its own `L1131`
/// and returns `None`, so a malformed op reports one precise diagnostic instead of a cascade.
struct Fields<'a> {
    index: usize,
    op: &'a str,
    base: &'a str,
    entries: &'a [MapEntry],
}

impl Fields<'_> {
    fn pointer(&self, key: &str) -> String {
        format!("{}/{key}", self.base)
    }

    fn node(&self, key: &str) -> Option<&Node> {
        self.entries
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| &entry.node)
    }

    fn require(&self, key: &str, help: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<&Node> {
        match self.node(key) {
            Some(node) => Some(node),
            None => {
                diagnostics.push(
                    ops_error(
                        self.base,
                        format!(
                            "op {} (`{}`) is missing the required key `{key}`",
                            self.index, self.op
                        ),
                    )
                    .with_help(help.to_string()),
                );
                None
            }
        }
    }

    /// Reads a `"x,y,z"` coordinate token as three signed integers.
    fn coord_int(&self, key: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<[i64; 3]> {
        let node = self.require(
            key,
            &format!("write `{key}` as one comma-joined token, e.g. `{key}: \"3,10,4\"`"),
            diagnostics,
        )?;
        let parts = self.token(key, node, diagnostics)?;
        let mut out = [0i64; 3];
        for (slot, part) in out.iter_mut().zip(parts.iter()) {
            match part.trim().parse::<i64>() {
                Ok(value) => *slot = value,
                Err(_) => {
                    diagnostics.push(
                        ops_error(
                            &self.pointer(key),
                            format!(
                                "`{key}` component `{}` is not a whole number",
                                part.trim()
                            ),
                        )
                        .with_help("cell coordinates are whole numbers; `ellipsoid`/`cylinder` are the ops that take decimals"),
                    );
                    return None;
                }
            }
        }
        Some(out)
    }

    /// Reads a `"x,y,z"` coordinate token as three non-negative extents.
    fn coord_extent(&self, key: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<[u32; 3]> {
        let values = self.coord_int(key, diagnostics)?;
        let mut out = [0u32; 3];
        for (slot, value) in out.iter_mut().zip(values) {
            match u32::try_from(value) {
                Ok(value) => *slot = value,
                Err(_) => {
                    diagnostics.push(
                        ops_error(
                            &self.pointer(key),
                            format!("`{key}` component `{value}` is negative"),
                        )
                        .with_help(
                            "`grow` and `crop` take non-negative cell counts; `shift` is the op \
                             that takes a signed offset",
                        ),
                    );
                    return None;
                }
            }
        }
        Some(out)
    }

    /// Reads a `"x,y,z"` coordinate token as three finite decimals.
    fn coord_float(&self, key: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<[f64; 3]> {
        let node = self.require(
            key,
            &format!("write `{key}` as one comma-joined token, e.g. `{key}: \"3.5,10,4.5\"`"),
            diagnostics,
        )?;
        let parts = self.token(key, node, diagnostics)?;
        let mut out = [0.0f64; 3];
        for (slot, part) in out.iter_mut().zip(parts.iter()) {
            match part.trim().parse::<f64>() {
                Ok(value) if value.is_finite() => *slot = value,
                _ => {
                    diagnostics.push(
                        ops_error(
                            &self.pointer(key),
                            format!("`{key}` component `{}` is not a finite number", part.trim()),
                        )
                        .with_help("write each component as a decimal, e.g. `3.5`"),
                    );
                    return None;
                }
            }
        }
        Some(out)
    }

    /// Splits a coordinate token into its three components.
    fn token(
        &self,
        key: &str,
        node: &Node,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<[String; 3]> {
        let NodeValue::Scalar(Value::String(text)) = &node.value else {
            diagnostics.push(
                ops_error(
                    &self.pointer(key),
                    format!(
                        "`{key}` must be one comma-joined coordinate token, found {} (line {})",
                        node.value.type_phrase(),
                        node.line
                    ),
                )
                .with_help(format!(
                    "write the three components as one token: `{key}: \"3,10,4\"` (the strict YAML \
                     subset has no inline sequences)"
                )),
            );
            return None;
        };
        let parts: Vec<&str> = text.split(',').collect();
        if parts.len() != 3 {
            diagnostics.push(
                ops_error(
                    &self.pointer(key),
                    format!(
                        "`{key}` has {} comma-separated component{}, expected 3 (x, y, z)",
                        parts.len(),
                        if parts.len() == 1 { "" } else { "s" }
                    ),
                )
                .with_help(format!("write `{key}: \"3,10,4\"`")),
            );
            return None;
        }
        Some([
            parts[0].to_string(),
            parts[1].to_string(),
            parts[2].to_string(),
        ])
    }

    fn radii(&self, diagnostics: &mut Vec<Diagnostic>) -> Option<[f64; 3]> {
        let radii = self.coord_float("radii", diagnostics)?;
        for value in radii {
            if value <= 0.0 {
                diagnostics.push(
                    ops_error(
                        &self.pointer("radii"),
                        format!("`radii` component `{value}` is not greater than zero"),
                    )
                    .with_help("every semi-axis must be positive; a flat ellipsoid is a `box`"),
                );
                return None;
            }
        }
        Some(radii)
    }

    fn positive(&self, key: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<f64> {
        let node = self.require(key, &format!("add `{key}: <number>`"), diagnostics)?;
        let value = match &node.value {
            NodeValue::Scalar(Value::Number(n)) => crate::fields::number(n),
            other => {
                diagnostics.push(
                    ops_error(
                        &self.pointer(key),
                        format!(
                            "`{key}` must be a number, found {} (line {})",
                            other.type_phrase(),
                            node.line
                        ),
                    )
                    .with_help(format!("write `{key}: 3.5`")),
                );
                return None;
            }
        };
        if !(value.is_finite() && value > 0.0) {
            diagnostics.push(
                ops_error(
                    &self.pointer(key),
                    format!("`{key}` is {value}; it must be finite and greater than zero"),
                )
                .with_help(format!("write `{key}` as a positive number")),
            );
            return None;
        }
        Some(value)
    }

    fn layer_index(&self, diagnostics: &mut Vec<Diagnostic>) -> Option<u32> {
        let node = self.require("y", "add `y: <layer index>`", diagnostics)?;
        match &node.value {
            NodeValue::Scalar(Value::Number(n)) => {
                match n.as_u64().and_then(|v| u32::try_from(v).ok()) {
                    Some(value) => Some(value),
                    None => {
                        diagnostics.push(
                            ops_error(&self.pointer("y"), "`y` must be a non-negative layer index")
                                .with_help("layers are numbered from 0, bottom-up"),
                        );
                        None
                    }
                }
            }
            other => {
                diagnostics.push(
                    ops_error(
                        &self.pointer("y"),
                        format!(
                            "`y` must be a layer index, found {} (line {})",
                            other.type_phrase(),
                            node.line
                        ),
                    )
                    .with_help("layers are numbered from 0, bottom-up"),
                );
                None
            }
        }
    }

    fn text(&self, diagnostics: &mut Vec<Diagnostic>) -> Option<String> {
        let node = self.require("text", "add `text: \"...\"`", diagnostics)?;
        envelope::expect_string(
            node,
            &self.pointer("text"),
            "a comment's `text`",
            code::VOXEL_OPS_DOCUMENT_STRUCTURE,
            diagnostics,
        )
    }

    /// Reads `glyph`: exactly one character. A digit glyph resolves as a number in YAML, so numbers
    /// are accepted and rendered back to their text — `glyph: 1` and `glyph: "1"` mean the same.
    fn glyph(&self, diagnostics: &mut Vec<Diagnostic>) -> Option<char> {
        let node = self.require(
            "glyph",
            "add `glyph: <one character>` (`.` erases the cell)",
            diagnostics,
        )?;
        let text = match &node.value {
            NodeValue::Scalar(Value::String(text)) => text.clone(),
            NodeValue::Scalar(Value::Number(n)) => n.to_string(),
            NodeValue::Scalar(Value::Bool(b)) => b.to_string(),
            other => {
                diagnostics.push(
                    ops_error(
                        &self.pointer("glyph"),
                        format!(
                            "`glyph` must be a single character, found {} (line {})",
                            other.type_phrase(),
                            node.line
                        ),
                    )
                    .with_help("quote the glyph if it looks like something else: `glyph: \".\"`"),
                );
                return None;
            }
        };
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(glyph), None) => Some(glyph),
            _ => {
                diagnostics.push(
                    ops_error(
                        &self.pointer("glyph"),
                        format!(
                            "`glyph` is `{text}`, which is {} characters; a glyph is exactly one",
                            text.chars().count()
                        ),
                    )
                    .with_help(
                        "one printable-ASCII character naming a palette entry, or `.` to erase",
                    ),
                );
                None
            }
        }
    }

    fn mode(&self, diagnostics: &mut Vec<Diagnostic>) -> Option<WriteMode> {
        let Some(node) = self.node("mode") else {
            return Some(WriteMode::Place);
        };
        match self.enum_value("mode", node, &["place", "paint"], diagnostics)? {
            "place" => Some(WriteMode::Place),
            _ => Some(WriteMode::Paint),
        }
    }

    fn axis(&self, diagnostics: &mut Vec<Diagnostic>) -> Option<VoxelAxis> {
        let node = self.require("axis", "add `axis: x` (or `y`, `z`)", diagnostics)?;
        match self.enum_value("axis", node, &["x", "y", "z"], diagnostics)? {
            "x" => Some(VoxelAxis::X),
            "y" => Some(VoxelAxis::Y),
            _ => Some(VoxelAxis::Z),
        }
    }

    fn keep(&self, diagnostics: &mut Vec<Diagnostic>) -> Option<MirrorKeep> {
        let node = self.require(
            "keep",
            "add `keep: low` (or `high`) — which half is copied over the other",
            diagnostics,
        )?;
        match self.enum_value("keep", node, &["low", "high"], diagnostics)? {
            "low" => Some(MirrorKeep::Low),
            _ => Some(MirrorKeep::High),
        }
    }

    /// Reads a keyword-valued key, returning the matched keyword from `valid`.
    fn enum_value(
        &self,
        key: &str,
        node: &Node,
        valid: &'static [&'static str],
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<&'static str> {
        let NodeValue::Scalar(Value::String(text)) = &node.value else {
            diagnostics.push(
                ops_error(
                    &self.pointer(key),
                    format!(
                        "`{key}` must be one of {}, found {} (line {})",
                        valid.join("/"),
                        node.value.type_phrase(),
                        node.line
                    ),
                )
                .with_help(format!("write `{key}: {}`", valid[0])),
            );
            return None;
        };
        if let Some(matched) = valid.iter().find(|candidate| *candidate == text) {
            return Some(matched);
        }
        let mut diagnostic = ops_error(
            &self.pointer(key),
            format!(
                "`{key}` is `{text}`; the valid values are {}",
                valid.join(", ")
            ),
        );
        diagnostic = match suggest::nearest(text, valid.iter().copied()) {
            Some(candidate) => diagnostic.with_help(format!("nearest match is `{candidate}`")),
            None => diagnostic.with_help(format!("write `{key}: {}`", valid[0])),
        };
        diagnostics.push(diagnostic);
        None
    }
}

// --- The grid ----------------------------------------------------------------------------------

/// The in-memory cell grid the ops execute against — the same `(y, z, x)` layout the slice rows
/// use, so a row is a contiguous span.
///
/// `writers` is a parallel last-writer-op-index array feeding the `mirror`-shadow lint (`L1134`):
/// `writers[i]` is the 0-based index of the op that most recently *changed* `cells[i]`, or
/// `u32::MAX` for a cell the current ops session has not touched (still holding the model's
/// original value — never worth a shadow warning, since no op in this run owns it). A reshape
/// (`shift`/`grow`/`crop`) carries both arrays through the same remap, so provenance survives a
/// grid resize.
#[derive(Clone, PartialEq, Eq)]
struct Grid {
    size: [u32; 3],
    cells: Vec<char>,
    writers: Vec<u32>,
    /// The index of the op currently executing, written by `apply`'s loop before each op runs —
    /// what a changed cell's `writers` entry is set to.
    current_op: u32,
}

/// The sentinel meaning "no op in this session has written this cell".
const NO_WRITER: u32 = u32::MAX;

impl Grid {
    fn from_doc(doc: &VoxelDocument) -> Grid {
        let [sx, sy, sz] = doc.size;
        let mut cells = vec![EMPTY_GLYPH; (sx as usize) * (sy as usize) * (sz as usize)];
        for (y, layer) in doc.slices.iter().enumerate().take(sy as usize) {
            for (z, row) in layer.iter().enumerate().take(sz as usize) {
                for (x, glyph) in row.chars().enumerate().take(sx as usize) {
                    cells[(y * sz as usize + z) * sx as usize + x] = glyph;
                }
            }
        }
        let writers = vec![NO_WRITER; cells.len()];
        Grid {
            size: doc.size,
            cells,
            writers,
            current_op: NO_WRITER,
        }
    }

    fn index(&self, x: u32, y: u32, z: u32) -> usize {
        let [sx, _, sz] = self.size;
        (y as usize * sz as usize + z as usize) * sx as usize + x as usize
    }

    fn in_bounds(&self, at: [i64; 3]) -> bool {
        at.iter()
            .zip(self.size)
            .all(|(&value, extent)| value >= 0 && value < i64::from(extent))
    }

    fn get(&self, x: u32, y: u32, z: u32) -> char {
        self.cells[self.index(x, y, z)]
    }

    /// Writes one cell by index, recording `current_op` as its writer when the glyph actually
    /// changes. The shared endpoint every writing path (shape ops, `mirror`) funnels through, so
    /// the `L1134` provenance trail never has a second way to update.
    fn write_cell(&mut self, index: usize, glyph: char) -> bool {
        if self.cells[index] == glyph {
            return false;
        }
        self.cells[index] = glyph;
        self.writers[index] = self.current_op;
        true
    }

    /// Writes one cell, clipping out-of-range coordinates and honouring `mode`. Returns whether the
    /// cell's glyph actually changed — the number the summary reports.
    fn write(&mut self, at: [i64; 3], glyph: char, mode: WriteMode) -> bool {
        if !self.in_bounds(at) {
            return false;
        }
        let index = self.index(at[0] as u32, at[1] as u32, at[2] as u32);
        if mode == WriteMode::Paint && self.cells[index] == EMPTY_GLYPH {
            return false;
        }
        self.write_cell(index, glyph)
    }

    fn to_slices(&self) -> Vec<Vec<String>> {
        let [sx, sy, sz] = self.size;
        (0..sy)
            .map(|y| {
                (0..sz)
                    .map(|z| (0..sx).map(|x| self.get(x, y, z)).collect())
                    .collect()
            })
            .collect()
    }
}

// --- Execution ---------------------------------------------------------------------------------

/// The result of applying a list of ops to a model's source text.
#[derive(Clone, Debug)]
pub struct VoxelEdit {
    /// The complete new file text, already parsed and validated.
    pub text: String,
    /// The document `text` parses to.
    pub doc: VoxelDocument,
    /// How many cells each op changed, in op order (a `comment` op always reports 0).
    pub changed: Vec<u64>,
    /// Whether `text` is the original bytes patched at the edited cells (`true`) or a canonical
    /// re-emit with comments replayed (`false` — a grid reshape, a y-shift, or a row the patcher
    /// could not verify).
    pub surgical: bool,
    /// The comments a re-emit dropped because the canonical writer has nowhere to put them. Always
    /// empty for a surgical edit, which preserves every byte it does not target.
    pub dropped_comments: Vec<StrayComment>,
    /// `L1134` warnings: a `mirror` op that discarded cells an earlier op in this same run wrote.
    /// One entry per (mirror op, writer op) pair that lost cells, in the order the mirrors that
    /// raised them ran.
    pub warnings: Vec<Diagnostic>,
}

/// Applies `ops` to the voxel model in `source`, returning the new file text and what changed.
///
/// The model is parsed and validated first, the ops are executed against an in-memory grid, and the
/// result is parsed and validated again before it is returned — so a caller that writes
/// [`VoxelEdit::text`] can never persist a document that would not check.
pub(crate) fn apply(source: &str, ops: &[VoxelOp]) -> Result<VoxelEdit, Vec<Diagnostic>> {
    let doc = crate::check_voxel(source)?;
    let extracted = extract(source);
    let mut comments = extracted.comments.clone();
    let before = Grid::from_doc(&doc);
    let mut grid = before.clone();
    let mut changed = Vec::with_capacity(ops.len());
    let mut commented: Vec<u32> = Vec::new();
    let mut warnings: Vec<Diagnostic> = Vec::new();
    let mut reshaped = false;
    let mut layer_lines = extracted.layer_lines.clone();
    let mut moved_out: Vec<StrayComment> = Vec::new();

    {
        let mut state = EditState {
            comments: &mut comments,
            layer_lines: &mut layer_lines,
            dropped: &mut moved_out,
            commented: &mut commented,
            warnings: &mut warnings,
        };
        for (index, op) in ops.iter().enumerate() {
            reshaped |= op.reshapes();
            changed.push(execute(index, op, ops, &doc, &mut grid, &mut state)?);
        }
    }

    let mut new_doc = doc.clone();
    new_doc.size = grid.size;
    new_doc.slices = grid.to_slices();

    // The surgical path is attempted only when the grid kept its shape, and only accepted when the
    // patched text re-parses to exactly the document the ops produced — which is what makes
    // per-character patching safe against a row whose quoting the new glyph would have changed.
    let patched = (!reshaped)
        .then(|| surgical_patch(source, &before, &grid, &commented, &comments))
        .flatten()
        .filter(|text| crate::check_voxel(text).is_ok_and(|checked| checked == new_doc));

    let (text, surgical, dropped) = match patched {
        Some(text) => (text, true, Vec::new()),
        None => (
            crate::voxel::emit_with_comments(&new_doc, &comments),
            false,
            extracted.stray.into_iter().chain(moved_out).collect(),
        ),
    };

    let doc = crate::check_voxel(&text)?;
    Ok(VoxelEdit {
        text,
        doc,
        changed,
        surgical,
        dropped_comments: dropped,
        warnings,
    })
}

/// The mutable accumulators an op list's execution threads through beyond the grid itself: the
/// replayable comments, which layers a `comment` op has already touched (so a second `comment` on
/// the same layer replaces rather than duplicates), and the `L1134` warnings raised along the way.
/// Bundled so `execute` stays under clippy's argument-count lint without an `#[allow]`.
struct EditState<'a> {
    comments: &'a mut VoxelComments,
    /// Source line of each layer comment still taken from the file, kept in step with `comments`.
    layer_lines: &'a mut BTreeMap<u32, usize>,
    /// Layer comments a remap pushed off the grid, one entry per text line.
    dropped: &'a mut Vec<StrayComment>,
    commented: &'a mut Vec<u32>,
    warnings: &'a mut Vec<Diagnostic>,
}

/// Remaps the layer comments and their recorded source lines together, collecting the comments
/// whose layer leaves the grid into `state.dropped`.
fn remap_comments(state: &mut EditState<'_>, remap: impl Fn(u32) -> Option<u32>) {
    for (y, text) in state.comments.remap_layers(&remap) {
        let start = state.layer_lines.get(&y).copied();
        for (i, line) in text.split('\n').enumerate() {
            state.dropped.push(StrayComment {
                line: start.map_or(0, |s| s + i),
                text: if line.is_empty() {
                    "#".to_string()
                } else {
                    format!("# {line}")
                },
            });
        }
    }
    let lines = std::mem::take(state.layer_lines);
    *state.layer_lines = lines
        .into_iter()
        .filter_map(|(y, line)| remap(y).map(|moved| (moved, line)))
        .collect();
}

/// Executes one op, returning how many cells it changed. `ops` is the full list (for the
/// `mirror`-shadow lint's writer-op name lookup).
fn execute(
    index: usize,
    op: &VoxelOp,
    ops: &[VoxelOp],
    doc: &VoxelDocument,
    grid: &mut Grid,
    state: &mut EditState<'_>,
) -> Result<u64, Vec<Diagnostic>> {
    grid.current_op = index as u32;
    if let Some(glyph) = op_glyph(op) {
        check_glyph(index, op, glyph, doc)?;
    }
    let count = match op {
        VoxelOp::Set { at, glyph, mode } => {
            if !grid.in_bounds(*at) {
                return Err(vec![out_of_range(index, op, *at, grid.size)]);
            }
            u64::from(grid.write(*at, *glyph, *mode))
        }
        VoxelOp::Box {
            from,
            to,
            glyph,
            mode,
        } => {
            let clip = |i: usize| {
                (
                    from[i].min(to[i]).max(0),
                    from[i].max(to[i]).min(i64::from(grid.size[i]) - 1),
                )
            };
            let (x_range, y_range, z_range) = (clip(0), clip(1), clip(2));
            let mut count = 0;
            if x_range.0 <= x_range.1 && y_range.0 <= y_range.1 && z_range.0 <= z_range.1 {
                for y in y_range.0..=y_range.1 {
                    for z in z_range.0..=z_range.1 {
                        for x in x_range.0..=x_range.1 {
                            count += u64::from(grid.write([x, y, z], *glyph, *mode));
                        }
                    }
                }
            }
            count
        }
        VoxelOp::Ellipsoid {
            center,
            radii,
            glyph,
            mode,
        } => {
            let mut count = 0;
            for (x, y, z) in bounding_cells(center, radii, grid.size) {
                let d = [
                    (x as f64 - center[0]) / radii[0],
                    (y as f64 - center[1]) / radii[1],
                    (z as f64 - center[2]) / radii[2],
                ];
                if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= 1.0 {
                    count += u64::from(grid.write([x, y, z], *glyph, *mode));
                }
            }
            count
        }
        VoxelOp::Cylinder {
            base,
            axis,
            radius,
            height,
            glyph,
            mode,
        } => {
            let a = axis.index();
            let mut radii = [*radius; 3];
            radii[a] = *height;
            let mut center = *base;
            center[a] = base[a] + height / 2.0;
            let mut count = 0;
            for (x, y, z) in bounding_cells(&center, &radii, grid.size) {
                let cell = [x as f64, y as f64, z as f64];
                // Inclusive at the base, exclusive at the far end: stacking two cylinders of
                // height h then covers 2h layers with no doubled seam.
                if cell[a] < base[a] || cell[a] >= base[a] + height {
                    continue;
                }
                let (u, v) = other_axes(a);
                let du = (cell[u] - base[u]) / radius;
                let dv = (cell[v] - base[v]) / radius;
                if du * du + dv * dv <= 1.0 {
                    count += u64::from(grid.write([x, y, z], *glyph, *mode));
                }
            }
            count
        }
        VoxelOp::Line {
            from,
            to,
            glyph,
            mode,
        } => line_cells(*from, *to, grid.size)
            .ok_or_else(|| vec![line_too_long(index, *from, *to)])?
            .into_iter()
            .map(|cell| u64::from(grid.write(cell, *glyph, *mode)))
            .sum(),
        VoxelOp::Mirror { axis, keep } => {
            let (count, shadow) = mirror(grid, *axis, *keep, ops);
            state.warnings.extend(shadow);
            count
        }
        VoxelOp::Shift { by } => {
            let count = shift(grid, *by);
            if by[1] != 0 {
                let (dy, height) = (by[1], i64::from(grid.size[1]));
                remap_comments(state, |y| {
                    i64::from(y)
                        .checked_add(dy)
                        .filter(|n| (0..height).contains(n))
                        .map(|n| n as u32)
                });
            }
            count
        }
        VoxelOp::Grow { low, high } => {
            let count =
                grow(grid, *low, *high).map_err(|d| vec![d.with_path(format!("/ops/{index}"))])?;
            let dy = low[1];
            remap_comments(state, |y| y.checked_add(dy));
            count
        }
        VoxelOp::Crop { from, to } => {
            for component in 0..3 {
                if from[component] > to[component] || to[component] >= grid.size[component] {
                    return Err(vec![bad_crop(index, *from, *to, grid.size)]);
                }
            }
            let count = crop(grid, from, to);
            let (lo, hi) = (from[1], to[1]);
            remap_comments(state, |y| (y >= lo && y <= hi).then(|| y - lo));
            count
        }
        VoxelOp::Comment { y, text } => {
            state.comments.set_layer(*y, text.clone());
            state.layer_lines.remove(y);
            if !state.commented.contains(y) {
                state.commented.push(*y);
            }
            0
        }
    };
    Ok(count)
}

/// The glyph an op writes, if it writes one.
fn op_glyph(op: &VoxelOp) -> Option<char> {
    match op {
        VoxelOp::Set { glyph, .. }
        | VoxelOp::Box { glyph, .. }
        | VoxelOp::Ellipsoid { glyph, .. }
        | VoxelOp::Cylinder { glyph, .. }
        | VoxelOp::Line { glyph, .. } => Some(*glyph),
        _ => None,
    }
}

/// Every glyph but `.` must already name a palette entry — the ops engine never invents one, so a
/// typo'd glyph is caught before any cell moves rather than surfacing as an `L1114` afterwards.
fn check_glyph(
    index: usize,
    op: &VoxelOp,
    glyph: char,
    doc: &VoxelDocument,
) -> Result<(), Vec<Diagnostic>> {
    let needle = glyph.to_string();
    if glyph == EMPTY_GLYPH || doc.palette.iter().any(|entry| entry.glyph == needle) {
        return Ok(());
    }
    let known: Vec<&str> = doc
        .palette
        .iter()
        .map(|entry| entry.glyph.as_str())
        .collect();
    Err(vec![
        Diagnostic::new(
            code::VOXEL_OP_INAPPLICABLE,
            format!(
                "op {index} (`{}`) writes the glyph `{glyph}`, which names no palette entry",
                op.name()
            ),
        )
        .with_path(format!("/ops/{index}"))
        .with_help(format!(
            "add `{glyph}` to the model's palette first, or use one of: {} (or `.` to erase)",
            if known.is_empty() {
                "(the palette is empty)".to_string()
            } else {
                known.join(", ")
            }
        )),
    ])
}

fn out_of_range(index: usize, op: &VoxelOp, at: [i64; 3], size: [u32; 3]) -> Diagnostic {
    Diagnostic::new(
        code::VOXEL_OP_INAPPLICABLE,
        format!(
            "op {index} (`{}`) addresses the cell ({}, {}, {}), outside this {}x{}x{} model",
            op.name(),
            at[0],
            at[1],
            at[2],
            size[0],
            size[1],
            size[2]
        ),
    )
    .with_path(format!("/ops/{index}"))
    .with_help(format!(
        "valid coordinates are x 0..={}, y 0..={}, z 0..={}; a shape op (`box`, `ellipsoid`, \
         `cylinder`, `line`) clips instead of failing, but a `set` is exact",
        size[0] - 1,
        size[1] - 1,
        size[2] - 1
    ))
}

fn bad_crop(index: usize, from: [u32; 3], to: [u32; 3], size: [u32; 3]) -> Diagnostic {
    Diagnostic::new(
        code::VOXEL_OP_INAPPLICABLE,
        format!(
            "op {index} (`crop`) keeps ({}, {}, {})..=({}, {}, {}), which is empty or reaches \
             outside this {}x{}x{} model",
            from[0], from[1], from[2], to[0], to[1], to[2], size[0], size[1], size[2]
        ),
    )
    .with_path(format!("/ops/{index}"))
    .with_help(format!(
        "`crop` takes an inclusive range inside the grid: each `from` component must be <= its \
         `to`, and `to` must be < the extent (x < {}, y < {}, z < {})",
        size[0], size[1], size[2]
    ))
}

/// Every in-grid cell of the axis-aligned bounding box of a `center`/`radii` shape, in the grid's
/// own `(y, z, x)` scan order so a shape fills identically wherever it is evaluated.
fn bounding_cells(center: &[f64; 3], radii: &[f64; 3], size: [u32; 3]) -> Vec<(i64, i64, i64)> {
    let lo = |i: usize| ((center[i] - radii[i]).ceil() as i64).max(0);
    let hi = |i: usize| ((center[i] + radii[i]).floor() as i64).min(i64::from(size[i]) - 1);
    let mut cells = Vec::new();
    for y in lo(1)..=hi(1) {
        for z in lo(2)..=hi(2) {
            for x in lo(0)..=hi(0) {
                cells.push((x, y, z));
            }
        }
    }
    cells
}

/// The two axes that are not `axis`, in ascending order.
fn other_axes(axis: usize) -> (usize, usize) {
    match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    }
}

fn line_too_long(index: usize, from: [i64; 3], to: [i64; 3]) -> Diagnostic {
    Diagnostic::new(
        code::VOXEL_OP_INAPPLICABLE,
        format!(
            "op {index} (`line`) from ({}, {}, {}) to ({}, {}, {}) spans more than \
             {VOXEL_MAX_CELLS} cells",
            from[0], from[1], from[2], to[0], to[1], to[2]
        ),
    )
    .with_path(format!("/ops/{index}"))
    .with_help(
        "a `line` clips to the grid, but one spanning more than the cell ceiling is not a model \
         edit; bring the endpoints closer to the model",
    )
}

/// The in-grid cells of a 3D integer Bresenham line, both endpoints included when in the grid.
/// Deterministic: the driving axis is the first of the three with the largest extent. `None` when
/// the line spans more than [`VOXEL_MAX_CELLS`] cells (or its extent overflows `i64`).
fn line_cells(from: [i64; 3], to: [i64; 3], size: [u32; 3]) -> Option<Vec<[i64; 3]>> {
    let mut delta = [0i64; 3];
    for i in 0..3 {
        delta[i] = to[i].checked_sub(from[i])?;
    }
    let extent = delta.map(i64::unsigned_abs);
    if extent.iter().any(|&e| e > VOXEL_MAX_CELLS) {
        return None;
    }
    let extent = extent.map(|e| e as i64);
    let step = delta.map(i64::signum);
    let mut major = 0;
    for axis in 1..3 {
        if extent[axis] > extent[major] {
            major = axis;
        }
    }
    let (u, v) = other_axes(major);

    let mut cell = from;
    let mut cells = Vec::new();
    let in_grid = |cell: [i64; 3]| (0..3).all(|i| (0..i64::from(size[i])).contains(&cell[i]));
    if in_grid(cell) {
        cells.push(cell);
    }
    let mut error_u = 2 * extent[u] - extent[major];
    let mut error_v = 2 * extent[v] - extent[major];
    for _ in 0..extent[major] {
        if error_u > 0 {
            cell[u] += step[u];
            error_u -= 2 * extent[major];
        }
        if error_v > 0 {
            cell[v] += step[v];
            error_v -= 2 * extent[major];
        }
        error_u += 2 * extent[u];
        error_v += 2 * extent[v];
        cell[major] += step[major];
        if in_grid(cell) {
            cells.push(cell);
        }
    }
    Some(cells)
}

/// Copies the kept half of `axis` onto the other half. On an odd extent the centre slice mirrors
/// onto itself and is left alone.
///
/// Alongside the changed-cell count, tallies the `L1134` mirror-shadow lint: for every discarded
/// cell whose value this call actually changes, if that cell's last writer was an earlier op in
/// this run, one warning is owed to that writer (tallied, not raised per cell — a shape op can lose
/// dozens of cells to one mirror and that is one finding, not dozens).
fn mirror(
    grid: &mut Grid,
    axis: VoxelAxis,
    keep: MirrorKeep,
    ops: &[VoxelOp],
) -> (u64, Vec<Diagnostic>) {
    let a = axis.index();
    let extent = grid.size[a];
    let mirror_index = grid.current_op;
    let mut count = 0u64;
    let mut shadowed: BTreeMap<u32, u64> = BTreeMap::new();
    let [sx, sy, sz] = grid.size;
    for y in 0..sy {
        for z in 0..sz {
            for x in 0..sx {
                let cell = [x, y, z];
                if cell[a] >= extent / 2 {
                    continue;
                }
                let mut mirrored = cell;
                mirrored[a] = extent - 1 - cell[a];
                let (source, target) = match keep {
                    MirrorKeep::Low => (cell, mirrored),
                    MirrorKeep::High => (mirrored, cell),
                };
                let glyph = grid.get(source[0], source[1], source[2]);
                let target_index = grid.index(target[0], target[1], target[2]);
                let prior_writer = grid.writers[target_index];
                if grid.write_cell(target_index, glyph) {
                    count += 1;
                    if prior_writer != NO_WRITER && prior_writer < mirror_index {
                        *shadowed.entry(prior_writer).or_insert(0) += 1;
                    }
                }
            }
        }
    }
    let range = mirror_discard_range(axis, keep, extent);
    let warnings = shadowed
        .into_iter()
        .map(|(writer_index, cells)| {
            mirror_shadow_warning(
                mirror_index as usize,
                axis,
                keep,
                writer_index as usize,
                ops[writer_index as usize].name(),
                cells,
                &range,
            )
        })
        .collect();
    (count, warnings)
}

/// The axis letter a mirror-shadow message names the cell coordinate by.
fn axis_letter(axis: VoxelAxis) -> char {
    match axis {
        VoxelAxis::X => 'x',
        VoxelAxis::Y => 'y',
        VoxelAxis::Z => 'z',
    }
}

fn keep_word(keep: MirrorKeep) -> &'static str {
    match keep {
        MirrorKeep::Low => "low",
        MirrorKeep::High => "high",
    }
}

/// The discarded half's range along `axis`, in the same inequality shape a reader would write by
/// hand — `keep: low` discards the high half (`x >= threshold`), `keep: high` discards the low half
/// (`x < threshold`), split at `extent - extent / 2` so an odd extent's self-mirroring centre slice
/// (never discarded) is excluded from both.
fn mirror_discard_range(axis: VoxelAxis, keep: MirrorKeep, extent: u32) -> String {
    let letter = axis_letter(axis);
    match keep {
        MirrorKeep::Low => format!("{letter} >= {}", extent - extent / 2),
        MirrorKeep::High => format!("{letter} < {}", extent / 2),
    }
}

fn mirror_shadow_warning(
    mirror_index: usize,
    axis: VoxelAxis,
    keep: MirrorKeep,
    writer_index: usize,
    writer_name: &str,
    cells: u64,
    range: &str,
) -> Diagnostic {
    Diagnostic::new(
        code::VOXEL_OPS_SHADOWED_BY_MIRROR,
        format!(
            "op {mirror_index} (mirror {} keep {}) overwrote {cells} cell{} that op {writer_index} \
             ({writer_name}) wrote in the discarded half ({range})",
            axis_letter(axis),
            keep_word(keep),
            if cells == 1 { "" } else { "s" },
        ),
    )
    .with_path(format!("/ops/{mirror_index}"))
    .with_help(
        "author those cells in the kept half or order them after the mirror; if the overwrite is \
         intended, ignore this",
    )
}

/// Translates every cell by `by`, dropping what leaves the grid and emptying what it vacates. The
/// `writers` provenance moves with each cell, same as its glyph — a shift does not erase the
/// `L1134` write-history a later `mirror` reads.
fn shift(grid: &mut Grid, by: [i64; 3]) -> u64 {
    let [sx, sy, sz] = grid.size;
    let mut next_cells = vec![EMPTY_GLYPH; grid.cells.len()];
    let mut next_writers = vec![NO_WRITER; grid.writers.len()];
    for y in 0..sy {
        for z in 0..sz {
            for x in 0..sx {
                let target = [
                    i64::from(x).saturating_add(by[0]),
                    i64::from(y).saturating_add(by[1]),
                    i64::from(z).saturating_add(by[2]),
                ];
                if !grid.in_bounds(target) {
                    continue;
                }
                let source_index = grid.index(x, y, z);
                let index = grid.index(target[0] as u32, target[1] as u32, target[2] as u32);
                next_cells[index] = grid.cells[source_index];
                next_writers[index] = grid.writers[source_index];
            }
        }
    }
    let count = next_cells
        .iter()
        .zip(&grid.cells)
        .filter(|(a, b)| a != b)
        .count() as u64;
    grid.cells = next_cells;
    grid.writers = next_writers;
    count
}

/// Enlarges the grid with empty cells on each side, returning how many cells were added. `writers`
/// provenance carries over for every surviving cell (new cells are untouched, `NO_WRITER`) — same
/// rule as [`shift`].
fn grow(grid: &mut Grid, low: [u32; 3], high: [u32; 3]) -> Result<u64, Diagnostic> {
    let mut size = [0u32; 3];
    for i in 0..3 {
        size[i] = grid.size[i]
            .checked_add(low[i])
            .and_then(|s| s.checked_add(high[i]))
            .ok_or_else(|| grid_too_large(None, low, high))?;
    }
    let cells: u128 = size.iter().map(|&d| u128::from(d)).product();
    if cells > u128::from(VOXEL_MAX_CELLS) {
        return Err(grid_too_large(Some(size), low, high));
    }
    let total = cells as usize;
    let mut next = Grid {
        size,
        cells: vec![EMPTY_GLYPH; total],
        writers: vec![NO_WRITER; total],
        current_op: grid.current_op,
    };
    let [sx, sy, sz] = grid.size;
    for y in 0..sy {
        for z in 0..sz {
            for x in 0..sx {
                let source_index = grid.index(x, y, z);
                let index = next.index(x + low[0], y + low[1], z + low[2]);
                next.cells[index] = grid.cells[source_index];
                next.writers[index] = grid.writers[source_index];
            }
        }
    }
    let added = next.cells.len() as u64 - grid.cells.len() as u64;
    *grid = next;
    Ok(added)
}

/// `size` is `None` when an extent overflowed `u32` before a product could be formed.
fn grid_too_large(size: Option<[u32; 3]>, low: [u32; 3], high: [u32; 3]) -> Diagnostic {
    let result = match size {
        Some(s) => format!("a {}x{}x{} grid", s[0], s[1], s[2]),
        None => "a grid extent past 32 bits".to_string(),
    };
    Diagnostic::new(
        code::VOXEL_OP_INAPPLICABLE,
        format!(
            "growing by low {low:?} and high {high:?} would make {result}, past the \
             {VOXEL_MAX_CELLS}-cell ceiling"
        ),
    )
    .with_help("grow by less, or split the model; a model may hold at most 256x256x256 cells")
}

/// Shrinks the grid to the inclusive box `from..=to`, returning how many cells were discarded.
/// `writers` provenance carries over for every kept cell — same rule as [`shift`].
fn crop(grid: &mut Grid, from: &[u32; 3], to: &[u32; 3]) -> u64 {
    let size = [
        to[0] - from[0] + 1,
        to[1] - from[1] + 1,
        to[2] - from[2] + 1,
    ];
    let total = (size[0] as usize) * (size[1] as usize) * (size[2] as usize);
    let mut next = Grid {
        size,
        cells: vec![EMPTY_GLYPH; total],
        writers: vec![NO_WRITER; total],
        current_op: grid.current_op,
    };
    for y in 0..size[1] {
        for z in 0..size[2] {
            for x in 0..size[0] {
                let source_index = grid.index(x + from[0], y + from[1], z + from[2]);
                let index = next.index(x, y, z);
                next.cells[index] = grid.cells[source_index];
                next.writers[index] = grid.writers[source_index];
            }
        }
    }
    let dropped = grid.cells.len() as u64 - next.cells.len() as u64;
    *grid = next;
    dropped
}

// --- Upscale -------------------------------------------------------------------------------------

/// The factor range [`upscale`] accepts. 1 is a pointless copy; above 8 a 16^3 model is already a
/// 128^3 one, almost certainly a mistake rather than an intent.
const UPSCALE_FACTOR_RANGE: std::ops::RangeInclusive<u32> = 2..=8;

/// The result of nearest-neighbor upscaling a voxel document by an integer factor — the engine
/// behind `ludion voxel upscale`.
#[derive(Clone, Debug)]
pub struct VoxelUpscale {
    /// The complete new file text, already parsed and validated.
    pub text: String,
    /// The document `text` parses to (the upscaled model).
    pub doc: VoxelDocument,
    /// The factor applied.
    pub factor: u32,
    /// The source document's grid extent, before upscaling.
    pub source_size: [u32; 3],
    /// The source document's declared `scale`, before dividing by `factor` — kept alongside
    /// [`VoxelDocument::scale`] on the output so a caller can report old -> new without
    /// re-deriving the source value through a `* factor` that float rounding might not invert.
    pub source_scale: Option<f64>,
    /// The source's `generated_from`, dropped from the output — an upscale is not produced by the
    /// source's generator script, so carrying the key forward would make the output look like
    /// something it is not (and would fail a later `voxel-drift` regeneration check).
    pub dropped_generated_from: Option<String>,
    /// The comments a canonical re-emit dropped because the writer has nowhere to put them. An
    /// upscale always re-emits canonically (every cell moves), so this is the same bookkeeping
    /// [`VoxelEdit::dropped_comments`] does for a `grow`/`crop` reshape.
    pub dropped_comments: Vec<StrayComment>,
}

/// Upscales the voxel model in `source` by `factor`: each source cell becomes a `factor`^3 block of
/// identical cells, `size` and `scale` are rescaled so the model's declared world footprint is
/// unchanged, and every source layer's anatomy comment carries onto the first of its `factor` output
/// layers. `origin_label` — typically the source file's name — is appended to the header as one
/// `upscaled x<factor> from <label>` line when given; this function stays IO-free, so the caller
/// supplies it rather than this reading the filesystem.
///
/// The output is always a full canonical re-emit ([`crate::voxel::emit_with_comments`]) — there is no
/// surgical form of "every cell moves" — so a comment in a position the writer cannot reproduce is
/// dropped and reported, exactly as a `grow`/`crop` reshape reports it.
///
/// # Errors
/// [`code::VOXEL_OP_INAPPLICABLE`] (`L1132`) when `factor` is outside `2..=8`, or when scaling
/// `size` by `factor` would overflow a 32-bit dimension. Otherwise everything [`crate::check_voxel`]
/// can report, both on the source (before scaling) and — defensively — on the generated text.
pub(crate) fn upscale(
    source: &str,
    factor: u32,
    origin_label: Option<&str>,
) -> Result<VoxelUpscale, Vec<Diagnostic>> {
    if !UPSCALE_FACTOR_RANGE.contains(&factor) {
        return Err(vec![invalid_upscale_factor(factor)]);
    }

    let doc = crate::check_voxel(source)?;
    let extracted = extract(source);
    let new_size = scale_up_size(doc.size, factor).map_err(|d| vec![d])?;
    let new_cells: u128 = new_size.iter().map(|&d| u128::from(d)).product();
    if new_cells > u128::from(VOXEL_MAX_CELLS) {
        return Err(vec![upscale_too_large(new_size, factor)]);
    }

    let before = Grid::from_doc(&doc);
    let mut cells = vec![EMPTY_GLYPH; new_size.iter().map(|&d| d as usize).product()];
    let [sx, _, sz] = new_size;
    let [osx, osy, osz] = doc.size;
    for y in 0..osy {
        for z in 0..osz {
            for x in 0..osx {
                let glyph = before.get(x, y, z);
                if glyph == EMPTY_GLYPH {
                    continue;
                }
                for dy in 0..factor {
                    for dz in 0..factor {
                        let row_start = ((y * factor + dy) as usize * sz as usize
                            + (z * factor + dz) as usize)
                            * sx as usize;
                        let base = row_start + (x * factor) as usize;
                        for dx in 0..factor as usize {
                            cells[base + dx] = glyph;
                        }
                    }
                }
            }
        }
    }
    let cell_count = cells.len();
    let grid = Grid {
        size: new_size,
        cells,
        writers: vec![NO_WRITER; cell_count],
        current_op: NO_WRITER,
    };

    let dropped_generated_from = doc.generated_from.clone();
    let source_scale = doc.scale;
    let mut new_doc = doc.clone();
    new_doc.size = new_size;
    new_doc.slices = grid.to_slices();
    new_doc.scale = source_scale.map(|scale| scale / f64::from(factor));
    new_doc.generated_from = None;
    // `budget_exempt` is carried over as-is by the `doc.clone()` above.

    let mut comments = extracted.comments.clone();
    comments.remap_layers(|y| y.checked_mul(factor));
    comments.header.push(match origin_label {
        Some(label) => format!("upscaled x{factor} from {label}"),
        None => format!("upscaled x{factor}"),
    });

    let text = crate::voxel::emit_with_comments(&new_doc, &comments);
    let doc = crate::check_voxel(&text)?;

    Ok(VoxelUpscale {
        text,
        doc,
        factor,
        source_size: [osx, osy, osz],
        source_scale,
        dropped_generated_from,
        dropped_comments: extracted.stray,
    })
}

/// Scales a grid extent by `factor` component-wise, `u32`-checked. Its own function (rather than
/// inlined into [`upscale`]) so the overflow path is unit-testable without building a document
/// whose `slices` actually hold a multi-billion-character grid.
fn scale_up_size(size: [u32; 3], factor: u32) -> Result<[u32; 3], Diagnostic> {
    let mut out = [0u32; 3];
    for (slot, dim) in out.iter_mut().zip(size) {
        *slot = dim
            .checked_mul(factor)
            .ok_or_else(|| upscale_size_overflow(size, factor))?;
    }
    Ok(out)
}

fn invalid_upscale_factor(factor: u32) -> Diagnostic {
    Diagnostic::new(
        code::VOXEL_OP_INAPPLICABLE,
        format!(
            "upscale factor {factor} is out of range; it must be between {} and {} inclusive",
            UPSCALE_FACTOR_RANGE.start(),
            UPSCALE_FACTOR_RANGE.end()
        ),
    )
    .with_help(
        "1 is a pointless copy and above 8 is almost certainly a mistake (a 16^3 model at 8 is \
         already 128^3); pick a factor in 2..=8",
    )
}

fn upscale_too_large(size: [u32; 3], factor: u32) -> Diagnostic {
    Diagnostic::new(
        code::VOXEL_OP_INAPPLICABLE,
        format!(
            "upscaling by {factor} would make a {}x{}x{} grid, past the {VOXEL_MAX_CELLS}-cell \
             ceiling",
            size[0], size[1], size[2]
        ),
    )
    .with_help("choose a smaller factor, or shrink the source model first")
}

fn upscale_size_overflow(size: [u32; 3], factor: u32) -> Diagnostic {
    Diagnostic::new(
        code::VOXEL_OP_INAPPLICABLE,
        format!(
            "upscaling this {}x{}x{} model by {factor} would overflow a 32-bit grid dimension",
            size[0], size[1], size[2]
        ),
    )
    .with_help("choose a smaller factor, or shrink the source model first")
}

// --- Surgical write-back -------------------------------------------------------------------------

/// One slice row's position in the source text: the line it lives on, and the byte column its first
/// cell character sits at (the same as its char column — a row is printable ASCII).
struct RowSpan {
    line: usize,
    col: usize,
}

struct LayerSpan {
    marker: usize,
    rows: Vec<RowSpan>,
}

/// Rewrites `source` in place: one character per changed cell, plus the comment line above each
/// layer a `comment` op touched. Returns `None` when the text cannot be located confidently, which
/// makes the caller fall back to a canonical re-emit.
fn surgical_patch(
    source: &str,
    before: &Grid,
    after: &Grid,
    commented: &[u32],
    comments: &VoxelComments,
) -> Option<String> {
    let mut lines: Vec<String> = source.split('\n').map(str::to_string).collect();
    let layers = locate_layers(&lines, before)?;

    let [sx, sy, sz] = after.size;
    for y in 0..sy {
        for z in 0..sz {
            let span = layers.get(y as usize)?.rows.get(z as usize)?;
            for x in 0..sx {
                let glyph = after.get(x, y, z);
                if glyph == before.get(x, y, z) {
                    continue;
                }
                if !glyph.is_ascii() {
                    return None;
                }
                let mut bytes = std::mem::take(&mut lines[span.line]).into_bytes();
                *bytes.get_mut(span.col + x as usize)? = glyph as u8;
                lines[span.line] = String::from_utf8(bytes).ok()?;
            }
        }
    }

    // Comment edits change the line count, so they run last and from the bottom up — every span
    // index above the edit stays valid.
    let mut targets: Vec<u32> = commented.to_vec();
    targets.sort_unstable();
    targets.dedup();
    for y in targets.into_iter().rev() {
        let marker = layers.get(y as usize)?.marker;
        let indent: String = lines[marker]
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();
        let mut start = marker;
        while start > 0 && lines[start - 1].trim_start().starts_with('#') {
            start -= 1;
        }
        let replacement: Vec<String> = comments
            .layer(y)
            .into_iter()
            .flat_map(|text| {
                text.lines()
                    .map(|line| comment_line(&indent, line).trim_end().to_string())
                    .collect::<Vec<_>>()
            })
            .collect();
        lines.splice(start..marker, replacement);
    }

    Some(lines.join("\n"))
}

/// Finds every slice layer's `-` marker line and the position of each of its rows' cells, verifying
/// each located row against the parsed grid. Any mismatch returns `None` rather than guessing.
fn locate_layers(lines: &[String], grid: &Grid) -> Option<Vec<LayerSpan>> {
    let start = lines
        .iter()
        .position(|line| line.trim_end() == "slices:" && !line.starts_with(char::is_whitespace))?;
    let [sx, sy, sz] = grid.size;

    let mut layers: Vec<LayerSpan> = Vec::with_capacity(sy as usize);
    for (offset, line) in lines.iter().enumerate().skip(start + 1) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if is_layer_marker(trimmed) {
            layers.push(LayerSpan {
                marker: offset,
                rows: Vec::with_capacity(sz as usize),
            });
            continue;
        }
        if !trimmed.starts_with("- ") {
            // Not a slice row: `slices:` must be the last key of a canonical document, so anything
            // else here means the text is shaped in a way this patcher does not model.
            return None;
        }
        let y = u32::try_from(layers.len().checked_sub(1)?).ok()?;
        let layer = layers.last_mut()?;
        let z = u32::try_from(layer.rows.len()).ok()?;
        if y >= sy || z >= sz {
            return None;
        }
        let dash = line.find('-')?;
        if !line[..dash].chars().all(char::is_whitespace) {
            return None;
        }
        let mut col = dash + 2;
        if line.as_bytes().get(col) == Some(&b'"') {
            col += 1;
        }
        let end = col + sx as usize;
        let row = line.get(col..end)?;
        let expected: String = (0..sx).map(|x| grid.get(x, y, z)).collect();
        if row != expected || !row.is_ascii() {
            return None;
        }
        let rest = line[end..].trim_end();
        if !(rest.is_empty() || rest == "\"") {
            return None;
        }
        layer.rows.push(RowSpan { line: offset, col });
    }

    let shaped =
        layers.len() == sy as usize && layers.iter().all(|layer| layer.rows.len() == sz as usize);
    shaped.then_some(layers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_anchor_renumbers_both_bounds() {
        let up = |y: u32| y.checked_add(1);
        assert_eq!(
            relabel_anchor("y3-y5 the tower", 3, 4, &up),
            "y4-y6 the tower"
        );
        assert_eq!(relabel_anchor("y3 the tower", 3, 4, &up), "y4 the tower");
    }

    #[test]
    fn a_range_anchor_with_a_dropped_bound_or_trailing_text_is_unchanged() {
        let up = |y: u32| y.checked_add(1);
        assert_eq!(relabel_anchor("y3-y5x", 3, 4, &up), "y3-y5x");
        assert_eq!(relabel_anchor("y3x", 3, 4, &up), "y3x");
        let off_grid = |y: u32| (y < 5).then_some(y + 1);
        assert_eq!(
            relabel_anchor("y3-y5 tower", 3, 4, &off_grid),
            "y3-y5 tower"
        );
    }

    #[test]
    fn a_line_is_symmetric_and_inclusive() {
        let cells = line_cells([0, 0, 0], [3, 1, 0], [8, 8, 8]).unwrap();
        assert_eq!(cells.first(), Some(&[0, 0, 0]));
        assert_eq!(cells.last(), Some(&[3, 1, 0]));
        assert_eq!(cells.len(), 4);
    }

    #[test]
    fn a_degenerate_line_is_one_cell() {
        assert_eq!(
            line_cells([2, 2, 2], [2, 2, 2], [8, 8, 8]),
            Some(vec![[2, 2, 2]])
        );
    }

    #[test]
    fn a_line_keeps_only_its_in_grid_cells() {
        let cells = line_cells([-2, 0, 0], [3, 0, 0], [2, 1, 1]).unwrap();
        assert_eq!(cells, vec![[0, 0, 0], [1, 0, 0]]);
    }

    #[test]
    fn a_line_past_the_ceiling_or_the_i64_range_is_refused() {
        let size = [4, 4, 4];
        assert!(line_cells([0, 0, 0], [10_000_000_000, 0, 0], size).is_none());
        assert!(line_cells([i64::MIN, 0, 0], [i64::MAX, 0, 0], size).is_none());
    }

    #[test]
    fn scaling_size_normally_multiplies_every_component() {
        assert_eq!(scale_up_size([13, 12, 11], 3).unwrap(), [39, 36, 33]);
    }

    #[test]
    fn scaling_size_past_u32_is_l1132() {
        let diagnostic = scale_up_size([u32::MAX, 1, 1], 2).unwrap_err();
        assert_eq!(diagnostic.code, code::VOXEL_OP_INAPPLICABLE);
    }
}
