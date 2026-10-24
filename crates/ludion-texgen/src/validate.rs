//! PBR plausibility measures over evaluated maps, and the engine's default bounds for them.
//!
//! Pure and sequential: each measure is one pass over a map in texel order, so the values are
//! bit-exact wherever the maps are.

use crate::eval::Maps;
use crate::image::Image;

/// The darkest and brightest plausible sRGB albedo luminance.
const ALBEDO_RANGE: (f32, f32) = (30.0 / 255.0, 240.0 / 255.0);

/// A seam ratio reported when the interior has no variation but the wrap edge does.
const SEAM_UNBOUNDED: f32 = 1000.0;

/// One named measurement of a map set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measure {
    /// The validator's name, as a rules file spells it.
    pub name: &'static str,
    /// The measured value.
    pub value: f32,
}

/// How a breach is reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// A warning (`L1287`).
    Warn,
    /// An error (`L1288`).
    Error,
}

/// The bounds a measure must stay inside, and how a breach is reported.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rule {
    /// The smallest allowed value, when bounded below.
    pub min: Option<f32>,
    /// The largest allowed value, when bounded above.
    pub max: Option<f32>,
    /// How a breach is reported.
    pub level: Level,
}

impl Rule {
    /// Whether `value` lies inside the bounds.
    pub fn passes(&self, value: f32) -> bool {
        self.min.is_none_or(|m| value >= m) && self.max.is_none_or(|m| value <= m)
    }
}

/// Min, mean and max of one map (a color map by luminance).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapStat {
    /// The smallest sample.
    pub min: f32,
    /// The mean sample.
    pub mean: f32,
    /// The largest sample.
    pub max: f32,
}

/// Every validator name, in report order.
pub const NAMES: [&str; 9] = [
    "albedo_out_of_range",
    "metallic_ambiguous",
    "roughness_mirror",
    "normal_invalid",
    "height_range",
    "seam_base_color",
    "seam_height",
    "repetition_peak",
    "repetition_lowfreq",
];

/// The engine's default bounds for the validator `name`, or `None` for an unknown name.
pub fn default_rule(name: &str) -> Option<Rule> {
    let (min, max, level) = match name {
        "albedo_out_of_range" => (None, Some(0.02), Level::Warn),
        "metallic_ambiguous" => (None, Some(0.10), Level::Warn),
        "roughness_mirror" => (None, Some(0.05), Level::Warn),
        "normal_invalid" => (None, Some(0.0), Level::Error),
        "height_range" => (Some(0.05), None, Level::Warn),
        "seam_base_color" | "seam_height" => (None, Some(1.5), Level::Warn),
        "repetition_peak" => (None, Some(0.6), Level::Warn),
        "repetition_lowfreq" => (None, Some(0.5), Level::Warn),
        _ => return None,
    };
    Some(Rule { min, max, level })
}

fn luminance(t: &[f32]) -> f32 {
    if t.len() == 1 {
        t[0]
    } else {
        0.2126 * t[0] + 0.7152 * t[1] + 0.0722 * t[2]
    }
}

fn share(image: &Image, outside: impl Fn(&[f32]) -> bool) -> f32 {
    let count = image.data.len() / image.channels;
    let hits = image
        .data
        .chunks(image.channels)
        .filter(|t| outside(t))
        .count();
    hits as f32 / count as f32
}

/// Mean |Δ| across the wrap divided by mean |Δ| between interior neighbours; about 1 when tileable.
fn seam(image: &Image) -> f32 {
    let n = image.size as i64;
    let l = |x: i64, y: i64| luminance(image.texel(x, y));
    let (mut wrap, mut inner) = (0.0f64, 0.0f64);
    for i in 0..n {
        wrap += f64::from((l(n - 1, i) - l(0, i)).abs());
        wrap += f64::from((l(i, n - 1) - l(i, 0)).abs());
        for j in 0..n - 1 {
            inner += f64::from((l(j + 1, i) - l(j, i)).abs());
            inner += f64::from((l(i, j + 1) - l(i, j)).abs());
        }
    }
    let wrap = wrap / (2 * n) as f64;
    let inner = inner / (2 * n * (n - 1)) as f64;
    if inner == 0.0 {
        return if wrap == 0.0 { 0.0 } else { SEAM_UNBOUNDED };
    }
    (wrap / inner) as f32
}

/// The side of the base-color luminance image the repetition measures run on.
const REPETITION_SIDE: u32 = 64;

/// The mean-removed luminance of `image` at `min(size, 64)` and its variance; `None` when flat.
fn centred_luminance(image: &Image) -> Option<(Vec<f64>, usize, f64)> {
    let small = image.downsample(REPETITION_SIDE);
    let r = small.size as usize;
    let values: Vec<f64> = small
        .data
        .chunks(small.channels)
        .map(|t| f64::from(luminance(t)))
        .collect();
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let centred: Vec<f64> = values.iter().map(|v| v - mean).collect();
    let v0 = centred.iter().map(|v| v * v).sum::<f64>() / centred.len() as f64;
    (v0 > 0.0).then_some((centred, r, v0))
}

/// The smallest of the image's and its two axis-residuals' autocorrelation peaks. A pattern that
/// is a function of one coordinate (grain, board rows) vanishes once that axis' profile is
/// removed; a 2-D lattice keeps a periodic residual in both. Axis-aligned only.
fn repetition_peak(base_color: &Image) -> f32 {
    let Some((l, r, v0)) = centred_luminance(base_color) else {
        return 0.0;
    };
    let full = peak(&l, r, v0);
    let residual_peak = |res: Vec<f64>| {
        let vr = res.iter().map(|v| v * v).sum::<f64>() / res.len() as f64;
        if vr <= 1e-6 * v0 {
            0.0
        } else {
            peak(&res, r, vr)
        }
    };
    let rows = residual_peak(
        (0..r * r)
            .map(|i| {
                let row = &l[i / r * r..][..r];
                l[i] - row.iter().sum::<f64>() / r as f64
            })
            .collect(),
    );
    let cols = residual_peak(
        (0..r * r)
            .map(|i| l[i] - (0..r).map(|y| l[y * r + i % r]).sum::<f64>() / r as f64)
            .collect(),
    );
    (full.min(rows).min(cols)) as f32
}

/// The largest circular autocorrelation among the local maxima away from zero offset: near 1 when
/// the same features repeat across the tile, 0 when the correlation only falls off.
fn peak(l: &[f64], r: usize, v0: f64) -> f64 {
    let norm = (r * r) as f64 * v0;
    let mut a = vec![0.0f64; r * r];
    for dy in 0..r {
        for dx in 0..r {
            let mut sum = 0.0f64;
            for y in 0..r {
                let row = y * r;
                let shifted = ((y + dy) % r) * r;
                for x in 0..r {
                    sum += l[row + x] * l[shifted + (x + dx) % r];
                }
            }
            a[dy * r + dx] = sum / norm;
        }
    }
    let far = (r / 16).max(1);
    let toroidal = |d: usize| d.min(r - d);
    let mut best = 0.0f64;
    for dy in 0..r {
        for dx in 0..r {
            if toroidal(dx).max(toroidal(dy)) < far {
                continue;
            }
            let v = a[dy * r + dx];
            let is_max = (0..9).filter(|k| *k != 4).all(|k| {
                let (nx, ny) = ((dx + r + k % 3 - 1) % r, (dy + r + k / 3 - 1) % r);
                v >= a[ny * r + nx]
            });
            if is_max {
                best = best.max(v);
            }
        }
    }
    best
}

/// The variance of the 4x4 block means over the pixel variance: high when one tile-scale blotch
/// dominates the image.
fn repetition_lowfreq(base_color: &Image) -> f32 {
    let Some((l, r, v0)) = centred_luminance(base_color) else {
        return 0.0;
    };
    let bounds = |i: usize| (i * r / 4, (i + 1) * r / 4);
    let mut means = [0.0f64; 16];
    for (b, mean) in means.iter_mut().enumerate() {
        let ((x0, x1), (y0, y1)) = (bounds(b % 4), bounds(b / 4));
        let mut sum = 0.0f64;
        for y in y0..y1 {
            for x in x0..x1 {
                sum += l[y * r + x];
            }
        }
        *mean = sum / ((x1 - x0) * (y1 - y0)) as f64;
    }
    let grand = means.iter().sum::<f64>() / 16.0;
    let var = means.iter().map(|m| (m - grand) * (m - grand)).sum::<f64>() / 16.0;
    (var / v0).clamp(0.0, 1.0) as f32
}

/// Measures `maps` against every validator, in [`NAMES`] order.
pub fn measure(maps: &Maps) -> Vec<Measure> {
    let (lo, hi) = ALBEDO_RANGE;
    let albedo = share(&maps.base_color, |t| {
        let y = crate::encode::srgb(luminance(t));
        y < lo || y > hi
    });
    let metallic = share(&maps.metallic, |t| t[0] > 0.1 && t[0] < 0.9);
    let mirror = share(&maps.roughness, |t| t[0] < 0.05);
    let normal = share(&maps.normal, |t| {
        let (x, y, z) = (t[0] * 2.0 - 1.0, t[1] * 2.0 - 1.0, t[2] * 2.0 - 1.0);
        let len = (x * x + y * y + z * z).sqrt();
        (len - 1.0).abs() > 0.05 || z <= 0.0
    });
    let (hmin, hmax) = extent(&maps.height);
    let values = [
        albedo,
        metallic,
        mirror,
        normal,
        hmax - hmin,
        seam(&maps.base_color),
        seam(&maps.height),
        repetition_peak(&maps.base_color),
        repetition_lowfreq(&maps.base_color),
    ];
    NAMES
        .iter()
        .zip(values)
        .map(|(name, value)| Measure { name, value })
        .collect()
}

fn extent(image: &Image) -> (f32, f32) {
    image
        .data
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)))
}

fn stat(image: &Image) -> MapStat {
    let values: Vec<f32> = image.data.chunks(image.channels).map(luminance).collect();
    let (min, max) = values
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    let mean = values.iter().map(|v| f64::from(*v)).sum::<f64>() / values.len() as f64;
    MapStat {
        min,
        mean: mean as f32,
        max,
    }
}

/// Per-map statistics for the report: `base_color`, `roughness`, `metallic`, `normal`,
/// `occlusion`, `emissive` (when authored) and `height`.
pub fn map_stats(maps: &Maps) -> Vec<(&'static str, MapStat)> {
    let mut stats = vec![
        ("base_color", stat(&maps.base_color)),
        ("roughness", stat(&maps.roughness)),
        ("metallic", stat(&maps.metallic)),
        ("normal", stat(&maps.normal)),
        ("occlusion", stat(&maps.occlusion)),
    ];
    if let Some(emissive) = &maps.emissive {
        stats.push(("emissive", stat(emissive)));
    }
    stats.push(("height", stat(&maps.height)));
    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(size: u32, base: f32, rough: f32, metal: f32, height: f32) -> Maps {
        Maps {
            base_color: Image::filled(size, 3, base),
            roughness: Image::filled(size, 1, rough),
            metallic: Image::filled(size, 1, metal),
            normal: Image::generate(size, 3, |_, _, t| t.copy_from_slice(&[0.5, 0.5, 1.0])),
            occlusion: Image::filled(size, 1, 1.0),
            emissive: None,
            height: Image::filled(size, 1, height),
        }
    }

    fn value(measures: &[Measure], name: &str) -> f32 {
        measures.iter().find(|m| m.name == name).unwrap().value
    }

    #[test]
    fn flat_maps_hit_the_expected_values() {
        let m = measure(&flat(8, 0.2, 0.5, 0.0, 0.5));
        assert_eq!(m.len(), NAMES.len());
        assert_eq!(value(&m, "albedo_out_of_range"), 0.0);
        assert_eq!(value(&m, "metallic_ambiguous"), 0.0);
        assert_eq!(value(&m, "roughness_mirror"), 0.0);
        assert_eq!(value(&m, "normal_invalid"), 0.0);
        assert_eq!(value(&m, "height_range"), 0.0);
        assert_eq!(value(&m, "seam_height"), 0.0);
        let m = measure(&flat(8, 0.0, 0.01, 0.5, 0.5));
        assert_eq!(value(&m, "albedo_out_of_range"), 1.0);
        assert_eq!(value(&m, "roughness_mirror"), 1.0);
        assert_eq!(value(&m, "metallic_ambiguous"), 1.0);
    }

    #[test]
    fn invalid_normals_are_counted() {
        let mut maps = flat(4, 0.2, 0.5, 0.0, 0.5);
        maps.normal = Image::generate(4, 3, |x, _, t| {
            let z = if x == 0 { 0.2 } else { 1.0 };
            t.copy_from_slice(&[0.5, 0.5, z]);
        });
        assert_eq!(value(&measure(&maps), "normal_invalid"), 0.25);
    }

    #[test]
    fn a_hard_wrap_edge_is_a_large_seam_and_periodic_noise_is_not() {
        let mut maps = flat(32, 0.2, 0.5, 0.0, 0.5);
        maps.height = Image::generate(32, 1, |x, _, t| t[0] = x as f32 / 31.0);
        assert!(value(&measure(&maps), "seam_height") > 10.0);

        let recipe = "format: 1
kind: texture
steps:
  n:
    op: noise
    type: gradient
    scale: 4
    octaves: 3
outputs:
  base_color: n
  roughness: n
  height: n
";
        let loaded = crate::load(recipe, &crate::FsResolver::new(".")).unwrap();
        let maps = crate::evaluate(&loaded, 64, &mut |_, _| {});
        let seam = value(&measure(&maps), "seam_height");
        assert!((0.5..1.5).contains(&seam), "seam {seam}");
    }

    fn lattice(dots: u32) -> Image {
        Image::generate(64, 3, |x, y, t| {
            let cell = 64 / dots;
            let (cx, cy) = (x % cell, y % cell);
            let on = cx < cell / 2 && cy < cell / 2;
            t.fill(if on { 0.8 } else { 0.1 });
        })
    }

    #[test]
    fn a_lattice_of_repeated_features_peaks_and_flat_or_noise_does_not() {
        let mut maps = flat(64, 0.2, 0.5, 0.0, 0.5);
        maps.base_color = lattice(4);
        assert!(value(&measure(&maps), "repetition_peak") > 0.9);

        let flat_maps = flat(64, 0.2, 0.5, 0.0, 0.5);
        let m = measure(&flat_maps);
        assert_eq!(value(&m, "repetition_peak"), 0.0);
        assert_eq!(value(&m, "repetition_lowfreq"), 0.0);
    }

    fn peak_of(image: Image) -> f32 {
        let mut maps = flat(64, 0.2, 0.5, 0.0, 0.5);
        maps.base_color = image;
        value(&measure(&maps), "repetition_peak")
    }

    #[test]
    fn stripes_along_one_axis_are_not_a_grid() {
        let band = |v: u32| if (v / 8).is_multiple_of(2) { 0.8 } else { 0.1 };
        let horizontal = Image::generate(64, 3, |_, y, t| t.fill(band(y % 16)));
        let vertical = Image::generate(64, 3, |x, _, t| t.fill(band(x % 16)));
        assert!(peak_of(horizontal) < 0.2);
        assert!(peak_of(vertical) < 0.2);
    }

    #[test]
    fn a_board_floor_with_grain_along_the_boards_stays_low() {
        let boards = Image::generate(64, 3, |_, y, t| {
            let base = if (y / 8).is_multiple_of(2) { 0.7 } else { 0.3 };
            t.fill(base + 0.05 * ludion_core::math::sin(y as f32 * 1.7));
        });
        assert!(peak_of(boards) < 0.6);
    }

    #[test]
    fn a_grout_grid_is_still_a_grid() {
        let grout = Image::generate(64, 3, |x, y, t| {
            let line = x % 16 < 2 || y % 16 < 2;
            t.fill(if line { 0.1 } else { 0.7 });
        });
        assert!(peak_of(grout) > 0.6);
    }

    #[test]
    fn gradient_noise_has_no_strong_peak_and_a_blob_has_low_frequency_energy() {
        let recipe = "format: 1
kind: texture
steps:
  n:
    op: noise
    type: gradient
    scale: 16
outputs:
  base_color: n
  roughness: n
  height: n
";
        let loaded = crate::load(recipe, &crate::FsResolver::new(".")).unwrap();
        let mut maps = crate::evaluate(&loaded, 64, &mut |_, _| {});
        let m = measure(&maps);
        assert!(
            value(&m, "repetition_peak") < 0.6,
            "{}",
            value(&m, "repetition_peak")
        );
        assert!(
            value(&m, "repetition_lowfreq") < 0.2,
            "{}",
            value(&m, "repetition_lowfreq")
        );

        maps.base_color = Image::generate(64, 3, |x, y, t| {
            let (dx, dy) = (x as f32 - 31.5, y as f32 - 31.5);
            t.fill(if dx * dx + dy * dy < 14.0 * 14.0 {
                0.9
            } else {
                0.1
            });
        });
        assert!(value(&measure(&maps), "repetition_lowfreq") > 0.5);
    }

    #[test]
    fn defaults_cover_every_name_and_only_those() {
        for name in NAMES {
            assert!(default_rule(name).is_some(), "{name}");
        }
        assert!(default_rule("nope").is_none());
        assert!(default_rule("height_range").unwrap().passes(0.5));
        assert!(!default_rule("height_range").unwrap().passes(0.01));
    }

    #[test]
    fn map_stats_lists_emissive_only_when_authored() {
        let mut maps = flat(4, 0.2, 0.5, 0.0, 0.5);
        assert_eq!(map_stats(&maps).len(), 6);
        maps.emissive = Some(Image::filled(4, 3, 0.1));
        assert_eq!(map_stats(&maps).len(), 7);
    }
}
