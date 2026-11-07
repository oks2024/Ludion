//! The game-scheduled feed: entities carrying a [`SpatialVolume`] into the [`Index3d`] resource.

use std::any::TypeId;

use ludion_core::ids::EntityId;
use ludion_ecs::{Component, Parent, SystemCtx, World};

use super::{Handle, Index3d, SpatialVolume};
use crate::{ParentPose, WorldPosition};

/// Stages every entity carrying a [`SpatialVolume`] and a position component `P` into `index` as
/// `Handle::entity(id)`, centred on its world translation (the composed one under a `Parent`, via
/// `R`). Pushes only: the caller builds, so it can stage its own items (a pool's `Handle::slot`s)
/// between this call and [`Index3d::build`].
pub fn push_entity_volumes<P: Component + WorldPosition, R: ParentPose>(
    world: &mut World,
    index: &mut Index3d,
) {
    let parented = world
        .entity_indices_with(TypeId::of::<Parent>())
        .next()
        .is_some();
    if parented {
        // `query_ids` borrows the world mutably, so the world poses are read in a second pass.
        let mut filed: Vec<(EntityId, SpatialVolume, [f32; 3])> = Vec::new();
        for (entity, (volume, position)) in world.query_ids::<(&SpatialVolume, &P)>() {
            filed.push((entity, volume.clone(), position.world_position()));
        }
        for (entity, volume, center) in filed {
            let center = R::world_pose(world, entity).map_or(center, |(t, _)| t);
            index.push(volume.item(Handle::entity(entity), center));
        }
    } else {
        for (entity, (volume, position)) in world.query_ids::<(&SpatialVolume, &P)>() {
            index.push(volume.item(Handle::entity(entity), position.world_position()));
        }
    }
}

/// Rebuilds the [`Index3d`] resource from every entity carrying a [`SpatialVolume`] and a position
/// component `P`.
///
/// A game registers a concrete instantiation directly as a system —
/// `.system("spatial3d_rebuild", spatial3d::rebuild::<Transform, TransformHierarchy>)` — at a chosen
/// schedule position. **Schedule every reader after it**: a reader before it sees the previous
/// tick's index, and a snapshot restore does not carry the index, so such a reader would see an
/// empty one on the first tick after a restore. The resource is taken, cleared, refilled and
/// reinserted, so steady state allocates nothing. It is outside `WorldHash`.
pub fn rebuild<P: Component + WorldPosition, R: ParentPose>(world: &mut World, _ctx: &SystemCtx) {
    let mut index = world.remove_resource::<Index3d>().unwrap_or_default();
    index.clear();
    push_entity_volumes::<P, R>(world, &mut index);
    index.build();
    world.insert_resource(index);
}
