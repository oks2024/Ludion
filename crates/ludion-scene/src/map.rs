//! Map source documents (`*.map.yaml`, `kind: map`) and the shared kit (`kind: map-kit`) — the
//! glyph-grid authoring layer that compiles into a canonical [`Scene`] of real entities.
//!
//! Two `kind:`-dispatched documents, hand-parsed like [`crate::mesh`]/[`crate::voxel`] and sharing
//! the same strict-YAML layer ([`crate::yaml`]):
//!
//! - **`kind: map`** ([`MapDoc`]): one district's source — a glyph grid (`rows`) laid on the XZ
//!   ground plane (`col → +X`, `row → +Z`, first row lowest Z), a sparse [`MapMarker`] list for
//!   off-grid landmarks, a `fragments:` list of hand-authored scene files merged verbatim, and a
//!   `kit:` path (a *string* — the CLI resolves it; the generator receives the parsed kit).
//! - **`kind: map-kit`** ([`MapKit`]): the shared legend — each glyph (`tiles:`) and each marker
//!   kind (`kinds:`) maps to an ordered list of engine primitives ([`KitPrimitive`]: a mesh source,
//!   an optional collider, a `y:` offset — where the game bakes its floor rule — a `scale`, optional
//!   `variants`, and an expansion mode). The kit references only engine builtins; it knows nothing
//!   about the game's vocabulary ("house", "well"). Its optional `probe:` ([`MapProbe`]) names the
//!   walker the CLI's `map-check` measures with; the generator ignores it.
//!
//! [`generate`] is a **pure, deterministic, IO-free** function: `(map, kit, already-parsed
//! fragments) → Scene`. Same inputs → byte-identical [`crate::emit`] output. It does no file IO —
//! the CLI resolves the kit path, reads the fragment files, and parses them before calling
//! in. Cross-district gate anchors are validated by `map-check` (raising
//! `code::MAP_DANGLING_GATE_ANCHOR`), which is why that check — needing to resolve a sibling map off
//! disk — is not in this generator.
//!
//! # Emission order and diff stability (the generator's obligation)
//!
//! The canonical writer preserves the `entities` sequence verbatim and enforces no name uniqueness,
//! so diff stability is entirely the generator's job: **coordinate-derived unique names**
//! (`floor-r012-c007`; merged rects encode extent) and a **fixed emission order** (fragments →
//! grid rects/props row-major → markers in source order). A one-glyph edit then touches only the
//! affected rect/entity, and **variant selection is seeded from `(seed, cell coords)`** — never
//! draw order — so an edit in one corner never reshuffles variants elsewhere.

use ludion_core::codes::code;
use ludion_core::collections::{StableMap, StableSet};
use ludion_core::diagnostics::Diagnostic;
use ludion_core::suggest;
use ludion_core::text::single_char;
use serde_json::{Value, json};

use crate::envelope;
use crate::parse::node_to_value;
use crate::yaml::{self, MapEntry, Node, NodeValue, pointer_token};
use crate::{Scene, SceneEntity};
use ludion_ecs::Registry;

/// The map / map-kit document format version this crate reads.
pub const MAP_FORMAT_VERSION: u64 = 1;

/// The reserved grid glyph meaning "empty cell": it emits nothing and is exempt from the kit-glyph
/// check (a visual hole in the grid — a gate gap, the space outside a building footprint).
const VOID_GLYPH: char = ' ';
/// Per-axis cap on a declared `bounds` extent (a `rows:` map is bounded by its text). It bounds the
/// grid only; `map-check` caps its own sampling lattice separately, by sample count.
const MAX_MAP_EXTENT: u32 = 1024;

const MAP_KEYS: [&str; 9] = [
    "format",
    "kind",
    "name",
    "seed",
    "kit",
    "cell_size",
    "origin",
    "base",
    "rows",
];
const MAP_KEYS_EXTRA: [&str; 4] = ["markers", "fragments", "bounds", "physics"];
const MARKER_KEYS: [&str; 7] = [
    "name",
    "kind",
    "x",
    "z",
    "rotation",
    "variant",
    "components",
];
const KIT_KEYS: [&str; 6] = ["format", "kind", "name", "tiles", "kinds", "probe"];
const PROBE_KEYS: [&str; 2] = ["radius", "speed"];
const PRIMITIVE_KEYS: [&str; 9] = [
    "name", "shape", "voxel", "mesh", "color", "collider", "y", "scale", "variants",
];
const PRIMITIVE_KEYS_EXTRA: [&str; 7] = [
    "merge",
    "footprint",
    "static",
    "sculpt",
    "roughness",
    "metallic",
    "emissive",
];
const COLLIDER_KEYS: [&str; 4] = ["aabb", "circle", "box", "surface"];

// --- Data model --------------------------------------------------------------------------------

/// A parsed, structurally valid map source document (`kind: map`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MapDoc {
    /// Optional scene name — becomes [`Scene::name`].
    pub name: Option<String>,
    /// Optional sim seed — becomes [`Scene::seed`] and seeds variant selection.
    pub seed: Option<u64>,
    /// The map-kit path, relative to the map file. Stored verbatim; the CLI resolves and parses it.
    pub kit: String,
    /// World units per grid cell (`> 0`).
    pub cell_size: f64,
    /// World XZ of the grid's `(col 0, row 0)` corner.
    pub origin: [f64; 2],
    /// Optional glyph whose kit primitives are emitted (rectangle-merged) under every non-void cell.
    pub base: Option<String>,
    /// The glyph grid, one equal-length string per row (`col → +X`, `row → +Z`).
    pub rows: Vec<String>,
    /// The grid extent in cells (`[width, height]`) of a grid-less, marker-only map; exclusive with
    /// `rows` and `base`. It frames the markers and emits no cells.
    pub bounds: Option<[u32; 2]>,
    /// The generated scene's `physics:` block (fragments cannot carry one — it would be dropped).
    pub physics: Option<crate::ScenePhysics>,
    /// Off-grid markers, in source order.
    pub markers: Vec<MapMarker>,
    /// Hand-authored scene files merged verbatim, first, in order.
    pub fragments: Vec<String>,
    /// The map source's file name, for [`Scene::generated_from`]. Not a YAML key — the CLI sets it
    /// before calling [`generate`]; the parser always leaves it `None`.
    pub source: Option<String>,
    /// The reflection applied to the compiled grid and markers. Not a YAML key — the caller sets it
    /// (see [`MapOverrides`]); the parser always leaves it [`Mirror::None`].
    pub mirror: Mirror,
}

/// The axis a compiled map is reflected across: `X` flips the room left-right, `Z` front-back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mirror {
    /// No reflection.
    #[default]
    None,
    /// Reflect across the grid's centre line in X.
    X,
    /// Reflect across the grid's centre line in Z.
    Z,
}

impl Mirror {
    /// The CLI/provenance spelling, `None` when there is no mirror.
    pub fn as_str(self) -> Option<&'static str> {
        match self {
            Mirror::None => None,
            Mirror::X => Some("x"),
            Mirror::Z => Some("z"),
        }
    }
}

/// Caller-side overrides of a map's kit, seed and mirror. Applied to the [`MapDoc`] before
/// [`generate`], so an overridden compile equals compiling a map file carrying that kit and seed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapOverrides {
    /// Replaces `kit:` (relative to the map file, like the map's own value).
    pub kit: Option<String>,
    /// Replaces `seed:`.
    pub seed: Option<u64>,
    /// Reflects the compiled output.
    pub mirror: Mirror,
}

impl MapOverrides {
    /// Sets the doc's kit, seed and mirror from whichever overrides are present. Run it before the
    /// kit is resolved.
    pub fn apply(&self, doc: &mut MapDoc) {
        if let Some(kit) = &self.kit {
            doc.kit = kit.clone();
        }
        if let Some(seed) = self.seed {
            doc.seed = Some(seed);
        }
        doc.mirror = self.mirror;
    }

    /// Whether no override is set.
    pub fn is_empty(&self) -> bool {
        self.kit.is_none() && self.seed.is_none() && self.mirror == Mirror::None
    }

    /// The one spelling `generated_from` records and `scripts/map-drift.mjs` parses: the map, then
    /// ` --kit <k>`, ` --seed <n>` and ` --mirror x|z` for the fields that are set, in that order.
    pub fn provenance(&self, map: &str) -> String {
        let mut out = map.to_string();
        if let Some(kit) = &self.kit {
            out.push_str(" --kit ");
            out.push_str(kit);
        }
        if let Some(seed) = self.seed {
            out.push_str(&format!(" --seed {seed}"));
        }
        if let Some(axis) = self.mirror.as_str() {
            out.push_str(" --mirror ");
            out.push_str(axis);
        }
        out
    }
}

impl MapDoc {
    /// The `bounds:` extent when it is what frames the map — declared and no `rows` to override it.
    fn frame(&self) -> Option<[u32; 2]> {
        self.bounds.filter(|_| self.rows.is_empty())
    }
}

/// One off-grid marker: a landmark placed at an exact world position.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MapMarker {
    /// Optional explicit entity-name prefix (else derived from `kind` + source index).
    pub name: Option<String>,
    /// Optional kit `kinds` entry to stamp (a marker may be pure `components` passthrough instead).
    pub kind: Option<String>,
    /// World X.
    pub x: f64,
    /// World Z.
    pub z: f64,
    /// Optional rotation about Y, in degrees.
    pub rotation: Option<f64>,
    /// Optional forced variant index into the kit entry's candidate mesh list.
    pub variant: Option<usize>,
    /// Optional verbatim components merged onto the marker's first (or only) entity.
    pub components: Vec<(String, Value)>,
}

impl MapMarker {
    /// The entity-name prefix this marker generates at source `index`: its explicit `name`, else
    /// `<kind>-<index>`, else `marker-<index>`. Shared by the generator's marker pass and the
    /// [`MapGrid::marker_table`] report so a table row's name matches the emitted entity.
    pub fn derived_name(&self, index: usize) -> String {
        self.name.clone().unwrap_or_else(|| match &self.kind {
            Some(kind) => format!("{kind}-{index}"),
            None => format!("marker-{index}"),
        })
    }
}

/// The pure grid geometry of a map — cell size, origin, and extent — with world↔cell conversions.
///
/// One shared coordinate implementation for the generator (its grid/marker passes) and
/// `ludion map-check` (the report ruler, the marker table, and the `--at` query), so neither
/// hand-rolls the arithmetic. IO-free and spatial-free: it needs only a [`MapDoc`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapGrid {
    origin: [f64; 2],
    cell_size: f64,
    width: usize,
    height: usize,
}

/// One row of [`MapGrid::marker_table`]: a marker resolved to its world position and grid cell.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkerCell {
    /// The entity-name prefix this marker generates (see [`MapMarker::derived_name`]).
    pub name: String,
    /// The marker's kit `kind`, if any (a pure-`components` marker has none).
    pub kind: Option<String>,
    /// The marker's world XZ.
    pub world: [f64; 2],
    /// The `(row, col)` cell the marker falls in, or `None` if it lies outside the grid.
    pub cell: Option<(usize, usize)>,
}

impl MapGrid {
    /// The grid extent read off a map doc: `width` is the longest row's cell count, `height` the row
    /// count (on a rectangular grid — the only kind that generates — every row has this width); a
    /// `bounds:` map reads its declared extent.
    pub fn new(doc: &MapDoc) -> Self {
        let (width, height) = match doc.frame() {
            Some([w, h]) => (w as usize, h as usize),
            None => (
                doc.rows
                    .iter()
                    .map(|r| r.chars().count())
                    .max()
                    .unwrap_or(0),
                doc.rows.len(),
            ),
        };
        MapGrid {
            origin: doc.origin,
            cell_size: doc.cell_size,
            width,
            height,
        }
    }

    /// A world XZ point as `generate` places it under `mirror` (unchanged for [`Mirror::None`]).
    pub fn mirror_point(&self, mirror: Mirror, point: [f64; 2]) -> [f64; 2] {
        let mut out = point;
        match mirror {
            Mirror::None => {}
            Mirror::X => {
                out[0] = mirror_line_sum(
                    mirror,
                    self.origin,
                    [self.width, self.height],
                    self.cell_size,
                ) - point[0]
            }
            Mirror::Z => {
                out[1] = mirror_line_sum(
                    mirror,
                    self.origin,
                    [self.width, self.height],
                    self.cell_size,
                ) - point[1]
            }
        }
        out
    }

    /// World XZ of the grid's `(col 0, row 0)` corner.
    pub fn origin(&self) -> [f64; 2] {
        self.origin
    }

    /// World units per cell.
    pub fn cell_size(&self) -> f64 {
        self.cell_size
    }

    /// Column count (grid width in cells).
    pub fn width(&self) -> usize {
        self.width
    }

    /// Row count (grid height in cells).
    pub fn height(&self) -> usize {
        self.height
    }

    /// Total cell count (`width * height`).
    pub fn cells(&self) -> usize {
        self.width * self.height
    }

    /// Whether `(row, col)` lies on the grid's outer perimeter.
    pub fn is_edge(&self, row: usize, col: usize) -> bool {
        self.height > 0
            && self.width > 0
            && (row == 0 || row + 1 == self.height || col == 0 || col + 1 == self.width)
    }

    /// The world XZ center of a single cell.
    pub fn cell_center(&self, row: usize, col: usize) -> [f64; 2] {
        self.rect_center(row, col, row, col)
    }

    /// The world XZ center of the inclusive cell rectangle `rows [r0, r1] × cols [c0, c1]`.
    pub fn rect_center(&self, r0: usize, c0: usize, r1: usize, c1: usize) -> [f64; 2] {
        [
            self.origin[0] + ((c0 + c1 + 1) as f64 / 2.0) * self.cell_size,
            self.origin[1] + ((r0 + r1 + 1) as f64 / 2.0) * self.cell_size,
        ]
    }

    /// The cell a world position falls in, or `None` if it lies outside the grid.
    pub fn cell_of(&self, x: f64, z: f64) -> Option<(usize, usize)> {
        if self.cell_size <= 0.0 {
            return None;
        }
        let col = (x - self.origin[0]) / self.cell_size;
        let row = (z - self.origin[1]) / self.cell_size;
        if col < 0.0 || row < 0.0 {
            return None;
        }
        let (col, row) = (col.floor() as usize, row.floor() as usize);
        (col < self.width && row < self.height).then_some((row, col))
    }

    /// Each marker resolved to its world position and grid cell, in source order — the pure data
    /// behind `map-check`'s marker table (no registry, no occupancy, no GPU).
    pub fn marker_table(&self, doc: &MapDoc) -> Vec<MarkerCell> {
        doc.markers
            .iter()
            .enumerate()
            .map(|(index, marker)| MarkerCell {
                name: marker.derived_name(index),
                kind: marker.kind.clone(),
                world: [marker.x, marker.z],
                cell: self.cell_of(marker.x, marker.z),
            })
            .collect()
    }
}

/// A parsed, structurally valid map-kit document (`kind: map-kit`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MapKit {
    /// Optional kit name.
    pub name: Option<String>,
    /// Glyph → primitive list, sorted by glyph (deterministic iteration).
    pub tiles: Vec<(String, Vec<KitPrimitive>)>,
    /// Marker kind → primitive list, sorted by kind.
    pub kinds: Vec<(String, Vec<KitPrimitive>)>,
    /// The walker the occupancy probe should measure with. Read by `map-check` only — it never
    /// reaches the generated scene.
    pub probe: MapProbe,
}

/// The kit's optional `probe:` block: the walker `map-check` measures occupancy and traversal with,
/// authored beside the content so a report is not quoted at a CLI default the game never uses.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MapProbe {
    /// The walker's collision radius in world units (the game's own player radius).
    pub radius: Option<f64>,
    /// The walker's speed in world units per second (the game's own player speed).
    pub speed: Option<f64>,
}

impl MapKit {
    fn tile(&self, glyph: &str) -> Option<&[KitPrimitive]> {
        self.tiles
            .iter()
            .find(|(g, _)| g == glyph)
            .map(|(_, p)| p.as_slice())
    }

    fn kind(&self, name: &str) -> Option<&[KitPrimitive]> {
        self.kinds
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, p)| p.as_slice())
    }
}

/// One emitted primitive of a kit entry: a mesh source plus its placement attributes.
#[derive(Clone, Debug, PartialEq)]
pub struct KitPrimitive {
    /// Optional entity-name prefix (else derived from the glyph/kind, with a per-primitive index).
    pub name: Option<String>,
    /// The primitive's mesh source (variant candidate index 0). `None` is a collider-only
    /// primitive: one entity carrying its `collider` and no `Mesh`.
    pub mesh: Option<MeshSpec>,
    /// Additional mesh candidates chosen deterministically by `(seed, cell coords)`.
    pub variants: Vec<MeshSpec>,
    /// Optional flat RGBA color (omitted → the `Mesh` default, opaque white).
    pub color: Option<[f64; 4]>,
    /// Optional static collider.
    pub collider: Option<ColliderSpec>,
    /// The `collider`'s surface tag (`collider: { box: [..], surface: name }`), emitted as a
    /// `SurfaceTag` beside the `Collider`.
    pub surface: Option<String>,
    /// Y offset of the emitted `Transform` (default `0`). Where the game bakes its floor rule.
    pub y: f64,
    /// Per-cell base scale (default `[1, 1, 1]`); `merge` multiplies the X/Z axes by the rect size.
    pub scale: [f64; 3],
    /// Optional `Mesh.roughness` (`[0, 1]`; absent → not emitted, the `Mesh` default).
    pub roughness: Option<f64>,
    /// Optional `Mesh.metallic` (`[0, 1]`; absent → not emitted).
    pub metallic: Option<f64>,
    /// Optional `Mesh.emissive` (`>= 0`; absent → not emitted).
    pub emissive: Option<f64>,
    /// `Some(true)` → same-glyph cells rectangle-merge into one scaled entity per rect (floors).
    /// Absent merges only for the map's `base` glyph; `Some(false)` on a base primitive emits it
    /// per cell instead.
    pub merge: Option<bool>,
    /// `Some([w, h])` → runs of the glyph tile with one unscaled prop per `w × h`-cell footprint.
    pub footprint: Option<[u32; 2]>,
    /// `true` → the emitted mesh entities carry `Static: {}` (never the `box` collider sibling).
    pub is_static: bool,
}

impl Default for KitPrimitive {
    fn default() -> Self {
        KitPrimitive {
            name: None,
            mesh: Some(MeshSpec::Shape("cube".to_string())),
            variants: Vec::new(),
            color: None,
            collider: None,
            surface: None,
            y: 0.0,
            scale: [1.0, 1.0, 1.0],
            roughness: None,
            metallic: None,
            emissive: None,
            merge: None,
            footprint: None,
            is_static: false,
        }
    }
}

impl KitPrimitive {
    /// Whether same-glyph cells rectangle-merge: an explicit `merge` wins, else only the `base`.
    fn merges(&self, is_base: bool) -> bool {
        self.merge.unwrap_or(is_base)
    }
}

/// A mesh source, mirroring the engine `Mesh` component's `shape` field.
#[derive(Clone, Debug, PartialEq)]
pub enum MeshSpec {
    /// A built-in primitive (`cube`/`plane`/`sphere`) — a bare `shape:` string.
    Shape(String),
    /// A voxel model document path (`voxels/*.voxel.yaml`).
    Voxel(String),
    /// A mesh asset document path (`meshes/*.mesh.yaml`).
    Mesh(String),
    /// A sculpt document path (`sculpts/*.sculpt.yaml`).
    Sculpt(String),
}

impl MeshSpec {
    /// The `Mesh.shape` value: a bare string for a primitive, else a one-key
    /// `{voxel|asset|sculpt: path}`.
    fn shape_value(&self) -> Value {
        match self {
            MeshSpec::Shape(s) => Value::String(s.clone()),
            MeshSpec::Sculpt(p) => json!({ "sculpt": p }),
            MeshSpec::Voxel(p) => json!({ "voxel": p }),
            MeshSpec::Mesh(p) => json!({ "asset": p }),
        }
    }
}

/// A static collider shape, mirroring the engine `Collider` component's `shape` field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColliderSpec {
    /// An axis-aligned box, `half_extents` `[x, z]`.
    Aabb([f64; 2]),
    /// A circle of the given radius.
    Circle(f64),
    /// A 3D box, `half_extents` `[x, y, z]`: the solver and nav see it, unlike the 2D shapes.
    Box([f64; 3]),
}

// --- Parsing: map source (`kind: map`) ---------------------------------------------------------

/// Parses strict-subset YAML into a [`MapDoc`], checking structure/shape only (`L1021`).
///
/// Grid semantics (ragged rows, unknown glyphs, marker bounds, name uniqueness) are validated by
/// [`generate`], not here — this is the structural analog of [`crate::parse_mesh`].
///
/// # Errors
/// `L1021` for a malformed envelope, a wrong-typed field, or an unknown key; each diagnostic carries
/// a JSON Pointer into the document where applicable.
pub fn parse_map(source: &str) -> Result<MapDoc, Vec<Diagnostic>> {
    let entries = root_mapping(source, "kind: map")?;
    let mut diagnostics = Vec::new();
    let mut doc = MapDoc {
        cell_size: 1.0,
        ..MapDoc::default()
    };
    let mut seen = SeenKeys::default();

    for entry in &entries {
        match entry.key.as_str() {
            "format" => {
                seen.format = true;
                envelope::check_envelope(
                    entry,
                    code::MAP_DOCUMENT_STRUCTURE,
                    code::MAP_DOCUMENT_STRUCTURE,
                    MAP_FORMAT_VERSION,
                    "the map document",
                    "map",
                    "map",
                    &mut diagnostics,
                );
            }
            "kind" => {
                seen.kind = true;
                envelope::check_kind(
                    entry,
                    code::MAP_DOCUMENT_STRUCTURE,
                    "map",
                    "a map source document",
                    &mut diagnostics,
                );
            }
            "name" => {
                doc.name = envelope::expect_string(
                    &entry.node,
                    "/name",
                    "the map name",
                    code::MAP_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "seed" => {
                doc.seed = expect_u64(&entry.node, "/seed", "the seed", &mut diagnostics);
            }
            "kit" => {
                seen.kit = true;
                doc.kit = envelope::expect_string(
                    &entry.node,
                    "/kit",
                    "the kit path",
                    code::MAP_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                )
                .unwrap_or_default();
            }
            "cell_size" => {
                seen.cell_size = true;
                if let Some(v) =
                    expect_f64(&entry.node, "/cell_size", "cell_size", &mut diagnostics)
                {
                    if v > 0.0 && v.is_finite() {
                        doc.cell_size = v;
                    } else {
                        diagnostics.push(
                            structure("`cell_size` must be a positive, finite number")
                                .with_path("/cell_size"),
                        );
                    }
                }
            }
            "origin" => {
                seen.origin = true;
                if let Some(v) =
                    expect_number_array::<2>(&entry.node, "/origin", "origin", &mut diagnostics)
                {
                    if v.iter().all(|c| c.is_finite()) {
                        doc.origin = v;
                    } else {
                        diagnostics.push(
                            structure("`origin` must be two finite numbers").with_path("/origin"),
                        );
                    }
                }
            }
            "base" => {
                doc.base = envelope::expect_string(
                    &entry.node,
                    "/base",
                    "the base glyph",
                    code::MAP_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "rows" => {
                seen.rows = true;
                doc.rows = expect_string_seq(&entry.node, "/rows", "rows", &mut diagnostics);
            }
            "bounds" => {
                seen.bounds = true;
                doc.bounds = expect_pair(&entry.node, "/bounds", "bounds", &mut diagnostics);
                if doc.bounds.is_some_and(|b| b.contains(&0)) {
                    diagnostics.push(
                        structure("`bounds` must be two positive integers (width, height)")
                            .with_path("/bounds"),
                    );
                    doc.bounds = None;
                } else if doc
                    .bounds
                    .is_some_and(|b| b.iter().any(|&e| e > MAX_MAP_EXTENT))
                {
                    diagnostics.push(
                        structure(&format!(
                            "`bounds` is above the largest {MAX_MAP_EXTENT} cells per axis"
                        ))
                        .with_path("/bounds")
                        .with_help(format!(
                            "a marker-only map frames at most {MAX_MAP_EXTENT} x {MAX_MAP_EXTENT} \
                             cells; split the district into maps"
                        )),
                    );
                    doc.bounds = None;
                }
            }
            "physics" => {
                doc.physics = crate::physics::parse_physics(&entry.node, &mut diagnostics);
            }
            "markers" => {
                doc.markers = parse_markers(&entry.node, &mut diagnostics);
            }
            "fragments" => {
                doc.fragments =
                    expect_string_seq(&entry.node, "/fragments", "fragments", &mut diagnostics);
            }
            unknown => diagnostics.push(unknown_map_key(unknown, entry)),
        }
    }

    seen.require_map(&mut diagnostics);
    if seen.bounds && seen.rows {
        diagnostics.push(
            structure("a map declares either `rows` or `bounds`, not both")
                .with_path("/bounds")
                .with_help("keep `rows` for a glyph grid, or `bounds` for a marker-only map"),
        );
    }
    if seen.bounds && doc.base.is_some() {
        diagnostics.push(
            structure("`base` fills a glyph grid, which a `bounds` map does not have")
                .with_path("/base")
                .with_help("drop `base`, or declare `rows` instead of `bounds`"),
        );
    }

    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

#[derive(Default)]
struct SeenKeys {
    format: bool,
    kind: bool,
    kit: bool,
    cell_size: bool,
    origin: bool,
    rows: bool,
    bounds: bool,
    tiles: bool,
}

impl SeenKeys {
    fn require_map(&self, diagnostics: &mut Vec<Diagnostic>) {
        if !self.format {
            diagnostics.push(missing("format", "add `format: 1` as the first line"));
        }
        if !self.kind {
            diagnostics.push(missing("kind", "add `kind: map`"));
        }
        if !self.kit {
            diagnostics.push(missing(
                "kit",
                "add `kit: <path to the shared map-kit file>`",
            ));
        }
        if !self.cell_size {
            diagnostics.push(missing(
                "cell_size",
                "add `cell_size:` (world units per cell)",
            ));
        }
        if !self.origin {
            diagnostics.push(missing(
                "origin",
                "add `origin:` — the world XZ of the grid's (col 0, row 0) corner",
            ));
        }
        if !self.rows && !self.bounds {
            diagnostics.push(missing(
                "rows",
                "add `rows:` (one equal-length glyph string per row), or `bounds:` (the \
                    `[width, height]` in cells of a marker-only map)",
            ));
        }
    }
}

fn parse_markers(node: &Node, diagnostics: &mut Vec<Diagnostic>) -> Vec<MapMarker> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            structure(&format!(
                "`markers` must be a sequence, found {} (line {})",
                node.value.type_phrase(),
                node.line
            ))
            .with_path("/markers"),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        if let Some(marker) = parse_marker(item, index, diagnostics) {
            out.push(marker);
        }
    }
    out
}

fn parse_marker(node: &Node, index: usize, diagnostics: &mut Vec<Diagnostic>) -> Option<MapMarker> {
    let base = format!("/markers/{index}");
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            structure(&format!(
                "each marker must be a mapping, found {} (line {})",
                node.value.type_phrase(),
                node.line
            ))
            .with_path(base),
        );
        return None;
    };
    let mut marker = MapMarker::default();
    let before = diagnostics.len();
    let mut x_seen = false;
    let mut z_seen = false;
    for entry in entries {
        match entry.key.as_str() {
            "name" => {
                marker.name = envelope::expect_string(
                    &entry.node,
                    &format!("{base}/name"),
                    "the marker name",
                    code::MAP_DOCUMENT_STRUCTURE,
                    diagnostics,
                );
            }
            "kind" => {
                marker.kind = envelope::expect_string(
                    &entry.node,
                    &format!("{base}/kind"),
                    "the marker kind",
                    code::MAP_DOCUMENT_STRUCTURE,
                    diagnostics,
                );
            }
            "x" => {
                x_seen = true;
                marker.x = expect_f64(&entry.node, &format!("{base}/x"), "x", diagnostics)
                    .unwrap_or_default();
            }
            "z" => {
                z_seen = true;
                marker.z = expect_f64(&entry.node, &format!("{base}/z"), "z", diagnostics)
                    .unwrap_or_default();
            }
            "rotation" => {
                let pointer = format!("{base}/rotation");
                marker.rotation = expect_f64(&entry.node, &pointer, "rotation", diagnostics)
                    .filter(|r| {
                        let finite = r.is_finite();
                        if !finite {
                            diagnostics.push(
                                structure("`rotation` must be a finite number").with_path(&pointer),
                            );
                        }
                        finite
                    });
            }
            "variant" => {
                marker.variant = expect_u64(
                    &entry.node,
                    &format!("{base}/variant"),
                    "variant",
                    diagnostics,
                )
                .map(|v| v as usize);
            }
            "components" => {
                marker.components = parse_components(&entry.node, &base, diagnostics);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                &base,
                "a marker",
                MARKER_KEYS,
                code::MAP_DOCUMENT_STRUCTURE,
            )),
        }
    }
    if !x_seen {
        diagnostics.push(structure("a marker is missing its `x` position").with_path(&base));
    }
    if !z_seen {
        diagnostics.push(structure("a marker is missing its `z` position").with_path(&base));
    }
    (diagnostics.len() == before).then_some(marker)
}

/// Parses a marker's verbatim `components:` mapping into `(name, value)` pairs, exactly as an
/// entity's `components` are read (component shape is the game registry's concern, not the map's).
fn parse_components(
    node: &Node,
    base: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<(String, Value)> {
    let NodeValue::Mapping(components) = &node.value else {
        diagnostics.push(
            structure(&format!(
                "`components` must be a mapping of component name to value, found {} (line {})",
                node.value.type_phrase(),
                node.line
            ))
            .with_path(format!("{base}/components")),
        );
        return Vec::new();
    };
    components
        .iter()
        .map(|c| (c.key.clone(), node_to_value(&c.node)))
        .collect()
}

// --- Parsing: map-kit (`kind: map-kit`) --------------------------------------------------------

/// Parses strict-subset YAML into a [`MapKit`], checking structure/shape only (`L1021`).
///
/// Kit semantics (empty primitive lists, `merge`/`footprint` clashes, bad footprints) are validated
/// by [`generate`], not here.
///
/// # Errors
/// `L1021` for a malformed envelope, a wrong-typed field, or an unknown key; each diagnostic carries
/// a JSON Pointer into the document where applicable.
pub fn parse_map_kit(source: &str) -> Result<MapKit, Vec<Diagnostic>> {
    let entries = root_mapping(source, "kind: map-kit")?;
    let mut diagnostics = Vec::new();
    let mut kit = MapKit::default();
    let mut seen = SeenKeys::default();

    for entry in &entries {
        match entry.key.as_str() {
            "format" => {
                seen.format = true;
                envelope::check_envelope(
                    entry,
                    code::MAP_DOCUMENT_STRUCTURE,
                    code::MAP_DOCUMENT_STRUCTURE,
                    MAP_FORMAT_VERSION,
                    "the map-kit document",
                    "map-kit",
                    "map-kit",
                    &mut diagnostics,
                );
            }
            "kind" => {
                seen.kind = true;
                envelope::check_kind(
                    entry,
                    code::MAP_DOCUMENT_STRUCTURE,
                    "map-kit",
                    "a map-kit document",
                    &mut diagnostics,
                );
            }
            "name" => {
                kit.name = envelope::expect_string(
                    &entry.node,
                    "/name",
                    "the kit name",
                    code::MAP_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "tiles" => {
                seen.tiles = true;
                kit.tiles = parse_kit_group(&entry.node, "tiles", &mut diagnostics);
            }
            "kinds" => {
                kit.kinds = parse_kit_group(&entry.node, "kinds", &mut diagnostics);
            }
            "probe" => {
                kit.probe = parse_kit_probe(&entry.node, &mut diagnostics);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "",
                "a map-kit document",
                KIT_KEYS,
                code::MAP_DOCUMENT_STRUCTURE,
            )),
        }
    }

    if !seen.format {
        diagnostics.push(missing("format", "add `format: 1` as the first line"));
    }
    if !seen.kind {
        diagnostics.push(missing("kind", "add `kind: map-kit`"));
    }
    if !seen.tiles {
        diagnostics.push(missing(
            "tiles",
            "add `tiles:` — a mapping of each grid glyph to a list of primitives",
        ));
    }

    if diagnostics.is_empty() {
        Ok(kit)
    } else {
        Err(diagnostics)
    }
}

/// Parses the kit's optional `probe:` block. A wrong-typed field or unknown key is `L1021`; a
/// non-finite or non-positive value is `L1022` (a probe of radius 0 measures nothing).
fn parse_kit_probe(node: &Node, diagnostics: &mut Vec<Diagnostic>) -> MapProbe {
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            structure(&format!(
                "`probe` must be a mapping of `radius`/`speed`, found {} (line {})",
                node.value.type_phrase(),
                node.line
            ))
            .with_path("/probe"),
        );
        return MapProbe::default();
    };

    let mut probe = MapProbe::default();
    for entry in entries {
        let field = match entry.key.as_str() {
            "radius" => &mut probe.radius,
            "speed" => &mut probe.speed,
            unknown => {
                diagnostics.push(envelope::unknown_key(
                    unknown,
                    entry,
                    "/probe",
                    "a map-kit `probe` block",
                    PROBE_KEYS,
                    code::MAP_DOCUMENT_STRUCTURE,
                ));
                continue;
            }
        };
        let pointer = format!("/probe/{}", entry.key);
        let what = format!("probe {}", entry.key);
        let Some(value) = expect_f64(&entry.node, &pointer, &what, diagnostics) else {
            continue;
        };
        if !(value.is_finite() && value > 0.0) {
            diagnostics.push(
                Diagnostic::new(
                    code::MAP_KIT_INVALID,
                    format!(
                        "`probe.{}` must be a positive number, found {value}",
                        entry.key
                    ),
                )
                .with_path(pointer)
                .with_help(match entry.key.as_str() {
                    "radius" => "set it to the walker's collision radius in world units",
                    _ => "set it to the walker's speed in world units per second",
                }),
            );
            continue;
        }
        *field = Some(value);
    }
    probe
}

/// Parses a kit `tiles:` / `kinds:` mapping of key → primitive list, returned sorted by key.
fn parse_kit_group(
    node: &Node,
    group: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<(String, Vec<KitPrimitive>)> {
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            structure(&format!(
                "`{group}` must be a mapping of key to a primitive list, found {} (line {})",
                node.value.type_phrase(),
                node.line
            ))
            .with_path(format!("/{group}")),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let pointer = format!("/{group}/{}", pointer_token(&entry.key));
        let primitives = parse_primitive_list(&entry.node, &pointer, diagnostics);
        out.push((entry.key.clone(), primitives));
    }
    out.sort_by(|(a, _), (b, _)| a.cmp(b));
    out
}

fn parse_primitive_list(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<KitPrimitive> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            structure(&format!(
                "a kit entry must be a sequence of primitives, found {} (line {})",
                node.value.type_phrase(),
                node.line
            ))
            .with_path(pointer.to_string()),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        if let Some(prim) = parse_primitive(item, &format!("{pointer}/{index}"), diagnostics) {
            out.push(prim);
        }
    }
    out
}

fn parse_primitive(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<KitPrimitive> {
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            structure(&format!(
                "a primitive must be a mapping, found {} (line {})",
                node.value.type_phrase(),
                node.line
            ))
            .with_path(pointer.to_string()),
        );
        return None;
    };
    let mut prim = KitPrimitive::default();
    let before = diagnostics.len();
    // A primitive names one mesh source (shape | voxel | mesh | sculpt), or none when collider-only.
    let mut mesh: Option<MeshSpec> = None;
    let mut sources = 0usize;
    let mut has_collider = false;
    // Mesh-only keys seen, refused on a collider-only primitive.
    let mut inert: Vec<&'static str> = Vec::new();
    for entry in entries {
        let child = |seg: &str| format!("{pointer}/{seg}");
        match entry.key.as_str() {
            "name" => {
                prim.name = envelope::expect_string(
                    &entry.node,
                    &child("name"),
                    "the primitive name",
                    code::MAP_DOCUMENT_STRUCTURE,
                    diagnostics,
                );
            }
            "shape" => {
                sources += 1;
                mesh = envelope::expect_string(
                    &entry.node,
                    &child("shape"),
                    "the shape",
                    code::MAP_DOCUMENT_STRUCTURE,
                    diagnostics,
                )
                .map(MeshSpec::Shape);
            }
            "voxel" => {
                sources += 1;
                mesh = envelope::expect_string(
                    &entry.node,
                    &child("voxel"),
                    "the voxel path",
                    code::MAP_DOCUMENT_STRUCTURE,
                    diagnostics,
                )
                .map(MeshSpec::Voxel);
            }
            "mesh" => {
                sources += 1;
                mesh = envelope::expect_string(
                    &entry.node,
                    &child("mesh"),
                    "the mesh path",
                    code::MAP_DOCUMENT_STRUCTURE,
                    diagnostics,
                )
                .map(MeshSpec::Mesh);
            }
            "sculpt" => {
                sources += 1;
                mesh = envelope::expect_string(
                    &entry.node,
                    &child("sculpt"),
                    "the sculpt path",
                    code::MAP_DOCUMENT_STRUCTURE,
                    diagnostics,
                )
                .map(MeshSpec::Sculpt);
            }
            "color" => {
                inert.push("color");
                prim.color =
                    expect_number_array::<4>(&entry.node, &child("color"), "color", diagnostics);
            }
            "roughness" => {
                inert.push("roughness");
                prim.roughness =
                    expect_f64(&entry.node, &child("roughness"), "roughness", diagnostics);
            }
            "metallic" => {
                inert.push("metallic");
                prim.metallic =
                    expect_f64(&entry.node, &child("metallic"), "metallic", diagnostics);
            }
            "emissive" => {
                inert.push("emissive");
                prim.emissive =
                    expect_f64(&entry.node, &child("emissive"), "emissive", diagnostics);
            }
            "collider" => {
                has_collider = true;
                if let Some((spec, surface)) =
                    parse_collider(&entry.node, &child("collider"), diagnostics)
                {
                    prim.collider = Some(spec);
                    prim.surface = surface;
                }
            }
            "y" => {
                prim.y = expect_f64(&entry.node, &child("y"), "y", diagnostics).unwrap_or_default();
            }
            "scale" => {
                inert.push("scale");
                if let Some(s) =
                    expect_number_array::<3>(&entry.node, &child("scale"), "scale", diagnostics)
                {
                    prim.scale = s;
                }
            }
            "variants" => {
                inert.push("variants");
                prim.variants = parse_variants(&entry.node, &child("variants"), diagnostics);
            }
            "merge" => {
                prim.merge = expect_bool(&entry.node, &child("merge"), "merge", diagnostics);
            }
            "footprint" => {
                prim.footprint =
                    expect_pair(&entry.node, &child("footprint"), "footprint", diagnostics);
            }
            "static" => {
                inert.push("static");
                prim.is_static = expect_bool(&entry.node, &child("static"), "static", diagnostics)
                    .unwrap_or(false);
            }
            unknown => {
                let mut valid: Vec<&'static str> = PRIMITIVE_KEYS.to_vec();
                valid.extend_from_slice(&PRIMITIVE_KEYS_EXTRA);
                diagnostics.push(unknown_key_dyn(
                    unknown,
                    entry,
                    pointer,
                    "a primitive",
                    &valid,
                ));
            }
        }
    }
    prim.mesh = mesh;
    if sources == 0 {
        if !has_collider {
            diagnostics.push(
                structure(
                    "a primitive needs a mesh source (`shape`, `voxel`, `mesh` or `sculpt`) or a `collider`",
                )
                .with_help(
                    "add `shape: cube` (or another source) for a visible piece, or a `collider` alone for an invisible blocker",
                )
                .with_path(pointer.to_string()),
            );
        } else {
            for key in inert {
                diagnostics.push(
                    Diagnostic::new(
                        code::MAP_KIT_INVALID,
                        format!(
                            "`{key}` has no effect on a collider-only primitive: it has no mesh"
                        ),
                    )
                    .with_help(format!(
                        "remove `{key}`, or add a mesh source (`shape`, `voxel`, `mesh` or `sculpt`)"
                    ))
                    .with_path(format!("{pointer}/{key}")),
                );
            }
        }
    }
    (diagnostics.len() == before).then_some(prim)
}

fn parse_collider(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<(ColliderSpec, Option<String>)> {
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            structure(&format!(
                "`collider` must be a mapping (`aabb: [x, z]`, `circle: r` or `box: [x, y, z]`), found {} (line {})",
                node.value.type_phrase(),
                node.line
            ))
            .with_path(pointer.to_string()),
        );
        return None;
    };
    let mut spec = None;
    let mut surface = None;
    for entry in entries {
        match entry.key.as_str() {
            "surface" => match &entry.node.value {
                NodeValue::Scalar(Value::String(name)) if !name.is_empty() => {
                    surface = Some(name.clone());
                }
                _ => diagnostics.push(
                    Diagnostic::new(
                        code::MAP_KIT_INVALID,
                        format!(
                            "a collider `surface` must be a non-empty surface name, found {} (line {})",
                            entry.node.value.type_phrase(),
                            entry.node.line
                        ),
                    )
                    .with_help("name a surface declared under the scene's `physics.surfaces`")
                    .with_path(format!("{pointer}/surface")),
                ),
            },
            "aabb" => {
                spec = expect_number_array::<2>(
                    &entry.node,
                    &format!("{pointer}/aabb"),
                    "aabb half-extents",
                    diagnostics,
                )
                .map(ColliderSpec::Aabb);
            }
            "circle" => {
                spec = expect_f64(
                    &entry.node,
                    &format!("{pointer}/circle"),
                    "circle radius",
                    diagnostics,
                )
                .map(ColliderSpec::Circle);
            }
            "box" => {
                spec = expect_number_array::<3>(
                    &entry.node,
                    &format!("{pointer}/box"),
                    "box half-extents",
                    diagnostics,
                )
                .map(ColliderSpec::Box);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                pointer,
                "a collider",
                COLLIDER_KEYS,
                code::MAP_DOCUMENT_STRUCTURE,
            )),
        }
    }
    if surface.is_some() && matches!(spec, Some(ColliderSpec::Aabb(_) | ColliderSpec::Circle(_))) {
        diagnostics.push(
            Diagnostic::new(
                code::MAP_KIT_INVALID,
                "a collider `surface` needs a 3D `box` collider: `aabb` and `circle` live in the 2D spatial index, which reports no surfaces",
            )
            .with_help("use `box: [x, y, z]`, or drop `surface`")
            .with_path(format!("{pointer}/surface")),
        );
    }
    spec.map(|spec| (spec, surface))
}

fn parse_variants(node: &Node, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Vec<MeshSpec> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            structure(&format!(
                "`variants` must be a sequence of mesh sources, found {} (line {})",
                node.value.type_phrase(),
                node.line
            ))
            .with_path(pointer.to_string()),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let child = format!("{pointer}/{index}");
        let NodeValue::Mapping(entries) = &item.value else {
            diagnostics.push(
                structure(
                    "a variant must be a one-key mesh source (`shape`/`voxel`/`mesh`/`sculpt`)",
                )
                .with_path(child),
            );
            continue;
        };
        let mut mesh = None;
        for entry in entries {
            let value = envelope::expect_string(
                &entry.node,
                &format!("{child}/{}", pointer_token(&entry.key)),
                "a mesh source",
                code::MAP_DOCUMENT_STRUCTURE,
                diagnostics,
            );
            mesh = match (entry.key.as_str(), value) {
                ("shape", Some(s)) => Some(MeshSpec::Shape(s)),
                ("voxel", Some(s)) => Some(MeshSpec::Voxel(s)),
                ("mesh", Some(s)) => Some(MeshSpec::Mesh(s)),
                ("sculpt", Some(s)) => Some(MeshSpec::Sculpt(s)),
                (other, _) => {
                    diagnostics.push(
                        structure(&format!(
                            "a variant key must be `shape`, `voxel`, `mesh`, or `sculpt`, found `{other}`"
                        ))
                        .with_path(child.clone()),
                    );
                    None
                }
            };
        }
        if let Some(mesh) = mesh {
            out.push(mesh);
        }
    }
    out
}

// --- Generation --------------------------------------------------------------------------------

/// Compiles a map source into a canonical [`Scene`] of real entities — the map-authoring pipeline's
/// deterministic core.
///
/// Emission order is fixed (fragments verbatim → grid rects/props row-major → markers in source
/// order), names are coordinate-derived and validated for global uniqueness, and variant/rotation
/// selection is seeded from `(seed, cell coords)` — so the same inputs always produce byte-identical
/// [`crate::emit`] output and a one-glyph edit touches only the affected entities.
///
/// This is a **pure function**: `fragments` are already-parsed scene files (the `String` is each
/// fragment's path, for diagnostics), and no file IO happens here. `map.source`, if set by the
/// caller, becomes [`Scene::generated_from`].
///
/// # Errors
/// All problems found, collected: ragged rows (`L1019`) / unknown glyphs (`L1020`) in the grid;
/// invalid kit entries or marker `variant`s (`L1022`); a marker out of bounds (`L1023`); an unknown
/// marker kind (`L1024`); a generated-name collision across fragments/grid/markers (`L1025`); a
/// footprint run that does not divide evenly (`L1026`). Each carries a JSON Pointer into the map
/// source where applicable.
///
/// A fragment's entity references (`Parent`, `Joint`, …: the `registry`'s entity-reference fields)
/// are local to it; composition rebases them past the entities emitted before it, and one that names
/// no entity of its fragment is `L1017`.
pub fn generate(
    map: &MapDoc,
    kit: &MapKit,
    fragments: &[(String, Scene)],
    registry: &Registry,
) -> Result<Scene, Vec<Diagnostic>> {
    generate_traced(map, kit, fragments, registry).map(|(scene, _)| scene)
}

/// [`generate`], plus each generated entity's source pointer into the map document (`/fragments/N`,
/// `/rows/R`, `/markers/I`), parallel to the scene's entities, so a diagnostic about the generated
/// scene can be reported where its entity was written.
pub fn generate_traced(
    map: &MapDoc,
    kit: &MapKit,
    fragments: &[(String, Scene)],
    registry: &Registry,
) -> Result<(Scene, Vec<String>), Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();

    let extent = check_grid(map, kit, &mut diagnostics);
    validate_kit(kit, map, &mut diagnostics);

    if map.mirror != Mirror::None && !map.fragments.is_empty() {
        diagnostics.push(
            Diagnostic::new(
                code::MAP_MIRROR_FRAGMENTS,
                "a mirrored map cannot carry `fragments:`; their transforms are not reflected",
            )
            .with_path("/fragments")
            .with_help(
                "move the fragment's entities onto markers (marker `components:` carry lights and \
                 game components), or drop the mirror",
            ),
        );
    }

    let mut entities: Vec<SceneEntity> = Vec::new();

    // 1. Fragments, verbatim, first, in order. Only their entities survive the merge, so a scene
    // block a fragment carries would be dropped silently.
    let mut origins: Vec<Origin> = Vec::new();
    for (fi, (path, scene)) in fragments.iter().enumerate() {
        let pointer = format!("/fragments/{fi}");
        for (key, present) in [
            ("physics", scene.physics.is_some()),
            ("seed", scene.seed.is_some()),
            ("preload", !scene.preload.is_empty()),
        ] {
            if present {
                let help = if key == "preload" {
                    "maps cannot declare `preload`; put it on a hand-authored scene".to_string()
                } else {
                    format!("declare `{key}` on the map instead")
                };
                diagnostics.push(
                    Diagnostic::new(
                        code::MAP_DOCUMENT_STRUCTURE,
                        format!(
                            "the fragment `{path}` declares `{key}`, which the merge would drop"
                        ),
                    )
                    .with_path(pointer.clone())
                    .with_help(help),
                );
            }
        }
        let offset = entities.len();
        for (j, entity) in scene.entities.iter().enumerate() {
            let mut entity = entity.clone();
            let overrides = entity
                .instance
                .iter_mut()
                .flat_map(|instance| instance.overrides.values_mut().flatten());
            for (component, value) in entity.components.iter_mut().chain(overrides) {
                let dangling = crate::expand::ref_targets(registry, component, value)
                    .into_iter()
                    .find(|&target| target >= scene.entities.len());
                if let Some(target) = dangling {
                    diagnostics.push(
                        Diagnostic::new(
                            code::SCENE_INVALID_ENTITY_REF,
                            format!(
                                "entity {j} of the fragment `{path}` has a `{component}` reference to \
                                 entity {target}, but the fragment has {} entities",
                                scene.entities.len()
                            ),
                        )
                        .with_path(pointer.clone())
                        .with_help(
                            "entity references inside a fragment are local to it: point them at \
                             another entity of the same fragment",
                        ),
                    );
                    continue;
                }
                crate::expand::remap_positions(registry, component, value, &|p| Some(p + offset));
            }
            entities.push(entity);
            origins.push(Origin {
                pointer: pointer.clone(),
                label: format!("entity {j} of `{path}`"),
            });
        }
    }

    let fragment_count = entities.len();

    // Grid + marker passes only run when the grid is rectangular (a ragged grid has no
    // well-defined column count to place cells against).
    if let Some((width, height)) = extent {
        let seed = map.seed.unwrap_or(0);
        let grid = Grid::new(map, width, height);

        // 2. Grid rects/props, sorted row-major by anchor cell. A `bounds` map has no glyphs, and
        // `glyph_at` past the rows would read as non-void to the base underlay.
        if map.frame().is_none() {
            let mut items: Vec<GridItem> = Vec::new();
            emit_grid(&grid, kit, seed, &mut items, &mut diagnostics);
            items.sort_by_key(|item| (item.r0, item.c0, item.seq));
            for item in items {
                for entity in item.entities {
                    entities.push(entity);
                    origins.push(item.origin.clone());
                }
            }
        }

        // 3. Markers, source order.
        for (index, marker) in map.markers.iter().enumerate() {
            emit_marker(
                &grid,
                kit,
                seed,
                marker,
                index,
                &mut entities,
                &mut diagnostics,
            );
            origins.resize(
                entities.len(),
                Origin {
                    pointer: format!("/markers/{index}"),
                    label: format!("marker {index}"),
                },
            );
        }

        if map.mirror != Mirror::None {
            let line_sum = mirror_line_sum(map.mirror, map.origin, [width, height], map.cell_size);
            for entity in &mut entities[fragment_count..] {
                mirror_entity(entity, map.mirror, line_sum);
            }
        }
    }

    check_name_collisions(&entities, &origins, &mut diagnostics);

    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let pointers = origins.into_iter().map(|origin| origin.pointer).collect();
    Ok((
        Scene {
            name: map.name.clone(),
            generated_from: map.source.clone(),
            seed: map.seed,
            physics: map.physics.clone(),
            preload: Vec::new(),
            entities,
        },
        pointers,
    ))
}

/// The grid geometry shared by the grid and marker passes: the pure [`MapGrid`] coordinate math plus
/// the row glyphs and base underlay the generator reads.
struct Grid {
    geom: MapGrid,
    /// The rows as glyph vectors: a per-cell lookup is an index, not a scan of the row.
    rows: Vec<Vec<char>>,
    /// The `base:` underlay glyph, if set and a single character.
    base: Option<char>,
}

impl Grid {
    fn new(map: &MapDoc, width: usize, height: usize) -> Self {
        Grid {
            geom: MapGrid {
                origin: map.origin,
                cell_size: map.cell_size,
                width,
                height,
            },
            rows: map.rows.iter().map(|r| r.chars().collect()).collect(),
            base: map.base.as_deref().and_then(single_char),
        }
    }

    fn width(&self) -> usize {
        self.geom.width
    }

    fn height(&self) -> usize {
        self.geom.height
    }

    fn cell_size(&self) -> f64 {
        self.geom.cell_size
    }

    fn glyph_at(&self, row: usize, col: usize) -> Option<char> {
        self.rows.get(row)?.get(col).copied()
    }

    /// The world XZ center of the cell rectangle `cols [c0, c1]` × `rows [r0, r1]` (inclusive).
    fn rect_center(&self, r0: usize, c0: usize, r1: usize, c1: usize) -> [f64; 2] {
        self.geom.rect_center(r0, c0, r1, c1)
    }

    /// The cell a world position falls in, or `None` if it lies outside the grid.
    fn cell_of(&self, x: f64, z: f64) -> Option<(usize, usize)> {
        self.geom.cell_of(x, z)
    }
}

struct GridItem {
    r0: usize,
    c0: usize,
    seq: usize,
    entities: Vec<SceneEntity>,
    origin: Origin,
}

/// Where a generated entity came from in the map source, for [`code::MAP_NAME_COLLISION`].
#[derive(Clone)]
struct Origin {
    pointer: String,
    label: String,
}

/// The kit primitive a grid pass is expanding, to label what it emits.
struct Source<'a> {
    glyph: &'a str,
    prim_index: usize,
}

impl Source<'_> {
    fn origin(&self, row: usize, col: usize) -> Origin {
        Origin {
            pointer: format!("/rows/{row}"),
            label: format!(
                "glyph `{}` primitive {} at row {row}, col {col}",
                self.glyph, self.prim_index
            ),
        }
    }
}

/// The full grid expansion: base underlay (rect-merged under every non-void cell) then each
/// non-base glyph's primitive stack, per each primitive's mode. Every emitted item is tagged with
/// its anchor cell for the row-major sort in [`generate`].
fn emit_grid(
    grid: &Grid,
    kit: &MapKit,
    seed: u64,
    items: &mut Vec<GridItem>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    // Base underlay: the base glyph's primitives, rect-merged over all non-void cells.
    if let Some(base) = grid.base
        && let Some(primitives) = kit.tile(&base.to_string())
    {
        let mask = |r: usize, c: usize| grid.glyph_at(r, c) != Some(VOID_GLYPH);
        let glyph = base.to_string();
        for (prim_index, prim) in primitives.iter().enumerate() {
            let prefix = prim_prefix(prim, "floor", prim_index);
            let src = Source {
                glyph: &glyph,
                prim_index,
            };
            if prim.merges(true) {
                emit_merged(grid, prim, &prefix, seed, &mask, &src, items);
            } else {
                emit_per_cell(grid, prim, &prefix, seed, &mask, &src, items);
            }
        }
    }

    // Each non-base, non-void glyph, in the kit's sorted-glyph order.
    for (glyph_str, primitives) in &kit.tiles {
        let Some(glyph) = single_char(glyph_str) else {
            continue; // a malformed kit key is validated in `validate_kit`.
        };
        if grid.base == Some(glyph) {
            continue; // the base glyph contributes only the underlay.
        }
        let mask = |r: usize, c: usize| grid.glyph_at(r, c) == Some(glyph);
        for (prim_index, prim) in primitives.iter().enumerate() {
            let prefix = prim_prefix(prim, glyph_str, prim_index);
            let src = Source {
                glyph: glyph_str,
                prim_index,
            };
            if prim.merges(false) {
                emit_merged(grid, prim, &prefix, seed, &mask, &src, items);
            } else if let Some([w, h]) = prim.footprint {
                emit_footprint(
                    grid,
                    prim,
                    &prefix,
                    seed,
                    glyph,
                    (w, h),
                    &src,
                    items,
                    diagnostics,
                );
            } else {
                emit_per_cell(grid, prim, &prefix, seed, &mask, &src, items);
            }
        }
    }
}

/// A primitive's name prefix: its explicit `name`, else the default (glyph/kind), suffixed with the
/// primitive index for non-first primitives so a multi-primitive stack never self-collides.
fn prim_prefix(prim: &KitPrimitive, default: &str, prim_index: usize) -> String {
    let base = prim.name.as_deref().unwrap_or(default);
    if prim_index == 0 {
        base.to_string()
    } else {
        format!("{base}-p{prim_index}")
    }
}

/// Greedy row-major maximal-rectangle merge over the `mask` cells; one scaled entity per rect.
fn emit_merged(
    grid: &Grid,
    prim: &KitPrimitive,
    prefix: &str,
    seed: u64,
    mask: &dyn Fn(usize, usize) -> bool,
    src: &Source<'_>,
    items: &mut Vec<GridItem>,
) {
    let mut covered = vec![false; grid.width() * grid.height()];
    let idx = |r: usize, c: usize| r * grid.width() + c;
    for r0 in 0..grid.height() {
        for c0 in 0..grid.width() {
            if covered[idx(r0, c0)] || !mask(r0, c0) {
                continue;
            }
            // Extend right along the row.
            let mut c1 = c0;
            while c1 + 1 < grid.width() && !covered[idx(r0, c1 + 1)] && mask(r0, c1 + 1) {
                c1 += 1;
            }
            // Extend down while every column [c0, c1] of the next row is free.
            let mut r1 = r0;
            'grow: while r1 + 1 < grid.height() {
                for c in c0..=c1 {
                    if covered[idx(r1 + 1, c)] || !mask(r1 + 1, c) {
                        break 'grow;
                    }
                }
                r1 += 1;
            }
            for r in r0..=r1 {
                for c in c0..=c1 {
                    covered[idx(r, c)] = true;
                }
            }
            let cols = (c1 - c0 + 1) as f64;
            let rows = (r1 - r0 + 1) as f64;
            let center = grid.rect_center(r0, c0, r1, c1);
            let scale = [prim.scale[0] * cols, prim.scale[1], prim.scale[2] * rows];
            let name = format!(
                "{prefix}-r{r0:03}-c{c0:03}-w{:02}-h{:02}",
                c1 - c0 + 1,
                r1 - r0 + 1
            );
            let variant = variant_pick(seed, r0, c0, prim.candidate_count());
            let collider = prim
                .collider
                .map(|shape| scale_collider(shape, cols, rows, grid.cell_size()));
            let entity = build_entity(
                &name,
                center,
                prim.y,
                0.0,
                scale,
                prim,
                variant,
                collider,
                &[],
            );
            items.push(GridItem {
                r0,
                c0,
                seq: items.len(),
                entities: entity,
                origin: src.origin(r0, c0),
            });
        }
    }
}

/// Greedy row-major fixed `w × h`-block tiling over the glyph's cells; one unscaled prop per block.
/// A cell that cannot be covered by a whole block is `L1026`.
#[allow(clippy::too_many_arguments)]
fn emit_footprint(
    grid: &Grid,
    prim: &KitPrimitive,
    prefix: &str,
    seed: u64,
    glyph: char,
    (w, h): (u32, u32),
    src: &Source<'_>,
    items: &mut Vec<GridItem>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let (w, h) = (w as usize, h as usize);
    let mut covered = vec![false; grid.width() * grid.height()];
    let idx = |r: usize, c: usize| r * grid.width() + c;
    let is_glyph = |r: usize, c: usize| grid.glyph_at(r, c) == Some(glyph);
    let mut reported_rows = StableSet::new();
    for r0 in 0..grid.height() {
        for c0 in 0..grid.width() {
            if covered[idx(r0, c0)] || !is_glyph(r0, c0) {
                continue;
            }
            // A block must fit inside the grid and be entirely uncovered glyph cells.
            let fits = r0 + h <= grid.height()
                && c0 + w <= grid.width()
                && (r0..r0 + h)
                    .all(|r| (c0..c0 + w).all(|c| !covered[idx(r, c)] && is_glyph(r, c)));
            if !fits {
                if reported_rows.insert(r0) {
                    diagnostics.push(
                        Diagnostic::new(
                            code::MAP_FOOTPRINT_MISFIT,
                            format!(
                                "a `{glyph}` run at row {r0}, col {c0} does not divide evenly into \
                                 whole {w}x{h}-cell footprint pieces"
                            ),
                        )
                        .with_path(format!("/rows/{r0}"))
                        .with_help(format!(
                            "make the glyph's runs a whole multiple of the {w}x{h} footprint, or \
                             pick a piece whose footprint divides the run"
                        )),
                    );
                }
                covered[idx(r0, c0)] = true; // avoid re-reporting the same stuck cell
                continue;
            }
            for r in r0..r0 + h {
                for c in c0..c0 + w {
                    covered[idx(r, c)] = true;
                }
            }
            let center = grid.rect_center(r0, c0, r0 + h - 1, c0 + w - 1);
            let name = format!("{prefix}-r{r0:03}-c{c0:03}");
            let variant = variant_pick(seed, r0, c0, prim.candidate_count());
            let entity = build_entity(
                &name,
                center,
                prim.y,
                0.0,
                prim.scale,
                prim,
                variant,
                prim.collider,
                &[],
            );
            items.push(GridItem {
                r0,
                c0,
                seq: items.len(),
                entities: entity,
                origin: src.origin(r0, c0),
            });
        }
    }
}

/// One unscaled entity per `mask` cell.
fn emit_per_cell(
    grid: &Grid,
    prim: &KitPrimitive,
    prefix: &str,
    seed: u64,
    mask: &dyn Fn(usize, usize) -> bool,
    src: &Source<'_>,
    items: &mut Vec<GridItem>,
) {
    for r in 0..grid.height() {
        for c in 0..grid.width() {
            if !mask(r, c) {
                continue;
            }
            let center = grid.rect_center(r, c, r, c);
            let name = format!("{prefix}-r{r:03}-c{c:03}");
            let variant = variant_pick(seed, r, c, prim.candidate_count());
            let entity = build_entity(
                &name,
                center,
                prim.y,
                0.0,
                prim.scale,
                prim,
                variant,
                prim.collider,
                &[],
            );
            items.push(GridItem {
                r0: r,
                c0: c,
                seq: items.len(),
                entities: entity,
                origin: src.origin(r, c),
            });
        }
    }
}

/// Emits a marker's entities: one per kit primitive when a `kind` is set (the verbatim `components`
/// merged onto the first), else a single entity carrying just the `Transform` and `components`.
fn emit_marker(
    grid: &Grid,
    kit: &MapKit,
    seed: u64,
    marker: &MapMarker,
    index: usize,
    entities: &mut Vec<SceneEntity>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let base = format!("/markers/{index}");
    let Some((row, col)) = grid.cell_of(marker.x, marker.z) else {
        diagnostics.push(
            Diagnostic::new(
                code::MAP_MARKER_OUT_OF_BOUNDS,
                format!(
                    "marker {index} at ({}, {}) lies outside the grid",
                    marker.x, marker.z
                ),
            )
            .with_path(base)
            .with_help("move the marker inside the grid extent, or enlarge/shift the grid"),
        );
        return;
    };
    let rotation = marker.rotation.unwrap_or(0.0);
    let center = [marker.x, marker.z];
    let prefix = marker.derived_name(index);

    let Some(kind) = &marker.kind else {
        // Pure passthrough: one entity, Transform + components.
        entities.push(build_entity_marker(
            &prefix,
            center,
            0.0,
            rotation,
            &marker.components,
        ));
        return;
    };

    let Some(primitives) = kit.kind(kind) else {
        let kinds: Vec<&str> = kit.kinds.iter().map(|(k, _)| k.as_str()).collect();
        let mut d = Diagnostic::new(
            code::MAP_UNKNOWN_KIND,
            format!("marker {index} names kind `{kind}`, which the kit does not define"),
        )
        .with_path(format!("{base}/kind"));
        d = match suggest::nearest(kind, kinds) {
            Some(c) => d.with_help(format!("nearest match is `{c}`")),
            None => d.with_help(
                "add the kind to the kit's `kinds`, or drop it for a passthrough marker",
            ),
        };
        diagnostics.push(d);
        return;
    };

    for (prim_index, prim) in primitives.iter().enumerate() {
        let name = if prim_index == 0 {
            prefix.clone()
        } else {
            format!("{prefix}-p{prim_index}")
        };
        let variant = match marker.variant {
            Some(v) if v < prim.candidate_count() => v,
            Some(v) => {
                diagnostics.push(
                    Diagnostic::new(
                        code::MAP_KIT_INVALID,
                        format!(
                            "marker {index} forces variant {v}, but kind `{kind}` primitive \
                             {prim_index} has only {} candidate(s)",
                            prim.candidate_count()
                        ),
                    )
                    .with_path(format!("{base}/variant")),
                );
                0
            }
            None => variant_pick(seed, row, col, prim.candidate_count()),
        };
        let extra = if prim_index == 0 {
            marker.components.as_slice()
        } else {
            &[]
        };
        let entity = build_entity(
            &name,
            center,
            prim.y,
            rotation,
            prim.scale,
            prim,
            variant,
            prim.collider,
            extra,
        );
        entities.extend(entity);
    }
}

// --- Entity/component construction -------------------------------------------------------------

impl KitPrimitive {
    /// The number of mesh candidates (the primitive's own source plus its `variants`).
    fn candidate_count(&self) -> usize {
        1 + self.variants.len()
    }

    /// The mesh source at candidate index `i` (0 = the primitive's own source); `None` for a
    /// collider-only primitive.
    fn candidate(&self, i: usize) -> Option<&MeshSpec> {
        if i == 0 {
            self.mesh.as_ref()
        } else {
            Some(&self.variants[i - 1])
        }
    }
}

/// Builds one entity with a `Transform`, a `Mesh` (variant-selected), an optional `Collider`, and
/// any verbatim passthrough components appended. A 3D `box` collider rides a second, unit-scale
/// `<name>-collider` entity instead: a solver-owned collider on a scaled entity is `L1290`. A
/// collider-only primitive (no mesh source) is one unit-scale entity carrying its `Collider`, of
/// any shape, with no sibling.
#[allow(clippy::too_many_arguments)]
fn build_entity(
    name: &str,
    center: [f64; 2],
    y: f64,
    rotation: f64,
    scale: [f64; 3],
    prim: &KitPrimitive,
    variant: usize,
    collider: Option<ColliderSpec>,
    extra: &[(String, Value)],
) -> Vec<SceneEntity> {
    let Some(mesh) = prim.candidate(variant) else {
        let mut components = vec![(
            "Transform".to_string(),
            transform_value(center, y, rotation, [1.0, 1.0, 1.0]),
        )];
        if let Some(collider) = collider {
            components.push(("Collider".to_string(), collider_value(collider)));
            components.extend(surface_tag(prim));
        }
        components.extend(extra.iter().cloned());
        return vec![SceneEntity {
            presentation: false,
            name: Some(name.to_string()),
            components,
            instance: None,
        }];
    };
    let mut components = vec![
        (
            "Transform".to_string(),
            transform_value(center, y, rotation, scale),
        ),
        ("Mesh".to_string(), mesh_value(mesh, prim)),
    ];
    let mut sibling = None;
    match collider {
        Some(collider @ ColliderSpec::Box(_)) => {
            sibling = Some(box_collider_entity(
                name,
                center,
                y,
                rotation,
                collider,
                prim.surface.as_deref(),
            ));
        }
        Some(collider) => components.push(("Collider".to_string(), collider_value(collider))),
        None => {}
    }
    if prim.is_static {
        components.push(("Static".to_string(), json!({})));
    }
    components.extend(extra.iter().cloned());
    let mut entities = vec![SceneEntity {
        presentation: false,
        name: Some(name.to_string()),
        components,
        instance: None,
    }];
    entities.extend(sibling);
    entities
}

/// The name of the unit-scale sibling entity that carries the kit `box` collider of entity `name`.
pub fn box_collider_name(name: &str) -> String {
    format!("{name}-collider")
}

/// The unit-scale entity that carries a kit `box` collider beside its mesh entity.
fn box_collider_entity(
    name: &str,
    center: [f64; 2],
    y: f64,
    rotation: f64,
    collider: ColliderSpec,
    surface: Option<&str>,
) -> SceneEntity {
    let mut components = vec![
        (
            "Transform".to_string(),
            transform_value(center, y, rotation, [1.0, 1.0, 1.0]),
        ),
        ("Collider".to_string(), collider_value(collider)),
    ];
    components.extend(surface.map(surface_tag_value));
    SceneEntity {
        presentation: false,
        name: Some(box_collider_name(name)),
        components,
        instance: None,
    }
}

/// The `SurfaceTag` component riding beside a primitive's `Collider`, when it names a surface.
fn surface_tag(prim: &KitPrimitive) -> Option<(String, Value)> {
    prim.surface.as_deref().map(surface_tag_value)
}

fn surface_tag_value(surface: &str) -> (String, Value) {
    ("SurfaceTag".to_string(), json!({ "surface": surface }))
}

/// Builds a passthrough-only marker entity: a `Transform` plus the verbatim components.
fn build_entity_marker(
    name: &str,
    center: [f64; 2],
    y: f64,
    rotation: f64,
    extra: &[(String, Value)],
) -> SceneEntity {
    let mut components = vec![(
        "Transform".to_string(),
        transform_value(center, y, rotation, [1.0, 1.0, 1.0]),
    )];
    components.extend(extra.iter().cloned());
    SceneEntity {
        presentation: false,
        name: Some(name.to_string()),
        components,
        instance: None,
    }
}

/// The nearest `f32`, widened back. Every engine field is `f32`, and the canonical writer prints an
/// `f32` field holding an `f32`-exact value as its shortest `f32` decimal, so a computed `f64`
/// (`origin + k * cell_size`) lands on the text the reader would round-trip to, not a 17-digit tail.
fn f32_exact(v: f64) -> f64 {
    f64::from(v as f32)
}

fn transform_value(center: [f64; 2], y: f64, rotation: f64, scale: [f64; 3]) -> Value {
    json!({
        "translation": [f32_exact(center[0]), f32_exact(y), f32_exact(center[1])],
        "rotation": [0.0, f32_exact(rotation), 0.0],
        "scale": [f32_exact(scale[0]), f32_exact(scale[1]), f32_exact(scale[2])],
    })
}

/// How a built-in world-space field reflects across the mirror plane.
#[derive(Clone, Copy)]
enum MirrorKind {
    /// A direction: the mirrored axis component is negated.
    Vector,
    /// An axial vector (angular velocity): the two other components are negated.
    Axial,
    /// A world point: reflected across the grid line like a translation.
    Point,
}

/// Built-in components whose fields hold world-space vectors or points. Game components are copied
/// verbatim.
const MIRRORED_FIELDS: &[(&str, &str, MirrorKind)] = &[
    ("SpotLight", "direction", MirrorKind::Vector),
    ("DirectionalLight", "direction", MirrorKind::Vector),
    ("NavSteering", "velocity", MirrorKind::Vector),
    ("Velocity", "linear", MirrorKind::Vector),
    ("Velocity", "angular", MirrorKind::Axial),
    ("NavAgent", "target", MirrorKind::Point),
];

/// Negates `v`, keeping zero as `+0.0`.
fn negated(v: f64) -> f64 {
    if v == 0.0 { 0.0 } else { -v }
}

/// The coordinate sum of the mirror plane's two reflected values (`reflected = line_sum - v`) on the
/// mirrored axis: `2 * origin + extent * cell_size`. The one rule `generate` and `map-check` share.
fn mirror_line_sum(mirror: Mirror, origin: [f64; 2], size: [usize; 2], cell_size: f64) -> f64 {
    let axis = usize::from(mirror != Mirror::X);
    2.0 * origin[axis] + size[axis] as f64 * cell_size
}

/// Reflects an entity across the grid line whose coordinate sum is `line_sum`. Translation is
/// mirrored while scale is kept, so a piece keeps its handedness and no scale goes negative. X
/// negates yaw and Z maps it to 180-θ in (-180, 180], so the forward direction reflects too. The
/// built-in world-space vectors in [`MIRRORED_FIELDS`] reflect with it; everything else is left as
/// it is.
fn mirror_entity(entity: &mut SceneEntity, mirror: Mirror, line_sum: f64) {
    let axis = match mirror {
        Mirror::None => return,
        Mirror::X => 0,
        Mirror::Z => 2,
    };
    if let Some((_, transform)) = entity.components.iter_mut().find(|(n, _)| n == "Transform") {
        if let Some(t) = transform
            .get_mut("translation")
            .and_then(Value::as_array_mut)
            && let Some(v) = t.get(axis).and_then(Value::as_f64)
        {
            t[axis] = json!(f32_exact(line_sum - v));
        }
        if let Some(r) = transform.get_mut("rotation").and_then(Value::as_array_mut)
            && let Some(yaw) = r.get(1).and_then(Value::as_f64)
        {
            r[1] = json!(if axis == 0 {
                negated(yaw)
            } else {
                let mut wrapped = 180.0 - yaw;
                if wrapped > 180.0 {
                    wrapped -= 360.0;
                }
                if wrapped <= -180.0 {
                    wrapped += 360.0;
                }
                let wrapped = f32_exact(wrapped);
                if wrapped == 0.0 { 0.0 } else { wrapped }
            });
        }
    }
    for &(component, field, kind) in MIRRORED_FIELDS {
        let Some((_, value)) = entity.components.iter_mut().find(|(n, _)| n == component) else {
            continue;
        };
        let Some(obj) = value.as_object_mut() else {
            continue;
        };
        if matches!(kind, MirrorKind::Point) && !obj.contains_key(field) {
            obj.insert(field.to_string(), json!([0.0, 0.0, 0.0]));
        }
        let Some(v) = obj.get_mut(field).and_then(Value::as_array_mut) else {
            continue;
        };
        if v.len() != 3 || !v.iter().all(Value::is_number) {
            continue;
        }
        for (i, slot) in v.iter_mut().enumerate() {
            let n = slot.as_f64().unwrap_or(0.0);
            let out = match kind {
                MirrorKind::Vector if i == axis => negated(n),
                MirrorKind::Axial if i != axis => negated(n),
                MirrorKind::Point if i == axis => f32_exact(line_sum - n),
                _ => continue,
            };
            *slot = json!(out);
        }
    }
}

fn mesh_value(mesh: &MeshSpec, prim: &KitPrimitive) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("shape".to_string(), mesh.shape_value());
    for (key, value) in [
        ("roughness", prim.roughness),
        ("metallic", prim.metallic),
        ("emissive", prim.emissive),
    ] {
        if let Some(v) = value {
            obj.insert(key.to_string(), json!(f32_exact(v)));
        }
    }
    if let Some(c) = prim.color {
        obj.insert(
            "color".to_string(),
            json!([
                f32_exact(c[0]),
                f32_exact(c[1]),
                f32_exact(c[2]),
                f32_exact(c[3])
            ]),
        );
    }
    Value::Object(obj)
}

fn collider_value(spec: ColliderSpec) -> Value {
    let shape = match spec {
        ColliderSpec::Aabb([x, z]) => {
            json!({ "aabb": { "half_extents": [f32_exact(x), f32_exact(z)] } })
        }
        ColliderSpec::Circle(r) => json!({ "circle": { "radius": f32_exact(r) } }),
        ColliderSpec::Box([x, y, z]) => {
            json!({ "box": { "half_extents": [f32_exact(x), f32_exact(y), f32_exact(z)] } })
        }
    };
    json!({ "shape": shape, "body": "static" })
}

/// A merged rect's collider grows to cover the rect. [`validate_kit`] guarantees a merged primitive's
/// collider is a cell-sized `aabb` or `box`, so the circle arm is never reached.
fn scale_collider(spec: ColliderSpec, cols: f64, rows: f64, cell_size: f64) -> ColliderSpec {
    match spec {
        ColliderSpec::Aabb(_) => {
            ColliderSpec::Aabb([cols * cell_size / 2.0, rows * cell_size / 2.0])
        }
        ColliderSpec::Box([_, hy, _]) => {
            ColliderSpec::Box([cols * cell_size / 2.0, hy, rows * cell_size / 2.0])
        }
        ColliderSpec::Circle(r) => ColliderSpec::Circle(r),
    }
}

// --- Semantic validation -----------------------------------------------------------------------

/// Validates the grid: rectangular rows (`L1019`) and every non-void glyph present in the kit
/// (`L1020`). Returns the grid extent `(width, height)` when rectangular — or the declared `bounds`
/// of a grid-less map — else `None`; markers on an empty grid are `L1023` here, once.
fn check_grid(
    map: &MapDoc,
    kit: &MapKit,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<(usize, usize)> {
    if let Some([w, h]) = map.frame() {
        return Some((w as usize, h as usize));
    }
    if map.rows.is_empty() {
        if !map.markers.is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    code::MAP_MARKER_OUT_OF_BOUNDS,
                    format!(
                        "the map has {} marker(s) but no grid frame: `rows` is empty, so every \
                            marker lies outside it",
                        map.markers.len()
                    ),
                )
                .with_path("/rows")
                .with_help(
                    "add grid `rows`, or declare `bounds: [width, height]` for a marker-only map",
                ),
            );
        }
        return None;
    }
    let width = map.rows[0].chars().count();
    let mut ragged = false;
    for (r, row) in map.rows.iter().enumerate() {
        let len = row.chars().count();
        if len != width {
            diagnostics.push(
                Diagnostic::new(
                    code::TILEMAP_RAGGED_ROWS,
                    format!("row {r} has {len} glyphs but row 0 has {width}; the grid is not rectangular"),
                )
                .with_path(format!("/rows/{r}"))
                .with_help("pad every row to the same glyph count (a space is the void/empty glyph)"),
            );
            ragged = true;
        }
    }

    // Unknown glyphs: every non-void glyph must have a kit tile entry. Report each once.
    let mut reported = StableSet::new();
    for (r, row) in map.rows.iter().enumerate() {
        for (c, ch) in row.chars().enumerate() {
            if ch == VOID_GLYPH || reported.contains(&ch) {
                continue;
            }
            if kit.tile(&ch.to_string()).is_none() {
                reported.insert(ch);
                diagnostics.push(
                    Diagnostic::new(
                        code::TILEMAP_INVALID_GLYPH,
                        format!(
                            "the glyph `{ch}` at row {r}, col {c} has no `tiles` entry in the kit"
                        ),
                    )
                    .with_path(format!("/rows/{r}"))
                    .with_help(format!(
                        "add `{ch}` to the kit's `tiles`, or use a space for an empty cell"
                    )),
                );
            }
        }
    }

    // The base glyph, if named, must be a single char with a kit tile entry.
    if let Some(base) = &map.base {
        match single_char(base) {
            Some(ch) if kit.tile(&ch.to_string()).is_some() => {}
            _ => diagnostics.push(
                Diagnostic::new(
                    code::TILEMAP_INVALID_GLYPH,
                    format!("the base glyph `{base}` is not a single glyph with a `tiles` entry"),
                )
                .with_path("/base")
                .with_help("set `base` to one glyph that the kit's `tiles` defines"),
            ),
        }
    }

    (!ragged).then_some((width, map.rows.len()))
}

/// Validates the kit's primitives: a non-empty list per entry, a valid `footprint`, no
/// `merge`+`footprint` clash, and a merged primitive's collider covering exactly its cells (`L1022`);
/// a single-char, non-void key per glyph (`L1020`).
fn validate_kit(kit: &MapKit, map: &MapDoc, diagnostics: &mut Vec<Diagnostic>) {
    let base = map.base.as_deref().and_then(single_char);
    for (glyph, primitives) in &kit.tiles {
        let pointer = format!("/tiles/{}", pointer_token(glyph));
        match single_char(glyph) {
            None => diagnostics.push(
                Diagnostic::new(
                    code::TILEMAP_INVALID_GLYPH,
                    format!("the kit tile key `{glyph}` is not a single glyph"),
                )
                .with_path(pointer.clone())
                .with_help("each `tiles` key is exactly one character"),
            ),
            Some(VOID_GLYPH) => diagnostics.push(
                Diagnostic::new(
                    code::TILEMAP_INVALID_GLYPH,
                    "the kit tile key ` ` is the reserved void glyph, so its cells would never emit",
                )
                .with_path(pointer.clone())
                .with_help("a space in `rows` is an empty cell; give this tile another glyph"),
            ),
            Some(_) => {}
        }
        validate_primitive_list(primitives, &pointer, diagnostics);
        let is_base = base.is_some() && single_char(glyph) == base;
        for (i, prim) in primitives.iter().enumerate() {
            if prim.merges(is_base) {
                validate_merged_collider(
                    prim,
                    map.cell_size,
                    &format!("{pointer}/{i}"),
                    diagnostics,
                );
            }
        }
    }
    for (kind, primitives) in &kit.kinds {
        let pointer = format!("/kinds/{}", pointer_token(kind));
        validate_primitive_list(primitives, &pointer, diagnostics);
        for (i, prim) in primitives.iter().enumerate() {
            if prim.is_static {
                diagnostics.push(
                    Diagnostic::new(
                        code::MAP_KIT_INVALID,
                        "a marker kind is a gameplay entity; `static` marks tile dressing only",
                    )
                    .with_path(format!("{pointer}/{i}/static"))
                    .with_help("remove `static` from the kind's primitive"),
                );
            }
        }
    }
}

/// A merged primitive's rect grows by whole cells, so its collider must be a cell-sized box: a circle
/// cannot cover a rect, and any other box would be silently replaced by the rect's extent.
fn validate_merged_collider(
    prim: &KitPrimitive,
    cell_size: f64,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let half = cell_size / 2.0;
    let problem = match prim.collider {
        None => return,
        Some(ColliderSpec::Circle(_)) => {
            "a `circle` collider cannot cover a merged rectangle".to_string()
        }
        Some(ColliderSpec::Aabb(h)) if h != [half, half] => format!(
            "the `aabb` of a merged primitive must be one cell, [{half}, {half}] at `cell_size` \
                {cell_size}, found [{}, {}]",
            h[0], h[1]
        ),
        Some(ColliderSpec::Box([hx, _, hz])) if [hx, hz] != [half, half] => format!(
            "the `box` of a merged primitive must be one cell in x and z, [{half}, y, {half}] at \
             `cell_size` {cell_size}, found [{hx}, y, {hz}]"
        ),
        Some(ColliderSpec::Aabb(_) | ColliderSpec::Box(_)) => return,
    };
    diagnostics.push(
        Diagnostic::new(code::MAP_KIT_INVALID, problem)
            .with_path(format!("{pointer}/collider"))
            .with_help(format!(
                "a merged primitive (`merge: true`, or a `base` glyph primitive without `merge: false`) takes `aabb: \
                    [{half}, {half}]` or `box: [{half}, hy, {half}]`; its collider grows to the \
                 merged rect. Use a per-cell glyph for other shapes"
            )),
    );
}

fn validate_primitive_list(
    primitives: &[KitPrimitive],
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if primitives.is_empty() {
        diagnostics.push(
            Diagnostic::new(
                code::MAP_KIT_INVALID,
                "a kit entry has an empty primitive list",
            )
            .with_path(pointer.to_string())
            .with_help("list at least one primitive, or remove the entry"),
        );
    }
    for (i, prim) in primitives.iter().enumerate() {
        let p = format!("{pointer}/{i}");
        if prim.merge == Some(true) && prim.footprint.is_some() {
            diagnostics.push(
                Diagnostic::new(
                    code::MAP_KIT_INVALID,
                    "a primitive sets both `merge` and `footprint`; choose one expansion mode",
                )
                .with_path(p.clone()),
            );
        }
        if let Some([w, h]) = prim.footprint
            && (w == 0 || h == 0)
        {
            diagnostics.push(
                Diagnostic::new(
                    code::MAP_KIT_INVALID,
                    format!("a `footprint` must be two positive integers, found [{w}, {h}]"),
                )
                .with_path(format!("{p}/footprint")),
            );
        }
        if !prim.y.is_finite() || prim.scale.iter().any(|s| !s.is_finite()) {
            diagnostics.push(
                Diagnostic::new(
                    code::MAP_KIT_INVALID,
                    "a primitive has a non-finite `y` or `scale`",
                )
                .with_path(p.clone()),
            );
        }
        if prim.color.is_some_and(|c| c.iter().any(|v| !v.is_finite())) {
            diagnostics.push(
                Diagnostic::new(
                    code::MAP_KIT_INVALID,
                    "a primitive has a non-finite `color` component",
                )
                .with_path(format!("{p}/color")),
            );
        }
        for (key, value, max, default) in [
            ("roughness", prim.roughness, Some(1.0), "1"),
            ("metallic", prim.metallic, Some(1.0), "0"),
            ("emissive", prim.emissive, None, "0"),
        ] {
            if let Some(v) = value
                && (!v.is_finite() || v < 0.0 || max.is_some_and(|m| v > m))
            {
                let range = if max.is_some() { "in [0, 1]" } else { ">= 0" };
                diagnostics.push(
                    Diagnostic::new(
                        code::MAP_KIT_INVALID,
                        format!("`{key}` {v} is out of range: it must be finite and {range}, as on `Mesh`"),
                    )
                    .with_path(format!("{p}/{key}"))
                    .with_help(format!(
                        "use a value {range}; absent means the `Mesh` default, {default}"
                    )),
                );
            }
        }
        let collider_finite = match prim.collider {
            Some(ColliderSpec::Aabb(half)) => half.iter().all(|v| v.is_finite()),
            Some(ColliderSpec::Circle(radius)) => radius.is_finite(),
            Some(ColliderSpec::Box(half)) => half.iter().all(|v| v.is_finite()),
            None => true,
        };
        if !collider_finite {
            diagnostics.push(
                Diagnostic::new(
                    code::MAP_KIT_INVALID,
                    "a primitive has a non-finite `collider` size",
                )
                .with_path(format!("{p}/collider")),
            );
        }
    }
}

/// Every named entity's name must be unique across fragments, grid, and markers (`L1025`);
/// `origins` runs parallel to `entities` and names both copies in the diagnostic.
fn check_name_collisions(
    entities: &[SceneEntity],
    origins: &[Origin],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut seen: StableMap<&str, usize> = StableMap::new();
    let mut reported = StableSet::new();
    for (index, entity) in entities.iter().enumerate() {
        let Some(name) = entity.name.as_deref() else {
            continue;
        };
        let Some(&first) = seen.get(&name) else {
            seen.insert(name, index);
            continue;
        };
        if reported.insert(name) {
            let (a, b) = (&origins[first], &origins[index]);
            diagnostics.push(
                Diagnostic::new(
                    code::MAP_NAME_COLLISION,
                    format!(
                        "the generated entity name `{name}` is used more than once: by {} ({}) and \
                            by {} ({})",
                        a.label, a.pointer, b.label, b.pointer
                    ),
                )
                .with_path(b.pointer.clone())
                .with_help(
                    "give the colliding kit primitive or marker a distinct `name`, or rename the \
                     fragment entity",
                ),
            );
        }
    }
}

// --- Deterministic variant selection -----------------------------------------------------------

/// Picks a candidate index from `(seed, row, col)` with a splitmix64 finalizer — independent per
/// cell, so an edit in one cell never changes another cell's pick. `count <= 1` always yields 0.
fn variant_pick(seed: u64, row: usize, col: usize, count: usize) -> usize {
    if count <= 1 {
        return 0;
    }
    let mut z = seed
        ^ (row as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (col as u64)
            .wrapping_mul(0xc2b2_ae3d_27d4_eb4f)
            .rotate_left(32);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z % count as u64) as usize
}

// --- Small parse helpers -----------------------------------------------------------------------

fn root_mapping(source: &str, minimal: &str) -> Result<Vec<MapEntry>, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|d| vec![d])?;
    let Some(root) = root else {
        return Err(vec![
            structure(&format!(
                "the document is empty; a minimal one needs `format: 1` and `{minimal}`"
            ))
            .with_path(""),
        ]);
    };
    match root.value {
        NodeValue::Mapping(entries) => Ok(entries),
        other => Err(vec![
            structure(&format!(
                "the document root must be a mapping, found {} (line {})",
                other.type_phrase(),
                root.line
            ))
            .with_path(""),
        ]),
    }
}

fn structure(message: &str) -> Diagnostic {
    Diagnostic::new(code::MAP_DOCUMENT_STRUCTURE, message.to_string())
}

fn missing(key: &str, help: &str) -> Diagnostic {
    Diagnostic::new(
        code::MAP_DOCUMENT_STRUCTURE,
        format!("the document is missing the required top-level key `{key}`"),
    )
    .with_path("")
    .with_help(help.to_string())
}

fn unknown_map_key(key: &str, entry: &MapEntry) -> Diagnostic {
    let mut valid: Vec<&'static str> = MAP_KEYS.to_vec();
    valid.extend_from_slice(&MAP_KEYS_EXTRA);
    unknown_key_dyn(key, entry, "", "a map document", &valid)
}

fn unknown_key_dyn(
    key: &str,
    entry: &MapEntry,
    base: &str,
    context: &str,
    valid: &[&str],
) -> Diagnostic {
    let mut d = Diagnostic::new(
        code::MAP_DOCUMENT_STRUCTURE,
        format!(
            "{context} does not have a key named `{key}` (line {})",
            entry.key_line
        ),
    )
    .with_path(format!("{base}/{}", pointer_token(key)));
    d = match suggest::nearest(key, valid.iter().copied()) {
        Some(c) => d.with_help(format!("nearest match is `{c}`")),
        None => d.with_help(format!("valid keys are: {}", valid.join(", "))),
    };
    d
}

fn expect_f64(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<f64> {
    match &node.value {
        NodeValue::Scalar(Value::Number(n)) => Some(crate::fields::number(n)),
        other => {
            diagnostics.push(
                structure(&format!(
                    "`{what}` must be a number, found {} (line {})",
                    other.type_phrase(),
                    node.line
                ))
                .with_path(pointer.to_string()),
            );
            None
        }
    }
}

fn expect_u64(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<u64> {
    match &node.value {
        NodeValue::Scalar(Value::Number(n)) if n.as_u64().is_some() => n.as_u64(),
        other => {
            diagnostics.push(
                structure(&format!(
                    "`{what}` must be a non-negative integer, found {} (line {})",
                    other.type_phrase(),
                    node.line
                ))
                .with_path(pointer.to_string()),
            );
            None
        }
    }
}

fn expect_bool(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<bool> {
    match &node.value {
        NodeValue::Scalar(Value::Bool(b)) => Some(*b),
        other => {
            diagnostics.push(
                structure(&format!(
                    "`{what}` must be a boolean, found {} (line {})",
                    other.type_phrase(),
                    node.line
                ))
                .with_path(pointer.to_string()),
            );
            None
        }
    }
}

fn expect_number_array<const N: usize>(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<[f64; N]> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            structure(&format!(
                "`{what}` must be a sequence of {N} numbers, found {} (line {})",
                node.value.type_phrase(),
                node.line
            ))
            .with_path(pointer.to_string()),
        );
        return None;
    };
    if items.len() != N {
        diagnostics.push(
            structure(&format!(
                "`{what}` must have exactly {N} numbers, found {}",
                items.len()
            ))
            .with_path(pointer.to_string()),
        );
        return None;
    }
    let mut out = [0.0f64; N];
    let mut ok = true;
    for (i, item) in items.iter().enumerate() {
        match &item.value {
            NodeValue::Scalar(Value::Number(n)) => out[i] = crate::fields::number(n),
            other => {
                diagnostics.push(
                    structure(&format!(
                        "`{what}[{i}]` must be a number, found {} (line {})",
                        other.type_phrase(),
                        item.line
                    ))
                    .with_path(format!("{pointer}/{i}")),
                );
                ok = false;
            }
        }
    }
    ok.then_some(out)
}

fn expect_pair(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<[u32; 2]> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            structure(&format!(
                "`{what}` must be a sequence of two integers, found {} (line {})",
                node.value.type_phrase(),
                node.line
            ))
            .with_path(pointer.to_string()),
        );
        return None;
    };
    if items.len() != 2 {
        diagnostics.push(
            structure(&format!(
                "`{what}` must have exactly 2 integers, found {}",
                items.len()
            ))
            .with_path(pointer.to_string()),
        );
        return None;
    }
    let mut out = [0u32; 2];
    let mut ok = true;
    for (i, item) in items.iter().enumerate() {
        match &item.value {
            NodeValue::Scalar(Value::Number(n)) if n.as_u64().is_some() => {
                let v = n.as_u64().unwrap();
                if let Ok(v) = u32::try_from(v) {
                    out[i] = v;
                } else {
                    diagnostics.push(
                        structure(&format!(
                            "`{what}[{i}]` is {v}, above the largest {}",
                            u32::MAX
                        ))
                        .with_path(format!("{pointer}/{i}")),
                    );
                    ok = false;
                }
            }
            other => {
                diagnostics.push(
                    structure(&format!(
                        "`{what}[{i}]` must be a non-negative integer, found {} (line {})",
                        other.type_phrase(),
                        item.line
                    ))
                    .with_path(format!("{pointer}/{i}")),
                );
                ok = false;
            }
        }
    }
    ok.then_some(out)
}

fn expect_string_seq(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<String> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            structure(&format!(
                "`{what}` must be a sequence of strings, found {} (line {})",
                node.value.type_phrase(),
                node.line
            ))
            .with_path(pointer.to_string()),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        match &item.value {
            NodeValue::Scalar(Value::String(s)) => out.push(s.clone()),
            other => diagnostics.push(
                structure(&format!(
                    "`{what}[{i}]` must be a string, found {} (line {})",
                    other.type_phrase(),
                    item.line
                ))
                .with_path(format!("{pointer}/{i}"))
                .with_help("quote the value so a numeric- or boolean-looking line stays a string"),
            ),
        }
    }
    out
}

#[cfg(test)]
#[path = "map_tests.rs"]
mod tests;
