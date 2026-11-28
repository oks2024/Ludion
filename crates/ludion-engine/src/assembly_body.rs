//! Which body a `kind: assembly` document has: `mesh:` present makes it skinned, otherwise it is
//! voxel. Every site that reads `Mesh.shape: { assembly }` routes on this answer.

use ludion_assets::AssetRoot;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_scene::AssemblyDocument;

/// The body of a `kind: assembly` document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AssemblyBody {
    /// Parts built from voxels.
    Voxel,
    /// One skinned mesh document and the clips it binds.
    Skinned,
}

/// Reads and validates the assembly document `rel` (text only: no part, mesh or clip read), returning
/// it with its bytes' content hash.
///
/// # Errors
/// `L1101` if the file cannot be read, else the document's own `L1137`/`L1138` diagnostics.
pub(crate) fn read_document(
    root: AssetRoot<'_>,
    rel: &str,
) -> Result<(AssemblyDocument, u64), Vec<Diagnostic>> {
    let bytes = ludion_assets::load_bytes(root, rel).map_err(|diagnostic| vec![diagnostic])?;
    let source = std::str::from_utf8(&bytes.bytes).map_err(|_| {
        vec![
            Diagnostic::new(
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
                format!("assembly asset `{rel}` is not valid UTF-8 text"),
            )
            .with_help("an assembly document is a strict-YAML text file"),
        ]
    })?;
    let doc = ludion_scene::check_assembly(source)?;
    Ok((doc, bytes.hash))
}

/// The body of the assembly document at `rel`. It reads the file on every call, so a per-frame caller
/// memoizes the answer.
///
/// # Errors
/// [`read_document`]'s.
pub(crate) fn body_of(root: AssetRoot<'_>, rel: &str) -> Result<AssemblyBody, Vec<Diagnostic>> {
    read_document(root, rel).map(|(doc, _)| match doc.mesh {
        Some(_) => AssemblyBody::Skinned,
        None => AssemblyBody::Voxel,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mesh_key_decides_the_body() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-assembly-body")
            .join(std::process::id().to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skinned.assembly.yaml"),
            "format: 1\nkind: assembly\nmesh: skin.mesh.yaml\nclips:\n  idle:\n    clip: idle.anim.yaml\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("voxel.assembly.yaml"),
            "format: 1\nkind: assembly\nname: box\nsize:\n  - 2\n  - 1\n  - 1\npalette:\n  A:\n    color:\n      - 1.0\n      - 0.0\n      - 0.0\n      - 1.0\nparts:\n  - name: body\n    file: body.part.yaml\n    place:\n      - 0\n      - 0\n      - 0\n",
        )
        .unwrap();
        let root = AssetRoot::content(&dir);

        // Neither fixture's mesh, clip or part exists: only the document is read.
        assert_eq!(
            body_of(root, "skinned.assembly.yaml"),
            Ok(AssemblyBody::Skinned)
        );
        assert_eq!(
            body_of(root, "voxel.assembly.yaml"),
            Ok(AssemblyBody::Voxel)
        );
        let missing = body_of(root, "absent.assembly.yaml").unwrap_err();
        assert_eq!(missing[0].code, code::ASSET_FILE_IO);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
