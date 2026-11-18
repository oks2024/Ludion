//! Camera zones and the occluder query, the engine half of [`ludion_camera::CameraZone`].
//!
//! Each tick, before [`step_rig`](ludion_camera::step_rig), [`apply`] finds the zones whose sensor
//! contains the rig target's origin and hands the winner to [`resolve_zone`]. Membership is
//! recomputed from the physics world every tick, not tracked from sensor events, so a respawn, a
//! teleport, a restore and a load are correct by construction. A rig that is in no zone, in a world with no
//! `CameraZone`, runs no query and touches no state. Zones are skipped, reporting `L1553` once, in a world with no
//! `PhysicsWorld`; two top-priority zones on the target report `L1555` once per pair and the lower
//! entity id wins.
//!
//! [`camera_occluders`] is a pure read for a game's fade system.

use ludion_camera::{CameraRig, CameraZone, RigConfig, invalid_config, resolve_zone, rig_focus};
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_ecs::World;
use ludion_physics::{LayerSet, PhysicsError, PhysicsWorld, QueryFilter};
use ludion_spatial::Shape;

use crate::debug_draw::debug_draw;

/// Radius of the probe sphere at the target's origin, in metres.
const ZONE_PROBE: f32 = 0.01;
/// The active zone's cross and its line to the target (linear RGBA).
const ZONE: [f32; 4] = [0.9, 0.5, 1.0, 1.0];
const CROSS: f32 = 0.2;

/// Resolves the zone the target at `target_position` stands in onto `rig` (a clone of the stored
/// rig); the caller writes it back. Returns at once when no entity carries a [`CameraZone`] and
/// the rig is in none (a despawned active zone still falls back).
pub(crate) fn apply(
    world: &mut World,
    camera: EntityId,
    rig: &mut CameraRig,
    target: EntityId,
    target_position: [f32; 3],
) {
    if rig.state.zone.is_none() && world.query_ids::<(&CameraZone,)>().next().is_none() {
        return;
    }
    let Some(physics) = world.resource::<PhysicsWorld>() else {
        let key = crate::diagnostics::offender_key(world, camera);
        crate::diagnostics::report(
            world,
            code::CAMERA_ZONE_NO_PHYSICS,
            key,
            "the world has `CameraZone` entities but no `PhysicsWorld`, so no zone can contain the \
             camera target; zones are skipped: add physics to the game",
        );
        return;
    };
    let filter = QueryFilter::new().include_sensors().exclude(target);
    let probe = Shape::Sphere { radius: ZONE_PROBE };
    let hits = physics
        .overlap(&probe, target_position, [0.0, 0.0, 0.0, 1.0], &filter)
        .unwrap_or_default();
    let mut candidates: Vec<(EntityId, i32, RigConfig)> = hits
        .into_iter()
        .filter_map(|id| match world.get::<CameraZone>(id) {
            Ok(Some(zone)) if invalid_config(&zone.rig).is_none() => {
                Some((id, zone.priority, zone.rig.clone()))
            }
            _ => None,
        })
        .collect();
    candidates.sort_by_key(|&(id, priority, _)| (std::cmp::Reverse(priority), id));
    candidates.dedup_by_key(|c| c.0);
    if let [(first, p0, _), (second, p1, _), ..] = candidates.as_slice()
        && p0 == p1
    {
        let key = format!("{first}+{second}");
        crate::diagnostics::report(
            world,
            code::CAMERA_ZONE_EQUAL_PRIORITY,
            key,
            format!(
                "the camera target stands in {first} and {second}, two `CameraZone`s of priority \
                 {p0}; {first} wins: give them distinct priorities"
            ),
        );
    }
    let active = candidates.first().map(|(id, _, rig)| (*id, rig));
    resolve_zone(rig, active);
    if let Some((id, _)) = active
        && let Some(at) = crate::hierarchy::world_transform(world, id).map(|p| p.trs.translation)
    {
        debug_draw(world, "camera")
            .cross(at, CROSS, ZONE)
            .line(at, target_position, ZONE);
    }
}

/// The entities on `layers` between the scene camera and the rig's target, sorted by [`EntityId`]
/// and deduplicated: a sphere of `radius` swept from the rig's focus to the posed eye, skipping the
/// target. A game's fade system calls it to decide what to hide.
///
/// It reads the last posed camera and the last physics step, so it is one tick behind, like
/// [`ludion_camera::camera_relative`]. It returns `Ok(vec![])` with no rig, pose, target or
/// physics world, and `Err` when physics rejects the cast (an undeclared layer name, a bad radius).
///
/// ```
/// use ludion_ecs::World;
/// use ludion_engine::camera_occluders;
/// use ludion_physics::LayerSet;
///
/// let mut world = World::new();
/// let camera = world.spawn();
/// let props = LayerSet::default();
/// assert_eq!(camera_occluders(&world, camera, &props, 0.2).unwrap(), vec![]);
/// ```
pub fn camera_occluders(
    world: &World,
    camera: EntityId,
    layers: &LayerSet,
    radius: f32,
) -> Result<Vec<EntityId>, PhysicsError> {
    let Ok(Some(rig)) = world.get::<CameraRig>(camera) else {
        return Ok(Vec::new());
    };
    let (Some(aim), Some(target)) = (rig.aim(), rig.target) else {
        return Ok(Vec::new());
    };
    let (Some(physics), Some(target_pos)) = (
        world.resource::<PhysicsWorld>(),
        crate::hierarchy::world_transform(world, target).map(|pose| pose.trs.translation),
    ) else {
        return Ok(Vec::new());
    };
    let focus = rig_focus(
        &rig.rig,
        target_pos,
        rig.state.anchor_y,
        rig.state.look_ahead,
    );
    let delta = [
        aim.eye[0] - focus[0],
        aim.eye[1] - focus[1],
        aim.eye[2] - focus[2],
    ];
    let length = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
    if length <= f32::EPSILON {
        return Ok(Vec::new());
    }
    let filter = QueryFilter::new().include_set(layers).exclude(target);
    let hits = physics.shape_cast(
        &Shape::Sphere { radius },
        focus,
        [0.0, 0.0, 0.0, 1.0],
        delta,
        length,
        &filter,
    )?;
    let mut entities: Vec<EntityId> = hits.into_iter().map(|hit| hit.entity).collect();
    entities.sort_unstable();
    entities.dedup();
    Ok(entities)
}
