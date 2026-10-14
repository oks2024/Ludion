//! Deterministic math: `glam` re-export plus pure-Rust transcendental wrappers.
//!
//! Plain arithmetic and `glam` vector/matrix ops are already deterministic (Rust does not
//! auto-contract to FMA), so they need no wrapping and are re-exported as-is via [`glam`].
//!
//! Transcendental functions are the hazard: the platform C `libm` implements `sin`, `pow`, etc.
//! differently on Windows and macOS, so the same expression can produce different last-bit results
//! per OS. The wrappers below route every transcendental through the pure-Rust [`libm`] crate,
//! whose output depends only on the input — identical on every platform and toolchain-pinned build.
//! **Always call these instead of the `f32`/`f64` inherent methods (`x.sin()`, `x.powf(y)`) on the
//! sim path** — every strict `clippy.toml` bans them. The set covers every `f32`/`f64` transcendental
//! method; `sqrt` is the one that is also safe from `std`.
//!
//! Top-level functions operate on `f32` (the engine's default precision, matching `glam`); the
//! [`f64`](mod@self::f64) submodule mirrors them for double precision.
//!
//! The rotation convention is Euler XYZ degrees, intrinsic ([`euler_xyz_degrees_to_quat`] and its
//! inverse). [`Trs`] and [`Affine3`] are the sim-grade transform composition: scalar `f32`, fixed
//! evaluation order, no `glam` `Quat`/`Mat4` arithmetic, pinned bit-for-bit by `tests/trs_bits.rs`.
//!
//! # Orbit framing
//!
//! [`Orbit`] places a camera from a target, an azimuth, a pitch and a distance, and reads the
//! angles back off a rotation — what `ludion render --look-at` and a follow camera share.
//!
//! # Example
//!
//! ```
//! use ludion_core::math;
//! use ludion_core::math::glam::Vec3;
//!
//! let v = Vec3::new(3.0, 4.0, 0.0);
//! assert!((v.length() - 5.0).abs() < 1e-6);
//!
//! assert!((math::sqrt(9.0) - 3.0).abs() < 1e-6);
//! assert!(math::sin(0.0).abs() < 1e-6);
//! ```

mod orbit;
mod rotation;
mod rotation_f64;
mod trs;

pub use orbit::Orbit;
pub use rotation::{euler_xyz_degrees_to_quat, quat_mul, quat_rotate, quat_to_euler_xyz_degrees};
pub use trs::{Affine3, Trs};

/// Re-export of the [`glam`] linear-algebra library. Use `ludion_core::math::glam::{Vec2, Vec3, ..}`.
pub use glam;

/// Sine of `x` (radians).
#[inline]
pub fn sin(x: f32) -> f32 {
    libm::sinf(x)
}

/// Cosine of `x` (radians).
#[inline]
pub fn cos(x: f32) -> f32 {
    libm::cosf(x)
}

/// Tangent of `x` (radians).
#[inline]
pub fn tan(x: f32) -> f32 {
    libm::tanf(x)
}

/// Four-quadrant arctangent of `y / x` (radians).
#[inline]
pub fn atan2(y: f32, x: f32) -> f32 {
    libm::atan2f(y, x)
}

/// Square root of `x`.
///
/// IEEE `sqrt` is correctly rounded and identical everywhere, so this is bit-identical to
/// `f32::sqrt`, which the clippy lint allows. It stays as the one `ludion::math` import path the
/// sim crates and games already call, so `sqrt` sits beside the other math wrappers.
#[inline]
pub fn sqrt(x: f32) -> f32 {
    libm::sqrtf(x)
}

/// `x` raised to the power `y`.
#[inline]
pub fn powf(x: f32, y: f32) -> f32 {
    libm::powf(x, y)
}

/// `e` raised to the power `x`.
#[inline]
pub fn exp(x: f32) -> f32 {
    libm::expf(x)
}

/// Natural logarithm of `x`.
#[inline]
pub fn ln(x: f32) -> f32 {
    libm::logf(x)
}

/// Arcsine of `x` (radians).
#[inline]
pub fn asin(x: f32) -> f32 {
    libm::asinf(x)
}

/// Arccosine of `x` (radians).
#[inline]
pub fn acos(x: f32) -> f32 {
    libm::acosf(x)
}

/// Arctangent of `x` (radians).
#[inline]
pub fn atan(x: f32) -> f32 {
    libm::atanf(x)
}

/// Sine and cosine of `x` (radians), as `(sin, cos)`.
#[inline]
pub fn sin_cos(x: f32) -> (f32, f32) {
    libm::sincosf(x)
}

/// Hyperbolic sine of `x`.
#[inline]
pub fn sinh(x: f32) -> f32 {
    libm::sinhf(x)
}

/// Hyperbolic cosine of `x`.
#[inline]
pub fn cosh(x: f32) -> f32 {
    libm::coshf(x)
}

/// Hyperbolic tangent of `x`.
#[inline]
pub fn tanh(x: f32) -> f32 {
    libm::tanhf(x)
}

/// Inverse hyperbolic sine of `x`.
#[inline]
pub fn asinh(x: f32) -> f32 {
    libm::asinhf(x)
}

/// Inverse hyperbolic cosine of `x`.
#[inline]
pub fn acosh(x: f32) -> f32 {
    libm::acoshf(x)
}

/// Inverse hyperbolic tangent of `x`.
#[inline]
pub fn atanh(x: f32) -> f32 {
    libm::atanhf(x)
}

/// `2` raised to the power `x`.
#[inline]
pub fn exp2(x: f32) -> f32 {
    libm::exp2f(x)
}

/// `e^x - 1`, accurate for small `x`.
#[inline]
pub fn exp_m1(x: f32) -> f32 {
    libm::expm1f(x)
}

/// Logarithm of `x` in `base`, computed as `ln(x) / ln(base)`.
#[inline]
pub fn log(x: f32, base: f32) -> f32 {
    ln(x) / ln(base)
}

/// Base-2 logarithm of `x`.
#[inline]
pub fn log2(x: f32) -> f32 {
    libm::log2f(x)
}

/// Base-10 logarithm of `x`.
#[inline]
pub fn log10(x: f32) -> f32 {
    libm::log10f(x)
}

/// `ln(1 + x)`, accurate for small `x`.
#[inline]
pub fn ln_1p(x: f32) -> f32 {
    libm::log1pf(x)
}

/// Cube root of `x`.
#[inline]
pub fn cbrt(x: f32) -> f32 {
    libm::cbrtf(x)
}

/// Length of the hypotenuse, `sqrt(x² + y²)`, without intermediate overflow.
#[inline]
pub fn hypot(x: f32, y: f32) -> f32 {
    libm::hypotf(x, y)
}

/// Double-precision (`f64`) mirrors of the transcendental wrappers.
pub mod f64 {
    pub use super::rotation_f64::{euler_xyz_degrees_to_quat, quat_to_euler_xyz_degrees};

    /// Sine of `x` (radians).
    #[inline]
    pub fn sin(x: f64) -> f64 {
        libm::sin(x)
    }

    /// Cosine of `x` (radians).
    #[inline]
    pub fn cos(x: f64) -> f64 {
        libm::cos(x)
    }

    /// Tangent of `x` (radians).
    #[inline]
    pub fn tan(x: f64) -> f64 {
        libm::tan(x)
    }

    /// Four-quadrant arctangent of `y / x` (radians).
    #[inline]
    pub fn atan2(y: f64, x: f64) -> f64 {
        libm::atan2(y, x)
    }

    /// Square root of `x`; bit-identical to `f64::sqrt`, kept for the reason on [`super::sqrt`].
    #[inline]
    pub fn sqrt(x: f64) -> f64 {
        libm::sqrt(x)
    }

    /// `x` raised to the power `y`.
    #[inline]
    pub fn powf(x: f64, y: f64) -> f64 {
        libm::pow(x, y)
    }

    /// `e` raised to the power `x`.
    #[inline]
    pub fn exp(x: f64) -> f64 {
        libm::exp(x)
    }

    /// Natural logarithm of `x`.
    #[inline]
    pub fn ln(x: f64) -> f64 {
        libm::log(x)
    }

    /// Arcsine of `x` (radians).
    #[inline]
    pub fn asin(x: f64) -> f64 {
        libm::asin(x)
    }

    /// Arccosine of `x` (radians).
    #[inline]
    pub fn acos(x: f64) -> f64 {
        libm::acos(x)
    }

    /// Arctangent of `x` (radians).
    #[inline]
    pub fn atan(x: f64) -> f64 {
        libm::atan(x)
    }

    /// Sine and cosine of `x` (radians), as `(sin, cos)`.
    #[inline]
    pub fn sin_cos(x: f64) -> (f64, f64) {
        libm::sincos(x)
    }

    /// Hyperbolic sine of `x`.
    #[inline]
    pub fn sinh(x: f64) -> f64 {
        libm::sinh(x)
    }

    /// Hyperbolic cosine of `x`.
    #[inline]
    pub fn cosh(x: f64) -> f64 {
        libm::cosh(x)
    }

    /// Hyperbolic tangent of `x`.
    #[inline]
    pub fn tanh(x: f64) -> f64 {
        libm::tanh(x)
    }

    /// Inverse hyperbolic sine of `x`.
    #[inline]
    pub fn asinh(x: f64) -> f64 {
        libm::asinh(x)
    }

    /// Inverse hyperbolic cosine of `x`.
    #[inline]
    pub fn acosh(x: f64) -> f64 {
        libm::acosh(x)
    }

    /// Inverse hyperbolic tangent of `x`.
    #[inline]
    pub fn atanh(x: f64) -> f64 {
        libm::atanh(x)
    }

    /// `2` raised to the power `x`.
    #[inline]
    pub fn exp2(x: f64) -> f64 {
        libm::exp2(x)
    }

    /// `e^x - 1`, accurate for small `x`.
    #[inline]
    pub fn exp_m1(x: f64) -> f64 {
        libm::expm1(x)
    }

    /// Logarithm of `x` in `base`, computed as `ln(x) / ln(base)`.
    #[inline]
    pub fn log(x: f64, base: f64) -> f64 {
        ln(x) / ln(base)
    }

    /// Base-2 logarithm of `x`.
    #[inline]
    pub fn log2(x: f64) -> f64 {
        libm::log2(x)
    }

    /// Base-10 logarithm of `x`.
    #[inline]
    pub fn log10(x: f64) -> f64 {
        libm::log10(x)
    }

    /// `ln(1 + x)`, accurate for small `x`.
    #[inline]
    pub fn ln_1p(x: f64) -> f64 {
        libm::log1p(x)
    }

    /// Cube root of `x`.
    #[inline]
    pub fn cbrt(x: f64) -> f64 {
        libm::cbrt(x)
    }

    /// Length of the hypotenuse, `sqrt(x² + y²)`, without intermediate overflow.
    #[inline]
    pub fn hypot(x: f64, y: f64) -> f64 {
        libm::hypot(x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_wrappers_are_sane() {
        assert!((sqrt(16.0) - 4.0).abs() < 1e-6);
        assert!((sin(std::f32::consts::FRAC_PI_2) - 1.0).abs() < 1e-6);
        assert!((cos(0.0) - 1.0).abs() < 1e-6);
        assert!((powf(2.0, 10.0) - 1024.0).abs() < 1e-3);
        assert!((ln(exp(1.0)) - 1.0).abs() < 1e-6);
        assert!((atan2(1.0, 1.0) - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
        assert!((tan(0.0)).abs() < 1e-6);
        assert!((asin(1.0) - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!(acos(1.0).abs() < 1e-6);
        assert!((atan(1.0) - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
        let (s, c) = sin_cos(0.0);
        assert!(s.abs() < 1e-6 && (c - 1.0).abs() < 1e-6);
        assert!((sinh(1.0) - 1.175_201_2).abs() < 1e-6);
        assert!((cosh(0.0) - 1.0).abs() < 1e-6);
        assert!((tanh(1.0) - 0.761_594_2).abs() < 1e-6);
        assert!((asinh(sinh(1.0)) - 1.0).abs() < 1e-6);
        assert!((acosh(cosh(1.0)) - 1.0).abs() < 1e-6);
        assert!((atanh(tanh(0.5)) - 0.5).abs() < 1e-6);
        assert!((exp2(10.0) - 1024.0).abs() < 1e-3);
        assert!((exp_m1(0.0)).abs() < 1e-6);
        assert!((log(8.0, 2.0) - 3.0).abs() < 1e-6);
        assert!((log2(1024.0) - 10.0).abs() < 1e-6);
        assert!((log10(1000.0) - 3.0).abs() < 1e-6);
        assert!(ln_1p(0.0).abs() < 1e-6);
        assert!((cbrt(27.0) - 3.0).abs() < 1e-6);
        assert!((hypot(3.0, 4.0) - 5.0).abs() < 1e-6);
    }

    #[test]
    fn f64_wrappers_are_sane() {
        assert!((f64::sqrt(2.0) - std::f64::consts::SQRT_2).abs() < 1e-12);
        assert!((f64::ln(f64::exp(1.0)) - 1.0).abs() < 1e-12);
        assert!((f64::asin(1.0) - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert!(f64::acos(1.0).abs() < 1e-12);
        assert!((f64::atan(1.0) - std::f64::consts::FRAC_PI_4).abs() < 1e-12);
        let (s, c) = f64::sin_cos(0.0);
        assert!(s.abs() < 1e-12 && (c - 1.0).abs() < 1e-12);
        assert!((f64::sinh(1.0) - 1.175_201_193_643_801_4).abs() < 1e-12);
        assert!((f64::cosh(0.0) - 1.0).abs() < 1e-12);
        assert!((f64::tanh(1.0) - 0.761_594_155_955_764_9).abs() < 1e-12);
        assert!((f64::asinh(f64::sinh(1.0)) - 1.0).abs() < 1e-12);
        assert!((f64::acosh(f64::cosh(1.0)) - 1.0).abs() < 1e-12);
        assert!((f64::atanh(f64::tanh(0.5)) - 0.5).abs() < 1e-12);
        assert!((f64::exp2(10.0) - 1024.0).abs() < 1e-9);
        assert!(f64::exp_m1(0.0).abs() < 1e-12);
        assert!((f64::log(8.0, 2.0) - 3.0).abs() < 1e-12);
        assert!((f64::log2(1024.0) - 10.0).abs() < 1e-12);
        assert!((f64::log10(1000.0) - 3.0).abs() < 1e-12);
        assert!(f64::ln_1p(0.0).abs() < 1e-12);
        assert!((f64::cbrt(27.0) - 3.0).abs() < 1e-12);
        assert!((f64::hypot(3.0, 4.0) - 5.0).abs() < 1e-12);
    }
}
