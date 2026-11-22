//! Camera-relative stick mapping.

use ludion_core::math::sin_cos;

use crate::rig::{CameraRig, RigMode};

const DEG_TO_RAD: f32 = core::f32::consts::PI / 180.0;

/// The world-space XZ direction for a stick `[x, y]` (`y` pushes away from the camera) mapped
/// through `yaw_deg`. Magnitude is preserved and `[0, 0]` gives `[0, 0, 0]`. Pure; use it to map
/// against a yaw the game latched.
///
/// # Example
///
/// ```
/// let forward = ludion_camera::camera_relative_yaw([0.0, 1.0], 0.0);
/// assert_eq!(forward, [0.0, 0.0, -1.0]);
/// ```
pub fn camera_relative_yaw(stick: [f32; 2], yaw_deg: f32) -> [f32; 3] {
    let (s, c) = sin_cos(yaw_deg * DEG_TO_RAD);
    let (right, forward) = ([c, -s], [-s, -c]);
    [
        stick[0] * right[0] + stick[1] * forward[0],
        0.0,
        stick[0] * right[1] + stick[1] * forward[1],
    ]
}

/// [`camera_relative_yaw`] at the rig's last posed yaw ([`CameraRig::yaw_deg`]). The rig runs after
/// game systems, so a system reads the yaw posed on the previous tick; the yaw is taken before
/// shake, so a shake never twists the controls.
///
/// While a `chase` or `follow` rig has a nonzero applied framing weight, the stick maps through the
/// unframed orbit yaw (`state.yaw_deg`) instead: a corner peek never turns the controls.
pub fn camera_relative(stick: [f32; 2], rig: &CameraRig) -> [f32; 3] {
    let framed = rig.state.framing_weight != 0.0
        && matches!(rig.rig.mode, RigMode::Chase(_) | RigMode::Follow(_));
    camera_relative_yaw(
        stick,
        if framed {
            rig.state.yaw_deg
        } else {
            rig.yaw_deg()
        },
    )
}
