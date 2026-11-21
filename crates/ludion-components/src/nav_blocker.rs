//! The [`NavBlocker`] component: a box that overlays the navigation grids each `nav_update` without
//! re-sampling physics (a closed door, a crate pushed into a corridor, a mud patch).

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::number::engine_number;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const DEFAULT_HALF_EXTENT: f32 = 0.5;

/// The region a [`NavBlocker`] covers, in the entity's frame.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum NavBlockerShape {
    /// An oriented box centred on the entity, posed by its world transform.
    Box {
        /// Half the box's size on each axis, metres, before the entity's scale (default `[0.5; 3]`).
        half_extents: [f32; 3],
    },
}

impl Default for NavBlockerShape {
    fn default() -> Self {
        NavBlockerShape::Box {
            half_extents: [DEFAULT_HALF_EXTENT; 3],
        }
    }
}

/// What a [`NavBlocker`] does to the nodes it covers.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NavBlockerMode {
    /// Agents cannot enter the covered nodes (default).
    #[default]
    Block,
    /// Entering the covered nodes costs `cost` extra metres, once per crossing.
    Cost,
    /// The nodes stay passable, but a routed walker stops `at_blocker` before them until the game
    /// disables the blocker or switches it to `cost`.
    Interact,
}

/// A dynamic obstacle for the navigation grids. Every `nav_update` overlays the enabled blockers on each
/// class grid (without re-sampling physics): a node is covered when its position lies inside the posed box
/// inflated by the class radius and the agent's body overlaps the box vertically. `block` beats `interact`
/// beats `cost` where blockers overlap. A moving blocker is followed tick by tick.
///
/// `classes` lists the `NavConfig.classes` names the blocker applies to; empty is every class. A name no
/// class has is `L4147`. A routed walker stops `at_blocker` before an `interact` region and continues the
/// tick after the blocker is disabled or becomes `cost`; there is no done call. Flow fields treat `interact`
/// as blocked. Non-uniform scale is approximated per axis.
///
/// A non-finite or non-positive half-extent, a negative or non-finite `cost` and an empty `classes` entry
/// are rejected (`L1527`).
///
/// ```yaml
/// components:
///   NavBlocker:
///     shape:
///       box:
///         half_extents: [0.5, 1.0, 0.1]
///     mode: interact
///     classes: [walker]
/// ```
///
/// ```
/// use ludion_components::{NavBlocker, NavBlockerMode};
///
/// let b: NavBlocker = serde_json::from_value(serde_json::json!({ "mode": "cost", "cost": 3.0 })).unwrap();
/// assert_eq!(b.mode, NavBlockerMode::Cost);
/// assert!(b.enabled);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct NavBlocker {
    /// The covered region (default a `[0.5; 3]` box).
    #[serde(default, skip_serializing_if = "is_default_shape")]
    pub shape: NavBlockerShape,
    /// What the blocker does (default `block`).
    #[serde(default, skip_serializing_if = "is_default_mode")]
    pub mode: NavBlockerMode,
    /// Metres added on entering a covered node in `cost` mode (default `0`).
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub cost: f32,
    /// The agent classes the blocker applies to; empty is every class (default empty).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<String>,
    /// Whether the blocker is in effect (default `true`).
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

impl Default for NavBlocker {
    fn default() -> Self {
        NavBlocker {
            shape: NavBlockerShape::default(),
            mode: NavBlockerMode::Block,
            cost: 0.0,
            classes: Vec::new(),
            enabled: true,
        }
    }
}

fn default_true() -> bool {
    true
}
fn is_true(value: &bool) -> bool {
    *value
}
fn is_zero_f32(value: &f32) -> bool {
    value.to_bits() == 0
}
fn is_default_shape(value: &NavBlockerShape) -> bool {
    *value == NavBlockerShape::default()
}
fn is_default_mode(value: &NavBlockerMode) -> bool {
    *value == NavBlockerMode::Block
}

impl SemanticValidate for NavBlocker {
    /// Rejects a non-finite or non-positive half-extent, a negative or non-finite `cost` and an empty
    /// `classes` entry (`L1527`, pointers `/shape/box/half_extents/{i}`, `/cost`, `/classes/{i}`). A wrong
    /// *shape* is the schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut reject = |pointer: String, what: String, help: &str| {
            out.push(
                Diagnostic::new(code::NAV_BLOCKER_INVALID, what)
                    .with_path(pointer)
                    .with_help(format!("{help}; `ludion explain L1527`")),
            );
        };
        let extents = value
            .pointer("/shape/box/half_extents")
            .and_then(Value::as_array);
        for (i, v) in extents.into_iter().flatten().enumerate() {
            if let Some(n) = v.as_f64().map(engine_number)
                && !(n.is_finite() && n > 0.0)
            {
                reject(
                    format!("/shape/box/half_extents/{i}"),
                    format!("the `NavBlocker` half-extent {i} must be finite and above 0"),
                    "write three positive half-extents in metres",
                );
            }
        }
        if let Some(cost) = value.get("cost").and_then(Value::as_f64).map(engine_number)
            && !(cost.is_finite() && cost >= 0.0)
        {
            reject(
                "/cost".into(),
                "the `NavBlocker` `cost` must be finite and not negative".into(),
                "write 0 or a positive number of metres",
            );
        }
        let classes = value.get("classes").and_then(Value::as_array);
        for (i, _) in classes
            .into_iter()
            .flatten()
            .enumerate()
            .filter(|(_, v)| v.as_str() == Some(""))
        {
            reject(
                format!("/classes/{i}"),
                "a `NavBlocker` `classes` name is empty".into(),
                "name a `NavConfig.classes` entry, or remove the name to apply to every class",
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
        assert_eq!(
            serde_json::to_value(NavBlocker::default()).unwrap(),
            json!({})
        );
        let back: NavBlocker = serde_json::from_value(json!({})).unwrap();
        assert_eq!(back, NavBlocker::default());
        let blocker = NavBlocker {
            shape: NavBlockerShape::Box {
                half_extents: [1.0, 2.0, 0.25],
            },
            mode: NavBlockerMode::Interact,
            cost: 2.5,
            classes: vec!["walker".into()],
            enabled: false,
        };
        let value = serde_json::to_value(&blocker).unwrap();
        assert_eq!(
            value["shape"],
            json!({ "box": { "half_extents": [1.0, 2.0, 0.25] } })
        );
        let back: NavBlocker = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(back, blocker);
        assert!(NavBlocker::validate_semantics(&value).is_empty());
    }

    #[test]
    fn invalid_values_report_with_pointers() {
        let found = NavBlocker::validate_semantics(&json!({
            "shape": { "box": { "half_extents": [0.5, 0.0, 1e40] } },
            "cost": -1.0,
            "classes": ["walker", ""],
        }));
        let paths: Vec<_> = found.iter().filter_map(|d| d.path.as_deref()).collect();
        assert_eq!(
            paths,
            [
                "/shape/box/half_extents/1",
                "/shape/box/half_extents/2",
                "/cost",
                "/classes/1"
            ]
        );
        assert!(found.iter().all(|d| d.code == code::NAV_BLOCKER_INVALID));
    }
}
