//! Region algebra: the signed distance of a [`MeshEditRegionShape`], the weight its `feather` turns
//! that distance into, and the world-space box the pair occupies.
//!
//! The primitives are the sculpt part vocabulary read in the source mesh's own coordinates (a
//! `capsule` is a polyline of two or more points, and a `fill` contributes only its bounding sphere
//! here): a `box`'s `at` is its **centre** and its `size` the **full** extent along each axis, exactly as a
//! sculpt box part reads the same two fields.
//!
//! Every irrational operation goes through [`ludion_core::math`], so a region evaluates to the same
//! bits on Windows x86-64 and macOS arm64 — the derived PNG is byte-compared across both.

use ludion_core::math::f64::sqrt;
use ludion_scene::{MeshEditRegion, MeshEditRegionShape};
use serde::Serialize;

/// An axis-aligned box, `min` component-wise below `max`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Aabb {
    /// The lower corner.
    pub min: [f64; 3],
    /// The upper corner.
    pub max: [f64; 3],
}

impl Aabb {
    /// The box containing exactly `point`.
    pub fn point(point: [f64; 3]) -> Self {
        Aabb {
            min: point,
            max: point,
        }
    }

    /// Grows the box to contain `point`.
    pub fn expand(&mut self, point: [f64; 3]) {
        self.min = [0, 1, 2].map(|axis| self.min[axis].min(point[axis]));
        self.max = [0, 1, 2].map(|axis| self.max[axis].max(point[axis]));
    }

    /// The box containing every vertex of a flat `positions` list (3 floats per vertex), or `None`
    /// for an empty list.
    pub fn of_positions(positions: &[f64]) -> Option<Self> {
        let mut vertices = positions.chunks_exact(3);
        let first = vertices.next()?;
        let mut bounds = Aabb::point([first[0], first[1], first[2]]);
        for vertex in vertices {
            bounds.expand([vertex[0], vertex[1], vertex[2]]);
        }
        Some(bounds)
    }
}

/// The signed distance from `point` to `shape`'s surface: negative inside, positive outside, in the
/// source mesh's own units.
///
/// # Example
/// ```
/// use ludion_scene::MeshEditRegionShape;
///
/// let sphere = MeshEditRegionShape::Sphere { at: [0.0; 3], radius: 2.0 };
/// assert_eq!(ludion_mesh_edit::region::signed_distance(&sphere, [5.0, 0.0, 0.0]), 3.0);
/// ```
pub fn signed_distance(shape: &MeshEditRegionShape, point: [f64; 3]) -> f64 {
    match shape {
        MeshEditRegionShape::Sphere { at, radius } => length(sub(point, *at)) - radius,
        MeshEditRegionShape::Capsule { points, radius } => {
            // The unsigned distance to the nearest segment, then one subtraction: a two-point
            // capsule evaluates to the bits the single-segment form did.
            let mut nearest = f64::INFINITY;
            for pair in points.windows(2) {
                nearest = nearest.min(segment_distance(point, pair[0], pair[1]));
            }
            if let [only] = points.as_slice() {
                nearest = segment_distance(point, *only, *only);
            }
            nearest - radius
        }
        MeshEditRegionShape::Box { at, size } => {
            // The exact exterior distance plus the largest interior slab depth: the standard box
            // field, negative inside and a true distance outside.
            let q = [0, 1, 2].map(|axis| (point[axis] - at[axis]).abs() - size[axis] * 0.5);
            let outside = [0, 1, 2].map(|axis| q[axis].max(0.0));
            let inside = q[0].max(q[1]).max(q[2]).min(0.0);
            length(outside) + inside
        }
        // The outer bound only: the patch is a flood over the surface, which the derive walks.
        MeshEditRegionShape::Fill { at, max_radius, .. } => length(sub(point, *at)) - max_radius,
    }
}

/// The unsigned distance from `point` to the segment `from`-`to`.
fn segment_distance(point: [f64; 3], from: [f64; 3], to: [f64; 3]) -> f64 {
    let segment = sub(to, from);
    let offset = sub(point, from);
    let squared = dot(segment, segment);
    // A zero-length segment is a sphere at `from`; nothing rejects one, so it must not divide by
    // zero here.
    let along = if squared > 0.0 {
        (dot(offset, segment) / squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    length(sub(offset, scale(segment, along)))
}

/// The op weight at `point`: `1` inside the shape's surface, falling linearly to `0` over `feather`
/// world units outside it. A `feather` of `0` (or absent) is a hard edge.
///
/// # Example
/// ```
/// use ludion_scene::{MeshEditRegion, MeshEditRegionShape};
///
/// let region = MeshEditRegion {
///     shape: MeshEditRegionShape::Sphere { at: [0.0; 3], radius: 1.0 },
///     feather: Some(1.0),
///     key: None,
/// };
/// assert_eq!(ludion_mesh_edit::region::weight(&region, [0.5, 0.0, 0.0]), 1.0);
/// assert_eq!(ludion_mesh_edit::region::weight(&region, [1.5, 0.0, 0.0]), 0.5);
/// assert_eq!(ludion_mesh_edit::region::weight(&region, [2.5, 0.0, 0.0]), 0.0);
/// ```
pub fn weight(region: &MeshEditRegion, point: [f64; 3]) -> f64 {
    let distance = signed_distance(&region.shape, point);
    if distance <= 0.0 {
        return 1.0;
    }
    let feather = region.feather.unwrap_or(0.0);
    if feather <= 0.0 || distance >= feather {
        return 0.0;
    }
    1.0 - distance / feather
}

/// The world-space box outside which a region paints nothing — the shape grown by its `feather`,
/// except a `fill`, whose feather falls inside its radius. It is what a zero-texel op reports
/// against the mesh's own bounds.
pub fn bounds(region: &MeshEditRegion) -> Aabb {
    let pad = region.feather.unwrap_or(0.0).max(0.0);
    let (min, max) = match &region.shape {
        MeshEditRegionShape::Sphere { at, radius: reach } => {
            let reach = reach + pad;
            (offset(*at, -reach), offset(*at, reach))
        }
        MeshEditRegionShape::Fill {
            at,
            max_radius: reach,
            ..
        } => (offset(*at, -reach), offset(*at, *reach)),
        MeshEditRegionShape::Capsule { points, radius } => {
            let reach = radius + pad;
            let low = [0, 1, 2]
                .map(|axis| points.iter().map(|p| p[axis]).fold(f64::INFINITY, f64::min) - reach);
            let high = [0, 1, 2].map(|axis| {
                points
                    .iter()
                    .map(|p| p[axis])
                    .fold(f64::NEG_INFINITY, f64::max)
                    + reach
            });
            (low, high)
        }
        MeshEditRegionShape::Box { at, size } => {
            let low = [0, 1, 2].map(|axis| at[axis] - size[axis] * 0.5 - pad);
            let high = [0, 1, 2].map(|axis| at[axis] + size[axis] * 0.5 + pad);
            (low, high)
        }
    };
    Aabb { min, max }
}

fn offset(point: [f64; 3], by: f64) -> [f64; 3] {
    [0, 1, 2].map(|axis| point[axis] + by)
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|axis| a[axis] - b[axis])
}

fn scale(a: [f64; 3], by: f64) -> [f64; 3] {
    [0, 1, 2].map(|axis| a[axis] * by)
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn length(a: [f64; 3]) -> f64 {
    sqrt(dot(a, a))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sphere() -> MeshEditRegionShape {
        MeshEditRegionShape::Sphere {
            at: [1.0, 2.0, 3.0],
            radius: 2.0,
        }
    }

    fn capsule() -> MeshEditRegionShape {
        MeshEditRegionShape::Capsule {
            points: vec![[0.0, 0.0, 0.0], [4.0, 0.0, 0.0]],
            radius: 1.0,
        }
    }

    /// Two segments meeting at a right angle: (0,0,0) to (4,0,0) to (4,3,0).
    fn bent() -> MeshEditRegionShape {
        MeshEditRegionShape::Capsule {
            points: vec![[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 3.0, 0.0]],
            radius: 1.0,
        }
    }

    fn unit_box() -> MeshEditRegionShape {
        MeshEditRegionShape::Box {
            at: [0.0, 0.0, 0.0],
            size: [2.0, 4.0, 6.0],
        }
    }

    #[test]
    fn sphere_distances_are_the_hand_computed_ones() {
        assert_eq!(signed_distance(&sphere(), [1.0, 2.0, 3.0]), -2.0);
        assert_eq!(signed_distance(&sphere(), [4.0, 2.0, 3.0]), 1.0);
        assert_eq!(signed_distance(&sphere(), [1.0, 2.0, 5.0]), 0.0);
        // 3-4-5: the point is 5 from the centre, so 3 outside a radius-2 sphere.
        assert_eq!(signed_distance(&sphere(), [4.0, 6.0, 3.0]), 3.0);
    }

    #[test]
    fn capsule_distances_are_the_hand_computed_ones() {
        // On the axis, inside: the full radius deep.
        assert_eq!(signed_distance(&capsule(), [2.0, 0.0, 0.0]), -1.0);
        // Beside the axis, between the caps.
        assert_eq!(signed_distance(&capsule(), [2.0, 3.0, 0.0]), 2.0);
        // Past the far cap, on the axis: the segment parameter clamps to 1.
        assert_eq!(signed_distance(&capsule(), [7.0, 0.0, 0.0]), 2.0);
        // Past the near cap, off axis: 3-4-5 from the `from` endpoint.
        assert_eq!(signed_distance(&capsule(), [-3.0, 4.0, 0.0]), 4.0);
    }

    #[test]
    fn a_degenerate_capsule_is_a_sphere_at_its_start() {
        let point = MeshEditRegionShape::Capsule {
            points: vec![[1.0, 1.0, 1.0], [1.0, 1.0, 1.0]],
            radius: 1.0,
        };
        assert_eq!(signed_distance(&point, [4.0, 1.0, 1.0]), 2.0);
    }

    #[test]
    fn a_polyline_capsule_measures_to_its_nearest_segment() {
        // Beside the second segment: 2 from the x = 4 leg, so 1 outside the radius.
        assert_eq!(signed_distance(&bent(), [6.0, 1.5, 0.0]), 1.0);
        // On the first segment, the second is farther: the full radius deep.
        assert_eq!(signed_distance(&bent(), [2.0, 0.0, 0.0]), -1.0);
        // Past the far end of the second segment: (3, 1) from (4,3,0), so sqrt(10) from it.
        assert_eq!(signed_distance(&bent(), [7.0, 4.0, 0.0]), sqrt(10.0) - 1.0);
        // Below the joint, off both legs: 3-4-5 from the joint (4,0,0).
        assert_eq!(signed_distance(&bent(), [7.0, -4.0, 0.0]), 4.0);
    }

    #[test]
    fn a_fill_measures_to_its_bounding_sphere() {
        let fill = MeshEditRegionShape::Fill {
            at: [1.0, 2.0, 3.0],
            tolerance: 0.1,
            max_radius: 2.0,
            map: None,
            color_feather: None,
        };
        assert_eq!(signed_distance(&fill, [4.0, 6.0, 3.0]), 3.0);
        let region = MeshEditRegion {
            shape: fill,
            feather: Some(1.0),
            key: None,
        };
        // The fill's feather is inside its radius, so it does not grow the box.
        assert_eq!(bounds(&region).min, [-1.0, 0.0, 1.0]);
        assert_eq!(bounds(&region).max, [3.0, 4.0, 5.0]);
    }

    #[test]
    fn a_polyline_bounds_cover_every_point() {
        let region = MeshEditRegion {
            shape: bent(),
            feather: None,
            key: None,
        };
        assert_eq!(bounds(&region).min, [-1.0, -1.0, -1.0]);
        assert_eq!(bounds(&region).max, [5.0, 4.0, 1.0]);
    }

    #[test]
    fn box_distances_are_the_hand_computed_ones() {
        // The centre is the deepest interior point: half the smallest extent.
        assert_eq!(signed_distance(&unit_box(), [0.0, 0.0, 0.0]), -1.0);
        // Interior, nearer one face: the largest slab depth wins.
        assert_eq!(signed_distance(&unit_box(), [0.0, 1.5, 0.0]), -0.5);
        // On a face.
        assert_eq!(signed_distance(&unit_box(), [1.0, 0.0, 0.0]), 0.0);
        // Straight out from a face.
        assert_eq!(signed_distance(&unit_box(), [3.0, 0.0, 0.0]), 2.0);
        // Out past an edge: 3-4-5 in the two axes that are outside.
        assert_eq!(signed_distance(&unit_box(), [4.0, 6.0, 0.0]), 5.0);
    }

    #[test]
    fn feather_is_linear_and_zero_is_a_hard_edge() {
        let sharp = MeshEditRegion {
            shape: sphere(),
            feather: None,
            key: None,
        };
        assert_eq!(weight(&sharp, [1.0, 2.0, 5.0]), 1.0);
        assert_eq!(weight(&sharp, [1.0, 2.0, 5.000_001]), 0.0);

        let soft = MeshEditRegion {
            shape: sphere(),
            feather: Some(4.0),
            key: None,
        };
        assert_eq!(weight(&soft, [4.0, 2.0, 3.0]), 0.75);
        assert_eq!(weight(&soft, [1.0, 2.0, 9.0]), 0.0);
    }

    #[test]
    fn bounds_grow_by_the_feather() {
        let region = MeshEditRegion {
            shape: capsule(),
            feather: Some(0.5),
            key: None,
        };
        let box_of = bounds(&region);
        assert_eq!(box_of.min, [-1.5, -1.5, -1.5]);
        assert_eq!(box_of.max, [5.5, 1.5, 1.5]);

        let region = MeshEditRegion {
            shape: unit_box(),
            feather: None,
            key: None,
        };
        let box_of = bounds(&region);
        assert_eq!(box_of.min, [-1.0, -2.0, -3.0]);
        assert_eq!(box_of.max, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn positions_bound_every_vertex() {
        let bounds = Aabb::of_positions(&[0.0, 1.0, 2.0, -1.0, 5.0, 0.5]).expect("two vertices");
        assert_eq!(bounds.min, [-1.0, 1.0, 0.5]);
        assert_eq!(bounds.max, [0.0, 5.0, 2.0]);
        assert_eq!(Aabb::of_positions(&[]), None);
    }
}
