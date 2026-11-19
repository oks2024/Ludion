//! The [`PoseSnapshot`] component: the outgoing side of the crossfade that follows a ragdoll folding
//! back into its assembly (`Plan/paired-animations.md` decision 12, Neon Shroud request 0008).

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::FrameIndex;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One part's captured local pose: the sampled rotation and translation channels an assembly clip
/// would have produced for it.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct SnapshotPart {
    /// The assembly part's name.
    pub part: String,
    /// The rotation channel as a quaternion `[x, y, z, w]`.
    pub rotation: [f32; 4],
    /// The translation channel in model units (before the entity's `scale`).
    pub translation: [f32; 3],
}

/// The pose a voxel assembly's crossfade starts from, captured when a ragdoll folds back into it.
///
/// It is the outgoing side of the entity's fade exactly while `Animation.from` is empty,
/// `Animation.fade_started` equals `started`, and the fade is active; otherwise it is inert. The next
/// `crossfade()` (a non-empty `from`, a new `fade_started`) or a hard clip cut therefore retires it
/// with no system. `switch_ragdoll_to_assembly` writes it and `switch_to_ragdoll_with` removes it; a
/// stale, inert snapshot between those two calls is ordinary hashed state.
///
/// **Presentation only.** Extract reads it; no system does. A scene without it poses exactly as
/// before.
///
/// ```yaml
/// components:
///   PoseSnapshot:
///     started: 120
///     parts:
///       - { part: torso, rotation: [0.0, 0.0, 0.0, 1.0], translation: [0.0, 0.0, 0.0] }
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct PoseSnapshot {
    /// The `fade_started` frame of the crossfade this snapshot feeds (default frame `0`).
    #[serde(default = "frame_zero")]
    #[schemars(with = "u64")]
    pub started: FrameIndex,
    /// The captured parts, by assembly part name (default none: nothing to fade from).
    #[serde(default)]
    pub parts: Vec<SnapshotPart>,
}

impl Default for PoseSnapshot {
    fn default() -> Self {
        PoseSnapshot {
            started: FrameIndex::ZERO,
            parts: Vec::new(),
        }
    }
}

impl SemanticValidate for PoseSnapshot {
    /// A non-finite element, or a rotation with a squared length under 1e-12 (`L1751`,
    /// `/parts/<index>/<field>`).
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let Some(parts) = value.get("parts").and_then(Value::as_array) else {
            return out;
        };
        for (index, part) in parts.iter().enumerate() {
            for field in ["rotation", "translation"] {
                let Some(items) = part.get(field).and_then(Value::as_array) else {
                    continue;
                };
                let mut finite = true;
                for (axis, v) in items.iter().enumerate() {
                    if !v.as_f64().is_some_and(f64::is_finite) {
                        finite = false;
                        out.push(invalid(
                            index,
                            &format!("{field}/{axis}"),
                            &format!("`{field}` entry {axis} ({v}) must be a finite number"),
                        ));
                    }
                }
                if finite && field == "rotation" {
                    let len_sq: f64 = items.iter().filter_map(Value::as_f64).map(|v| v * v).sum();
                    if len_sq < 1e-12 {
                        out.push(invalid(
                            index,
                            field,
                            "`rotation` has a length near zero, so it is not a quaternion",
                        ));
                    }
                }
            }
        }
        out
    }
}

fn invalid(index: usize, field: &str, message: &str) -> Diagnostic {
    Diagnostic::new(
        code::POSE_SNAPSHOT_INVALID,
        format!("PoseSnapshot part {index}: {message}"),
    )
    .with_path(format!("/parts/{index}/{field}"))
    .with_help("give each part a finite non-zero `[x, y, z, w]` rotation and a finite translation")
}

fn frame_zero() -> FrameIndex {
    FrameIndex::ZERO
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn found(value: Value) -> Vec<(String, String)> {
        PoseSnapshot::validate_semantics(&value)
            .into_iter()
            .map(|d| (d.code.to_string(), d.path.unwrap_or_default()))
            .collect()
    }

    #[test]
    fn a_valid_snapshot_has_no_diagnostics() {
        assert!(found(json!({})).is_empty());
        assert!(
            found(json!({"parts": [
                {"part": "torso", "rotation": [0, 0, 0, 1], "translation": [0, 1, 0]}
            ]}))
            .is_empty()
        );
    }

    #[test]
    fn a_non_finite_element_and_a_zero_rotation_are_l1751_at_their_pointer() {
        let diagnostics = found(json!({"parts": [
            {"part": "a", "rotation": [0, 0, 0, 0], "translation": [0, 0, 0]},
            {"part": "b", "rotation": [0, 0, 0, 1], "translation": [0, "x", 0]},
            {"part": "c", "rotation": [0, null, 0, 1], "translation": [0, 0, 0]},
        ]}));
        for pointer in [
            "/parts/0/rotation",
            "/parts/1/translation/1",
            "/parts/2/rotation/1",
        ] {
            assert!(
                diagnostics.contains(&("L1751".to_string(), pointer.to_string())),
                "{pointer} in {diagnostics:?}"
            );
        }
    }

    #[test]
    fn the_default_round_trips_through_serde() {
        let snapshot = PoseSnapshot::default();
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(
            serde_json::from_value::<PoseSnapshot>(value).unwrap(),
            snapshot
        );
        assert_eq!(
            serde_json::from_value::<PoseSnapshot>(json!({})).unwrap(),
            snapshot
        );
    }
}
