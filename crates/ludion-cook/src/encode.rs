//! One level to block bytes: BC7 through `intel_tex_2`, ASTC 4×4 through `ctt-astcenc`.

use ctt_astcenc as astc;
use intel_tex_2 as tex;
use ludion_assets::cooked::{ColorSpace, CompressedFormat};

use crate::CookError;
use crate::mips::pad_to_blocks;
use ludion_assets::mips::Level;

/// The encoder state of one `cook` call: the ASTC context (BC7 needs none).
pub(crate) enum Encoder {
    Bc7 { alpha: bool },
    Astc4x4(Box<astc::Context>),
}

impl Encoder {
    pub(crate) fn new(
        format: CompressedFormat,
        color_space: ColorSpace,
        alpha: bool,
    ) -> Result<Self, CookError> {
        match format {
            CompressedFormat::Bc7 => Ok(Encoder::Bc7 { alpha }),
            CompressedFormat::Astc4x4 => {
                let profile = match color_space {
                    ColorSpace::Srgb => astc::Profile::LdrSrgb,
                    ColorSpace::Linear => astc::Profile::Ldr,
                };
                let config =
                    astc::config_init(profile, 4, 4, 1, astc::Preset::Medium, astc::Flags::empty())
                        .map_err(|error| CookError::Encoder(error.to_string()))?;
                let context = astc::Context::new(&config)
                    .map_err(|error| CookError::Encoder(error.to_string()))?;
                Ok(Encoder::Astc4x4(Box::new(context)))
            }
        }
    }

    /// The block bytes of `level`, 16 per 4×4 block, padded to whole blocks first.
    pub(crate) fn encode(&mut self, level: &Level) -> Result<Vec<u8>, CookError> {
        let padded = pad_to_blocks(level);
        match self {
            Encoder::Bc7 { alpha } => {
                let settings = if *alpha {
                    tex::bc7::alpha_basic_settings()
                } else {
                    tex::bc7::opaque_basic_settings()
                };
                let surface = tex::RgbaSurface {
                    data: &padded.rgba,
                    width: padded.width as u32,
                    height: padded.height as u32,
                    stride: padded.width as u32 * 4,
                };
                Ok(tex::bc7::compress_blocks(&settings, &surface))
            }
            Encoder::Astc4x4(context) => {
                let mut data = padded.rgba.as_ptr() as *mut std::os::raw::c_void;
                let mut image = astc::bindings::astcenc_image {
                    dim_x: padded.width as u32,
                    dim_y: padded.height as u32,
                    dim_z: 1,
                    data_type: astc::bindings::astcenc_type_ASTCENC_TYPE_U8,
                    data: &mut data,
                };
                let mut out = vec![0u8; padded.width / 4 * (padded.height / 4) * 16];
                context
                    .compress(&mut image, astc::Swizzle::IDENTITY, &mut out)
                    .map_err(|error| CookError::Encoder(error.to_string()))?;
                context
                    .compress_reset()
                    .map_err(|error| CookError::Encoder(error.to_string()))?;
                Ok(out)
            }
        }
    }
}
