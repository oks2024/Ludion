//! The per-tick rig pipeline.

use ludion_core::math::{
    Orbit, atan2, euler_xyz_degrees_to_quat, powf, quat_mul, quat_rotate,
    quat_to_euler_xyz_degrees, sin_cos, sqrt,
};
use ludion_core::time::FrameIndex;
use ludion_scene::PathSampler;

use crate::collision::{RigCollision, collide, held_eye, release};
use crate::first_person::{apply_look, first_person_eye, looked};
use crate::orbit::{chase_look, chased, clamp_pitch, orbit_eye};
use crate::rig::{Aim, Blend, CameraRig, Framing, Pose, Recenter, RigMode, RigState};
use crate::shake::shake_noise;
use crate::spring::{damp, decay, ease, lerp_pose, spring_step, wrap180};
use crate::track::{Desired, desired, rig_focus};

const DEG_TO_RAD: f32 = core::f32::consts::PI / 180.0;
const RAD_TO_DEG: f32 = 180.0 / core::f32::consts::PI;
const FRAMING_REST: f32 = 1e-4;

/// What the engine resolved for one rig tick.
#[derive(Clone, Copy, Debug)]
pub struct RigInput<'a> {
    /// The target's world position, or `None` when it names no live entity.
    pub target: Option<[f32; 3]>,
    /// The loaded track path, or `None` when it is empty or failed to load. Every mode but `track`
    /// ignores it.
    pub path: Option<&'a PathSampler>,
    /// Gameplay seconds this tick; `0` while the clock is held.
    pub game_dt: f32,
    /// Sim seconds this tick.
    pub dt: f32,
    /// The gameplay frame, frozen while the clock is held.
    pub game_frame: FrameIndex,
    /// The camera's current vertical field of view in degrees, when it is perspective.
    pub camera_fov_y_deg: Option<f32>,
    /// The sphere cast the collision stage uses, or `None` to skip the stage (the crate has no
    /// physics of its own).
    pub collision: Option<&'a dyn RigCollision>,
}

/// The camera pose a tick produced, shake included.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RigOutput {
    /// World position for the camera's `Transform`.
    pub translation: [f32; 3],
    /// Euler XYZ degrees for the camera's `Transform`.
    pub rotation_deg: [f32; 3],
    /// The field of view to write, when the rig sets one.
    pub fov_y_deg: Option<f32>,
    /// The collision stage found both the cast origin and the target inside geometry and holds the
    /// boom at `min_distance`.
    pub stuck: bool,
}

/// Advances `rig` one tick and returns the camera pose, or `None` (touching nothing, so the engine
/// holds the last pose) when there is no target or, in `track` mode, no path.
///
/// A rig without `state.last_applied`, or flagged by [`snap_rig`](crate::snap_rig), snaps; a `rig`
/// that differs from `last_applied` starts a blend from the current pose. Springs, look-ahead, the
/// anchor, trauma, the kick decay and the shake clock advance only while `game_dt > 0`; a held tick re-poses from
/// the frozen state. A `first_person` rig has no springs: it applies and zeroes `look_deg` on every
/// tick it poses, held clock included, and places the eye rigidly on the target. A `chase` or
/// `follow` rig springs only a pivot toward the focus (`position_half_life`) and hangs the eye on a
/// boom from it at unsprung angles: `chase` applies and zeroes `look_deg` like `first_person`,
/// `follow` zeroes it unread and holds its authored yaw and pitch.
///
/// # Example
///
/// ```
/// use ludion_camera::{CameraRig, RigInput, step_rig};
/// use ludion_core::time::FrameIndex;
/// use ludion_scene::{PathDocument, PathInterpolation, PathSampler};
///
/// let path = PathSampler::new(&PathDocument {
///     interpolation: PathInterpolation::Linear,
///     points: vec![[0.0, 0.0, 0.0], [0.0, 0.0, -50.0]],
///     ..PathDocument::default()
/// });
/// let mut rig = CameraRig::default();
/// let input = RigInput {
///     target: Some([0.0, 0.0, -10.0]),
///     path: Some(&path),
///     game_dt: 1.0 / 60.0,
///     dt: 1.0 / 60.0,
///     game_frame: FrameIndex::ZERO,
///     camera_fov_y_deg: None,
///     collision: None,
/// };
/// let out = step_rig(&mut rig, &input).unwrap();
/// assert!((out.translation[1] - 2.2).abs() < 1e-5 && (out.translation[2] + 5.5).abs() < 1e-5);
/// ```
pub fn step_rig(rig: &mut CameraRig, input: &RigInput) -> Option<RigOutput> {
    let target = input.target?;
    let sampler = match &rig.rig.mode {
        RigMode::Track(_) => Some(input.path?),
        RigMode::FirstPerson(_) | RigMode::Chase(_) | RigMode::Follow(_) => None,
    };
    match &rig.rig.mode {
        RigMode::FirstPerson(fp) => {
            let look = std::mem::take(&mut rig.look_deg);
            apply_look(&mut rig.state, fp, look);
        }
        RigMode::Chase(chase) => {
            let look = std::mem::take(&mut rig.look_deg);
            if look != [0.0; 2] {
                rig.state.look_idle = 0.0;
            }
            chase_look(&mut rig.state, chase, look);
        }
        RigMode::Follow(follow) => {
            rig.look_deg = [0.0; 2];
            rig.state.yaw_deg = wrap180(follow.yaw_deg);
            rig.state.pitch_deg = follow.pitch_deg;
        }
        RigMode::Track(_) => {}
    }

    let bad_blend = rig
        .state
        .blend
        .as_ref()
        .is_some_and(|blend| unusable_blend(blend, input.game_frame));
    let snapped = rig.state.snap || rig.state.last_applied.is_none() || bad_blend;
    if snapped {
        snap(rig, target, sampler);
    } else if rig.state.last_applied.as_ref() != Some(&rig.rig) {
        switch(rig, target, sampler, input);
    }
    if !snapped && input.game_dt > 0.0 {
        advance(rig, target, sampler, input.game_dt);
    }
    if let RigMode::FirstPerson(fp) = &rig.rig.mode {
        rig.state.eye = first_person_eye(fp, target, rig.state.yaw_deg);
    }
    let mut live = Pose {
        eye: rig.state.eye,
        yaw_deg: rig.state.yaw_deg,
        pitch_deg: rig.state.pitch_deg,
        fov_y_deg: rig.rig.fov_y_deg,
    };
    let mut origin = None;
    if let Some(boom) = boom(rig, rig.state.yaw_deg, rig.state.pitch_deg) {
        rig.state.eye = boom.eye;
        (live.eye, live.yaw_deg, live.pitch_deg) = (boom.eye, boom.yaw_deg, boom.pitch_deg);
        origin = Some(boom.origin);
    } else if let RigMode::Track(_) = &rig.rig.mode {
        let state = &rig.state;
        origin = Some(rig_focus(
            &rig.rig,
            target,
            state.anchor_y,
            state.look_ahead,
        ));
    }

    let mut pose = blended_pose(rig, input, live);
    let mut stuck = false;
    match (origin, input.collision) {
        (Some(origin), Some(collision)) if rig.rig.collision.enabled => {
            (pose.eye, stuck) = collide(
                &mut rig.state,
                &rig.rig.collision,
                collision,
                origin,
                pose.eye,
                target,
                input.game_dt,
            );
        }
        _ => release(&mut rig.state),
    }
    rig.state.pose = Some(pose);
    resolved_camera(rig, target, [0.0; 2]).map(|resolved| RigOutput {
        stuck,
        ..resolved.view
    })
}

/// The frame's resolved camera: the pre-effect aim and the final view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedCamera {
    /// The eye and orientation before kick, roll, FOV channels and shake ([`CameraRig::aim`]).
    pub aim: Aim,
    /// The camera pose after the post-aim stage, as [`step_rig`] returns it.
    pub view: RigOutput,
}

/// The one seam that produces a frame's camera from a rig's last pose: extra look, then aim, then
/// the post-aim stage. [`step_rig`] goes through it with no extra look; a presenter that latches
/// fresher mouse motion calls it with the look accumulated since the tick.
///
/// `None` before the first pose. With `extra_look_deg == [0, 0]` the result is the tick's camera
/// bit for bit. The extra look `[yaw, pitch]` (degrees, as [`add_look`](crate::add_look)) applies
/// only to a `first_person` or `chase` rig with no blend running; pitch clamps to the mode's
/// range (`chase` moves it only with `look_pitch`), the eye follows the new angles (about the
/// pivot, for `chase`, framed like the tick's boom), and kick, roll, the FOV channels and shake
/// apply on top. The latched eye keeps the tick's collided boom length without casting again: the
/// presenter has no physics. In any other case it is ignored.
///
/// # Example
///
/// ```
/// use ludion_camera::{CameraRig, RigInput, resolved_camera, step_rig};
/// use ludion_core::time::FrameIndex;
///
/// let mut rig = CameraRig::default();
/// rig.rig.mode = ludion_camera::RigMode::FirstPerson(Default::default());
/// let target = [0.0, 0.0, -3.0];
/// let input = RigInput {
///     target: Some(target),
///     path: None,
///     game_dt: 1.0 / 60.0,
///     dt: 1.0 / 60.0,
///     game_frame: FrameIndex::ZERO,
///     camera_fov_y_deg: None,
///     collision: None,
/// };
/// let out = step_rig(&mut rig, &input).unwrap();
/// assert_eq!(resolved_camera(&rig, target, [0.0; 2]).unwrap().view, out);
/// let turned = resolved_camera(&rig, target, [10.0, 0.0]).unwrap();
/// assert_eq!(turned.aim.yaw_deg, 10.0);
/// ```
pub fn resolved_camera(
    rig: &CameraRig,
    target: [f32; 3],
    extra_look_deg: [f32; 2],
) -> Option<ResolvedCamera> {
    let mut pose = rig.state.pose.clone()?;
    if extra_look_deg != [0.0; 2] && rig.state.blend.is_none() {
        match &rig.rig.mode {
            RigMode::FirstPerson(fp) => {
                (pose.yaw_deg, pose.pitch_deg) =
                    looked(fp, pose.yaw_deg, pose.pitch_deg, extra_look_deg);
                pose.eye = first_person_eye(fp, target, pose.yaw_deg);
            }
            RigMode::Chase(chase) => {
                let (yaw, pitch) = if rig.state.framing_weight == 0.0 {
                    (pose.yaw_deg, pose.pitch_deg)
                } else {
                    (rig.state.yaw_deg, rig.state.pitch_deg)
                };
                let (yaw, pitch) = chased(chase, yaw, pitch, extra_look_deg);
                if let Some(boom) = boom(rig, yaw, pitch) {
                    pose.eye = held_eye(&rig.state, boom.origin, boom.eye);
                    (pose.yaw_deg, pose.pitch_deg) = (boom.yaw_deg, boom.pitch_deg);
                } else {
                    (pose.yaw_deg, pose.pitch_deg) = (yaw, pitch);
                }
            }
            RigMode::Track(_) | RigMode::Follow(_) => {}
        }
    }
    Some(ResolvedCamera {
        aim: Aim {
            eye: pose.eye,
            yaw_deg: pose.yaw_deg,
            pitch_deg: pose.pitch_deg,
        },
        view: post_aim(rig, &pose),
    })
}

/// A rewound or hand-edited blend snaps; `switch` only ever writes `seconds > 0`.
fn unusable_blend(blend: &Blend, frame: FrameIndex) -> bool {
    let from = &blend.from;
    blend.started > frame
        || !(blend.seconds > 0.0 && blend.seconds.is_finite())
        || from.eye.iter().any(|v| !v.is_finite())
        || !from.yaw_deg.is_finite()
        || !from.pitch_deg.is_finite()
        || from.fov_y_deg.is_some_and(|v| !v.is_finite())
        || blend.yaw_span.is_some_and(|v| !v.is_finite())
}

/// Re-initialises the projection and the springs at rest on the desired pose of the current config.
fn rest_on_desired(rig: &mut CameraRig, target: [f32; 3], sampler: Option<&PathSampler>) {
    let state = &rig.state;
    let sampler = match (&rig.rig.mode, sampler) {
        (RigMode::FirstPerson(fp), _) => {
            let eye = first_person_eye(fp, target, state.yaw_deg);
            let state = &mut rig.state;
            state.eye = eye;
            if let Some(pitch) = fp.enter_pitch_deg {
                state.pitch_deg = pitch;
            }
            state.eye_velocity = [0.0; 3];
            state.yaw_velocity = 0.0;
            state.pitch_velocity = 0.0;
            state.pivot = None;
            state.pivot_velocity = [0.0; 3];
            return;
        }
        (RigMode::Chase(_) | RigMode::Follow(_), _) => {
            let focus = rig_focus(&rig.rig, target, state.anchor_y, state.look_ahead);
            let (yaw_deg, pitch_deg) = match &rig.rig.mode {
                RigMode::Chase(chase) => (
                    chase.yaw_deg.map_or(state.yaw_deg, wrap180),
                    clamp_pitch(chase, chase.pitch_deg),
                ),
                RigMode::Follow(follow) => (wrap180(follow.yaw_deg), follow.pitch_deg),
                _ => unreachable!("matched a boom mode"),
            };
            let state = &mut rig.state;
            state.pivot = Some(focus);
            state.pivot_velocity = [0.0; 3];
            state.eye_velocity = [0.0; 3];
            state.yaw_deg = yaw_deg;
            state.pitch_deg = pitch_deg;
            state.yaw_velocity = 0.0;
            state.pitch_velocity = 0.0;
            if let Some(boom) = boom(rig, rig.state.yaw_deg, rig.state.pitch_deg) {
                rig.state.eye = boom.eye;
            }
            return;
        }
        (RigMode::Track(_), Some(sampler)) => sampler,
        (RigMode::Track(_), None) => return,
    };
    let last = (state.yaw_deg, state.pitch_deg);
    let Desired {
        param,
        eye,
        yaw_deg,
        pitch_deg,
    } = desired(
        &rig.rig,
        sampler,
        target,
        state.anchor_y,
        state.look_ahead,
        None,
        last,
    );
    let state = &mut rig.state;
    state.param = Some(param);
    state.eye = eye;
    state.eye_velocity = [0.0; 3];
    state.yaw_deg = yaw_deg;
    state.pitch_deg = pitch_deg;
    state.yaw_velocity = 0.0;
    state.pitch_velocity = 0.0;
    state.pivot = None;
    state.pivot_velocity = [0.0; 3];
}

/// A `chase` or `follow` boom: where it starts, the eye and the framed angles.
struct Boom {
    origin: [f32; 3],
    eye: [f32; 3],
    yaw_deg: f32,
    pitch_deg: f32,
}

/// The boom of a `chase` or `follow` rig from the sprung pivot at the unframed `yaw_deg` and
/// `pitch_deg`, with the applied framing weight folded in; `None` in other modes. At weight `0` it
/// is the plain orbit.
fn boom(rig: &CameraRig, yaw_deg: f32, pitch_deg: f32) -> Option<Boom> {
    let state = &rig.state;
    let (distance, chase) = match &rig.rig.mode {
        RigMode::Chase(chase) => (chase.distance, Some(chase)),
        RigMode::Follow(follow) => (follow.distance, None),
        RigMode::Track(_) | RigMode::FirstPerson(_) => return None,
    };
    let pivot = state.pivot?;
    let w = state.framing_weight;
    if w == 0.0 {
        return Some(Boom {
            origin: pivot,
            eye: orbit_eye(pivot, yaw_deg, pitch_deg, distance),
            yaw_deg,
            pitch_deg,
        });
    }
    let framing = &rig.framing;
    let (s, c) = sin_cos(yaw_deg * DEG_TO_RAD);
    let [ox, oy, oz] = framing.pivot_offset;
    let origin = [
        pivot[0] + (ox * c + oz * s) * w,
        pivot[1] + oy * w,
        pivot[2] + (-ox * s + oz * c) * w,
    ];
    let yaw = wrap180(yaw_deg + framing.yaw_offset_deg * w);
    let pitch = pitch_deg + framing.pitch_offset_deg * w;
    let pitch = chase.map_or_else(
        || pitch.clamp(-89.0, 89.0),
        |chase| clamp_pitch(chase, pitch),
    );
    let distance = distance + (framing.distance.unwrap_or(distance) - distance) * w;
    Some(Boom {
        origin,
        eye: orbit_eye(origin, yaw, pitch, distance),
        yaw_deg: yaw,
        pitch_deg: pitch,
    })
}

/// The framing weight the game asks for, in `[0, 1]`; a non-finite weight is none.
fn framing_goal(framing: &Framing) -> f32 {
    let weight = framing.weight;
    if weight.is_finite() {
        weight.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// One step of the applied framing weight toward `goal`, snapped to exactly the goal once within
/// `1e-4` so a returned peek rests at `0.0` and skips serialization.
fn ease_framing(weight: f32, goal: f32, half_life: f32, dt: f32) -> f32 {
    let next = damp(weight, goal, half_life, dt);
    if (next - goal).abs() < FRAMING_REST {
        goal
    } else {
        next
    }
}

/// Turns the chase yaw toward the heading of the target's horizontal motion once look has been
/// idle for `delay`. Counts idle time only while enabled, so a disabled recenter leaves the state
/// untouched.
fn recenter(state: &mut RigState, cfg: &Recenter, velocity: [f32; 2], speed: f32, dt: f32) {
    if !cfg.enabled {
        state.look_idle = 0.0;
        return;
    }
    state.look_idle += dt;
    if state.look_idle >= cfg.delay && speed >= cfg.min_speed {
        let goal = atan2(-velocity[0], -velocity[1]) * RAD_TO_DEG;
        let yaw_goal = state.yaw_deg + wrap180(goal - state.yaw_deg);
        state.yaw_deg = wrap180(damp(state.yaw_deg, yaw_goal, cfg.half_life, dt));
    }
}

fn snap(rig: &mut CameraRig, target: [f32; 3], sampler: Option<&PathSampler>) {
    rig.state.anchor_y = target[1];
    rig.state.look_ahead = [0.0; 2];
    rig.state.last_target = Some(target);
    rest_on_desired(rig, target, sampler);
    rig.state.blend = None;
    release(&mut rig.state);
    rig.state.framing_weight = framing_goal(&rig.framing);
    rig.state.last_applied = Some(rig.rig.clone());
    rig.state.snap = false;
}

/// A config write: blend from the current pose into the new config, whose projection and springs
/// start at rest. The anchor, look-ahead and velocity history carry over.
fn switch(rig: &mut CameraRig, target: [f32; 3], sampler: Option<&PathSampler>, input: &RigInput) {
    let old_fov = rig
        .state
        .last_applied
        .as_ref()
        .and_then(|cfg| cfg.fov_y_deg);
    let mut from = rig.state.pose.clone().unwrap_or(Pose {
        eye: rig.state.eye,
        yaw_deg: rig.state.yaw_deg,
        pitch_deg: rig.state.pitch_deg,
        fov_y_deg: old_fov,
    });
    from.fov_y_deg = from.fov_y_deg.or(input.camera_fov_y_deg);
    rest_on_desired(rig, target, sampler);
    let blend_in = &rig.rig.blend_in;
    let yaw_span = Some(wrap180(rig.state.yaw_deg - from.yaw_deg));
    rig.state.blend = (blend_in.seconds > 0.0).then_some(Blend {
        from,
        yaw_span,
        started: input.game_frame,
        seconds: blend_in.seconds,
        curve: blend_in.curve,
    });
    rig.state.last_applied = Some(rig.rig.clone());
}

fn advance(rig: &mut CameraRig, target: [f32; 3], sampler: Option<&PathSampler>, dt: f32) {
    rig.trauma = (rig.trauma - rig.shake.decay_per_second * dt).max(0.0);
    rig.state.shake_time += f64::from(dt);
    let kick = &rig.kick;
    rig.kick_deg = rig.kick_deg.map(|x| decay(x, kick.half_life, dt));
    rig.fov_kick_deg = decay(rig.fov_kick_deg, kick.fov_half_life, dt);
    if matches!(rig.rig.mode, RigMode::FirstPerson(_)) {
        rig.state.last_target = Some(target);
        return;
    }

    let cfg = &rig.rig;
    let state = &mut rig.state;
    let last_target = state.last_target.unwrap_or(target);
    let velocity = [
        (target[0] - last_target[0]) / dt,
        (target[2] - last_target[2]) / dt,
    ];
    let speed = sqrt(velocity[0] * velocity[0] + velocity[1] * velocity[1]);
    let la = &cfg.look_ahead;
    let goal = if speed < 1e-6 {
        [0.0; 2]
    } else {
        let scale = if la.full_speed > 0.0 {
            (speed / la.full_speed).min(1.0)
        } else {
            1.0
        };
        let reach = la.distance * scale / speed;
        [velocity[0] * reach, velocity[1] * reach]
    };
    state.look_ahead = [
        damp(state.look_ahead[0], goal[0], la.half_life, dt),
        damp(state.look_ahead[1], goal[1], la.half_life, dt),
    ];

    let va = &cfg.vertical_anchor;
    let anchor_goal = if rig.grounded {
        Some(target[1])
    } else if target[1] > state.anchor_y + va.up_slack {
        Some(target[1] - va.up_slack)
    } else if target[1] < state.anchor_y - va.down_slack {
        Some(target[1] + va.down_slack)
    } else {
        None
    };
    if let Some(goal) = anchor_goal {
        state.anchor_y = damp(state.anchor_y, goal, va.half_life, dt);
    }
    state.last_target = Some(target);

    if matches!(cfg.mode, RigMode::Chase(_) | RigMode::Follow(_)) {
        let focus = rig_focus(cfg, target, state.anchor_y, state.look_ahead);
        let pivot = state.pivot.unwrap_or(focus);
        let mut next = pivot;
        for axis in 0..3 {
            (next[axis], state.pivot_velocity[axis]) = spring_step(
                pivot[axis],
                state.pivot_velocity[axis],
                focus[axis],
                cfg.springs.position_half_life,
                dt,
            );
        }
        state.pivot = Some(next);
        let goal = framing_goal(&rig.framing);
        state.framing_weight = ease_framing(state.framing_weight, goal, rig.framing.half_life, dt);
        if let RigMode::Chase(chase) = &cfg.mode {
            recenter(state, &chase.recenter, velocity, speed, dt);
        }
        return;
    }
    let Some(sampler) = sampler else {
        return;
    };

    let want = desired(
        cfg,
        sampler,
        target,
        state.anchor_y,
        state.look_ahead,
        state.param,
        (state.yaw_deg, state.pitch_deg),
    );
    state.param = Some(want.param);
    let half_life = cfg.springs.position_half_life;
    for axis in 0..3 {
        (state.eye[axis], state.eye_velocity[axis]) = spring_step(
            state.eye[axis],
            state.eye_velocity[axis],
            want.eye[axis],
            half_life,
            dt,
        );
    }
    let half_life = cfg.springs.rotation_half_life;
    let yaw_goal = state.yaw_deg + wrap180(want.yaw_deg - state.yaw_deg);
    let (yaw, yaw_velocity) =
        spring_step(state.yaw_deg, state.yaw_velocity, yaw_goal, half_life, dt);
    state.yaw_deg = wrap180(yaw);
    state.yaw_velocity = yaw_velocity;
    (state.pitch_deg, state.pitch_velocity) = spring_step(
        state.pitch_deg,
        state.pitch_velocity,
        want.pitch_deg,
        half_life,
        dt,
    );
}

/// The rig's own pose, or the running blend toward it (dropped once it completes).
fn blended_pose(rig: &mut CameraRig, input: &RigInput, live: Pose) -> Pose {
    let state = &mut rig.state;
    let Some(blend) = &state.blend else {
        return live;
    };
    let frames = input
        .game_frame
        .as_u64()
        .saturating_sub(blend.started.as_u64());
    let t = frames as f32 * input.dt / blend.seconds;
    if t < 1.0 {
        lerp_pose(&blend.from, &live, blend.yaw_span, ease(blend.curve, t))
    } else {
        state.blend = None;
        live
    }
}

/// The post-aim stage: the pose as a camera transform with the recoil kick, roll, field-of-view
/// channels and then the trauma shake applied to the result only. Effects that must never leak into
/// `CameraRig::aim` belong here. With every channel at zero the arithmetic is the plain pose's.
fn post_aim(rig: &CameraRig, pose: &Pose) -> RigOutput {
    let mut translation = pose.eye;
    let mut pitch_deg = pose.pitch_deg + rig.kick_deg[1];
    if let RigMode::FirstPerson(fp) = &rig.rig.mode
        && rig.kick_deg[1] != 0.0
    {
        pitch_deg = pitch_deg.clamp(-fp.pitch_limit_deg, fp.pitch_limit_deg);
    }
    let mut rotation_deg = Orbit {
        target: pose.eye,
        azimuth_deg: wrap180(pose.yaw_deg + rig.kick_deg[0]),
        pitch_deg,
        distance: 1.0,
    }
    .rotation_deg();
    let roll = rig.roll_deg + rig.kick_deg[2];
    if roll != 0.0 {
        let base = euler_xyz_degrees_to_quat(rotation_deg);
        rotation_deg =
            quat_to_euler_xyz_degrees(quat_mul(base, euler_xyz_degrees_to_quat([0.0, 0.0, roll])));
    }
    if rig.trauma > 0.0 {
        let shake = &rig.shake;
        let magnitude = powf(rig.trauma, shake.exponent);
        let noise =
            |channel: u32| shake_noise(shake.seed, channel, rig.state.shake_time, shake.frequency);
        let base = euler_xyz_degrees_to_quat(rotation_deg);
        let offset = quat_rotate(
            base,
            [0, 1, 2].map(|i| shake.max_offset[i] * noise(i as u32) * magnitude),
        );
        translation = [0, 1, 2].map(|i| translation[i] + offset[i]);
        let angles = [0, 1, 2].map(|i| shake.max_angle_deg[i] * noise(3 + i as u32) * magnitude);
        rotation_deg = quat_to_euler_xyz_degrees(quat_mul(base, euler_xyz_degrees_to_quat(angles)));
    }
    let fov_y_deg = if rig.fov_offset_deg != 0.0 || rig.fov_kick_deg != 0.0 {
        pose.fov_y_deg
            .map(|fov| (fov + rig.fov_offset_deg + rig.fov_kick_deg).clamp(1.0, 179.0))
    } else {
        pose.fov_y_deg
    };
    RigOutput {
        translation,
        rotation_deg,
        fov_y_deg,
        stuck: false,
    }
}
