//! [`IgnoreCollisions`]: the entities a motor's casts and fit tests look through.

use ludion_core::ids::EntityId;
use ludion_ecs::{SemanticValidate, World};

use crate::sweep::{MAX_PARTNERS, Partners};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Makes a motor's own queries ignore every collider `with` names: its sweeps, ground probes,
/// posture and resize tests and [`character_fit`](crate::character_fit). A paired action needs it,
/// because two capsules standing at a 0.5 m choke overlap; each participant of an N-role action
/// lists all the others.
///
/// It is read at query time and is not part of the physics blob. It does not stop the solver from
/// pushing a dynamic body that `with` names; it only changes what the motor sees. Without the
/// component, or with an empty list, every motor result is unchanged. A motor looks through at most
/// [`MAX_PARTNERS`] (4) entities: a longer list uses its first four at runtime and is `L1759` in a
/// document.
///
/// ```
/// use ludion_core::ids::EntityId;
/// use ludion_physics::IgnoreCollisions;
///
/// let partner = EntityId::new(7, 0);
/// let ignore = IgnoreCollisions { with: vec![partner] };
/// assert_eq!(ignore.clone(), ignore);
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IgnoreCollisions {
    /// The entities whose colliders the motor looks through.
    #[schemars(with = "ludion_ecs::EntityRefCollection<Vec<ludion_ecs::EntityRefSchema>>")]
    pub with: Vec<EntityId>,
}

impl SemanticValidate for IgnoreCollisions {
    /// `L1759` for a list longer than [`MAX_PARTNERS`].
    fn validate_semantics(value: &serde_json::Value) -> Vec<ludion_core::diagnostics::Diagnostic> {
        let count = value
            .get("with")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len);
        if count <= MAX_PARTNERS {
            return Vec::new();
        }
        vec![
            ludion_core::diagnostics::Diagnostic::new(
                ludion_core::codes::code::IGNORE_COLLISIONS_TOO_MANY,
                format!(
                    "`with` lists {count} entities; a motor looks through at most {MAX_PARTNERS}"
                ),
            )
            .with_path("/with")
            .with_help("list at most 4 entities; a paired action lists the other roles"),
        ]
    }
}

/// The partners `entity`'s motor ignores (the first [`MAX_PARTNERS`] of its [`IgnoreCollisions`]);
/// none without one. Never allocates.
pub(crate) fn motor_partners(world: &World, entity: EntityId) -> Partners {
    // Unrecorded: an optional component must not demand a game's query declaration.
    world
        .get_unrecorded::<IgnoreCollisions>(entity)
        .ok()
        .flatten()
        .map_or(Partners::NONE, |ignore| Partners::from_slice(&ignore.with))
}
