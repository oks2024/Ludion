//! Native PTY host for the editor's embedded terminals.
//!
//! Generic spawn/write/resize/kill/list surface over `portable-pty` (ConPTY on
//! Windows). Nothing here is Claude-specific — persona/command assembly lives in
//! the frontend; this module only runs whatever program it is handed and streams
//! the raw PTY bytes back out through an [`PtyEventSink`].
//!
//! The registry ([`PtyRegistry`]) is generic over the sink so its plumbing is
//! testable without a Tauri `AppHandle` (see the tests at the bottom of the file).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as _;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};

/// Waiter poll cadence: how often the waiter thread checks for child exit / a kill
/// request. Small enough to feel instant, large enough to be free at idle.
const WAIT_POLL: Duration = Duration::from_millis(50);

/// Drain poll cadence for [`PtyRegistry::shutdown`]. Tighter than [`WAIT_POLL`] because a caller is
/// blocked on it — teardown is the one place latency is paid synchronously.
const DRAIN_POLL: Duration = Duration::from_millis(10);

/// A cursor-position report for row 1, column 1 — the answer to the `ESC [ 6 n` device-status
/// query. See [`ConPty::drop`] for why teardown sends one.
const CURSOR_POSITION_REPORT: &[u8] = b"\x1b[1;1R";

/// How long a dropped registry gives its sessions to close. Bounded because this can run on the
/// app's exit path, and generous because the alternative to waiting is a leaked pseudoconsole.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Env vars accepted either as ordered pairs (`[["K","V"], …]`) or a JS object.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum EnvVars {
    Pairs(Vec<(String, String)>),
    Map(HashMap<String, String>),
}

impl Default for EnvVars {
    fn default() -> Self {
        EnvVars::Pairs(Vec::new())
    }
}

impl EnvVars {
    fn for_each(&self, mut f: impl FnMut(&str, &str)) {
        match self {
            EnvVars::Pairs(v) => v.iter().for_each(|(k, val)| f(k, val)),
            EnvVars::Map(m) => m.iter().for_each(|(k, val)| f(k, val)),
        }
    }
}

fn default_cols() -> u16 {
    80
}
fn default_rows() -> u16 {
    24
}

/// Parameters for [`PtyRegistry::spawn`]. Field names are the pinned frontend contract.
#[derive(Debug, Clone, Deserialize)]
pub struct SpawnOpts {
    pub id: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub env: EnvVars,
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,
}

/// Structured, non-panicking failure surface. Serializes to a readable string at
/// the command boundary.
#[derive(Debug)]
pub enum PtyError {
    AlreadyLive(String),
    NoSession(String),
    CmdlineTooLong(String),
    Spawn(String),
    Io(String),
}

impl std::fmt::Display for PtyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PtyError::AlreadyLive(id) => write!(f, "pty session '{id}' is already live"),
            PtyError::NoSession(id) => write!(f, "no live pty session '{id}'"),
            PtyError::CmdlineTooLong(msg) => write!(f, "{msg}"),
            PtyError::Spawn(msg) => write!(f, "failed to spawn pty: {msg}"),
            PtyError::Io(msg) => write!(f, "pty io error: {msg}"),
        }
    }
}

impl std::error::Error for PtyError {}

/// The OS ceiling on an assembled command line. Windows' `CreateProcess` caps
/// `lpCommandLine` at 32,767 UTF-16 units; the Unix limit is on the whole argv+envp
/// block, and macOS' `ARG_MAX` is the tighter of the two we ship on.
#[cfg(windows)]
const CMDLINE_LIMIT: usize = 32_767;
#[cfg(not(windows))]
const CMDLINE_LIMIT: usize = 262_144;

/// Conservative upper bound on the assembled command line, in the units the OS counts.
/// Deliberately over-estimates the per-argument quoting (a quote or backslash may need one
/// escape character): a check that trips slightly early is correct, one that trips late is not.
fn cmdline_units(program: &str, args: &[String]) -> usize {
    let quoted = |s: &str| s.encode_utf16().count() + 3 + s.matches(['"', '\\']).count();
    quoted(program) + args.iter().map(|a| quoted(a)).sum::<usize>()
}

/// Reject an over-long command line before spawning, naming the argument responsible.
/// `CreateProcess` otherwise fails with an opaque OS error, and the argument that blows the
/// budget is always the big one (today: `--append-system-prompt`, carrying the persona file).
fn check_cmdline(program: &str, args: &[String]) -> Result<(), PtyError> {
    let total = cmdline_units(program, args);
    if total <= CMDLINE_LIMIT {
        return Ok(());
    }
    let biggest = args
        .iter()
        .enumerate()
        .max_by_key(|(_, a)| a.encode_utf16().count());
    let culprit = match biggest {
        Some((i, arg)) => {
            let len = arg.encode_utf16().count();
            // Name the flag it belongs to when there is one — `--append-system-prompt` is far
            // more actionable than "argument 4".
            match args.get(i.wrapping_sub(1)).filter(|f| f.starts_with("--")) {
                Some(flag) if i > 0 => format!(" The largest argument is `{flag}` ({len} chars)."),
                _ => format!(" The largest argument is {len} chars."),
            }
        }
        None => String::new(),
    };
    Err(PtyError::CmdlineTooLong(format!(
        "command line is {total} characters, over this OS's {CMDLINE_LIMIT}-character limit.{culprit} \
         The persona file is the usual cause — trim `editor/personas/<view>.md` (it is passed inline \
         as the appended system prompt) and try again."
    )))
}

/// Receives the raw output/exit stream of every session. Cloned into each
/// reader thread, so it must be cheap to clone and `Send`.
pub trait PtyEventSink: Clone + Send + 'static {
    /// Raw PTY bytes. Never pre-decoded to `String` here: a read can split a
    /// UTF-8 multibyte sequence, so lossy decoding at this seam would corrupt it.
    fn output(&self, id: &str, bytes: &[u8]);
    /// Fired exactly once when the child exits (including after an explicit kill).
    fn exit(&self, id: &str, code: Option<i32>);
}

/// An open pseudoconsole, counted for as long as the OS holds it.
///
/// Dropping the master IS `ClosePseudoConsole` on Windows, and it can block — so the count falls
/// only once the OS has actually released the host process, which is precisely what
/// [`PtyRegistry::shutdown`] waits on. Wrapping it (rather than counting at the two call sites) is
/// what makes the count true no matter *who* drops the session: the closer thread on a normal exit,
/// `shutdown`, or the registry's own `Drop`.
///
/// The stakes are not bookkeeping: a pseudoconsole that outlives its owner is an orphaned
/// `conhost.exe` spinning a full core until the machine reboots. [`ConPty::drop`] carries the how
/// and the why.
struct ConPty {
    /// Both `Some` until `Drop`, which takes them in a load-bearing order (see below).
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Option<Box<dyn Write + Send>>,
    open: Arc<AtomicUsize>,
}

impl ConPty {
    fn new(
        master: Box<dyn MasterPty + Send>,
        writer: Box<dyn Write + Send>,
        open: Arc<AtomicUsize>,
    ) -> Self {
        open.fetch_add(1, Ordering::SeqCst);
        Self {
            master: Some(master),
            writer: Some(writer),
            open,
        }
    }

    fn write(&mut self, data: &str) -> Result<(), PtyError> {
        match self.writer.as_mut() {
            Some(writer) => writer
                .write_all(data.as_bytes())
                .and_then(|()| writer.flush())
                .map_err(|e| PtyError::Io(e.to_string())),
            None => Err(PtyError::Io("pseudoconsole is closing".to_string())),
        }
    }

    fn resize(&self, size: PtySize) -> Result<(), PtyError> {
        match self.master.as_ref() {
            Some(master) => master.resize(size).map_err(|e| PtyError::Io(e.to_string())),
            None => Err(PtyError::Io("pseudoconsole is closing".to_string())),
        }
    }
}

impl Drop for ConPty {
    fn drop(&mut self) {
        // ANSWER THE CURSOR QUERY BEFORE CLOSING, or this drop never returns.
        //
        // portable-pty creates the pseudoconsole with `PSEUDOCONSOLE_INHERIT_CURSOR`, so conhost
        // opens by emitting a DSR cursor-position query (`ESC [ 6 n`) and does not finish starting
        // up until a terminal answers it. A live xterm.js frontend does. Nobody else ever does —
        // and a conhost still stuck in that handshake cannot process `ClosePseudoConsole` either,
        // so the close blocks FOREVER (measured: no return in 90 s; answering it first, 10 ms) and
        // strands the host process as an orphan that spins a core on a headless session. This is
        // what accumulated four `conhost.exe --headless --inheritcursor` processes on the Windows
        // CI agent (2026-08-09) and is the reason this file used to describe the close as taking
        // "tens of seconds": it was not slow, it was hung, and the process exited before anyone saw
        // it finish.
        //
        // The race is why it looked intermittent: a session torn down before conhost emits the
        // query closes cleanly, so only the sessions that lived a few milliseconds longer leak.
        // Sending the report unprompted is harmless — the session is over, and a frontend that
        // already answered leaves conhost with nothing waiting on this one.
        if let Some(writer) = self.writer.as_mut() {
            let _ = writer.write_all(CURSOR_POSITION_REPORT);
            let _ = writer.flush();
        }
        // Then in this order: input handle, the close itself, and only then the count — a
        // `shutdown` that returned before the OS had released the host process would report a leak
        // as clean. The writer lives in this struct, rather than beside it in `Session`, precisely
        // so both steps are stated here instead of resting on field-declaration order.
        drop(self.writer.take());
        drop(self.master.take());
        self.open.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Live per-session handles held in the registry: the pseudoconsole (which owns resize and the
/// write half, so that its teardown order is expressible) and a kill flag the waiter thread
/// watches. The read half lives in the reader thread; the `Child` lives in the waiter thread.
///
/// Windows/ConPTY notes that shape this design:
/// - Dropping the master is `ClosePseudoConsole`, which BLOCKS until the attached
///   child exits, and the reader only sees EOF once the master is closed. So exit is
///   detected off the pty (via the child handle), not off reader EOF. It also blocks
///   forever unless conhost's opening cursor query has been answered — see [`ConPty::drop`],
///   which is where a session's teardown actually lives.
/// - `clone_killer()` can hand back a handle that reports `ERROR_INVALID_HANDLE`, so
///   we do not kill through a cloned handle; the waiter kills the child it owns.
///
/// The waiter thread polls the owned child, is the sole owner of the reap + `exit`
/// emit, and honours the kill flag — so kill and natural exit share one path.
struct Session {
    pty: ConPty,
    kill_flag: Arc<AtomicBool>,
}

pub struct PtyRegistry<S: PtyEventSink> {
    sessions: Arc<Mutex<HashMap<String, Session>>>,
    /// Pseudoconsoles the OS still holds. Rises with every successful spawn and falls only when a
    /// `ConPty` has finished closing — which is later than the session leaving `sessions`, since the
    /// close is deliberately offloaded. `shutdown` therefore has to wait on both.
    open_cons: Arc<AtomicUsize>,
    sink: S,
}

impl<S: PtyEventSink> PtyRegistry<S> {
    pub fn new(sink: S) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            open_cons: Arc::new(AtomicUsize::new(0)),
            sink,
        }
    }

    pub fn spawn(&self, opts: SpawnOpts) -> Result<(), PtyError> {
        let mut sessions = self.sessions.lock().unwrap();
        if sessions.contains_key(&opts.id) {
            return Err(PtyError::AlreadyLive(opts.id));
        }
        check_cmdline(&opts.program, &opts.args)?;

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: opts.rows,
                cols: opts.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::Spawn(e.to_string()))?;

        let mut cmd = CommandBuilder::new(&opts.program);
        cmd.args(&opts.args);
        if !opts.cwd.is_empty() {
            cmd.cwd(&opts.cwd);
        }
        opts.env.for_each(|k, v| {
            cmd.env(k, v);
        });

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| PtyError::Spawn(e.to_string()))?;
        // Release the slave side now: with it open the master never sees EOF when
        // the child exits, so the reader thread would block forever.
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| PtyError::Spawn(e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| PtyError::Spawn(e.to_string()))?;

        // Reader: forward raw bytes until EOF. EOF arrives only once the waiter
        // drops the master (see `Session` note), so this thread outlives the child.
        let reader_sink = self.sink.clone();
        let reader_id = opts.id.clone();
        let reader_thread = thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => reader_sink.output(&reader_id, &buf[..n]),
                }
            }
        });

        // Waiter: poll the owned child for exit (a blocking `wait` could not be
        // interrupted by a kill request). On a kill request, terminate the child it
        // owns (the owned handle works even where a cloned killer does not). Once the
        // child is gone, reap from the registry and emit `exit` immediately. Kill and
        // natural exit converge here, and `exit` fires exactly once.
        let kill_flag = Arc::new(AtomicBool::new(false));
        let waiter_flag = Arc::clone(&kill_flag);
        let sink = self.sink.clone();
        let sessions_for_thread = Arc::clone(&self.sessions);
        let id = opts.id.clone();
        thread::spawn(move || {
            let mut child = child;
            let code = loop {
                if waiter_flag.load(Ordering::Relaxed) {
                    let _ = child.kill();
                }
                match child.try_wait() {
                    Ok(Some(status)) => break Some(status.exit_code() as i32),
                    Ok(None) => thread::sleep(WAIT_POLL),
                    Err(_) => break None,
                }
            };
            let removed = sessions_for_thread.lock().unwrap().remove(&id);
            sink.exit(&id, code);
            // Closing the pseudoconsole (dropping the master) is not instant even when it is
            // healthy, so it is offloaded and never delays `exit`, the list-removal, or a same-id
            // respawn. Dropping the master also EOFs the reader, which this thread then joins.
            // Offloaded is not unobserved: `open_cons` falls only when the close has finished, so
            // `shutdown` can still wait for it.
            thread::spawn(move || {
                drop(removed);
                let _ = reader_thread.join();
            });
        });

        sessions.insert(
            opts.id,
            Session {
                pty: ConPty::new(pair.master, writer, Arc::clone(&self.open_cons)),
                kill_flag,
            },
        );
        Ok(())
    }

    /// Kill every live session and block until the OS has released every pseudoconsole, or
    /// `timeout` elapses. Returns whether it drained.
    ///
    /// Both halves of the wait are load-bearing: a session leaves `sessions` the moment the child is
    /// reaped, but its pseudoconsole closes later on a detached thread (see `spawn`). Waiting only
    /// on the map would return while the host process was still open — the exact leak this exists to
    /// prevent.
    pub fn shutdown(&self, timeout: Duration) -> bool {
        for session in self.sessions.lock().unwrap().values() {
            session.kill_flag.store(true, Ordering::Relaxed);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let drained = self.sessions.lock().unwrap().is_empty()
                && self.open_cons.load(Ordering::SeqCst) == 0;
            if drained {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(DRAIN_POLL);
        }
    }

    pub fn write(&self, id: &str, data: &str) -> Result<(), PtyError> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get_mut(id)
            .ok_or_else(|| PtyError::NoSession(id.to_string()))?;
        session.pty.write(data)
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), PtyError> {
        let sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get(id)
            .ok_or_else(|| PtyError::NoSession(id.to_string()))?;
        session.pty.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
    }

    pub fn kill(&self, id: &str) -> Result<(), PtyError> {
        let sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get(id)
            .ok_or_else(|| PtyError::NoSession(id.to_string()))?;
        // Signal only; the waiter thread terminates the child it owns, then reaps and
        // emits the single `exit`. Non-blocking, and there is no cloned-handle kill.
        session.kill_flag.store(true, Ordering::Relaxed);
        Ok(())
    }

    pub fn list(&self) -> Vec<String> {
        self.sessions.lock().unwrap().keys().cloned().collect()
    }
}

/// Closing the registry closes its pseudoconsoles. Windows does not take a child down with its
/// parent and does not close a pseudoconsole for a process that exits without doing so, so without
/// this every dropped registry — an exiting app, a finished test binary — leaves an orphaned
/// `conhost.exe` behind. Bounded rather than unbounded: on the exit path a hang would be worse than
/// the leak, and killing the children first (what `shutdown` does) is what makes the close prompt.
impl<S: PtyEventSink> Drop for PtyRegistry<S> {
    fn drop(&mut self) {
        let _ = self.shutdown(SHUTDOWN_GRACE);
    }
}

// ---- Tauri glue ------------------------------------------------------------

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

#[derive(Clone, Serialize)]
struct OutputPayload {
    id: String,
    #[serde(rename = "dataB64")]
    data_b64: String,
}

#[derive(Clone, Serialize)]
struct ExitPayload {
    id: String,
    code: Option<i32>,
}

/// Sink that forwards to the webview as Tauri events.
#[derive(Clone)]
pub struct AppSink {
    app: tauri::AppHandle,
}

impl PtyEventSink for AppSink {
    fn output(&self, id: &str, bytes: &[u8]) {
        use tauri::Emitter as _;
        let _ = self.app.emit(
            "pty://output",
            OutputPayload {
                id: id.to_string(),
                data_b64: B64.encode(bytes),
            },
        );
    }
    fn exit(&self, id: &str, code: Option<i32>) {
        use tauri::Emitter as _;
        let _ = self.app.emit(
            "pty://exit",
            ExitPayload {
                id: id.to_string(),
                code,
            },
        );
    }
}

pub type AppPtyRegistry = PtyRegistry<AppSink>;

pub fn registry(app: tauri::AppHandle) -> AppPtyRegistry {
    PtyRegistry::new(AppSink { app })
}

#[tauri::command]
pub fn pty_spawn(state: tauri::State<'_, AppPtyRegistry>, opts: SpawnOpts) -> Result<(), String> {
    state.spawn(opts).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn pty_write(
    state: tauri::State<'_, AppPtyRegistry>,
    id: String,
    data: String,
) -> Result<(), String> {
    state.write(&id, &data).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn pty_resize(
    state: tauri::State<'_, AppPtyRegistry>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    state.resize(&id, cols, rows).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn pty_kill(state: tauri::State<'_, AppPtyRegistry>, id: String) -> Result<(), String> {
    state.kill(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn pty_list(state: tauri::State<'_, AppPtyRegistry>) -> Vec<String> {
    state.list()
}

/// Atomically write `<cwd>/.ludion/editor-context.json` (the editor→agent context
/// bridge). Temp-file-then-rename so a reader never sees a half-written file.
#[tauri::command]
pub fn write_editor_context(cwd: String, json: String) -> Result<(), String> {
    let dir = Path::new(&cwd).join(".ludion");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let target = dir.join("editor-context.json");
    let tmp = dir.join("editor-context.json.tmp");
    std::fs::write(&tmp, json.as_bytes()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &target).map_err(|e| e.to_string())?;
    Ok(())
}

/// Read a UTF-8 text file for the frontend. Deliberately dumb: the drawer uses it to
/// tail a spawned instance's statusline sink (`<project>/.ludion/claude-status-<id>.json`),
/// which the host has no push channel for. A missing/locked file surfaces as an `Err`
/// string the caller treats as "no payload yet", never a crash.
#[tauri::command]
pub fn read_editor_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}

/// A file's modification time in milliseconds since the UNIX epoch — the Game Designer view's
/// change-poll signal for the open source file, and the baseline a save adopts.
#[derive(Debug, Clone, Serialize)]
pub struct FileStat {
    #[serde(rename = "mtimeMs")]
    pub mtime_ms: f64,
}

/// Read a file's mtime as epoch milliseconds. Errors (missing/unreadable) surface as a string.
fn file_mtime_ms(path: &Path) -> Result<f64, String> {
    let modified = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map_err(|e| e.to_string())?;
    let dur = modified
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?;
    Ok(dur.as_secs_f64() * 1000.0)
}

/// Stat a path — a file for the code panel's change poll, a directory for the frontend's
/// project-dir probe. A missing path is an `Err` the caller reads as "gone / not readable", never a
/// crash.
#[tauri::command]
pub fn stat_editor_file(path: String) -> Result<FileStat, String> {
    file_mtime_ms(Path::new(&path)).map(|mtime_ms| FileStat { mtime_ms })
}

/// Atomically write a text file (temp sibling + rename, so a reader never sees a half-written
/// file — the same posture as `write_editor_context`) and return the new mtime, which the
/// frontend adopts as its own baseline so its own write doesn't read back as an external change.
/// Missing parent directories are created first.
#[tauri::command]
pub fn write_editor_file(path: String, text: String) -> Result<FileStat, String> {
    let target = Path::new(&path);
    if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create `{}`: {e}", parent.display()))?;
    }
    let name = target
        .file_name()
        .ok_or_else(|| format!("not a file path: {path}"))?;
    let mut tmp_name = name.to_os_string();
    tmp_name.push(".tmp");
    let tmp = target.with_file_name(tmp_name);
    std::fs::write(&tmp, text.as_bytes()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    file_mtime_ms(target).map(|mtime_ms| FileStat { mtime_ms })
}

/// Repo-relative paths the drawer needs to assemble a `claude` spawn: the persona file it
/// passes to `--append-system-prompt-file` (read at spawn, so edits to the versioned file
/// take effect on the next terminal), the statusline relay script, and the repo root (for
/// the `ludion-mcp` dev invocation's `--manifest-path`). `launch_project` carries the process's
/// `--project` argument, if any (`crate::project::launch_project`), for the boot probe.
#[derive(Debug, Clone, Serialize)]
pub struct EditorPaths {
    #[serde(rename = "repoRoot")]
    pub repo_root: String,
    #[serde(rename = "personasDir")]
    pub personas_dir: String,
    #[serde(rename = "relayScript")]
    pub relay_script: String,
    #[serde(rename = "launchProject")]
    pub launch_project: Option<String>,
}

/// Resolve the repo root from this crate's compile-time location
/// (`<repo>/editor/app/src-tauri`). Correct for the dev/local-build editor — the same
/// posture as the `cargo run -p ludion-mcp` fallback — and the single point to revisit if
/// the editor is ever shipped as a relocatable bundle.
pub fn repo_root() -> Result<std::path::PathBuf, String> {
    Path::new(env!("CARGO_MANIFEST_DIR")) // <repo>/editor/app/src-tauri
        .parent() // <repo>/editor/app
        .and_then(Path::parent) // <repo>/editor
        .and_then(Path::parent) // <repo>
        .map(Path::to_path_buf)
        .ok_or_else(|| "could not resolve repo root from manifest dir".to_string())
}

/// The repo-relative paths the drawer needs, derived from [`repo_root`].
#[tauri::command]
pub fn editor_paths() -> Result<EditorPaths, String> {
    let repo_root = repo_root()?;
    let s = |p: std::path::PathBuf| p.to_string_lossy().into_owned();
    Ok(EditorPaths {
        repo_root: s(repo_root.clone()),
        personas_dir: s(repo_root.join("editor").join("personas")),
        relay_script: s(repo_root
            .join("editor")
            .join("terminal")
            .join("statusline-relay.mjs")),
        launch_project: crate::project::launch_project(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::time::{Duration, Instant};

    #[derive(Debug)]
    enum Ev {
        Output(Vec<u8>),
        Exit(Option<i32>),
    }

    #[derive(Clone)]
    struct TestSink {
        tx: Sender<Ev>,
    }

    impl PtyEventSink for TestSink {
        fn output(&self, _id: &str, bytes: &[u8]) {
            let _ = self.tx.send(Ev::Output(bytes.to_vec()));
        }
        fn exit(&self, _id: &str, code: Option<i32>) {
            let _ = self.tx.send(Ev::Exit(code));
        }
    }

    fn sink() -> (TestSink, Receiver<Ev>) {
        let (tx, rx) = channel();
        (TestSink { tx }, rx)
    }

    #[test]
    fn cmdline_check_passes_a_realistic_spawn_and_names_the_oversized_argument() {
        // A persona-sized system prompt plus the inline mcp/settings JSON stays well inside budget.
        let ok = vec![
            "--model".to_string(),
            "opus".to_string(),
            "--append-system-prompt".to_string(),
            "p".repeat(8_000),
            "--mcp-config".to_string(),
            "{\"mcpServers\":{}}".to_string(),
        ];
        assert!(check_cmdline("claude", &ok).is_ok());

        let over = vec![
            "--model".to_string(),
            "opus".to_string(),
            "--append-system-prompt".to_string(),
            "p".repeat(CMDLINE_LIMIT + 1),
        ];
        let msg = check_cmdline("claude", &over).unwrap_err().to_string();
        assert!(msg.contains("--append-system-prompt"), "{msg}");
        assert!(msg.contains("editor/personas"), "{msg}");
        assert!(!msg.contains("failed to spawn pty"), "{msg}");
    }

    #[test]
    fn cmdline_check_runs_before_anything_is_allocated() {
        let (s, _rx) = sink();
        let registry = PtyRegistry::new(s);
        let err = registry
            .spawn(SpawnOpts {
                id: "too-long".to_string(),
                program: "claude".to_string(),
                args: vec!["x".repeat(CMDLINE_LIMIT + 1)],
                cwd: String::new(),
                env: EnvVars::default(),
                cols: 80,
                rows: 24,
            })
            .unwrap_err()
            .to_string();
        assert!(err.contains("over this OS's"), "{err}");
        assert!(registry.list().is_empty());
    }

    // A long-lived child that writes something first. Natural-exit children (`echo` and
    // friends) are NOT used here: headless, no terminal emulator answers ConPTY's initial
    // `\x1b[6n` cursor-position query, so such a child can stall and never exit — a real
    // xterm frontend replies and unblocks it. The deterministic path is spawn -> kill.
    //
    // The child must emit on its own, because only Windows gets output for free: ConPTY's
    // init handshake reaches the reader before the child writes a byte, while a unix pty
    // carries exactly what the child sends and a bare `sleep` sends nothing. So the unix
    // side echoes a token first, then sleeps — long-lived and observable on both.
    fn long_opts(id: &str) -> SpawnOpts {
        let (program, args) = if cfg!(windows) {
            (
                "ping",
                vec!["-n".to_string(), "60".to_string(), "127.0.0.1".to_string()],
            )
        } else {
            (
                "sh",
                vec!["-c".to_string(), "echo ready; sleep 60".to_string()],
            )
        };
        SpawnOpts {
            id: id.to_string(),
            program: program.to_string(),
            args,
            cwd: String::new(),
            env: EnvVars::default(),
            cols: 80,
            rows: 24,
        }
    }

    /// Block until an event satisfying `pred` arrives (returning it), or time out.
    fn wait_for(rx: &Receiver<Ev>, timeout: Duration, pred: impl Fn(&Ev) -> bool) -> Option<Ev> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(ev) = rx.recv_timeout(Duration::from_millis(200))
                && pred(&ev)
            {
                return Some(ev);
            }
        }
        None
    }

    #[test]
    fn spawn_forwards_output_write_resize_then_kill_exits_and_reaps() {
        let (s, rx) = sink();
        let reg = PtyRegistry::new(s);
        reg.spawn(long_opts("s1")).unwrap();
        assert_eq!(reg.list(), vec!["s1".to_string()]);

        // Output plumbing: ConPTY's init handshake reliably reaches the reader/sink.
        match wait_for(&rx, Duration::from_secs(5), |e| matches!(e, Ev::Output(_))) {
            Some(Ev::Output(bytes)) => assert!(!bytes.is_empty(), "empty output chunk"),
            _ => panic!("no output was forwarded from the pty"),
        }

        // Write/resize succeed on a live session.
        reg.write("s1", "\r").unwrap();
        reg.resize("s1", 100, 40).unwrap();

        reg.kill("s1").unwrap();
        match wait_for(&rx, Duration::from_secs(10), |e| matches!(e, Ev::Exit(_))) {
            Some(Ev::Exit(_code)) => {}
            _ => panic!("kill did not produce an exit event"),
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        while !reg.list().is_empty() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(reg.list().is_empty(), "session was not reaped after kill");

        // Reaped is not closed: the pseudoconsole outlives the session by however long the detached
        // closer takes. Gate that too, or the test passes while leaking a `conhost.exe`.
        //
        // This is the test that catches the cursor-query deadlock (`ConPty::drop`), because it is
        // the one that waits for output — i.e. the one whose session lives long enough for conhost
        // to ask the question. It closes in ~10 ms when the answer is sent and never at all when it
        // is not, so the budget is generous rather than tuned.
        assert!(
            reg.shutdown(Duration::from_secs(10)),
            "pseudoconsole was still open after the session was reaped"
        );
    }

    #[test]
    fn duplicate_id_is_rejected() {
        let (s, rx) = sink();
        let reg = PtyRegistry::new(s);
        reg.spawn(long_opts("dup1")).unwrap();

        match reg.spawn(long_opts("dup1")) {
            Err(PtyError::AlreadyLive(id)) => assert_eq!(id, "dup1"),
            other => panic!("expected AlreadyLive, got {other:?}"),
        }

        // `kill` only raises a flag, so returning here would end the test — and, under a one-process
        // -per-test runner, the whole process — before the waiter had killed the child or closed the
        // pseudoconsole. That is how this test used to leak an orphaned host on every CI run.
        reg.kill("dup1").unwrap();
        assert!(
            wait_for(&rx, Duration::from_secs(10), |e| matches!(e, Ev::Exit(_))).is_some(),
            "kill did not produce an exit event"
        );
        assert!(
            reg.shutdown(Duration::from_secs(10)),
            "pseudoconsole leaked"
        );
    }

    /// The regression gate for the leak itself: a registry closed with a session still live must
    /// close that session's pseudoconsole, with the caller killing nothing first.
    ///
    /// **Waiting for output is what makes this a gate.** A session torn down before conhost emits
    /// its cursor-position query closes cleanly even with the deadlock present, so a test that
    /// spawned and immediately shut down would pass either way (this one did, before the wait was
    /// added). The first output chunk IS that query, so it is the signal that the session has
    /// reached the state that used to hang forever.
    ///
    /// It asserts on our own open-pseudoconsole count, never the OS process table: the CI agents and
    /// dev boxes this runs on have unrelated `conhost.exe` processes coming and going, and a flaky
    /// test is a P0 bug.
    #[test]
    fn shutdown_closes_a_pseudoconsole_that_asked_for_the_cursor() {
        let (s, rx) = sink();
        let reg = PtyRegistry::new(s);
        reg.spawn(long_opts("live")).unwrap();
        assert_eq!(reg.open_cons.load(Ordering::SeqCst), 1);
        assert!(
            wait_for(&rx, Duration::from_secs(5), |e| matches!(e, Ev::Output(_))).is_some(),
            "no output from the pty — the session never reached the handshake this gates"
        );

        assert!(
            reg.shutdown(Duration::from_secs(10)),
            "shutdown did not drain a live session"
        );
        assert!(reg.list().is_empty());
        assert_eq!(reg.open_cons.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn write_and_resize_on_missing_session_error() {
        let (s, _rx) = sink();
        let reg = PtyRegistry::new(s);
        assert!(matches!(
            reg.write("ghost", "x"),
            Err(PtyError::NoSession(_))
        ));
        assert!(matches!(
            reg.resize("ghost", 10, 10),
            Err(PtyError::NoSession(_))
        ));
        assert!(matches!(reg.kill("ghost"), Err(PtyError::NoSession(_))));
    }

    #[test]
    fn write_editor_context_writes_atomically() {
        let dir = std::env::temp_dir().join(format!("ludion-pty-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cwd = dir.to_string_lossy().to_string();

        write_editor_context(cwd.clone(), r#"{"a":1}"#.to_string()).unwrap();
        let written =
            std::fs::read_to_string(Path::new(&cwd).join(".ludion").join("editor-context.json"))
                .unwrap();
        assert_eq!(written, r#"{"a":1}"#);

        // A second write replaces the file in place (rename-over-existing).
        write_editor_context(cwd.clone(), r#"{"a":2}"#.to_string()).unwrap();
        let written =
            std::fs::read_to_string(Path::new(&cwd).join(".ludion").join("editor-context.json"))
                .unwrap();
        assert_eq!(written, r#"{"a":2}"#);

        // `read_editor_file` round-trips a written file and errors (not panics) on a missing one.
        assert_eq!(
            read_editor_file(
                Path::new(&cwd)
                    .join(".ludion")
                    .join("editor-context.json")
                    .to_string_lossy()
                    .into_owned()
            )
            .unwrap(),
            r#"{"a":2}"#
        );
        assert!(
            read_editor_file(
                Path::new(&cwd)
                    .join("nope.json")
                    .to_string_lossy()
                    .into_owned()
            )
            .is_err()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stat_and_write_editor_file_round_trip_atomically() {
        let dir = std::env::temp_dir().join(format!("ludion-file-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("movement.rs");
        let path_s = path.to_string_lossy().into_owned();

        // Stat on a missing file is an error, not a panic.
        assert!(stat_editor_file(path_s.clone()).is_err());

        // Write returns a plausible mtime and lands the exact bytes; the temp sibling is gone.
        let first = write_editor_file(path_s.clone(), "fn a() {}\n".to_string()).unwrap();
        assert!(first.mtime_ms > 0.0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fn a() {}\n");
        assert!(
            !dir.join("movement.rs.tmp").exists(),
            "temp file was left behind"
        );

        // Stat matches the write's reported mtime (same underlying file), and a second write
        // replaces the content in place.
        let statted = stat_editor_file(path_s.clone()).unwrap();
        assert_eq!(statted.mtime_ms, first.mtime_ms);
        write_editor_file(path_s.clone(), "fn b() {}\n".to_string()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fn b() {}\n");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_editor_file_creates_missing_parent_dirs() {
        let dir = std::env::temp_dir().join(format!("ludion-file-parent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("a").join("b").join("file.txt");
        write_editor_file(path.to_string_lossy().into_owned(), "hi\n".to_string()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hi\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn editor_paths_resolves_repo_layout() {
        let paths = editor_paths().unwrap();
        // The relay/persona paths hang off the resolved repo root.
        assert!(paths.personas_dir.starts_with(&paths.repo_root));
        assert!(paths.relay_script.starts_with(&paths.repo_root));
        assert!(paths.relay_script.ends_with("statusline-relay.mjs"));
        assert!(paths.personas_dir.ends_with("personas"));
    }
}
