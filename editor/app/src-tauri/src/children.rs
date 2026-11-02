//! The host's one-shot children — `cargo run … gen`, `check-scene` — owned by a registry so none
//! outlives the editor or runs without bound.
//!
//! [`HostChildren::run`] spawns, registers, drains both pipes and polls to the end; a deadline or
//! [`HostChildren::kill_all`] stops the child's whole **process tree**, not just the process:
//! the child is a `cargo run` wrapper whose real work (rustc, the verb itself) is a descendant, and
//! killing the wrapper alone neither stops that work nor frees the pipes it holds. Windows does not
//! kill a child with its parent, hence `taskkill /T`; elsewhere the child leads its own process
//! group and the group is signalled. Nothing here names gen, cargo or Tauri.
//!
//! This module owns every spawn in the host: [`HostChildren::run`] for one-shot children,
//! [`spawn_tree`] / [`spawn_direct`] for the long-lived ones the stage supervises itself.

use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// A cold `cargo run` build, allowed on top of whatever the verb itself may take.
pub const BUILD_ALLOWANCE: Duration = Duration::from_secs(30 * 60);

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const POLL: Duration = Duration::from_millis(50);
/// How long to wait for a pipe's reader after the child is reaped. After a tree kill every pipe
/// holder is dead so EOF is immediate; the bound is for a holder that escaped the tree.
const DRAIN_WAIT: Duration = Duration::from_secs(2);

/// What a finished child left behind.
#[derive(Debug)]
pub struct RunOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// `None` if the child was killed.
    pub code: Option<i32>,
    /// The deadline, not a kill request, stopped it.
    pub timed_out: bool,
    /// A kill request ([`HostChildren::kill`], `kill_all`), not the deadline, stopped it.
    pub cancelled: bool,
}

#[derive(Default)]
pub struct HostChildren {
    next: AtomicU64,
    live: Mutex<HashMap<u64, Arc<AtomicBool>>>,
}

/// Deregisters on every exit path, a panic included.
struct Registration<'a> {
    owner: &'a HostChildren,
    id: u64,
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        if let Ok(mut live) = self.owner.live.lock() {
            live.remove(&self.id);
        }
    }
}

impl HostChildren {
    /// Run `command` to completion or `deadline`, whichever is first.
    pub fn run(&self, command: Command, deadline: Duration) -> std::io::Result<RunOutput> {
        self.run_with(command, deadline, |_| {})
    }

    /// [`run`](Self::run), reporting the child's registry id (for [`kill`](Self::kill)) once it is
    /// registered.
    pub fn run_with(
        &self,
        mut command: Command,
        deadline: Duration,
        on_start: impl FnOnce(u64),
    ) -> std::io::Result<RunOutput> {
        tree_posture(&mut command);

        // Registered before the spawn so a `kill_all` racing it still reaches this child.
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let flag = Arc::new(AtomicBool::new(false));
        self.live
            .lock()
            .map_err(|_| std::io::Error::other("child registry poisoned"))?
            .insert(id, flag.clone());
        let _registration = Registration { owner: self, id };

        let mut child = command.spawn()?;
        on_start(id);
        let stdout = drain(child.stdout.take());
        let stderr = drain(child.stderr.take());

        let start = Instant::now();
        let mut killed = false;
        let mut timed_out = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(err) => {
                    kill_tree(&mut child);
                    return Err(err);
                }
            }
            if !killed {
                let expired = start.elapsed() >= deadline;
                if expired || flag.load(Ordering::SeqCst) {
                    timed_out = expired && !flag.load(Ordering::SeqCst);
                    kill_tree(&mut child);
                    killed = true;
                }
            }
            thread::sleep(POLL);
        };
        Ok(RunOutput {
            stdout: stdout.recv_timeout(DRAIN_WAIT).unwrap_or_default(),
            stderr: stderr.recv_timeout(DRAIN_WAIT).unwrap_or_default(),
            code: if killed { None } else { status.code() },
            timed_out,
            cancelled: killed && !timed_out,
        })
    }

    /// Ask one child to stop; `false` if it is not (or no longer) registered.
    pub fn kill(&self, id: u64) -> bool {
        match self
            .live
            .lock()
            .ok()
            .and_then(|live| live.get(&id).cloned())
        {
            Some(flag) => {
                flag.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    /// Stop every live child and wait up to `grace` for the registry to empty; whether it did.
    pub fn kill_all(&self, grace: Duration) -> bool {
        let end = Instant::now() + grace;
        loop {
            match self.live.lock() {
                Ok(live) => {
                    for flag in live.values() {
                        flag.store(true, Ordering::SeqCst);
                    }
                    if live.is_empty() {
                        return true;
                    }
                }
                Err(_) => return false,
            }
            if Instant::now() >= end {
                return false;
            }
            thread::sleep(POLL);
        }
    }

    /// How many children are registered.
    #[cfg(test)]
    pub fn live(&self) -> usize {
        self.live.lock().map(|live| live.len()).unwrap_or(0)
    }
}

/// Read a pipe to EOF on its own thread: a child that fills an undrained pipe blocks forever.
fn drain<R: std::io::Read + Send + 'static>(pipe: Option<R>) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        let _ = tx.send(bytes);
    });
    rx
}

/// Stdin null, stdout and stderr piped, and no console window on Windows: how the host runs every
/// child, so none inherits the host's terminal or flashes a window.
pub fn spawn_posture(command: &mut Command) {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(CREATE_NO_WINDOW);
    }
}

/// [`spawn_posture`] plus a process group of its own on unix, so [`kill_tree`] can signal the
/// whole tree.
fn tree_posture(command: &mut Command) {
    spawn_posture(command);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
}

/// Spawn a child that may have descendants (a `cargo` wrapper); end it with [`kill_tree`].
pub fn spawn_tree(command: &mut Command) -> std::io::Result<Child> {
    tree_posture(command);
    command.spawn()
}

/// Spawn a direct executable with no descendants; `Child::kill` ends it.
pub fn spawn_direct(command: &mut Command) -> std::io::Result<Child> {
    spawn_posture(command);
    command.spawn()
}

/// End `child` and its whole process tree.
pub fn kill_tree(child: &mut Child) {
    let pid = child.id().to_string();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        let _ = Command::new("taskkill")
            .args(["/pid", &pid, "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

    /// Keeps a fork from landing while a test holds a port lock. `flock(2)` belongs to the open
    /// file *description*, which a `fork()` duplicates, and portable-pty spawns by fork+exec (its
    /// `pre_exec`), so a pty test forking inside a lock test's hold-then-drop window leaves the
    /// lock held in the forked child until it execs. Forking tests take the read side, lock tests
    /// that assert "released" the write side.
    static FORK_GUARD: RwLock<()> = RwLock::new(());

    /// Held while spawning through a fork+exec path.
    pub(crate) fn forking() -> RwLockReadGuard<'static, ()> {
        FORK_GUARD.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// Held for the whole body of a test that asserts a port lock is free after its holder dropped.
    pub(crate) fn no_forks() -> RwLockWriteGuard<'static, ()> {
        FORK_GUARD.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// A command re-running this test binary on one `#[test]`, gated by `env` so an ordinary test
    /// run of that test is a no-op (the stage tests' `sleeper_child` idiom).
    pub(crate) fn child_of(test: &str, env: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", test, "--nocapture"]).env(env, "1");
        command
    }

    #[test]
    fn sleeper_child() {
        if std::env::var_os("LUDION_CHILDREN_SLEEPER").is_some() {
            thread::sleep(Duration::from_secs(30));
        }
    }

    /// Sleeps 4 s, then writes the file named by `LUDION_CHILDREN_MARKER`: proof it survived.
    #[test]
    fn marker_child() {
        if let Some(path) = std::env::var_os("LUDION_CHILDREN_MARKER") {
            thread::sleep(Duration::from_secs(4));
            let _ = std::fs::write(path, b"alive");
        }
    }

    /// Spawns a [`marker_child`] grandchild that inherits this process's stdout, then sleeps.
    #[test]
    #[allow(clippy::zombie_processes)] // the tree kill, not a wait, is what ends it
    fn grandparent_child() {
        if std::env::var_os("LUDION_CHILDREN_GRANDPARENT").is_some() {
            let _grandchild = child_of("children::tests::marker_child", "LUDION_CHILDREN_UNUSED")
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            thread::sleep(Duration::from_secs(30));
        }
    }

    #[test]
    fn talker_child() {
        if std::env::var_os("LUDION_CHILDREN_TALKER").is_some() {
            println!("said-out");
            eprintln!("said-err");
            std::process::exit(7);
        }
    }

    #[test]
    fn a_child_past_its_deadline_is_killed() {
        let children = HostChildren::default();
        let start = Instant::now();
        let out = children
            .run(
                child_of("children::tests::sleeper_child", "LUDION_CHILDREN_SLEEPER"),
                Duration::from_millis(300),
            )
            .unwrap();
        assert!(out.timed_out);
        assert!(!out.cancelled);
        assert_eq!(out.code, None);
        assert!(start.elapsed() < Duration::from_secs(10));
        assert_eq!(children.live(), 0);
    }

    #[test]
    fn a_deadline_kill_takes_the_grandchild_too() {
        let marker =
            std::env::temp_dir().join(format!("ludion-children-marker-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let children = HostChildren::default();
        let mut command = child_of(
            "children::tests::grandparent_child",
            "LUDION_CHILDREN_GRANDPARENT",
        );
        command.env("LUDION_CHILDREN_MARKER", &marker);
        let out = children.run(command, Duration::from_millis(500)).unwrap();
        assert!(out.timed_out);
        // A plain child kill leaves the grandchild alive to write its marker.
        thread::sleep(Duration::from_secs(5));
        let survived = marker.exists();
        let _ = std::fs::remove_file(&marker);
        assert!(!survived, "the grandchild outlived the kill");
    }

    #[test]
    fn kill_all_stops_a_running_child() {
        let children = Arc::new(HostChildren::default());
        let runner = {
            let children = children.clone();
            thread::spawn(move || {
                children.run(
                    child_of("children::tests::sleeper_child", "LUDION_CHILDREN_SLEEPER"),
                    Duration::from_secs(60),
                )
            })
        };
        while children.live() == 0 {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(children.kill_all(Duration::from_secs(10)));
        let out = runner.join().unwrap().unwrap();
        assert!(!out.timed_out, "a kill request is not a deadline");
        assert!(out.cancelled);
        assert_eq!(out.code, None);
        assert_eq!(children.live(), 0);
    }

    #[test]
    fn kill_by_id_stops_only_that_child() {
        let children = Arc::new(HostChildren::default());
        let (tx, rx) = mpsc::channel();
        let runner = {
            let children = children.clone();
            thread::spawn(move || {
                children.run_with(
                    child_of("children::tests::sleeper_child", "LUDION_CHILDREN_SLEEPER"),
                    Duration::from_secs(60),
                    |id| tx.send(id).unwrap(),
                )
            })
        };
        let id = rx.recv().unwrap();
        assert!(!children.kill(id + 1000));
        assert!(children.kill(id));
        let out = runner.join().unwrap().unwrap();
        assert_eq!(out.code, None);
        assert!(out.cancelled);
        assert!(!children.kill(id), "deregistered once finished");
        assert_eq!(children.live(), 0);
    }

    #[test]
    fn a_fast_childs_output_and_exit_code_round_trip() {
        let children = HostChildren::default();
        let out = children
            .run(
                child_of("children::tests::talker_child", "LUDION_CHILDREN_TALKER"),
                Duration::from_secs(60),
            )
            .unwrap();
        assert!(!out.timed_out);
        assert_eq!(out.code, Some(7));
        assert!(String::from_utf8_lossy(&out.stdout).contains("said-out"));
        assert!(String::from_utf8_lossy(&out.stderr).contains("said-err"));
    }

    /// Class gate: a child spawned with a bare `.output()` / `.spawn()` is invisible to app exit,
    /// has no deadline or is killed without its tree. Every one goes through this module.
    #[test]
    fn no_host_source_spawns_a_child_outside_children() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(&src).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|ext| ext != "rs") || path.ends_with("children.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let tests = [
                "#[cfg(test)]\nmod tests",
                "#[cfg(test)]\npub(crate) mod tests",
            ]
            .iter()
            .filter_map(|marker| text.find(marker))
            .min()
            .unwrap_or(text.len());
            let production = &text[..tests];
            if production.contains(".output()") || production.contains(".spawn()") {
                offenders.push(path.file_name().unwrap().to_string_lossy().into_owned());
            }
        }
        assert!(
            offenders.is_empty(),
            "bare `.output()`/`.spawn()` in {offenders:?}: spawn through \
             `children::{{spawn_tree, spawn_direct}}` or `HostChildren`"
        );
    }
}
