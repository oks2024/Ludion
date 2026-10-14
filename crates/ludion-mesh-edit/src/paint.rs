//! Op application: blending an authored colour into a map through a region's weight.
//!
//! The working map is `f64` **linear** light, converted once on the way in and once on the way out
//! through [`ludion_assets`]' sRGB helpers — the one definition of the transfer function, in the
//! crate that owns PNG colour. Ops therefore compose without a re-quantisation per op: a map is
//! rounded to bytes exactly once, after the last op and the dilate pass.
//!
//! An authored `color` is quantised to its sRGB byte before it is decoded, because that is the byte
//! the map can hold: painting at full weight then reproduces the authored colour exactly rather
//! than a value a round trip through 8 bits would have moved.

use ludion_assets::{Image, decode_srgb, encode_srgb};
use ludion_core::math::f64::sqrt;
use ludion_scene::{MeshEditKey, MeshEditRegion};
use serde::Serialize;

use crate::fill::FillWeights;
use crate::raster::Coverage;
use crate::region::{self, Aabb};

/// How far a written map bleeds outward past a UV island, in texels. Two covers bilinear filtering
/// and the first GPU-generated mip, so a lit blob does not pull the background across its border.
pub const DILATE_PAD: u32 = 2;

/// A map under edit: straight alpha and **linear** RGB, one `[r, g, b, a]` per texel in
/// [`Coverage`]'s row-major, top-row-first order.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkingMap {
    width: u32,
    height: u32,
    texels: Vec<[f64; 4]>,
}

impl WorkingMap {
    /// A `width` × `height` map of opaque black — what an op creates when the source has no map in
    /// the slot it targets.
    pub fn opaque_black(width: u32, height: u32) -> Self {
        WorkingMap {
            width,
            height,
            texels: vec![[0.0, 0.0, 0.0, 1.0]; (width as usize) * (height as usize)],
        }
    }

    /// The decoded map an op paints over, linearised.
    pub fn from_image(image: &Image) -> Self {
        WorkingMap {
            width: image.width,
            height: image.height,
            texels: image
                .rgba
                .chunks_exact(4)
                .map(|texel| {
                    [
                        f64::from(decode_srgb(texel[0])),
                        f64::from(decode_srgb(texel[1])),
                        f64::from(decode_srgb(texel[2])),
                        f64::from(texel[3]) / 255.0,
                    ]
                })
                .collect(),
        }
    }

    /// The map width in texels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The map height in texels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The map as tightly packed RGBA8, ready for [`ludion_assets::encode_png`]: the one rounding
    /// to bytes in the whole pipeline.
    pub fn to_rgba8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.texels.len() * 4);
        for texel in &self.texels {
            for channel in &texel[..3] {
                out.push(encode_srgb(channel.clamp(0.0, 1.0) as f32));
            }
            out.push((texel[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
        }
        out
    }
}

/// What one op changed: the instrument a blind author reads instead of the picture.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct PaintOutcome {
    /// Texels whose weight was above zero.
    pub texels: usize,
    /// The model-space box of the surface points those texels sit on, or `None` for an op that
    /// painted nothing.
    pub bounds: Option<Aabb>,
    /// How many distinct triangles those texels belong to.
    pub triangles: usize,
    /// Texels the region reached (weight above zero) before its key filter — equal to `texels`
    /// when the region has no key.
    #[serde(skip)]
    pub region_texels: usize,
    /// Texels the region reached at a weight above zero before `strength` scaled it — what tells a
    /// zero `strength` from a region that reaches nothing.
    #[serde(skip)]
    pub reached_texels: usize,
    /// The colour distance and sRGB bytes of the closest texel the key rejected, or `None` when it
    /// rejected nothing. The instrument behind a key that matched nothing.
    #[serde(skip)]
    pub nearest_rejected: Option<(f64, [u8; 3])>,
}

/// A region `key` evaluated against a key map at the extent of the map being painted: each target
/// texel carries the colour distance of the key-map texel under it.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyFilter {
    distances_sq: Vec<f64>,
    samples: Vec<[u8; 3]>,
    tolerance: f64,
    color_feather: f64,
}

impl KeyFilter {
    /// The texel's colour weight: `1` within the tolerance, fading linearly to `0` across the
    /// `color_feather` band past it, `0` beyond.
    fn weight(&self, index: usize) -> f64 {
        colour_weight(self.distances_sq[index], self.tolerance, self.color_feather)
    }
}

/// The colour weight of a texel `distance_sq` (squared sRGB distance) from the wanted colour: `1`
/// within `tolerance`, `(tolerance + color_feather - d) / color_feather` inside the band past it,
/// `0` beyond. A `color_feather` of `0` is the hard edge, compared on the squares so no `sqrt`
/// touches a texel inside the tolerance.
pub fn colour_weight(distance_sq: f64, tolerance: f64, color_feather: f64) -> f64 {
    if distance_sq <= tolerance * tolerance {
        return 1.0;
    }
    if color_feather <= 0.0 {
        return 0.0;
    }
    let reach = tolerance + color_feather;
    let distance = sqrt(distance_sq);
    if distance >= reach {
        return 0.0;
    }
    (reach - distance) / color_feather
}

/// A map's encoded RGBA8 read at another extent's texel centres: what a `key` and a `fill` compare
/// colours through.
#[derive(Clone, Debug, PartialEq)]
pub struct MapSampler {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

impl MapSampler {
    /// A sampler over tightly packed RGBA8 of `width` × `height`.
    pub fn new(rgba: Vec<u8>, width: u32, height: u32) -> Self {
        MapSampler {
            rgba,
            width,
            height,
        }
    }

    /// The sRGB bytes of the texel nearest the centre of texel (`x`, `y`) of a `target_w` ×
    /// `target_h` map, in integer math (`((2x + 1) * width) / (2 * target_w)`), so two maps of
    /// different extents still line up and the sample is the stored byte rather than a filtered
    /// blend.
    pub fn sample(&self, x: u32, y: u32, target_w: u32, target_h: u32) -> [u8; 3] {
        let (width, height) = (u64::from(self.width), u64::from(self.height));
        let (target_w, target_h) = (u64::from(target_w), u64::from(target_h));
        let ky = ((2 * u64::from(y) + 1) * height) / (2 * target_h);
        let kx = ((2 * u64::from(x) + 1) * width) / (2 * target_w);
        let at = ((ky * width + kx) * 4) as usize;
        [self.rgba[at], self.rgba[at + 1], self.rgba[at + 2]]
    }
}

/// The Euclidean distance squared between an sRGB byte colour and a wanted colour, both read as
/// components in `[0, 1]` — the one metric a `key` and a `fill` share.
pub fn colour_distance_sq(sample: [u8; 3], wanted: [f64; 3]) -> f64 {
    let mut squared = 0.0;
    for (byte, wanted) in sample.iter().zip(wanted) {
        let delta = f64::from(*byte) / 255.0 - wanted;
        squared += delta * delta;
    }
    squared
}

/// Builds the [`KeyFilter`] for a `width` × `height` target from the key map.
///
/// Each target texel samples the key map through [`MapSampler`]. The distance is Euclidean over the
/// sRGB components in `[0, 1]`.
pub fn key_filter(colours: &MapSampler, key: &MeshEditKey, width: u32, height: u32) -> KeyFilter {
    let mut distances_sq = Vec::with_capacity((width as usize) * (height as usize));
    let mut samples = Vec::with_capacity((width as usize) * (height as usize));
    for y in 0..height {
        for x in 0..width {
            let sample = colours.sample(x, y, width, height);
            distances_sq.push(colour_distance_sq(sample, key.color));
            samples.push(sample);
        }
    }
    KeyFilter {
        distances_sq,
        samples,
        tolerance: key.tolerance,
        color_feather: key.color_feather.unwrap_or(0.0),
    }
}

/// Blends `color` (an authored sRGB value in `[0, 1]`) into `map` wherever `region` reaches the
/// surface, weighted by the region's falloff times `strength`.
///
/// `coverage` must have been rasterised at `map`'s own extent; texels beyond either are ignored.
/// A `key` filter, built at that same extent, scales each texel's weight by its colour weight. A
/// `fill` region's weights are the flood's ([`FillWeights`]), not the signed-distance falloff.
pub fn paint(
    map: &mut WorkingMap,
    coverage: &Coverage,
    color: [f64; 3],
    strength: f64,
    region: &MeshEditRegion,
    fill: Option<&FillWeights>,
    key: Option<&KeyFilter>,
) -> PaintOutcome {
    let target = color.map(|component| f64::from(decode_srgb(to_srgb_byte(component))));
    let mut outcome = PaintOutcome {
        texels: 0,
        bounds: None,
        triangles: 0,
        region_texels: 0,
        reached_texels: 0,
        nearest_rejected: None,
    };
    let mut nearest_rejected: Option<(f64, usize)> = None;
    let mut touched = vec![false; coverage.triangles()];
    let texels = map.texels.len().min(coverage.len());

    for index in 0..texels {
        let Some((triangle, point)) = coverage.at(index) else {
            continue;
        };
        let base = match fill {
            Some(fill) => fill.weights[index],
            None => region::weight(region, point),
        };
        if base > 0.0 {
            outcome.reached_texels += 1;
        }
        let mut weight = base * strength;
        if weight <= 0.0 {
            continue;
        }
        outcome.region_texels += 1;
        if let Some(key) = key {
            let matched = key.weight(index);
            if matched <= 0.0 {
                let distance_sq = key.distances_sq[index];
                if nearest_rejected.is_none_or(|(nearest, _)| distance_sq < nearest) {
                    nearest_rejected = Some((distance_sq, index));
                }
                continue;
            }
            weight *= matched;
        }
        for (channel, target) in map.texels[index][..3].iter_mut().zip(target) {
            *channel = *channel * (1.0 - weight) + target * weight;
        }
        outcome.texels += 1;
        match &mut outcome.bounds {
            Some(bounds) => bounds.expand(point),
            none => *none = Some(Aabb::point(point)),
        }
        if let Some(seen) = touched.get_mut(triangle as usize)
            && !*seen
        {
            *seen = true;
            outcome.triangles += 1;
        }
    }
    if let (Some(key), Some((distance_sq, index))) = (key, nearest_rejected) {
        outcome.nearest_rejected = Some((sqrt(distance_sq), key.samples[index]));
    }
    outcome
}

/// Bleeds the map outward into texels no triangle covers, `pad` rings deep, so bilinear sampling
/// and the first mip cannot pull the background across a UV island's border.
///
/// Each ring is the average of the already-filled texels among a texel's eight neighbours, read
/// from a snapshot of the ring before it — so the result does not depend on the order texels are
/// visited in.
pub fn dilate(map: &mut WorkingMap, coverage: &Coverage, pad: u32) {
    let (width, height) = (map.width as i64, map.height as i64);
    let mut filled: Vec<bool> = (0..map.texels.len())
        .map(|index| coverage.at(index).is_some())
        .collect();

    for _ in 0..pad {
        let previous = map.texels.clone();
        let settled = filled.clone();
        for y in 0..height {
            for x in 0..width {
                let index = (y * width + x) as usize;
                if settled[index] {
                    continue;
                }
                let mut sum = [0.0; 4];
                let mut count = 0.0;
                for (dy, dx) in NEIGHBOURS {
                    let (ny, nx) = (y + dy, x + dx);
                    if ny < 0 || nx < 0 || ny >= height || nx >= width {
                        continue;
                    }
                    let neighbour = (ny * width + nx) as usize;
                    if !settled[neighbour] {
                        continue;
                    }
                    for (total, value) in sum.iter_mut().zip(previous[neighbour]) {
                        *total += value;
                    }
                    count += 1.0;
                }
                if count == 0.0 {
                    continue;
                }
                map.texels[index] = sum.map(|total| total / count);
                filled[index] = true;
            }
        }
    }
}

/// The eight neighbours in a fixed order, so the sum that averages them is order-stable.
const NEIGHBOURS: [(i64, i64); 8] = [
    (-1, -1),
    (-1, 0),
    (-1, 1),
    (0, -1),
    (0, 1),
    (1, -1),
    (1, 0),
    (1, 1),
];

/// How many texels of an encoded RGBA8 map have a non-black colour — the "is anything lit?" count
/// a report carries per written map.
pub fn non_black_texels(rgba: &[u8]) -> usize {
    rgba.chunks_exact(4)
        .filter(|texel| texel[0] != 0 || texel[1] != 0 || texel[2] != 0)
        .count()
}

/// An authored sRGB component in `[0, 1]` as the byte a map stores.
fn to_srgb_byte(component: f64) -> u8 {
    (component.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::rasterize;
    use ludion_scene::MeshEditRegionShape;

    /// A unit square in the XZ plane whose UVs fill the middle half of the map, so there is an
    /// island border to bleed across.
    fn island(width: u32) -> Coverage {
        let positions = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0];
        let uvs = [0.25, 0.25, 0.75, 0.25, 0.75, 0.75, 0.25, 0.75];
        rasterize(&positions, &uvs, &[0, 1, 2, 0, 2, 3], width, width).expect("the island covers")
    }

    fn everywhere() -> MeshEditRegion {
        MeshEditRegion {
            shape: MeshEditRegionShape::Box {
                at: [0.5, 0.0, 0.5],
                size: [4.0, 4.0, 4.0],
            },
            feather: None,
            key: None,
        }
    }

    #[test]
    fn a_created_map_starts_opaque_black() {
        let map = WorkingMap::opaque_black(2, 2);
        assert_eq!(map.to_rgba8(), [0u8, 0, 0, 255].repeat(4));
        assert_eq!(non_black_texels(&map.to_rgba8()), 0);
    }

    /// Full weight reproduces the authored colour exactly: the byte the author asked for is the
    /// byte the map holds, which is what makes a report's non-black count mean something.
    #[test]
    fn full_weight_writes_the_authored_colour() {
        let coverage = island(8);
        let mut map = WorkingMap::opaque_black(8, 8);
        let outcome = paint(
            &mut map,
            &coverage,
            [0.05, 1.0, 0.35],
            1.0,
            &everywhere(),
            None,
            None,
        );
        assert_eq!(outcome.texels, 16);
        assert_eq!(outcome.triangles, 2);
        let rgba = map.to_rgba8();
        let lit = 8 * 3 + 3;
        assert_eq!(&rgba[lit * 4..lit * 4 + 4], &[13, 255, 89, 255]);
        assert_eq!(non_black_texels(&rgba), 16);
    }

    /// Half weight is a blend in **linear** light: black and white at 0.5 is linear 0.5, which
    /// encodes to 188, not to the 128 a blend on the stored bytes would give.
    #[test]
    fn a_half_weight_blend_is_linear() {
        let coverage = island(8);
        let mut map = WorkingMap::opaque_black(8, 8);
        paint(
            &mut map,
            &coverage,
            [1.0, 1.0, 1.0],
            0.5,
            &everywhere(),
            None,
            None,
        );
        let rgba = map.to_rgba8();
        let lit = 8 * 3 + 3;
        assert_eq!(&rgba[lit * 4..lit * 4 + 3], &[188, 188, 188]);
    }

    #[test]
    fn ops_compose_without_a_round_trip_through_bytes() {
        let coverage = island(8);
        let mut once = WorkingMap::opaque_black(8, 8);
        paint(
            &mut once,
            &coverage,
            [1.0, 1.0, 1.0],
            0.75,
            &everywhere(),
            None,
            None,
        );

        let mut twice = WorkingMap::opaque_black(8, 8);
        paint(
            &mut twice,
            &coverage,
            [1.0, 1.0, 1.0],
            0.5,
            &everywhere(),
            None,
            None,
        );
        paint(
            &mut twice,
            &coverage,
            [1.0, 1.0, 1.0],
            0.5,
            &everywhere(),
            None,
            None,
        );
        assert_eq!(once.to_rgba8(), twice.to_rgba8(), "1 - (1-0.5)^2 == 0.75");
    }

    #[test]
    fn a_region_that_misses_the_surface_paints_nothing() {
        let coverage = island(8);
        let mut map = WorkingMap::opaque_black(8, 8);
        let far = MeshEditRegion {
            shape: MeshEditRegionShape::Sphere {
                at: [50.0, 50.0, 50.0],
                radius: 1.0,
            },
            feather: Some(0.5),
            key: None,
        };
        let outcome = paint(&mut map, &coverage, [1.0, 1.0, 1.0], 1.0, &far, None, None);
        assert_eq!(outcome.texels, 0);
        assert_eq!(outcome.bounds, None);
        assert_eq!(outcome.triangles, 0);
        assert_eq!(non_black_texels(&map.to_rgba8()), 0);
    }

    #[test]
    fn the_outcome_bounds_the_painted_surface() {
        let coverage = island(8);
        let mut map = WorkingMap::opaque_black(8, 8);
        let half = MeshEditRegion {
            shape: MeshEditRegionShape::Box {
                at: [0.25, 0.0, 0.5],
                size: [0.5, 1.0, 4.0],
            },
            feather: None,
            key: None,
        };
        let outcome = paint(&mut map, &coverage, [1.0, 0.0, 0.0], 1.0, &half, None, None);
        let bounds = outcome.bounds.expect("half the island is painted");
        assert!(bounds.max[0] <= 0.5, "{bounds:?}");
        assert_eq!(outcome.texels, 8);
    }

    fn red_key(tolerance: f64) -> MeshEditKey {
        MeshEditKey {
            map: ludion_scene::MeshEditMap::BaseColor,
            color: [1.0, 0.0, 0.0],
            tolerance,
            color_feather: None,
        }
    }

    #[test]
    fn the_colour_weight_fades_across_the_band() {
        // Dyadic values, so every distance and weight below is exact.
        assert_eq!(
            colour_weight(0.0625, 0.25, 0.5),
            1.0,
            "exactly on the tolerance"
        );
        assert_eq!(colour_weight(0.0, 0.0, 0.0), 1.0, "an exact match");
        assert_eq!(colour_weight(0.25, 0.25, 0.5), 0.5, "0.5 is mid-band");
        assert_eq!(
            colour_weight(0.5625, 0.25, 0.5),
            0.0,
            "0.75 is the band's end"
        );
        assert_eq!(colour_weight(1.0, 0.25, 0.5), 0.0, "beyond the band");
        assert_eq!(
            colour_weight(0.25, 0.25, 0.0),
            0.0,
            "no feather is the hard edge"
        );
    }

    /// A key map whose columns step away from red by 32/255 per column: with a tolerance of 0.26
    /// and a colour feather of 0.5, the weight is 1 up to 0.26, then falls linearly to 0 at 0.76.
    #[test]
    fn a_color_feather_softens_the_key_edge() {
        let coverage = island(8);
        let mut key_map = Vec::new();
        for _ in 0..8 {
            for x in 0..8u8 {
                key_map.extend_from_slice(&[255 - x * 32, 0, 0, 255]);
            }
        }
        let key = MeshEditKey {
            color_feather: Some(0.5),
            ..red_key(0.26)
        };
        let filter = key_filter(&MapSampler::new(key_map, 8, 8), &key, 8, 8);
        let distance = |x: usize| f64::from(x as u8 * 32) / 255.0;
        for x in 0..8 {
            let expected = if distance(x) <= 0.26 {
                1.0
            } else {
                ((0.76 - distance(x)) / 0.5).max(0.0)
            };
            assert!(
                (filter.weight(x) - expected).abs() < 1e-12,
                "column {x}: {} vs {expected}",
                filter.weight(x)
            );
        }

        let mut map = WorkingMap::opaque_black(8, 8);
        let outcome = paint(
            &mut map,
            &coverage,
            [1.0, 1.0, 1.0],
            1.0,
            &everywhere(),
            None,
            Some(&filter),
        );
        // The island covers columns 2..6: distances 0.25, 0.38, 0.50, 0.63 — none past the band.
        assert_eq!(outcome.texels, 16);
        assert_eq!(outcome.nearest_rejected, None);
        let rgba = map.to_rgba8();
        let row = 3 * 8;
        let reds: Vec<u8> = (2..6).map(|x| rgba[(row + x) * 4]).collect();
        assert_eq!(reds[0], 255, "inside the tolerance paints fully");
        assert!(
            reds.windows(2).all(|pair| pair[0] > pair[1]),
            "the paint fades with the colour distance: {reds:?}"
        );
    }

    /// An absent `color_feather` and an explicit `0` paint exactly what the hard key did.
    #[test]
    fn a_zero_color_feather_is_the_hard_key() {
        let coverage = island(8);
        let mut key_map = Vec::new();
        for _ in 0..8 {
            for x in 0..8u8 {
                key_map.extend_from_slice(&[255 - x * 32, 0, 0, 255]);
            }
        }
        let painted = |key: &MeshEditKey| {
            let filter = key_filter(&MapSampler::new(key_map.clone(), 8, 8), key, 8, 8);
            let mut map = WorkingMap::opaque_black(8, 8);
            let outcome = paint(
                &mut map,
                &coverage,
                [1.0, 1.0, 1.0],
                1.0,
                &everywhere(),
                None,
                Some(&filter),
            );
            (outcome, map.to_rgba8())
        };
        let absent = painted(&red_key(0.3));
        let zero = painted(&MeshEditKey {
            color_feather: Some(0.0),
            ..red_key(0.3)
        });
        assert_eq!(absent, zero);
        assert_eq!(
            absent.0.texels, 4,
            "only column 2 of the island is within 0.3"
        );
    }

    /// A key map red on the island's left half and black elsewhere keeps exactly those texels, and
    /// reports the closest one it rejected: black is a full 1.0 from red.
    #[test]
    fn a_key_keeps_only_the_texels_it_matches() {
        let coverage = island(8);
        let mut key_map = [0u8, 0, 0, 255].repeat(64);
        for (x, y) in (2..4).flat_map(|x| (2..6).map(move |y| (x, y))) {
            let at = (y * 8 + x) * 4;
            key_map[at..at + 3].copy_from_slice(&[255, 0, 0]);
        }
        let filter = key_filter(&MapSampler::new(key_map, 8, 8), &red_key(0.1), 8, 8);
        let mut map = WorkingMap::opaque_black(8, 8);
        let outcome = paint(
            &mut map,
            &coverage,
            [1.0, 1.0, 1.0],
            1.0,
            &everywhere(),
            None,
            Some(&filter),
        );
        assert_eq!(outcome.region_texels, 16);
        assert_eq!(outcome.texels, 8);
        assert_eq!(non_black_texels(&map.to_rgba8()), 8);
        assert_eq!(outcome.nearest_rejected, Some((1.0, [0, 0, 0])));
    }

    /// A tolerance below every texel's distance matches nothing, yet the region still reached
    /// texels — the two counts that tell `L1178` from `L1177`.
    #[test]
    fn a_key_that_matches_nothing_still_counts_the_region() {
        let coverage = island(8);
        let key_map = [0u8, 0, 0, 255].repeat(64);
        let filter = key_filter(&MapSampler::new(key_map, 8, 8), &red_key(0.5), 8, 8);
        let mut map = WorkingMap::opaque_black(8, 8);
        let outcome = paint(
            &mut map,
            &coverage,
            [1.0, 1.0, 1.0],
            1.0,
            &everywhere(),
            None,
            Some(&filter),
        );
        assert_eq!((outcome.texels, outcome.region_texels), (0, 16));
    }

    /// Each target texel samples the key texel nearest its centre: a 4-wide target over a 2-wide
    /// key map reads key texel 0 for x = 0, 1 and key texel 1 for x = 2, 3.
    #[test]
    fn a_key_map_of_another_extent_is_sampled_at_texel_centres() {
        let key_map = [255, 0, 0, 255, 0, 0, 0, 255];
        let filter = key_filter(
            &MapSampler::new(key_map.to_vec(), 2, 1),
            &red_key(0.1),
            4,
            2,
        );
        let passes: Vec<bool> = (0..8).map(|index| filter.weight(index) > 0.0).collect();
        assert_eq!(passes, [true, true, false, false, true, true, false, false]);
    }

    /// The border rings take the colour and the ring past the last one stays background — what
    /// keeps a bilinear sample and the first mip from dragging black across the island edge.
    #[test]
    fn dilate_bleeds_exactly_pad_rings_past_the_island() {
        let coverage = island(8);
        let mut map = WorkingMap::opaque_black(8, 8);
        paint(
            &mut map,
            &coverage,
            [1.0, 1.0, 1.0],
            1.0,
            &everywhere(),
            None,
            None,
        );
        dilate(&mut map, &coverage, DILATE_PAD);

        let rgba = map.to_rgba8();
        let at = |x: usize, y: usize| rgba[(y * 8 + x) * 4];
        assert_eq!(at(3, 3), 255, "inside the island");
        assert_eq!(at(1, 3), 255, "one ring out");
        assert_eq!(at(0, 3), 255, "two rings out");
        assert_eq!(at(0, 0), 255, "the diagonal is a neighbour too");
        assert_eq!(non_black_texels(&rgba), 64, "an 8-wide map is fully bled");
    }

    #[test]
    fn dilate_leaves_a_texel_with_no_filled_neighbour_alone() {
        let coverage = island(16);
        let mut map = WorkingMap::opaque_black(16, 16);
        paint(
            &mut map,
            &coverage,
            [1.0, 1.0, 1.0],
            1.0,
            &everywhere(),
            None,
            None,
        );
        dilate(&mut map, &coverage, DILATE_PAD);
        let rgba = map.to_rgba8();
        assert_eq!(rgba[0], 0, "the far corner is three rings out");
        // The island is 8 texels wide; two rings of bleed make it 12.
        assert_eq!(non_black_texels(&rgba), 12 * 12);
    }

    #[test]
    fn dilate_does_not_depend_on_the_pass_order() {
        let coverage = island(16);
        let mut map = WorkingMap::opaque_black(16, 16);
        paint(
            &mut map,
            &coverage,
            [0.2, 0.4, 0.6],
            1.0,
            &everywhere(),
            None,
            None,
        );
        let mut twin = map.clone();
        dilate(&mut map, &coverage, 1);
        dilate(&mut twin, &coverage, 1);
        assert_eq!(map, twin);
    }

    #[test]
    fn an_existing_map_round_trips_through_the_working_form() {
        let image = Image {
            width: 2,
            height: 1,
            rgba: vec![13, 255, 89, 255, 0, 0, 0, 128],
            hash: 0,
        };
        assert_eq!(WorkingMap::from_image(&image).to_rgba8(), image.rgba);
    }
}
