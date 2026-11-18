//! The `chase` and `follow` pose: the eye on a boom from the sprung pivot, yaw and pitch unsprung.

use ludion_core::math::Orbit;

use crate::rig::{ChaseRig, RigState};
use crate::spring::wrap180;

/// Applies a `[yaw, pitch]` look delta to the state's yaw and pitch: yaw wrapped, pitch moved only
/// with `look_pitch` and clamped to the rig's range. A zero delta still clamps, which covers
/// `set_look` and a tightened range.
pub(crate) fn chase_look(state: &mut RigState, chase: &ChaseRig, look: [f32; 2]) {
    (state.yaw_deg, state.pitch_deg) = chased(chase, state.yaw_deg, state.pitch_deg, look);
}

/// `(yaw, pitch)` after a `[yaw, pitch]` look delta on a `chase` rig.
pub(crate) fn chased(chase: &ChaseRig, yaw_deg: f32, pitch_deg: f32, look: [f32; 2]) -> (f32, f32) {
    let pitch = if chase.look_pitch {
        pitch_deg + look[1]
    } else {
        pitch_deg
    };
    (wrap180(yaw_deg + look[0]), clamp_pitch(chase, pitch))
}

/// `pitch_deg` inside the chase range (`min` wins over a reversed range, which validation rejects).
pub(crate) fn clamp_pitch(chase: &ChaseRig, pitch_deg: f32) -> f32 {
    pitch_deg.min(chase.pitch_max_deg).max(chase.pitch_min_deg)
}

/// The eye of a `chase` or `follow` rig: `distance` metres from `pivot` along the orbit angles
/// (yaw 0 puts the eye on the pivot's +Z side, positive pitch raises it).
///
/// # Example
///
/// ```
/// use ludion_camera::orbit_eye;
///
/// let eye = orbit_eye([1.0, 0.0, -4.0], 0.0, 0.0, 6.0);
/// assert!((eye[0] - 1.0).abs() < 1e-5 && eye[1].abs() < 1e-5 && (eye[2] - 2.0).abs() < 1e-5);
/// ```
#[must_use]
pub fn orbit_eye(pivot: [f32; 3], yaw_deg: f32, pitch_deg: f32, distance: f32) -> [f32; 3] {
    Orbit {
        target: pivot,
        azimuth_deg: yaw_deg,
        pitch_deg,
        distance,
    }
    .eye()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_limits_fix_the_pitch() {
        let chase = ChaseRig {
            pitch_min_deg: 25.0,
            pitch_max_deg: 25.0,
            ..ChaseRig::default()
        };
        let mut state = RigState::default();
        chase_look(&mut state, &chase, [0.0, 40.0]);
        assert_eq!(state.pitch_deg, 25.0);
        chase_look(&mut state, &chase, [0.0, -90.0]);
        assert_eq!(state.pitch_deg, 25.0);
    }

    #[test]
    fn yaw_wraps_across_the_seam() {
        let mut state = RigState {
            yaw_deg: 170.0,
            ..RigState::default()
        };
        chase_look(&mut state, &ChaseRig::default(), [20.0, 0.0]);
        assert!((state.yaw_deg + 170.0).abs() < 1e-4);
    }

    #[test]
    fn without_look_pitch_the_pitch_delta_is_dropped() {
        let chase = ChaseRig {
            look_pitch: false,
            ..ChaseRig::default()
        };
        let mut state = RigState {
            pitch_deg: 20.0,
            ..RigState::default()
        };
        chase_look(&mut state, &chase, [5.0, 30.0]);
        assert_eq!((state.yaw_deg, state.pitch_deg), (5.0, 20.0));
    }

    #[test]
    fn a_zero_look_still_clamps() {
        let mut state = RigState {
            pitch_deg: 80.0,
            ..RigState::default()
        };
        chase_look(&mut state, &ChaseRig::default(), [0.0; 2]);
        assert_eq!(state.pitch_deg, 70.0);
    }
}
