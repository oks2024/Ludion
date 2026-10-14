//! Implementation of `ludion import <input.gltf|.glb|.png> --out <dest> [flags]` — the one
//! foreign-format-in verb (our own text → our own binary is `generate-atlas`).
//!
//! A build-time compiler (like `ludion new`), no engine process, deterministic in both directions:
//! the same input writes byte-identical output.
//!
//! - **glTF**: compiled to an [`Import`] plan (`ludion-import`), emitted through the canonical
//!   writer with the built-in component registry, validated whole, and only then written as
//!   `model.prefab.yaml` + `meshes/*.mesh.yaml` + `clips/*.anim.yaml` + `textures/*.png`. Unsupported
//!   features are `L1202` (hard error by default; warnings under `--allow-lossy`).
//! - **PNG**: decoded and turned into an editable `kind: sprite` document
//!   (`ludion_sprite::import_png`) — lossless while the image's colours fit the glyph budget,
//!   `L4014` when they do not, and `--quantize N` merges them down with the cost reported as
//!   `L4015`.
//!
//! The input's extension picks the path, so the two flag sets are per-format: a flag belonging to
//! the other format is a usage error rather than a silently ignored argument.

use std::path::Path;
use std::process::ExitCode;

use crate::diagnostic;
use ludion_assets::Image;
use ludion_core::atomic::write_atomic;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_import::Import;

/// Runs `ludion import`, routing on the input's extension.
pub fn run(
    input: &Path,
    out: &Path,
    allow_lossy: bool,
    quantize: Option<usize>,
    name: Option<String>,
) -> ExitCode {
    let is_png = input
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("png"));
    if is_png {
        if allow_lossy {
            return diagnostic::usage_error(
                "--allow-lossy downgrades glTF's unsupported-feature errors (L1202) and \
                 means nothing for a PNG; a lossy sprite import is `--quantize N`, which reports \
                 what it cost",
            );
        }
        return run_png(input, out, quantize, name);
    }
    if quantize.is_some() || name.is_some() {
        return diagnostic::usage_error(
            "--quantize and --name apply to a PNG sprite import; a glTF import takes \
             neither (its names come from the glTF's own nodes)",
        );
    }
    run_gltf(input, out, allow_lossy)
}

/// Imports a PNG as a `kind: sprite` document.
fn run_png(input: &Path, out: &Path, quantize: Option<usize>, name: Option<String>) -> ExitCode {
    let bytes = match std::fs::read(input) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!(
                "{}",
                Diagnostic::new(
                    code::ASSET_FILE_IO,
                    format!("could not read `{}`: {error}", input.display()),
                )
                .with_help("check the path (relative to the current directory) and permissions")
            );
            return ExitCode::FAILURE;
        }
    };
    let label = input.display().to_string();
    let (width, height, rgba) = match ludion_assets::decode_png(&bytes, &label) {
        Ok(decoded) => decoded,
        Err(diagnostic) => {
            diagnostic::print(&diagnostic);
            return ExitCode::FAILURE;
        }
    };
    let image = Image {
        width,
        height,
        rgba,
        hash: ludion_assets::content_hash(&bytes),
    };

    let stem = file_stem(input);
    let imported =
        match ludion_sprite::import_png(&image, Some(name.unwrap_or(stem.clone())), quantize) {
            Ok(imported) => imported,
            Err(diagnostic) => {
                diagnostic::print(&diagnostic);
                return ExitCode::FAILURE;
            }
        };

    // `--out` names the document itself, so a sprite import can write beside its siblings; a
    // directory is the convenience case, not the contract.
    let out_path = if out.is_dir() {
        out.join(format!("{stem}.sprite.yaml"))
    } else {
        out.to_path_buf()
    };
    if out_path.exists() {
        diagnostic::print(
            &Diagnostic::new(
                code::CREATE_TARGET_EXISTS,
                format!(
                    "`{}` already exists; `import` never overwrites a sprite document",
                    out_path.display()
                ),
            )
            .with_help("the document is editable: remove it to re-import, or pass another `--out`"),
        );
        return ExitCode::FAILURE;
    }
    let text = ludion_scene::emit_sprite(&imported.doc);
    if let Err(error) = write_atomic(&out_path, &text) {
        eprintln!(
            "{}",
            Diagnostic::new(
                code::OUTPUT_IO,
                format!("could not write `{}`: {error}", out_path.display()),
            )
            .with_help("check the destination is writable")
        );
        return ExitCode::FAILURE;
    }

    println!(
        "{}: wrote {} ({width}x{height}, {} colours)",
        input.display(),
        out_path.display(),
        imported.doc.palette.len(),
    );
    if let Some(report) = imported.quantized {
        diagnostic::print(
            &Diagnostic::new(
                code::SPRITE_IMPORT_QUANTIZED,
                format!(
                    "--quantize merged {} colours down to {}; the largest per-channel change is \
                     {} of 255",
                    report.colors_before, report.colors_after, report.max_channel_error
                ),
            )
            .with_path("/palette")
            .with_help(
                "read the written document and adjust the palette by hand where the merge is \
                 visible, or re-import with a larger `--quantize N`",
            ),
        );
    }
    ExitCode::SUCCESS
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// What a successful glTF import wrote, for the line `run_gltf` and `ludion sync` print.
pub(crate) struct GltfImported {
    pub parts: usize,
    pub meshes: usize,
    pub textures: usize,
    pub clips: usize,
    /// Lossy-feature warnings (only present under `allow_lossy`); never fatal.
    pub warnings: Vec<Diagnostic>,
}

/// Imports a glTF as a `kind: prefab` plus its mesh, clip and texture assets.
fn run_gltf(input: &Path, out: &Path, allow_lossy: bool) -> ExitCode {
    match import_gltf(input, out, allow_lossy) {
        Ok(imported) => {
            for warning in &imported.warnings {
                diagnostic::print(warning);
            }
            println!(
                "{}: imported {} part{}, {} mesh{}, {} texture{}, {} clip{} -> {}",
                input.display(),
                imported.parts,
                if imported.parts == 1 { "" } else { "s" },
                imported.meshes,
                if imported.meshes == 1 { "" } else { "es" },
                imported.textures,
                if imported.textures == 1 { "" } else { "s" },
                imported.clips,
                if imported.clips == 1 { "" } else { "s" },
                out.display(),
            );
            ExitCode::SUCCESS
        }
        Err(diagnostics) => diagnostic::report(input, &diagnostics),
    }
}

/// Compiles `input` and writes its tree under `out`: the whole plan is validated before any file
/// is written.
pub(crate) fn import_gltf(
    input: &Path,
    out: &Path,
    allow_lossy: bool,
) -> Result<GltfImported, Vec<Diagnostic>> {
    let clips_dir = out.join(ludion_import::CLIP_DIR);
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

    let mut import = ludion_import::import(
        &bytes,
        ludion_import::ExternalFiles::Beside(base_dir),
        allow_lossy,
    )?;

    // The importer sees bytes, not paths: the provenance marker is the input relative to the prefab.
    import.prefab.generated_from = Some(
        crate::generate_atlas::relative_to(out, input, code::IMPORT_OUTPUT_IO)
            .map_err(|diagnostic| vec![diagnostic])?,
    );
    // Emit the prefab canonically (registry supplies component/field order), then re-validate the
    // whole plan as `check-scene` does before writing anything — an imported tree that would not
    // check is a bug, caught here rather than written out.
    let registry = ludion_engine::builtin_registry();
    let prefab_yaml = ludion_scene::emit_prefab(&import.prefab, &registry);
    let checked = ludion_scene::check_prefab(&prefab_yaml, &registry)?;
    let body = checked.to_scene();
    for errors in [
        ludion_engine::scene_hierarchy_body_diagnostics(&body),
        ludion_engine::prefab_singleton_diagnostics(&body),
    ] {
        if !errors.is_empty() {
            return Err(errors);
        }
    }
    validate_meshes(&import)?;
    // The importer sees bytes, not paths: the provenance marker is the input relative to `clips/`.
    let source = crate::generate_atlas::relative_to(&clips_dir, input, code::IMPORT_OUTPUT_IO)
        .map_err(|diagnostic| vec![diagnostic])?;
    for clip in &mut import.clips {
        clip.doc.generated_from = Some(source.clone());
    }
    validate_clips(&import)?;

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
