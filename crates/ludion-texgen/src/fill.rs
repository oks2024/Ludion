//! Region ops on a thresholded height: `flood_fill` labels connected regions, `distance` measures how
//! far each texel is from the nearest inside texel. Both treat the tile as a torus.

use ludion_core::math::f64::sqrt;
use ludion_core::math::sin_cos;
use ludion_core::par;

use crate::hash::{fold32, hash2, lowbias32, unit};
use crate::image::Image;
use crate::ops::{BandSide, FillOutput};

/// One connected region of inside texels.
struct Region {
    /// Smallest unwrapped column and row.
    min: (i64, i64),
    /// Bounding box size in texels, at most the tile.
    span: (i64, i64),
    /// Hash of the quantized centroid: the region's random value.
    hash: u32,
}

const NONE: u32 = u32::MAX;

/// Labels the 4-connected regions of texels `>= threshold` (wrapping across the tile edge) and
/// fills each per `output`; texels outside every region are 0.
///
/// A region's random value comes from its centroid quantized to 1/64 tile and `step_seed`, so a
/// region keeps its value across build sizes; two regions whose centroids round to the same 1/64
/// cell share one. `angle` and `angle_jitter` orient the `gradient` output.
pub fn flood_fill(
    input: &Image,
    threshold: f32,
    output: FillOutput,
    angle: f32,
    angle_jitter: f32,
    step_seed: u64,
) -> Image {
    let n = input.size as usize;
    let seed = fold32(step_seed);
    let mut label = vec![NONE; n * n];
    let mut unwrapped = vec![(0i64, 0i64); n * n];
    let mut regions: Vec<Region> = Vec::new();
    let mut queue: Vec<usize> = Vec::new();
    for start in 0..n * n {
        if label[start] != NONE || input.data[start] < threshold {
            continue;
        }
        let id = regions.len() as u32;
        let (sx, sy) = ((start % n) as i64, (start / n) as i64);
        label[start] = id;
        unwrapped[start] = (sx, sy);
        queue.clear();
        queue.push(start);
        let (mut min, mut max) = ((sx, sy), (sx, sy));
        let (mut sum_x, mut sum_y, mut count) = (0i64, 0i64, 0i64);
        let mut head = 0;
        while head < queue.len() {
            let at = queue[head];
            head += 1;
            let (ux, uy) = unwrapped[at];
            let (x, y) = ((at % n) as i64, (at / n) as i64);
            min = (min.0.min(ux), min.1.min(uy));
            max = (max.0.max(ux), max.1.max(uy));
            sum_x += ux;
            sum_y += uy;
            count += 1;
            for (dx, dy) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
                let nx = (x + dx).rem_euclid(n as i64) as usize;
                let ny = (y + dy).rem_euclid(n as i64) as usize;
                let next = ny * n + nx;
                if label[next] == NONE && input.data[next] >= threshold {
                    label[next] = id;
                    unwrapped[next] = (ux + dx, uy + dy);
                    queue.push(next);
                }
            }
        }
        let centre = |sum: i64| ((sum as f64 / count as f64 + 0.5) / n as f64).rem_euclid(1.0);
        let quant = |c: f64| ((c * 64.0 + 0.5) as u32) % 64;
        regions.push(Region {
            min,
            span: (
                (max.0 - min.0 + 1).min(n as i64),
                (max.1 - min.1 + 1).min(n as i64),
            ),
            hash: hash2(quant(centre(sum_x)), quant(centre(sum_y)), seed),
        });
    }
    let channels = if output == FillOutput::Uv { 3 } else { 1 };
    Image::generate(input.size, channels, |x, y, t| {
        let at = y as usize * n + x as usize;
        let Some(region) = regions.get(label[at] as usize) else {
            return;
        };
        let (ux, uy) = unwrapped[at];
        let px = (((ux - region.min.0) as f32 + 0.5) / region.span.0 as f32).clamp(0.0, 1.0);
        let py = (((uy - region.min.1) as f32 + 0.5) / region.span.1 as f32).clamp(0.0, 1.0);
        match output {
            FillOutput::Random => t[0] = unit(region.hash),
            FillOutput::Gradient => {
                let jitter = unit(lowbias32(region.hash ^ 0x9E37_79B9)) - 0.5;
                let radians = (angle + jitter * 360.0 * angle_jitter).to_radians();
                let (dy, dx) = sin_cos(radians);
                let (low, high) = (dx.min(0.0) + dy.min(0.0), dx.max(0.0) + dy.max(0.0));
                t[0] = ((px * dx + py * dy - low) / (high - low)).clamp(0.0, 1.0);
            }
            FillOutput::Uv => {
                t[0] = px;
                t[1] = py;
                t[2] = unit(region.hash);
            }
        }
    })
}

/// "No feature in reach"; large enough to dwarf any squared distance, small enough to add to.
const FAR: f64 = 1e20;

/// Felzenszwalb–Huttenlocher lower envelope: `out[q] = min over p of (q - p)² + f[p]`.
fn squared_edt(f: &[f64], out: &mut [f64]) {
    let n = f.len();
    let mut v = vec![0usize; n];
    let mut z = vec![0.0f64; n + 1];
    let mut k = 0usize;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        let at = |i: usize| f[i] + (i * i) as f64;
        // z[0] is -inf, so the walk back stops at k == 0.
        let s = loop {
            let s = (at(q) - at(v[k])) / (2.0 * (q as f64 - v[k] as f64));
            if s > z[k] {
                break s;
            }
            k -= 1;
        };
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    k = 0;
    for (q, o) in out.iter_mut().enumerate() {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let d = q as f64 - v[k] as f64;
        *o = d * d + f[v[k]];
    }
}

/// Squared periodic distance along every row of `data` (`n` rows of `n`), in place.
fn edt_rows(data: &mut [f64], n: usize) {
    par::par_chunks(data, n, |_, row| {
        let line: Vec<f64> = (0..3 * n).map(|i| row[i % n]).collect();
        let mut out = vec![0.0; 3 * n];
        squared_edt(&line, &mut out);
        row.copy_from_slice(&out[n..2 * n]);
    });
}

fn transpose(data: &[f64], n: usize) -> Vec<f64> {
    let mut out = vec![0.0; n * n];
    for y in 0..n {
        for x in 0..n {
            out[x * n + y] = data[y * n + x];
        }
    }
    out
}

/// Squared periodic texel distance to the nearest texel with `target(value)`, indexed
/// `[x * n + y]`; `FAR` where there is none.
fn squared_texel_distances(input: &Image, target: impl Fn(f32) -> bool) -> Vec<f64> {
    let n = input.size as usize;
    let mut grid: Vec<f64> = input
        .data
        .iter()
        .map(|v| if target(*v) { 0.0 } else { FAR })
        .collect();
    edt_rows(&mut grid, n);
    let mut columns = transpose(&grid, n);
    edt_rows(&mut columns, n);
    columns
}

/// Distance in tile units from each texel centre to the nearest texel `>= threshold`, periodic,
/// divided by `max` and clamped to `[0, 1]`. An input with no such texel is all 1.
pub fn distance(input: &Image, threshold: f32, max: f32) -> Image {
    let n = input.size as usize;
    let columns = squared_texel_distances(input, |v| v >= threshold);
    let (size, max) = (n as f64, f64::from(max));
    Image::generate(input.size, 1, |x, y, t| {
        let d2 = columns[x as usize * n + y as usize];
        t[0] = (sqrt(d2) / size / max).clamp(0.0, 1.0) as f32;
    })
}

/// A line of constant `width` (tile units) along the `level` contour of a gray height: 1 on the
/// contour, falling to 0 at the band's edge over the last `softness` share of it.
///
/// Texels at or above `level` are inside. A texel's distance is measured to the nearest texel on
/// the other side, less half a texel since the contour lies between them. `both` lights
/// `width / 2` each way; `inside` and `outside` light `width` on their own side only. An input
/// wholly on one side has no contour and gives 0.
pub fn band(input: &Image, level: f32, width: f32, side: BandSide, softness: f32) -> Image {
    let n = input.size as usize;
    let size = n as f64;
    let to_outside = squared_texel_distances(input, |v| v < level);
    let to_inside = squared_texel_distances(input, |v| v >= level);
    let reach = f64::from(match side {
        BandSide::Both => width / 2.0,
        BandSide::Inside | BandSide::Outside => width,
    });
    let softness = f64::from(softness);
    Image::generate(input.size, 1, |x, y, t| {
        let at = x as usize * n + y as usize;
        let inside = input.data[y as usize * n + x as usize] >= level;
        if inside && side == BandSide::Outside || !inside && side == BandSide::Inside {
            return;
        }
        let d2 = if inside {
            to_outside[at]
        } else {
            to_inside[at]
        };
        let edge = (sqrt(d2) / size - 0.5 / size).max(0.0);
        t[0] = if softness > 0.0 {
            ((reach - edge) / (reach * softness)).clamp(0.0, 1.0)
        } else if edge < reach {
            1.0
        } else {
            0.0
        } as f32;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask(size: u32, inside: impl Fn(i64, i64) -> bool + Sync) -> Image {
        Image::generate(size, 1, |x, y, t| {
            t[0] = if inside(i64::from(x), i64::from(y)) {
                1.0
            } else {
                0.0
            }
        })
    }

    #[test]
    fn a_square_across_both_seams_is_one_region() {
        let m = mask(32, |x, y| !(4..28).contains(&x) && !(4..28).contains(&y));
        let out = flood_fill(&m, 0.5, FillOutput::Random, 0.0, 0.0, 9);
        let v = out.texel(0, 0)[0];
        assert!(v > 0.0);
        for (x, y) in [(31, 31), (3, 3), (0, 31), (31, 0), (3, 28)] {
            assert_eq!(out.texel(x, y)[0], v, "({x}, {y})");
        }
        assert_eq!(out.texel(10, 10)[0], 0.0);
    }

    #[test]
    fn a_region_keeps_its_random_across_sizes() {
        let grid = |size: u32| {
            let cell = i64::from(size) / 4;
            mask(size, move |x, y| {
                (x % cell >= cell / 4 && x % cell < cell * 3 / 4)
                    && (y % cell >= cell / 4 && y % cell < cell * 3 / 4)
            })
        };
        let (a, b) = (
            flood_fill(&grid(64), 0.5, FillOutput::Random, 0.0, 0.0, 5),
            flood_fill(&grid(128), 0.5, FillOutput::Random, 0.0, 0.0, 5),
        );
        let mut seen = Vec::new();
        for j in 0..4 {
            for i in 0..4 {
                let at = |image: &Image| {
                    let cell = i64::from(image.size) / 4;
                    image.texel(i * cell + cell / 2, j * cell + cell / 2)[0]
                };
                assert_eq!(at(&a), at(&b), "square ({i}, {j})");
                seen.push(at(&a).to_bits());
            }
        }
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 16, "each square has its own value");
    }

    #[test]
    fn uv_rises_left_to_right_inside_a_square() {
        let m = mask(32, |x, y| (8..16).contains(&x) && (8..16).contains(&y));
        let out = flood_fill(&m, 0.5, FillOutput::Uv, 0.0, 0.0, 0);
        assert_eq!(out.channels, 3);
        assert!(out.texel(9, 12)[0] < out.texel(14, 12)[0]);
        assert!(out.texel(12, 9)[1] < out.texel(12, 14)[1]);
        assert_eq!(out.texel(12, 12)[2], out.texel(9, 9)[2]);
        assert_eq!(out.texel(0, 0), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn gradient_runs_along_its_angle() {
        let m = mask(32, |x, y| (8..16).contains(&x) && (8..16).contains(&y));
        let out = flood_fill(&m, 0.5, FillOutput::Gradient, 0.0, 0.0, 0);
        assert!(out.texel(8, 12)[0] < out.texel(15, 12)[0]);
        assert_eq!(out.texel(12, 8)[0], out.texel(12, 15)[0]);
    }

    #[test]
    fn distance_matches_a_brute_force_periodic_search() {
        let n = 32i64;
        let m = Image::generate(32, 1, |x, y, t| {
            t[0] = if lowbias32(y * 32 + x).is_multiple_of(23) {
                1.0
            } else {
                0.0
            }
        });
        let out = distance(&m, 0.5, 1.0);
        for y in 0..n {
            for x in 0..n {
                let mut best = f64::INFINITY;
                for sy in 0..n {
                    for sx in 0..n {
                        if m.texel(sx, sy)[0] < 0.5 {
                            continue;
                        }
                        let (dx, dy) = ((x - sx).abs(), (y - sy).abs());
                        let (dx, dy) = (dx.min(n - dx) as f64, dy.min(n - dy) as f64);
                        best = best.min((dx * dx + dy * dy).sqrt());
                    }
                }
                let want = best / n as f64;
                let got = f64::from(out.texel(x, y)[0]);
                assert!((got - want).abs() < 1e-6, "({x}, {y}): {got} vs {want}");
            }
        }
    }

    fn disc(size: u32) -> Image {
        let (c, r) = (f64::from(size) / 2.0, f64::from(size) / 4.0);
        mask(size, move |x, y| {
            let (dx, dy) = (x as f64 + 0.5 - c, y as f64 + 0.5 - c);
            dx * dx + dy * dy < r * r
        })
    }

    #[test]
    fn band_lights_a_ring_on_the_contour() {
        let out = band(&disc(64), 0.5, 4.0 / 64.0, BandSide::Both, 0.5);
        assert!(out.texel(32 + 16, 32)[0] > 0.9, "{}", out.texel(48, 32)[0]);
        assert_eq!(out.texel(32, 32)[0], 0.0);
        assert_eq!(out.texel(32 + 26, 32)[0], 0.0);
    }

    #[test]
    fn band_sides_split_at_the_contour() {
        let m = disc(64);
        let inside = band(&m, 0.5, 4.0 / 64.0, BandSide::Inside, 0.5);
        let outside = band(&m, 0.5, 4.0 / 64.0, BandSide::Outside, 0.5);
        let lit = |image: &Image| {
            let mut seen = Vec::new();
            for y in 0..64 {
                for x in 0..64 {
                    if image.texel(x, y)[0] > 0.0 {
                        seen.push(m.texel(x, y)[0] >= 0.5);
                    }
                }
            }
            seen
        };
        let (a, b) = (lit(&inside), lit(&outside));
        assert!(!a.is_empty() && a.iter().all(|s| *s));
        assert!(!b.is_empty() && b.iter().all(|s| !*s));
    }

    #[test]
    fn band_width_is_in_tile_units() {
        let run = |size: u32| {
            let out = band(&disc(size), 0.5, 4.0 / 64.0, BandSide::Both, 0.0);
            let row = (0..size)
                .filter(|x| out.texel(i64::from(*x), i64::from(size / 2))[0] > 0.0)
                .count();
            row as f32 / size as f32
        };
        // Two ring crossings on a row through the centre: the left and right edges of the disc.
        assert!(
            (run(64) - run(128)).abs() <= 2.0 / 64.0,
            "{} {}",
            run(64),
            run(128)
        );
    }

    #[test]
    fn band_commutes_with_rolling() {
        let height = Image::generate(32, 1, |x, y, t| {
            t[0] = ((x * 7 + y * 13) % 11) as f32 / 11.0;
        });
        let rolled = Image::generate(32, 1, |x, y, t| {
            t.copy_from_slice(height.texel(i64::from(x) - 5, i64::from(y) - 9));
        });
        let a = band(&height, 0.5, 0.1, BandSide::Both, 0.5);
        let b = band(&rolled, 0.5, 0.1, BandSide::Both, 0.5);
        for y in 0..32 {
            for x in 0..32 {
                assert_eq!(a.texel(x, y)[0], b.texel(x + 5, y + 9)[0]);
            }
        }
    }

    #[test]
    fn a_flat_input_has_no_band() {
        for v in [0.0, 1.0] {
            let out = band(&Image::filled(16, 1, v), 0.5, 0.2, BandSide::Both, 0.5);
            assert!(out.data.iter().all(|v| *v == 0.0));
        }
    }

    #[test]
    fn nothing_inside_is_all_far() {
        let out = distance(&Image::filled(16, 1, 0.0), 0.5, 0.1);
        assert!(out.data.iter().all(|v| *v == 1.0));
    }
}
