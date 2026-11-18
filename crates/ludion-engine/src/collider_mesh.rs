//! The engine's seam behind `Collider.shape: { mesh: <path> }`: the [`ColliderMeshes`] loader that
//! reads a `kind: mesh` document from the content root.
//!
//! Installed at every world (re)build (`Engine::install_collider_meshes`) and by the bare hosts'
//! driver, never snapshotted or hashed. A body remembers the [`TriangleMesh::hash`] it was built
//! from (the document's byte hash), so a restore beside an edited document rebuilds it.
//! `asset.reload` does not reach it: a collider that changes mid-run without a recorded input would
//! break replays.

use ludion_assets::AssetRoot;
use std::path::PathBuf;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_physics::{ColliderMeshes, TriangleMesh};

/// A library that reads mesh documents relative to `root`, the same paths `Mesh.shape: { asset }`
/// takes.
pub(crate) fn library(root: PathBuf) -> ColliderMeshes {
    ColliderMeshes::new(Box::new(move |rel| load(AssetRoot::content(&root), rel)))
}

/// Reads and validates the mesh at `rel`. A document with several faults returns the first as the
/// diagnostic, the rest attached as its `related`.
fn load(root: AssetRoot<'_>, rel: &str) -> Result<TriangleMesh, Diagnostic> {
    let ((document, tags), hash) =
        crate::mesh_asset::with_mesh_source(root, rel, ludion_scene::check_mesh_surfaces).map_err(
            |diagnostics| {
                let mut faults = diagnostics.into_iter();
                let first = faults.next().unwrap_or_else(|| {
                    Diagnostic::new(
                        code::MESH_DOCUMENT_STRUCTURE,
                        format!("mesh asset `{rel}` is not a valid mesh document"),
                    )
                });
                faults.fold(first, Diagnostic::with_related)
            },
        )?;
    let (surfaces, triangle_surfaces) = tags.map_or_else(Default::default, |tags| {
        let per_triangle = tags.per_triangle();
        (tags.names, per_triangle)
    });
    Ok(TriangleMesh {
        vertices: document
            .positions
            .chunks_exact(3)
            .map(|p| [p[0] as f32, p[1] as f32, p[2] as f32])
            .collect(),
        triangles: document
            .indices
            .chunks_exact(3)
            .map(|t| [t[0], t[1], t[2]])
            .collect(),
        hash,
        surfaces,
        triangle_surfaces,
    })
}
