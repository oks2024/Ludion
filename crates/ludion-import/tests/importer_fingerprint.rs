//! The importer fingerprint follows exactly the importer's dependency closure.

#[path = "../build/fingerprint.rs"]
mod fingerprint;

use std::fs;
use std::path::{Path, PathBuf};

fn cargo() -> PathBuf {
    std::env::var_os("CARGO").map_or_else(|| PathBuf::from("cargo"), PathBuf::from)
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn krate(root: &Path, dir: &str, name: &str, deps: &str) {
    write(
        root,
        &format!("{dir}/Cargo.toml"),
        &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{deps}"),
    );
    write(root, &format!("{dir}/src/lib.rs"), "pub fn f() {}\n");
}

fn print(root: &Path) -> String {
    let closure = fingerprint::closure(&cargo(), &root.join("a/Cargo.toml"), "a").unwrap();
    let inputs = fingerprint::inputs(&closure).unwrap();
    fingerprint::fingerprint(&inputs, "rustc test", "target test").unwrap()
}

#[test]
fn the_fingerprint_moves_with_the_closure_and_only_with_it() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("fp-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    write(
        &root,
        "Cargo.toml",
        "[workspace]\nresolver = \"2\"\nmembers = [\"a\", \"b\", \"c\"]\nexclude = [\"vendor/d\", \"vendor/e\"]\n",
    );
    write(&root, "Cargo.lock", "# lock\n");
    krate(
        &root,
        "a",
        "a",
        "[dependencies]\nb = { path = \"../b\" }\nd = { path = \"../vendor/d\" }\n[dev-dependencies]\nc = { path = \"../c\" }\n",
    );
    krate(&root, "b", "b", "");
    krate(&root, "c", "c", "");
    krate(
        &root,
        "vendor/d",
        "d",
        // Like the vendored rapier3d, an excluded crate is its own (empty) workspace.
        "[dependencies]\ne = { path = \"../e\" }\n[workspace]\n",
    );
    krate(&root, "vendor/e", "e", "[workspace]\n");

    let closure = fingerprint::closure(&cargo(), &root.join("a/Cargo.toml"), "a").unwrap();
    let names: Vec<&str> = closure.crates.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["a", "b", "d", "e"]);
    assert!(
        fingerprint::watch_paths(&closure)
            .iter()
            .any(|path| path.ends_with("Cargo.lock"))
    );
    let watched = fingerprint::watch_paths(&closure);
    assert!(
        watched.contains(&root.join("Cargo.lock").canonicalize().unwrap_or_default())
            || watched.iter().any(|p| p.ends_with("Cargo.lock"))
    );

    let base = print(&root);
    assert_eq!(base, print(&root), "the fingerprint is deterministic");

    write(&root, "c/src/lib.rs", "pub fn changed() {}\n");
    assert_eq!(base, print(&root), "a dev-only crate is not in the closure");
    write(&root, "b/src/lib.rs", "pub fn f() {}\n");
    assert_eq!(
        base,
        print(&root),
        "identical bytes rewritten change nothing"
    );

    let mut last = base;
    for (rel, text) in [
        ("b/src/lib.rs", "pub fn g() {}\n"),
        ("vendor/e/src/lib.rs", "pub fn g() {}\n"),
        ("Cargo.lock", "# lock 2\n"),
        ("b/src/extra.rs", "\n"),
    ] {
        write(&root, rel, text);
        let now = print(&root);
        assert_ne!(last, now, "editing `{rel}` must move the fingerprint");
        last = now;
    }
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn the_real_closure_excludes_the_engine_and_renderer() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let closure = fingerprint::closure(&cargo(), &manifest, "ludion-import").unwrap();
    let names: Vec<&str> = closure.crates.iter().map(|(n, _)| n.as_str()).collect();
    for wanted in ["ludion-components", "ludion-core", "rapier3d", "parry3d"] {
        assert!(names.contains(&wanted), "{wanted} missing from {names:?}");
    }
    for unwanted in ["ludion-engine", "ludion-cli", "ludion-render"] {
        assert!(!names.contains(&unwanted), "{unwanted} in {names:?}");
    }
}

#[test]
fn the_constant_is_sixteen_lowercase_hex_digits() {
    let hex = ludion_import::IMPORTER_FINGERPRINT;
    assert_eq!(hex.len(), 16);
    assert!(
        hex.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
}
