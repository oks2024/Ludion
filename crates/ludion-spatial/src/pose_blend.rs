//! The [`JointPoses`] and [`PoseBlend`] builtin components.

use crate::joint_motor::{check_non_negative, motor_invalid};
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// A jointed part's named drive poses, carried beside its [`Joint`](crate::Joint). The engine's
/// `pose_blend` system turns the pose its group's [`PoseBlend`] names into the part's
/// [`JointMotor`](crate::JointMotor): `target = pose · w`, `stiffness · w`, `damping · w`,
/// `max_force` unscaled (`w` is the blend `weight`). At `w == 0`, or when the part has no such pose,
/// the part carries no motor and goes limp.
///
/// A part's group root is `group` if set, else its `SkinBone.skin`; a part with neither is inert.
/// Each pose has the arity of the part's joint kind — one number for a hinge or slider, three for a
/// ball — and, like `JointMotor.target`, is measured from the joint's captured zero (the rig rest).
///
/// ```yaml
/// components:
///   JointPoses:
///     group: { index: 3, generation: 0 }
///     stiffness: 40.0
///     damping: 8.0
///     max_force: 200.0
///     poses:
///       curl: [60.0]
///       stand: [0.0]
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct JointPoses {
    /// The ragdoll group root whose [`PoseBlend`] drives this part (default unset: the part's
    /// `SkinBone.skin`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<ludion_ecs::EntityRefSchema>")]
    pub group: Option<EntityId>,
    /// The drive stiffness at weight 1 (see `JointMotor.stiffness`); zero or more.
    pub stiffness: f32,
    /// The drive damping at weight 1 (see `JointMotor.damping`); zero or more.
    pub damping: f32,
    /// The largest torque or force the drive may apply per axis; never scaled by the weight
    /// (default unset: uncapped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_force: Option<f32>,
    /// Named targets, in the units and arity of `JointMotor.target`, kept in sorted name order.
    pub poses: BTreeMap<String, Vec<f32>>,
}

/// The active pose and its weight for a ragdoll group, carried by the group's root entity. Every
/// [`JointPoses`] part of the group is driven toward `pose` at strength `weight`; a game writes
/// `weight` each tick to ramp the group between limp (`0`) and fully posed (`1`).
///
/// ```yaml
/// components:
///   PoseBlend:
///     pose: curl
///     weight: 0.5
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct PoseBlend {
    /// The name of the pose every part of the group is driven toward.
    pub pose: String,
    /// The blend strength, `0` (limp, no motors) to `1` (full pose).
    #[schemars(range(min = 0.0, max = 1.0))]
    pub weight: f32,
}

impl SemanticValidate for JointPoses {
    /// Rejects a pose that is not 1 or 3 elements (`/poses/<name>`) or holds a non-finite one
    /// (`/poses/<name>/<i>`), and a `stiffness`, `damping` or `max_force` that is negative or
    /// non-finite — all `L1188`. Defensive: runs on a schema-clean value.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();

        if let Some(poses) = value.get("poses").and_then(Value::as_object) {
            for (name, pose) in poses {
                let Some(pose) = pose.as_array() else {
                    continue;
                };
                if !matches!(pose.len(), 1 | 3) {
                    out.push(motor_invalid(
                        format!(
                            "pose `{name}` has {} elements; it must have 1 (hinge or slider) or 3 \
                             (ball)",
                            pose.len()
                        ),
                        &format!("/poses/{name}"),
                    ));
                }
                for (i, element) in pose.iter().enumerate() {
                    if let Some(element) = element.as_f64().filter(|e| !e.is_finite()) {
                        out.push(motor_invalid(
                            format!("pose `{name}` element {i} is {element}; it must be finite"),
                            &format!("/poses/{name}/{i}"),
                        ));
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trips_with_group_and_max_force() {
        let mut poses = BTreeMap::new();
        poses.insert("stand".to_string(), vec![0.0]);
        poses.insert("curl".to_string(), vec![60.0]);
        let part = JointPoses {
            group: Some(EntityId::new(3, 0)),
            stiffness: 40.0,
            damping: 8.0,
            max_force: Some(200.0),
            poses,
        };
        let value = serde_json::to_value(&part).unwrap();
        assert_eq!(value["max_force"], json!(200.0));
        assert_eq!(value["poses"], json!({ "curl": [60.0], "stand": [0.0] }));
        assert_eq!(serde_json::from_value::<JointPoses>(value).unwrap(), part);
    }

    #[test]
    fn group_and_max_force_are_absent_when_none() {
        let part = JointPoses {
            group: None,
            stiffness: 1.0,
            damping: 1.0,
            max_force: None,
            poses: BTreeMap::new(),
        };
        let value = serde_json::to_value(&part).unwrap();
        assert!(value.get("group").is_none(), "{value}");
        assert!(value.get("max_force").is_none(), "{value}");
        assert_eq!(serde_json::from_value::<JointPoses>(value).unwrap(), part);
    }

    #[test]
    fn pose_blend_round_trips() {
        let blend = PoseBlend {
            pose: "curl".to_string(),
            weight: 0.5,
        };
        let value = serde_json::to_value(&blend).unwrap();
        assert_eq!(value, json!({ "pose": "curl", "weight": 0.5 }));
        assert_eq!(serde_json::from_value::<PoseBlend>(value).unwrap(), blend);
    }

    #[test]
    fn wrong_arity_is_l1188_at_the_pose() {
        let bad = json!({
            "stiffness": 1.0, "damping": 1.0,
            "poses": { "ok": [1.0], "bad": [1.0, 2.0] }
        });
        let out = JointPoses::validate_semantics(&bad);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].code.as_str(), "L1188");
        assert_eq!(out[0].path.as_deref(), Some("/poses/bad"));
    }

    #[test]
    fn a_negative_field_is_l1188_at_that_field() {
        for field in ["stiffness", "damping", "max_force"] {
            let mut bad = json!({ "stiffness": 1.0, "damping": 1.0, "poses": { "a": [1.0] } });
            bad[field] = json!(-0.5);
            let out = JointPoses::validate_semantics(&bad);
            assert_eq!(out.len(), 1, "{field}");
            assert_eq!(out[0].code.as_str(), "L1188");
            let pointer = format!("/{field}");
            assert_eq!(out[0].path.as_deref(), Some(pointer.as_str()));
        }
    }

    #[test]
    fn valid_poses_are_accepted() {
        let ok = json!({
            "stiffness": 0.0, "damping": 0.0, "max_force": 1.0,
            "poses": { "a": [30.0], "b": [0.0, 10.0, -5.0] }
        });
        assert!(JointPoses::validate_semantics(&ok).is_empty());
    }
}
