//! Blend-space weights: how a 1D or 2D parameter weighs a blend's point clips
//! (`Plan/animation-layers.md` decisions 13 and 14).
//!
//! A blend is a set of points, each a clip at a position. The 1D weights are linear between the
//! sorted neighbours and clamp at the ends; the 2D weights are barycentric over a deterministic
//! triangulation of the points, clamped to the hull. Everything is plain `f64` arithmetic with no
//! transcendentals, so the weights are bit-identical on every platform.
//!
//! ```
//! use ludion_scene::{blend_weights_1d, blend_weights_2d, triangulate};
//!
//! let weights = blend_weights_1d(&[0.0, 2.0, 4.0], 1.0);
//! assert_eq!(weights.iter().collect::<Vec<_>>(), vec![(0, 0.5), (1, 0.5)]);
//!
//! let points = [[0.0, 0.0], [2.0, 0.0], [0.0, 2.0]];
//! let triangles = triangulate(&points);
//! assert_eq!(triangles, vec![[0, 1, 2]]);
//! assert_eq!(blend_weights_2d(&points, &triangles, [2.0, 0.0]).dominant(), 1);
//! ```

/// The most points one blend may have.
pub const MAX_BLEND_POINTS: usize = 16;

/// Up to three point indices with their weights; zero weights are dropped and the rest sum to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlendWeights {
    entries: [(u16, f64); 3],
    len: u8,
}

impl BlendWeights {
    fn one(index: u16) -> Self {
        Self {
            entries: [(index, 1.0), (0, 0.0), (0, 0.0)],
            len: 1,
        }
    }

    fn from_pairs(pairs: &[(u16, f64)]) -> Self {
        let mut out = Self {
            entries: [(0, 0.0); 3],
            len: 0,
        };
        for &(index, weight) in pairs {
            if weight > 0.0 {
                out.entries[usize::from(out.len)] = (index, weight);
                out.len += 1;
            }
        }
        out
    }

    /// The `(point index, weight)` entries, in a fixed order.
    pub fn iter(&self) -> impl Iterator<Item = (u16, f64)> + '_ {
        self.entries[..usize::from(self.len)].iter().copied()
    }

    /// The index with the largest weight, ties to the lower index. 0 when empty.
    #[must_use]
    pub fn dominant(&self) -> u16 {
        let mut best: Option<(u16, f64)> = None;
        for (index, weight) in self.iter() {
            best = match best {
                Some((bi, bw)) if bw > weight || (bw == weight && bi < index) => Some((bi, bw)),
                _ => Some((index, weight)),
            };
        }
        best.map_or(0, |(index, _)| index)
    }
}

/// Weights for a 1D blend. `ats` is the points' positions in point order; `x` clamps to their
/// range and interpolates linearly between the sorted neighbours.
///
/// # Panics
/// If `ats` is empty.
#[must_use]
pub fn blend_weights_1d(ats: &[f64], x: f64) -> BlendWeights {
    let mut order: Vec<u16> = (0..ats.len() as u16).collect();
    order.sort_by(|&a, &b| {
        ats[usize::from(a)]
            .total_cmp(&ats[usize::from(b)])
            .then(a.cmp(&b))
    });
    let first = order[0];
    let last = order[order.len() - 1];
    if x.is_nan() || x <= ats[usize::from(first)] {
        return BlendWeights::one(first);
    }
    if x >= ats[usize::from(last)] {
        return BlendWeights::one(last);
    }
    for pair in order.windows(2) {
        let (lo, hi) = (pair[0], pair[1]);
        let (a, b) = (ats[usize::from(lo)], ats[usize::from(hi)]);
        if x >= a && x < b {
            let t = (x - a) / (b - a);
            return BlendWeights::from_pairs(&[(lo, 1.0 - t), (hi, t)]);
        }
    }
    BlendWeights::one(last)
}

fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Positive when `d` lies strictly inside the circumcircle of the counter-clockwise `a, b, c`.
fn in_circle(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> f64 {
    let (ax, ay) = (a[0] - d[0], a[1] - d[1]);
    let (bx, by) = (b[0] - d[0], b[1] - d[1]);
    let (cx, cy) = (c[0] - d[0], c[1] - d[1]);
    (ax * ax + ay * ay) * (bx * cy - cx * by) - (bx * bx + by * by) * (ax * cy - cx * ay)
        + (cx * cx + cy * cy) * (ax * by - bx * ay)
}

/// Whether the interiors of two triangles intersect (touching edges and corners do not count).
fn overlaps(points: &[[f64; 2]], t1: [u16; 3], t2: [u16; 3]) -> bool {
    let at = |t: [u16; 3]| t.map(|i| points[usize::from(i)]);
    let (p, q) = (at(t1), at(t2));
    let separates = |edges: &[[f64; 2]; 3], others: &[[f64; 2]; 3]| {
        let sign = if orient(edges[0], edges[1], edges[2]) > 0.0 {
            1.0
        } else {
            -1.0
        };
        (0..3).any(|k| {
            let (a, b) = (edges[k], edges[(k + 1) % 3]);
            others.iter().all(|&o| sign * orient(a, b, o) <= 0.0)
        })
    };
    !(separates(&p, &q) || separates(&q, &p))
}

/// The deterministic triangulation of `points` (decision 14): every index triple `a < b < c` that
/// is non-degenerate and has no point strictly inside its circumcircle, taken in lexicographic
/// order and skipped when it overlaps one already taken. Collinear points give no triangles.
#[must_use]
pub fn triangulate(points: &[[f64; 2]]) -> Vec<[u16; 3]> {
    let n = points.len();
    let mut accepted: Vec<[u16; 3]> = Vec::new();
    for a in 0..n {
        for b in a + 1..n {
            for c in b + 1..n {
                let (pa, pb, pc) = (points[a], points[b], points[c]);
                let area = orient(pa, pb, pc);
                if area == 0.0 {
                    continue;
                }
                let (qb, qc) = if area > 0.0 { (pb, pc) } else { (pc, pb) };
                let empty = (0..n)
                    .filter(|&d| d != a && d != b && d != c)
                    .all(|d| in_circle(pa, qb, qc, points[d]) <= 0.0);
                if !empty {
                    continue;
                }
                let triple = [a as u16, b as u16, c as u16];
                if accepted.iter().all(|&t| !overlaps(points, t, triple)) {
                    accepted.push(triple);
                }
            }
        }
    }
    accepted
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

/// The closest point of triangle `abc` to `p` as barycentric weights, and its squared distance.
fn closest_in_triangle(a: [f64; 2], b: [f64; 2], c: [f64; 2], p: [f64; 2]) -> ([f64; 3], f64) {
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    let at = |w: [f64; 3]| {
        let q = [
            w[0] * a[0] + w[1] * b[0] + w[2] * c[0],
            w[0] * a[1] + w[1] * b[1] + w[2] * c[1],
        ];
        let d = sub(p, q);
        (w, dot(d, d))
    };
    if d1 <= 0.0 && d2 <= 0.0 {
        return at([1.0, 0.0, 0.0]);
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return at([0.0, 1.0, 0.0]);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return at([1.0 - v, v, 0.0]);
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return at([0.0, 0.0, 1.0]);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return at([1.0 - w, 0.0, w]);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return at([0.0, 1.0 - w, w]);
    }
    let denom = 1.0 / (va + vb + vc);
    let (v, w) = (vb * denom, vc * denom);
    at([1.0 - v - w, v, w])
}

/// Weights for a 2D blend: barycentric in the triangle whose closest point to `p` is nearest (ties
/// to the first), which clamps a query outside the hull. With no triangles, the nearest point.
///
/// # Panics
/// If `points` is empty.
#[must_use]
pub fn blend_weights_2d(points: &[[f64; 2]], triangles: &[[u16; 3]], p: [f64; 2]) -> BlendWeights {
    let mut best: Option<(&[u16; 3], [f64; 3], f64)> = None;
    for triangle in triangles {
        let [a, b, c] = triangle.map(|i| points[usize::from(i)]);
        let (w, d2) = closest_in_triangle(a, b, c, p);
        if best.is_none_or(|(_, _, bd)| d2 < bd) {
            best = Some((triangle, w, d2));
        }
    }
    match best {
        Some((triangle, w, _)) => BlendWeights::from_pairs(&[
            (triangle[0], w[0]),
            (triangle[1], w[1]),
            (triangle[2], w[2]),
        ]),
        None => {
            let mut nearest = 0usize;
            let mut nd = f64::INFINITY;
            for (i, q) in points.iter().enumerate() {
                let d = sub(p, *q);
                let d2 = dot(d, d);
                if d2 < nd {
                    (nearest, nd) = (i, d2);
                }
            }
            BlendWeights::one(nearest as u16)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CROSS: [[f64; 2]; 5] = [[0.0, 0.0], [2.0, 0.0], [0.0, 2.0], [-2.0, 0.0], [0.0, -2.0]];

    fn pairs(w: &BlendWeights) -> Vec<(u16, f64)> {
        w.iter().collect()
    }

    #[test]
    fn one_d_ends_clamp_and_midpoint() {
        let ats = [4.0, 0.0, 2.0];
        assert_eq!(pairs(&blend_weights_1d(&ats, 0.0)), vec![(1, 1.0)]);
        assert_eq!(pairs(&blend_weights_1d(&ats, -3.0)), vec![(1, 1.0)]);
        assert_eq!(pairs(&blend_weights_1d(&ats, 4.0)), vec![(0, 1.0)]);
        assert_eq!(pairs(&blend_weights_1d(&ats, 9.0)), vec![(0, 1.0)]);
        assert_eq!(pairs(&blend_weights_1d(&ats, 2.0)), vec![(2, 1.0)]);
        assert_eq!(
            pairs(&blend_weights_1d(&ats, 1.0)),
            vec![(1, 0.5), (2, 0.5)]
        );
        assert_eq!(blend_weights_1d(&ats, 3.0).dominant(), 0);
    }

    #[test]
    fn dominant_ties_to_the_lower_index() {
        let w = blend_weights_1d(&[0.0, 2.0], 1.0);
        assert_eq!(w.dominant(), 0);
    }

    #[test]
    fn cross_triangulation_is_four_centre_fans_and_repeatable() {
        let first = triangulate(&CROSS);
        assert_eq!(first, vec![[0, 1, 2], [0, 1, 4], [0, 2, 3], [0, 3, 4]]);
        assert_eq!(first, triangulate(&CROSS));
    }

    #[test]
    fn collinear_points_give_no_triangles() {
        assert!(triangulate(&[[0.0, 0.0], [1.0, 1.0], [2.0, 2.0], [3.0, 3.0]]).is_empty());
    }

    #[test]
    fn cross_weights() {
        let tris = triangulate(&CROSS);
        assert_eq!(
            pairs(&blend_weights_2d(&CROSS, &tris, [0.0, 0.0])),
            vec![(0, 1.0)]
        );
        assert_eq!(
            pairs(&blend_weights_2d(&CROSS, &tris, [2.0, 0.0])),
            vec![(1, 1.0)]
        );
        let edge = blend_weights_2d(&CROSS, &tris, [1.0, 0.0]);
        assert_eq!(pairs(&edge), vec![(0, 0.5), (1, 0.5)]);
        let outside = blend_weights_2d(&CROSS, &tris, [9.0, 0.0]);
        assert_eq!(pairs(&outside), vec![(1, 1.0)]);
        for p in [
            [0.3, 0.4],
            [-0.7, 1.1],
            [1.5, -0.2],
            [5.0, 5.0],
            [0.0, -1.0],
        ] {
            let w = blend_weights_2d(&CROSS, &tris, p);
            let sum: f64 = w.iter().map(|(_, w)| w).sum();
            assert!((sum - 1.0).abs() < 1e-12, "{p:?}: {sum}");
        }
    }
}
