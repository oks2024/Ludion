//! The pick's geometry half: one ray against a triangle list.
//!
//! Möller–Trumbore, **double-sided** (a generated mesh is flagged double-sided and its winding is
//! not something an author can see), nearest positive `t`, ties resolved to the **lower triangle
//! index** — the tie rule the UV rasteriser and `ludion-sculpt`'s triangle soup already follow.
//! Brute force over the list: a few thousand triangles per pick is microseconds, and a pick is one
//! ray, not a frame.
//!
//! Pure geometry: the camera that turns a pixel into a ray belongs to the model sheet, so it is the
//! caller that supplies `origin` and `direction`. Two camera definitions would disagree silently
//! with the picture the author is reading the pixel off.

use serde::Serialize;

/// Rays shorter than this (in model units) are treated as starting behind the surface, so a ray
/// whose origin sits on a triangle does not hit that triangle at `t == 0`.
const MIN_DISTANCE: f64 = 1e-9;

/// Where a ray met the surface.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct RayHit {
    /// The distance along `direction`, in `direction`'s own units.
    pub t: f64,
    /// The hit point, in the mesh's own coordinates — the space a region is written in.
    pub position: [f64; 3],
    /// Which triangle was hit (an index into the triangle list, not into `indices`).
    pub triangle: u32,
    /// The hit's barycentric weights over the triangle's three corners, in corner order.
    pub barycentric: [f64; 3],
    /// The interpolated UV, when the mesh carries a UV set.
    pub uv: Option<[f64; 2]>,
}

/// Casts one ray at a triangle list and returns the nearest hit, or `None` when the ray misses.
///
/// `positions` is 3 floats per vertex, `indices` 3 per triangle, `uvs` (when present) 2 per vertex
/// — a [`ludion_scene::MeshDocument`]'s own lists. `direction` need not be normalized; `t` is in
/// its units.
///
/// # Example
/// ```
/// // One triangle in the z = 0 plane, hit straight down the -Z axis.
/// let positions = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
/// let hit = ludion_mesh_edit::ray::cast(
///     [0.2, 0.2, 5.0],
///     [0.0, 0.0, -1.0],
///     &positions,
///     &[0, 1, 2],
///     None,
/// )
/// .expect("the ray meets the triangle");
/// assert_eq!(hit.triangle, 0);
/// assert_eq!(hit.position, [0.2, 0.2, 0.0]);
/// ```
pub fn cast(
    origin: [f64; 3],
    direction: [f64; 3],
    positions: &[f64],
    indices: &[u32],
    uvs: Option<&[f64]>,
) -> Option<RayHit> {
    let vertices = positions.len() / 3;
    let mut best: Option<RayHit> = None;

    for (triangle, corners) in indices.chunks_exact(3).enumerate() {
        if corners.iter().any(|index| *index as usize >= vertices) {
            continue;
        }
        let corner = [0, 1, 2].map(|slot| vertex(positions, corners[slot] as usize));
        let Some((t, u, v)) = intersect(origin, direction, &corner) else {
            continue;
        };
        // Strictly nearer only: an equal `t` keeps the triangle already found, which is the lower
        // index because the list is walked in order.
        if best.is_some_and(|hit| t >= hit.t) {
            continue;
        }
        let barycentric = [1.0 - u - v, u, v];
        best = Some(RayHit {
            t,
            position: [0, 1, 2].map(|axis| origin[axis] + direction[axis] * t),
            triangle: triangle as u32,
            barycentric,
            uv: uvs.and_then(|uvs| interpolate_uv(uvs, corners, barycentric)),
        });
    }
    best
}

/// Möller–Trumbore without the back-face cull: returns `(t, u, v)` for a hit in front of `origin`.
fn intersect(
    origin: [f64; 3],
    direction: [f64; 3],
    corner: &[[f64; 3]; 3],
) -> Option<(f64, f64, f64)> {
    let edge1 = sub(corner[1], corner[0]);
    let edge2 = sub(corner[2], corner[0]);
    let pvec = cross(direction, edge2);
    let determinant = dot(edge1, pvec);
    if determinant == 0.0 {
        return None;
    }
    let inverse = 1.0 / determinant;
    let tvec = sub(origin, corner[0]);
    let u = dot(tvec, pvec) * inverse;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let qvec = cross(tvec, edge1);
    let v = dot(direction, qvec) * inverse;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = dot(edge2, qvec) * inverse;
    (t > MIN_DISTANCE).then_some((t, u, v))
}

fn interpolate_uv(uvs: &[f64], corners: &[u32], barycentric: [f64; 3]) -> Option<[f64; 2]> {
    let mut uv = [0.0; 2];
    for (corner, weight) in corners.iter().zip(barycentric) {
        let base = (*corner as usize) * 2;
        let pair = uvs.get(base..base + 2)?;
        uv[0] += pair[0] * weight;
        uv[1] += pair[1] * weight;
    }
    Some(uv)
}

fn vertex(positions: &[f64], index: usize) -> [f64; 3] {
    [
        positions[index * 3],
        positions[index * 3 + 1],
        positions[index * 3 + 2],
    ]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|axis| a[axis] - b[axis])
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two triangles: index 0 in the `z = 0` plane, index 1 one unit behind it.
    const PAIR: [f64; 18] = [
        0.0, 0.0, 0.0, //
        2.0, 0.0, 0.0, //
        0.0, 2.0, 0.0, //
        0.0, 0.0, -1.0, //
        2.0, 0.0, -1.0, //
        0.0, 2.0, -1.0,
    ];
    const PAIR_INDICES: [u32; 6] = [0, 1, 2, 3, 4, 5];

    #[test]
    fn a_ray_down_the_axis_hits_the_nearest_triangle() {
        let hit = cast(
            [0.5, 0.5, 4.0],
            [0.0, 0.0, -1.0],
            &PAIR,
            &PAIR_INDICES,
            None,
        )
        .expect("the pair is in front of the ray");
        assert_eq!(hit.triangle, 0);
        assert_eq!(hit.t, 4.0);
        assert_eq!(hit.position, [0.5, 0.5, 0.0]);
        assert_eq!(hit.barycentric, [0.5, 0.25, 0.25]);
    }

    #[test]
    fn a_ray_beside_the_mesh_misses() {
        assert_eq!(
            cast(
                [5.0, 5.0, 4.0],
                [0.0, 0.0, -1.0],
                &PAIR,
                &PAIR_INDICES,
                None
            ),
            None
        );
    }

    /// Two triangles at the same depth: the tie goes to the lower index whichever order the ray
    /// meets them in the list.
    #[test]
    fn a_tie_resolves_to_the_lower_index() {
        let coplanar = [
            0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 2.0, 0.0, // triangle 0
            0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 2.0, 0.0, // triangle 1, the same place
        ];
        let hit = cast(
            [0.5, 0.5, 4.0],
            [0.0, 0.0, -1.0],
            &coplanar,
            &[0, 1, 2, 3, 4, 5],
            None,
        )
        .expect("both triangles are hit");
        assert_eq!(hit.triangle, 0);
    }

    /// The generated meshes this crate edits are flagged double-sided, so a ray meeting a triangle
    /// from behind is a hit, not a miss.
    #[test]
    fn a_back_face_is_hit() {
        let hit = cast(
            [0.5, 0.5, -4.0],
            [0.0, 0.0, 1.0],
            &PAIR[..9],
            &[0, 1, 2],
            None,
        )
        .expect("the back of the triangle is a surface too");
        assert_eq!(hit.triangle, 0);
        assert_eq!(hit.t, 4.0);
    }

    #[test]
    fn a_hit_interpolates_the_uv_set() {
        let uvs = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0];
        let hit = cast(
            [0.5, 0.5, 4.0],
            [0.0, 0.0, -1.0],
            &PAIR[..9],
            &[0, 1, 2],
            Some(&uvs),
        )
        .expect("the ray hits");
        assert_eq!(hit.uv, Some([0.25, 0.25]));
    }

    #[test]
    fn a_ray_pointing_away_misses() {
        assert_eq!(
            cast([0.5, 0.5, 4.0], [0.0, 0.0, 1.0], &PAIR, &PAIR_INDICES, None),
            None
        );
    }

    #[test]
    fn a_ray_parallel_to_the_triangle_misses() {
        assert_eq!(
            cast([0.5, 0.5, 4.0], [1.0, 0.0, 0.0], &PAIR, &PAIR_INDICES, None),
            None
        );
    }
}
