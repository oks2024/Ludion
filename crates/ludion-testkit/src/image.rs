//! A decoded RGBA8 image, and the golden PNG decoder.
//!
//! Decoding is `ludion_assets::decode_png`, re-coded to `L2204`: a bad golden is a test failure,
//! not an asset failure.

use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

/// A decoded image in tightly packed, top-to-bottom RGBA8 (no row padding) — the common currency
/// [`crate::ssim`] compares.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DecodedImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Packed RGBA8 bytes, `width * height * 4` long, row-major, top row first.
    pub rgba: Vec<u8>,
}

/// Reads and decodes the golden PNG at `path`.
///
/// # Errors
/// - `L2201` if the file does not exist or cannot be read.
/// - `L2204` if the bytes are not a decodable PNG.
pub fn read_golden(path: &Path) -> Result<DecodedImage, Diagnostic> {
    let bytes = std::fs::read(path).map_err(|error| {
        Diagnostic::new(
            code::RENDER_GOLDEN_MISSING,
            format!("could not read golden image `{}`: {error}", path.display()),
        )
        .with_help("run `ludion test --bless` to create it")
    })?;
    decode_rgba8(&bytes, path)
}

/// Decodes an in-memory PNG (e.g. a screenshot that arrived over the wire, never touching disk) with
/// the exact decoder [`read_golden`] uses — the candidate side of a comparison whose image did not
/// come from an in-process render.
///
/// # Errors
/// `L2204` if the bytes are not a decodable PNG.
pub(crate) fn decode_png(bytes: &[u8]) -> Result<DecodedImage, Diagnostic> {
    decode_rgba8(bytes, Path::new("<png bytes>"))
}

/// Decodes RGBA8/RGB8/grayscale PNG `bytes` (named `path` for diagnostics).
///
/// # Errors
/// `L2204` if the bytes are not a decodable PNG, or the header declares a side over 8192.
fn decode_rgba8(bytes: &[u8], path: &Path) -> Result<DecodedImage, Diagnostic> {
    let (width, height, rgba) = ludion_assets::decode_png(bytes, &path.display().to_string())
        .map_err(|error| {
            Diagnostic::new(
                code::RENDER_GOLDEN_PNG_DECODE,
                format!("golden image: {}", error.message),
            )
            .with_help("re-bless the golden (`ludion test --bless`) to write a fresh, valid PNG")
        })?;
    Ok(DecodedImage {
        width,
        height,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(rgba).unwrap();
        }
        bytes
    }

    #[test]
    fn decodes_rgba_round_trip() {
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 128];
        let bytes = tiny_png(2, 1, &rgba);
        let image = decode_rgba8(&bytes, Path::new("test.png")).unwrap();
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.rgba, rgba);
    }

    #[test]
    fn bad_bytes_are_l2204() {
        let error = decode_rgba8(b"not a png", Path::new("junk.png")).unwrap_err();
        assert_eq!(error.code.as_str(), "L2204");
    }

    #[test]
    fn missing_file_is_l2201() {
        let path = std::env::temp_dir().join("ludion-testkit-definitely-missing-42.png");
        let error = read_golden(&path).unwrap_err();
        assert_eq!(error.code.as_str(), "L2201");
        assert!(error.help.is_some());
    }

    /// A hand-written 16-bit RGBA PNG with a `gAMA` chunk (source gamma 0.45).
    fn rgba16_with_gamma(width: u32, height: u32, data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Sixteen);
            encoder.set_source_gamma(png::ScaledFloat::new(0.45));
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(data).unwrap();
        }
        bytes
    }

    #[test]
    fn decodes_16_bit_and_ignores_gamma() {
        // Two samples per channel byte pair; the decoder keeps the high byte, with no gamma applied.
        let data = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0];
        let image = decode_rgba8(&rgba16_with_gamma(1, 1, &data), Path::new("deep.png")).unwrap();
        assert_eq!(image.rgba, [0x12, 0x56, 0x9a, 0xde]);
    }

    #[test]
    fn an_oversized_header_is_l2204() {
        let bytes = tiny_png(8193, 1, &vec![0u8; 8193 * 4]);
        let error = decode_rgba8(&bytes, Path::new("wide.png")).unwrap_err();
        assert_eq!(error.code.as_str(), "L2204");
        assert!(error.message.contains("8193"), "{}", error.message);
    }
}
