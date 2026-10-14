//! Embeds the agent-docs (`docs/*.md`) into the binary so `ludion docs` works when the CLI is
//! installed outside the engine checkout.
//!
//! Two layers, one source of truth:
//! - The binary always embeds from the **vendored** copy at `crates/ludion-cli/embedded-docs/`
//!   (committed, shipped in the crates.io package — the tarball has no repo-root `docs/`).
//! - In a workspace checkout (`../../docs` exists), the repo-root `docs/` stays the human source of
//!   truth, and this script **verifies** the vendored copy mirrors it byte for byte, failing the
//!   build with the exact fix command when it does not.
//!
//! Checking rather than writing is deliberate: a build script that edits committed files makes every
//! `cargo build` non-hermetic (it can dirty the working tree, and it fails outright on a read-only or
//! vendored checkout). The mirroring itself lives in `node scripts/check.mjs --fix`, which is pure
//! Node — so clearing this failure never needs the build that is failing.
//!
//! The generated `EMBEDDED_DOCS` table is `include!`d by `src/docs.rs`. `rerun-if-changed` tracks
//! both directories in a checkout (either side can drift), the vendored dir alone in a packaged
//! build.

use std::fmt::Write as _;
use std::path::Path;

/// The one command that repairs a mirror drift, named in the failure so nobody has to go looking.
const FIX_COMMAND: &str = "node scripts/check.mjs --fix";

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let manifest_dir = Path::new(&manifest_dir);
    let repo_docs = manifest_dir.join("..").join("..").join("docs");
    let vendored = manifest_dir.join("embedded-docs");

    println!("cargo:rerun-if-changed={}", vendored.display());
    for (_, path) in md_files(&vendored) {
        println!("cargo:rerun-if-changed={path}");
    }
    if repo_docs.is_dir() {
        println!("cargo:rerun-if-changed={}", repo_docs.display());
        for (_, path) in md_files(&repo_docs) {
            println!("cargo:rerun-if-changed={path}");
        }
        check_mirror(&repo_docs, &vendored);
    }

    let mut generated = String::from("pub static EMBEDDED_DOCS: &[(&str, &str)] = &[\n");
    for (stem, path) in md_files(&vendored) {
        // `{:?}` escapes backslashes (Windows paths) and quotes into valid Rust string literals.
        writeln!(generated, "    ({stem:?}, include_str!({path:?})),").expect("write generated");
    }
    generated.push_str("];\n");

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    std::fs::write(Path::new(&out_dir).join("embedded_docs.rs"), generated)
        .expect("write embedded_docs.rs");
}

/// The `.md` files directly inside `dir` as sorted `(stem, path)` pairs — sorted because `read_dir`
/// order is OS-dependent and the embedded table (thus listing output) must be deterministic.
fn md_files(dir: &Path) -> Vec<(String, String)> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in entries {
        let path = entry.expect("read docs entry").path();
        if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("doc filename is UTF-8")
            .to_string();
        files.push((stem, path.to_str().expect("doc path is UTF-8").to_string()));
    }
    files.sort();
    files
}

/// Fails the build if the vendored dir is not a byte-for-byte mirror of the repo `docs/`, listing
/// every offending topic and the single command that fixes them all.
fn check_mirror(repo_docs: &Path, vendored: &Path) {
    let source = md_files(repo_docs);
    let mut drift = Vec::new();

    for (stem, path) in &source {
        let body = std::fs::read(path).expect("read repo doc");
        match std::fs::read(vendored.join(format!("{stem}.md"))) {
            Ok(mirrored) if mirrored == body => {}
            Ok(_) => drift.push(format!("  changed: {stem}.md")),
            Err(_) => drift.push(format!("  missing from embedded-docs/: {stem}.md")),
        }
    }
    for (stem, _) in md_files(vendored) {
        if !source.iter().any(|(s, _)| *s == stem) {
            drift.push(format!("  no longer in docs/: {stem}.md"));
        }
    }

    if !drift.is_empty() {
        drift.sort();
        panic!(
            "\ncrates/ludion-cli/embedded-docs/ is out of date with docs/:\n{}\n\nRun `{FIX_COMMAND}` \
             to re-mirror them, then commit the result. The vendored copy is what ships inside the \
             crates.io tarball, so it is committed, not generated at build time.\n",
            drift.join("\n")
        );
    }
}
