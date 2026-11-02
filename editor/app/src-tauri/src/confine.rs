//! Path confinement for the host's file IPC.
//!
//! A path is allowed iff, once resolved, it lies under the repo root or under a directory (itself or
//! an ancestor) that [`crate::project::read_project`] accepts. The host holds no "open project"
//! state: the frontend owns it, and views read files before `project_info` answers. Commands
//! validate here and keep operating on the caller's original string. A symlink swapped between the
//! check and the use is accepted residue.

use std::path::{Component, Path};

/// Refuses `path` unless it is absolute, free of `..`, and resolves under the repo or a project.
///
/// # Errors
/// A readable message naming the path and what was expected.
pub fn check(path: &str) -> Result<(), String> {
    check_under(path, &crate::pty::repo_root()?)
}

fn check_under(path: &str, repo_root: &Path) -> Result<(), String> {
    if path.is_empty() || path.contains('\0') || !Path::new(path).is_absolute() {
        return Err(format!(
            "`{path}` is not an absolute path; the editor host resolves nothing against its own \
             working directory."
        ));
    }
    let p = Path::new(path);
    // Split on both separators: after a `\\?\` prefix `/` is not one, so `components()` misses it.
    if p.components().any(|c| c == Component::ParentDir)
        || path.split(['/', '\\']).any(|s| s == "..")
    {
        return Err(format!(
            "`{path}` climbs with `..`; pass the resolved absolute path."
        ));
    }
    let outside = || {
        format!(
            "`{path}` is outside the engine checkout and every Ludion project: the editor host \
             only reads and writes under the repo (`{}`) or a directory holding a ludion.toml.",
            repo_root.display()
        )
    };
    let existing = nearest_existing(p).ok_or_else(outside)?;
    let canonical = std::fs::canonicalize(existing).map_err(|_| outside())?;
    if let Ok(root) = std::fs::canonicalize(repo_root)
        && starts_with_ignore_case(&canonical, &root)
    {
        return Ok(());
    }
    for dir in canonical.ancestors() {
        if dir.join("ludion.toml").is_file() && crate::project::read_project(dir).is_ok() {
            return Ok(());
        }
    }
    Err(outside())
}

fn nearest_existing(path: &Path) -> Option<&Path> {
    path.ancestors().find(|a| a.exists())
}

fn starts_with_ignore_case(path: &Path, prefix: &Path) -> bool {
    let mut p = path.components();
    prefix.components().all(|pc| {
        p.next().is_some_and(|c| {
            c.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&pc.as_os_str().to_string_lossy())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    const MANIFEST: &str = "name = \"t\"\ndefault_scene = \"scenes/main.yaml\"\n";

    struct Fixture {
        base: PathBuf,
        repo: PathBuf,
        project: PathBuf,
        outside: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let base =
                std::env::temp_dir().join(format!("ludion-confine-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&base);
            let base = {
                fs::create_dir_all(&base).unwrap();
                fs::canonicalize(&base).unwrap()
            };
            let repo = base.join("repo");
            let project = base.join("project");
            let outside = base.join("outside");
            for d in [&repo, &project, &outside] {
                fs::create_dir_all(d).unwrap();
            }
            fs::write(project.join("ludion.toml"), MANIFEST).unwrap();
            Self {
                base,
                repo,
                project,
                outside,
            }
        }
        fn check(&self, p: &Path) -> Result<(), String> {
            check_under(&p.to_string_lossy(), &self.repo)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    #[test]
    fn repo_paths_are_allowed() {
        let f = Fixture::new("repo");
        fs::write(f.repo.join("a.txt"), "x").unwrap();
        assert!(f.check(&f.repo.join("a.txt")).is_ok());
        assert!(f.check(&f.repo).is_ok());
    }

    #[test]
    fn project_paths_are_allowed_existing_or_not() {
        let f = Fixture::new("project");
        fs::write(f.project.join("a.txt"), "x").unwrap();
        assert!(f.check(&f.project).is_ok());
        assert!(f.check(&f.project.join("a.txt")).is_ok());
        assert!(f.check(&f.project.join("missing.txt")).is_ok());
        assert!(f.check(&f.project.join("a/b/c/new.txt")).is_ok());
    }

    #[test]
    fn a_directory_without_a_manifest_is_refused_and_named() {
        let f = Fixture::new("outside");
        let err = f.check(&f.outside.join("x.txt")).unwrap_err();
        assert!(err.contains("outside the engine checkout"), "{err}");
        assert!(err.contains("x.txt"), "{err}");
    }

    #[test]
    fn a_malformed_manifest_does_not_make_a_root() {
        let f = Fixture::new("malformed");
        fs::write(f.outside.join("ludion.toml"), "not = [valid").unwrap();
        assert!(f.check(&f.outside.join("x.txt")).is_err());
    }

    #[test]
    fn relative_paths_are_refused() {
        let f = Fixture::new("relative");
        let err = check_under("scenes/main.yaml", &f.repo).unwrap_err();
        assert!(err.contains("not an absolute path"), "{err}");
        assert!(check_under("", &f.repo).is_err());
    }

    #[test]
    fn parent_dir_is_refused() {
        let f = Fixture::new("dotdot");
        let p = format!("{}/../x", f.project.display());
        let err = check_under(&p, &f.repo).unwrap_err();
        assert!(err.contains("climbs with `..`"), "{err}");
    }

    #[test]
    fn repo_path_in_other_case_is_allowed_on_case_insensitive_fs() {
        let f = Fixture::new("case");
        fs::write(f.base.join("Probe"), "x").unwrap();
        if !f.base.join("probe").exists() {
            return;
        }
        fs::write(f.repo.join("a.txt"), "x").unwrap();
        let upper = f.base.join("REPO").join("a.txt");
        assert!(f.check(&upper).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_project_is_refused() {
        let f = Fixture::new("symlink");
        fs::write(f.outside.join("secret.txt"), "x").unwrap();
        std::os::unix::fs::symlink(&f.outside, f.project.join("link")).unwrap();
        assert!(f.check(&f.project.join("link/secret.txt")).is_err());
    }
}
