//! The [`JointZero`] builtin component.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A stored zero for a [`Joint`](crate::Joint), carried by the joint's carrier entity beside it.
/// Without one, `ludion-physics` captures the joint's zero from the two bodies' relative pose at
/// build, so the zero is wherever the bodies happened to be. With one, the joint's limits,
/// [`JointMotor`](crate::JointMotor) `target` and [`JointPoses`](crate::JointPoses) are measured from
/// `rotation` instead, and the stored value survives a body rebuild, a ragdoll retire and
/// re-admission, a snapshot restore and a scene save and reload.
///
/// `rotation` is the quaternion `[x, y, z, w]` of the joint's body `b` in body `a`'s frame that
/// counts as zero, `q_a⁻¹·q_b`. The default is the identity. The anchors keep coinciding at build;
/// only the rotation comes from the zero. A body that starts outside the limits measured from the
/// zero is pulled back by the solver within the first step.
///
/// ```yaml
/// components:
///   JointZero:
///     rotation: [0.0, 0.0, 0.0, 1.0]
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct JointZero {
    /// The rotation of `b` relative to `a` that is the joint's zero, as `[x, y, z, w]`.
    #[serde(default = "identity")]
    pub rotation: [f32; 4],
}

fn identity() -> [f32; 4] {
    [0.0, 0.0, 0.0, 1.0]
}

impl Default for JointZero {
    fn default() -> Self {
        Self {
            rotation: identity(),
        }
    }
}

impl SemanticValidate for JointZero {
    /// Rejects a `rotation` with a non-finite element or a length under `1e-6` (`/rotation`), both
    /// `L1750`. JSON Schema reaches neither. Defensive: runs on a schema-clean value.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let Some(rotation) = value.get("rotation").and_then(Value::as_array) else {
            return Vec::new();
        };
        let mut length_sq = 0.0_f64;
        for element in rotation {
            let Some(element) = element.as_f64() else {
                continue;
            };
            if !element.is_finite() {
                return vec![zero_invalid(format!(
                    "joint zero `rotation` holds {element}; every element must be finite"
                ))];
            }
            length_sq += element * element;
        }
        if length_sq < 1e-12 {
            return vec![zero_invalid(
                "joint zero `rotation` has length under 1e-6; it must be a quaternion".to_string(),
            )];
        }
        Vec::new()
    }
}

/// `L1750` — an invalid `JointZero`.
fn zero_invalid(message: String) -> Diagnostic {
    Diagnostic::new(code::JOINT_ZERO_INVALID, message)
        .with_path("/rotation".to_string())
        .with_help("give `rotation` four finite numbers `[x, y, z, w]` of a non-zero quaternion")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_empty_value_is_the_identity() {
        let zero: JointZero = serde_json::from_value(json!({})).unwrap();
        assert_eq!(zero, JointZero::default());
        assert_eq!(zero.rotation, [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn round_trips() {
        let zero = JointZero {
            rotation: [0.0, 0.382_683_43, 0.0, 0.923_879_5],
        };
        let value = serde_json::to_value(zero).unwrap();
        assert_eq!(serde_json::from_value::<JointZero>(value).unwrap(), zero);
    }

    #[test]
    fn validation_table() {
        let cases = [
            (json!({}), 0),
            (json!({ "rotation": [0.0, 0.0, 0.0, 1.0] }), 0),
            (json!({ "rotation": [0.0, 1.0, 0.0, 1.0] }), 0),
            (json!({ "rotation": [0.0, 0.0, 0.0, 0.0] }), 1),
            (json!({ "rotation": [0.0, 0.0, 0.0, 1e-7] }), 1),
        ];
        for (value, expected) in cases {
            let out = JointZero::validate_semantics(&value);
            assert_eq!(out.len(), expected, "{value}");
            for d in out {
                assert_eq!(d.code.as_str(), "L1750");
                assert_eq!(d.path.as_deref(), Some("/rotation"));
            }
        }
    }
}
