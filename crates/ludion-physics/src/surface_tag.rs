//! [`SurfaceId`], [`SurfaceTable`] and [`SurfaceTag`]: the named surfaces a scene declares and the
//! side component that tags a collider with one (`Plan/surface-tags.md`). The solver never reads a
//! tag; only hits report it.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The most surfaces a scene may declare: a [`SurfaceId`] indexes the declared list.
pub const MAX_SURFACES: usize = 256;

/// A declared surface: its index in the scene's `physics.surfaces`.
///
/// ```
/// use ludion_physics::{SurfaceId, SurfaceTable};
///
/// let table = SurfaceTable::new(vec!["concrete".into(), "metal_grate".into()], None);
/// assert_eq!(table.id("metal_grate"), Some(SurfaceId(1)));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SurfaceId(pub u16);

/// A scene's declared surfaces as the physics world reads them: the names in declaration order (the
/// order is the [`SurfaceId`]) and the optional default every untagged collider hit reports.
///
/// ```
/// use ludion_physics::{SurfaceId, SurfaceTable};
///
/// let table = SurfaceTable::new(
///     vec!["concrete".into(), "dirt".into()],
///     Some(SurfaceId(1)),
/// );
/// assert_eq!(table.name(SurfaceId(0)), Some("concrete"));
/// assert_eq!(table.default(), Some(SurfaceId(1)));
/// assert_eq!(table.id("lava"), None);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceTable {
    names: Vec<String>,
    default: Option<SurfaceId>,
}

impl SurfaceTable {
    /// A table over `names`, `default` indexing into it. More than [`MAX_SURFACES`] names or an
    /// out-of-range default is a host bug, not scene data, and panics.
    ///
    /// # Panics
    /// When `names` is longer than [`MAX_SURFACES`] or `default` is not an index into it.
    pub fn new(names: Vec<String>, default: Option<SurfaceId>) -> Self {
        assert!(
            names.len() <= MAX_SURFACES,
            "a surface table holds at most {MAX_SURFACES} surfaces, got {}",
            names.len()
        );
        assert!(
            default.is_none_or(|id| usize::from(id.0) < names.len()),
            "the default surface must index the declared names"
        );
        SurfaceTable { names, default }
    }

    /// The id of `name`, or `None` when the table does not declare it.
    pub fn id(&self, name: &str) -> Option<SurfaceId> {
        self.names
            .iter()
            .position(|candidate| candidate == name)
            .map(|i| SurfaceId(i as u16))
    }

    /// The name of `id`, or `None` when it is out of range.
    pub fn name(&self, id: SurfaceId) -> Option<&str> {
        self.names.get(usize::from(id.0)).map(String::as_str)
    }

    /// The surface an untagged collider hit reports, if the scene sets one.
    pub fn default(&self) -> Option<SurfaceId> {
        self.default
    }

    /// Every declared name, in declaration order.
    pub fn names(&self) -> &[String] {
        &self.names
    }
}

/// Tags the entity's collider with a surface declared under `physics.surfaces`. Every hit that
/// names the collider reports it (`surface` on a query hit, `MoveReport.ground.surface`). Put it
/// beside a solver-claimed `Collider` or a `CharacterMotor`; an entity with neither is `L1692` in
/// `check-scene`. The name must be declared (`L1690`) and not empty (`L1691`). The solver never
/// reads it: editing it takes effect from the next write with no body rebuilt.
///
/// `parts` gives single parts their own surface. An integer key indexes the entity's own `compound`
/// `Collider` parts: a hit reports the part whose shape lies nearest the hit point (lowest index on
/// ties). A string key names a `kind: assembly` part and takes effect on the bodies
/// `switch_to_ragdoll` spawns. Every other part reports `surface`. A part *named* `"1"` must be
/// quoted in YAML, or it parses as the integer.
///
/// `cells` tags a `heightfield` collider per cell (`cells: { surfaces: [dirt, concrete], runs:
/// [[40, 0], [8, 1]] }`, row-major, `(rows-1)*(cols-1)` cells); a mesh collider's triangles are
/// tagged in its mesh document instead.
///
/// ```yaml
/// components:
///   SurfaceTag:
///     surface: metal_grate
///     parts:
///       - { part: 1, surface: concrete }
///       - { part: helmet, surface: metal }
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct SurfaceTag {
    /// The declared surface name.
    pub surface: String,
    /// Per-part overrides; a part without an entry reports `surface`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<PartSurface>,
    /// Per-cell surfaces for a `heightfield` collider; a cell outside the table reports the part,
    /// then `surface`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cells: Option<CellSurfaces>,
}

/// A heightfield's per-cell surface table: local names, and `[count, local index]` runs over the
/// `(rows-1)*(cols-1)` cells in row-major order.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
pub struct CellSurfaces {
    /// The surface names the runs index; each must be declared under `physics.surfaces`.
    pub surfaces: Vec<String>,
    /// Run-length `[count, index into surfaces]` pairs, row-major over the cells.
    pub runs: Vec<[u32; 2]>,
}

impl CellSurfaces {
    /// The index into [`surfaces`](Self::surfaces) of each cell, the runs expanded.
    pub fn per_cell(&self) -> Vec<u16> {
        let mut out = Vec::new();
        for [count, index] in &self.runs {
            out.extend(std::iter::repeat_n(*index as u16, *count as usize));
        }
        out
    }
}

/// One [`SurfaceTag::parts`] entry: a part key and the declared surface it reports.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
pub struct PartSurface {
    /// An index into the entity's `compound` parts, or an assembly part name.
    pub part: SurfacePart,
    /// The declared surface name.
    pub surface: String,
}

/// Which part a [`PartSurface`] names. `Index` comes first so `1` parses as an integer; a part
/// named `"1"` must be quoted.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(untagged)]
pub enum SurfacePart {
    /// A part of the entity's own `compound` collider.
    Index(u32),
    /// A part of a `kind: assembly` mesh.
    Name(String),
}

impl SurfaceTag {
    /// The integer-keyed entries (compound part index, surface name); string keys never reach the
    /// physics world.
    pub(crate) fn index_parts(&self) -> Vec<(u32, String)> {
        self.parts
            .iter()
            .filter_map(|p| match &p.part {
                SurfacePart::Index(i) => Some((*i, p.surface.clone())),
                SurfacePart::Name(_) => None,
            })
            .collect()
    }
}

impl SemanticValidate for SurfaceTag {
    /// Rejects an empty name (`L1691`, pointer `/surface`), an empty part surface (`L1691`,
    /// `/parts/<j>/surface`) and an empty or repeated part key (`L1694`, `/parts/<j>/part`). A wrong
    /// *shape* is the schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut found = Vec::new();
        if value.get("surface").and_then(Value::as_str) == Some("") {
            found.push(
                Diagnostic::new(code::SURFACES_INVALID, "`SurfaceTag` `surface` is empty")
                    .with_path("/surface")
                    .with_help("name a surface `physics.surfaces` declares"),
            );
        }
        found.extend(validate_cells(value.get("cells")));
        let Some(parts) = value.get("parts").and_then(Value::as_array) else {
            return found;
        };
        for (j, entry) in parts.iter().enumerate() {
            if entry.get("surface").and_then(Value::as_str) == Some("") {
                found.push(
                    Diagnostic::new(
                        code::SURFACES_INVALID,
                        "`SurfaceTag` part `surface` is empty",
                    )
                    .with_path(format!("/parts/{j}/surface"))
                    .with_help("name a surface `physics.surfaces` declares"),
                );
            }
            let Some(key) = entry.get("part") else {
                continue;
            };
            if key.as_str() == Some("") {
                found.push(
                    Diagnostic::new(code::SURFACE_PART_INVALID, "`SurfaceTag` part key is empty")
                        .with_path(format!("/parts/{j}/part"))
                        .with_help("use a compound part index or an assembly part name"),
                );
            } else if parts[..j]
                .iter()
                .any(|earlier| earlier.get("part") == Some(key))
            {
                found.push(
                    Diagnostic::new(
                        code::SURFACE_PART_INVALID,
                        format!("`SurfaceTag` part key {key} appears twice"),
                    )
                    .with_path(format!("/parts/{j}/part"))
                    .with_help("give each part one entry"),
                );
            }
        }
        found
    }
}

/// `L1696` for a malformed `cells` table: an empty or repeated name, a zero count, or a local
/// index past the name list. The cell total is checked against the grid by `check-scene`.
fn validate_cells(cells: Option<&Value>) -> Vec<Diagnostic> {
    let Some(cells) = cells else {
        return Vec::new();
    };
    let invalid = |message: String, pointer: String, help: &str| {
        Diagnostic::new(code::MESH_SURFACES_INVALID, message)
            .with_path(pointer)
            .with_help(help.to_string())
    };
    let mut found = Vec::new();
    let names: Vec<&str> = cells
        .get("surfaces")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    for (i, name) in names.iter().enumerate() {
        if name.is_empty() {
            found.push(invalid(
                format!("`SurfaceTag` cells surface {i} is empty"),
                format!("/cells/surfaces/{i}"),
                "name a surface `physics.surfaces` declares",
            ));
        } else if names[..i].contains(name) {
            found.push(invalid(
                format!("`SurfaceTag` cells surface `{name}` appears twice"),
                format!("/cells/surfaces/{i}"),
                "list each surface once and point runs at its single index",
            ));
        }
    }
    let runs = cells
        .get("runs")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for (k, run) in runs.iter().enumerate() {
        let count = run.get(0).and_then(Value::as_u64);
        let index = run.get(1).and_then(Value::as_u64);
        if count == Some(0) {
            found.push(invalid(
                format!("`SurfaceTag` cells run {k} has a zero count"),
                format!("/cells/runs/{k}"),
                "drop the empty run",
            ));
        }
        if index.is_some_and(|index| index as usize >= names.len()) {
            found.push(invalid(
                format!(
                    "`SurfaceTag` cells run {k} names local surface {}, but `surfaces` lists {}",
                    index.unwrap_or(0),
                    names.len()
                ),
                format!("/cells/runs/{k}"),
                "use an index below the length of `cells.surfaces`",
            ));
        }
    }
    found
}

/// A patch of ground that overrides the surface of whatever lies inside it: a puddle, a rug, a
/// wet patch. Put it on an entity with a solver-claimed sensor `Collider` (`L1693` otherwise).
/// Any hit whose point lies inside one or more overlays reports the highest `priority`'s surface,
/// ties to the lowest `EntityId`. The name must be declared (`L1690`) and not empty (`L1691`).
/// Tags only: an overlay never reaches the solver.
///
/// ```yaml
/// components:
///   Collider: { shape: { cylinder: { radius: 1.2, half_height: 0.05 } }, body: static, sensor: true }
///   SurfaceOverlay: { surface: puddle, priority: 1 }
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct SurfaceOverlay {
    /// The declared surface name.
    pub surface: String,
    /// The higher priority wins where overlays overlap; ties go to the lowest entity.
    #[serde(skip_serializing_if = "is_zero")]
    pub priority: i32,
}

fn is_zero(value: &i32) -> bool {
    *value == 0
}

impl SemanticValidate for SurfaceOverlay {
    /// Rejects an empty name (`L1691`, pointer `/surface`). A wrong *shape* is the schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        match value.get("surface").and_then(Value::as_str) {
            Some("") => vec![
                Diagnostic::new(
                    code::SURFACES_INVALID,
                    "`SurfaceOverlay` `surface` is empty",
                )
                .with_path("/surface")
                .with_help("name a surface `physics.surfaces` declares"),
            ],
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_empty_overlay_surface_is_l1691() {
        let found = SurfaceOverlay::validate_semantics(&json!({ "surface": "" }));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path.as_deref(), Some("/surface"));
        assert!(SurfaceOverlay::validate_semantics(&json!({ "surface": "puddle" })).is_empty());
    }

    #[test]
    fn an_empty_surface_is_l1691() {
        let found = SurfaceTag::validate_semantics(&json!({ "surface": "" }));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path.as_deref(), Some("/surface"));
        assert!(SurfaceTag::validate_semantics(&json!({ "surface": "dirt" })).is_empty());
        assert!(SurfaceTag::validate_semantics(&json!({})).is_empty());
    }

    #[test]
    fn part_entries_validate_their_surface_and_key() {
        let tag = json!({ "surface": "a", "parts": [
            { "part": 1, "surface": "" },
            { "part": "", "surface": "b" },
            { "part": 1, "surface": "b" },
            { "part": "helmet", "surface": "b" },
        ] });
        let found = SurfaceTag::validate_semantics(&tag);
        let at: Vec<_> = found
            .iter()
            .map(|d| (d.code, d.path.as_deref().unwrap()))
            .collect();
        assert_eq!(
            at,
            [
                (code::SURFACES_INVALID, "/parts/0/surface"),
                (code::SURFACE_PART_INVALID, "/parts/1/part"),
                (code::SURFACE_PART_INVALID, "/parts/2/part"),
            ]
        );
    }

    #[test]
    fn an_integer_key_parses_as_an_index_and_a_quoted_one_as_a_name() {
        let tag: SurfaceTag = serde_json::from_value(json!({ "surface": "a", "parts": [
            { "part": 1, "surface": "b" }, { "part": "1", "surface": "c" },
        ] }))
        .unwrap();
        assert_eq!(tag.parts[0].part, SurfacePart::Index(1));
        assert_eq!(tag.parts[1].part, SurfacePart::Name("1".into()));
        let bare = serde_json::to_value(SurfaceTag::default()).unwrap();
        assert!(bare.get("parts").is_none());
    }

    #[test]
    #[should_panic(expected = "default surface")]
    fn an_out_of_range_default_panics() {
        let _ = SurfaceTable::new(vec!["a".into()], Some(SurfaceId(1)));
    }
}
