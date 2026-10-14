//! The `f64` twins of [`super::euler_xyz_degrees_to_quat`] and [`super::quat_to_euler_xyz_degrees`],
//! used by clip sampling and clip import, which work in double precision. The groupings are the
//! bits: do not rewrite them in the `f32` pair's style.

use super::f64::{atan2, cos, sin, sqrt};
use super::glam::{DMat3, DQuat};

/// A quaternion `[x, y, z, w]` from Euler XYZ **degrees** — the `Transform` convention, equal to
/// glam's `EulerRot::XYZ` (`qx * qy * qz`) — built from half-angle axis quaternions on the libm
/// `sin`/`cos`, so the bits agree on every OS.
pub fn euler_xyz_degrees_to_quat(e: [f64; 3]) -> [f64; 4] {
    let axis = |deg: f64, axis: usize| {
        let angle = deg.to_radians() * 0.5;
        let mut q = [0.0, 0.0, 0.0, cos(angle)];
        q[axis] = sin(angle);
        q
    };
    quat_mul(quat_mul(axis(e[0], 0), axis(e[1], 1)), axis(e[2], 2))
}

/// Hamilton product `a * b` of two `[x, y, z, w]` quaternions.
fn quat_mul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

/// A quaternion as Euler XYZ degrees: glam's algorithm, gimbal-lock branch included, on libm
/// `atan2`.
pub fn quat_to_euler_xyz_degrees(q: [f64; 4]) -> [f64; 3] {
    let m = DMat3::from_quat(DQuat::from_xyzw(q[0], q[1], q[2], q[3]));
    let cy = sqrt(m.x_axis.x * m.x_axis.x + m.y_axis.x * m.y_axis.x);
    let (x, y, z) = if cy > 16.0 * f64::EPSILON {
        (
            atan2(m.z_axis.y, m.z_axis.z),
            atan2(-m.z_axis.x, cy),
            atan2(m.y_axis.x, m.x_axis.x),
        )
    } else {
        (atan2(-m.y_axis.z, m.y_axis.y), atan2(-m.z_axis.x, cy), 0.0)
    };
    // glam negates parity-even orders.
    [(-x).to_degrees(), (-y).to_degrees(), (-z).to_degrees()]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The quaternion builder is glam's `EulerRot::XYZ` — the engine's convention — to rounding.
    #[test]
    fn euler_to_quat_matches_the_engines_convention() {
        use crate::math::glam::EulerRot;
        for e in [
            [0.0, 0.0, 0.0],
            [30.0, 0.0, 0.0],
            [10.0, 20.0, 30.0],
            [-170.0, 45.0, 99.0],
            [90.0, -90.0, 180.0],
            [359.0, -270.0, 12.5],
        ] {
            let got = euler_xyz_degrees_to_quat(e);
            let want = DQuat::from_euler(
                EulerRot::XYZ,
                e[0].to_radians(),
                e[1].to_radians(),
                e[2].to_radians(),
            );
            let want = [want.x, want.y, want.z, want.w];
            for i in 0..4 {
                assert!(
                    (got[i] - want[i]).abs() < 1e-12,
                    "{e:?}: {got:?} vs {want:?}"
                );
            }
        }
    }

    #[test]
    fn round_trips_away_from_gimbal_lock() {
        for degrees in [[12.0, 34.0, 56.0], [-20.0, 15.0, -70.0], [0.0, 0.0, 0.0]] {
            let back = quat_to_euler_xyz_degrees(euler_xyz_degrees_to_quat(degrees));
            for axis in 0..3 {
                assert!(
                    (back[axis] - degrees[axis]).abs() < 1e-9,
                    "{degrees:?} -> {back:?}"
                );
            }
        }
    }
}
