//! [`IgnoreCollisions`]: the one entity a motor's casts and fit tests look through.

use ludion_core::ids::EntityId;
use ludion_ecs::{SemanticValidate, World};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Makes a motor's own queries ignore `with`'s collider: its sweeps, ground probes, posture and
/// resize tests and [`character_fit`](crate::character_fit). A paired action needs it, because two
/// capsules standing at a 0.5 m choke overlap.
///
/// It is read at query time and is not part of the physics blob. It does not stop the solver from
/// pushing a dynamic body that `with` names; it only changes what the motor sees. Without the
/// component every motor result is unchanged.
///
/// ```
/// use ludion_core::ids::EntityId;
/// use ludion_physics::IgnoreCollisions;
///
/// let partner = EntityId::new(7, 0);
/// let ignore = IgnoreCollisions { with: partner };
/// assert_eq!(ignore.clone(), ignore);
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IgnoreCollisions {
    /// The entity whose collider the motor looks through.
    #[schemars(with = "ludion_ecs::EntityRefSchema")]
    pub with: EntityId,
}

impl SemanticValidate for IgnoreCollisions {
    /// No cross-field rules: one reference.
    fn validate_semantics(_value: &serde_json::Value) -> Vec<ludion_core::diagnostics::Diagnostic> {
        Vec::new()
    }
}

/// The partner `entity`'s motor ignores, `None` without an [`IgnoreCollisions`].
pub(crate) fn motor_partner(world: &World, entity: EntityId) -> Option<EntityId> {
    // Unrecorded: an optional component must not demand a game's query declaration.
    world
        .get_unrecorded::<IgnoreCollisions>(entity)
        .ok()
        .flatten()
        .map(|ignore| ignore.with)
}
