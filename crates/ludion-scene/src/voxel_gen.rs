//! The voxel generator intermediate: `kind: voxel-gen` JSON in, a [`VoxelDocument`] plus its
//! authoring comments out (`L1133`) — the engine half of the from-scratch generation path.
//!
//! Generating a model that is too large to hand-author is a two-step pipeline: a committed Node
//! script (`<name>.voxel.gen.mjs`, over the shared `scripts/voxel/grid.mjs` library) builds the grid
//! and prints one JSON object; `ludion voxel from-json` turns that into canonical `.voxel.yaml`.
//! The split exists so that **scripts never write YAML** — canonical serialization stays in exactly
//! one place, the way `emit_voxel` is the one writer for every other birthing verb
//! (`Plan/voxel-authoring-tools.md` decision 5). The specimen this replaces hand-concatenated the
//! format, comment conventions included, by string template.
//!
//! JSON rather than the crate's strict-YAML subset because the producer is a Node script:
//! `JSON.stringify` is in every runtime, needs no library, and cannot emit a construct the reader
//! rejects. This module therefore owns only the intermediate's *shape*; the moment it parses, the
//! described model goes through [`crate::validate_voxel`] unchanged, so a mis-sized row is the same
//! `L1114` a hand-authored one would raise.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::suggest;
use serde_json::{Map, Value};

use crate::voxel::{PaletteEntry, VOXEL_FORMAT_VERSION, VoxelDocument};
use crate::voxel_edit::VoxelComments;
use crate::yaml::pointer_token;

/// The `kind:` an intermediate must declare — deliberately not `voxel`, so a stray `.voxel.yaml`
/// converted to JSON cannot be mistaken for one.
const GEN_KIND: &str = "voxel-gen";

const GEN_KEYS: [&str; 11] = [
    "format",
    "kind",
    "name",
    "size",
    "scale",
    "generated_from",
    "budget_exempt",
    "palette",
    "header",
    "layer_comments",
    "layers",
];

/// The palette-entry keys an intermediate may carry — the same four the YAML format defines.
const PALETTE_ENTRY_KEYS: [&str; 4] = ["color", "emissive", "metallic", "roughness"];

/// Parses a generator intermediate into the model it describes and the comments to write with it.
pub(crate) fn from_json(source: &str) -> Result<(VoxelDocument, VoxelComments), Vec<Diagnostic>> {
    let root: Value = serde_json::from_str(source).map_err(|error| {
        vec![
            Diagnostic::new(
                code::VOXEL_GEN_INTERMEDIATE,
                format!("the generator intermediate is not valid JSON: {error}"),
            )
            .with_path("")
            .with_help(
                "a generator script must print exactly one JSON object and nothing else — a stray \
                 `console.log` of progress text lands in the same stream and breaks the parse",
            ),
        ]
    })?;
    let Value::Object(root) = root else {
        return Err(vec![
            Diagnostic::new(
                code::VOXEL_GEN_INTERMEDIATE,
                format!(
                    "the generator intermediate must be a JSON object, found {}",
                    type_phrase(&root)
                ),
            )
            .with_path("")
            .with_help(
                "print one object with `format`, `kind: \"voxel-gen\"`, `size`, `palette`, and \
                 `layers` (`ludion explain L1133` prints the full skeleton)",
            ),
        ]);
    };

    let mut diagnostics = Vec::new();
    let mut doc = VoxelDocument::default();
    let mut comments = VoxelComments::default();

    for key in root.keys() {
        if !GEN_KEYS.contains(&key.as_str()) {
            diagnostics.push(unknown_key(key));
        }
    }
    check_format(&root, &mut diagnostics);
    check_kind(&root, &mut diagnostics);

    if let Some(size) = require(&root, "size", "the grid extent", &mut diagnostics)
        .and_then(|node| parse_size(node, &mut diagnostics))
    {
        doc.size = size;
    }
    doc.name = optional_string(&root, "name", &mut diagnostics);
    doc.scale = optional_number(&root, "scale", &mut diagnostics);
    if let Some(value) = optional_string(&root, "generated_from", &mut diagnostics)
        && reject_empty(&value, "generated_from", &mut diagnostics)
    {
        doc.generated_from = Some(value);
    }
    doc.budget_exempt = optional_bool(&root, "budget_exempt", &mut diagnostics);
    if let Some(node) = require(&root, "palette", "the glyph colors", &mut diagnostics) {
        doc.palette = parse_palette(node, &mut diagnostics);
    }
    if let Some(node) = require(&root, "layers", "the grid rows", &mut diagnostics) {
        doc.slices = parse_layers(node, &mut diagnostics);
    }
    comments.header = parse_header(&root, &mut diagnostics);
    parse_layer_comments(&root, &mut comments, &mut diagnostics);

    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    // The intermediate is well-formed; the model it describes now faces exactly the checks a
    // hand-authored one does, so a mis-sized row or an unknown glyph reads as the usual `L1114`.
    let diagnostics = crate::validate_voxel(&doc);
    if diagnostics.is_empty() {
        Ok((doc, comments))
    } else {
        Err(diagnostics)
    }
}

fn check_format(root: &Map<String, Value>, diagnostics: &mut Vec<Diagnostic>) {
    let Some(node) = require(
        root,
        "format",
        "the intermediate format version",
        diagnostics,
    ) else {
        return;
    };
    match node.as_u64() {
        Some(version) if version == VOXEL_FORMAT_VERSION => {}
        Some(version) => diagnostics.push(
            error(
                "/format",
                format!(
                    "the generator intermediate declares `format: {version}`, but this engine \
                     reads format {VOXEL_FORMAT_VERSION}"
                ),
            )
            .with_help(format!("write `\"format\": {VOXEL_FORMAT_VERSION}`")),
        ),
        None => diagnostics.push(
            error(
                "/format",
                format!(
                    "`format` must be the number {VOXEL_FORMAT_VERSION}, found {}",
                    type_phrase(node)
                ),
            )
            .with_help(format!("write `\"format\": {VOXEL_FORMAT_VERSION}`")),
        ),
    }
}

fn check_kind(root: &Map<String, Value>, diagnostics: &mut Vec<Diagnostic>) {
    let Some(node) = require(root, "kind", "what this document is", diagnostics) else {
        return;
    };
    match node.as_str() {
        Some(GEN_KIND) => {}
        Some(other) => diagnostics.push(
            error(
                "/kind",
                format!(
                    "this is a `{other}` document; `voxel from-json` reads a `{GEN_KIND}` \
                     intermediate"
                ),
            )
            .with_help(format!(
                "write `\"kind\": \"{GEN_KIND}\"` — the `.voxel.yaml` model this produces is the \
                 `kind: voxel` document, not its input"
            )),
        ),
        None => diagnostics.push(
            error(
                "/kind",
                format!("`kind` must be a string, found {}", type_phrase(node)),
            )
            .with_help(format!("write `\"kind\": \"{GEN_KIND}\"`")),
        ),
    }
}

/// `size`: an array of exactly three positive integers that fit in `u32`.
fn parse_size(node: &Value, diagnostics: &mut Vec<Diagnostic>) -> Option<[u32; 3]> {
    let Some(items) = node.as_array() else {
        diagnostics.push(
            error(
                "/size",
                format!(
                    "`size` must be an array of three positive integers, found {}",
                    type_phrase(node)
                ),
            )
            .with_help("write `\"size\": [16, 24, 16]` — width (x), height (y), depth (z)"),
        );
        return None;
    };
    if items.len() != 3 {
        diagnostics.push(
            error(
                "/size",
                format!(
                    "`size` must have exactly 3 entries (x, y, z), found {}",
                    items.len()
                ),
            )
            .with_help("supply exactly three positive integers"),
        );
        return None;
    }
    let mut out = [0u32; 3];
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match item
            .as_u64()
            .filter(|value| *value >= 1 && *value <= u64::from(u32::MAX))
        {
            Some(value) => out[index] = value as u32,
            None => {
                diagnostics.push(
                    error(
                        format!("/size/{index}"),
                        format!(
                            "`size[{index}]` must be a positive integer (at least 1), found {}",
                            type_phrase(item)
                        ),
                    )
                    .with_help("each voxel dimension must be a positive whole number"),
                );
                ok = false;
            }
        }
    }
    ok.then_some(out)
}

/// `palette`: an object of glyph keys to `{color, emissive?, metallic?, roughness?}` entries,
/// returned in sorted-glyph order (the order the canonical writer emits). Glyph *validity* is left
/// to [`crate::validate_voxel`] — one place decides what a glyph may be.
fn parse_palette(node: &Value, diagnostics: &mut Vec<Diagnostic>) -> Vec<PaletteEntry> {
    let Some(entries) = node.as_object() else {
        diagnostics.push(
            error(
                "/palette",
                format!(
                    "`palette` must be an object of glyphs to colors, found {}",
                    type_phrase(node)
                ),
            )
            .with_help("write `\"palette\": {\"A\": {\"color\": [1.0, 1.0, 1.0, 1.0]}}`"),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(entries.len());
    for (glyph, entry) in entries {
        let pointer = format!("/palette/{}", pointer_token(glyph));
        if let Some(entry) = parse_palette_entry(entry, &pointer, glyph, diagnostics) {
            out.push(entry);
        }
    }
    out.sort_by(|a, b| a.glyph.cmp(&b.glyph));
    out
}

fn parse_palette_entry(
    node: &Value,
    pointer: &str,
    glyph: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<PaletteEntry> {
    let Some(fields) = node.as_object() else {
        diagnostics.push(
            error(
                pointer,
                format!(
                    "the palette entry `{glyph}` must be an object with a `color`, found {}",
                    type_phrase(node)
                ),
            )
            .with_help("write `{\"color\": [r, g, b, a]}`"),
        );
        return None;
    };
    for key in fields.keys() {
        if !PALETTE_ENTRY_KEYS.contains(&key.as_str()) {
            diagnostics.push(unknown_field(
                key,
                &format!("{pointer}/{}", pointer_token(key)),
                "a palette entry",
                PALETTE_ENTRY_KEYS,
            ));
        }
    }
    let color = match fields.get("color") {
        Some(color) => parse_color(color, &format!("{pointer}/color"), diagnostics),
        None => {
            diagnostics.push(
                error(
                    pointer,
                    format!("the palette entry `{glyph}` is missing its required `color`"),
                )
                .with_help("add `\"color\": [red, green, blue, alpha]`"),
            );
            None
        }
    };
    // Range checks belong to `validate_voxel`; only "is it a number" is this module's business.
    let scalar = |field: &str, diagnostics: &mut Vec<Diagnostic>| -> Option<f64> {
        let value = fields.get(field)?;
        match value.as_f64().map(ludion_core::number::engine_number) {
            Some(value) => Some(value),
            None => {
                diagnostics.push(
                    error(
                        format!("{pointer}/{field}"),
                        format!("`{field}` must be a number, found {}", type_phrase(value)),
                    )
                    .with_help(format!("write `\"{field}\": 0.5`")),
                );
                None
            }
        }
    };
    let emissive = scalar("emissive", diagnostics);
    let metallic = scalar("metallic", diagnostics);
    let roughness = scalar("roughness", diagnostics);

    color.map(|color| PaletteEntry {
        glyph: glyph.to_string(),
        color,
        emissive,
        metallic,
        roughness,
    })
}

fn parse_color(node: &Value, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<[f64; 4]> {
    let Some(items) = node.as_array() else {
        diagnostics.push(
            error(
                pointer,
                format!(
                    "`color` must be an array of four numbers, found {}",
                    type_phrase(node)
                ),
            )
            .with_help("write `[red, green, blue, alpha]`"),
        );
        return None;
    };
    if items.len() != 4 {
        diagnostics.push(
            error(
                pointer,
                format!(
                    "`color` must have exactly 4 components (red, green, blue, alpha), found {}",
                    items.len()
                ),
            )
            .with_help("supply exactly four numbers"),
        );
        return None;
    }
    let mut out = [0.0f64; 4];
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match item.as_f64().map(ludion_core::number::engine_number) {
            Some(value) => out[index] = value,
            None => {
                diagnostics.push(error(
                    format!("{pointer}/{index}"),
                    format!(
                        "`color[{index}]` must be a number, found {}",
                        type_phrase(item)
                    ),
                ));
                ok = false;
            }
        }
    }
    ok.then_some(out)
}

/// `layers`: an array of layers, each an array of row strings — `layers[y][z]`, the same
/// orientation as the YAML `slices`, so the value passes straight through.
fn parse_layers(node: &Value, diagnostics: &mut Vec<Diagnostic>) -> Vec<Vec<String>> {
    let Some(layers) = node.as_array() else {
        diagnostics.push(
            error(
                "/layers",
                format!(
                    "`layers` must be an array of layers, found {}",
                    type_phrase(node)
                ),
            )
            .with_help(
                "write one layer per y level, bottom-up, each an array of row strings: \
                 `[[\"AA\", \"AA\"]]`",
            ),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(layers.len());
    for (y, layer) in layers.iter().enumerate() {
        let Some(rows) = layer.as_array() else {
            diagnostics.push(
                error(
                    format!("/layers/{y}"),
                    format!(
                        "layer {y} must be an array of row strings, found {}",
                        type_phrase(layer)
                    ),
                )
                .with_help("each layer holds one row string per z (row r maps to z = r)"),
            );
            out.push(Vec::new());
            continue;
        };
        let mut row_strings = Vec::with_capacity(rows.len());
        for (z, row) in rows.iter().enumerate() {
            match row.as_str() {
                Some(text) => row_strings.push(text.to_string()),
                None => diagnostics.push(
                    error(
                        format!("/layers/{y}/{z}"),
                        format!(
                            "row z = {z} of layer y = {y} must be a string, found {}",
                            type_phrase(row)
                        ),
                    )
                    .with_help("rows are strings of glyphs, `.` for an empty cell"),
                ),
            }
        }
        out.push(row_strings);
    }
    out
}

/// `header`: the model's docstring, one array entry per line.
fn parse_header(root: &Map<String, Value>, diagnostics: &mut Vec<Diagnostic>) -> Vec<String> {
    let Some(node) = root.get("header") else {
        return Vec::new();
    };
    let Some(items) = node.as_array() else {
        diagnostics.push(
            error(
                "/header",
                format!(
                    "`header` must be an array of comment lines, found {}",
                    type_phrase(node)
                ),
            )
            .with_help("write `\"header\": [\"what this reads as at a glance\", \"...\"]`"),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        match item.as_str() {
            Some(text) => out.push(text.to_string()),
            None => diagnostics.push(error(
                format!("/header/{index}"),
                format!(
                    "`header[{index}]` must be a string, found {}",
                    type_phrase(item)
                ),
            )),
        }
    }
    out
}

/// `layer_comments`: layer index (as a decimal string — JSON has no integer keys) to anatomy text.
fn parse_layer_comments(
    root: &Map<String, Value>,
    comments: &mut VoxelComments,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(node) = root.get("layer_comments") else {
        return;
    };
    let Some(entries) = node.as_object() else {
        diagnostics.push(
            error(
                "/layer_comments",
                format!(
                    "`layer_comments` must be an object of layer indices to text, found {}",
                    type_phrase(node)
                ),
            )
            .with_help("write `\"layer_comments\": {\"0\": \"splayed feet\", \"12\": \"hips\"}`"),
        );
        return;
    };
    for (key, value) in entries {
        let pointer = format!("/layer_comments/{}", pointer_token(key));
        let Ok(y) = key.parse::<u32>() else {
            diagnostics.push(
                error(
                    &pointer,
                    format!("`{key}` is not a layer index; the keys of `layer_comments` are y levels written as decimal strings"),
                )
                .with_help("write `\"12\": \"hips\"` — JSON object keys are always strings"),
            );
            continue;
        };
        match value.as_str() {
            Some(text) => comments.set_layer(y, text),
            None => diagnostics.push(error(
                &pointer,
                format!(
                    "the comment for layer {y} must be a string, found {}",
                    type_phrase(value)
                ),
            )),
        }
    }
}

// --- Shared plumbing ----------------------------------------------------------------------------

fn error(pointer: impl Into<String>, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(code::VOXEL_GEN_INTERMEDIATE, message).with_path(pointer.into())
}

/// Fetches a required top-level key, reporting its absence.
fn require<'a>(
    root: &'a Map<String, Value>,
    key: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<&'a Value> {
    match root.get(key) {
        Some(node) => Some(node),
        None => {
            diagnostics.push(
                error(
                    format!("/{key}"),
                    format!("the generator intermediate is missing `{key}` ({what})"),
                )
                .with_help(
                    "every intermediate carries `format`, `kind`, `size`, `palette`, and \
                     `layers` — `ludion explain L1133` prints the skeleton",
                ),
            );
            None
        }
    }
}

fn optional_string(
    root: &Map<String, Value>,
    key: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<String> {
    let node = root.get(key)?;
    match node.as_str() {
        Some(text) => Some(text.to_string()),
        None => {
            diagnostics.push(error(
                format!("/{key}"),
                format!("`{key}` must be a string, found {}", type_phrase(node)),
            ));
            None
        }
    }
}

fn optional_number(
    root: &Map<String, Value>,
    key: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<f64> {
    let node = root.get(key)?;
    match node.as_f64().map(ludion_core::number::engine_number) {
        Some(value) => Some(value),
        None => {
            diagnostics.push(error(
                format!("/{key}"),
                format!("`{key}` must be a number, found {}", type_phrase(node)),
            ));
            None
        }
    }
}

fn optional_bool(
    root: &Map<String, Value>,
    key: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<bool> {
    let node = root.get(key)?;
    match node.as_bool() {
        Some(value) => Some(value),
        None => {
            diagnostics.push(error(
                format!("/{key}"),
                format!("`{key}` must be a boolean, found {}", type_phrase(node)),
            ));
            None
        }
    }
}

/// Whether an authored string is non-empty. Emptiness is caught here rather than left to
/// `validate_voxel` because an empty path names no file at all — there is no consistency question
/// left to ask about it.
fn reject_empty(value: &str, key: &str, diagnostics: &mut Vec<Diagnostic>) -> bool {
    if !value.is_empty() {
        return true;
    }
    diagnostics.push(
        error(
            format!("/{key}"),
            format!("`{key}` is empty; it must name the generator script that owns this model"),
        )
        .with_help(
            "write the script's path relative to the model, e.g. `\"brazier.voxel.gen.mjs\"`",
        ),
    );
    false
}

fn unknown_key(key: &str) -> Diagnostic {
    unknown_field(
        key,
        &format!("/{}", pointer_token(key)),
        "the generator intermediate",
        GEN_KEYS,
    )
}

fn unknown_field<const N: usize>(
    key: &str,
    pointer: &str,
    what: &str,
    known: [&str; N],
) -> Diagnostic {
    let diagnostic = error(pointer, format!("{what} does not have a key named `{key}`"));
    match suggest::nearest(key, known) {
        Some(candidate) => diagnostic.with_help(format!("nearest match is `{candidate}`")),
        None => diagnostic.with_help(format!("the keys are: {}", known.join(", "))),
    }
}

/// A JSON value's type, phrased for a diagnostic — the JSON analog of `NodeValue::type_phrase`.
fn type_phrase(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GEM: &str = r#"{
      "format": 1, "kind": "voxel-gen", "name": "gem",
      "size": [2, 1, 2],
      "palette": {"A": {"color": [1.0, 0.0, 0.0, 1.0]}},
      "layers": [["A.", ".A"]]
    }"#;

    #[test]
    fn a_minimal_intermediate_becomes_a_model() {
        let (doc, comments) = from_json(GEM).expect("the intermediate is well-formed");
        assert_eq!(doc.size, [2, 1, 2]);
        assert_eq!(doc.name.as_deref(), Some("gem"));
        assert_eq!(doc.slices, vec![vec!["A.".to_string(), ".A".to_string()]]);
        assert!(comments.is_empty());
    }

    #[test]
    fn header_and_layer_comments_ride_along() {
        let source = GEM.replace(
            "\"layers\"",
            "\"header\": [\"a gem\"], \"layer_comments\": {\"0\": \"the only layer\"}, \"layers\"",
        );
        let (_doc, comments) = from_json(&source).expect("comments parse");
        assert_eq!(comments.header, vec!["a gem".to_string()]);
        assert_eq!(comments.layer(0), Some("the only layer"));
    }

    #[test]
    fn a_misspelled_key_suggests_the_nearest() {
        let diagnostics = from_json(&GEM.replace("\"layers\"", "\"layer\"")).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/layer"))
            .expect("an unknown-key diagnostic");
        assert_eq!(d.code, code::VOXEL_GEN_INTERMEDIATE);
        assert_eq!(d.help.as_deref(), Some("nearest match is `layers`"));
    }

    #[test]
    fn a_layer_index_that_is_not_a_number_is_l1133() {
        let source = GEM.replace(
            "\"layers\"",
            "\"layer_comments\": {\"hips\": \"x\"}, \"layers\"",
        );
        let diagnostics = from_json(&source).unwrap_err();
        assert_eq!(diagnostics[0].code, code::VOXEL_GEN_INTERMEDIATE);
        assert_eq!(diagnostics[0].path.as_deref(), Some("/layer_comments/hips"));
    }

    /// A structurally valid intermediate whose grid does not match `size` is the model's own
    /// `L1114`, not this module's `L1133` — one place decides what a valid model is.
    #[test]
    fn a_short_row_is_the_models_own_l1114() {
        let diagnostics = from_json(&GEM.replace("\"A.\"", "\"A\"")).unwrap_err();
        assert_eq!(diagnostics[0].code, code::VOXEL_DATA_CONSISTENCY);
    }

    #[test]
    fn a_non_json_stream_names_the_parse_failure() {
        let diagnostics = from_json("building brazier...\n{}").unwrap_err();
        assert_eq!(diagnostics[0].code, code::VOXEL_GEN_INTERMEDIATE);
        assert_eq!(diagnostics[0].path.as_deref(), Some(""));
    }

    #[test]
    fn the_wrong_kind_is_rejected() {
        let diagnostics = from_json(&GEM.replace("voxel-gen", "voxel")).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/kind"))
            .expect("a /kind diagnostic");
        assert!(d.message.contains("voxel-gen"), "{}", d.message);
    }
}
