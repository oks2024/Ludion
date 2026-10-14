//! The documents a sculpt's `shape: voxel` and `shape: mesh` parts name, resolved by the caller.
//!
//! This crate does no IO — it is handed a validated document and returns geometry — so a part whose
//! field *is* another document cannot read it. The caller walks [`crate::voxel_paths`] and
//! [`crate::mesh_paths`], reads each file, and records it here under the path the part writes;
//! [`crate::mesh_with`] then meshes with no file access of its own.
//!
//! Both kinds land as one [`SignedDistanceGrid`] per distinct path, `Arc`-shared by every part
//! naming it, so several parts on one source document pay for the field once. A voxel model's grid
//! is the exact distance transform of its occupancy ([`crate::edt`]); a mesh document's is the
//! triangle soup's signed distance ([`crate::soup`]) sampled onto a lattice of its own bounding box.

use std::sync::Arc;

use ludion_core::par;
use ludion_scene::{MeshDocument, VoxelDocument};

use crate::edt::{SignedDistanceGrid, signed_distance};
use crate::soup::TriangleSoup;

/// Empty voxels added on every side of a model's occupancy before the distance transform runs.
///
/// The transform's sample clamps inside the half-voxel band beyond the outermost voxel centres, so
/// a model filled to its own grid boundary would put its surface half a voxel outside the box. One
/// empty voxel of margin keeps every surface interior, where the field ramps between two real
/// centres and the zero level lands exactly on the model's faces.
pub(crate) const PAD: usize = 1;

/// Cells along the longest axis of a mesh source's own bounding box.
///
/// The sculpt lattice a mesh part folds into is `resolution` cells per world unit — `48` by default
/// — so a model-sized mesh sampled this finely carries the detail that lattice can show. It is a
/// bound on the *source*: a mesh part costs the same wherever it sits and however large the model
/// it joins.
const MESH_CELLS: usize = 64;

/// Cells of empty margin on every side of a mesh's box, so the surface stays inside the lattice and
/// the trilinear reconstruction has a cell to ramp through on both sides of it.
const MESH_PAD: usize = 2;

/// Cells one worker takes at a time when sampling a mesh's lattice. Every cell is an independent
/// read of the soup, so the values are identical at any thread count.
const MESH_CHUNK: usize = 4096;

/// One voxel document's field, plus the model extent the padding is measured against.
pub(crate) struct VoxelSource {
    /// The transform of the padded occupancy.
    pub(crate) grid: Arc<SignedDistanceGrid>,
    /// The document's own `size`, without [`PAD`].
    pub(crate) dims: [usize; 3],
}

/// One mesh document's field: the soup's signed distance on a lattice of its own box.
///
/// `origin` is the document-space position of grid coordinate `(0, 0, 0)` and `cell` the document
/// units one grid cell spans, so a document-space point `p` samples at `(p - origin) / cell` and the
/// stored value — in cell units — scales back out by `cell`. `bounds` is the mesh's own box, which
/// the padding ring lies outside of.
pub(crate) struct MeshSource {
    pub(crate) grid: Arc<SignedDistanceGrid>,
    pub(crate) cell: f32,
    pub(crate) origin: [f32; 3],
    pub(crate) bounds: ([f32; 3], [f32; 3]),
}

/// One resolved source document per path a sculpt's `shape: voxel` and `shape: mesh` parts name.
///
/// Insertion order is the caller's; lookup is by the exact path string, so a part and its source
/// agree on spelling the way the file system does not have to. The two kinds are kept apart: a path
/// means whatever the part naming it says it means.
#[derive(Default)]
pub struct SculptSources {
    /// `(path, source)` in insertion order — a short list (one entry per distinct source document),
    /// walked linearly rather than hashed, because a hashed container has no place in this crate.
    voxels: Vec<(String, VoxelSource)>,
    meshes: Vec<(String, MeshSource)>,
}

impl SculptSources {
    /// An empty set — what [`crate::mesh`] meshes against, and therefore what makes a document
    /// carrying a `shape: voxel` or `shape: mesh` part refuse to mesh.
    pub fn new() -> Self {
        SculptSources::default()
    }

    /// Records `doc`'s occupancy as the source for `path`, replacing any earlier entry.
    ///
    /// The distance transform runs here, once per call, so a caller that inserts each distinct path
    /// once pays for each source document once however many parts name it. `doc` must be
    /// [`ludion_scene::check_voxel`]-clean: its `size` is then three positive integers consistent
    /// with its slices, which is what the transform is indexed by.
    pub fn insert_voxel(&mut self, path: impl Into<String>, doc: &VoxelDocument) {
        let path = path.into();
        let dims = doc.size.map(|n| n as usize);
        let padded = dims.map(|n| n + 2 * PAD);
        // `x + padded[0] * (y + padded[1] * z)`, the transform's own index — a cell is occupied when
        // its glyph names a palette entry, the one definition of "filled" every voxel sweep uses.
        let mut occupancy = vec![false; padded[0] * padded[1] * padded[2]];
        doc.for_each_cell(|x, y, z, index| {
            if index.is_some() {
                let [x, y, z] = [x, y, z].map(|c| c as usize + PAD);
                occupancy[x + padded[0] * (y + padded[1] * z)] = true;
            }
        });
        let source = VoxelSource {
            grid: Arc::new(signed_distance(&occupancy, padded)),
            dims,
        };
        match self.voxels.iter_mut().find(|(known, _)| *known == path) {
            Some(entry) => entry.1 = source,
            None => self.voxels.push((path, source)),
        }
    }

    /// Records `doc`'s triangles as the source for `path`, replacing any earlier entry.
    ///
    /// The soup's signed distance is sampled onto a lattice of the mesh's own bounding box here,
    /// once per call: one query is a tree walk, which the fold cannot afford at every lattice corner
    /// of every model the part joins, and the grid makes a mesh part cost exactly what a voxel part
    /// costs to sample. `doc` must be [`ludion_scene::check_mesh`]-clean, so its indices are in
    /// range and its positions finite.
    pub fn insert_mesh(&mut self, path: impl Into<String>, doc: &MeshDocument) {
        let path = path.into();
        let source = mesh_source(doc);
        match self.meshes.iter_mut().find(|(known, _)| *known == path) {
            Some(entry) => entry.1 = source,
            None => self.meshes.push((path, source)),
        }
    }

    /// Whether a voxel source is recorded for `path`.
    pub fn contains_voxel(&self, path: &str) -> bool {
        self.voxel(path).is_some()
    }

    /// Whether a mesh source is recorded for `path`.
    pub fn contains_mesh(&self, path: &str) -> bool {
        self.mesh(path).is_some()
    }

    /// The number of distinct source documents recorded, of both kinds.
    pub fn len(&self) -> usize {
        self.voxels.len() + self.meshes.len()
    }

    /// Whether nothing is recorded.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn voxel(&self, path: &str) -> Option<&VoxelSource> {
        self.voxels
            .iter()
            .find(|(known, _)| known == path)
            .map(|(_, source)| source)
    }

    pub(crate) fn mesh(&self, path: &str) -> Option<&MeshSource> {
        self.meshes
            .iter()
            .find(|(known, _)| known == path)
            .map(|(_, source)| source)
    }
}

/// Samples a mesh document's triangle soup onto a lattice of its own bounding box.
///
/// The cell spans `longest extent / MESH_CELLS`, so a mesh is sampled at the same relative detail
/// whatever its size, and the lattice is bounded at `(MESH_CELLS + 2 · MESH_PAD)³` cells however
/// many triangles the document holds. A document with no triangle at all gets a one-cell positive
/// grid: it contributes no surface rather than refusing to mesh.
fn mesh_source(doc: &MeshDocument) -> MeshSource {
    let Some(soup) = TriangleSoup::new(&doc.positions, &doc.indices) else {
        return MeshSource {
            grid: Arc::new(SignedDistanceGrid::from_values([1; 3], vec![1.0])),
            cell: 1.0,
            origin: [-0.5; 3],
            bounds: ([0.0; 3], [0.0; 3]),
        };
    };

    let (lo, hi) = soup.bounds();
    let extent = [0, 1, 2].map(|axis| hi[axis] - lo[axis]);
    let longest = extent[0].max(extent[1]).max(extent[2]);
    // A flat or single-point soup still has a surface to measure; one unit per cell is as good a
    // scale as any where there is no extent to derive one from.
    let cell = if longest > 0.0 {
        longest / MESH_CELLS as f32
    } else {
        1.0
    };
    let dims = extent.map(|span| (span / cell).ceil() as usize + 2 * MESH_PAD);
    let origin = [0, 1, 2].map(|axis| lo[axis] - MESH_PAD as f32 * cell);

    let mut values = vec![0.0f32; dims[0] * dims[1] * dims[2]];
    par::par_chunks(&mut values, MESH_CHUNK, |chunk_index, chunk| {
        for (offset, value) in chunk.iter_mut().enumerate() {
            let at = chunk_index * MESH_CHUNK + offset;
            let cell_index = [
                at % dims[0],
                (at / dims[0]) % dims[1],
                at / (dims[0] * dims[1]),
            ];
            let centre =
                [0, 1, 2].map(|axis| origin[axis] + (cell_index[axis] as f32 + 0.5) * cell);
            // Stored in cell units, the convention `SignedDistanceGrid::sample` reads and the
            // part's frame scales back out of.
            *value = soup.signed_distance(centre) / cell;
        }
    });

    MeshSource {
        grid: Arc::new(SignedDistanceGrid::from_values(dims, values)),
        cell,
        origin,
        bounds: (lo, hi),
    }
}
