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
pub const NAMES: [&str; 7] = [
    "albedo_out_of_range",
    "metallic_ambiguous",
    "roughness_mirror",
    "normal_invalid",
    "height_range",
    "seam_base_color",
    "seam_height",
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
