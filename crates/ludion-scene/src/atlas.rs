//! Atlas documents: the bake's input (`kind: atlas-source`) and its committed output
//! (`kind: atlas`) — `Plan/sprite-engine.md`, "Data and bake".
//!
//! An **atlas source** lists the inputs one packed image is built from (sprite documents and loose
//! PNGs, as paths relative to the source file) plus the flipbooks the atlas carries, and is
//! hand-authored. An **atlas** is *derived*: `ludion generate-atlas` writes the `.png` and this
//! document beside it, and both are committed so nothing packs at runtime. A derived document is
//! write-only for authors — hand-edit the source and re-run the bake; the `atlas-drift` gate
//! byte-compares the committed result.
//!
//! The two live in one module because they share a vocabulary — `pixels_per_unit`, `filter`,
//! [`Flipbook`] — and a diagnostic code, and because keeping the derived shape next to the shape it
//! derives from is what stops the two drifting.
//!
//! Both mirror the crate's usual split: [`parse_source`]/[`parse`] check structure and shape,
//! [`validate_source`]/[`validate`] check semantics, and [`emit_source`]/[`emit`] write canonical
//! text — all under `L1153`. What this module deliberately does **not** check is anything needing
//! the packed image or the input files: whether an input path exists, whether two regions overlap,
//! whether an atlas-source flipbook's frames will resolve. Those are the bake's, because only the
//! bake has read the inputs.

use std::collections::BTreeMap;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::paths::sibling_refusal;
use ludion_core::suggest;
use serde_json::Value;

use crate::envelope;
use crate::fields::parse_number;
use crate::scalar::{format_float, scalar_string};
use crate::yaml::{self, MapEntry, Node, NodeValue, pointer_token};

/// The atlas-source document format version this crate reads and writes.
pub const ATLAS_SOURCE_FORMAT_VERSION: u64 = 1;

/// The atlas document format version this crate reads and writes.
pub const ATLAS_FORMAT_VERSION: u64 = 1;

const ATLAS_SOURCE_KEYS: [&str; 7] = [
    "format",
    "kind",
    "name",
    "pixels_per_unit",
    "filter",
    "inputs",
    "flipbooks",
];

const ATLAS_KEYS: [&str; 8] = [
    "format",
    "kind",
    "generated_from",
    "pixels_per_unit",
    "maps",
    "filter",
    "regions",
    "flipbooks",
];

const MAPS_KEYS: [&str; 1] = ["color"];
const FLIPBOOK_KEYS: [&str; 5] = ["name", "frames", "fps", "loop", "hold"];
const REGION_KEYS: [&str; 2] = ["rect", "pivot"];
const FILTERS: [&str; 2] = ["nearest", "linear"];

/// How a sampler reads an atlas's texels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AtlasFilter {
    /// Point sampling — the pixel-art default, and why it is the default at all: a linear filter on
    /// pixel art is the single most common way crisp source art arrives on screen blurred.
    #[default]
    Nearest,
    /// Bilinear sampling, for art authored at a density the screen will not match exactly.
    Linear,
}

impl AtlasFilter {
    /// The `filter:` keyword this variant is written as.
    fn keyword(self) -> &'static str {
        match self {
            AtlasFilter::Nearest => "nearest",
            AtlasFilter::Linear => "linear",
        }
    }
}

/// One named animation over an atlas's regions, carried verbatim from the source onto the derived
/// atlas so a game resolves a clip name against the same document it resolves a frame name against.
#[derive(Clone, Debug, PartialEq)]
pub struct Flipbook {
    /// The clip's name, unique within the document.
    pub name: String,
    /// The regions to show, in order. On an [`AtlasDocument`] every entry names one of its
    /// `regions`; on an [`AtlasSourceDocument`] the names cannot be checked here, because that file
    /// has not read the sprite documents its inputs point at — the bake checks them.
    pub frames: Vec<String>,
    /// Frames per second, finite and greater than zero.
    pub fps: f64,
    /// Whether the clip repeats. A one-shot holds its last frame. The YAML key is `loop`, which is
    /// a Rust keyword — hence the field name.
    pub looped: bool,
    /// How many clip frames each entry of [`Self::frames`] holds for, same length as `frames` and
    /// every entry at least 1. `None` means one frame each. Integer counts by construction, so tick
    /// quantization is exact.
    pub hold: Option<Vec<u32>>,
}

/// The default [`Flipbook::looped`], omitted by the emitters when it is the authored value.
const FLIPBOOK_LOOPED_DEFAULT: bool = true;

/// A parsed atlas-source document: what `ludion generate-atlas` reads.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct AtlasSourceDocument {
    /// The output stem for the generated `.png` and `.atlas.yaml`. `None` means the CLI derives it
    /// from this file's own name — this crate does no IO and does not know that name, so the
    /// fallback cannot be resolved here.
    pub name: Option<String>,
    /// How many texels of this atlas's art span one world unit. Art is authored at a density, the
    /// way a voxel model declares `scale:`.
    pub pixels_per_unit: f64,
    /// How a sampler reads the packed image.
    pub filter: AtlasFilter,
    /// The inputs to pack, as paths relative to this file's directory: a `kind: sprite` document
    /// contributes one region per frame, a loose PNG contributes one region named by its file stem.
    pub inputs: Vec<String>,
    /// The flipbooks to carry onto the generated atlas, in authored order.
    pub flipbooks: Vec<Flipbook>,
}

/// One packed region of an atlas: where it sits in the image, and what it rotates about.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AtlasRegion {
    /// `[x, y, w, h]` in texels, top-left origin, `w` and `h` at least 1.
    pub rect: [u32; 4],
    /// The rotation/anchor point in texels from the region's **own** top-left, so a region carries
    /// its pivot wherever the packer moves it.
    pub pivot: [f64; 2],
}

/// A parsed atlas document: derived, committed, and write-only for authors.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct AtlasDocument {
    /// The `kind: atlas-source` document this was generated from, relative to this file. The drift
    /// gate's provenance marker: it is both the contract and how the gate finds the pair.
    pub generated_from: String,
    /// How many texels of this atlas's art span one world unit.
    pub pixels_per_unit: f64,
    /// The packed colour image, relative to this file, premultiplied alpha. The YAML shape is
    /// `maps: { color: … }` so an emissive or normal map would be an additive key rather than a
    /// second document.
    pub color_map: String,
    /// How a sampler reads the packed image.
    pub filter: AtlasFilter,
    /// Every region by name. A `BTreeMap` because name order *is* the canonical order — a packer
    /// change must not reshuffle the file.
    pub regions: BTreeMap<String, AtlasRegion>,
    /// The flipbooks carried from the source, in authored order.
    pub flipbooks: Vec<Flipbook>,
}

impl AtlasDocument {
    /// The region named `name`, or `None`.
    ///
    /// A `None` here is what a `Sprite`'s `frame` resolving against this atlas reports as
    /// [`code::SPRITE_FRAME_UNKNOWN`], with [`Self::nearest_region_name`] supplying the suggestion.
    pub fn region(&self, name: &str) -> Option<&AtlasRegion> {
        self.regions.get(name)
    }

    /// The region name closest to `name` by edit distance, when one is close enough to plausibly be
    /// the intended one — the did-you-mean half of [`code::SPRITE_FRAME_UNKNOWN`].
    ///
    /// # Example
    /// ```
    /// let source = "format: 1\nkind: atlas\ngenerated_from: ship.atlas-source.yaml\n\
    ///               pixels_per_unit: 16.0\nmaps:\n  color: ship.png\nregions:\n  ship/idle:\n    \
    ///               rect:\n      - 0\n      - 0\n      - 2\n      - 2\n    pivot:\n      - 1.0\n\
    ///               \x20     - 1.0\n";
    /// let atlas = ludion_scene::check_atlas(source).expect("the atlas is valid");
    /// assert_eq!(atlas.nearest_region_name("ship/idel"), Some("ship/idle"));
    /// assert_eq!(atlas.nearest_region_name("explosion"), None);
    /// ```
    pub fn nearest_region_name(&self, name: &str) -> Option<&str> {
        suggest::nearest(name, self.regions.keys().map(String::as_str))
    }

    /// The flipbook named `name`, or `None` — what an `Animation`'s clip name resolves against for a
    /// sprite drawn from this atlas.
    ///
    /// A linear scan over a handful of clips, in authored order; names are unique by
    /// [`validate`](crate::check_atlas), so the first match is the only one.
    pub fn flipbook(&self, name: &str) -> Option<&Flipbook> {
        self.flipbooks.iter().find(|clip| clip.name == name)
    }
}

// --- Atlas source: parse ------------------------------------------------------------------------

/// Parses strict-subset YAML into an [`AtlasSourceDocument`], checking structure/shape only.
pub(crate) fn parse_source(source: &str) -> Result<AtlasSourceDocument, Vec<Diagnostic>> {
    let entries = root_entries(
        source,
        "atlas-source",
        "an atlas source needs at least `format: 1`, `kind: atlas-source`, `pixels_per_unit`, \
         and `inputs`",
        "a minimal atlas source is:\n  format: 1\n  kind: atlas-source\n  pixels_per_unit: 16.0\n  \
         inputs:\n    - sprites/ship.sprite.yaml",
    )?;

    let mut diagnostics = Vec::new();
    let mut doc = AtlasSourceDocument::default();
    let mut format_seen = false;
    let mut kind_seen = false;
    let mut density_seen = false;
    let mut inputs_seen = false;

    for entry in &entries {
        match entry.key.as_str() {
            "format" => {
                format_seen = true;
                envelope::check_envelope(
                    entry,
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    ATLAS_SOURCE_FORMAT_VERSION,
                    "the atlas source document",
                    "atlas source",
                    "atlas source",
                    &mut diagnostics,
                );
            }
            "kind" => {
                kind_seen = true;
                envelope::check_kind(
                    entry,
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    "atlas-source",
                    "an atlas source document",
                    &mut diagnostics,
                );
            }
            "name" => {
                doc.name = envelope::expect_string(
                    &entry.node,
                    "/name",
                    "the atlas name",
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "pixels_per_unit" => {
                density_seen = true;
                if let Some(value) = parse_number(
                    &entry.node,
                    "/pixels_per_unit",
                    "pixels_per_unit",
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                ) {
                    doc.pixels_per_unit = value;
                }
            }
            "filter" => {
                if let Some(filter) = parse_filter(&entry.node, &mut diagnostics) {
                    doc.filter = filter;
                }
            }
            "inputs" => {
                inputs_seen = true;
                doc.inputs = parse_string_list(&entry.node, "/inputs", "inputs", &mut diagnostics);
            }
            "flipbooks" => {
                doc.flipbooks = parse_flipbooks(entry, &mut diagnostics);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "",
                "an atlas source document",
                ATLAS_SOURCE_KEYS,
                code::SPRITE_DOCUMENT_STRUCTURE,
            )),
        }
    }

    if !format_seen {
        diagnostics.push(missing_key(
            "format",
            "add `format: 1` as the first line",
            "atlas source document",
        ));
    }
    if !kind_seen {
        diagnostics.push(missing_key(
            "kind",
            "add `kind: atlas-source`",
            "atlas source document",
        ));
    }
    if !density_seen {
        diagnostics.push(missing_key(
            "pixels_per_unit",
            "add `pixels_per_unit`: how many texels of this art span one world unit",
            "atlas source document",
        ));
    }
    if !inputs_seen {
        diagnostics.push(missing_key(
            "inputs",
            "add `inputs`: one sprite document or PNG path per line, relative to this file",
            "atlas source document",
        ));
    }

    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

// --- Atlas: parse -------------------------------------------------------------------------------

/// Parses strict-subset YAML into an [`AtlasDocument`], checking structure/shape only.
pub(crate) fn parse(source: &str) -> Result<AtlasDocument, Vec<Diagnostic>> {
    let entries = root_entries(
        source,
        "atlas",
        "an atlas needs at least `format: 1`, `kind: atlas`, `generated_from`, `pixels_per_unit`, \
         `maps`, and `regions`",
        "an atlas is generated, never hand-written — run `ludion generate-atlas` on its \
         `kind: atlas-source`",
    )?;

    let mut diagnostics = Vec::new();
    let mut doc = AtlasDocument::default();
    let mut format_seen = false;
    let mut kind_seen = false;
    let mut source_seen = false;
    let mut density_seen = false;
    let mut maps_seen = false;
    let mut regions_seen = false;

    for entry in &entries {
        match entry.key.as_str() {
            "format" => {
                format_seen = true;
                envelope::check_envelope(
                    entry,
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    ATLAS_FORMAT_VERSION,
                    "the atlas document",
                    "atlas",
                    "atlas",
                    &mut diagnostics,
                );
            }
            "kind" => {
                kind_seen = true;
                envelope::check_kind(
                    entry,
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    "atlas",
                    "an atlas document",
                    &mut diagnostics,
                );
            }
            "generated_from" => {
                source_seen = true;
                if let Some(value) = envelope::expect_string(
                    &entry.node,
                    "/generated_from",
                    "the atlas source path",
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                ) {
                    doc.generated_from = value;
                }
            }
            "pixels_per_unit" => {
                density_seen = true;
                if let Some(value) = parse_number(
                    &entry.node,
                    "/pixels_per_unit",
                    "pixels_per_unit",
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                ) {
                    doc.pixels_per_unit = value;
                }
            }
            "maps" => {
                maps_seen = true;
                if let Some(value) = parse_maps(entry, &mut diagnostics) {
                    doc.color_map = value;
                }
            }
            "filter" => {
                if let Some(filter) = parse_filter(&entry.node, &mut diagnostics) {
                    doc.filter = filter;
                }
            }
            "regions" => {
                regions_seen = true;
                doc.regions = parse_regions(entry, &mut diagnostics);
            }
            "flipbooks" => {
                doc.flipbooks = parse_flipbooks(entry, &mut diagnostics);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "",
                "an atlas document",
                ATLAS_KEYS,
                code::SPRITE_DOCUMENT_STRUCTURE,
            )),
        }
    }

    if !format_seen {
        diagnostics.push(missing_key(
            "format",
            "add `format: 1` as the first line",
            "atlas document",
        ));
    }
    if !kind_seen {
        diagnostics.push(missing_key("kind", "add `kind: atlas`", "atlas document"));
    }
    if !source_seen {
        diagnostics.push(missing_key(
            "generated_from",
            "add `generated_from`: the `kind: atlas-source` this was baked from, relative to this \
             file — it is the drift gate's provenance marker",
            "atlas document",
        ));
    }
    if !density_seen {
        diagnostics.push(missing_key(
            "pixels_per_unit",
            "add `pixels_per_unit`: how many texels of this art span one world unit",
            "atlas document",
        ));
    }
    if !maps_seen {
        diagnostics.push(missing_key(
            "maps",
            "add `maps` with a `color`: the packed image, relative to this file",
            "atlas document",
        ));
    }
    if !regions_seen {
        diagnostics.push(missing_key(
            "regions",
            "add `regions`: one named `rect`/`pivot` pair per packed sprite",
            "atlas document",
        ));
    }

    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

// --- Shared parse helpers -----------------------------------------------------------------------

/// Parses `source` down to its root mapping's entries, or the "empty"/"not a mapping" diagnostic.
fn root_entries(
    source: &str,
    kind: &str,
    empty_message: &str,
    empty_help: &str,
) -> Result<Vec<MapEntry>, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!("the {kind} document is empty; {empty_message}"),
            )
            .with_path("")
            .with_help(empty_help.to_string()),
        ]);
    };
    let NodeValue::Mapping(entries) = root.value else {
        return Err(vec![
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "the {kind} document root must be a mapping, found {} (line {})",
                    root.value.type_phrase(),
                    root.line
                ),
            )
            .with_path("")
            .with_help(format!(
                "start the file with the top-level keys `format:` and `kind: {kind}`"
            )),
        ]);
    };
    Ok(entries)
}

fn missing_key(key: &str, help: &str, doc_noun: &str) -> Diagnostic {
    envelope::missing_key(key, help, code::SPRITE_DOCUMENT_STRUCTURE, doc_noun)
}

/// Parses `filter`: one of [`FILTERS`], with a nearest-match suggestion on a miss.
fn parse_filter(node: &Node, diagnostics: &mut Vec<Diagnostic>) -> Option<AtlasFilter> {
    let NodeValue::Scalar(Value::String(text)) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "`filter` must be a string, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path("/filter")
            .with_help(format!("`filter` is one of: {}", FILTERS.join(", "))),
        );
        return None;
    };
    match text.as_str() {
        "nearest" => Some(AtlasFilter::Nearest),
        "linear" => Some(AtlasFilter::Linear),
        other => {
            let mut diagnostic = Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!("`{other}` is not a sampler filter (line {})", node.line),
            )
            .with_path("/filter");
            diagnostic = match suggest::nearest(other, FILTERS) {
                Some(candidate) => diagnostic.with_help(format!("nearest match is `{candidate}`")),
                None => diagnostic.with_help(format!(
                    "`filter` is one of: {}; remove the key for `nearest`",
                    FILTERS.join(", ")
                )),
            };
            diagnostics.push(diagnostic);
            None
        }
    }
}

/// Parses a sequence of strings.
fn parse_string_list(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<String> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "`{what}` must be a sequence, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(format!("write `{what}` as one quoted entry per line")),
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
                        "`{what}[{index}]` must be a string, found {} (line {})",
                        other.type_phrase(),
                        item.line
                    ),
                )
                .with_path(format!("{pointer}/{index}"))
                .with_help("quote the entry so a numeric-looking name stays a string"),
            ),
        }
    }
    out
}

/// Parses `maps`: a mapping whose only recognized key is the required `color`.
fn parse_maps(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Option<String> {
    let NodeValue::Mapping(entries) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "`maps` must be a mapping, found {} (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/maps")
            .with_help("write `maps` with a `color`:\n  maps:\n    color: ship.png"),
        );
        return None;
    };
    let mut color = None;
    let mut color_seen = false;
    for sub in entries {
        match sub.key.as_str() {
            "color" => {
                color_seen = true;
                color = envelope::expect_string(
                    &sub.node,
                    "/maps/color",
                    "the packed image path",
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    diagnostics,
                );
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                sub,
                "/maps",
                "an atlas's `maps`",
                MAPS_KEYS,
                code::SPRITE_DOCUMENT_STRUCTURE,
            )),
        }
    }
    if !color_seen {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                "`maps` is missing its required `color`",
            )
            .with_path("/maps")
            .with_help("add `color`: the packed image, relative to this file"),
        );
    }
    color
}

/// Parses `regions`: a mapping of region name to `{rect, pivot}`.
fn parse_regions(
    entry: &MapEntry,
    diagnostics: &mut Vec<Diagnostic>,
) -> BTreeMap<String, AtlasRegion> {
    let NodeValue::Mapping(entries) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "`regions` must be a mapping of region names to rects, found {} (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/regions")
            .with_help(
                "each region names one packed rectangle:\n  regions:\n    ship/idle:\n      \
                 rect:\n        - 0\n        - 0\n        - 16\n        - 12\n      pivot:\n     \
                 \x20  - 8.0\n        - 6.0",
            ),
        );
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for sub in entries {
        let pointer = format!("/regions/{}", pointer_token(&sub.key));
        if let Some(region) = parse_region(&sub.node, &pointer, diagnostics) {
            out.insert(sub.key.clone(), region);
        }
    }
    out
}

/// Parses one region: a mapping with a required `rect` and a required `pivot`.
fn parse_region(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<AtlasRegion> {
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "a region must be a mapping with `rect` and `pivot`, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("give the region a `rect` of four integers and a `pivot` of two numbers"),
        );
        return None;
    };
    let mut rect = None;
    let mut pivot = None;
    for sub in entries {
        match sub.key.as_str() {
            "rect" => {
                rect = parse_rect(&sub.node, &format!("{pointer}/rect"), diagnostics);
            }
            "pivot" => {
                pivot = parse_pivot(&sub.node, &format!("{pointer}/pivot"), diagnostics);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                sub,
                pointer,
                "a region",
                REGION_KEYS,
                code::SPRITE_DOCUMENT_STRUCTURE,
            )),
        }
    }
    if rect.is_none() {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                "a region is missing its required `rect`",
            )
            .with_path(pointer.to_string())
            .with_help("add `rect`: four integers `[x, y, w, h]` in texels, top-left origin"),
        );
    }
    if pivot.is_none() {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                "a region is missing its required `pivot`",
            )
            .with_path(pointer.to_string())
            .with_help("add `pivot`: two numbers in texels from the region's own top-left"),
        );
    }
    Some(AtlasRegion {
        rect: rect?,
        pivot: pivot?,
    })
}

/// Parses a region's `rect`: four non-negative integers that fit in `u32`.
fn parse_rect(node: &Node, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<[u32; 4]> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "`rect` must be a sequence of four integers, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("write `rect` as four integers: x, y, width, height"),
        );
        return None;
    };
    if items.len() != 4 {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "`rect` must have exactly 4 entries (x, y, w, h), found {}",
                    items.len()
                ),
            )
            .with_path(pointer.to_string())
            .with_help("supply exactly four integers"),
        );
        return None;
    }
    let mut out = [0u32; 4];
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        let value = match &item.value {
            NodeValue::Scalar(Value::Number(n)) => n.as_u64().filter(|v| *v <= u64::from(u32::MAX)),
            _ => None,
        };
        match value {
            Some(v) => out[index] = v as u32,
            None => {
                diagnostics.push(
                    Diagnostic::new(
                        code::SPRITE_DOCUMENT_STRUCTURE,
                        format!(
                            "`rect[{index}]` must be a non-negative integer, found {} (line {})",
                            item.value.type_phrase(),
                            item.line
                        ),
                    )
                    .with_path(format!("{pointer}/{index}"))
                    .with_help("a packed rect is measured in whole texels"),
                );
                ok = false;
            }
        }
    }
    ok.then_some(out)
}

/// Parses a region's `pivot`: two numbers.
fn parse_pivot(node: &Node, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<[f64; 2]> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "`pivot` must be a sequence of two numbers, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("write `pivot` as two numbers (px, py)"),
        );
        return None;
    };
    if items.len() != 2 {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "`pivot` must have exactly 2 entries (px, py), found {}",
                    items.len()
                ),
            )
            .with_path(pointer.to_string())
            .with_help("supply exactly two numbers"),
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
                        code::SPRITE_DOCUMENT_STRUCTURE,
                        format!(
                            "`pivot[{index}]` must be a number, found {} (line {})",
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

/// Parses `flipbooks`: a sequence of `{name, frames, fps, loop?, hold?}` entries, in authored order.
fn parse_flipbooks(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Vec<Flipbook> {
    let NodeValue::Sequence(items) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "`flipbooks` must be a sequence of clip entries, found {} (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/flipbooks")
            .with_help(
                "write one entry per clip:\n  flipbooks:\n    - name: ship/fly\n      frames:\n  \
                 \x20     - ship/idle\n      fps: 8.0",
            ),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let pointer = format!("/flipbooks/{index}");
        if let Some(flipbook) = parse_flipbook(item, &pointer, diagnostics) {
            out.push(flipbook);
        }
    }
    out
}

/// Parses one flipbook entry.
fn parse_flipbook(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Flipbook> {
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "a flipbook must be a mapping with `name`, `frames`, and `fps`, found {} \
                     (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(
                "write the entry as keys under one `-`:\n    - name: ship/fly\n      frames:\n  \
                 \x20     - ship/idle\n      fps: 8.0",
            ),
        );
        return None;
    };
    let mut name = None;
    let mut frames = None;
    let mut fps = None;
    let mut looped = FLIPBOOK_LOOPED_DEFAULT;
    let mut hold = None;
    for sub in entries {
        match sub.key.as_str() {
            "name" => {
                name = envelope::expect_string(
                    &sub.node,
                    &format!("{pointer}/name"),
                    "a flipbook's `name`",
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    diagnostics,
                );
            }
            "frames" => {
                frames = Some(parse_string_list(
                    &sub.node,
                    &format!("{pointer}/frames"),
                    "frames",
                    diagnostics,
                ));
            }
            "fps" => {
                fps = parse_number(
                    &sub.node,
                    &format!("{pointer}/fps"),
                    "fps",
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    diagnostics,
                );
            }
            "loop" => match &sub.node.value {
                NodeValue::Scalar(Value::Bool(value)) => looped = *value,
                other => diagnostics.push(
                    Diagnostic::new(
                        code::SPRITE_DOCUMENT_STRUCTURE,
                        format!(
                            "`loop` must be a boolean, found {} (line {})",
                            other.type_phrase(),
                            sub.node.line
                        ),
                    )
                    .with_path(format!("{pointer}/loop"))
                    .with_help(
                        "write `loop: false` for a one-shot that holds its last frame; remove the \
                         key to loop",
                    ),
                ),
            },
            "hold" => {
                hold = parse_hold(&sub.node, &format!("{pointer}/hold"), diagnostics);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                sub,
                pointer,
                "a flipbook",
                FLIPBOOK_KEYS,
                code::SPRITE_DOCUMENT_STRUCTURE,
            )),
        }
    }
    let required = [
        (
            name.is_none(),
            "name",
            "the clip's name, unique in this document",
        ),
        (
            frames.is_none(),
            "frames",
            "the region names to show, in order",
        ),
        (fps.is_none(), "fps", "how many clip frames pass per second"),
    ];
    for (_, key, help) in required.into_iter().filter(|(absent, _, _)| *absent) {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!("a flipbook is missing its required `{key}`"),
            )
            .with_path(pointer.to_string())
            .with_help(format!("add `{key}`: {help}")),
        );
    }
    Some(Flipbook {
        name: name?,
        frames: frames?,
        fps: fps?,
        looped,
        hold,
    })
}

/// Parses a flipbook's `hold`: a sequence of non-negative integers. The `>= 1` rule and the
/// same-length-as-`frames` rule are semantic, checked by the validators.
fn parse_hold(node: &Node, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<Vec<u32>> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "`hold` must be a sequence of integers, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(
                "write one whole frame count per entry of `frames`; remove the key to hold each \
                 frame once",
            ),
        );
        return None;
    };
    let mut out = Vec::with_capacity(items.len());
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        let value = match &item.value {
            NodeValue::Scalar(Value::Number(n)) => n.as_u64().filter(|v| *v <= u64::from(u32::MAX)),
            _ => None,
        };
        match value {
            Some(v) => out.push(v as u32),
            None => {
                diagnostics.push(
                    Diagnostic::new(
                        code::SPRITE_DOCUMENT_STRUCTURE,
                        format!(
                            "`hold[{index}]` must be a non-negative integer, found {} (line {})",
                            item.value.type_phrase(),
                            item.line
                        ),
                    )
                    .with_path(format!("{pointer}/{index}"))
                    .with_help("a hold is a whole number of clip frames"),
                );
                ok = false;
            }
        }
    }
    ok.then_some(out)
}

// --- Validate -----------------------------------------------------------------------------------

/// Validates semantic invariants of an already-parsed [`AtlasSourceDocument`] (`L1153`):
/// `pixels_per_unit` finite and positive, a `name` that is a plain file stem, a non-empty `inputs` list of non-empty paths, and
/// well-shaped flipbooks.
///
/// Flipbook frame *names* are deliberately not resolved here: an atlas source has not read the
/// sprite documents its inputs name, so it cannot know which frames exist. That check belongs to the
/// bake, which has.
pub(crate) fn validate_source(doc: &AtlasSourceDocument) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    validate_density(doc.pixels_per_unit, &mut diagnostics);
    if let Some(name) = &doc.name
        && let Some(refusal) = ludion_core::paths::stem_refusal(name)
    {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!("`name` {}", refusal.describe()),
            )
            .with_path("/name")
            .with_help("write a plain file stem such as `ship`; it names the generated `.atlas.yaml` and `.png`"),
        );
    }
    if doc.inputs.is_empty() {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                "`inputs` is empty; an atlas source must name at least one sprite document or PNG",
            )
            .with_path("/inputs")
            .with_help("add one path per line, relative to this file's directory"),
        );
    }
    for (index, input) in doc.inputs.iter().enumerate() {
        if input.is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    format!("input {index} is an empty path"),
                )
                .with_path(format!("/inputs/{index}"))
                .with_help("name a `.sprite.yaml` document or a `.png`, relative to this file"),
            );
        }
    }
    validate_flipbooks(&doc.flipbooks, None, &mut diagnostics);
    diagnostics
}

/// Validates semantic invariants of an already-parsed [`AtlasDocument`] (`L1153`): a non-empty
/// `generated_from` and colour map, `pixels_per_unit` finite and positive, a non-empty `regions`
/// map whose names are non-empty and whose rects and pivots are usable, and flipbooks whose frames
/// name regions this atlas actually has.
///
/// Region *overlap* is deliberately not checked: the packer decides the layout, and a deliberate
/// alias of one rect under two names is legal content. `maps.color` may climb with `..` but is
/// refused when absolute, drive-prefixed or a Windows device.
pub(crate) fn validate(doc: &AtlasDocument) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if doc.generated_from.is_empty() {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                "`generated_from` is empty; it must name the atlas source this was baked from",
            )
            .with_path("/generated_from")
            .with_help(
                "the drift gate finds an atlas's source through this key, so an atlas without one \
                 is unregeneratable; write the path relative to this file",
            ),
        );
    }
    if doc.color_map.is_empty() {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                "`maps.color` is empty; it must name the packed image",
            )
            .with_path("/maps/color")
            .with_help("write the `.png` path relative to this file"),
        );
    } else if let Some(refusal) = sibling_refusal(&doc.color_map) {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!("`maps.color` (`{}`) {}", doc.color_map, refusal.describe()),
            )
            .with_path("/maps/color")
            .with_help("write the `.png` path relative to this file"),
        );
    }
    validate_density(doc.pixels_per_unit, &mut diagnostics);

    if doc.regions.is_empty() {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                "`regions` is empty; an atlas with no regions can never be sampled",
            )
            .with_path("/regions")
            .with_help("re-run `ludion generate-atlas` on a source whose `inputs` produce frames"),
        );
    }
    for (name, region) in &doc.regions {
        let pointer = format!("/regions/{}", pointer_token(name));
        if name.is_empty() {
            diagnostics.push(
                Diagnostic::new(code::SPRITE_DOCUMENT_STRUCTURE, "a region name is empty")
                    .with_path(pointer.clone())
                    .with_help(
                        "a region is named `<sprite document>/<frame>` or after a PNG's file stem",
                    ),
            );
        }
        let [_, _, w, h] = region.rect;
        for (index, (extent, axis)) in [(w, "width"), (h, "height")].into_iter().enumerate() {
            if extent < 1 {
                diagnostics.push(
                    Diagnostic::new(
                        code::SPRITE_DOCUMENT_STRUCTURE,
                        format!("region `{name}` has a {axis} of {extent} texels"),
                    )
                    .with_path(format!("{pointer}/rect/{}", index + 2))
                    .with_help("a region covers at least one texel on each axis"),
                );
            }
        }
        let bounds = [f64::from(w), f64::from(h)];
        for (index, (value, limit)) in region.pivot.into_iter().zip(bounds).enumerate() {
            if !(value.is_finite() && (0.0..=limit).contains(&value)) {
                diagnostics.push(
                    Diagnostic::new(
                        code::SPRITE_DOCUMENT_STRUCTURE,
                        format!(
                            "region `{name}` has a pivot component {value} at index {index}, \
                             outside the region's 0..={limit} texels"
                        ),
                    )
                    .with_path(format!("{pointer}/pivot/{index}"))
                    .with_help(
                        "a pivot is measured in texels from the region's own top-left, so it moves \
                         with the region and must lie on or inside it",
                    ),
                );
            }
        }
    }

    validate_flipbooks(&doc.flipbooks, Some(&doc.regions), &mut diagnostics);
    diagnostics
}

fn validate_density(pixels_per_unit: f64, diagnostics: &mut Vec<Diagnostic>) {
    if !(pixels_per_unit.is_finite() && pixels_per_unit > 0.0) {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                format!(
                    "`pixels_per_unit` is {pixels_per_unit}; it must be finite and greater than \
                     zero"
                ),
            )
            .with_path("/pixels_per_unit")
            .with_help(
                "`pixels_per_unit` converts a region's texel size into world units, so a zero or \
                 negative density would collapse every sprite drawn from this atlas",
            ),
        );
    }
}

/// The shape rules a flipbook obeys in both families. `regions` is `Some` only for an
/// [`AtlasDocument`], the one family that knows which names exist.
fn validate_flipbooks(
    flipbooks: &[Flipbook],
    regions: Option<&BTreeMap<String, AtlasRegion>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut seen: Vec<&str> = Vec::new();
    for (index, flipbook) in flipbooks.iter().enumerate() {
        let pointer = format!("/flipbooks/{index}");
        if flipbook.name.is_empty() {
            diagnostics.push(
                Diagnostic::new(code::SPRITE_DOCUMENT_STRUCTURE, "a flipbook name is empty")
                    .with_path(format!("{pointer}/name"))
                    .with_help("a clip is selected by name, so it must have one"),
            );
        }
        if seen.contains(&flipbook.name.as_str()) {
            diagnostics.push(
                Diagnostic::new(
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    format!(
                        "the flipbook name `{}` is used more than once",
                        flipbook.name
                    ),
                )
                .with_path(format!("{pointer}/name"))
                .with_help("each clip resolves by name, so a shared one hides the later clip"),
            );
        } else {
            seen.push(&flipbook.name);
        }

        if flipbook.frames.is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    format!("flipbook `{}` has no frames", flipbook.name),
                )
                .with_path(format!("{pointer}/frames"))
                .with_help("list at least one region name"),
            );
        }
        if let Some(regions) = regions {
            for (slot, frame) in flipbook.frames.iter().enumerate() {
                if !regions.contains_key(frame) {
                    let mut diagnostic = Diagnostic::new(
                        code::SPRITE_DOCUMENT_STRUCTURE,
                        format!("`{frame}` names no region of this atlas"),
                    )
                    .with_path(format!("{pointer}/frames/{slot}"));
                    let names = regions.keys().map(String::as_str);
                    diagnostic = match suggest::nearest(frame, names) {
                        Some(candidate) => {
                            diagnostic.with_help(format!("nearest match is `{candidate}`"))
                        }
                        None => diagnostic.with_help(
                            "regenerate the atlas so it carries the region, or correct the name — \
                             `regions:` lists every legal value",
                        ),
                    };
                    diagnostics.push(diagnostic);
                }
            }
        }

        if !(flipbook.fps.is_finite() && flipbook.fps > 0.0) {
            diagnostics.push(
                Diagnostic::new(
                    code::SPRITE_DOCUMENT_STRUCTURE,
                    format!(
                        "flipbook `{}` has an `fps` of {}; it must be finite and greater than zero",
                        flipbook.name, flipbook.fps
                    ),
                )
                .with_path(format!("{pointer}/fps"))
                .with_help("a clip at zero frames per second never advances"),
            );
        }

        if let Some(hold) = &flipbook.hold {
            if hold.len() != flipbook.frames.len() {
                diagnostics.push(
                    Diagnostic::new(
                        code::SPRITE_DOCUMENT_STRUCTURE,
                        format!(
                            "flipbook `{}` has {} hold entries but {} frames",
                            flipbook.name,
                            hold.len(),
                            flipbook.frames.len()
                        ),
                    )
                    .with_path(format!("{pointer}/hold"))
                    .with_help(
                        "`hold` is one count per frame, in the same order; remove it to hold every \
                         frame once",
                    ),
                );
            }
            for (slot, &count) in hold.iter().enumerate() {
                if count < 1 {
                    diagnostics.push(
                        Diagnostic::new(
                            code::SPRITE_DOCUMENT_STRUCTURE,
                            format!("flipbook `{}` holds a frame for 0 frames", flipbook.name),
                        )
                        .with_path(format!("{pointer}/hold/{slot}"))
                        .with_help(
                            "a hold of 0 would show the frame for no time at all; drop the frame \
                             from `frames` instead",
                        ),
                    );
                }
            }
        }
    }
}

// --- Emit ---------------------------------------------------------------------------------------

/// Writes `doc` in canonical form: `format`, `kind`, `name` (optional), `pixels_per_unit`, `filter`
/// (only when it is not the default), `inputs`, `flipbooks` (omitted when empty).
pub(crate) fn emit_source(doc: &AtlasSourceDocument) -> String {
    let mut out = String::new();
    out.push_str("format: 1\n");
    out.push_str("kind: atlas-source\n");
    if let Some(name) = &doc.name {
        out.push_str("name: ");
        out.push_str(&scalar_string(name));
        out.push('\n');
    }
    out.push_str("pixels_per_unit: ");
    out.push_str(&format_float(doc.pixels_per_unit));
    out.push('\n');
    emit_filter(&mut out, doc.filter);

    if doc.inputs.is_empty() {
        out.push_str("inputs: []\n");
    } else {
        out.push_str("inputs:\n");
        for input in &doc.inputs {
            out.push_str("  - ");
            out.push_str(&scalar_string(input));
            out.push('\n');
        }
    }

    emit_flipbooks(&mut out, &doc.flipbooks);
    out
}

/// Writes `doc` in canonical form: `format`, `kind`, `generated_from`, `pixels_per_unit`, `maps`,
/// `filter` (only when it is not the default), `regions` (in name order), `flipbooks` (omitted when
/// empty).
pub(crate) fn emit(doc: &AtlasDocument) -> String {
    let mut out = String::new();
    out.push_str("format: 1\n");
    out.push_str("kind: atlas\n");
    out.push_str("generated_from: ");
    out.push_str(&scalar_string(&doc.generated_from));
    out.push('\n');
    out.push_str("pixels_per_unit: ");
    out.push_str(&format_float(doc.pixels_per_unit));
    out.push('\n');
    out.push_str("maps:\n  color: ");
    out.push_str(&scalar_string(&doc.color_map));
    out.push('\n');
    emit_filter(&mut out, doc.filter);

    if doc.regions.is_empty() {
        out.push_str("regions: {}\n");
    } else {
        out.push_str("regions:\n");
        for (name, region) in &doc.regions {
            out.push_str("  ");
            out.push_str(&scalar_string(name));
            out.push_str(":\n    rect:\n");
            for component in region.rect {
                out.push_str("      - ");
                out.push_str(&component.to_string());
                out.push('\n');
            }
            out.push_str("    pivot:\n");
            for component in region.pivot {
                out.push_str("      - ");
                out.push_str(&format_float(component));
                out.push('\n');
            }
        }
    }

    emit_flipbooks(&mut out, &doc.flipbooks);
    out
}

/// Writes `filter:` only when it is not the default — canonical means minimal, in both families.
fn emit_filter(out: &mut String, filter: AtlasFilter) {
    if filter != AtlasFilter::default() {
        out.push_str("filter: ");
        out.push_str(filter.keyword());
        out.push('\n');
    }
}

fn emit_flipbooks(out: &mut String, flipbooks: &[Flipbook]) {
    if flipbooks.is_empty() {
        return;
    }
    out.push_str("flipbooks:\n");
    for flipbook in flipbooks {
        out.push_str("  - name: ");
        out.push_str(&scalar_string(&flipbook.name));
        out.push('\n');
        if flipbook.frames.is_empty() {
            out.push_str("    frames: []\n");
        } else {
            out.push_str("    frames:\n");
            for frame in &flipbook.frames {
                out.push_str("      - ");
                out.push_str(&scalar_string(frame));
                out.push('\n');
            }
        }
        out.push_str("    fps: ");
        out.push_str(&format_float(flipbook.fps));
        out.push('\n');
        if flipbook.looped != FLIPBOOK_LOOPED_DEFAULT {
            out.push_str("    loop: false\n");
        }
        if let Some(hold) = &flipbook.hold {
            if hold.is_empty() {
                out.push_str("    hold: []\n");
            } else {
                out.push_str("    hold:\n");
                for count in hold {
                    out.push_str("      - ");
                    out.push_str(&count.to_string());
                    out.push('\n');
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD_SOURCE: &str = "\
format: 1
kind: atlas-source
name: ship
pixels_per_unit: 16.0
filter: linear
inputs:
  - sprites/ship.sprite.yaml
  - fx/explosion.png
flipbooks:
  - name: ship/fly
    frames:
      - ship/idle
      - ship/bank-left
    fps: 8.0
    loop: false
    hold:
      - 1
      - 2
";

    const GOOD_ATLAS: &str = "\
format: 1
kind: atlas
generated_from: ship.atlas-source.yaml
pixels_per_unit: 16.0
maps:
  color: ship.png
regions:
  explosion:
    rect:
      - 32
      - 0
      - 8
      - 8
    pivot:
      - 4.0
      - 4.0
  ship/bank-left:
    rect:
      - 16
      - 0
      - 16
      - 12
    pivot:
      - 8.0
      - 6.0
  ship/idle:
    rect:
      - 0
      - 0
      - 16
      - 12
    pivot:
      - 8.0
      - 6.0
";

    fn check_source(source: &str) -> Result<AtlasSourceDocument, Vec<Diagnostic>> {
        let doc = parse_source(source)?;
        let diagnostics = validate_source(&doc);
        if diagnostics.is_empty() {
            Ok(doc)
        } else {
            Err(diagnostics)
        }
    }

    fn check_atlas(source: &str) -> Result<AtlasDocument, Vec<Diagnostic>> {
        let doc = parse(source)?;
        let diagnostics = validate(&doc);
        if diagnostics.is_empty() {
            Ok(doc)
        } else {
            Err(diagnostics)
        }
    }

    fn source_errors(source: &str) -> Vec<Diagnostic> {
        match check_source(source) {
            Ok(_) => panic!("expected the atlas source to be rejected"),
            Err(diagnostics) => diagnostics,
        }
    }

    #[test]
    fn a_source_name_must_be_a_plain_stem() {
        for bad in ["a/b", "'a\\b'", "../x", "\"\"", "."] {
            let errors =
                source_errors(&GOOD_SOURCE.replace("name: ship\n", &format!("name: {bad}\n")));
            assert!(
                errors.iter().any(|d| d.path.as_deref() == Some("/name")),
                "{bad}: {errors:?}"
            );
        }
    }

    fn atlas_errors(source: &str) -> Vec<Diagnostic> {
        match check_atlas(source) {
            Ok(_) => panic!("expected the atlas to be rejected"),
            Err(diagnostics) => diagnostics,
        }
    }

    fn find<'a>(diagnostics: &'a [Diagnostic], path: &str) -> &'a Diagnostic {
        diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some(path))
            .unwrap_or_else(|| panic!("no diagnostic at {path}; got {diagnostics:#?}"))
    }

    #[test]
    fn the_good_source_round_trips_byte_identically() {
        let doc = check_source(GOOD_SOURCE).expect("the atlas source is valid");
        assert_eq!(emit_source(&doc), GOOD_SOURCE);
    }

    #[test]
    fn the_good_atlas_round_trips_byte_identically() {
        let doc = check_atlas(GOOD_ATLAS).expect("the atlas is valid");
        assert_eq!(emit(&doc), GOOD_ATLAS);
    }

    #[test]
    fn a_default_filter_is_not_written() {
        let doc = check_source(&GOOD_SOURCE.replace("filter: linear\n", ""))
            .expect("the atlas source is valid");
        assert_eq!(doc.filter, AtlasFilter::Nearest);
        assert!(!emit_source(&doc).contains("filter:"));
        // Spelling the default out loud is legal input that re-emits shorter.
        let spelled = check_source(&GOOD_SOURCE.replace("filter: linear", "filter: nearest"))
            .expect("the atlas source is valid");
        assert!(!emit_source(&spelled).contains("filter:"));
    }

    #[test]
    fn an_unknown_top_level_key_suggests_the_nearest() {
        let diagnostics = source_errors(&GOOD_SOURCE.replace("inputs:", "input:\n  - a\ninputs:"));
        let diagnostic = find(&diagnostics, "/input");
        assert_eq!(diagnostic.code.as_str(), "L1153");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("nearest match is `inputs`")
        );
    }

    #[test]
    fn an_unknown_filter_suggests_the_nearest() {
        let diagnostics = source_errors(&GOOD_SOURCE.replace("filter: linear", "filter: linaer"));
        let diagnostic = find(&diagnostics, "/filter");
        assert_eq!(diagnostic.code.as_str(), "L1153");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("nearest match is `linear`")
        );
    }

    #[test]
    fn a_non_positive_density_is_rejected() {
        let diagnostics =
            source_errors(&GOOD_SOURCE.replace("pixels_per_unit: 16.0", "pixels_per_unit: 0.0"));
        assert_eq!(
            find(&diagnostics, "/pixels_per_unit").code.as_str(),
            "L1153"
        );
    }

    #[test]
    fn an_empty_inputs_list_is_rejected() {
        let source = "format: 1\nkind: atlas-source\npixels_per_unit: 16.0\ninputs: []\n";
        assert_eq!(
            find(&source_errors(source), "/inputs").code.as_str(),
            "L1153"
        );
    }

    #[test]
    fn a_hold_of_a_different_length_is_rejected() {
        let diagnostics =
            source_errors(&GOOD_SOURCE.replace("      - 1\n      - 2\n", "      - 1\n"));
        assert_eq!(
            find(&diagnostics, "/flipbooks/0/hold").code.as_str(),
            "L1153"
        );
    }

    #[test]
    fn a_zero_hold_is_rejected() {
        let diagnostics =
            source_errors(&GOOD_SOURCE.replace("      - 1\n      - 2\n", "      - 0\n      - 2\n"));
        assert_eq!(
            find(&diagnostics, "/flipbooks/0/hold/0").code.as_str(),
            "L1153"
        );
    }

    #[test]
    fn a_duplicate_flipbook_name_is_rejected() {
        let doubled = format!(
            "{GOOD_SOURCE}  - name: ship/fly\n    frames:\n      - ship/idle\n    fps: 4.0\n"
        );
        assert_eq!(
            find(&source_errors(&doubled), "/flipbooks/1/name")
                .code
                .as_str(),
            "L1153"
        );
    }

    #[test]
    fn an_atlas_source_does_not_resolve_its_flipbook_frames() {
        // The atlas source has not read the sprite documents its inputs name, so a frame name it
        // cannot resolve is the bake's finding, not this validator's.
        let doc = check_source(&GOOD_SOURCE.replace("ship/bank-left", "ship/nonsense"))
            .expect("an unresolvable frame name is legal in an atlas source");
        assert_eq!(doc.flipbooks[0].frames[1], "ship/nonsense");
    }

    #[test]
    fn an_atlas_resolves_its_flipbook_frames() {
        let with_clip = format!(
            "{GOOD_ATLAS}flipbooks:\n  - name: ship/fly\n    frames:\n      - ship/idel\n    \
             fps: 8.0\n"
        );
        let diagnostics = atlas_errors(&with_clip);
        let diagnostic = find(&diagnostics, "/flipbooks/0/frames/0");
        assert_eq!(diagnostic.code.as_str(), "L1153");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("nearest match is `ship/idle`")
        );
    }

    #[test]
    fn a_color_map_outside_the_content_root_is_rejected() {
        for bad in ["C:/x.png", "/x.png", "sub/nul.png"] {
            let broken = GOOD_ATLAS.replace(
                "  color: ship.png
",
                &format!(
                    "  color: {bad}
"
                ),
            );
            let diagnostics = atlas_errors(&broken);
            let d = find(&diagnostics, "/maps/color");
            assert_eq!(d.code.as_str(), "L1153", "{bad}");
        }
        let ok = GOOD_ATLAS.replace(
            "  color: ship.png
",
            "  color: ../ship.png
",
        );
        assert!(check_atlas(&ok).is_ok());
    }

    #[test]
    fn a_region_name_with_a_slash_is_pointer_escaped() {
        let broken = GOOD_ATLAS.replace(
            "  ship/idle:\n    rect:\n      - 0\n      - 0\n      - 16\n      - 12",
            "  ship/idle:\n    rect:\n      - 0\n      - 0\n      - 0\n      - 12",
        );
        let diagnostics = atlas_errors(&broken);
        let diagnostic = find(&diagnostics, "/regions/ship~1idle/rect/2");
        assert_eq!(diagnostic.code.as_str(), "L1153");
    }

    #[test]
    fn a_pivot_outside_its_region_is_rejected() {
        let broken = GOOD_ATLAS.replace(
            "    pivot:\n      - 4.0\n      - 4.0",
            "    pivot:\n      - 4.0\n      - 9.0",
        );
        assert_eq!(
            find(&atlas_errors(&broken), "/regions/explosion/pivot/1")
                .code
                .as_str(),
            "L1153"
        );
    }

    #[test]
    fn an_unknown_region_key_suggests_the_nearest() {
        let broken = GOOD_ATLAS.replace("  explosion:\n    rect:", "  explosion:\n    rects:");
        let diagnostics = atlas_errors(&broken);
        let diagnostic = find(&diagnostics, "/regions/explosion/rects");
        assert_eq!(diagnostic.help.as_deref(), Some("nearest match is `rect`"));
    }

    #[test]
    fn a_missing_generated_from_is_reported_once() {
        let broken = GOOD_ATLAS.replace("generated_from: ship.atlas-source.yaml\n", "");
        let diagnostics = atlas_errors(&broken);
        assert_eq!(find(&diagnostics, "").code.as_str(), "L1153");
        assert!(
            diagnostics
                .iter()
                .any(|d| d.message.contains("generated_from"))
        );
    }

    #[test]
    fn region_lookup_and_did_you_mean() {
        let doc = check_atlas(GOOD_ATLAS).expect("the atlas is valid");
        assert_eq!(
            doc.region("ship/idle").map(|r| r.rect),
            Some([0, 0, 16, 12])
        );
        assert!(doc.region("ship/idel").is_none());
        assert_eq!(doc.nearest_region_name("ship/idel"), Some("ship/idle"));
        assert_eq!(doc.nearest_region_name("something-else"), None);
    }

    #[test]
    fn the_wrong_kind_is_reported() {
        let diagnostics = atlas_errors(&GOOD_ATLAS.replace("kind: atlas", "kind: atlas-source"));
        assert_eq!(find(&diagnostics, "/kind").code.as_str(), "L1153");
    }
}
