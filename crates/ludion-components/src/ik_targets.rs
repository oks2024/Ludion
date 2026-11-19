//! The [`IkTargets`] component: where an assembly's `ik:` chains and look-ats aim
//! (`Plan/animation-layers.md` decisions 37–38, Neon Shroud request 0007).

use std::collections::BTreeMap;

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, pointer_token};
use ludion_core::time::{FrameIndex, GameTime, SimTime};
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::perception::Look;

/// One target of an [`IkTargets`]: exactly one of a world point or a direction, and a weight that
/// ramps.
///
/// The weight is `weight` once `fade` seconds have passed since `changed`; before that it lerps from
/// `from_weight`. Use [`fade_ik_target`] to change it without a jump.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct IkTarget {
    /// A world-space point to reach or face (default none). To follow an entity, write it from that
    /// entity's world position on each tick it moves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<[f32; 3]>,
    /// A direction in the model's frame, in `Look`'s convention: positive yaw turns left, positive
    /// pitch looks down (default none). A look-at only: a chain ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<Look>,
    /// The target weight in `[0, 1]` (default `0` — the target is off; `L1673` outside the range).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub weight: f32,
    /// The weight the ramp starts from at `changed` (default `0`; `L1673` outside `[0, 1]`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub from_weight: f32,
    /// The frame the weight ramp began at (default frame `0`).
    #[serde(default = "frame_zero", skip_serializing_if = "is_frame_zero")]
    #[schemars(with = "u64")]
    pub changed: FrameIndex,
    /// The ramp's length in seconds; `0` (default) jumps straight to `weight` (`L1673` when negative
    /// or not finite).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub fade: f32,
}

impl Default for IkTarget {
    fn default() -> Self {
        IkTarget {
            point: None,
            direction: None,
            weight: 0.0,
            from_weight: 0.0,
            changed: FrameIndex::ZERO,
            fade: 0.0,
        }
    }
}

/// Targets for a voxel assembly's `ik:` entries, keyed by the entry's name.
///
/// A chain reaches its target with its effector; a look-at turns its aim parts so `forward` faces
/// it. Each target sets exactly one of `point` or `direction` (`L1673`); `weight` blends
/// from the animated pose to the solved one and ramps over `fade` seconds from `changed`.
///
/// **Presentation only.** Extract solves the IK after the clip and its layers; no system reads
/// the component, and a scene without it poses exactly as before. It is ordinary hashed sim state,
/// so write it only when a choice changes (a `point` that follows a moving entity is rewritten
/// each tick it moves). An entry with no target, or at weight `0`, is not solved. An assembly without `ik:`
/// ignores the component.
///
/// ```yaml
/// components:
///   IkTargets:
///     targets:
///       look:
///         direction: { yaw_deg: 30.0 }
///         weight: 1.0
///       hand_r:
///         point: [2.0, 1.0, -3.0]
///         weight: 1.0
/// ```
///
/// ```
/// use ludion_components::{IkTarget, IkTargets, fade_ik_target, ik_target_weight};
/// use ludion_core::time::{SimTime, TickRate};
///
/// let mut ik = IkTargets::default();
/// ik.targets.insert("look".into(), IkTarget { point: Some([0.0, 1.0, -3.0]), ..Default::default() });
/// let now = SimTime::from_ticks(60, TickRate::DEFAULT);
/// assert!(fade_ik_target(&mut ik, "look", 1.0, 0.5, now));
/// // Half the fade later the target is half on.
/// let later = SimTime::from_ticks(75, TickRate::DEFAULT);
/// assert_eq!(ik_target_weight(&ik.targets["look"], later), 0.5);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
pub struct IkTargets {
    /// The targets by `ik:` entry name (default none).
    #[serde(default)]
    pub targets: BTreeMap<String, IkTarget>,
}

/// The target's effective weight at `now`: `weight` once the ramp is over (or has no `fade`), else a
/// lerp from `from_weight`.
#[must_use]
pub fn ik_target_weight(target: &IkTarget, now: SimTime) -> f32 {
    ik_target_weight_at(target, GameTime::from(now))
}

/// [`ik_target_weight`] at a fractional-tick gameplay instant: what `extract` evaluates.
#[must_use]
pub fn ik_target_weight_at(target: &IkTarget, now: GameTime) -> f32 {
    if target.fade <= 0.0 || !target.fade.is_finite() {
        return target.weight;
    }
    let elapsed = now.ticks_since(target.changed).max(0.0) * now.rate().seconds_per_tick();
    let t = (elapsed / f64::from(target.fade)).clamp(0.0, 1.0) as f32;
    target.from_weight + (target.weight - target.from_weight) * t
}

/// Ramps the target `name` to `weight` over `seconds`, starting from its current weight at `now`
/// (use the game time). Returns `false` when there is no such target; a target already at that
/// weight is left untouched (so a per-frame call does not churn the world hash).
///
/// ```
/// use ludion_components::{IkTarget, IkTargets, fade_ik_target};
/// use ludion_core::time::{SimTime, TickRate};
///
/// let mut ik = IkTargets::default();
/// ik.targets.insert("look".into(), IkTarget::default());
/// let now = SimTime::from_ticks(10, TickRate::DEFAULT);
/// assert!(fade_ik_target(&mut ik, "look", 1.0, 0.25, now));
/// assert!(!fade_ik_target(&mut ik, "missing", 1.0, 0.25, now));
/// assert_eq!(ik.targets["look"].weight, 1.0);
/// ```
pub fn fade_ik_target(
    ik: &mut IkTargets,
    name: &str,
    weight: f32,
    seconds: f32,
    now: SimTime,
) -> bool {
    let Some(target) = ik.targets.get_mut(name) else {
        return false;
    };
    #[allow(clippy::float_cmp)]
    if target.weight == weight {
        return true;
    }
    target.from_weight = ik_target_weight(target, now);
    target.weight = weight;
    target.fade = seconds;
    target.changed = FrameIndex(now.ticks());
    true
}

impl SemanticValidate for IkTargets {
    /// A target that sets none or both of `point` and `direction`, a weight outside
    /// `[0, 1]`, a negative or non-finite `fade`, or a non-numeric `point` entry (`L1673`,
    /// `/targets/<name>/<field>`).
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let Some(targets) = value.get("targets").and_then(Value::as_object) else {
            return out;
        };
        for (name, target) in targets {
            if name.is_empty() {
                out.push(invalid(name, "", "the target has an empty name"));
            }
            let set = ["point", "direction"]
                .iter()
                .filter(|field| target.get(**field).is_some_and(|v| !v.is_null()))
                .count();
            if set != 1 {
                out.push(invalid(
                    name,
                    "",
                    &format!("the target sets {set} of `point` and `direction`; set exactly one"),
                ));
            }
            for field in ["weight", "from_weight"] {
                if let Some(v) = target.get(field).and_then(Value::as_f64)
                    && !(0.0..=1.0).contains(&v)
                {
                    out.push(invalid(
                        name,
                        field,
                        &format!("`{field}` {v} is outside [0, 1]"),
                    ));
                }
            }
            if let Some(v) = target.get("fade").and_then(Value::as_f64)
                && (!v.is_finite() || v < 0.0)
            {
                out.push(invalid(
                    name,
                    "fade",
                    &format!("`fade` {v} must be finite and not negative"),
                ));
            }
            for field in ["point"] {
                if let Some(items) = target.get(field).and_then(Value::as_array) {
                    for (axis, v) in items.iter().enumerate() {
                        if !v.as_f64().is_some_and(f64::is_finite) {
                            out.push(invalid(
                                name,
                                &format!("{field}/{axis}"),
                                &format!("`{field}` entry {axis} ({v}) must be a finite number"),
                            ));
                        }
                    }
                }
            }
            if let Some(direction) = target.get("direction").filter(|v| v.is_object()) {
                for field in ["yaw_deg", "pitch_deg"] {
                    if let Some(v) = direction.get(field)
                        && !v.as_f64().is_some_and(f64::is_finite)
                    {
                        out.push(invalid(
                            name,
                            &format!("direction/{field}"),
                            &format!("`{field}` ({v}) must be a finite number"),
                        ));
                    }
                }
            }
        }
        out
    }
}

fn invalid(name: &str, field: &str, message: &str) -> Diagnostic {
    let pointer = if field.is_empty() {
        format!("/targets/{}", pointer_token(name))
    } else {
        format!("/targets/{}/{field}", pointer_token(name))
    };
    Diagnostic::new(
        code::IK_TARGET_INVALID,
        format!("IkTargets target `{name}`: {message}"),
    )
    .with_path(pointer)
    .with_help(
        "set exactly one of `point` or `direction`, weights in [0, 1], and a finite \
         non-negative fade",
    )
}

#[allow(clippy::float_cmp)]
fn is_zero(value: &f32) -> bool {
    *value == 0.0
}

fn frame_zero() -> FrameIndex {
    FrameIndex::ZERO
}

fn is_frame_zero(frame: &FrameIndex) -> bool {
    frame.as_u64() == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_core::time::TickRate;
    use serde_json::json;

    fn messages(value: Value) -> Vec<(String, String)> {
        IkTargets::validate_semantics(&value)
            .into_iter()
            .map(|d| (d.code.to_string(), d.path.unwrap_or_default()))
            .collect()
    }

    #[test]
    fn a_valid_target_has_no_diagnostics() {
        assert!(
            messages(json!({"targets": {"look": {"point": [0, 1, 2], "weight": 1.0}}})).is_empty()
        );
        assert!(messages(json!({"targets": {"look": {"direction": {"yaw_deg": 5.0}}}})).is_empty());
        assert!(messages(json!({})).is_empty());
    }

    #[test]
    fn every_invalid_field_is_l1673_at_its_pointer() {
        let found = messages(json!({"targets": {
            "none": {},
            "two": {"point": [0, 0, 0], "direction": {}},
            "w": {"point": [0, 0, 0], "weight": 1.5},
            "fw": {"point": [0, 0, 0], "from_weight": -0.5},
            "fade": {"point": [0, 0, 0], "fade": -1.0},
            "pt": {"point": [0, "x", 0]},
        }}));
        for pointer in [
            "/targets/none",
            "/targets/two",
            "/targets/w/weight",
            "/targets/fw/from_weight",
            "/targets/fade/fade",
            "/targets/pt/point/1",
        ] {
            assert!(
                found.contains(&("L1673".to_string(), pointer.to_string())),
                "{pointer} in {found:?}"
            );
        }
    }

    #[test]
    fn the_weight_ramps_over_the_fade() {
        let target = IkTarget {
            weight: 1.0,
            from_weight: 0.0,
            changed: FrameIndex(60),
            fade: 0.5,
            ..Default::default()
        };
        let at = |ticks| ik_target_weight(&target, SimTime::from_ticks(ticks, TickRate::DEFAULT));
        assert_eq!(at(60), 0.0);
        assert_eq!(at(75), 0.5);
        assert_eq!(at(200), 1.0);
    }

    #[test]
    fn fading_to_the_current_weight_writes_nothing() {
        let mut ik = IkTargets::default();
        ik.targets.insert(
            "look".into(),
            IkTarget {
                weight: 1.0,
                ..Default::default()
            },
        );
        let before = ik.clone();
        let now = SimTime::from_ticks(90, TickRate::DEFAULT);
        assert!(fade_ik_target(&mut ik, "look", 1.0, 0.5, now));
        assert_eq!(ik, before);
        assert!(fade_ik_target(&mut ik, "look", 0.0, 0.5, now));
        assert_ne!(ik, before);
        assert!(!fade_ik_target(&mut ik, "none", 0.0, 0.5, now));
    }

    #[test]
    fn the_default_round_trips_through_serde() {
        let ik = IkTargets::default();
        let value = serde_json::to_value(&ik).unwrap();
        assert_eq!(serde_json::from_value::<IkTargets>(value).unwrap(), ik);
    }
}
