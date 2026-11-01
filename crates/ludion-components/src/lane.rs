//! The [`LaneConstraint`] component.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::number::engine_number;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a lane does with a position past the end of an open path.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LaneEnds {
    /// The lane continues straight along the last (or first) horizontal direction.
    #[default]
    Extend,
    /// The character stops at the end of the path.
    Stop,
}

/// Holds a [`CharacterMotor`](ludion_physics::CharacterMotor) to a `kind: path` document: the
/// character's displacement is projected onto the path's horizontal direction and a pull keeps it
/// on the centre line, eased by `blend`. An entity needs a `CharacterMotor` for the lane to be read
/// (`L1302`).
///
/// `s` and `blend` are the lane's own state, written by `move_character`; a game writes `engaged`.
/// Changing `path` at runtime must also null `s`: use [`set_path`](Self::set_path).
///
/// # Example
/// ```
/// use ludion_components::{LaneConstraint, LaneEnds};
///
/// let lane: LaneConstraint = serde_json::from_value(serde_json::json!({
///     "path": "paths/corridor.path.yaml",
///     "engaged": true,
///     "ends": "stop",
/// }))
/// .unwrap();
/// assert!(lane.engaged);
/// assert_eq!(lane.ends, LaneEnds::Stop);
/// assert_eq!(lane.s, None);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct LaneConstraint {
    /// The `kind: path` document, relative to the content root (default `""` — inert). A raw write
    /// must also null `s`; [`set_path`](Self::set_path) does both.
    #[serde(default)]
    #[schemars(with = "ludion_ecs::AssetPathSchema")]
    pub path: String,
    /// Whether the lane holds the character; `blend` eases toward 1 when set and toward 0 when
    /// not (default `false`).
    #[serde(default)]
    pub engaged: bool,
    /// Metres of the lane's centre line to the right of the path, along its lateral (default `0.0`).
    #[serde(default)]
    pub offset: f32,
    /// Seconds of gameplay time for `blend` to rise from 0 to 1; `0` is instant (default `0.2`).
    #[serde(default = "default_engage_time")]
    pub engage_time: f32,
    /// Seconds of gameplay time for `blend` to fall from 1 to 0; `0` is instant (default `0.3`).
    #[serde(default = "default_disengage_time")]
    pub disengage_time: f32,
    /// Metres per second the pull toward the centre line moves the character at full weight
    /// (default `4.0`).
    #[serde(default = "default_pull_speed")]
    pub pull_speed: f32,
    /// Metres of arc length either side of `s` searched for the nearest path point (default `2.0`).
    #[serde(default = "default_search_window")]
    pub search_window: f32,
    /// What happens past the end of an open path (default `extend`).
    #[serde(default)]
    pub ends: LaneEnds,
    /// Metres along the path the character is at; `null` until the lane has found its place, then
    /// rewritten by `move_character`.
    #[serde(default)]
    pub s: Option<f32>,
    /// The eased weight, `0..=1`, written by `move_character` (default `0.0`).
    #[serde(default)]
    pub blend: f32,
}

fn default_engage_time() -> f32 {
    0.2
}

fn default_disengage_time() -> f32 {
    0.3
}

fn default_pull_speed() -> f32 {
    4.0
}

fn default_search_window() -> f32 {
    2.0
}

impl Default for LaneConstraint {
    fn default() -> Self {
        LaneConstraint {
            path: String::new(),
            engaged: false,
            offset: 0.0,
            engage_time: default_engage_time(),
            disengage_time: default_disengage_time(),
            pull_speed: default_pull_speed(),
            search_window: default_search_window(),
            ends: LaneEnds::Extend,
            s: None,
            blend: 0.0,
        }
    }
}

impl LaneConstraint {
    /// Points the lane at another path and forgets where it was on the old one, so the next move
    /// finds its place again (`s = None`).
    pub fn set_path(&mut self, path: impl Into<String>) {
        self.path = path.into();
        self.s = None;
    }
}

/// The first field of `lane` that breaks a range rule, with the rule, in declaration order. The
/// runtime check for a lane built in Rust (`L1300`); the document validator reports every field.
#[doc(hidden)]
pub fn invalid_field(lane: &LaneConstraint) -> Option<(&'static str, &'static str)> {
    const NOT_NEGATIVE: &str = "must be a finite number, not negative";
    const FINITE: &str = "must be a finite number";
    let non_negative =
        |field, value: f32| (!value.is_finite() || value < 0.0).then_some((field, NOT_NEGATIVE));
    let finite = |field, value: f32| (!value.is_finite()).then_some((field, FINITE));
    finite("offset", lane.offset)
        .or_else(|| non_negative("engage_time", lane.engage_time))
        .or_else(|| non_negative("disengage_time", lane.disengage_time))
        .or_else(|| non_negative("pull_speed", lane.pull_speed))
        .or_else(|| non_negative("search_window", lane.search_window))
        .or_else(|| lane.s.and_then(|s| finite("s", s)))
        .or_else(|| finite("blend", lane.blend))
        .or_else(|| {
            (!(0.0..=1.0).contains(&lane.blend)).then_some(("blend", "must be between 0 and 1"))
        })
}

impl SemanticValidate for LaneConstraint {
    /// Rejects an out-of-range field (`L1300`, pointer `/<field>`): `engage_time`,
    /// `disengage_time`, `pull_speed` or `search_window` negative or not finite, `offset` or `s`
    /// not finite, `blend` not finite or outside `[0, 1]`. A wrong *shape* is the schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let number = |field: &str| value.get(field)?.as_f64().map(engine_number);
        let mut found = Vec::new();
        let mut report = |field: &str, number: f64, rule: &str, help: &str| {
            found.push(
                Diagnostic::new(
                    code::LANE_CONSTRAINT_INVALID,
                    format!("`LaneConstraint` `{field}` {number} {rule}"),
                )
                .with_path(format!("/{field}"))
                .with_help(help.to_string()),
            );
        };
        for (field, unit) in [
            ("engage_time", "seconds"),
            ("disengage_time", "seconds"),
            ("pull_speed", "metres per second"),
            ("search_window", "metres"),
        ] {
            if let Some(n) = number(field).filter(|n| !n.is_finite() || *n < 0.0) {
                report(
                    field,
                    n,
                    "is not a finite, non-negative number",
                    &format!("give `{field}` a finite, non-negative number of {unit}"),
                );
            }
        }
        for (field, unit) in [("offset", "metres"), ("s", "metres along the path")] {
            if let Some(n) = number(field).filter(|n| !n.is_finite()) {
                report(
                    field,
                    n,
                    "is not a finite number",
                    &format!("give `{field}` a finite number of {unit}"),
                );
            }
        }
        if let Some(n) = number("blend").filter(|n| !n.is_finite() || !(0.0..=1.0).contains(n)) {
            report(
                "blend",
                n,
                "is not a number from 0 to 1",
                "give `blend` a number from 0 (free) to 1 (held to the lane)",
            );
        }
        found
    }
}
