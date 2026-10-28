//! [`SurfaceVelocity`]: a belt's velocity, added to the carry of a motor standing on it.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::number::engine_number;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A conveyor belt's velocity, in metres per second in the **collider's local frame** (so it turns
/// with the entity). A [`CharacterMotor`](super::CharacterMotor) grounded on the entity is carried
/// by `velocity` times the gameplay `dt` each tick, on top of the platform's own motion, and
/// [`PlatformCarry::surface`](super::PlatformCarry::surface) reports it. Put it on a `static` or
/// `kinematic` collider: a rigid support is never carried (`L1299`).
///
/// ```yaml
/// components:
///   SurfaceVelocity:
///     velocity: [1.5, 0.0, 0.0]
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
#[serde(default)]
pub struct SurfaceVelocity {
    /// The belt's velocity in the collider's local frame, in metres per second (finite).
    pub velocity: [f32; 3],
}

impl SemanticValidate for SurfaceVelocity {
    /// Rejects a `velocity` component that is not a finite `f32` (`L1298`, pointer
    /// `/velocity/{i}`): a non-finite number, or one past `f32` range. A wrong *shape* is the
    /// schema's job and adds nothing here.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let Some(components) = value.get("velocity").and_then(Value::as_array) else {
            return Vec::new();
        };
        components
            .iter()
            .enumerate()
            .filter_map(|(index, component)| {
                let number = component.as_f64().map(engine_number)?;
                (!number.is_finite()).then(|| {
                    Diagnostic::new(
                        code::SURFACE_VELOCITY_INVALID,
                        format!(
                            "`SurfaceVelocity` `velocity[{index}]` {number} is not a finite number"
                        ),
                    )
                    .with_path(format!("/velocity/{index}"))
                    .with_help("give each component a finite number of metres per second")
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_default_validates_clean() {
        assert!(SurfaceVelocity::validate_semantics(&json!({})).is_empty());
        let value = serde_json::to_value(SurfaceVelocity::default()).unwrap();
        assert!(SurfaceVelocity::validate_semantics(&value).is_empty());
    }

    #[test]
    fn an_empty_document_is_the_default() {
        let belt: SurfaceVelocity = serde_json::from_value(json!({})).unwrap();
        assert_eq!(belt, SurfaceVelocity::default());
    }

    #[test]
    fn a_finite_velocity_is_clean() {
        assert!(
            SurfaceVelocity::validate_semantics(&json!({ "velocity": [1.5, 0.0, -2.0] }))
                .is_empty()
        );
    }

    #[test]
    fn an_overflowing_component_names_its_pointer() {
        let found = SurfaceVelocity::validate_semantics(&json!({ "velocity": [0.0, 1e39, 0.0] }));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::SURFACE_VELOCITY_INVALID);
        assert_eq!(found[0].path.as_deref(), Some("/velocity/1"));
    }
}
