//! The resamples and block padding around the CPU mip chain (`ludion_assets::mips`). All f32
//! arithmetic is basic IEEE operations plus the libm-safe sRGB helpers from `ludion-assets`, so the
//! output is identical on every platform.

use ludion_assets::mips::{Level, Mixer, next_mip};

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
