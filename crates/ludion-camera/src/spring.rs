//! Springs, dampers and the angle helpers the stages share.

use ludion_core::math::{atan2, exp, exp2, sqrt};

use crate::rig::{BlendCurve, Pose};

const RAD_TO_DEG: f32 = 180.0 / core::f32::consts::PI;

/// The root of `(1 + u)·e^-u = 0.5`: a critically damped spring from rest has halved its error
/// once `u = HALF_LIFE_U · t / half_life` reaches it.
pub(crate) const HALF_LIFE_U: f32 = 1.678_347;

/// Steps a critically damped spring one `dt` (closed form, exact for any `dt`) and returns the new
/// `(position, velocity)`. A `half_life` of `0` is rigid.
pub(crate) fn spring_step(x: f32, v: f32, goal: f32, half_life: f32, dt: f32) -> (f32, f32) {
    if half_life <= 0.0 {
        return (goal, 0.0);
    }
    let y = HALF_LIFE_U / half_life;
    let j0 = x - goal;
    let j1 = v + j0 * y;
    let e = exp(-y * dt);
    (goal + e * (j0 + j1 * dt), e * (v - j1 * y * dt))
}

/// Moves `x` toward `goal` so the remaining error halves every `half_life` seconds; `0` is rigid.
pub(crate) fn damp(x: f32, goal: f32, half_life: f32, dt: f32) -> f32 {
    if half_life <= 0.0 {
        return goal;
    }
    x + (goal - x) * (1.0 - exp2(-dt / half_life))
}

/// Wraps degrees into `(-180, 180]`.
pub(crate) fn wrap180(degrees: f32) -> f32 {
    degrees - 360.0 * ((degrees - 180.0) / 360.0).ceil()
}

/// The `(yaw, pitch)` in degrees of looking from `eye` to `focus`, [`ludion_core::math::Orbit`]
/// convention. `None` when they coincide; a vertical look keeps `last_yaw`.
pub(crate) fn look_angles(eye: [f32; 3], focus: [f32; 3], last_yaw: f32) -> Option<(f32, f32)> {
    let f = [focus[0] - eye[0], focus[1] - eye[1], focus[2] - eye[2]];
    if f[0] * f[0] + f[1] * f[1] + f[2] * f[2] < 1e-12 {
        return None;
    }
    let horizontal = sqrt(f[0] * f[0] + f[2] * f[2]);
    let yaw = if horizontal < 1e-6 {
        last_yaw
    } else {
        atan2(-f[0], -f[2]) * RAD_TO_DEG
    };
    Some((yaw, atan2(-f[1], horizontal) * RAD_TO_DEG))
}

/// The blend weight for progress `t` in `[0, 1]`.
pub(crate) fn ease(curve: BlendCurve, t: f32) -> f32 {
    match curve {
        BlendCurve::Linear => t,
        BlendCurve::EaseInOut => t * t * (3.0 - 2.0 * t),
    }
}

/// `from` at `w = 0`, `to` at `w = 1`: the eye by lerp, the yaw along the shortest arc, the field of
/// view by lerp when both set one (else `to`'s).
pub(crate) fn lerp_pose(from: &Pose, to: &Pose, w: f32) -> Pose {
    let lerp = |a: f32, b: f32| a + (b - a) * w;
    Pose {
        eye: [
            lerp(from.eye[0], to.eye[0]),
            lerp(from.eye[1], to.eye[1]),
            lerp(from.eye[2], to.eye[2]),
        ],
        yaw_deg: wrap180(from.yaw_deg + wrap180(to.yaw_deg - from.yaw_deg) * w),
        pitch_deg: lerp(from.pitch_deg, to.pitch_deg),
        fov_y_deg: match (from.fov_y_deg, to.fov_y_deg) {
            (Some(a), Some(b)) => Some(lerp(a, b)),
            (_, b) => b,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(yaw_deg: f32, fov_y_deg: Option<f32>) -> Pose {
        Pose {
            yaw_deg,
            fov_y_deg,
            ..Pose::default()
        }
    }

    #[test]
    fn half_life_constant_solves_its_equation() {
        let u = f64::from(HALF_LIFE_U);
        assert!(((1.0 + u) * ludion_core::math::f64::exp(-u) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_spring_halves_its_error_each_half_life_from_rest() {
        let (mut x, mut v) = (1.0, 0.0);
        for _ in 0..60 {
            (x, v) = spring_step(x, v, 0.0, 1.0, 1.0 / 60.0);
        }
        assert!((x - 0.5).abs() < 1e-3, "{x} (v {v})");
        let (one_step, _) = spring_step(1.0, 0.0, 0.0, 1.0, 1.0);
        assert!(
            (one_step - x).abs() < 1e-3,
            "exact for any dt: {one_step} vs {x}"
        );
    }

    #[test]
    fn a_zero_half_life_is_rigid() {
        assert_eq!(spring_step(3.0, 5.0, 1.0, 0.0, 0.016), (1.0, 0.0));
        assert_eq!(damp(3.0, 1.0, 0.0, 0.016), 1.0);
    }

    #[test]
    fn a_damper_halves_its_error_per_half_life() {
        let mut x = 1.0;
        for _ in 0..30 {
            x = damp(x, 0.0, 0.5, 1.0 / 60.0);
        }
        assert!((x - 0.5).abs() < 1e-4, "{x}");
    }

    #[test]
    fn wrap180_lands_in_the_half_open_range() {
        assert_eq!(wrap180(180.0), 180.0);
        assert_eq!(wrap180(-180.0), 180.0);
        assert_eq!(wrap180(190.0), -170.0);
        assert_eq!(wrap180(-190.0), 170.0);
        assert_eq!(wrap180(540.0), 180.0);
        assert_eq!(wrap180(45.0), 45.0);
    }

    #[test]
    fn look_angles_follow_the_orbit_convention() {
        let (yaw, pitch) = look_angles([0.0; 3], [0.0, 0.0, -1.0], 7.0).unwrap();
        assert!(yaw.abs() < 1e-4 && pitch.abs() < 1e-4);
        let (yaw, pitch) = look_angles([0.0; 3], [-1.0, -1.0, 0.0], 7.0).unwrap();
        assert!((yaw - 90.0).abs() < 1e-3 && (pitch - 45.0).abs() < 1e-3);
        assert_eq!(look_angles([1.0; 3], [1.0; 3], 0.0), None);
        let (yaw, pitch) = look_angles([0.0; 3], [0.0, -2.0, 0.0], 33.0).unwrap();
        assert_eq!((yaw, pitch), (33.0, 90.0));
    }

    #[test]
    fn lerp_pose_takes_the_short_way_round_the_seam() {
        let (a, b) = (pose(170.0, None), pose(-170.0, None));
        assert_eq!(lerp_pose(&a, &b, 0.0).yaw_deg, 170.0);
        assert_eq!(lerp_pose(&a, &b, 0.5).yaw_deg, 180.0);
        let mid = lerp_pose(&pose(0.0, Some(40.0)), &pose(0.0, Some(60.0)), 0.5);
        assert_eq!(mid.fov_y_deg, Some(50.0));
        let start = lerp_pose(&pose(0.0, None), &pose(0.0, Some(60.0)), 0.0);
        assert_eq!(start.fov_y_deg, Some(60.0));
    }

    #[test]
    fn ease_pins_both_ends() {
        for curve in [BlendCurve::Linear, BlendCurve::EaseInOut] {
            assert_eq!((ease(curve, 0.0), ease(curve, 1.0)), (0.0, 1.0));
        }
        assert_eq!(ease(BlendCurve::EaseInOut, 0.5), 0.5);
    }
}
