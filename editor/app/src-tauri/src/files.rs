//! Recursive directory listing for the editor's asset browsers.
//!
//! The Voxel Artist view walks `<content-root>/voxels/**` and treats the directory tree *as* the
//! taxonomy (`Plan/editor-voxel-artist.md`), so the host needs a listing primitive next to the
//! `read`/`stat`/`write_editor_file` trio in [`crate::pty`]. Path handling matches those commands:
//! the frontend passes an absolute path, confined by [`crate::confine`] to the repo or a Ludion
//! project.

use std::path::Path;

use serde::Serialize;

/// How deep the walk goes before giving up. A voxel library is two or three levels; the cap only
/// exists so a pathological tree cannot spin the host.
const MAX_DEPTH: usize = 32;

/// One entry of a [`list_editor_dir`] listing — enough to build a tree without a second call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DirEntry {
    /// Path relative to the listed root, always `/`-separated (so the frontend can split it into
    /// tree levels without knowing the host OS) and never empty.
    #[serde(rename = "relPath")]
    pub rel_path: String,
    /// The final path segment.
    pub name: String,
    /// True for a directory. Directories are always listed, whatever `ext` says — an empty
    /// category is still a category.
    #[serde(rename = "isDir")]
    pub is_dir: bool,
}

/// Does `name` end with the caller's extension filter?
///
/// The filter is a *suffix*, with or without its leading dot, matched case-insensitively — the
/// asset the browser wants is `*.voxel.yaml`, which `Path::extension` (`yaml`) cannot express.
/// An empty filter matches every file.
fn matches_ext(name: &str, ext: &str) -> bool {
    let suffix = ext.strip_prefix('.').unwrap_or(ext);
    if suffix.is_empty() {
        return true;
    }
    name.to_ascii_lowercase()
        .ends_with(&format!(".{}", suffix.to_ascii_lowercase()))
}

/// Depth-first walk emitting directories before their contents, so the flat list reads as a tree
/// top-down. Children are ordered directories-first then by name (case-insensitive), which is the
/// order the browser renders.
///
/// An unreadable *sub*directory is skipped rather than failing the whole listing: one
/// permission-denied folder should not blank the browser. An unreadable *root* is the caller's
/// error (see [`list_editor_dir`]).
fn walk(dir: &Path, prefix: &str, ext: &str, depth: usize, out: &mut Vec<DirEntry>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<(String, bool)> = Vec::new();
    for entry in read.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // Symlinks are skipped, not followed: a link back up the tree would make the walk
        // unbounded, and no asset library needs one.
        if file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        children.push((name, file_type.is_dir()));
    }
    children.sort_by(|(a_name, a_dir), (b_name, b_dir)| {
        b_dir
            .cmp(a_dir)
            .then_with(|| {
                a_name
                    .to_ascii_lowercase()
                    .cmp(&b_name.to_ascii_lowercase())
            })
            .then_with(|| a_name.cmp(b_name))
    });

    for (name, is_dir) in children {
        let rel_path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if is_dir {
            let child = dir.join(&name);
            out.push(DirEntry {
                rel_path: rel_path.clone(),
                name,
                is_dir: true,
            });
            walk(&child, &rel_path, ext, depth + 1, out);
        } else if matches_ext(&name, ext) {
            out.push(DirEntry {
                rel_path,
                name,
                is_dir: false,
            });
        }
    }
}

/// List `path` recursively, keeping files whose name ends with `ext` (a suffix such as
/// `.voxel.yaml`; empty or omitted keeps every file). Directories are always included.
///
/// Errors are readable, not io-speak: a missing root names the path and says what was expected.
#[tauri::command(async)]
pub fn list_editor_dir(path: String, ext: Option<String>) -> Result<Vec<DirEntry>, String> {
    crate::confine::check(&path)?;
    let root = Path::new(&path);
    let meta = std::fs::metadata(root).map_err(|e| {
        format!("cannot list `{path}`: {e}. Expected a directory of assets — switch project from the title bar.")
    })?;
    if !meta.is_dir() {
        return Err(format!(
            "cannot list `{path}`: it is a file, not a directory. Pass the directory that contains the assets."
        ));
    }
    let ext = ext.unwrap_or_default();
    let mut out = Vec::new();
    walk(root, "", &ext, 0, &mut out);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway tree under the OS temp dir, removed on drop.
    struct Tree(std::path::PathBuf);

    impl Tree {
        fn new(tag: &str) -> Tree {
            let dir = std::env::temp_dir().join(format!(
                "ludion-files-test-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("ludion.toml"),
                "name = \"t\"\ndefault_scene = \"scenes/main.yaml\"\n",
            )
            .unwrap();
            Tree(dir)
        }
        fn file(&self, rel: &str) {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        fn dir(&self, rel: &str) {
            std::fs::create_dir_all(self.0.join(rel)).unwrap();
        }
        fn path(&self) -> String {
            self.0.to_string_lossy().into_owned()
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn ext_filter_is_a_case_insensitive_suffix_with_an_optional_dot() {
        assert!(matches_ext("statue.voxel.yaml", ".voxel.yaml"));
        assert!(matches_ext("statue.voxel.yaml", "voxel.yaml"));
        assert!(matches_ext("STATUE.VOXEL.YAML", ".voxel.yaml"));
        assert!(!matches_ext("statue.mesh.yaml", ".voxel.yaml"));
        assert!(!matches_ext("voxel.yaml", ".voxel.yaml")); // the dot is required
        assert!(matches_ext("anything", "")); // no filter
        assert!(matches_ext("anything", "."));
    }

    #[test]
    fn listing_is_recursive_filtered_and_ordered_directories_first() {
        let t = Tree::new("list");
        t.file("statue.voxel.yaml");
        t.file("notes.md");
        t.file("enemies/boar.voxel.yaml");
        t.file("enemies/Wolf.voxel.yaml");
        t.file("props/idol.voxel.yaml");
        t.dir("empty");

        let out = list_editor_dir(t.path(), Some(".voxel.yaml".into())).unwrap();
        let rels: Vec<&str> = out.iter().map(|e| e.rel_path.as_str()).collect();
        assert_eq!(
            rels,
            vec![
                "empty",
                "enemies",
                "enemies/boar.voxel.yaml",
                "enemies/Wolf.voxel.yaml",
                "props",
                "props/idol.voxel.yaml",
                "statue.voxel.yaml",
            ],
            "directories first, then files, each group case-insensitively by name"
        );
        assert!(out.iter().filter(|e| e.is_dir).count() == 3);
        assert_eq!(
            out.iter()
                .find(|e| e.rel_path == "enemies/boar.voxel.yaml")
                .unwrap()
                .name,
            "boar.voxel.yaml"
        );
        // `notes.md` was filtered out; an empty directory still appears.
        assert!(!rels.contains(&"notes.md"));
    }

    #[test]
    fn no_filter_lists_every_file() {
        let t = Tree::new("nofilter");
        t.file("a.txt");
        t.file("sub/b.bin");
        let out = list_editor_dir(t.path(), None).unwrap();
        let rels: Vec<&str> = out.iter().map(|e| e.rel_path.as_str()).collect();
        assert_eq!(rels, vec!["sub", "sub/b.bin", "a.txt", "ludion.toml"]);
    }

    #[test]
    fn a_root_outside_every_project_is_refused() {
        let t = Tree::new("confined");
        std::fs::remove_file(t.0.join("ludion.toml")).unwrap();
        let err = list_editor_dir(t.path(), None).unwrap_err();
        assert!(err.contains("outside the engine checkout"), "{err}");
    }

    #[test]
    fn missing_and_non_directory_roots_are_readable_errors() {
        let t = Tree::new("errors");
        let missing = t.0.join("nope").to_string_lossy().into_owned();
        let err = list_editor_dir(missing.clone(), None).unwrap_err();
        assert!(err.contains(&missing), "{err}");
        assert!(err.contains("Expected a directory"), "{err}");

        t.file("a.txt");
        let file = t.0.join("a.txt").to_string_lossy().into_owned();
        let err = list_editor_dir(file, None).unwrap_err();
        assert!(err.contains("it is a file, not a directory"), "{err}");
    }
}
