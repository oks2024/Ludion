//! Implementation of `ludion generate-atlas <source.{atlas,font}-source.yaml>... [--out-dir <dir>]
//! [--check]`.
//!
//! A build-time compiler (like `ludion generate-map`), no engine process. It bakes two source
//! kinds to the same committed-pair shape (`<stem>.png` beside a derived document): a
//! `kind: font-source` is read with its TTF/OTF (resolved beside the source) and baked by
//! [`ludion_font::bake::bake`] into `<stem>.font.yaml`; a `kind: atlas-source` document is the
//! sprite path below.
//!
//! The atlas path reads a `kind: atlas-source` document, resolves each `inputs:` entry **relative to the source file's
//! directory**, bakes every sprite document and decodes every loose PNG, hands them to the pure,
//! IO-free [`ludion_sprite::pack`], and writes the committed pair — `<stem>.png` beside
//! `<stem>.atlas.yaml`. Deterministic: the same inputs write byte-identical files, which is the
//! property the drift gate (`scripts/atlas-drift.mjs`) leans on.
//!
//! **Why the self-check re-`check_atlas`es its own output:** an atlas document is derived and
//! write-only for authors, so nothing downstream would ever catch a non-canonical emit — the drift
//! gate would just fail forever on a file no edit could fix. Re-parsing and re-emitting before the
//! write proves the pair is canonical and re-readable, and refuses to write when it is not.

use std::path::{Component, Path, PathBuf, Prefix, PrefixComponent};
use std::process::ExitCode;

use ludion_assets::Image;
use ludion_core::atomic::write_all_atomic;
use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};
use ludion_scene::{AtlasSourceDocument, FontMode};
use ludion_sprite::{AtlasInput, AtlasInputFrame, FrameImage};

use crate::diagnostic;

/// One baked source: the exact bytes of both committed files, where they go, and the one-line
/// description `generate` prints.
struct Baked {
    yaml: String,
    png: Vec<u8>,
    yaml_path: PathBuf,
    png_path: PathBuf,
    summary: String,
    /// The code a drifted file is reported under.
    drift_code: DiagnosticCode,
}

/// Runs `ludion generate-atlas <sources>... [--out-dir dir] [--check]`: bake each source in turn,
/// aggregating the exit code (non-zero if any input fails). Unlike `generate-map`'s `--out`, the
/// destination is a *directory*, so it stays meaningful with several inputs.
pub fn run(sources: &[PathBuf], out_dir: Option<&Path>, check: bool) -> ExitCode {
    let mut failed = false;
    for source in sources {
        let code = if check {
            check_one(source, out_dir)
        } else {
            generate_one(source, out_dir)
        };
        if code == ExitCode::FAILURE {
            failed = true;
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Bakes one source and writes both committed files.
fn generate_one(source_path: &Path, out_dir: Option<&Path>) -> ExitCode {
    let baked = match bake_source(source_path, out_dir) {
        Ok(baked) => baked,
        Err(code) => return code,
    };

    // The document that names the image goes last, so a kill never leaves it naming a missing file.
    if let Err(error) = write_all_atomic(&[
        (&baked.png_path, baked.png.as_slice()),
        (&baked.yaml_path, baked.yaml.as_bytes()),
    ]) {
        eprintln!(
            "{}",
            Diagnostic::new(
                code::OUTPUT_IO,
                format!(
                    "could not write `{}` and `{}`: {error}",
                    baked.png_path.display(),
                    baked.yaml_path.display()
                ),
            )
            .with_help("check the destination is writable")
        );
        return ExitCode::FAILURE;
    }

    println!(
        "{}: wrote {} + {} ({})",
        source_path.display(),
        baked.yaml_path.display(),
        baked.png_path.display(),
        baked.summary,
    );
    ExitCode::SUCCESS
}

/// Re-bakes one source in memory and byte-compares **both** committed files against it — the
/// mid-loop form of the CI drift gate. Writes nothing; exits non-zero if either file drifted or is
/// missing.
fn check_one(source_path: &Path, out_dir: Option<&Path>) -> ExitCode {
    let baked = match bake_source(source_path, out_dir) {
        Ok(baked) => baked,
        Err(code) => return code,
    };

    let mut drifted = Vec::new();
    for (path, bytes) in [
        (&baked.yaml_path, baked.yaml.as_bytes()),
        (&baked.png_path, baked.png.as_slice()),
    ] {
        match std::fs::read(path) {
            Ok(existing) if existing == bytes => {}
            Ok(_) => drifted.push(drift(
                baked.drift_code,
                path,
                "the committed file differs from what this source now bakes",
            )),
            Err(error) => drifted.push(drift(
                baked.drift_code,
                path,
                &format!("could not read the committed file to compare: {error}"),
            )),
        }
    }

    if drifted.is_empty() {
        println!(
            "{}: up to date -> {}, {}",
            source_path.display(),
            baked.yaml_path.display(),
            baked.png_path.display(),
        );
        return ExitCode::SUCCESS;
    }
    diagnostic::report(source_path, &drifted)
}

/// The `DRIFT <path>` diagnostic, one per differing or unreadable committed file. Reuses the baked
/// format's own structure code so `ludion explain` answers for the gate that raised it.
fn drift(code: DiagnosticCode, path: &Path, reason: &str) -> Diagnostic {
    Diagnostic::new(code, format!("DRIFT {}: {reason}", path.display()))
        .with_help("run `ludion generate-atlas` (without --check) to regenerate it")
}

/// Reads a source, dispatches on its `kind:` and bakes it. Shared by `generate_one` (which writes
/// the pair) and `check_one` (which compares it).
fn bake_source(source_path: &Path, out_dir: Option<&Path>) -> Result<Baked, ExitCode> {
    let text = match std::fs::read_to_string(source_path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!(
                "{}",
                Diagnostic::new(
                    code::SCENE_IO,
                    format!("could not read `{}`: {error}", source_path.display()),
                )
                .with_help("check the path (relative to the current directory) and permissions")
            );
            return Err(ExitCode::FAILURE);
        }
    };
    match ludion_scene::peek_kind(&text).as_deref() {
        Some("atlas-source") => bake_atlas_source(&text, source_path, out_dir),
        Some("font-source") => bake_font_source(&text, source_path, out_dir),
        other => {
            let found = other.map_or_else(
                || "a document with no `kind:`".to_string(),
                |kind| format!("`kind: {kind}`"),
            );
            let diagnostic = Diagnostic::new(
                code::DOC_ENVELOPE_INVALID,
                format!(
                    "generate-atlas bakes `kind: atlas-source` or `kind: font-source`; this \
                     document is {found}"
                ),
            )
            .with_path("/kind")
            .with_help("point `generate-atlas` at a `*.atlas-source.yaml` or `*.font-source.yaml`");
            Err(diagnostic::report(source_path, &[diagnostic]))
        }
    }
}

/// Loads an atlas source and everything it references, packs it, and canonically emits the atlas
/// document, running the parse→emit idempotence self-check.
fn bake_atlas_source(
    text: &str,
    source_path: &Path,
    out_dir: Option<&Path>,
) -> Result<Baked, ExitCode> {
    let doc = ludion_scene::check_atlas_source(text)
        .map_err(|diagnostics| diagnostic::report(source_path, &diagnostics))?;

    let source_dir = source_path.parent().unwrap_or_else(|| Path::new("."));
    let dir = out_dir.map_or_else(|| source_dir.to_path_buf(), Path::to_path_buf);
    let stem = output_stem(&doc, source_path);
    let generated_from = relative_to(&dir, source_path, code::OUTPUT_IO)
        .map_err(|diagnostic| diagnostic::report(source_path, &[diagnostic]))?;

    let inputs = load_inputs(&doc, source_dir)
        .map_err(|diagnostics| diagnostic::report(source_path, &diagnostics))?;
    let bake = ludion_sprite::pack(&doc, &generated_from, &inputs)
        .map_err(|diagnostics| diagnostic::report(source_path, &diagnostics))?;

    let yaml = ludion_scene::emit_atlas(&bake.doc);
    if let Err(diagnostics) = self_check_idempotent(&yaml) {
        return Err(diagnostic::report(source_path, &diagnostics));
    }

    Ok(Baked {
        yaml_path: dir.join(format!("{stem}.atlas.yaml")),
        // `pack` derives the colour map's file name from the same stem, so writing the PNG anywhere
        // else would commit a document naming a file that is not there.
        png_path: dir.join(format!("{stem}.png")),
        summary: format!(
            "{}x{}, {} regions",
            bake.image.width,
            bake.image.height,
            bake.doc.regions.len()
        ),
        png: bake.png,
        yaml,
        drift_code: code::SPRITE_DOCUMENT_STRUCTURE,
    })
}

/// Bakes a font source: reads its face beside the source, runs the pure bake, emits the font
/// document and self-checks it like the atlas path.
fn bake_font_source(
    text: &str,
    source_path: &Path,
    out_dir: Option<&Path>,
) -> Result<Baked, ExitCode> {
    let doc = ludion_scene::check_font_source(text)
        .map_err(|diagnostics| diagnostic::report(source_path, &diagnostics))?;

    let source_dir = source_path.parent().unwrap_or_else(|| Path::new("."));
    let dir = out_dir.map_or_else(|| source_dir.to_path_buf(), Path::to_path_buf);
    let stem = match doc.name.as_deref() {
        Some(name) => name.to_string(),
        None => document_stem(source_path, ".font-source"),
    };
    let generated_from = relative_to(&dir, source_path, code::OUTPUT_IO)
        .map_err(|diagnostic| diagnostic::report(source_path, &[diagnostic]))?;

    let face_path = source_dir.join(&doc.file);
    let bytes = std::fs::read(&face_path).map_err(|error| {
        let diagnostic = Diagnostic::new(
            code::FONT_FILE,
            format!(
                "could not read the font file `{}`: {error}",
                face_path.display()
            ),
        )
        .with_path("/file")
        .with_help("the TTF/OTF is resolved beside the font source");
        diagnostic::report(source_path, &[diagnostic])
    })?;

    let bake = ludion_font::bake::bake(&doc, &bytes, &generated_from, &stem)
        .map_err(|diagnostics| diagnostic::report(source_path, &diagnostics))?;
    let yaml = ludion_scene::emit_font(&bake.doc);
    if let Err(diagnostics) = self_check_font_idempotent(&yaml) {
        return Err(diagnostic::report(source_path, &diagnostics));
    }

    let mode = match bake.doc.mode {
        FontMode::Sdf => "sdf",
        FontMode::Bitmap => "bitmap",
    };
    Ok(Baked {
        yaml_path: dir.join(format!("{stem}.font.yaml")),
        png_path: dir.join(format!("{stem}.png")),
        summary: format!(
            "{}x{} {mode}, {} glyphs, {} kern pairs, {} missing",
            bake.image.width,
            bake.image.height,
            bake.doc.glyphs.len(),
            bake.doc.kerning.len(),
            bake.doc.missing.len(),
        ),
        png: bake.png,
        yaml,
        drift_code: code::FONT_DOCUMENT_STRUCTURE,
    })
}

/// [`self_check_idempotent`] for a font document.
fn self_check_font_idempotent(yaml: &str) -> Result<(), Vec<Diagnostic>> {
    let reparsed = ludion_scene::check_font(yaml)?;
    if ludion_scene::emit_font(&reparsed) != yaml {
        return Err(vec![
            Diagnostic::new(
                code::FONT_DOCUMENT_STRUCTURE,
                "the generated font document did not survive a parse→emit round-trip (an internal \
                 bake invariant failed); neither file was written",
            )
            .with_help("this is a bug in the font bake — please report it"),
        ]);
    }
    Ok(())
}

/// Re-parses the emitted atlas document and asserts it re-emits byte-for-byte, proving the packer
/// produced canonical, re-readable output. Defensive: on a mismatch (or an unparseable emit) it
/// returns a diagnostic rather than writing a file the drift gate would then reject forever.
fn self_check_idempotent(yaml: &str) -> Result<(), Vec<Diagnostic>> {
    let reparsed = ludion_scene::check_atlas(yaml)?;
    if ludion_scene::emit_atlas(&reparsed) != yaml {
        return Err(vec![
            Diagnostic::new(
                code::SPRITE_DOCUMENT_STRUCTURE,
                "the generated atlas document did not survive a parse→emit round-trip (an internal \
                 packer invariant failed); neither file was written",
            )
            .with_help("this is a bug in the atlas packer — please report it"),
        ]);
    }
    Ok(())
}

/// Resolves every `inputs:` entry against the source file's directory and turns it into packer
/// input: a `.yaml`/`.yml` sprite document contributes one frame per baked frame, a `.png`
/// contributes its whole image as one frame pivoted at its centre.
///
/// Every defect is collected, not just the first — one run names every broken input.
fn load_inputs(
    doc: &AtlasSourceDocument,
    source_dir: &Path,
) -> Result<Vec<AtlasInput>, Vec<Diagnostic>> {
    let mut inputs = Vec::with_capacity(doc.inputs.len());
    let mut diagnostics = Vec::new();

    for (index, entry) in doc.inputs.iter().enumerate() {
        let path = source_dir.join(entry);
        let pointer = format!("/inputs/{index}");
        let extension = path
            .extension()
            .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();

        let loaded = match extension.as_str() {
            "yaml" | "yml" => load_sprite(&path, &pointer),
            "png" => load_png(&path, &pointer),
            _ => Err(vec![unsupported_input(&path, &pointer)]),
        };
        match loaded {
            Ok(input) => inputs.push(input),
            Err(mut errors) => diagnostics.append(&mut errors),
        }
    }

    if diagnostics.is_empty() {
        Ok(inputs)
    } else {
        Err(diagnostics)
    }
}

/// Bakes one `kind: sprite` input. Its diagnostics point into the *sprite* document, not the atlas
/// source, so each message is prefixed with the input file that raised it — otherwise a pointer
/// like `/frames/idle` would read as a pointer into the source being compiled.
fn load_sprite(path: &Path, pointer: &str) -> Result<AtlasInput, Vec<Diagnostic>> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| vec![missing_input(path, pointer, &error)])?;
    let doc =
        ludion_scene::check_sprite(&text).map_err(|diagnostics| from_input(path, diagnostics))?;
    // A `palette: { from }` include is resolved into a concrete inline palette before the bake sees
    // it — so an included and an equivalent inline sprite bake to byte-identical frames, and the bake
    // stays unaware includes exist. The resolved document faces `bake`'s own full re-validation next.
    let doc = ludion_engine::resolve_sprite_palette(path, doc)
        .map_err(|diagnostics| from_input(path, diagnostics))?;
    let frames = ludion_sprite::bake(&doc).map_err(|diagnostics| from_input(path, diagnostics))?;

    let base = doc
        .name
        .as_deref()
        .filter(|name| !name.is_empty())
        .map_or_else(|| document_stem(path, ".sprite"), str::to_string);
    Ok(AtlasInput {
        base,
        frames: frames
            .into_iter()
            .map(|frame| AtlasInputFrame {
                name: Some(frame.name),
                image: frame.image,
                pivot: frame.pivot,
            })
            .collect(),
    })
}

/// Decodes one loose-PNG input: its whole image is a single region named by the file stem, pivoted
/// at its centre (the packer's default anchor — a PNG carries no authored pivot).
fn load_png(path: &Path, pointer: &str) -> Result<AtlasInput, Vec<Diagnostic>> {
    let bytes = std::fs::read(path).map_err(|error| vec![missing_input(path, pointer, &error)])?;
    let label = path.display().to_string();
    let (width, height, rgba) =
        ludion_assets::decode_png(&bytes, &label).map_err(|error| vec![error])?;
    let image = Image {
        width,
        height,
        rgba,
        hash: ludion_assets::content_hash(&bytes),
    };
    Ok(AtlasInput {
        base: path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default(),
        frames: vec![AtlasInputFrame {
            name: None,
            pivot: [f64::from(width) / 2.0, f64::from(height) / 2.0],
            image: FrameImage::from_srgb8_straight(&image),
        }],
    })
}

/// Re-messages a referenced document's diagnostics with the file that raised them, keeping code,
/// severity, pointer and help intact.
fn from_input(path: &Path, diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
    diagnostics
        .into_iter()
        .map(|mut diagnostic| {
            diagnostic.message = format!("{}: {}", path.display(), diagnostic.message);
            diagnostic
        })
        .collect()
}

fn missing_input(path: &Path, pointer: &str, error: &std::io::Error) -> Diagnostic {
    Diagnostic::new(
        code::SPRITE_DOCUMENT_STRUCTURE,
        format!(
            "could not read the atlas input `{}`: {error}",
            path.display()
        ),
    )
    .with_path(pointer)
    .with_help("`inputs:` entries resolve relative to the atlas source's own directory")
}

fn unsupported_input(path: &Path, pointer: &str) -> Diagnostic {
    Diagnostic::new(
        code::SPRITE_DOCUMENT_STRUCTURE,
        format!(
            "the atlas input `{}` is neither a sprite document nor an image",
            path.display()
        ),
    )
    .with_path(pointer)
    .with_help("an `inputs:` entry is a `kind: sprite` document (`.yaml`/`.yml`) or a loose `.png`")
}

/// The output stem both written files share: the source's `name:`, else its own file name with
/// `.yaml`/`.yml` and then a trailing `.atlas-source` dropped
/// (`ship.atlas-source.yaml` → `ship`).
fn output_stem(doc: &AtlasSourceDocument, source_path: &Path) -> String {
    match doc.name.as_deref() {
        Some(name) => name.to_string(),
        None => document_stem(source_path, ".atlas-source"),
    }
}

/// A referenced document's stem: its file name with `.yaml`/`.yml` and then `suffix` dropped.
fn document_stem(path: &Path, suffix: &str) -> String {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = name
        .strip_suffix(".yaml")
        .or_else(|| name.strip_suffix(".yml"))
        .unwrap_or(&name);
    stem.strip_suffix(suffix).unwrap_or(stem).to_string()
}

/// `file` expressed relative to `dir` with forward slashes — the `generated_from:` provenance the
/// drift gate rediscovers the pair through. In the default layout (the pair written beside its
/// source) this is just the file name.
///
/// # Errors
/// `code` when `dir` and `file` are on different drives: no relative path exists, and an absolute
/// one would not be portable.
pub(crate) fn relative_to(
    dir: &Path,
    file: &Path,
    code: DiagnosticCode,
) -> Result<String, Diagnostic> {
    let file_name = || {
        file.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let (Ok(dir), Ok(file)) = (std::path::absolute(dir), std::path::absolute(file)) else {
        return Ok(file_name());
    };
    let mut left = dir.components().peekable();
    let mut right = file.components().peekable();
    if let (Some(Component::Prefix(left_prefix)), Some(Component::Prefix(right_prefix))) =
        (left.peek().copied(), right.peek().copied())
    {
        let drive = |prefix: PrefixComponent| match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                Some(letter.to_ascii_uppercase())
            }
            _ => None,
        };
        let same = match (drive(left_prefix), drive(right_prefix)) {
            (Some(a), Some(b)) => a == b,
            _ => left_prefix == right_prefix,
        };
        if !same {
            return Err(Diagnostic::new(
                code,
                format!(
                    "`{}` is on a different drive from the output `{}`: `generated_from` cannot name it relatively",
                    file.display(),
                    dir.display()
                ),
            )
            .with_help(format!(
                "write the output on the source's drive (`{}`), e.g. `--out` inside the project",
                right_prefix.as_os_str().to_string_lossy()
            )));
        }
        // `\\?\C:` and `C:` name one drive but differ as components: step past both.
        left.next();
        right.next();
    }
    while left.peek().is_some() && left.peek() == right.peek() {
        left.next();
        right.next();
    }
    let mut parts: Vec<String> = vec!["..".to_string(); left.count()];
    parts.extend(right.map(|part| part.as_os_str().to_string_lossy().into_owned()));
    if parts.is_empty() {
        return Ok(file_name());
    }
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_on_another_drive_has_no_relative_path() {
        if !Path::new(r"C:\a").is_absolute() {
            return;
        }
        let err = relative_to(
            Path::new(r"Q:\out"),
            Path::new(r"C:\src\x.gltf"),
            code::OUTPUT_IO,
        )
        .unwrap_err();
        assert_eq!(err.code, code::OUTPUT_IO);
        assert!(err.message.contains("different drive"), "{}", err.message);
        assert_eq!(
            relative_to(
                Path::new(r"c:\out"),
                Path::new(r"C:\src\x.gltf"),
                code::OUTPUT_IO
            )
            .unwrap(),
            "../src/x.gltf"
        );
        assert_eq!(
            relative_to(
                Path::new(r"\\?\C:\out"),
                Path::new(r"C:\src\x.gltf"),
                code::OUTPUT_IO
            )
            .unwrap(),
            "../src/x.gltf"
        );
        assert!(
            relative_to(
                Path::new(r"\\?\Q:\out"),
                Path::new(r"C:\src\x.gltf"),
                code::OUTPUT_IO
            )
            .is_err()
        );
    }
}
