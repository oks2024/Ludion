//! The CPU mip chain and the resamples around it. All f32 arithmetic is basic IEEE operations plus
//! the libm-safe sRGB helpers from `ludion-assets`, so the output is identical on every platform.

use ludion_assets::{decode_srgb, encode_srgb};

/// A tightly packed RGBA8 level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Level {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// How a map's channels average: sRGB colour in linear light, everything else in byte space.
struct Mixer {
    srgb: bool,
    table: [f32; 256],
}

impl Mixer {
    fn new(srgb: bool) -> Self {
        let mut table = [0.0; 256];
        if srgb {
            for (byte, slot) in (0..=255u8).zip(table.iter_mut()) {
                *slot = decode_srgb(byte);
            }
        }
        Mixer { srgb, table }
    }

    fn is_linear_light(&self, channel: usize) -> bool {
        self.srgb && channel < 3
    }

    fn load(&self, byte: u8, channel: usize) -> f32 {
        if self.is_linear_light(channel) {
            self.table[usize::from(byte)]
        } else {
            f32::from(byte)
        }
    }

    fn store(&self, value: f32, channel: usize) -> u8 {
        if self.is_linear_light(channel) {
            encode_srgb(value)
        } else {
            (value + 0.5).clamp(0.0, 255.0) as u8
        }
    }
}

/// The next mip: a 2×2 box with edge-clamped taps for odd sizes. Colour is averaged in linear
/// light when `srgb`; alpha and every linear-map channel as `(a + b + c + d + 2) / 4` in bytes.
pub(crate) fn next_mip(source: &Level, srgb: bool) -> Level {
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

/// Halves `source` with [`next_mip`] until neither side exceeds `cap`.
pub(crate) fn cap_extent(mut source: Level, cap: u32, srgb: bool) -> Level {
    let cap = cap.max(4) as usize;
    while source.width.max(source.height) > cap {
        source = next_mip(&source, srgb);
    }
    source
}

/// Bilinear resample to exactly `width` × `height` (an upscale to the next block multiple), in
/// linear light for sRGB colour.
pub(crate) fn resample(source: &Level, width: usize, height: usize, srgb: bool) -> Level {
    let mixer = Mixer::new(srgb);
    let mut rgba = vec![0u8; width * height * 4];
    let position = |index: usize, from: usize, to: usize| {
        let at = ((index as f32 + 0.5) * from as f32 / to as f32 - 0.5).max(0.0);
        let low = (at.floor() as usize).min(from - 1);
        (low, (low + 1).min(from - 1), at - low as f32)
    };
    for y in 0..height {
        let (y0, y1, ty) = position(y, source.height, height);
        for x in 0..width {
            let (x0, x1, tx) = position(x, source.width, width);
            for channel in 0..4 {
                let tap = |row: usize, column: usize| {
                    mixer.load(
                        source.rgba[(row * source.width + column) * 4 + channel],
                        channel,
                    )
                };
                let top = tap(y0, x0) + (tap(y0, x1) - tap(y0, x0)) * tx;
                let bottom = tap(y1, x0) + (tap(y1, x1) - tap(y1, x0)) * tx;
                rgba[(y * width + x) * 4 + channel] =
                    mixer.store(top + (bottom - top) * ty, channel);
            }
        }
    }
    Level {
        width,
        height,
        rgba,
    }
}

/// `source` padded to a multiple of 4 per side by edge replication — encoder input only; the
/// padding lies outside the level's logical extent.
pub(crate) fn pad_to_blocks(source: &Level) -> Level {
    let (width, height) = (
        source.width.next_multiple_of(4),
        source.height.next_multiple_of(4),
    );
    if (width, height) == (source.width, source.height) {
        return source.clone();
    }
    let mut rgba = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let from = (y.min(source.height - 1) * source.width + x.min(source.width - 1)) * 4;
            rgba[(y * width + x) * 4..][..4].copy_from_slice(&source.rgba[from..from + 4]);
        }
    }
    Level {
        width,
        height,
        rgba,
    }
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

    #[test]
    fn padding_replicates_the_edge() {
        let level = Level {
            width: 1,
            height: 1,
            rgba: vec![9, 8, 7, 6],
        };
        let padded = pad_to_blocks(&level);
        assert_eq!((padded.width, padded.height), (4, 4));
        assert!(padded.rgba.chunks(4).all(|texel| texel == [9, 8, 7, 6]));
    }

    #[test]
    fn resample_of_a_flat_map_stays_flat() {
        let level = Level {
            width: 3,
            height: 5,
            rgba: [10, 20, 30, 40].repeat(15),
        };
        let up = resample(&level, 4, 8, true);
        assert!(up.rgba.chunks(4).all(|texel| texel == [10, 20, 30, 40]));
    }

    #[test]
    fn cap_halves_until_it_fits() {
        let level = Level {
            width: 64,
            height: 48,
            rgba: vec![255; 64 * 48 * 4],
        };
        let capped = cap_extent(level, 16, false);
        assert_eq!((capped.width, capped.height), (16, 12));
    }
}
