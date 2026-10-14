//! Sim-grade transform composition: [`Trs`] (translation, rotation, scale) and [`Affine3`].
//!
//! Scalar `f32` arithmetic in a fixed order — no `glam` `Quat`/`Mat4` arithmetic — so a composed
//! transform is bit-identical on every platform (`tests/trs_bits.rs` pins it). Rotations are unit
//! quaternions `[x, y, z, w]`; the Euler convention lives in [`super::euler_xyz_degrees_to_quat`].

use super::rotation::{quat_mul, quat_rotate, quat_to_mat3};
use super::sqrt;

/// A transform as translation, rotation (unit quaternion `[x, y, z, w]`) and per-axis scale.
///
/// Applying it to a point scales, then rotates, then translates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trs {
    /// Translation, `[x, y, z]`.
    pub translation: [f32; 3],
    /// Unit quaternion, `[x, y, z, w]`.
    pub rotation: [f32; 4],
    /// Per-axis scale.
    pub scale: [f32; 3],
}

/// A 3×4 affine transform, column-major: the x, y and z axes, then the translation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine3 {
    /// The x, y and z axes, then the translation.
    pub cols: [[f32; 3]; 4],
}

impl Trs {
    /// No translation, no rotation, unit scale.
    pub const IDENTITY: Trs = Trs {
        translation: [0.0; 3],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0; 3],
    };

    /// `self` as parent, `child` in its space: the child's transform in the parent's parent space.
    ///
    /// `t = p.t + rotate(p.r, p.s ⊙ c.t)`, `r = p.r · c.r`, `s = p.s ⊙ c.s`. The translation is
    /// always the exact affine result. Rotation and scale are exact only when no shear arises
    /// (parent scale uniform, or the parent's rotation identity or aligned with the child's axes);
    /// otherwise the shear is dropped — that is the documented sim result, not an error. `r` is
    /// never renormalized: over a depth of at most 64 the drift is sub-ulp per level.
    ///
    /// # Example
    /// ```
    /// use ludion_core::math::{euler_xyz_degrees_to_quat, Trs};
    /// let parent = Trs {
    ///     translation: [10.0, 0.0, 0.0],
    ///     rotation: euler_xyz_degrees_to_quat([0.0, 90.0, 0.0]),
    ///     scale: [1.0; 3],
    /// };
    /// let child = Trs { translation: [1.0, 0.0, 0.0], ..Trs::IDENTITY };
    /// let t = parent.mul_trs(&child).translation;
    /// assert!((t[0] - 10.0).abs() < 1e-6 && t[1].abs() < 1e-6 && (t[2] + 1.0).abs() < 1e-6, "{t:?}");
    /// ```
    pub fn mul_trs(&self, child: &Trs) -> Trs {
        let scaled = [
            self.scale[0] * child.translation[0],
            self.scale[1] * child.translation[1],
            self.scale[2] * child.translation[2],
        ];
        let rotated = quat_rotate(self.rotation, scaled);
        Trs {
            translation: [
                self.translation[0] + rotated[0],
                self.translation[1] + rotated[1],
                self.translation[2] + rotated[2],
            ],
            rotation: quat_mul(self.rotation, child.rotation),
            scale: [
                self.scale[0] * child.scale[0],
                self.scale[1] * child.scale[1],
                self.scale[2] * child.scale[2],
            ],
        }
    }

    /// `world` expressed in `self`'s space: the `local` with `self.mul_trs(&local)` placing it at `world`.
    ///
    /// The translation is always exact: `s⁻¹ ⊙ rotate(conj(r), w.t − t)`. Rotation (`conj(r) · w.r`)
    /// and scale (`w.s / s`) are exact except under shear (non-uniform parent scale with a rotation
    /// not aligned to the child's axes), where they are the closest rigid pose. A zero scale
    /// component yields non-finite values, never a panic.
    pub fn localize(&self, world: &Trs) -> Trs {
        let rotation = [
            -self.rotation[0],
            -self.rotation[1],
            -self.rotation[2],
            self.rotation[3],
        ];
        let back = quat_rotate(
            rotation,
            [
                world.translation[0] - self.translation[0],
                world.translation[1] - self.translation[1],
                world.translation[2] - self.translation[2],
            ],
        );
        Trs {
            translation: [
                back[0] / self.scale[0],
                back[1] / self.scale[1],
                back[2] / self.scale[2],
            ],
            rotation: quat_mul(rotation, world.rotation),
            scale: [
                (1.0 / self.scale[0]) * world.scale[0],
                (1.0 / self.scale[1]) * world.scale[1],
                (1.0 / self.scale[2]) * world.scale[2],
            ],
        }
    }

    /// The parent that places `local` at `self`: the `p` with `p.mul_trs(local)` landing on `self`.
    ///
    /// The translation is always exact: `self.t − rotate(p.r, p.s ⊙ local.t)`. Rotation
    /// (`self.r · conj(local.r)`) and scale (`self.s / local.s`) are exact except under shear. A zero
    /// scale component yields non-finite values, never a panic.
    pub fn parent_for(&self, local: &Trs) -> Trs {
        let rotation = quat_mul(
            self.rotation,
            [
                -local.rotation[0],
                -local.rotation[1],
                -local.rotation[2],
                local.rotation[3],
            ],
        );
        let scale = [
            self.scale[0] / local.scale[0],
            self.scale[1] / local.scale[1],
            self.scale[2] / local.scale[2],
        ];
        let rotated = quat_rotate(
            rotation,
            [
                scale[0] * local.translation[0],
                scale[1] * local.translation[1],
                scale[2] * local.translation[2],
            ],
        );
        Trs {
            translation: [
                self.translation[0] - rotated[0],
                self.translation[1] - rotated[1],
                self.translation[2] - rotated[2],
            ],
            rotation,
            scale,
        }
    }

    /// The world→local transform: `r' = conj(r)`, `s' = 1/s`, `t' = s' ⊙ rotate(r', -t)`.
    ///
    /// Exact for uniform scale. Under non-uniform scale with rotation, composing it with a child
    /// misplaces the child's translation; use [`Trs::localize`] to re-express a world pose. A zero
    /// scale component yields non-finite values, never a panic.
    pub fn inverse(&self) -> Trs {
        let rotation = [
            -self.rotation[0],
            -self.rotation[1],
            -self.rotation[2],
            self.rotation[3],
        ];
        let scale = [
            1.0 / self.scale[0],
            1.0 / self.scale[1],
            1.0 / self.scale[2],
        ];
        let back = quat_rotate(
            rotation,
            [
                -self.translation[0],
                -self.translation[1],
                -self.translation[2],
            ],
        );
        Trs {
            translation: [scale[0] * back[0], scale[1] * back[1], scale[2] * back[2]],
            rotation,
            scale,
        }
    }

    /// The affine form: columns are the rotation matrix's axes times the scale, then `t`.
    pub fn to_affine(&self) -> Affine3 {
        let m = quat_to_mat3(self.rotation);
        let s = self.scale;
        Affine3 {
            cols: [
                [m[0][0] * s[0], m[0][1] * s[0], m[0][2] * s[0]],
                [m[1][0] * s[1], m[1][1] * s[1], m[1][2] * s[1]],
                [m[2][0] * s[2], m[2][1] * s[2], m[2][2] * s[2]],
                self.translation,
            ],
        }
    }
}

impl Affine3 {
    /// The identity transform.
    pub const IDENTITY: Affine3 = Affine3 {
        cols: [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 0.0],
        ],
    };

    /// `self · rhs`: `rhs` applied first.
    pub fn mul_affine(&self, rhs: &Affine3) -> Affine3 {
        let a = &self.cols;
        // Each dot product is summed left to right: (a0*b0 + a1*b1) + a2*b2.
        let lin = |v: [f32; 3]| {
            [
                (a[0][0] * v[0] + a[1][0] * v[1]) + a[2][0] * v[2],
                (a[0][1] * v[0] + a[1][1] * v[1]) + a[2][1] * v[2],
                (a[0][2] * v[0] + a[1][2] * v[1]) + a[2][2] * v[2],
            ]
        };
        let t = lin(rhs.cols[3]);
        Affine3 {
            cols: [
                lin(rhs.cols[0]),
                lin(rhs.cols[1]),
                lin(rhs.cols[2]),
                [t[0] + a[3][0], t[1] + a[3][1], t[2] + a[3][2]],
            ],
        }
    }

    /// `A·p + t`, each dot product summed left to right.
    pub fn transform_point(&self, p: [f32; 3]) -> [f32; 3] {
        let a = &self.cols;
        [
            ((a[0][0] * p[0] + a[1][0] * p[1]) + a[2][0] * p[2]) + a[3][0],
            ((a[0][1] * p[0] + a[1][1] * p[1]) + a[2][1] * p[2]) + a[3][1],
            ((a[0][2] * p[0] + a[1][2] * p[1]) + a[2][2] * p[2]) + a[3][2],
        ]
    }

    /// The scale [`Affine3::to_trs_lossy`] reports: the column lengths, `s.x` negated when the
    /// determinant is negative. Cheaper than the full decomposition when no rotation is needed.
    pub fn lossy_scale(&self) -> [f32; 3] {
        let [c0, c1, c2, _] = self.cols;
        let len = |c: [f32; 3]| sqrt((c[0] * c[0] + c[1] * c[1]) + c[2] * c[2]);
        let dot = |a: [f32; 3], b: [f32; 3]| (a[0] * b[0] + a[1] * b[1]) + a[2] * b[2];
        let cross = |a: [f32; 3], b: [f32; 3]| {
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
        };
        let mut scale = [len(c0), len(c1), len(c2)];
        if dot(c0, cross(c1, c2)) < 0.0 {
            scale[0] = -scale[0];
        }
        scale
    }

    /// Decomposes back to translation, rotation and scale.
    ///
    /// `t` is column 3; `s_i` is the length of column `i` (`s.x` negated when the determinant is
    /// negative); the rotation comes from the Gram-Schmidt-orthonormalized columns (x, then y minus
    /// its x projection, then `z = x × y`) via the Shepperd trace branch. Exact (to rounding) for a
    /// shear-free matrix; shear is dropped otherwise. A zero-length column yields non-finite
    /// values, never a panic.
    pub fn to_trs_lossy(&self) -> Trs {
        let [mut c0, c1, _, translation] = self.cols;
        let len = |c: [f32; 3]| sqrt((c[0] * c[0] + c[1] * c[1]) + c[2] * c[2]);
        let dot = |a: [f32; 3], b: [f32; 3]| (a[0] * b[0] + a[1] * b[1]) + a[2] * b[2];
        let cross = |a: [f32; 3], b: [f32; 3]| {
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
        };
        let scale = self.lossy_scale();
        if scale[0] < 0.0 {
            c0 = [-c0[0], -c0[1], -c0[2]];
        }
        let l0 = len(c0);
        let x = [c0[0] / l0, c0[1] / l0, c0[2] / l0];
        let proj = dot(c1, x);
        let y_raw = [
            c1[0] - proj * x[0],
            c1[1] - proj * x[1],
            c1[2] - proj * x[2],
        ];
        let ly = len(y_raw);
        let y = [y_raw[0] / ly, y_raw[1] / ly, y_raw[2] / ly];
        let z = cross(x, y);
        Trs {
            translation,
            rotation: mat3_to_quat(x, y, z),
            scale,
        }
    }
}

/// The unit quaternion of the rotation with columns `x`, `y`, `z` (Shepperd: branch on the largest
/// diagonal term so the divisor never cancels).
fn mat3_to_quat(x: [f32; 3], y: [f32; 3], z: [f32; 3]) -> [f32; 4] {
    let (m00, m10, m20) = (x[0], x[1], x[2]);
    let (m01, m11, m21) = (y[0], y[1], y[2]);
    let (m02, m12, m22) = (z[0], z[1], z[2]);
    let trace = (m00 + m11) + m22;
    if trace > 0.0 {
        let s = sqrt(trace + 1.0) * 2.0;
        [(m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, s * 0.25]
    } else if m00 > m11 && m00 > m22 {
        let s = sqrt(((1.0 + m00) - m11) - m22) * 2.0;
        [s * 0.25, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s]
    } else if m11 > m22 {
        let s = sqrt(((1.0 + m11) - m00) - m22) * 2.0;
        [(m01 + m10) / s, s * 0.25, (m12 + m21) / s, (m02 - m20) / s]
    } else {
        let s = sqrt(((1.0 + m22) - m00) - m11) * 2.0;
        [(m02 + m20) / s, (m12 + m21) / s, s * 0.25, (m10 - m01) / s]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::euler_xyz_degrees_to_quat;

    fn trs(t: [f32; 3], euler: [f32; 3], s: [f32; 3]) -> Trs {
        Trs {
            translation: t,
            rotation: euler_xyz_degrees_to_quat(euler),
            scale: s,
        }
    }

    fn close(a: &[f32], b: &[f32], tol: f32) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol)
    }

    fn affine_close(a: &Affine3, b: &Affine3, tol: f32) -> bool {
        a.cols.iter().zip(&b.cols).all(|(x, y)| close(x, y, tol))
    }

    fn quat_close(a: [f32; 4], b: [f32; 4], tol: f32) -> bool {
        let neg = b.map(|c| -c);
        close(&a, &b, tol) || close(&a, &neg, tol)
    }

    #[test]
    fn identity_laws() {
        let x = trs([1.0, -2.0, 3.0], [10.0, 20.0, 30.0], [1.0, 2.0, 3.0]);
        assert_eq!(Trs::IDENTITY.mul_trs(&x), x);
        assert_eq!(Trs::IDENTITY.to_affine(), Affine3::IDENTITY);
        let a = x.to_affine();
        assert_eq!(Affine3::IDENTITY.mul_affine(&a), a);
        assert_eq!(a.mul_affine(&Affine3::IDENTITY), a);
        assert_eq!(
            Affine3::IDENTITY.transform_point([4.0, 5.0, 6.0]),
            [4.0, 5.0, 6.0]
        );
    }

    #[test]
    fn mul_trs_agrees_with_mul_affine_under_uniform_parent_scale() {
        let p = trs([5.0, 1.0, -3.0], [20.0, -40.0, 70.0], [2.0; 3]);
        let c = trs([1.0, 2.0, 3.0], [-15.0, 33.0, 10.0], [1.0, 0.5, 3.0]);
        let composed = p.mul_trs(&c).to_affine();
        let exact = p.to_affine().mul_affine(&c.to_affine());
        assert!(
            affine_close(&composed, &exact, 1e-5),
            "{composed:?} vs {exact:?}"
        );
    }

    #[test]
    fn a_trs_times_its_inverse_is_identity_for_uniform_scale() {
        let x = trs([5.0, 1.0, -3.0], [20.0, -40.0, 70.0], [2.5; 3]);
        let id = x.mul_trs(&x.inverse());
        assert!(close(&id.translation, &[0.0; 3], 1e-5), "{id:?}");
        assert!(
            quat_close(id.rotation, [0.0, 0.0, 0.0, 1.0], 1e-5),
            "{id:?}"
        );
        assert!(close(&id.scale, &[1.0; 3], 1e-6), "{id:?}");
    }

    #[test]
    fn localize_keeps_world_translation_under_rotated_non_uniform_scale() {
        let parent = trs([0.0; 3], [0.0, 90.0, 0.0], [2.0, 1.0, 1.0]);
        let world = trs([1.0, 0.0, 0.0], [0.0; 3], [1.0; 3]);
        let t = parent.mul_trs(&parent.localize(&world)).translation;
        assert!(close(&t, &world.translation, 1e-5), "{t:?}");
    }

    #[test]
    fn localize_recomposes_to_the_world_translation_for_random_parents() {
        let mut state = 0x9E37_79B9_u32;
        let mut next = move || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f32 / (1u32 << 24) as f32
        };
        for _ in 0..500 {
            let mut v = |lo: f32, hi: f32| lo + (hi - lo) * next();
            let parent = trs(
                [v(-9.0, 9.0), v(-9.0, 9.0), v(-9.0, 9.0)],
                [v(-180.0, 180.0), v(-180.0, 180.0), v(-180.0, 180.0)],
                [v(0.25, 4.0), v(0.25, 4.0), v(0.25, 4.0)],
            );
            let world = trs(
                [v(-9.0, 9.0), v(-9.0, 9.0), v(-9.0, 9.0)],
                [v(-180.0, 180.0), v(-180.0, 180.0), v(-180.0, 180.0)],
                [v(0.25, 4.0), v(0.25, 4.0), v(0.25, 4.0)],
            );
            let t = parent.mul_trs(&parent.localize(&world)).translation;
            assert!(
                close(&t, &world.translation, 1e-3),
                "{parent:?} {world:?} -> {t:?}"
            );
        }
    }

    #[test]
    fn parent_for_places_the_local_at_the_world_translation_for_random_poses() {
        let mut state = 0x85EB_CA6B_u32;
        let mut next = move || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f32 / (1u32 << 24) as f32
        };
        for _ in 0..500 {
            let mut v = |lo: f32, hi: f32| lo + (hi - lo) * next();
            let mut pose = || {
                trs(
                    [v(-9.0, 9.0), v(-9.0, 9.0), v(-9.0, 9.0)],
                    [v(-180.0, 180.0), v(-180.0, 180.0), v(-180.0, 180.0)],
                    [v(0.25, 4.0), v(0.25, 4.0), v(0.25, 4.0)],
                )
            };
            let (world, local) = (pose(), pose());
            let t = world.parent_for(&local).mul_trs(&local).translation;
            assert!(
                close(&t, &world.translation, 1e-3),
                "{world:?} {local:?} -> {t:?}"
            );
        }
    }

    #[test]
    fn to_trs_lossy_round_trips_a_shear_free_transform() {
        for x in [
            trs([1.0, 2.0, 3.0], [10.0, 20.0, 30.0], [1.0, 2.0, 3.0]),
            trs([0.0; 3], [170.0, 80.0, -120.0], [0.5, 0.5, 0.5]),
            trs([-4.0, 0.0, 9.0], [0.0, 180.0, 0.0], [1.0; 3]),
            trs([0.0; 3], [90.0, 0.0, 0.0], [-1.0, 2.0, 1.0]),
        ] {
            let back = x.to_affine().to_trs_lossy();
            assert!(
                close(&back.translation, &x.translation, 1e-5),
                "{x:?} -> {back:?}"
            );
            assert!(close(&back.scale, &x.scale, 1e-5), "{x:?} -> {back:?}");
            assert!(
                affine_close(&back.to_affine(), &x.to_affine(), 1e-5),
                "{x:?} -> {back:?}"
            );
            assert!(quat_close(back.rotation, x.rotation, 1e-5) || x.scale[0] < 0.0);
        }
    }

    #[test]
    fn non_uniform_scale_under_rotation_loses_the_shear() {
        let p = trs([0.0; 3], [0.0, 0.0, 45.0], [2.0, 1.0, 1.0]);
        let c = trs([0.0; 3], [0.0, 0.0, 45.0], [1.0; 3]);
        let exact = p.to_affine().mul_affine(&c.to_affine());
        let lossy = exact.to_trs_lossy().to_affine();
        let composed = p.mul_trs(&c).to_affine();
        assert!(
            !affine_close(&lossy, &exact, 1e-3),
            "{lossy:?} vs {exact:?}"
        );
        assert!(
            !affine_close(&composed, &exact, 1e-3),
            "{composed:?} vs {exact:?}"
        );
    }

    #[test]
    fn a_zero_scale_inverse_is_non_finite_not_a_panic() {
        let x = trs([1.0; 3], [0.0; 3], [0.0, 1.0, 1.0]);
        assert!(!x.inverse().translation[0].is_finite());
    }
}
