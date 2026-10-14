//! A debounced, coalescing filesystem watcher for hot-reload loops.
//!
//! [`watch`] wraps `notify`'s native OS watcher (engine-architecture.md §3) in a background thread
//! that coalesces bursts of change events into single batches of changed paths. `ludion dev` watches
//! a game's `src/` with it; `ludion-server` watches the project root with it for asset reloads. Like
//! the rest of this crate it is **GPU-free by boundary rule** — it deals in paths and strings, never
//! a `wgpu` type.
//!
//! The debounce is a *quiet-period* coalesce: once a change arrives, the batch keeps growing until
//! `debounce` elapses with no further change, then the whole batch is delivered as one message. A
//! storm of edits (an editor writing several files, a formatter rewriting a tree, `cargo` touching
//! files) becomes one batch, not a queue of stale ones — the flake-and-storm defense the plan's risk
//! list calls for.

use std::path::Path;
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use notify::{Event, EventKind, RecursiveMode, Watcher};

/// A running filesystem watch. Dropping it stops the OS watch and joins the debounce thread — no
/// watcher and no thread outlives the handle.
///
/// Both fields are `Option` only so [`Drop`] can take them in order: the watcher is dropped first
/// (closing the internal event channel, which lets the debounce thread's blocking receive return and
/// the thread exit), then the debounce thread is joined.
pub struct WatchHandle {
    watcher: Option<notify::RecommendedWatcher>,
    debounce: Option<JoinHandle<()>>,
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        // Drop the OS watcher first: it owns the sender feeding the debounce thread, so dropping it
        // disconnects that channel and the thread's `recv`/`recv_timeout` returns, ending the loop.
        self.watcher = None;
        if let Some(handle) = self.debounce.take() {
            let _ = handle.join();
        }
    }
}

/// Starts watching `root` recursively, coalescing bursts of change events into batches of changed
/// paths.
///
/// Returns a receiver of change batches and a [`WatchHandle`] whose drop stops the watch. Each batch
/// is a non-empty, de-duplicated, sorted list of the paths (as lossy UTF-8 strings) that changed
/// during one quiet-period window. Access-only events (a file being *read*) are filtered out, so a
/// reader never triggers a batch.
///
/// # Errors
/// `L1103` if the OS watcher cannot be created, cannot begin watching `root`, or the debounce thread
/// cannot be spawned.
///
/// # Example
/// ```no_run
/// use std::time::Duration;
/// use std::path::Path;
///
/// let (changes, _handle) = ludion_assets::watch(Path::new("src"), Duration::from_millis(300))?;
/// // `_handle` must stay alive: dropping it stops the watch.
/// while let Ok(batch) = changes.recv() {
///     println!("{} path(s) changed", batch.len());
/// }
/// # Ok::<(), ludion_core::diagnostics::Diagnostic>(())
/// ```
pub fn watch(
    root: &Path,
    debounce: Duration,
) -> Result<(Receiver<Vec<String>>, WatchHandle), Diagnostic> {
    let (raw_tx, raw_rx) = crossbeam_channel::unbounded::<Event>();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
        // A watcher-internal error (a transient overflow, say) is dropped rather than crashing the
        // thread — the next good event still coalesces. Only startup failure (below) is fatal.
        if let Ok(event) = result {
            let _ = raw_tx.send(event);
        }
    })
    .map_err(|error| watcher_error(root, &error))?;
    watcher
        .watch(root, RecursiveMode::Recursive)
        .map_err(|error| watcher_error(root, &error))?;

    let (batch_tx, batch_rx) = crossbeam_channel::unbounded::<Vec<String>>();
    let debounce_thread = std::thread::Builder::new()
        .name("ludion-watch-debounce".to_string())
        .spawn(move || debounce_loop(&raw_rx, &batch_tx, debounce))
        .map_err(|error| {
            Diagnostic::new(
                code::WATCHER_START_FAILED,
                format!("could not spawn the file-watch debounce thread: {error}"),
            )
        })?;

    Ok((
        batch_rx,
        WatchHandle {
            watcher: Some(watcher),
            debounce: Some(debounce_thread),
        },
    ))
}

/// The debounce thread body: block for the first event of a burst, then keep extending the batch
/// until `debounce` elapses with no further event, and deliver the coalesced batch as one message.
/// Ends when the raw channel disconnects (the watcher was dropped) or the consumer hangs up.
fn debounce_loop(raw: &Receiver<Event>, out: &Sender<Vec<String>>, debounce: Duration) {
    loop {
        // Block for the first event of a new burst; a disconnect means the watcher was dropped.
        let mut paths = match raw.recv() {
            Ok(event) => collect_paths(&event),
            Err(_) => return,
        };
        // Extend the burst until the channel goes quiet for `debounce`.
        loop {
            match raw.recv_timeout(debounce) {
                Ok(event) => paths.extend(collect_paths(&event)),
                Err(RecvTimeoutError::Timeout) => break,
                // Watcher dropped mid-burst: deliver what we have (if anything), then stop.
                Err(RecvTimeoutError::Disconnected) => {
                    let _ = deliver(out, paths);
                    return;
                }
            }
        }
        if deliver(out, paths).is_err() {
            return; // consumer hung up
        }
    }
}

/// De-duplicates and sorts `paths`, sending them as one batch when non-empty. Returns `Err` if the
/// consumer has hung up (so the loop can stop). An empty batch (e.g. only access events arrived) is
/// dropped without sending.
fn deliver(out: &Sender<Vec<String>>, paths: Vec<String>) -> Result<(), ()> {
    let batch = coalesce(paths);
    if batch.is_empty() {
        return Ok(());
    }
    out.send(batch).map_err(|_| ())
}

/// The changed paths carried by one event, as lossy UTF-8 strings — empty for access-only events (a
/// file being read is not a content change) and for kinds that carry no path.
fn collect_paths(event: &Event) -> Vec<String> {
    if matches!(event.kind, EventKind::Access(_)) {
        return Vec::new();
    }
    event
        .paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

/// De-duplicates and sorts a batch of changed paths — the pure core of [`deliver`], separated so it
/// can be unit-tested without a running watcher. Sorting makes a batch deterministic regardless of
/// the order the OS reported the events in.
fn coalesce(mut paths: Vec<String>) -> Vec<String> {
    paths.sort();
    paths.dedup();
    paths
}

/// Builds the `L1103` diagnostic for a watcher that could not start.
fn watcher_error(root: &Path, error: &notify::Error) -> Diagnostic {
    Diagnostic::new(
        code::WATCHER_START_FAILED,
        format!("could not watch `{}`: {error}", root.display()),
    )
    .with_help("check the directory exists and is readable; run `ludion explain L1103` for more")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesce_sorts_and_dedups() {
        let batch = coalesce(vec![
            "b.rs".to_string(),
            "a.rs".to_string(),
            "b.rs".to_string(),
        ]);
        assert_eq!(batch, vec!["a.rs".to_string(), "b.rs".to_string()]);
    }

    #[test]
    fn coalesce_of_empty_is_empty() {
        assert!(coalesce(Vec::new()).is_empty());
    }

    #[test]
    fn access_events_carry_no_paths() {
        use notify::event::{AccessKind, EventKind};
        let event = Event::new(EventKind::Access(AccessKind::Read))
            .add_path(std::path::PathBuf::from("read-only.rs"));
        assert!(collect_paths(&event).is_empty());
    }

    #[test]
    fn modify_events_carry_their_paths() {
        use notify::event::{EventKind, ModifyKind};
        let event = Event::new(EventKind::Modify(ModifyKind::Any))
            .add_path(std::path::PathBuf::from("changed.rs"));
        assert_eq!(collect_paths(&event), vec!["changed.rs".to_string()]);
    }

    /// The real thing: watch a temp dir, write a file, and receive a coalesced batch naming it.
    /// A generous timeout keeps it robust (a flaky watch is a P0 bug under the determinism pillar).
    #[test]
    fn watch_reports_a_written_file() {
        let dir = std::env::temp_dir().join(format!("ludion-watch-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp watch dir");

        let (changes, _handle) =
            watch(&dir, Duration::from_millis(50)).expect("watcher starts on a real dir");

        // Write after the watcher is established so the create/modify is observed.
        std::fs::write(dir.join("touched.rs"), "// hi").expect("write watched file");

        let batch = changes
            .recv_timeout(Duration::from_secs(5))
            .expect("a change batch arrives within the timeout");
        assert!(
            batch.iter().any(|p| p.ends_with("touched.rs")),
            "batch should name the written file, got {batch:?}",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn watching_a_missing_dir_is_l1103() {
        let missing = std::env::temp_dir().join("ludion-watch-definitely-missing-xyz");
        let _ = std::fs::remove_dir_all(&missing);
        // `WatchHandle` is not `Debug`, so match rather than `unwrap_err`.
        match watch(&missing, Duration::from_millis(50)) {
            Ok(_) => panic!("watching a missing directory must fail"),
            Err(error) => assert_eq!(error.code.as_str(), "L1103"),
        }
    }
}
