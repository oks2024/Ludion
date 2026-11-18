//! The [`NavLink`] component: a directed edge between two points of the navigation grid that routed
//! agents plan across (a ladder, a vault, a jump, a door).

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::number::engine_number;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Metres an end may be from a walkable node and still snap to it.
const DEFAULT_SNAP_RADIUS: f32 = 0.75;

/// An off-mesh link: `nav_path` and routed agents (`NavGoal` → `NavRoute`) plan across it as an edge from
/// `from` to `to` (both ways when `bidirectional`). A walker that reaches the start stops with
/// `NavRoute.status = at_link`; the game plays the traversal, moves the body, and calls `nav_link_done`.
/// The engine never moves the body. `nav_update` rebuilds the link table every call: a disabled or
/// removed link drops out and the routes using it plan again.
///
/// `from` and `to` are in the entity's frame: posed by its `Transform` when it has one, world coordinates
/// otherwise. Each snaps to the nearest walkable node within `snap_radius` (an end that finds none warns
/// `L4145` and the link is left out). `cost` is metres **added** to the 3D distance between the two
/// snapped nodes, so a slow climb can be made to lose to a short walk. `kind` is free text for the game
/// (`"ladder"`, `"vault"`) and is not read by the engine.
///
/// Non-finite endpoints, a negative or non-finite `cost` and a non-positive or non-finite `snap_radius`
/// are rejected (`L1526`).
///
/// ```yaml
/// components:
///   NavLink:
///     from: [4.5, 0.0, 2.5]
///     to: [4.5, 3.0, 2.5]
///     kind: ladder
///     cost: 2.0
/// ```
///
/// ```
/// use ludion_components::NavLink;
///
/// let link: NavLink = serde_json::from_value(serde_json::json!({ "kind": "ladder" })).unwrap();
/// assert!(link.bidirectional && link.enabled);
/// assert_eq!(link.snap_radius, 0.75);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct NavLink {
    /// The link's start, in the entity's frame (default the origin).
    #[serde(default, skip_serializing_if = "is_zero_vec3")]
    pub from: [f32; 3],
    /// The link's end, in the entity's frame (default the origin).
    #[serde(default, skip_serializing_if = "is_zero_vec3")]
    pub to: [f32; 3],
    /// Whether the link can also be crossed from `to` to `from` (default `true`).
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub bidirectional: bool,
    /// What the traversal is, for the game: free text, never read by the engine (default empty).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// Metres added to the distance between the two snapped nodes (default `0`).
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub cost: f32,
    /// Whether routes may use the link (default `true`).
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// Metres an end may be from a walkable node and still snap to it (default `0.75`).
    #[serde(
        default = "default_snap_radius",
        skip_serializing_if = "is_default_snap_radius"
    )]
    pub snap_radius: f32,
}

impl Default for NavLink {
    fn default() -> Self {
        NavLink {
            from: [0.0; 3],
            to: [0.0; 3],
            bidirectional: true,
            kind: String::new(),
            cost: 0.0,
            enabled: true,
            snap_radius: DEFAULT_SNAP_RADIUS,
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_snap_radius() -> f32 {
    DEFAULT_SNAP_RADIUS
}
fn is_true(value: &bool) -> bool {
    *value
}
fn is_zero_vec3(value: &[f32; 3]) -> bool {
    value.iter().all(|c| c.to_bits() == 0)
}
fn is_zero_f32(value: &f32) -> bool {
    value.to_bits() == 0
}
fn is_default_snap_radius(value: &f32) -> bool {
    value.to_bits() == DEFAULT_SNAP_RADIUS.to_bits()
}

fn non_finite(value: &Value) -> bool {
    value
        .as_f64()
        .map(engine_number)
        .is_some_and(|n| !n.is_finite())
}

impl SemanticValidate for NavLink {
    /// Rejects a non-finite `from` or `to` component, a negative or non-finite `cost` and a non-positive or
    /// non-finite `snap_radius` (`L1526`, pointers `/from/{i}`, `/to/{i}`, `/cost`, `/snap_radius`). A
    /// wrong *shape* is the schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut reject = |pointer: String, what: String, help: &str| {
            out.push(
                Diagnostic::new(code::NAV_LINK_INVALID, what)
                    .with_path(pointer)
                    .with_help(format!("{help}; `ludion explain L1526`")),
            );
        };
        for field in ["from", "to"] {
            let items = value.get(field).and_then(Value::as_array);
            for (i, _) in items
                .into_iter()
                .flatten()
                .enumerate()
                .filter(|(_, v)| non_finite(v))
            {
                reject(
                    format!("/{field}/{i}"),
                    format!("the `NavLink` `{field}` component {i} must be finite"),
                    "write three finite numbers",
                );
            }
        }
        if let Some(cost) = value.get("cost").and_then(Value::as_f64).map(engine_number)
            && !(cost.is_finite() && cost >= 0.0)
        {
            reject(
                "/cost".into(),
                "the `NavLink` `cost` must be finite and not negative".into(),
                "write 0 or a positive number of metres",
            );
        }
        if let Some(radius) = value
            .get("snap_radius")
            .and_then(Value::as_f64)
            .map(engine_number)
            && !(radius.is_finite() && radius > 0.0)
        {
            reject(
                "/snap_radius".into(),
                "the `NavLink` `snap_radius` must be finite and above 0".into(),
                "write a positive number of metres",
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_serialize_to_an_empty_map_and_round_trip() {
        assert_eq!(serde_json::to_value(NavLink::default()).unwrap(), json!({}));
        let back: NavLink = serde_json::from_value(json!({})).unwrap();
        assert_eq!(back, NavLink::default());
        let link = NavLink {
            from: [1.0, 0.0, 2.0],
            to: [1.0, 3.0, 2.0],
            bidirectional: false,
            kind: "ladder".into(),
            cost: 2.5,
            enabled: false,
            snap_radius: 1.0,
        };
        let back: NavLink = serde_json::from_value(serde_json::to_value(&link).unwrap()).unwrap();
        assert_eq!(back, link);
        assert!(NavLink::validate_semantics(&serde_json::to_value(&link).unwrap()).is_empty());
    }

    #[test]
    fn invalid_values_report_with_pointers() {
        let found = NavLink::validate_semantics(&json!({
            "from": [0.0, 1e40, 0.0],
            "to": [1e40, 0.0, 0.0],
            "cost": -1.0,
            "snap_radius": 0.0,
        }));
        let paths: Vec<_> = found.iter().filter_map(|d| d.path.as_deref()).collect();
        assert_eq!(paths, ["/from/1", "/to/0", "/cost", "/snap_radius"]);
        assert!(found.iter().all(|d| d.code == code::NAV_LINK_INVALID));
    }
}
