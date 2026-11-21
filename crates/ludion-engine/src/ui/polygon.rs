//! Ear-clipping triangulation of a [`UiShape`](crate::components::UiShape) polygon and its even-odd
//! point test (`Plan/ui-hud.md` decisions 69 and 73).
//!
//! Deterministic and pure IEEE arithmetic: no transcendentals, orientation tests in `f64`.

use std::fmt;

/// Why a polygon cannot be filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolygonFault {
    /// Fewer than 3 distinct points.
    TooFew,
    /// The signed area is (numerically) zero.
    ZeroArea,
    /// Edges `a` and `b` (edge `i` runs from point `i` to point `i + 1`) cross or touch.
    SelfIntersecting {
        /// The first edge index.
        a: usize,
        /// The second edge index.
        b: usize,
    },
    /// A lap found no ear: the polygon is not simple.
    NoEar,
}

impl fmt::Display for PolygonFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PolygonFault::TooFew => write!(f, "fewer than 3 distinct points"),
            PolygonFault::ZeroArea => write!(f, "zero area"),
            PolygonFault::SelfIntersecting { a, b } => {
                write!(f, "edges {a} and {b} cross or touch")
            }
            PolygonFault::NoEar => write!(f, "no ear to clip (not a simple polygon)"),
        }
    }
}

type P = [f64; 2];

fn wide(p: [f32; 2]) -> P {
    [f64::from(p[0]), f64::from(p[1])]
}

/// The cross product `(b - a) x (c - a)`.
fn orient(a: P, b: P, c: P) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// `points` without consecutive duplicates, the closing point included if it repeats the first.
pub(crate) fn dedup_points(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut out: Vec<[f32; 2]> = Vec::with_capacity(points.len());
    for &p in points {
        if out.last() != Some(&p) {
            out.push(p);
        }
    }
    while out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    out
}

fn on_segment(a: P, b: P, p: P) -> bool {
    p[0] >= a[0].min(b[0])
        && p[0] <= a[0].max(b[0])
        && p[1] >= a[1].min(b[1])
        && p[1] <= a[1].max(b[1])
}

/// Whether the closed segments `p1p2` and `p3p4` share a point.
fn segments_meet(p1: P, p2: P, p3: P, p4: P) -> bool {
    let d1 = orient(p3, p4, p1);
    let d2 = orient(p3, p4, p2);
    let d3 = orient(p1, p2, p3);
    let d4 = orient(p1, p2, p4);
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    (d1 == 0.0 && on_segment(p3, p4, p1))
        || (d2 == 0.0 && on_segment(p3, p4, p2))
        || (d3 == 0.0 && on_segment(p1, p2, p3))
        || (d4 == 0.0 && on_segment(p1, p2, p4))
}

/// Triangulates a simple polygon (either winding) into triangles of indices into `points` after
/// consecutive duplicates (and a repeated closing point) are dropped; [`dedup_points`] gives that list.
///
/// Ear clipping in point order from a cursor: the first strictly convex corner with no other remaining
/// vertex inside or on its triangle is clipped, then the search continues from the next vertex; a
/// collinear corner is removed without a triangle.
pub(crate) fn triangulate(points: &[[f32; 2]]) -> Result<Vec<[u16; 3]>, PolygonFault> {
    let pts = dedup_points(points);
    let n = pts.len();
    if n < 3 {
        return Err(PolygonFault::TooFew);
    }
    let p: Vec<P> = pts.iter().copied().map(wide).collect();
    let area2: f64 = (0..n)
        .map(|i| {
            let (a, b) = (p[i], p[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    if area2.abs() * 0.5 <= 1e-6 {
        return Err(PolygonFault::ZeroArea);
    }
    for i in 0..n {
        for j in (i + 2)..n {
            if i == 0 && j == n - 1 {
                continue;
            }
            if segments_meet(p[i], p[(i + 1) % n], p[j], p[(j + 1) % n]) {
                return Err(PolygonFault::SelfIntersecting { a: i, b: j });
            }
        }
    }

    let sign = if area2 > 0.0 { 1.0 } else { -1.0 };
    let mut remaining: Vec<usize> = (0..n).collect();
    let mut triangles = Vec::with_capacity(n - 2);
    let mut cursor = 0usize;
    while remaining.len() >= 3 {
        let len = remaining.len();
        let mut clipped = false;
        for step in 0..len {
            let pos = (cursor + step) % len;
            let (ia, ib, ic) = (
                remaining[(pos + len - 1) % len],
                remaining[pos],
                remaining[(pos + 1) % len],
            );
            let cross = orient(p[ia], p[ib], p[ic]) * sign;
            if cross < 0.0 {
                continue;
            }
            if cross > 0.0 {
                let blocked = remaining.iter().any(|&q| {
                    q != ia
                        && q != ib
                        && q != ic
                        && orient(p[ia], p[ib], p[q]) * sign >= 0.0
                        && orient(p[ib], p[ic], p[q]) * sign >= 0.0
                        && orient(p[ic], p[ia], p[q]) * sign >= 0.0
                });
                if blocked {
                    continue;
                }
                triangles.push([ia as u16, ib as u16, ic as u16]);
            }
            remaining.remove(pos);
            cursor = if pos >= remaining.len() { 0 } else { pos };
            clipped = true;
            break;
        }
        if !clipped {
            return Err(PolygonFault::NoEar);
        }
    }
    Ok(triangles)
}

/// Whether `point` is inside `polygon` by the even-odd rule. `polygon` is the deduplicated list; fewer
/// than 3 points contain nothing.
pub(crate) fn polygon_contains(polygon: &[[f32; 2]], point: [f32; 2]) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    let p = wide(point);
    let mut inside = false;
    let mut prev = wide(polygon[polygon.len() - 1]);
    for &q in polygon {
        let q = wide(q);
        if (q[1] > p[1]) != (prev[1] > p[1])
            && p[0] < (prev[0] - q[0]) * (p[1] - q[1]) / (prev[1] - q[1]) + q[0]
        {
            inside = !inside;
        }
        prev = q;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(points: &[[f32; 2]], t: [u16; 3]) -> f64 {
        let [a, b, c] = t.map(|i| wide(points[usize::from(i)]));
        orient(a, b, c).abs() * 0.5
    }

    fn signed_area(points: &[[f32; 2]]) -> f64 {
        let p: Vec<P> = points.iter().copied().map(wide).collect();
        (0..p.len())
            .map(|i| {
                let (a, b) = (p[i], p[(i + 1) % p.len()]);
                a[0] * b[1] - b[0] * a[1]
            })
            .sum::<f64>()
            * 0.5
    }

    const SQUARE: [[f32; 2]; 4] = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];

    fn star() -> Vec<[f32; 2]> {
        (0..10)
            .map(|i| {
                let angle = i as f64 * std::f64::consts::PI / 5.0;
                let r = if i % 2 == 0 { 10.0 } else { 4.0 };
                [
                    (r * ludion_core::math::f64::sin(angle)) as f32,
                    (-r * ludion_core::math::f64::cos(angle)) as f32,
                ]
            })
            .collect()
    }

    #[test]
    fn a_square_is_two_triangles_in_either_winding() {
        let cw = triangulate(&SQUARE).unwrap();
        assert_eq!(cw.len(), 2);
        let mut ccw = SQUARE;
        ccw.reverse();
        assert_eq!(triangulate(&ccw).unwrap().len(), 2);
    }

    #[test]
    fn a_concave_star_covers_its_area() {
        let star = star();
        let triangles = triangulate(&star).unwrap();
        assert_eq!(triangles.len(), 8);
        let total: f64 = triangles.iter().map(|t| area(&star, *t)).sum();
        assert!((total - signed_area(&star).abs()).abs() < 1e-3, "{total}");
    }

    #[test]
    fn a_bow_tie_is_self_intersecting() {
        let bow = [[0.0, 0.0], [10.0, 10.0], [10.0, 0.0], [0.0, 12.0]];
        assert!(matches!(
            triangulate(&bow),
            Err(PolygonFault::SelfIntersecting { .. })
        ));
    }

    #[test]
    fn three_collinear_points_have_no_area() {
        assert_eq!(
            triangulate(&[[0.0, 0.0], [1.0, 1.0], [2.0, 2.0]]),
            Err(PolygonFault::ZeroArea)
        );
    }

    #[test]
    fn two_distinct_points_are_too_few() {
        assert_eq!(
            triangulate(&[[1.0, 1.0], [1.0, 1.0], [4.0, 0.0]]),
            Err(PolygonFault::TooFew)
        );
        assert_eq!(triangulate(&[]), Err(PolygonFault::TooFew));
    }

    #[test]
    fn a_repeated_closing_point_is_dropped() {
        let mut closed = SQUARE.to_vec();
        closed.push(SQUARE[0]);
        assert_eq!(dedup_points(&closed), SQUARE.to_vec());
        assert_eq!(triangulate(&closed).unwrap(), triangulate(&SQUARE).unwrap());
    }

    #[test]
    fn a_256_point_comb_triangulates() {
        let mut comb = vec![[0.0f32, 0.0], [1000.0, 0.0]];
        for k in 0..254 {
            comb.push([1000.0 - 3.9 * (k + 1) as f32, 10.0 + (k % 2) as f32 * 5.0]);
        }
        assert_eq!(comb.len(), 256);
        let triangles = triangulate(&comb).unwrap();
        assert_eq!(triangles.len(), 254);
        let total: f64 = triangles.iter().map(|t| area(&comb, *t)).sum();
        assert!((total - signed_area(&comb).abs()).abs() < 1e-1, "{total}");
    }

    #[test]
    fn the_same_input_gives_the_same_indices() {
        let star = star();
        assert_eq!(triangulate(&star), triangulate(&star));
    }

    #[test]
    fn a_notch_inside_the_hull_is_outside_the_polygon() {
        // A "U": the notch between the arms is inside the convex hull.
        let u = [
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 20.0],
            [7.0, 20.0],
            [7.0, 5.0],
            [3.0, 5.0],
            [3.0, 20.0],
            [0.0, 20.0],
        ];
        assert!(polygon_contains(&u, [1.0, 10.0]));
        assert!(polygon_contains(&u, [5.0, 2.0]));
        assert!(!polygon_contains(&u, [5.0, 10.0]));
        assert!(!polygon_contains(&u, [-1.0, 10.0]));
        assert!(triangulate(&u).is_ok());
    }
}
