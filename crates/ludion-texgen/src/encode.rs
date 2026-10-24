//! Quantizing evaluated maps and writing them as PNG bytes.

use ludion_core::math::powf;

use crate::eval::Maps;
use crate::image::Image;

/// The encoded map set, one PNG per map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapSet {
    /// sRGB RGBA8.
    pub base_color: Vec<u8>,
    /// RGBA8 with roughness in G and metallic in B.
    pub metallic_roughness: Vec<u8>,
    /// RGBA8 tangent-space normal.
    pub normal: Vec<u8>,
    /// RGBA8 with occlusion in RGB.
    pub occlusion: Vec<u8>,
    /// sRGB RGBA8, only when the recipe authors emissive.
    pub emissive: Option<Vec<u8>>,
    /// 16-bit linear grayscale.
    pub height: Vec<u8>,
}

fn q8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

fn q16(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16
}

pub(crate) fn srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * powf(v, 1.0 / 2.4) - 0.055
    }
}

fn png(
    size: u32,
    depth: png::BitDepth,
    color: png::ColorType,
    data: &[u8],
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, size, size);
        encoder.set_color(color);
        encoder.set_depth(depth);
        let mut writer = encoder
            .write_header()
            .map_err(|e| format!("could not write PNG header: {e}"))?;
        writer
            .write_image_data(data)
            .map_err(|e| format!("could not write PNG pixel data: {e}"))?;
    }
    Ok(bytes)
}

fn rgba8(image_size: u32, texel: impl Fn(usize) -> [u8; 4]) -> Result<Vec<u8>, String> {
    let count = image_size as usize * image_size as usize;
    let mut data = Vec::with_capacity(count * 4);
    for i in 0..count {
        data.extend_from_slice(&texel(i));
    }
    png(
        image_size,
        png::BitDepth::Eight,
        png::ColorType::Rgba,
        &data,
    )
}

fn color_png(image: &Image, to_srgb: bool) -> Result<Vec<u8>, String> {
    rgba8(image.size, |i| {
        let t = &image.data[i * 3..i * 3 + 3];
        let c = |v: f32| q8(if to_srgb { srgb(v) } else { v });
        [c(t[0]), c(t[1]), c(t[2]), 255]
    })
}

/// Encodes every map: quantize, convert base color and emissive to sRGB, write PNG.
///
/// # Errors
/// The PNG encoder's reason, as one line.
pub fn encode(maps: &Maps) -> Result<MapSet, String> {
    let size = maps.height.size;
    let metallic_roughness = rgba8(size, |i| {
        [
            0,
            q8(maps.roughness.data[i]),
            q8(maps.metallic.data[i]),
            255,
        ]
    })?;
    let occlusion = rgba8(size, |i| {
        let o = q8(maps.occlusion.data[i]);
        [o, o, o, 255]
    })?;
    let height: Vec<u8> = maps
        .height
        .data
        .iter()
        .flat_map(|v| q16(*v).to_be_bytes())
        .collect();
    Ok(MapSet {
        base_color: color_png(&maps.base_color, true)?,
        metallic_roughness,
        normal: color_png(&maps.normal, false)?,
        occlusion,
        emissive: maps
            .emissive
            .as_ref()
            .map(|e| color_png(e, true))
            .transpose()?,
        height: png(
            size,
            png::BitDepth::Sixteen,
            png::ColorType::Grayscale,
            &height,
        )?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantization_rounds_and_clamps() {
        assert_eq!(q8(-1.0), 0);
        assert_eq!(q8(2.0), 255);
        assert_eq!(q8(0.5), 128);
        assert_eq!(q16(1.0), 65535);
    }

    #[test]
    fn srgb_endpoints_and_midpoint() {
        assert_eq!(srgb(0.0), 0.0);
        assert!((srgb(1.0) - 1.0).abs() < 1e-6);
        assert!((srgb(0.2158605) - 0.5019608).abs() < 1e-4);
    }
}
