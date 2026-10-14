//! The seeded flood fill: the texels a `fill` region reaches, walked over the **surface**.
//!
//! A fill starts at the surface point `at` and grows through neighbouring texels whose colour stays
//! within `tolerance` of the seed's and whose surface point stays within `max_radius` of `at` in a
//! straight 3D line. Its membership is **reachability**, so the member set does not depend on the
//! order texels are visited in; the walk still uses a FIFO queue and a fixed neighbour order so
//! nothing else does either.
//!
//! # Adjacency
//!
//! Two texels are neighbours when the surface says so, not when the UV layout happens to put them
//! side by side:
//!
//! 1. **Weld.** A vertex's canonical id is the lowest vertex index with the same position bits
//!    (`-0.0` folded into `0.0`). The weld is exact — a generated mesh duplicates a vertex where a UV
//!    seam runs, and the copies share their bits; a vertex merely *near* another is a different one.
//! 2. **Edges.** Each triangle has three welded edges. Two triangles meeting on an edge are
//!    **UV-continuous** when their own corner UVs agree at both ends, and their islands are then one
//!    union-find component; when the UVs differ the edge is a **seam**. Triangles that meet only at
//!    a vertex are not adjacent.
//! 3. **Grid step.** Two 4-neighbour texels connect when both are covered and their triangles share
//!    an island. Diagonal neighbours never connect, and islands that merely touch in UV never leak
//!    into each other.
//! 4. **Seam bridge.** Each seam is cut into one segment per texel of its longer UV side; at every
//!    cut, each side's UV point finds the nearest covered texel *of that side's island* within
//!    [`BRIDGE_REACH`] texels, and the pair is connected both ways.
//! 5. **Uncovered texels** are never nodes: the walk crosses a UV gap only through a seam bridge.
//!
//! Every tie goes to the lower triangle index, then the lower texel index, and every irrational
//! operation goes through [`ludion_core::math`], so a fill evaluates to the same texels on both
//! pinned platforms.

use std::collections::{BTreeMap, VecDeque};

use ludion_core::math::f64::sqrt;

use crate::paint::{MapSampler, colour_weight};
use crate::raster::Coverage;

/// How far, in texels, a seam bridge looks on each side for a texel of the island it is bridging.
pub const BRIDGE_REACH: i64 = 2;

/// How far from the surface a seed may sit, as a fraction of the mesh's bounding-box diagonal,
/// before it counts as off the surface. A picked point is on the surface to float precision; this
/// allows for the digits a document holds it in.
pub const SEED_EPSILON_FRACTION: f64 = 1e-3;

/// One UV seam: two triangles that share a welded edge but not its UVs.
#[derive(Clone, Debug, PartialEq)]
pub struct Seam {
    /// The two triangles, by index.
    pub triangles: [u32; 2],
    /// Each triangle's UVs at the edge's two ends: `uv[side][end]`, with end `0` the endpoint of
    /// lower canonical id.
    pub uv: [[[f64; 2]; 2]; 2],
}

/// The mesh's surface adjacency: which UV island each triangle is in, and where the islands part.
#[derive(Clone, Debug, PartialEq)]
pub struct Surface {
    /// The island of each triangle: the lowest triangle index of its UV-continuous component.
    pub island: Vec<u32>,
    /// Every seam, in the order the welded edges sort.
    pub seams: Vec<Seam>,
}

/// One triangle on a welded edge: its index and its UVs at the edge's lower and higher end.
type EdgeSide = (u32, [f64; 2], [f64; 2]);

/// Builds the [`Surface`] of a mesh whose lists [`crate::raster::rasterize`] has already validated
/// (`positions` 3 floats per vertex, `uvs` 2, `indices` 3 per triangle).
pub fn surface(positions: &[f64], uvs: &[f64], indices: &[u32]) -> Surface {
    let vertices = positions.len() / 3;
    let mut welded: BTreeMap<[u64; 3], u32> = BTreeMap::new();
    let canonical: Vec<u32> = (0..vertices)
        .map(|vertex| {
            // Adding 0.0 turns -0.0 into 0.0, so the two zeros weld.
            let key = [0, 1, 2].map(|axis| (positions[vertex * 3 + axis] + 0.0).to_bits());
            *welded.entry(key).or_insert(vertex as u32)
        })
        .collect();

    let uv = |vertex: usize| [uvs[vertex * 2], uvs[vertex * 2 + 1]];
    let triangles = indices.len() / 3;
    // Per welded edge: each triangle on it with its UVs at the lower and the higher end.
    let mut edges: BTreeMap<(u32, u32), Vec<EdgeSide>> = BTreeMap::new();
    for triangle in 0..triangles {
        let corners = [
            indices[triangle * 3] as usize,
            indices[triangle * 3 + 1] as usize,
            indices[triangle * 3 + 2] as usize,
        ];
        for (from, to) in [(0, 1), (1, 2), (2, 0)] {
            let (a, b) = (corners[from], corners[to]);
            let (low, high) = (canonical[a], canonical[b]);
            if low == high {
                continue;
            }
            let (at_low, at_high) = if low < high { (a, b) } else { (b, a) };
            edges
                .entry((low.min(high), low.max(high)))
                .or_default()
                .push((triangle as u32, uv(at_low), uv(at_high)));
        }
    }

    let mut parent: Vec<u32> = (0..triangles as u32).collect();
    let mut seams = Vec::new();
    for sharing in edges.values() {
        for (i, (first, first_low, first_high)) in sharing.iter().enumerate() {
            for (second, second_low, second_high) in &sharing[i + 1..] {
                if first == second {
                    continue;
                }
                if first_low == second_low && first_high == second_high {
                    union(&mut parent, *first, *second);
                } else {
                    seams.push(Seam {
                        triangles: [*first, *second],
                        uv: [[*first_low, *first_high], [*second_low, *second_high]],
                    });
                }
            }
        }
    }
    let island = (0..triangles as u32)
        .map(|triangle| find(&mut parent, triangle))
        .collect();
    Surface { island, seams }
}

fn find(parent: &mut [u32], mut node: u32) -> u32 {
    while parent[node as usize] != node {
        let grandparent = parent[parent[node as usize] as usize];
        parent[node as usize] = grandparent;
        node = grandparent;
    }
    node
}

/// Joins two components under the lower root, so an island is named by its lowest triangle.
fn union(parent: &mut [u32], a: u32, b: u32) {
    let (a, b) = (find(parent, a), find(parent, b));
    if a != b {
        parent[a.max(b) as usize] = a.min(b);
    }
}

/// The distance from `at` to the nearest point of the triangle list, and that point. The nearest
/// triangle wins a tie by lower index. A mesh with no triangle is infinitely far.
pub fn nearest_surface(positions: &[f64], indices: &[u32], at: [f64; 3]) -> (f64, [f64; 3]) {
    let vertex = |index: u32| {
        let at = index as usize * 3;
        [positions[at], positions[at + 1], positions[at + 2]]
    };
    let mut best: Option<(f64, [f64; 3])> = None;
    for corners in indices.chunks_exact(3) {
        let point = closest_on_triangle(
            at,
            vertex(corners[0]),
            vertex(corners[1]),
            vertex(corners[2]),
        );
        let squared = dot(sub(point, at), sub(point, at));
        if best.is_none_or(|(nearest, _)| squared < nearest) {
            best = Some((squared, point));
        }
    }
    match best {
        Some((squared, point)) => (sqrt(squared), point),
        None => (f64::INFINITY, at),
    }
}

/// The point of triangle `a`, `b`, `c` nearest `p`: the vertex, edge or face region `p` falls in.
fn closest_on_triangle(p: [f64; 3], a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> [f64; 3] {
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return add(a, scale(ab, d1 / (d1 - d3)));
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return add(a, scale(ac, d2 / (d2 - d6)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        let along = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return add(b, scale(sub(c, b), along));
    }
    let sum = va + vb + vc;
    if sum == 0.0 {
        // A triangle with no area has no face region; its nearest point is on one of its edges.
        return nearest_of(
            p,
            [
                closest_on_segment(p, a, b),
                closest_on_segment(p, b, c),
                closest_on_segment(p, c, a),
            ],
        );
    }
    add(add(a, scale(ab, vb / sum)), scale(ac, vc / sum))
}

fn closest_on_segment(p: [f64; 3], from: [f64; 3], to: [f64; 3]) -> [f64; 3] {
    let segment = sub(to, from);
    let squared = dot(segment, segment);
    let along = if squared > 0.0 {
        (dot(sub(p, from), segment) / squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    add(from, scale(segment, along))
}

fn nearest_of(p: [f64; 3], candidates: [[f64; 3]; 3]) -> [f64; 3] {
    let mut best = candidates[0];
    for candidate in candidates {
        if dot(sub(candidate, p), sub(candidate, p)) < dot(sub(best, p), sub(best, p)) {
            best = candidate;
        }
    }
    best
}

/// The texels a fill reached, each with its weight.
#[derive(Clone, Debug, PartialEq)]
pub struct FillWeights {
    /// One weight per texel of the map, row-major like [`Coverage`]; `0` outside the fill.
    pub weights: Vec<f64>,
    /// The texel the walk started from.
    pub seed: usize,
}

/// Why a fill could not start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FillRefusal {
    /// The texel nearest the seed is farther from it than `max_radius`, or with a `feather` no
    /// nearer than it.
    SeedOutsideRadius {
        /// How far the nearest texel's surface point is from the seed.
        distance: f64,
    },
    /// The coverage has no covered texel at all, so there is nothing to walk.
    NoTexels,
}

/// A fill's parameters, as its region document spells them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FillSpec {
    /// The seed point.
    pub at: [f64; 3],
    /// The colour distance from the seed's colour that joins the patch at full weight.
    pub tolerance: f64,
    /// The straight-line reach from `at`.
    pub max_radius: f64,
    /// The world-unit fade inside `max_radius`.
    pub feather: Option<f64>,
    /// The colour-distance fade past `tolerance`.
    pub color_feather: Option<f64>,
}

/// Floods from the surface texel nearest `spec.at`.
///
/// A texel is a member when the walk reaches it, its surface point is within `max_radius` of `at`,
/// and its `colours` sample is nearer than `tolerance + color_feather` to the seed texel's by
/// Euclidean sRGB distance in `[0, 1]` (within `tolerance` inclusive when there is no colour
/// feather). A member's weight is its radius weight — `1`, or with a `feather` falling linearly to
/// `0` over the last `feather` units inside the radius — times its colour weight
/// ([`crate::paint::colour_weight`]). The seed is always a member.
///
/// # Errors
/// [`FillRefusal`] when there is no seed texel or it lies outside `max_radius`.
pub fn flood(
    coverage: &Coverage,
    surface: &Surface,
    spec: &FillSpec,
    colours: &MapSampler,
) -> Result<FillWeights, FillRefusal> {
    let FillSpec {
        at,
        tolerance,
        max_radius,
        feather,
        color_feather,
    } = *spec;
    let color_feather = color_feather.unwrap_or(0.0);
    let (width, height) = (coverage.width(), coverage.height());
    let columns = width as usize;

    let mut seed: Option<(f64, u32, usize)> = None;
    for index in 0..coverage.len() {
        let Some((triangle, point)) = coverage.at(index) else {
            continue;
        };
        let offset = sub(point, at);
        let squared = dot(offset, offset);
        if seed.is_none_or(|(nearest, lowest, _)| {
            squared < nearest || (squared == nearest && triangle < lowest)
        }) {
            seed = Some((squared, triangle, index));
        }
    }
    let Some((squared, _, seed)) = seed else {
        return Err(FillRefusal::NoTexels);
    };
    let distance = sqrt(squared);
    // A feathered fill weighs `0` at the radius, so a seed exactly on it would paint nothing.
    let feathered = feather.is_some_and(|feather| feather > 0.0);
    if distance > max_radius || (feathered && distance >= max_radius) {
        return Err(FillRefusal::SeedOutsideRadius { distance });
    }

    let sample_at = |index: usize| {
        colours.sample(
            (index % columns) as u32,
            (index / columns) as u32,
            width,
            height,
        )
    };
    let wanted = sample_at(seed);
    let weight = |distance: f64| match feather {
        Some(feather) if feather > 0.0 => ((max_radius - distance) / feather).min(1.0),
        _ => 1.0,
    };
    // A texel's membership is its own property, so it is decided once, whichever neighbour asks.
    let member = |index: usize| -> Option<f64> {
        let (_, point) = coverage.at(index)?;
        let distance = sqrt(dot(sub(point, at), sub(point, at)));
        if distance > max_radius {
            return None;
        }
        let mut squared = 0.0;
        for (byte, wanted) in sample_at(index).iter().zip(wanted) {
            let delta = f64::from(*byte) / 255.0 - f64::from(wanted) / 255.0;
            squared += delta * delta;
        }
        let matched = colour_weight(squared, tolerance, color_feather);
        (matched > 0.0).then(|| weight(distance) * matched)
    };

    let bridges = bridges(coverage, surface);
    let island_of = |index: usize| coverage.at(index).map(|(t, _)| surface.island[t as usize]);
    let mut weights = vec![0.0; coverage.len()];
    let mut visited = vec![false; coverage.len()];
    let mut queue = VecDeque::new();
    visited[seed] = true;
    weights[seed] = weight(distance);
    queue.push_back(seed);

    while let Some(current) = queue.pop_front() {
        let (x, y) = (current % columns, current / columns);
        let island = island_of(current);
        let mut next: Vec<usize> = Vec::with_capacity(8);
        if y > 0 {
            next.push(current - columns);
        }
        if x > 0 {
            next.push(current - 1);
        }
        if x + 1 < columns {
            next.push(current + 1);
        }
        if y + 1 < height as usize {
            next.push(current + columns);
        }
        // A grid neighbour of another island is left unmarked: a seam bridge may still reach it.
        next.retain(|neighbour| island_of(*neighbour).is_some() && island_of(*neighbour) == island);
        let first = bridges.partition_point(|(from, _)| (*from as usize) < current);
        next.extend(
            bridges[first..]
                .iter()
                .take_while(|(from, _)| *from as usize == current)
                .map(|(_, to)| *to as usize),
        );
        for neighbour in next {
            if visited[neighbour] {
                continue;
            }
            visited[neighbour] = true;
            if let Some(weight) = member(neighbour) {
                weights[neighbour] = weight;
                queue.push_back(neighbour);
            }
        }
    }
    Ok(FillWeights { weights, seed })
}

/// The texel pairs every seam connects, sorted and deduplicated, both directions.
fn bridges(coverage: &Coverage, surface: &Surface) -> Vec<(u32, u32)> {
    let (width, height) = (f64::from(coverage.width()), f64::from(coverage.height()));
    let mut pairs = Vec::new();
    for seam in &surface.seams {
        let length = |side: usize| {
            let [low, high] = seam.uv[side];
            let (du, dv) = ((high[0] - low[0]) * width, (high[1] - low[1]) * height);
            sqrt(du * du + dv * dv)
        };
        let segments = length(0).max(length(1)).ceil().max(1.0) as u32;
        for step in 0..=segments {
            let s = f64::from(step) / f64::from(segments);
            let point = |side: usize| {
                let [low, high] = seam.uv[side];
                [
                    low[0] + (high[0] - low[0]) * s,
                    low[1] + (high[1] - low[1]) * s,
                ]
            };
            let a = nearest_texel(coverage, surface, seam.triangles[0], point(0));
            let b = nearest_texel(coverage, surface, seam.triangles[1], point(1));
            if let (Some(a), Some(b)) = (a, b) {
                pairs.push((a, b));
                pairs.push((b, a));
            }
        }
    }
    pairs.sort_unstable();
    pairs.dedup();
    pairs
}

/// The covered texel of `triangle`'s island whose centre is nearest the UV point, searched within
/// [`BRIDGE_REACH`] texels of the texel the point falls in; a tie goes to the lower index.
fn nearest_texel(
    coverage: &Coverage,
    surface: &Surface,
    triangle: u32,
    uv: [f64; 2],
) -> Option<u32> {
    let (width, height) = (i64::from(coverage.width()), i64::from(coverage.height()));
    let (u, v) = (uv[0] * width as f64, uv[1] * height as f64);
    let (cx, cy) = (u.floor() as i64, v.floor() as i64);
    let island = surface.island[triangle as usize];
    let mut best: Option<(f64, u32)> = None;
    for y in (cy - BRIDGE_REACH).max(0)..=(cy + BRIDGE_REACH).min(height - 1) {
        for x in (cx - BRIDGE_REACH).max(0)..=(cx + BRIDGE_REACH).min(width - 1) {
            let index = (y * width + x) as usize;
            let Some((covering, _)) = coverage.at(index) else {
                continue;
            };
            if surface.island[covering as usize] != island {
                continue;
            }
            let (dx, dy) = (x as f64 + 0.5 - u, y as f64 + 0.5 - v);
            let squared = dx * dx + dy * dy;
            if best.is_none_or(|(nearest, _)| squared < nearest) {
                best = Some((squared, index as u32));
            }
        }
    }
    best.map(|(_, index)| index)
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|axis| a[axis] - b[axis])
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|axis| a[axis] + b[axis])
}

fn scale(a: [f64; 3], by: f64) -> [f64; 3] {
    [0, 1, 2].map(|axis| a[axis] * by)
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::rasterize;

    /// A sampler over a `width` × `height` map, its texel `(x, y)` coloured by `colour(x, y)`.
    fn map(width: u32, height: u32, colour: impl Fn(u32, u32) -> [u8; 3]) -> MapSampler {
        let mut rgba = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let [r, g, b] = colour(x, y);
                rgba.extend_from_slice(&[r, g, b, 255]);
            }
        }
        MapSampler::new(rgba, width, height)
    }

    fn grey(width: u32, height: u32) -> MapSampler {
        map(width, height, |_, _| [128, 128, 128])
    }

    /// Two unit quads side by side in the XZ plane (x in 0..1 and 1..2), the shared-edge vertices
    /// **duplicated**, each quad its own UV island with a gap between them. Vertices 0..4 are the
    /// left quad, 4..8 the right; `nudge` moves the right quad's shared-edge vertices in x.
    fn strip(nudge: f64) -> (Vec<f64>, Vec<f64>, Vec<u32>) {
        let positions = vec![
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            1.0,
            0.0,
            1.0,
            0.0,
            0.0,
            1.0, //
            1.0 + nudge,
            0.0,
            0.0,
            2.0,
            0.0,
            0.0,
            2.0,
            0.0,
            1.0,
            1.0 + nudge,
            0.0,
            1.0,
        ];
        let uvs = vec![
            0.05, 0.05, 0.45, 0.05, 0.45, 0.95, 0.05, 0.95, //
            0.55, 0.05, 0.95, 0.05, 0.95, 0.95, 0.55, 0.95,
        ];
        (positions, uvs, vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7])
    }

    /// The unit quad in the XZ plane with UVs over the whole map: texel centres and their surface
    /// points are all dyadic, so every distance below is exact.
    fn plate() -> (Vec<f64>, Vec<f64>, Vec<u32>) {
        (
            vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0],
            vec![0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0],
            vec![0, 1, 2, 0, 2, 3],
        )
    }

    fn members(weights: &FillWeights) -> Vec<usize> {
        (0..weights.weights.len())
            .filter(|index| weights.weights[*index] > 0.0)
            .collect()
    }

    fn flooded(
        mesh: &(Vec<f64>, Vec<f64>, Vec<u32>),
        size: u32,
        at: [f64; 3],
        tolerance: f64,
        max_radius: f64,
        feather: Option<f64>,
        colours: &MapSampler,
    ) -> Result<FillWeights, FillRefusal> {
        let (positions, uvs, indices) = mesh;
        let coverage = rasterize(positions, uvs, indices, size, size).expect("the mesh covers");
        let surface = surface(positions, uvs, indices);
        let spec = FillSpec {
            at,
            tolerance,
            max_radius,
            feather,
            color_feather: None,
        };
        flood(&coverage, &surface, &spec, colours)
    }

    fn colour_flooded(
        size: u32,
        at: [f64; 3],
        tolerance: f64,
        color_feather: Option<f64>,
        colours: &MapSampler,
    ) -> FillWeights {
        let (positions, uvs, indices) = plate();
        let coverage = rasterize(&positions, &uvs, &indices, size, size).expect("covers");
        let surface = surface(&positions, &uvs, &indices);
        let spec = FillSpec {
            at,
            tolerance,
            max_radius: 10.0,
            feather: None,
            color_feather,
        };
        flood(&coverage, &surface, &spec, colours).expect("seeds")
    }

    /// Columns step away from the seed's black by 64/255 in red: a tolerance of 0.1 keeps column
    /// 0 at 1, a colour feather of 0.6 fades columns 1 and 2 (0.25, 0.50) and stops at column 3
    /// (0.75, past 0.7). The band texels are members, so the walk crosses them.
    #[test]
    fn a_color_feather_fades_the_fill_and_walks_through_the_band() {
        let colours = map(8, 8, |x, _| [(x * 64).min(255) as u8, 0, 0]);
        let filled = colour_flooded(8, [0.0625, 0.0, 0.0625], 0.1, Some(0.6), &colours);
        let distance = |x: u32| f64::from((x * 64).min(255)) / 255.0;
        for x in 0..8u32 {
            let expected = if x == 0 {
                1.0
            } else {
                ((0.7 - distance(x)) / 0.6).max(0.0)
            };
            let weight = filled.weights[x as usize];
            assert!(
                (weight - expected).abs() < 1e-12,
                "column {x}: {weight} vs {expected}"
            );
        }
        assert!(filled.weights[2] > 0.0 && filled.weights[3] == 0.0);
        // Every row reached column 2, which only the walk through column 1 could do.
        for y in 0..8 {
            assert!(filled.weights[y * 8 + 2] > 0.0, "row {y}");
        }
    }

    /// The band is a fade, not a bridge past the cut: a band texel beyond an out-of-reach column
    /// is not reached, and an absent colour feather is the hard edge bit-for-bit.
    #[test]
    fn the_band_stops_at_its_reach_and_an_absent_feather_is_the_hard_edge() {
        // Columns 0..2 black, column 2 a wall of full red, columns 3.. dark red again.
        let colours = map(8, 8, |x, _| match x {
            2 => [255, 0, 0],
            0 | 1 => [0, 0, 0],
            _ => [64, 0, 0],
        });
        let filled = colour_flooded(8, [0.0625, 0.0, 0.0625], 0.1, Some(0.3), &colours);
        assert!(filled.weights[1] == 1.0 && filled.weights[2] == 0.0);
        assert!(
            (3..8).all(|x| filled.weights[x] == 0.0),
            "the wall cuts the walk: {:?}",
            &filled.weights[..8]
        );

        let hard = colour_flooded(8, [0.0625, 0.0, 0.0625], 0.3, None, &colours);
        let zero = colour_flooded(8, [0.0625, 0.0, 0.0625], 0.3, Some(0.0), &colours);
        assert_eq!(hard, zero);
    }

    #[test]
    fn a_fill_crosses_a_uv_seam_through_the_weld() {
        let mesh = strip(0.0);
        let filled = flooded(&mesh, 32, [0.5, 0.0, 0.5], 0.1, 3.0, None, &grey(32, 32))
            .expect("the seed is on the left quad");
        let left = members(&filled).iter().filter(|i| **i % 32 < 16).count();
        let right = members(&filled).iter().filter(|i| **i % 32 >= 16).count();
        assert!(left > 0 && right > 0, "{left} left, {right} right");
    }

    /// A 1e-9 gap is a different vertex: the seam is not a seam, it is a boundary.
    #[test]
    fn the_weld_is_exact() {
        let mesh = strip(1e-9);
        let filled = flooded(&mesh, 32, [0.5, 0.0, 0.5], 0.1, 3.0, None, &grey(32, 32))
            .expect("the seed is on the left quad");
        assert!(!members(&filled).is_empty());
        assert!(members(&filled).iter().all(|index| index % 32 < 16));
    }

    /// Two quads far apart in 3D whose UV islands abut: the grid neighbours across the boundary
    /// belong to different islands, so nothing crosses.
    #[test]
    fn uv_touching_islands_do_not_leak() {
        let positions = vec![
            0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, //
            10.0, 0.0, 0.0, 11.0, 0.0, 0.0, 11.0, 0.0, 1.0, 10.0, 0.0, 1.0,
        ];
        let uvs = vec![
            0.05, 0.05, 0.5, 0.05, 0.5, 0.95, 0.05, 0.95, //
            0.5, 0.05, 0.95, 0.05, 0.95, 0.95, 0.5, 0.95,
        ];
        let mesh = (positions, uvs, vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
        let filled = flooded(&mesh, 32, [0.5, 0.0, 0.5], 0.1, 100.0, None, &grey(32, 32))
            .expect("the seed is on the left quad");
        assert!(!members(&filled).is_empty());
        assert!(members(&filled).iter().all(|index| index % 32 < 16));
    }

    /// Left texels black, right texels red: one channel apart by exactly 1, so the distance is
    /// exactly 1 and the boundary is exactly testable.
    #[test]
    fn the_tolerance_bound_is_inclusive() {
        let mesh = plate();
        let colours = map(16, 16, |x, _| if x < 8 { [0, 0, 0] } else { [255, 0, 0] });
        let coverage = rasterize(&mesh.0, &mesh.1, &mesh.2, 16, 16).expect("covers");
        let seed_at = [0.1, 0.0, 0.1];

        let below = flooded(&mesh, 16, seed_at, 1.0 - 1e-9, 10.0, None, &colours).expect("seeds");
        let expected: Vec<usize> = (0..256).filter(|index| index % 16 < 8).collect();
        assert_eq!(members(&below), expected);

        let exact = flooded(&mesh, 16, seed_at, 1.0, 10.0, None, &colours).expect("seeds");
        assert_eq!(members(&exact).len(), coverage.covered());
    }

    #[test]
    fn the_radius_bound_is_the_straight_line_count() {
        let mesh = plate();
        let coverage = rasterize(&mesh.0, &mesh.1, &mesh.2, 16, 16).expect("covers");
        let at = [0.4, 0.0, 0.55];
        let radius = 0.3;
        let filled = flooded(&mesh, 16, at, 0.1, radius, None, &grey(16, 16)).expect("seeds");
        let brute = (0..coverage.len())
            .filter(|index| {
                let (_, point) = coverage.at(*index).expect("the plate is fully covered");
                let offset = sub(point, at);
                sqrt(dot(offset, offset)) <= radius
            })
            .count();
        assert_eq!(members(&filled).len(), brute);
    }

    /// Weights fall from 1 to 0 across the last `feather` inside the radius: on an 8-wide map the
    /// texels of row 0 sit 0.125 apart, so from the first texel they are 0, 0.125, 0.25 … away.
    #[test]
    fn a_feather_falls_inside_the_radius() {
        let mesh = plate();
        let at = [0.0625, 0.0, 0.0625];
        let filled = flooded(&mesh, 8, at, 0.1, 0.25, Some(0.25), &grey(8, 8)).expect("seeds");
        let weight = |index: usize| filled.weights[index];
        assert_eq!(filled.seed, 0);
        assert!((weight(0) - 1.0).abs() < 1e-12, "{}", weight(0));
        assert!((weight(1) - 0.5).abs() < 1e-12, "{}", weight(1));
        assert_eq!(weight(3), 0.0, "0.1875 past the seed is outside the radius");
    }

    #[test]
    fn a_seed_outside_its_radius_is_refused() {
        let mesh = plate();
        let refused = flooded(&mesh, 8, [0.0, 0.0, 0.0], 0.1, 0.05, None, &grey(8, 8))
            .expect_err("the nearest texel centre is 0.088 away");
        let FillRefusal::SeedOutsideRadius { distance } = refused else {
            panic!("{refused:?}");
        };
        assert!((distance - sqrt(2.0) * 0.0625).abs() < 1e-12, "{distance}");
    }

    /// A feathered fill weighs `0` at the radius, so a seed exactly on it is refused, not painted
    /// as nothing; the same seed without a feather is a member.
    #[test]
    fn a_feathered_seed_on_the_radius_is_refused() {
        let mesh = plate();
        let seed_at = [0.0, 0.0, 0.0];
        let refused = flooded(&mesh, 8, seed_at, 0.1, 0.05, None, &grey(8, 8)).expect_err("far");
        let FillRefusal::SeedOutsideRadius { distance } = refused else {
            panic!("{refused:?}");
        };

        let sharp = flooded(&mesh, 8, seed_at, 0.1, distance, None, &grey(8, 8)).expect("inside");
        assert_eq!(sharp.weights[sharp.seed], 1.0);
        let feathered = flooded(&mesh, 8, seed_at, 0.1, distance, Some(0.01), &grey(8, 8))
            .expect_err("weight 0 at the radius");
        assert_eq!(feathered, FillRefusal::SeedOutsideRadius { distance });
    }

    #[test]
    fn the_nearest_surface_point_follows_the_triangle_regions() {
        // A right triangle in the XZ plane: (0,0,0), (1,0,0), (0,0,1).
        let positions = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        let nearest = |at| nearest_surface(&positions, &[0, 1, 2], at);

        assert_eq!(
            nearest([0.25, 1.0, 0.25]),
            (1.0, [0.25, 0.0, 0.25]),
            "above the face"
        );
        let (distance, point) = nearest([1.0, 0.0, 1.0]);
        assert!((distance - sqrt(0.5)).abs() < 1e-12, "{distance}");
        assert!(
            (point[0] - 0.5).abs() < 1e-12 && (point[2] - 0.5).abs() < 1e-12,
            "{point:?}"
        );
        assert_eq!(
            nearest([2.0, 0.0, -0.5]),
            (sqrt(1.25), [1.0, 0.0, 0.0]),
            "past a vertex"
        );
        assert_eq!(
            nearest([-1.0, 0.0, 0.5]),
            (1.0, [0.0, 0.0, 0.5]),
            "past an edge"
        );
        assert_eq!(nearest_surface(&[], &[], [0.0; 3]).0, f64::INFINITY);
    }

    /// Two quads mirrored about `at`, the one with the lower triangle indices on the higher texel
    /// indices: the seed is equidistant from a texel of each, and the lower triangle wins.
    #[test]
    fn a_tie_goes_to_the_lower_triangle_then_the_lower_texel() {
        let positions = vec![
            2.0, 0.0, 0.0, 3.0, 0.0, 0.0, 3.0, 0.0, 1.0, 2.0, 0.0, 1.0, //
            0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0,
        ];
        let uvs = vec![
            0.5, 0.0, 1.0, 0.0, 1.0, 1.0, 0.5, 1.0, //
            0.0, 0.0, 0.5, 0.0, 0.5, 1.0, 0.0, 1.0,
        ];
        let mesh = (positions, uvs, vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
        let filled = flooded(
            &mesh,
            16,
            [1.5, 0.0, 0.03125],
            0.1,
            1.0,
            None,
            &grey(16, 16),
        )
        .expect("seeds");
        assert_eq!(
            filled.seed, 8,
            "texel 7 is as near but belongs to triangle 2"
        );

        // Within one triangle it is the lower texel: 0.125 is between texels 0 and 1 of row 0.
        let mesh = plate();
        let filled =
            flooded(&mesh, 8, [0.125, 0.0, 0.0625], 0.1, 1.0, None, &grey(8, 8)).expect("seeds");
        assert_eq!(filled.seed, 0);
    }

    /// A mesh nothing of which the UVs cover has no texel to start from.
    #[test]
    fn an_uncovered_map_has_no_seed() {
        let mesh = (
            vec![0.0; 9],
            vec![0.5, 0.5, 0.5, 0.5, 0.5, 0.5],
            vec![0, 1, 2],
        );
        let refused =
            flooded(&mesh, 4, [0.0; 3], 0.1, 1.0, None, &grey(4, 4)).expect_err("nothing covered");
        assert_eq!(refused, FillRefusal::NoTexels);
    }
}
