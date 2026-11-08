//! Sculpt model documents: `sculpts/*.sculpt.yaml` — one implicit-surface model per document.
//!
//! A sculpt is a list of named primitives (`sphere`, `capsule`, `ellipsoid`, `box`, `torus`, `cone`,
//! plus the `voxel` and `mesh` parts that read another document's shape as their field)
//! with real dimensions, folded in file order by `union` / `subtract` / `intersect` with a per-part
//! `blend` radius, optionally mirrored or repeated, each carrying a palette `material` and a `bone`
//! of the document's `bones:` tree. This module owns the text: structure (`L1156`), data consistency
//! (`L1157`), the bone tree (`L1160`), the canonical writer and the numeric override domain. Meshing
//! lives outside this crate, so it never names a renderer type.
//!
//! Two things the signatures do not say:
//!
//! - **A key that belongs to another shape is `L1157`, raised by the parse.** A part's `shape` decides
//!   its key set, so it is read first; a `radii` on a `box` is a data defect listing the `box`'s keys,
//!   while a key no part has at all (`radis`) is the structure code with a nearest match. [`SculptShape`]
//!   carries only its own shape's fields, so the misfit cannot survive to [`validate`].
//! - **Canonical means minimal for part defaults** (the sprite rule, not the voxel one): `at` at the
//!   origin, `op: union`, `blend: 0` and `sharp: false` are not written, and `round: 0` on a box is
//!   not either. Defaults compare bitwise, so an authored `-0.0` survives. `resolution` and
//!   `triangles` are always written. Byte-idempotence holds for every document; a document spelling a
//!   default out loud re-emits shorter once.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::paths::sibling_refusal;
use ludion_core::suggest;
use serde_json::Value;

use crate::envelope;
use crate::fields::parse_number;
use crate::overrides::{AppliedOverride, SceneOverride, unescape};
use crate::scalar::{format_float, scalar_string};
use crate::voxel::{self, PaletteEntry};
use crate::yaml::{self, MapEntry, Node, NodeValue, pointer_token};

/// The sculpt document format version this crate reads and writes.
pub const SCULPT_FORMAT_VERSION: u64 = 1;

/// `resolution` when the document omits it: grid cells per world unit.
pub const SCULPT_DEFAULT_RESOLUTION: u32 = 48;

/// The smallest `resolution` [`crate::validate_sculpt`] accepts.
pub const SCULPT_MIN_RESOLUTION: u32 = 4;

/// The largest `resolution` [`crate::validate_sculpt`] accepts.
pub const SCULPT_MAX_RESOLUTION: u32 = 128;

/// The largest `repeat.count` [`crate::validate_sculpt`] accepts — each copy is a full copy of the
/// part, and the lattice is built before any cell ceiling applies.
pub const SCULPT_MAX_REPEAT: u32 = 64;

/// `triangles` when the document omits it: the decimation target.
pub const SCULPT_DEFAULT_TRIANGLES: u32 = 5000;

/// The smallest `triangles` [`crate::validate_sculpt`] accepts — a tetrahedron-ish floor below which
/// decimation cannot keep a closed surface.
pub const SCULPT_MIN_TRIANGLES: u32 = 12;

const STRUCTURE: ludion_core::diagnostics::DiagnosticCode = code::SCULPT_DOCUMENT_STRUCTURE;
const DATA: ludion_core::diagnostics::DiagnosticCode = code::SCULPT_DOCUMENT_DATA;
const BONES: ludion_core::diagnostics::DiagnosticCode = code::SCULPT_BONE_TREE;

const SCULPT_KEYS: [&str; 10] = [
    "format",
    "kind",
    "name",
    "resolution",
    "triangles",
    "budget_exempt",
    "rim",
    "palette",
    "bones",
    "parts",
];

const BONE_KEYS: [&str; 4] = ["name", "parent", "head", "tail"];

const REPEAT_KEYS: [&str; 2] = ["count", "step"];

const DEFORM_KEYS: [&str; 3] = ["bend", "twist", "taper"];

const TURN_KEYS: [&str; 2] = ["axis", "degrees"];

const TAPER_KEYS: [&str; 3] = ["axis", "from", "to"];

const NOISE_KEYS: [&str; 3] = ["amplitude", "scale", "seed"];

const SHAPE_NAMES: [&str; 8] = [
    "sphere",
    "capsule",
    "ellipsoid",
    "box",
    "torus",
    "cone",
    "voxel",
    "mesh",
];

const OP_NAMES: [&str; 3] = ["union", "subtract", "intersect"];

const AXIS_NAMES: [&str; 3] = ["x", "y", "z"];

/// Every key a part may carry under some shape, the vocabulary an unknown key is matched against.
const PART_VOCABULARY: [&str; 22] = [
    "name", "shape", "radius", "radii", "size", "round", "tube", "height", "from", "to", "path",
    "at", "rotation", "op", "blend", "mirror", "repeat", "material", "bone", "sharp", "deform",
    "noise",
];

/// The keys whose presence depends on the part's shape.
const SHAPE_FIELD_KEYS: [&str; 9] = [
    "radius", "radii", "size", "round", "tube", "height", "from", "to", "path",
];

/// The domain of the numeric override pointers, spelled out in every `L4008` this module raises.
const SCULPT_POINTER_SHAPES: &str = "a sculpt override addresses one numeric field: `/resolution`, \
     `/triangles`, `/palette/<glyph>/color/<0..3>`, `/palette/<glyph>/emissive|metallic|roughness`, \
     `/parts/<i>/blend`, `/parts/<i>/at/<0..2>`, `/parts/<i>/rotation/<0..2>`, \
     `/parts/<i>/repeat/count`, `/parts/<i>/repeat/step/<0..2>`, or one of the part shape's own \
     fields (`radius`, `tube`, `height`, `round`, `radii/<0..2>`, `size/<0..2>`, `from/<0..2>`, \
     `to/<0..2>`)";

/// A parsed, structurally valid sculpt model document.
#[derive(Clone, Debug, PartialEq)]
pub struct SculptDocument {
    /// Optional human-readable model name.
    pub name: Option<String>,
    /// Grid cells per world unit, [`SCULPT_MIN_RESOLUTION`]`..=`[`SCULPT_MAX_RESOLUTION`].
    pub resolution: u32,
    /// Decimation target triangle count, `>=` [`SCULPT_MIN_TRIANGLES`].
    pub triangles: u32,
    /// `Some(true)` opts the meshed result out of the vertex-budget warning; `Some(false)` is an
    /// `L1157` (the key only means something as `true`).
    pub budget_exempt: Option<bool>,
    /// One rim highlight for the whole model. It wins over the entity's `Mesh.rim` and any
    /// inherited one. Ranges are `L1157`.
    pub rim: Option<crate::rim::DocRim>,
    /// Palette entries in sorted-glyph order — the voxel palette shape, one-glyph keys.
    pub palette: Vec<PaletteEntry>,
    /// The bone tree in authored order; `bones[0]` is the root.
    pub bones: Vec<SculptBone>,
    /// The parts, in evaluation (file) order.
    pub parts: Vec<SculptPart>,
}

impl Default for SculptDocument {
    fn default() -> Self {
        Self {
            name: None,
            resolution: SCULPT_DEFAULT_RESOLUTION,
            triangles: SCULPT_DEFAULT_TRIANGLES,
            budget_exempt: None,
            rim: None,
            palette: Vec::new(),
            bones: Vec::new(),
            parts: Vec::new(),
        }
    }
}

impl SculptDocument {
    /// The palette entry keyed by exactly `glyph`, or `None`.
    pub fn palette_entry(&self, glyph: &str) -> Option<&PaletteEntry> {
        self.palette.iter().find(|entry| entry.glyph == glyph)
    }

    /// The index into [`SculptDocument::bones`] of the bone `part` is parented to — the root (`0`)
    /// when the part names none. `None` when the named bone does not exist, or there are no bones
    /// (both `L1160` on a validated document).
    pub fn bone_index(&self, part: &SculptPart) -> Option<usize> {
        match &part.bone {
            Some(name) => self.bones.iter().position(|bone| bone.name == *name),
            None => (!self.bones.is_empty()).then_some(0),
        }
    }
}

/// One entry of a sculpt's `bones:` tree.
#[derive(Clone, Debug, PartialEq)]
pub struct SculptBone {
    /// The bone's unique name, what a part's `bone` and a child's `parent` refer to.
    pub name: String,
    /// The parent bone's name; `None` only for the first entry, the root.
    pub parent: Option<String>,
    /// Where the bone starts, in the document's coordinates; authored with [`SculptBone::tail`] or
    /// not at all (`L1160` for one without the other). A bone with no position is a pure grouping
    /// node.
    pub head: Option<[f64; 3]>,
    /// Where the bone ends, in the document's coordinates — the head-to-tail segment is the bone's
    /// axis and length.
    pub tail: Option<[f64; 3]>,
}

/// One primitive of a sculpt, in the fold order of [`SculptDocument::parts`].
#[derive(Clone, Debug, PartialEq)]
pub struct SculptPart {
    /// The part's unique name.
    pub name: String,
    /// The primitive and its own dimensions, in world units.
    pub shape: SculptShape,
    /// The primitive's center. Always `[0, 0, 0]` on a capsule, which is placed by `from`/`to`.
    pub at: [f64; 3],
    /// Euler XYZ rotation in degrees; `None` means unrotated.
    pub rotation: Option<[f64; 3]>,
    /// How the part combines with the field folded so far.
    pub op: SculptOp,
    /// Smooth-combine radius in world units, `>= 0`; `0` is the exact `min`/`max`.
    pub blend: f64,
    /// Duplicates the part across the model's origin plane normal to this axis, before its op runs.
    pub mirror: Option<SculptAxis>,
    /// Copies the part along an arithmetic march, all copies one op.
    pub repeat: Option<SculptRepeat>,
    /// The palette glyph the part's surface takes; required on a `union` part and refused on a
    /// `subtract` or `intersect`, which contribute no material.
    pub material: Option<String>,
    /// The bone the part is parented to; `None` means the root.
    pub bone: Option<String>,
    /// Marks the part for the crease-angle normal split.
    pub sharp: bool,
    /// Domain deformations bending, twisting and tapering the part's own space before its
    /// primitive is measured; `None` leaves the primitive as authored.
    pub deform: Option<SculptDeform>,
    /// A seeded value-noise displacement of the part's surface; `None` leaves it smooth.
    pub noise: Option<SculptNoise>,
}

/// A part's `deform`: the domain deformations applied to the sample point, always in the order
/// `bend`, `twist`, `taper` whatever order the document writes them in. At least one must be
/// authored — an empty `deform` is `L1157`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SculptDeform {
    /// Bends the part in the plane of `axis` and the next axis (`x`→`y`, `y`→`z`, `z`→`x`).
    pub bend: Option<SculptTurn>,
    /// Twists the part around `axis`.
    pub twist: Option<SculptTurn>,
    /// Narrows or widens the part's cross-section along `axis`.
    pub taper: Option<SculptTaper>,
}

/// A `bend` or a `twist`: a rotation of `degrees` per world unit travelled along `axis`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SculptTurn {
    /// The axis the rotation angle grows along.
    pub axis: SculptAxis,
    /// Degrees of rotation per world unit along `axis`; finite, either sign.
    pub degrees: f64,
}

/// A `taper`: the cross-section perpendicular to `axis` is scaled from `from` at the part's low end
/// to `to` at its high end, measured over the part's own undeformed extent along that axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SculptTaper {
    /// The axis the scale runs along.
    pub axis: SculptAxis,
    /// The cross-section scale at the low end of the part's extent, `> 0`; `1` keeps it.
    pub from: f64,
    /// The cross-section scale at the high end, `> 0`.
    pub to: f64,
}

/// A part's `noise`: a seeded value-noise displacement added to the part's distance, the cheap way
/// to break a primitive's perfect surface into skin, bark or rock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SculptNoise {
    /// Peak displacement in world units, `>= 0`; the surface moves within `±amplitude`.
    pub amplitude: f64,
    /// The noise lattice's spacing in world units, `> 0` — the feature size. Below two grid cells
    /// the lattice out-runs the sampling and the displacement reads as sampling error.
    pub scale: f64,
    /// The seed the noise lattice is drawn from; the document's one `u64`, the sprite `seed` rule.
    /// A different seed is a different surface, the same seed always the same one.
    pub seed: u64,
}

/// A sculpt primitive and its shape-specific fields, in world units.
#[derive(Clone, Debug, PartialEq)]
pub enum SculptShape {
    /// A sphere centered on the part's `at`.
    Sphere {
        /// Radius, `> 0`.
        radius: f64,
    },
    /// A capsule: the segment `from`–`to` swept by `radius`.
    Capsule {
        /// Radius, `> 0`.
        radius: f64,
        /// One end of the core segment.
        from: [f64; 3],
        /// The other end of the core segment.
        to: [f64; 3],
    },
    /// An axis-aligned (before `rotation`) ellipsoid centered on `at`.
    Ellipsoid {
        /// Semi-axis lengths, each `> 0`.
        radii: [f64; 3],
    },
    /// A box centered on `at`, optionally with rounded edges.
    Box {
        /// Full extents, each `> 0`.
        size: [f64; 3],
        /// Edge rounding radius, `0..=` half the smallest `size` component; `0` is sharp.
        round: f64,
    },
    /// A torus in the XZ plane (before `rotation`), centered on `at`.
    Torus {
        /// Distance from the center to the tube's center line, `> 0`.
        radius: f64,
        /// The tube's radius, `> 0`.
        tube: f64,
    },
    /// A cone standing on its base disc at `at`, apex up.
    Cone {
        /// Base radius, `> 0`.
        radius: f64,
        /// Base-to-apex height, `> 0`.
        height: f64,
    },
    /// A voxel model's own occupancy, as the exact signed Euclidean distance transform of its grid
    /// — so existing voxel content gains blending, bevelling and carving.
    ///
    /// The model keeps its own bottom-center origin scaled by `size`: `at` places the grid's bottom
    /// center, the solid spans `±size · width / 2` on x, `[0, size · height]` on y and
    /// `±size · depth / 2` on z, exactly as a `Mesh.shape: voxel` entity is placed.
    Voxel {
        /// The `.voxel.yaml` document this part reads its occupancy from, **relative to the sculpt
        /// document's own directory** — the rule a `kind: assembly` reads its `file:` parts by,
        /// through the same `ludion_assets` seam a scene's `Mesh.shape: voxel` path takes from its
        /// content root. Non-empty.
        path: String,
        /// World units per voxel, `> 0`.
        size: f64,
    },
    /// An imported mesh's own triangles, as the signed distance to that surface — so a generated or
    /// modelled mesh blends, bevels and carves like any other part.
    ///
    /// The part's local frame is the mesh document's own coordinates, so `at` places the mesh
    /// exactly where a `Mesh.shape: mesh` entity's `Transform` would.
    Mesh {
        /// The `.mesh.yaml` document this part reads its triangles from, **relative to the sculpt
        /// document's own directory** — the [`SculptShape::Voxel`] rule exactly. Non-empty.
        path: String,
        /// World units per document unit — the uniform scale the mesh's own coordinates are read
        /// at, `> 0`. Omitted means `1`: an imported mesh is already in world units, and a
        /// generated one normalized to a unit box is fitted with this.
        size: Option<f64>,
    },
}

impl SculptShape {
    /// The shape's document spelling (`sphere`, `capsule`, ...).
    pub fn name(&self) -> &'static str {
        match self {
            SculptShape::Sphere { .. } => "sphere",
            SculptShape::Capsule { .. } => "capsule",
            SculptShape::Ellipsoid { .. } => "ellipsoid",
            SculptShape::Box { .. } => "box",
            SculptShape::Torus { .. } => "torus",
            SculptShape::Cone { .. } => "cone",
            SculptShape::Voxel { .. } => "voxel",
            SculptShape::Mesh { .. } => "mesh",
        }
    }

    /// The shape's own keys, in canonical order.
    pub fn keys(&self) -> &'static [&'static str] {
        shape_keys(self.name())
    }
}

/// How a part combines with the field folded so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SculptOp {
    /// Adds the part's volume; the default.
    #[default]
    Union,
    /// Carves the part's volume away.
    Subtract,
    /// Keeps only the volume inside both.
    Intersect,
}

impl SculptOp {
    /// The op's document spelling.
    pub fn name(self) -> &'static str {
        match self {
            SculptOp::Union => "union",
            SculptOp::Subtract => "subtract",
            SculptOp::Intersect => "intersect",
        }
    }
}

/// A model axis, naming the mirror plane normal to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SculptAxis {
    /// Mirror across the `x = 0` plane.
    X,
    /// Mirror across the `y = 0` plane.
    Y,
    /// Mirror across the `z = 0` plane.
    Z,
}

impl SculptAxis {
    /// The axis's document spelling.
    pub fn name(self) -> &'static str {
        match self {
            SculptAxis::X => "x",
            SculptAxis::Y => "y",
            SculptAxis::Z => "z",
        }
    }
}

/// A part's `repeat`: `count` copies, each translated by `step` from the previous one.
#[derive(Clone, Debug, PartialEq)]
pub struct SculptRepeat {
    /// The number of copies, the original included, `>= 2`.
    pub count: u32,
    /// The translation between consecutive copies.
    pub step: [f64; 3],
}

fn shape_keys(shape: &str) -> &'static [&'static str] {
    match shape {
        "sphere" => &["radius"],
        "capsule" => &["radius", "from", "to"],
        "ellipsoid" => &["radii"],
        "box" => &["size", "round"],
        "torus" => &["radius", "tube"],
        "cone" => &["radius", "height"],
        "voxel" | "mesh" => &["path", "size"],
        _ => &[],
    }
}

/// Whether a part of `shape` takes `key`; only shape fields and a capsule's `at` can be refused.
fn shape_accepts(shape: &str, key: &str) -> bool {
    if key == "at" {
        return shape != "capsule";
    }
    !SHAPE_FIELD_KEYS.contains(&key) || shape_keys(shape).contains(&key)
}

// --- Parse (structure, `L1156`; misfit shape keys, `L1157`) -------------------------------------

/// Parses strict-subset YAML into a [`SculptDocument`], checking structure only.
pub(crate) fn parse(source: &str) -> Result<SculptDocument, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![
            Diagnostic::new(
                STRUCTURE,
                "the sculpt document is empty; a sculpt needs at least `format: 1`, \
                 `kind: sculpt`, `palette`, `bones`, and `parts`",
            )
            .with_path("")
            .with_help(
                "a minimal sculpt is:\n  format: 1\n  kind: sculpt\n  palette:\n    S:\n      \
                 color: ...\n  bones:\n    - name: root\n  parts:\n    - name: ball\n      \
                 shape: sphere\n      radius: 0.5\n      material: S",
            ),
        ]);
    };
    let NodeValue::Mapping(entries) = root.value else {
        return Err(vec![
            Diagnostic::new(
                STRUCTURE,
                format!(
                    "the sculpt document root must be a mapping, found {} (line {})",
                    root.value.type_phrase(),
                    root.line
                ),
            )
            .with_path("")
            .with_help("start the file with the top-level keys `format:` and `kind: sculpt`"),
        ]);
    };

    let mut diagnostics = Vec::new();
    let mut doc = SculptDocument::default();
    let mut seen: Vec<&str> = Vec::new();

    for entry in &entries {
        seen.push(entry.key.as_str());
        match entry.key.as_str() {
            "format" => envelope::check_envelope(
                entry,
                STRUCTURE,
                STRUCTURE,
                SCULPT_FORMAT_VERSION,
                "the sculpt document",
                "sculpt",
                "sculpt",
                &mut diagnostics,
            ),
            "kind" => envelope::check_kind(
                entry,
                STRUCTURE,
                "sculpt",
                "a sculpt model document",
                &mut diagnostics,
            ),
            "name" => {
                doc.name = envelope::expect_string(
                    &entry.node,
                    "/name",
                    "the sculpt name",
                    STRUCTURE,
                    &mut diagnostics,
                );
            }
            "resolution" => {
                if let Some(value) =
                    parse_u32(&entry.node, "/resolution", "resolution", &mut diagnostics)
                {
                    doc.resolution = value;
                }
            }
            "triangles" => {
                if let Some(value) =
                    parse_u32(&entry.node, "/triangles", "triangles", &mut diagnostics)
                {
                    doc.triangles = value;
                }
            }
            "budget_exempt" => {
                doc.budget_exempt = parse_bool(
                    &entry.node,
                    "/budget_exempt",
                    "budget_exempt",
                    &mut diagnostics,
                );
            }
            "rim" => {
                doc.rim = crate::rim::parse_rim(&entry.node, "/rim", STRUCTURE, &mut diagnostics);
            }
            "palette" => doc.palette = voxel::parse_palette(entry, STRUCTURE, &mut diagnostics),
            "bones" => doc.bones = parse_bones(&entry.node, &mut diagnostics),
            "parts" => doc.parts = parse_parts(&entry.node, &mut diagnostics),
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "",
                "a sculpt document",
                SCULPT_KEYS,
                STRUCTURE,
            )),
        }
    }

    for (key, help) in [
        ("format", "add `format: 1` as the first line"),
        ("kind", "add `kind: sculpt`"),
        (
            "palette",
            "add a `palette:` of one-glyph keys, each with a `color`",
        ),
        (
            "bones",
            "add the bone tree; an unrigged model has one bone:\n  bones:\n    - name: root",
        ),
        ("parts", "add a `parts:` list of named primitives"),
    ] {
        if !seen.contains(&key) {
            diagnostics.push(envelope::missing_key(
                key,
                help,
                STRUCTURE,
                "sculpt document",
            ));
        }
    }

    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

fn parse_bones(node: &Node, diagnostics: &mut Vec<Diagnostic>) -> Vec<SculptBone> {
    let Some(items) = expect_sequence(node, "/bones", "bones", diagnostics) else {
        return Vec::new();
    };
    let mut bones = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let pointer = format!("/bones/{index}");
        let Some(entries) = expect_mapping(item, &pointer, "a bone", diagnostics) else {
            continue;
        };
        let mut name = None;
        let mut name_seen = false;
        let mut parent = None;
        let mut head = None;
        let mut tail = None;
        for entry in entries {
            match entry.key.as_str() {
                "name" => {
                    name_seen = true;
                    name = envelope::expect_string(
                        &entry.node,
                        &format!("{pointer}/name"),
                        "a bone `name`",
                        STRUCTURE,
                        diagnostics,
                    );
                }
                "parent" => {
                    parent = envelope::expect_string(
                        &entry.node,
                        &format!("{pointer}/parent"),
                        "a bone `parent`",
                        STRUCTURE,
                        diagnostics,
                    );
                }
                "head" => {
                    head = parse_vec3(&entry.node, &format!("{pointer}/head"), "head", diagnostics);
                }
                "tail" => {
                    tail = parse_vec3(&entry.node, &format!("{pointer}/tail"), "tail", diagnostics);
                }
                unknown => diagnostics.push(envelope::unknown_key(
                    unknown, entry, &pointer, "a bone", BONE_KEYS, STRUCTURE,
                )),
            }
        }
        if !name_seen {
            diagnostics.push(missing_nested(&pointer, "a bone", "name"));
        }
        if let Some(name) = name {
            bones.push(SculptBone {
                name,
                parent,
                head,
                tail,
            });
        }
    }
    bones
}

fn parse_parts(node: &Node, diagnostics: &mut Vec<Diagnostic>) -> Vec<SculptPart> {
    let Some(items) = expect_sequence(node, "/parts", "parts", diagnostics) else {
        return Vec::new();
    };
    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| parse_part(item, index, diagnostics))
        .collect()
}

/// The shape-field values one part authored, before the shape decides which it needs.
#[derive(Default)]
struct ShapeFields {
    radius: Option<f64>,
    radii: Option<[f64; 3]>,
    size: Option<[f64; 3]>,
    round: Option<f64>,
    tube: Option<f64>,
    height: Option<f64>,
    from: Option<[f64; 3]>,
    to: Option<[f64; 3]>,
    path: Option<String>,
    /// A `voxel` or `mesh` part's `size` — one scalar, where every other shape's `size` is the three
    /// full extents of a box.
    scalar_size: Option<f64>,
}

fn parse_part(node: &Node, index: usize, diagnostics: &mut Vec<Diagnostic>) -> Option<SculptPart> {
    let pointer = format!("/parts/{index}");
    let entries = expect_mapping(node, &pointer, "a part", diagnostics)?;

    // `shape` decides the whole key set, so it is read before any other key.
    let shape = match entries.iter().find(|entry| entry.key == "shape") {
        Some(entry) => parse_keyword(
            &entry.node,
            &format!("{pointer}/shape"),
            "shape",
            &SHAPE_NAMES,
            diagnostics,
        ),
        None => {
            diagnostics.push(missing_nested(&pointer, "a part", "shape"));
            None
        }
    };

    let mut name = None;
    let mut fields = ShapeFields::default();
    let mut at = [0.0; 3];
    let mut rotation = None;
    let mut op = SculptOp::Union;
    let mut blend = 0.0;
    let mut mirror = None;
    let mut repeat = None;
    let mut material = None;
    let mut bone = None;
    let mut sharp = false;
    let mut deform = None;
    let mut noise = None;

    for entry in entries {
        let key = entry.key.as_str();
        let key_pointer = format!("{pointer}/{}", pointer_token(key));
        if let Some(shape) = shape
            && !shape_accepts(shape, key)
        {
            diagnostics.push(misfit_key(shape, key, entry, &key_pointer));
            continue;
        }
        let node = &entry.node;
        match key {
            "name" => {
                name = envelope::expect_string(
                    node,
                    &key_pointer,
                    "a part `name`",
                    STRUCTURE,
                    diagnostics,
                );
            }
            "shape" => {}
            "radius" => {
                fields.radius = parse_number(node, &key_pointer, key, STRUCTURE, diagnostics)
            }
            "radii" => fields.radii = parse_vec3(node, &key_pointer, key, diagnostics),
            "size" if matches!(shape, Some("voxel") | Some("mesh")) => {
                fields.scalar_size = parse_number(node, &key_pointer, key, STRUCTURE, diagnostics);
            }
            "size" => fields.size = parse_vec3(node, &key_pointer, key, diagnostics),
            "path" => {
                fields.path = envelope::expect_string(
                    node,
                    &key_pointer,
                    "a part `path`",
                    STRUCTURE,
                    diagnostics,
                );
            }
            "round" => fields.round = parse_number(node, &key_pointer, key, STRUCTURE, diagnostics),
            "tube" => fields.tube = parse_number(node, &key_pointer, key, STRUCTURE, diagnostics),
            "height" => {
                fields.height = parse_number(node, &key_pointer, key, STRUCTURE, diagnostics)
            }
            "from" => fields.from = parse_vec3(node, &key_pointer, key, diagnostics),
            "to" => fields.to = parse_vec3(node, &key_pointer, key, diagnostics),
            "at" => {
                if let Some(value) = parse_vec3(node, &key_pointer, key, diagnostics) {
                    at = value;
                }
            }
            "rotation" => rotation = parse_vec3(node, &key_pointer, key, diagnostics),
            "op" => {
                op = match parse_keyword(node, &key_pointer, key, &OP_NAMES, diagnostics) {
                    Some("subtract") => SculptOp::Subtract,
                    Some("intersect") => SculptOp::Intersect,
                    _ => SculptOp::Union,
                };
            }
            "blend" => {
                if let Some(value) = parse_number(node, &key_pointer, key, STRUCTURE, diagnostics) {
                    blend = value;
                }
            }
            "mirror" => mirror = parse_axis(node, &key_pointer, key, diagnostics),
            "repeat" => repeat = parse_repeat(node, &key_pointer, diagnostics),
            "material" => {
                material = envelope::expect_string(
                    node,
                    &key_pointer,
                    "a part `material`",
                    STRUCTURE,
                    diagnostics,
                );
            }
            "bone" => {
                bone = envelope::expect_string(
                    node,
                    &key_pointer,
                    "a part `bone`",
                    STRUCTURE,
                    diagnostics,
                );
            }
            "sharp" => {
                if let Some(value) = parse_bool(node, &key_pointer, key, diagnostics) {
                    sharp = value;
                }
            }
            "deform" => deform = parse_deform(node, &key_pointer, diagnostics),
            "noise" => noise = parse_noise(node, &key_pointer, diagnostics),
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                &pointer,
                "a sculpt part",
                PART_VOCABULARY,
                STRUCTURE,
            )),
        }
    }

    if !entries.iter().any(|entry| entry.key == "name") {
        diagnostics.push(missing_nested(&pointer, "a part", "name"));
    }
    let shape = build_shape(shape?, fields, entries, &pointer, diagnostics)?;
    Some(SculptPart {
        name: name?,
        shape,
        at,
        rotation,
        op,
        blend,
        mirror,
        repeat,
        material,
        bone,
        sharp,
        deform,
        noise,
    })
}

/// Assembles the shape from its authored fields, reporting each required field the part omits.
fn build_shape(
    shape: &str,
    fields: ShapeFields,
    entries: &[MapEntry],
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<SculptShape> {
    let mut require = |present: bool, key: &str| {
        if !present && !entries.iter().any(|entry| entry.key == key) {
            diagnostics.push(
                Diagnostic::new(
                    STRUCTURE,
                    format!("a `{shape}` part is missing its required `{key}`"),
                )
                .with_path(pointer.to_string())
                .with_help(format!(
                    "a `{shape}` part takes: {}",
                    shape_keys(shape).join(", ")
                )),
            );
        }
    };
    match shape {
        "sphere" => {
            require(fields.radius.is_some(), "radius");
            Some(SculptShape::Sphere {
                radius: fields.radius?,
            })
        }
        "capsule" => {
            require(fields.radius.is_some(), "radius");
            require(fields.from.is_some(), "from");
            require(fields.to.is_some(), "to");
            Some(SculptShape::Capsule {
                radius: fields.radius?,
                from: fields.from?,
                to: fields.to?,
            })
        }
        "ellipsoid" => {
            require(fields.radii.is_some(), "radii");
            Some(SculptShape::Ellipsoid {
                radii: fields.radii?,
            })
        }
        "box" => {
            require(fields.size.is_some(), "size");
            Some(SculptShape::Box {
                size: fields.size?,
                round: fields.round.unwrap_or(0.0),
            })
        }
        "torus" => {
            require(fields.radius.is_some(), "radius");
            require(fields.tube.is_some(), "tube");
            Some(SculptShape::Torus {
                radius: fields.radius?,
                tube: fields.tube?,
            })
        }
        "cone" => {
            require(fields.radius.is_some(), "radius");
            require(fields.height.is_some(), "height");
            Some(SculptShape::Cone {
                radius: fields.radius?,
                height: fields.height?,
            })
        }
        "voxel" => {
            require(fields.path.is_some(), "path");
            require(fields.scalar_size.is_some(), "size");
            Some(SculptShape::Voxel {
                path: fields.path?,
                size: fields.scalar_size?,
            })
        }
        // `shape` is one of `SHAPE_NAMES` by the time the part is built, so the rest is `mesh` —
        // the one shape whose `size` is optional, since a mesh document is already in world units.
        _ => {
            require(fields.path.is_some(), "path");
            Some(SculptShape::Mesh {
                path: fields.path?,
                size: fields.scalar_size,
            })
        }
    }
}

/// `L1157` for a key that exists on some part but not on one of this `shape`.
fn misfit_key(shape: &str, key: &str, entry: &MapEntry, pointer: &str) -> Diagnostic {
    let keys = shape_keys(shape);
    let message = if key == "at" {
        format!(
            "a `capsule` part does not take `at` (line {}); a capsule is placed by `from` and `to`",
            entry.key_line
        )
    } else {
        format!(
            "a `{shape}` part does not take `{key}` (line {})",
            entry.key_line
        )
    };
    let list = format!("the `{shape}` keys are: {}", keys.join(", "));
    let help = match suggest::nearest(key, keys.iter().copied()) {
        Some(candidate) => format!("nearest match is `{candidate}`; {list}"),
        None => list,
    };
    Diagnostic::new(DATA, message)
        .with_path(pointer.to_string())
        .with_help(help)
}

fn parse_repeat(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<SculptRepeat> {
    let entries = expect_mapping(node, pointer, "`repeat`", diagnostics)?;
    let mut count = None;
    let mut step = None;
    for entry in entries {
        let key_pointer = format!("{pointer}/{}", pointer_token(&entry.key));
        match entry.key.as_str() {
            "count" => count = parse_u32(&entry.node, &key_pointer, "count", diagnostics),
            "step" => step = parse_vec3(&entry.node, &key_pointer, "step", diagnostics),
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                pointer,
                "`repeat`",
                REPEAT_KEYS,
                STRUCTURE,
            )),
        }
    }
    require_keys(entries, REPEAT_KEYS, pointer, "`repeat`", diagnostics);
    Some(SculptRepeat {
        count: count?,
        step: step?,
    })
}

fn parse_deform(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<SculptDeform> {
    let entries = expect_mapping(node, pointer, "`deform`", diagnostics)?;
    let mut deform = SculptDeform::default();
    for entry in entries {
        let key_pointer = format!("{pointer}/{}", pointer_token(&entry.key));
        match entry.key.as_str() {
            "bend" => deform.bend = parse_turn(&entry.node, &key_pointer, "bend", diagnostics),
            "twist" => deform.twist = parse_turn(&entry.node, &key_pointer, "twist", diagnostics),
            "taper" => deform.taper = parse_taper(&entry.node, &key_pointer, diagnostics),
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                pointer,
                "`deform`",
                DEFORM_KEYS,
                STRUCTURE,
            )),
        }
    }
    // An empty `deform` parses; it is `L1157`, so the author sees one message and not two.
    Some(deform)
}

fn parse_turn(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<SculptTurn> {
    let noun = format!("`{what}`");
    let entries = expect_mapping(node, pointer, &noun, diagnostics)?;
    let mut axis = None;
    let mut degrees = None;
    for entry in entries {
        let key_pointer = format!("{pointer}/{}", pointer_token(&entry.key));
        match entry.key.as_str() {
            "axis" => axis = parse_axis(&entry.node, &key_pointer, "axis", diagnostics),
            "degrees" => {
                degrees =
                    parse_number(&entry.node, &key_pointer, "degrees", STRUCTURE, diagnostics);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown, entry, pointer, &noun, TURN_KEYS, STRUCTURE,
            )),
        }
    }
    require_keys(entries, TURN_KEYS, pointer, &noun, diagnostics);
    Some(SculptTurn {
        axis: axis?,
        degrees: degrees?,
    })
}

fn parse_taper(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<SculptTaper> {
    let entries = expect_mapping(node, pointer, "`taper`", diagnostics)?;
    let mut axis = None;
    let mut from = None;
    let mut to = None;
    for entry in entries {
        let key_pointer = format!("{pointer}/{}", pointer_token(&entry.key));
        match entry.key.as_str() {
            "axis" => axis = parse_axis(&entry.node, &key_pointer, "axis", diagnostics),
            "from" => {
                from = parse_number(&entry.node, &key_pointer, "from", STRUCTURE, diagnostics)
            }
            "to" => to = parse_number(&entry.node, &key_pointer, "to", STRUCTURE, diagnostics),
            unknown => diagnostics.push(envelope::unknown_key(
                unknown, entry, pointer, "`taper`", TAPER_KEYS, STRUCTURE,
            )),
        }
    }
    require_keys(entries, TAPER_KEYS, pointer, "`taper`", diagnostics);
    Some(SculptTaper {
        axis: axis?,
        from: from?,
        to: to?,
    })
}

fn parse_noise(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<SculptNoise> {
    let entries = expect_mapping(node, pointer, "`noise`", diagnostics)?;
    let mut amplitude = None;
    let mut scale = None;
    let mut seed = None;
    for entry in entries {
        let key_pointer = format!("{pointer}/{}", pointer_token(&entry.key));
        match entry.key.as_str() {
            "amplitude" => {
                amplitude = parse_number(
                    &entry.node,
                    &key_pointer,
                    "amplitude",
                    STRUCTURE,
                    diagnostics,
                );
            }
            "scale" => {
                scale = parse_number(&entry.node, &key_pointer, "scale", STRUCTURE, diagnostics)
            }
            "seed" => seed = parse_u64(&entry.node, &key_pointer, "seed", diagnostics),
            unknown => diagnostics.push(envelope::unknown_key(
                unknown, entry, pointer, "`noise`", NOISE_KEYS, STRUCTURE,
            )),
        }
    }
    require_keys(entries, NOISE_KEYS, pointer, "`noise`", diagnostics);
    Some(SculptNoise {
        amplitude: amplitude?,
        scale: scale?,
        seed: seed?,
    })
}

/// Reports every key of `keys` the mapping does not carry, in `keys` order.
fn require_keys<const N: usize>(
    entries: &[MapEntry],
    keys: [&'static str; N],
    pointer: &str,
    noun: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for key in keys {
        if !entries.iter().any(|entry| entry.key == key) {
            diagnostics.push(missing_nested(pointer, noun, key));
        }
    }
}

/// An axis-valued key: `mirror`, or a deformer's `axis`. `what` is the key it was read from, so the
/// message names what the author wrote.
fn parse_axis(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<SculptAxis> {
    parse_keyword(node, pointer, what, &AXIS_NAMES, diagnostics).map(|axis| match axis {
        "x" => SculptAxis::X,
        "y" => SculptAxis::Y,
        _ => SculptAxis::Z,
    })
}

fn missing_nested(pointer: &str, noun: &str, key: &str) -> Diagnostic {
    Diagnostic::new(STRUCTURE, format!("{noun} is missing its required `{key}`"))
        .with_path(pointer.to_string())
        .with_help(format!("add `{key}:` to it"))
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
                    "write `{what}` as a list, one `- name: ...` entry per item"
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

fn parse_vec3(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<[f64; 3]> {
    let help = format!("write `{what}` as three numbers (x, y, z), one per line");
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
                    "`{what}` must have exactly 3 components (x, y, z), found {} (line {})",
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
    if value.is_none() {
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
                "write `{what}` as a whole number without a decimal point"
            )),
        );
    }
    value
}

/// The one `u64` slot of a sculpt document: a `noise` `seed`, the sprite `seed` rule.
fn parse_u64(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<u64> {
    let value = match &node.value {
        NodeValue::Scalar(Value::Number(n)) => n.as_u64(),
        _ => None,
    };
    if value.is_none() {
        diagnostics.push(
            Diagnostic::new(
                STRUCTURE,
                format!(
                    "`{what}` must be a non-negative integer that fits in 64 bits, found {} \
                     (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(format!(
                "write `{what}` as a whole number without a decimal point, e.g. `{what}: 7`"
            )),
        );
    }
    value
}

fn parse_bool(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<bool> {
    match &node.value {
        NodeValue::Scalar(Value::Bool(value)) => Some(*value),
        other => {
            diagnostics.push(
                Diagnostic::new(
                    STRUCTURE,
                    format!(
                        "`{what}` must be a boolean, found {} (line {})",
                        other.type_phrase(),
                        node.line
                    ),
                )
                .with_path(pointer.to_string())
                .with_help(format!("write `{what}: true` or remove the key")),
            );
            None
        }
    }
}

/// A string drawn from a closed set: `choices` holds every accepted spelling.
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

// --- Validate (data, `L1157`; bone tree, `L1160`) -----------------------------------------------

/// Validates the data invariants of an already-parsed [`SculptDocument`]. A voxel or mesh part's
/// `path` may climb with `..` but is refused when absolute, drive-prefixed or a Windows device.
pub(crate) fn validate(doc: &SculptDocument) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    if !(SCULPT_MIN_RESOLUTION..=SCULPT_MAX_RESOLUTION).contains(&doc.resolution) {
        diagnostics.push(
            Diagnostic::new(
                DATA,
                format!(
                    "`resolution` is {}, outside {SCULPT_MIN_RESOLUTION}..={SCULPT_MAX_RESOLUTION} \
                     cells per world unit",
                    doc.resolution
                ),
            )
            .with_path("/resolution")
            .with_help(format!(
                "the default is {SCULPT_DEFAULT_RESOLUTION}; raise it for thin features, lower it \
                 for a coarse prop"
            )),
        );
    }
    if doc.triangles < SCULPT_MIN_TRIANGLES {
        diagnostics.push(
            Diagnostic::new(
                DATA,
                format!(
                    "`triangles` is {}, below the minimum of {SCULPT_MIN_TRIANGLES}",
                    doc.triangles
                ),
            )
            .with_path("/triangles")
            .with_help(format!(
                "the default is {SCULPT_DEFAULT_TRIANGLES} for a prop; a closed surface needs at \
                 least {SCULPT_MIN_TRIANGLES}"
            )),
        );
    }
    if let Some(rim) = &doc.rim {
        diagnostics.extend(crate::rim::validate_rim(rim, "/rim", DATA));
    }
    if doc.budget_exempt == Some(false) {
        diagnostics.push(
            Diagnostic::new(
                DATA,
                "`budget_exempt` is `false`; the only meaningful value is `true` (the key exists \
                 solely to opt out of the vertex-budget warning)",
            )
            .with_path("/budget_exempt")
            .with_help(
                "remove the `budget_exempt` key — its absence already means the budget warning \
                 applies",
            ),
        );
    }

    voxel::validate_palette(&doc.palette, DATA, "sculpt", &mut diagnostics);
    validate_bones(doc, &mut diagnostics);

    for (index, part) in doc.parts.iter().enumerate() {
        validate_part(doc, index, part, &mut diagnostics);
    }
    diagnostics
}

fn validate_part(
    doc: &SculptDocument,
    index: usize,
    part: &SculptPart,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let pointer = format!("/parts/{index}");
    let name = part.name.as_str();
    if name.is_empty() {
        diagnostics.push(
            Diagnostic::new(DATA, format!("part {index} has an empty `name`"))
                .with_path(format!("{pointer}/name"))
                .with_help("name the part for what it is — `head`, `plinth`"),
        );
    } else if doc.parts[..index].iter().any(|other| other.name == name) {
        diagnostics.push(
            Diagnostic::new(
                DATA,
                format!("the part name `{name}` is used more than once"),
            )
            .with_path(format!("{pointer}/name"))
            .with_help(
                "each part needs its own name — a report or a pointer naming a duplicated name \
                 cannot say which part it means",
            ),
        );
    }

    let mut check = Checker {
        part: name,
        pointer: &pointer,
        diagnostics,
    };
    match &part.shape {
        SculptShape::Sphere { radius } => check.positive(*radius, "radius"),
        SculptShape::Capsule { radius, from, to } => {
            check.positive(*radius, "radius");
            check.finite_vec(from, "from");
            check.finite_vec(to, "to");
        }
        SculptShape::Ellipsoid { radii } => check.positive_vec(radii, "radii"),
        SculptShape::Box { size, round } => {
            check.positive_vec(size, "size");
            check.round(*round, size);
        }
        SculptShape::Torus { radius, tube } => {
            check.positive(*radius, "radius");
            check.positive(*tube, "tube");
        }
        SculptShape::Cone { radius, height } => {
            check.positive(*radius, "radius");
            check.positive(*height, "height");
        }
        SculptShape::Voxel { path, size } => {
            if path.is_empty() {
                check.push(
                    "path",
                    format!("voxel part `{name}` has an empty `path`"),
                    "point `path` at the `.voxel.yaml` document, written relative to this sculpt's \
                     own directory",
                );
            } else if let Some(refusal) = sibling_refusal(path) {
                check.push(
                    "path",
                    format!("`path` (`{path}`) {}", refusal.describe()),
                    "point `path` at the `.voxel.yaml` document, written relative to this sculpt's \
                     own directory",
                );
            }
            check.positive(*size, "size");
        }
        SculptShape::Mesh { path, size } => {
            if path.is_empty() {
                check.push(
                    "path",
                    format!("mesh part `{name}` has an empty `path`"),
                    "point `path` at the `.mesh.yaml` document, written relative to this sculpt's \
                     own directory",
                );
            } else if let Some(refusal) = sibling_refusal(path) {
                check.push(
                    "path",
                    format!("`path` (`{path}`) {}", refusal.describe()),
                    "point `path` at the `.mesh.yaml` document, written relative to this sculpt's \
                     own directory",
                );
            }
            if let Some(size) = size {
                check.positive(*size, "size");
            }
        }
    }
    check.finite_vec(&part.at, "at");
    if matches!(part.shape, SculptShape::Capsule { .. }) && part.at.iter().any(|c| *c != 0.0) {
        check.push(
            "at",
            format!(
                "capsule part `{name}` has a non-zero `at`; a capsule is placed by `from` and `to`"
            ),
            "move the offset into `from` and `to`, and remove `at`",
        );
    }
    if let Some(rotation) = &part.rotation {
        check.finite_vec(rotation, "rotation");
    }
    if !part.blend.is_finite() {
        check.push(
            "blend",
            format!(
                "part `{name}` has a non-finite `blend` value `{}`",
                part.blend
            ),
            "`blend` must be a finite number",
        );
    } else if part.blend < 0.0 {
        check.push(
            "blend",
            format!("part `{name}`'s `blend` is {}; it must be >= 0", part.blend),
            "`blend` is a smoothing radius in world units; `0` is a hard edge",
        );
    }
    if let Some(repeat) = &part.repeat {
        if repeat.count < 2 {
            check.push(
                "repeat/count",
                format!(
                    "part `{name}`'s `repeat` `count` is {}; a repeat needs at least 2 copies",
                    repeat.count
                ),
                "set `count` to 2 or more, or remove `repeat`",
            );
        } else if repeat.count > SCULPT_MAX_REPEAT {
            check.push(
                "repeat/count",
                format!(
                    "part `{name}`'s `repeat` `count` is {}; a repeat holds at most \
                     {SCULPT_MAX_REPEAT} copies",
                    repeat.count
                ),
                "split the run into several parts, or raise `step`",
            );
        }
        check.finite_vec(&repeat.step, "repeat/step");
    }
    if let Some(deform) = &part.deform {
        check.deform(deform);
    }
    if let Some(noise) = &part.noise {
        check.noise(noise);
    }

    match (&part.material, part.op) {
        (None, SculptOp::Union) => check.push_at(
            pointer.clone(),
            format!("union part `{name}` has no `material`; a union part needs one"),
            palette_help(doc, "add `material:` naming a palette glyph"),
        ),
        (Some(_), SculptOp::Subtract | SculptOp::Intersect) => check.push(
            "material",
            format!(
                "part `{name}` is a `{}`: a subtract/intersect part contributes no material; \
                 remove `material`",
                part.op.name()
            ),
            "a subtract or intersect only re-weights the parts before it, so its `material` is \
             never seen",
        ),
        (Some(glyph), SculptOp::Union) if doc.palette_entry(glyph).is_none() => check.push(
            "material",
            format!("part `{name}`'s `material` `{glyph}` is not a palette glyph"),
            palette_help(
                doc,
                "name a glyph the palette defines, or add it to the palette",
            ),
        ),
        _ => {}
    }

    if let Some(bone) = &part.bone
        && !doc.bones.iter().any(|b| b.name == *bone)
    {
        let names: Vec<&str> = doc.bones.iter().map(|b| b.name.as_str()).collect();
        let help = match suggest::nearest(bone, names) {
            Some(candidate) => format!("nearest match is `{candidate}`"),
            None => "`bone` must name an entry of `bones:`, or be removed to mean the root".into(),
        };
        diagnostics.push(
            Diagnostic::new(
                BONES,
                format!("part `{name}` names the bone `{bone}`, which is not in `bones:`"),
            )
            .with_path(format!("{pointer}/bone"))
            .with_help(help),
        );
    }
}

fn palette_help(doc: &SculptDocument, fix: &str) -> String {
    if doc.palette.is_empty() {
        format!("{fix}; the palette is empty")
    } else {
        let glyphs: Vec<&str> = doc.palette.iter().map(|e| e.glyph.as_str()).collect();
        format!("{fix}; the palette holds `{}`", glyphs.join("`, `"))
    }
}

/// The per-part `L1157` checks, pointed at `/parts/<i>/<field>`.
struct Checker<'a> {
    part: &'a str,
    pointer: &'a str,
    diagnostics: &'a mut Vec<Diagnostic>,
}

impl Checker<'_> {
    fn push(&mut self, field: &str, message: String, help: impl Into<String>) {
        let pointer = format!("{}/{field}", self.pointer);
        self.push_at(pointer, message, help.into());
    }

    fn push_at(&mut self, pointer: String, message: String, help: impl Into<String>) {
        self.diagnostics.push(
            Diagnostic::new(DATA, message)
                .with_path(pointer)
                .with_help(help.into()),
        );
    }

    fn finite(&mut self, value: f64, field: &str) -> bool {
        if value.is_finite() {
            return true;
        }
        let part = self.part;
        self.push(
            field,
            format!("part `{part}` has a non-finite `{field}` value `{value}`"),
            "every sculpt number must be finite",
        );
        false
    }

    fn positive(&mut self, value: f64, field: &str) {
        if self.finite(value, field) && value <= 0.0 {
            let part = self.part;
            self.push(
                field,
                format!("part `{part}`'s `{field}` is {value}; it must be greater than 0"),
                "a dimension must be positive — a zero or negative size has no surface",
            );
        }
    }

    fn finite_vec(&mut self, values: &[f64; 3], field: &str) {
        for (axis, value) in values.iter().enumerate() {
            self.finite(*value, &format!("{field}/{axis}"));
        }
    }

    fn positive_vec(&mut self, values: &[f64; 3], field: &str) {
        for (axis, value) in values.iter().enumerate() {
            self.positive(*value, &format!("{field}/{axis}"));
        }
    }

    fn deform(&mut self, deform: &SculptDeform) {
        if deform.bend.is_none() && deform.twist.is_none() && deform.taper.is_none() {
            let part = self.part;
            self.push(
                "deform",
                format!("part `{part}`'s `deform` is empty"),
                "give `deform` at least one of `bend`, `twist`, `taper`, or remove the key",
            );
            return;
        }
        for (turn, key) in [(&deform.bend, "bend"), (&deform.twist, "twist")] {
            if let Some(turn) = turn {
                self.finite(turn.degrees, &format!("deform/{key}/degrees"));
            }
        }
        if let Some(taper) = &deform.taper {
            for (value, key) in [(taper.from, "from"), (taper.to, "to")] {
                let field = format!("deform/taper/{key}");
                if self.finite(value, &field) && value <= 0.0 {
                    let part = self.part;
                    self.push(
                        &field,
                        format!(
                            "part `{part}`'s `taper` `{key}` is {value}; it must be greater than 0"
                        ),
                        "a taper scale multiplies the cross-section — `1` keeps it, `0.5` halves \
                         it; `0` would collapse the part to a line, and a negative scale turns it \
                         inside out",
                    );
                }
            }
        }
    }

    fn noise(&mut self, noise: &SculptNoise) {
        let part = self.part;
        if self.finite(noise.amplitude, "noise/amplitude") && noise.amplitude < 0.0 {
            self.push(
                "noise/amplitude",
                format!(
                    "part `{part}`'s `noise` `amplitude` is {}; it must be >= 0",
                    noise.amplitude
                ),
                "`amplitude` is the peak displacement in world units; `0` leaves the surface alone",
            );
        }
        if self.finite(noise.scale, "noise/scale") && noise.scale <= 0.0 {
            self.push(
                "noise/scale",
                format!(
                    "part `{part}`'s `noise` `scale` is {}; it must be greater than 0",
                    noise.scale
                ),
                "`scale` is the noise lattice's spacing in world units — the feature size, not a \
                 frequency",
            );
        }
    }

    fn round(&mut self, round: f64, size: &[f64; 3]) {
        if !self.finite(round, "round") {
            return;
        }
        let part = self.part;
        if round < 0.0 {
            self.push(
                "round",
                format!("part `{part}`'s `round` is {round}; it must be >= 0"),
                "`round` is an edge radius; `0` is a sharp box",
            );
            return;
        }
        if size.iter().all(|c| c.is_finite() && *c > 0.0) {
            let limit = size.iter().copied().fold(f64::INFINITY, f64::min) / 2.0;
            if round > limit {
                self.push(
                    "round",
                    format!(
                        "part `{part}`'s `round` is {round}, more than half its smallest `size` \
                         ({limit})"
                    ),
                    format!("lower `round` to at most {limit}, or grow the box"),
                );
            }
        }
    }
}

fn validate_bones(doc: &SculptDocument, diagnostics: &mut Vec<Diagnostic>) {
    if doc.bones.is_empty() {
        diagnostics.push(
            Diagnostic::new(BONES, "`bones` is empty; a sculpt needs a root bone")
                .with_path("/bones")
                .with_help("an unrigged model has one bone:\n  bones:\n    - name: root"),
        );
        return;
    }
    let names: Vec<&str> = doc.bones.iter().map(|bone| bone.name.as_str()).collect();
    for (index, bone) in doc.bones.iter().enumerate() {
        let pointer = format!("/bones/{index}");
        if bone.name.is_empty() {
            diagnostics.push(
                Diagnostic::new(BONES, format!("bone {index} has an empty `name`"))
                    .with_path(format!("{pointer}/name"))
                    .with_help("name the bone for what it moves — `root`, `head`, `arm-l`"),
            );
        } else if names[..index].contains(&bone.name.as_str()) {
            diagnostics.push(
                Diagnostic::new(
                    BONES,
                    format!("the bone name `{}` is used more than once", bone.name),
                )
                .with_path(format!("{pointer}/name"))
                .with_help("each bone needs its own name; rename or remove the duplicate"),
            );
        }
        validate_bone_position(bone, &pointer, diagnostics);
        match (&bone.parent, index) {
            (Some(_), 0) => diagnostics.push(
                Diagnostic::new(
                    BONES,
                    format!(
                        "the first bone `{}` has a `parent`; the first entry of `bones:` is the root",
                        bone.name
                    ),
                )
                .with_path(format!("{pointer}/parent"))
                .with_help("remove its `parent`, or move the root bone to the top of the list"),
            ),
            (None, 0) => {}
            (None, _) => diagnostics.push(
                Diagnostic::new(
                    BONES,
                    format!(
                        "bone `{}` has no `parent`, so it is a second root; only the first bone \
                         is the root",
                        bone.name
                    ),
                )
                .with_path(pointer)
                .with_help(format!(
                    "give it a `parent` naming another bone, e.g. `parent: {}`",
                    doc.bones[0].name
                )),
            ),
            (Some(parent), _) if *parent == bone.name => diagnostics.push(
                Diagnostic::new(
                    BONES,
                    format!("bone `{}` names itself as its own `parent`", bone.name),
                )
                .with_path(format!("{pointer}/parent"))
                .with_help("point `parent` at the bone this one hangs off"),
            ),
            (Some(parent), _) if !names.contains(&parent.as_str()) => {
                let help = match suggest::nearest(parent, names.iter().copied()) {
                    Some(candidate) => format!("nearest match is `{candidate}`"),
                    None => "`parent` must name another bone of this document".to_string(),
                };
                diagnostics.push(
                    Diagnostic::new(
                        BONES,
                        format!(
                            "bone `{}` names the parent `{parent}`, which is not a bone",
                            bone.name
                        ),
                    )
                    .with_path(format!("{pointer}/parent"))
                    .with_help(help),
                );
            }
            (Some(_), _) => {}
        }
    }

    let parents: Vec<Option<usize>> = doc
        .bones
        .iter()
        .map(|bone| {
            bone.parent
                .as_deref()
                .and_then(|name| names.iter().position(|n| *n == name))
        })
        .collect();
    for index in 0..doc.bones.len() {
        let Some(cycle) = cycle_from(index, &parents) else {
            continue;
        };
        // A one-bone cycle is self-parenting, already reported; reporting only from the cycle's
        // lowest member names each cycle once.
        if cycle.len() < 2 || cycle.iter().min() != Some(&index) {
            continue;
        }
        let mut chain: Vec<String> = cycle.iter().map(|&i| format!("`{}`", names[i])).collect();
        chain.push(format!("`{}`", names[index]));
        diagnostics.push(
            Diagnostic::new(
                BONES,
                format!("the bone `parent` chain loops: {}", chain.join(" -> ")),
            )
            .with_path(format!("/bones/{index}/parent"))
            .with_help("the bones form a tree: re-point one `parent` at a bone outside the loop"),
        );
    }
}

/// `head` and `tail` are one pair: both authored (each component finite) or neither.
fn validate_bone_position(bone: &SculptBone, pointer: &str, diagnostics: &mut Vec<Diagnostic>) {
    let name = &bone.name;
    for (end, other, values) in [("head", "tail", &bone.head), ("tail", "head", &bone.tail)] {
        let Some(values) = values else {
            continue;
        };
        for (axis, value) in values.iter().enumerate() {
            if !value.is_finite() {
                diagnostics.push(
                    Diagnostic::new(
                        BONES,
                        format!("bone `{name}`'s `{end}` has a non-finite component `{value}`"),
                    )
                    .with_path(format!("{pointer}/{end}/{axis}"))
                    .with_help("a bone position is three finite world-unit coordinates"),
                );
            }
        }
        if (end == "head" && bone.tail.is_none()) || (end == "tail" && bone.head.is_none()) {
            diagnostics.push(
                Diagnostic::new(
                    BONES,
                    format!("bone `{name}` has a `{end}` but no `{other}`"),
                )
                .with_path(format!("{pointer}/{end}"))
                .with_help(format!(
                    "`head` and `tail` are the two ends of one segment: add `{other}:`, or remove \
                     `{end}:` to leave the bone unpositioned"
                )),
            );
        }
    }
}

/// The cycle reachable by following `parents` from `start`, in walk order from the first repeated
/// node, or `None` when the chain ends at a root.
fn cycle_from(start: usize, parents: &[Option<usize>]) -> Option<Vec<usize>> {
    let mut path: Vec<usize> = Vec::new();
    let mut current = start;
    loop {
        if let Some(position) = path.iter().position(|&node| node == current) {
            return Some(path[position..].to_vec());
        }
        path.push(current);
        current = parents[current]?;
    }
}

// --- Emit ---------------------------------------------------------------------------------------

/// Writes `doc` in canonical form (see the module docs for the minimal-default rule).
pub(crate) fn emit(doc: &SculptDocument) -> String {
    let mut out = String::new();
    out.push_str("format: 1\n");
    out.push_str("kind: sculpt\n");
    if let Some(name) = &doc.name {
        out.push_str(&format!("name: {}\n", scalar_string(name)));
    }
    out.push_str(&format!("resolution: {}\n", doc.resolution));
    out.push_str(&format!("triangles: {}\n", doc.triangles));
    if let Some(exempt) = doc.budget_exempt {
        out.push_str(&format!("budget_exempt: {exempt}\n"));
    }
    if let Some(rim) = &doc.rim {
        crate::rim::emit_rim(&mut out, rim);
    }
    voxel::emit_palette(&mut out, &doc.palette);

    if doc.bones.is_empty() {
        out.push_str("bones: []\n");
    } else {
        out.push_str("bones:\n");
        for bone in &doc.bones {
            out.push_str(&format!("  - name: {}\n", scalar_string(&bone.name)));
            if let Some(parent) = &bone.parent {
                out.push_str(&format!("    parent: {}\n", scalar_string(parent)));
            }
            if let Some(head) = &bone.head {
                write_vec3(&mut out, "    ", "head", head);
            }
            if let Some(tail) = &bone.tail {
                write_vec3(&mut out, "    ", "tail", tail);
            }
        }
    }

    if doc.parts.is_empty() {
        out.push_str("parts: []\n");
        return out;
    }
    out.push_str("parts:\n");
    for part in &doc.parts {
        emit_part(&mut out, part);
    }
    out
}

fn emit_part(out: &mut String, part: &SculptPart) {
    const INDENT: &str = "    ";
    out.push_str(&format!("  - name: {}\n", scalar_string(&part.name)));
    out.push_str(&format!("{INDENT}shape: {}\n", part.shape.name()));
    match &part.shape {
        SculptShape::Sphere { radius } => write_float(out, INDENT, "radius", *radius),
        SculptShape::Capsule { radius, from, to } => {
            write_float(out, INDENT, "radius", *radius);
            write_vec3(out, INDENT, "from", from);
            write_vec3(out, INDENT, "to", to);
        }
        SculptShape::Ellipsoid { radii } => write_vec3(out, INDENT, "radii", radii),
        SculptShape::Box { size, round } => {
            write_vec3(out, INDENT, "size", size);
            if !is_zero(*round) {
                write_float(out, INDENT, "round", *round);
            }
        }
        SculptShape::Torus { radius, tube } => {
            write_float(out, INDENT, "radius", *radius);
            write_float(out, INDENT, "tube", *tube);
        }
        SculptShape::Cone { radius, height } => {
            write_float(out, INDENT, "radius", *radius);
            write_float(out, INDENT, "height", *height);
        }
        SculptShape::Voxel { path, size } => {
            out.push_str(&format!(
                "{INDENT}path: {}
",
                scalar_string(path)
            ));
            write_float(out, INDENT, "size", *size);
        }
        SculptShape::Mesh { path, size } => {
            out.push_str(&format!("{INDENT}path: {}\n", scalar_string(path)));
            if let Some(size) = size {
                write_float(out, INDENT, "size", *size);
            }
        }
    }
    if !part.at.iter().all(|c| is_zero(*c)) {
        write_vec3(out, INDENT, "at", &part.at);
    }
    if let Some(rotation) = &part.rotation {
        write_vec3(out, INDENT, "rotation", rotation);
    }
    if part.op != SculptOp::Union {
        out.push_str(&format!("{INDENT}op: {}\n", part.op.name()));
    }
    if !is_zero(part.blend) {
        write_float(out, INDENT, "blend", part.blend);
    }
    if let Some(axis) = part.mirror {
        out.push_str(&format!("{INDENT}mirror: {}\n", axis.name()));
    }
    if let Some(repeat) = &part.repeat {
        out.push_str(&format!(
            "{INDENT}repeat:\n{INDENT}  count: {}\n",
            repeat.count
        ));
        write_vec3(out, "      ", "step", &repeat.step);
    }
    if let Some(material) = &part.material {
        out.push_str(&format!("{INDENT}material: {}\n", scalar_string(material)));
    }
    if let Some(bone) = &part.bone {
        out.push_str(&format!("{INDENT}bone: {}\n", scalar_string(bone)));
    }
    if part.sharp {
        out.push_str(&format!("{INDENT}sharp: true\n"));
    }
    // `deform` and `noise` close the part block: they modify the shape the keys above describe.
    if let Some(deform) = &part.deform {
        out.push_str(&format!("{INDENT}deform:\n"));
        for (turn, key) in [(&deform.bend, "bend"), (&deform.twist, "twist")] {
            if let Some(turn) = turn {
                out.push_str(&format!("{INDENT}  {key}:\n"));
                out.push_str(&format!("{INDENT}    axis: {}\n", turn.axis.name()));
                write_float(out, "        ", "degrees", turn.degrees);
            }
        }
        if let Some(taper) = &deform.taper {
            out.push_str(&format!("{INDENT}  taper:\n"));
            out.push_str(&format!("{INDENT}    axis: {}\n", taper.axis.name()));
            write_float(out, "        ", "from", taper.from);
            write_float(out, "        ", "to", taper.to);
        }
    }
    if let Some(noise) = &part.noise {
        out.push_str(&format!("{INDENT}noise:\n"));
        write_float(out, "      ", "amplitude", noise.amplitude);
        write_float(out, "      ", "scale", noise.scale);
        out.push_str(&format!("{INDENT}  seed: {}\n", noise.seed));
    }
}

/// A default is omitted only when it is exactly `+0.0`, so an authored `-0.0` round-trips.
fn is_zero(value: f64) -> bool {
    value.to_bits() == 0
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

// --- Render-time overrides ----------------------------------------------------------------------

/// Applies render-time overrides to a parsed sculpt document, in order, returning what each changed.
///
/// The sculpt counterpart of [`crate::apply_palette_overrides`], over a wider domain: every numeric
/// field is a pointer — `/resolution`, `/triangles`, the palette's fields, and each part's numbers
/// by index (`/parts/2/blend`, `/parts/0/at/1`, `/parts/3/size/2`, `/parts/1/repeat/count`). A shape
/// field is addressable only on a part of that shape. Writing a component of an unset `rotation`
/// creates it at `[0, 0, 0]` first, and [`AppliedOverride::from`] comes back `None`. Range and
/// finiteness are not checked entry by entry: once every override is written the document is
/// validated, so an out-of-range value is the ordinary `L1157`.
///
/// # Errors
/// `L4008` per override whose pointer does not address a numeric field (a part's `op`, `material`,
/// `bone`, a bone, a vector as a whole), names a part index or vector component that does not
/// exist, or carries a value of the wrong kind (a non-number, or a non-integer for `resolution`,
/// `triangles` and `repeat/count`). Failures are collected. When every override applies, the
/// sculpt's own error codes (`L1157`) come back instead. On failure `doc` is left partially
/// overridden and the caller must discard it.
///
/// # Example
/// ```
/// use ludion_scene::{SceneOverride, apply_sculpt_overrides};
/// use serde_json::json;
///
/// let source = "format: 1\nkind: sculpt\nresolution: 32\ntriangles: 4000\npalette:\n  S:\n    \
///               color:\n      - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\nbones:\n  - name: \
///               root\nparts:\n  - name: ball\n    shape: sphere\n    radius: 0.5\n    \
///               material: S\n";
/// let mut model = ludion_scene::check_sculpt(source).unwrap();
/// let overrides = [
///     SceneOverride::parse("/parts/0/blend=0.2").unwrap(),
///     SceneOverride::parse("/parts/0/at/1=0.75").unwrap(),
///     SceneOverride::parse("/resolution=64").unwrap(),
/// ];
///
/// let applied = apply_sculpt_overrides(&mut model, &overrides).unwrap();
/// assert_eq!(applied[0].from, Some(json!(0.0)));
/// assert_eq!(model.parts[0].blend, 0.2);
/// assert_eq!(model.parts[0].at, [0.0, 0.75, 0.0]);
/// assert_eq!(model.resolution, 64);
///
/// // `op` is not a number: the pointer is refused, not coerced.
/// let bad = [SceneOverride::parse("/parts/0/op=1").unwrap()];
/// let diagnostics = apply_sculpt_overrides(&mut model, &bad).unwrap_err();
/// assert_eq!(diagnostics[0].code.as_str(), "L4008");
/// ```
pub fn apply_sculpt_overrides(
    doc: &mut SculptDocument,
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
        voxel::into_result(applied, validate(doc))
    } else {
        Err(diagnostics)
    }
}

/// One numeric slot an override pointer resolved to.
enum Target<'a> {
    Float(&'a mut f64),
    /// A field that may be absent, created on first write — the `size` idiom for a `mesh` part,
    /// whose omitted default is its second element, not `0.0`.
    OptionalFloat(&'a mut Option<f64>, f64),
    Component(&'a mut [f64; 3], usize),
    OptionalComponent(&'a mut Option<[f64; 3]>, usize),
    Count(&'a mut u32),
}

fn apply_one(
    doc: &mut SculptDocument,
    over: &SceneOverride,
) -> Result<AppliedOverride, Diagnostic> {
    let pointer = over.pointer.as_str();
    let segments: Vec<String> = pointer.split('/').skip(1).map(unescape).collect();
    let target = match segments.first().map(String::as_str) {
        Some("palette") => return voxel::apply_palette_one(&mut doc.palette, over),
        Some("bones") => {
            let Some(raw) = segments.get(1) else {
                return Err(unsupported_pointer(pointer));
            };
            let count = doc.bones.len();
            let Some(index) = raw.parse::<usize>().ok().filter(|i| *i < count) else {
                return Err(override_error(
                    pointer,
                    format!(
                        "override pointer `{pointer}` names bone `{raw}`, but the sculpt has {count} bones"
                    ),
                    format!("bone indices run 0..{count}, in file order"),
                ));
            };
            let rest: Vec<&str> = segments[2..].iter().map(String::as_str).collect();
            bone_target(&mut doc.bones[index], pointer, &rest)?
        }
        Some("resolution") if segments.len() == 1 => Target::Count(&mut doc.resolution),
        Some("triangles") if segments.len() == 1 => Target::Count(&mut doc.triangles),
        Some("parts") => {
            let Some(raw) = segments.get(1) else {
                return Err(unsupported_pointer(pointer));
            };
            let count = doc.parts.len();
            let Some(index) = raw.parse::<usize>().ok().filter(|i| *i < count) else {
                return Err(override_error(
                    pointer,
                    format!(
                        "override pointer `{pointer}` names part `{raw}`, but the sculpt has {count} parts"
                    ),
                    format!("part indices run 0..{count}, in file order"),
                ));
            };
            let rest: Vec<&str> = segments[2..].iter().map(String::as_str).collect();
            part_target(&mut doc.parts[index], pointer, &rest)?
        }
        _ => return Err(unsupported_pointer(pointer)),
    };

    let from = match target {
        Target::Float(slot) => {
            let value = number(pointer, &over.value)?;
            let from = Value::from(*slot);
            *slot = value;
            Some(from)
        }
        Target::OptionalFloat(slot, default) => {
            let value = number(pointer, &over.value)?;
            let from = slot.map(Value::from);
            *slot.get_or_insert(default) = value;
            from
        }
        Target::Component(vector, axis) => {
            let value = number(pointer, &over.value)?;
            let from = Value::from(vector[axis]);
            vector[axis] = value;
            Some(from)
        }
        Target::OptionalComponent(vector, axis) => {
            let value = number(pointer, &over.value)?;
            let from = vector.map(|v| Value::from(v[axis]));
            vector.get_or_insert([0.0; 3])[axis] = value;
            from
        }
        Target::Count(slot) => {
            let value = over
                .value
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(|| {
                    override_error(
                        pointer,
                        format!(
                            "the override at `{pointer}` expects a non-negative integer, but the value is {}",
                            ludion_core::diagnostics::value_phrase(&over.value)
                        ),
                        "write a whole number, e.g. `/resolution=64`",
                    )
                })?;
            let from = Value::from(*slot);
            *slot = value;
            Some(from)
        }
    };
    Ok(AppliedOverride {
        pointer: pointer.to_string(),
        from,
        to: over.value.clone(),
    })
}

fn part_target<'a>(
    part: &'a mut SculptPart,
    pointer: &str,
    rest: &[&str],
) -> Result<Target<'a>, Diagnostic> {
    let is_capsule = matches!(part.shape, SculptShape::Capsule { .. });
    // Cloned up front so the messages below can name the part while one of its fields is borrowed.
    let name = part.name.clone();
    match rest {
        ["blend"] => Ok(Target::Float(&mut part.blend)),
        ["at", axis] if !is_capsule => {
            Ok(Target::Component(&mut part.at, component(pointer, axis)?))
        }
        ["rotation", axis] => Ok(Target::OptionalComponent(
            &mut part.rotation,
            component(pointer, axis)?,
        )),
        ["repeat", ..] => {
            let Some(repeat) = part.repeat.as_mut() else {
                return Err(absent_parent(
                    pointer,
                    &format!("part `{name}`"),
                    "repeat",
                    "an override changes an authored `repeat`; add `repeat:` with a `count` and a `step` to the file first",
                ));
            };
            match &rest[1..] {
                ["count"] => Ok(Target::Count(&mut repeat.count)),
                ["step", axis] => Ok(Target::Component(
                    &mut repeat.step,
                    component(pointer, axis)?,
                )),
                _ => Err(unsupported_pointer(pointer)),
            }
        }
        ["deform", ..] => {
            let Some(deform) = part.deform.as_mut() else {
                return Err(absent_parent(
                    pointer,
                    &format!("part `{name}`"),
                    "deform",
                    "add `deform:` with a `bend`, a `twist` or a `taper` to the file first",
                ));
            };
            match &rest[1..] {
                [turn @ ("bend" | "twist"), "degrees"] => {
                    let slot = match *turn {
                        "bend" => deform.bend.as_mut(),
                        _ => deform.twist.as_mut(),
                    };
                    slot.map(|slot| Target::Float(&mut slot.degrees))
                        .ok_or_else(|| {
                            absent_parent(
                                pointer,
                                &format!("part `{name}`'s `deform`"),
                                turn,
                                format!(
                                    "add `{turn}:` with an `axis` and `degrees` under `deform:` first"
                                ),
                            )
                        })
                }
                ["taper", end @ ("from" | "to")] => deform
                    .taper
                    .as_mut()
                    .map(|taper| {
                        Target::Float(if *end == "from" {
                            &mut taper.from
                        } else {
                            &mut taper.to
                        })
                    })
                    .ok_or_else(|| {
                        absent_parent(
                            pointer,
                            &format!("part `{name}`'s `deform`"),
                            "taper",
                            "add `taper:` with an `axis`, a `from` and a `to` under `deform:` first",
                        )
                    }),
                _ => Err(unsupported_pointer(pointer)),
            }
        }
        ["noise", ..] => {
            let Some(noise) = part.noise.as_mut() else {
                return Err(absent_parent(
                    pointer,
                    &format!("part `{name}`"),
                    "noise",
                    "add `noise:` with an `amplitude`, a `scale` and a `seed` to the file first",
                ));
            };
            match &rest[1..] {
                ["amplitude"] => Ok(Target::Float(&mut noise.amplitude)),
                ["scale"] => Ok(Target::Float(&mut noise.scale)),
                // `seed` is the document's one `u64`: a different seed is a different surface, not
                // a value a sweep walks through.
                _ => Err(unsupported_pointer(pointer)),
            }
        }
        [
            key @ ("name" | "shape" | "op" | "mirror" | "material" | "bone" | "sharp" | "path"),
            ..,
        ] => Err(override_error(
            pointer,
            format!("override pointer `{pointer}` addresses `{key}`, which is not a numeric field"),
            SCULPT_POINTER_SHAPES,
        )),
        _ => {
            let shape = part.shape.name();
            let keys = part.shape.keys();
            shape_target(&mut part.shape, pointer, rest).ok_or_else(|| {
                override_error(
                    pointer,
                    format!(
                        "override pointer `{pointer}` does not address a numeric field of part `{name}`, a `{shape}`"
                    ),
                    format!("a `{shape}` part's own fields are: {}; {SCULPT_POINTER_SHAPES}", keys.join(", ")),
                )
            })?
        }
    }
}

/// The slot a shape-field pointer names, `None` when the shape has no such field; a bad component
/// index is its own error.
fn shape_target<'a>(
    shape: &'a mut SculptShape,
    pointer: &str,
    rest: &[&str],
) -> Option<Result<Target<'a>, Diagnostic>> {
    let target = match (shape, rest) {
        (
            SculptShape::Sphere { radius }
            | SculptShape::Capsule { radius, .. }
            | SculptShape::Torus { radius, .. }
            | SculptShape::Cone { radius, .. },
            ["radius"],
        ) => Ok(Target::Float(radius)),
        (SculptShape::Torus { tube, .. }, ["tube"]) => Ok(Target::Float(tube)),
        (SculptShape::Cone { height, .. }, ["height"]) => Ok(Target::Float(height)),
        (SculptShape::Box { round, .. }, ["round"]) => Ok(Target::Float(round)),
        // A `voxel` part's `size` is one scalar (world units per voxel), so it is addressed whole
        // like a `radius` — never by component, the way a `box`'s three extents are.
        (SculptShape::Voxel { size, .. }, ["size"]) => Ok(Target::Float(size)),
        // A `mesh` part's `size` is the same scalar, but optional: an omitted one reads as `1`
        // (already world units), so an override on it creates the key rather than editing it.
        (SculptShape::Mesh { size, .. }, ["size"]) => Ok(Target::OptionalFloat(size, 1.0)),
        (SculptShape::Ellipsoid { radii }, ["radii", axis])
        | (SculptShape::Box { size: radii, .. }, ["size", axis])
        | (SculptShape::Capsule { from: radii, .. }, ["from", axis])
        | (SculptShape::Capsule { to: radii, .. }, ["to", axis]) => {
            component(pointer, axis).map(|axis| Target::Component(radii, axis))
        }
        _ => return None,
    };
    Some(target)
}

/// The slot a `/bones/<i>/…` pointer names.
///
/// Only an authored `head`/`tail` is addressable. The two are one pair (`L1160`), so creating half
/// of it the way an unset `rotation` is created would leave the document invalid for a reason its
/// author never wrote.
fn bone_target<'a>(
    bone: &'a mut SculptBone,
    pointer: &str,
    rest: &[&str],
) -> Result<Target<'a>, Diagnostic> {
    let name = bone.name.clone();
    match rest {
        [end @ ("head" | "tail"), axis] => {
            let axis = component(pointer, axis)?;
            let slot = if *end == "head" {
                &mut bone.head
            } else {
                &mut bone.tail
            };
            match slot {
                Some(values) => Ok(Target::Component(values, axis)),
                None => Err(absent_parent(
                    pointer,
                    &format!("bone `{name}`"),
                    end,
                    "author `head:` and `tail:` together on the bone first — they are the two ends \
                     of one segment, and an override never writes half a pair",
                )),
            }
        }
        _ => Err(unsupported_pointer(pointer)),
    }
}

/// The `L4008` for a pointer whose parent key `owner` does not carry: the pointer is well formed,
/// but there is nothing authored under it to change.
fn absent_parent(pointer: &str, owner: &str, parent: &str, fix: impl Into<String>) -> Diagnostic {
    override_error(
        pointer,
        format!("override pointer `{pointer}` addresses the `{parent}` of {owner}, which has none"),
        fix,
    )
}

fn component(pointer: &str, segment: &str) -> Result<usize, Diagnostic> {
    segment.parse::<usize>().ok().filter(|axis| *axis < 3).ok_or_else(|| {
        override_error(
            pointer,
            format!(
                "override pointer `{pointer}` addresses component `{segment}`, but a vector has three"
            ),
            "`0` is x, `1` y, `2` z — e.g. `/parts/0/at/1=0.5`",
        )
    })
}

fn number(pointer: &str, value: &Value) -> Result<f64, Diagnostic> {
    value
        .as_f64()
        .map(ludion_core::number::engine_number)
        .ok_or_else(|| {
            override_error(
                pointer,
                format!(
                    "the override at `{pointer}` expects a number, but the value is {}",
                    ludion_core::diagnostics::value_phrase(value)
                ),
                "every addressable sculpt field is a number, e.g. `/parts/0/blend=0.1`",
            )
        })
}

fn unsupported_pointer(pointer: &str) -> Diagnostic {
    override_error(
        pointer,
        format!("override pointer `{pointer}` does not address a numeric field of this sculpt"),
        SCULPT_POINTER_SHAPES,
    )
}

fn override_error(pointer: &str, message: String, help: impl Into<String>) -> Diagnostic {
    Diagnostic::new(code::SCENE_OVERRIDE_INVALID, message)
        .with_path(pointer.to_string())
        .with_help(help.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The idol of the plan, canonical: every shape, every optional key, block style.
    const IDOL: &str = "\
format: 1
kind: sculpt
name: idol
resolution: 32
triangles: 4000
palette:
  E:
    color:
      - 0.2
      - 1.0
      - 0.6
      - 1.0
    emissive: 4.0
  S:
    color:
      - 0.35
      - 0.33
      - 0.3
      - 1.0
    roughness: 0.85
bones:
  - name: root
  - name: head
    parent: root
parts:
  - name: plinth
    shape: box
    size:
      - 1.2
      - 0.3
      - 1.2
    round: 0.02
    at:
      - 0.0
      - 0.15
      - 0.0
    material: S
    sharp: true
  - name: body
    shape: capsule
    radius: 0.25
    from:
      - 0.0
      - 0.3
      - 0.0
    to:
      - 0.0
      - 1.1
      - 0.0
    blend: 0.1
    material: S
  - name: head
    shape: ellipsoid
    radii:
      - 0.22
      - 0.18
      - 0.2
    at:
      - 0.0
      - 1.25
      - 0.0
    blend: 0.08
    material: S
    bone: head
  - name: eye
    shape: sphere
    radius: 0.05
    at:
      - -0.09
      - 1.28
      - 0.16
    blend: 0.02
    mirror: x
    material: E
    bone: head
  - name: mouth
    shape: ellipsoid
    radii:
      - 0.1
      - 0.03
      - 0.08
    at:
      - 0.0
      - 1.18
      - 0.18
    op: subtract
    blend: 0.02
    bone: head
  - name: spine
    shape: cone
    radius: 0.04
    height: 0.12
    at:
      - 0.0
      - 0.5
      - -0.24
    blend: 0.03
    repeat:
      count: 5
      step:
        - 0.0
        - 0.12
        - 0.0
    material: S
  - name: halo
    shape: torus
    radius: 0.3
    tube: 0.02
    at:
      - 0.0
      - 1.6
      - 0.0
    rotation:
      - 90.0
      - 0.0
      - 0.0
    op: intersect
";

    /// A document with the idol's envelope, palette and bones, and `parts` as given.
    fn with_parts(parts: &str) -> String {
        format!(
            "format: 1\nkind: sculpt\npalette:\n  S:\n    color:\n      - 1.0\n      - 1.0\n      \
             - 1.0\n      - 1.0\nbones:\n  - name: root\n  - name: head\n    parent: root\n\
             parts:\n{parts}"
        )
    }

    fn idol() -> SculptDocument {
        parse(IDOL).expect("the idol parses")
    }

    #[track_caller]
    fn assert_reported(diagnostics: &[Diagnostic], code: &str, path: &str) -> Diagnostic {
        diagnostics
            .iter()
            .find(|d| d.code.as_str() == code && d.path.as_deref() == Some(path))
            .cloned()
            .unwrap_or_else(|| panic!("expected {code} at `{path}`, got {diagnostics:#?}"))
    }

    #[track_caller]
    fn parse_err(source: &str, code: &str, path: &str) -> Diagnostic {
        let diagnostics = parse(source).expect_err("the document must not parse");
        assert_reported(&diagnostics, code, path)
    }

    #[track_caller]
    fn validate_err(doc: &SculptDocument, code: &str, path: &str) -> Diagnostic {
        assert_reported(&validate(doc), code, path)
    }

    #[test]
    fn the_idol_parses_validates_and_is_a_canonical_fixed_point() {
        let doc = idol();
        assert!(validate(&doc).is_empty(), "{:#?}", validate(&doc));
        assert_eq!(doc.name.as_deref(), Some("idol"));
        assert_eq!(doc.parts.len(), 7);
        assert_eq!(doc.parts[1].shape.name(), "capsule");
        assert_eq!(doc.parts[3].mirror, Some(SculptAxis::X));
        assert_eq!(doc.parts[4].op, SculptOp::Subtract);
        assert_eq!(
            doc.parts[5].repeat,
            Some(SculptRepeat {
                count: 5,
                step: [0.0, 0.12, 0.0]
            })
        );
        assert_eq!(doc.bone_index(&doc.parts[0]), Some(0));
        assert_eq!(doc.bone_index(&doc.parts[2]), Some(1));
        assert_eq!(emit(&doc), IDOL);
    }

    /// The `ludion sculpt new` scaffold: one palette entry, a root bone, no parts yet.
    #[test]
    fn the_empty_scaffold_checks_clean_and_round_trips() {
        const SCAFFOLD: &str = "\
format: 1
kind: sculpt
name: prop
resolution: 32
triangles: 4000
palette:
  S:
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
bones:
  - name: root
parts: []
";
        let doc = crate::check_sculpt(SCAFFOLD).expect("the scaffold checks clean");
        assert!(doc.parts.is_empty());
        assert_eq!(emit(&doc), SCAFFOLD);
        assert_eq!(emit(&parse(&emit(&doc)).expect("re-parses")), SCAFFOLD);
    }

    #[test]
    fn emit_is_byte_idempotent_on_a_non_canonical_document() {
        let source = with_parts(
            "  - name: ball\n    shape: sphere\n    op: union\n    blend: 0\n    radius: 1\n    \
             sharp: false\n    material: S\n    at:\n      - 0\n      - -0.0\n      - 0\n",
        );
        let doc = parse(&source).expect("parses");
        let once = emit(&doc);
        assert_ne!(once, source);
        assert_eq!(emit(&parse(&once).expect("re-parses")), once);
        let defaults = format!(
            "resolution: {SCULPT_DEFAULT_RESOLUTION}\ntriangles: {SCULPT_DEFAULT_TRIANGLES}\n"
        );
        assert!(once.contains(&defaults), "{once}");
        assert!(!once.contains("op:") && !once.contains("sharp:") && !once.contains("blend:"));
        // The authored `-0.0` is not the default and survives.
        assert!(
            once.contains("    at:\n      - 0.0\n      - -0.0\n      - 0.0\n"),
            "{once}"
        );
    }

    #[test]
    fn defaults_apply_when_keys_are_omitted() {
        let doc = parse(&with_parts(
            "  - name: ball\n    shape: sphere\n    radius: 0.5\n    material: S\n",
        ))
        .expect("parses");
        assert_eq!(doc.resolution, SCULPT_DEFAULT_RESOLUTION);
        assert_eq!(doc.triangles, SCULPT_DEFAULT_TRIANGLES);
        let part = &doc.parts[0];
        assert_eq!(part.at, [0.0; 3]);
        assert_eq!(part.op, SculptOp::Union);
        assert_eq!(part.blend, 0.0);
        assert!(part.bone.is_none() && part.rotation.is_none() && !part.sharp);
        assert!(validate(&doc).is_empty());
    }

    /// Every key of the deformer additions, in canonical order: a positioned bone, and a part
    /// carrying all three deformers and a `noise`.
    const DEFORMED: &str = "\
format: 1
kind: sculpt
resolution: 32
triangles: 4000
palette:
  S:
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
bones:
  - name: root
    head:
      - 0.0
      - 0.0
      - 0.0
    tail:
      - 0.0
      - 1.0
      - 0.0
  - name: head
    parent: root
parts:
  - name: horn
    shape: cone
    radius: 0.2
    height: 1.0
    material: S
    deform:
      bend:
        axis: y
        degrees: 20.0
      twist:
        axis: y
        degrees: -35.5
      taper:
        axis: y
        from: 1.0
        to: 0.25
    noise:
      amplitude: 0.01
      scale: 0.08
      seed: 7
";

    #[test]
    fn deformers_bone_positions_and_noise_round_trip() {
        let doc = crate::check_sculpt(DEFORMED).expect("the deformed document checks clean");
        assert_eq!(doc.bones[0].head, Some([0.0, 0.0, 0.0]));
        assert_eq!(doc.bones[0].tail, Some([0.0, 1.0, 0.0]));
        assert!(doc.bones[1].head.is_none() && doc.bones[1].tail.is_none());
        let deform = doc.parts[0].deform.as_ref().expect("the part deforms");
        assert_eq!(
            deform.bend,
            Some(SculptTurn {
                axis: SculptAxis::Y,
                degrees: 20.0
            })
        );
        assert_eq!(deform.twist.unwrap().degrees, -35.5);
        assert_eq!(
            deform.taper,
            Some(SculptTaper {
                axis: SculptAxis::Y,
                from: 1.0,
                to: 0.25
            })
        );
        assert_eq!(
            doc.parts[0].noise,
            Some(SculptNoise {
                amplitude: 0.01,
                scale: 0.08,
                seed: 7
            })
        );
        assert_eq!(emit(&doc), DEFORMED);
        assert_eq!(emit(&parse(&emit(&doc)).expect("re-parses")), DEFORMED);
    }

    /// Each of the three deformers is independent: one alone emits only its own block.
    #[test]
    fn one_deformer_alone_round_trips() {
        for block in [
            "    deform:\n      bend:\n        axis: x\n        degrees: 90.0\n",
            "    deform:\n      twist:\n        axis: z\n        degrees: 0.5\n",
            "    deform:\n      taper:\n        axis: y\n        from: 0.5\n        to: 2.0\n",
            "    noise:\n      amplitude: 0.0\n      scale: 1.0\n      seed: 0\n",
        ] {
            let source = with_parts(&format!(
                "  - name: ball\n    shape: sphere\n    radius: 0.5\n    material: S\n{block}"
            ));
            let doc = parse(&source).expect("parses");
            assert!(validate(&doc).is_empty(), "{:#?}", validate(&doc));
            let once = emit(&doc);
            assert!(once.ends_with(block), "{once}");
            assert_eq!(emit(&parse(&once).expect("re-parses")), once);
        }
    }

    /// A number that overflows `f32` once narrowed reads as infinite, so validation rejects it
    /// instead of the mesher receiving `inf`.
    #[test]
    fn a_number_past_the_f32_range_is_non_finite_at_validation() {
        let source = IDOL.replace("radius: 0.25", "radius: 1e39");
        assert_ne!(source, IDOL, "the idol has a `radius: 0.25` to replace");
        let doc = parse(&source).unwrap_or_else(|d| panic!("{d:#?}"));
        let diagnostics = validate(&doc);
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1157"
                && d.path.as_deref().is_some_and(|p| p.ends_with("/radius"))
                && d.message.contains("non-finite")),
            "{diagnostics:#?}"
        );
    }

    // --- L1156 ------------------------------------------------------------------------------------

    #[test]
    fn structure_defects_are_l1156_at_their_pointer() {
        let d = parse_err(
            "format: 1\nkind: sculpt\npalette: {}\nbones: []\n",
            "L1156",
            "",
        );
        assert!(d.message.contains("`parts`"), "{}", d.message);
        parse_err(
            &IDOL.replace("kind: sculpt", "kind: voxel"),
            "L1156",
            "/kind",
        );
        parse_err(
            &IDOL.replace("resolution: 32", "resolution: 32.5"),
            "L1156",
            "/resolution",
        );
        parse_err(
            &IDOL.replace("sharp: true", "sharp: yes"),
            "L1156",
            "/parts/0/sharp",
        );
        parse_err(
            &IDOL.replace("count: 5", "count: many"),
            "L1156",
            "/parts/5/repeat/count",
        );
        parse_err(
            &with_parts(
                "  - name: ball\n    shape: sphere\n    radius: 0.5\n    at:\n      - 1.0\n",
            ),
            "L1156",
            "/parts/0/at",
        );
        let d = parse_err(
            &with_parts("  - name: ball\n    shape: sphere\n"),
            "L1156",
            "/parts/0",
        );
        assert!(d.message.contains("`radius`"), "{}", d.message);
        parse_err(
            &with_parts("  - shape: sphere\n    radius: 1.0\n"),
            "L1156",
            "/parts/0",
        );
        parse_err(&with_parts("  - just a string\n"), "L1156", "/parts/0");
        parse_err(
            &IDOL.replace("  - name: head\n    parent: root\n", "  - parent: root\n"),
            "L1156",
            "/bones/1",
        );
    }

    #[test]
    fn unknown_keys_and_values_carry_a_nearest_match() {
        let d = parse_err(&IDOL.replace("parts:", "partz:"), "L1156", "/partz");
        assert_eq!(d.help.as_deref(), Some("nearest match is `parts`"));
        let d = parse_err(
            &IDOL.replace("radius: 0.05", "radis: 0.05"),
            "L1156",
            "/parts/3/radis",
        );
        assert_eq!(d.help.as_deref(), Some("nearest match is `radius`"));
        let d = parse_err(
            &IDOL.replace("shape: sphere", "shape: sphre"),
            "L1156",
            "/parts/3/shape",
        );
        assert_eq!(d.help.as_deref(), Some("nearest match is `sphere`"));
        let d = parse_err(
            &IDOL.replace("op: subtract", "op: subtact"),
            "L1156",
            "/parts/4/op",
        );
        assert_eq!(d.help.as_deref(), Some("nearest match is `subtract`"));
        parse_err(
            &IDOL.replace("mirror: x", "mirror: w"),
            "L1156",
            "/parts/3/mirror",
        );
        parse_err(
            &IDOL.replace("    parent: root\n", "    parnt: root\n"),
            "L1156",
            "/bones/1/parnt",
        );
        parse_err(
            &IDOL.replace("      count: 5\n", "      count: 5\n      cnt: 1\n"),
            "L1156",
            "/parts/5/repeat/cnt",
        );
    }

    // --- L1157 ------------------------------------------------------------------------------------

    #[test]
    fn a_key_of_another_shape_is_l1157_with_the_shape_key_list() {
        let source = with_parts(
            "  - name: slab\n    shape: box\n    size:\n      - 1.0\n      - 1.0\n      - 1.0\n    \
             radii:\n      - 1.0\n      - 1.0\n      - 1.0\n    material: S\n",
        );
        let diagnostics = parse(&source).unwrap_err();
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        let d = assert_reported(&diagnostics, "L1157", "/parts/0/radii");
        assert!(
            d.message.contains("`box`") && d.message.contains("`radii`"),
            "{}",
            d.message
        );
        assert_eq!(d.help.as_deref(), Some("the `box` keys are: size, round"));

        let d = parse_err(
            &IDOL.replace("    radius: 0.04\n", "    radius: 0.04\n    tube: 0.1\n"),
            "L1157",
            "/parts/5/tube",
        );
        assert!(
            d.help.as_deref().unwrap().contains("radius, height"),
            "{d:?}"
        );

        let d = parse_err(
            &IDOL.replace(
                "    radius: 0.25\n",
                "    radius: 0.25\n    at:\n      - 0.0\n      - 0.0\n      - 0.0\n",
            ),
            "L1157",
            "/parts/1/at",
        );
        assert!(d.message.contains("`from` and `to`"), "{}", d.message);
    }

    #[test]
    fn data_defects_are_l1157_at_their_pointer() {
        let mut doc = idol();
        doc.resolution = 200;
        validate_err(&doc, "L1157", "/resolution");
        doc.resolution = 3;
        validate_err(&doc, "L1157", "/resolution");

        let mut doc = idol();
        doc.triangles = 11;
        validate_err(&doc, "L1157", "/triangles");

        let mut doc = idol();
        doc.budget_exempt = Some(false);
        validate_err(&doc, "L1157", "/budget_exempt");

        let mut doc = idol();
        doc.parts[2].name = "body".into();
        validate_err(&doc, "L1157", "/parts/2/name");

        let mut doc = idol();
        doc.parts[3].material = Some("Q".into());
        let d = validate_err(&doc, "L1157", "/parts/3/material");
        assert!(d.help.as_deref().unwrap().contains("`E`, `S`"), "{d:?}");

        let mut doc = idol();
        doc.parts[0].material = None;
        validate_err(&doc, "L1157", "/parts/0");
        // The idol validates clean with neither its subtract nor its intersect carrying one.
        assert!(doc.parts[4].material.is_none() && doc.parts[6].material.is_none());

        let mut doc = idol();
        doc.parts[4].material = Some("S".into());
        let d = validate_err(&doc, "L1157", "/parts/4/material");
        assert!(
            d.message
                .contains("a subtract/intersect part contributes no material; remove `material`"),
            "{}",
            d.message
        );
        let mut doc = idol();
        doc.parts[6].material = Some("E".into());
        validate_err(&doc, "L1157", "/parts/6/material");

        let mut doc = idol();
        doc.parts[5].repeat.as_mut().unwrap().count = 1;
        validate_err(&doc, "L1157", "/parts/5/repeat/count");

        let mut doc = idol();
        doc.parts[5].repeat.as_mut().unwrap().count = SCULPT_MAX_REPEAT;
        assert!(validate(&doc).is_empty(), "{:#?}", validate(&doc));
        doc.parts[5].repeat.as_mut().unwrap().count = SCULPT_MAX_REPEAT + 1;
        validate_err(&doc, "L1157", "/parts/5/repeat/count");

        let mut doc = idol();
        doc.parts[1].blend = -0.1;
        validate_err(&doc, "L1157", "/parts/1/blend");

        let mut doc = idol();
        doc.parts[3].shape = SculptShape::Sphere { radius: 0.0 };
        validate_err(&doc, "L1157", "/parts/3/radius");

        let mut doc = idol();
        doc.parts[2].shape = SculptShape::Ellipsoid {
            radii: [0.2, -1.0, 0.2],
        };
        validate_err(&doc, "L1157", "/parts/2/radii/1");

        let mut doc = idol();
        doc.parts[0].shape = SculptShape::Box {
            size: [1.2, 0.3, 1.2],
            round: 0.2,
        };
        validate_err(&doc, "L1157", "/parts/0/round");

        let mut doc = idol();
        doc.parts[0].at[1] = f64::NAN;
        validate_err(&doc, "L1157", "/parts/0/at/1");

        let mut doc = idol();
        doc.parts[1].at = [0.0, 1.0, 0.0];
        validate_err(&doc, "L1157", "/parts/1/at");

        let mut doc = idol();
        doc.palette[1].glyph = "SS".into();
        validate_err(&doc, "L1157", "/palette/SS");
    }

    #[test]
    fn deform_and_noise_defects_are_l1157_at_their_pointer() {
        let turn = |degrees| {
            Some(SculptTurn {
                axis: SculptAxis::Y,
                degrees,
            })
        };
        let deformed = |deform: SculptDeform| {
            let mut doc = idol();
            doc.parts[1].deform = Some(deform);
            doc
        };

        let doc = deformed(SculptDeform::default());
        let d = validate_err(&doc, "L1157", "/parts/1/deform");
        assert!(d.message.contains("is empty"), "{}", d.message);

        let doc = deformed(SculptDeform {
            bend: turn(f64::INFINITY),
            ..SculptDeform::default()
        });
        validate_err(&doc, "L1157", "/parts/1/deform/bend/degrees");

        let doc = deformed(SculptDeform {
            twist: turn(f64::NAN),
            ..SculptDeform::default()
        });
        validate_err(&doc, "L1157", "/parts/1/deform/twist/degrees");

        for (from, to, field) in [
            (0.0, 1.0, "from"),
            (1.0, -0.5, "to"),
            (f64::NAN, 1.0, "from"),
        ] {
            let doc = deformed(SculptDeform {
                taper: Some(SculptTaper {
                    axis: SculptAxis::Y,
                    from,
                    to,
                }),
                ..SculptDeform::default()
            });
            validate_err(&doc, "L1157", &format!("/parts/1/deform/taper/{field}"));
        }

        let noised = |amplitude, scale| {
            let mut doc = idol();
            doc.parts[1].noise = Some(SculptNoise {
                amplitude,
                scale,
                seed: 0,
            });
            doc
        };
        validate_err(&noised(-0.1, 0.5), "L1157", "/parts/1/noise/amplitude");
        validate_err(&noised(f64::NAN, 0.5), "L1157", "/parts/1/noise/amplitude");
        validate_err(&noised(0.1, 0.0), "L1157", "/parts/1/noise/scale");
        assert!(validate(&noised(0.0, 0.5)).is_empty());
    }

    #[test]
    fn deform_and_noise_structure_defects_are_l1156() {
        let with_block = |block: &str| {
            with_parts(&format!(
                "  - name: ball\n    shape: sphere\n    radius: 0.5\n    material: S\n{block}"
            ))
        };
        parse_err(&with_block("    deform: 3\n"), "L1156", "/parts/0/deform");
        let d = parse_err(
            &with_block("    deform:\n      bnd:\n        axis: x\n        degrees: 1.0\n"),
            "L1156",
            "/parts/0/deform/bnd",
        );
        assert_eq!(d.help.as_deref(), Some("nearest match is `bend`"));
        parse_err(
            &with_block("    deform:\n      twist:\n        axis: w\n        degrees: 1.0\n"),
            "L1156",
            "/parts/0/deform/twist/axis",
        );
        parse_err(
            &with_block("    deform:\n      taper:\n        axis: y\n        from: 1.0\n"),
            "L1156",
            "/parts/0/deform/taper",
        );
        parse_err(
            &with_block("    noise:\n      amplitude: 0.1\n      scale: 0.2\n      seed: -1\n"),
            "L1156",
            "/parts/0/noise/seed",
        );
        parse_err(
            &with_block("    noise:\n      amplitude: 0.1\n      scale: 0.2\n"),
            "L1156",
            "/parts/0/noise",
        );
        let d = parse_err(&with_block("    defrm: {}\n"), "L1156", "/parts/0/defrm");
        assert_eq!(d.help.as_deref(), Some("nearest match is `deform`"));
    }

    // --- L1160 ------------------------------------------------------------------------------------

    #[test]
    fn bone_tree_defects_are_l1160_at_their_pointer() {
        let bone = |name: &str, parent: Option<&str>| SculptBone {
            name: name.into(),
            parent: parent.map(Into::into),
            head: None,
            tail: None,
        };

        let mut doc = idol();
        doc.bones[1].parent = Some("rot".into());
        let d = validate_err(&doc, "L1160", "/bones/1/parent");
        assert_eq!(d.help.as_deref(), Some("nearest match is `root`"));

        let mut doc = idol();
        doc.bones.push(bone("tail", None));
        validate_err(&doc, "L1160", "/bones/2");

        let mut doc = idol();
        doc.bones[0].parent = Some("head".into());
        validate_err(&doc, "L1160", "/bones/0/parent");

        let mut doc = idol();
        doc.bones = vec![
            bone("root", None),
            bone("a", Some("b")),
            bone("b", Some("a")),
        ];
        doc.parts.iter_mut().for_each(|part| part.bone = None);
        let d = validate_err(&doc, "L1160", "/bones/1/parent");
        assert!(d.message.contains("`a` -> `b` -> `a`"), "{}", d.message);

        let mut doc = idol();
        doc.bones[1].name = "root".into();
        validate_err(&doc, "L1160", "/bones/1/name");

        let mut doc = idol();
        doc.parts[3].bone = Some("haed".into());
        let d = validate_err(&doc, "L1160", "/parts/3/bone");
        assert_eq!(d.help.as_deref(), Some("nearest match is `head`"));

        let mut doc = idol();
        doc.bones.clear();
        validate_err(&doc, "L1160", "/bones");
    }

    #[test]
    fn a_half_authored_or_non_finite_bone_position_is_l1160() {
        let mut doc = idol();
        doc.bones[1].head = Some([0.0, 1.1, 0.0]);
        let d = validate_err(&doc, "L1160", "/bones/1/head");
        assert!(d.message.contains("no `tail`"), "{}", d.message);

        let mut doc = idol();
        doc.bones[1].tail = Some([0.0, 1.4, 0.0]);
        validate_err(&doc, "L1160", "/bones/1/tail");

        let mut doc = idol();
        doc.bones[0].head = Some([0.0, 0.0, 0.0]);
        doc.bones[0].tail = Some([0.0, f64::NAN, 0.0]);
        validate_err(&doc, "L1160", "/bones/0/tail/1");

        let mut doc = idol();
        doc.bones[0].head = Some([0.0, 0.0, 0.0]);
        doc.bones[0].tail = Some([0.0, 1.1, 0.0]);
        assert!(validate(&doc).is_empty(), "{:#?}", validate(&doc));
    }

    // --- Overrides (L4008) ------------------------------------------------------------------------

    fn set(doc: &mut SculptDocument, spec: &str) -> Result<Vec<AppliedOverride>, Vec<Diagnostic>> {
        apply_sculpt_overrides(doc, &[SceneOverride::parse(spec).unwrap()])
    }

    #[test]
    fn numeric_pointers_override_their_field() {
        let mut doc = idol();
        let applied = set(&mut doc, "/parts/2/blend=0.2").unwrap();
        assert_eq!(applied[0].from, Some(Value::from(0.08)));
        assert_eq!(doc.parts[2].blend, 0.2);

        set(&mut doc, "/parts/0/size/1=0.5").unwrap();
        assert_eq!(
            doc.parts[0].shape,
            SculptShape::Box {
                size: [1.2, 0.5, 1.2],
                round: 0.02
            }
        );
        set(&mut doc, "/parts/1/to/1=1.4").unwrap();
        set(&mut doc, "/parts/6/tube=0.05").unwrap();
        set(&mut doc, "/parts/5/repeat/count=7").unwrap();
        set(&mut doc, "/parts/5/repeat/step/1=0.1").unwrap();
        set(&mut doc, "/triangles=12000").unwrap();
        set(&mut doc, "/palette/S/roughness=0.5").unwrap();
        assert_eq!(doc.parts[5].repeat.as_ref().unwrap().count, 7);
        assert_eq!(doc.triangles, 12000);
        assert_eq!(doc.palette_entry("S").unwrap().roughness, Some(0.5));

        let applied = set(&mut doc, "/parts/3/rotation/2=45").unwrap();
        assert_eq!(applied[0].from, None, "an unset rotation is created");
        assert_eq!(doc.parts[3].rotation, Some([0.0, 0.0, 45.0]));
        assert!(validate(&doc).is_empty());
    }

    #[test]
    fn an_override_the_document_rejects_is_its_own_error() {
        for (spec, path) in [
            ("/parts/5/repeat/count=4000000000", "/parts/5/repeat/count"),
            ("/resolution=100000", "/resolution"),
            ("/palette/S/roughness=2", "/palette/S/roughness"),
        ] {
            let mut doc = idol();
            let diagnostics = set(&mut doc, spec).expect_err(spec);
            assert_reported(&diagnostics, "L1157", path);
        }
    }

    #[test]
    fn a_pointer_that_is_not_a_numeric_field_is_l4008() {
        for spec in [
            "/parts/0/op=1",
            "/parts/0/material=1",
            "/parts/9/blend=0.1",
            "/parts/0/radii/0=1",
            "/parts/0/at/3=1",
            "/parts/0/at=1",
            "/parts/1/at/0=1",
            "/parts/0/repeat/count=2",
            "/parts/0/blend=\"soft\"",
            "/resolution=1.5",
            "/bones/0/name=1",
            "/name=1",
            "/palette/Q/emissive=1",
        ] {
            let mut doc = idol();
            let diagnostics = set(&mut doc, spec).expect_err(spec);
            assert_eq!(diagnostics.len(), 1, "{spec}");
            assert_eq!(diagnostics[0].code.as_str(), "L4008", "{spec}");
            let pointer = spec.split('=').next().unwrap();
            assert_eq!(diagnostics[0].path.as_deref(), Some(pointer), "{spec}");
        }
    }

    // --- shape: voxel ----------------------------------------------------------------------------

    /// A voxel-shaped part round-trips: `path` then `size` in the shape's own key order, with every
    /// key that belongs to any part still legal on it.
    #[test]
    fn a_voxel_part_round_trips_with_every_common_key() {
        const SOURCE: &str = "\
format: 1
kind: sculpt
resolution: 48
triangles: 5000
palette:
  S:
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
bones:
  - name: root
  - name: head
    parent: root
parts:
  - name: hull
    shape: voxel
    path: \"../voxels/hull.voxel.yaml\"
    size: 0.125
    at:
      - 0.0
      - 0.2
      - 0.0
    rotation:
      - 0.0
      - 90.0
      - 0.0
    blend: 0.05
    mirror: x
    material: S
    bone: head
    sharp: true
    deform:
      twist:
        axis: y
        degrees: 12.0
    noise:
      amplitude: 0.01
      scale: 0.2
      seed: 7
";
        let doc = crate::check_sculpt(SOURCE).unwrap_or_else(|d| panic!("{d:#?}"));
        assert_eq!(
            doc.parts[0].shape,
            SculptShape::Voxel {
                path: "../voxels/hull.voxel.yaml".to_string(),
                size: 0.125,
            }
        );
        assert_eq!(doc.parts[0].shape.keys(), ["path", "size"]);
        assert_eq!(emit(&doc), SOURCE);
        assert_eq!(emit(&parse(&emit(&doc)).expect("re-parses")), SOURCE);
    }

    /// `size` is one scalar on a `voxel` and three extents on a `box`, so the shape decides how the
    /// key is read; a key belonging to another shape is the ordinary misfit `L1157`.
    #[test]
    fn a_voxel_part_takes_its_own_keys_only() {
        let d = parse_err(
            &with_parts(
                "  - name: hull\n    shape: voxel\n    path: hull.voxel.yaml\n    radius: 0.5\n",
            ),
            "L1157",
            "/parts/0/radius",
        );
        assert!(d.help.as_deref().unwrap().contains("path, size"), "{d:?}");

        let d = parse_err(
            &with_parts(
                "  - name: hull\n    shape: voxel\n    path: hull.voxel.yaml\n    size:\n      \
                 - 1.0\n      - 1.0\n      - 1.0\n",
            ),
            "L1156",
            "/parts/0/size",
        );
        assert!(d.message.contains("must be a number"), "{}", d.message);

        // A `box`'s three extents keep reading as a vector.
        let doc = crate::check_sculpt(&with_parts(
            "  - name: slab\n    shape: box\n    size:\n      - 1.0\n      - 0.2\n      - 1.0\n    \
             material: S\n",
        ))
        .unwrap_or_else(|d| panic!("{d:#?}"));
        assert_eq!(doc.parts[0].shape.name(), "box");
    }

    /// The data rules a voxel part adds: a path that names nothing, and a non-positive scale.
    #[test]
    fn voxel_part_data_defects_are_l1157_at_their_pointer() {
        let source = with_parts(
            "  - name: hull\n    shape: voxel\n    path: hull.voxel.yaml\n    size: 0.125\n    \
             material: S\n",
        );
        let mut doc = parse(&source).unwrap_or_else(|d| panic!("{d:#?}"));
        assert!(validate(&doc).is_empty(), "{:#?}", validate(&doc));

        doc.parts[0].shape = SculptShape::Voxel {
            path: String::new(),
            size: 0.125,
        };
        let d = validate_err(&doc, "L1157", "/parts/0/path");
        assert!(d.message.contains("empty `path`"), "{}", d.message);

        doc.parts[0].shape = SculptShape::Voxel {
            path: "hull.voxel.yaml".to_string(),
            size: 0.0,
        };
        validate_err(&doc, "L1157", "/parts/0/size");
    }

    /// A part's `path` may climb with `..` but not name an absolute, drive or device path.
    #[test]
    fn part_paths_outside_the_content_root_are_l1157() {
        for kind in ["voxel", "mesh"] {
            let source = with_parts(if kind == "voxel" {
                "  - name: hull\n    shape: voxel\n    path: hull.voxel.yaml\n    size: 0.125\n    \
                 material: S\n"
            } else {
                "  - name: idol\n    shape: mesh\n    path: idol.mesh.yaml\n    material: S\n"
            });
            let mut doc = parse(&source).unwrap_or_else(|d| panic!("{d:#?}"));
            let shape = |path: &str| {
                let path = path.to_string();
                if kind == "voxel" {
                    SculptShape::Voxel { path, size: 0.125 }
                } else {
                    SculptShape::Mesh { path, size: None }
                }
            };
            for path in ["C:/x", "/x", "sub/nul.yaml"] {
                doc.parts[0].shape = shape(path);
                validate_err(&doc, "L1157", "/parts/0/path");
            }
            doc.parts[0].shape = shape("../ok.yaml");
            assert!(validate(&doc).is_empty(), "{:#?}", validate(&doc));
        }
    }

    /// A voxel part's `size` is one number, so it is addressed whole — the `radius` rule, not the
    /// `box` `size/<axis>` one.
    #[test]
    fn a_voxel_parts_size_is_a_scalar_pointer() {
        let source = with_parts(
            "  - name: hull\n    shape: voxel\n    path: hull.voxel.yaml\n    size: 0.125\n    \
             material: S\n",
        );
        let mut doc = crate::check_sculpt(&source).unwrap_or_else(|d| panic!("{d:#?}"));
        let applied = set(&mut doc, "/parts/0/size=0.25").unwrap();
        assert_eq!(applied[0].from, Some(Value::from(0.125)));
        assert_eq!(
            doc.parts[0].shape,
            SculptShape::Voxel {
                path: "hull.voxel.yaml".to_string(),
                size: 0.25,
            }
        );
        for spec in ["/parts/0/size/0=0.25", "/parts/0/path=1"] {
            let diagnostics = set(&mut doc, spec).expect_err(spec);
            assert_eq!(diagnostics[0].code.as_str(), "L4008", "{spec}");
        }
    }

    // --- shape: mesh -----------------------------------------------------------------------------

    /// A mesh-shaped part round-trips with `path` then `size`, and an omitted `size` stays omitted:
    /// it is the one shape key with a meaningful default, since a mesh document is already in world
    /// units.
    #[test]
    fn a_mesh_part_round_trips_with_and_without_its_size() {
        const HEAD: &str = "\
format: 1
kind: sculpt
resolution: 48
triangles: 5000
palette:
  S:
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
bones:
  - name: root
parts:
";
        let scaled = format!(
            "{HEAD}  - name: idol\n    shape: mesh\n    path: \"../meshes/idol.mesh.yaml\"\n    \
             size: 1.5\n    blend: 0.05\n    material: S\n"
        );
        let doc = crate::check_sculpt(&scaled).unwrap_or_else(|d| panic!("{d:#?}"));
        assert_eq!(
            doc.parts[0].shape,
            SculptShape::Mesh {
                path: "../meshes/idol.mesh.yaml".to_string(),
                size: Some(1.5),
            }
        );
        assert_eq!(doc.parts[0].shape.keys(), ["path", "size"]);
        assert_eq!(emit(&doc), scaled);

        let plain = format!(
            "{HEAD}  - name: idol\n    shape: mesh\n    path: \"../meshes/idol.mesh.yaml\"\n    \
             material: S\n"
        );
        let doc = crate::check_sculpt(&plain).unwrap_or_else(|d| panic!("{d:#?}"));
        assert_eq!(
            doc.parts[0].shape,
            SculptShape::Mesh {
                path: "../meshes/idol.mesh.yaml".to_string(),
                size: None,
            }
        );
        assert_eq!(emit(&doc), plain);
    }

    /// A `mesh` part takes `path` and `size` only, and its `size` is one scalar like a `voxel`'s.
    #[test]
    fn a_mesh_part_takes_its_own_keys_only() {
        let d = parse_err(
            &with_parts(
                "  - name: idol\n    shape: mesh\n    path: idol.mesh.yaml\n    radius: 0.5\n",
            ),
            "L1157",
            "/parts/0/radius",
        );
        assert!(d.help.as_deref().unwrap().contains("path, size"), "{d:?}");

        let d = parse_err(
            &with_parts(
                "  - name: idol\n    shape: mesh\n    path: idol.mesh.yaml\n    size:\n      \
                 - 1.0\n      - 1.0\n      - 1.0\n",
            ),
            "L1156",
            "/parts/0/size",
        );
        assert!(d.message.contains("must be a number"), "{}", d.message);

        // `path` is the one required key: a `mesh` part without one is the structure code.
        parse_err(
            &with_parts("  - name: idol\n    shape: mesh\n    material: S\n"),
            "L1156",
            "/parts/0",
        );
    }

    /// The data rules a mesh part adds: an empty path, and a non-positive scale when authored.
    #[test]
    fn mesh_part_data_defects_are_l1157_at_their_pointer() {
        let source = with_parts(
            "  - name: idol\n    shape: mesh\n    path: idol.mesh.yaml\n    material: S\n",
        );
        let mut doc = parse(&source).unwrap_or_else(|d| panic!("{d:#?}"));
        assert!(validate(&doc).is_empty(), "{:#?}", validate(&doc));

        doc.parts[0].shape = SculptShape::Mesh {
            path: String::new(),
            size: None,
        };
        let d = validate_err(&doc, "L1157", "/parts/0/path");
        assert!(d.message.contains("empty `path`"), "{}", d.message);

        doc.parts[0].shape = SculptShape::Mesh {
            path: "idol.mesh.yaml".to_string(),
            size: Some(0.0),
        };
        validate_err(&doc, "L1157", "/parts/0/size");
    }

    /// A mesh part's `size` is a scalar pointer like a voxel's — the `a_voxel_parts_size_is_a_scalar_pointer`
    /// rule, on a part that already authors one.
    #[test]
    fn a_mesh_parts_authored_size_is_a_scalar_pointer() {
        let source = with_parts(
            "  - name: idol\n    shape: mesh\n    path: idol.mesh.yaml\n    size: 1.5\n    \
             material: S\n",
        );
        let mut doc = crate::check_sculpt(&source).unwrap_or_else(|d| panic!("{d:#?}"));
        let applied = set(&mut doc, "/parts/0/size=2.0").unwrap();
        assert_eq!(applied[0].from, Some(Value::from(1.5)));
        assert_eq!(
            doc.parts[0].shape,
            SculptShape::Mesh {
                path: "idol.mesh.yaml".to_string(),
                size: Some(2.0),
            }
        );
        for spec in ["/parts/0/size/0=0.25", "/parts/0/path=1"] {
            let diagnostics = set(&mut doc, spec).expect_err(spec);
            assert_eq!(diagnostics[0].code.as_str(), "L4008", "{spec}");
        }
    }

    /// An override on a mesh part's omitted `size` creates it — `from: None` marks the creation,
    /// the way an unauthored `rotation` does — set to the override's value, not the `1` default.
    #[test]
    fn an_override_creates_a_mesh_parts_omitted_size() {
        let source = with_parts(
            "  - name: idol\n    shape: mesh\n    path: idol.mesh.yaml\n    material: S\n",
        );
        let mut doc = crate::check_sculpt(&source).unwrap_or_else(|d| panic!("{d:#?}"));
        assert_eq!(
            doc.parts[0].shape,
            SculptShape::Mesh {
                path: "idol.mesh.yaml".to_string(),
                size: None,
            }
        );
        let applied = set(&mut doc, "/parts/0/size=0.6").unwrap();
        assert_eq!(applied[0].from, None);
        assert_eq!(
            doc.parts[0].shape,
            SculptShape::Mesh {
                path: "idol.mesh.yaml".to_string(),
                size: Some(0.6),
            }
        );
        assert!(emit(&doc).contains("    size: 0.6\n"), "{}", emit(&doc));
    }

    /// The `L4008` field listing for a `mesh` part names `size` among its own fields, whether or not
    /// the part authored one.
    #[test]
    fn a_mesh_parts_unsupported_pointer_lists_size_in_its_help() {
        let source = with_parts(
            "  - name: idol\n    shape: mesh\n    path: idol.mesh.yaml\n    material: S\n",
        );
        let mut doc = crate::check_sculpt(&source).unwrap_or_else(|d| panic!("{d:#?}"));
        let d = set(&mut doc, "/parts/0/height=1.0").unwrap_err();
        assert_eq!(d[0].code.as_str(), "L4008");
        assert!(
            d[0].help.as_deref().unwrap().contains("path, size"),
            "{d:?}"
        );
    }

    // --- Deform, noise and bone-position pointers -------------------------------------------------

    /// The document with a deformed, noised part and a positioned bone that the pointer tests drive.
    fn deformed() -> SculptDocument {
        let source = format!(
            "format: 1\nkind: sculpt\npalette:\n  S:\n    color:\n      - 1.0\n      - 1.0\n  \
             \x20   - 1.0\n      - 1.0\nbones:\n  - name: root\n    head:\n      - 0.0\n      \
             - 0.0\n      - 0.0\n    tail:\n      - 0.0\n      - 1.0\n      - 0.0\n  - name: \
             tip\n    parent: root\nparts:\n{}",
            "  - name: limb\n    shape: capsule\n    radius: 0.2\n    from:\n      - 0.0\n      \
             - 0.0\n      - 0.0\n    to:\n      - 0.0\n      - 1.0\n      - 0.0\n    \
             material: S\n    deform:\n      bend:\n        axis: y\n        degrees: 30.0\n   \
             \x20  taper:\n        axis: y\n        from: 1.0\n        to: 0.5\n    noise:\n  \
             \x20   amplitude: 0.02\n      scale: 0.1\n      seed: 3\n"
        );
        crate::check_sculpt(&source).unwrap_or_else(|d| panic!("{d:#?}"))
    }

    #[test]
    fn deform_noise_and_bone_pointers_override_their_field() {
        let mut doc = deformed();
        let applied = set(&mut doc, "/parts/0/deform/bend/degrees=45").unwrap();
        assert_eq!(applied[0].from, Some(Value::from(30.0)));
        assert_eq!(
            doc.parts[0].deform.as_ref().unwrap().bend.unwrap().degrees,
            45.0
        );

        set(&mut doc, "/parts/0/deform/taper/from=0.8").unwrap();
        set(&mut doc, "/parts/0/deform/taper/to=0.25").unwrap();
        let taper = doc.parts[0].deform.as_ref().unwrap().taper.unwrap();
        assert_eq!((taper.from, taper.to), (0.8, 0.25));

        set(&mut doc, "/parts/0/noise/amplitude=0.05").unwrap();
        set(&mut doc, "/parts/0/noise/scale=0.3").unwrap();
        let noise = doc.parts[0].noise.unwrap();
        assert_eq!((noise.amplitude, noise.scale, noise.seed), (0.05, 0.3, 3));

        let applied = set(&mut doc, "/bones/0/head/1=-0.25").unwrap();
        assert_eq!(applied[0].from, Some(Value::from(0.0)));
        set(&mut doc, "/bones/0/tail/1=1.5").unwrap();
        assert_eq!(doc.bones[0].head, Some([0.0, -0.25, 0.0]));
        assert_eq!(doc.bones[0].tail, Some([0.0, 1.5, 0.0]));
        assert!(validate(&doc).is_empty(), "{:#?}", validate(&doc));
    }

    /// A pointer whose parent the part (or bone) does not carry is the same `L4008` an absent
    /// `repeat` raises, and it names what is missing — never a half-written `deform` or half a
    /// head/tail pair.
    #[test]
    fn a_pointer_whose_parent_is_absent_names_it() {
        for (spec, absent) in [
            ("/parts/0/deform/bend/degrees=10", "deform"),
            ("/parts/0/noise/amplitude=0.1", "noise"),
            ("/bones/1/head/0=1", "head"),
        ] {
            let mut doc = deformed();
            doc.parts[0].deform = None;
            doc.parts[0].noise = None;
            let diagnostics = set(&mut doc, spec).expect_err(spec);
            assert_eq!(diagnostics[0].code.as_str(), "L4008", "{spec}");
            let pointer = spec.split('=').next().unwrap();
            assert_eq!(diagnostics[0].path.as_deref(), Some(pointer), "{spec}");
            assert!(
                diagnostics[0].message.contains(&format!("`{absent}`")),
                "{spec}: {}",
                diagnostics[0].message
            );
            assert!(diagnostics[0].message.ends_with("which has none"), "{spec}");
        }

        // The `deform` is there but the named deformer is not.
        let mut doc = deformed();
        let diagnostics = set(&mut doc, "/parts/0/deform/twist/degrees=10").expect_err("no twist");
        assert!(
            diagnostics[0].message.contains("`twist`"),
            "{}",
            diagnostics[0].message
        );
        assert_eq!(doc.parts[0].deform.as_ref().unwrap().twist, None);

        // A bone index and a sub-key the domain does not hold.
        for spec in [
            "/bones/9/head/0=1",
            "/bones/0/head/3=1",
            "/bones/0/parent=1",
            "/parts/0/noise/seed=9",
            "/parts/0/deform/taper/axis=1",
        ] {
            let mut doc = deformed();
            let diagnostics = set(&mut doc, spec).expect_err(spec);
            assert_eq!(diagnostics[0].code.as_str(), "L4008", "{spec}");
        }
    }
}
