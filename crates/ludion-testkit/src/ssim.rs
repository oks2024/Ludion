//! Windowed grayscale SSIM (structural similarity) plus per-tile colour error, localized on a tile
//! grid.
//!
//! Per `CLAUDE.md`'s tiered-determinism rule, images gate on perceptual comparison, not bit
//! equality (bit-exact goldens only apply on a pinned GPU, and even then the comparison itself must
//! not flap). This module is pure integer/`f64` CPU math over decoded RGBA8 buffers — no
//! `f32`-vs-`f64` platform drift, no GPU involvement — so the same two images always produce the
//! same score everywhere.
//!
//! # Method
//!
//! **Structure.** The textbook SSIM: an 11×11 sliding window over the Rec. 709 luma plane, scored
//! with the standard index and averaged. The window weights are the separable binomial kernel
//! `[1, 10, 45, 120, 210, 252, 210, 120, 45, 10, 1] / 1024` (sigma about 1.58, the Gaussian's
//! discrete twin) — exact rationals, so the score uses no transcendental and is identical on every
//! machine. Only windows lying wholly inside the image count. The global score
//! ([`Comparison::ssim`]) is the mean over all windows.
//!
//! **Localization.** The image is also partitioned into a fixed **8×8 grid of non-overlapping
//! tiles** ([`TILE_GRID`]); a window belongs to the tile holding its centre pixel, and a tile's
//! score is the **minimum** of its windows. A mean would let a 16×16 element vanishing out of a
//! 1080p image average away; the worst window cannot. A tile with no window centre (an image under
//! 11 px in a dimension) falls back to one flat window over the whole tile, and an image with no
//! window at all scores as the pixel-weighted mean of those flat tile scores.
//!
//! **Colour.** SSIM sees luma only, so a hue change that preserves brightness is invisible to it.
//! Each tile therefore also carries [`TileDiff::color_error`], the largest of its R, G, B and A mean
//! absolute channel differences (exact integer sums); the gate in [`crate::golden`] bounds it.
//!
//! Luma uses Rec. 709 weights over the sRGB-encoded bytes directly (no linearization — golden
//! comparison cares about the encoded pixels that actually get written to disk, not scene-linear
//! light). Alpha is not part of SSIM but is part of the colour error, so a translucent regression
//! cannot hide behind opaque luma. [`mean_absolute_error`] is the secondary signal, over all four channels.
//!
//! Constants: `k1 = 0.01`, `k2 = 0.03`, `L = 255` (the standard 8-bit dynamic range) — the textbook
//! defaults. Identical images score exactly 1.0: every expression is symmetric in its two inputs.

use serde::{Deserialize, Serialize};

use crate::image::DecodedImage;

/// SSIM stability constant `k1` (standard default).
const K1: f64 = 0.01;
/// SSIM stability constant `k2` (standard default).
const K2: f64 = 0.03;
/// Dynamic range of an 8-bit channel.
const DYNAMIC_RANGE: f64 = 255.0;

/// The number of tiles along each axis of the region-localization grid (clamped down for images
/// smaller than this in a given dimension — see the private `tile_bounds`).
pub const TILE_GRID: usize = 8;

/// The sliding window's side, in pixels.
const WINDOW: usize = 11;
/// Pixels between a window's centre and its edge.
const RADIUS: usize = WINDOW / 2;
/// The window's 1-D binomial weights, over [`KERNEL_DENOMINATOR`].
const KERNEL_NUMERATORS: [f64; WINDOW] = [
    1.0, 10.0, 45.0, 120.0, 210.0, 252.0, 210.0, 120.0, 45.0, 10.0, 1.0,
];
/// `2^10`, the sum of [`KERNEL_NUMERATORS`] — a power of two, so every weight is exact.
const KERNEL_DENOMINATOR: f64 = 1024.0;

/// One axis-aligned pixel rectangle within an image.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct TileRect {
    /// Left edge, in pixels.
    pub x: u32,
    /// Top edge, in pixels.
    pub y: u32,
    /// Width, in pixels.
    pub width: u32,
    /// Height, in pixels.
    pub height: u32,
}

/// One tile's scores plus everything an agent needs to locate it without an image viewer.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct TileDiff {
    /// Zero-based row in the tile grid (0 = top).
    pub row: usize,
    /// Zero-based column in the tile grid (0 = left).
    pub col: usize,
    /// The tile's pixel rectangle.
    pub rect: TileRect,
    /// This tile's SSIM score: the minimum over the windows centred in it (same formula and
    /// constants as the global score).
    pub ssim: f64,
    /// The largest of the tile's mean absolute R, G, B and A channel differences (0–255 scale).
    #[serde(default)]
    pub color_error: f64,
    /// A coarse human name for the tile's position, one of the nine compass-style labels documented
    /// on `region_name`.
    pub region: String,
}

/// The tile grid's shape (for the report's `tile_grid` field).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct TileGrid {
    /// Number of tile columns.
    pub cols: usize,
    /// Number of tile rows.
    pub rows: usize,
}

/// The full result of comparing two same-sized images.
#[derive(Clone, Debug)]
pub struct Comparison {
    /// The mean SSIM over every window (see the module doc for the small-image fallback).
    pub ssim: f64,
    /// Mean absolute per-channel difference (0–255 scale) over every pixel and every channel
    /// (R, G, B, A).
    pub mae: f64,
    /// The largest [`TileDiff::color_error`] of any tile.
    pub max_color_error: f64,
    /// The tile grid's shape.
    pub grid: TileGrid,
    /// Every tile, in row-major order (row 0 first, left to right within a row).
    pub tiles: Vec<TileDiff>,
}

/// Compares two same-dimensioned RGBA8 images. Panics if `a` and `b` differ in size — callers
/// (`ludion-testkit`'s [`crate::golden`]) check dimensions first, since a mismatch is reported
/// differently (no SSIM at all, not a low one).
pub fn compare(a: &DecodedImage, b: &DecodedImage) -> Comparison {
    assert_eq!(
        (a.width, a.height),
        (b.width, b.height),
        "compare() requires equal dimensions"
    );
    let (width, height) = (a.width, a.height);

    let luma_a = luma_plane(a);
    let luma_b = luma_plane(b);

    let cols = tile_bounds(width);
    let rows = tile_bounds(height);

    let windowed = windowed_ssim(
        &luma_a,
        &luma_b,
        width as usize,
        height as usize,
        &cols,
        &rows,
        None,
    );
    let color_errors = tile_color_errors(a, b, &cols, &rows);

    let mut tiles = Vec::with_capacity(cols.len() * rows.len());
    let mut flat_sum = 0.0_f64;
    let mut total_pixels = 0.0_f64;

    for (row_index, &(y, h)) in rows.iter().enumerate() {
        for (col_index, &(x, w)) in cols.iter().enumerate() {
            let index = row_index * cols.len() + col_index;
            let pixels = f64::from(w) * f64::from(h);
            let ssim = match windowed.tile_min[index] {
                Some(score) => score,
                None => {
                    let flat = ssim_of_region(&luma_a, &luma_b, width, x, y, w, h);
                    flat_sum += flat * pixels;
                    flat
                }
            };
            total_pixels += pixels;
            tiles.push(TileDiff {
                row: row_index,
                col: col_index,
                rect: TileRect {
                    x,
                    y,
                    width: w,
                    height: h,
                },
                ssim,
                color_error: color_errors[index],
                region: region_name(row_index, rows.len(), col_index, cols.len()),
            });
        }
    }

    let ssim = if windowed.count > 0 {
        windowed.sum / windowed.count as f64
    } else if total_pixels > 0.0 {
        flat_sum / total_pixels
    } else {
        1.0
    };
    let max_color_error = tiles.iter().fold(0.0_f64, |m, t| m.max(t.color_error));

    Comparison {
        ssim,
        mae: mean_absolute_error(a, b),
        max_color_error,
        grid: TileGrid {
            cols: cols.len(),
            rows: rows.len(),
        },
        tiles,
    }
}

/// The result of [`masked_ssim`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MaskedSsim {
    /// The mean window SSIM over the windows lying wholly inside the mask.
    pub ssim: f64,
    /// How many windows that is.
    pub windows: usize,
}

/// The mean SSIM of the 11×11 windows lying wholly inside `mask` (row-major, one flag per pixel),
/// scored exactly as [`compare`] scores them. A mask's edge is eroded by the window for free.
/// `None` when no window qualifies. Panics on a size mismatch between the images or the mask.
pub fn masked_ssim(a: &DecodedImage, b: &DecodedImage, mask: &[bool]) -> Option<MaskedSsim> {
    assert_eq!(
        (a.width, a.height),
        (b.width, b.height),
        "masked_ssim() requires equal dimensions"
    );
    assert_eq!(
        mask.len(),
        a.width as usize * a.height as usize,
        "masked_ssim() requires one mask flag per pixel"
    );
    let (width, height) = (a.width as usize, a.height as usize);
    let cols = tile_bounds(a.width);
    let rows = tile_bounds(a.height);
    let windowed = windowed_ssim(
        &luma_plane(a),
        &luma_plane(b),
        width,
        height,
        &cols,
        &rows,
        Some(mask),
    );
    (windowed.count > 0).then(|| MaskedSsim {
        ssim: windowed.sum / windowed.count as f64,
        windows: windowed.count as usize,
    })
}

/// Mean absolute difference per channel (R, G, B, A), 0–255 scale, over every pixel.
///
/// A secondary signal alongside SSIM: SSIM is a structural measure (it can shrug off a uniform
/// color shift that MAE catches immediately), so the two together give an agent two independent
/// readings on the same failure.
pub fn mean_absolute_error(a: &DecodedImage, b: &DecodedImage) -> f64 {
    assert_eq!(
        (a.width, a.height),
        (b.width, b.height),
        "compare() requires equal dimensions"
    );
    if a.rgba.is_empty() {
        return 0.0;
    }
    let sum: u64 = a
        .rgba
        .iter()
        .zip(b.rgba.iter())
        .map(|(&x, &y)| u64::from(x.abs_diff(y)))
        .sum();
    sum as f64 / a.rgba.len() as f64
}

/// Rec. 709 luma (ignoring alpha) for every pixel, as a flat row-major `f64` plane.
fn luma_plane(image: &DecodedImage) -> Vec<f64> {
    image
        .rgba
        .chunks_exact(4)
        .map(|px| 0.2126 * f64::from(px[0]) + 0.7152 * f64::from(px[1]) + 0.0722 * f64::from(px[2]))
        .collect()
}

/// Splits a dimension of `size` pixels into up to [`TILE_GRID`] tiles, returning `(start, length)`
/// pairs that exactly cover `0..size`. Tiles are as equal as possible; any remainder is distributed
/// one pixel at a time to the first tiles, so no tile is ever more than one pixel larger than
/// another. Images smaller than `TILE_GRID` in this dimension get one tile per pixel instead of
/// padding out to eight.
fn tile_bounds(size: u32) -> Vec<(u32, u32)> {
    if size == 0 {
        return Vec::new();
    }
    let count = TILE_GRID.min(size as usize).max(1);
    let base = size / count as u32;
    let remainder = size % count as u32;
    let mut bounds = Vec::with_capacity(count);
    let mut cursor = 0u32;
    for i in 0..count {
        let length = base + u32::from(i < remainder as usize);
        bounds.push((cursor, length));
        cursor += length;
    }
    bounds
}

/// For each pixel coordinate along one axis, the index of the tile (in `bounds`) containing it.
fn tile_index_per_pixel(bounds: &[(u32, u32)]) -> Vec<usize> {
    let mut map = Vec::new();
    for (index, &(_, length)) in bounds.iter().enumerate() {
        map.extend(std::iter::repeat_n(index, length as usize));
    }
    map
}

/// The sliding-window result: the sum and count of every window's score, and each tile's worst
/// window (`None` when no window is centred in the tile).
struct Windowed {
    sum: f64,
    count: u64,
    tile_min: Vec<Option<f64>>,
}

/// Runs the 11×11 binomial window over every fully-inside position of the two luma planes.
///
/// Separable: a horizontal pass writes five moment planes (`a`, `b`, `a²`, `b²`, `ab`), then the
/// vertical pass combines eleven of their rows per output row and scores each window on the fly.
fn windowed_ssim(
    a: &[f64],
    b: &[f64],
    width: usize,
    height: usize,
    cols: &[(u32, u32)],
    rows: &[(u32, u32)],
    mask: Option<&[bool]>,
) -> Windowed {
    let mut result = Windowed {
        sum: 0.0,
        count: 0,
        tile_min: vec![None; cols.len() * rows.len()],
    };
    if width < WINDOW || height < WINDOW {
        return result;
    }
    let out_w = width - (WINDOW - 1);
    let out_h = height - (WINDOW - 1);
    let col_tile = tile_index_per_pixel(cols);
    let row_tile = tile_index_per_pixel(rows);
    let inside = mask.map(|m| mask_window_counts(m, width, height));

    let mut planes: [Vec<f64>; 5] = std::array::from_fn(|_| vec![0.0; out_w * height]);
    let mut moments: [Vec<f64>; 5] = std::array::from_fn(|_| vec![0.0; width]);
    for y in 0..height {
        let ra = &a[y * width..(y + 1) * width];
        let rb = &b[y * width..(y + 1) * width];
        for x in 0..width {
            moments[0][x] = ra[x];
            moments[1][x] = rb[x];
            moments[2][x] = ra[x] * ra[x];
            moments[3][x] = rb[x] * rb[x];
            moments[4][x] = ra[x] * rb[x];
        }
        for (plane, moment) in planes.iter_mut().zip(&moments) {
            let out = &mut plane[y * out_w..(y + 1) * out_w];
            for (k, &numerator) in KERNEL_NUMERATORS.iter().enumerate() {
                let weight = numerator / KERNEL_DENOMINATOR;
                for (o, &m) in out.iter_mut().zip(&moment[k..k + out_w]) {
                    *o += weight * m;
                }
            }
        }
    }

    let c1 = (K1 * DYNAMIC_RANGE).powi(2);
    let c2 = (K2 * DYNAMIC_RANGE).powi(2);
    let mut acc: [Vec<f64>; 5] = std::array::from_fn(|_| vec![0.0; out_w]);
    for oy in 0..out_h {
        for (acc_plane, plane) in acc.iter_mut().zip(&planes) {
            acc_plane.fill(0.0);
            for (k, &numerator) in KERNEL_NUMERATORS.iter().enumerate() {
                let weight = numerator / KERNEL_DENOMINATOR;
                let row = &plane[(oy + k) * out_w..(oy + k + 1) * out_w];
                for (o, &m) in acc_plane.iter_mut().zip(row) {
                    *o += weight * m;
                }
            }
        }
        let tile_row = row_tile[oy + RADIUS] * cols.len();
        for ox in 0..out_w {
            if let Some(counts) = &inside
                && counts[oy * out_w + ox] != (WINDOW * WINDOW) as u32
            {
                continue;
            }
            let (mean_a, mean_b) = (acc[0][ox], acc[1][ox]);
            let var_a = (acc[2][ox] - mean_a * mean_a).max(0.0);
            let var_b = (acc[3][ox] - mean_b * mean_b).max(0.0);
            // Cauchy–Schwarz bound: keeps rounding from pushing the covariance past what the two
            // clamped variances allow, which would break exact 1.0 on identical images.
            let bound = (var_a * var_b).sqrt();
            let covariance = (acc[4][ox] - mean_a * mean_b).clamp(-bound, bound);
            let score = ssim_index(mean_a, mean_b, var_a, var_b, covariance, c1, c2);
            result.sum += score;
            result.count += 1;
            let slot = &mut result.tile_min[tile_row + col_tile[ox + RADIUS]];
            *slot = Some(slot.map_or(score, |m| m.min(score)));
        }
    }
    result
}

/// For every window position (row-major over the `(width - 10) × (height - 10)` grid), how many of
/// its 121 pixels the mask covers; a window lies wholly inside the mask when it counts all of them.
fn mask_window_counts(mask: &[bool], width: usize, height: usize) -> Vec<u32> {
    let (out_w, out_h) = (width - (WINDOW - 1), height - (WINDOW - 1));
    let stride = width + 1;
    let mut integral = vec![0u32; stride * (height + 1)];
    for y in 0..height {
        let mut row = 0;
        for x in 0..width {
            row += u32::from(mask[y * width + x]);
            integral[(y + 1) * stride + x + 1] = integral[y * stride + x + 1] + row;
        }
    }
    let mut counts = Vec::with_capacity(out_w * out_h);
    for y in 0..out_h {
        for x in 0..out_w {
            let (x1, y1) = (x + WINDOW, y + WINDOW);
            counts.push(
                integral[y1 * stride + x1] + integral[y * stride + x]
                    - integral[y * stride + x1]
                    - integral[y1 * stride + x],
            );
        }
    }
    counts
}

/// The SSIM index from one window's moments.
fn ssim_index(
    mean_a: f64,
    mean_b: f64,
    var_a: f64,
    var_b: f64,
    covariance: f64,
    c1: f64,
    c2: f64,
) -> f64 {
    let numerator = (2.0 * mean_a * mean_b + c1) * (2.0 * covariance + c2);
    let denominator = (mean_a * mean_a + mean_b * mean_b + c1) * (var_a + var_b + c2);
    numerator / denominator
}

/// Per tile (row-major), the largest of the R, G, B and A mean absolute differences.
fn tile_color_errors(
    a: &DecodedImage,
    b: &DecodedImage,
    cols: &[(u32, u32)],
    rows: &[(u32, u32)],
) -> Vec<f64> {
    let width = a.width as usize;
    let mut errors = Vec::with_capacity(cols.len() * rows.len());
    for &(y, h) in rows {
        for &(x, w) in cols {
            let mut sums = [0u64; 4];
            for row in y..y + h {
                let start = (row as usize * width + x as usize) * 4;
                let end = start + w as usize * 4;
                for (pa, pb) in a.rgba[start..end]
                    .chunks_exact(4)
                    .zip(b.rgba[start..end].chunks_exact(4))
                {
                    for (sum, (&ca, &cb)) in sums.iter_mut().zip(pa.iter().zip(pb)) {
                        *sum += u64::from(ca.abs_diff(cb));
                    }
                }
            }
            let pixels = f64::from(w) * f64::from(h);
            let worst = sums.iter().copied().max().unwrap_or(0);
            errors.push(if pixels > 0.0 {
                worst as f64 / pixels
            } else {
                0.0
            });
        }
    }
    errors
}

/// SSIM over one rectangular region of two luma planes (each plane row-major with row stride
/// `plane_width`), using the region itself as the single SSIM window. The small-image fallback.
fn ssim_of_region(a: &[f64], b: &[f64], plane_width: u32, x: u32, y: u32, w: u32, h: u32) -> f64 {
    let n = f64::from(w) * f64::from(h);
    if n <= 0.0 {
        return 1.0;
    }

    let mut sum_a = 0.0;
    let mut sum_b = 0.0;
    for row in y..y + h {
        let start = (row * plane_width + x) as usize;
        let end = start + w as usize;
        for i in start..end {
            sum_a += a[i];
            sum_b += b[i];
        }
    }
    let mean_a = sum_a / n;
    let mean_b = sum_b / n;

    let mut var_a = 0.0;
    let mut var_b = 0.0;
    let mut covariance = 0.0;
    for row in y..y + h {
        let start = (row * plane_width + x) as usize;
        let end = start + w as usize;
        for i in start..end {
            let da = a[i] - mean_a;
            let db = b[i] - mean_b;
            var_a += da * da;
            var_b += db * db;
            covariance += da * db;
        }
    }
    var_a /= n;
    var_b /= n;
    covariance /= n;

    let c1 = (K1 * DYNAMIC_RANGE).powi(2);
    let c2 = (K2 * DYNAMIC_RANGE).powi(2);
    ssim_index(mean_a, mean_b, var_a, var_b, covariance, c1, c2)
}

/// A coarse compass-style name for a tile's position, from its row/column within the grid.
///
/// Rows and columns are bucketed into thirds (top/middle/bottom, left/center/right), giving one of
/// nine labels: `top-left`, `top-center`, `top-right`, `middle-left`, `center`, `middle-right`,
/// `bottom-left`, `bottom-center`, `bottom-right`. This is deliberately coarse — an agent with no
/// image viewer needs "where, roughly," not a precise coordinate (the tile's `rect` already gives
/// the exact pixels).
fn region_name(row: usize, rows: usize, col: usize, cols: usize) -> String {
    let row_bucket = bucket(row, rows);
    let col_bucket = bucket(col, cols);
    const ROW_NAMES: [&str; 3] = ["top", "middle", "bottom"];
    const COL_NAMES: [&str; 3] = ["left", "center", "right"];
    if row_bucket == 1 && col_bucket == 1 {
        "center".to_string()
    } else {
        format!("{}-{}", ROW_NAMES[row_bucket], COL_NAMES[col_bucket])
    }
}

/// Maps `index` in `0..count` onto one of three buckets (0/1/2), as evenly as `count` allows.
fn bucket(index: usize, count: usize) -> usize {
    if count <= 1 {
        return 1; // a single row/column is unambiguously the middle one
    }
    ((index * 3) / count).min(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, px: [u8; 4]) -> DecodedImage {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            rgba.extend_from_slice(&px);
        }
        DecodedImage {
            width,
            height,
            rgba,
        }
    }

    /// A deterministic high-frequency grey texture (an LCG), opaque.
    fn noise(width: u32, height: u32, seed: u32) -> DecodedImage {
        let mut state = seed;
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let v = (state >> 24) as u8;
            rgba.extend_from_slice(&[v, v, v, 255]);
        }
        DecodedImage {
            width,
            height,
            rgba,
        }
    }

    #[test]
    fn identical_images_score_exactly_one() {
        let image = solid(64, 64, [120, 60, 200, 255]);
        let result = compare(&image, &image);
        assert_eq!(
            result.ssim, 1.0,
            "identical images must score exactly 1.0, no tolerance"
        );
        assert_eq!(result.mae, 0.0);
        assert_eq!(result.max_color_error, 0.0);
        for tile in &result.tiles {
            assert_eq!(tile.ssim, 1.0, "every tile must also score exactly 1.0");
            assert_eq!(tile.color_error, 0.0);
        }
    }

    #[test]
    fn identical_textured_images_score_exactly_one() {
        let image = noise(97, 83, 7);
        let result = compare(&image, &image);
        assert_eq!(result.ssim, 1.0);
        assert!(result.tiles.iter().all(|t| t.ssim == 1.0));
    }

    #[test]
    fn fully_different_images_score_low() {
        let a = solid(64, 64, [0, 0, 0, 255]);
        let b = solid(64, 64, [255, 255, 255, 255]);
        let result = compare(&a, &b);
        assert!(result.ssim < 0.5, "ssim = {}", result.ssim);
        // R/G/B differ by the full 255; A does not (both fully opaque) — MAE averages all four
        // channels, so black-vs-white lands at 255*3/4 = 191.25, not 255.
        assert!(result.mae > 150.0, "mae = {}", result.mae);
        assert_eq!(result.max_color_error, 255.0);
    }

    #[test]
    fn localized_perturbation_is_isolated_to_its_neighbourhood() {
        // 64x64 image, tile grid 8x8 -> 8px tiles. Damage exactly tile (row 0, col 0): x in 0..8,
        // y in 0..8. Windows reach 5 px, so only the tile's immediate neighbours may bleed.
        let a = solid(64, 64, [200, 200, 200, 255]);
        let mut b = a.clone();
        for y in 0..8u32 {
            for x in 0..8u32 {
                let i = ((y * 64 + x) * 4) as usize;
                b.rgba[i..i + 3].copy_from_slice(&[10, 10, 10]);
            }
        }

        let result = compare(&a, &b);
        assert!(result.ssim < 1.0, "global ssim must reflect the damage");
        assert!(result.mae > 0.0);

        let damaged_tile = result
            .tiles
            .iter()
            .find(|t| t.row == 0 && t.col == 0)
            .unwrap();
        assert!(
            damaged_tile.ssim < 0.9,
            "damaged tile ssim = {}",
            damaged_tile.ssim
        );
        assert_eq!(damaged_tile.region, "top-left");
        assert!(damaged_tile.color_error > 100.0);

        for tile in &result.tiles {
            if tile.row > 1 || tile.col > 1 {
                assert_eq!(
                    tile.ssim, 1.0,
                    "tile ({}, {}) must be undamaged",
                    tile.row, tile.col
                );
                assert_eq!(tile.color_error, 0.0);
            }
        }
    }

    #[test]
    fn colour_only_change_passes_ssim_but_fails_colour_error() {
        // Both pixels have Rec. 709 luma of about 128: grey versus a saturated orange.
        let a = solid(64, 64, [128, 128, 128, 255]);
        let b = solid(64, 64, [200, 120, 0, 255]);
        let result = compare(&a, &b);
        assert!(result.ssim > 0.99, "luma is unchanged: {}", result.ssim);
        assert!(result.max_color_error >= 70.0, "{}", result.max_color_error);
    }

    #[test]
    fn a_flattened_16x16_patch_drops_its_tile_far_below_the_mean() {
        let a = noise(960, 540, 11);
        let mut b = a.clone();
        for y in 200..216usize {
            for x in 400..416usize {
                let i = (y * 960 + x) * 4;
                b.rgba[i..i + 3].copy_from_slice(&[128, 128, 128]);
            }
        }
        let result = compare(&a, &b);
        let worst = result
            .tiles
            .iter()
            .map(|t| t.ssim)
            .fold(f64::INFINITY, f64::min);
        assert!(worst < 0.5, "worst tile ssim = {worst}");
        assert!(
            result.ssim > 0.99,
            "the global mean hides it: {}",
            result.ssim
        );
    }

    #[test]
    fn tile_bounds_covers_the_whole_dimension_with_near_equal_tiles() {
        let bounds = tile_bounds(66); // 66 / 8 = 8 remainder 2
        assert_eq!(bounds.len(), 8);
        let total: u32 = bounds.iter().map(|&(_, len)| len).sum();
        assert_eq!(total, 66);
        // Contiguous, starting at 0.
        let mut cursor = 0;
        for &(start, len) in &bounds {
            assert_eq!(start, cursor);
            cursor += len;
        }
        // Near-equal: no tile is more than one pixel larger than the smallest.
        let min = bounds.iter().map(|&(_, l)| l).min().unwrap();
        let max = bounds.iter().map(|&(_, l)| l).max().unwrap();
        assert!(max - min <= 1);
    }

    #[test]
    fn tile_bounds_degrades_gracefully_below_the_grid_size() {
        let bounds = tile_bounds(3);
        assert_eq!(bounds, vec![(0, 1), (1, 1), (2, 1)]);
        let bounds = tile_bounds(0);
        assert!(bounds.is_empty());
    }

    #[test]
    fn region_names_cover_all_nine_compass_labels() {
        // 8x8 grid: bucket(0..2)=0, bucket(3..4)=1 roughly, bucket(6..7)=2.
        assert_eq!(region_name(0, 8, 0, 8), "top-left");
        assert_eq!(region_name(0, 8, 7, 8), "top-right");
        assert_eq!(region_name(7, 8, 0, 8), "bottom-left");
        assert_eq!(region_name(7, 8, 7, 8), "bottom-right");
        assert_eq!(region_name(3, 8, 3, 8), "center");
        assert_eq!(region_name(0, 8, 3, 8), "top-center");
        assert_eq!(region_name(3, 8, 0, 8), "middle-left");
    }

    #[test]
    fn single_pixel_images_compare_without_panicking() {
        let a = solid(1, 1, [10, 20, 30, 255]);
        let b = solid(1, 1, [10, 20, 30, 255]);
        let result = compare(&a, &b);
        assert_eq!(result.ssim, 1.0);
        assert_eq!(result.grid, TileGrid { cols: 1, rows: 1 });
    }

    #[test]
    fn images_below_the_window_size_use_the_flat_tile_score() {
        let a = noise(10, 40, 5);
        let b = solid(10, 40, [0, 0, 0, 255]);
        let result = compare(&a, &b);
        assert!(result.ssim < 0.5, "{}", result.ssim);
    }

    fn rect_mask(width: u32, height: u32, x0: u32, y0: u32, w: u32, h: u32) -> Vec<bool> {
        (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| (x0..x0 + w).contains(&x) && (y0..y0 + h).contains(&y))
            })
            .collect()
    }

    #[test]
    fn masked_identical_images_score_one_over_the_expected_windows() {
        let image = noise(64, 64, 3);
        let mask = rect_mask(64, 64, 10, 10, 20, 20);
        let result = masked_ssim(&image, &image, &mask).unwrap();
        assert_eq!(result.ssim, 1.0);
        assert_eq!(result.windows, 100);
    }

    #[test]
    fn an_empty_mask_has_no_score() {
        let image = noise(32, 32, 3);
        assert_eq!(masked_ssim(&image, &image, &vec![false; 32 * 32]), None);
        // A mask smaller than one window has none either.
        assert_eq!(
            masked_ssim(&image, &image, &rect_mask(32, 32, 4, 4, 10, 10)),
            None
        );
    }

    #[test]
    fn masked_ssim_ignores_damage_outside_and_sees_damage_inside() {
        let a = noise(64, 64, 3);
        let mask = rect_mask(64, 64, 10, 10, 20, 20);
        let mut outside = a.clone();
        outside.rgba[..3].copy_from_slice(&[255, 0, 0]);
        assert_eq!(masked_ssim(&a, &outside, &mask).unwrap().ssim, 1.0);

        let mut inside = a.clone();
        let i = (20 * 64 + 20) * 4;
        inside.rgba[i..i + 3].copy_from_slice(&[0, 0, 0]);
        assert!(masked_ssim(&a, &inside, &mask).unwrap().ssim < 1.0);
    }
}
