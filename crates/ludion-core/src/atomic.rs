//! The one atomic file write: bytes land in a hidden temp sibling, then rename over the target.
//!
//! A reader, or a process kill, sees the old bytes or the new bytes, never a prefix; rename
//! replaces the target atomically on Windows and POSIX (`Plan/phase-16-profile.md` decision 4).
//! Kill-safe, not crash-durable: nothing is synced to disk, so after a power loss or OS crash the
//! target may hold the old bytes, the new bytes, or an empty or partial file (the rename can reach
//! disk before the data). A writer that emits several files puts the document or record that names
//! the others last, so a kill never leaves a document naming files that do not exist.
//! [`copy_atomic`] does the same for a file copy, and [`create_new_atomic`] is the no-clobber
//! variant that refuses an existing target.
//!
//! # Example
//!
//! ```
//! use ludion_core::atomic::write_atomic;
//!
//! let dir = std::env::temp_dir().join(format!("ludion-atomic-doc-{}", std::process::id()));
//! let target = dir.join("out.yaml");
//! write_atomic(&target, "a: 1\n").unwrap();
//! assert_eq!(std::fs::read_to_string(&target).unwrap(), "a: 1\n");
//! std::fs::remove_dir_all(&dir).unwrap();
//! ```

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// The temp file a write to `path` lands on first: `.{name}.{pid}.{n}.tmp` in the target's directory
/// (same volume, so the rename is atomic; hidden; the pid and a per-process counter keep concurrent
/// writers, in one process or two, from ever sharing a temp).
pub fn temp_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let n = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{name}.{}.{n}.tmp", std::process::id()))
}

/// Whether `name` (a file name, not a path) looks like a [`temp_sibling`] left by a crashed write.
pub fn is_temp_name(name: &str) -> bool {
    name.starts_with('.') && name.ends_with(".tmp")
}

/// Whether `name` is a [`temp_sibling`] of the file called `owned`: `.{owned}.{pid}.{n}.tmp`, with
/// only digits between the name and the suffix.
pub fn is_temp_name_for(name: &str, owned: &str) -> bool {
    name.strip_prefix('.')
        .and_then(|rest| rest.strip_prefix(owned))
        .and_then(|rest| rest.strip_prefix('.'))
        .and_then(|rest| rest.strip_suffix(".tmp"))
        .is_some_and(|ids| {
            let mut parts = ids.split('.');
            let digits = |part: Option<&str>| {
                part.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            };
            digits(parts.next()) && digits(parts.next()) && parts.next().is_none()
        })
}

/// Writes `bytes` to `path` through a [`temp_sibling`] and a rename, creating missing parent
/// directories. On failure the temp is removed and `path` is untouched.
pub fn write_atomic(path: &Path, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let temp = temp_sibling(path);
    if let Err(error) = fs::write(&temp, bytes) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    Ok(())
}

/// Publishes `bytes` at `path` only if `path` is absent: a [`temp_sibling`] is written whole, then
/// hard-linked into place, so the check is the write itself and `path` never exists half-written.
/// Creates missing parent directories. An existing `path` is `ErrorKind::AlreadyExists` and keeps
/// its bytes; the temp is always removed.
///
/// ```
/// use ludion_core::atomic::create_new_atomic;
///
/// let dir = std::env::temp_dir().join(format!("ludion-create-doc-{}", std::process::id()));
/// let target = dir.join("out.yaml");
/// create_new_atomic(&target, "a: 1\n").unwrap();
/// let again = create_new_atomic(&target, "a: 2\n").unwrap_err();
/// assert_eq!(again.kind(), std::io::ErrorKind::AlreadyExists);
/// assert_eq!(std::fs::read_to_string(&target).unwrap(), "a: 1\n");
/// std::fs::remove_dir_all(&dir).unwrap();
/// ```
pub fn create_new_atomic(path: &Path, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let temp = temp_sibling(path);
    if let Err(error) = fs::write(&temp, bytes) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    let linked = match fs::hard_link(&temp, path) {
        Err(error) if error.kind() == io::ErrorKind::Unsupported && !path.exists() => {
            fs::rename(&temp, path)
        }
        other => other,
    };
    let _ = fs::remove_file(&temp);
    linked
}

/// Writes several files, not all-or-nothing: parents are created and every temp is written before any
/// rename, and a failure there removes the temps and leaves every target untouched. Then the temps are
/// renamed one at a time in slice order, so a rename failing part-way leaves the earlier targets new
/// and the later ones old (their temps removed). Callers put the document that names the others last,
/// so the mix never names a missing file, and rerunning the writer repairs it.
pub fn write_all_atomic(files: &[(&Path, &[u8])]) -> io::Result<()> {
    let mut staged: Vec<PathBuf> = Vec::with_capacity(files.len());
    let discard = |staged: &[PathBuf]| {
        for temp in staged {
            let _ = fs::remove_file(temp);
        }
    };
    for (path, bytes) in files {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            && let Err(error) = fs::create_dir_all(parent)
        {
            discard(&staged);
            return Err(error);
        }
        let temp = temp_sibling(path);
        staged.push(temp.clone());
        if let Err(error) = fs::write(&temp, bytes) {
            discard(&staged);
            return Err(error);
        }
    }
    for (index, (path, _)) in files.iter().enumerate() {
        if let Err(error) = fs::rename(&staged[index], path) {
            discard(&staged[index..]);
            return Err(error);
        }
    }
    Ok(())
}

/// Copies `from` to `to` through a [`temp_sibling`] and a rename, creating missing parent
/// directories. On failure the temp is removed and `to` is untouched.
pub fn copy_atomic(from: &Path, to: &Path) -> io::Result<u64> {
    if let Some(parent) = to.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let temp = temp_sibling(to);
    let copied = match fs::copy(from, &temp) {
        Ok(copied) => copied,
        Err(error) => {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
    };
    if let Err(error) = fs::rename(&temp, to) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ludion-atomic-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entries(dir: &Path) -> usize {
        fs::read_dir(dir).unwrap().count()
    }

    #[test]
    fn success_writes_bytes_and_leaves_one_entry() {
        let dir = scratch("ok");
        write_atomic(&dir.join("a.txt"), "hello").unwrap();
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "hello");
        assert_eq!(entries(&dir), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_batch_writes_every_file() {
        let dir = scratch("batch-ok");
        let (a, b) = (dir.join("a.png"), dir.join("sub/b.yaml"));
        write_all_atomic(&[(&a, b"png"), (&b, b"yaml")]).unwrap();
        assert_eq!(fs::read(&a).unwrap(), b"png");
        assert_eq!(fs::read(&b).unwrap(), b"yaml");
        assert_eq!(entries(&dir), 2, "no temp left");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_batch_that_cannot_stage_its_second_file_leaves_the_first_target_alone() {
        let dir = scratch("batch-fail");
        let (a, blocker) = (dir.join("a.png"), dir.join("blocker"));
        fs::write(&a, "old").unwrap();
        fs::write(&blocker, "a file, not a directory").unwrap();
        let result = write_all_atomic(&[(&a, b"new"), (&blocker.join("b.yaml"), b"yaml")]);
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&a).unwrap(), "old");
        assert_eq!(entries(&dir), 2, "only the two files the test made");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replaces_an_existing_file() {
        let dir = scratch("replace");
        fs::write(dir.join("a.txt"), "old old old").unwrap();
        write_atomic(&dir.join("a.txt"), "new").unwrap();
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "new");
        assert_eq!(entries(&dir), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn creates_missing_parent_dirs() {
        let dir = scratch("parents");
        write_atomic(&dir.join("x/y/a.txt"), b"z").unwrap();
        assert_eq!(fs::read(dir.join("x/y/a.txt")).unwrap(), b"z");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn target_untouched_on_failure() {
        let dir = scratch("fail");
        let target = dir.join("t");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep"), "k").unwrap();
        assert!(write_atomic(&target, "x").is_err());
        assert_eq!(fs::read_to_string(target.join("keep")).unwrap(), "k");
        assert_eq!(entries(&dir), 1);
        assert_eq!(entries(&target), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn parent_that_is_a_file_errs_and_creates_nothing() {
        let dir = scratch("parentfile");
        fs::write(dir.join("f"), "x").unwrap();
        assert!(write_atomic(&dir.join("f/a.txt"), "x").is_err());
        assert_eq!(entries(&dir), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn temp_names_are_recognised() {
        let temp = temp_sibling(Path::new("d/mesh.mesh.yaml"));
        let name = temp.file_name().unwrap().to_str().unwrap();
        assert!(is_temp_name(name), "{name}");
        assert_eq!(temp.parent().unwrap(), Path::new("d"));
        assert!(!is_temp_name("mesh.mesh.yaml"));
        assert!(is_temp_name_for(name, "mesh.mesh.yaml"));
    }

    #[test]
    fn temp_names_belong_to_one_file() {
        assert!(is_temp_name_for(".a.png.12.0.tmp", "a.png"));
        assert!(!is_temp_name_for(".a.png.12.0.tmp", "b.png"));
        assert!(!is_temp_name_for(".a.png.x.0.tmp", "a.png"));
        assert!(!is_temp_name_for(".a.png.12.tmp", "a.png"));
        assert!(!is_temp_name_for(".other.tmp", "a.png"));
    }

    #[test]
    fn replacing_a_target_leaves_its_other_hard_links_alone() {
        let dir = scratch("hardlink");
        let (target, alias) = (dir.join("t.txt"), dir.join("alias.txt"));
        fs::write(&target, "old").unwrap();
        fs::hard_link(&target, &alias).unwrap();
        write_atomic(&target, "new").unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        assert_eq!(fs::read_to_string(&alias).unwrap(), "old");
        assert_eq!(entries(&dir), 2);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copy_atomic_copies_and_leaves_hard_links_alone() {
        let dir = scratch("copy");
        let (source, target, alias) = (dir.join("s.txt"), dir.join("t.txt"), dir.join("alias.txt"));
        fs::write(&source, "new").unwrap();
        fs::write(&target, "old").unwrap();
        fs::hard_link(&target, &alias).unwrap();
        assert_eq!(copy_atomic(&source, &target).unwrap(), 3);
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        assert_eq!(fs::read_to_string(&alias).unwrap(), "old");
        assert_eq!(entries(&dir), 3);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copy_atomic_missing_source_errs_and_creates_nothing() {
        let dir = scratch("copymissing");
        assert!(copy_atomic(&dir.join("absent"), &dir.join("t.txt")).is_err());
        assert_eq!(entries(&dir), 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn create_new_refuses_an_existing_target_and_leaves_no_temp() {
        let dir = scratch("create-new");
        let target = dir.join("sub/a.txt");
        create_new_atomic(&target, "first").unwrap();
        let error = create_new_atomic(&target, "second").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(&target).unwrap(), "first");
        assert_eq!(entries(&dir.join("sub")), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn two_temps_for_one_target_differ() {
        let target = Path::new("d/a.txt");
        assert_ne!(temp_sibling(target), temp_sibling(target));
    }
}
