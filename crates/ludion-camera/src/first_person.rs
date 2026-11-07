//! The `first_person` pose: the eye rigidly on the target, yaw and pitch from look deltas.

use ludion_core::math::sin_cos;

use crate::rig::{FirstPersonRig, RigState};
use crate::spring::wrap180;

const DEG_TO_RAD: f32 = core::f32::consts::PI / 180.0;

/// Applies a `[yaw, pitch]` look delta to the state's yaw and pitch: yaw wrapped, pitch clamped to
/// the mode's limit. A zero delta still clamps, which covers `set_look` and a tightened limit.
pub(crate) fn apply_look(state: &mut RigState, fp: &FirstPersonRig, look: [f32; 2]) {
    state.yaw_deg = wrap180(state.yaw_deg + look[0]);
    let limit = fp.pitch_limit_deg;
    state.pitch_deg = (state.pitch_deg + look[1]).clamp(-limit, limit);
}

/// The eye a `first_person` rig wants: `target` plus `fp.eye_offset` in the yaw frame (right, up,
/// back).
///
/// # Example
///
/// ```
/// use ludion_camera::{FirstPersonRig, first_person_eye};
///
/// let eye = first_person_eye(&FirstPersonRig::default(), [1.0, 0.0, -4.0], 0.0);
/// assert!((eye[0] - 1.0).abs() < 1e-5 && (eye[1] - 1.6).abs() < 1e-5 && (eye[2] + 4.0).abs() < 1e-5);
/// ```
#[must_use]
pub fn first_person_eye(fp: &FirstPersonRig, target: [f32; 3], yaw_deg: f32) -> [f32; 3] {
    let (s, c) = sin_cos(yaw_deg * DEG_TO_RAD);
    let [ox, oy, oz] = fp.eye_offset;
    [
        target[0] + ox * c + oz * s,
        target[1] + oy,
        target[2] - ox * s + oz * c,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_clamps_at_the_limit_either_way() {
        let fp = FirstPersonRig::default();
        let mut state = RigState::default();
        apply_look(&mut state, &fp, [0.0, 500.0]);
        assert_eq!(state.pitch_deg, 89.0);
        apply_look(&mut state, &fp, [0.0, -1000.0]);
        assert_eq!(state.pitch_deg, -89.0);
    }

    #[test]
    fn yaw_wraps_across_the_seam() {
        let fp = FirstPersonRig::default();
        let mut state = RigState {
            yaw_deg: 170.0,
            ..RigState::default()
        };
        apply_look(&mut state, &fp, [20.0, 0.0]);
        assert!((state.yaw_deg + 170.0).abs() < 1e-4);
    }

    #[test]
    fn a_zero_look_still_clamps() {
        let fp = FirstPersonRig {
            pitch_limit_deg: 30.0,
            ..FirstPersonRig::default()
        };
        let mut state = RigState {
            pitch_deg: 80.0,
            ..RigState::default()
        };
        apply_look(&mut state, &fp, [0.0; 2]);
        assert_eq!(state.pitch_deg, 30.0);
    }

    #[test]
    fn the_offset_turns_with_yaw() {
        let fp = FirstPersonRig {
            eye_offset: [1.0, 0.0, 0.0],
            ..FirstPersonRig::default()
        };
        // Yaw 90 turns left: the right vector points to -Z.
        let eye = first_person_eye(&fp, [0.0; 3], 90.0);
        assert!(eye[0].abs() < 1e-5 && (eye[2] + 1.0).abs() < 1e-5);
    }
}
