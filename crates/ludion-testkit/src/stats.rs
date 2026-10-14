//! [`ImageStats`] — four normalized defect metrics over a single rendered image.
//!
//! Per `CLAUDE.md`'s North Star, a rendering change must be provable without a human looking at the
//! screen. [`crate::ssim`] answers "did this frame change?" against a committed golden; this module
//! answers the different question "does this frame carry a *defect*?", with no reference image at
//! all. The four numbers are deliberately **defect detectors, not quality scores** — nothing here
//! attempts to measure whether an image looks good, only whether it shows quantization contouring,
//! un-antialiased steps, or channels pinned to the rails.
//!
//! Every metric is:
//!
//! - **Deterministic** — a pure function of the RGBA8 bytes, summed in a fixed row-major order, so
//!   the same image always yields bit-identical `f64`s.
//! - **Normalized to `[0, 1]`, higher = more defect** — an unbounded score cannot be a threshold.
//! - **Resolution-stable** — every count is a ratio, and every *spatial* threshold is expressed as a
//!   fraction of the image's own extent, so the same content rendered at 960×540 and 1920×1080
//!   scores close. The one deliberate exception is [`ImageStats::edge_aliasing`]'s five-sample
//!   profile, which is fixed at the pixel grid because aliasing *is* a pixel-grid defect.
//!
//! Stability holds for content whose features scale with the frame — geometry, gradients, coverage.
//! A pattern locked to the pixel grid (a dither, film grain) does not exist "at two resolutions" in
//! the first place, and a gradient steep enough that its bands are only two or three pixels wide
//! sits where integer run lengths quantize the answer; both read lower the finer the grid gets.
//! Neither weakens a comparison made at one resolution, which is what a defect gate does.
//!
//! Luminance is Rec. 709 over the sRGB-encoded bytes, rounded to 8 bits — the same domain
//! [`crate::ssim`] uses, and the domain the defects themselves live in: banding is quantization of
//! the encoded output, and clipping is a byte on the rail. Linearizing first would measure a
//! different image than the one that gets written to disk.
//!
//! # Example
//!
//! ```
//! use ludion_testkit::{DecodedImage, image_stats};
//!
//! // A solid black frame: every colour channel sits on the black rail.
//! let black = DecodedImage {
//!     width: 8,
//!     height: 8,
//!     rgba: vec![0, 0, 0, 255].repeat(8 * 8),
//! };
//! let stats = image_stats(&black);
//! assert_eq!(stats.clipped_low, 1.0);
//! assert_eq!(stats.clipped_high, 0.0);
//! assert_eq!(stats.banding_score, 0.0); // no gradient at all, so nothing can be contoured
//! ```

use serde::{Deserialize, Serialize};

use crate::image::DecodedImage;

/// The fraction of the image's extent used as the banding gate's probe radius: a region counts as a
/// gradient only if its luma changes across `extent / 64` (twice this radius) by a small,
/// non-zero amount. Relative to the extent so the gate sees the same *content* slope at any
/// resolution.
const BANDING_PROBE_DIVISOR: usize = 128;

/// The largest luma change (0–255) across the probe span that still counts as a *shallow* gradient.
/// Anything steeper is content (an edge, a texture), not a surface where contouring is a defect.
const BANDING_SHALLOW_MAX: i32 = 8;

/// A constant-luma run this fraction of the image's extent long is a fully-formed band: `1/128` of
/// the frame, twice the step a full-range 8-bit ramp across the whole frame produces. Run lengths
/// are counted from one, not zero, so a single-pixel run — the baseline of any unstructured content,
/// at any resolution — always scores exactly zero.
const BANDING_RUN_DIVISOR: f64 = 128.0;

/// The share of the image that must be shallow gradient before a banding score is reported at all.
/// Contouring is a defect of *regions*; a handful of qualifying pixels (the shoulders of a blurred
/// edge, say) would otherwise let a negligible population speak for the whole frame.
const BANDING_MIN_COVERAGE: f64 = 0.01;

/// The smallest luma transition (0–255) that counts as a *strong* edge — a quarter of the dynamic
/// range. Below this an edge is content detail (or dither), and its hardness is not evidence of
/// aliasing.
const EDGE_MIN_AMPLITUDE: f64 = 64.0;

/// How much of the transition amplitude the samples *outside* the edge may drift by and still count
/// as flat shoulders. A profile with sloped shoulders is a wide ramp, not an edge.
const EDGE_SHOULDER_MAX: f64 = 0.15;

/// Hardness at or below which an edge is fully anti-aliased (one honest intermediate sample carries
/// half the transition, giving hardness `0.5`; `0.75` still allows a small one).
const EDGE_HARD_LO: f64 = 0.75;

/// Hardness at or above which an edge is a bare step: essentially the whole transition happens
/// between two adjacent pixels.
const EDGE_HARD_HI: f64 = 0.95;

/// The four defect metrics computed over one rendered image.
///
/// All four are in `[0, 1]` with **higher meaning more of the defect**, and all four are ratios, so
/// they are comparable between two renders of the same content at different resolutions. See the
/// module documentation for the shared guarantees.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct ImageStats {
    /// False contouring in smooth gradients: of the pixels sitting in a shallow-gradient region
    /// (luma changing by 1–8 of 255 across 1/64 of the frame), the mean size of the exactly-constant
    /// luma patch each one belongs to — the shorter of its run along the row and its run down the
    /// column, counted from one — as a fraction of a full band (1/128 of the frame), capped at 1. A
    /// flat fill, a noise field, and any frame whose shallow-gradient regions cover less than 1% of
    /// it all score `0`; a cleanly quantized gradient scores `1`. Adding output dither collapses the
    /// patches toward single pixels and drives it down, without touching the image's low-frequency
    /// content.
    pub banding_score: f64,
    /// Of the strong edges in the image (a luma transition of at least 64 of 255 with flat
    /// neighbourhoods on both sides), the mean *hardness* — how close each one comes to putting its
    /// whole transition between two adjacent pixels, scored `0` at or below a half-and-half split
    /// and `1` at a bare step, on a ramp rather than a threshold so no edge sits on a cliff. A ratio
    /// over edge pixels rather than
    /// over all pixels, so a scene with more geometry does not score worse for it; supersampling
    /// turns hard transitions into soft ones and drives this down, and an image with no strong edges
    /// at all (a blur, a flat fill) scores `0`.
    pub edge_aliasing: f64,
    /// The fraction of colour channel samples (R, G and B of every pixel; alpha excluded) sitting
    /// exactly on the black rail, value `0`. Per *channel* rather than per pixel because a crushed
    /// channel is a real defect under saturated coloured light — but counted as a fraction of
    /// samples, not "any channel of the pixel", so an authored dark blue costs 2/3 rather than
    /// saturating the metric. Rises when exposure or the tonemap curve crushes shadows.
    pub clipped_low: f64,
    /// The fraction of colour channel samples (R, G and B of every pixel; alpha excluded) sitting
    /// exactly on the white rail, value `255` — the mirror of [`ImageStats::clipped_low`], with the
    /// same per-channel-sample definition and the same reasoning. Rises when exposure or the tonemap
    /// curve blows out highlights, and catches a single blown channel long before all three rail
    /// together.
    pub clipped_high: f64,
}

/// Computes the four defect metrics of `image`.
///
/// A pure, deterministic function of the pixel bytes; a zero-sized or truncated image scores all
/// zeros. See [`ImageStats`] for what each number means and the module documentation for the
/// properties every one of them holds to.
///
/// # Example
///
/// ```
/// use ludion_testkit::{DecodedImage, image_stats};
///
/// // A hard black/white split: every edge pixel is a bare step.
/// let mut rgba = Vec::new();
/// for _ in 0..64 {
///     for x in 0..64u8 {
///         let v = if x < 32 { 0 } else { 255 };
///         rgba.extend_from_slice(&[v, v, v, 255]);
///     }
/// }
/// let stats = image_stats(&DecodedImage { width: 64, height: 64, rgba });
/// assert_eq!(stats.edge_aliasing, 1.0);
/// ```
pub fn image_stats(image: &DecodedImage) -> ImageStats {
    let width = image.width as usize;
    let height = image.height as usize;
    let zero = ImageStats {
        banding_score: 0.0,
        edge_aliasing: 0.0,
        clipped_low: 0.0,
        clipped_high: 0.0,
    };
    if width == 0 || height == 0 || image.rgba.len() < width * height * 4 {
        return zero;
    }

    let luma = luma_plane(image);
    let (clipped_low, clipped_high) = clipping(&image.rgba);
    ImageStats {
        banding_score: banding_score(&luma, width, height),
        edge_aliasing: edge_aliasing(&luma, width, height),
        clipped_low,
        clipped_high,
    }
}

/// Rec. 709 luma of every pixel, rounded to 8 bits, row-major. Rounding to the output's own
/// precision is what makes "exactly constant run" mean the same thing the eye sees as a band.
fn luma_plane(image: &DecodedImage) -> Vec<u8> {
    image
        .rgba
        .chunks_exact(4)
        .map(|px| {
            let luma =
                0.2126 * f64::from(px[0]) + 0.7152 * f64::from(px[1]) + 0.0722 * f64::from(px[2]);
            luma.round().clamp(0.0, 255.0) as u8
        })
        .collect()
}

/// Fraction of R/G/B samples exactly at `0` and exactly at `255`.
fn clipping(rgba: &[u8]) -> (f64, f64) {
    let mut low = 0u64;
    let mut high = 0u64;
    let mut total = 0u64;
    for pixel in rgba.chunks_exact(4) {
        for &channel in &pixel[..3] {
            total += 1;
            match channel {
                0 => low += 1,
                255 => high += 1,
                _ => {}
            }
        }
    }
    if total == 0 {
        return (0.0, 0.0);
    }
    (low as f64 / total as f64, high as f64 / total as f64)
}

/// Mean banded-ness of the shallow-gradient pixels (see [`ImageStats::banding_score`]).
///
/// A band is a *patch* of constant luma, so the run is measured along both axes and the **shorter**
/// one wins: a contour is only visible if the flat region extends both along the contour and across
/// it. Taking the shorter run is also what makes the measure immune to a row that a dither happens
/// to leave alone — any per-pixel pattern breaks at least one axis everywhere.
fn banding_score(luma: &[u8], width: usize, height: usize) -> f64 {
    let probe_x = (width / BANDING_PROBE_DIVISOR).max(1);
    let probe_y = (height / BANDING_PROBE_DIVISOR).max(1);
    if width <= 2 * probe_x || height <= 2 * probe_y {
        return 0.0;
    }
    let target_x = (width as f64 / BANDING_RUN_DIVISOR).max(1.0);
    let target_y = (height as f64 / BANDING_RUN_DIVISOR).max(1.0);

    let mut runs_x = vec![0u32; width * height];
    for y in 0..height {
        fill_run_lengths(luma, &mut runs_x, y * width, 1, width);
    }
    let mut runs_y = vec![0u32; width * height];
    for x in 0..width {
        fill_run_lengths(luma, &mut runs_y, x, width, height);
    }

    let mut sum = 0.0;
    let mut counted = 0u64;
    for y in probe_y..height - probe_y {
        for x in probe_x..width - probe_x {
            let index = y * width + x;
            let delta_x = i32::from(luma[index + probe_x]) - i32::from(luma[index - probe_x]);
            let delta_y =
                i32::from(luma[index + probe_y * width]) - i32::from(luma[index - probe_y * width]);
            let delta = delta_x.abs().max(delta_y.abs());
            if delta == 0 || delta > BANDING_SHALLOW_MAX {
                continue;
            }
            let credit_x = (f64::from(runs_x[index] - 1) / target_x).min(1.0);
            let credit_y = (f64::from(runs_y[index] - 1) / target_y).min(1.0);
            sum += credit_x.min(credit_y);
            counted += 1;
        }
    }
    let probed = ((width - 2 * probe_x) * (height - 2 * probe_y)) as f64;
    if (counted as f64) < BANDING_MIN_COVERAGE * probed {
        0.0
    } else {
        sum / counted as f64
    }
}

/// Writes into `runs` the length of the maximal constant-value run containing each sample of the
/// `count`-long sequence starting at `start` and stepping by `stride`.
fn fill_run_lengths(luma: &[u8], runs: &mut [u32], start: usize, stride: usize, count: usize) {
    let mut begin = 0;
    while begin < count {
        let value = luma[start + begin * stride];
        let mut end = begin + 1;
        while end < count && luma[start + end * stride] == value {
            end += 1;
        }
        let length = (end - begin) as u32;
        for i in begin..end {
            runs[start + i * stride] = length;
        }
        begin = end;
    }
}

/// Fraction of strong-edge pixels whose transition is hard (see [`ImageStats::edge_aliasing`]).
///
/// The five-sample profile is fixed at the pixel grid — two neighbours either side — rather than
/// scaled to the image's extent, because aliasing is by definition a defect *of* the pixel grid: a
/// bare step is a bare step at any resolution, and a widened window would read every real edge as
/// soft. Resolution stability comes instead from the ratio: both the edge count and the hard count
/// grow with edge length, so the quotient does not move.
fn edge_aliasing(luma: &[u8], width: usize, height: usize) -> f64 {
    let mut sum = 0.0;
    let mut edges = 0u64;
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            let horizontal = (x >= 2 && x + 2 < width).then(|| {
                edge_profile([
                    luma[index - 2],
                    luma[index - 1],
                    luma[index],
                    luma[index + 1],
                    luma[index + 2],
                ])
            });
            let vertical = (y >= 2 && y + 2 < height).then(|| {
                edge_profile([
                    luma[index - 2 * width],
                    luma[index - width],
                    luma[index],
                    luma[index + width],
                    luma[index + 2 * width],
                ])
            });
            let profile = [horizontal.flatten(), vertical.flatten()]
                .into_iter()
                .flatten()
                .max_by(|a, b| a.0.total_cmp(&b.0));
            let Some((_, hardness)) = profile else {
                continue;
            };
            sum += ((hardness - EDGE_HARD_LO) / (EDGE_HARD_HI - EDGE_HARD_LO)).clamp(0.0, 1.0);
            edges += 1;
        }
    }
    if edges == 0 { 0.0 } else { sum / edges as f64 }
}

/// Classifies one five-sample luma profile, returning `(amplitude, hardness)` when it describes a
/// strong edge with flat shoulders. `hardness` is the largest adjacent-sample step as a fraction of
/// the whole transition: `1.0` when the transition happens in one step, `0.5` when an intermediate
/// sample splits it in half.
fn edge_profile(taps: [u8; 5]) -> Option<(f64, f64)> {
    let low = taps.iter().copied().min()?;
    let high = taps.iter().copied().max()?;
    let amplitude = f64::from(high - low);
    if amplitude < EDGE_MIN_AMPLITUDE {
        return None;
    }
    let shoulder = EDGE_SHOULDER_MAX * amplitude;
    if f64::from(taps[1].abs_diff(taps[0])) > shoulder
        || f64::from(taps[4].abs_diff(taps[3])) > shoulder
    {
        return None;
    }
    let step = taps.windows(2).map(|w| w[1].abs_diff(w[0])).max()?;
    Some((amplitude, f64::from(step) / amplitude))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Absolute tolerance for "the same content at two resolutions scores the same".
    const RESOLUTION_TOLERANCE: f64 = 0.05;

    /// The classic 4×4 ordered-dither matrix — the cheapest stand-in for an output dither pattern.
    const BAYER4: [[u32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

    /// The luma range a gradient fixture spans, in 8-bit levels: shallow enough (like an authored
    /// night sky) that its bands are several pixels wide at any test resolution.
    const GRADIENT_RANGE: f64 = 64.0;

    /// Builds an image from a per-pixel grey level.
    fn grey(width: u32, height: u32, level: impl Fn(u32, u32) -> u8) -> DecodedImage {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let v = level(x, y);
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        DecodedImage {
            width,
            height,
            rgba,
        }
    }

    fn solid(width: u32, height: u32, level: u8) -> DecodedImage {
        grey(width, height, |_, _| level)
    }

    /// The continuous top-to-bottom ramp every gradient fixture quantizes, spanning
    /// [`GRADIENT_RANGE`] of the 8-bit scale.
    fn ramp_at(y: u32, height: u32) -> f64 {
        GRADIENT_RANGE * f64::from(y) / f64::from(height - 1)
    }

    /// The ordered-dither offset at one pixel, in 8-bit levels — what an output dither adds before
    /// the value is truncated to 8 bits.
    fn dither_at(x: u32, y: u32) -> f64 {
        (f64::from(BAYER4[(y % 4) as usize][(x % 4) as usize]) + 0.5) / 16.0
    }

    /// The ramp quantized to 8 bits, with or without an output dither in front of the truncation.
    fn gradient(width: u32, height: u32, dither: bool) -> DecodedImage {
        grey(width, height, |x, y| {
            if dither {
                (ramp_at(y, height) + dither_at(x, y)).floor() as u8
            } else {
                ramp_at(y, height).round() as u8
            }
        })
    }

    /// A deterministic per-pixel hash over the full 0–255 range — noise, not a dither.
    fn noise(width: u32, height: u32) -> DecodedImage {
        grey(width, height, |x, y| {
            let mut h = u64::from(x)
                .wrapping_mul(0x9E37_79B9)
                .wrapping_add(u64::from(y).wrapping_mul(0x85EB_CA6B));
            h ^= h >> 13;
            h = h.wrapping_mul(0xC2B2_AE35);
            (h >> 24) as u8
        })
    }

    /// How much of pixel `(x, y)` a disc of 0.3× the image width covers, sampled `factor`×`factor`
    /// times: `factor` of 1 is an aliased render of the disc, 4 a supersampled one.
    fn disc_coverage(x: u32, y: u32, width: u32, height: u32, factor: u32) -> f64 {
        let radius = f64::from(width) * 0.3;
        let (cx, cy) = (f64::from(width) / 2.0, f64::from(height) / 2.0);
        let mut inside = 0.0;
        for sy in 0..factor {
            for sx in 0..factor {
                let px = f64::from(x) + (f64::from(sx) + 0.5) / f64::from(factor);
                let py = f64::from(y) + (f64::from(sy) + 0.5) / f64::from(factor);
                if (px - cx).powi(2) + (py - cy).powi(2) <= radius * radius {
                    inside += 1.0;
                }
            }
        }
        inside / f64::from(factor * factor)
    }

    /// A white disc on black.
    fn disc(width: u32, height: u32, factor: u32) -> DecodedImage {
        grey(width, height, |x, y| {
            (255.0 * disc_coverage(x, y, width, height, factor)).round() as u8
        })
    }

    /// Box-blurs `image` with a radius of 1/64 of its width, leaving no edge intact.
    fn blurred(image: &DecodedImage) -> DecodedImage {
        let radius = i64::from((image.width / 64).max(2));
        let luma = luma_plane(image);
        let (w, h) = (i64::from(image.width), i64::from(image.height));
        grey(image.width, image.height, |x, y| {
            let mut sum = 0.0;
            let mut count = 0.0;
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let sx = (i64::from(x) + dx).clamp(0, w - 1);
                    let sy = (i64::from(y) + dy).clamp(0, h - 1);
                    sum += f64::from(luma[(sy * w + sx) as usize]);
                    count += 1.0;
                }
            }
            (sum / count).round() as u8
        })
    }

    /// A white disc composited over the quantized gradient — every metric reads non-trivially, and
    /// every feature is defined in image-relative terms, so the same content exists at any size.
    fn composite(width: u32, height: u32, dither: bool, factor: u32) -> DecodedImage {
        let background = luma_plane(&gradient(width, height, dither));
        grey(width, height, |x, y| {
            let coverage = disc_coverage(x, y, width, height, factor);
            let under = f64::from(background[(y * width + x) as usize]);
            (under * (1.0 - coverage) + 255.0 * coverage).round() as u8
        })
    }

    #[test]
    fn dither_collapses_the_banding_of_a_quantized_gradient() {
        // Measured: 1.0000 clean vs 0.3375 dithered at 512², 1.0000 vs 0.3058 at 1024².
        let clean = image_stats(&gradient(512, 512, false)).banding_score;
        let dithered = image_stats(&gradient(512, 512, true)).banding_score;
        assert_eq!(clean, 1.0, "a cleanly quantized gradient is fully banded");
        assert!(
            dithered < 0.4,
            "dither must collapse the constant runs, got {dithered}"
        );
        assert!(
            clean > dithered * 2.0,
            "clean {clean} vs dithered {dithered} is not a usable separation"
        );
    }

    #[test]
    fn noise_and_flat_fills_are_not_mistaken_for_bands() {
        // A run of one pixel scores zero at any resolution, so unstructured content floors the
        // metric instead of riding a resolution-dependent baseline.
        assert_eq!(image_stats(&noise(512, 512)).banding_score, 0.0);
        assert_eq!(image_stats(&noise(1024, 1024)).banding_score, 0.0);
        assert_eq!(image_stats(&solid(512, 512, 200)).banding_score, 0.0);
    }

    #[test]
    fn supersampling_collapses_the_aliasing_of_a_hard_disc() {
        // Measured: 1.0000 hard vs 0.3164 at 4× supersampling (512²), 1.0000 vs 0.3440 at 1024².
        let hard = image_stats(&disc(512, 512, 1)).edge_aliasing;
        let supersampled = image_stats(&disc(512, 512, 4)).edge_aliasing;
        assert_eq!(
            hard, 1.0,
            "every edge of a hard-sampled disc is a bare step"
        );
        assert!(
            supersampled < 0.4,
            "supersampling must soften the edges, got {supersampled}"
        );
        assert!(
            hard > supersampled * 2.0,
            "hard {hard} vs supersampled {supersampled} is not a usable separation"
        );
    }

    #[test]
    fn a_blurred_image_has_no_hard_edges() {
        assert_eq!(image_stats(&blurred(&disc(512, 512, 1))).edge_aliasing, 0.0);
        assert_eq!(image_stats(&solid(512, 512, 128)).edge_aliasing, 0.0);
    }

    #[test]
    fn clipping_counts_channel_samples_on_the_rails_exactly() {
        let black = image_stats(&solid(64, 64, 0));
        assert_eq!((black.clipped_low, black.clipped_high), (1.0, 0.0));

        let white = image_stats(&solid(64, 64, 255));
        assert_eq!((white.clipped_low, white.clipped_high), (0.0, 1.0));

        // A mid-grey ramp that never reaches either rail.
        let ramp = image_stats(&grey(64, 64, |x, _| 1 + (x * 253 / 63) as u8));
        assert_eq!((ramp.clipped_low, ramp.clipped_high), (0.0, 0.0));

        // One channel crushed and one blown in the same pixel: a third of the samples each, which a
        // per-pixel "all three channels" test would score as no clipping at all.
        let saturated = image_stats(&DecodedImage {
            width: 8,
            height: 8,
            rgba: [255, 40, 0, 255].repeat(8 * 8),
        });
        assert_eq!(saturated.clipped_low, 1.0 / 3.0);
        assert_eq!(saturated.clipped_high, 1.0 / 3.0);
    }

    #[test]
    fn the_same_content_scores_the_same_at_two_resolutions() {
        // Every feature of the composite is image-relative, and every metric lands off its rails,
        // so this compares the four numbers where they are free to drift.
        let small = image_stats(&composite(512, 512, true, 4));
        let large = image_stats(&composite(1024, 1024, true, 4));
        for (name, a, b) in [
            ("banding_score", small.banding_score, large.banding_score),
            ("edge_aliasing", small.edge_aliasing, large.edge_aliasing),
            ("clipped_low", small.clipped_low, large.clipped_low),
            ("clipped_high", small.clipped_high, large.clipped_high),
        ] {
            assert!(
                (a - b).abs() <= RESOLUTION_TOLERANCE,
                "{name} moved from {a} at 512² to {b} at 1024², beyond {RESOLUTION_TOLERANCE}"
            );
            assert!(a > 0.0 && a < 1.0, "{name} is pinned to a rail at {a}");
        }
    }

    #[test]
    fn every_metric_stays_in_range_and_repeats_bit_exactly() {
        for image in [
            solid(64, 64, 0),
            noise(256, 256),
            gradient(256, 256, false),
            gradient(256, 256, true),
            composite(256, 256, false, 1),
            blurred(&disc(256, 256, 1)),
        ] {
            let first = image_stats(&image);
            assert_eq!(
                first,
                image_stats(&image),
                "not a pure function of the bytes"
            );
            for value in [
                first.banding_score,
                first.edge_aliasing,
                first.clipped_low,
                first.clipped_high,
            ] {
                assert!((0.0..=1.0).contains(&value), "{value} is outside [0, 1]");
            }
        }
    }

    #[test]
    fn degenerate_images_score_zero() {
        let empty = image_stats(&DecodedImage {
            width: 0,
            height: 0,
            rgba: Vec::new(),
        });
        assert_eq!(empty.banding_score, 0.0);
        assert_eq!(empty.edge_aliasing, 0.0);
        assert_eq!(empty.clipped_low, 0.0);
        assert_eq!(empty.clipped_high, 0.0);

        let truncated = image_stats(&DecodedImage {
            width: 8,
            height: 8,
            rgba: vec![0; 4],
        });
        assert_eq!(truncated.clipped_low, 0.0);
    }

    #[test]
    fn stats_round_trip_as_json_with_the_documented_field_names() {
        let stats = image_stats(&composite(256, 256, false, 1));
        let json = serde_json::to_string(&stats).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        for key in [
            "banding_score",
            "edge_aliasing",
            "clipped_low",
            "clipped_high",
        ] {
            assert!(value.get(key).is_some(), "stats JSON is missing `{key}`");
        }
        assert_eq!(serde_json::from_str::<ImageStats>(&json).unwrap(), stats);
    }
}
