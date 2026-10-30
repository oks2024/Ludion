//! Meshing a `Mesh.shape: voxel` document into renderer-ready [`MeshData`].
//!
//! A voxel model is a strict-YAML `voxels/*.voxel.yaml` document ([`ludion_scene::check_voxel`], Wave
//! 1). This module is the engine-side seam that turns one into the plain vertex data the renderer
//! consumes — the voxel sibling of [`crate::mesh_asset`], with the same `load(root, rel)` signature:
//! read the bytes ([`ludion_assets`]), parse + validate ([`ludion_scene`]), then **greedily mesh**
//! the grid into an indexed, per-quad-colored triangle list. The renderer never sees YAML;
//! `ludion-scene` never sees a wgpu type; both invariants hold because the meshing lives here.
//!
//! The mesher ([`mesh`]) is a pure, exact function of the [`VoxelDocument`] with a **pinned
//! deterministic order** — same grid ⇒ bit-identical vertex stream (conformance-tested by a
//! double run). The order is:
//! - **face directions** −X, +X, −Y, +Y, −Z, +Z ([`FACES`]);
//! - **planes** ascending along the face's axis;
//! - a per-plane mask scanned **row-major** in a pinned axis→`(u, v)` table (`u = (axis+1) % 3`,
//!   `v = (axis+2) % 3`, so `(u, v, axis)` is right-handed for every axis);
//! - at each unclaimed solid cell the quad **extends along `u`, then `v`**, merging **only** across
//!   cells that share a palette index (a merged quad stays material-uniform);
//! - interior faces — a solid neighbor on the face's far side — are culled.
//!
//! Normals are exact axis-aligned ±1 unit vectors (no normalization, no float error); every
//! coordinate is a small integer ± a half-integer, exact in `f32`. Grid reads go through
//! [`Grid::at`], which treats out-of-bounds as empty — the chunked-voxel-worlds seam, costing
//! nothing today. Grid→model space is bottom-center, 1 voxel = 1 unit: cell `(x, y, z)` spans
//! `[x−sx/2, x+1−sx/2] × [y, y+1] × [z−sz/2, z+1−sz/2]`.

use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::math::glam::Vec3;
use ludion_renderer::MeshData;
use ludion_scene::VoxelDocument;

/// The pinned face-direction sweep order: `(axis, positive)` for −X, +X, −Y, +Y, −Z, +Z. Emission
/// follows this order exactly, so it is the vertex-stream order.
const FACES: [(usize, bool); 6] = [
    (0, false), // −X
    (0, true),  // +X
    (1, false), // −Y
    (1, true),  // +Y
    (2, false), // −Z
    (2, true),  // +Z
];

/// Reads, validates, and greedily meshes the voxel document at `root`/`rel` into [`MeshData`].
///
/// # Errors
/// - `L1101` if the file cannot be read.
/// - `L1113` if the bytes are not valid UTF-8 text, or the document is structurally malformed.
/// - `L1114` if the document's data is inconsistent (dimensions, glyphs, colors).
///
/// All diagnostics carry a JSON Pointer into the *voxel document*; the caller ([`extract`](crate::extract))
/// re-points them at the referencing `Mesh` component.
pub(crate) fn load(root: &Path, rel: &str) -> Result<MeshData, Vec<Diagnostic>> {
    let (doc, hash) = read_document(root, rel)?;
    Ok(mesh(&doc, hash))
}

/// Reads, validates, and builds the [`VoxelStaticGrid`] for the voxel document at `root`/`rel` — the
/// D15 accessor's disk-loading entry point (`Plan/gi-wave0-voxelizer.md` §6), mirroring [`load`] but
/// returning the authored occupancy grid instead of meshed geometry. Shares [`read_document`] with
/// `load`, so a malformed or missing asset reports the same diagnostics either way.
///
/// # Errors
/// `L1101`/`L1113`/`L1114`, same as [`load`].
pub(crate) fn load_grid(root: &Path, rel: &str) -> Result<VoxelStaticGrid, Vec<Diagnostic>> {
    let (doc, hash) = read_document(root, rel)?;
    Ok(static_grid(&doc, hash))
}

/// Reads and validates the voxel document at `root`/`rel`, returning it with the source bytes'
/// content hash — the read+parse step shared by [`load`], [`load_grid`] and the sculpt loader, whose
/// `shape: voxel` parts read their field out of a voxel document ([`crate::sculpt_mesh`]).
pub(crate) fn read_document(
    root: &Path,
    rel: &str,
) -> Result<(VoxelDocument, u64), Vec<Diagnostic>> {
    let bytes = ludion_assets::load_bytes(root, rel).map_err(|diagnostic| vec![diagnostic])?;
    let source = std::str::from_utf8(&bytes.bytes).map_err(|_| {
        vec![
            Diagnostic::new(
                code::VOXEL_DOCUMENT_STRUCTURE,
                format!("voxel asset `{rel}` is not valid UTF-8 text"),
            )
            .with_help("a voxel document is a strict-YAML text file (`voxels/*.voxel.yaml`)"),
        ]
    })?;
    let doc = ludion_scene::check_voxel(source)?;
    Ok((doc, bytes.hash))
}

/// A dense palette-index grid, built once from a [`VoxelDocument`] by one linear
/// [`VoxelDocument::for_each_cell`] sweep. `at` is the single bounds-checked lookup the mesher reads
/// through — out-of-bounds reads as empty (`None`), the seam a chunked-voxel-worlds phase would read
/// across. The grid exists for the *shape* of the mesher's access: a greedy merge walks neighbours
/// along every axis and re-reads each plane's cells, which row strings answer only by decoding a row
/// again. The emitted stream is a pure function of the document either way.
struct Grid {
    /// Extent in cells, as `i64` so a neighbor at `−1` is expressible without underflow.
    size: [i64; 3],
    /// Palette index per cell (`None` = empty), flat: `x + sx*(y + sy*z)`.
    cells: Vec<Option<u8>>,
}

impl Grid {
    fn from_doc(doc: &VoxelDocument) -> Grid {
        let [sx, sy, sz] = doc.size;
        let mut cells = vec![None; (sx as usize) * (sy as usize) * (sz as usize)];
        doc.for_each_cell(|x, y, z, index| {
            let flat = x as usize + sx as usize * (y as usize + sy as usize * z as usize);
            cells[flat] = index;
        });
        Grid {
            size: [i64::from(sx), i64::from(sy), i64::from(sz)],
            cells,
        }
    }

    /// The palette index of cell `c`, or `None` for an empty or out-of-bounds cell. Out-of-bounds
    /// reads as empty deliberately — the chunked-worlds seam.
    fn at(&self, c: [i64; 3]) -> Option<u8> {
        let [x, y, z] = c;
        let [sx, sy, sz] = self.size;
        if x < 0 || y < 0 || z < 0 || x >= sx || y >= sy || z >= sz {
            return None;
        }
        let flat = x as usize + sx as usize * (y as usize + sy as usize * z as usize);
        self.cells[flat]
    }
}

/// One occupied cell's renderer-relevant data — the D15 accessor's per-cell payload
/// (`Plan/gi-wave0-voxelizer.md` §6): enough for a consumer (the W0 voxelizer) to deposit a
/// world-space GI cell without re-resolving the palette.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxelCell {
    /// The palette entry's RGBA color.
    pub albedo: [f32; 4],
    /// The palette entry's emissive strength (`0.0` if unauthored) — the same channel `mesh` threads
    /// into `vertex_materials[2]`.
    pub emissive: f32,
}

/// The authored voxel occupancy grid, kept reachable past meshing (GI plan decision D15, option A —
/// `Plan/gi-wave0-voxelizer.md` §6). Dense, flat-indexed `x + sx*(y + sy*z)` exactly like the
/// mesher's internal [`Grid`], `None` for an empty cell. Plain data: a consumer reads a filled
/// cell's `albedo`/`emissive` directly, with no palette lookup left to do.
#[derive(Clone, Debug, PartialEq)]
pub struct VoxelStaticGrid {
    /// Extent in cells, `[sx, sy, sz]` — [`VoxelDocument::size`].
    pub extent: [u32; 3],
    /// Per-cell data, flat-indexed `x + sx*(y + sy*z)`, `None` for empty.
    pub cells: Vec<Option<VoxelCell>>,
    /// Content hash of the source document's bytes — the same key [`MeshData::hash`] carries for the
    /// mesh built from the very same document. The grid is a pure function of those bytes, so this
    /// stands in for its contents wherever a consumer needs a cheap identity (the voxelizer's
    /// instance signature keys on it, which is what makes a hot-reloaded model invalidate).
    pub hash: u64,
}

/// Builds the [`VoxelStaticGrid`] for `doc` — the D15 accessor. `hash` is the source document's
/// content hash, carried through verbatim. Reuses [`Grid::from_doc`] for occupancy and
/// [`palette_channels`] for the same color/material resolution [`mesh`] performs, so the mesher and
/// the D15 accessor can never disagree about what a palette index means (pinned by
/// `static_grid_agrees_with_mesher_grid_on_committed_model`).
pub(crate) fn static_grid(doc: &VoxelDocument, hash: u64) -> VoxelStaticGrid {
    let grid = Grid::from_doc(doc);
    let (colors, materials) = palette_channels(doc);
    let cells = grid
        .cells
        .iter()
        .map(|palette_index| {
            palette_index.map(|pi| VoxelCell {
                albedo: colors[pi as usize],
                emissive: materials[pi as usize][2],
            })
        })
        .collect();
    VoxelStaticGrid {
        extent: doc.size,
        cells,
        hash,
    }
}

/// What greedily meshing a voxel document actually costs the renderer, in primitives — the number
/// that turns `budget_exempt:` from a shrug into an informed call, since `L1115` counts *cells* and
/// a wall of them collapses into one quad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoxelMeshStats {
    /// Merged quads emitted (the greedy mesher's unit of work).
    pub quads: u64,
    /// Triangles emitted — two per quad.
    pub triangles: u64,
    /// Vertices emitted — four per quad, never shared between quads (each carries its own normal
    /// and per-quad material).
    pub vertices: u64,
}

/// The primitive counts `mesh` would emit for `doc`. Meshes the document and counts; there is no
/// cheaper honest answer, because greedy merging is what the question is about.
pub fn voxel_mesh_stats(doc: &VoxelDocument) -> VoxelMeshStats {
    // The hash is the GPU mesh-cache key, which counting has no use for.
    let data = mesh(doc, 0);
    let vertices = (data.positions.len() / 3) as u64;
    VoxelMeshStats {
        quads: vertices / 4,
        triangles: (data.indices.len() / 3) as u64,
        vertices,
    }
}

/// Resolves `doc`'s palette into per-entry `(albedo, [roughness, metallic, emissive])` channels, in
/// palette order — the one place both [`mesh`] and [`static_grid`] read authored color/material
/// data, so they can never resolve a palette index differently. An unauthored channel takes the
/// default material (`roughness` 1.0, `metallic` 0.0, `emissive` 0.0), narrowing `f64` → `f32`.
fn palette_channels(doc: &VoxelDocument) -> (Vec<[f32; 4]>, Vec<[f32; 3]>) {
    let colors = doc
        .palette
        .iter()
        .map(|entry| {
            [
                entry.color[0] as f32,
                entry.color[1] as f32,
                entry.color[2] as f32,
                entry.color[3] as f32,
            ]
        })
        .collect();
    let materials = doc
        .palette
        .iter()
        .map(|entry| {
            [
                entry.roughness.map_or(1.0, |r| r as f32),
                entry.metallic.map_or(0.0, |m| m as f32),
                entry.emissive.map_or(0.0, |e| e as f32),
            ]
        })
        .collect();
    (colors, materials)
}

/// The object-space box of a voxel grid of `size`: 1 voxel = 1 unit, X and Z centered, Y sitting on
/// zero — the frame [`mesh`] emits. The one copy of this rule: the GI voxelizer and `render.bounds`
/// both size a voxel model through it.
pub(crate) fn grid_box(size: [u32; 3]) -> (Vec3, Vec3) {
    let [sx, sy, sz] = size;
    let (hx, hz) = (sx as f32 * 0.5, sz as f32 * 0.5);
    (Vec3::new(-hx, 0.0, -hz), Vec3::new(hx, sy as f32, hz))
}

/// Greedily meshes `doc` into [`MeshData`], per the pinned order documented on the module. `hash` is
/// the source bytes' content hash (the GPU mesh-cache key), threaded through unchanged.
///
/// Pure and exact: the same document always yields a byte-identical vertex stream.
pub(crate) fn mesh(doc: &VoxelDocument, hash: u64) -> MeshData {
    let grid = Grid::from_doc(doc);
    // Bottom-center origin: X and Z are centered (offset = half the extent), Y sits on the ground.
    let offset = [doc.size[0] as f32 * 0.5, 0.0, doc.size[2] as f32 * 0.5];
    let to_model = |axis: usize, coord: i64| coord as f32 - offset[axis];
    let (colors, materials) = palette_channels(doc);
    let authored_material = doc.palette.iter().any(|entry| {
        entry.roughness.is_some() || entry.metallic.is_some() || entry.emissive.is_some()
    });

    let mut positions: Vec<f32> = Vec::new();
    let mut normals: Vec<f32> = Vec::new();
    let mut vertex_colors: Vec<f32> = Vec::new();
    let mut vertex_materials: Vec<f32> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for &(axis, positive) in &FACES {
        let u = (axis + 1) % 3;
        let v = (axis + 2) % 3;
        let u_extent = grid.size[u];
        let v_extent = grid.size[v];
        let mut normal = [0.0_f32; 3];
        normal[axis] = if positive { 1.0 } else { -1.0 };
        let delta = if positive { 1 } else { -1 };

        for a in 0..grid.size[axis] {
            // Mask of this plane's exposed faces: `mask[vi * u_extent + ui]` = the cell's palette
            // index when its face in this direction is exposed, else `None` (empty or interior).
            let mut mask: Vec<Option<u8>> = vec![None; (u_extent * v_extent) as usize];
            for vi in 0..v_extent {
                for ui in 0..u_extent {
                    let mut cell = [0_i64; 3];
                    cell[axis] = a;
                    cell[u] = ui;
                    cell[v] = vi;
                    let Some(pi) = grid.at(cell) else { continue };
                    let mut neighbor = cell;
                    neighbor[axis] += delta;
                    if grid.at(neighbor).is_none() {
                        mask[(vi * u_extent + ui) as usize] = Some(pi);
                    }
                }
            }

            // Greedy merge, row-major (v outer, u inner): extend along u, then along v, merging only
            // cells sharing a palette index.
            let mut claimed = vec![false; mask.len()];
            for vi in 0..v_extent {
                for ui in 0..u_extent {
                    let start = (vi * u_extent + ui) as usize;
                    if claimed[start] {
                        continue;
                    }
                    let Some(pi) = mask[start] else { continue };

                    let mut w = 1;
                    while ui + w < u_extent {
                        let idx = (vi * u_extent + ui + w) as usize;
                        if claimed[idx] || mask[idx] != Some(pi) {
                            break;
                        }
                        w += 1;
                    }

                    let mut h = 1;
                    'grow: while vi + h < v_extent {
                        for k in 0..w {
                            let idx = ((vi + h) * u_extent + ui + k) as usize;
                            if claimed[idx] || mask[idx] != Some(pi) {
                                break 'grow;
                            }
                        }
                        h += 1;
                    }

                    for dv in 0..h {
                        for du in 0..w {
                            claimed[((vi + dv) * u_extent + ui + du) as usize] = true;
                        }
                    }

                    let pb = if positive { a + 1 } else { a };
                    let (u0, u1, v0, v1) = (ui, ui + w, vi, vi + h);
                    // Corners as (u-coord, v-coord). Winding is CCW seen from outside so
                    // `(b−a)×(c−a)` matches the outward normal (mesh pipeline culls back faces).
                    let corners: [(i64, i64); 4] = if positive {
                        [(u0, v0), (u1, v0), (u1, v1), (u0, v1)]
                    } else {
                        [(u0, v0), (u0, v1), (u1, v1), (u1, v0)]
                    };
                    let color = colors[pi as usize];
                    let material = materials[pi as usize];
                    let base = (positions.len() / 3) as u32;
                    for (uc, vc) in corners {
                        let mut g = [0_i64; 3];
                        g[axis] = pb;
                        g[u] = uc;
                        g[v] = vc;
                        positions.extend_from_slice(&[
                            to_model(0, g[0]),
                            to_model(1, g[1]),
                            to_model(2, g[2]),
                        ]);
                        normals.extend_from_slice(&normal);
                        vertex_colors.extend_from_slice(&color);
                        vertex_materials.extend_from_slice(&material);
                    }
                    indices.extend_from_slice(&[
                        base,
                        base + 1,
                        base + 2,
                        base,
                        base + 2,
                        base + 3,
                    ]);
                }
            }
        }
    }

    // Voxel quads carry no texture mapping, so every vertex takes the unmapped default `(0, 0)` and
    // the placeholder tangent frame `(+X, +1)`.
    let vertex_count = positions.len() / 3;
    MeshData {
        positions,
        normals,
        colors: vertex_colors,
        materials: vertex_materials,
        uvs: vec![0.0; vertex_count * 2],
        tangents: [1.0, 0.0, 0.0, 1.0].repeat(vertex_count),
        weights: [1.0, 0.0, 0.0, 0.0].repeat(vertex_count),
        bone_indices: vec![0; vertex_count * 4],
        skin_joints: Vec::new(),
        textures: None,
        authored_material,
        base_color_mean: [1.0; 4],
        double_sided: false,
        indices,
        hash,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_scene::PaletteEntry;
    use proptest::prelude::*;

    /// A palette entry with a single glyph and an opaque color.
    fn entry(glyph: &str, color: [f64; 4]) -> PaletteEntry {
        PaletteEntry {
            glyph: glyph.to_string(),
            color,
            emissive: None,
            metallic: None,
            roughness: None,
        }
    }

    /// Builds a document directly from parts (the mesher is a pure function of the document, so
    /// tests skip the YAML layer). `slices[y][z]` is a row string; column `c` is `x = c`.
    fn doc(size: [u32; 3], palette: Vec<PaletteEntry>, slices: &[&[&str]]) -> VoxelDocument {
        VoxelDocument {
            name: None,
            size,
            scale: None,
            generated_from: None,
            budget_exempt: None,
            palette,
            slices: slices
                .iter()
                .map(|layer| layer.iter().map(|r| r.to_string()).collect())
                .collect(),
        }
    }

    const RED: [f64; 4] = [1.0, 0.0, 0.0, 1.0];
    const GREEN: [f64; 4] = [0.0, 1.0, 0.0, 1.0];

    /// One quad's expected data: outward normal, four corner positions (in emission order), color.
    type Quad = ([f32; 3], [[f32; 3]; 4], [f32; 4]);

    /// Expands a list of expected quads into flat [`MeshData`] arrays using the same fixed indexing
    /// the mesher emits (two triangles `0,1,2` and `0,2,3` per quad). The quad geometry itself is
    /// hand-authored, so the comparison is a genuine pinned-stream check, not a re-run of the mesher.
    /// The quads carry no authored material, so every vertex takes the default material.
    fn expected(quads: &[Quad], hash: u64) -> MeshData {
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut colors = Vec::new();
        let mut materials = Vec::new();
        let mut uvs = Vec::new();
        let mut tangents = Vec::new();
        let mut indices = Vec::new();
        for (normal, corners, color) in quads {
            let base = (positions.len() / 3) as u32;
            for corner in corners {
                positions.extend_from_slice(corner);
                normals.extend_from_slice(normal);
                colors.extend_from_slice(color);
                materials.extend_from_slice(&[1.0, 0.0, 0.0]);
                uvs.extend_from_slice(&[0.0, 0.0]);
                tangents.extend_from_slice(&[1.0, 0.0, 0.0, 1.0]);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        let vertex_count = positions.len() / 3;
        MeshData {
            positions,
            normals,
            colors,
            materials,
            uvs,
            tangents,
            weights: [1.0, 0.0, 0.0, 0.0].repeat(vertex_count),
            bone_indices: vec![0; vertex_count * 4],
            skin_joints: Vec::new(),
            textures: None,
            authored_material: false,
            base_color_mean: [1.0; 4],
            double_sided: false,
            indices,
            hash,
        }
    }

    /// A single voxel emits the six faces in the pinned −X,+X,−Y,+Y,−Z,+Z order — 24 verts, 12 tris
    /// — with the exact hand-computed vertex stream (bottom-center origin: y ∈ [0,1], x,z ∈ ±0.5).
    #[test]
    fn single_voxel_exact_stream() {
        let model = doc([1, 1, 1], vec![entry("A", RED)], &[&["A"]]);
        let c = [1.0_f32, 0.0, 0.0, 1.0];
        let quads: Vec<Quad> = vec![
            // −X, X = −0.5
            (
                [-1.0, 0.0, 0.0],
                [
                    [-0.5, 0.0, -0.5],
                    [-0.5, 0.0, 0.5],
                    [-0.5, 1.0, 0.5],
                    [-0.5, 1.0, -0.5],
                ],
                c,
            ),
            // +X, X = 0.5
            (
                [1.0, 0.0, 0.0],
                [
                    [0.5, 0.0, -0.5],
                    [0.5, 1.0, -0.5],
                    [0.5, 1.0, 0.5],
                    [0.5, 0.0, 0.5],
                ],
                c,
            ),
            // −Y, Y = 0
            (
                [0.0, -1.0, 0.0],
                [
                    [-0.5, 0.0, -0.5],
                    [0.5, 0.0, -0.5],
                    [0.5, 0.0, 0.5],
                    [-0.5, 0.0, 0.5],
                ],
                c,
            ),
            // +Y, Y = 1
            (
                [0.0, 1.0, 0.0],
                [
                    [-0.5, 1.0, -0.5],
                    [-0.5, 1.0, 0.5],
                    [0.5, 1.0, 0.5],
                    [0.5, 1.0, -0.5],
                ],
                c,
            ),
            // −Z, Z = −0.5
            (
                [0.0, 0.0, -1.0],
                [
                    [-0.5, 0.0, -0.5],
                    [-0.5, 1.0, -0.5],
                    [0.5, 1.0, -0.5],
                    [0.5, 0.0, -0.5],
                ],
                c,
            ),
            // +Z, Z = 0.5
            (
                [0.0, 0.0, 1.0],
                [
                    [-0.5, 0.0, 0.5],
                    [0.5, 0.0, 0.5],
                    [0.5, 1.0, 0.5],
                    [-0.5, 1.0, 0.5],
                ],
                c,
            ),
        ];
        assert_eq!(mesh(&model, 0x77), expected(&quads, 0x77));
    }

    /// Two same-palette cells along x merge into a single box: the shared interior faces are culled,
    /// and the ±Y / ±Z faces merge across both cells into one quad each — 6 quads, 24 verts.
    #[test]
    fn two_cells_same_entry_merge_into_one_box() {
        let model = doc([2, 1, 1], vec![entry("A", RED)], &[&["AA"]]);
        let c = [1.0_f32, 0.0, 0.0, 1.0];
        let quads: Vec<Quad> = vec![
            // −X (only cell 0), X = −1
            (
                [-1.0, 0.0, 0.0],
                [
                    [-1.0, 0.0, -0.5],
                    [-1.0, 0.0, 0.5],
                    [-1.0, 1.0, 0.5],
                    [-1.0, 1.0, -0.5],
                ],
                c,
            ),
            // +X (only cell 1), X = 1
            (
                [1.0, 0.0, 0.0],
                [
                    [1.0, 0.0, -0.5],
                    [1.0, 1.0, -0.5],
                    [1.0, 1.0, 0.5],
                    [1.0, 0.0, 0.5],
                ],
                c,
            ),
            // −Y, merged across x ∈ [−1, 1]
            (
                [0.0, -1.0, 0.0],
                [
                    [-1.0, 0.0, -0.5],
                    [1.0, 0.0, -0.5],
                    [1.0, 0.0, 0.5],
                    [-1.0, 0.0, 0.5],
                ],
                c,
            ),
            // +Y, merged
            (
                [0.0, 1.0, 0.0],
                [
                    [-1.0, 1.0, -0.5],
                    [-1.0, 1.0, 0.5],
                    [1.0, 1.0, 0.5],
                    [1.0, 1.0, -0.5],
                ],
                c,
            ),
            // −Z, merged
            (
                [0.0, 0.0, -1.0],
                [
                    [-1.0, 0.0, -0.5],
                    [-1.0, 1.0, -0.5],
                    [1.0, 1.0, -0.5],
                    [1.0, 0.0, -0.5],
                ],
                c,
            ),
            // +Z, merged
            (
                [0.0, 0.0, 1.0],
                [
                    [-1.0, 0.0, 0.5],
                    [1.0, 0.0, 0.5],
                    [1.0, 1.0, 0.5],
                    [-1.0, 1.0, 0.5],
                ],
                c,
            ),
        ];
        assert_eq!(mesh(&model, 0), expected(&quads, 0));
    }

    /// Two different-palette cells along x: the shared faces are still culled (both solid), but the
    /// ±Y / ±Z faces do NOT merge across the palette boundary — 10 quads, each material-uniform.
    #[test]
    fn two_cells_different_entries_cull_but_do_not_merge() {
        let model = doc(
            [2, 1, 1],
            vec![entry("A", RED), entry("B", GREEN)],
            &[&["AB"]],
        );
        let a = [1.0_f32, 0.0, 0.0, 1.0]; // A = index 0 (red)
        let b = [0.0_f32, 1.0, 0.0, 1.0]; // B = index 1 (green)
        let quads: Vec<Quad> = vec![
            // −X (cell 0, A), X = −1
            (
                [-1.0, 0.0, 0.0],
                [
                    [-1.0, 0.0, -0.5],
                    [-1.0, 0.0, 0.5],
                    [-1.0, 1.0, 0.5],
                    [-1.0, 1.0, -0.5],
                ],
                a,
            ),
            // +X (cell 1, B), X = 1
            (
                [1.0, 0.0, 0.0],
                [
                    [1.0, 0.0, -0.5],
                    [1.0, 1.0, -0.5],
                    [1.0, 1.0, 0.5],
                    [1.0, 0.0, 0.5],
                ],
                b,
            ),
            // −Y, cell 0 (A), x ∈ [−1, 0]
            (
                [0.0, -1.0, 0.0],
                [
                    [-1.0, 0.0, -0.5],
                    [0.0, 0.0, -0.5],
                    [0.0, 0.0, 0.5],
                    [-1.0, 0.0, 0.5],
                ],
                a,
            ),
            // −Y, cell 1 (B), x ∈ [0, 1]
            (
                [0.0, -1.0, 0.0],
                [
                    [0.0, 0.0, -0.5],
                    [1.0, 0.0, -0.5],
                    [1.0, 0.0, 0.5],
                    [0.0, 0.0, 0.5],
                ],
                b,
            ),
            // +Y, cell 0 (A)
            (
                [0.0, 1.0, 0.0],
                [
                    [-1.0, 1.0, -0.5],
                    [-1.0, 1.0, 0.5],
                    [0.0, 1.0, 0.5],
                    [0.0, 1.0, -0.5],
                ],
                a,
            ),
            // +Y, cell 1 (B)
            (
                [0.0, 1.0, 0.0],
                [
                    [0.0, 1.0, -0.5],
                    [0.0, 1.0, 0.5],
                    [1.0, 1.0, 0.5],
                    [1.0, 1.0, -0.5],
                ],
                b,
            ),
            // −Z, cell 0 (A)
            (
                [0.0, 0.0, -1.0],
                [
                    [-1.0, 0.0, -0.5],
                    [-1.0, 1.0, -0.5],
                    [0.0, 1.0, -0.5],
                    [0.0, 0.0, -0.5],
                ],
                a,
            ),
            // −Z, cell 1 (B)
            (
                [0.0, 0.0, -1.0],
                [
                    [0.0, 0.0, -0.5],
                    [0.0, 1.0, -0.5],
                    [1.0, 1.0, -0.5],
                    [1.0, 0.0, -0.5],
                ],
                b,
            ),
            // +Z, cell 0 (A)
            (
                [0.0, 0.0, 1.0],
                [
                    [-1.0, 0.0, 0.5],
                    [0.0, 0.0, 0.5],
                    [0.0, 1.0, 0.5],
                    [-1.0, 1.0, 0.5],
                ],
                a,
            ),
            // +Z, cell 1 (B)
            (
                [0.0, 0.0, 1.0],
                [
                    [0.0, 0.0, 0.5],
                    [1.0, 0.0, 0.5],
                    [1.0, 1.0, 0.5],
                    [0.0, 1.0, 0.5],
                ],
                b,
            ),
        ];
        assert_eq!(mesh(&model, 0), expected(&quads, 0));
    }

    /// A single 1×1×1 cell is a full box: 6 quads, 24 verts, 12 tris.
    #[test]
    fn single_voxel_counts() {
        let model = doc([1, 1, 1], vec![entry("A", RED)], &[&["A"]]);
        let data = mesh(&model, 0);
        assert_eq!(quad_count(&data), 6);
        assert_eq!(data.positions.len(), 24 * 3);
        assert_eq!(data.indices.len(), 12 * 3);
    }

    /// The published counts are the emitted stream's, in the engine's own 4-verts/6-indices-per-quad
    /// terms: a single cell is a full box, and a two-material pair is the case where merging is
    /// *refused*, so the number an author reads reflects the palette they authored.
    #[test]
    fn stats_count_the_emitted_primitives() {
        let solid = doc([1, 1, 1], vec![entry("A", RED)], &[&["A"]]);
        assert_eq!(
            voxel_mesh_stats(&solid),
            VoxelMeshStats {
                quads: 6,
                triangles: 12,
                vertices: 24
            }
        );
        // 'AB' along x: the shared faces cull, but ±Y/±Z do not merge across the palette boundary.
        let two_tone = doc(
            [2, 1, 1],
            vec![entry("A", RED), entry("B", GREEN)],
            &[&["AB"]],
        );
        assert_eq!(
            voxel_mesh_stats(&two_tone),
            VoxelMeshStats {
                quads: 10,
                triangles: 20,
                vertices: 40
            }
        );
        // The same two cells in one material merge instead — the collapse that makes the number
        // worth printing at all.
        let one_tone = doc([2, 1, 1], vec![entry("A", RED)], &[&["AA"]]);
        assert_eq!(voxel_mesh_stats(&one_tone).quads, 6);
    }

    /// A hollow 3×3×3 shell (center empty) exercises interior culling: the six cells around the
    /// cavity expose inward-facing faces. Outer surface = 6×9 = 54 unit faces (each side merges to
    /// one quad); inner surface = 6 unit faces (each isolated). So 12 quads, 24 tris, and the total
    /// covered area is 60 — far below the un-culled 26 cells × 6 = 156 faces.
    #[test]
    fn hollow_shell_culls_interior() {
        let full = "###";
        let ring = "###";
        let mid = "#.#"; // center cell (1,1,1) empty
        let model = doc(
            [3, 3, 3],
            vec![entry("#", RED)],
            &[
                &[full, full, full], // y = 0
                &[ring, mid, ring],  // y = 1
                &[full, full, full], // y = 2
            ],
        );
        let data = mesh(&model, 0);
        assert_eq!(
            quad_count(&data),
            12,
            "6 outer quads + 6 isolated inner faces"
        );
        assert_eq!(data.indices.len() / 3, 24);
        assert!(
            (total_area(&data) - 60.0).abs() < 1e-4,
            "surface area is conserved across merging"
        );
        assert!(total_area(&data) < 156.0, "interior faces are culled");
    }

    /// A 3D checkerboard (fill cells with even coordinate sum) is the worst case: no two filled cells
    /// touch, so nothing culls and nothing merges. 14 filled cells × 6 faces = 84 quads.
    #[test]
    fn checkerboard_never_merges() {
        // even-sum cells filled with 'A'. Row r is z = r, column c is x = c.
        let e = |z: u32, y: u32| -> String {
            (0..3)
                .map(|x| {
                    if (x + y + z).is_multiple_of(2) {
                        'A'
                    } else {
                        '.'
                    }
                })
                .collect()
        };
        let layer = |y: u32| vec![e(0, y), e(1, y), e(2, y)];
        let model = VoxelDocument {
            name: None,
            size: [3, 3, 3],
            scale: None,
            generated_from: None,
            budget_exempt: None,
            palette: vec![entry("A", RED)],
            slices: vec![layer(0), layer(1), layer(2)],
        };
        let data = mesh(&model, 0);
        assert_eq!(
            quad_count(&data),
            84,
            "14 isolated cells, no culling, no merging"
        );
        assert!((total_area(&data) - 84.0).abs() < 1e-4);
    }

    /// The mesher is pure: the same document meshed twice yields a byte-identical stream.
    #[test]
    fn double_run_is_bit_identical() {
        let model = doc(
            [3, 2, 3],
            vec![entry("A", RED), entry("B", GREEN)],
            &[
                &["A.B", "BBA", ".A."], // y = 0
                &["AB.", "..A", "B.B"], // y = 1
            ],
        );
        let first = mesh(&model, 0xABCD);
        let second = mesh(&model, 0xABCD);
        assert_eq!(first, second);
    }

    /// The hash threads through untouched (the GPU mesh-cache key), like `mesh_asset::decode`.
    #[test]
    fn hash_is_preserved() {
        let model = doc([1, 1, 1], vec![entry("A", RED)], &[&["A"]]);
        assert_eq!(mesh(&model, 0xDEAD_BEEF).hash, 0xDEAD_BEEF);
    }

    /// The bounds-checked grid lookup reads out-of-bounds — negative or beyond the extent — as empty
    /// (`None`), the chunked-voxel-worlds seam. In-bounds cells read their palette index.
    #[test]
    fn grid_reads_out_of_bounds_as_empty() {
        let model = doc([2, 1, 1], vec![entry("A", RED)], &[&["A."]]);
        let grid = Grid::from_doc(&model);
        assert_eq!(grid.at([0, 0, 0]), Some(0)); // filled
        assert_eq!(grid.at([1, 0, 0]), None); // empty glyph
        assert_eq!(grid.at([-1, 0, 0]), None); // out of bounds (below)
        assert_eq!(grid.at([2, 0, 0]), None); // out of bounds (above x)
        assert_eq!(grid.at([0, 5, 0]), None); // out of bounds (above y)
        assert_eq!(grid.at([0, 0, -3]), None); // out of bounds (below z)
    }

    /// D15 grid-vs-mesh agreement (`Plan/gi-wave0-voxelizer.md` §12 test 5), on a frozen copy of a
    /// vigil model: `static_grid`'s filled-cell set matches the mesher's occupancy grid
    /// (`Grid::at`) cell for cell, and a filled cell's albedo/emissive equal its palette entry.
    /// `torch-post.voxel.yaml` authors two emissive glyphs (`F`, `f`) alongside a plain and a
    /// metallic one, so the spot-check exercises every material lane, not just the default.
    #[test]
    fn static_grid_agrees_with_mesher_grid_on_committed_model() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/torch-post.voxel.yaml");
        let source = std::fs::read_to_string(&path).expect("the frozen torch-post model reads");
        let model =
            ludion_scene::check_voxel(&source).expect("the frozen torch-post model validates");

        let mesher_grid = Grid::from_doc(&model);
        let grid = static_grid(&model, ludion_assets::content_hash(source.as_bytes()));
        let (colors, materials) = palette_channels(&model);

        assert_eq!(grid.extent, model.size);
        assert_eq!(grid.cells.len(), mesher_grid.cells.len());

        let mut filled = 0usize;
        for (cell, mesher_cell) in grid.cells.iter().zip(mesher_grid.cells.iter()) {
            assert_eq!(
                cell.is_some(),
                mesher_cell.is_some(),
                "occupancy must agree cell-for-cell"
            );
            if let (Some(cell), Some(pi)) = (cell, mesher_cell) {
                filled += 1;
                assert_eq!(cell.albedo, colors[*pi as usize]);
                assert_eq!(cell.emissive, materials[*pi as usize][2]);
            }
        }
        assert!(filled > 0, "the model has at least one filled cell");
        // Not vacuous: torch-post's `F`/`f` glyphs author nonzero emissive, so this exercises the
        // emissive assertion above on real data, not only the unauthored default.
        assert!(
            grid.cells.iter().flatten().any(|c| c.emissive > 0.0),
            "torch-post authors emissive material; the grid must carry it"
        );
    }

    /// The number of quads (each 4 verts / 6 indices).
    fn quad_count(data: &MeshData) -> usize {
        assert_eq!(data.positions.len() % (3 * 4), 0, "4 verts per quad");
        assert_eq!(data.indices.len() % 6, 0, "2 tris per quad");
        data.positions.len() / (3 * 4)
    }

    /// Sum of quad areas: for each quad `v0,v1,v2,v3`, `|(v1−v0) × (v3−v0)|` (the two edges meeting
    /// at `v0`). Invariant across merging, so it equals the total exposed-face count.
    fn total_area(data: &MeshData) -> f32 {
        let point = |i: usize| {
            [
                data.positions[i * 3],
                data.positions[i * 3 + 1],
                data.positions[i * 3 + 2],
            ]
        };
        let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        let cross = |a: [f32; 3], b: [f32; 3]| {
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
        };
        let len = |a: [f32; 3]| (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
        let mut area = 0.0;
        let quads = data.positions.len() / 12;
        for q in 0..quads {
            let v0 = point(q * 4);
            let v1 = point(q * 4 + 1);
            let v3 = point(q * 4 + 3);
            area += len(cross(sub(v1, v0), sub(v3, v0)));
        }
        area
    }

    /// Independently counts exposed unit faces of `doc`: every filled cell's face whose neighbor is
    /// empty or out of bounds. Merging conserves this total, so it must equal [`total_area`].
    fn exposed_faces(model: &VoxelDocument) -> f32 {
        let grid = Grid::from_doc(model);
        let mut count = 0.0;
        for (axis, positive) in FACES {
            let delta = if positive { 1 } else { -1 };
            for x in 0..grid.size[0] {
                for y in 0..grid.size[1] {
                    for z in 0..grid.size[2] {
                        let cell = [x, y, z];
                        if grid.at(cell).is_some() {
                            let mut n = cell;
                            n[axis] += delta;
                            if grid.at(n).is_none() {
                                count += 1.0;
                            }
                        }
                    }
                }
            }
        }
        count
    }

    /// A palette entry authoring all three material channels.
    fn entry_mat(glyph: &str, color: [f64; 4], mat: [f64; 3]) -> PaletteEntry {
        PaletteEntry {
            glyph: glyph.to_string(),
            color,
            roughness: Some(mat[0]),
            metallic: Some(mat[1]),
            emissive: Some(mat[2]),
        }
    }

    // Property strategy: a small random grid over the given two-entry palette (glyphs '.', 'A', 'B').
    fn arb_model_with(palette: Vec<PaletteEntry>) -> impl Strategy<Value = VoxelDocument> {
        (1u32..=4, 1u32..=4, 1u32..=4).prop_flat_map(move |(sx, sy, sz)| {
            let palette = palette.clone();
            let cells = (sx * sy * sz) as usize;
            prop::collection::vec(0u8..3, cells).prop_map(move |glyphs| {
                let mut slices = Vec::with_capacity(sy as usize);
                let mut it = glyphs.iter();
                for _y in 0..sy {
                    let mut layer = Vec::with_capacity(sz as usize);
                    for _z in 0..sz {
                        let row: String = (0..sx)
                            .map(|_| match it.next().unwrap() {
                                1 => 'A',
                                2 => 'B',
                                _ => '.',
                            })
                            .collect();
                        layer.push(row);
                    }
                    slices.push(layer);
                }
                VoxelDocument {
                    name: None,
                    size: [sx, sy, sz],
                    scale: None,
                    generated_from: None,
                    budget_exempt: None,
                    palette: palette.clone(),
                    slices,
                }
            })
        })
    }

    // The default strategy: no palette entry authors any material.
    fn arb_model() -> impl Strategy<Value = VoxelDocument> {
        arb_model_with(vec![entry("A", RED), entry("B", GREEN)])
    }

    // A strategy whose two palette entries author distinct materials — the authored-material path.
    fn arb_material_model() -> impl Strategy<Value = VoxelDocument> {
        arb_model_with(vec![
            entry_mat("A", RED, [0.2, 1.0, 3.0]),
            entry_mat("B", GREEN, [0.8, 0.0, 0.0]),
        ])
    }

    proptest! {
        /// The stats never disagree with the stream they summarize, on any grid.
        #[test]
        fn stats_agree_with_the_stream(model in arb_model()) {
            let data = mesh(&model, 0);
            let stats = voxel_mesh_stats(&model);
            prop_assert_eq!(stats.vertices, (data.positions.len() / 3) as u64);
            prop_assert_eq!(stats.triangles, (data.indices.len() / 3) as u64);
            prop_assert_eq!(stats.quads * 4, stats.vertices);
            prop_assert_eq!(stats.quads * 2, stats.triangles);
        }

        /// Structural invariants of the emitted stream on any small grid.
        #[test]
        fn stream_is_well_formed(model in arb_model()) {
            let data = mesh(&model, 0);
            let verts = data.positions.len() / 3;
            prop_assert_eq!(data.normals.len(), data.positions.len());
            prop_assert_eq!(data.colors.len(), verts * 4);
            prop_assert_eq!(data.materials.len(), verts * 3);
            prop_assert_eq!(verts % 4, 0, "4 verts per quad");
            prop_assert_eq!(data.indices.len() % 6, 0, "2 tris per quad");
            for &i in &data.indices {
                prop_assert!((i as usize) < verts, "every index is in range");
            }
        }

        /// Every normal is an exact ±1 axis-aligned unit vector — no normalization error.
        #[test]
        fn normals_are_exact_unit_axes(model in arb_model()) {
            let data = mesh(&model, 0);
            for n in data.normals.chunks_exact(3) {
                let nonzero: Vec<f32> = n.iter().copied().filter(|&c| c != 0.0).collect();
                prop_assert_eq!(nonzero.len(), 1, "exactly one nonzero component");
                prop_assert!(nonzero[0] == 1.0 || nonzero[0] == -1.0, "±1 exactly");
            }
        }

        /// Every quad is material-uniform: its four vertices carry one identical palette color.
        #[test]
        fn quads_are_material_uniform(model in arb_model()) {
            let data = mesh(&model, 0);
            let palette: Vec<[f32; 4]> = model
                .palette
                .iter()
                .map(|e| [e.color[0] as f32, e.color[1] as f32, e.color[2] as f32, e.color[3] as f32])
                .collect();
            for quad in data.colors.chunks_exact(16) {
                let first = &quad[0..4];
                prop_assert!(quad.chunks_exact(4).all(|c| c == first), "one color per quad");
                let color: [f32; 4] = [first[0], first[1], first[2], first[3]];
                prop_assert!(palette.contains(&color), "color is a palette entry");
            }
        }

        /// Culling + merge conserve surface area: total quad area equals the independently counted
        /// exposed unit faces.
        #[test]
        fn area_equals_exposed_faces(model in arb_model()) {
            let data = mesh(&model, 0);
            prop_assert!((total_area(&data) - exposed_faces(&model)).abs() < 1e-3);
        }

        /// A palette that authors material emits it per quad, parallel to color: each quad's four
        /// vertices carry one identical `(color, material)` pair drawn from a palette entry, and
        /// `authored_material` is set.
        #[test]
        fn quads_carry_palette_material(model in arb_material_model()) {
            let data = mesh(&model, 0);
            prop_assert!(data.authored_material, "the palette authors material");
            let entries: Vec<([f32; 4], [f32; 3])> = model
                .palette
                .iter()
                .map(|e| {
                    let color = [e.color[0] as f32, e.color[1] as f32, e.color[2] as f32, e.color[3] as f32];
                    let mat = [
                        e.roughness.map_or(1.0, |r| r as f32),
                        e.metallic.map_or(0.0, |m| m as f32),
                        e.emissive.map_or(0.0, |v| v as f32),
                    ];
                    (color, mat)
                })
                .collect();
            for (color_quad, mat_quad) in data.colors.chunks_exact(16).zip(data.materials.chunks_exact(12)) {
                let color: [f32; 4] = color_quad[0..4].try_into().unwrap();
                let mat: [f32; 3] = mat_quad[0..3].try_into().unwrap();
                prop_assert!(mat_quad.chunks_exact(3).all(|m| m == mat), "one material per quad");
                prop_assert!(entries.contains(&(color, mat)), "quad's (color, material) is a palette entry");
            }
        }
    }
}
