//! The editor's **stage sessions**: extra engine children, owned by the host.
//!
//! A view that wants a scene to itself gets a stage slot ([`StageSlot`]) — the Voxel Artist looks
//! at one model at a time on `<project>/scenes/model-stage.yaml`, the Effects view at one emitter
//! on `<project>/scenes/effects-stage.yaml`, each served by its own `ludion serve` process, separate
//! from the game session the rest of the editor attaches to, so neither ever touches the human's
//! working scene (`Plan/editor-voxel-artist.md`). The game session lives here too
//! ([`StageSlot::Game`]): the host runs the open project's own binary on `127.0.0.1:9411`, and the
//! main session auto-attaches once it is listening. One host implementation serves every slot; a
//! slot only names its default port, its scene, and whether it watches assets. Three things hang
//! off each child process and live here together because they share its lifetime:
//!
//! - **the process** — spawned lazily, readiness confirmed by parsing its `listening on ws://…`
//!   line, status pushed to the frontend as events, restartable, torn down gracefully. A stage's
//!   port is reclaimed on start only from an orphan a dead editor host recorded ([`reclaim_port`],
//!   [`try_port_lock`]); anything else holding it leaves the slot `Busy`. The game's port is
//!   only ever *someone's* engine, so the host probes it first ([`probe_port`]) and reports a
//!   [`StagePhase::Busy`] status instead of reclaiming or spawning, unless asked to `replace` a
//!   listener that is itself a Ludion engine;
//! - **the asset watcher** — `<content-root>/voxels/**` and `sculpts/**`, debounced, forwarding
//!   `asset.reload {path}` to *this* session. Only the model slot wants one; it is not the only
//!   reloader (the pattern editor sends `asset.reload` itself, by design);
//! - **`check-scene`** — the subprocess behind the model readout (contract 2: one checker, the
//!   engine's).
//!
//! Owned-child teardown follows [`crate::pty`]'s waiter-thread pattern: a poll loop owns the
//! `Child`, watches a kill flag, and is the sole emitter of the terminal status — so an explicit
//! stop and a natural exit converge on one path and fire exactly once.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher as _};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::children::{BUILD_ALLOWANCE, HostChildren};
use crate::wire;

/// Waiter poll cadence — the same reasoning as `pty`'s: instant to a human, free at idle.
const WAIT_POLL: Duration = Duration::from_millis(50);

/// How long a watched file must be quiet before its `asset.reload` is sent. Editors and the
/// editor's own atomic writes produce bursts; 150 ms coalesces them into one reload.
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(150);

/// Bound on every host→stage wire call. The watcher and the teardown path run on borrowed threads;
/// a wedged engine must never park one indefinitely.
const WIRE_TIMEOUT: Duration = Duration::from_secs(3);

/// How long the stage engine is given to answer `engine.shutdown` before the child is killed.
const GRACEFUL_TEARDOWN: Duration = Duration::from_secs(3);

/// How long `stop` waits for the waiter thread to reap a killed child — the flag is honoured on the
/// waiter's next poll tick, so this is a few ticks of slack, not a teardown budget.
const KILL_REAP: Duration = Duration::from_secs(2);

/// How many trailing stderr lines are kept for a failure message. Enough for a `check-scene`-style
/// diagnostic block plus the cargo line above it.
const STDERR_TAIL: usize = 40;

/// The default bind address for the model stage engine — deliberately not the game session's own
/// default ([`DEFAULT_GAME_ADDR`], `127.0.0.1:9411`): both stages and the game run at once, so none
/// can share a port.
pub const DEFAULT_STAGE_ADDR: &str = "127.0.0.1:9412";

/// The default bind address for the effects stage engine. A port of its own, because both stages
/// run at once — switching views must never have to tear the other one down.
pub const DEFAULT_EFFECTS_STAGE_ADDR: &str = "127.0.0.1:9413";

/// The default bind address for the game session — the port `DEFAULT_URL` and every persona config
/// dial (decision 5, `Plan/editor-open-project.md`).
pub const DEFAULT_GAME_ADDR: &str = "127.0.0.1:9411";

// ---- slots -----------------------------------------------------------------

/// Which stage session this is. The host holds one per slot, all live at once; every status carries
/// its slot so the single `stage://status` event stream stays shared and each view filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StageSlot {
    /// The Voxel Artist's model stage.
    Model,
    /// The Effects view's particle stage.
    Effects,
    /// The open project's own game, run as a host slot (decision 5, `Plan/editor-open-project.md`).
    Game,
}

impl StageSlot {
    /// Where this slot binds when the caller names no address.
    pub fn default_addr(&self) -> &'static str {
        match self {
            StageSlot::Model => DEFAULT_STAGE_ADDR,
            StageSlot::Effects => DEFAULT_EFFECTS_STAGE_ADDR,
            StageSlot::Game => DEFAULT_GAME_ADDR,
        }
    }

    /// Whether this slot watches `<content-root>/voxels/**` and `<content-root>/sculpts/**` for
    /// `asset.reload`. Only the model stage does: an emitter is component fields, so there is no
    /// asset to reload, and the game reloads nothing — it is the human's own working scene.
    pub fn watches_models(&self) -> bool {
        matches!(self, StageSlot::Model)
    }

    /// The sentence a missing-scene error uses to say which file this slot expected.
    pub fn scene_hint(&self) -> &'static str {
        match self {
            StageSlot::Model => "The Voxel Artist view serves `<project>/scenes/model-stage.yaml`",
            StageSlot::Effects => "The Effects view serves `<project>/scenes/effects-stage.yaml`",
            StageSlot::Game => {
                "The game starts on `default_scene` from the project's `ludion.toml`"
            }
        }
    }
}

// ---- status ----------------------------------------------------------------

/// The stage session's lifecycle state, as the frontend sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StagePhase {
    /// No child; nothing has been started (or a stop completed).
    Stopped,
    /// Spawned, not yet listening. The engine may still be building — this can last minutes on a
    /// cold build, which is exactly why readiness is an event and not a blocked command.
    Starting,
    /// The child announced its url; the watcher is running.
    Listening,
    /// The child exited before it ever listened. `message` carries its stderr tail.
    Failed,
    /// The child exited after having listened (crash, external kill, or our own stop).
    Exited,
    /// The slot's port is already held by something else, so nothing was spawned. `listener` names
    /// what holds it; `url` is the address that was probed.
    Busy,
}

/// A snapshot of the stage session — the return value of every command here and the payload of the
/// `stage://status` event, so the frontend can re-sync after a remount instead of replaying events.
#[derive(Debug, Clone, Serialize)]
pub struct StageStatus {
    /// Which session this snapshot describes — the frontend's filter on the shared event stream.
    pub slot: StageSlot,
    pub phase: StagePhase,
    /// `ws://host:port`, present from `Listening` onward.
    pub url: Option<String>,
    /// The stage scene this session was started on.
    pub scene: Option<String>,
    /// A readable explanation for `Failed` (and any warning worth surfacing otherwise).
    pub message: Option<String>,
    /// The child's exit code, when it exited and one was available.
    pub code: Option<i32>,
    /// What holds the slot's port, present only on [`StagePhase::Busy`].
    pub listener: Option<Listener>,
}

impl StageStatus {
    fn stopped(slot: StageSlot) -> StageStatus {
        StageStatus {
            slot,
            phase: StagePhase::Stopped,
            url: None,
            scene: None,
            message: None,
            code: None,
            listener: None,
        }
    }
}

/// Receives every status transition. Cloned into the reader/waiter threads, so it must be cheap to
/// clone and `Send` — the same shape as [`crate::pty::PtyEventSink`], and for the same reason: the
/// registry's plumbing stays testable without a Tauri `AppHandle`.
pub trait StageEventSink: Clone + Send + 'static {
    fn status(&self, status: &StageStatus);
}

// ---- the `listening on …` line ---------------------------------------------

/// The `ws://…` url a `listening on ws://…` line announces, or `None` for any other line.
///
/// `ludion serve` prints exactly this line first and flushes it (see `crates/ludion/src/host.rs`),
/// which is what makes readiness observable without polling the port.
pub fn parse_listening_url(line: &str) -> Option<String> {
    let url = line.trim().strip_prefix("listening on ")?.trim();
    url.starts_with("ws://").then(|| url.to_string())
}

// ---- the CLI invocation ----------------------------------------------------

/// The argv for a `ludion-cli` subcommand, run out of the repo via cargo.
///
/// The same launch form the terminal drawer uses for `ludion-mcp` (`terminal/spawnConfig.ts`): it
/// works whether or not a `ludion` binary is on PATH, and `--manifest-path` pins the workspace so
/// the caller's cwd is irrelevant.
pub fn cli_argv(repo_root: &Path, args: &[&str]) -> Vec<String> {
    let mut argv = vec![
        "run".to_string(),
        "-q".to_string(),
        "--manifest-path".to_string(),
        repo_root.join("Cargo.toml").to_string_lossy().into_owned(),
        "-p".to_string(),
        "ludion-cli".to_string(),
        "--".to_string(),
    ];
    argv.extend(args.iter().map(|a| (*a).to_string()));
    argv
}

/// The editor's standard child-process posture, shared by `check-scene`, the stages and the game:
/// no inherited stdin, piped output, and no flashing console window on Windows (the host is a GUI
/// process).
fn spawn_posture(command: &mut Command) {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
}

/// `cargo run -p ludion-cli -- <args>`, out of the repo, with [`spawn_posture`].
fn cargo(repo_root: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("cargo");
    command
        .args(cli_argv(repo_root, args))
        .current_dir(repo_root);
    spawn_posture(&mut command);
    command
}

/// `cargo build` for a [`StageCommand`], with [`spawn_posture`] and the command's own cwd.
fn build_command_for(repo_root: &Path, command: &StageCommand) -> Command {
    let mut c = Command::new("cargo");
    c.args(command.build_argv(repo_root))
        .current_dir(command.cwd(repo_root));
    spawn_posture(&mut c);
    c
}

/// Whether nothing is listening on `addr`, asked the way the engine will ask it: by binding.
///
/// The listener is dropped immediately. A bind that never accepted a connection leaves no `TIME_WAIT`
/// behind, so this does not itself deny the port to the child a moment later.
fn port_is_free(addr: &str) -> bool {
    std::net::TcpListener::bind(addr).is_ok()
}

/// What holds a busy port, as far as the handshake can tell.
///
/// The handshake, not `engine.describe`, is the identifying call: its payload is the whole
/// component/protocol schema, nothing a Busy card can show, where the handshake reply is exactly
/// the one field worth surfacing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Listener {
    /// A Ludion engine answered the handshake.
    Ludion {
        #[serde(rename = "engineVersion")]
        engine_version: String,
    },
    /// The port is held by something that failed or ignored the handshake.
    Foreign,
}

/// What a bind attempt on `addr` would find right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortProbe {
    Free,
    Busy(Listener),
}

/// Probe `addr` the way the engine itself will: bind first, and if that fails, try the handshake a
/// real client would use. A foreign listener fails the websocket handshake or times out
/// ([`WIRE_TIMEOUT`]) and is named [`Listener::Foreign`] either way.
fn probe_port(addr: &str) -> PortProbe {
    if port_is_free(addr) {
        return PortProbe::Free;
    }
    match wire::WireClient::connect(&format!("ws://{addr}"), WIRE_TIMEOUT) {
        Ok(client) => {
            let engine_version = client.engine_version().unwrap_or("unknown").to_string();
            client.close();
            PortProbe::Busy(Listener::Ludion { engine_version })
        }
        Err(_) => PortProbe::Busy(Listener::Foreign),
    }
}

/// What to do about whatever still holds `addr`, for a human who has to do it by hand.
///
/// Named per platform because the command is: there is no portable way to say "show me who holds
/// this port". The editor host is a GUI binary and already carries `cfg(windows)` for its spawn
/// posture, so this follows that seam rather than inventing one.
fn free_the_port_hint(addr: &str) -> String {
    let port = addr.rsplit(':').next().unwrap_or(addr);
    #[cfg(windows)]
    {
        format!(
            "find it with `netstat -ano | findstr :{port}` and stop it with `taskkill /PID <pid> /F`"
        )
    }
    #[cfg(not(windows))]
    {
        format!("free it with `lsof -tnP -iTCP:{port} -sTCP:LISTEN | xargs kill`")
    }
}

/// Take `addr` back from a previous stage engine that outlived the host, so a start is not refused
/// by the corpse of the last one.
///
/// The orphan is real and routine: the engine is a child of the host, so any exit that skips the
/// host's own teardown — a host crash, a Ctrl-C on `tauri dev`, a rebuild that restarts the host —
/// leaves the engine alive, reparented to init, still holding the port. The next start then dies on
/// `L3009` with nothing the human can act on.
///
/// Only called for an orphan this host can account for: the port's lock is free (no live editor
/// runs a stage there) and a record names an engine an editor spawned and never saw exit. That
/// engine answers the one command that ends it; a listener that is not a Ludion engine refuses the
/// call and gets named instead. A live editor's stage, a hand-run `ludion serve` and any other
/// engine are never asked to shut down — the slot is `Busy` instead.
///
/// Known limit: if a recorded orphan died on its own and an unrelated Ludion engine then bound the
/// port, that engine is reclaimed. Proving otherwise needs the engine's PID in the handshake, which
/// moves the wire schema hash.
///
/// # Errors
/// A human-readable message naming the port and how to free it, when the port is still held after
/// [`GRACEFUL_TEARDOWN`].
fn reclaim_port(addr: &str) -> Result<(), String> {
    reclaim_port_within(addr, WIRE_TIMEOUT, GRACEFUL_TEARDOWN)
}

/// [`reclaim_port`] with its two budgets injected, so the give-up path is testable without spending
/// them (the [`Debouncer`] precedent: policy is a parameter, not a constant read inside).
fn reclaim_port_within(addr: &str, wire: Duration, budget: Duration) -> Result<(), String> {
    if port_is_free(addr) {
        return Ok(());
    }
    let _ = wire::send_one(&format!("ws://{addr}"), "engine.shutdown", None, wire);
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if port_is_free(addr) {
            return Ok(());
        }
        thread::sleep(WAIT_POLL);
    }
    Err(format!(
        "`{addr}` is already in use, and whatever holds it did not answer `engine.shutdown` \
         within {:.0?} — so it is not a Ludion engine, or it is wedged. {}, then try again.",
        budget,
        free_the_port_hint(addr)
    ))
}

// ---- the reload debouncer --------------------------------------------------

/// Coalesces filesystem churn into one `asset.reload` per path.
///
/// Pure and clock-injected (`now` is a parameter) so the policy is unit-testable: each `push`
/// restarts that path's quiet period, and `take_due` yields only the paths that have been still
/// for the whole delay. Distinct paths never merge; repeats of one path never duplicate.
#[derive(Debug)]
pub struct Debouncer {
    pending: HashMap<String, Instant>,
    delay: Duration,
}

impl Debouncer {
    pub fn new(delay: Duration) -> Debouncer {
        Debouncer {
            pending: HashMap::new(),
            delay,
        }
    }

    pub fn push(&mut self, path: String, now: Instant) {
        self.pending.insert(path, now);
    }

    /// The paths quiet for `delay`, removed from the pending set. Sorted, so a batch's send order
    /// does not depend on hash iteration order.
    pub fn take_due(&mut self, now: Instant) -> Vec<String> {
        let mut due: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, seen)| now.duration_since(**seen) >= self.delay)
            .map(|(path, _)| path.clone())
            .collect();
        due.sort();
        for path in &due {
            self.pending.remove(path);
        }
        due
    }

    /// How long until the earliest pending path comes due, or `None` when nothing is pending.
    pub fn next_deadline(&self, now: Instant) -> Option<Duration> {
        self.pending
            .values()
            .map(|seen| self.delay.saturating_sub(now.duration_since(*seen)))
            .min()
    }
}

/// A changed file's path as the engine names it: relative to the content root, `/`-separated —
/// byte-identical to how a scene references the model (`voxels/statue.voxel.yaml`).
///
/// `None` for anything outside the root or for an atomic-write temp sibling (`*.tmp`), which is
/// never an asset the engine holds; the rename that follows fires an event for the real path.
/// Everything else is forwarded unfiltered — the engine ignores paths it does not hold, and that
/// is explicitly the blessed shape.
pub fn relative_asset_path(content_root: &Path, changed: &Path) -> Option<String> {
    let rel = changed.strip_prefix(content_root).ok()?;
    let rel = rel.to_str()?.replace('\\', "/");
    if rel.is_empty() || rel.ends_with(".tmp") {
        return None;
    }
    Some(rel)
}

/// The watcher's channel→debounce→send loop, factored out of the thread so it is testable without
/// a filesystem, an engine, or a `Stage`.
///
/// `send` receives each coalesced batch (content-root-relative, sorted) and returns `false` to end
/// the loop. The loop also ends when `stop` flips or the sender is dropped.
fn drain_reloads(
    rx: mpsc::Receiver<PathBuf>,
    root: PathBuf,
    stop: Arc<AtomicBool>,
    mut send: impl FnMut(Vec<String>) -> bool,
) {
    let mut debouncer = Debouncer::new(RELOAD_DEBOUNCE);
    while !stop.load(Ordering::Relaxed) {
        // With nothing pending there is no deadline to honour, so fall back to the idle poll — it
        // is what makes `stop` observable on a silent watcher.
        let wait = debouncer.next_deadline(Instant::now()).unwrap_or(WAIT_POLL);
        match rx.recv_timeout(wait.max(Duration::from_millis(10))) {
            Ok(path) => {
                if let Some(rel) = relative_asset_path(&root, &path) {
                    debouncer.push(rel, Instant::now());
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let due = debouncer.take_due(Instant::now());
        if !due.is_empty() && !send(due) {
            break;
        }
    }
}

// ---- the session -----------------------------------------------------------

/// Start/restart parameters, named by the frontend.
#[derive(Debug, Clone, Deserialize)]
pub struct StageOpts {
    /// Absolute path to the stage scene (`<project>/scenes/model-stage.yaml`). A parameter, never
    /// hardcoded: the project directory is user-configurable.
    pub scene: String,
    /// `host:port` to bind. Defaults to [`DEFAULT_STAGE_ADDR`].
    #[serde(default)]
    pub addr: Option<String>,
    /// Absolute path to the project directory. Required for [`StageSlot::Game`] (the game is the
    /// open project's own binary, run with the project as its manifest and cwd); unused otherwise.
    #[serde(default)]
    pub project: Option<String>,
}

/// The command a stage session runs, one per [`StageSlot`] family: the stages build and run
/// `ludion serve` out of the repo, the game builds and runs the open project's own binary. The host
/// builds with cargo, then spawns the built executable itself, so the child it owns and kills is the
/// engine and not a `cargo run` wrapper.
enum StageCommand {
    /// `ludion serve <args>` built from the repo workspace, cwd = the repo root.
    Cli { args: Vec<String> },
    /// The project's own binary run with `--scene <scene> --addr <addr>`, cwd = the project. Built
    /// without `-p`: the project's manifest has exactly one binary.
    Game {
        project: PathBuf,
        scene: String,
        addr: String,
    },
}

impl StageCommand {
    /// `cargo build` for the engine binary, with rustc's rendered diagnostics left on stderr and
    /// the built artifacts reported as JSON on stdout.
    fn build_argv(&self, repo_root: &Path) -> Vec<String> {
        let manifest = match self {
            StageCommand::Cli { .. } => repo_root.join("Cargo.toml"),
            StageCommand::Game { project, .. } => project.join("Cargo.toml"),
        };
        let mut argv = vec![
            "build".to_string(),
            "-q".to_string(),
            "--message-format=json-render-diagnostics".to_string(),
            "--manifest-path".to_string(),
            manifest.to_string_lossy().into_owned(),
        ];
        if let StageCommand::Cli { .. } = self {
            argv.extend(["-p", "ludion-cli", "--bin", "ludion"].map(String::from));
        }
        argv
    }

    /// The built engine's own argv.
    fn engine_args(&self) -> Vec<String> {
        match self {
            StageCommand::Cli { args } => args.clone(),
            StageCommand::Game { scene, addr, .. } => vec![
                "--scene".to_string(),
                scene.clone(),
                "--addr".to_string(),
                addr.clone(),
            ],
        }
    }

    fn cwd(&self, repo_root: &Path) -> PathBuf {
        match self {
            StageCommand::Cli { .. } => repo_root.to_path_buf(),
            StageCommand::Game { project, .. } => project.clone(),
        }
    }

    /// Names the actual build and where it runs, for a failure message.
    fn describe(&self, repo_root: &Path) -> String {
        match self {
            StageCommand::Cli { .. } => format!(
                "`cargo build -p ludion-cli --bin ludion` out of {}",
                repo_root.display()
            ),
            StageCommand::Game { project, .. } => {
                format!("`cargo build` out of {}", project.display())
            }
        }
    }
}

/// The bin executable a single cargo message declares, or `None` if it is not a `compiler-artifact`
/// for a `bin` target that produced an `executable`. The build's stdout reader folds it over every
/// message and keeps the last hit.
fn artifact_executable(value: &serde_json::Value) -> Option<PathBuf> {
    if value.get("reason").and_then(serde_json::Value::as_str) != Some("compiler-artifact") {
        return None;
    }
    let is_bin = value
        .get("target")
        .and_then(|t| t.get("kind"))
        .and_then(serde_json::Value::as_array)
        .is_some_and(|kinds| kinds.iter().any(|k| k.as_str() == Some("bin")));
    if !is_bin {
        return None;
    }
    value
        .get("executable")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
}

/// The side-effecting steps of a start, behind a seam so tests can hold one open or fail it.
trait StartSteps {
    fn probe(&self, addr: &str) -> PortProbe;
    fn reclaim(&self, addr: &str) -> Result<(), String>;
    fn sync(&self, repo_root: &Path, project: &Path) -> Result<(), String>;
    fn spawn(&self, command: &mut Command) -> std::io::Result<Child>;
}

/// The production steps: the real probe, port reclaim, `ludion sync` and process spawn.
struct RealSteps<'a> {
    children: &'a HostChildren,
}

impl StartSteps for RealSteps<'_> {
    fn probe(&self, addr: &str) -> PortProbe {
        probe_port(addr)
    }

    fn reclaim(&self, addr: &str) -> Result<(), String> {
        reclaim_port(addr)
    }

    fn sync(&self, repo_root: &Path, project: &Path) -> Result<(), String> {
        sync_project(self.children, repo_root, project)
    }

    fn spawn(&self, command: &mut Command) -> std::io::Result<Child> {
        command.spawn()
    }
}

/// The live watcher's handle. Dropping it stops delivery (the `notify` watcher goes with it) and
/// the flag ends the debounce thread.
struct WatchHandle {
    stop: Arc<AtomicBool>,
    _watcher: notify::RecommendedWatcher,
}

struct Inner {
    status: StageStatus,
    /// Set to true to ask the waiter thread to terminate the child it owns.
    kill: Option<Arc<AtomicBool>>,
    /// Bumped on every reservation and cancel; threads carrying a stale generation stop touching
    /// shared state, so a superseded or cancelled start can never spawn, and a superseded child's
    /// exit can never overwrite a fresh session's status.
    generation: u64,
    watch: Option<WatchHandle>,
    /// What the reserved or running session serves: the project directory for
    /// [`StageSlot::Game`], the content root for a stage. `None` before the first start.
    serving: Option<PathBuf>,
    /// The stage port's lock (see [`try_port_lock`]), owned by the slot rather than by one start
    /// attempt: a start takes it in its pre-steps, a newer start of the same slot takes it over
    /// from a cancelled one, and it is released with [`Stage::release_port_lock`].
    port_lock: Option<(u16, File)>,
}

/// One stage session, the one named by its [`StageSlot`]. Cheap to clone (shared inner + a cloned
/// sink) so it can be moved into the detached teardown thread.
pub struct Stage<S: StageEventSink> {
    slot: StageSlot,
    inner: Arc<Mutex<Inner>>,
    sink: S,
    repo_root: PathBuf,
    /// The host's one-shot child registry, so `ludion sync` dies with the editor.
    children: Arc<HostChildren>,
    /// Where the per-port lock and orphan-record files live (see [`try_port_lock`]).
    state_dir: PathBuf,
}

impl<S: StageEventSink> Clone for Stage<S> {
    fn clone(&self) -> Self {
        Stage {
            slot: self.slot,
            inner: Arc::clone(&self.inner),
            sink: self.sink.clone(),
            repo_root: self.repo_root.clone(),
            children: Arc::clone(&self.children),
            state_dir: self.state_dir.clone(),
        }
    }
}

/// Whether `start` should hand back the running session untouched: it is up (or reserved), and it
/// serves what is being asked for — the same project for the game, the same content root for a stage.
fn reuses_running(phase: StagePhase, running: Option<&Path>, asked: Option<&Path>) -> bool {
    matches!(phase, StagePhase::Starting | StagePhase::Listening) && running == asked
}

// ---- port identity ---------------------------------------------------------

/// What an editor host leaves behind for the engine it spawned on a stage port, so a later host can
/// tell its orphan from an engine it must not touch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    engine_pid: u32,
    host_pid: u32,
}

/// The port of a `host:port` address, or `None` for port `0` (the OS picks, so nothing can collide
/// on it) and for an address with no parseable port.
fn port_of(addr: &str) -> Option<u16> {
    addr.rsplit(':')
        .next()
        .and_then(|port| port.parse::<u16>().ok())
        .filter(|port| *port != 0)
}

fn lock_path(dir: &Path, port: u16) -> PathBuf {
    dir.join(format!("stage-{port}.lock"))
}

fn record_path(dir: &Path, port: u16) -> PathBuf {
    dir.join(format!("stage-{port}.json"))
}

/// Take the advisory lock that marks a live session on `port`. `Some(file)` holds it until dropped
/// (the OS also releases it if the holder's process dies, crash included); `None` means another
/// live editor on this machine holds it.
fn try_port_lock(dir: &Path, port: u16) -> std::io::Result<Option<File>> {
    std::fs::create_dir_all(dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(lock_path(dir, port))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

fn write_record(dir: &Path, port: u16, record: &Record) {
    let _ = std::fs::create_dir_all(dir);
    if let Ok(text) = serde_json::to_string(record) {
        let _ = std::fs::write(record_path(dir, port), text);
    }
}

/// The record for `port`, left on disk. A missing or unreadable file is `None`.
fn read_record(dir: &Path, port: u16) -> Option<Record> {
    let text = std::fs::read_to_string(record_path(dir, port)).ok()?;
    serde_json::from_str(&text).ok()
}

/// [`read_record`], then remove the file.
fn take_record(dir: &Path, port: u16) -> Option<Record> {
    let record = read_record(dir, port);
    let _ = std::fs::remove_file(record_path(dir, port));
    record
}

/// What the waiter thread of a session is handed.
struct Session {
    command: StageCommand,
    kill: Arc<AtomicBool>,
    generation: u64,
    scene: String,
    content_root: PathBuf,
    /// The stage port the session's lock and record are keyed by; `None` for the game and port `0`.
    port: Option<u16>,
}

/// What the pre-steps of a start decided.
enum Prepared {
    Spawn {
        command: StageCommand,
    },
    /// The slot's port is held by something else; nothing is spawned.
    Busy(StageStatus),
}

/// A child's stderr, reduced as it arrives to what a failure message needs (see `StderrScan`).
fn spawn_stderr_reader(stderr: std::process::ChildStderr, scan: Arc<Mutex<StderrScan>>) {
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            scan.lock().unwrap().push(line);
        }
    });
}

/// Poll `child` to exit, killing it when `kill` is raised. The exit code, if there was one.
fn reap(child: &mut Child, kill: &AtomicBool) -> Option<i32> {
    loop {
        if kill.load(Ordering::Relaxed) {
            let _ = child.kill();
        }
        match child.try_wait() {
            Ok(Some(status)) => return status.code(),
            Ok(None) => thread::sleep(WAIT_POLL),
            Err(_) => return None,
        }
    }
}

impl<S: StageEventSink> Stage<S> {
    pub fn new(
        slot: StageSlot,
        sink: S,
        repo_root: PathBuf,
        children: Arc<HostChildren>,
    ) -> Stage<S> {
        Stage {
            slot,
            inner: Arc::new(Mutex::new(Inner {
                status: StageStatus::stopped(slot),
                kill: None,
                generation: 0,
                watch: None,
                serving: None,
                port_lock: None,
            })),
            sink,
            repo_root,
            children,
            state_dir: std::env::temp_dir().join("ludion-editor"),
        }
    }

    /// Keep the port lock and record files in `dir` instead of the shared default, so concurrent
    /// tests never see each other's.
    #[cfg(test)]
    fn with_state_dir(mut self, dir: PathBuf) -> Stage<S> {
        self.state_dir = dir;
        self
    }

    pub fn status(&self) -> StageStatus {
        self.inner.lock().unwrap().status.clone()
    }

    /// Publish a status transition, unless a newer generation has taken over.
    fn set_status(&self, generation: u64, status: StageStatus) {
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.generation != generation {
                return;
            }
            inner.status = status.clone();
        }
        self.sink.status(&status);
    }

    /// Start the stage session if it is not already up or in flight. Idempotent: an existing
    /// `Starting` or `Listening` session — a start still in its slow pre-steps included — is
    /// returned untouched when it is serving what was asked for (the project for the game, the
    /// content root for a stage), which is what makes the view's first-entry call safe to repeat on
    /// every remount and to race. A slot serving something *else* is stopped first, so the fresh
    /// start always serves what was actually asked for.
    ///
    /// `replace` only matters for [`StageSlot::Game`]: `true` reclaims the port from a listener
    /// that is itself a Ludion engine before spawning; a foreign listener is never reclaimed
    /// regardless. Model/Effects ignore it — their port is always the host's own orphan to reclaim.
    ///
    /// Returns immediately — readiness arrives as a `Listening` status event, because a cold
    /// build compiles first.
    pub fn start(&self, opts: StageOpts, replace: bool) -> Result<StageStatus, String> {
        self.start_with(
            opts,
            replace,
            &RealSteps {
                children: &self.children,
            },
        )
    }

    /// [`Self::start`] with its side-effecting steps injected, so the busy/replace branches and the
    /// concurrent-start races are testable without a real listener or child (the
    /// [`reclaim_port_within`] precedent).
    /// A [`StageSlot::Game`] start runs `ludion sync` on the project once the port probe says to
    /// spawn, so the game never starts against a stale derived tree; a failure is returned like any
    /// other refusal.
    fn start_with(
        &self,
        opts: StageOpts,
        replace: bool,
        steps: &dyn StartSteps,
    ) -> Result<StageStatus, String> {
        let scene = PathBuf::from(&opts.scene);
        if !scene.is_file() {
            return Err(format!(
                "no scene at `{}`. {}; create it (or switch project from the title bar) and try \
                 again.",
                scene.display(),
                self.slot.scene_hint()
            ));
        }
        let content_root = scene
            .parent()
            .ok_or_else(|| {
                format!(
                    "`{}` has no parent directory to serve assets from",
                    scene.display()
                )
            })?
            .to_path_buf();
        let addr = opts
            .addr
            .clone()
            .unwrap_or_else(|| self.slot.default_addr().to_string());
        let game_project = match self.slot {
            StageSlot::Game => Some(
                opts.project
                    .clone()
                    .ok_or_else(|| "the game slot needs a project directory".to_string())?,
            ),
            StageSlot::Model | StageSlot::Effects => None,
        };
        let serving = match &game_project {
            Some(project) => PathBuf::from(project),
            None => content_root.clone(),
        };

        let Some(generation) = self.reserve(&serving, &opts.scene)? else {
            return Ok(self.status());
        };
        let prepared = match self.pre_steps(steps, &opts, replace, &addr, game_project) {
            Ok(prepared) => prepared,
            Err(message) => {
                self.release_port_lock(generation);
                self.set_status(generation, StageStatus::stopped(self.slot));
                return Err(message);
            }
        };
        let command = match prepared {
            Prepared::Spawn { command } => command,
            Prepared::Busy(status) => {
                self.release_port_lock(generation);
                self.set_status(generation, status);
                return Ok(self.status());
            }
        };

        let kill = Arc::new(AtomicBool::new(false));
        // Launch under the lock: a stop that cancelled this start has bumped the generation, and a
        // child spawned after that would be one nobody owns. `kill` being set marks a waiter-owned
        // child (see `stop`).
        let spawned = {
            let mut inner = self.inner.lock().unwrap();
            if inner.generation != generation {
                let status = inner.status.clone();
                drop(inner);
                self.release_port_lock(generation);
                return Ok(status);
            }
            let child = steps.spawn(&mut build_command_for(&self.repo_root, &command));
            if child.is_ok() {
                inner.kill = Some(Arc::clone(&kill));
            }
            child
        };
        let child = match spawned {
            Ok(child) => child,
            Err(e) => {
                self.release_port_lock(generation);
                self.set_status(generation, StageStatus::stopped(self.slot));
                return Err(format!(
                    "could not start the stage engine: {e}. The editor builds it with {} — check \
                     that cargo is on PATH.",
                    command.describe(&self.repo_root)
                ));
            }
        };

        let session = Session {
            command,
            kill,
            generation,
            scene: opts.scene.clone(),
            content_root,
            port: port_of(&addr),
        };
        self.spawn_session_threads(child, session);
        Ok(self.status())
    }

    /// Claim the slot for a start: reuse a session that already serves `serving`, stop one that
    /// serves something else, and otherwise bump the generation and publish `Starting` — all under
    /// one lock, so two concurrent starts cannot both proceed. `None` means "reuse the session
    /// already up or in flight"; `Some` is the generation this start owns.
    fn reserve(&self, serving: &Path, scene: &str) -> Result<Option<u64>, String> {
        for attempt in 0..2 {
            let reserved = {
                let mut inner = self.inner.lock().unwrap();
                let phase = inner.status.phase;
                if reuses_running(phase, inner.serving.as_deref(), Some(serving)) {
                    return Ok(None);
                }
                if matches!(phase, StagePhase::Starting | StagePhase::Listening) {
                    None
                } else {
                    inner.generation += 1;
                    inner.kill = None;
                    inner.serving = Some(serving.to_path_buf());
                    inner.status = StageStatus {
                        scene: Some(scene.to_string()),
                        phase: StagePhase::Starting,
                        ..StageStatus::stopped(self.slot)
                    };
                    Some((inner.generation, inner.status.clone()))
                }
            };
            if let Some((generation, status)) = reserved {
                self.sink.status(&status);
                return Ok(Some(generation));
            }
            // Serving something else: drop it, never holding the lock across the blocking stop.
            if attempt == 0 {
                self.stop();
            }
        }
        Err("the slot's previous session is still stopping; try again".to_string())
    }

    /// The slow, side-effecting steps before a spawn: reclaim the port, probe it (the game), and
    /// sync the project's derived imports so the game never starts against a stale tree.
    fn pre_steps(
        &self,
        steps: &dyn StartSteps,
        opts: &StageOpts,
        replace: bool,
        addr: &str,
        game_project: Option<String>,
    ) -> Result<Prepared, String> {
        let Some(project) = game_project else {
            return self.stage_pre_steps(steps, opts, addr);
        };
        match steps.probe(addr) {
            PortProbe::Free => {}
            PortProbe::Busy(Listener::Ludion { .. }) if replace => steps.reclaim(addr)?,
            PortProbe::Busy(listener) => {
                let message = match &listener {
                    Listener::Ludion { engine_version } => {
                        format!("`{addr}` is already serving a Ludion engine ({engine_version}).")
                    }
                    Listener::Foreign => format!(
                        "`{addr}` is held by something that is not a Ludion engine — {}.",
                        free_the_port_hint(addr)
                    ),
                };
                return Ok(Prepared::Busy(self.busy_status(
                    opts,
                    addr,
                    message,
                    Some(listener),
                )));
            }
        }
        steps.sync(&self.repo_root, Path::new(&project))?;
        Ok(Prepared::Spawn {
            command: StageCommand::Game {
                project: PathBuf::from(project),
                scene: opts.scene.clone(),
                addr: addr.to_string(),
            },
        })
    }

    /// A stage's pre-steps. Another live editor on this machine holds the port's lock, or the port
    /// is served by an engine this host cannot show it left behind: either way the slot is `Busy`
    /// and nothing is asked of the listener. Only an engine an editor recorded and then died
    /// without reaping — the lock free, the record present — is reclaimed.
    fn stage_pre_steps(
        &self,
        steps: &dyn StartSteps,
        opts: &StageOpts,
        addr: &str,
    ) -> Result<Prepared, String> {
        let port = port_of(addr);
        if let Some(port) = port {
            match self.hold_port_lock(port) {
                Ok(true) => {}
                Ok(false) => {
                    let message = format!(
                        "`{addr}` is in use by another Ludion editor's stage session — close the \
                         stage there, or {}.",
                        free_the_port_hint(addr)
                    );
                    return Ok(Prepared::Busy(self.busy_status(opts, addr, message, None)));
                }
                Err(e) => {
                    return Err(format!(
                        "could not take the stage port lock in {}: {e}",
                        self.state_dir.display()
                    ));
                }
            }
        }
        match steps.probe(addr) {
            PortProbe::Free => {}
            PortProbe::Busy(Listener::Ludion { .. })
                if port.is_some_and(|port| read_record(&self.state_dir, port).is_some()) =>
            {
                // Consumed only once the engine is gone: a reclaim that times out leaves the orphan
                // recorded for the next try.
                steps.reclaim(addr)?;
                if let Some(port) = port {
                    take_record(&self.state_dir, port);
                }
            }
            PortProbe::Busy(listener) => {
                let message = match &listener {
                    Listener::Ludion { engine_version } => format!(
                        "`{addr}` is served by a Ludion engine ({engine_version}) that no editor \
                         left behind — {}, then try again.",
                        free_the_port_hint(addr)
                    ),
                    Listener::Foreign => format!(
                        "`{addr}` is held by something that is not a Ludion engine — {}.",
                        free_the_port_hint(addr)
                    ),
                };
                return Ok(Prepared::Busy(self.busy_status(
                    opts,
                    addr,
                    message,
                    Some(listener),
                )));
            }
        }
        Ok(Prepared::Spawn {
            command: StageCommand::Cli {
                args: vec![
                    "serve".to_string(),
                    "--scene".to_string(),
                    opts.scene.clone(),
                    "--addr".to_string(),
                    addr.to_string(),
                ],
            },
        })
    }

    /// Make sure this slot holds `port`'s lock: keep one it already holds (a cancelled start's —
    /// this process cannot take a second handle on its own lock) or take it, dropping a lock for
    /// another port. `false` means another live editor holds it.
    fn hold_port_lock(&self, port: u16) -> std::io::Result<bool> {
        let mut inner = self.inner.lock().unwrap();
        if inner
            .port_lock
            .as_ref()
            .is_some_and(|(held, _)| *held == port)
        {
            return Ok(true);
        }
        inner.port_lock = None;
        Ok(match try_port_lock(&self.state_dir, port)? {
            Some(lock) => {
                inner.port_lock = Some((port, lock));
                true
            }
            None => false,
        })
    }

    /// Release the slot's port lock on behalf of the start or session of `generation`. A stale
    /// generation releases it only when no newer start is under way: a newer one has taken the lock
    /// over and keeps it.
    fn release_port_lock(&self, generation: u64) {
        let mut inner = self.inner.lock().unwrap();
        let newer_under_way = inner.generation != generation
            && matches!(
                inner.status.phase,
                StagePhase::Starting | StagePhase::Listening
            );
        if !newer_under_way {
            inner.port_lock = None;
        }
    }

    /// The `Busy` status for a start that found its port held, with `message` saying by what.
    fn busy_status(
        &self,
        opts: &StageOpts,
        addr: &str,
        message: String,
        listener: Option<Listener>,
    ) -> StageStatus {
        StageStatus {
            slot: self.slot,
            phase: StagePhase::Busy,
            url: Some(format!("ws://{addr}")),
            scene: Some(opts.scene.clone()),
            message: Some(message),
            code: None,
            listener,
        }
    }

    /// The reader and waiter threads of a session. The waiter owns the build child first, then the
    /// engine it built: stderr of both is reduced into one failure message, the build's JSON stdout
    /// yields the executable, and the engine's `listening on` line promotes the status to
    /// `Listening`. It honours the kill flag at every step and is the sole emitter of the terminal
    /// status — kill, build failure and natural exit converge here (`pty.rs`'s pattern).
    fn spawn_session_threads(&self, mut build: Child, session: Session) {
        let Session {
            command,
            kill,
            generation,
            scene,
            content_root,
            port,
        } = session;
        let scan: Arc<Mutex<StderrScan>> = Arc::new(Mutex::new(StderrScan::default()));
        if let Some(stderr) = build.stderr.take() {
            spawn_stderr_reader(stderr, Arc::clone(&scan));
        }
        // The last bin artifact the build reports is the engine to run.
        let executable = build.stdout.take().map(|stdout| {
            thread::spawn(move || {
                BufReader::new(stdout)
                    .lines()
                    .map_while(Result::ok)
                    .filter_map(|line| serde_json::from_str::<serde_json::Value>(&line).ok())
                    .filter_map(|message| artifact_executable(&message))
                    .last()
            })
        });

        let stage = self.clone();
        let scene_path = scene;
        thread::spawn(move || {
            let mut code = reap(&mut build, &kill);
            // Why the session ended, when the stderr scan cannot say.
            let mut failure: Option<String> = None;
            let mut engine: Option<Child> = None;
            // A killed build is not joined: its reader ends with the pipe, and nobody wants its answer.
            if code == Some(0) && !kill.load(Ordering::Relaxed) {
                match executable.and_then(|reader| reader.join().ok()).flatten() {
                    None => {
                        failure = Some(format!(
                            "{} built no binary to run",
                            command.describe(&stage.repo_root)
                        ));
                    }
                    Some(exe) => {
                        let inner = stage.inner.lock().unwrap();
                        if inner.generation == generation && !kill.load(Ordering::Relaxed) {
                            let mut engine_command = Command::new(&exe);
                            engine_command
                                .args(command.engine_args())
                                .current_dir(command.cwd(&stage.repo_root));
                            spawn_posture(&mut engine_command);
                            match engine_command.spawn() {
                                Ok(child) => {
                                    if let Some(port) = port {
                                        let record = Record {
                                            engine_pid: child.id(),
                                            host_pid: std::process::id(),
                                        };
                                        write_record(&stage.state_dir, port, &record);
                                    }
                                    engine = Some(child);
                                }
                                Err(e) => {
                                    failure = Some(format!(
                                        "could not start the stage engine `{}`: {e}",
                                        exe.display()
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            if let Some(mut engine) = engine {
                if let Some(stderr) = engine.stderr.take() {
                    spawn_stderr_reader(stderr, Arc::clone(&scan));
                }
                if let Some(stdout) = engine.stdout.take() {
                    stage.spawn_listening_reader(stdout, generation, &scene_path, &content_root);
                }
                code = reap(&mut engine, &kill);
                if let Some(port) = port {
                    take_record(&stage.state_dir, port);
                }
            }
            // The session is over: free the port's lock before the terminal status says so.
            stage.release_port_lock(generation);
            stage.stop_watcher(generation);
            let listened = {
                let inner = stage.inner.lock().unwrap();
                inner.generation == generation && inner.status.phase == StagePhase::Listening
            };
            // The stderr tail explains both terminal shapes worth explaining: a start that never
            // listened, and a session that died with a non-zero code (a crash, or a kill that beat
            // the graceful shutdown). A clean exit says nothing, because there is nothing to say.
            let message = if listened && code == Some(0) {
                None
            } else {
                Some(
                    failure
                        .or_else(|| scan.lock().unwrap().message())
                        .unwrap_or_else(|| {
                            format!(
                                "the stage engine exited with code {code:?} and said nothing on stderr"
                            )
                        }),
                )
            };
            let url = stage.inner.lock().unwrap().status.url.clone();
            stage.set_status(
                generation,
                StageStatus {
                    slot: stage.slot,
                    phase: if listened {
                        StagePhase::Exited
                    } else {
                        StagePhase::Failed
                    },
                    url,
                    scene: Some(scene_path),
                    message,
                    code,
                    listener: None,
                },
            );
            let mut inner = stage.inner.lock().unwrap();
            if inner.generation == generation {
                inner.kill = None;
            }
        });
    }

    /// stdout of the engine: the one machine-parseable line, then the human-readable header. The
    /// loop must run to EOF, never stop at the line it came for: dropping the reader closes the
    /// pipe, and the engine's very next `println!` of its header panics on a broken stdout (os
    /// error 232).
    fn spawn_listening_reader(
        &self,
        stdout: std::process::ChildStdout,
        generation: u64,
        scene: &str,
        content_root: &Path,
    ) {
        let stage = self.clone();
        let scene_path = scene.to_string();
        let content_root = content_root.to_path_buf();
        thread::spawn(move || {
            let mut announced = false;
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if announced {
                    continue;
                }
                let Some(url) = parse_listening_url(&line) else {
                    continue;
                };
                announced = true;
                stage.set_status(
                    generation,
                    StageStatus {
                        slot: stage.slot,
                        phase: StagePhase::Listening,
                        url: Some(url.clone()),
                        scene: Some(scene_path.clone()),
                        message: None,
                        code: None,
                        listener: None,
                    },
                );
                if stage.slot.watches_models() {
                    stage.start_watcher(generation, &content_root);
                }
            }
        });
    }

    /// Stop the session and return once it is down. A start still in its pre-steps (no child yet,
    /// so no `kill` flag) is cancelled outright: the generation moves on and the in-flight start
    /// spawns nothing. Otherwise `engine.shutdown` over the wire goes first (a clean exit releases
    /// the port and lets the engine flush), then the kill flag as the fallback — the waiter owns
    /// the engine process itself, so the kill reaches it on every OS — then a wait of
    /// [`KILL_REAP`] for the waiter thread's reap tick.
    ///
    /// Blocking and bounded by [`GRACEFUL_TEARDOWN`] plus [`KILL_REAP`]; `stage_stop` wraps it as
    /// an async Tauri command and app exit runs it inline. It never contacts a listener this slot
    /// did not spawn: unless the phase is `Starting` or `Listening` it returns without sending
    /// anything, so a `Busy` status (an attached server we only ever probed) and a stale
    /// `Exited`/`Failed` url are both left alone (decision 5).
    pub fn stop(&self) {
        let (url, kill) = {
            let mut inner = self.inner.lock().unwrap();
            if !matches!(
                inner.status.phase,
                StagePhase::Starting | StagePhase::Listening
            ) {
                return;
            }
            let Some(kill) = inner.kill.clone() else {
                inner.generation += 1;
                inner.status = StageStatus::stopped(self.slot);
                let status = inner.status.clone();
                drop(inner);
                self.sink.status(&status);
                return;
            };
            (inner.status.url.clone(), kill)
        };
        let acked = match url {
            Some(url) => wire::send_one(&url, "engine.shutdown", None, GRACEFUL_TEARDOWN).is_ok(),
            None => false,
        };
        // An acked shutdown gets the whole budget to land on its own. The kill is the fallback, and
        // firing it early is not free: a killed engine exits non-zero instead of flushing and
        // exiting 0.
        if acked && self.wait_for_exit(GRACEFUL_TEARDOWN) {
            return;
        }
        kill.store(true, Ordering::Relaxed);
        self.wait_for_exit(KILL_REAP);
    }

    /// Block until the session leaves `Starting`/`Listening`, or `budget` elapses. `true` if it did.
    fn wait_for_exit(&self, budget: Duration) -> bool {
        let deadline = Instant::now() + budget;
        loop {
            let phase = self.inner.lock().unwrap().status.phase;
            if !matches!(phase, StagePhase::Starting | StagePhase::Listening) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(WAIT_POLL);
        }
    }

    /// Stop, then start again on `opts`. Used by the view's explicit restart control and after a
    /// crash. `stop` returning only once the slot is down is what keeps `start`'s idempotence check
    /// from seeing the dying session and handing it back.
    pub fn restart(&self, opts: StageOpts) -> Result<StageStatus, String> {
        self.stop();
        self.start(opts, false)
    }

    // ---- the watcher -------------------------------------------------------

    /// Watch `<content-root>/voxels` and `<content-root>/sculpts` recursively and forward
    /// debounced `asset.reload`s to this session. A project missing one or both directories is not
    /// an error — nothing to watch there yet.
    ///
    /// This is the reloader every file writer (the persona, the palette editor, the slice painter,
    /// the sculpt loop) relies on; it is not the only one — `PatternEditor.tsx` sends `asset.reload`
    /// itself after its own writes, by design.
    fn start_watcher(&self, generation: u64, content_root: &Path) {
        let roots: Vec<PathBuf> = ["voxels", "sculpts"]
            .into_iter()
            .map(|name| content_root.join(name))
            .filter(|dir| dir.is_dir())
            .collect();
        if roots.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel::<PathBuf>();
        let mut watcher =
            match notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Ok(event) = event {
                    for path in event.paths {
                        let _ = tx.send(path);
                    }
                }
            }) {
                Ok(watcher) => watcher,
                Err(_) => return, // no watcher backend: the view degrades to manual restarts
            };
        // One watcher, both roots registered on it — a single thread and a single debounce stream
        // instead of one per directory.
        let watched = roots
            .iter()
            .filter(|dir| watcher.watch(dir, RecursiveMode::Recursive).is_ok())
            .count();
        if watched == 0 {
            return;
        }

        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let stage = self.clone();
        let root = content_root.to_path_buf();
        thread::spawn(move || {
            drain_reloads(rx, root, thread_stop, |batch| {
                let url = {
                    let inner = stage.inner.lock().unwrap();
                    if inner.generation != generation {
                        return false; // superseded session: this watcher is done
                    }
                    inner.status.url.clone()
                };
                let Some(url) = url else { return true };
                // One connection per batch: reloads are human-paced, so a persistent socket would
                // only buy a reconnect state machine.
                if let Ok(mut client) = wire::WireClient::connect(&url, WIRE_TIMEOUT) {
                    for path in batch {
                        let _ = client.call("asset.reload", Some(json!({ "path": path })));
                    }
                    client.close();
                }
                true
            });
        });

        let mut inner = self.inner.lock().unwrap();
        if inner.generation != generation {
            return; // superseded while we were setting up; drop the watcher on the floor
        }
        inner.watch = Some(WatchHandle {
            stop,
            _watcher: watcher,
        });
    }

    fn stop_watcher(&self, generation: u64) {
        let mut inner = self.inner.lock().unwrap();
        if inner.generation != generation {
            return;
        }
        if let Some(watch) = inner.watch.take() {
            watch.stop.store(true, Ordering::Relaxed);
        }
    }
}

/// A stage child's stderr, reduced as it arrives to what a failure message needs: a bounded tail,
/// and the last rustc-style `error[...]` block on its own.
///
/// The block is not redundant with the tail. The engine prints its content-root diagnostics before it
/// binds, and a warning block is exactly as long as an error block — so a scene directory noisy
/// enough (vigil's `scenes/` skips its own game kinds) pushes the one line that says *why* out of a
/// bounded tail entirely. Keeping the block separately means an evicted error is still the first
/// thing the message says.
#[derive(Debug, Default)]
struct StderrScan {
    /// The last [`STDERR_TAIL`] lines, in order.
    tail: Vec<String>,
    /// The most recent `error[...]` header and its indented continuation lines.
    error: Vec<String>,
    /// Whether the last line seen belonged to [`error`](StderrScan::error), so an indented line
    /// continues that block rather than attaching to nothing.
    in_error: bool,
}

impl StderrScan {
    fn push(&mut self, line: String) {
        if self.tail.len() == STDERR_TAIL {
            self.tail.remove(0);
        }
        // A rustc-style block is its header plus the indented lines under it, so an unindented line
        // ends the one in hand. A new header replaces the last: what a failure wants is the error it
        // died on, not the first one it survived.
        if line.starts_with("error") {
            self.in_error = true;
            self.error = vec![line.clone()];
        } else if self.in_error && line.starts_with(char::is_whitespace) {
            self.error.push(line.clone());
        } else {
            self.in_error = false;
        }
        self.tail.push(line);
    }

    /// What the session failed with, or `None` if the child said nothing at all.
    ///
    /// The tail alone when it still holds the error block — which is the ordinary case, and repeating
    /// two lines above their own copy reads like a bug. The block first, then the tail, when the tail
    /// has lost it.
    fn message(&self) -> Option<String> {
        let tail = self.tail.join("\n");
        let error = self.error.join("\n");
        match (error.trim().is_empty(), tail.trim().is_empty()) {
            (false, false) if !tail.contains(&error) => Some(format!("{error}\n\n{tail}")),
            (_, false) => Some(tail),
            (false, true) => Some(error),
            (true, true) => None,
        }
    }
}

// ---- check-scene -----------------------------------------------------------

/// The raw result of a `ludion check-scene` run, handed to the frontend verbatim.
#[derive(Debug, Clone, Serialize)]
pub struct CheckSceneResult {
    pub stdout: String,
    pub stderr: String,
    /// The process exit code; `None` if it was killed by a signal.
    #[serde(rename = "exitCode")]
    pub exit_code: Option<i32>,
}

/// Run `ludion check-scene <path> --json` and return its output.
///
/// One checker, the engine's (`Plan/editor-voxel-artist.md` contract 2): the model readout — size,
/// filled count, component count, `L1129` — is the summary object this process prints on stdout,
/// never a TS reimplementation of voxel connectivity. Debouncing is the caller's job.
///
/// Runs through `children`, so the editor's exit stops it and a hung run is bounded by the build
/// allowance plus a minute.
pub fn check_scene(
    children: &HostChildren,
    repo_root: &Path,
    path: &str,
) -> Result<CheckSceneResult, String> {
    let deadline = BUILD_ALLOWANCE + Duration::from_secs(60);
    let output = children
        .run(cargo(repo_root, &["check-scene", path, "--json"]), deadline)
        .map_err(|e| {
            format!(
                "could not run `check-scene`: {e}. The editor runs it as \
                 `cargo run -p ludion-cli -- check-scene` out of {} — check that cargo is on PATH.",
                repo_root.display()
            )
        })?;
    let mut stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if output.timed_out {
        stderr.push_str(&format!(
            "\nthe editor stopped `ludion check-scene` after {} s (its deadline: the build \
             allowance plus a minute).\n",
            deadline.as_secs()
        ));
    }
    Ok(CheckSceneResult {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr,
        exit_code: output.code,
    })
}

/// Brings the project's derived `[[import]]` trees fresh (`ludion sync`, a pipeline verb run as a
/// host subprocess) so the game starts against them. A project with no `ludion.toml` has none and
/// spawns nothing. A cold miss is silent for ~10 s: the stage shows no "syncing" status.
fn sync_project(children: &HostChildren, repo_root: &Path, project: &Path) -> Result<(), String> {
    if !project.join("ludion.toml").is_file() {
        return Ok(());
    }
    let project = project.to_string_lossy();
    let deadline = BUILD_ALLOWANCE + Duration::from_secs(10 * 60);
    let output = children
        .run(
            cargo(repo_root, &["sync", "--project", &project, "--json"]),
            deadline,
        )
        .map_err(|e| {
            format!(
                "could not run `sync`: {e}. The editor runs it as \
                 `cargo run -p ludion-cli -- sync` out of {} — check that cargo is on PATH.",
                repo_root.display()
            )
        })?;
    if output.timed_out {
        return Err(format!(
            "the editor stopped `ludion sync --project {project}` after {} s (its deadline: the \
             build allowance plus ten minutes).",
            deadline.as_secs()
        ));
    }
    if output.cancelled {
        return Err("the editor stopped `ludion sync` (it is exiting)".to_string());
    }
    if output.code == Some(0) {
        return Ok(());
    }
    Err(format!(
        "could not sync the project's derived imports (`ludion sync --project {project}`): {}",
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

// ---- Tauri glue ------------------------------------------------------------

/// Sink that forwards status transitions to the webview as `stage://status` events.
#[derive(Clone)]
pub struct AppStageSink {
    app: tauri::AppHandle,
}

impl StageEventSink for AppStageSink {
    fn status(&self, status: &StageStatus) {
        use tauri::Emitter as _;
        let _ = self.app.emit("stage://status", status.clone());
    }
}

pub type AppStage = Stage<AppStageSink>;

/// Every stage session the host owns, one per [`StageSlot`], all live at once — switching views
/// never tears another view's engine down. The managed Tauri state.
pub struct AppStages {
    model: AppStage,
    effects: AppStage,
    game: AppStage,
    pub repo_root: PathBuf,
    /// The one-shot pipeline children (`gen`, `check-scene`, `sync`), stopped at app exit.
    pub children: Arc<HostChildren>,
}

impl AppStages {
    pub fn get(&self, slot: StageSlot) -> &AppStage {
        match slot {
            StageSlot::Model => &self.model,
            StageSlot::Effects => &self.effects,
            StageSlot::Game => &self.game,
        }
    }

    /// Stop every session at once, returning when all are down. Blocking, bounded by one slot's
    /// teardown budget rather than three — the app-exit path (`lib.rs`) runs it inline.
    pub fn stop_all(&self) {
        thread::scope(|scope| {
            for stage in [&self.model, &self.effects, &self.game] {
                scope.spawn(move || stage.stop());
            }
        });
    }
}

pub fn stages(app: tauri::AppHandle, repo_root: PathBuf) -> AppStages {
    let sink = AppStageSink { app };
    let children = Arc::new(HostChildren::default());
    AppStages {
        model: Stage::new(
            StageSlot::Model,
            sink.clone(),
            repo_root.clone(),
            children.clone(),
        ),
        effects: Stage::new(
            StageSlot::Effects,
            sink.clone(),
            repo_root.clone(),
            children.clone(),
        ),
        game: Stage::new(StageSlot::Game, sink, repo_root.clone(), children.clone()),
        repo_root,
        children,
    }
}

#[tauri::command(async)]
pub fn stage_start(
    state: tauri::State<'_, AppStages>,
    slot: StageSlot,
    opts: StageOpts,
    replace: Option<bool>,
) -> Result<StageStatus, String> {
    state.get(slot).start(opts, replace.unwrap_or(false))
}

#[tauri::command]
pub fn stage_status(state: tauri::State<'_, AppStages>, slot: StageSlot) -> StageStatus {
    state.get(slot).status()
}

#[tauri::command(async)]
pub fn stage_restart(
    state: tauri::State<'_, AppStages>,
    slot: StageSlot,
    opts: StageOpts,
) -> Result<StageStatus, String> {
    state.get(slot).restart(opts)
}

/// Blocking, bounded by the graceful-teardown budget; an async command so the UI thread never
/// waits (the `stage_start`/`stage_restart` precedent). The promise resolving means the slot is
/// actually down — the terminal status still arrives separately as a `stage://status` event.
#[tauri::command(async)]
pub fn stage_stop(state: tauri::State<'_, AppStages>, slot: StageSlot) {
    state.get(slot).stop();
}

#[tauri::command(async)]
pub fn run_check_scene(
    state: tauri::State<'_, AppStages>,
    path: String,
) -> Result<CheckSceneResult, String> {
    check_scene(&state.children, &state.repo_root, &path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc::{Receiver, Sender, channel};

    #[derive(Clone)]
    struct TestSink(Sender<StageStatus>);

    impl StageEventSink for TestSink {
        fn status(&self, status: &StageStatus) {
            let _ = self.0.send(status.clone());
        }
    }

    fn sink() -> (TestSink, Receiver<StageStatus>) {
        let (tx, rx) = channel();
        (TestSink(tx), rx)
    }

    /// A step held open until released; `entered` fires when the step is reached.
    type Gate = (Sender<()>, Receiver<()>);

    /// Test steps: each gate blocks the first call only, so the call that follows runs free.
    struct FakeSteps {
        probe: PortProbe,
        probe_gate: Mutex<Option<Gate>>,
        sync_gate: Mutex<Option<Gate>>,
        reclaim_gate: Mutex<Option<Gate>>,
        reclaim_error: Option<String>,
        /// Spawn the test binary as a long-lived child instead of failing the spawn.
        sleeper: bool,
        /// Spawn a fake build: after `delay` it prints one cargo `compiler-artifact` line naming
        /// `executable`, then exits 0, so the real waiter goes on to spawn that executable.
        artifact: Option<(PathBuf, Duration)>,
        probes: AtomicUsize,
        reclaims: AtomicUsize,
        spawns: AtomicUsize,
    }

    impl FakeSteps {
        fn new() -> FakeSteps {
            FakeSteps {
                probe: PortProbe::Free,
                probe_gate: Mutex::new(None),
                sync_gate: Mutex::new(None),
                reclaim_gate: Mutex::new(None),
                reclaim_error: None,
                sleeper: false,
                artifact: None,
                probes: AtomicUsize::new(0),
                reclaims: AtomicUsize::new(0),
                spawns: AtomicUsize::new(0),
            }
        }

        fn probing(probe: PortProbe) -> FakeSteps {
            FakeSteps {
                probe,
                ..FakeSteps::new()
            }
        }

        /// Hold the first `probe` open; returns (fires on entry, releases it).
        fn gate_probe(&self) -> (Receiver<()>, Sender<()>) {
            let (entered_tx, entered_rx) = channel();
            let (release_tx, release_rx) = channel();
            *self.probe_gate.lock().unwrap() = Some((entered_tx, release_rx));
            (entered_rx, release_tx)
        }

        fn gate_sync(&self) -> (Receiver<()>, Sender<()>) {
            let (entered_tx, entered_rx) = channel();
            let (release_tx, release_rx) = channel();
            *self.sync_gate.lock().unwrap() = Some((entered_tx, release_rx));
            (entered_rx, release_tx)
        }

        fn gate_reclaim(&self) -> (Receiver<()>, Sender<()>) {
            let (entered_tx, entered_rx) = channel();
            let (release_tx, release_rx) = channel();
            *self.reclaim_gate.lock().unwrap() = Some((entered_tx, release_rx));
            (entered_rx, release_tx)
        }

        fn pass(gate: &Mutex<Option<Gate>>) {
            let taken = gate.lock().unwrap().take();
            if let Some((entered, release)) = taken {
                let _ = entered.send(());
                // Bounded, so a failed assertion elsewhere can never wedge the test binary.
                let _ = release.recv_timeout(Duration::from_secs(10));
            }
        }

        fn probed(&self) -> usize {
            self.probes.load(Ordering::SeqCst)
        }

        fn reclaimed(&self) -> usize {
            self.reclaims.load(Ordering::SeqCst)
        }

        fn spawned(&self) -> usize {
            self.spawns.load(Ordering::SeqCst)
        }
    }

    impl StartSteps for FakeSteps {
        fn probe(&self, _addr: &str) -> PortProbe {
            self.probes.fetch_add(1, Ordering::SeqCst);
            Self::pass(&self.probe_gate);
            self.probe.clone()
        }

        fn reclaim(&self, _addr: &str) -> Result<(), String> {
            self.reclaims.fetch_add(1, Ordering::SeqCst);
            Self::pass(&self.reclaim_gate);
            match &self.reclaim_error {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        }

        fn sync(&self, _repo_root: &Path, _project: &Path) -> Result<(), String> {
            Self::pass(&self.sync_gate);
            Ok(())
        }

        fn spawn(&self, _command: &mut Command) -> std::io::Result<Child> {
            self.spawns.fetch_add(1, Ordering::SeqCst);
            if let Some((executable, delay)) = &self.artifact {
                let line = json!({
                    "reason": "compiler-artifact",
                    "target": { "kind": ["bin"] },
                    "executable": executable,
                });
                return Command::new(std::env::current_exe()?)
                    .args([
                        "--exact",
                        "stage::tests::artifact_build_child",
                        "--nocapture",
                    ])
                    .env("LUDION_STAGE_ARTIFACT_LINE", line.to_string())
                    .env(
                        "LUDION_STAGE_ARTIFACT_DELAY_MS",
                        delay.as_millis().to_string(),
                    )
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn();
            }
            if !self.sleeper {
                return Err(std::io::Error::other("fake spawn"));
            }
            Command::new(std::env::current_exe()?)
                .args(["--exact", "stage::tests::sleeper_child", "--nocapture"])
                .env("LUDION_STAGE_SLEEPER", "1")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
        }
    }

    /// The long-lived child [`FakeSteps`] launches by re-running this binary on this test. It
    /// returns at once in an ordinary run; the sleep is a safety net, not a timing the tests use.
    #[test]
    fn sleeper_child() {
        if std::env::var_os("LUDION_STAGE_SLEEPER").is_some() {
            thread::sleep(Duration::from_secs(30));
        }
    }

    /// The fake build [`FakeSteps`] launches by re-running this binary on this test: it waits
    /// `LUDION_STAGE_ARTIFACT_DELAY_MS`, prints the cargo artifact line in
    /// `LUDION_STAGE_ARTIFACT_LINE` and exits 0. An ordinary run does nothing.
    #[test]
    fn artifact_build_child() {
        if let Some(line) = std::env::var_os("LUDION_STAGE_ARTIFACT_LINE") {
            let delay = std::env::var("LUDION_STAGE_ARTIFACT_DELAY_MS")
                .ok()
                .and_then(|ms| ms.parse().ok())
                .unwrap_or(0);
            thread::sleep(Duration::from_millis(delay));
            println!("{}", line.to_string_lossy());
        }
    }

    fn opts_for(scene: &Path, project: Option<&str>) -> StageOpts {
        StageOpts {
            scene: scene.to_string_lossy().into_owned(),
            addr: Some("127.0.0.1:0".to_string()),
            project: project.map(str::to_string),
        }
    }

    /// A real file on disk so `Stage::start`'s scene-exists check passes; its content is
    /// irrelevant, since none of these tests get far enough to load it.
    fn scene_file(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, "").unwrap();
        path
    }

    /// The whole stage path against a REAL `ludion serve`: spawn → `listening` → a watcher-driven
    /// `asset.reload` over the hand-rolled wire client → graceful `engine.shutdown` → `Exited`.
    ///
    /// `#[ignore]`d because it compiles and runs the engine (minutes on a cold build) — the
    /// rest of this module is deliberately unit-testable without one. Run it after touching the
    /// handshake, the status machine, or the watcher:
    /// `cargo test --manifest-path editor/app/src-tauri/Cargo.toml -- --ignored --nocapture`.
    #[test]
    #[ignore = "spawns a real ludion serve; run explicitly after touching the wire/status/watcher"]
    fn live_stage_session_starts_reloads_and_shuts_down() {
        let repo_root = crate::pty::repo_root().unwrap();
        let scene =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/stage/scenes/model-stage.yaml");
        let (s, rx) = sink();
        let stage = Stage::new(StageSlot::Model, s, repo_root.clone(), Arc::default());
        stage
            .start(
                StageOpts {
                    scene: scene.to_string_lossy().into_owned(),
                    // Port 0 lets the OS pick, so the test never collides with a real editor session.
                    addr: Some("127.0.0.1:0".to_string()),
                    project: None,
                },
                false,
            )
            .unwrap();

        let url = loop {
            let status = rx
                .recv_timeout(Duration::from_secs(600))
                .expect("no status event; the stage engine never started");
            match status.phase {
                StagePhase::Starting => continue,
                StagePhase::Listening => break status.url.expect("listening without a url"),
                other => panic!("{other:?}: {:?}", status.message),
            }
        };

        // The watcher's own send path: a real `asset.reload` for a model the engine holds.
        wire::send_one(
            &url,
            "asset.reload",
            Some(json!({ "path": "voxels/brazier.voxel.yaml" })),
            WIRE_TIMEOUT,
        )
        .expect("asset.reload was rejected");
        // And one it does not — the blessed no-op that lets the watcher forward everything.
        wire::send_one(
            &url,
            "asset.reload",
            Some(json!({ "path": "voxels/not-a-model.voxel.yaml" })),
            WIRE_TIMEOUT,
        )
        .expect("an unheld path must be a cheap no-op, not an error");

        stage.stop();
        let terminal = loop {
            let status = rx
                .recv_timeout(Duration::from_secs(30))
                .expect("no exit status");
            if status.phase != StagePhase::Listening {
                break status;
            }
        };
        assert_eq!(terminal.phase, StagePhase::Exited, "{:?}", terminal.message);
        assert_eq!(
            terminal.code,
            Some(0),
            "graceful shutdown should exit 0: {terminal:?}"
        );
    }

    /// The kill flag alone — no graceful `engine.shutdown` — must take the engine down and free its
    /// port on every OS, because the waiter owns the engine process itself rather than a `cargo
    /// run` wrapper. Unreproducible on Windows (cargo's job object hides a wrapper kill), so this
    /// is the macOS oracle.
    ///
    /// `#[ignore]`d for the same reason as [`live_stage_session_starts_reloads_and_shuts_down`].
    /// `cargo test --manifest-path editor/app/src-tauri/Cargo.toml live_stage_kill -- --ignored --nocapture`.
    #[test]
    #[ignore = "spawns a real ludion serve; run explicitly after touching the spawn/kill path"]
    fn live_stage_kill_flag_frees_the_port() {
        let repo_root = crate::pty::repo_root().unwrap();
        let scene =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/stage/scenes/model-stage.yaml");
        let (s, rx) = sink();
        let stage = Stage::new(StageSlot::Model, s, repo_root, Arc::default());
        stage
            .start(
                StageOpts {
                    scene: scene.to_string_lossy().into_owned(),
                    addr: Some("127.0.0.1:0".to_string()),
                    project: None,
                },
                false,
            )
            .unwrap();
        let url = loop {
            let status = rx
                .recv_timeout(Duration::from_secs(600))
                .expect("no status event; the stage engine never started");
            match status.phase {
                StagePhase::Starting => continue,
                StagePhase::Listening => break status.url.expect("listening without a url"),
                other => panic!("{other:?}: {:?}", status.message),
            }
        };

        let kill = stage
            .inner
            .lock()
            .unwrap()
            .kill
            .clone()
            .expect("a running session has a kill flag");
        kill.store(true, Ordering::Relaxed);
        loop {
            let status = rx
                .recv_timeout(Duration::from_secs(30))
                .expect("no terminal status after the kill");
            if !matches!(status.phase, StagePhase::Starting | StagePhase::Listening) {
                break;
            }
        }
        let addr = url.trim_start_matches("ws://");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !port_is_free(addr) {
            assert!(
                Instant::now() < deadline,
                "the engine still holds {addr} after the kill flag"
            );
            thread::sleep(WAIT_POLL);
        }
    }

    /// The whole game-slot path against a fresh scaffolded project: `ludion new` a scratch project,
    /// spawn it as [`StageSlot::Game`], wait for `Listening`, `scene.load` its own scene over the
    /// hand-rolled wire client, then a graceful `engine.shutdown` → `Exited`. Proves the game slot
    /// builds a project outside the engine workspace, the same way a human's own project would.
    ///
    /// `#[ignore]`d for the same reason as [`live_stage_session_starts_reloads_and_shuts_down`]: it
    /// compiles and runs a real project — a cold `cargo new` + build every time, since the
    /// scratch dir is unique per run rather than a reused cache. Run it after touching the wire
    /// handshake, the game command, or the probe/busy/replace path:
    /// `cargo test --manifest-path editor/app/src-tauri/Cargo.toml live_game -- --ignored --nocapture`.
    #[test]
    #[ignore = "scaffolds and spawns a real project; run explicitly after touching wire/game/probe"]
    fn live_game_session_starts_loads_a_scene_and_shuts_down() {
        let repo_root = crate::pty::repo_root().unwrap();
        // A unique dir this run owns outright: `create_dir` (never `create_dir_all`) fails loudly
        // if it already exists, so the `remove_dir_all` at the end can only ever remove a directory
        // this test just created.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let scratch =
            std::env::temp_dir().join(format!("ludion-live-game-{}-{nanos}", std::process::id()));
        std::fs::create_dir(&scratch).expect("create a fresh scratch dir");

        let mut new_cmd = Command::new("cargo");
        new_cmd
            .args(cli_argv(&repo_root, &["new", "live-game"]))
            .current_dir(&scratch);
        spawn_posture(&mut new_cmd);
        let output = new_cmd.output().expect("run `ludion new`");
        assert!(
            output.status.success(),
            "`ludion new` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let project = scratch.join("live-game");
        let (s, rx) = sink();
        let stage = Stage::new(StageSlot::Game, s, repo_root, Arc::default());
        stage
            .start(
                StageOpts {
                    scene: project
                        .join("scenes/main.yaml")
                        .to_string_lossy()
                        .into_owned(),
                    // Port 0 lets the OS pick, so the test never collides with a real editor session.
                    addr: Some("127.0.0.1:0".to_string()),
                    project: Some(project.to_string_lossy().into_owned()),
                },
                false,
            )
            .unwrap();

        let url = loop {
            let status = rx
                .recv_timeout(Duration::from_secs(1200))
                .expect("no status event; the game never started");
            match status.phase {
                StagePhase::Starting => continue,
                StagePhase::Listening => break status.url.expect("listening without a url"),
                other => panic!("{other:?}: {:?}", status.message),
            }
        };

        // `scene.load`'s `path` resolves against the content root — the *directory* of the scene
        // the engine was started with (`<project>/scenes`), so the file within it is named alone.
        wire::send_one(
            &url,
            "scene.load",
            Some(json!({ "path": "main.yaml" })),
            WIRE_TIMEOUT,
        )
        .expect("scene.load was rejected");

        stage.stop();
        let terminal = loop {
            let status = rx
                .recv_timeout(Duration::from_secs(30))
                .expect("no exit status");
            if status.phase != StagePhase::Listening {
                break status;
            }
        };
        assert_eq!(terminal.phase, StagePhase::Exited, "{:?}", terminal.message);
        assert_eq!(
            terminal.code,
            Some(0),
            "graceful shutdown should exit 0: {terminal:?}"
        );

        // Best-effort: this run's own unique dir, never anything it did not just create.
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn listening_line_parses_only_the_announcement() {
        assert_eq!(
            parse_listening_url("listening on ws://127.0.0.1:9412").as_deref(),
            Some("ws://127.0.0.1:9412")
        );
        // The line is printed with a trailing newline and can arrive with surrounding whitespace.
        assert_eq!(
            parse_listening_url("  listening on ws://127.0.0.1:0\n").as_deref(),
            Some("ws://127.0.0.1:0")
        );
        assert_eq!(parse_listening_url("engine version: 0.1.0"), None);
        assert_eq!(parse_listening_url("listening on http://x"), None);
        assert_eq!(parse_listening_url("now listening on ws://x"), None);
        assert_eq!(parse_listening_url(""), None);
    }

    #[test]
    fn debouncer_coalesces_repeats_and_waits_out_the_quiet_period() {
        let delay = Duration::from_millis(150);
        let mut d = Debouncer::new(delay);
        let t0 = Instant::now();

        d.push("voxels/statue.voxel.yaml".into(), t0);
        d.push(
            "voxels/statue.voxel.yaml".into(),
            t0 + Duration::from_millis(100),
        );
        // The second write restarted the quiet period: nothing is due at t0+150.
        assert!(d.take_due(t0 + Duration::from_millis(150)).is_empty());
        assert_eq!(
            d.take_due(t0 + Duration::from_millis(250)),
            vec!["voxels/statue.voxel.yaml".to_string()],
            "one reload for two writes"
        );
        assert!(
            d.take_due(t0 + Duration::from_millis(400)).is_empty(),
            "and only once"
        );
    }

    #[test]
    fn debouncer_keeps_distinct_paths_apart_and_orders_a_batch() {
        let mut d = Debouncer::new(Duration::from_millis(150));
        let t0 = Instant::now();
        d.push("voxels/props/idol.voxel.yaml".into(), t0);
        d.push("voxels/enemies/statue.voxel.yaml".into(), t0);
        assert_eq!(
            d.take_due(t0 + Duration::from_millis(200)),
            vec![
                "voxels/enemies/statue.voxel.yaml".to_string(),
                "voxels/props/idol.voxel.yaml".to_string(),
            ]
        );
    }

    #[test]
    fn debouncer_deadline_drives_the_wait() {
        let mut d = Debouncer::new(Duration::from_millis(150));
        let t0 = Instant::now();
        assert_eq!(d.next_deadline(t0), None);
        d.push("a".into(), t0);
        assert_eq!(d.next_deadline(t0), Some(Duration::from_millis(150)));
        assert_eq!(
            d.next_deadline(t0 + Duration::from_millis(100)),
            Some(Duration::from_millis(50))
        );
        assert_eq!(
            d.next_deadline(t0 + Duration::from_secs(1)),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn reload_paths_are_content_root_relative_forward_slashed_and_skip_temp_siblings() {
        let root = Path::new("/p/scenes");
        assert_eq!(
            relative_asset_path(
                root,
                Path::new("/p/scenes/voxels/enemies/statue.voxel.yaml")
            )
            .as_deref(),
            Some("voxels/enemies/statue.voxel.yaml"),
            "matches how a scene references the model"
        );
        assert_eq!(
            relative_asset_path(root, Path::new("/p/scenes/sculpts/idol.sculpt.yaml")).as_deref(),
            Some("sculpts/idol.sculpt.yaml"),
            "a sculpt change reports under sculpts/ with no other change"
        );
        // The editor's own atomic write creates this sibling; the rename fires the real path.
        assert_eq!(
            relative_asset_path(root, Path::new("/p/scenes/voxels/statue.voxel.yaml.tmp")),
            None
        );
        assert_eq!(
            relative_asset_path(root, Path::new("/other/statue.voxel.yaml")),
            None
        );
        assert_eq!(relative_asset_path(root, root), None);

        #[cfg(windows)]
        assert_eq!(
            relative_asset_path(
                Path::new(r"C:\p\scenes"),
                Path::new(r"C:\p\scenes\voxels\statue.voxel.yaml")
            )
            .as_deref(),
            Some("voxels/statue.voxel.yaml")
        );
    }

    #[test]
    fn reload_loop_coalesces_a_burst_into_one_batch_and_stops_on_request() {
        let (tx, rx) = mpsc::channel::<PathBuf>();
        let stop = Arc::new(AtomicBool::new(false));
        let batches: Arc<Mutex<Vec<Vec<String>>>> = Arc::new(Mutex::new(Vec::new()));

        let loop_batches = Arc::clone(&batches);
        let loop_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            drain_reloads(rx, PathBuf::from("/p/scenes"), loop_stop, |batch| {
                loop_batches.lock().unwrap().push(batch);
                true
            });
        });

        // A save burst: the same file three times, a second file once, plus the atomic-write temp
        // sibling and something outside the content root (both dropped, not forwarded).
        for _ in 0..3 {
            tx.send(PathBuf::from("/p/scenes/voxels/boar.voxel.yaml"))
                .unwrap();
        }
        tx.send(PathBuf::from("/p/scenes/voxels/props/idol.voxel.yaml"))
            .unwrap();
        tx.send(PathBuf::from("/p/scenes/voxels/boar.voxel.yaml.tmp"))
            .unwrap();
        tx.send(PathBuf::from("/elsewhere/boar.voxel.yaml"))
            .unwrap();

        // Long enough for the quiet period to elapse and the loop to flush once.
        thread::sleep(RELOAD_DEBOUNCE * 4);
        stop.store(true, Ordering::Relaxed);
        drop(tx);
        handle.join().unwrap();

        let batches = batches.lock().unwrap();
        let flat: Vec<String> = batches.iter().flatten().cloned().collect();
        assert_eq!(
            flat,
            vec![
                "voxels/boar.voxel.yaml".to_string(),
                "voxels/props/idol.voxel.yaml".to_string(),
            ],
            "one reload per touched asset; temp siblings and foreign paths never forwarded"
        );
    }

    #[test]
    fn reload_loop_ends_when_the_sender_refuses() {
        let (tx, rx) = mpsc::channel::<PathBuf>();
        let handle = thread::spawn(move || {
            drain_reloads(
                rx,
                PathBuf::from("/p/scenes"),
                Arc::new(AtomicBool::new(false)),
                |_| false,
            );
        });
        tx.send(PathBuf::from("/p/scenes/voxels/boar.voxel.yaml"))
            .unwrap();
        // The loop must return on its own (a superseded session); the join is the assertion.
        handle.join().unwrap();
        drop(tx);
    }

    #[test]
    fn a_project_without_a_manifest_syncs_nothing() {
        // A spawn out of a repo root that does not exist would fail.
        let repo = Path::new("/ludion-no-such-repo");
        let project = std::env::temp_dir().join("ludion-stage-no-manifest");
        assert_eq!(
            sync_project(&HostChildren::default(), repo, &project),
            Ok(())
        );
    }

    #[test]
    fn sync_runs_the_repo_pinned_cli_with_json() {
        let argv = cli_argv(Path::new("/repo"), &["sync", "--project", "/p", "--json"]);
        assert_eq!(&argv[7..], &["sync", "--project", "/p", "--json"]);
    }

    #[test]
    fn cli_argv_is_the_repo_pinned_cargo_run_form() {
        let argv = cli_argv(
            Path::new("/repo"),
            &["serve", "--scene", "s.yaml", "--addr", "127.0.0.1:9412"],
        );
        assert_eq!(argv[0], "run");
        assert_eq!(argv[1], "-q");
        assert_eq!(argv[2], "--manifest-path");
        assert!(argv[3].ends_with("Cargo.toml"), "{}", argv[3]);
        assert_eq!(
            &argv[4..],
            &[
                "-p",
                "ludion-cli",
                "--",
                "serve",
                "--scene",
                "s.yaml",
                "--addr",
                "127.0.0.1:9412"
            ]
        );
    }

    /// A port nobody holds is taken as-is: no wire call, no wait, no surprise for the common start.
    #[test]
    fn reclaiming_a_free_port_is_a_no_op() {
        // A port the OS just handed out and we handed straight back: free, and named.
        let free = {
            let probe = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
            probe.local_addr().unwrap().to_string()
        };
        assert!(port_is_free(&free));
        assert_eq!(
            reclaim_port_within(&free, Duration::from_millis(50), Duration::from_millis(50)),
            Ok(())
        );
    }

    /// A listener that is not a stage engine cannot be asked to leave, so the human is told which
    /// port and what to run. The budgets are injected: the give-up path is a policy, not a wait.
    #[test]
    fn a_port_held_by_something_that_is_not_an_engine_names_the_port_and_the_remedy() {
        let held = TcpListener::bind("127.0.0.1:0").expect("a loopback port to hold");
        let addr = held.local_addr().unwrap().to_string();
        let port = addr.rsplit(':').next().unwrap().to_string();

        let err = reclaim_port_within(&addr, Duration::from_millis(50), Duration::from_millis(50))
            .unwrap_err();
        assert!(err.contains(&addr), "the message names the port: {err}");
        assert!(
            err.contains("did not answer `engine.shutdown`"),
            "it says what was tried: {err}"
        );
        assert!(
            err.contains(&port),
            "the remedy names the port too, so it can be pasted: {err}"
        );
    }

    /// The hint is the actionable half of the message: whatever the platform's verb, it carries the
    /// port, because that is the part the human has to substitute.
    #[test]
    fn the_free_the_port_hint_carries_the_port() {
        let hint = free_the_port_hint("127.0.0.1:9412");
        assert!(hint.contains("9412"), "{hint}");
        assert!(
            !hint.contains("127.0.0.1:9412"),
            "a port, not an address: {hint}"
        );
    }

    /// The incident this scan exists for: `ludion serve` on vigil's scene directory prints ~28
    /// three-line `L1155` blocks before it binds, so the bind failure under them falls off a
    /// 40-line tail entirely. The message must still lead with it.
    #[test]
    fn a_bind_failure_buried_under_a_wall_of_warnings_still_leads_the_message() {
        let mut scan = StderrScan::default();
        for _ in 0..30 {
            scan.push(
                "warning[L1155]: weapons/w.yaml: declares `kind: weapon` — skipped".to_string(),
            );
            scan.push("  --> /kind".to_string());
            scan.push("  help: host the scene through the game's own binary".to_string());
        }
        scan.push(
            "error[L3009]: could not bind 127.0.0.1:9412: Address already in use".to_string(),
        );
        scan.push("  help: check the bind address is free and well-formed, then retry".to_string());
        // More trailing lines than the tail holds: these are what evict it.
        for i in 0..(STDERR_TAIL + 5) {
            scan.push(format!("quests/q{i}.yaml: skipped"));
        }

        let message = scan.message().expect("a child that spoke has a message");
        assert!(
            message.starts_with("error[L3009]: could not bind"),
            "the reason leads, not the noise: {message}"
        );
        assert!(
            message.contains("the bind address is free"),
            "the block keeps its indented help line: {message}"
        );
        assert!(
            message.contains(&format!("quests/q{}.yaml", STDERR_TAIL + 4)),
            "the tail still follows, so nothing that came with it is lost: {message}"
        );
    }

    /// The ordinary case — the error is still in the tail, so it is not repeated above its own copy.
    #[test]
    fn an_error_still_inside_the_tail_is_not_printed_twice() {
        let mut scan = StderrScan::default();
        scan.push("warning[L1155]: skipped 28 documents: weapon (5)".to_string());
        scan.push(
            "error[L3009]: could not bind 127.0.0.1:9412: Address already in use".to_string(),
        );
        scan.push("  help: check the bind address is free and well-formed, then retry".to_string());

        let message = scan.message().expect("a child that spoke has a message");
        assert_eq!(message.matches("could not bind").count(), 1, "{message}");
        assert!(message.contains("skipped 28 documents"), "{message}");
    }

    /// A block ends at the first unindented line, and the last error wins over one the child survived.
    #[test]
    fn the_scan_keeps_the_last_error_block_and_stops_it_at_an_unindented_line() {
        let mut scan = StderrScan::default();
        scan.push("error[L1101]: a first failure it recovered from".to_string());
        scan.push("  help: recoverable".to_string());
        scan.push("Compiling ludion-cli".to_string());
        scan.push("error[L3009]: the one it died on".to_string());
        scan.push("  --> /addr".to_string());
        scan.push("note: unindented, so the block ends here".to_string());

        assert_eq!(
            scan.error,
            vec!["error[L3009]: the one it died on", "  --> /addr"],
        );
    }

    /// A child that said nothing has nothing to report, and the caller supplies the exit-code line.
    #[test]
    fn a_silent_child_yields_no_message() {
        assert_eq!(StderrScan::default().message(), None);
    }

    #[test]
    fn a_missing_stage_scene_is_a_readable_error_and_leaves_the_session_stopped() {
        let (s, rx) = sink();
        let stage = Stage::new(StageSlot::Model, s, PathBuf::from("/repo"), Arc::default());
        let missing = std::env::temp_dir().join("ludion-no-such-stage.yaml");
        let err = stage
            .start(
                StageOpts {
                    scene: missing.to_string_lossy().into_owned(),
                    addr: None,
                    project: None,
                },
                false,
            )
            .unwrap_err();
        assert!(err.contains("no scene at"), "{err}");
        assert!(err.contains("model-stage.yaml"), "{err}");
        assert_eq!(stage.status().phase, StagePhase::Stopped);
        assert!(
            rx.try_recv().is_err(),
            "a rejected start emits no status event"
        );
    }

    /// The same rejection from the other slot points at the other scene — a view is never told to
    /// create the file it does not serve.
    #[test]
    fn a_missing_effects_stage_scene_names_the_effects_scene() {
        let (s, _rx) = sink();
        let stage = Stage::new(
            StageSlot::Effects,
            s,
            PathBuf::from("/repo"),
            Arc::default(),
        );
        let missing = std::env::temp_dir().join("ludion-no-such-effects-stage.yaml");
        let err = stage
            .start(
                StageOpts {
                    scene: missing.to_string_lossy().into_owned(),
                    addr: None,
                    project: None,
                },
                false,
            )
            .unwrap_err();
        assert!(err.contains("no scene at"), "{err}");
        assert!(err.contains("effects-stage.yaml"), "{err}");
        assert!(!err.contains("model-stage.yaml"), "{err}");
        assert_eq!(stage.status().slot, StageSlot::Effects);
    }

    #[test]
    fn game_build_argv_has_no_dash_p_and_the_engine_args_carry_scene_and_addr() {
        let repo_root = Path::new("/repo");
        let project = PathBuf::from("/p/game");
        let command = StageCommand::Game {
            project: project.clone(),
            scene: "scenes/main.yaml".to_string(),
            addr: "127.0.0.1:9411".to_string(),
        };
        let argv = command.build_argv(repo_root);
        assert_eq!(
            &argv[..4],
            &[
                "build",
                "-q",
                "--message-format=json-render-diagnostics",
                "--manifest-path"
            ]
        );
        assert_eq!(argv.len(), 5, "no -p and no --bin: {argv:?}");
        assert!(argv[4].ends_with("Cargo.toml"), "{}", argv[4]);
        assert!(
            argv[4].starts_with(&project.to_string_lossy().into_owned()),
            "the manifest is under the project, not the repo: {}",
            argv[4]
        );
        assert_eq!(
            command.engine_args(),
            ["--scene", "scenes/main.yaml", "--addr", "127.0.0.1:9411"]
        );
        assert_eq!(command.cwd(repo_root), project);
    }

    #[test]
    fn cli_build_argv_builds_the_ludion_bin_from_the_repo_workspace() {
        let command = StageCommand::Cli {
            args: vec![
                "serve".to_string(),
                "--scene".to_string(),
                "s.yaml".to_string(),
            ],
        };
        let repo_root = Path::new("/repo");
        let argv = command.build_argv(repo_root);
        assert_eq!(
            &argv[..4],
            &[
                "build",
                "-q",
                "--message-format=json-render-diagnostics",
                "--manifest-path"
            ]
        );
        assert_eq!(
            argv[4],
            repo_root.join("Cargo.toml").to_string_lossy().into_owned()
        );
        assert_eq!(&argv[5..], &["-p", "ludion-cli", "--bin", "ludion"]);
        assert_eq!(command.engine_args(), ["serve", "--scene", "s.yaml"]);
        assert_eq!(command.cwd(repo_root), repo_root);
    }

    /// A `bin` `compiler-artifact` with an `executable` yields its path; a `lib` artifact, a
    /// non-artifact message, and a bin artifact without an `executable` all yield `None` — the
    /// build reader folds this over the output and keeps the last hit.
    #[test]
    fn artifact_executable_matches_only_a_bin_with_an_executable() {
        let bin = json!({
            "reason": "compiler-artifact",
            "target": { "kind": ["bin"], "name": "game" },
            "executable": "/t/debug/game"
        });
        assert_eq!(
            artifact_executable(&bin),
            Some(PathBuf::from("/t/debug/game"))
        );

        let lib = json!({
            "reason": "compiler-artifact",
            "target": { "kind": ["lib"], "name": "foo" },
            "executable": serde_json::Value::Null
        });
        assert_eq!(artifact_executable(&lib), None);

        let message = json!({ "reason": "build-finished", "success": true });
        assert_eq!(artifact_executable(&message), None);

        let bin_no_exe = json!({
            "reason": "compiler-artifact",
            "target": { "kind": ["bin"], "name": "game" },
            "executable": serde_json::Value::Null
        });
        assert_eq!(artifact_executable(&bin_no_exe), None);
    }

    #[test]
    fn game_slot_defaults_to_9411_and_does_not_watch_models() {
        assert_eq!(StageSlot::Game.default_addr(), DEFAULT_GAME_ADDR);
        assert_eq!(StageSlot::Game.default_addr(), "127.0.0.1:9411");
        assert!(!StageSlot::Game.watches_models());
    }

    #[test]
    fn game_without_a_project_is_a_readable_error() {
        let (s, _rx) = sink();
        let stage = Stage::new(StageSlot::Game, s, PathBuf::from("/repo"), Arc::default());
        let scene = scene_file("ludion-game-no-project.yaml");
        let err = stage
            .start(
                StageOpts {
                    scene: scene.to_string_lossy().into_owned(),
                    addr: None,
                    project: None,
                },
                false,
            )
            .unwrap_err();
        assert!(err.contains("needs a project directory"), "{err}");
    }

    /// A Ludion listener without `replace` is reported `Busy`, never reclaimed, never spawned.
    #[test]
    fn busy_ludion_without_replace_is_reported_busy_with_its_version() {
        let (s, rx) = sink();
        let stage = Stage::new(StageSlot::Game, s, PathBuf::from("/repo"), Arc::default());
        let scene = scene_file("ludion-game-busy-ludion.yaml");
        let status = stage
            .start_with(
                StageOpts {
                    scene: scene.to_string_lossy().into_owned(),
                    addr: Some("127.0.0.1:9411".to_string()),
                    project: Some("/p/game".to_string()),
                },
                false,
                &FakeSteps::probing(PortProbe::Busy(Listener::Ludion {
                    engine_version: "0.1.0".to_string(),
                })),
            )
            .unwrap();
        assert_eq!(status.phase, StagePhase::Busy);
        assert_eq!(status.url.as_deref(), Some("ws://127.0.0.1:9411"));
        assert_eq!(
            status.listener,
            Some(Listener::Ludion {
                engine_version: "0.1.0".to_string()
            })
        );
        assert!(
            status
                .message
                .unwrap()
                .contains("already serving a Ludion engine (0.1.0)")
        );
        let reserved = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("the reservation's starting event");
        assert_eq!(reserved.phase, StagePhase::Starting);
        let event = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("a busy status event");
        assert_eq!(event.phase, StagePhase::Busy);
    }

    /// Replace is never valid for a foreign listener, even when asked for.
    #[test]
    fn busy_foreign_with_replace_is_still_busy() {
        let (s, _rx) = sink();
        let stage = Stage::new(StageSlot::Game, s, PathBuf::from("/repo"), Arc::default());
        let scene = scene_file("ludion-game-busy-foreign.yaml");
        let status = stage
            .start_with(
                StageOpts {
                    scene: scene.to_string_lossy().into_owned(),
                    addr: Some("127.0.0.1:9411".to_string()),
                    project: Some("/p/game".to_string()),
                },
                true,
                &FakeSteps::probing(PortProbe::Busy(Listener::Foreign)),
            )
            .unwrap();
        assert_eq!(status.phase, StagePhase::Busy);
        assert_eq!(status.listener, Some(Listener::Foreign));
        assert!(status.message.unwrap().contains("not a Ludion engine"));
    }

    #[test]
    fn each_slot_binds_its_own_port_and_only_the_model_watches_assets() {
        assert_eq!(StageSlot::Model.default_addr(), DEFAULT_STAGE_ADDR);
        assert_eq!(StageSlot::Effects.default_addr(), "127.0.0.1:9413");
        assert_ne!(
            StageSlot::Model.default_addr(),
            StageSlot::Effects.default_addr(),
            "both stages run at once, so they cannot share a port"
        );
        assert!(StageSlot::Model.watches_models());
        assert!(
            !StageSlot::Effects.watches_models(),
            "an emitter is component fields; there is no asset to reload"
        );
    }

    #[test]
    fn stopping_an_idle_stage_is_a_no_op() {
        let (s, _rx) = sink();
        let stage = Stage::new(StageSlot::Model, s, PathBuf::from("/repo"), Arc::default());
        stage.stop();
        assert_eq!(stage.status().phase, StagePhase::Stopped);
    }

    /// A state dir of this test's own, so port locks and records never leak between tests (or into
    /// a developer's real editor).
    fn state_dir(name: &str) -> StateDir {
        let dir =
            std::env::temp_dir().join(format!("ludion-stage-state-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        StateDir(dir)
    }

    /// A test's state dir, removed on drop. Declare it before any lock `File` so it drops after.
    struct StateDir(PathBuf);

    impl std::ops::Deref for StateDir {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for StateDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn port_opts(scene: &Path, port: u16) -> StageOpts {
        StageOpts {
            scene: scene.to_string_lossy().into_owned(),
            addr: Some(format!("127.0.0.1:{port}")),
            project: None,
        }
    }

    fn ludion_listener() -> PortProbe {
        PortProbe::Busy(Listener::Ludion {
            engine_version: "0.1.0".to_string(),
        })
    }

    #[test]
    fn port_zero_and_unparseable_addresses_have_no_identity() {
        assert_eq!(port_of("127.0.0.1:9412"), Some(9412));
        assert_eq!(port_of("127.0.0.1:0"), None);
        assert_eq!(port_of("nonsense"), None);
    }

    #[test]
    fn a_held_port_lock_is_refused_until_the_holder_drops_it() {
        let dir = state_dir("lock-helper");
        let held = try_port_lock(&dir, 40101).unwrap().expect("a free lock");
        assert!(try_port_lock(&dir, 40101).unwrap().is_none(), "held");
        assert!(
            try_port_lock(&dir, 40102).unwrap().is_some(),
            "another port"
        );
        drop(held);
        assert!(try_port_lock(&dir, 40101).unwrap().is_some(), "released");
    }

    #[test]
    fn a_record_round_trips_once_and_a_bad_one_is_nothing() {
        let dir = state_dir("record-helper");
        assert_eq!(take_record(&dir, 40103), None, "missing");
        let record = Record {
            engine_pid: 7,
            host_pid: 9,
        };
        write_record(&dir, 40103, &record);
        assert_eq!(take_record(&dir, 40103), Some(record));
        assert_eq!(take_record(&dir, 40103), None, "taking removes it");
        std::fs::write(record_path(&dir, 40103), "not json").unwrap();
        assert_eq!(take_record(&dir, 40103), None, "unreadable");
    }

    /// An engine nobody recorded — another editor's, a `ludion serve` run by hand — is never asked
    /// to shut down.
    #[test]
    fn a_stage_never_shuts_down_an_engine_no_editor_left() {
        let (s, _rx) = sink();
        let dir = state_dir("no-record");
        let stage = Stage::new(StageSlot::Model, s, PathBuf::from("/repo"), Arc::default())
            .with_state_dir(dir.to_path_buf());
        let scene = scene_file("ludion-stage-no-record.yaml");
        let steps = FakeSteps::probing(ludion_listener());

        let status = stage
            .start_with(port_opts(&scene, 40111), false, &steps)
            .unwrap();
        assert_eq!(status.phase, StagePhase::Busy, "{status:?}");
        assert_eq!(steps.reclaimed(), 0, "nothing shut down");
        assert_eq!(steps.spawned(), 0);
        assert_eq!(stage.status().phase, StagePhase::Busy);
    }

    /// Another live editor holds the port's lock: this one asks nothing of the port at all.
    #[test]
    fn a_second_editor_gets_busy_while_the_first_holds_the_port() {
        let (s, _rx) = sink();
        let dir = state_dir("second-editor");
        let first_editor = try_port_lock(&dir, 40112)
            .unwrap()
            .expect("editor 1's lock");
        let stage = Stage::new(StageSlot::Model, s, PathBuf::from("/repo"), Arc::default())
            .with_state_dir(dir.to_path_buf());
        let scene = scene_file("ludion-stage-second-editor.yaml");
        let steps = FakeSteps::probing(ludion_listener());

        let status = stage
            .start_with(port_opts(&scene, 40112), false, &steps)
            .unwrap();
        assert_eq!(status.phase, StagePhase::Busy, "{status:?}");
        assert_eq!(steps.probed(), 0);
        assert_eq!(steps.reclaimed(), 0);
        assert_eq!(steps.spawned(), 0);
        drop(first_editor);
    }

    /// The editor that spawned the engine died without seeing it exit: its record is still on disk
    /// and its lock is gone. That engine is the orphan reclaiming exists for.
    #[test]
    fn an_orphan_recorded_by_a_dead_editor_is_reclaimed() {
        let (s, _rx) = sink();
        let dir = state_dir("orphan");
        write_record(
            &dir,
            40113,
            &Record {
                engine_pid: 1,
                host_pid: 2,
            },
        );
        let stage = Stage::new(StageSlot::Model, s, PathBuf::from("/repo"), Arc::default())
            .with_state_dir(dir.to_path_buf());
        let scene = scene_file("ludion-stage-orphan.yaml");
        let steps = FakeSteps::probing(ludion_listener());

        let _ = stage.start_with(port_opts(&scene, 40113), false, &steps);
        assert_eq!(steps.reclaimed(), 1);
        assert_eq!(steps.spawned(), 1, "the start went on to spawn");
        assert_eq!(take_record(&dir, 40113), None, "the record was consumed");
    }

    /// The lock marks a live session: held while it runs, gone once `stop()` returns.
    #[test]
    fn the_port_lock_is_released_when_the_session_ends() {
        let (s, _rx) = sink();
        let dir = state_dir("lock-lifetime");
        let stage = Stage::new(StageSlot::Model, s, PathBuf::from("/repo"), Arc::default())
            .with_state_dir(dir.to_path_buf());
        let scene = scene_file("ludion-stage-lock-lifetime.yaml");
        let steps = FakeSteps {
            sleeper: true,
            ..FakeSteps::new()
        };

        stage
            .start_with(port_opts(&scene, 40114), false, &steps)
            .unwrap();
        assert!(
            try_port_lock(&dir, 40114).unwrap().is_none(),
            "a running session holds its port's lock"
        );
        stage.stop();
        assert!(
            try_port_lock(&dir, 40114).unwrap().is_some(),
            "the lock outlived the session"
        );
    }

    /// Two starts racing through the slow pre-steps spawn one child: the second finds the first's
    /// reservation and returns it.
    #[test]
    fn a_concurrent_start_returns_the_one_in_flight() {
        let (s, _rx) = sink();
        let stage = Stage::new(StageSlot::Model, s, PathBuf::from("/repo"), Arc::default());
        let scene = scene_file("ludion-stage-concurrent.yaml");
        let steps = FakeSteps::new();
        let (entered, release) = steps.gate_probe();

        thread::scope(|scope| {
            let first = scope.spawn(|| stage.start_with(opts_for(&scene, None), false, &steps));
            entered
                .recv_timeout(Duration::from_secs(5))
                .expect("the first start never reached reclaim");

            let t0 = Instant::now();
            let second = stage.start_with(opts_for(&scene, None), false, &steps);
            let waited = t0.elapsed();
            let _ = release.send(());
            let _ = first.join().unwrap();

            let status = second.expect("a concurrent start returns the in-flight one");
            assert_eq!(status.phase, StagePhase::Starting);
            assert!(
                waited < Duration::from_secs(2),
                "second start took {waited:?}"
            );
        });
        assert_eq!(steps.spawned(), 1, "one child per reservation");
    }

    /// A stop that lands while the start is still in its slow pre-steps cancels it: nothing spawns
    /// afterwards, and the slot stays down.
    #[test]
    fn a_stop_during_a_game_start_cancels_it() {
        let (s, _rx) = sink();
        let stage = Stage::new(StageSlot::Game, s, PathBuf::from("/repo"), Arc::default());
        let scene = scene_file("ludion-stage-cancel.yaml");
        let steps = FakeSteps::new();
        let (entered, release) = steps.gate_sync();

        thread::scope(|scope| {
            let start =
                scope.spawn(|| stage.start_with(opts_for(&scene, Some("/p/game")), false, &steps));
            entered
                .recv_timeout(Duration::from_secs(5))
                .expect("the start never reached sync");
            let phase_during = stage.status().phase;

            let t0 = Instant::now();
            stage.stop();
            let stopped_in = t0.elapsed();
            let phase_after_stop = stage.status().phase;
            let _ = release.send(());
            let _ = start.join().unwrap();

            assert_eq!(
                phase_during,
                StagePhase::Starting,
                "the reservation is visible"
            );
            assert!(
                stopped_in < Duration::from_secs(1),
                "stop took {stopped_in:?}"
            );
            assert_eq!(phase_after_stop, StagePhase::Stopped);
        });
        assert_eq!(steps.spawned(), 0, "a cancelled start spawns nothing");
        assert_eq!(stage.status().phase, StagePhase::Stopped);
    }

    /// A stop cancels a start that is still reclaiming an orphan; the next start in the same editor
    /// must not find the cancelled one's port lock held against it (a second handle on a lock the
    /// process already holds is refused) and report another editor's session.
    #[test]
    fn a_cancelled_start_does_not_block_the_next_start() {
        let (s, _rx) = sink();
        let dir = state_dir("cancelled-start");
        write_record(
            &dir,
            40123,
            &Record {
                engine_pid: 1,
                host_pid: 2,
            },
        );
        let stage = Stage::new(StageSlot::Model, s, PathBuf::from("/repo"), Arc::default())
            .with_state_dir(dir.to_path_buf());
        let scene = scene_file("ludion-stage-cancelled-start.yaml");
        let steps = FakeSteps {
            probe: ludion_listener(),
            sleeper: true,
            ..FakeSteps::new()
        };
        let (entered, release) = steps.gate_reclaim();

        thread::scope(|scope| {
            let first = scope.spawn(|| stage.start_with(port_opts(&scene, 40123), false, &steps));
            entered
                .recv_timeout(Duration::from_secs(5))
                .expect("the first start never reached reclaim");
            stage.stop();
            assert_eq!(stage.status().phase, StagePhase::Stopped);

            let second = stage.start_with(port_opts(&scene, 40123), false, &steps);
            let phase = second.as_ref().map(|status| status.phase);
            let _ = release.send(());
            let _ = first.join().unwrap();
            assert_eq!(phase, Ok(StagePhase::Starting), "{second:?}");
        });
        assert_eq!(stage.status().phase, StagePhase::Starting);
        stage.stop();
        assert!(
            try_port_lock(&dir, 40123).unwrap().is_some(),
            "the lock outlived the sessions"
        );
    }

    /// A start cancelled by `stop()` and never followed by another leaves no lock behind once its
    /// slow step returns.
    #[test]
    fn a_cancelled_start_releases_the_port_lock_when_nothing_follows() {
        let (s, _rx) = sink();
        let dir = state_dir("cancelled-alone");
        write_record(
            &dir,
            40124,
            &Record {
                engine_pid: 1,
                host_pid: 2,
            },
        );
        let stage = Stage::new(StageSlot::Model, s, PathBuf::from("/repo"), Arc::default())
            .with_state_dir(dir.to_path_buf());
        let scene = scene_file("ludion-stage-cancelled-alone.yaml");
        let steps = FakeSteps::probing(ludion_listener());
        let (entered, release) = steps.gate_reclaim();

        thread::scope(|scope| {
            let first = scope.spawn(|| stage.start_with(port_opts(&scene, 40124), false, &steps));
            entered
                .recv_timeout(Duration::from_secs(5))
                .expect("the start never reached reclaim");
            stage.stop();
            let _ = release.send(());
            let _ = first.join().unwrap();
        });
        assert_eq!(stage.status().phase, StagePhase::Stopped);
        assert_eq!(steps.spawned(), 0);
        assert!(
            try_port_lock(&dir, 40124).unwrap().is_some(),
            "the cancelled start kept the port locked"
        );
    }

    /// `stop()` returning means the slot's child is gone, not merely asked to go.
    #[test]
    fn stop_returns_only_after_the_child_is_reaped() {
        let (s, _rx) = sink();
        let stage = Stage::new(StageSlot::Model, s, PathBuf::from("/repo"), Arc::default());
        let scene = scene_file("ludion-stage-reap.yaml");
        let steps = FakeSteps {
            sleeper: true,
            ..FakeSteps::new()
        };

        let status = stage
            .start_with(opts_for(&scene, None), false, &steps)
            .unwrap();
        assert_eq!(status.phase, StagePhase::Starting);
        stage.stop();
        let phase = stage.status().phase;
        assert!(
            !matches!(phase, StagePhase::Starting | StagePhase::Listening),
            "the child was still running when stop() returned: {phase:?}"
        );
    }

    /// An executable that stands in for the built engine: it leaves `started` beside itself, then
    /// sleeps. A script, because the engine is spawned with `serve --scene …` arguments the test
    /// binary would reject.
    fn fake_engine(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let started = dir.join("started");
        #[cfg(windows)]
        {
            let script = dir.join("engine.cmd");
            std::fs::write(
                &script,
                format!(
                    "@echo off\r\necho started> \"{}\"\r\nping -n 30 127.0.0.1 >nul\r\n",
                    started.display()
                ),
            )
            .unwrap();
            script
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let script = dir.join("engine.sh");
            std::fs::write(
                &script,
                format!(
                    "#!/bin/sh\necho started > '{}'\nexec sleep 30\n",
                    started.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            script
        }
    }

    fn pid_alive(pid: u32) -> bool {
        #[cfg(windows)]
        {
            let out = Command::new("tasklist")
                .args(["/NH", "/FI", &format!("PID eq {pid}")])
                .output()
                .unwrap();
            let pid = pid.to_string();
            String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .any(|word| word == pid)
        }
        #[cfg(not(windows))]
        {
            Command::new("kill")
                .args(["-0", &pid.to_string()])
                .status()
                .is_ok_and(|status| status.success())
        }
    }

    fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(25));
        }
    }

    /// The engine the waiter spawns from the build's artifact line is the process `stop()` ends:
    /// its record exists while it runs, and once `stop()` returns it is gone, its record is
    /// consumed and its port lock is free. The old `cargo run` wrapper design left it alive.
    #[test]
    fn stop_ends_the_engine_the_build_produced() {
        let (s, _rx) = sink();
        let dir = state_dir("built-engine");
        let executable = fake_engine(&dir);
        let stage = Stage::new(StageSlot::Model, s, dir.to_path_buf(), Arc::default())
            .with_state_dir(dir.to_path_buf());
        let scene = scene_file("ludion-stage-built-engine.yaml");
        let steps = FakeSteps {
            artifact: Some((executable, Duration::ZERO)),
            ..FakeSteps::new()
        };

        stage
            .start_with(port_opts(&scene, 40121), false, &steps)
            .unwrap();
        wait_until("the engine's record", || read_record(&dir, 40121).is_some());
        let record = read_record(&dir, 40121).unwrap();
        wait_until("the engine to start", || dir.join("started").is_file());
        assert!(pid_alive(record.engine_pid), "the engine runs");
        assert!(
            try_port_lock(&dir, 40121).unwrap().is_none(),
            "a running session holds its port's lock"
        );

        stage.stop();
        assert!(
            !pid_alive(record.engine_pid),
            "the engine outlived stop(): pid {}",
            record.engine_pid
        );
        assert_eq!(read_record(&dir, 40121), None, "the record was consumed");
        assert!(
            try_port_lock(&dir, 40121).unwrap().is_some(),
            "the lock outlived the session"
        );
    }

    /// A stop while the build is still running spawns no engine.
    #[test]
    fn a_stop_during_the_build_spawns_no_engine() {
        let (s, _rx) = sink();
        let dir = state_dir("stop-in-build");
        let executable = fake_engine(&dir);
        let stage = Stage::new(StageSlot::Model, s, dir.to_path_buf(), Arc::default())
            .with_state_dir(dir.to_path_buf());
        let scene = scene_file("ludion-stage-stop-in-build.yaml");
        let steps = FakeSteps {
            artifact: Some((executable, Duration::from_millis(1500))),
            ..FakeSteps::new()
        };

        stage
            .start_with(port_opts(&scene, 40122), false, &steps)
            .unwrap();
        stage.stop();
        // Past the build's delay: a build that was not killed would have handed over by now.
        thread::sleep(Duration::from_millis(2500));
        assert!(!dir.join("started").exists(), "an engine was spawned");
        assert_eq!(read_record(&dir, 40122), None);
        assert!(!matches!(
            stage.status().phase,
            StagePhase::Starting | StagePhase::Listening
        ));
    }

    #[test]
    fn a_failed_pre_step_returns_its_error_and_leaves_the_slot_stopped() {
        let (s, _rx) = sink();
        let stage = Stage::new(StageSlot::Model, s, PathBuf::from("/repo"), Arc::default());
        let scene = scene_file("ludion-stage-prestep.yaml");
        let dir = state_dir("prestep");
        write_record(
            &dir,
            40115,
            &Record {
                engine_pid: 1,
                host_pid: 2,
            },
        );
        let stage = stage.with_state_dir(dir.to_path_buf());
        let steps = FakeSteps {
            probe: ludion_listener(),
            reclaim_error: Some("port held".to_string()),
            ..FakeSteps::new()
        };

        let err = stage
            .start_with(port_opts(&scene, 40115), false, &steps)
            .unwrap_err();
        assert_eq!(err, "port held");
        assert!(
            read_record(&dir, 40115).is_some(),
            "a reclaim that failed leaves the orphan recorded"
        );
        assert_eq!(stage.status().phase, StagePhase::Stopped);
        assert_eq!(steps.spawned(), 0);
    }

    /// An attached `Busy` server is never this slot's to shut down: `stop()` must not even open a
    /// connection to it (decision 5).
    #[test]
    fn stopping_a_busy_slot_never_contacts_the_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();

        let (s, _rx) = sink();
        let stage = Stage::new(StageSlot::Game, s, PathBuf::from("/repo"), Arc::default());
        {
            let mut inner = stage.inner.lock().unwrap();
            inner.status = StageStatus {
                slot: StageSlot::Game,
                phase: StagePhase::Busy,
                url: Some(format!("ws://{addr}")),
                scene: None,
                message: None,
                code: None,
                listener: Some(Listener::Ludion {
                    engine_version: "0.1.0".to_string(),
                }),
            };
        }

        stage.stop();

        match listener.accept() {
            Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::WouldBlock),
            Ok(_) => panic!("stop() connected to a listener this slot did not spawn"),
        }
        assert_eq!(stage.status().phase, StagePhase::Busy);
    }

    /// The url on a terminal status is stale: `stop()` must not contact it either.
    #[test]
    fn stopping_an_exited_slot_never_contacts_its_stale_url() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();

        let (s, _rx) = sink();
        let stage = Stage::new(StageSlot::Game, s, PathBuf::from("/repo"), Arc::default());
        {
            let mut inner = stage.inner.lock().unwrap();
            inner.status = StageStatus {
                slot: StageSlot::Game,
                phase: StagePhase::Exited,
                url: Some(format!("ws://{addr}")),
                scene: None,
                message: None,
                code: Some(0),
                listener: None,
            };
        }

        stage.stop();

        match listener.accept() {
            Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::WouldBlock),
            Ok(_) => panic!("stop() connected to a stale url"),
        }
        assert_eq!(stage.status().phase, StagePhase::Exited);
    }

    #[test]
    fn reuses_running_same_serving_is_true() {
        let p = Path::new("/p/a");
        assert!(reuses_running(StagePhase::Listening, Some(p), Some(p)));
        assert!(reuses_running(StagePhase::Starting, Some(p), Some(p)));
    }

    #[test]
    fn reuses_running_different_project_is_false() {
        assert!(!reuses_running(
            StagePhase::Listening,
            Some(Path::new("/p/a")),
            Some(Path::new("/p/b")),
        ));
    }

    #[test]
    fn reuses_running_a_stage_on_another_content_root_is_false() {
        assert!(!reuses_running(
            StagePhase::Listening,
            Some(Path::new("/p/a/scenes")),
            Some(Path::new("/p/b/scenes")),
        ));
        assert!(!reuses_running(
            StagePhase::Starting,
            None,
            Some(Path::new("/p/b/scenes"))
        ));
    }

    #[test]
    fn reuses_running_exited_is_false() {
        let p = Path::new("/p/a");
        assert!(!reuses_running(StagePhase::Exited, Some(p), Some(p)));
    }

    #[test]
    fn reuses_running_busy_is_false() {
        let p = Path::new("/p/a");
        assert!(!reuses_running(StagePhase::Busy, Some(p), Some(p)));
    }

    #[test]
    fn status_from_a_superseded_generation_is_ignored() {
        let (s, rx) = sink();
        let stage = Stage::new(StageSlot::Model, s, PathBuf::from("/repo"), Arc::default());
        stage.inner.lock().unwrap().generation = 7;
        stage.set_status(
            6,
            StageStatus {
                slot: StageSlot::Model,
                phase: StagePhase::Failed,
                url: None,
                scene: None,
                message: Some("stale".into()),
                code: Some(1),
                listener: None,
            },
        );
        assert_eq!(stage.status().phase, StagePhase::Stopped);
        assert!(rx.try_recv().is_err());
    }
}
