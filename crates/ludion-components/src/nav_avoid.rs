//! [`NavAvoidance`]: the opt-in agent-vs-agent avoidance settings carried by `NavAgent.avoidance`.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::number::engine_number;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Most neighbours a [`NavAvoidance`] may consider.
pub const NAV_AVOIDANCE_MAX_NEIGHBORS: u32 = 16;

const DEFAULT_HORIZON_S: f32 = 1.5;
const DEFAULT_MAX_NEIGHBORS: u32 = 6;

/// Velocity-obstacle avoidance for a walking `NavAgent`. When present it replaces the agent's separation
/// push with an ORCA velocity computed from its `Index3d` neighbours (other walking agents), capped at the
/// agent's `speed`. Absent, the agent steers exactly as before. A flyer cannot have it.
///
/// `radius` is the agent's body radius for avoidance; absent, it is its nav class radius. `horizon_s` is how
/// far ahead, in seconds, a predicted collision is avoided. `max_neighbors` (1 to 16) bounds the neighbours
/// considered. Between two avoiding agents the one with the higher `priority` keeps its course and the other
/// takes all the avoidance; equal priorities share it. An agent without `avoidance` is dodged fully. An
/// invalid value is `L1528`.
///
/// ```yaml
/// components:
///   NavAgent:
///     avoidance:
///       horizon_s: 2
///       priority: 1
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct NavAvoidance {
    /// Avoidance radius, metres; absent is the class radius.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f32>,
    /// Look-ahead, seconds (default `1.5`).
    #[serde(
        default = "default_horizon_s",
        skip_serializing_if = "is_default_horizon_s"
    )]
    pub horizon_s: f32,
    /// Most neighbours considered, 1 to 16 (default `6`).
    #[serde(
        default = "default_max_neighbors",
        skip_serializing_if = "is_default_max_neighbors"
    )]
    pub max_neighbors: u32,
    /// The higher priority yields less (default `0`).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub priority: u32,
}

impl Default for NavAvoidance {
    fn default() -> Self {
        NavAvoidance {
            radius: None,
            horizon_s: DEFAULT_HORIZON_S,
            max_neighbors: DEFAULT_MAX_NEIGHBORS,
            priority: 0,
        }
    }
}

fn default_horizon_s() -> f32 {
    DEFAULT_HORIZON_S
}
fn default_max_neighbors() -> u32 {
    DEFAULT_MAX_NEIGHBORS
}
fn is_default_horizon_s(value: &f32) -> bool {
    value.to_bits() == DEFAULT_HORIZON_S.to_bits()
}
fn is_default_max_neighbors(value: &u32) -> bool {
    *value == DEFAULT_MAX_NEIGHBORS
}
fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

/// Validates a `NavAgent`'s `avoidance` member (`L1528`, pointers `/avoidance/radius`, `/avoidance/horizon_s`,
/// `/avoidance/max_neighbors`, `/avoidance` for a flyer). `agent` is the `NavAgent` JSON value.
pub(crate) fn validate_avoidance(agent: &Value) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let Some(avoidance) = agent.get("avoidance").filter(|v| !v.is_null()) else {
        return out;
    };
    let mut reject = |pointer: &str, what: String, help: &str| {
        out.push(
            Diagnostic::new(code::NAV_AVOIDANCE_INVALID, what)
                .with_path(pointer.to_string())
                .with_help(format!("{help}; `ludion explain L1528`")),
        );
    };
    for field in ["radius", "horizon_s"] {
        if let Some(n) = avoidance
            .get(field)
            .and_then(Value::as_f64)
            .map(engine_number)
            && !(n.is_finite() && n > 0.0)
        {
            reject(
                &format!("/avoidance/{field}"),
                format!("the `NavAgent` `avoidance.{field}` must be finite and above 0"),
                "write a positive number",
            );
        }
    }
    if let Some(n) = avoidance.get("max_neighbors").and_then(Value::as_u64)
        && !(1..=u64::from(NAV_AVOIDANCE_MAX_NEIGHBORS)).contains(&n)
    {
        reject(
            "/avoidance/max_neighbors",
            format!(
                "`NavAgent` `avoidance.max_neighbors` is {n}, outside 1 to {NAV_AVOIDANCE_MAX_NEIGHBORS}"
            ),
            "write a count from 1 to 16",
        );
    }
    if agent.get("flying").and_then(Value::as_bool) == Some(true) {
        reject(
            "/avoidance",
            "a flying `NavAgent` cannot have `avoidance`".into(),
            "avoidance steers walkers only; remove it or set `flying: false`",
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_map_is_the_default_and_round_trips() {
        let avoidance: NavAvoidance = serde_json::from_value(json!({})).unwrap();
        assert_eq!(avoidance, NavAvoidance::default());
        assert_eq!(serde_json::to_value(avoidance).unwrap(), json!({}));
        let custom = NavAvoidance {
            radius: Some(0.8),
            horizon_s: 2.0,
            max_neighbors: 8,
            priority: 1,
        };
        let back: NavAvoidance =
            serde_json::from_value(serde_json::to_value(custom).unwrap()).unwrap();
        assert_eq!(back, custom);
    }

    #[test]
    fn invalid_values_report_with_pointers() {
        let found = validate_avoidance(&json!({
            "flying": true,
            "avoidance": { "radius": 0.0, "horizon_s": -1.0, "max_neighbors": 17 },
        }));
        let paths: Vec<_> = found.iter().filter_map(|d| d.path.as_deref()).collect();
        assert_eq!(
            paths,
            [
                "/avoidance/radius",
                "/avoidance/horizon_s",
                "/avoidance/max_neighbors",
                "/avoidance"
            ]
        );
        assert!(found.iter().all(|d| d.code == code::NAV_AVOIDANCE_INVALID));
        assert!(validate_avoidance(&json!({ "avoidance": {} })).is_empty());
        assert_eq!(
            validate_avoidance(&json!({ "avoidance": { "max_neighbors": 0 } })).len(),
            1
        );
    }
}
