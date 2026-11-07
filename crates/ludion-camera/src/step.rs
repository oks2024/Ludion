//! The per-tick rig pipeline.

use ludion_core::math::{
    Orbit, euler_xyz_degrees_to_quat, powf, quat_mul, quat_rotate, quat_to_euler_xyz_degrees, sqrt,
};
use ludion_core::time::FrameIndex;
use ludion_scene::PathSampler;

use crate::first_person::{apply_look, first_person_eye};
use crate::rig::{Blend, CameraRig, Pose, RigMode};
use crate::shake::shake_noise;
use crate::spring::{damp, decay, ease, lerp_pose, spring_step, wrap180};
use crate::track::{Desired, desired};

/// What the engine resolved for one rig tick.
#[derive(Clone, Copy, Debug)]
pub struct RigInput<'a> {
    /// The target's world position, or `None` when it names no live entity.
    pub target: Option<[f32; 3]>,
    /// The loaded track path, or `None` when it is empty or failed to load. A `first_person` rig
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
}

/// Advances `rig` one tick and returns the camera pose, or `None` (touching nothing, so the engine
/// holds the last pose) when there is no target or, in `track` mode, no path.
///
/// A rig without `state.last_applied`, or flagged by [`snap_rig`](crate::snap_rig), snaps; a `rig`
/// that differs from `last_applied` starts a blend from the current pose. Springs, look-ahead, the
/// anchor, trauma, the kick decay and the shake clock advance only while `game_dt > 0`; a held tick re-poses from
/// the frozen state. A `first_person` rig has no springs: it applies and zeroes `look_deg` on every
/// tick it poses, held clock included, and places the eye rigidly on the target.
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
/// };
/// let out = step_rig(&mut rig, &input).unwrap();
/// assert!((out.translation[1] - 2.2).abs() < 1e-5 && (out.translation[2] + 5.5).abs() < 1e-5);
/// ```
pub fn step_rig(rig: &mut CameraRig, input: &RigInput) -> Option<RigOutput> {
    let target = input.target?;
    let sampler = match &rig.rig.mode {
        RigMode::Track(_) => Some(input.path?),
        RigMode::FirstPerson(_) => None,
    };
    if let RigMode::FirstPerson(fp) = &rig.rig.mode {
        let look = std::mem::take(&mut rig.look_deg);
        apply_look(&mut rig.state, fp, look);
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

    let pose = blended_pose(rig, input);
    rig.state.pose = Some(pose.clone());
    Some(post_aim(rig, &pose))
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
            state.eye_velocity = [0.0; 3];
            state.yaw_velocity = 0.0;
            state.pitch_velocity = 0.0;
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
}

fn snap(rig: &mut CameraRig, target: [f32; 3], sampler: Option<&PathSampler>) {
    rig.state.anchor_y = target[1];
    rig.state.look_ahead = [0.0; 2];
    rig.state.last_target = Some(target);
    rest_on_desired(rig, target, sampler);
    rig.state.blend = None;
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
    let Some(sampler) = sampler else {
        rig.state.last_target = Some(target);
        return;
    };

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
fn blended_pose(rig: &mut CameraRig, input: &RigInput) -> Pose {
    let state = &mut rig.state;
    let live = Pose {
        eye: state.eye,
        yaw_deg: state.yaw_deg,
        pitch_deg: state.pitch_deg,
        fov_y_deg: rig.rig.fov_y_deg,
    };
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
    }
}
