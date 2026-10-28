//! The `camera_rig` system: advances the scene's [`CameraRig`] each tick and writes the result to
//! the camera entity's `Transform` (and the perspective `fov_y_deg`).
//!
//! The system installs only when the world holds a rig at build, restore or scene switch, so a
//! rig-less world keeps its schedule, describe output and hashes. It runs after the physics triple
//! and before the presentation systems, so the camera reads this tick's settled target pose.
//!
//! The pipeline is [`ludion_camera::step_rig`], run on the gameplay clock: a held clock freezes the
//! springs, blend and shake and re-poses from the frozen state. A rig whose state has no
//! `last_applied` snaps on its first tick, so a loaded or switched-to scene starts on its pose while
//! a restore, which carries the state, resumes bit-exact.
//!
//! A rig holds its last pose (and reports once) when it is on an entity without a `Camera`
//! (`L1307`) or with a `Parent` (`L1309`), carries an out-of-range field (`L1304`), names a
//! despawned target (`L1305`) or a track path that fails to load (`L1306`). With several rigs only
//! the lowest entity id runs (`L1308`). A `target` of `None` or an empty `path` is inert, not a fault.

use ludion_camera::{CameraRig, RigInput, RigMode, invalid_field, step_rig};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::{Parent, Schedule, SystemCtx, World};
use serde_json::Value;

use crate::components::{Camera, Projection, Transform};
use crate::path_follow::PathLibrary;

pub(crate) const SYSTEM_NAME: &str = "camera_rig";

/// Whether `world` holds at least one [`CameraRig`] — the key the engine installs [`camera_rig`] on.
pub(crate) fn present(world: &World) -> bool {
    world.entities().any(|id| world.has::<CameraRig>(id))
}

/// Adds [`camera_rig`] to `schedule` when the world it is built for holds a rig. Called after the
/// physics systems and before the presentation systems.
pub(crate) fn install(schedule: &mut Schedule, present: bool) {
    if present {
        schedule.add_system(SYSTEM_NAME, camera_rig);
    }
}

/// The system's `engine.describe` entry at `order`, with no call site to attribute.
pub(crate) fn describe_entry(order: usize) -> Value {
    serde_json::json!({
        "name": SYSTEM_NAME,
        "order": order,
        "fn_path": "ludion_engine::camera_rig",
        "registered_at": Value::Null,
        "queries": [{
            "components": [
                {"name": "CameraRig", "access": "write"},
                {"name": "Transform", "access": "write"},
                {"name": "Camera", "access": "write"},
            ],
        }],
    })
}

/// Reports `L1310` once per [`CameraRig`] in a world whose schedule lacks [`camera_rig`].
pub(crate) fn report_uninstalled(world: &mut World, installed: bool) {
    if installed {
        return;
    }
    let ids: Vec<EntityId> = world
        .query_ids::<(&CameraRig,)>()
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        crate::diagnostics::report(
            world,
            code::CAMERA_RIG_NOT_INSTALLED,
            id.to_string(),
            format!(
                "{id} has a `CameraRig`, but this world's schedule has no `camera_rig`, so it never \
                 poses the camera: the engine installs it only when a scene, snapshot or district is \
                 built with a rig already in it (author the rig in the scene file)"
            ),
        );
    }
}

/// Advances the scene's [`CameraRig`] one tick and poses its camera (the module docs have the
/// rest).
pub fn camera_rig(world: &mut World, ctx: &SystemCtx) {
    let mut ids = world.query_ids::<(&CameraRig,)>().map(|(id, _)| id);
    let Some(entity) = ids.next() else {
        return;
    };
    let second = ids.next();
    drop(ids);
    if let Some(other) = second {
        crate::diagnostics::report(
            world,
            code::CAMERA_RIG_DUPLICATE,
            "camera_rig",
            format!(
                "{entity} and {other} both carry a `CameraRig`; only the one on {entity} runs: \
                 keep one rig, on the camera entity"
            ),
        );
    }
    if !world.has::<Camera>(entity) {
        crate::diagnostics::report(
            world,
            code::CAMERA_RIG_WITHOUT_CAMERA,
            entity.to_string(),
            format!(
                "{entity} has a `CameraRig` but no `Camera`, so there is nothing to pose: put the \
                 rig on the camera entity"
            ),
        );
        return;
    }
    if world.has::<Parent>(entity) {
        crate::diagnostics::report(
            world,
            code::CAMERA_RIG_PARENTED,
            entity.to_string(),
            format!(
                "{entity} has a `CameraRig` and a `Parent`; the rig writes a world-space pose: \
                 make the camera a root entity"
            ),
        );
        return;
    }
    let Ok(Some(rig)) = world
        .get::<CameraRig>(entity)
        .map(Option::<&CameraRig>::cloned)
    else {
        return;
    };
    if let Some((pointer, rule)) = invalid_field(&rig) {
        crate::diagnostics::report_full(
            world,
            entity.to_string(),
            Diagnostic::new(
                code::CAMERA_RIG_INVALID,
                format!("{entity} `CameraRig` `{pointer}` {rule}; the camera holds its pose"),
            )
            .with_path(pointer)
            .with_help("give the field a value in range; `ludion explain L1304` lists the rules"),
        );
        return;
    }
    let RigMode::Track(track) = &rig.rig.mode;
    let (Some(target), false) = (rig.target, track.path.is_empty()) else {
        return;
    };
    let Some(target_position) =
        crate::hierarchy::world_transform(world, target).map(|pose| pose.trs.translation)
    else {
        crate::diagnostics::report(
            world,
            code::CAMERA_RIG_TARGET_MISSING,
            entity.to_string(),
            format!(
                "the `CameraRig` on {entity} follows {target}, which does not exist; the camera \
                 holds its pose until `target` names a live entity"
            ),
        );
        return;
    };
    let Some(library) = world.resource_mut::<PathLibrary>() else {
        return;
    };
    let sampler = match library.sampler(&track.path) {
        Ok(sampler) => sampler,
        Err(causes) => {
            let diagnostic = causes.iter().cloned().fold(
                Diagnostic::new(
                    code::CAMERA_RIG_PATH_UNLOADED,
                    format!(
                        "the `CameraRig` track path `{}` failed to load: {}",
                        track.path,
                        causes
                            .iter()
                            .map(|cause| cause.message.as_str())
                            .collect::<Vec<_>>()
                            .join("; ")
                    ),
                )
                .with_path("/rig/mode/track/path")
                .with_help(
                    "point `path` at an existing `kind: path` document, relative to the content root",
                ),
                Diagnostic::with_related,
            );
            crate::diagnostics::report_full(world, entity.to_string(), diagnostic);
            return;
        }
    };
    let camera_fov_y_deg = match world.get::<Camera>(entity) {
        Ok(Some(Camera {
            projection: Projection::Perspective { fov_y_deg },
            ..
        })) => Some(*fov_y_deg),
        _ => None,
    };
    let mut rig = rig;
    let output = step_rig(
        &mut rig,
        &RigInput {
            target: Some(target_position),
            path: Some(&sampler),
            game_dt: ctx.game_dt,
            dt: ctx.dt,
            game_frame: ctx.game_frame,
            camera_fov_y_deg,
        },
    );
    if let Ok(Some(stored)) = world.get_mut::<CameraRig>(entity) {
        *stored = rig;
    }
    let Some(output) = output else {
        return;
    };
    if let Ok(Some(transform)) = world.get_mut::<Transform>(entity) {
        transform.translation = output.translation;
        transform.rotation = output.rotation_deg;
    }
    if let Some(fov) = output.fov_y_deg
        && let Ok(Some(Camera {
            projection: Projection::Perspective { fov_y_deg },
            ..
        })) = world.get_mut::<Camera>(entity)
    {
        *fov_y_deg = fov;
    }
}
