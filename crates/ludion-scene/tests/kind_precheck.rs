//! `sniff_kind` never misroutes a real doc: over every `*.yaml` the workspace commits (`games/`,
//! `crates/`, `docs/`), `Absent` files are ones `peek_kind_checked` finds no `kind` in, and every
//! `Kind(k)` file answers `Ok(Some(k))` or `Err` under the full peek.
//!
//! An `Absent` file is full-parsed only when its text holds `kind` or a `\`: a strict-subset key
//! decodes to `kind` only by spelling it, or through a double-quoted escape — plain and
//! single-quoted line folding insert spaces. Every `Kind` file is full-peeked: the real-content
//! agreement proof.

use ludion_scene::KindSniff;
use std::path::{Path, PathBuf};

fn collect_yaml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if !(name.starts_with('.') || name == "target" || name == "node_modules") {
                collect_yaml(&path, out);
            }
        } else if name.ends_with(".yaml") {
            out.push(path);
        }
    }
}

#[test]
fn the_sniff_agrees_with_the_full_peek_on_every_committed_doc() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for dir in ["games", "crates", "docs"] {
        collect_yaml(&workspace.join(dir), &mut files);
    }

    let mut kind_answers = 0;
    let mut disagreements = Vec::new();
    for path in &files {
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        match ludion_scene::sniff_kind(&source) {
            KindSniff::Undecided => {}
            KindSniff::Absent => {
                if (source.contains("kind") || source.contains('\\'))
                    && matches!(ludion_scene::peek_kind_checked(&source), Ok(Some(_)))
                {
                    disagreements.push(format!("{}: Absent", path.display()));
                }
            }
            KindSniff::Kind(kind) => {
                kind_answers += 1;
                if let Ok(full) = ludion_scene::peek_kind_checked(&source)
                    && full.as_deref() != Some(kind)
                {
                    disagreements.push(format!("{}: Kind({kind}) vs {full:?}", path.display()));
                }
            }
        }
    }

    assert!(
        disagreements.is_empty(),
        "sniff_kind disagrees with peek_kind_checked: {disagreements:#?}"
    );
    if workspace.join("games").is_dir() {
        assert!(
            kind_answers > 0,
            "the corpus walk got no `Kind` answer under {} — the walk is broken",
            workspace.display()
        );
    }
}
