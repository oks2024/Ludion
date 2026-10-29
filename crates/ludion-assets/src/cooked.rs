//! The runtime half of the texture cook: the content key, the shared types and the KTX2 reader.
//!
//! A *cooked* map is a source PNG's pixels compressed to BC7 (desktop) or ASTC 4×4 (iOS) with a
//! full CPU mip chain, stored in a KTX2 file named by [`cook_key`]. The encoders live in
//! `ludion-cook`, which only the CLI links; everything the engine needs to *find and read* a cooked
//! map lives here, GPU-free, so the runtime never carries an encoder.
//!
//! # Example
//! ```no_run
//! use ludion_assets::cooked::{ColorSpace, CookTarget, read_ktx2};
//! let bytes = std::fs::read("0123456789abcdef.ktx2")?;
//! let image = read_ktx2(&bytes, CookTarget::Desktop, ColorSpace::Srgb)?;
//! assert_eq!(image.levels.len() as u32, image.level_count());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::fmt;

use ktx2::{Format, Header, Index, LevelIndex, Reader, dfd};
use xxhash_rust::xxh3::xxh3_64;

/// The cook format's version. Part of every [`cook_key`], so bumping it invalidates every cooked
/// map. Bump it whenever an encoder, its preset, the mip filter or the file layout changes the
/// output bytes (`ludion-cook`'s pin test enforces the encoder half).
pub const COOKER_VERSION: u32 = 2;

/// Which GPU family a map is cooked for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CookTarget {
    /// BC7, for desktop GPUs.
    Desktop,
    /// ASTC 4×4, for iOS.
    Ios,
}

impl CookTarget {
    /// The store directory name and key tag: `"desktop"` or `"ios"`.
    pub fn name(self) -> &'static str {
        match self {
            CookTarget::Desktop => "desktop",
            CookTarget::Ios => "ios",
        }
    }

    /// The target a store directory name or manifest entry names; the inverse of [`CookTarget::name`].
    ///
    /// # Example
    /// ```
    /// use ludion_assets::cooked::CookTarget;
    /// assert_eq!(CookTarget::from_name("ios"), Some(CookTarget::Ios));
    /// assert_eq!(CookTarget::from_name("web"), None);
    /// ```
    pub fn from_name(name: &str) -> Option<CookTarget> {
        match name {
            "desktop" => Some(CookTarget::Desktop),
            "ios" => Some(CookTarget::Ios),
            _ => None,
        }
    }

    /// The block format this target ships.
    pub fn format(self) -> CompressedFormat {
        match self {
            CookTarget::Desktop => CompressedFormat::Bc7,
            CookTarget::Ios => CompressedFormat::Astc4x4,
        }
    }
}

/// How a map's bytes are interpreted: colour is sRGB-encoded, data maps are linear.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum ColorSpace {
    /// sRGB-encoded colour (albedo, emissive).
    Srgb,
    /// Linear data (normal, metallic-roughness, occlusion).
    Linear,
}

/// Everything besides the source bytes that decides a cooked map's bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CookSettings {
    /// The GPU family.
    pub target: CookTarget,
    /// The map's colour space.
    pub color_space: ColorSpace,
    /// Halve the map until neither side exceeds this (`None`: keep the source size).
    pub max_texture_size: Option<u32>,
}

/// A GPU block-compression format, 16 bytes per 4×4 block.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CompressedFormat {
    /// BC7 (desktop).
    Bc7,
    /// ASTC 4×4 LDR (iOS).
    Astc4x4,
}

/// A decoded KTX2 file: a block-compressed texture with its whole mip chain.
#[derive(Clone, PartialEq, Debug)]
pub struct CompressedImage {
    /// The block format of every level.
    pub format: CompressedFormat,
    /// Whether the colour is sRGB-encoded (the format's `_SRGB` variant).
    pub srgb: bool,
    /// Level 0 width in texels (a multiple of 4).
    pub width: u32,
    /// Level 0 height in texels (a multiple of 4).
    pub height: u32,
    /// Block bytes per level, level 0 first.
    pub levels: Vec<Vec<u8>>,
    /// [`srgb_mean`](crate::srgb_mean) of the source pixels before any cap or resample; stored in
    /// the file's key/value block.
    pub mean: [f32; 4],
}

/// The key/value entry that carries a cooked map's [`CompressedImage::mean`]: four little-endian `f32`.
pub const SRGB_MEAN_KEY: &[u8] = b"LudionSrgbMean\0";

impl CompressedImage {
    /// A level's size in texels (each side halves, never below 1).
    pub fn level_extent(&self, level: u32) -> (u32, u32) {
        (
            (self.width >> level.min(31)).max(1),
            (self.height >> level.min(31)).max(1),
        )
    }

    /// A level's size in blocks (a partial block counts, so 1×1 is one block).
    pub fn blocks(&self, level: u32) -> (u32, u32) {
        let (w, h) = self.level_extent(level);
        (w.div_ceil(4), h.div_ceil(4))
    }

    /// The number of levels stored.
    pub fn level_count(&self) -> u32 {
        self.levels.len() as u32
    }
}

/// The content key of one cooked map: XXH3-64 over a fixed little-endian record of the source
/// file's hash ([`Image::hash`](crate::Image::hash)), [`COOKER_VERSION`] and every field of
/// `settings`. Equal key means equal cooked bytes; a changed source, setting or cooker moves it.
///
/// # Example
/// ```
/// use ludion_assets::cooked::{ColorSpace, CookSettings, CookTarget, cook_key};
/// let settings = CookSettings { target: CookTarget::Desktop, color_space: ColorSpace::Srgb, max_texture_size: None };
/// assert_eq!(cook_key(7, &settings), cook_key(7, &settings));
/// assert_ne!(cook_key(7, &settings), cook_key(8, &settings));
/// ```
pub fn cook_key(source_hash: u64, settings: &CookSettings) -> u64 {
    let mut record = Vec::with_capacity(22);
    record.extend_from_slice(b"LCK1");
    record.extend_from_slice(&source_hash.to_le_bytes());
    record.extend_from_slice(&COOKER_VERSION.to_le_bytes());
    record.push(match settings.target {
        CookTarget::Desktop => 0,
        CookTarget::Ios => 1,
    });
    record.push(match settings.color_space {
        ColorSpace::Srgb => 0,
        ColorSpace::Linear => 1,
    });
    record.extend_from_slice(&settings.max_texture_size.unwrap_or(0).to_le_bytes());
    xxh3_64(&record)
}

/// The store file name for a key: sixteen lowercase hex digits and `.ktx2`.
///
/// # Example
/// ```
/// assert_eq!(ludion_assets::cooked::cooked_file_name(0xab), "00000000000000ab.ktx2");
/// ```
pub fn cooked_file_name(key: u64) -> String {
    format!("{key:016x}.ktx2")
}

/// Why a KTX2 file was refused.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ktx2Error {
    /// The container is malformed or truncated.
    Parse(String),
    /// The file's format is not the one the target and colour space imply.
    FormatMismatch {
        /// The `VkFormat` the request implies.
        expected: u32,
        /// The `VkFormat` the file carries (0 for undefined).
        found: u32,
    },
    /// Supercompressed levels are not supported.
    Supercompressed,
    /// 3D, array or cube textures are not supported.
    UnsupportedLayout,
    /// Level 0 is zero-sized or not a multiple of the 4×4 block.
    BadExtent {
        /// Header width.
        width: u32,
        /// Header height.
        height: u32,
    },
    /// The level count is zero or above the full chain.
    BadLevelCount {
        /// The header's level count.
        found: u32,
        /// The full chain's length.
        max: u32,
    },
    /// A level's byte length is not its block count times 16.
    BadLevelLength {
        /// The level.
        level: u32,
        /// The expected byte length.
        expected: u64,
        /// The stored byte length.
        found: u64,
    },
}

impl fmt::Display for Ktx2Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ktx2Error::Parse(why) => write!(f, "malformed KTX2 container: {why}"),
            Ktx2Error::FormatMismatch { expected, found } => {
                write!(f, "KTX2 format {found} where the target implies {expected}")
            }
            Ktx2Error::Supercompressed => {
                f.write_str("supercompressed KTX2 levels are not supported")
            }
            Ktx2Error::UnsupportedLayout => {
                f.write_str("KTX2 file is 3D, an array or a cube map; only plain 2D is supported")
            }
            Ktx2Error::BadExtent { width, height } => {
                write!(
                    f,
                    "KTX2 level 0 is {width}x{height}; a cooked map is a non-empty multiple of 4 per side"
                )
            }
            Ktx2Error::BadLevelCount { found, max } => {
                write!(f, "KTX2 level count {found} is outside 1..={max}")
            }
            Ktx2Error::BadLevelLength {
                level,
                expected,
                found,
            } => {
                write!(
                    f,
                    "KTX2 level {level} holds {found} bytes, expected {expected}"
                )
            }
        }
    }
}

impl std::error::Error for Ktx2Error {}

/// The `VkFormat` a target and colour space imply.
fn expected_format(target: CookTarget, color_space: ColorSpace) -> Format {
    expected_format_of(target.format(), color_space == ColorSpace::Srgb)
}

fn expected_format_of(format: CompressedFormat, srgb: bool) -> Format {
    match (format, srgb) {
        (CompressedFormat::Bc7, true) => Format::BC7_SRGB_BLOCK,
        (CompressedFormat::Bc7, false) => Format::BC7_UNORM_BLOCK,
        (CompressedFormat::Astc4x4, true) => Format::ASTC_4x4_SRGB_BLOCK,
        (CompressedFormat::Astc4x4, false) => Format::ASTC_4x4_UNORM_BLOCK,
    }
}

/// The data format descriptor region of a cooked file: a little-endian `u32` total size, then the
/// basic block `ktx2` generates for the format. `write_ktx2` emits exactly this and `read_ktx2`
/// accepts nothing else.
pub fn dfd_region(format: CompressedFormat, srgb: bool) -> Vec<u8> {
    dfd_region_of(expected_format_of(format, srgb))
}

fn dfd_region_of(vk: Format) -> Vec<u8> {
    let block = match dfd::Basic::from_format(vk) {
        Ok((basic, _)) => dfd::Block::Basic(basic).to_vec(),
        // Every format used here is in ktx2's DFD table; an empty block still yields a well-formed
        // prefix, and writer and reader agree by construction.
        Err(_) => Vec::new(),
    };
    let mut region = ((block.len() + 4) as u32).to_le_bytes().to_vec();
    region.extend_from_slice(&block);
    region
}

/// Reads a cooked KTX2 file, validating that it is exactly the format `target` and `color_space`
/// imply and that every level is whole.
///
/// # Errors
/// [`Ktx2Error`] for a malformed or truncated container, a format mismatch, an unsupported layout
/// or an inconsistent level table. Never panics on file data.
///
/// # Example
/// ```no_run
/// use ludion_assets::cooked::{ColorSpace, CookTarget, read_ktx2};
/// let bytes = std::fs::read("0123456789abcdef.ktx2")?;
/// let image = read_ktx2(&bytes, CookTarget::Ios, ColorSpace::Linear)?;
/// assert_eq!(image.width % 4, 0);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn read_ktx2(
    bytes: &[u8],
    target: CookTarget,
    color_space: ColorSpace,
) -> Result<CompressedImage, Ktx2Error> {
    let expected = expected_format(target, color_space);
    // `ktx2` 0.5 panics on some corrupt descriptors, so `Reader::new` only ever sees the exact
    // descriptor this format's writer emits; anything else is refused before it parses.
    let stored_format = bytes
        .get(12..16)
        .and_then(|field| field.try_into().ok())
        .map(u32::from_le_bytes);
    if stored_format != Some(expected.value()) {
        return Err(Ktx2Error::FormatMismatch {
            expected: expected.value(),
            found: stored_format.unwrap_or(0),
        });
    }
    let field = |at: usize| {
        bytes
            .get(at..at + 4)
            .and_then(|field| field.try_into().ok())
            .map(u32::from_le_bytes)
    };
    let region = dfd_region(target.format(), color_space == ColorSpace::Srgb);
    let stored_region = field(48)
        .zip(field(52))
        .filter(|(_, length)| *length as usize == region.len())
        .and_then(|(offset, length)| {
            bytes.get(offset as usize..(offset as usize).checked_add(length as usize)?)
        });
    if stored_region != Some(region.as_slice()) {
        return Err(Ktx2Error::Parse(
            "unexpected data format descriptor".to_string(),
        ));
    }
    let reader = Reader::new(bytes).map_err(|error| Ktx2Error::Parse(error.to_string()))?;
    let header = reader.header();
    if header.format != Some(expected) {
        return Err(Ktx2Error::FormatMismatch {
            expected: expected.value(),
            found: header.format.map_or(0, |format| format.value()),
        });
    }
    if header.supercompression_scheme.is_some() {
        return Err(Ktx2Error::Supercompressed);
    }
    if header.pixel_depth != 0 || header.layer_count > 1 || header.face_count != 1 {
        return Err(Ktx2Error::UnsupportedLayout);
    }
    let (width, height) = (header.pixel_width, header.pixel_height);
    if width == 0 || height == 0 || width % 4 != 0 || height % 4 != 0 {
        return Err(Ktx2Error::BadExtent { width, height });
    }
    let max = 32 - width.max(height).leading_zeros();
    if header.level_count == 0 || header.level_count > max {
        return Err(Ktx2Error::BadLevelCount {
            found: header.level_count,
            max,
        });
    }
    let mean = field(56)
        .zip(field(60))
        .and_then(|(offset, length)| {
            bytes.get(offset as usize..(offset as usize).checked_add(length as usize)?)
        })
        .and_then(srgb_mean_entry)
        .ok_or_else(|| Ktx2Error::Parse("no base-colour mean entry".to_string()))?;
    let mut image = CompressedImage {
        format: target.format(),
        srgb: color_space == ColorSpace::Srgb,
        width,
        height,
        levels: Vec::with_capacity(header.level_count as usize),
        mean,
    };
    for (index, level) in reader.levels().enumerate() {
        let index = index as u32;
        let (blocks_x, blocks_y) = image.blocks(index);
        let expected = u64::from(blocks_x) * u64::from(blocks_y) * 16;
        if level.data.len() as u64 != expected {
            return Err(Ktx2Error::BadLevelLength {
                level: index,
                expected,
                found: level.data.len() as u64,
            });
        }
        image.levels.push(level.data.to_vec());
    }
    Ok(image)
}

/// Bytes per texel of a probe cube: `R16G16B16A16_SFLOAT`.
const PROBE_TEXEL_BYTES: u64 = 8;
const PROBE_ALIGN: usize = 16;
/// The key/value entry that carries a probe's baked irradiance, one `[f32; 3]` per cube axis (+X −X +Y −Y +Z −Z): 18 little-endian `f32`.
const PROBE_IRRADIANCE_KEY: &[u8] = b"LudionProbeIrradiance\0";

fn probe_level_bytes(face: u32, level: u32) -> u64 {
    let edge = u64::from((face >> level).max(1));
    6 * edge * edge * PROBE_TEXEL_BYTES
}

/// A probe cube read back from its KTX2 entry.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeCube {
    /// Mip `l`'s six faces back to back (+X, −X, +Y, −Y, +Z, −Z), tight rows.
    pub levels: Vec<Vec<u8>>,
    /// The emissive part of the same cube, same layout; `None` for a probe that sees no emissive.
    pub emissive: Option<Vec<Vec<u8>>>,
    /// The probe's baked diffuse irradiance, per cube axis (+X −X +Y −Y +Z −Z).
    pub irradiance: [[f32; 3]; 6],
}

/// Serialises a prefiltered probe cube as a KTX2 file: `R16G16B16A16_SFLOAT`, six faces,
/// no supercompression. `levels[l]` holds mip `l`'s six faces back to back (+X, −X, +Y, −Y, +Z, −Z),
/// each `edge × edge` texels with tight rows, `edge = max(face >> l, 1)`. With `emissive` (same
/// layout) the file is a two-layer cube array, each level holding the lit layer's faces then the
/// emissive layer's; without it, no layers. `irradiance` is the probe's baked diffuse irradiance per cube
/// axis, stored as one key/value entry.
///
/// # Example
/// ```
/// use ludion_assets::cooked::{ProbeCube, read_probe_cube, write_probe_cube};
/// let levels = vec![vec![7u8; 6 * 2 * 2 * 8], vec![9u8; 6 * 8]];
/// let bytes = write_probe_cube(2, &levels, None, [[0.5, 1.0, 2.0]; 6]);
/// assert_eq!(
///     read_probe_cube(&bytes, 2, 2)?,
///     ProbeCube { levels, emissive: None, irradiance: [[0.5, 1.0, 2.0]; 6] }
/// );
/// # Ok::<(), ludion_assets::cooked::Ktx2Error>(())
/// ```
pub fn write_probe_cube(
    face: u32,
    levels: &[Vec<u8>],
    emissive: Option<&[Vec<u8>]>,
    irradiance: [[f32; 3]; 6],
) -> Vec<u8> {
    let joined: Vec<Vec<u8>> = match emissive {
        Some(emissive) => levels
            .iter()
            .zip(emissive)
            .map(|(lit, em)| [lit.as_slice(), em.as_slice()].concat())
            .collect(),
        None => Vec::new(),
    };
    let levels = if emissive.is_some() { &joined } else { levels };
    let region = dfd_region_of(Format::R16G16B16A16_SFLOAT);
    let level_count = levels.len();
    let dfd_offset = Header::LENGTH + level_count * LevelIndex::LENGTH;

    // The key/value block sits right after the descriptor (both are 4-aligned): the entry's byte
    // length, the key, the value, then padding to 4.
    let mut kvd = Vec::new();
    kvd.extend_from_slice(&((PROBE_IRRADIANCE_KEY.len() + 72) as u32).to_le_bytes());
    kvd.extend_from_slice(PROBE_IRRADIANCE_KEY);
    for c in irradiance.iter().flatten() {
        kvd.extend_from_slice(&c.to_le_bytes());
    }
    kvd.resize(kvd.len().next_multiple_of(4), 0);
    let kvd_offset = dfd_offset + region.len();

    // Smallest level first: offsets run from the last level back to level 0.
    let mut offsets = vec![0usize; level_count];
    let mut cursor = (kvd_offset + kvd.len()).next_multiple_of(PROBE_ALIGN);
    for (offset, level) in offsets.iter_mut().zip(levels).rev() {
        *offset = cursor;
        cursor = (cursor + level.len()).next_multiple_of(PROBE_ALIGN);
    }

    let header = Header {
        format: Some(Format::R16G16B16A16_SFLOAT),
        type_size: 2,
        pixel_width: face,
        pixel_height: face,
        pixel_depth: 0,
        layer_count: if emissive.is_some() { 2 } else { 0 },
        face_count: 6,
        level_count: level_count as u32,
        supercompression_scheme: None,
        index: Index {
            dfd_byte_offset: dfd_offset as u32,
            dfd_byte_length: region.len() as u32,
            kvd_byte_offset: kvd_offset as u32,
            kvd_byte_length: kvd.len() as u32,
            sgd_byte_offset: 0,
            sgd_byte_length: 0,
        },
    };

    let mut out = Vec::with_capacity(cursor);
    out.extend_from_slice(&header.as_bytes());
    for (offset, level) in offsets.iter().zip(levels) {
        let entry = LevelIndex {
            byte_offset: *offset as u64,
            byte_length: level.len() as u64,
            uncompressed_byte_length: level.len() as u64,
        };
        out.extend_from_slice(&entry.as_bytes());
    }
    out.extend_from_slice(&region);
    out.extend_from_slice(&kvd);
    for (offset, level) in offsets.iter().zip(levels).rev() {
        out.resize(*offset, 0);
        out.extend_from_slice(level);
    }
    out.resize(cursor, 0);
    out
}

/// Reads a probe cube written by [`write_probe_cube`], validating that it is exactly a
/// `face × face` cube of `level_count` whole levels, with no layers or the two of an emissive entry.
///
/// # Errors
/// [`Ktx2Error`] for a malformed or truncated container, another format, supercompression, a layout
/// that is not a six-face cube (one or two layers) of the requested extent, a level of the wrong length, or a missing
/// irradiance entry. Never panics on file data.
///
/// # Example
/// ```
/// use ludion_assets::cooked::{Ktx2Error, read_probe_cube};
/// assert!(matches!(read_probe_cube(&[0; 8], 2, 1), Err(Ktx2Error::FormatMismatch { .. })));
/// ```
pub fn read_probe_cube(bytes: &[u8], face: u32, level_count: u32) -> Result<ProbeCube, Ktx2Error> {
    let expected = Format::R16G16B16A16_SFLOAT;
    // `ktx2` 0.5 panics on some corrupt descriptors, so `Reader::new` only ever sees the exact
    // descriptor the writer emits.
    let stored_format = bytes
        .get(12..16)
        .and_then(|field| field.try_into().ok())
        .map(u32::from_le_bytes);
    if stored_format != Some(expected.value()) {
        return Err(Ktx2Error::FormatMismatch {
            expected: expected.value(),
            found: stored_format.unwrap_or(0),
        });
    }
    let field = |at: usize| {
        bytes
            .get(at..at + 4)
            .and_then(|field| field.try_into().ok())
            .map(u32::from_le_bytes)
    };
    let region = dfd_region_of(expected);
    let stored_region = field(48)
        .zip(field(52))
        .filter(|(_, length)| *length as usize == region.len())
        .and_then(|(offset, length)| {
            bytes.get(offset as usize..(offset as usize).checked_add(length as usize)?)
        });
    if stored_region != Some(region.as_slice()) {
        return Err(Ktx2Error::Parse(
            "unexpected data format descriptor".to_string(),
        ));
    }
    let reader = Reader::new(bytes).map_err(|error| Ktx2Error::Parse(error.to_string()))?;
    let header = reader.header();
    if header.supercompression_scheme.is_some() {
        return Err(Ktx2Error::Supercompressed);
    }
    if header.pixel_depth != 0 || !matches!(header.layer_count, 0 | 2) || header.face_count != 6 {
        return Err(Ktx2Error::Parse(
            "a probe cube has six faces, no depth and no layers or an emissive pair".to_string(),
        ));
    }
    let layers = u64::from(header.layer_count.max(1));
    if (header.pixel_width, header.pixel_height) != (face, face) {
        return Err(Ktx2Error::Parse(format!(
            "probe cube is {}x{}, expected {face}x{face}",
            header.pixel_width, header.pixel_height
        )));
    }
    if header.level_count != level_count {
        return Err(Ktx2Error::BadLevelCount {
            found: header.level_count,
            max: level_count,
        });
    }
    let irradiance = field(56)
        .zip(field(60))
        .and_then(|(offset, length)| {
            bytes.get(offset as usize..(offset as usize).checked_add(length as usize)?)
        })
        .and_then(probe_irradiance)
        .ok_or_else(|| Ktx2Error::Parse("the probe irradiance entry is missing".to_string()))?;
    let mut levels = Vec::with_capacity(level_count as usize);
    let mut emissive = Vec::new();
    for (index, level) in reader.levels().enumerate() {
        let one = probe_level_bytes(face, index as u32);
        let expected = one * layers;
        if level.data.len() as u64 != expected {
            return Err(Ktx2Error::BadLevelLength {
                level: index as u32,
                expected,
                found: level.data.len() as u64,
            });
        }
        if layers == 2 {
            let (lit, em) = level.data.split_at(one as usize);
            levels.push(lit.to_vec());
            emissive.push(em.to_vec());
        } else {
            levels.push(level.data.to_vec());
        }
    }
    if levels.len() as u32 != level_count {
        return Err(Ktx2Error::Parse(
            "the level index is incomplete".to_string(),
        ));
    }
    Ok(ProbeCube {
        levels,
        emissive: (layers == 2).then_some(emissive),
        irradiance,
    })
}

/// The mean value of the key/value block `kvd`, if its first entry is [`SRGB_MEAN_KEY`]'s.
fn srgb_mean_entry(kvd: &[u8]) -> Option<[f32; 4]> {
    let length = u32::from_le_bytes(kvd.get(..4)?.try_into().ok()?) as usize;
    let entry = kvd.get(4..4usize.checked_add(length)?)?;
    let value = entry.strip_prefix(SRGB_MEAN_KEY)?;
    if value.len() != 16 {
        return None;
    }
    Some(std::array::from_fn(|i| {
        f32::from_le_bytes(value[i * 4..i * 4 + 4].try_into().expect("4 bytes"))
    }))
}

/// The irradiance value of the key/value block `kvd`, if its first entry is the probe's.
fn probe_irradiance(kvd: &[u8]) -> Option<[[f32; 3]; 6]> {
    let length = u32::from_le_bytes(kvd.get(..4)?.try_into().ok()?) as usize;
    let entry = kvd.get(4..4usize.checked_add(length)?)?;
    let value = entry.strip_prefix(PROBE_IRRADIANCE_KEY)?;
    if value.len() != 72 {
        return None;
    }
    let c = |i: usize| f32::from_le_bytes(value[i * 4..i * 4 + 4].try_into().expect("4 bytes"));
    Some(std::array::from_fn(|a| {
        [c(a * 3), c(a * 3 + 1), c(a * 3 + 2)]
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: CookSettings = CookSettings {
        target: CookTarget::Desktop,
        color_space: ColorSpace::Srgb,
        max_texture_size: None,
    };

    #[test]
    fn key_is_pinned() {
        // Moving this value means the key record changed: every cooked store is orphaned.
        assert_eq!(
            cook_key(0x0123_4567_89ab_cdef, &BASE),
            0x01ce_6a01_0eac_3d42
        );
    }

    #[test]
    fn every_field_moves_the_key() {
        let key = cook_key(1, &BASE);
        assert_ne!(key, cook_key(2, &BASE));
        assert_ne!(
            key,
            cook_key(
                1,
                &CookSettings {
                    target: CookTarget::Ios,
                    ..BASE
                }
            )
        );
        assert_ne!(
            key,
            cook_key(
                1,
                &CookSettings {
                    color_space: ColorSpace::Linear,
                    ..BASE
                }
            )
        );
        assert_ne!(
            key,
            cook_key(
                1,
                &CookSettings {
                    max_texture_size: Some(512),
                    ..BASE
                }
            )
        );
        assert_ne!(
            cook_key(
                1,
                &CookSettings {
                    max_texture_size: Some(512),
                    ..BASE
                }
            ),
            cook_key(
                1,
                &CookSettings {
                    max_texture_size: Some(1024),
                    ..BASE
                }
            )
        );
    }

    #[test]
    fn file_name_is_sixteen_hex_digits() {
        assert_eq!(cooked_file_name(0xab), "00000000000000ab.ktx2");
        assert_eq!(cooked_file_name(u64::MAX), "ffffffffffffffff.ktx2");
    }

    #[test]
    fn blocks_never_reach_zero() {
        let image = CompressedImage {
            format: CompressedFormat::Bc7,
            srgb: true,
            width: 8,
            height: 4,
            levels: vec![],
            mean: [1.0; 4],
        };
        assert_eq!(image.blocks(0), (2, 1));
        assert_eq!(image.blocks(3), (1, 1));
        assert_eq!(image.level_extent(2), (2, 1));
    }

    fn probe_levels(face: u32, count: u32) -> Vec<Vec<u8>> {
        (0..count)
            .map(|l| vec![l as u8 + 1; probe_level_bytes(face, l) as usize])
            .collect()
    }

    #[test]
    fn a_probe_cube_round_trips() {
        let levels = probe_levels(8, 4);
        let bytes = write_probe_cube(8, &levels, None, [[0.25, 3.0, 1e-7]; 6]);
        assert_eq!(
            read_probe_cube(&bytes, 8, 4).unwrap(),
            ProbeCube {
                levels,
                emissive: None,
                irradiance: [[0.25, 3.0, 1e-7]; 6]
            }
        );
    }

    #[test]
    fn a_two_layer_probe_cube_round_trips_its_emissive_layer() {
        let levels = probe_levels(8, 4);
        let emissive: Vec<Vec<u8>> = levels.iter().map(|l| vec![200; l.len()]).collect();
        let bytes = write_probe_cube(8, &levels, Some(&emissive), [[1.0, 2.0, 3.0]; 6]);
        assert_eq!(
            read_probe_cube(&bytes, 8, 4).unwrap(),
            ProbeCube {
                levels,
                emissive: Some(emissive),
                irradiance: [[1.0, 2.0, 3.0]; 6]
            }
        );
    }

    #[test]
    fn a_probe_cube_with_three_layers_is_refused() {
        let mut bytes = write_probe_cube(8, &probe_levels(8, 4), None, [[1.0; 3]; 6]);
        // Header: layerCount at byte 32.
        bytes[32..36].copy_from_slice(&3u32.to_le_bytes());
        assert!(matches!(
            read_probe_cube(&bytes, 8, 4),
            Err(Ktx2Error::Parse(_))
        ));
    }

    #[test]
    fn a_probe_cube_without_its_irradiance_is_an_error() {
        let mut bytes = write_probe_cube(8, &probe_levels(8, 4), None, [[1.0; 3]; 6]);
        // Header index: kvd offset at byte 56, kvd length at 60.
        bytes[60..64].copy_from_slice(&0u32.to_le_bytes());
        assert!(read_probe_cube(&bytes, 8, 4).is_err());
        let at = u32::from_le_bytes(bytes[56..60].try_into().unwrap()) as usize;
        bytes[60..64].copy_from_slice(&40u32.to_le_bytes());
        bytes[at + 4] = b'X';
        assert!(read_probe_cube(&bytes, 8, 4).is_err());
    }

    #[test]
    fn a_truncated_probe_cube_is_an_error_not_a_panic() {
        let bytes = write_probe_cube(8, &probe_levels(8, 4), None, [[1.0; 3]; 6]);
        for cut in [0, 5, 12, 16, 60, 100, 200, bytes.len() / 2, bytes.len() - 1] {
            assert!(
                read_probe_cube(&bytes[..cut], 8, 4).is_err(),
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn a_probe_cube_of_another_shape_is_refused() {
        let bytes = write_probe_cube(8, &probe_levels(8, 4), None, [[1.0; 3]; 6]);
        assert!(read_probe_cube(&bytes, 16, 4).is_err());
        assert!(read_probe_cube(&bytes, 8, 3).is_err());
        assert!(read_probe_cube(&bytes, 8, 5).is_err());
    }

    #[test]
    fn a_compressed_2d_file_is_not_a_probe_cube() {
        let mut file = Vec::new();
        let header = Header {
            format: Some(Format::BC7_SRGB_BLOCK),
            type_size: 1,
            pixel_width: 4,
            pixel_height: 4,
            pixel_depth: 0,
            layer_count: 0,
            face_count: 1,
            level_count: 1,
            supercompression_scheme: None,
            index: Index {
                dfd_byte_offset: 0,
                dfd_byte_length: 0,
                kvd_byte_offset: 0,
                kvd_byte_length: 0,
                sgd_byte_offset: 0,
                sgd_byte_length: 0,
            },
        };
        file.extend_from_slice(&header.as_bytes());
        assert!(matches!(
            read_probe_cube(&file, 4, 1),
            Err(Ktx2Error::FormatMismatch { .. })
        ));
    }
}
