//! Mesh-edit documents: `meshes/*.mesh-edit.yaml` — an ordered, replayable list of authored edits
//! to a mesh's texture maps.
//!
//! A mesh-edit document names a `kind: mesh` document as its `source`, a derived directory as its
//! `out`, and an `ops:` list read in file order, each op reading the result of the one before. The
//! source mesh and its committed maps are never rewritten: the derived tree is generated from
//! (source bytes, edit bytes, source map bytes) and is write-only
//! (`Plan/mesh-texture-editing.md`).
//!
//! This module owns mesh-edit document *structure* (shape/type checking, `L1173`) and *data
//! consistency* (finiteness, ranges, the `out` containment rule; `L1174`). It is pure text: no path
//! is resolved and no file is read here, so whether `source` exists, whether it carries `uvs`, and
//! whether a `size` names a map the source already has belong to the consumer that resolves those
//! paths (`L1176`).
//!
//! It reuses the crate's shared strict-YAML layer ([`crate::yaml`]) and the canonical-writer
//! primitives ([`crate::scalar`]), so a mesh-edit document is ryu-float, byte-idempotent canonical
//! text for the same reasons a mesh document is.

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};
use ludion_core::paths::{PathRefusal, relative_refusal, sibling_refusal};
use ludion_core::suggest;
use serde_json::Value;

use crate::envelope;
use crate::fields::parse_number;
use crate::scalar::{format_float, scalar_string};
use crate::yaml::{self, MapEntry, Node, NodeValue};

/// The mesh-edit document format version this crate reads and writes.
pub const MESH_EDIT_FORMAT_VERSION: u64 = 1;

const STRUCTURE: DiagnosticCode = code::MESH_EDIT_STRUCTURE;
const DATA: DiagnosticCode = code::MESH_EDIT_DATA;

const MESH_EDIT_KEYS: [&str; 6] = ["format", "kind", "name", "source", "out", "ops"];

const PAINT_KEYS: [&str; 6] = ["op", "map", "size", "color", "strength", "region"];

const OP_NAMES: [&str; 1] = ["paint"];

/// The five texture slots a mesh document carries, spelled as [`crate::MeshTextures`]' fields
/// rather than as the document's `*_texture` keys — a map is named here, not a key referenced.
const MAP_NAMES: [&str; 5] = [
    "base_color",
    "metallic_roughness",
    "normal",
    "occlusion",
    "emissive",
];

const REGION_NAMES: [&str; 4] = ["sphere", "capsule", "box", "fill"];

const SPHERE_KEYS: [&str; 3] = ["at", "radius", "feather"];
const CAPSULE_KEYS: [&str; 3] = ["points", "radius", "feather"];
const BOX_KEYS: [&str; 3] = ["at", "size", "feather"];
const FILL_KEYS: [&str; 6] = [
    "at",
    "tolerance",
    "max_radius",
    "map",
    "feather",
    "color_feather",
];

/// The keys a region mapping may carry: its shape, plus the optional `key` colour filter.
const REGION_ENTRY_NAMES: [&str; 5] = ["sphere", "capsule", "box", "fill", "key"];
const KEY_KEYS: [&str; 4] = ["map", "color", "tolerance", "color_feather"];

/// A parsed, structurally valid mesh-edit document.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MeshEditDocument {
    /// Optional human-readable name; the file stem when absent.
    pub name: Option<String>,
    /// The `kind: mesh` document this edit derives from, **relative to this document's own
    /// directory** — the rule a [`crate::TextureRef`] path follows. Never rewritten.
    pub source: String,
    /// The derived directory this edit owns, relative to this document's own directory. `None`
    /// means "the document's own stem" — see [`MeshEditDocument::out_dir`].
    pub out: Option<String>,
    /// The ops, in file order; each reads the result of the one before.
    pub ops: Vec<MeshEditOp>,
}

impl MeshEditDocument {
    /// The derived directory this document owns: `out` when authored, else `stem` — the caller's
    /// spelling of the document's own file stem, since this crate resolves no paths.
    ///
    /// # Example
    /// ```
    /// let source = "\
    /// format: 1
    /// kind: mesh-edit
    /// source: mesh-0.mesh.yaml
    /// ops: []
    /// ";
    /// let edit = ludion_scene::check_mesh_edit(source).expect("the edit is valid");
    /// assert_eq!(edit.out_dir("idol-lit"), "idol-lit");
    /// ```
    pub fn out_dir<'a>(&'a self, stem: &'a str) -> &'a str {
        self.out.as_deref().unwrap_or(stem)
    }
}

/// One op of a mesh-edit document. `paint` is the only verb: it writes a colour into one map,
/// weighted by a region.
#[derive(Clone, Debug, PartialEq)]
pub enum MeshEditOp {
    /// Paint `color` into `map` wherever `region` covers the surface.
    Paint {
        /// The texture slot this op writes.
        map: MeshEditMap,
        /// The square extent of a map the source does not have. `None` takes the source's
        /// base-colour dimensions; which of the two applies is the consumer's business, since only
        /// it can see the source.
        size: Option<u32>,
        /// The authored sRGB colour, each component in `[0, 1]`.
        color: [f64; 3],
        /// Scales the region's weight (never the colour), in `[0, 1]`. `None` is the full `1`.
        strength: Option<f64>,
        /// Where on the surface this op applies, in the source mesh's own coordinates.
        region: MeshEditRegion,
    },
}

impl MeshEditOp {
    /// The op's document spelling (`paint`).
    pub fn name(&self) -> &'static str {
        match self {
            MeshEditOp::Paint { .. } => "paint",
        }
    }
}

/// One of a mesh document's five texture slots, named as a [`crate::MeshTextures`] field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshEditMap {
    /// Albedo, sRGB.
    BaseColor,
    /// glTF's packing: `G` = roughness, `B` = metallic.
    MetallicRoughness,
    /// Tangent-space normals.
    Normal,
    /// Ambient occlusion in `R`.
    Occlusion,
    /// Emission, sRGB.
    Emissive,
}

impl MeshEditMap {
    /// The map's document spelling (`base_color`, `metallic_roughness`, `normal`, `occlusion`,
    /// `emissive`).
    pub fn name(self) -> &'static str {
        match self {
            MeshEditMap::BaseColor => "base_color",
            MeshEditMap::MetallicRoughness => "metallic_roughness",
            MeshEditMap::Normal => "normal",
            MeshEditMap::Occlusion => "occlusion",
            MeshEditMap::Emissive => "emissive",
        }
    }
}

/// A region and its edge: weight `1` inside the shape's surface, falling linearly to `0` over
/// `feather` world units outside it — and, when `key` is present, only where the texel's colour
/// matches it.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshEditRegion {
    /// The primitive, in the source mesh's own coordinates.
    pub shape: MeshEditRegionShape,
    /// The falloff distance outside the surface, in world units, `>= 0`. `None` is the sharp `0`.
    pub feather: Option<f64>,
    /// A colour filter over the shape: a texel it rejects gets weight `0`. `None` keeps every
    /// texel the shape reaches.
    pub key: Option<MeshEditKey>,
}

/// A colour-match filter inside a region: a texel whose `map` colour is within `tolerance` of
/// `color` keeps its weight, one farther than `tolerance + color_feather` gets weight `0`, and one
/// between the two is scaled linearly from `1` down to `0`.
///
/// The distance is Euclidean over sRGB components in `[0, 1]` — the same metric a `fill` region
/// uses against its seed.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshEditKey {
    /// The map whose colour is compared. Any of the five slots, read as the texel's stored bytes.
    pub map: MeshEditMap,
    /// The colour to match, sRGB, each component in `[0, 1]`.
    pub color: [f64; 3],
    /// The largest Euclidean sRGB distance that still matches fully, `>= 0`.
    pub tolerance: f64,
    /// The colour distance past `tolerance` over which the match fades to `0`, `>= 0`. `None` is
    /// the hard `0`.
    pub color_feather: Option<f64>,
}

/// A region primitive — the sculpt part vocabulary, in the source mesh's own coordinates.
#[derive(Clone, Debug, PartialEq)]
pub enum MeshEditRegionShape {
    /// Everything within `radius` of `at`.
    Sphere {
        /// The centre.
        at: [f64; 3],
        /// The radius, `> 0`.
        radius: f64,
    },
    /// Everything within `radius` of the polyline through `points`.
    Capsule {
        /// The polyline's vertices, in order; at least two. Two points are a straight segment.
        points: Vec<[f64; 3]>,
        /// The radius, `> 0`.
        radius: f64,
    },
    /// An axis-aligned box of full extent `size`, centred on `at`.
    Box {
        /// The centre.
        at: [f64; 3],
        /// The full extent along each axis, every component `> 0`.
        size: [f64; 3],
    },
    /// The connected patch of surface around `at` whose colour matches the colour at `at`: a flood
    /// over the mesh's surface, bounded by `max_radius`.
    Fill {
        /// The seed, on the surface.
        at: [f64; 3],
        /// The largest Euclidean sRGB distance from the seed's colour that still joins the patch,
        /// `>= 0`.
        tolerance: f64,
        /// The largest straight-line distance from `at` the patch may reach, `> 0`.
        max_radius: f64,
        /// The map whose colour is compared; `None` reads [`MeshEditMap::BaseColor`].
        map: Option<MeshEditMap>,
        /// The colour distance past `tolerance` over which a texel still joins the patch, its
        /// weight fading linearly to `0`, `>= 0`. `None` is the hard `0`.
        color_feather: Option<f64>,
    },
}

impl MeshEditRegionShape {
    /// The shape's document spelling (`sphere`, `capsule`, `box`, `fill`).
    pub fn name(&self) -> &'static str {
        match self {
            MeshEditRegionShape::Sphere { .. } => "sphere",
            MeshEditRegionShape::Capsule { .. } => "capsule",
            MeshEditRegionShape::Box { .. } => "box",
            MeshEditRegionShape::Fill { .. } => "fill",
        }
    }
}

// --- Parse (structure, `L1173`) -----------------------------------------------------------------

/// Parses strict-subset YAML into a [`MeshEditDocument`], checking structure/shape only (`L1173`).
pub(crate) fn parse(source: &str) -> Result<MeshEditDocument, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![
            Diagnostic::new(
                STRUCTURE,
                "the mesh-edit document is empty; an edit needs at least `format: 1`, \
                 `kind: mesh-edit`, `source`, and `ops`",
            )
            .with_path("")
            .with_help(
                "a minimal edit is:\n  format: 1\n  kind: mesh-edit\n  source: mesh-0.mesh.yaml\n  \
                 ops: []",
            ),
        ]);
    };
    let NodeValue::Mapping(entries) = root.value else {
        return Err(vec![
            Diagnostic::new(
                STRUCTURE,
                format!(
                    "the mesh-edit document root must be a mapping, found {} (line {})",
                    root.value.type_phrase(),
                    root.line
                ),
            )
            .with_path("")
            .with_help("start the file with the top-level keys `format:` and `kind: mesh-edit`"),
        ]);
    };

    let mut diagnostics = Vec::new();
    let mut doc = MeshEditDocument::default();
    let mut seen: Vec<&str> = Vec::new();

    for entry in &entries {
        seen.push(entry.key.as_str());
        match entry.key.as_str() {
            "format" => envelope::check_envelope(
                entry,
                STRUCTURE,
                STRUCTURE,
                MESH_EDIT_FORMAT_VERSION,
                "the mesh-edit document",
                "mesh-edit",
                "mesh-edit",
                &mut diagnostics,
            ),
            "kind" => envelope::check_kind(
                entry,
                STRUCTURE,
                "mesh-edit",
                "a mesh-edit document",
                &mut diagnostics,
            ),
            "name" => {
                doc.name = envelope::expect_string(
                    &entry.node,
                    "/name",
                    "the mesh-edit name",
                    STRUCTURE,
                    &mut diagnostics,
                );
            }
            "source" => {
                if let Some(value) = envelope::expect_string(
                    &entry.node,
                    "/source",
                    "the `source` mesh document",
                    STRUCTURE,
                    &mut diagnostics,
                ) {
                    doc.source = value;
                }
            }
            "out" => {
                doc.out = envelope::expect_string(
                    &entry.node,
                    "/out",
                    "the `out` directory",
                    STRUCTURE,
                    &mut diagnostics,
                );
            }
            "ops" => doc.ops = parse_ops(&entry.node, &mut diagnostics),
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "",
                "a mesh-edit document",
                MESH_EDIT_KEYS,
                STRUCTURE,
            )),
        }
    }

    for (key, help) in [
        ("format", "add `format: 1` as the first line"),
        ("kind", "add `kind: mesh-edit`"),
        (
            "source",
            "add `source:` naming the mesh document this edits, relative to this file",
        ),
        (
            "ops",
            "add an `ops:` list; an edit with nothing to do is `ops: []`",
        ),
    ] {
        if !seen.contains(&key) {
            diagnostics.push(envelope::missing_key(
                key,
                help,
                STRUCTURE,
                "mesh-edit document",
            ));
        }
    }

    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

fn parse_ops(node: &Node, diagnostics: &mut Vec<Diagnostic>) -> Vec<MeshEditOp> {
    let Some(items) = expect_sequence(node, "/ops", "ops", diagnostics) else {
        return Vec::new();
    };
    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| parse_op(item, index, diagnostics))
        .collect()
}

fn parse_op(node: &Node, index: usize, diagnostics: &mut Vec<Diagnostic>) -> Option<MeshEditOp> {
    let pointer = format!("/ops/{index}");
    let entries = expect_mapping(node, &pointer, "an op", diagnostics)?;

    let before = diagnostics.len();
    let mut op_name = None;
    let mut map = None;
    let mut size = None;
    let mut color = None;
    let mut strength = None;
    let mut region = None;
    let mut seen: Vec<&str> = Vec::new();

    for entry in entries {
        seen.push(entry.key.as_str());
        let field = format!("{pointer}/{}", entry.key);
        match entry.key.as_str() {
            "op" => {
                op_name = parse_keyword(&entry.node, &field, "op", &OP_NAMES, diagnostics);
            }
            "map" => map = parse_map(&entry.node, &field, diagnostics),
            "size" => size = parse_u32(&entry.node, &field, "size", diagnostics),
            "color" => color = parse_vec3(&entry.node, &field, "color", diagnostics),
            "strength" => {
                strength = parse_number(&entry.node, &field, "strength", STRUCTURE, diagnostics)
            }
            "region" => region = parse_region(&entry.node, &field, diagnostics),
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                &pointer,
                "a paint op",
                PAINT_KEYS,
                STRUCTURE,
            )),
        }
    }

    for (key, help) in [
        ("op", "add `op: paint`"),
        ("map", "add `map: emissive`"),
        ("color", "add a `color:` of three numbers in [0, 1]"),
        (
            "region",
            "add a `region:` of exactly one of `sphere`, `capsule`, `box` or `fill`",
        ),
    ] {
        if !seen.contains(&key) {
            diagnostics.push(missing_nested(&pointer, "an op", key, help));
        }
    }

    // A single wrong key anywhere in the op makes the whole op unrepresentable: report every
    // problem in it, then drop it rather than inventing a default that nothing authored.
    if diagnostics.len() != before {
        return None;
    }
    match op_name? {
        "paint" => Some(MeshEditOp::Paint {
            map: map?,
            size,
            color: color?,
            strength,
            region: region?,
        }),
        other => unreachable!("`{other}` is not one of {OP_NAMES:?}"),
    }
}

/// Parses a `map:` value that a paint op writes: of the five texture slots, this engine paints
/// `emissive`.
fn parse_map(node: &Node, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<MeshEditMap> {
    let map = parse_slot(node, pointer, diagnostics)?;
    if map != MeshEditMap::Emissive {
        diagnostics.push(
            Diagnostic::new(
                STRUCTURE,
                format!(
                    "`map: {}` names a texture slot this engine does not paint (line {})",
                    map.name(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("`emissive` is the one map a paint op may target"),
        );
        return None;
    }
    Some(map)
}

/// Parses a `map:` value naming any of the five texture slots — what a colour match reads.
fn parse_slot(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<MeshEditMap> {
    let name = parse_keyword(node, pointer, "map", &MAP_NAMES, diagnostics)?;
    Some(match name {
        "base_color" => MeshEditMap::BaseColor,
        "metallic_roughness" => MeshEditMap::MetallicRoughness,
        "normal" => MeshEditMap::Normal,
        "occlusion" => MeshEditMap::Occlusion,
        "emissive" => MeshEditMap::Emissive,
        other => unreachable!("`{other}` is not one of {MAP_NAMES:?}"),
    })
}

/// Parses a `region:` value: a mapping of exactly one shape key, plus that shape's own fields and
/// an optional `feather`.
fn parse_region(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<MeshEditRegion> {
    let entries = expect_mapping(node, pointer, "a region", diagnostics)?;
    let before = diagnostics.len();

    let mut shape_entry = None;
    let mut key_entry = None;
    let mut shapes = 0;
    let mut unknown = false;
    for entry in entries {
        if entry.key == "key" {
            key_entry = Some(entry);
        } else if REGION_NAMES.contains(&entry.key.as_str()) {
            shapes += 1;
            shape_entry.get_or_insert(entry);
        } else {
            unknown = true;
            diagnostics.push(envelope::unknown_key(
                &entry.key,
                entry,
                pointer,
                "a region",
                REGION_ENTRY_NAMES,
                STRUCTURE,
            ));
        }
    }
    if shapes != 1 && !unknown {
        diagnostics.push(
            Diagnostic::new(
                STRUCTURE,
                format!(
                    "a region names exactly one shape, found {shapes} (line {})",
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(format!(
                "write one of: {}; a second shape is a second op",
                REGION_NAMES.join(", ")
            )),
        );
    }

    let shape = match shape_entry {
        Some(entry) if shapes == 1 => parse_shape(entry, pointer, diagnostics),
        _ => None,
    };
    let key =
        key_entry.and_then(|entry| parse_key(&entry.node, &format!("{pointer}/key"), diagnostics));
    if diagnostics.len() != before {
        return None;
    }
    let (shape, feather) = shape?;
    Some(MeshEditRegion {
        shape,
        feather,
        key,
    })
}

/// Parses the one shape entry of a region: the shape's own fields and its optional `feather`.
fn parse_shape(
    entry: &MapEntry,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<(MeshEditRegionShape, Option<f64>)> {
    let name = REGION_NAMES.iter().find(|name| **name == entry.key)?;
    let shape_pointer = format!("{pointer}/{name}");
    let fields = expect_mapping(
        &entry.node,
        &shape_pointer,
        format!("a `{name}` region").as_str(),
        diagnostics,
    )?;

    let before = diagnostics.len();
    let valid: &[&'static str] = match *name {
        "sphere" => &SPHERE_KEYS,
        "capsule" => &CAPSULE_KEYS,
        "box" => &BOX_KEYS,
        "fill" => &FILL_KEYS,
        other => unreachable!("`{other}` is not one of {REGION_NAMES:?}"),
    };
    let mut at = None;
    let mut points = None;
    let mut radius = None;
    let mut size = None;
    let mut tolerance = None;
    let mut max_radius = None;
    let mut map = None;
    let mut feather = None;
    let mut color_feather = None;
    let mut seen: Vec<&str> = Vec::new();
    let mut legacy_capsule = false;

    for field in fields {
        seen.push(field.key.as_str());
        let field_pointer = format!("{shape_pointer}/{}", field.key);
        if *name == "capsule" && matches!(field.key.as_str(), "from" | "to") {
            if !legacy_capsule {
                diagnostics.push(legacy_capsule_keys(&shape_pointer, field));
            }
            legacy_capsule = true;
            continue;
        }
        if !valid.contains(&field.key.as_str()) {
            diagnostics.push(unknown_region_key(name, field, &shape_pointer));
            continue;
        }
        match field.key.as_str() {
            "at" => at = parse_vec3(&field.node, &field_pointer, "at", diagnostics),
            "points" => points = parse_points(&field.node, &field_pointer, diagnostics),
            "radius" => {
                radius = parse_number(
                    &field.node,
                    &field_pointer,
                    "radius",
                    STRUCTURE,
                    diagnostics,
                )
            }
            "size" => size = parse_vec3(&field.node, &field_pointer, "size", diagnostics),
            "tolerance" => {
                tolerance = parse_number(
                    &field.node,
                    &field_pointer,
                    "tolerance",
                    STRUCTURE,
                    diagnostics,
                );
            }
            "max_radius" => {
                max_radius = parse_number(
                    &field.node,
                    &field_pointer,
                    "max_radius",
                    STRUCTURE,
                    diagnostics,
                );
            }
            "map" => map = parse_slot(&field.node, &field_pointer, diagnostics),
            "feather" => {
                feather = parse_number(
                    &field.node,
                    &field_pointer,
                    "feather",
                    STRUCTURE,
                    diagnostics,
                );
            }
            "color_feather" => {
                color_feather = parse_number(
                    &field.node,
                    &field_pointer,
                    "color_feather",
                    STRUCTURE,
                    diagnostics,
                );
            }
            other => unreachable!("`{other}` passed the key check for `{name}`"),
        }
    }

    for key in valid
        .iter()
        .filter(|key| !matches!(**key, "feather" | "map" | "color_feather"))
    {
        // The old `from`/`to` spelling already carried its own fix-it; `points` is not also missing.
        let superseded = legacy_capsule && *key == "points";
        if !seen.contains(key) && !superseded {
            diagnostics.push(missing_nested(
                &shape_pointer,
                format!("a `{name}` region").as_str(),
                key,
                format!("add `{key}:` to it").as_str(),
            ));
        }
    }
    if diagnostics.len() != before {
        return None;
    }

    let shape = match *name {
        "sphere" => MeshEditRegionShape::Sphere {
            at: at?,
            radius: radius?,
        },
        "capsule" => MeshEditRegionShape::Capsule {
            points: points?,
            radius: radius?,
        },
        "box" => MeshEditRegionShape::Box {
            at: at?,
            size: size?,
        },
        "fill" => MeshEditRegionShape::Fill {
            at: at?,
            tolerance: tolerance?,
            max_radius: max_radius?,
            map,
            color_feather,
        },
        other => unreachable!("`{other}` is not one of {REGION_NAMES:?}"),
    };
    Some((shape, feather))
}

/// Parses a region's `key:` filter: a mapping of `map`, `color` and `tolerance`, all required, and
/// an optional `color_feather`.
fn parse_key(node: &Node, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<MeshEditKey> {
    let fields = expect_mapping(node, pointer, "a `key` filter", diagnostics)?;
    let before = diagnostics.len();
    let mut map = None;
    let mut color = None;
    let mut tolerance = None;
    let mut color_feather = None;
    let mut seen: Vec<&str> = Vec::new();

    for field in fields {
        seen.push(field.key.as_str());
        let field_pointer = format!("{pointer}/{}", field.key);
        match field.key.as_str() {
            "map" => map = parse_slot(&field.node, &field_pointer, diagnostics),
            "color" => color = parse_vec3(&field.node, &field_pointer, "color", diagnostics),
            "tolerance" => {
                tolerance = parse_number(
                    &field.node,
                    &field_pointer,
                    "tolerance",
                    STRUCTURE,
                    diagnostics,
                );
            }
            "color_feather" => {
                color_feather = parse_number(
                    &field.node,
                    &field_pointer,
                    "color_feather",
                    STRUCTURE,
                    diagnostics,
                );
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                field,
                pointer,
                "a `key` filter",
                KEY_KEYS,
                STRUCTURE,
            )),
        }
    }
    for key in KEY_KEYS.into_iter().filter(|key| *key != "color_feather") {
        if !seen.contains(&key) {
            diagnostics.push(missing_nested(
                pointer,
                "a `key` filter",
                key,
                format!("add `{key}:` to it").as_str(),
            ));
        }
    }
    if diagnostics.len() != before {
        return None;
    }
    Some(MeshEditKey {
        map: map?,
        color: color?,
        tolerance: tolerance?,
        color_feather,
    })
}

/// Parses a capsule's `points:` — a sequence of three-number points. The minimum count is a data
/// rule (`L1174`), not a structure one.
fn parse_points(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Vec<[f64; 3]>> {
    let items = expect_sequence(node, pointer, "points", diagnostics)?;
    let mut points = Vec::with_capacity(items.len());
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match parse_vec3(item, &format!("{pointer}/{index}"), "a point", diagnostics) {
            Some(point) => points.push(point),
            None => ok = false,
        }
    }
    ok.then_some(points)
}

/// The one diagnostic for a capsule still spelled with `from`/`to`: the polyline replaced them.
fn legacy_capsule_keys(shape_pointer: &str, field: &MapEntry) -> Diagnostic {
    Diagnostic::new(
        STRUCTURE,
        format!(
            "a `capsule` region takes `points:`, a polyline of two or more points, in place of \
             `from` and `to` (line {})",
            field.key_line
        ),
    )
    .with_path(format!("{shape_pointer}/{}", field.key))
    .with_help(
        "rewrite the segment as its two end points, `from` first:\n  points:\n    - - <from x>\n      \
         - <from y>\n      - <from z>\n    - - <to x>\n      - <to y>\n      - <to z>",
    )
}

/// The "a `sphere` region does not have a key named `from`" diagnostic. Each shape's key list is a
/// fixed-size array so [`envelope::unknown_key`] can suggest over it, which is why the shape is
/// matched here rather than passed as a slice.
fn unknown_region_key(name: &str, field: &MapEntry, pointer: &str) -> Diagnostic {
    let noun = format!("a `{name}` region");
    match name {
        "sphere" => {
            envelope::unknown_key(&field.key, field, pointer, &noun, SPHERE_KEYS, STRUCTURE)
        }
        "capsule" => {
            envelope::unknown_key(&field.key, field, pointer, &noun, CAPSULE_KEYS, STRUCTURE)
        }
        "box" => envelope::unknown_key(&field.key, field, pointer, &noun, BOX_KEYS, STRUCTURE),
        "fill" => envelope::unknown_key(&field.key, field, pointer, &noun, FILL_KEYS, STRUCTURE),
        other => unreachable!("`{other}` is not one of {REGION_NAMES:?}"),
    }
}

fn missing_nested(pointer: &str, noun: &str, key: &str, help: &str) -> Diagnostic {
    Diagnostic::new(STRUCTURE, format!("{noun} is missing its required `{key}`"))
        .with_path(pointer.to_string())
        .with_help(help.to_string())
}

fn expect_sequence<'a>(
    node: &'a Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<&'a [Node]> {
    match &node.value {
        NodeValue::Sequence(items) => Some(items),
        other => {
            diagnostics.push(
                Diagnostic::new(
                    STRUCTURE,
                    format!(
                        "`{what}` must be a sequence, found {} (line {})",
                        other.type_phrase(),
                        node.line
                    ),
                )
                .with_path(pointer.to_string())
                .with_help(format!(
                    "write `{what}` as a list, one `- ...` entry per item"
                )),
            );
            None
        }
    }
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
            diagnostics.push(
                Diagnostic::new(
                    STRUCTURE,
                    format!(
                        "{what} must be a mapping, found {} (line {})",
                        other.type_phrase(),
                        node.line
                    ),
                )
                .with_path(pointer.to_string())
                .with_help("write it as `key: value` lines"),
            );
            None
        }
    }
}

fn parse_u32(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<u32> {
    let value = match &node.value {
        NodeValue::Scalar(Value::Number(n)) => n.as_u64().and_then(|v| u32::try_from(v).ok()),
        _ => None,
    };
    match value {
        Some(value) => Some(value),
        None => {
            diagnostics.push(
                Diagnostic::new(
                    STRUCTURE,
                    format!(
                        "`{what}` must be a non-negative integer that fits in 32 bits, found {} \
                         (line {})",
                        node.value.type_phrase(),
                        node.line
                    ),
                )
                .with_path(pointer.to_string())
                .with_help(format!(
                    "write `{what}` as a whole number, e.g. `{what}: 1024`"
                )),
            );
            None
        }
    }
}

fn parse_vec3(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<[f64; 3]> {
    let help = format!("write `{what}` as three numbers, one per line");
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                STRUCTURE,
                format!(
                    "`{what}` must be a sequence of three numbers, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(help),
        );
        return None;
    };
    if items.len() != 3 {
        diagnostics.push(
            Diagnostic::new(
                STRUCTURE,
                format!(
                    "`{what}` must have exactly 3 components, found {} (line {})",
                    items.len(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(help),
        );
        return None;
    }
    let mut out = [0.0; 3];
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match parse_number(
            item,
            &format!("{pointer}/{index}"),
            what,
            STRUCTURE,
            diagnostics,
        ) {
            Some(value) => out[index] = value,
            None => ok = false,
        }
    }
    ok.then_some(out)
}

fn parse_keyword(
    node: &Node,
    pointer: &str,
    what: &str,
    choices: &[&'static str],
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<&'static str> {
    let listed = format!("`{what}` is one of: {}", choices.join(", "));
    match &node.value {
        NodeValue::Scalar(Value::String(s)) => {
            if let Some(choice) = choices.iter().copied().find(|choice| *choice == s.as_str()) {
                return Some(choice);
            }
            let help = match suggest::nearest(s, choices.iter().copied()) {
                Some(candidate) => format!("nearest match is `{candidate}`"),
                None => listed,
            };
            diagnostics.push(
                Diagnostic::new(
                    STRUCTURE,
                    format!("unknown {what} `{s}` (line {})", node.line),
                )
                .with_path(pointer.to_string())
                .with_help(help),
            );
            None
        }
        other => {
            diagnostics.push(
                Diagnostic::new(
                    STRUCTURE,
                    format!(
                        "`{what}` must be a string, found {} (line {})",
                        other.type_phrase(),
                        node.line
                    ),
                )
                .with_path(pointer.to_string())
                .with_help(listed),
            );
            None
        }
    }
}

// --- Validate (data, `L1174`) -------------------------------------------------------------------

/// Validates the data invariants of an already-parsed [`MeshEditDocument`] (`L1174`): every number
/// finite, radii and extents positive, colours and strengths in `[0, 1]`, feathers non-negative, a
/// non-empty `source`, and an `out` that stays below this document's own directory.
///
/// Nothing here touches the filesystem: whether `source` exists, carries `uvs`, or already has the
/// map an op sizes is the consumer's `L1176`.
pub(crate) fn validate(doc: &MeshEditDocument) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    if doc.source.is_empty() {
        diagnostics.push(
            Diagnostic::new(
                DATA,
                "`source` is empty; it must name the mesh document this edit derives from",
            )
            .with_path("/source")
            .with_help(
                "write it relative to this document's own directory, e.g. `mesh-0.mesh.yaml`",
            ),
        );
    } else if let Some(refusal) = sibling_refusal(&doc.source) {
        diagnostics.push(
            Diagnostic::new(
                DATA,
                format!("`source` (`{}`) {}", doc.source, refusal.describe()),
            )
            .with_path("/source")
            .with_help(
                "write it relative to this document's own directory, e.g. `mesh-0.mesh.yaml`",
            ),
        );
    }
    if let Some(out) = &doc.out {
        check_out(out, &mut diagnostics);
    }

    for (index, op) in doc.ops.iter().enumerate() {
        let pointer = format!("/ops/{index}");
        let MeshEditOp::Paint {
            size,
            color,
            strength,
            region,
            ..
        } = op;
        if let Some(size) = size
            && *size == 0
        {
            diagnostics.push(
                Diagnostic::new(
                    DATA,
                    "`size` is 0; a created map needs a positive square extent",
                )
                .with_path(format!("{pointer}/size"))
                .with_help("give the map an extent in texels, e.g. `size: 1024`"),
            );
        }
        check_colour(
            color,
            &format!("{pointer}/color"),
            "a colour is an authored sRGB value, not an intensity; the glow's intensity is the \
             scene's `Mesh.emissive`",
            &mut diagnostics,
        );
        if let Some(strength) = strength {
            let at = format!("{pointer}/strength");
            if check_finite(*strength, &at, "`strength`", &mut diagnostics)
                && !(0.0..=1.0).contains(strength)
            {
                diagnostics.push(
                    Diagnostic::new(
                        DATA,
                        format!("`strength` is {strength}, outside the valid range [0, 1]"),
                    )
                    .with_path(at)
                    .with_help(
                        "`strength` scales the region's weight, which is already 1 inside the \
                         region's surface",
                    ),
                );
            }
        }
        validate_region(region, &format!("{pointer}/region"), &mut diagnostics);
    }

    diagnostics
}

fn validate_region(region: &MeshEditRegion, pointer: &str, diagnostics: &mut Vec<Diagnostic>) {
    let shape_pointer = format!("{pointer}/{}", region.shape.name());
    match &region.shape {
        MeshEditRegionShape::Sphere { at, radius } => {
            check_point(at, &shape_pointer, "at", diagnostics);
            check_radius(*radius, &shape_pointer, diagnostics);
        }
        MeshEditRegionShape::Capsule { points, radius } => {
            if points.len() < 2 {
                diagnostics.push(
                    Diagnostic::new(
                        DATA,
                        format!(
                            "a capsule needs at least two points, found {}",
                            points.len()
                        ),
                    )
                    .with_path(format!("{shape_pointer}/points"))
                    .with_help("two points are a straight segment; more bend it into a polyline"),
                );
            }
            for (index, point) in points.iter().enumerate() {
                for (component, value) in point.iter().enumerate() {
                    check_finite(
                        *value,
                        &format!("{shape_pointer}/points/{index}/{component}"),
                        "a `points` component",
                        diagnostics,
                    );
                }
            }
            check_radius(*radius, &shape_pointer, diagnostics);
        }
        MeshEditRegionShape::Fill {
            at,
            tolerance,
            max_radius,
            color_feather,
            ..
        } => {
            check_point(at, &shape_pointer, "at", diagnostics);
            check_radius_key(*max_radius, "max_radius", &shape_pointer, diagnostics);
            check_tolerance(*tolerance, &shape_pointer, diagnostics);
            check_color_feather(*color_feather, &shape_pointer, diagnostics);
        }
        MeshEditRegionShape::Box { at, size } => {
            check_point(at, &shape_pointer, "at", diagnostics);
            for (component, value) in size.iter().enumerate() {
                let extent = format!("{shape_pointer}/size/{component}");
                if check_finite(*value, &extent, "a `size` component", diagnostics) && *value <= 0.0
                {
                    diagnostics.push(
                        Diagnostic::new(
                            DATA,
                            format!(
                                "`size[{component}]` is {value}; a box extent must be greater \
                                 than 0"
                            ),
                        )
                        .with_path(extent)
                        .with_help("`size` is the box's full extent along each axis"),
                    );
                }
            }
        }
    }
    if let Some(feather) = region.feather {
        let at = format!("{shape_pointer}/feather");
        if check_finite(feather, &at, "`feather`", diagnostics) && feather < 0.0 {
            diagnostics.push(
                Diagnostic::new(
                    DATA,
                    format!("`feather` is {feather}; a falloff distance cannot be negative"),
                )
                .with_path(at)
                .with_help(
                    "`feather` extends the region outward in world units; 0 is a sharp edge",
                ),
            );
        }
    }
    if let Some(key) = &region.key {
        let key_pointer = format!("{pointer}/key");
        check_colour(
            &key.color,
            &format!("{key_pointer}/color"),
            "`color` is the authored sRGB colour to match, each component in [0, 1]",
            diagnostics,
        );
        check_tolerance(key.tolerance, &key_pointer, diagnostics);
        check_color_feather(key.color_feather, &key_pointer, diagnostics);
    }
}

fn check_point(point: &[f64; 3], pointer: &str, key: &str, diagnostics: &mut Vec<Diagnostic>) {
    for (component, value) in point.iter().enumerate() {
        check_finite(
            *value,
            &format!("{pointer}/{key}/{component}"),
            &format!("a `{key}` component"),
            diagnostics,
        );
    }
}

fn check_radius(radius: f64, pointer: &str, diagnostics: &mut Vec<Diagnostic>) {
    check_radius_key(radius, "radius", pointer, diagnostics);
}

fn check_radius_key(radius: f64, key: &str, pointer: &str, diagnostics: &mut Vec<Diagnostic>) {
    let at = format!("{pointer}/{key}");
    if check_finite(radius, &at, &format!("`{key}`"), diagnostics) && radius <= 0.0 {
        diagnostics.push(
            Diagnostic::new(
                DATA,
                format!("`{key}` is {radius}; a region radius must be greater than 0"),
            )
            .with_path(at)
            .with_help("a region with no volume covers no texel"),
        );
    }
}

/// A colour-match tolerance: finite and `>= 0`; `0` matches only the exact colour.
fn check_tolerance(tolerance: f64, pointer: &str, diagnostics: &mut Vec<Diagnostic>) {
    let at = format!("{pointer}/tolerance");
    if check_finite(tolerance, &at, "`tolerance`", diagnostics) && tolerance < 0.0 {
        diagnostics.push(
            Diagnostic::new(
                DATA,
                format!("`tolerance` is {tolerance}; a colour distance cannot be negative"),
            )
            .with_path(at)
            .with_help(
                "the distance is Euclidean over sRGB components in [0, 1]; 0 matches only the \
                 exact colour",
            ),
        );
    }
}

/// A colour fade past the tolerance: finite and `>= 0`; `0` is the hard colour edge.
fn check_color_feather(
    color_feather: Option<f64>,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(color_feather) = color_feather else {
        return;
    };
    let at = format!("{pointer}/color_feather");
    if check_finite(color_feather, &at, "`color_feather`", diagnostics) && color_feather < 0.0 {
        diagnostics.push(
            Diagnostic::new(
                DATA,
                format!("`color_feather` is {color_feather}; a colour distance cannot be negative"),
            )
            .with_path(at)
            .with_help(
                "`color_feather` fades the match to 0 over that much sRGB distance past \
                 `tolerance`; 0 is a hard colour edge",
            ),
        );
    }
}

/// The op colour's range rule (`[0, 1]` each component), applied to a colour at `pointer`.
fn check_colour(color: &[f64; 3], pointer: &str, help: &str, diagnostics: &mut Vec<Diagnostic>) {
    for (component, value) in color.iter().enumerate() {
        let at = format!("{pointer}/{component}");
        if !check_finite(*value, &at, "a `color` component", diagnostics) {
            continue;
        }
        if !(0.0..=1.0).contains(value) {
            diagnostics.push(
                Diagnostic::new(
                    DATA,
                    format!("`color[{component}]` is {value}, outside the valid range [0, 1]"),
                )
                .with_path(at)
                .with_help(help.to_string()),
            );
        }
    }
}

/// Pushes an `L1174` for a non-finite `value`, returning whether it is finite — so a range check
/// reads `if check_finite(..) && out_of_range`, never reporting a `NaN` twice.
fn check_finite(value: f64, pointer: &str, what: &str, diagnostics: &mut Vec<Diagnostic>) -> bool {
    if value.is_finite() {
        return true;
    }
    diagnostics.push(
        Diagnostic::new(
            DATA,
            format!("{what} is {value}, which is not a finite number"),
        )
        .with_path(pointer.to_string()),
    );
    false
}

/// The `out` containment rule: the derive owns every byte under `out`, so it must name a directory
/// below the edit document itself — `ludion_core::paths::relative_refusal`'s rule.
fn check_out(out: &str, diagnostics: &mut Vec<Diagnostic>) {
    let refuse = |reason: &str, fix: &str| {
        Diagnostic::new(DATA, format!("`out` {reason}"))
            .with_path("/out")
            .with_help(fix.to_string())
    };
    if out.is_empty() {
        diagnostics.push(refuse(
            "is empty; it must name the derived directory this edit owns",
            "remove the key to use this document's own stem, or name a directory beside it",
        ));
        return;
    }
    if let Some(refusal) = relative_refusal(out) {
        let help = if refusal == PathRefusal::ParentDir {
            "the derive owns every byte under `out`, so it may only write below this document"
        } else {
            "write it as a directory name beside this file, e.g. `out: idol-lit`"
        };
        diagnostics.push(refuse(&format!("(`{out}`) {}", refusal.describe()), help));
    }
}

// --- Emit ---------------------------------------------------------------------------------------

/// Writes `doc` in canonical form: `format`, `kind`, `name` (optional), `source`, `out` (optional),
/// `ops` in authored order, block style, ryu shortest-round-trip floats. An absent `size`,
/// `strength` or `feather` is not written, so a document authors exactly the keys it means.
pub(crate) fn emit(doc: &MeshEditDocument) -> String {
    let mut out = String::new();
    out.push_str("format: 1\n");
    out.push_str("kind: mesh-edit\n");
    if let Some(name) = &doc.name {
        out.push_str(&format!("name: {}\n", scalar_string(name)));
    }
    out.push_str(&format!("source: {}\n", scalar_string(&doc.source)));
    if let Some(dir) = &doc.out {
        out.push_str(&format!("out: {}\n", scalar_string(dir)));
    }
    if doc.ops.is_empty() {
        out.push_str("ops: []\n");
        return out;
    }
    out.push_str("ops:\n");
    for op in &doc.ops {
        emit_op(&mut out, op);
    }
    out
}

fn emit_op(out: &mut String, op: &MeshEditOp) {
    const INDENT: &str = "    ";
    out.push_str(&format!("  - op: {}\n", op.name()));
    let MeshEditOp::Paint {
        map,
        size,
        color,
        strength,
        region,
    } = op;
    out.push_str(&format!("{INDENT}map: {}\n", map.name()));
    if let Some(size) = size {
        out.push_str(&format!("{INDENT}size: {size}\n"));
    }
    write_vec3(out, INDENT, "color", color);
    if let Some(strength) = strength {
        write_float(out, INDENT, "strength", *strength);
    }
    out.push_str(&format!("{INDENT}region:\n"));
    emit_region(out, region);
}

fn emit_region(out: &mut String, region: &MeshEditRegion) {
    const SHAPE: &str = "      ";
    const FIELD: &str = "        ";
    out.push_str(&format!("{SHAPE}{}:\n", region.shape.name()));
    match &region.shape {
        MeshEditRegionShape::Sphere { at, radius } => {
            write_vec3(out, FIELD, "at", at);
            write_float(out, FIELD, "radius", *radius);
        }
        MeshEditRegionShape::Capsule { points, radius } => {
            write_points(out, FIELD, points);
            write_float(out, FIELD, "radius", *radius);
        }
        MeshEditRegionShape::Box { at, size } => {
            write_vec3(out, FIELD, "at", at);
            write_vec3(out, FIELD, "size", size);
        }
        MeshEditRegionShape::Fill {
            at,
            tolerance,
            max_radius,
            map,
            ..
        } => {
            write_vec3(out, FIELD, "at", at);
            write_float(out, FIELD, "tolerance", *tolerance);
            write_float(out, FIELD, "max_radius", *max_radius);
            if let Some(map) = map {
                out.push_str(&format!("{FIELD}map: {}\n", map.name()));
            }
        }
    }
    if let Some(feather) = region.feather {
        write_float(out, FIELD, "feather", feather);
    }
    if let MeshEditRegionShape::Fill {
        color_feather: Some(color_feather),
        ..
    } = region.shape
    {
        write_float(out, FIELD, "color_feather", color_feather);
    }
    if let Some(key) = &region.key {
        out.push_str(&format!("{SHAPE}key:\n"));
        out.push_str(&format!("{FIELD}map: {}\n", key.map.name()));
        write_vec3(out, FIELD, "color", &key.color);
        write_float(out, FIELD, "tolerance", key.tolerance);
        if let Some(color_feather) = key.color_feather {
            write_float(out, FIELD, "color_feather", color_feather);
        }
    }
}

fn write_float(out: &mut String, indent: &str, key: &str, value: f64) {
    out.push_str(&format!("{indent}{key}: {}\n", format_float(value)));
}

fn write_vec3(out: &mut String, indent: &str, key: &str, values: &[f64; 3]) {
    out.push_str(&format!("{indent}{key}:\n"));
    for value in values {
        out.push_str(&format!("{indent}  - {}\n", format_float(*value)));
    }
}

/// A polyline as a block sequence of block sequences: `- - x`, then the point's other two
/// components aligned under the first.
fn write_points(out: &mut String, indent: &str, points: &[[f64; 3]]) {
    out.push_str(&format!("{indent}points:\n"));
    for point in points {
        for (component, value) in point.iter().enumerate() {
            let lead = if component == 0 { "- - " } else { "  - " };
            out.push_str(&format!("{indent}  {lead}{}\n", format_float(*value)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GLOWING_EYE: &str = "\
format: 1
kind: mesh-edit
name: idol-lit
source: mesh-0.mesh.yaml
out: idol-lit
ops:
  - op: paint
    map: emissive
    size: 1024
    color:
      - 0.05
      - 1.0
      - 0.35
    strength: 1.0
    region:
      sphere:
        at:
          - 0.118
          - 1.342
          - 0.203
        radius: 0.035
        feather: 0.012
";

    const MINIMAL: &str = "\
format: 1
kind: mesh-edit
source: mesh-0.mesh.yaml
ops: []
";

    fn parsed(source: &str) -> MeshEditDocument {
        let doc = parse(source).expect("the edit should parse");
        assert!(validate(&doc).is_empty(), "{:?}", validate(&doc));
        doc
    }

    #[test]
    fn parses_the_documented_example() {
        let doc = parsed(GLOWING_EYE);
        assert_eq!(doc.name.as_deref(), Some("idol-lit"));
        assert_eq!(doc.source, "mesh-0.mesh.yaml");
        assert_eq!(doc.out_dir("whatever"), "idol-lit");
        let MeshEditOp::Paint {
            map,
            size,
            color,
            strength,
            region,
        } = &doc.ops[0];
        assert_eq!(*map, MeshEditMap::Emissive);
        assert_eq!(*size, Some(1024));
        assert_eq!(*color, [0.05, 1.0, 0.35]);
        assert_eq!(*strength, Some(1.0));
        assert_eq!(region.feather, Some(0.012));
        assert_eq!(
            region.shape,
            MeshEditRegionShape::Sphere {
                at: [0.118, 1.342, 0.203],
                radius: 0.035,
            }
        );
    }

    #[test]
    fn emit_is_a_fixed_point() {
        for source in [GLOWING_EYE, MINIMAL] {
            let doc = parsed(source);
            assert_eq!(emit(&doc), source);
            let round_tripped = parse(&emit(&doc)).expect("the emitted text parses");
            assert_eq!(emit(&round_tripped), emit(&doc));
        }
    }

    #[test]
    fn an_absent_option_is_not_written() {
        let doc = parsed(MINIMAL);
        assert_eq!(doc.out, None);
        assert_eq!(doc.out_dir("idol-lit"), "idol-lit");
        let text = emit(&doc);
        assert!(!text.contains("name") && !text.contains("out"), "{text}");
    }

    #[test]
    fn a_capsule_and_a_box_round_trip() {
        let source = "\
format: 1
kind: mesh-edit
source: mesh-0.mesh.yaml
ops:
  - op: paint
    map: emissive
    color:
      - 1.0
      - 0.0
      - 0.0
    region:
      capsule:
        points:
          - - 0.0
            - 0.0
            - 0.0
          - - 0.0
            - 1.0
            - 0.0
        radius: 0.25
  - op: paint
    map: emissive
    color:
      - 0.0
      - 0.0
      - 1.0
    region:
      box:
        at:
          - 0.0
          - 0.5
          - 0.0
        size:
          - 1.0
          - 2.0
          - 1.0
        feather: 0.1
";
        let doc = parsed(source);
        assert_eq!(doc.ops.len(), 2);
        assert_eq!(emit(&doc), source);
    }

    #[test]
    fn an_unknown_top_level_key_suggests_the_nearest() {
        let diagnostics = parse(&MINIMAL.replace("source:", "sources:")).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/sources"))
            .expect("the unknown key is reported");
        assert_eq!(d.code, STRUCTURE);
        assert_eq!(d.help.as_deref(), Some("nearest match is `source`"));
    }

    #[test]
    fn a_missing_source_is_l1173() {
        let diagnostics = parse("format: 1\nkind: mesh-edit\nops: []\n").unwrap_err();
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == STRUCTURE && d.message.contains("`source`")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn bad_kind_is_l1173() {
        let diagnostics = parse(&MINIMAL.replace("kind: mesh-edit", "kind: mesh")).unwrap_err();
        assert_eq!(diagnostics[0].code, STRUCTURE);
        assert_eq!(diagnostics[0].path.as_deref(), Some("/kind"));
    }

    #[test]
    fn an_unknown_op_is_l1173() {
        let diagnostics = parse(&GLOWING_EYE.replace("op: paint", "op: pain")).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/ops/0/op"))
            .expect("the unknown op is reported");
        assert_eq!(d.code, STRUCTURE);
        assert_eq!(d.help.as_deref(), Some("nearest match is `paint`"));
    }

    #[test]
    fn an_unknown_op_key_suggests_the_nearest() {
        let diagnostics = parse(&GLOWING_EYE.replace("    map:", "    maps:")).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/ops/0/maps"))
            .expect("the unknown op key is reported");
        assert_eq!(d.code, STRUCTURE);
        assert_eq!(d.help.as_deref(), Some("nearest match is `map`"));
    }

    /// The five slot names all parse as known; `emissive` is the one a paint op may target, and the
    /// other four are refused by name rather than as a typo.
    #[test]
    fn a_map_this_engine_does_not_paint_is_l1173() {
        for map in ["base_color", "metallic_roughness", "normal", "occlusion"] {
            let diagnostics =
                parse(&GLOWING_EYE.replace("map: emissive", &format!("map: {map}"))).unwrap_err();
            let d = diagnostics
                .iter()
                .find(|d| d.path.as_deref() == Some("/ops/0/map"))
                .unwrap_or_else(|| panic!("`{map}` is reported"));
            assert_eq!(d.code, STRUCTURE);
            assert!(d.message.contains("does not paint"), "{}", d.message);
            assert_eq!(
                d.help.as_deref(),
                Some("`emissive` is the one map a paint op may target")
            );
        }
    }

    #[test]
    fn an_unknown_map_suggests_the_nearest() {
        let diagnostics = parse(&GLOWING_EYE.replace("map: emissive", "map: emisive")).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/ops/0/map"))
            .expect("the unknown map is reported");
        assert_eq!(d.help.as_deref(), Some("nearest match is `emissive`"));
    }

    #[test]
    fn an_unknown_region_shape_is_l1173() {
        let diagnostics = parse(&GLOWING_EYE.replace("      sphere:", "      spere:")).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/ops/0/region/spere"))
            .expect("the unknown region shape is reported");
        assert_eq!(d.code, STRUCTURE);
        assert_eq!(d.help.as_deref(), Some("nearest match is `sphere`"));
    }

    #[test]
    fn a_region_naming_two_shapes_is_l1173() {
        let source = "\
format: 1
kind: mesh-edit
source: mesh-0.mesh.yaml
ops:
  - op: paint
    map: emissive
    color:
      - 1.0
      - 1.0
      - 1.0
    region:
      sphere:
        at:
          - 0.0
          - 0.0
          - 0.0
        radius: 1.0
      box:
        at:
          - 0.0
          - 0.0
          - 0.0
        size:
          - 1.0
          - 1.0
          - 1.0
";
        let diagnostics = parse(source).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/ops/0/region"))
            .unwrap_or_else(|| panic!("the two-shape region is reported: {diagnostics:?}"));
        assert_eq!(d.code, STRUCTURE);
        assert!(d.message.contains("exactly one shape"), "{}", d.message);
    }

    #[test]
    fn a_key_from_another_region_shape_is_l1173() {
        let diagnostics = parse(&GLOWING_EYE.replace("        at:", "        from:")).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/ops/0/region/sphere/from"))
            .expect("a capsule key on a sphere is reported");
        assert_eq!(d.code, STRUCTURE);
        assert_eq!(
            d.help.as_deref(),
            Some("valid keys are: at, radius, feather")
        );
    }

    #[test]
    fn a_region_missing_its_radius_is_l1173() {
        let diagnostics = parse(&GLOWING_EYE.replace("        radius: 0.035\n", "")).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/ops/0/region/sphere"))
            .expect("the missing radius is reported");
        assert_eq!(d.code, STRUCTURE);
        assert!(d.message.contains("`radius`"), "{}", d.message);
    }

    #[test]
    fn a_non_positive_radius_is_l1174() {
        let doc = parse(&GLOWING_EYE.replace("radius: 0.035", "radius: 0.0")).expect("parses");
        let diagnostics = validate(&doc);
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/ops/0/region/sphere/radius"))
            .expect("the radius is reported");
        assert_eq!(d.code, DATA);
    }

    #[test]
    fn a_colour_outside_the_unit_range_is_l1174() {
        let doc = parse(&GLOWING_EYE.replace("      - 1.0\n", "      - 1.5\n")).expect("parses");
        let diagnostics = validate(&doc);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == DATA && d.path.as_deref() == Some("/ops/0/color/1")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn a_strength_outside_the_unit_range_is_l1174() {
        let doc = parse(&GLOWING_EYE.replace("strength: 1.0", "strength: 2.0")).expect("parses");
        assert!(
            validate(&doc)
                .iter()
                .any(|d| d.code == DATA && d.path.as_deref() == Some("/ops/0/strength"))
        );
    }

    #[test]
    fn a_negative_feather_is_l1174() {
        let doc = parse(&GLOWING_EYE.replace("feather: 0.012", "feather: -0.5")).expect("parses");
        assert!(
            validate(&doc).iter().any(
                |d| d.code == DATA && d.path.as_deref() == Some("/ops/0/region/sphere/feather")
            )
        );
    }

    #[test]
    fn a_zero_map_size_is_l1174() {
        let doc = parse(&GLOWING_EYE.replace("size: 1024", "size: 0")).expect("parses");
        assert!(
            validate(&doc)
                .iter()
                .any(|d| d.code == DATA && d.path.as_deref() == Some("/ops/0/size"))
        );
    }

    #[test]
    fn a_non_positive_box_extent_is_l1174() {
        let doc = MeshEditDocument {
            source: "mesh-0.mesh.yaml".into(),
            ops: vec![MeshEditOp::Paint {
                map: MeshEditMap::Emissive,
                size: None,
                color: [0.0, 0.0, 0.0],
                strength: None,
                region: MeshEditRegion {
                    shape: MeshEditRegionShape::Box {
                        at: [0.0; 3],
                        size: [1.0, -1.0, 1.0],
                    },
                    feather: None,
                    key: None,
                },
            }],
            ..Default::default()
        };
        assert!(
            validate(&doc)
                .iter()
                .any(|d| d.code == DATA && d.path.as_deref() == Some("/ops/0/region/box/size/1"))
        );
    }

    #[test]
    fn a_non_finite_number_is_l1174() {
        let doc = MeshEditDocument {
            source: "mesh-0.mesh.yaml".into(),
            ops: vec![MeshEditOp::Paint {
                map: MeshEditMap::Emissive,
                size: None,
                color: [0.0, 0.0, 0.0],
                strength: None,
                region: MeshEditRegion {
                    shape: MeshEditRegionShape::Sphere {
                        at: [0.0, f64::NAN, 0.0],
                        radius: 1.0,
                    },
                    feather: None,
                    key: None,
                },
            }],
            ..Default::default()
        };
        assert!(
            validate(&doc)
                .iter()
                .any(|d| d.code == DATA && d.path.as_deref() == Some("/ops/0/region/sphere/at/1"))
        );
    }

    #[test]
    fn an_empty_source_is_l1174() {
        let doc = MeshEditDocument::default();
        assert!(
            validate(&doc)
                .iter()
                .any(|d| d.code == DATA && d.path.as_deref() == Some("/source"))
        );
    }

    #[test]
    fn a_source_may_climb_but_not_name_a_device_or_drive() {
        for (source, refused) in [
            ("../m.mesh.yaml", false),
            ("../nul.mesh.yaml", true),
            ("C:/m.mesh.yaml", true),
            ("/m.mesh.yaml", true),
        ] {
            let doc = MeshEditDocument {
                source: source.to_string(),
                ..Default::default()
            };
            let hit = validate(&doc)
                .iter()
                .any(|d| d.code == DATA && d.path.as_deref() == Some("/source"));
            assert_eq!(hit, refused, "{source}");
        }
    }

    #[test]
    fn an_out_that_escapes_the_document_is_l1174() {
        for escape in [
            "../lit",
            "a/../../lit",
            "/abs/lit",
            "\\abs\\lit",
            "C:\\lit",
            "lit/C:/x",
            "lit\\C:\\x",
            "a:stream",
            "lit/.. /x",
            "lit/.../x",
            "lit/nul",
            "CON",
            "",
        ] {
            let doc = MeshEditDocument {
                source: "mesh-0.mesh.yaml".into(),
                out: Some(escape.to_string()),
                ..Default::default()
            };
            assert!(
                validate(&doc)
                    .iter()
                    .any(|d| d.code == DATA && d.path.as_deref() == Some("/out")),
                "`{escape}` must be refused"
            );
        }
        let doc = MeshEditDocument {
            source: "mesh-0.mesh.yaml".into(),
            out: Some("variants/idol-lit".into()),
            ..Default::default()
        };
        assert!(validate(&doc).is_empty(), "a nested directory is legal");
    }

    #[test]
    fn a_source_that_leaves_the_tree_is_l1174() {
        for source in ["C:/m.mesh.yaml", "/abs/m.mesh.yaml"] {
            let doc = MeshEditDocument {
                source: source.into(),
                ..Default::default()
            };
            assert!(
                validate(&doc)
                    .iter()
                    .any(|d| d.code == DATA && d.path.as_deref() == Some("/source")),
                "`{source}` must be refused"
            );
        }
        let doc = MeshEditDocument {
            source: "../meshes/m.mesh.yaml".into(),
            ..Default::default()
        };
        assert!(validate(&doc).is_empty(), "a `..` source is read-only");
    }

    /// One op painting the emissive map around `region` (already indented six spaces).
    fn with_region(region: &str) -> String {
        format!(
            "\
format: 1
kind: mesh-edit
source: mesh-0.mesh.yaml
ops:
  - op: paint
    map: emissive
    color:
      - 1.0
      - 0.0
      - 0.0
    region:
{region}"
        )
    }

    fn only_region(doc: &MeshEditDocument) -> &MeshEditRegion {
        let MeshEditOp::Paint { region, .. } = &doc.ops[0];
        region
    }

    #[test]
    fn a_three_point_capsule_round_trips() {
        let source = with_region(
            "      capsule:
        points:
          - - 0.0
            - 0.0
            - 0.0
          - - 1.0
            - 0.5
            - 0.0
          - - 2.0
            - 0.5
            - -1.25
        radius: 0.25
        feather: 0.1
",
        );
        let doc = parsed(&source);
        assert_eq!(
            only_region(&doc).shape,
            MeshEditRegionShape::Capsule {
                points: vec![[0.0, 0.0, 0.0], [1.0, 0.5, 0.0], [2.0, 0.5, -1.25]],
                radius: 0.25,
            }
        );
        assert_eq!(emit(&doc), source);
    }

    #[test]
    fn a_fill_round_trips_with_and_without_its_map() {
        let bare = with_region(
            "      fill:
        at:
          - 0.1
          - 0.2
          - 0.3
        tolerance: 0.15
        max_radius: 0.05
",
        );
        let doc = parsed(&bare);
        let MeshEditRegionShape::Fill { map, .. } = &only_region(&doc).shape else {
            panic!("a fill region");
        };
        assert_eq!(*map, None, "an absent map stays absent");
        assert_eq!(emit(&doc), bare);

        let mapped = with_region(
            "      fill:
        at:
          - 0.1
          - 0.2
          - 0.3
        tolerance: 0.15
        max_radius: 0.05
        map: emissive
        feather: 0.01
",
        );
        let doc = parsed(&mapped);
        let MeshEditRegionShape::Fill { map, .. } = &only_region(&doc).shape else {
            panic!("a fill region");
        };
        assert_eq!(*map, Some(MeshEditMap::Emissive));
        assert_eq!(emit(&doc), mapped);
    }

    #[test]
    fn a_key_round_trips_inside_a_sphere_and_a_fill() {
        let key = "      key:
        map: base_color
        color:
          - 0.1
          - 0.1
          - 0.1
        tolerance: 0.15
";
        let sphere = with_region(&format!(
            "      sphere:
        at:
          - 0.0
          - 0.0
          - 0.0
        radius: 0.5
        feather: 0.1
{key}"
        ));
        let doc = parsed(&sphere);
        assert_eq!(
            only_region(&doc).key,
            Some(MeshEditKey {
                map: MeshEditMap::BaseColor,
                color: [0.1, 0.1, 0.1],
                tolerance: 0.15,
                color_feather: None,
            })
        );
        assert_eq!(emit(&doc), sphere);

        let fill = with_region(&format!(
            "      fill:
        at:
          - 0.0
          - 0.0
          - 0.0
        tolerance: 0.2
        max_radius: 0.5
{key}"
        ));
        assert_eq!(emit(&parsed(&fill)), fill);
    }

    /// `color_feather` sits after `tolerance` in a key and after `feather` in a fill, and survives
    /// the canonical round trip in both.
    #[test]
    fn a_color_feather_round_trips_on_a_key_and_a_fill() {
        let keyed = with_region(
            "      sphere:
        at:
          - 0.0
          - 0.0
          - 0.0
        radius: 0.5
      key:
        map: base_color
        color:
          - 0.1
          - 0.1
          - 0.1
        tolerance: 0.15
        color_feather: 0.05
",
        );
        let doc = parsed(&keyed);
        let key = only_region(&doc).key.as_ref().expect("a key");
        assert_eq!(key.color_feather, Some(0.05));
        assert_eq!(emit(&doc), keyed);

        let fill = with_region(
            "      fill:
        at:
          - 0.1
          - 0.2
          - 0.3
        tolerance: 0.15
        max_radius: 0.05
        map: emissive
        feather: 0.01
        color_feather: 0.1
",
        );
        let doc = parsed(&fill);
        let MeshEditRegionShape::Fill { color_feather, .. } = &only_region(&doc).shape else {
            panic!("a fill region");
        };
        assert_eq!(*color_feather, Some(0.1));
        assert_eq!(emit(&doc), fill);
    }

    #[test]
    fn a_negative_or_non_finite_color_feather_is_l1174() {
        let keyed = with_region(
            "      sphere:
        at:
          - 0.0
          - 0.0
          - 0.0
        radius: 0.5
      key:
        map: base_color
        color:
          - 0.1
          - 0.1
          - 0.1
        tolerance: 0.15
        color_feather: -0.05
",
        );
        let diagnostics = validate(&parse(&keyed).expect("parses"));
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/ops/0/region/key/color_feather"))
            .unwrap_or_else(|| panic!("{diagnostics:?}"));
        assert_eq!(d.code, DATA);
        assert!(d.message.contains("cannot be negative"), "{}", d.message);

        let fill = with_region(
            "      fill:
        at:
          - 0.0
          - 0.0
          - 0.0
        tolerance: 0.1
        max_radius: 0.5
        color_feather: 0.1
",
        );
        let mut doc = parse(&fill).expect("parses");
        let MeshEditOp::Paint { region, .. } = &mut doc.ops[0];
        let MeshEditRegionShape::Fill { color_feather, .. } = &mut region.shape else {
            panic!("a fill region");
        };
        *color_feather = Some(f64::NAN);
        let diagnostics = validate(&doc);
        assert!(
            diagnostics.iter().any(|d| d.code == DATA
                && d.path.as_deref() == Some("/ops/0/region/fill/color_feather")
                && d.message.contains("not a finite number")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn an_unknown_key_field_lists_color_feather() {
        let source = with_region(
            "      sphere:
        at:
          - 0.0
          - 0.0
          - 0.0
        radius: 0.5
      key:
        map: base_color
        color:
          - 0.1
          - 0.1
          - 0.1
        tolerance: 0.15
        softness: 0.05
",
        );
        let diagnostics = parse(&source).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/ops/0/region/key/softness"))
            .unwrap_or_else(|| panic!("{diagnostics:?}"));
        assert_eq!(d.code, STRUCTURE);
        assert!(
            d.help
                .as_deref()
                .unwrap_or("")
                .contains("map, color, tolerance, color_feather"),
            "{:?}",
            d.help
        );
    }

    /// A key may read any of the five slots, while a paint op still writes only `emissive`.
    #[test]
    fn a_key_may_name_any_slot_while_a_paint_may_not() {
        let source = with_region(
            "      sphere:
        at:
          - 0.0
          - 0.0
          - 0.0
        radius: 0.5
      key:
        map: base_color
        color:
          - 0.0
          - 0.5
          - 1.0
        tolerance: 0.0
",
        );
        parsed(&source);
        let diagnostics = parse(&source.replace("map: emissive", "map: base_color")).unwrap_err();
        assert!(diagnostics.iter().any(
            |d| d.path.as_deref() == Some("/ops/0/map") && d.message.contains("does not paint")
        ));
    }

    #[test]
    fn the_old_from_to_capsule_is_one_l1173_naming_points() {
        let source = with_region(
            "      capsule:
        from:
          - 0.0
          - 0.0
          - 0.0
        to:
          - 0.0
          - 1.0
          - 0.0
        radius: 0.25
",
        );
        let diagnostics = parse(&source).unwrap_err();
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let d = &diagnostics[0];
        assert_eq!(d.code, STRUCTURE);
        assert_eq!(d.path.as_deref(), Some("/ops/0/region/capsule/from"));
        assert!(d.message.contains("points"), "{}", d.message);
        assert!(d.help.as_deref().unwrap_or("").contains("points:"));
    }

    #[test]
    fn a_one_point_capsule_is_l1174() {
        let source = with_region(
            "      capsule:
        points:
          - - 0.0
            - 0.0
            - 0.0
        radius: 0.25
",
        );
        let doc = parse(&source).expect("parses");
        assert!(
            validate(&doc).iter().any(
                |d| d.code == DATA && d.path.as_deref() == Some("/ops/0/region/capsule/points")
            )
        );
    }

    #[test]
    fn a_negative_key_tolerance_or_colour_is_l1174() {
        let region = "      sphere:
        at:
          - 0.0
          - 0.0
          - 0.0
        radius: 0.5
      key:
        map: base_color
        color:
          - 0.1
          - 1.5
          - 0.1
        tolerance: -0.1
";
        let doc = parse(&with_region(region)).expect("parses");
        let diagnostics = validate(&doc);
        for at in ["/ops/0/region/key/tolerance", "/ops/0/region/key/color/1"] {
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.code == DATA && d.path.as_deref() == Some(at)),
                "{at}: {diagnostics:?}"
            );
        }
    }

    #[test]
    fn a_fill_needs_a_non_negative_tolerance_and_a_positive_radius() {
        let region = "      fill:
        at:
          - 0.0
          - 0.0
          - 0.0
        tolerance: -1.0
        max_radius: 0.0
";
        let doc = parse(&with_region(region)).expect("parses");
        let diagnostics = validate(&doc);
        for at in [
            "/ops/0/region/fill/tolerance",
            "/ops/0/region/fill/max_radius",
        ] {
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.code == DATA && d.path.as_deref() == Some(at)),
                "{at}: {diagnostics:?}"
            );
        }
    }

    #[test]
    fn a_shape_plus_key_plus_second_shape_is_l1173() {
        let source = with_region(
            "      sphere:
        at:
          - 0.0
          - 0.0
          - 0.0
        radius: 1.0
      key:
        map: base_color
        color:
          - 0.0
          - 0.0
          - 0.0
        tolerance: 0.1
      box:
        at:
          - 0.0
          - 0.0
          - 0.0
        size:
          - 1.0
          - 1.0
          - 1.0
",
        );
        let diagnostics = parse(&source).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/ops/0/region"))
            .unwrap_or_else(|| panic!("the two-shape region is reported: {diagnostics:?}"));
        assert_eq!(d.code, STRUCTURE);
        assert!(d.message.contains("exactly one shape"), "{}", d.message);
    }

    #[test]
    fn a_key_missing_its_tolerance_is_l1173() {
        let source = with_region(
            "      sphere:
        at:
          - 0.0
          - 0.0
          - 0.0
        radius: 1.0
      key:
        map: base_color
        color:
          - 0.0
          - 0.0
          - 0.0
",
        );
        let diagnostics = parse(&source).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code == STRUCTURE
                && d.path.as_deref() == Some("/ops/0/region/key")
                && d.message.contains("`tolerance`")),
            "{diagnostics:?}"
        );
    }

    /// The strict-subset reader takes a block sequence of block sequences, which is how `points`
    /// is written.
    #[test]
    fn nested_block_sequences_parse() {
        let root = yaml::parse_document("points:\n  - - 1\n    - 2\n  - - 3\n    - 4\n")
            .expect("parses")
            .expect("a document");
        let NodeValue::Mapping(entries) = root.value else {
            panic!("a mapping");
        };
        let NodeValue::Sequence(outer) = &entries[0].node.value else {
            panic!("a sequence");
        };
        assert_eq!(outer.len(), 2);
        assert!(
            outer
                .iter()
                .all(|item| matches!(&item.value, NodeValue::Sequence(inner) if inner.len() == 2))
        );
    }

    /// `ludion explain L1173` … `L1178` must print: every code this arc reserved is registered in
    /// the one table, and the three the derive and the pick own are named here so the registry's
    /// reachability check sees them.
    #[test]
    fn every_mesh_edit_code_explains() {
        for reserved in [
            code::MESH_EDIT_STRUCTURE,
            code::MESH_EDIT_DATA,
            code::MESH_EDIT_PICK_UNRESOLVED,
            code::MESH_EDIT_INAPPLICABLE,
            code::MESH_EDIT_OP_PAINTED_NOTHING,
            code::MESH_EDIT_MATCH_FAILED,
        ] {
            let entry = ludion_core::codes::explain(reserved.as_str())
                .unwrap_or_else(|| panic!("{reserved} is registered"));
            assert!(!entry.explanation.is_empty());
        }
    }
}
