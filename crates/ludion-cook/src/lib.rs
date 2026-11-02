#![forbid(unsafe_code)]
#![warn(missing_docs)]
//! The cook stage's encoders: an RGBA8 map becomes a BC7 (desktop) or ASTC 4×4 (iOS) block texture
//! with a full CPU mip chain, serialised as KTX2.
//!
//! Only the CLI links this crate (`Plan/cooked-assets.md` decision 1). The engine computes the
//! content key and reads the result through `ludion_assets::cooked`, so a game never carries an
//! encoder. A [`cook`] call runs on the calling thread; cooking many maps in parallel is the
//! caller's job, and the bytes are identical at any thread count.
//!
//! A level 0 that is not a multiple of 4 is resampled up to the next multiple (padding would shrink
//! the content inside UV 0..1); lower levels that are not whole blocks are padded by edge
//! replication for the encoder only.
//!
//! # Example
//! ```
//! use ludion_assets::Image;
//! use ludion_assets::cooked::{ColorSpace, CookSettings, CookTarget, read_ktx2};
//!
//! let rgba: Vec<u8> = (0..8 * 8).flat_map(|i| [(i * 4) as u8, 255 - (i * 4) as u8, 90, 255]).collect();
//! let image = Image { width: 8, height: 8, rgba, hash: 0 };
//! let settings = CookSettings { target: CookTarget::Desktop, color_space: ColorSpace::Srgb, max_texture_size: None, alpha_cutoff: None };
//!
//! let cooked = ludion_cook::cook(&image, &settings)?;
//! let file = ludion_cook::write_ktx2(&cooked);
//! let read = read_ktx2(&file, settings.target, settings.color_space)?;
//! assert_eq!(read.level_count(), 4); // 8x8, 4x4, 2x2, 1x1
//! assert_eq!(read, cooked);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::fmt;

use ludion_assets::Image;
use ludion_assets::cooked::{CompressedImage, CookSettings};

mod encode;
mod ktx2_write;
mod mips;

pub use ktx2_write::write_ktx2;

/// The BC7 encoder preset (`intel_tex_2`): `opaque_basic`, or `alpha_basic` for a map with any
/// alpha below 255.
pub const BC7_PRESET: &str = "opaque_basic";
/// The ASTC encoder preset (`ctt-astcenc`, the reference astcenc).
pub const ASTC_PRESET: &str = "medium";
/// The encoder crates and the exact versions the cooked bytes depend on.
pub const ENCODER_VERSIONS: [(&str, &str); 2] =
    [("intel_tex_2", "0.5.0"), ("ctt-astcenc", "0.6.0")];
/// The `COOKER_VERSION` [`ENCODER_VERSIONS`] were pinned at.
pub const ENCODER_VERSIONS_FOR: u32 = 3;

/// Why a map could not be cooked.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CookError {
    /// The image is empty or its byte length is not `width * height * 4`.
    BadImage {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
        /// The RGBA byte length supplied.
        len: usize,
    },
    /// The encoder refused the level.
    Encoder(String),
}

impl fmt::Display for CookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CookError::BadImage { width, height, len } => {
                write!(
                    f,
                    "cannot cook a {width}x{height} image holding {len} RGBA bytes (expected {})",
                    u64::from(*width) * u64::from(*height) * 4
                )
            }
            CookError::Encoder(why) => write!(f, "texture encoder failed: {why}"),
        }
    }
}

impl std::error::Error for CookError {}

/// Cooks `image` for `settings`: cap to `max_texture_size`, align level 0 to a multiple of 4, build
/// the mip chain and encode every level. The result's `mean` is `srgb_mean` of the source pixels,
/// taken before any cap or resample.
///
/// # Errors
/// [`CookError::BadImage`] for an empty image or a wrong byte length; [`CookError::Encoder`] if
/// the ASTC encoder rejects a level.
pub fn cook(image: &Image, settings: &CookSettings) -> Result<CompressedImage, CookError> {
    use ludion_assets::cooked::ColorSpace;

    let bad = || CookError::BadImage {
        width: image.width,
        height: image.height,
        len: image.rgba.len(),
    };
    let (width, height) = (image.width as usize, image.height as usize);
    if width == 0
        || height == 0
        || width.checked_mul(height).and_then(|n| n.checked_mul(4)) != Some(image.rgba.len())
    {
        return Err(bad());
    }
    let srgb = settings.color_space == ColorSpace::Srgb;
    let mean = ludion_assets::srgb_mean(&image.rgba);

    let mut level = ludion_assets::mips::Level {
        width,
        height,
        rgba: image.rgba.clone(),
    };
    if let Some(cap) = settings.max_texture_size {
        level = mips::cap_extent(level, cap, srgb);
    }
    let (aligned_width, aligned_height) = (
        level.width.next_multiple_of(4),
        level.height.next_multiple_of(4),
    );
    if (aligned_width, aligned_height) != (level.width, level.height) {
        level = mips::resample(&level, aligned_width, aligned_height, srgb);
    }

    let alpha = level.rgba.chunks_exact(4).any(|texel| texel[3] != 255);
    let format = settings.target.format();
    let mut encoder = encode::Encoder::new(format, settings.color_space, alpha)?;
    let (base_width, base_height) = (level.width as u32, level.height as u32);
    let mut levels = Vec::new();
    for level in chain(level, srgb, settings.alpha_cutoff) {
        levels.push(encoder.encode(&level)?);
    }
    Ok(CompressedImage {
        format,
        srgb,
        width: base_width,
        height: base_height,
        levels,
        mean,
    })
}

/// The mip levels to encode, level 0 first: the plain box-filter chain, or with a mask cutoff the
/// coverage-preserving one so thin cutouts survive minification.
fn chain(
    level0: ludion_assets::mips::Level,
    srgb: bool,
    cutoff: Option<f32>,
) -> Vec<ludion_assets::mips::Level> {
    if let Some(cutoff) = cutoff {
        return ludion_assets::mips::mask_mip_chain(level0, srgb, cutoff);
    }
    let mut levels = vec![level0];
    while let Some(last) = levels.last().filter(|l| l.width != 1 || l.height != 1) {
        let next = ludion_assets::mips::next_mip(last, srgb);
        levels.push(next);
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strands() -> ludion_assets::mips::Level {
        let mut rgba = vec![0u8; 64 * 64 * 4];
        for y in 0..64 {
            for x in 0..64 {
                let texel = &mut rgba[(y * 64 + x) * 4..][..4];
                texel[..3].copy_from_slice(&[(x * 4) as u8, (y * 4) as u8, 90]);
                texel[3] = if x % 8 == 0 {
                    255 - (y * 2 + x / 8 * 3) as u8
                } else {
                    0
                };
            }
        }
        ludion_assets::mips::Level {
            width: 64,
            height: 64,
            rgba,
        }
    }

    #[test]
    fn no_cutoff_is_the_plain_box_chain() {
        let chain = chain(strands(), true, None);
        let mut expected = vec![strands()];
        while expected.last().unwrap().width > 1 {
            let next = ludion_assets::mips::next_mip(expected.last().unwrap(), true);
            expected.push(next);
        }
        assert_eq!(chain, expected);
    }

    #[test]
    fn a_cutoff_is_the_coverage_preserving_chain() {
        assert_eq!(
            chain(strands(), true, Some(0.5)),
            ludion_assets::mips::mask_mip_chain(strands(), true, 0.5)
        );
    }
}
