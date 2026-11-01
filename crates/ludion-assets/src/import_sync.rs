//! The `[[import]]` sync's shared vocabulary: which glTF files a table's `from` glob names, the
//! content hash and cheap file fingerprint (size and mtime) of a source's directory, and the
//! `<to>/.ludion-sync.json` record that binds a derived tree to the sources it was built from.
//! `ludion sync` writes the record; test harnesses read it to refuse a stale tree.

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
    /// [`directory_fingerprint`] of the glTF's directory, sorted by path. Empty in a record written
    /// before fingerprints, or when a stamp could not be taken; an empty fingerprint never matches.
    #[serde(default)]
    pub fingerprint: Vec<FileStamp>,
}

/// One source file's stamp: relative path (forward slashes), size in bytes and modification time in
/// nanoseconds since the Unix epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    /// Relative to the source directory, forward slashes.
    pub path: String,
    /// Length in bytes.
    pub size: u64,
    /// Modification time, nanoseconds since the Unix epoch.
    pub mtime_ns: u64,
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

/// The stamp (path, size, mtime in ns) of every file [`directory_hash`] reads, sorted by path, with
/// no content read. Empty when any file's mtime is unavailable, pre-epoch or overflows, which sends
/// the caller to the content hash. Walk errors propagate like [`directory_hash`]'s.
pub fn directory_fingerprint(dir: &Path) -> std::io::Result<Vec<FileStamp>> {
    let mut files = Vec::new();
    collect_files(dir, dir, &mut files)?;
    files.sort();
    let mut stamps = Vec::with_capacity(files.len());
    for relative in files {
        let metadata = std::fs::metadata(dir.join(&relative))?;
        let mtime_ns = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|since| u64::try_from(since.as_nanos()).ok());
        let Some(mtime_ns) = mtime_ns else {
            return Ok(Vec::new());
        };
        stamps.push(FileStamp {
            path: relative,
            size: metadata.len(),
            mtime_ns,
        });
    }
    Ok(stamps)
}

/// The source's hash as [`hex`]: the recorded one when `fingerprint` is non-empty and equals the
/// recorded entry's, else [`directory_hash`]. Take `fingerprint` *before* calling this, so a file
/// changing in between leaves older stamps than content and the next run re-hashes.
pub fn source_hash(
    dir: &Path,
    fingerprint: &[FileStamp],
    recorded: Option<&SyncRecordEntry>,
) -> std::io::Result<String> {
    if let Some(entry) = recorded
        && !fingerprint.is_empty()
        && entry.fingerprint == fingerprint
    {
        return Ok(entry.hash.clone());
    }
    Ok(hex(directory_hash(dir)?))
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

    struct Dir(std::path::PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("ludion-import-sync-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("a.gltf"), "{}").unwrap();
            std::fs::write(dir.join("b.bin"), "abcd").unwrap();
            Dir(dir)
        }

        fn write(&self, name: &str, bytes: &str, mtime_ns: u64) {
            let file = std::fs::File::options()
                .write(true)
                .create(true)
                .truncate(true)
                .open(self.0.join(name))
                .unwrap();
            std::io::Write::write_all(&mut &file, bytes.as_bytes()).unwrap();
            file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_nanos(mtime_ns))
                .unwrap();
        }

        /// A record entry as a sync would leave it for the directory right now.
        fn entry(&self) -> SyncRecordEntry {
            SyncRecordEntry {
                source: "a.gltf".into(),
                hash: hex(directory_hash(&self.0).unwrap()),
                outputs: vec!["m.yaml".into()],
                fingerprint: directory_fingerprint(&self.0).unwrap(),
            }
        }

        fn resolved(&self, entry: &SyncRecordEntry) -> String {
            let fingerprint = directory_fingerprint(&self.0).unwrap();
            source_hash(&self.0, &fingerprint, Some(entry)).unwrap()
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const SECOND: u64 = 1_000_000_000;

    fn base_mtime(entry: &SyncRecordEntry, name: &str) -> u64 {
        entry
            .fingerprint
            .iter()
            .find(|stamp| stamp.path == name)
            .unwrap()
            .mtime_ns
    }

    #[test]
    fn an_unchanged_directory_returns_the_recorded_hash_without_reading() {
        let dir = Dir::new("same");
        let mut entry = dir.entry();
        entry.hash = "not-a-real-hash".into();
        assert_eq!(dir.resolved(&entry), "not-a-real-hash");
    }

    #[test]
    fn a_content_change_is_never_fresh() {
        let dir = Dir::new("content");
        let entry = dir.entry();
        let mtime = base_mtime(&entry, "b.bin") + SECOND;
        dir.write("b.bin", "different length", mtime);
        assert_ne!(dir.resolved(&entry), entry.hash);
    }

    #[test]
    fn a_size_change_with_a_restored_mtime_is_never_fresh() {
        let dir = Dir::new("size");
        let entry = dir.entry();
        let mtime = base_mtime(&entry, "b.bin");
        dir.write("b.bin", "longer than before", mtime);
        assert_ne!(dir.resolved(&entry), entry.hash);
    }

    #[test]
    fn an_added_file_is_never_fresh() {
        let dir = Dir::new("added");
        let entry = dir.entry();
        dir.write("c.png", "x", 5 * SECOND);
        assert_ne!(dir.resolved(&entry), entry.hash);
    }

    #[test]
    fn a_removed_file_is_never_fresh() {
        let dir = Dir::new("removed");
        let entry = dir.entry();
        std::fs::remove_file(dir.0.join("b.bin")).unwrap();
        assert_ne!(dir.resolved(&entry), entry.hash);
    }

    #[test]
    fn a_same_size_edit_with_a_new_mtime_is_never_fresh() {
        let dir = Dir::new("samesize");
        let entry = dir.entry();
        let mtime = base_mtime(&entry, "b.bin") + SECOND;
        dir.write("b.bin", "wxyz", mtime);
        assert_ne!(dir.resolved(&entry), entry.hash);
    }

    #[test]
    fn an_older_record_without_fingerprints_parses_and_hashes() {
        let dir = Dir::new("old");
        let record: SyncRecord = serde_json::from_str(
            r#"{"importer":"x","entries":{"a":{"source":"a.gltf","hash":"h","outputs":[]}}}"#,
        )
        .unwrap();
        let entry = &record.entries["a"];
        assert!(entry.fingerprint.is_empty());
        assert_eq!(dir.resolved(entry), hex(directory_hash(&dir.0).unwrap()));
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
