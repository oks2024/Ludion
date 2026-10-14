//! The engine's rotation convention as scalar `f32` arithmetic: quaternion product and rotation,
//! and the Euler XYZ degrees ↔ quaternion pair.
//!
//! Everything here is written out component by component in a fixed order — no `glam` `Quat` or
//! `Mat3` arithmetic — so the order is pinned by this crate's bit fixture instead of inherited from
//! whichever glam backend (SSE2, NEON, scalar) a target compiles. The transcendentals go through
//! this module's libm wrappers.

use super::{atan2, cos, sin, sqrt};

/// Degrees to radians, as `f32::to_radians` computes it — a plain multiply, no transcendental.
const DEG_TO_RAD: f32 = core::f32::consts::PI / 180.0;

/// The Hamilton product `l * r` of two quaternions `[x, y, z, w]` (apply `r`, then `l`).
///
/// The grouping `(t0 + t1) + (t2 + t3)` is glam's own SSE2 and NEON order, so the result is
/// bit-identical to `Quat * Quat` on both; it is part of the contract, do not regroup.
///
/// # Example
/// ```
/// use ludion_core::math::quat_mul;
/// let identity = [0.0, 0.0, 0.0, 1.0];
/// let q = [0.5, 0.5, 0.5, 0.5];
/// assert_eq!(quat_mul(identity, q), q);
/// ```
pub fn quat_mul(l: [f32; 4], r: [f32; 4]) -> [f32; 4] {
    let [lx, ly, lz, lw] = l;
    let [rx, ry, rz, rw] = r;
    [
        (lw * rx + lx * rw) + (ly * rz - lz * ry),
        (lw * ry - lx * rz) + (ly * rw + lz * rx),
        (lw * rz + lx * ry) + (lz * rw - ly * rx),
        (lw * rw - lx * rx) + (-(ly * ry) - lz * rz),
    ]
}

/// Rotates `v` by the **unit** quaternion `q` `[x, y, z, w]`.
///
/// Fixed order: `t = 2·(q.xyz × v)`, `v' = (v + w·t) + (q.xyz × t)`.
///
/// # Example
/// ```
/// use ludion_core::math::{euler_xyz_degrees_to_quat, quat_rotate};
/// let q = euler_xyz_degrees_to_quat([0.0, 90.0, 0.0]);
/// let v = quat_rotate(q, [1.0, 0.0, 0.0]);
/// assert!((v[2] + 1.0).abs() < 1e-6 && v[0].abs() < 1e-6, "{v:?}");
/// ```
pub fn quat_rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let [qx, qy, qz, qw] = q;
    let t = [
        (qy * v[2] - qz * v[1]) * 2.0,
        (qz * v[0] - qx * v[2]) * 2.0,
        (qx * v[1] - qy * v[0]) * 2.0,
    ];
    let c = [
        qy * t[2] - qz * t[1],
        qz * t[0] - qx * t[2],
        qx * t[1] - qy * t[0],
    ];
    [
        (v[0] + qw * t[0]) + c[0],
        (v[1] + qw * t[1]) + c[1],
        (v[2] + qw * t[2]) + c[2],
    ]
}

/// The rotation matrix of the unit quaternion `q`, as columns `[x_axis, y_axis, z_axis]`, with
/// glam 0.30's `Mat3::from_quat` groupings.
pub(super) fn quat_to_mat3(q: [f32; 4]) -> [[f32; 3]; 3] {
    let [x, y, z, w] = q;
    let x2 = x + x;
    let y2 = y + y;
    let z2 = z + z;
    let xx = x * x2;
    let xy = x * y2;
    let xz = x * z2;
    let yy = y * y2;
    let yz = y * z2;
    let zz = z * z2;
    let wx = w * x2;
    let wy = w * y2;
    let wz = w * z2;
    [
        [1.0 - (yy + zz), xy + wz, xz - wy],
        [xy - wz, 1.0 - (xx + zz), yz + wx],
        [xz + wy, yz - wx, 1.0 - (xx + yy)],
    ]
}

/// A quaternion `[x, y, z, w]` from the engine's rotation convention: **Euler XYZ angles in
/// degrees, intrinsic** — the product `qx · qy · qz` of the three per-axis rotations, which is what
/// `Quat::from_euler(EulerRot::XYZ, …)` computes. Every rendered model matrix comes from this pair
/// (`extract`'s `rotation_quat`).
///
/// Uses the pure-Rust libm `sin`/`cos`, never glam's `Quat::from_euler`, which bottoms out in the
/// platform C library: a last-bit difference there would be a cross-arch hash divergence. The
/// mirror image of [`quat_to_euler_xyz_degrees`].
///
/// # Example
/// ```
/// use ludion_core::math::euler_xyz_degrees_to_quat;
/// assert_eq!(euler_xyz_degrees_to_quat([0.0, 0.0, 0.0]), [0.0, 0.0, 0.0, 1.0]);
/// ```
pub fn euler_xyz_degrees_to_quat(euler_degrees: [f32; 3]) -> [f32; 4] {
    let hx = euler_degrees[0] * DEG_TO_RAD * 0.5;
    let hy = euler_degrees[1] * DEG_TO_RAD * 0.5;
    let hz = euler_degrees[2] * DEG_TO_RAD * 0.5;
    let qx = [sin(hx), 0.0, 0.0, cos(hx)];
    let qy = [0.0, sin(hy), 0.0, cos(hy)];
    let qz = [0.0, 0.0, sin(hz), cos(hz)];
    quat_mul(quat_mul(qx, qy), qz)
}

/// Euler XYZ degrees from a quaternion `[x, y, z, w]` — the inverse of
/// [`euler_xyz_degrees_to_quat`], and what a pose component storing Euler angles writes back when
/// the mirror hands it the solver's rotation.
///
/// Mirrors glam 0.30's `Quat::to_euler(EulerRot::XYZ)` algorithm — gimbal-lock branch included —
/// with the transcendentals routed through libm for the same cross-arch reason. The
/// input is normalized first, by hand rather than through `Quat::normalize`, because glam's SSE2
/// backend divides by the length where its NEON backend multiplies by the reciprocal.
///
/// Total: a zero-length input normalizes to non-finite components and yields whatever `atan2` makes
/// of them, rather than panicking.
///
/// # Example
/// ```
/// use ludion_core::math::{euler_xyz_degrees_to_quat, quat_to_euler_xyz_degrees};
/// let back = quat_to_euler_xyz_degrees(euler_xyz_degrees_to_quat([30.0, 0.0, 0.0]));
/// assert!((back[0] - 30.0).abs() < 1e-3, "{back:?}");
/// ```
pub fn quat_to_euler_xyz_degrees(quat: [f32; 4]) -> [f32; 3] {
    let len = sqrt(quat[0] * quat[0] + quat[1] * quat[1] + quat[2] * quat[2] + quat[3] * quat[3]);
    let m = quat_to_mat3([quat[0] / len, quat[1] / len, quat[2] / len, quat[3] / len]);
    let (x_axis, y_axis, z_axis) = (m[0], m[1], m[2]);
    let cy = sqrt(x_axis[0] * x_axis[0] + y_axis[0] * y_axis[0]);
    let (x, y, z) = if cy > 16.0 * f32::EPSILON {
        (
            atan2(z_axis[1], z_axis[2]),
            atan2(-z_axis[0], cy),
            atan2(y_axis[0], x_axis[0]),
        )
    } else {
        (atan2(-y_axis[2], y_axis[1]), atan2(-z_axis[0], cy), 0.0)
    };
    // glam negates the parity-even orders, XYZ among them.
    [(-x).to_degrees(), (-y).to_degrees(), (-z).to_degrees()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::glam::{EulerRot, Quat};
    use crate::rng::DeterministicRng;

    /// The convention claim is the whole point of these functions, so it is asserted against
    /// glam's own constructor rather than against a transcribed table of numbers.
    #[test]
    fn matches_glam_from_euler_xyz() {
        for degrees in [
            [0.0, 0.0, 0.0],
            [30.0, 0.0, 0.0],
            [0.0, -45.0, 0.0],
            [0.0, 0.0, 90.0],
            [12.0, 34.0, 56.0],
            [-170.0, 25.0, -95.0],
        ] {
            let ours = Quat::from_array(euler_xyz_degrees_to_quat(degrees));
            let glam = Quat::from_euler(
                EulerRot::XYZ,
                degrees[0].to_radians(),
                degrees[1].to_radians(),
                degrees[2].to_radians(),
            );
            assert!(
                ours.abs_diff_eq(glam, 1e-6),
                "{degrees:?}: {ours:?} vs {glam:?}"
            );
        }
    }

    #[test]
    fn round_trips_away_from_gimbal_lock() {
        for degrees in [[12.0, 34.0, 56.0], [-20.0, 15.0, -70.0], [0.0, 0.0, 0.0]] {
            let back = quat_to_euler_xyz_degrees(euler_xyz_degrees_to_quat(degrees));
            for axis in 0..3 {
                assert!(
                    (back[axis] - degrees[axis]).abs() < 1e-3,
                    "{degrees:?} -> {back:?}"
                );
            }
        }
    }

    #[test]
    fn identity_is_the_identity_quaternion() {
        assert_eq!(
            euler_xyz_degrees_to_quat([0.0, 0.0, 0.0]),
            [0.0, 0.0, 0.0, 1.0]
        );
        assert_eq!(
            quat_to_euler_xyz_degrees([0.0, 0.0, 0.0, 1.0]),
            [0.0, 0.0, 0.0]
        );
    }

    fn bits4(q: [f32; 4]) -> [u32; 4] {
        q.map(f32::to_bits)
    }

    #[test]
    fn quat_mul_is_bit_identical_to_glam() {
        let mut rng = DeterministicRng::new(0x51_AB_17);
        let mut pairs: Vec<([f32; 4], [f32; 4])> = Vec::new();
        let specials: [[f32; 4]; 9] = [
            [0.0, 0.0, 0.0, 1.0],
            [0.0, 0.0, 0.0, -1.0],
            [-0.0, 0.0, -0.0, 1.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.5, 0.5, 0.5, 0.5],
            [0.0, 0.0, 0.0, 0.0],
            [-0.0, -0.0, -0.0, -0.0],
        ];
        for a in specials {
            for b in specials {
                pairs.push((a, b));
            }
        }
        let random = |rng: &mut DeterministicRng, normalize: bool| {
            let mut q = [0.0f32; 4];
            for c in &mut q {
                *c = rng.next_f32() * 4.0 - 2.0;
            }
            if normalize {
                let len = sqrt(q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]);
                q = q.map(|c| c / len);
            }
            q
        };
        for i in 0..2000 {
            let normalize = i % 2 == 0;
            let a = random(&mut rng, normalize);
            let b = random(&mut rng, normalize);
            pairs.push((a, b));
        }
        for (a, b) in pairs {
            let ours = quat_mul(a, b);
            let glam = (Quat::from_array(a) * Quat::from_array(b)).to_array();
            assert_eq!(bits4(ours), bits4(glam), "{a:?} * {b:?}");
        }
    }

    #[test]
    fn quat_rotate_matches_the_rotation_matrix() {
        let q = euler_xyz_degrees_to_quat([12.0, 34.0, 56.0]);
        let m = quat_to_mat3(q);
        let v = [1.5, -2.0, 0.75];
        let got = quat_rotate(q, v);
        for i in 0..3 {
            let want = m[0][i] * v[0] + m[1][i] * v[1] + m[2][i] * v[2];
            assert!((got[i] - want).abs() < 1e-5, "{got:?}");
        }
    }
}
