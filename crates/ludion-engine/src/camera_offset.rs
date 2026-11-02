//! The presentation camera offset: which [`CameraOffset`] extract applies, and the latched
//! diagnostics for the ones it does not.
//!
//! The offset lives on a presentation entity, so it is never hashed and no sim system sees it.
//! Extract composes it onto the rig's pose in one place (`extract::compose_camera`), shared by the
//! drawn camera and the interpolation blend.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::{World, is_presentation};

use crate::components::{Camera, CameraOffset};

/// The offset extract applies and its entity: the lowest-id `CameraOffset` on a presentation
/// entity, or `None` when there is none, it is not finite, or it is the identity.
pub(crate) fn active_entity(world: &World, ids: &[EntityId]) -> Option<(EntityId, CameraOffset)> {
    if world.component_count::<CameraOffset>() == 0 {
        return None;
    }
    let (id, offset) = lowest(world, ids)?;
    (offset.is_finite() && !offset.is_identity()).then_some((id, offset))
}

/// [`active_entity`] without the entity.
pub(crate) fn active(world: &World, ids: &[EntityId]) -> Option<CameraOffset> {
    active_entity(world, ids).map(|(_, offset)| offset)
}

fn lowest(world: &World, ids: &[EntityId]) -> Option<(EntityId, CameraOffset)> {
    ids.iter().copied().find_map(|id| {
        if !is_presentation(id) {
            return None;
        }
        world
            .get::<CameraOffset>(id)
            .ok()
            .flatten()
            .map(|offset| (id, *offset))
    })
}

/// Latches the offsets extract does not apply: one on a sim entity (`L1333`), every presentation
/// offset after the lowest (`L1334`), a non-finite one (`L1332`) or a `fov_delta` that clamps (`L1335`). The
/// key names the entity, so each reports once.
pub(crate) fn report(world: &World, ids: &[EntityId], degraded: &mut Vec<(String, Diagnostic)>) {
    if world.component_count::<CameraOffset>() == 0 {
        return;
    }
    let camera = crate::extract::single_camera(world, ids)
        .ok()
        .flatten()
        .and_then(|id| world.get::<Camera>(id).ok().flatten());
    let mut seen_presentation = false;
    for &id in ids {
        if !world.has::<CameraOffset>(id) {
            continue;
        }
        let key = crate::diagnostics::offender_key(world, id);
        if !is_presentation(id) {
            degraded.push((
                format!("{key}/camera-offset-sim"),
                Diagnostic::new(
                    code::CAMERA_OFFSET_ON_SIM_ENTITY,
                    format!("{id} carries a `CameraOffset` on a sim entity; it is never applied"),
                )
                .with_help(
                    "put the `CameraOffset` on a presentation entity (`spawn_presentation`)",
                ),
            ));
        } else if seen_presentation {
            degraded.push((
                format!("{key}/camera-offset-duplicate"),
                Diagnostic::new(
                    code::CAMERA_OFFSET_DUPLICATE,
                    format!(
                        "{id} carries a second `CameraOffset`; only the lowest entity id is applied"
                    ),
                )
                .with_help("keep one offset entity and drive it from a presentation system"),
            ));
        } else {
            seen_presentation = true;
            if let Some(offset) = world.get::<CameraOffset>(id).ok().flatten() {
                report_applied(offset, &key, camera, degraded);
            }
        }
    }
}

fn report_applied(
    offset: &CameraOffset,
    key: &str,
    camera: Option<&Camera>,
    degraded: &mut Vec<(String, Diagnostic)>,
) {
    if !offset.is_finite() {
        degraded.push((
            format!("{key}/camera-offset-invalid"),
            Diagnostic::new(
                code::CAMERA_OFFSET_INVALID,
                "a `CameraOffset` field is not finite; the whole offset is ignored and the rig's \
                 pose is drawn",
            )
            .with_help("write finite numbers"),
        ));
    } else if camera.is_some_and(|camera| offset.clamps_fov(&camera.projection)) {
        degraded.push((
            format!("{key}/camera-offset-fov"),
            Diagnostic::new(
                code::CAMERA_OFFSET_FOV_CLAMPED,
                format!(
                    "`CameraOffset.fov_delta` {} takes the field of view outside {} to {} degrees; \
                     it is clamped",
                    offset.fov_delta,
                    CameraOffset::FOV_RANGE.0,
                    CameraOffset::FOV_RANGE.1
                ),
            )
            .with_help("keep `Camera.fov_y_deg + fov_delta` within 1 to 179"),
        ));
    }
}
