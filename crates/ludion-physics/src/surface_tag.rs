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
/// ```yaml
/// components:
///   SurfaceTag:
///     surface: metal_grate
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct SurfaceTag {
    /// The declared surface name.
    pub surface: String,
}

impl SemanticValidate for SurfaceTag {
    /// Rejects an empty name (`L1691`, pointer `/surface`). A wrong *shape* is the schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        match value.get("surface").and_then(Value::as_str) {
            Some("") => vec![
                Diagnostic::new(code::SURFACES_INVALID, "`SurfaceTag` `surface` is empty")
                    .with_path("/surface")
                    .with_help("name a surface `physics.surfaces` declares"),
            ],
            _ => Vec::new(),
        }
    }
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
    #[should_panic(expected = "default surface")]
    fn an_out_of_range_default_panics() {
        let _ = SurfaceTable::new(vec!["a".into()], Some(SurfaceId(1)));
    }
}
