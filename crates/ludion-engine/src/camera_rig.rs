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
//! A `first_person`, `chase` or `follow` rig needs only a live target: no path, no `L1306`. Look is
//! applied on every tick the rig poses, a held clock included.
//!
//! The `camera` debug-draw category shows, for `track`, the track, the search window, the projection,
//! the desired and actual eye, the focus, the anchor, the look-ahead and a running blend; for
//! `first_person`, the target, its line to the eye, the eye, a 2 m aim ray and a running blend; for
//! `chase` and `follow`, the focus, the sprung pivot, the boom from the pivot to the eye, the eye and
//! a running blend. While a collision pull-in holds the boom short, the collided eye and its line to
//! the uncollided eye draw too. A held rig draws nothing.
//!
//! With `collision.enabled` the stage casts a sphere through [`PhysicsWorld::shape_cast`], excluding
//! the target entity (its child colliders are not excluded: put them on an ignored layer); sensors
//! are skipped. A rejected cast, such as an undeclared layer name, reports `L1550` once and skips
//! the stage; a world with no `PhysicsWorld` reports `L1551` once; a pivot and target both inside
//! geometry report `L1552` once.

use std::sync::Arc;

use std::cell::Cell;

use ludion_camera::{
    CameraRig, RigCollision, RigCollisionConfig, RigInput, RigMode, invalid_field, rig_focus,
    step_rig, track_eye,
};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::{Parent, Schedule, SystemCtx, World};
use ludion_physics::{PhysicsWorld, QueryFilter};
use ludion_scene::PathSampler;
use ludion_spatial::Shape;
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
/// A collision pull-in: the collided eye and its line to the uncollided eye.
const PULLED: [f32; 4] = [1.0, 0.2, 0.2, 1.0];
/// A running blend's source eye and its line to the actual eye.
const BLEND: [f32; 4] = [1.0, 0.5, 0.1, 1.0];
const CROSS: f32 = 0.2;
/// Length of the first-person aim ray, in metres.
const AIM_RAY: f32 = 2.0;
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
        let key = crate::diagnostics::offender_key(world, entity);
        crate::diagnostics::report(
            world,
            code::CAMERA_RIG_WITHOUT_CAMERA,
            key,
            format!(
                "{entity} has a `CameraRig` but no `Camera`, so there is nothing to pose: put the \
                 rig on the camera entity"
            ),
        );
        return;
    }
    if world.has::<Parent>(entity) {
        let key = crate::diagnostics::offender_key(world, entity);
        crate::diagnostics::report(
            world,
            code::CAMERA_RIG_PARENTED,
            key,
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
        let key = crate::diagnostics::offender_key(world, entity);
        crate::diagnostics::report_full(
            world,
            key,
            Diagnostic::new(
                code::CAMERA_RIG_INVALID,
                format!("{entity} `CameraRig` `{pointer}` {rule}; the camera holds its pose"),
            )
            .with_path(pointer)
            .with_help("give the field a value in range; `ludion explain L1304` lists the rules"),
        );
        return;
    }
    let track_path = match &rig.rig.mode {
        RigMode::Track(track) if track.path.is_empty() => return,
        RigMode::Track(track) => Some(track.path.clone()),
        RigMode::FirstPerson(_) | RigMode::Chase(_) | RigMode::Follow(_) => None,
    };
    let Some(target) = rig.target else {
        return;
    };
    let Some(target_position) =
        crate::hierarchy::world_transform(world, target).map(|pose| pose.trs.translation)
    else {
        let key = crate::diagnostics::offender_key(world, entity);
        crate::diagnostics::report(
            world,
            code::CAMERA_RIG_TARGET_MISSING,
            key,
            format!(
                "the `CameraRig` on {entity} follows {target}, which does not exist; the camera \
                 holds its pose until `target` names a live entity"
            ),
        );
        return;
    };
    let sampler = match &track_path {
        Some(path) => match load_sampler(world, entity, path) {
            Some(sampler) => Some(sampler),
            None => return,
        },
        None => None,
    };

    let camera_fov_y_deg = match world.get::<Camera>(entity) {
        Ok(Some(Camera {
            projection: Projection::Perspective { fov_y_deg },
            ..
        })) => Some(*fov_y_deg),
        _ => None,
    };
    let mut rig = rig;
    crate::camera_zone::apply(world, entity, &mut rig, target, target_position);
    let cut = rig.state.snap || rig.state.last_applied.is_none();
    let physics = world.resource::<PhysicsWorld>();
    let no_physics = rig.rig.collision.enabled && physics.is_none();
    let cast = physics
        .filter(|_| rig.rig.collision.enabled)
        .map(|physics| PhysicsRigCollision::new(physics, &rig.rig.collision, target));
    let output = step_rig(
        &mut rig,
        &RigInput {
            target: Some(target_position),
            path: sampler.as_deref(),
            game_dt: ctx.game_dt,
            dt: ctx.dt,
            game_frame: ctx.game_frame,
            camera_fov_y_deg,
            collision: cast.as_ref().map(|cast| cast as &dyn RigCollision),
        },
    );
    let rejected = cast.and_then(|cast| cast.failed.take());
    if no_physics || rejected.is_some() || output.as_ref().is_some_and(|out| out.stuck) {
        report_collision(
            world,
            entity,
            no_physics,
            rejected,
            output.is_some_and(|o| o.stuck),
        );
    }
    if let Some(output) = &output {
        match &sampler {
            Some(sampler) => draw(world, &rig, sampler, target_position, output.translation),
            None if matches!(rig.rig.mode, RigMode::Chase(_) | RigMode::Follow(_)) => {
                draw_orbit(world, &rig, target_position, output.translation);
            }
            None => draw_first_person(world, &rig, target_position, output.translation),
        }
    }
    if let Ok(Some(stored)) = world.get_mut::<CameraRig>(entity) {
        *stored = rig;
    }
    let Some(output) = output else {
        return;
    };
    if cut {
        crate::interpolation::cut_presentation(world, entity);
    }
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

/// The collision stage's physics: a sphere cast of the rig's radius that skips the target, sensors
/// and the ignored layers.
struct PhysicsRigCollision<'a> {
    physics: &'a PhysicsWorld,
    filter: QueryFilter,
    /// The first physics error of the tick; the cast reports clear after it.
    failed: Cell<Option<String>>,
}

impl std::fmt::Debug for PhysicsRigCollision<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhysicsRigCollision")
            .finish_non_exhaustive()
    }
}

impl<'a> PhysicsRigCollision<'a> {
    fn new(physics: &'a PhysicsWorld, cfg: &RigCollisionConfig, target: EntityId) -> Self {
        let mut filter = QueryFilter::new().exclude(target);
        for name in &cfg.include_layers {
            filter = filter.include_layer(name.as_str());
        }
        for name in &cfg.ignore_layers {
            filter = filter.ignore_layer(name.as_str());
        }
        Self {
            physics,
            filter,
            failed: Cell::new(None),
        }
    }
}

impl RigCollision for PhysicsRigCollision<'_> {
    fn cast(&self, from: [f32; 3], dir: [f32; 3], max: f32, radius: f32) -> Option<f32> {
        let shape = Shape::Sphere { radius };
        match self
            .physics
            .shape_cast(&shape, from, [0.0, 0.0, 0.0, 1.0], dir, max, &self.filter)
        {
            Ok(hits) => hits.first().map(|hit| hit.distance),
            Err(error) => {
                if let Some(first) = self.failed.take() {
                    self.failed.set(Some(first));
                } else {
                    self.failed.set(Some(error.to_string()));
                }
                None
            }
        }
    }
}

/// Reports the collision stage's faults, each once per rig.
fn report_collision(
    world: &mut World,
    entity: EntityId,
    no_physics: bool,
    rejected: Option<String>,
    stuck: bool,
) {
    let key = crate::diagnostics::offender_key(world, entity);
    if no_physics {
        crate::diagnostics::report(
            world,
            code::CAMERA_RIG_COLLISION_NO_PHYSICS,
            key.clone(),
            format!(
                "the `CameraRig` on {entity} has `collision.enabled` but the world has no \
                 `PhysicsWorld`; the camera runs without collision"
            ),
        );
    }
    if let Some(error) = rejected {
        crate::diagnostics::report_full(
            world,
            key.clone(),
            Diagnostic::new(
                code::CAMERA_RIG_COLLISION_QUERY,
                format!(
                    "the `CameraRig` on {entity} could not cast its collision sphere: {error}; \
                     the camera runs without collision"
                ),
            )
            .with_path("/rig/collision")
            .with_help(
                "declare the layer names in `include_layers` and `ignore_layers` under \
                 `physics.layers`",
            ),
        );
    }
    if stuck {
        crate::diagnostics::report(
            world,
            code::CAMERA_RIG_COLLISION_STUCK,
            key,
            format!(
                "the `CameraRig` on {entity} has its pivot and target inside geometry; the boom \
                 is held at `collision.min_distance`: move the pivot out of the wall, or put the \
                 target's own colliders on an `ignore_layers` layer"
            ),
        );
    }
}

/// The sampler of the track `path`, or `None` after reporting `L1306` once (the camera holds).
fn load_sampler(world: &mut World, entity: EntityId, path: &str) -> Option<Arc<PathSampler>> {
    let library = world.resource_mut::<PathLibrary>()?;
    let causes = match library.sampler(path) {
        Ok(sampler) => return Some(sampler),
        Err(causes) => causes,
    };
    let diagnostic = causes.iter().cloned().fold(
        Diagnostic::new(
            code::CAMERA_RIG_PATH_UNLOADED,
            format!(
                "the `CameraRig` track path `{path}` failed to load: {}",
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
    let key = crate::diagnostics::offender_key(world, entity);
    crate::diagnostics::report_full(world, key, diagnostic);
    None
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
    let RigMode::Track(track) = &rig.rig.mode else {
        return;
    };
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
    if state.boom.is_some() {
        pen.cross(eye, CROSS, PULLED).line(eye, state.eye, PULLED);
    }
    if let Some(blend) = &state.blend {
        pen.cross(blend.from.eye, CROSS, BLEND)
            .line(blend.from.eye, eye, BLEND);
    }
}

/// Draws a first-person rig's `camera` primitives: the target, the line to the eye, the eye, a
/// [`AIM_RAY`] metre ray along the aim and, while one runs, the blend's source eye.
fn draw_first_person(world: &mut World, rig: &CameraRig, target: [f32; 3], eye: [f32; 3]) {
    let state = &rig.state;
    let aim = rig.aim();
    let mut pen = debug_draw(world, CATEGORY);
    pen.cross(target, CROSS, PROJECTED)
        .line(target, eye, DESIRED)
        .cross(eye, CROSS, EYE);
    if let Some(aim) = aim {
        let forward = aim.forward().map(|v| v * AIM_RAY);
        pen.ray(aim.eye, forward, FOCUS);
    }
    if let Some(blend) = &state.blend {
        pen.cross(blend.from.eye, CROSS, BLEND)
            .line(blend.from.eye, eye, BLEND);
    }
}

/// Draws a `chase` or `follow` rig's `camera` primitives: the focus, the sprung pivot, the boom from
/// the pivot to the eye, the eye and, while one runs, the blend's source eye.
fn draw_orbit(world: &mut World, rig: &CameraRig, target: [f32; 3], eye: [f32; 3]) {
    let state = &rig.state;
    let Some(pivot) = state.pivot else {
        return;
    };
    let focus = rig_focus(&rig.rig, target, state.anchor_y, state.look_ahead);
    let mut pen = debug_draw(world, CATEGORY);
    pen.cross(focus, CROSS, FOCUS)
        .cross(pivot, CROSS, PROJECTED)
        .line(pivot, eye, DESIRED)
        .cross(eye, CROSS, EYE);
    if state.boom.is_some() {
        pen.cross(eye, CROSS, PULLED).line(eye, state.eye, PULLED);
    }
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
