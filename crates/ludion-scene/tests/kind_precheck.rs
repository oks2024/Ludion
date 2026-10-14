//! `may_declare_kind` never skips a real doc: over every `*.yaml` the workspace commits (`games/`,
//! `crates/`, `docs/`), a file `peek_kind_checked` finds a top-level `kind` in is one the textual
//! pre-check answers `true` for.
//!
//! Only a file the pre-check answers `false` for can break that, and such a file is full-parsed
//! only when its text holds `kind` or a `\`: a strict-subset key decodes to `kind` only by spelling
//! it, or through a double-quoted escape — plain and single-quoted line folding insert spaces.

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
fn no_committed_doc_is_a_false_negative() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for dir in ["games", "crates", "docs"] {
        collect_yaml(&workspace.join(dir), &mut files);
    }

    let mut kind_docs = 0;
    let mut false_negatives = Vec::new();
    for path in &files {
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        if ludion_scene::may_declare_kind(&source) {
            kind_docs += usize::from(
                ludion_scene::peek_kind_checked(&source)
                    .ok()
                    .flatten()
                    .is_some(),
            );
        } else if (source.contains("kind") || source.contains('\\'))
            && ludion_scene::peek_kind_checked(&source)
                .ok()
                .flatten()
                .is_some()
        {
            false_negatives.push(path.display().to_string());
        }
    }

    assert!(
        false_negatives.is_empty(),
        "may_declare_kind skipped a doc peek_kind_checked finds a `kind` in: {false_negatives:#?}"
    );
    if workspace.join("games").is_dir() {
        assert!(
            kind_docs > 0,
            "the corpus walk found no kind-bearing doc under {} — the walk is broken",
            workspace.display()
        );
    }
}
