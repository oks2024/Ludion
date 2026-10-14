//! The [`JointMotor`] builtin component.

use crate::JointKind;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A drive on a [`Joint`](crate::Joint)'s free axes, carried by the joint's carrier entity beside it.
/// `ludion-physics` pushes it onto the existing rapier joint every tick it changes — a change never
/// rebuilds the joint, so the joint's captured zero is kept and `target` is always measured from it.
///
/// `target` is one number for a `hinge` (degrees) or a `slider` (world units) and three for a `ball`
/// (x/y/z degrees), in the same axes and units as the joint's `limits`. The drive is a force-based
/// spring-damper toward `target`: `stiffness` pulls (N·m per radian of error on a hinge or ball, N/m
/// on a slider), `damping` resists velocity (N·m·s/rad, N·s/m), `max_force` optionally caps the real
/// torque (N·m) or force (N) per axis (default unset: uncapped). A motor with `stiffness` and `damping` both `0` is passive — the joint
/// behaves as if it had none. A motor on a `fixed` joint or with the wrong `target` arity for its
/// kind leaves the joint passive and reports `L3037`; a motor on a carrier with no `Joint` is inert.
///
/// ```yaml
/// components:
///   JointMotor:
///     target: [30.0]
///     stiffness: 40.0
///     damping: 8.0
///     max_force: 200.0
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct JointMotor {
    /// The drive target, from the joint's captured zero: `[t]` for a hinge (degrees) or slider
    /// (world units), `[x, y, z]` for a ball (degrees).
    pub target: Vec<f32>,
    /// How hard the drive pulls toward `target`: N·m/rad (hinge, ball) or N/m (slider); zero or more.
    pub stiffness: f32,
    /// How hard the drive resists relative velocity: N·m·s/rad or N·s/m; zero or more.
    pub damping: f32,
    /// The largest torque (N·m, hinge/ball) or force (N, slider) the drive may apply per axis (default
    /// unset: uncapped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_force: Option<f32>,
}

/// How many `target` numbers a joint of `kind` takes — hinge and slider one, ball three — or `None`
/// for a `fixed` joint, which has no free axis to drive.
pub fn expected_targets(kind: &JointKind) -> Option<usize> {
    match kind {
        JointKind::Hinge { .. } | JointKind::Slider { .. } => Some(1),
        JointKind::Ball { .. } => Some(3),
        JointKind::Fixed {} => None,
    }
}

impl SemanticValidate for JointMotor {
    /// Rejects a `target` that is not 1 or 3 elements (`/target`) or holds a non-finite one
    /// (`/target/<i>`), and a `stiffness`, `damping` or `max_force` that is negative or non-finite
    /// (`/stiffness`, `/damping`, `/max_force`) — all `L1188`. JSON Schema reaches none of these:
    /// `range(min)` does not reach array elements and the arity is a two-value set. Defensive: runs
    /// on a schema-clean value.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();

        if let Some(target) = value.get("target").and_then(Value::as_array) {
            if !matches!(target.len(), 1 | 3) {
                out.push(motor_invalid(
                    format!(
                        "motor `target` has {} elements; it must have 1 (hinge or slider) or 3 \
                         (ball)",
                        target.len()
                    ),
                    "/target",
                ));
            }
            for (i, element) in target.iter().enumerate() {
                if let Some(element) = element.as_f64() {
                    check_target_element(element, i, &mut out);
                }
            }
        }

        for field in ["stiffness", "damping", "max_force"] {
            if let Some(number) = value.get(field).and_then(Value::as_f64) {
                check_non_negative(field, number, &mut out);
            }
        }

        out
    }
}

/// Pushes an `L1188` when a `target` element is non-finite. Non-finite cannot reach here through a
/// scene or the wire (both reject non-finite floats upstream — `ludion-scene`'s `scalar.rs`), but
/// the guard is exact and cheap, so the solver never sees a bad value (the `Joint` `check_limit`
/// precedent).
fn check_target_element(value: f64, index: usize, out: &mut Vec<Diagnostic>) {
    if !value.is_finite() {
        out.push(motor_invalid(
            format!("motor `target` element {index} is {value}; it must be finite"),
            &format!("/target/{index}"),
        ));
    }
}

/// Pushes an `L1188` when `field` is negative or non-finite (the same upstream note as
/// [`check_target_element`]).
fn check_non_negative(field: &str, value: f64, out: &mut Vec<Diagnostic>) {
    if !value.is_finite() || value < 0.0 {
        out.push(motor_invalid(
            format!(
                "motor `{field}` {value} is not valid; it must be a finite number of 0 or more"
            ),
            &format!("/{field}"),
        ));
    }
}

/// `L1188` — an invalid `JointMotor`.
fn motor_invalid(message: String, pointer: &str) -> Diagnostic {
    Diagnostic::new(code::JOINT_MOTOR_INVALID, message)
        .with_path(pointer.to_string())
        .with_help(
            "give `target` one finite number for a hinge or slider or three for a ball, and \
             `stiffness`, `damping` and `max_force` (if set) finite values of 0 or more",
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trips_with_max_force() {
        let motor = JointMotor {
            target: vec![30.0],
            stiffness: 40.0,
            damping: 8.0,
            max_force: Some(200.0),
        };
        let value = serde_json::to_value(&motor).unwrap();
        assert_eq!(
            value,
            json!({ "target": [30.0], "stiffness": 40.0, "damping": 8.0, "max_force": 200.0 })
        );
        assert_eq!(serde_json::from_value::<JointMotor>(value).unwrap(), motor);
    }

    #[test]
    fn max_force_is_absent_when_none() {
        let motor = JointMotor {
            target: vec![0.0, 0.0, 30.0],
            stiffness: 5.0,
            damping: 1.0,
            max_force: None,
        };
        let value = serde_json::to_value(&motor).unwrap();
        assert!(value.get("max_force").is_none(), "{value}");
        assert_eq!(serde_json::from_value::<JointMotor>(value).unwrap(), motor);
    }

    #[test]
    fn wrong_arity_is_l1188_at_target() {
        for target in [json!([]), json!([1.0, 2.0])] {
            let bad = json!({ "target": target, "stiffness": 1.0, "damping": 1.0 });
            let out = JointMotor::validate_semantics(&bad);
            assert_eq!(out.len(), 1, "{bad}");
            assert_eq!(out[0].code.as_str(), "L1188");
            assert_eq!(out[0].path.as_deref(), Some("/target"));
        }
    }

    /// A non-finite element cannot arrive through JSON (no JSON value can carry `NaN`/`inf`), so
    /// this unit-tests the guard directly, the `Joint` `check_limit` precedent.
    #[test]
    fn a_non_finite_target_element_is_l1188_at_its_index() {
        let mut out = Vec::new();
        check_target_element(f64::NAN, 1, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code.as_str(), "L1188");
        assert_eq!(out[0].path.as_deref(), Some("/target/1"));
    }

    #[test]
    fn a_negative_field_is_l1188_at_that_field() {
        for field in ["stiffness", "damping", "max_force"] {
            let mut bad = json!({ "target": [1.0], "stiffness": 1.0, "damping": 1.0 });
            bad[field] = json!(-0.5);
            let out = JointMotor::validate_semantics(&bad);
            assert_eq!(out.len(), 1, "{field}");
            assert_eq!(out[0].code.as_str(), "L1188");
            let pointer = format!("/{field}");
            assert_eq!(out[0].path.as_deref(), Some(pointer.as_str()));
        }
    }

    #[test]
    fn a_non_finite_field_is_l1188() {
        for field in ["stiffness", "damping", "max_force"] {
            let mut out = Vec::new();
            check_non_negative(field, f64::INFINITY, &mut out);
            assert_eq!(out.len(), 1, "{field}");
            assert_eq!(out[0].code.as_str(), "L1188");
        }
    }

    #[test]
    fn a_valid_hinge_and_ball_motor_are_accepted() {
        let cases = [
            json!({ "target": [30.0], "stiffness": 40.0, "damping": 8.0 }),
            json!({ "target": [0.0, 10.0, -5.0], "stiffness": 0.0, "damping": 0.0, "max_force": 1.0 }),
        ];
        for case in cases {
            let out = JointMotor::validate_semantics(&case);
            assert!(out.is_empty(), "{case:?} -> {out:?}");
        }
    }

    #[test]
    fn expected_targets_follows_the_joint_kind() {
        let hinge = JointKind::Hinge {
            axis: [0.0, 1.0, 0.0],
            limits: None,
        };
        let slider = JointKind::Slider {
            axis: [0.0, 1.0, 0.0],
            limits: None,
        };
        assert_eq!(expected_targets(&hinge), Some(1));
        assert_eq!(expected_targets(&slider), Some(1));
        assert_eq!(expected_targets(&JointKind::Ball { limits: None }), Some(3));
        assert_eq!(expected_targets(&JointKind::Fixed {}), None);
    }
}
