//! Shared by the integration tests that scaffold projects at a fixed scratch path.

use std::fs::{self, File, TryLockError};
use std::path::PathBuf;
use std::time::Duration;

// The crate's clippy denylist bans `Instant::now`, so the bound is a poll count: 1500 × 200 ms = 5 min.
const LOCK_POLL: Duration = Duration::from_millis(200);
const LOCK_POLLS: u32 = 1500;
const REMOVE_ATTEMPTS: u32 = 8;

/// Holds the cross-process lock on a stable scratch name; dropping it releases the lock.
pub struct ScratchLock {
    _file: File,
}

/// A fresh, empty scratch directory under this crate's target tmp dir (which lives under
/// `<root>/target`, so it is NOT inside `crates/*`/`games/*` — the workspace guard allows it).
///
/// The name is deliberately STABLE (not per-process unique): the scaffold shares the engine's
/// `CARGO_TARGET_DIR`, and cargo's fingerprint units are keyed by package name, not source path —
/// a same-named scaffold at a fresh path reuses the old path's dep-info and runs stale binaries.
/// One path per name keeps the fingerprints truthful, but two runs of the same test (a check lane and
/// a targeted run) would wipe each other's live project, so an exclusive lock on `<name>.lock`, beside
/// the directory and never in it, serializes them across processes. Bind the guard for the whole test
/// (`let (base, _lock) = …`, never `_`). A stale-dir removal failure panics here rather than
/// surfacing later as a confusing `ludion new` refusal.
pub fn stable_scratch(name: &str) -> (PathBuf, ScratchLock) {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    fs::create_dir_all(&root).expect("create target tmp dir");
    // Never truncate: another process may hold this file locked.
    let lock = File::options()
        .create(true)
        .write(true)
        .truncate(false)
        .open(root.join(format!("{name}.lock")))
        .expect("open scratch lock file");
    let mut polls = 0;
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) => {
                assert!(
                    polls < LOCK_POLLS,
                    "scratch lock {name}.lock in {root:?} still held after {:?}: another run of {name} holds it",
                    LOCK_POLL * LOCK_POLLS
                );
                if polls == 0 {
                    eprintln!("waiting for another run of {name}");
                }
                polls += 1;
                std::thread::sleep(LOCK_POLL);
            }
            Err(TryLockError::Error(e)) => panic!("could not lock scratch {name}: {e}"),
        }
    }

    let dir = root.join(name);
    // A grandchild's cwd inside the stale dir can hold it briefly on Windows.
    for attempt in 1..=REMOVE_ATTEMPTS {
        match fs::remove_dir_all(&dir) {
            Ok(()) => break,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => {
                assert!(
                    attempt < REMOVE_ATTEMPTS,
                    "stale scratch dir {dir:?} could not be removed: {e}"
                );
                std::thread::sleep(Duration::from_millis(250));
            }
        }
    }
    fs::create_dir_all(&dir).expect("create scratch dir");
    (dir, ScratchLock { _file: lock })
}
