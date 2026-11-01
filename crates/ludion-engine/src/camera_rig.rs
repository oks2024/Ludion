//! The `camera_rig` system: advances the scene's [`CameraRig`] each tick and writes the result to
//! the camera entity's `Transform` (and the perspective `fov_y_deg`).
//!
//! The system is installed on every schedule and returns at once in a world without a rig. It runs
//! after the physics triple and before the presentation systems, so the camera reads this tick's
//! settled target pose.
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
//!
//! The `camera` debug-draw category shows the track, the search window, the projection, the desired
//! and actual eye, the focus, the anchor, the look-ahead and a running blend; a held rig draws nothing.

use ludion_camera::{CameraRig, RigInput, RigMode, invalid_field, rig_focus, step_rig, track_eye};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::{Parent, Schedule, SystemCtx, World};
use ludion_scene::PathSampler;
use serde_json::Value;

use crate::components::{Camera, Projection, Transform};
use crate::debug_draw::debug_draw;
use crate::path_follow::PathLibrary;

pub(crate) const SYSTEM_NAME: &str = "camera_rig";
const CATEGORY: &str = "camera";
/// The track's polyline (linear RGBA).
const TRACK: [f32; 4] = [0.2, 0.6, 1.0, 1.0];
/// The search window around the projection, brighter than [`TRACK`].
const WINDOW: [f32; 4] = [0.6, 0.9, 1.0, 1.0];
/// The target's projection onto the track.
const PROJECTED: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// The desired eye and its line to the actual eye.
const DESIRED: [f32; 4] = [1.0, 0.8, 0.2, 1.0];
/// The actual (posed, shaken) eye.
const EYE: [f32; 4] = [0.2, 0.9, 0.6, 1.0];
/// The focus and the eye-to-focus line.
const FOCUS: [f32; 4] = [1.0, 0.3, 0.6, 1.0];
/// The vertical anchor line and cross.
const ANCHOR: [f32; 4] = [0.7, 0.4, 1.0, 1.0];
/// The look-ahead ray.
const LOOK_AHEAD: [f32; 4] = [0.6, 1.0, 0.2, 1.0];
/// A running blend's source eye and its line to the actual eye.
const BLEND: [f32; 4] = [1.0, 0.5, 0.1, 1.0];
const CROSS: f32 = 0.2;
/// Spans of the search-window polyline.
const WINDOW_SPANS: usize = 8;

/// Adds [`camera_rig`] to `schedule`. Called after the physics systems and before the presentation
/// systems.
pub(crate) fn install(schedule: &mut Schedule) {
    schedule.add_system(SYSTEM_NAME, camera_rig);
}

/// The system's `engine.describe` entry, with no call site to attribute; the caller sets its `order`.
pub(crate) fn describe_entry() -> Value {
    serde_json::json!({
        "name": SYSTEM_NAME,
        "order": 0,
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
    if let Some(output) = &output {
        draw(world, &rig, &sampler, target_position, output.translation);
    }
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

/// Draws one tick's `camera` primitives (the module docs name the category): the track, the search
/// window around the projection, the projected point, the desired and actual eye, the focus, the
/// anchored height, the look-ahead and, while one runs, the blend's source eye.
fn draw(
    world: &mut World,
    rig: &CameraRig,
    sampler: &PathSampler,
    target: [f32; 3],
    eye: [f32; 3],
) {
    let RigMode::Track(track) = &rig.rig.mode;
    let state = &rig.state;
    let Some(param) = state.param else {
        return;
    };
    let polyline = world
        .resource::<PathLibrary>()
        .and_then(|library| library.polyline(&track.path));
    let window = window_points(sampler, param, track.search_window);
    #[allow(clippy::cast_possible_truncation)]
    let projected = sampler.point_at(f64::from(param)).map(|c| c as f32);
    let desired = track_eye(track, sampler, param);
    let focus = rig_focus(&rig.rig, target, state.anchor_y, state.look_ahead);
    let anchored = [target[0], state.anchor_y, target[2]];
    let mut pen = debug_draw(world, CATEGORY);
    if let Some(polyline) = &polyline {
        pen.path(polyline, TRACK);
    }
    pen.path(&window, WINDOW)
        .cross(projected, CROSS, PROJECTED)
        .cross(desired, CROSS, DESIRED)
        .cross(eye, CROSS, EYE)
        .line(desired, eye, DESIRED)
        .cross(focus, CROSS, FOCUS)
        .line(eye, focus, FOCUS)
        .line(target, anchored, ANCHOR)
        .cross(anchored, CROSS * 0.5, ANCHOR)
        .ray(
            anchored,
            [state.look_ahead[0], 0.0, state.look_ahead[1]],
            LOOK_AHEAD,
        );
    if let Some(blend) = &state.blend {
        pen.cross(blend.from.eye, CROSS, BLEND)
            .line(blend.from.eye, eye, BLEND);
    }
}

/// [`WINDOW_SPANS`] spans of the path over `[param − window, param + window]`, clamped to the path on
/// an open one (`point_at` wraps a closed one).
fn window_points(sampler: &PathSampler, param: f32, window: f32) -> Vec<[f32; 3]> {
    let (param, window) = (f64::from(param), f64::from(window));
    let (mut from, mut to) = (param - window, param + window);
    if !sampler.closed() {
        from = from.max(0.0);
        to = to.min(sampler.length());
    }
    (0..=WINDOW_SPANS)
        .map(|i| {
            #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
            let at = sampler.point_at(from + (to - from) * i as f64 / WINDOW_SPANS as f64);
            #[allow(clippy::cast_possible_truncation)]
            at.map(|c| c as f32)
        })
        .collect()
}
