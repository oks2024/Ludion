//! The [`NavGoal`] and [`NavRoute`] components: a point-to-point goal for a [`NavAgent`](crate::NavAgent)
//! and the planned route `nav_steer` walks toward it.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::number::engine_number;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// When a routed agent plans a new route.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum NavReplan {
    /// Plan again when the goal moves to another cell or the next leg is no longer walkable.
    #[default]
    OnChange,
    /// Plan again only when the goal moves to another cell.
    Never,
}

/// Where a [`NavAgent`](crate::NavAgent) routes to. An entity with both walks a planned, string-pulled
/// route (see [`NavRoute`]) instead of reading a flow field; the goal then takes precedence over the
/// agent's `target`. The game inserts and removes it; removing it returns the agent to `target`.
///
/// `at` must be finite (`L1521`).
///
/// ```yaml
/// components:
///   NavAgent: {}
///   NavGoal:
///     at: [12.0, 0.0, -4.5]
/// ```
///
/// ```
/// use ludion_components::{NavGoal, NavReplan};
///
/// let goal: NavGoal = serde_json::from_value(serde_json::json!({ "at": [1.0, 0.0, 2.0] })).unwrap();
/// assert_eq!(goal.replan, NavReplan::OnChange);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
pub struct NavGoal {
    /// World position the agent routes to (default the origin).
    #[serde(default, skip_serializing_if = "is_zero_vec3")]
    pub at: [f32; 3],
    /// When to plan a new route (default `on_change`).
    #[serde(default, skip_serializing_if = "is_default_replan")]
    pub replan: NavReplan,
}

/// Where a [`NavRoute`] is in its life.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum NavRouteStatus {
    /// Not planned yet: the per-tick planning cap was reached; the agent stands still.
    #[default]
    Planning,
    /// Walking toward `waypoints[index]`.
    Following,
    /// Within `arrive_radius` of the last waypoint.
    Arrived,
    /// No route to the goal; the agent stands still until the goal changes cell.
    Unreachable,
}

/// The route `nav_steer` plans for a [`NavAgent`](crate::NavAgent) that carries a [`NavGoal`].
/// **Written by `nav_steer`**: a game reads it and never inserts one. It is hashed sim state, so a restored
/// world resumes the route it was on instead of planning again from elsewhere. `waypoints[0]` is the
/// node the route started from; `index` is the waypoint being walked to. A partial route (`partial`)
/// ends at the reachable point nearest the goal; `cost` is the path length along the grid links.
///
/// ```yaml
/// components:
///   NavRoute:
///     goal: [12.0, 0.0, -4.5]
///     waypoints: [[0.5, 0.0, 0.5], [6.5, 0.0, 3.5], [12.5, 0.0, -4.5]]
///     index: 1
///     status: following
///     cost: 14.2
/// ```
///
/// ```
/// use ludion_components::{NavRoute, NavRouteStatus};
///
/// let route: NavRoute = serde_json::from_value(serde_json::json!({})).unwrap();
/// assert_eq!(route.status, NavRouteStatus::Planning);
/// assert!(route.waypoints.is_empty());
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
pub struct NavRoute {
    /// The goal this route was planned for (default the origin).
    #[serde(default, skip_serializing_if = "is_zero_vec3")]
    pub goal: [f32; 3],
    /// The string-pulled route, start first (default none).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waypoints: Vec<[f32; 3]>,
    /// The waypoint being walked to (default `0`).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub index: u32,
    /// Where the route is in its life (default `planning`).
    #[serde(default, skip_serializing_if = "is_default_status")]
    pub status: NavRouteStatus,
    /// The route stops short of the goal (default `false`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub partial: bool,
    /// Path cost in metres along the grid links (default `0`).
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub cost: f32,
}

fn is_zero_vec3(value: &[f32; 3]) -> bool {
    value.iter().all(|c| c.to_bits() == 0)
}
fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}
fn is_zero_f32(value: &f32) -> bool {
    value.to_bits() == 0
}
fn is_false(value: &bool) -> bool {
    !*value
}
fn is_default_replan(value: &NavReplan) -> bool {
    *value == NavReplan::default()
}
fn is_default_status(value: &NavRouteStatus) -> bool {
    *value == NavRouteStatus::default()
}

fn non_finite(value: &Value) -> bool {
    value
        .as_f64()
        .map(engine_number)
        .is_some_and(|n| !n.is_finite())
}

impl SemanticValidate for NavGoal {
    /// Rejects a non-finite `at` component (`L1521`, pointer `/at/{i}`). A wrong *shape* is the
    /// schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let Some(items) = value.get("at").and_then(Value::as_array) else {
            return Vec::new();
        };
        items
            .iter()
            .enumerate()
            .filter(|(_, item)| non_finite(item))
            .map(|(i, _)| {
                Diagnostic::new(
                    code::NAV_GOAL_INVALID,
                    format!("the `NavGoal` `at` component {i} must be finite"),
                )
                .with_path(format!("/at/{i}"))
                .with_help("write three finite numbers; `ludion explain L1521`")
            })
            .collect()
    }
}

impl SemanticValidate for NavRoute {
    /// Rejects a non-finite `goal`, waypoint or `cost`, and an `index` past `waypoints` (`L1520`, pointers
    /// `/goal/{i}`, `/waypoints/{n}/{i}`, `/cost`, `/index`). A wrong *shape* is the schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut reject = |pointer: String, what: String| {
            out.push(
                Diagnostic::new(code::NAV_ROUTE_INVALID, what)
                    .with_path(pointer)
                    .with_help("delete the component and let `nav_steer` plan it again; `ludion explain L1520`"),
            );
        };
        if let Some(items) = value.get("goal").and_then(Value::as_array) {
            for (i, _) in items.iter().enumerate().filter(|(_, v)| non_finite(v)) {
                reject(
                    format!("/goal/{i}"),
                    format!("the `NavRoute` goal component {i} must be finite"),
                );
            }
        }
        let waypoints = value.get("waypoints").and_then(Value::as_array);
        for (n, point) in waypoints.into_iter().flatten().enumerate() {
            for (i, _) in point
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .filter(|(_, v)| non_finite(v))
            {
                reject(
                    format!("/waypoints/{n}/{i}"),
                    format!("the `NavRoute` waypoint {n} component {i} must be finite"),
                );
            }
        }
        if value.get("cost").is_some_and(non_finite) {
            reject("/cost".into(), "the `NavRoute` cost must be finite".into());
        }
        let len = waypoints.map_or(0, Vec::len) as u64;
        if let Some(index) = value.get("index").and_then(Value::as_u64)
            && index > len
        {
            reject(
                "/index".into(),
                format!("the `NavRoute` index {index} is past its {len} waypoints"),
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
        assert_eq!(serde_json::to_value(NavGoal::default()).unwrap(), json!({}));
        assert_eq!(
            serde_json::to_value(NavRoute::default()).unwrap(),
            json!({})
        );
        let route = NavRoute {
            goal: [1.0, 0.0, 2.0],
            waypoints: vec![[0.5, 0.0, 0.5], [1.5, 0.0, 2.5]],
            index: 1,
            status: NavRouteStatus::Following,
            partial: true,
            cost: 3.5,
        };
        let back: NavRoute = serde_json::from_value(serde_json::to_value(&route).unwrap()).unwrap();
        assert_eq!(back, route);
        let goal = NavGoal {
            at: [4.0, 0.0, -1.0],
            replan: NavReplan::Never,
        };
        let back: NavGoal = serde_json::from_value(serde_json::to_value(goal).unwrap()).unwrap();
        assert_eq!(back, goal);
        assert!(NavRoute::validate_semantics(&serde_json::to_value(&route).unwrap()).is_empty());
    }

    #[test]
    fn invalid_values_report_with_pointers() {
        let found = NavGoal::validate_semantics(&json!({ "at": [0.0, 1e40, 0.0] }));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path.as_deref(), Some("/at/1"));
        let found = NavRoute::validate_semantics(&json!({
            "goal": [1e40, 0.0, 0.0],
            "waypoints": [[0.0, 0.0, 0.0], [0.0, 0.0, 1e40]],
            "cost": 1e40,
            "index": 3,
        }));
        let paths: Vec<_> = found.iter().filter_map(|d| d.path.as_deref()).collect();
        assert_eq!(paths, ["/goal/0", "/waypoints/1/2", "/cost", "/index"]);
        assert!(
            NavRoute::validate_semantics(&json!({ "waypoints": [[0.0, 0.0, 0.0]], "index": 1 }))
                .is_empty()
        );
    }
}
