//! sRGB hex colours ↔ linear RGBA, the one decoder every text document shares.
//!
//! Documents spell a colour as `"#rrggbb"` or `"#rrggbbaa"` (sRGB bytes); the engine works in
//! linear light. Decoding goes through [`math::powf`](crate::math::powf), so the result is
//! bit-identical on every platform.

use crate::math::powf;

/// Decodes one sRGB-encoded channel in `0..=1` to linear light.
///
/// # Example
///
/// ```
/// use ludion_core::color::srgb_to_linear;
/// assert_eq!(srgb_to_linear(0.0), 0.0);
/// assert_eq!(srgb_to_linear(1.0), 1.0);
/// ```
pub fn srgb_to_linear(encoded: f32) -> f32 {
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        powf((encoded + 0.055) / 1.055, 2.4)
    }
}

/// Decodes `#rrggbb` or `#rrggbbaa` (case-insensitive) to linear RGBA; alpha is `byte / 255` and
/// `1.0` when absent. Anything else — `#rgb`, a missing `#`, a non-hex digit — is `None`.
///
/// # Example
///
/// ```
/// use ludion_core::color::parse_hex;
/// assert_eq!(parse_hex("#FFFFFF"), Some([1.0, 1.0, 1.0, 1.0]));
/// assert_eq!(parse_hex("#00000000"), Some([0.0, 0.0, 0.0, 0.0]));
/// assert!(parse_hex("#fff").is_none());
/// ```
pub fn parse_hex(text: &str) -> Option<[f32; 4]> {
    let digits = text.strip_prefix('#')?;
    if !matches!(digits.len(), 6 | 8) || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&digits[i * 2..i * 2 + 2], 16).ok();
    let linear = |i: usize| Some(srgb_to_linear(f32::from(byte(i)?) / 255.0));
    let alpha = if digits.len() == 8 {
        f32::from(byte(3)?) / 255.0
    } else {
        1.0
    };
    Some([linear(0)?, linear(1)?, linear(2)?, alpha])
}

/// The lowercase hex spelling of a linear colour, or `None` when some channel is not exactly what
/// [`parse_hex`] yields for a byte (compared bit for bit, so `-0.0` does not qualify). Alpha is
/// dropped when it is `ff`.
///
/// # Example
///
/// ```
/// use ludion_core::color::{hex_of_linear, parse_hex};
/// let c = parse_hex("#3A7BD580").unwrap().map(f64::from);
/// assert_eq!(hex_of_linear(c).as_deref(), Some("#3a7bd580"));
/// assert_eq!(hex_of_linear([0.5, 0.0, 0.0, 1.0]), None);
/// ```
pub fn hex_of_linear(rgba: [f64; 4]) -> Option<String> {
    let find = |value: f64, decode: fn(u8) -> f32| {
        (0..=255u8).find(|&b| f64::from(decode(b)).to_bits() == value.to_bits())
    };
    let rgb = |b: u8| srgb_to_linear(f32::from(b) / 255.0);
    let alpha = |b: u8| f32::from(b) / 255.0;
    let (r, g, b) = (
        find(rgba[0], rgb)?,
        find(rgba[1], rgb)?,
        find(rgba[2], rgb)?,
    );
    let a = find(rgba[3], alpha)?;
    Some(if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_decodes_to_linear() {
        assert_eq!(parse_hex("#000000"), Some([0.0, 0.0, 0.0, 1.0]));
        assert_eq!(parse_hex("#ffffff"), Some([1.0, 1.0, 1.0, 1.0]));
        let mid = parse_hex("#808080").unwrap()[0];
        assert!((mid - 0.2158605).abs() < 1e-5);
        let half = parse_hex("#ff000080").unwrap()[3];
        assert_eq!(half, 128.0 / 255.0);
    }

    #[test]
    fn hex_is_case_insensitive() {
        assert_eq!(parse_hex("#3A7BD5"), parse_hex("#3a7bd5"));
        assert_eq!(parse_hex("#3A7BD5FF"), parse_hex("#3a7bd5"));
    }

    #[test]
    fn malformed_hex_is_refused() {
        for bad in [
            "#fff", "#ffff", "ffffff", "#gggggg", "#fffffff", "", "#", "#ffffff0",
        ] {
            assert!(parse_hex(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn every_byte_round_trips() {
        for b in 0..=255u8 {
            let text = format!("#{b:02x}{b:02x}{b:02x}{b:02x}");
            let c = parse_hex(&text).unwrap().map(f64::from);
            let expected = if b == 255 {
                "#ffffff".to_string()
            } else {
                text
            };
            assert_eq!(hex_of_linear(c), Some(expected));
        }
    }

    #[test]
    fn inexact_and_negative_zero_have_no_hex() {
        assert_eq!(hex_of_linear([0.5, 0.0, 0.0, 1.0]), None);
        assert_eq!(hex_of_linear([-0.0, 0.0, 0.0, 1.0]), None);
        assert_eq!(
            hex_of_linear([0.0, 0.0, 0.0, 1.0]).as_deref(),
            Some("#000000")
        );
    }
}
