//! The importer fingerprint: a hash of every source the importer's output can depend on.
//!
//! The closure is derived, never listed: `cargo metadata --no-deps` is walked from `ludion-import`
//! over its non-dev path dependencies, so a new dependency joins the fingerprint with no edit here.
//! The `build.rs` reruns on mtime (`rerun-if-changed`) but the hash is of content, so a touch
//! reruns it and yields the same constant; the one miss is an edit whose file and directory mtimes
//! all predate the last build-script run. A packaged crate (`Cargo.toml.orig` beside the manifest)
//! has no workspace and hashes only its own files. Any other metadata failure is fatal to the
//! build, since a silent fallback would make the fingerprint unsound. Symlinks under `src/` are
//! followed. Built under vigil's detached workspace the hashed `Cargo.lock` is still the root one,
//! which is harmless because `sync` runs from the root-workspace `ludion` binary.

use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use xxhash_rust::xxh3::Xxh3;

const DOMAIN_TAG: &str = "ludion-importer-fingerprint v1";

/// The crates whose sources the root package's output depends on.
pub struct Closure {
    /// Where the root `Cargo.toml` / `Cargo.lock` live; `None` for a packaged crate.
    pub workspace_root: Option<PathBuf>,
    /// `(package name, crate directory)`, sorted by name and deduplicated.
    pub crates: Vec<(String, PathBuf)>,
}

struct Package {
    name: String,
    dir: PathBuf,
    path_deps: Vec<PathBuf>,
}

fn same_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn metadata(cargo: &Path, manifest: &Path) -> Result<(PathBuf, Vec<Package>), String> {
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(manifest)
        .output()
        .map_err(|error| format!("could not run `{} metadata`: {error}", cargo.display()))?;
    if !output.status.success() {
        return Err(format!(
            "`cargo metadata` failed for `{}`: {}",
            manifest.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("`cargo metadata` printed invalid JSON: {error}"))?;
    let missing = |what: &str| format!("`cargo metadata` output has no `{what}`");
    let workspace_root = value["workspace_root"]
        .as_str()
        .ok_or_else(|| missing("workspace_root"))?;
    let mut packages = Vec::new();
    for package in value["packages"]
        .as_array()
        .ok_or_else(|| missing("packages"))?
    {
        let name = package["name"].as_str().ok_or_else(|| missing("name"))?;
        let manifest_path = package["manifest_path"]
            .as_str()
            .ok_or_else(|| missing("manifest_path"))?;
        let dir = Path::new(manifest_path)
            .parent()
            .ok_or_else(|| missing("manifest_path parent"))?;
        let mut path_deps = Vec::new();
        for dep in package["dependencies"].as_array().into_iter().flatten() {
            // Over-inclusion is safe: optional and target-specific deps count; dev-deps cannot
            // reach the library's output.
            if dep["kind"].as_str() == Some("dev") {
                continue;
            }
            if let Some(path) = dep["path"].as_str() {
                path_deps.push(same_path(Path::new(path)));
            }
        }
        packages.push(Package {
            name: name.to_string(),
            dir: same_path(dir),
            path_deps,
        });
    }
    Ok((PathBuf::from(workspace_root), packages))
}

/// Walks the non-dev path dependencies of `root_pkg` (a member of the workspace at `manifest`).
pub fn closure(cargo: &Path, manifest: &Path, root_pkg: &str) -> Result<Closure, String> {
    let (workspace_root, packages) = metadata(cargo, manifest)?;
    let root = packages
        .iter()
        .find(|package| package.name == root_pkg)
        .map(|package| package.dir.clone())
        .ok_or_else(|| format!("package `{root_pkg}` is not in `{}`", manifest.display()))?;
    let mut known: BTreeMap<PathBuf, Package> = BTreeMap::new();
    for package in packages {
        known.insert(package.dir.clone(), package);
    }
    let mut seen: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut queue = VecDeque::from([root]);
    while let Some(dir) = queue.pop_front() {
        if seen.contains_key(&dir) {
            continue;
        }
        if !known.contains_key(&dir) {
            // A path crate outside the workspace (an `exclude`d vendored crate): its own manifest.
            let (_, packages) = metadata(cargo, &dir.join("Cargo.toml"))?;
            for package in packages {
                known.entry(package.dir.clone()).or_insert(package);
            }
        }
        let package = known
            .get(&dir)
            .ok_or_else(|| format!("no package found at `{}`", dir.display()))?;
        seen.insert(dir.clone(), package.name.clone());
        queue.extend(package.path_deps.iter().cloned());
    }
    let mut crates: Vec<(String, PathBuf)> =
        seen.into_iter().map(|(dir, name)| (name, dir)).collect();
    crates.sort();
    Ok(Closure {
        workspace_root: Some(workspace_root),
        crates,
    })
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) -> io::Result<()> {
    let mut entries = std::fs::read_dir(dir)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let key = format!("{rel}/{}", entry.file_name().to_string_lossy());
        let path = entry.path();
        if std::fs::metadata(&path)?.is_dir() {
            walk(&path, &key, out)?;
        } else {
            out.push((key, path));
        }
    }
    Ok(())
}

/// Every hashed file, keyed relatively (`<pkg>/<path in crate>`), sorted.
pub fn inputs(closure: &Closure) -> io::Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    for (name, dir) in &closure.crates {
        walk(&dir.join("src"), &format!("{name}/src"), &mut out)?;
        for file in ["Cargo.toml", "build.rs"] {
            if dir.join(file).is_file() {
                out.push((format!("{name}/{file}"), dir.join(file)));
            }
        }
    }
    if let Some(root) = &closure.workspace_root {
        for file in ["Cargo.toml", "Cargo.lock"] {
            if root.join(file).is_file() {
                out.push((format!("workspace/{file}"), root.join(file)));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// What `cargo:rerun-if-changed` must list: cargo stops watching the package once any is printed.
pub fn watch_paths(closure: &Closure) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for (_, dir) in &closure.crates {
        out.push(dir.join("src"));
        out.push(dir.join("Cargo.toml"));
        if dir.join("build.rs").is_file() {
            out.push(dir.join("build.rs"));
        }
    }
    if let Some(root) = &closure.workspace_root {
        out.push(root.join("Cargo.toml"));
        out.push(root.join("Cargo.lock"));
    }
    out
}

fn put(hasher: &mut Xxh3, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// xxh3-64 over the domain tag, each length-prefixed entry (key, then content), the compiler
/// version and the target, as 16 lowercase hex digits.
pub fn fingerprint(inputs: &[(String, PathBuf)], rustc: &str, target: &str) -> io::Result<String> {
    let mut hasher = Xxh3::new();
    put(&mut hasher, DOMAIN_TAG.as_bytes());
    for (key, path) in inputs {
        put(&mut hasher, key.as_bytes());
        put(&mut hasher, &std::fs::read(path)?);
    }
    put(&mut hasher, rustc.as_bytes());
    put(&mut hasher, target.as_bytes());
    Ok(format!("{:016x}", hasher.digest()))
}
