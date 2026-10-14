//! The derived-imports precondition for game test suites.
//!
//! A project's `[[import]]` trees (`ludion sync`) are derived and git-ignored. An unsynced tree
//! silently drops meshes and colliders, so two runs agree on the wrong world and a determinism
//! gate passes vacuously; a tree built from sources since edited is the same lie. A suite calls [`require_derived_imports`] in its content-root helper so
//! the missing tree fails loudly with the command that fixes it.

use std::path::{Path, PathBuf};

use ludion_assets::import_sync::{
    RECORD_FILE, SyncRecord, directory_hash, expand_import_sources, hex,
};

/// The `[[import]]` outputs of `project/ludion.toml` that are not synced, joined to `project`: the
/// `to` directory when absent or empty, and each `<to>/<stem>` directory the `from` glob expects
/// that is absent, empty, or built from a source that has changed since (the sync record's hash of
/// the source's directory no longer matches), so a stale or interrupted sync fails too. No manifest
/// means no imports. A manifest that fails to load (`ludion_engine::manifest::load`) panics.
///
/// ```no_run
/// let missing = ludion_testkit::missing_derived_imports(std::path::Path::new("games/breakout"));
/// assert!(missing.is_empty(), "run `ludion sync --project games/breakout`");
/// ```
pub fn missing_derived_imports(project: &Path) -> Vec<PathBuf> {
    if !project.join("ludion.toml").is_file() {
        return Vec::new();
    }
    let manifest = ludion_engine::manifest::load(project).unwrap_or_else(|d| panic!("{d}"));
    let unpopulated = |dir: &Path| std::fs::read_dir(dir).map_or(true, |mut d| d.next().is_none());
    let mut missing = Vec::new();
    for import in manifest.imports {
        let to = project.join(&import.to);
        if unpopulated(&to) {
            missing.push(to);
            continue;
        }
        let record = std::fs::read_to_string(to.join(RECORD_FILE))
            .ok()
            .and_then(|text| serde_json::from_str::<SyncRecord>(&text).ok());
        for source in expand_import_sources(project, &import.from) {
            let stem = Path::new(&source).file_stem().unwrap_or_default();
            let dir = to.join(stem);
            let fresh = record
                .as_ref()
                .and_then(|record| record.entries.get(&*stem.to_string_lossy()))
                .is_some_and(|entry| {
                    let source_dir =
                        project.join(Path::new(&source).parent().unwrap_or(Path::new("")));
                    entry.source == source
                        && directory_hash(&source_dir).is_ok_and(|hash| hex(hash) == entry.hash)
                });
            if unpopulated(&dir) || !fresh {
                missing.push(dir);
            }
        }
    }
    missing
}

/// Panics, one line per directory, when [`missing_derived_imports`] is non-empty.
///
/// ```no_run
/// ludion_testkit::require_derived_imports(std::path::Path::new(env!("CARGO_MANIFEST_DIR")));
/// ```
pub fn require_derived_imports(project: &Path) {
    let missing = missing_derived_imports(project);
    if missing.is_empty() {
        return;
    }
    let lines: Vec<String> = missing
        .iter()
        .map(|dir| {
            format!(
                "{} is missing or stale — this project's [[import]] tree is derived and git-ignored: run \
                 `ludion sync --project {}` (or `node scripts/import-sync.mjs`) first",
                dir.display(),
                project.display()
            )
        })
        .collect();
    panic!("{}", lines.join("\n"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_assets::import_sync::SyncRecordEntry;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "ludion-testkit-imports-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn manifest(&self, text: &str) {
            std::fs::write(self.0.join("ludion.toml"), text).unwrap();
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const IMPORT: &str = "name = \"x\"\ndefault_scene = \"s/a.yaml\"\n[[import]]\nfrom = \"src/*.gltf\"\nto = \"assets/town\"\n";

    #[test]
    fn no_manifest_means_nothing_missing() {
        let s = Scratch::new("none");
        assert!(missing_derived_imports(&s.0).is_empty());
    }

    #[test]
    fn manifest_without_imports_is_clean() {
        let s = Scratch::new("plain");
        s.manifest("name = \"x\"\ndefault_scene = \"s/a.yaml\"\n");
        assert!(missing_derived_imports(&s.0).is_empty());
    }

    #[test]
    fn absent_dir_is_missing() {
        let s = Scratch::new("absent");
        s.manifest(IMPORT);
        assert_eq!(missing_derived_imports(&s.0), vec![s.0.join("assets/town")]);
    }

    #[test]
    fn empty_dir_is_missing() {
        let s = Scratch::new("empty");
        s.manifest(IMPORT);
        std::fs::create_dir_all(s.0.join("assets/town")).unwrap();
        assert_eq!(missing_derived_imports(&s.0).len(), 1);
    }

    /// What `ludion sync` leaves for `stems`: each source under `src/`, an output file, and the record.
    fn sync(s: &Scratch, stems: &[&str]) {
        let to = s.0.join("assets/town");
        std::fs::create_dir_all(s.0.join("src")).unwrap();
        for stem in stems {
            std::fs::write(s.0.join(format!("src/{stem}.gltf")), "{}").unwrap();
        }
        let mut record = SyncRecord::default();
        for stem in stems {
            let source = format!("src/{stem}.gltf");
            std::fs::create_dir_all(to.join(stem)).unwrap();
            std::fs::write(to.join(stem).join("mesh.yaml"), "x").unwrap();
            let hash = hex(directory_hash(&s.0.join("src")).unwrap());
            record.entries.insert(
                stem.to_string(),
                SyncRecordEntry {
                    source,
                    hash,
                    outputs: vec!["mesh.yaml".into()],
                },
            );
        }
        std::fs::write(
            to.join(RECORD_FILE),
            serde_json::to_string(&record).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn a_synced_tree_is_clean() {
        let s = Scratch::new("full");
        s.manifest(IMPORT);
        sync(&s, &["a"]);
        assert!(missing_derived_imports(&s.0).is_empty());
        require_derived_imports(&s.0);
    }

    #[test]
    fn a_tree_missing_one_of_two_kits_is_missing() {
        let s = Scratch::new("partial");
        s.manifest(IMPORT);
        sync(&s, &["a", "b"]);
        std::fs::remove_dir_all(s.0.join("assets/town/b")).unwrap();
        assert_eq!(
            missing_derived_imports(&s.0),
            vec![s.0.join("assets/town/b")]
        );
    }

    #[test]
    #[should_panic(expected = "missing or stale")]
    fn a_source_edited_after_the_sync_is_stale() {
        let s = Scratch::new("stale");
        s.manifest(IMPORT);
        sync(&s, &["a"]);
        require_derived_imports(&s.0);
        std::fs::write(s.0.join("src/a.gltf"), "{\"edited\":1}").unwrap();
        assert_eq!(
            missing_derived_imports(&s.0),
            vec![s.0.join("assets/town/a")]
        );
        require_derived_imports(&s.0);
    }

    #[test]
    fn a_tree_without_a_record_is_stale() {
        let s = Scratch::new("norecord");
        s.manifest(IMPORT);
        sync(&s, &["a"]);
        std::fs::remove_file(s.0.join("assets/town").join(RECORD_FILE)).unwrap();
        assert_eq!(
            missing_derived_imports(&s.0),
            vec![s.0.join("assets/town/a")]
        );
    }

    #[test]
    #[should_panic(expected = "ludion sync")]
    fn require_panics_naming_the_fix() {
        let s = Scratch::new("panic");
        s.manifest(IMPORT);
        require_derived_imports(&s.0);
    }
}
