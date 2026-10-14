//! The `[[import]]` sync's shared vocabulary: which glTF files a table's `from` glob names, the
//! content hash of a source's directory, and the `<to>/.ludion-sync.json` record that binds a
//! derived tree to the sources it was built from. `ludion sync` writes the record; test harnesses
//! read it to refuse a stale tree.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::content_hash;

/// The ownership and freshness record's file name, `<to>/.ludion-sync.json`.
pub const RECORD_FILE: &str = ".ludion-sync.json";

/// The lock file a sync holds on a `to` root, `<to>/.ludion-sync.lock`.
pub const LOCK_FILE: &str = ".ludion-sync.lock";

/// The sync record: the importer that wrote the tree and one entry per source stem.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncRecord {
    /// The importer fingerprint: xxh3 of the `ludion` binary that wrote the tree, 16 hex digits.
    pub importer: String,
    /// The recorded sources, by stem.
    pub entries: BTreeMap<String, SyncRecordEntry>,
}

/// One source's record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncRecordEntry {
    /// The glTF, project-relative with forward slashes.
    pub source: String,
    /// [`directory_hash`] of the glTF's directory, as [`hex`].
    pub hash: String,
    /// The files the import wrote, relative to `<to>/<stem>/`, forward slashes, sorted. Empty in a
    /// record written before outputs were tracked, which counts as stale.
    #[serde(default)]
    pub outputs: Vec<String>,
}

/// A hash as the 16 lowercase hex digits the record stores.
pub fn hex(value: u64) -> String {
    format!("{value:016x}")
}

/// Matches `*` globs segment by segment (`*` stays inside one path segment) and returns the
/// project-relative `.gltf` files, forward slashes, sorted. The one enumeration `ludion sync` and the
/// test harness's derived-imports guard share, so the guard expects exactly what the sync writes.
pub fn expand_import_sources(root: &Path, pattern: &str) -> Vec<String> {
    let segments: Vec<&str> = pattern.split('/').collect();
    let mut current = vec![String::new()];
    for segment in &segments {
        let mut next = Vec::new();
        for base in &current {
            let join = |name: &str| {
                if base.is_empty() {
                    name.to_string()
                } else {
                    format!("{base}/{name}")
                }
            };
            if !segment.contains('*') {
                next.push(join(segment));
                continue;
            }
            let Ok(read) = root.join(base).read_dir() else {
                continue;
            };
            let mut names: Vec<String> = read
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| glob_match(segment, name))
                .collect();
            names.sort();
            next.extend(names.iter().map(|name| join(name)));
        }
        current = next;
        if current.is_empty() {
            break;
        }
    }
    current.retain(|relative| {
        let path = root.join(relative);
        path.is_file()
            && path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("gltf"))
    });
    current.sort();
    current
}

/// `*` matches any run of characters (including none) within one segment.
fn glob_match(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, rest) = parts.split_first().expect("split yields one part");
    let Some(mut tail) = name.strip_prefix(first) else {
        return false;
    };
    let Some((last, middle)) = rest.split_last() else {
        return tail.is_empty();
    };
    for part in middle {
        match tail.find(part) {
            Some(at) => tail = &tail[at + part.len()..],
            None => return false,
        }
    }
    tail.ends_with(last)
}

/// xxh3 over every file under `dir`, in sorted order: each file's relative name and content hash.
pub fn directory_hash(dir: &Path) -> std::io::Result<u64> {
    let mut files = Vec::new();
    collect_files(dir, dir, &mut files)?;
    files.sort();
    let mut digest = Vec::new();
    for relative in &files {
        digest.extend_from_slice(relative.as_bytes());
        digest.push(0);
        digest.extend_from_slice(&content_hash(&std::fs::read(dir.join(relative))?).to_le_bytes());
    }
    Ok(content_hash(&digest))
}

/// Appends every file under `dir`, relative to `base` with forward slashes, to `out`. A symbolic
/// link or junction is refused (`InvalidInput`, naming it): the walk neither follows nor hashes one.
/// The sync's own record, lock, staging (`.sync-*`) and rollback (`.old-*`) entries are not content,
/// so a `to` that sits under a source directory cannot change that directory's hash.
pub fn collect_files(base: &Path, dir: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
    for entry in dir.read_dir()? {
        let entry = entry?;
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if file_name == RECORD_FILE
            || file_name == LOCK_FILE
            || file_name.starts_with(".sync-")
            || file_name.starts_with(".old-")
        {
            continue;
        }
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("`{}` is a symbolic link or junction", path.display()),
            ));
        }
        if kind.is_dir() {
            collect_files(base, &path, out)?;
        } else if path
            .file_name()
            .is_some_and(|name| ludion_core::atomic::is_temp_name(&name.to_string_lossy()))
        {
            // A crashed atomic write's leftover is not content.
        } else if let Ok(relative) = path.strip_prefix(base) {
            let parts: Vec<_> = relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            out.push(parts.join("/"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_debris_does_not_move_the_source_hash() {
        let dir = std::env::temp_dir().join(format!("ludion-import-sync-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.gltf"), "{}").unwrap();
        let clean = directory_hash(&dir).unwrap();

        std::fs::write(dir.join(RECORD_FILE), "{}").unwrap();
        std::fs::write(dir.join(LOCK_FILE), "").unwrap();
        std::fs::write(dir.join(".old-a"), "x").unwrap();
        std::fs::create_dir_all(dir.join(".sync-a")).unwrap();
        std::fs::write(dir.join(".sync-a/model.prefab.yaml"), "x").unwrap();
        assert_eq!(directory_hash(&dir).unwrap(), clean);

        std::fs::write(dir.join("b.bin"), "x").unwrap();
        assert_ne!(directory_hash(&dir).unwrap(), clean);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_star_stays_inside_one_segment() {
        assert!(glob_match("*.gltf", "well.gltf"));
        assert!(glob_match("a*c*e", "abcde"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("kit", "kit"));
        assert!(!glob_match("*.gltf", "well.glb"));
        assert!(!glob_match("a*c", "ab"));
        assert!(!glob_match("kit", "kits"));
    }
}
