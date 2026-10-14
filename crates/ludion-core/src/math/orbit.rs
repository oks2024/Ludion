//! Orbit framing: a camera placed by a target, two angles and a distance.

use super::{
    asin, atan2, cos, euler_xyz_degrees_to_quat, quat_mul, quat_rotate, quat_to_euler_xyz_degrees,
    sin,
};

const DEG_TO_RAD: f32 = core::f32::consts::PI / 180.0;
const RAD_TO_DEG: f32 = 180.0 / core::f32::consts::PI;

/// A camera orbiting `target`: azimuth is degrees about +Y measured from +Z (0 puts the eye on the
/// target's +Z side, looking −Z), pitch is degrees *down* (positive looks down, valid `[-90, 90]`),
/// no roll. `ludion render --look-at` is built on it, and a follow-camera system can use the same
/// value to write its `Transform` and `PostProcess.focus_distance`.
///
/// # Example
///
/// ```
/// use ludion_core::math::Orbit;
/// let orbit = Orbit { target: [0.0, 1.0, 0.0], azimuth_deg: 0.0, pitch_deg: 58.0, distance: 30.0 };
/// let eye = orbit.eye();
/// assert!((eye[1] - 26.44).abs() < 1e-2 && (eye[2] - 15.9).abs() < 1e-2, "{eye:?}");
/// let (azimuth, pitch) = Orbit::angles_of(orbit.rotation_deg());
/// assert!(azimuth.abs() < 1e-3 && (pitch - 58.0).abs() < 1e-3);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Orbit {
    /// The point the camera looks at and focuses on.
    pub target: [f32; 3],
    /// Degrees about +Y from +Z.
    pub azimuth_deg: f32,
    /// Degrees looking down.
    pub pitch_deg: f32,
    /// Metres from the eye to the target.
    pub distance: f32,
}

impl Orbit {
    /// The camera position: `target + distance · (cos p·sin az, sin p, cos p·cos az)`.
    pub fn eye(&self) -> [f32; 3] {
        let (az, pitch) = (self.azimuth_deg * DEG_TO_RAD, self.pitch_deg * DEG_TO_RAD);
        let (cp, sp) = (cos(pitch), sin(pitch));
        [
            self.target[0] + self.distance * (cp * sin(az)),
            self.target[1] + self.distance * sp,
            self.target[2] + self.distance * (cp * cos(az)),
        ]
    }

    /// The camera's `Transform.rotation` (Euler XYZ degrees): yaw by the azimuth, then pitch down.
    pub fn rotation_deg(&self) -> [f32; 3] {
        let (az, pitch) = (
            self.azimuth_deg * DEG_TO_RAD * 0.5,
            self.pitch_deg * DEG_TO_RAD * 0.5,
        );
        let yaw = [0.0, sin(az), 0.0, cos(az)];
        let down = [sin(-pitch), 0.0, 0.0, cos(pitch)];
        quat_to_euler_xyz_degrees(quat_mul(yaw, down))
    }

    /// The `(azimuth_deg, pitch_deg)` a camera with this Euler rotation looks along; roll is ignored.
    pub fn angles_of(rotation_deg: [f32; 3]) -> (f32, f32) {
        let f = quat_rotate(euler_xyz_degrees_to_quat(rotation_deg), [0.0, 0.0, -1.0]);
        (atan2(-f[0], -f[2]) * RAD_TO_DEG, asin(-f[1]) * RAD_TO_DEG)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn orbit(azimuth_deg: f32, pitch_deg: f32) -> Orbit {
        Orbit {
            target: [1.0, 2.0, 3.0],
            azimuth_deg,
            pitch_deg,
            distance: 10.0,
        }
    }

    #[test]
    fn eye_follows_the_azimuth() {
        for (az, offset) in [
            (0.0, [0.0, 0.0, 10.0]),
            (90.0, [10.0, 0.0, 0.0]),
            (180.0, [0.0, 0.0, -10.0]),
        ] {
            let eye = orbit(az, 0.0).eye();
            for axis in 0..3 {
                assert!(
                    (eye[axis] - ([1.0, 2.0, 3.0][axis] + offset[axis])).abs() < 1e-4,
                    "az {az}: {eye:?}"
                );
            }
        }
    }

    #[test]
    fn angles_round_trip() {
        let mut az = -170.0;
        while az <= 180.0 {
            let mut pitch = -85.0;
            while pitch <= 85.0 {
                let (a, p) = Orbit::angles_of(orbit(az, pitch).rotation_deg());
                let wrapped = (a - az + 180.0).rem_euclid(360.0) - 180.0;
                assert!(
                    wrapped.abs() < 1e-2 && (p - pitch).abs() < 1e-2,
                    "{az}/{pitch} -> {a}/{p}"
                );
                pitch += 17.0;
            }
            az += 35.0;
        }
    }

    #[test]
    fn looking_straight_down_is_finite() {
        let rotation = orbit(30.0, 90.0).rotation_deg();
        assert!(rotation.iter().all(|v| v.is_finite()), "{rotation:?}");
        let f = quat_rotate(euler_xyz_degrees_to_quat(rotation), [0.0, 0.0, -1.0]);
        assert!((f[1] + 1.0).abs() < 1e-4, "{f:?}");
    }

    #[test]
    fn the_camera_looks_at_the_target() {
        let o = orbit(-40.0, 33.0);
        let (eye, f) = (
            o.eye(),
            quat_rotate(
                euler_xyz_degrees_to_quat(o.rotation_deg()),
                [0.0, 0.0, -1.0],
            ),
        );
        for axis in 0..3 {
            assert!((eye[axis] + f[axis] * o.distance - o.target[axis]).abs() < 1e-3);
        }
    }
}
