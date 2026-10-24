//! The `image` op's input: a PNG decoded to linear `f32` and resampled to the build size.

use std::io::Cursor;

use ludion_core::par;

use crate::image::Image;
use crate::ops::srgb_to_linear;

/// The largest side of a decodable PNG, in pixels.
pub const MAX_SIDE: u32 = 8192;

/// A decoded picture: `channels` (1 gray or 3 color) linear `f32` samples per pixel, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Samples per pixel: 1 or 3; alpha is dropped.
    pub channels: usize,
    /// `width * height * channels` samples.
    pub data: Vec<f32>,
}

/// Decodes PNG `bytes` to a [`Picture`]. 8-bit samples scale by 255, 16-bit by 65535; a color file
/// is sRGB-decoded to linear when `srgb` is set, a gray file is always taken linear.
///
/// # Errors
/// One line saying why the bytes are not a usable PNG.
pub fn decode(bytes: &[u8], srgb: bool) -> Result<Picture, String> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("not a readable PNG: {e}"))?;
    let declared = reader.info();
    if declared.width > MAX_SIDE || declared.height > MAX_SIDE {
        return Err(format!(
            "{}x{} pixels: a side is over {MAX_SIDE}",
            declared.width, declared.height
        ));
    }
    let mut buffer = vec![0u8; reader.output_buffer_size().unwrap_or(0)];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|e| format!("could not decode PNG pixels: {e}"))?;
    let (stride, channels) = match info.color_type {
        png::ColorType::Grayscale => (1, 1),
        png::ColorType::GrayscaleAlpha => (2, 1),
        png::ColorType::Rgb => (3, 3),
        png::ColorType::Rgba => (4, 3),
        png::ColorType::Indexed => return Err("an indexed PNG could not be expanded".into()),
    };
    let wide = info.bit_depth == png::BitDepth::Sixteen;
    let bytes_per_sample = if wide { 2 } else { 1 };
    let linearize = srgb && channels == 3;
    let sample = |pixel: &[u8], c: usize| {
        let at = c * bytes_per_sample;
        let v = if wide {
            f32::from(u16::from_be_bytes([pixel[at], pixel[at + 1]])) / 65535.0
        } else {
            f32::from(pixel[at]) / 255.0
        };
        if linearize { srgb_to_linear(v) } else { v }
    };
    let mut data = Vec::with_capacity(info.width as usize * info.height as usize * channels);
    for pixel in buffer[..info.buffer_size()].chunks_exact(stride * bytes_per_sample) {
        data.extend((0..channels).map(|c| sample(pixel, c)));
    }
    Ok(Picture {
        width: info.width,
        height: info.height,
        channels,
        data,
    })
}

/// For each output index along an axis, the source indices and weights that make it: a box average
/// when the source is longer than `size`, else bilinear at texel centres with wrap.
fn taps(len: usize, size: usize) -> Vec<Vec<(usize, f32)>> {
    (0..size)
        .map(|o| {
            if len > size {
                let (a, b) = (o * len / size, (o + 1) * len / size);
                let w = 1.0 / (b - a) as f32;
                (a..b).map(|i| (i, w)).collect()
            } else {
                let p = (o as f32 + 0.5) * (len as f32 / size as f32) - 0.5;
                let base = p.floor();
                let f = p - base;
                let i0 = (base as i64).rem_euclid(len as i64) as usize;
                let i1 = (base as i64 + 1).rem_euclid(len as i64) as usize;
                vec![(i0, 1.0 - f), (i1, f)]
            }
        })
        .collect()
}

/// Resamples to `size`×`size`, separably: a longer source axis is box-averaged, a shorter one
/// interpolated with wrap-around. A picture already `size` wide and tall comes back unchanged.
pub fn resample(picture: &Picture, size: u32) -> Image {
    let (w, h, ch) = (
        picture.width as usize,
        picture.height as usize,
        picture.channels,
    );
    let n = size as usize;
    let across = taps(w, n);
    let mut wide = vec![0.0f32; n * h * ch];
    par::par_chunks(&mut wide, n * ch, |y, row| {
        let src = &picture.data[y * w * ch..(y + 1) * w * ch];
        for (ox, texel) in row.chunks_mut(ch).enumerate() {
            for (c, out) in texel.iter_mut().enumerate() {
                *out = across[ox].iter().map(|&(i, k)| src[i * ch + c] * k).sum();
            }
        }
    });
    let down = taps(h, n);
    let mut image = Image::filled(size, ch, 0.0);
    par::par_chunks(&mut image.data, n * ch, |oy, row| {
        for (x, texel) in row.chunks_mut(ch).enumerate() {
            for (c, out) in texel.iter_mut().enumerate() {
                *out = down[oy]
                    .iter()
                    .map(|&(i, k)| wide[(i * n + x) * ch + c] * k)
                    .sum();
            }
        }
    });
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(
        width: u32,
        height: u32,
        color: png::ColorType,
        depth: png::BitDepth,
        raw: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(color);
        enc.set_depth(depth);
        enc.write_header().unwrap().write_image_data(raw).unwrap();
        out
    }

    #[test]
    fn gray8_scales_by_255() {
        let png = encode(
            2,
            1,
            png::ColorType::Grayscale,
            png::BitDepth::Eight,
            &[0, 255],
        );
        let p = decode(&png, true).unwrap();
        assert_eq!((p.width, p.height, p.channels), (2, 1, 1));
        assert_eq!(p.data, [0.0, 1.0]);
    }

    #[test]
    fn gray16_scales_by_65535_big_endian() {
        let png = encode(
            2,
            1,
            png::ColorType::Grayscale,
            png::BitDepth::Sixteen,
            &[0x80, 0x00, 0xff, 0xff],
        );
        let p = decode(&png, false).unwrap();
        assert!((p.data[0] - 32768.0 / 65535.0).abs() < 1e-7);
        assert_eq!(p.data[1], 1.0);
    }

    #[test]
    fn rgb_is_srgb_decoded_unless_linear() {
        let png = encode(
            1,
            1,
            png::ColorType::Rgb,
            png::BitDepth::Eight,
            &[128, 128, 128],
        );
        let s = decode(&png, true).unwrap();
        let l = decode(&png, false).unwrap();
        assert_eq!(s.channels, 3);
        assert!((s.data[0] - 0.2158605).abs() < 1e-5);
        assert!((l.data[0] - 128.0 / 255.0).abs() < 1e-7);
    }

    #[test]
    fn gray_is_linear_even_when_srgb_is_asked_and_alpha_is_dropped() {
        let png = encode(
            1,
            1,
            png::ColorType::GrayscaleAlpha,
            png::BitDepth::Eight,
            &[51, 10],
        );
        let p = decode(&png, true).unwrap();
        assert_eq!(p.channels, 1);
        assert_eq!(p.data, [51.0 / 255.0]);
        let rgba = encode(
            1,
            1,
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            &[255, 0, 255, 7],
        );
        let p = decode(&rgba, false).unwrap();
        assert_eq!(p.data, [1.0, 0.0, 1.0]);
    }

    #[test]
    fn palette_expands_to_color() {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, 2, 1);
        enc.set_color(png::ColorType::Indexed);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_palette(vec![255, 0, 0, 0, 0, 255]);
        enc.write_header()
            .unwrap()
            .write_image_data(&[0, 1])
            .unwrap();
        let p = decode(&out, false).unwrap();
        assert_eq!(p.channels, 3);
        assert_eq!(p.data, [1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn oversize_and_non_png_are_rejected() {
        let wide = encode(
            8193,
            1,
            png::ColorType::Grayscale,
            png::BitDepth::Eight,
            &vec![0; 8193],
        );
        assert!(decode(&wide, true).unwrap_err().contains("over 8192"));
        assert!(decode(b"definitely not a png", true).is_err());
    }

    fn ramp(w: u32, h: u32) -> Picture {
        let data = (0..w * h).map(|i| i as f32).collect();
        Picture {
            width: w,
            height: h,
            channels: 1,
            data,
        }
    }

    #[test]
    fn equal_size_is_the_identity() {
        let p = ramp(8, 8);
        assert_eq!(resample(&p, 8).data, p.data);
    }

    #[test]
    fn downsample_averages_source_texels() {
        let p = ramp(4, 4);
        let out = resample(&p, 2);
        // Texel (0, 0) averages source texels 0, 1, 4, 5.
        assert!((out.data[0] - 2.5).abs() < 1e-6);
        assert!((out.data[3] - 12.5).abs() < 1e-6);
    }

    #[test]
    fn upsample_wraps_across_the_seam() {
        let p = Picture {
            width: 2,
            height: 1,
            channels: 1,
            data: vec![0.0, 1.0],
        };
        let out = resample(&p, 4);
        // Output texel 0 sits a quarter before source texel 0's centre: it blends the wrapped
        // source texel 1 with source texel 0.
        assert!((out.data[0] - 0.25).abs() < 1e-6, "{:?}", out.data);
        assert!((out.data[3] - 0.75).abs() < 1e-6);
    }
}
