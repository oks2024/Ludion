//! The pose math of platform carry: pure functions over a platform's pose before and after a tick.

use ludion_core::math::{atan2, sin_cos};

/// The yaw about +Y, in radians, that turns `before` into `after` (both `[x, y, z, w]` unit
/// quaternions): the twist of `after * conj(before)` about the vertical axis, with pitch and roll
/// dropped. In `(-pi, pi]`.
pub(crate) fn yaw_delta(before: [f32; 4], after: [f32; 4]) -> f32 {
    let [bx, by, bz, bw] = before;
    let [ax, ay, az, aw] = after;
    // after * conj(before)
    let (cx, cy, cz, cw) = (-bx, -by, -bz, bw);
    let mut y = aw * cy - ax * cz + ay * cw + az * cx;
    let mut w = aw * cw - ax * cx - ay * cy - az * cz;
    if w < 0.0 {
        y = -y;
        w = -w;
    }
    let yaw = 2.0 * atan2(y, w);
    if yaw <= -core::f32::consts::PI {
        core::f32::consts::PI
    } else {
        yaw
    }
}

/// How a rider at `feet` moves when its platform goes from pose `(t0, q0)` to `(t1, q1)`: the
/// translation of its feet and the yaw it turns by, as if rigidly fixed to the platform (only the
/// yaw part of the rotation, about the platform's pivot). Identical poses give exact zeros, so a
/// static floor never drifts a rider.
pub(crate) fn rigid_carry(
    feet: [f32; 3],
    t0: [f32; 3],
    q0: [f32; 4],
    t1: [f32; 3],
    q1: [f32; 4],
) -> ([f32; 3], f32) {
    if t0 == t1 && q0 == q1 {
        return ([0.0; 3], 0.0);
    }
    let yaw = yaw_delta(q0, q1);
    let (sin, cos) = sin_cos(yaw);
    let (dx, dy, dz) = (feet[0] - t0[0], feet[1] - t0[1], feet[2] - t0[2]);
    let carried = [
        t1[0] + dx * cos + dz * sin,
        t1[1] + dy,
        t1[2] - dx * sin + dz * cos,
    ];
    (
        [
            carried[0] - feet[0],
            carried[1] - feet[1],
            carried[2] - feet[2],
        ],
        yaw,
    )
}

/// `velocity` turned by the unit quaternion `rotation` (`[x, y, z, w]`).
pub(crate) fn rotate(rotation: [f32; 4], velocity: [f32; 3]) -> [f32; 3] {
    let [x, y, z, w] = rotation;
    let u = [x, y, z];
    let t = cross(u, velocity);
    let t = [2.0 * t[0], 2.0 * t[1], 2.0 * t[2]];
    let ut = cross(u, t);
    [
        velocity[0] + w * t[0] + ut[0],
        velocity[1] + w * t[1] + ut[1],
        velocity[2] + w * t[2] + ut[2],
    ]
}

/// `rider` turned by `yaw` radians about the world +Y, as `[x, y, z, w]`. Unchanged when `yaw` is
/// zero.
pub(crate) fn rotate_yaw(rider: [f32; 4], yaw: f32) -> [f32; 4] {
    if yaw == 0.0 {
        return rider;
    }
    let (sin, cos) = sin_cos(yaw / 2.0);
    let [x, y, z, w] = rider;
    // (0, sin, 0, cos) * rider
    [
        cos * x + sin * z,
        cos * y + sin * w,
        cos * z - sin * x,
        cos * w - sin * y,
    ]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    fn yaw_quat(yaw: f32) -> [f32; 4] {
        let (sin, cos) = sin_cos(yaw / 2.0);
        [0.0, sin, 0.0, cos]
    }

    #[test]
    fn identical_poses_carry_exact_zero() {
        let pose = ([1.0, 2.0, 3.0], yaw_quat(0.7));
        let (translation, yaw) = rigid_carry([4.0, 5.0, 6.0], pose.0, pose.1, pose.0, pose.1);
        assert_eq!(translation.map(f32::to_bits), [0.0f32.to_bits(); 3]);
        assert_eq!(yaw.to_bits(), 0.0f32.to_bits());
    }

    #[test]
    fn a_quarter_turn_about_the_pivot() {
        let half_pi = core::f32::consts::FRAC_PI_2;
        let (translation, yaw) = rigid_carry(
            [1.0, 0.0, 0.0],
            [0.0; 3],
            IDENTITY,
            [0.0; 3],
            yaw_quat(half_pi),
        );
        assert!((yaw - half_pi).abs() < 1e-6, "{yaw}");
        // Ry(90°) maps +X to -Z.
        let feet = [1.0 + translation[0], translation[1], translation[2]];
        assert!(
            feet[0].abs() < 1e-6 && (feet[2] + 1.0).abs() < 1e-6,
            "{feet:?}"
        );
    }

    #[test]
    fn a_pitched_delta_has_no_yaw() {
        let (sin, cos) = sin_cos(0.4);
        assert_eq!(yaw_delta(IDENTITY, [sin, 0.0, 0.0, cos]), 0.0);
        assert_eq!(yaw_delta(IDENTITY, [0.0, 0.0, sin, cos]), 0.0);
    }

    #[test]
    fn yaw_wraps_into_minus_pi_pi() {
        for yaw in [-3.0, -1.0, 0.5, 3.0] {
            let found = yaw_delta(yaw_quat(1.0), yaw_quat(1.0 + yaw));
            let wrapped = (yaw + core::f32::consts::PI).rem_euclid(2.0 * core::f32::consts::PI)
                - core::f32::consts::PI;
            assert!(
                (found - wrapped).abs() < 1e-5,
                "{yaw}: {found} vs {wrapped}"
            );
            assert!(found > -core::f32::consts::PI && found <= core::f32::consts::PI);
        }
        // The shorter way round: the negated quaternion is the same rotation.
        let q = yaw_quat(0.3);
        assert!((yaw_delta(IDENTITY, [-q[0], -q[1], -q[2], -q[3]]) - 0.3).abs() < 1e-6);
    }

    #[test]
    fn a_translated_and_yawed_carry_maps_a_point_exactly() {
        let (t0, q0) = ([1.0, 0.5, -2.0], yaw_quat(0.4));
        let (t1, q1) = ([1.5, 0.7, -1.0], yaw_quat(0.9));
        let local = [0.8, 0.3, -0.6];
        let to_world = |t: [f32; 3], q: [f32; 4]| {
            let r = rotate(q, local);
            [t[0] + r[0], t[1] + r[1], t[2] + r[2]]
        };
        let feet = to_world(t0, q0);
        let (translation, yaw) = rigid_carry(feet, t0, q0, t1, q1);
        assert!((yaw - 0.5).abs() < 1e-6);
        let carried = [
            feet[0] + translation[0],
            feet[1] + translation[1],
            feet[2] + translation[2],
        ];
        let expected = to_world(t1, q1);
        for axis in 0..3 {
            assert!((carried[axis] - expected[axis]).abs() <= 1e-6, "{axis}");
        }
    }

    #[test]
    fn rotate_yaw_composes_with_the_rider() {
        let turned = rotate_yaw(yaw_quat(0.3), 0.5);
        assert!((yaw_delta(IDENTITY, turned) - 0.8).abs() < 1e-6);
        assert_eq!(rotate_yaw(yaw_quat(0.3), 0.0), yaw_quat(0.3));
    }
}
