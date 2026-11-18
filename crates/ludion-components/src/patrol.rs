//! The [`PatrolFollower`] and [`PatrolState`] components: a guard that walks a `kind: patrol`
//! document as nav legs, and the state `patrol_update` keeps for it.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::number::engine_number;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where a [`PatrolFollower`] rejoins its route when `paused` turns off.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PatrolResume {
    /// The stop nearest the guard (horizontal distance; ties to the lowest index).
    #[default]
    Nearest,
    /// The stop after the one it was heading to.
    Next,
    /// The stop it was heading to when it was paused.
    Last,
}

/// An entity that patrols a `kind: patrol` document: `patrol_update` writes its `NavGoal` for each
/// stop in turn, so every leg is a nav path around obstacles. The entity also needs a `NavAgent`.
/// `PatrolState` is the state `patrol_update` keeps; the game reads the facing from it (or from
/// `patrol_motion`) and turns the body itself.
///
/// While `paused` is true the patrol leaves the guard alone (the game or a behaviour drives it); when
/// it turns off the guard rejoins by `resume`. `route` must be a relative path without `..` (`L1524`).
///
/// ```yaml
/// components:
///   NavAgent: {}
///   PatrolFollower:
///     route: patrols/yard.patrol.yaml
///     resume: nearest
/// ```
///
/// ```
/// use ludion_components::{PatrolFollower, PatrolResume};
///
/// let guard: PatrolFollower =
///     serde_json::from_value(serde_json::json!({ "route": "patrols/yard.patrol.yaml" })).unwrap();
/// assert_eq!(guard.resume, PatrolResume::Nearest);
/// assert!(!guard.paused);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
pub struct PatrolFollower {
    /// The `kind: patrol` document, relative to the content root (default `""` — inert).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    #[schemars(with = "ludion_ecs::AssetPathSchema")]
    pub route: String,
    /// The stop to head for first, clamped to the last stop (default `0`).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub start: u32,
    /// Hands the guard to the game: `patrol_update` neither steers nor counts (default `false`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub paused: bool,
    /// Where the guard rejoins the route when `paused` turns off (default `nearest`).
    #[serde(default, skip_serializing_if = "is_default_resume")]
    pub resume: PatrolResume,
}

/// Where a [`PatrolState`] is in a stop.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PatrolPhase {
    /// Walking the leg toward `point`.
    #[default]
    Walking,
    /// Standing at `point`, counting its wait and look sequence.
    Stopped,
    /// `PatrolFollower.paused` is on; the game has the guard.
    Paused,
    /// A `once` patrol reached its last stop and finished.
    Done,
}

/// What `patrol_update` keeps for a [`PatrolFollower`]. **Written by `patrol_update`**: a game reads it
/// and never inserts one. It is hashed sim state, so a restored world resumes the stop it was in.
/// `point` is the stop being walked to or stood at; `look_deg` is the world yaw the guard should face
/// (`None` while walking); `arrived` is true only on the tick a stop began. Timers count gameplay time.
///
/// ```yaml
/// components:
///   PatrolState:
///     point: 2
///     phase: stopped
///     stop_left: 0.75
///     stop_elapsed: 0.25
///     look_deg: 45.0
/// ```
///
/// ```
/// use ludion_components::{PatrolPhase, PatrolState};
///
/// let state: PatrolState = serde_json::from_value(serde_json::json!({})).unwrap();
/// assert_eq!(state.phase, PatrolPhase::Walking);
/// assert!(state.forward);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct PatrolState {
    /// The stop being walked to or stood at (default `0`).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub point: u32,
    /// A `ping_pong` patrol is heading toward higher indices (default `true`).
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub forward: bool,
    /// Where the guard is in the stop (default `walking`).
    #[serde(default, skip_serializing_if = "is_default_phase")]
    pub phase: PatrolPhase,
    /// Gameplay seconds the current stop still lasts (default `0`).
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub stop_left: f32,
    /// Gameplay seconds since the current stop began (default `0`).
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub stop_elapsed: f32,
    /// The world yaw, in degrees, to face while stopped; absent while walking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look_deg: Option<f32>,
    /// True only on the tick a stop began (default `false`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub arrived: bool,
}

impl Default for PatrolState {
    fn default() -> Self {
        Self {
            point: 0,
            forward: true,
            phase: PatrolPhase::Walking,
            stop_left: 0.0,
            stop_elapsed: 0.0,
            look_deg: None,
            arrived: false,
        }
    }
}

fn default_true() -> bool {
    true
}
fn is_true(value: &bool) -> bool {
    *value
}
fn is_false(value: &bool) -> bool {
    !*value
}
fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}
fn is_zero_f32(value: &f32) -> bool {
    value.to_bits() == 0
}
fn is_default_resume(value: &PatrolResume) -> bool {
    *value == PatrolResume::default()
}
fn is_default_phase(value: &PatrolPhase) -> bool {
    *value == PatrolPhase::default()
}

fn number(value: &Value) -> Option<f64> {
    value.as_f64().map(engine_number)
}

impl SemanticValidate for PatrolFollower {
    /// Rejects an absolute or `..`-climbing `route` (`L1524`, pointer `/route`). A wrong *shape* is
    /// the schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let Some(route) = value.get("route").and_then(Value::as_str) else {
            return Vec::new();
        };
        let problem = if route.starts_with(['/', '\\']) || route.as_bytes().get(1) == Some(&b':') {
            "is an absolute path"
        } else if route.split(['/', '\\']).any(|part| part == "..") {
            "climbs out of the content root with `..`"
        } else {
            return Vec::new();
        };
        vec![
            Diagnostic::new(
                code::PATROL_FOLLOWER_INVALID,
                format!("the `PatrolFollower` `route` {problem}"),
            )
            .with_path("/route")
            .with_help("name a content-root-relative file such as `patrols/yard.patrol.yaml`; `ludion explain L1524`"),
        ]
    }
}

impl SemanticValidate for PatrolState {
    /// Rejects a negative or non-finite `stop_left` / `stop_elapsed` and a non-finite `look_deg`
    /// (`L1525`, pointers `/stop_left`, `/stop_elapsed`, `/look_deg`). A wrong *shape* is the
    /// schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut reject = |pointer: &str, what: String| {
            out.push(
                Diagnostic::new(code::PATROL_STATE_INVALID, what)
                    .with_path(pointer.to_string())
                    .with_help("delete the component and let `patrol_update` start again; `ludion explain L1525`"),
            );
        };
        for field in ["stop_left", "stop_elapsed"] {
            if let Some(n) = value.get(field).and_then(number)
                && (!n.is_finite() || n < 0.0)
            {
                reject(
                    &format!("/{field}"),
                    format!(
                        "the `PatrolState` `{field}` must be finite and not negative, found {n}"
                    ),
                );
            }
        }
        if let Some(n) = value.get("look_deg").and_then(number)
            && !n.is_finite()
        {
            reject(
                "/look_deg",
                "the `PatrolState` `look_deg` must be finite".into(),
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
            serde_json::to_value(PatrolFollower::default()).unwrap(),
            json!({})
        );
        assert_eq!(
            serde_json::to_value(PatrolState::default()).unwrap(),
            json!({})
        );
        let follower = PatrolFollower {
            route: "patrols/yard.patrol.yaml".into(),
            start: 2,
            paused: true,
            resume: PatrolResume::Last,
        };
        let back: PatrolFollower =
            serde_json::from_value(serde_json::to_value(&follower).unwrap()).unwrap();
        assert_eq!(back, follower);
        let state = PatrolState {
            point: 3,
            forward: false,
            phase: PatrolPhase::Stopped,
            stop_left: 0.5,
            stop_elapsed: 1.25,
            look_deg: Some(-45.0),
            arrived: true,
        };
        let value = serde_json::to_value(state).unwrap();
        assert_eq!(value["forward"], json!(false));
        let back: PatrolState = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(back, state);
        assert!(PatrolState::validate_semantics(&value).is_empty());
    }

    #[test]
    fn a_bad_route_reports_l1524() {
        for route in [
            "/abs/x.patrol.yaml",
            "C:/x.patrol.yaml",
            "a/../../x.patrol.yaml",
        ] {
            let found = PatrolFollower::validate_semantics(&json!({ "route": route }));
            assert_eq!(found.len(), 1, "{route}");
            assert_eq!(found[0].path.as_deref(), Some("/route"));
        }
        assert!(
            PatrolFollower::validate_semantics(&json!({ "route": "patrols/a.patrol.yaml" }))
                .is_empty()
        );
    }

    #[test]
    fn a_bad_state_reports_l1525_with_pointers() {
        let found = PatrolState::validate_semantics(&json!({
            "stop_left": -1.0,
            "stop_elapsed": 1e40,
            "look_deg": 1e40,
        }));
        let paths: Vec<_> = found.iter().filter_map(|d| d.path.as_deref()).collect();
        assert_eq!(paths, ["/stop_left", "/stop_elapsed", "/look_deg"]);
    }
}
