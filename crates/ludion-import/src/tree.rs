use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

use crate::Import;

/// What a successful glTF import wrote, for the line `run_gltf` and `ludion sync` print.
pub struct GltfImported {
    /// Entities in the written prefab.
    pub parts: usize,
    /// Mesh documents written.
    pub meshes: usize,
    /// Texture PNGs written.
    pub textures: usize,
    /// Animation clips written.
    pub clips: usize,
    /// Lossy-feature warnings (only present under `allow_lossy`); never fatal.
    pub warnings: Vec<Diagnostic>,
}

/// Compiles `input` and writes its tree under `out`: the whole plan is validated before any file
/// is written.
///
/// ```no_run
/// use std::path::Path;
///
/// let imported = ludion_import::import_gltf(
///     Path::new("art/crate.glb"),
///     Path::new("assets/crate"),
///     &ludion_import::ImportOptions::default(),
/// );
/// assert!(imported.is_ok());
/// ```
pub fn import_gltf(
    input: &Path,
    out: &Path,
    options: &crate::ImportOptions,
) -> Result<GltfImported, Vec<Diagnostic>> {
    let clips_dir = out.join(crate::CLIP_DIR);
    let bytes = std::fs::read(input).map_err(|error| {
        vec![
            Diagnostic::new(
                code::IMPORT_INVALID_GLTF,
                format!("could not read `{}`: {error}", input.display()),
            )
            .with_help("check the path (relative to the current directory) and permissions"),
        ]
    })?;
    let base_dir = input.parent().unwrap_or(Path::new("."));

    let mut import = crate::import(&bytes, crate::ExternalFiles::Beside(base_dir), options)?;

    // The importer sees bytes, not paths: the provenance marker is the input relative to the prefab.
    import.prefab.generated_from = Some(
        crate::relative_to(out, input, code::IMPORT_OUTPUT_IO)
            .map_err(|diagnostic| vec![diagnostic])?,
    );
    // Emit the prefab canonically (registry supplies component/field order), then re-validate the
    // whole plan as `check-scene` does before writing anything — an imported tree that would not
    // check is a bug, caught here rather than written out.
    let registry = ludion_components::builtin_registry();
    let prefab_yaml = ludion_scene::emit_prefab(&import.prefab, &registry);
    let checked = ludion_scene::check_prefab(&prefab_yaml, &registry)?;
    let body = checked.to_scene();
    for errors in [
        ludion_components::validators::scene_hierarchy_body_diagnostics(&body),
        ludion_components::validators::prefab_singleton_diagnostics(&body),
    ] {
        if !errors.is_empty() {
            return Err(errors);
        }
    }
    validate_meshes(&import)?;
    // The importer sees bytes, not paths: the provenance marker is the input relative to `clips/`.
    let source = crate::relative_to(&clips_dir, input, code::IMPORT_OUTPUT_IO)
        .map_err(|diagnostic| vec![diagnostic])?;
    for clip in &mut import.clips {
        clip.doc.generated_from = Some(source.clone());
    }
    validate_clips(&import)?;

    let stale = import.stale_outputs(out);
    if !stale.is_empty() {
        return Err(vec![
            Diagnostic::new(
                code::CREATE_TARGET_EXISTS,
                format!(
                    "`{}` holds files this import would not write: {}",
                    out.display(),
                    stale.join(", ")
                ),
            )
            .with_help(format!(
                "a re-import must not leave stale files beside the new tree: delete `{}` and \
                 import again (an `[[import]]` table under `ludion sync` does this for you)",
                out.display()
            )),
        ]);
    }

    import
        .write(out, &prefab_yaml)
        .map_err(|diagnostic| vec![diagnostic])?;

    Ok(GltfImported {
        parts: import.prefab.entities.len(),
        meshes: import.meshes.len(),
        textures: import.textures.len(),
        clips: import.clips.len(),
        warnings: std::mem::take(&mut import.warnings),
    })
}

/// Re-validates every emitted clip document, `generated_from` included.
fn validate_clips(import: &Import) -> Result<(), Vec<Diagnostic>> {
    for clip in &import.clips {
        let text = ludion_scene::emit_animation(&clip.doc);
        ludion_scene::check_animation(&text)?;
    }
    Ok(())
}

/// Re-validates every emitted mesh document (defensive — the importer builds them valid).
fn validate_meshes(import: &Import) -> Result<(), Vec<Diagnostic>> {
    for mesh in &import.meshes {
        let text = ludion_scene::emit_mesh(&mesh.doc);
        ludion_scene::check_mesh(&text)?;
    }
    Ok(())
}
