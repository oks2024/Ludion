//! Implementation of `ludion sculpt new` — the write side of sculpt authoring.
//!
//! The read side (`check-scene --bounds`, `render --model`) tells an author what a
//! sculpt *is*; today this is the only verb that changes one — there is no `sculpt apply`, so a
//! scaffolded model grows by hand-editing `parts:` in the text editor, checked with `check-scene`
//! after every change. `new` follows the voxel verb's validate-then-swap discipline: the whole
//! document is built in memory, checked with [`ludion_scene::check_sculpt`], and only then written
//! through [`ludion_core::atomic`], so a scaffold on disk is never half-written.
//!
//! It never overwrites an existing file — a birth that clobbers a sculpted model is a lost afternoon,
//! the same rule `voxel new` holds.

use std::path::Path;
use std::process::ExitCode;

use ludion_scene::{SculptBone, SculptDocument};

use crate::diagnostic;
use crate::voxel::{create_text, default_palette_entry, read_palette_from};

/// Runs `ludion sculpt new <out> [--name] [--palette-from]`.
pub fn run_new(out: &Path, name: Option<&str>, palette_from: Option<&Path>) -> ExitCode {
    let palette = match palette_from {
        Some(path) => match read_palette_from(path) {
            Ok(palette) => palette,
            Err(exit) => return exit,
        },
        None => vec![default_palette_entry()],
    };

    let name = name
        .map(str::to_string)
        .or_else(|| model_stem(out).map(str::to_string));
    let doc = SculptDocument {
        name: name.clone(),
        bones: vec![SculptBone {
            name: "root".to_string(),
            parent: None,
            head: None,
            tail: None,
        }],
        palette,
        ..SculptDocument::default()
    };
    let text = format!(
        "{}{}",
        scaffold_header(name.as_deref()),
        ludion_scene::emit_sculpt(&doc)
    );
    if let Err(exit) = write_new(out, &text) {
        return exit;
    }
    println!(
        "{}: created (0 parts, {} bone, {} palette entr{})",
        out.display(),
        doc.bones.len(),
        doc.palette.len(),
        if doc.palette.len() == 1 { "y" } else { "ies" }
    );
    ExitCode::SUCCESS
}

/// The scaffold's header comment: what the file is and where to read more. Plain text, not a
/// structured document field — the strict-subset YAML reader skips full-line comments anywhere, so
/// this survives a `check_sculpt` parse untouched but is not reproduced by `emit_sculpt` (unlike a
/// voxel model's `VoxelComments`, a sculpt document carries no comment-preserving structure).
fn scaffold_header(name: Option<&str>) -> String {
    let title = name.unwrap_or("model");
    format!(
        "# {title} — an implicit-surface sculpt model; add primitives to `parts:` to build the form.\n\
         # See `ludion docs sculpt-format` for the document's keys and `ludion docs sculpt-authoring`\n\
         # for the new -> check-scene -> render --model -> look loop.\n"
    )
}

/// The model's name for the scaffold: the file stem with its `.sculpt.yaml` suffix dropped.
fn model_stem(path: &Path) -> Option<&str> {
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_suffix(".sculpt.yaml").or_else(|| {
        name.strip_suffix(".yaml")
            .and_then(|stem| stem.strip_suffix(".sculpt").or(Some(stem)))
    })?;
    (!stem.is_empty()).then_some(stem)
}

/// Validates `text` as a sculpt document, then creates `path` with it, refusing when it exists —
/// the [`voxel::create_validated`](crate::voxel) shape, specialized to `check_sculpt` since `new`
/// is the crate's only sculpt-writing verb so far.
fn write_new(path: &Path, text: &str) -> Result<(), ExitCode> {
    if let Err(diagnostics) = ludion_scene::check_sculpt(text) {
        eprintln!("the scaffolded model would not validate; nothing was written:");
        return Err(diagnostic::report(path, &diagnostics));
    }
    create_text(
        path,
        text,
        "sculpt new",
        "edit its `parts:` by hand, or choose another path",
    )
}
