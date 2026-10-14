//! UV rasterisation: which texel of a map a mesh's surface reaches, and where on the surface.
//!
//! A texel is sampled at its centre `((x + 0.5) / width, (y + 0.5) / height)`, with `v` measured
//! **downward from the top row** — glTF's convention and the row order of
//! [`ludion_assets::Image::rgba`]. A texel covered by more than one UV triangle resolves to the
//! **lower triangle index**, the rule `ludion-sculpt`'s triangle soup already follows, so a texel
//! centre sitting exactly on a shared edge has one answer rather than two.
//!
//! **UVs must lie in `[0, 1]`.** A map is addressed in that square, so a UV outside it means the
//! mesh tiles or wraps its maps and one texel then serves several surface points — "paint here" has
//! no single answer. Such a source is refused (`L1176`) rather than silently painted through a wrap
//! rule the author never wrote.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

/// The largest square extent a derived map may have. A coverage grid holds a surface point per
/// texel, so the ceiling is memory, not format: 4096² is already ~470 MB of working set and four
/// times the largest map any generator in this engine writes.
pub const MAX_EXTENT: u32 = 4096;

/// Marks a texel no triangle covers.
const UNCOVERED: u32 = u32::MAX;

/// Which triangle covers each texel of a `width` × `height` map, and the surface point at its
/// centre. Row-major, top row first — [`ludion_assets::Image::rgba`]'s order.
#[derive(Clone, Debug, PartialEq)]
pub struct Coverage {
    width: u32,
    height: u32,
    triangles: usize,
    triangle: Vec<u32>,
    point: Vec<[f64; 3]>,
}

impl Coverage {
    /// The map width this coverage was built for.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The map height this coverage was built for.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The triangle count of the mesh that was rasterised.
    pub fn triangles(&self) -> usize {
        self.triangles
    }

    /// The texel count, `width * height`.
    pub fn len(&self) -> usize {
        self.triangle.len()
    }

    /// Whether the grid has no texels at all.
    pub fn is_empty(&self) -> bool {
        self.triangle.is_empty()
    }

    /// The triangle index and surface point at texel `index` (row-major), or `None` where no
    /// triangle covers it.
    pub fn at(&self, index: usize) -> Option<(u32, [f64; 3])> {
        match self.triangle.get(index).copied() {
            Some(UNCOVERED) | None => None,
            Some(triangle) => Some((triangle, self.point[index])),
        }
    }

    /// How many texels a triangle covers.
    pub fn covered(&self) -> usize {
        self.triangle.iter().filter(|t| **t != UNCOVERED).count()
    }
}

/// Rasterises a mesh's UV triangles into a `width` × `height` grid.
///
/// `positions` is 3 floats per vertex, `uvs` 2, `indices` 3 per triangle — a
/// [`ludion_scene::MeshDocument`]'s own lists.
///
/// # Errors
/// `L1176` when the lists do not agree, when an index is out of range, when `width`/`height` are
/// `0` or above [`MAX_EXTENT`], or when a UV lies outside `[0, 1]`.
///
/// # Example
/// ```
/// // One triangle filling the lower-left half of the UV square.
/// let positions = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
/// let uvs = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0];
/// let coverage = ludion_mesh_edit::raster::rasterize(&positions, &uvs, &[0, 1, 2], 2, 2)?;
/// assert_eq!(coverage.covered(), 3);
/// # Ok::<(), ludion_core::diagnostics::Diagnostic>(())
/// ```
pub fn rasterize(
    positions: &[f64],
    uvs: &[f64],
    indices: &[u32],
    width: u32,
    height: u32,
) -> Result<Coverage, Diagnostic> {
    let vertices = positions.len() / 3;
    if !positions.len().is_multiple_of(3)
        || uvs.len() != vertices * 2
        || !indices.len().is_multiple_of(3)
    {
        return Err(inapplicable(format!(
            "the source mesh's lists do not agree: {} position floats, {} uv floats, {} indices",
            positions.len(),
            uvs.len(),
            indices.len()
        )));
    }
    if width == 0 || height == 0 || width > MAX_EXTENT || height > MAX_EXTENT {
        return Err(inapplicable(format!(
            "a {width}x{height} map is outside the 1..={MAX_EXTENT} texel range this engine derives"
        )));
    }
    if let Some((index, value)) = uvs
        .iter()
        .enumerate()
        .find(|(_, value)| !(0.0..=1.0).contains(*value))
    {
        let axis = if index % 2 == 0 { 'u' } else { 'v' };
        return Err(inapplicable(format!(
            "vertex {}'s {axis} is {value}, outside the [0, 1] square a texture map covers",
            index / 2
        ))
        .with_help(
            "a UV outside the square means the mesh tiles or wraps its maps, so one texel serves \
             several surface points and a region has no single place to paint; re-unwrap the mesh \
             into one square",
        ));
    }

    let texels = (width as usize) * (height as usize);
    let mut coverage = Coverage {
        width,
        height,
        triangles: indices.len() / 3,
        triangle: vec![UNCOVERED; texels],
        point: vec![[0.0; 3]; texels],
    };

    for (triangle, corners) in indices.chunks_exact(3).enumerate() {
        let mut uv = [[0.0; 2]; 3];
        let mut position = [[0.0; 3]; 3];
        for (corner, index) in corners.iter().enumerate() {
            let vertex = *index as usize;
            if vertex >= vertices {
                return Err(inapplicable(format!(
                    "triangle {triangle} names vertex {vertex}, past the mesh's {vertices} vertices"
                )));
            }
            uv[corner] = [uvs[vertex * 2], uvs[vertex * 2 + 1]];
            position[corner] = [
                positions[vertex * 3],
                positions[vertex * 3 + 1],
                positions[vertex * 3 + 2],
            ];
        }
        rasterize_triangle(&mut coverage, triangle as u32, &uv, &position);
    }
    Ok(coverage)
}

/// Fills every texel whose centre falls inside one triangle, leaving texels an earlier (lower)
/// triangle already claimed alone.
fn rasterize_triangle(
    coverage: &mut Coverage,
    triangle: u32,
    uv: &[[f64; 2]; 3],
    position: &[[f64; 3]; 3],
) {
    let area = cross(uv[0], uv[1], uv[2]);
    if area == 0.0 {
        // A triangle with no UV area covers no texel centre; it is a seam, not a surface.
        return;
    }
    let (width, height) = (coverage.width as f64, coverage.height as f64);
    let min_u = uv[0][0].min(uv[1][0]).min(uv[2][0]);
    let max_u = uv[0][0].max(uv[1][0]).max(uv[2][0]);
    let min_v = uv[0][1].min(uv[1][1]).min(uv[2][1]);
    let max_v = uv[0][1].max(uv[1][1]).max(uv[2][1]);
    // The texel whose centre is at `u` sits at `u * width - 0.5`; widen by one either way so a
    // centre exactly on the bound is never skipped by the floor.
    let x0 = span_start(min_u * width);
    let x1 = span_end(max_u * width, coverage.width);
    let y0 = span_start(min_v * height);
    let y1 = span_end(max_v * height, coverage.height);

    for y in y0..y1 {
        let v = (f64::from(y) + 0.5) / height;
        for x in x0..x1 {
            let index = (y as usize) * (coverage.width as usize) + (x as usize);
            if coverage.triangle[index] != UNCOVERED {
                continue;
            }
            let u = (f64::from(x) + 0.5) / width;
            let p = [u, v];
            let bary = [
                cross(p, uv[1], uv[2]) / area,
                cross(uv[0], p, uv[2]) / area,
                cross(uv[0], uv[1], p) / area,
            ];
            if bary.iter().any(|weight| *weight < 0.0) {
                continue;
            }
            coverage.triangle[index] = triangle;
            coverage.point[index] = [0, 1, 2].map(|axis| {
                bary[0] * position[0][axis]
                    + bary[1] * position[1][axis]
                    + bary[2] * position[2][axis]
            });
        }
    }
}

fn span_start(edge: f64) -> u32 {
    let first = edge.floor() - 1.0;
    if first <= 0.0 { 0 } else { first as u32 }
}

fn span_end(edge: f64, limit: u32) -> u32 {
    let last = edge.ceil() + 1.0;
    if last <= 0.0 {
        0
    } else {
        (last as u32).min(limit)
    }
}

/// Twice the signed area of the UV triangle `a`, `b`, `c`.
fn cross(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn inapplicable(message: String) -> Diagnostic {
    Diagnostic::new(code::MESH_EDIT_INAPPLICABLE, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit square in the XZ plane, UVs covering the whole map, split along the u = v diagonal:
    /// triangle 0 is the lower-right half, triangle 1 the upper-left.
    fn quad() -> ([f64; 12], [f64; 8], [u32; 6]) {
        let positions = [
            0.0, 0.0, 0.0, // uv (0, 0)
            1.0, 0.0, 0.0, // uv (1, 0)
            1.0, 0.0, 1.0, // uv (1, 1)
            0.0, 0.0, 1.0, // uv (0, 1)
        ];
        let uvs = [0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0];
        (positions, uvs, [0, 1, 2, 0, 2, 3])
    }

    /// On a 2x2 grid the texel centres at (0.25, 0.25) and (0.75, 0.75) sit **exactly** on the
    /// shared u = v edge — both halves of the quad contain them — so the tie rule is what decides.
    #[test]
    fn a_texel_centre_on_a_shared_edge_takes_the_lower_triangle() {
        let (positions, uvs, indices) = quad();
        let coverage = rasterize(&positions, &uvs, &indices, 2, 2).expect("the quad rasterises");
        assert_eq!(coverage.covered(), 4);
        assert_eq!(coverage.at(0).map(|(t, _)| t), Some(0), "(0.25, 0.25)");
        assert_eq!(coverage.at(3).map(|(t, _)| t), Some(0), "(0.75, 0.75)");
        // The two texels that are unambiguously one side or the other.
        assert_eq!(coverage.at(1).map(|(t, _)| t), Some(0), "(0.75, 0.25)");
        assert_eq!(coverage.at(2).map(|(t, _)| t), Some(1), "(0.25, 0.75)");
    }

    /// Reversing the triangle order swaps which half wins the tie, which is what proves the rule is
    /// the *index* and not the geometry.
    #[test]
    fn the_tie_follows_the_index_not_the_winding() {
        let (positions, uvs, _) = quad();
        let coverage = rasterize(&positions, &uvs, &[0, 2, 3, 0, 1, 2], 2, 2).expect("rasterises");
        assert_eq!(coverage.at(0).map(|(t, _)| t), Some(0));
        assert_eq!(coverage.at(3).map(|(t, _)| t), Some(0));
        assert_eq!(coverage.at(1).map(|(t, _)| t), Some(1));
    }

    /// `v` runs downward from the top row, so the texel in row 0 reads the surface point whose UV
    /// `v` is small — the top of the map is `v = 0`.
    #[test]
    fn v_is_measured_downward_from_the_top_row() {
        let (positions, uvs, indices) = quad();
        let coverage = rasterize(&positions, &uvs, &indices, 2, 2).expect("the quad rasterises");
        let (_, top) = coverage.at(0).expect("row 0 is covered");
        let (_, bottom) = coverage.at(2).expect("row 1 is covered");
        assert!(top[2] < bottom[2], "{top:?} against {bottom:?}");
        assert_eq!(top, [0.25, 0.0, 0.25]);
        assert_eq!(bottom, [0.25, 0.0, 0.75]);
    }

    #[test]
    fn a_uv_island_leaves_the_rest_of_the_map_uncovered() {
        let (positions, _, indices) = quad();
        // The same quad packed into the middle half of the map.
        let uvs = [0.25, 0.25, 0.75, 0.25, 0.75, 0.75, 0.25, 0.75];
        let coverage = rasterize(&positions, &uvs, &indices, 8, 8).expect("the island rasterises");
        assert_eq!(coverage.covered(), 16);
        assert_eq!(
            coverage.at(0),
            None,
            "the corner texel is outside the island"
        );
        assert!(coverage.at(8 * 3 + 3).is_some(), "the island's own texels");
    }

    #[test]
    fn a_uv_outside_the_square_is_l1176() {
        let (positions, _, indices) = quad();
        let uvs = [0.0, 0.0, 2.0, 0.0, 2.0, 1.0, 0.0, 1.0];
        let error = rasterize(&positions, &uvs, &indices, 4, 4).expect_err("the tiling is refused");
        assert_eq!(error.code, code::MESH_EDIT_INAPPLICABLE);
        assert!(
            error.message.contains("vertex 1's u is 2"),
            "{}",
            error.message
        );
    }

    #[test]
    fn an_out_of_range_extent_is_l1176() {
        let (positions, uvs, indices) = quad();
        for (width, height) in [(0, 4), (4, 0), (MAX_EXTENT + 1, 4)] {
            let error = rasterize(&positions, &uvs, &indices, width, height)
                .expect_err("the extent is refused");
            assert_eq!(error.code, code::MESH_EDIT_INAPPLICABLE);
        }
    }

    #[test]
    fn an_index_past_the_vertices_is_l1176() {
        let (positions, uvs, _) = quad();
        let error = rasterize(&positions, &uvs, &[0, 1, 9], 2, 2).expect_err("refused");
        assert_eq!(error.code, code::MESH_EDIT_INAPPLICABLE);
        assert!(error.message.contains("vertex 9"), "{}", error.message);
    }

    #[test]
    fn a_degenerate_uv_triangle_covers_nothing() {
        let positions = [0.0; 9];
        let uvs = [0.5, 0.5, 0.5, 0.5, 0.5, 0.5];
        let coverage = rasterize(&positions, &uvs, &[0, 1, 2], 4, 4).expect("rasterises");
        assert_eq!(coverage.covered(), 0);
        assert_eq!(coverage.triangles(), 1);
    }
}
