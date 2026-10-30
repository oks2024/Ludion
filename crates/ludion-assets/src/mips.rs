//! The CPU mip chain: the 2×2 box filter `ludion-cook` bakes with and the renderer's coverage-
//! preserving chain for alpha-mask maps. All f32 arithmetic is basic IEEE operations plus the
//! libm-safe sRGB helpers of this crate, so the output is identical on every platform.

use crate::{decode_srgb, encode_srgb};

/// A tightly packed RGBA8 level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Level {
    /// Width in texels.
    pub width: usize,
    /// Height in texels.
    pub height: usize,
    /// `width * height * 4` bytes, row-major, top row first.
    pub rgba: Vec<u8>,
}

/// How a map's channels average: sRGB colour in linear light, everything else in byte space.
pub struct Mixer {
    srgb: bool,
    table: [f32; 256],
}

impl Mixer {
    /// A mixer for an sRGB colour map (`srgb`) or a linear one.
    pub fn new(srgb: bool) -> Self {
        let mut table = [0.0; 256];
        if srgb {
            for (byte, slot) in (0..=255u8).zip(table.iter_mut()) {
                *slot = decode_srgb(byte);
            }
        }
        Mixer { srgb, table }
    }

    /// Whether `channel` averages in linear light (colour channels of an sRGB map).
    pub fn is_linear_light(&self, channel: usize) -> bool {
        self.srgb && channel < 3
    }

    /// One byte as the value the mixer averages.
    pub fn load(&self, byte: u8, channel: usize) -> f32 {
        if self.is_linear_light(channel) {
            self.table[usize::from(byte)]
        } else {
            f32::from(byte)
        }
    }

    /// An averaged value back to a byte.
    pub fn store(&self, value: f32, channel: usize) -> u8 {
        if self.is_linear_light(channel) {
            encode_srgb(value)
        } else {
            (value + 0.5).clamp(0.0, 255.0) as u8
        }
    }
}

/// The next mip: a 2×2 box with edge-clamped taps for odd sizes. Colour is averaged in linear
/// light when `srgb`; alpha and every linear-map channel as `(a + b + c + d + 2) / 4` in bytes.
///
/// # Example
/// ```
/// use ludion_assets::mips::{Level, next_mip};
/// let level = Level { width: 2, height: 2, rgba: [0, 0, 0, 255].repeat(4) };
/// let next = next_mip(&level, false);
/// assert_eq!((next.width, next.height), (1, 1));
/// assert_eq!(next.rgba, [0, 0, 0, 255]);
/// ```
pub fn next_mip(source: &Level, srgb: bool) -> Level {
    let (width, height) = ((source.width / 2).max(1), (source.height / 2).max(1));
    let mixer = Mixer::new(srgb);
    let mut rgba = vec![0u8; width * height * 4];
    for y in 0..height {
        let rows = [
            (y * 2).min(source.height - 1),
            (y * 2 + 1).min(source.height - 1),
        ];
        for x in 0..width {
            let columns = [
                (x * 2).min(source.width - 1),
                (x * 2 + 1).min(source.width - 1),
            ];
            for channel in 0..4 {
                let tap = |row: usize, column: usize| {
                    source.rgba[(row * source.width + column) * 4 + channel]
                };
                let out = &mut rgba[(y * width + x) * 4 + channel];
                if mixer.is_linear_light(channel) {
                    let sum = mixer.load(tap(rows[0], columns[0]), channel)
                        + mixer.load(tap(rows[0], columns[1]), channel)
                        + mixer.load(tap(rows[1], columns[0]), channel)
                        + mixer.load(tap(rows[1], columns[1]), channel);
                    *out = mixer.store(sum / 4.0, channel);
                } else {
                    let sum = u32::from(tap(rows[0], columns[0]))
                        + u32::from(tap(rows[0], columns[1]))
                        + u32::from(tap(rows[1], columns[0]))
                        + u32::from(tap(rows[1], columns[1]));
                    *out = ((sum + 2) / 4) as u8;
                }
            }
        }
    }
    Level {
        width,
        height,
        rgba,
    }
}

/// The smallest alpha byte the shader's `alpha >= cutoff` test keeps; 256 when no byte passes.
///
/// # Example
/// ```
/// assert_eq!(ludion_assets::mips::cutoff_byte(0.5), 128);
/// assert_eq!(ludion_assets::mips::cutoff_byte(0.0), 0);
/// assert_eq!(ludion_assets::mips::cutoff_byte(2.0), 256);
/// ```
pub fn cutoff_byte(cutoff: f32) -> u16 {
    (0..=255u16)
        .find(|byte| f32::from(*byte) / 255.0 >= cutoff)
        .unwrap_or(256)
}

/// Texels whose alpha byte is at least `c`.
fn covered(level: &Level, c: u16) -> u64 {
    level
        .rgba
        .chunks_exact(4)
        .filter(|texel| u16::from(texel[3]) >= c)
        .count() as u64
}

/// `level` with alpha rescaled so `target` texels sit at or above `c`: the largest threshold `t`
/// that still keeps `target` texels is mapped onto `c` (floor division keeps lower alphas below it).
fn preserve_coverage(level: &Level, c: u16, target: u64) -> Level {
    let mut histogram = [0u64; 256];
    for texel in level.rgba.chunks_exact(4) {
        histogram[usize::from(texel[3])] += 1;
    }
    let mut threshold = 1u32;
    let mut at_or_above = 0u64;
    for t in (1..=255usize).rev() {
        at_or_above += histogram[t];
        if at_or_above >= target {
            threshold = t as u32;
            break;
        }
    }
    let mut rgba = level.rgba.clone();
    for texel in rgba.chunks_exact_mut(4) {
        texel[3] = (u32::from(texel[3]) * u32::from(c) / threshold).min(255) as u8;
    }
    Level {
        width: level.width,
        height: level.height,
        rgba,
    }
}

/// Every level of `level0`'s chain down to 1×1 (level 0 first, unchanged), each keeping level 0's
/// fraction of texels at or above `cutoff` — so thin alpha-tested bars survive minification.
///
/// The box chain is built from the previous *unscaled* level (no compounding); level `k` then has
/// its alpha rescaled so its covered count matches level 0's fraction. Colour is untouched. A cutoff
/// that keeps every texel or none, or a map whose every texel passes, returns the plain box chain.
///
/// # Example
/// ```
/// use ludion_assets::mips::{Level, mask_mip_chain};
/// // One opaque column in eight: a plain box filter averages it below the cutoff.
/// let mut rgba = vec![0u8; 8 * 8 * 4];
/// for y in 0..8 {
///     rgba[(y * 8) * 4 + 3] = 255;
/// }
/// let chain = mask_mip_chain(Level { width: 8, height: 8, rgba }, false, 0.5);
/// assert_eq!(chain.len(), 4);
/// assert!(chain[2].rgba.chunks(4).any(|texel| texel[3] >= 128));
/// ```
pub fn mask_mip_chain(level0: Level, srgb: bool, cutoff: f32) -> Vec<Level> {
    let c = cutoff_byte(cutoff);
    let covered0 = covered(&level0, c);
    let n0 = (level0.width * level0.height) as u64;
    let preserve = c != 0 && c != 256 && covered0 != 0 && covered0 != n0;
    let mut chain = vec![level0];
    let mut unscaled = chain[0].clone();
    while unscaled.width > 1 || unscaled.height > 1 {
        unscaled = next_mip(&unscaled, srgb);
        if preserve {
            let n = (unscaled.width * unscaled.height) as u64;
            let target = (covered0 * n + n0 / 2) / n0;
            chain.push(preserve_coverage(&unscaled, c, target));
        } else {
            chain.push(unscaled.clone());
        }
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker() -> Level {
        let (black, white) = ([0, 0, 0, 255], [255, 255, 255, 255]);
        let rgba = [black, white, white, black].concat();
        Level {
            width: 2,
            height: 2,
            rgba,
        }
    }

    #[test]
    fn srgb_colour_averages_in_linear_light() {
        assert_eq!(next_mip(&checker(), true).rgba, [188, 188, 188, 255]);
    }

    #[test]
    fn linear_maps_average_in_bytes() {
        assert_eq!(next_mip(&checker(), false).rgba, [128, 128, 128, 255]);
    }

    #[test]
    fn odd_sizes_clamp_their_taps() {
        let level = Level {
            width: 3,
            height: 1,
            rgba: vec![0, 0, 0, 255, 100, 100, 100, 255, 200, 200, 200, 255],
        };
        let next = next_mip(&level, false);
        assert_eq!((next.width, next.height), (1, 1));
        assert_eq!(next.rgba, [50, 50, 50, 255]);
    }

    fn plain_chain(level0: Level, srgb: bool) -> Vec<Level> {
        let mut chain = vec![level0];
        while chain.last().is_some_and(|l| l.width > 1 || l.height > 1) {
            let next = next_mip(chain.last().unwrap(), srgb);
            chain.push(next);
        }
        chain
    }

    /// 1-texel vertical strands, one column in eight, with a varying colour and an alpha that fades
    /// down the strand (a flat alpha ties every texel at the threshold).
    fn strands() -> Level {
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
        Level {
            width: 64,
            height: 64,
            rgba,
        }
    }

    #[test]
    fn strands_keep_their_coverage_down_the_chain() {
        let c = cutoff_byte(0.5);
        let chain = mask_mip_chain(strands(), true, 0.5);
        let target = covered(&chain[0], c) as f64 / (64.0 * 64.0);
        for level in &chain {
            let n = (level.width * level.height) as f64;
            let fraction = covered(level, c) as f64 / n;
            // Equal alphas tie at the threshold, so a coarse level may overshoot by a few texels.
            let slack = (4.0 / n).max(0.02);
            assert!(
                (fraction - target).abs() <= slack,
                "{}x{}: {fraction} vs {target}",
                level.width,
                level.height
            );
        }
        let plain = plain_chain(strands(), true);
        assert_eq!(
            covered(&plain[3], c),
            0,
            "the plain chain loses the strands"
        );
    }

    #[test]
    fn an_opaque_map_matches_the_plain_chain() {
        let level = Level {
            width: 16,
            height: 8,
            rgba: [10, 20, 30, 255].repeat(16 * 8),
        };
        assert_eq!(
            mask_mip_chain(level.clone(), true, 0.5),
            plain_chain(level, true)
        );
    }

    #[test]
    fn cutoff_zero_is_the_plain_chain() {
        assert_eq!(
            mask_mip_chain(strands(), true, 0.0),
            plain_chain(strands(), true)
        );
    }

    #[test]
    fn colour_is_the_plain_chains() {
        let masked = mask_mip_chain(strands(), true, 0.5);
        let plain = plain_chain(strands(), true);
        for (a, b) in masked.iter().zip(&plain) {
            let rgb = |l: &Level| -> Vec<u8> {
                l.rgba
                    .chunks(4)
                    .flat_map(|texel| texel[..3].to_vec())
                    .collect()
            };
            assert_eq!(rgb(a), rgb(b));
        }
    }
}
