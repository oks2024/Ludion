//! The gap between the `f64` a document yields and the `f32` the engine holds.
//!
//! A value such as `1e39` is a finite `f64` that becomes `inf` once narrowed to `f32`, past every
//! `is_finite()` check that ran on the `f64`, and `1e-46` is a positive `f64` that becomes `0.0`.
//! [`engine_number`] closes the overflow side at the read; [`positive_f32`] judges the underflow
//! side.

/// Returns `value`, or `±inf` when narrowing it to `f32` would overflow. The engine holds document
/// numbers as `f32`, so a number whose narrowing overflows reads as infinite and every existing
/// finiteness check rejects it. NaN and infinities pass through unchanged.
pub fn engine_number(value: f64) -> f64 {
    if !value.is_finite() || fits_f32(value) {
        value
    } else {
        value.signum() * f64::INFINITY
    }
}

/// Whether `value` stays finite once narrowed to `f32`. False for NaN and infinities.
pub fn fits_f32(value: f64) -> bool {
    (value as f32).is_finite()
}

/// Whether `value` narrows to a finite `f32` greater than zero: false for NaN, infinities, zero,
/// negatives, a number past `f32`'s range and a positive number too small for `f32` (below ~1.4e-45,
/// it narrows to `0.0`).
pub fn positive_f32(value: f64) -> bool {
    let narrowed = value as f32;
    narrowed.is_finite() && narrowed > 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflowing_numbers_read_as_infinity() {
        assert_eq!(engine_number(1e39), f64::INFINITY);
        assert_eq!(engine_number(-1e39), f64::NEG_INFINITY);
    }

    #[test]
    fn in_range_numbers_and_non_finite_pass_through() {
        assert_eq!(engine_number(1.5), 1.5);
        assert_eq!(engine_number(3.0e38), 3.0e38);
        assert!(engine_number(f64::NAN).is_nan());
        assert_eq!(engine_number(f64::INFINITY), f64::INFINITY);
    }

    #[test]
    fn fits_f32_rejects_overflow_and_non_finite() {
        assert!(fits_f32(3.0e38));
        assert!(!fits_f32(1e39));
        assert!(!fits_f32(f64::NAN));
        assert!(!fits_f32(f64::INFINITY));
    }

    #[test]
    fn positive_f32_judges_the_narrowed_value() {
        for ok in [1.0, 1.5e-45, f32::MAX as f64] {
            assert!(positive_f32(ok), "{ok}");
        }
        for bad in [1e-46, 0.0, -1.0, 1e39, f64::NAN, f64::INFINITY] {
            assert!(!positive_f32(bad), "{bad}");
        }
    }
}
