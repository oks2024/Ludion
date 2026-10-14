//! The signed distance to a triangle soup: closest-triangle distance for the magnitude, the
//! generalised winding number for the sign.
//!
//! A `shape: mesh` part's field is an imported mesh document's own triangles, and a generated one is
//! not a clean solid — it can be open, self-intersecting, or carry an inverted inner shell. The
//! closest-triangle distance is defined for any such soup; the sign is not, unless it is asked for
//! as a winding number. The generalised winding number `w(q)` is the sum of the signed solid angles
//! the triangles subtend at `q`, over `4π`: exactly `1` inside a closed surface and `0` outside it,
//! and a continuous number in between that stays meaningful where a ray-parity or
//! closest-triangle-normal test gives up. `w > 1/2` is inside.
//!
//! Both queries run against one BVH over the triangles, built in a fixed order. The winding number
//! uses it hierarchically (Barill et al., *Fast Winding Numbers for Soups and Clouds*): a node far
//! enough from the query contributes its aggregate dipole — one area-weighted normal sum at one
//! area-weighted centre — instead of its triangles, so only the triangles near the query are summed
//! one at a time. Every traversal order and every accumulation order is fixed by the tree, so the
//! result is a pure function of the soup's bytes.

use ludion_core::math;

/// The most triangles a BVH leaf holds. Small enough that the near-field of a query touches few
/// triangles, large enough that the tree stays shallow.
const LEAF_TRIANGLES: usize = 8;

/// How far a query must sit from a node's aggregate centre, in multiples of that node's radius,
/// before the node answers from its dipole instead of its triangles.
///
/// The dipole is the first term of the expansion, so its error falls off as `(radius / distance)²`;
/// at this distance it is below `10⁻²` of a winding number, far inside the margin the `1/2`
/// inside test needs — and every query close enough for that margin to matter recurses to exact
/// solid angles anyway.
const APPROXIMATION_DISTANCE: f32 = 2.5;

const FOUR_PI: f32 = 12.566_371;

/// One BVH node: its box, its triangles, and the winding-number aggregate of its whole subtree.
struct Node {
    /// The box enclosing every vertex of the subtree's triangles.
    min: [f32; 3],
    max: [f32; 3],
    /// The subtree's range in [`TriangleSoup::order`].
    start: u32,
    /// Triangles in the range when this is a leaf, `0` for an inner node.
    count: u32,
    /// Child node indices; meaningless on a leaf.
    children: [u32; 2],
    /// `Σ area · normal` over the subtree — the dipole moment, and zero over a closed surface.
    dipole: [f32; 3],
    /// The area-weighted centroid the dipole is measured at.
    centre: [f32; 3],
    /// The largest distance from `centre` to any vertex of the subtree.
    radius: f32,
}

/// A mesh document's triangles, indexed for distance and winding-number queries.
pub(crate) struct TriangleSoup {
    vertices: Vec<[f32; 3]>,
    /// Vertex index triples in the document's own triangle order.
    triangles: Vec<[u32; 3]>,
    /// Triangle indices in BVH leaf order — the document order permuted, never changed.
    order: Vec<u32>,
    /// `nodes[0]` is the root.
    nodes: Vec<Node>,
}

impl TriangleSoup {
    /// The soup of a validated mesh document's flat `positions` and `indices`, or `None` when it
    /// holds no triangle at all.
    pub(crate) fn new(positions: &[f64], indices: &[u32]) -> Option<TriangleSoup> {
        if indices.len() < 3 {
            return None;
        }
        let vertices: Vec<[f32; 3]> = positions
            .chunks_exact(3)
            .map(|p| [p[0] as f32, p[1] as f32, p[2] as f32])
            .collect();
        let triangles: Vec<[u32; 3]> = indices
            .chunks_exact(3)
            .map(|t| [t[0], t[1], t[2]])
            .collect();
        if triangles.is_empty() {
            return None;
        }
        let centroids: Vec<[f32; 3]> = triangles
            .iter()
            .map(|t| {
                let [a, b, c] = corners(&vertices, *t);
                [0, 1, 2].map(|axis| (a[axis] + b[axis] + c[axis]) / 3.0)
            })
            .collect();

        let mut soup = TriangleSoup {
            vertices,
            order: (0..triangles.len() as u32).collect(),
            triangles,
            nodes: Vec::new(),
        };
        soup.build(0, soup.order.len(), &centroids);
        Some(soup)
    }

    /// The box enclosing every vertex — the root node's, so a soup always has one.
    pub(crate) fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        (self.nodes[0].min, self.nodes[0].max)
    }

    /// The signed distance from `q`: the distance to the nearest triangle, negated where the
    /// generalised winding number says `q` is inside.
    pub(crate) fn signed_distance(&self, q: [f32; 3]) -> f32 {
        let mut best = (f32::INFINITY, u32::MAX);
        self.closest(0, q, &mut best);
        let distance = math::sqrt(best.0);
        if self.winding_number(q) > 0.5 {
            -distance
        } else {
            distance
        }
    }

    /// Builds the node covering `order[start..start + count]`, returning its index.
    ///
    /// The split is the median of the range's triangle centroids along the longest axis of their
    /// box, the range sorted by that coordinate with a stable sort — so equal centroids keep the
    /// document's own order and the tree is a pure function of the soup.
    fn build(&mut self, start: usize, count: usize, centroids: &[[f32; 3]]) -> u32 {
        let index = self.nodes.len() as u32;
        self.nodes.push(self.leaf(start, count));
        if count <= LEAF_TRIANGLES {
            return index;
        }

        let axis = widest_axis(&self.order[start..start + count], centroids);
        self.order[start..start + count]
            .sort_by(|a, b| centroids[*a as usize][axis].total_cmp(&centroids[*b as usize][axis]));
        let half = count / 2;
        let left = self.build(start, half, centroids);
        let right = self.build(start + half, count - half, centroids);
        let node = &mut self.nodes[index as usize];
        node.count = 0;
        node.children = [left, right];
        index
    }

    /// A leaf over `order[start..start + count]`: its box and its winding-number aggregate, both of
    /// which an inner node keeps unchanged over the same range.
    fn leaf(&self, start: usize, count: usize) -> Node {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        let mut dipole = [0.0f32; 3];
        let mut weighted = [0.0f32; 3];
        let mut area_total = 0.0f32;
        for &triangle in &self.order[start..start + count] {
            let [a, b, c] = corners(&self.vertices, self.triangles[triangle as usize]);
            for corner in [a, b, c] {
                for axis in 0..3 {
                    min[axis] = min[axis].min(corner[axis]);
                    max[axis] = max[axis].max(corner[axis]);
                }
            }
            // `½ (b - a) × (c - a)` is the area times the outward normal in one product.
            let moment = scale(cross(sub(b, a), sub(c, a)), 0.5);
            dipole = add(dipole, moment);
            let area = length(moment);
            area_total += area;
            for axis in 0..3 {
                weighted[axis] += area * (a[axis] + b[axis] + c[axis]) / 3.0;
            }
        }
        // A range of degenerate triangles weighs nothing; its box centre is the only centre it has.
        let centre = if area_total > 0.0 {
            weighted.map(|w| w / area_total)
        } else {
            [0, 1, 2].map(|axis| 0.5 * (min[axis] + max[axis]))
        };
        let mut radius = 0.0f32;
        for &triangle in &self.order[start..start + count] {
            for corner in corners(&self.vertices, self.triangles[triangle as usize]) {
                radius = radius.max(length(sub(corner, centre)));
            }
        }
        Node {
            min,
            max,
            start: start as u32,
            count: count as u32,
            children: [0; 2],
            dipole,
            centre,
            radius,
        }
    }

    /// Branch and bound for the nearest triangle: the nearer child first, a child whose box is
    /// already farther than the running best skipped. `best` is `(squared distance, triangle)`,
    /// ties to the lower triangle index so the pair is one answer whatever order the tree is walked
    /// in.
    fn closest(&self, index: u32, q: [f32; 3], best: &mut (f32, u32)) {
        let node = &self.nodes[index as usize];
        if node.count > 0 {
            for &triangle in &self.order[node.start as usize..(node.start + node.count) as usize] {
                let [a, b, c] = corners(&self.vertices, self.triangles[triangle as usize]);
                let distance = triangle_distance_sq(q, a, b, c);
                if distance < best.0 || (distance == best.0 && triangle < best.1) {
                    *best = (distance, triangle);
                }
            }
            return;
        }
        let [left, right] = node.children;
        let near = |child: u32| {
            let child = &self.nodes[child as usize];
            box_distance_sq(child.min, child.max, q)
        };
        let (mut first, mut second) = ((left, near(left)), (right, near(right)));
        if second.1 < first.1 {
            std::mem::swap(&mut first, &mut second);
        }
        for (child, distance) in [first, second] {
            if distance < best.0 {
                self.closest(child, q, best);
            }
        }
    }

    /// The generalised winding number at `q`: `1` inside a closed surface, `0` outside it.
    fn winding_number(&self, q: [f32; 3]) -> f32 {
        self.winding_of(0, q) / FOUR_PI
    }

    /// The subtree's total signed solid angle at `q` — from its dipole when `q` is far enough from
    /// its centre, otherwise from its children, and from its own triangles at a leaf.
    fn winding_of(&self, index: u32, q: [f32; 3]) -> f32 {
        let node = &self.nodes[index as usize];
        let offset = sub(node.centre, q);
        let distance_sq = dot(offset, offset);
        let reach = APPROXIMATION_DISTANCE * node.radius;
        if distance_sq > reach * reach {
            let distance = math::sqrt(distance_sq);
            return dot(node.dipole, offset) / (distance * distance * distance);
        }
        if node.count > 0 {
            let mut total = 0.0;
            for &triangle in &self.order[node.start as usize..(node.start + node.count) as usize] {
                let [a, b, c] = corners(&self.vertices, self.triangles[triangle as usize]);
                total += solid_angle(sub(a, q), sub(b, q), sub(c, q));
            }
            return total;
        }
        self.winding_of(node.children[0], q) + self.winding_of(node.children[1], q)
    }
}

/// The signed solid angle a triangle whose corners are `a`, `b`, `c` *relative to the query point*
/// subtends there, by the Van Oosterom–Strackee formula.
///
/// Positive on the side the triangle's winding turns away from — so a closed, outward-wound surface
/// sums to `4π` at an interior point. A query exactly on a corner gives `atan2(0, 0)`, which is `0`:
/// the surface is its own boundary case, and the mesher reads the sign of a lattice corner, never of
/// a point it placed on the surface.
fn solid_angle(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f32 {
    let (la, lb, lc) = (length(a), length(b), length(c));
    let numerator = dot(a, cross(b, c));
    let denominator = la * lb * lc + dot(a, b) * lc + dot(b, c) * la + dot(c, a) * lb;
    2.0 * math::atan2(numerator, denominator)
}

/// The squared distance from `p` to the triangle `a`, `b`, `c` — Ericson's closest-point-on-triangle
/// region test, which never divides by a quantity its own guard has not made positive. A fully
/// degenerate triangle (three collinear corners) falls through to its nearest edge or corner.
fn triangle_distance_sq(p: [f32; 3], a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f32 {
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return distance_sq(p, a);
    }
    let bp = sub(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return distance_sq(p, b);
    }
    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return distance_sq(p, c);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 && d1 > d3 {
        return distance_sq(p, add(a, scale(ab, d1 / (d1 - d3))));
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 && d2 > d6 {
        return distance_sq(p, add(a, scale(ac, d2 / (d2 - d6))));
    }
    let va = d3 * d6 - d5 * d4;
    let (numerator, denominator) = (d4 - d3, (d4 - d3) + (d5 - d6));
    if va <= 0.0 && numerator >= 0.0 && d5 - d6 >= 0.0 && denominator > 0.0 {
        return distance_sq(p, add(b, scale(sub(c, b), numerator / denominator)));
    }
    let total = va + vb + vc;
    if total <= 0.0 {
        return distance_sq(p, a)
            .min(distance_sq(p, b))
            .min(distance_sq(p, c));
    }
    let v = vb / total;
    let w = vc / total;
    distance_sq(p, add(add(a, scale(ab, v)), scale(ac, w)))
}

/// The squared distance from `q` to the axis-aligned box, `0` inside it.
fn box_distance_sq(min: [f32; 3], max: [f32; 3], q: [f32; 3]) -> f32 {
    let mut total = 0.0;
    for axis in 0..3 {
        let outside = (min[axis] - q[axis]).max(q[axis] - max[axis]).max(0.0);
        total += outside * outside;
    }
    total
}

/// The axis the range's centroids spread widest along; ties to the lower axis.
fn widest_axis(range: &[u32], centroids: &[[f32; 3]]) -> usize {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for &triangle in range {
        let centroid = centroids[triangle as usize];
        for axis in 0..3 {
            min[axis] = min[axis].min(centroid[axis]);
            max[axis] = max[axis].max(centroid[axis]);
        }
    }
    let spread = [0, 1, 2].map(|axis| max[axis] - min[axis]);
    if spread[0] >= spread[1] && spread[0] >= spread[2] {
        0
    } else if spread[1] >= spread[2] {
        1
    } else {
        2
    }
}

/// The three corner positions of a triangle. The document's indices are range-checked by
/// `ludion_scene::check_mesh`, so the lookup is total on a validated document.
fn corners(vertices: &[[f32; 3]], triangle: [u32; 3]) -> [[f32; 3]; 3] {
    triangle.map(|index| vertices[index as usize])
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length(a: [f32; 3]) -> f32 {
    math::sqrt(dot(a, a))
}

fn distance_sq(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = sub(a, b);
    dot(d, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit cube centred on the origin, 12 triangles wound outward.
    fn cube() -> (Vec<f64>, Vec<u32>) {
        let positions = vec![
            -0.5, -0.5, -0.5, 0.5, -0.5, -0.5, 0.5, 0.5, -0.5, -0.5, 0.5, -0.5, -0.5, -0.5, 0.5,
            0.5, -0.5, 0.5, 0.5, 0.5, 0.5, -0.5, 0.5, 0.5,
        ];
        let indices = vec![
            // -z, +z, -y, +y, -x, +x, each wound counter-clockwise seen from outside
            0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 3, 7, 6, 3, 6, 2, 0, 4, 7, 0, 7,
            3, 1, 2, 6, 1, 6, 5,
        ];
        (positions, indices)
    }

    /// An icosphere of `radius`, subdivided `steps` times — a closed surface with enough triangles
    /// to exercise the tree rather than a single leaf.
    fn icosphere(radius: f32, steps: usize) -> (Vec<f64>, Vec<u32>) {
        let t = (1.0 + 5.0f32.sqrt()) / 2.0;
        let mut vertices: Vec<[f32; 3]> = vec![
            [-1.0, t, 0.0],
            [1.0, t, 0.0],
            [-1.0, -t, 0.0],
            [1.0, -t, 0.0],
            [0.0, -1.0, t],
            [0.0, 1.0, t],
            [0.0, -1.0, -t],
            [0.0, 1.0, -t],
            [t, 0.0, -1.0],
            [t, 0.0, 1.0],
            [-t, 0.0, -1.0],
            [-t, 0.0, 1.0],
        ];
        let mut faces: Vec<[u32; 3]> = vec![
            [0, 11, 5],
            [0, 5, 1],
            [0, 1, 7],
            [0, 7, 10],
            [0, 10, 11],
            [1, 5, 9],
            [5, 11, 4],
            [11, 10, 2],
            [10, 7, 6],
            [7, 1, 8],
            [3, 9, 4],
            [3, 4, 2],
            [3, 2, 6],
            [3, 6, 8],
            [3, 8, 9],
            [4, 9, 5],
            [2, 4, 11],
            [6, 2, 10],
            [8, 6, 7],
            [9, 8, 1],
        ];
        for _ in 0..steps {
            let mut split = Vec::with_capacity(faces.len() * 4);
            for face in &faces {
                let mid = |a: u32, b: u32, vertices: &mut Vec<[f32; 3]>| {
                    let (p, q) = (vertices[a as usize], vertices[b as usize]);
                    vertices.push([0, 1, 2].map(|axis| 0.5 * (p[axis] + q[axis])));
                    vertices.len() as u32 - 1
                };
                let ab = mid(face[0], face[1], &mut vertices);
                let bc = mid(face[1], face[2], &mut vertices);
                let ca = mid(face[2], face[0], &mut vertices);
                split.push([face[0], ab, ca]);
                split.push([face[1], bc, ab]);
                split.push([face[2], ca, bc]);
                split.push([ab, bc, ca]);
            }
            faces = split;
        }
        let positions = vertices
            .iter()
            .flat_map(|v| {
                let scale = radius / length(*v);
                [v[0] * scale, v[1] * scale, v[2] * scale].map(|c| c as f64)
            })
            .collect();
        let indices = faces.iter().flat_map(|f| f.iter().copied()).collect();
        (positions, indices)
    }

    #[test]
    fn a_cube_reads_its_analytic_distance_inside_and_out() {
        let (positions, indices) = cube();
        let soup = TriangleSoup::new(&positions, &indices).expect("12 triangles");
        assert_eq!(soup.bounds(), ([-0.5; 3], [0.5; 3]));
        for (point, expected) in [
            ([0.0, 0.0, 0.0], -0.5),
            ([0.25, 0.0, 0.0], -0.25),
            ([1.5, 0.0, 0.0], 1.0),
            ([0.0, -2.0, 0.0], 1.5),
            ([0.5, 0.5, 0.5], 0.0),
        ] {
            let distance = soup.signed_distance(point);
            assert!(
                (distance - expected).abs() <= 1e-5,
                "{point:?}: {distance} is not {expected}"
            );
        }
    }

    /// The hierarchical winding number must agree with the direct sum over every triangle — that
    /// equality is the whole claim the dipole approximation makes.
    #[test]
    fn the_hierarchical_winding_number_matches_the_direct_sum() {
        let (positions, indices) = icosphere(0.7, 2);
        let soup = TriangleSoup::new(&positions, &indices).expect("a sphere");
        assert!(soup.triangles.len() >= 300, "{}", soup.triangles.len());

        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..400 {
            let mut next = || {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (state >> 40) as f32 / 16_777_216.0 * 3.0 - 1.5
            };
            let q = [next(), next(), next()];
            let direct: f32 = soup
                .triangles
                .iter()
                .map(|t| {
                    let [a, b, c] = corners(&soup.vertices, *t);
                    solid_angle(sub(a, q), sub(b, q), sub(c, q))
                })
                .sum::<f32>()
                / FOUR_PI;
            let hierarchical = soup.winding_number(q);
            // The bound [`APPROXIMATION_DISTANCE`] claims. Measured worst case over these samples
            // is about 3.4e-3, and the sign test below has a margin of 1/2.
            assert!(
                (direct - hierarchical).abs() <= 1e-2,
                "{q:?}: {hierarchical} is not {direct}"
            );
            assert_eq!(
                direct > 0.5,
                hierarchical > 0.5,
                "{q:?}: the inside test disagrees ({direct} vs {hierarchical})"
            );
        }
    }

    /// The sign survives an open surface: a sphere with one cap cut away still reads inside near its
    /// centre, which is exactly what a closest-triangle normal test cannot do.
    #[test]
    fn an_open_surface_still_has_an_inside() {
        let (positions, indices) = icosphere(0.7, 2);
        let kept: Vec<u32> = indices
            .chunks_exact(3)
            .filter(|t| t.iter().all(|&i| positions[i as usize * 3 + 1] < 0.45))
            .flat_map(|t| t.iter().copied())
            .collect();
        assert!(kept.len() < indices.len(), "the cap must be gone");
        let soup = TriangleSoup::new(&positions, &kept).expect("an open shell");
        assert!(soup.signed_distance([0.0, 0.0, 0.0]) < 0.0);
        assert!(soup.signed_distance([0.0, -0.3, 0.0]) < 0.0);
        assert!(soup.signed_distance([0.0, -2.0, 0.0]) > 0.0);
        assert!(soup.signed_distance([2.0, 0.0, 0.0]) > 0.0);
    }

    /// The closest-triangle search is branch and bound over the tree, so it must return exactly what
    /// the exhaustive scan does — never merely close.
    #[test]
    fn the_tree_finds_the_same_triangle_as_an_exhaustive_scan() {
        let (positions, indices) = icosphere(0.5, 2);
        let soup = TriangleSoup::new(&positions, &indices).expect("a sphere");
        let mut state = 0x243F_6A88_85A3_08D3u64;
        for _ in 0..200 {
            let mut next = || {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (state >> 40) as f32 / 16_777_216.0 * 2.0 - 1.0
            };
            let q = [next(), next(), next()];
            let mut best = (f32::INFINITY, u32::MAX);
            soup.closest(0, q, &mut best);
            let mut brute = (f32::INFINITY, u32::MAX);
            for (index, triangle) in soup.triangles.iter().enumerate() {
                let [a, b, c] = corners(&soup.vertices, *triangle);
                let distance = triangle_distance_sq(q, a, b, c);
                let index = index as u32;
                if distance < brute.0 || (distance == brute.0 && index < brute.1) {
                    brute = (distance, index);
                }
            }
            assert_eq!(best, brute, "{q:?}");
        }
    }
}
