//! Computes `LUDION_IMPORTER_FINGERPRINT`, the hash of the importer's dependency closure that
//! `ludion sync` records to know an import tree is stale. See `build/fingerprint.rs`.

#[path = "build/fingerprint.rs"]
mod fingerprint;

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let env =
        |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("cargo did not set `{name}`"));
    let manifest_dir = PathBuf::from(env("CARGO_MANIFEST_DIR"));
    let manifest = manifest_dir.join("Cargo.toml");

    let closure = if manifest_dir.join("Cargo.toml.orig").is_file() {
        fingerprint::Closure {
            workspace_root: None,
            crates: vec![("ludion-import".to_string(), manifest_dir.clone())],
        }
    } else {
        fingerprint::closure(&PathBuf::from(env("CARGO")), &manifest, "ludion-import")
            .unwrap_or_else(|message| {
                panic!(
                    "cannot fingerprint the importer: {message}\nhelp: run from a Ludion checkout"
                )
            })
    };

    let rustc = Command::new(env("RUSTC"))
        .arg("-V")
        .output()
        .unwrap_or_else(|error| panic!("could not run rustc -V: {error}"));
    let rustc = String::from_utf8_lossy(&rustc.stdout).trim().to_string();
    let inputs = fingerprint::inputs(&closure)
        .unwrap_or_else(|error| panic!("could not read the importer's sources: {error}"));
    let hex = fingerprint::fingerprint(&inputs, &rustc, &env("TARGET"))
        .unwrap_or_else(|error| panic!("could not hash the importer's sources: {error}"));

    for path in fingerprint::watch_paths(&closure) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!("cargo:rustc-env=LUDION_IMPORTER_FINGERPRINT={hex}");
}
