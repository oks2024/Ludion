//! 4×4 determinant and inverse as scalar `f32` arithmetic.
//!
//! glam's `Mat4::inverse` sums its `dot4` products in a different order on SSE2 and NEON, and its
//! `Mat4::determinant` is a SIMD lowering on one backend and scalar cofactors on the other, so the
//! last bits differ per architecture. These two are written out element by element in a fixed
//! order (2×2 sub-determinant Laplace expansion), with no glam `Vec4`/`Mat4` arithmetic, and pinned
//! by this crate's bit fixture (`tests/trs_bits.rs`). Use them instead of the glam methods on any
//! path whose result can reach a hash.

use glam::Mat4;

/// The determinant of `m`, evaluated in a fixed scalar order.
///
/// # Example
/// ```
/// use ludion_core::math::{glam::{Mat4, Vec3}, mat4_determinant};
/// let m = Mat4::from_scale(Vec3::new(2.0, 3.0, 4.0));
/// assert_eq!(mat4_determinant(m), 24.0);
/// ```
pub fn mat4_determinant(m: Mat4) -> f32 {
    let a = m.to_cols_array();
    let (s, c) = sub_determinants(&a);
    det_of(&s, &c)
}

/// The inverse of `m`, evaluated in a fixed scalar order: each cofactor is divided by the
/// determinant. A singular input yields non-finite values (as glam's does); it never panics.
///
/// # Example
/// ```
/// use ludion_core::math::{glam::{Mat4, Vec3}, mat4_inverse};
/// let m = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0)) * Mat4::from_scale(Vec3::new(2.0, 4.0, 0.5));
/// let id = mat4_inverse(m) * m;
/// assert!(id.abs_diff_eq(Mat4::IDENTITY, 1e-6));
/// ```
pub fn mat4_inverse(m: Mat4) -> Mat4 {
    let a = m.to_cols_array();
    let (s, c) = sub_determinants(&a);
    let det = det_of(&s, &c);
    // Indexed `a[row * 4 + col]` over the flat array: the inverse of the transpose is the transpose
    // of the inverse, so writing the result back in the same layout is the inverse of `m`.
    let [
        a00,
        a01,
        a02,
        a03,
        a10,
        a11,
        a12,
        a13,
        a20,
        a21,
        a22,
        a23,
        a30,
        a31,
        a32,
        a33,
    ] = a;
    let [s0, s1, s2, s3, s4, s5] = s;
    let [c0, c1, c2, c3, c4, c5] = c;
    let out = [
        a11 * c5 - a12 * c4 + a13 * c3,
        -a01 * c5 + a02 * c4 - a03 * c3,
        a31 * s5 - a32 * s4 + a33 * s3,
        -a21 * s5 + a22 * s4 - a23 * s3,
        -a10 * c5 + a12 * c2 - a13 * c1,
        a00 * c5 - a02 * c2 + a03 * c1,
        -a30 * s5 + a32 * s2 - a33 * s1,
        a20 * s5 - a22 * s2 + a23 * s1,
        a10 * c4 - a11 * c2 + a13 * c0,
        -a00 * c4 + a01 * c2 - a03 * c0,
        a30 * s4 - a31 * s2 + a33 * s0,
        -a20 * s4 + a21 * s2 - a23 * s0,
        -a10 * c3 + a11 * c1 - a12 * c0,
        a00 * c3 - a01 * c1 + a02 * c0,
        -a30 * s3 + a31 * s1 - a32 * s0,
        a20 * s3 - a21 * s1 + a22 * s0,
    ];
    Mat4::from_cols_array(&out.map(|v| v / det))
}

/// The six 2×2 minors of the top two rows (`s`) and of the bottom two rows (`c`).
fn sub_determinants(a: &[f32; 16]) -> ([f32; 6], [f32; 6]) {
    let [
        a00,
        a01,
        a02,
        a03,
        a10,
        a11,
        a12,
        a13,
        a20,
        a21,
        a22,
        a23,
        a30,
        a31,
        a32,
        a33,
    ] = *a;
    (
        [
            a00 * a11 - a10 * a01,
            a00 * a12 - a10 * a02,
            a00 * a13 - a10 * a03,
            a01 * a12 - a11 * a02,
            a01 * a13 - a11 * a03,
            a02 * a13 - a12 * a03,
        ],
        [
            a20 * a31 - a30 * a21,
            a20 * a32 - a30 * a22,
            a20 * a33 - a30 * a23,
            a21 * a32 - a31 * a22,
            a21 * a33 - a31 * a23,
            a22 * a33 - a32 * a23,
        ],
    )
}

fn det_of(s: &[f32; 6], c: &[f32; 6]) -> f32 {
    s[0] * c[5] - s[1] * c[4] + s[2] * c[3] + s[3] * c[2] - s[4] * c[1] + s[5] * c[0]
}
