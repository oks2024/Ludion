//! The bake's working image type and the one sRGB transfer pair the crate uses.
//!
//! Every effect reads and writes [`FrameImage`]: straight alpha, linear light, `f32`. Straight
//! because the ops are authored in terms of a texel's own colour (a gradient multiplies the colour,
//! not the colour-times-coverage), and linear because averaging and lerping encoded bytes darkens
//! the result by roughly its own gamma. Premultiplication happens exactly once, on the way out to
//! the atlas texture.

use ludion_assets::Image;
use ludion_core::math;

/// One frame in the bake's working format: straight (non-premultiplied) alpha, linear RGB, `f32`,
/// row-major, top row first.
///
/// Held as `[f32; 4]` per texel rather than packed bytes so an effect can read a neighbour, average
/// a window and write back without quantizing between ops — quantization is a one-time cost at the
/// end, not a per-op one.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameImage {
    /// Width in texels.
    pub width: u32,
    /// Height in texels.
    pub height: u32,
    /// `width * height` linear straight-alpha RGBA texels, row-major, top row first.
    pub texels: Vec<[f32; 4]>,
}

impl FrameImage {
    /// A transparent-black frame of `width * height` texels.
    pub fn new(width: u32, height: u32) -> Self {
        let count = (width as usize) * (height as usize);
        Self {
            width,
            height,
            texels: vec![[0.0; 4]; count],
        }
    }

    /// The texel at `(x, y)`, or transparent black when `(x, y)` lies outside the frame.
    ///
    /// Out of bounds is a *value*, not an error, because every neighbourhood effect (outline,
    /// shade, shadow, blur) reads past the edge and all of them want the same answer there: nothing
    /// is outside a sprite. Returning the value keeps the bounds test out of seven op bodies.
    pub fn get(&self, x: i32, y: i32) -> [f32; 4] {
        match self.index(x, y) {
            Some(index) => self.texels[index],
            None => [0.0; 4],
        }
    }

    /// The flat index of `(x, y)`, or `None` outside the frame.
    pub(crate) fn index(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return None;
        }
        Some((y as usize) * (self.width as usize) + (x as usize))
    }

    /// Encodes to RGBA8 with **straight** alpha: sRGB-encoded RGB, linear alpha.
    ///
    /// This is the human-facing form — contact sheets and the PNG import round trip — because it is
    /// what an image editor shows and what a decoded PNG holds. [`Image::hash`] is set to the
    /// content hash of the *encoded RGBA buffer*, not of any file: a baked image has no source PNG
    /// to take an identity from, and the pixels are the identity anyway.
    pub fn to_srgb8_straight(&self) -> Image {
        self.encode(false)
    }

    /// Encodes to RGBA8 with **premultiplied** alpha: RGB weighted by alpha in *linear* light, then
    /// sRGB-encoded.
    ///
    /// This is the atlas texture convention. The sprite pass blends `One / OneMinusSrcAlpha`, which
    /// needs the colour already weighted by its own coverage, and the sampler decodes sRGB before
    /// the blend sees it — so the weighting must happen in linear light, which is exactly where the
    /// bake already works. [`Image::hash`] is the content hash of the encoded RGBA buffer, as in
    /// [`Self::to_srgb8_straight`].
    pub fn to_srgb8_premultiplied(&self) -> Image {
        self.encode(true)
    }

    fn encode(&self, premultiply: bool) -> Image {
        let mut rgba = Vec::with_capacity(self.texels.len() * 4);
        for texel in &self.texels {
            let alpha = texel[3].clamp(0.0, 1.0);
            for channel in &texel[0..3] {
                let linear = channel.clamp(0.0, 1.0);
                let weighted = if premultiply { linear * alpha } else { linear };
                rgba.push(quantize(srgb_encode(weighted)));
            }
            rgba.push(quantize(alpha));
        }
        Image {
            width: self.width,
            height: self.height,
            hash: ludion_assets::content_hash(&rgba),
            rgba,
        }
    }

    /// The inverse of [`Self::to_srgb8_straight`]: sRGB-decode the RGB channels, read alpha as
    /// linear coverage.
    ///
    /// Exact enough to be an inverse in practice: an 8-bit code decoded to `f32` and re-encoded
    /// lands well inside half a quantization step, so `from_srgb8_straight(…).to_srgb8_straight()`
    /// reproduces the input bytes. That is what makes `bake(import(png)) == png` a testable claim.
    pub fn from_srgb8_straight(image: &Image) -> Self {
        let mut texels = Vec::with_capacity(image.rgba.len() / 4);
        for chunk in image.rgba.chunks_exact(4) {
            texels.push([
                srgb_decode(f32::from(chunk[0]) / 255.0),
                srgb_decode(f32::from(chunk[1]) / 255.0),
                srgb_decode(f32::from(chunk[2]) / 255.0),
                f32::from(chunk[3]) / 255.0,
            ]);
        }
        Self {
            width: image.width,
            height: image.height,
            texels,
        }
    }
}

/// The sRGB transfer function: linear light in `0..=1` to encoded `0..=1`.
///
/// The exact piecewise IEC 61966-2-1 form, with the power term through [`ludion_core::math`] so
/// x86-64 and arm64 agree bit-for-bit — a committed atlas PNG is regenerated on both and
/// byte-compared.
pub fn srgb_encode(linear: f32) -> f32 {
    let x = linear.clamp(0.0, 1.0);
    if x <= 0.003_130_8 {
        x * 12.92
    } else {
        1.055 * math::powf(x, 1.0 / 2.4) - 0.055
    }
}

/// The inverse of [`srgb_encode`]: encoded `0..=1` to linear light in `0..=1`.
pub fn srgb_decode(encoded: f32) -> f32 {
    let x = encoded.clamp(0.0, 1.0);
    if x <= 0.040_45 {
        x / 12.92
    } else {
        math::powf((x + 0.055) / 1.055, 2.4)
    }
}

/// The `f64` mirror of [`srgb_decode`], for the importer's palette: a sprite document stores colours
/// at `f64` because that is the width its text round-trips losslessly at.
pub fn srgb_decode_f64(encoded: f64) -> f64 {
    let x = encoded.clamp(0.0, 1.0);
    if x <= 0.040_45 {
        x / 12.92
    } else {
        math::f64::powf((x + 0.055) / 1.055, 2.4)
    }
}

/// Quantizes a `0..=1` value to a `u8` code by round-half-away-from-zero.
fn quantize(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_of_bounds_reads_transparent_black() {
        let image = FrameImage::new(2, 2);
        assert_eq!(image.get(-1, 0), [0.0; 4]);
        assert_eq!(image.get(0, 2), [0.0; 4]);
        assert_eq!(image.get(0, 0), [0.0; 4]);
    }

    #[test]
    fn srgb_round_trips_every_8_bit_code() {
        for code in 0..=255u8 {
            let encoded = f32::from(code) / 255.0;
            let back = quantize(srgb_encode(srgb_decode(encoded)));
            assert_eq!(back, code, "sRGB round trip lost code {code}");
        }
    }

    #[test]
    fn srgb_endpoints_quantize_to_the_endpoint_codes() {
        assert_eq!(srgb_encode(0.0), 0.0);
        assert_eq!(srgb_decode(0.0), 0.0);
        assert_eq!(srgb_decode(1.0), 1.0);
        // `1.055 * 1 - 0.055` lands one ulp under 1 in `f32`; what matters is that it still
        // quantizes to 255, so full brightness in never leaves as 254.
        assert_eq!(quantize(srgb_encode(1.0)), 255);
    }

    #[test]
    fn premultiplication_happens_in_linear_light() {
        let mut image = FrameImage::new(1, 1);
        // Full-brightness white at half coverage: the premultiplied byte is sRGB(0.5), well above
        // the 128 a straight byte multiply would give.
        image.texels[0] = [1.0, 1.0, 1.0, 0.5];
        let premultiplied = image.to_srgb8_premultiplied();
        assert_eq!(premultiplied.rgba[0], quantize(srgb_encode(0.5)));
        assert_eq!(premultiplied.rgba[0], 188);
        assert_eq!(premultiplied.rgba[3], 128);

        let straight = image.to_srgb8_straight();
        assert_eq!(straight.rgba[0], 255);
        assert_eq!(straight.rgba[3], 128);
    }

    #[test]
    fn straight_encode_and_decode_are_inverses() {
        let mut image = FrameImage::new(2, 1);
        image.texels[0] = [srgb_decode(0.2), srgb_decode(0.7), srgb_decode(1.0), 1.0];
        image.texels[1] = [
            srgb_decode(0.0),
            srgb_decode(0.5),
            srgb_decode(0.3),
            77.0 / 255.0,
        ];
        let encoded = image.to_srgb8_straight();
        let decoded = FrameImage::from_srgb8_straight(&encoded);
        assert_eq!(decoded.to_srgb8_straight().rgba, encoded.rgba);
    }

    #[test]
    fn the_hash_is_the_encoded_buffer_s_own() {
        let image = FrameImage::new(2, 2);
        let encoded = image.to_srgb8_straight();
        assert_eq!(encoded.hash, ludion_assets::content_hash(&encoded.rgba));
    }
}
