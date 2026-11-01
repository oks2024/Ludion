//! [`ParentPose`]: the seam through which a hierarchy supplies composed world poses.

use ludion_core::ids::EntityId;
use ludion_ecs::World;

/// Resolves an entity's composed world pose when its stored pose is relative to an ancestor.
///
/// An associated function rather than a method: systems are bare `fn(&mut World, &SystemCtx)`
/// pointers, so a resolver cannot be captured and is a type parameter instead.
pub trait ParentPose {
    /// `entity`'s world `(translation, rotation quat [x, y, z, w])` when it has a live ancestor;
    /// `None` when the stored pose is already world space (every root).
    fn world_pose(world: &World, entity: EntityId) -> Option<([f32; 3], [f32; 4])>;
}

/// The resolver for a pose component with no hierarchy: every stored pose is world space.
pub struct NoParent;

impl ParentPose for NoParent {
    fn world_pose(_world: &World, _entity: EntityId) -> Option<([f32; 3], [f32; 4])> {
        None
    }
}
