//! A decoded RGBA8 image, and the minimal PNG decoder golden files need.
//!
//! `ludion-testkit` deliberately does **not** depend on `ludion-assets` or `ludion-renderer`
//! directly (the crate graph in `engine-architecture.md` §2 keeps testkit's dependency edge to
//! `ludion-engine` only), so this is a small, local decoder over the same `png` crate
//! `ludion-assets` and `ludion-render-graph` already use — not a new dependency. Golden PNGs are
//! always produced by this crate's own [`crate::golden`] bless flow (via
//! `ludion_renderer::RenderedImage::encode_png`, reached through `ludion_engine::render_scene`), so
//! the decoder only needs to accept plain 8-bit RGBA — the shape our own encoder always writes. The
//! render side of the conversion is a plain field-by-field copy in [`crate::golden`] (its `width` /
//! `height` / `rgba` fields are public; naming the `RenderedImage` type itself would require
//! depending on `ludion-renderer`, which the crate graph reserves for later).

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
/// `L2204` if the bytes are not a decodable PNG.
fn decode_rgba8(bytes: &[u8], path: &Path) -> Result<DecodedImage, Diagnostic> {
    let decode_error = |detail: String| {
        Diagnostic::new(
            code::RENDER_GOLDEN_PNG_DECODE,
            format!(
                "golden image `{}` is not a valid PNG: {detail}",
                path.display()
            ),
        )
        .with_help("re-bless the golden (`ludion test --bless`) to write a fresh, valid PNG")
    };

    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|error| decode_error(format!("not a readable PNG: {error}")))?;
    let mut buffer = vec![0u8; reader.output_buffer_size().unwrap_or(0)];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|error| decode_error(format!("could not decode PNG pixels: {error}")))?;
    buffer.truncate(info.buffer_size());

    let (width, height) = (info.width, info.height);
    let pixels = (width as usize) * (height as usize);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => expand(&buffer, pixels, 3, |src, dst| {
            dst[0..3].copy_from_slice(&src[0..3]);
            dst[3] = 255;
        }),
        png::ColorType::GrayscaleAlpha => expand(&buffer, pixels, 2, |src, dst| {
            dst[0..3].fill(src[0]);
            dst[3] = src[1];
        }),
        png::ColorType::Grayscale => expand(&buffer, pixels, 1, |src, dst| {
            dst[0..3].fill(src[0]);
            dst[3] = 255;
        }),
        other => {
            return Err(decode_error(format!(
                "unsupported PNG color type {other:?} after expansion"
            )));
        }
    };
    Ok(DecodedImage {
        width,
        height,
        rgba,
    })
}

/// Widens `pixels` source samples of `stride` bytes each into RGBA8 via `fill`.
fn expand(src: &[u8], pixels: usize, stride: usize, fill: impl Fn(&[u8], &mut [u8; 4])) -> Vec<u8> {
    let mut out = vec![0u8; pixels * 4];
    for i in 0..pixels {
        let s = &src[i * stride..i * stride + stride];
        let mut px = [0u8; 4];
        fill(s, &mut px);
        out[i * 4..i * 4 + 4].copy_from_slice(&px);
    }
    out
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
}
