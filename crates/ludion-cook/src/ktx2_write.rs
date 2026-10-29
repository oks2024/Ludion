//! The KTX2 file around a cooked chain: header, level index, DFD, then the levels smallest first
//! (the spec order), each aligned to 16 bytes.

use ktx2::{Format, Header, Index, LevelIndex};
use ludion_assets::cooked::{CompressedFormat, CompressedImage, SRGB_MEAN_KEY, dfd_region};

const ALIGN: usize = 16;

fn vk_format(image: &CompressedImage) -> Format {
    match (image.format, image.srgb) {
        (CompressedFormat::Bc7, true) => Format::BC7_SRGB_BLOCK,
        (CompressedFormat::Bc7, false) => Format::BC7_UNORM_BLOCK,
        (CompressedFormat::Astc4x4, true) => Format::ASTC_4x4_SRGB_BLOCK,
        (CompressedFormat::Astc4x4, false) => Format::ASTC_4x4_UNORM_BLOCK,
    }
}

/// Serialises `image` as a KTX2 file: 2D, one layer and face, no supercompression, and one
/// key/value entry carrying [`CompressedImage::mean`].
///
/// # Example
/// ```
/// use ludion_assets::cooked::{CompressedFormat, CompressedImage};
/// let image = CompressedImage { format: CompressedFormat::Bc7, srgb: true, width: 4, height: 4, levels: vec![vec![0; 16]], mean: [1.0; 4] };
/// assert!(ludion_cook::write_ktx2(&image).starts_with(&[0xAB, b'K', b'T', b'X']));
/// ```
pub fn write_ktx2(image: &CompressedImage) -> Vec<u8> {
    let format = vk_format(image);
    let region = dfd_region(image.format, image.srgb);
    let level_count = image.levels.len();
    let dfd_offset = Header::LENGTH + level_count * LevelIndex::LENGTH;
    let dfd_length = region.len();

    // The key/value block sits right after the descriptor (both are 4-aligned): the entry's byte
    // length, the key, the value, then padding to 4.
    let mut kvd = Vec::new();
    kvd.extend_from_slice(&((SRGB_MEAN_KEY.len() + 16) as u32).to_le_bytes());
    kvd.extend_from_slice(SRGB_MEAN_KEY);
    for c in image.mean {
        kvd.extend_from_slice(&c.to_le_bytes());
    }
    kvd.resize(kvd.len().next_multiple_of(4), 0);
    let kvd_offset = dfd_offset + dfd_length;

    // Smallest level first: offsets run from the last level back to level 0.
    let mut offsets = vec![0usize; level_count];
    let mut cursor = (kvd_offset + kvd.len()).next_multiple_of(ALIGN);
    for (offset, level) in offsets.iter_mut().zip(&image.levels).rev() {
        *offset = cursor;
        cursor = (cursor + level.len()).next_multiple_of(ALIGN);
    }

    let header = Header {
        format: Some(format),
        type_size: 1,
        pixel_width: image.width,
        pixel_height: image.height,
        pixel_depth: 0,
        layer_count: 0,
        face_count: 1,
        level_count: level_count as u32,
        supercompression_scheme: None,
        index: Index {
            dfd_byte_offset: dfd_offset as u32,
            dfd_byte_length: dfd_length as u32,
            kvd_byte_offset: kvd_offset as u32,
            kvd_byte_length: kvd.len() as u32,
            sgd_byte_offset: 0,
            sgd_byte_length: 0,
        },
    };

    let mut out = Vec::with_capacity(cursor);
    out.extend_from_slice(&header.as_bytes());
    for (offset, level) in offsets.iter().zip(&image.levels) {
        let entry = LevelIndex {
            byte_offset: *offset as u64,
            byte_length: level.len() as u64,
            uncompressed_byte_length: level.len() as u64,
        };
        out.extend_from_slice(&entry.as_bytes());
    }
    out.extend_from_slice(&region);
    out.extend_from_slice(&kvd);
    for (offset, level) in offsets.iter().zip(&image.levels).rev() {
        out.resize(*offset, 0);
        out.extend_from_slice(level);
    }
    out.resize(cursor, 0);
    out
}
