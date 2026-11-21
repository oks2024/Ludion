//! `AssetRoot::local_document_unconfined` lets a document's own paths climb out of its directory;
//! only a command-line harness the local author drives may build one. The engine runtime, the
//! server and the wire build `AssetRoot::content`, which confines every read to the content root.

use std::fs;
use std::path::{Path, PathBuf};

/// The engine modules that root a harness at a document the author named.
const HARNESS_FILES: [&str; 4] = [
    "model_sheet.rs",
    "sprite_sheet.rs",
    "font_sheet.rs",
    "paired_sheet.rs",
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn unconfined_roots_are_built_only_by_harness_modules() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut offenders = Vec::new();
    for krate in ["ludion-engine", "ludion-server"] {
        let mut files = Vec::new();
        rust_files(&crates.join(krate).join("src"), &mut files);
        for file in files {
            let name = file.file_name().unwrap().to_string_lossy().into_owned();
            let text = fs::read_to_string(&file).unwrap();
            let allowed = krate == "ludion-engine" && HARNESS_FILES.contains(&name.as_str());
            if text.contains("local_document_unconfined") && !allowed {
                offenders.push(format!("{krate}/src/{name}"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "`AssetRoot::local_document_unconfined` is for CLI harnesses only; build \
         `AssetRoot::content` instead, or add the harness module to HARNESS_FILES: {offenders:?}"
    );
}
