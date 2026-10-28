//! The character motor on the engine's [`Transform`]: `ludion_physics::move_character` bound to
//! the pose component, which is what a game calls.

use ludion_core::ids::EntityId;
use ludion_ecs::World;
use ludion_physics::{MotorError, MoveReport, SlideConstraint};

use crate::components::Transform;

/// Moves `entity`'s feet by `displacement` through the physics world; see
/// [`ludion_physics::move_character`] for the algorithm and the call-ordering rule (call it from
/// a system registered before the physics systems).
///
/// ```no_run
/// # use ludion_core::ids::EntityId;
/// # use ludion_engine::move_character;
/// # use ludion_ecs::{SystemCtx, World};
/// fn walk(world: &mut World, _ctx: &SystemCtx, hero: EntityId) {
///     let _ = move_character(world, hero, [0.05, -0.2, 0.0]);
/// }
/// ```
pub fn move_character(
    world: &mut World,
    entity: EntityId,
    displacement: [f32; 3],
) -> Result<MoveReport, MotorError> {
    ludion_physics::move_character::<Transform>(world, entity, displacement)
}

/// [`move_character`] with a [`SlideConstraint`] reshaping the displacement before each cast.
pub fn move_character_with(
    world: &mut World,
    entity: EntityId,
    displacement: [f32; 3],
    constraint: Option<&mut dyn SlideConstraint>,
) -> Result<MoveReport, MotorError> {
    ludion_physics::move_character_with::<Transform>(world, entity, displacement, constraint)
}
