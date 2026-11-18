//! The [`Perceiver`] and [`SightTarget`] components: who looks, through which sight zones, and what
//! can be seen, from which sample points. `perception_update` in `ludion-engine` reads them.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::number::engine_number;
use ludion_ecs::SemanticValidate;
use ludion_physics::LayerSet;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Most sight zones a [`Perceiver`] holds, and most sample points a [`SightTarget`] holds.
pub const PERCEPTION_MAX_PARTS: usize = 8;

/// A facing offset from the entity's own rotation, in degrees. Yaw 0 looks along the entity's −Z,
/// **positive yaw turns left** and **positive pitch looks down**: the camera rig's convention.
///
/// ```
/// use ludion_components::Look;
///
/// let look: Look = serde_json::from_value(serde_json::json!({ "pitch_deg": 10.0 })).unwrap();
/// assert_eq!(look, Look { yaw_deg: 0.0, pitch_deg: 10.0 });
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
pub struct Look {
    /// Turn about +Y in degrees; positive turns left (default `0.0`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub yaw_deg: f32,
    /// Tilt in degrees; positive looks down (default `0.0`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pitch_deg: f32,
}

/// How a [`ZoneSweep`] moves the facing's yaw over gameplay time.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SweepShape {
    /// Constant-speed travel from `-amplitude_deg` to `+amplitude_deg`, a dwell, the way back, a dwell.
    #[default]
    PingPongLinear,
    /// `amplitude_deg * sin(2π t / period_s)`; `dwell_s` must be 0.
    Sine,
}

/// A closed-form yaw sweep of a [`Perceiver`]'s zones: a camera or searchlight panning to and fro.
/// It is a pure function of gameplay time (a held `GameClock` holds it).
///
/// ```
/// use ludion_components::{SweepShape, ZoneSweep};
///
/// let sweep: ZoneSweep = serde_json::from_value(serde_json::json!({
///     "amplitude_deg": 40.0, "period_s": 8.0, "dwell_s": 1.0,
/// }))
/// .unwrap();
/// assert_eq!(sweep.shape, SweepShape::PingPongLinear);
/// assert_eq!(sweep.phase_s, 0.0);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct ZoneSweep {
    /// Degrees either side of the look yaw (finite).
    pub amplitude_deg: f32,
    /// Seconds for one full cycle, above `2 * dwell_s`.
    pub period_s: f32,
    /// Seconds held at each end of a `ping_pong_linear` sweep (default `0.0`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub dwell_s: f32,
    /// Seconds added to gameplay time before the cycle position is taken (default `0.0`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub phase_s: f32,
    /// The motion (default `ping_pong_linear`).
    #[serde(default, skip_serializing_if = "is_default_shape")]
    pub shape: SweepShape,
}

/// One wedge of sight: a range, horizontal and vertical half-angles, and the classes it detects. A
/// point is in the zone when its horizontal and vertical angles are inside the half-angles and its
/// distance from the eye is within the effective range (a wedge, not a cone).
///
/// ```
/// use ludion_components::SightZone;
///
/// let zone: SightZone = serde_json::from_value(serde_json::json!({
///     "name": "inner", "range": 6.0, "half_angle_deg": 60.0,
///     "detects": ["player"], "see_through": ["cover_overhead"], "see_through_max_range": 3.0,
/// }))
/// .unwrap();
/// assert_eq!(zone.vertical_half_angle_deg, 45.0);
/// assert!(!zone.range_by_visibility);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct SightZone {
    /// The zone's name; unique within the perceiver (default `""`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Metres from the eye (finite and above 0).
    pub range: f32,
    /// Horizontal half-angle in degrees, in (0, 180].
    pub half_angle_deg: f32,
    /// Vertical half-angle in degrees, in (0, 90] (default `45.0`).
    #[serde(
        default = "default_vertical_half_angle_deg",
        skip_serializing_if = "is_default_vertical"
    )]
    pub vertical_half_angle_deg: f32,
    /// The `PerceptionConfig` classes the zone detects; empty means every class (default `[]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detects: Vec<String>,
    /// Layers a sample point's `blocked_by` loses while it is within `see_through_max_range` of the
    /// eye (default `[]`).
    #[serde(default, skip_serializing_if = "LayerSet::is_empty")]
    pub see_through: LayerSet,
    /// Metres from the eye within which `see_through` applies (default `0.0`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub see_through_max_range: f32,
    /// Scales `range` by the target's visibility (default `false`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub range_by_visibility: bool,
    /// Clamps the visibility that scales the range, `[lo, hi]` with `lo <= hi`; unset clamps only
    /// below 0 (default unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility_clamp: Option<[f32; 2]>,
}

/// An entity that sees: up to eight [`SightZone`]s from an eye point, a facing, and an optional
/// closed-form [`ZoneSweep`]. The entity's world rotation, then `look`, then the sweep's yaw orient
/// every zone. `perception_update` fills the `PerceptionResults` resource from it.
///
/// ```yaml
/// components:
///   Perceiver:
///     eye:
///       - 0.0
///       - 1.6
///       - 0.0
///     look:
///       pitch_deg: 5.0
///     sight:
///       - name: inner
///         range: 6.0
///         half_angle_deg: 60.0
///         detects:
///           - player
///       - name: outer
///         range: 14.0
///         half_angle_deg: 30.0
///         range_by_visibility: true
/// ```
///
/// ```
/// use ludion_components::Perceiver;
///
/// let guard: Perceiver = serde_json::from_value(serde_json::json!({
///     "eye": [0.0, 1.6, 0.0],
///     "sight": [{ "name": "inner", "range": 6.0, "half_angle_deg": 60.0 }],
/// }))
/// .unwrap();
/// assert!(guard.enabled);
/// assert_eq!(guard.sight.len(), 1);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Perceiver {
    /// Whether the perceiver sees; a disabled one produces no samples (default `true`).
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// The eye in the entity's local frame, metres (default `[0, 0, 0]`).
    #[serde(default, skip_serializing_if = "is_zero_vec3")]
    pub eye: [f32; 3],
    /// The facing the zones share, from the entity's rotation (default none).
    #[serde(default, skip_serializing_if = "is_default_look")]
    pub look: Look,
    /// The sight zones, one to eight.
    #[serde(default)]
    pub sight: Vec<SightZone>,
    /// A yaw sweep over gameplay time, applied to every zone (default none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sweep: Option<ZoneSweep>,
}

/// How many of a [`SightTarget`]'s sample points must be seen for the target to count as seen.
///
/// ```
/// use ludion_components::Require;
///
/// let rule: Require = serde_json::from_value(serde_json::json!({ "fraction": 0.5 })).unwrap();
/// assert_eq!(rule, Require::Fraction(0.5));
/// let all: Require = serde_json::from_value(serde_json::json!("all")).unwrap();
/// assert_eq!(all, Require::All);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Require {
    /// One visible point is enough.
    #[default]
    Any,
    /// Every point must be visible.
    All,
    /// At least `ceil(fraction * points)` points, with `fraction` in (0, 1].
    Fraction(f32),
}

/// One place on a target a ray is cast to, and the layers that hide it.
///
/// ```
/// use ludion_components::SightPoint;
///
/// let head: SightPoint = serde_json::from_value(serde_json::json!({
///     "offset": [0.0, 1.6, 0.0], "blocked_by": ["wall", "cover_high"],
/// }))
/// .unwrap();
/// assert_eq!(head.blocked_by.as_slice(), ["wall", "cover_high"]);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct SightPoint {
    /// The point in the target's local frame, metres.
    pub offset: [f32; 3],
    /// The layers that block the line of sight to this point; empty means nothing blocks it (a
    /// ray is not even cast; default `[]`).
    #[serde(default, skip_serializing_if = "LayerSet::is_empty")]
    pub blocked_by: LayerSet,
}

/// An entity a [`Perceiver`] can see: a `class` the zones detect, a `visibility` that scales
/// visibility-ranged zones, and one to eight sample [`SightPoint`]s with the rule that turns their
/// results into one answer.
///
/// ```yaml
/// components:
///   SightTarget:
///     class: player
///     visibility: 0.5
///     points:
///       - offset:
///           - 0.0
///           - 1.6
///           - 0.0
///         blocked_by:
///           - wall
///           - cover_high
///       - offset:
///           - 0.0
///           - 0.4
///           - 0.0
///         blocked_by:
///           - wall
///           - cover_low
///           - cover_high
///     require: any
/// ```
///
/// ```
/// use ludion_components::{Require, SightTarget};
///
/// let target: SightTarget = serde_json::from_value(serde_json::json!({
///     "class": "player",
///     "points": [{ "offset": [0.0, 1.0, 0.0] }],
/// }))
/// .unwrap();
/// assert_eq!(target.visibility, 1.0);
/// assert_eq!(target.require, Require::Any);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct SightTarget {
    /// A `PerceptionConfig` class name.
    pub class: String,
    /// Scales the range of `range_by_visibility` zones (default `1.0`; finite, not negative).
    #[serde(
        default = "default_visibility",
        skip_serializing_if = "is_default_visibility"
    )]
    pub visibility: f32,
    /// The sample points, one to eight.
    #[serde(default)]
    pub points: Vec<SightPoint>,
    /// How many points must be seen (default `any`).
    #[serde(default, skip_serializing_if = "is_default_require")]
    pub require: Require,
}

fn is_zero(value: &f32) -> bool {
    *value == 0.0
}
fn is_zero_vec3(value: &[f32; 3]) -> bool {
    *value == [0.0; 3]
}
fn is_false(value: &bool) -> bool {
    !*value
}
fn is_true(value: &bool) -> bool {
    *value
}
fn default_true() -> bool {
    true
}
fn is_default_shape(value: &SweepShape) -> bool {
    *value == SweepShape::default()
}
fn is_default_look(value: &Look) -> bool {
    *value == Look::default()
}
fn is_default_require(value: &Require) -> bool {
    *value == Require::default()
}
fn default_vertical_half_angle_deg() -> f32 {
    45.0
}
fn is_default_vertical(value: &f32) -> bool {
    *value == 45.0
}
fn default_visibility() -> f32 {
    1.0
}
fn is_default_visibility(value: &f32) -> bool {
    *value == 1.0
}

fn number(value: &Value) -> Option<f32> {
    value.as_f64().map(|n| engine_number(n) as f32)
}

/// A collector of diagnostics at one code, so each check is one line.
struct Found {
    code: ludion_core::diagnostics::DiagnosticCode,
    component: &'static str,
    out: Vec<Diagnostic>,
}

impl Found {
    fn new(code: ludion_core::diagnostics::DiagnosticCode, component: &'static str) -> Self {
        Self {
            code,
            component,
            out: Vec::new(),
        }
    }

    fn reject(&mut self, pointer: String, problem: String, help: &str) {
        self.out.push(
            Diagnostic::new(self.code, format!("`{}` {problem}", self.component))
                .with_path(pointer)
                .with_help(help.to_string()),
        );
    }

    /// Rejects `value[field]` when it is present, a number, and `fails` it.
    fn number_where(
        &mut self,
        value: &Value,
        base: &str,
        field: &str,
        fails: impl Fn(f32) -> bool,
        needs: &str,
        help: &str,
    ) {
        if let Some(n) = value.get(field).and_then(number)
            && fails(n)
        {
            self.reject(
                format!("{base}/{field}"),
                format!("`{field}` {n} {needs}"),
                help,
            );
        }
    }
}

impl SemanticValidate for Perceiver {
    /// Rejects what a schema cannot (`L1570`, pointer into the field): a zone count outside 1..=8,
    /// a `range` that is not finite and above 0, a `half_angle_deg` outside (0, 180], a
    /// `vertical_half_angle_deg` outside (0, 90], a repeated zone `name`, a negative or non-finite
    /// `see_through_max_range`, a `visibility_clamp` with `lo > hi`, a non-finite `eye` or `look`,
    /// and a sweep whose `period_s` does not exceed `2 * dwell_s`, whose sine carries a dwell, or
    /// whose numbers are not finite. A wrong *shape* is the schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut found = Found::new(code::PERCEIVER_INVALID, "Perceiver");
        let help = "see `ludion docs perception`; `ludion explain L1570`";
        if let Some(eye) = value.get("eye").and_then(Value::as_array) {
            for (i, c) in eye.iter().enumerate() {
                if number(c).is_some_and(|n| !n.is_finite()) {
                    found.reject(format!("/eye/{i}"), "`eye` must be finite".into(), help);
                }
            }
        }
        if let Some(look) = value.get("look") {
            for field in ["yaw_deg", "pitch_deg"] {
                found.number_where(
                    look,
                    "/look",
                    field,
                    |n| !n.is_finite(),
                    "is not finite",
                    help,
                );
            }
        }
        if let Some(zones) = value.get("sight").and_then(Value::as_array) {
            if zones.is_empty() || zones.len() > PERCEPTION_MAX_PARTS {
                found.reject(
                    "/sight".into(),
                    format!(
                        "`sight` holds {} zones; it needs 1 to {PERCEPTION_MAX_PARTS}",
                        zones.len()
                    ),
                    help,
                );
            }
            let mut names: Vec<&str> = Vec::new();
            for (i, zone) in zones.iter().enumerate() {
                let base = format!("/sight/{i}");
                if let Some(name) = zone.get("name").and_then(Value::as_str) {
                    if names.contains(&name) {
                        found.reject(
                            format!("{base}/name"),
                            format!("zone name `{name}` is repeated"),
                            help,
                        );
                    }
                    names.push(name);
                }
                found.number_where(
                    zone,
                    &base,
                    "range",
                    |n| !(n.is_finite() && n > 0.0),
                    "must be finite and above 0",
                    help,
                );
                found.number_where(
                    zone,
                    &base,
                    "half_angle_deg",
                    |n| !(n > 0.0 && n <= 180.0),
                    "must be in (0, 180]",
                    help,
                );
                found.number_where(
                    zone,
                    &base,
                    "vertical_half_angle_deg",
                    |n| !(n > 0.0 && n <= 90.0),
                    "must be in (0, 90]",
                    help,
                );
                found.number_where(
                    zone,
                    &base,
                    "see_through_max_range",
                    |n| !(n.is_finite() && n >= 0.0),
                    "must be finite and not negative",
                    help,
                );
                if let Some(clamp) = zone.get("visibility_clamp").and_then(Value::as_array) {
                    let ends: Vec<f32> = clamp.iter().filter_map(number).collect();
                    if let [lo, hi] = ends[..]
                        && (lo.is_nan() || hi.is_nan() || lo < 0.0 || lo > hi)
                    {
                        found.reject(
                            format!("{base}/visibility_clamp"),
                            format!("`visibility_clamp` [{lo}, {hi}] needs 0 <= lo <= hi"),
                            help,
                        );
                    }
                }
            }
        }
        if let Some(sweep) = value.get("sweep").filter(|s| s.is_object()) {
            for field in ["amplitude_deg", "phase_s"] {
                found.number_where(
                    sweep,
                    "/sweep",
                    field,
                    |n| !n.is_finite(),
                    "is not finite",
                    help,
                );
            }
            let period = sweep.get("period_s").and_then(number);
            let dwell = sweep.get("dwell_s").and_then(number).unwrap_or(0.0);
            if let Some(period) = period
                && !(period.is_finite() && dwell >= 0.0 && period > 2.0 * dwell)
            {
                found.reject(
                    "/sweep/period_s".into(),
                    format!("`period_s` {period} must be finite and exceed 2 * `dwell_s` ({dwell}), which is not negative"),
                    help,
                );
            }
            if sweep.get("shape").and_then(Value::as_str) == Some("sine") && dwell != 0.0 {
                found.reject(
                    "/sweep/dwell_s".into(),
                    format!("a `sine` sweep has no dwell, found {dwell}"),
                    help,
                );
            }
        }
        found.out
    }
}

impl SemanticValidate for SightTarget {
    /// Rejects what a schema cannot (`L1571`, pointer into the field): an empty `class`, a point
    /// count outside 1..=8, a non-finite `offset`, a negative or non-finite `visibility`, and a
    /// `require: { fraction }` outside (0, 1]. A wrong *shape* is the schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut found = Found::new(code::SIGHT_TARGET_INVALID, "SightTarget");
        let help = "see `ludion docs perception`; `ludion explain L1571`";
        if value.get("class").and_then(Value::as_str) == Some("") {
            found.reject("/class".into(), "`class` is empty".into(), help);
        }
        found.number_where(
            value,
            "",
            "visibility",
            |n| !(n.is_finite() && n >= 0.0),
            "must be finite and not negative",
            help,
        );
        if let Some(points) = value.get("points").and_then(Value::as_array) {
            if points.is_empty() || points.len() > PERCEPTION_MAX_PARTS {
                found.reject(
                    "/points".into(),
                    format!(
                        "`points` holds {} sample points; it needs 1 to {PERCEPTION_MAX_PARTS}",
                        points.len()
                    ),
                    help,
                );
            }
            for (i, point) in points.iter().enumerate() {
                let Some(offset) = point.get("offset").and_then(Value::as_array) else {
                    continue;
                };
                for (j, c) in offset.iter().enumerate() {
                    if number(c).is_some_and(|n| !n.is_finite()) {
                        found.reject(
                            format!("/points/{i}/offset/{j}"),
                            "`offset` must be finite".into(),
                            help,
                        );
                    }
                }
            }
        }
        if let Some(fraction) = value
            .get("require")
            .and_then(|r| r.get("fraction"))
            .and_then(number)
            && !(fraction > 0.0 && fraction <= 1.0)
        {
            found.reject(
                "/require/fraction".into(),
                format!("`fraction` {fraction} must be in (0, 1]"),
                help,
            );
        }
        found.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn zone() -> Value {
        json!({ "name": "inner", "range": 6.0, "half_angle_deg": 60.0 })
    }

    fn perceiver(extra: Value) -> Value {
        let mut base = json!({ "sight": [zone()] });
        for (k, v) in extra.as_object().unwrap() {
            base[k] = v.clone();
        }
        base
    }

    fn paths(found: Vec<Diagnostic>) -> Vec<String> {
        found
            .iter()
            .map(|d| d.path.clone().unwrap_or_default())
            .collect()
    }

    #[test]
    fn a_minimal_perceiver_is_clean_and_round_trips_without_defaults() {
        let value = perceiver(json!({}));
        assert!(Perceiver::validate_semantics(&value).is_empty());
        let parsed: Perceiver = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), value);
    }

    #[test]
    fn zone_counts_outside_one_to_eight_name_the_sight_pointer() {
        assert_eq!(
            paths(Perceiver::validate_semantics(&json!({ "sight": [] }))),
            ["/sight"]
        );
        let nine = json!({ "sight": vec![zone(); 9] });
        let found = Perceiver::validate_semantics(&nine);
        assert_eq!(found[0].path.as_deref(), Some("/sight"));
        assert_eq!(found[0].code, code::PERCEIVER_INVALID);
    }

    #[test]
    fn zone_fields_out_of_range_name_their_pointers() {
        let bad = json!({ "sight": [
            { "name": "a", "range": 0.0, "half_angle_deg": 60.0 },
            { "name": "b", "range": 5.0, "half_angle_deg": 181.0, "vertical_half_angle_deg": 91.0 },
            { "name": "b", "range": 5.0, "half_angle_deg": 0.0, "see_through_max_range": -1.0,
              "visibility_clamp": [2.0, 1.0] },
        ] });
        assert_eq!(
            paths(Perceiver::validate_semantics(&bad)),
            [
                "/sight/0/range",
                "/sight/1/half_angle_deg",
                "/sight/1/vertical_half_angle_deg",
                "/sight/2/name",
                "/sight/2/half_angle_deg",
                "/sight/2/see_through_max_range",
                "/sight/2/visibility_clamp",
            ]
        );
    }

    #[test]
    fn the_half_angle_and_vertical_edges_are_accepted() {
        let edge = json!({ "sight": [
            { "range": 1.0, "half_angle_deg": 180.0, "vertical_half_angle_deg": 90.0,
              "visibility_clamp": [0.0, 0.0] },
        ] });
        assert!(Perceiver::validate_semantics(&edge).is_empty());
    }

    #[test]
    fn sweep_rules() {
        let sweep = |s: Value| {
            paths(Perceiver::validate_semantics(&perceiver(
                json!({ "sweep": s }),
            )))
        };
        assert!(
            sweep(json!({ "amplitude_deg": 30.0, "period_s": 8.0, "dwell_s": 1.0 })).is_empty()
        );
        assert_eq!(
            sweep(json!({ "amplitude_deg": 30.0, "period_s": 2.0, "dwell_s": 1.0 })),
            ["/sweep/period_s"]
        );
        assert_eq!(
            sweep(
                json!({ "amplitude_deg": 30.0, "period_s": 8.0, "dwell_s": 1.0, "shape": "sine" })
            ),
            ["/sweep/dwell_s"]
        );
        assert_eq!(
            sweep(json!({ "amplitude_deg": 1e39, "period_s": 8.0 })),
            ["/sweep/amplitude_deg"]
        );
    }

    #[test]
    fn a_non_finite_eye_or_look_is_rejected() {
        let found = Perceiver::validate_semantics(&perceiver(
            json!({ "eye": [0.0, 1e39, 0.0], "look": { "yaw_deg": 1e39 } }),
        ));
        assert_eq!(paths(found), ["/eye/1", "/look/yaw_deg"]);
    }

    fn target(extra: Value) -> Value {
        let mut base = json!({ "class": "player", "points": [{ "offset": [0.0, 1.0, 0.0] }] });
        for (k, v) in extra.as_object().unwrap() {
            base[k] = v.clone();
        }
        base
    }

    #[test]
    fn a_minimal_target_is_clean_and_round_trips() {
        let value = target(json!({}));
        assert!(SightTarget::validate_semantics(&value).is_empty());
        let parsed: SightTarget = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), value);
    }

    #[test]
    fn target_fields_out_of_range_name_their_pointers() {
        let found = SightTarget::validate_semantics(&json!({
            "class": "", "visibility": -1.0,
            "points": [{ "offset": [0.0, 1e39, 0.0] }],
            "require": { "fraction": 1.5 },
        }));
        assert_eq!(found[0].code, code::SIGHT_TARGET_INVALID);
        assert_eq!(
            paths(found),
            [
                "/class",
                "/visibility",
                "/points/0/offset/1",
                "/require/fraction"
            ]
        );
        assert_eq!(
            paths(SightTarget::validate_semantics(
                &json!({ "class": "a", "points": [] })
            )),
            ["/points"]
        );
        let nine = target(json!({ "points": vec![json!({ "offset": [0, 0, 0] }); 9] }));
        assert_eq!(paths(SightTarget::validate_semantics(&nine)), ["/points"]);
    }

    #[test]
    fn fraction_edges() {
        let fraction = |f: f64| {
            SightTarget::validate_semantics(&target(json!({ "require": { "fraction": f } }))).len()
        };
        assert_eq!(fraction(1.0), 0);
        assert_eq!(fraction(0.0), 1);
        assert_eq!(fraction(0.001), 0);
    }

    #[test]
    fn require_reads_the_three_forms() {
        for (text, want) in [
            (json!("any"), Require::Any),
            (json!("all"), Require::All),
            (json!({ "fraction": 0.5 }), Require::Fraction(0.5)),
        ] {
            let parsed: Require = serde_json::from_value(text.clone()).unwrap();
            assert_eq!(parsed, want);
            assert_eq!(serde_json::to_value(parsed).unwrap(), text);
        }
    }
}
