//! Block-compressed (cooked) texture uploads: the format mapping and the checks that keep a malformed
//! or mismatched image from ever reaching wgpu. Pure and GPU-free; the upload itself is in `render.rs`.

use std::fmt;

use ludion_assets::cooked::{CompressedFormat, CompressedImage};

use crate::mipgen::mip_level_count;

/// Bytes per 4×4 block in both BC7 and ASTC 4×4.
pub(crate) const BLOCK_BYTES: u32 = 16;

/// Why a cooked image was refused at upload, before any wgpu call.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CompressedUploadError {
    /// The device did not request this format's compression feature.
    Unsupported {
        /// The format the image carries.
        format: CompressedFormat,
    },
    /// The image's colour space is not the one the material slot samples in.
    ColorSpaceMismatch {
        /// Whether the image is sRGB-encoded.
        image_srgb: bool,
        /// Whether the slot decodes sRGB.
        slot_srgb: bool,
    },
    /// Level 0 is zero-sized or not a multiple of the 4×4 block.
    Unaligned {
        /// Level 0 width.
        width: u32,
        /// Level 0 height.
        height: u32,
    },
    /// The image stores no levels.
    NoLevels,
    /// The image stores more levels than its extent has.
    TooManyLevels {
        /// Levels stored.
        levels: u32,
        /// The full chain's length.
        max: u32,
    },
    /// A level's byte length is not its block count times 16.
    LevelSize {
        /// The level.
        level: u32,
        /// The expected byte length.
        expected: usize,
        /// The stored byte length.
        actual: usize,
    },
}

impl fmt::Display for CompressedUploadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { format } => write!(
                f,
                "this GPU cannot sample {format:?}: its device did not offer the compression \
                 feature; sample the source map instead"
            ),
            Self::ColorSpaceMismatch {
                image_srgb,
                slot_srgb,
            } => write!(
                f,
                "the cooked map is {} but its material slot samples {}; a cooked map must be \
                 cooked in the slot's colour space",
                color_space(*image_srgb),
                color_space(*slot_srgb)
            ),
            Self::Unaligned { width, height } => write!(
                f,
                "the cooked map is {width}x{height}; block compression needs a non-zero multiple \
                 of 4 on both sides — re-cook the map"
            ),
            Self::NoLevels => write!(f, "the cooked map stores no mip levels — re-cook the map"),
            Self::TooManyLevels { levels, max } => write!(
                f,
                "the cooked map stores {levels} mip levels but its size has only {max} — re-cook \
                 the map"
            ),
            Self::LevelSize {
                level,
                expected,
                actual,
            } => write!(
                f,
                "mip level {level} of the cooked map is {actual} bytes, expected {expected} — \
                 re-cook the map"
            ),
        }
    }
}

impl std::error::Error for CompressedUploadError {}

fn color_space(srgb: bool) -> &'static str {
    if srgb { "sRGB" } else { "linear" }
}

/// The wgpu format of a cooked map: BC7 or ASTC 4×4, in the sRGB or linear variant.
pub(crate) fn wgpu_format(format: CompressedFormat, srgb: bool) -> wgpu::TextureFormat {
    match (format, srgb) {
        (CompressedFormat::Bc7, true) => wgpu::TextureFormat::Bc7RgbaUnormSrgb,
        (CompressedFormat::Bc7, false) => wgpu::TextureFormat::Bc7RgbaUnorm,
        (CompressedFormat::Astc4x4, true) => wgpu::TextureFormat::Astc {
            block: wgpu::AstcBlock::B4x4,
            channel: wgpu::AstcChannel::UnormSrgb,
        },
        (CompressedFormat::Astc4x4, false) => wgpu::TextureFormat::Astc {
            block: wgpu::AstcBlock::B4x4,
            channel: wgpu::AstcChannel::Unorm,
        },
    }
}

/// Checks `image` is safe to hand to wgpu for a slot that samples in `slot_srgb`: the colour spaces
/// agree (sampling sRGB bytes as linear, or the reverse, is the silent trap), level 0 is block
/// aligned, and every stored level has exactly its block count of bytes.
pub(crate) fn validate(
    image: &CompressedImage,
    slot_srgb: bool,
) -> Result<(), CompressedUploadError> {
    if image.srgb != slot_srgb {
        return Err(CompressedUploadError::ColorSpaceMismatch {
            image_srgb: image.srgb,
            slot_srgb,
        });
    }
    let (width, height) = (image.width, image.height);
    if width == 0 || height == 0 || width % 4 != 0 || height % 4 != 0 {
        return Err(CompressedUploadError::Unaligned { width, height });
    }
    let levels = image.level_count();
    if levels == 0 {
        return Err(CompressedUploadError::NoLevels);
    }
    let max = mip_level_count(width, height);
    if levels > max {
        return Err(CompressedUploadError::TooManyLevels { levels, max });
    }
    for (level, bytes) in image.levels.iter().enumerate() {
        let level = level as u32;
        let (bw, bh) = image.blocks(level);
        let expected = (bw * bh * BLOCK_BYTES) as usize;
        if bytes.len() != expected {
            return Err(CompressedUploadError::LevelSize {
                level,
                expected,
                actual: bytes.len(),
            });
        }
    }
    Ok(())
}

/// Checks a cooked image is safe to upload for a slot that samples in `slot_srgb` (the device
/// support check aside): the colour spaces agree, level 0 is block aligned and every level has its
/// block count of bytes. The engine runs it when it loads a cooked map, so a bad file falls back to
/// the source PNG and never reaches the GPU.
///
/// # Example
/// ```
/// use ludion_assets::cooked::{CompressedFormat, CompressedImage};
/// use ludion_renderer::{CompressedUploadError, check_compressed};
/// let image = CompressedImage {
///     format: CompressedFormat::Bc7,
///     srgb: true,
///     width: 4,
///     height: 4,
///     levels: vec![vec![0; 16]],
///     mean: [1.0; 4],
/// };
/// assert_eq!(check_compressed(&image, true), Ok(()));
/// assert!(matches!(
///     check_compressed(&image, false),
///     Err(CompressedUploadError::ColorSpaceMismatch { .. })
/// ));
/// ```
pub fn check_compressed(
    image: &CompressedImage,
    slot_srgb: bool,
) -> Result<(), CompressedUploadError> {
    validate(image, slot_srgb)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32, levels: u32, srgb: bool) -> CompressedImage {
        let mut image = CompressedImage {
            format: CompressedFormat::Bc7,
            srgb,
            width,
            height,
            levels: Vec::new(),
            mean: [1.0; 4],
        };
        image.levels = (0..levels)
            .map(|level| {
                let (bw, bh) = image.blocks(level);
                vec![0; (bw * bh * BLOCK_BYTES) as usize]
            })
            .collect();
        image
    }

    #[test]
    fn the_four_formats_map_to_their_wgpu_variants() {
        use wgpu::{AstcBlock::B4x4, AstcChannel, TextureFormat as F};
        assert_eq!(
            wgpu_format(CompressedFormat::Bc7, true),
            F::Bc7RgbaUnormSrgb
        );
        assert_eq!(wgpu_format(CompressedFormat::Bc7, false), F::Bc7RgbaUnorm);
        assert_eq!(
            wgpu_format(CompressedFormat::Astc4x4, true),
            F::Astc {
                block: B4x4,
                channel: AstcChannel::UnormSrgb
            }
        );
        assert_eq!(
            wgpu_format(CompressedFormat::Astc4x4, false),
            F::Astc {
                block: B4x4,
                channel: AstcChannel::Unorm
            }
        );
    }

    #[test]
    fn a_full_chain_down_to_one_texel_validates() {
        assert_eq!(validate(&image(64, 32, 7, true), true), Ok(()));
        assert_eq!(validate(&image(8, 8, 1, false), false), Ok(()));
    }

    #[test]
    fn a_colour_space_mismatch_is_refused() {
        assert_eq!(
            validate(&image(8, 8, 1, true), false),
            Err(CompressedUploadError::ColorSpaceMismatch {
                image_srgb: true,
                slot_srgb: false
            })
        );
    }

    #[test]
    fn an_unaligned_or_empty_extent_is_refused() {
        for (width, height) in [(6, 8), (8, 6), (0, 8), (8, 0)] {
            let mut bad = image(8, 8, 1, true);
            bad.width = width;
            bad.height = height;
            assert_eq!(
                validate(&bad, true),
                Err(CompressedUploadError::Unaligned { width, height })
            );
        }
    }

    #[test]
    fn a_bad_level_count_is_refused() {
        assert_eq!(
            validate(&image(8, 8, 0, true), true),
            Err(CompressedUploadError::NoLevels)
        );
        let mut many = image(8, 8, 4, true);
        many.levels.push(vec![0; 16]);
        assert_eq!(
            validate(&many, true),
            Err(CompressedUploadError::TooManyLevels { levels: 5, max: 4 })
        );
    }

    #[test]
    fn a_level_of_the_wrong_length_is_refused() {
        let mut bad = image(8, 8, 3, true);
        bad.levels[1].pop();
        assert_eq!(
            validate(&bad, true),
            Err(CompressedUploadError::LevelSize {
                level: 1,
                expected: 16,
                actual: 15
            })
        );
    }

    #[test]
    fn errors_say_what_to_do() {
        let text = CompressedUploadError::Unsupported {
            format: CompressedFormat::Astc4x4,
        }
        .to_string();
        assert!(
            text.contains("Astc4x4") && text.contains("source"),
            "{text}"
        );
    }
}
