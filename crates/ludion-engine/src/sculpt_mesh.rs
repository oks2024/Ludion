//! Meshing a `Mesh.shape: sculpt` document into renderer-ready [`MeshData`].
//!
//! A sculpt is a strict-YAML `sculpts/*.sculpt.yaml` document ([`ludion_scene::check_sculpt`]): named
//! primitive parts folded into one signed distance field. This module is the engine-side seam — the
//! sculpt sibling of [`crate::voxel_mesh`], with the same `load(root, rel)` signature: read the bytes
//! ([`ludion_assets`]), parse + validate ([`ludion_scene`]), mesh ([`ludion_sculpt::mesh_with`]), then
//! move the flat lists into [`MeshData`]. The mesher already emits the renderer's layout,
//! bottom-centered in world units, so nothing is re-derived here.
//!
//! The mesher is a pure crate, so the one thing it cannot do for itself is read the document a
//! `shape: voxel` or `shape: mesh` part names: [`read_sculpt_sources`] is that half, and it is what
//! makes this module the sculpt path's only file reader.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_renderer::MeshData;
use ludion_scene::{MESH_VERTEX_BUDGET, SculptDocument, SculptShape};
use ludion_sculpt::{SculptMesh, SculptSources};

use crate::asset_cache::{SourceFiles, fold_source_hashes};

pub use ludion_sculpt::SculptStats;

/// Reads, validates, and meshes the sculpt document at `root`/`rel` into [`MeshData`].
///
/// # Errors
/// - `L1101` if the file cannot be read.
/// - `L1156` if the bytes are not valid UTF-8 text, or the document is structurally malformed.
/// - `L1157` if the document's data is inconsistent, or its lattice exceeds the mesher's cap.
/// - `L1160` if its bone tree is invalid.
///
/// All diagnostics carry a JSON Pointer into the *sculpt document*; the caller
/// ([`extract`](crate::extract)) re-points them at the referencing `Mesh` component.
#[cfg(test)]
pub(crate) fn load(
    root: ludion_assets::AssetRoot<'_>,
    rel: &str,
) -> Result<MeshData, Vec<Diagnostic>> {
    load_with_sources(root, rel).map(|(data, _)| data)
}

/// Reads and validates the sculpt document at `root`/`rel`, returning it with its bytes' content hash.
pub(crate) fn read_document(
    root: ludion_assets::AssetRoot<'_>,
    rel: &str,
) -> Result<(SculptDocument, u64), Vec<Diagnostic>> {
    let bytes = ludion_assets::load_bytes(root, rel).map_err(|diagnostic| vec![diagnostic])?;
    let source = std::str::from_utf8(&bytes.bytes).map_err(|_| {
        vec![
            Diagnostic::new(
                code::SCULPT_DOCUMENT_STRUCTURE,
                format!("sculpt asset `{rel}` is not valid UTF-8 text"),
            )
            .with_help("a sculpt document is a strict-YAML text file (`sculpts/*.sculpt.yaml`)"),
        ]
    })?;
    let doc = ludion_scene::check_sculpt(source)?;
    Ok((doc, bytes.hash))
}

/// [`load`], additionally reporting **every file it read** — the sculpt document first, then each
/// distinct `shape: voxel` / `shape: mesh` part document, as `root`-relative paths, each with the
/// content hash of the bytes it decoded. That list is the
/// sculpt's reload dependency set ([`crate::asset_cache`]), and those files' hashes are folded into
/// [`MeshData::hash`], so an edited part re-keys the GPU mesh even when the sculpt is untouched.
pub(crate) fn load_with_sources(
    root: ludion_assets::AssetRoot<'_>,
    rel: &str,
) -> Result<(MeshData, SourceFiles), Vec<Diagnostic>> {
    let (doc, doc_hash) = read_document(root, rel)?;
    let (sources, files) = read_sculpt_sources_with_files(root, rel, &doc)?;
    let mesh = ludion_sculpt::mesh_with(&doc, &sources).map_err(|diagnostic| vec![diagnostic])?;
    let part_hashes: Vec<u64> = files.iter().map(|(_, hash)| *hash).collect();
    let hash = fold_source_hashes(doc_hash, &part_hashes);
    let read = std::iter::once((rel.to_string(), doc_hash))
        .chain(files)
        .collect();
    Ok((mesh_data(mesh, hash), read))
}

/// Reads the document behind every `shape: voxel` and `shape: mesh` part of `doc`, the sculpt at
/// `root`/`rel`.
///
/// `ludion-sculpt` is a pure crate, so a part whose field is another document is handed that
/// document rather than reading it. A part's `path` resolves **relative to the sculpt document's own
/// directory** — the rule a `kind: assembly` reads its `file:` parts by — through the same
/// [`ludion_assets::load_bytes`] seam a scene's `Mesh.shape: voxel` path takes from its content
/// root. Each distinct path is read once, whatever number of parts name it.
///
/// # Errors
/// `L1101` for a source that cannot be read, and the source document's own structure/data codes for
/// one that does not check (`L1113`/`L1114` for a voxel model, `L1110`/`L1111` for a mesh) — each
/// re-pointed at `/parts/<i>/path` of the *sculpt*, the document whose author can act on it.
pub fn read_sculpt_sources(
    root: ludion_assets::AssetRoot<'_>,
    rel: &str,
    doc: &SculptDocument,
) -> Result<SculptSources, Vec<Diagnostic>> {
    read_sculpt_sources_with_files(root, rel, doc).map(|(sources, _)| sources)
}

/// [`read_sculpt_sources`], additionally returning each distinct file it read as a `(resolved path,
/// content hash)` pair, in document order.
pub(crate) fn read_sculpt_sources_with_files(
    root: ludion_assets::AssetRoot<'_>,
    rel: &str,
    doc: &SculptDocument,
) -> Result<(SculptSources, SourceFiles), Vec<Diagnostic>> {
    let mut sources = SculptSources::new();
    let mut files = Vec::new();
    let mut errors = Vec::new();
    for (index, part) in doc.parts.iter().enumerate() {
        let at_part = |diagnostics: Vec<Diagnostic>| {
            diagnostics
                .into_iter()
                .map(|diagnostic| diagnostic.with_path(format!("/parts/{index}/path")))
        };
        match &part.shape {
            SculptShape::Voxel { path, .. } if !sources.contains_voxel(path) => {
                let resolved = ludion_core::paths::sibling(rel, path);
                match crate::voxel_mesh::read_document(root, &resolved) {
                    Ok((voxel, hash)) => {
                        sources.insert_voxel(path.clone(), &voxel);
                        files.push((resolved, hash));
                    }
                    Err(diagnostics) => errors.extend(at_part(diagnostics)),
                }
            }
            SculptShape::Mesh { path, .. } if !sources.contains_mesh(path) => {
                let resolved = ludion_core::paths::sibling(rel, path);
                match crate::mesh_asset::read_document(root, &resolved) {
                    Ok((mesh, hash)) => {
                        sources.insert_mesh(path.clone(), &mesh);
                        files.push((resolved, hash));
                    }
                    Err(diagnostics) => errors.extend(at_part(diagnostics)),
                }
            }
            _ => {}
        }
    }
    if errors.is_empty() {
        Ok((sources, files))
    } else {
        Err(errors)
    }
}

/// Moves a [`SculptMesh`]'s flat lists into [`MeshData`]. Every vertex carries a palette-blended
/// material, so `authored_material` is always set. The per-part weights stay behind: `MeshData` has
/// no channel for them. An implicit surface has no parameterization, so every vertex takes the
/// unmapped default UV `(0, 0)` and the placeholder tangent frame `(+X, +1)`. `hash` is the folded
/// content hash of the document and every part file it read (the GPU mesh-cache key).
fn mesh_data(mesh: SculptMesh, hash: u64) -> MeshData {
    let vertex_count = mesh.positions.len() / 3;
    MeshData {
        positions: mesh.positions,
        normals: mesh.normals,
        colors: mesh.colors,
        materials: mesh.materials,
        uvs: vec![0.0; vertex_count * 2],
        tangents: [1.0, 0.0, 0.0, 1.0].repeat(vertex_count),
        weights: [1.0, 0.0, 0.0, 0.0].repeat(vertex_count),
        bone_indices: vec![0; vertex_count * 4],
        skin_joints: Vec::new(),
        textures: None,
        authored_material: true,
        base_color_mean: [1.0; 4],
        double_sided: false,
        indices: mesh.indices,
        hash,
    }
}

/// The non-fatal style-budget warning (`L1112`) for a sculpt whose meshed vertex count (`stats`, from
/// `ludion_sculpt::mesh` on the same `doc`) exceeds the mesh budget. `None` when under budget or when the
/// document sets `budget_exempt: true`.
pub fn sculpt_budget_warning(doc: &SculptDocument, stats: &SculptStats) -> Option<Diagnostic> {
    let vertices = stats.vertices as usize;
    if doc.budget_exempt == Some(true) || vertices <= MESH_VERTEX_BUDGET {
        return None;
    }
    Some(
        Diagnostic::new(
            code::MESH_VERTEX_BUDGET_EXCEEDED,
            format!(
                "this sculpt meshes to {vertices} vertices, above the {MESH_VERTEX_BUDGET}-vertex \
                 style budget"
            ),
        )
        .with_path("/triangles")
        .with_help(
            "lower `triangles` (the decimation target), or set `budget_exempt: true` if the \
             density is intended — this is a warning, not an error, and does not block validation",
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One sphere part, small enough to mesh quickly.
    const BALL: &str = "\
format: 1
kind: sculpt
resolution: 16
triangles: 200
palette:
  S:
    color:
      - 0.5
      - 0.5
      - 0.5
      - 1.0
bones:
  - name: root
parts:
  - name: ball
    shape: sphere
    radius: 0.5
    material: S
";

    /// A sculpt on disk loads to a non-empty mesh in the renderer's layout, flagged as authoring its
    /// own material, keyed by the source bytes' content hash.
    #[test]
    fn sculpt_loads_to_mesh_data_keyed_by_content_hash() {
        let dir = std::env::temp_dir().join(format!("ludion-sculpt-mesh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sculpts")).unwrap();
        let rel = "sculpts/ball.sculpt.yaml";
        std::fs::write(dir.join(rel), BALL).unwrap();

        let data =
            load(ludion_assets::AssetRoot::content(&dir), rel).unwrap_or_else(|d| panic!("{d:?}"));
        let vertices = data.positions.len() / 3;
        assert!(vertices > 0 && !data.indices.is_empty());
        assert_eq!(data.normals.len(), vertices * 3);
        assert_eq!(data.colors.len(), vertices * 4);
        assert_eq!(data.materials.len(), vertices * 3);
        assert!(data.authored_material);
        assert_eq!(data.hash, ludion_assets::content_hash(BALL.as_bytes()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two byte-identical sculpt documents in different folders naming different part files are two
    /// different meshes, so their hashes — the GPU mesh-cache key — must differ.
    #[test]
    fn identical_sculpts_over_different_part_files_hash_apart() {
        let dir = std::env::temp_dir().join(format!("ludion-sculpt-alias-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sculpt = BALL.replace(
            "    shape: sphere
    radius: 0.5
",
            "    shape: voxel
    path: hull.voxel.yaml
    size: 0.5
",
        );
        let voxel = |glyph: &str| {
            format!(
                "format: 1
kind: voxel
size:
  - 1
  - 1
  - 1
palette:
  {glyph}:
    color:
      - 1.0
      - 0.5
      - 0.2
      - 1.0
slices:
  -
    - \"{glyph}\"
"
            )
        };
        for (folder, glyph) in [("a", "A"), ("b", "B")] {
            std::fs::create_dir_all(dir.join(folder)).unwrap();
            std::fs::write(dir.join(folder).join("x.sculpt.yaml"), &sculpt).unwrap();
            std::fs::write(dir.join(folder).join("hull.voxel.yaml"), voxel(glyph)).unwrap();
        }

        let a = load(ludion_assets::AssetRoot::content(&dir), "a/x.sculpt.yaml")
            .unwrap_or_else(|d| panic!("{d:?}"));
        let b = load(ludion_assets::AssetRoot::content(&dir), "b/x.sculpt.yaml")
            .unwrap_or_else(|d| panic!("{d:?}"));
        assert_ne!(a.hash, b.hash);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A broken document reports the scene layer's diagnostics, and a missing one `L1101`.
    #[test]
    fn sculpt_load_reports_document_and_io_errors() {
        let dir =
            std::env::temp_dir().join(format!("ludion-sculpt-mesh-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sculpts")).unwrap();
        std::fs::write(
            dir.join("sculpts/bad.sculpt.yaml"),
            BALL.replace("material: S", "material: Z"),
        )
        .unwrap();

        let errors = load(
            ludion_assets::AssetRoot::content(&dir),
            "sculpts/bad.sculpt.yaml",
        )
        .unwrap_err();
        assert!(
            errors.iter().all(|d| d.code == code::SCULPT_DOCUMENT_DATA),
            "{errors:?}"
        );
        let missing = load(
            ludion_assets::AssetRoot::content(&dir),
            "sculpts/nope.sculpt.yaml",
        )
        .unwrap_err();
        assert_eq!(missing[0].code.as_str(), "L1101");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The stats count the stream `load` emits, and the budget warning fires on the meshed vertex
    /// count unless the document is exempt.
    #[test]
    fn budget_warning_follows_the_meshed_count() {
        let mut doc = ludion_scene::check_sculpt(BALL).unwrap();
        let mesh = ludion_sculpt::mesh(&doc).unwrap();
        let mut stats = mesh.stats.clone();
        assert_eq!(stats.vertices as usize, mesh.positions.len() / 3);
        assert!(sculpt_budget_warning(&doc, &stats).is_none());

        stats.vertices = (MESH_VERTEX_BUDGET + 1) as u32;
        let warning = sculpt_budget_warning(&doc, &stats).expect("over budget");
        assert_eq!(warning.code, code::MESH_VERTEX_BUDGET_EXCEEDED);
        doc.budget_exempt = Some(true);
        assert!(sculpt_budget_warning(&doc, &stats).is_none());
    }
}
