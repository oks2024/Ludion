//! The [`Velocity`] builtin component.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Linear and angular velocity, on all three axes.
///
/// A registered builtin component: it round-trips through the scene format, publishes a schema for
/// `check-scene`, and enters the world hash and snapshots like any other component. A game reads and
/// writes it directly — `ludion-physics` pushes it into the solver each tick and pulls the resolved
/// velocity back, so forces and impulses are authored through `Velocity`, not through a separate
/// force-accumulator API (`Plan/physics.md`).
///
/// ```yaml
/// components:
///   Velocity:
///     linear:
///       - 0.0
///       - 0.0
///       - 0.0
///     angular:
///       - 0.0
///       - 0.0
///       - 0.0
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
pub struct Velocity {
    /// Linear velocity, in world units per second on all three axes (default zero).
    #[serde(default)]
    pub linear: [f32; 3],
    /// Angular velocity, in radians per second on all three axes (default zero).
    #[serde(default)]
    pub angular: [f32; 3],
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trips() {
        let velocity = Velocity {
            linear: [1.0, 2.0, 3.0],
            angular: [0.0, 0.5, 0.0],
        };
        let json = serde_json::to_value(velocity).unwrap();
        assert_eq!(
            json,
            json!({ "linear": [1.0, 2.0, 3.0], "angular": [0.0, 0.5, 0.0] })
        );
        assert_eq!(serde_json::from_value::<Velocity>(json).unwrap(), velocity);
    }

    #[test]
    fn empty_object_is_the_drift_safe_default() {
        let velocity: Velocity = serde_json::from_value(json!({})).unwrap();
        assert_eq!(velocity, Velocity::default());
        assert_eq!(velocity.linear, [0.0, 0.0, 0.0]);
        assert_eq!(velocity.angular, [0.0, 0.0, 0.0]);
    }
}
