//! Implementation of `ludion dev [--scene <FILE>] [--addr <ADDR>]` — the designer's hot-reload loop.
//!
//! `ludion dev` is a **supervisor**: a peer protocol client plus a child-process manager
//! (engine-architecture.md §8, `Plan/phase-7-designer-flow.md` decisions 1–2). It runs from the
//! game's own directory — `cargo build` and launching the game with cwd = the project dir work
//! identically for a workspace member (`games/*`) and a standalone `ludion new` scaffold, so there is
//! no `-p` package-selection logic. The loop:
//!
//! 1. Watch the game's `src/` (`ludion_assets::watch`, debounced ~300 ms).
//! 2. On a change: `cargo build`, with the game package's cargo fingerprints purged first so the
//!    build always recompiles the package from the bytes on disk (see [`cargo_build`] — cargo's
//!    mtime freshness guess can wrongly no-op and "succeed" with a binary that never saw the edit).
//!    A **failed build never kills the running child** — cargo's errors pass through to dev's stderr
//!    and the designer keeps playing the last good binary. A watcher batch with no `src/` change since
//!    the last build (a late second batch of an already-built save) is ignored.
//! 3. On build success: `engine.snapshot` over the wire from the running child, then `engine.shutdown`
//!    (hard-kill fallback on timeout), then start the freshly built child on the **same port**
//!    with `--restore <the kept snapshot>`: the child runs `engine.restore` before it binds, never
//!    loading the scene, and prints its outcome (`restored frame=… report=…` or `restore failed: …`)
//!    ahead of its `listening on ws://…` line, which dev waits for. Restore installs the snapshot's
//!    run-state, so a game that was running resumes running. Held input is dropped from the snapshot
//!    first: the old child's key-ups cannot reach it across a recompile.
//!
//!    If the `engine.snapshot` call itself fails, the designer's state cannot be had, and keeping a
//!    child that did not answer would mean the edit never runs. dev probes the child with a
//!    non-blocking `try_wait` only to word the message — a crash, or a live child that timed out
//!    (a wedged sim), refused, or skews the schema (`L3003`) — then stops the old child and starts a
//!    fresh (unpaused, unrestored) one from the just-built exe, so the watch loop keeps serving even
//!    though the state is lost. Every wire call dev makes has a timeout.
//!
//!    The snapshot file `<project>/.ludion/dev-snapshot.json` exists exactly while a snapshot is
//!    kept: written before the old child stops, deleted once a restore succeeds. A fresh `ludion dev`
//!    start that finds it resumes it, restoring through the same path a reload does.
//! 4. **Independently, if the game has a `scenes/` directory it is watched too** (same
//!    `ludion_assets::watch`, same debounce) — but a scenes change never rebuilds and never restores.
//!    The running world is deliberately preserved (only a *code* reload calls `engine.restore`), so a
//!    designer editing `scenes/*.yaml` would otherwise see nothing happen and have no idea why
//!    (friction #5, `phase-7-sokoban-proof.md`). dev instead prints a one-line hint and leaves applying
//!    the edit — `scene.load` over the wire, or a restart — to the designer.
//!
//!    The hint names *both* outcomes because the watched directory is also the child's **content
//!    root**: mesh/voxel assets and game data-docs live there too, and the child's own asset watch
//!    (`crates/ludion/src/lib.rs`) reloads those live, at a frame boundary, with no dev involvement.
//!    Only the scene file itself needs `scene.load` or a restart. dev cannot tell the two apart from
//!    a path — a game's registered doc kinds live in the game binary — so it states the rule rather
//!    than guessing, and the precise per-file signal stays where it already is: the `asset.reloaded`
//!    wire event.
//! 5. A project's `[[import]]` trees (`ludion sync`) are brought fresh at startup and before every
//!    reload's build; a sync failure is a failed build (the running child keeps serving). dev does
//!    **not** watch `art/`: a re-exported kit lands on the next reload or restart.
//!
//! The snapshot and the shutdown are ordinary wire commands through `ludion-client`; the restore is
//! the host's `--restore` boot flag. Parity by construction, no dev-private endpoints.
//!
//! ## Why the child runs a *copy* of the built binary
//!
//! On Windows a running `.exe` is locked, so `cargo build` cannot relink `target/…/<game>.exe` while
//! that binary is the running child — the link step would fail with "access denied", turning a
//! perfectly good edit into a spurious build failure. To keep "build first, only then stop the old
//! child" working on the primary dev platform, dev launches each child from a throwaway **copy** of
//! the built binary. The canonical build output is never locked, so a rebuild while the last-good
//! child is still serving always succeeds — which is exactly what lets a failed build leave that child
//! untouched.
//!
//! The copies live in a per-session directory, `<temp>/ludion-dev-session-<pid>/`, in which dev holds
//! an exclusive lock on `lock` for its whole life. Each new `ludion dev` first sweeps the session
//! directories whose lock is free and has sat untouched for a few seconds — sessions that died without
//! cleaning up — so nothing accumulates in the temp dir.
//!
//! ## Why the child cannot outlive dev
//!
//! dev holds the write end of every child's stdin, and the child (`--supervised`) exits gracefully
//! when it reaches EOF. The OS closes that pipe however dev dies — Ctrl-C, a panic, a hard kill — with
//! no platform-specific job object or process group, so an orphaned game never keeps the port.
//!
//! ## Status lines (stdout, one per step — the exit-gate test's parse surface)
//!
//! - `dev watching <src-dir>` — once, after the first child is up.
//! - the child's `listening on ws://…` line, relayed verbatim (first start and every reload).
//! - `dev reload building` — a change was detected and a rebuild started.
//! - `dev reload build failed` — the rebuild failed; the old child is still serving.
//! - `dev reload child exited; state lost, relaunching` — the build succeeded but the running child
//!   had already crashed on its own (a snapshot could not reach it, and `try_wait` confirmed it was
//!   gone); dev is starting a fresh, unrestored child from the just-built exe. Followed by that child's
//!   own `listening on ws://…` line — never by a `restored` line, since there was nothing to restore.
//! - `dev reload snapshot failed (<code>: <cause>); restarting fresh, session state lost` — the
//!   running child is alive but did not deliver its snapshot (a wedged sim times out as `L3054`, a
//!   schema skew is `L3003`); dev stops it and starts a fresh child, as above.
//! - `dev reload restored frame=<N> in <ms> ms` — the reload completed; `<ms>` is the
//!   source-change-detected → restored wall time. Wall clock is legitimate here: dev is the CLI
//!   supervisor, not sim-side.
//! - `dev resumed frame=<N> from the kept snapshot <path> in <ms> ms` — a fresh start found a kept
//!   snapshot and restored it (status `ok` with `restored_frame` and a `message`); the file is
//!   deleted. Delete it before starting dev to start fresh. A resume that does not restore prints
//!   `dev resume failed; running a fresh world, snapshot kept at <path> (…)` with status
//!   `restore_failed`, as a reload's restore failure does; a resume whose child cannot boot ends
//!   dev with the `L0013` and the file kept, so the rerun resumes.
//! - `dev reload launch failed; snapshot kept at <path>, retrying on the next save` — the build
//!   succeeded but the new child could not boot (its `L0013` is on stderr). dev keeps running with no
//!   child; the next save launches and restores the kept snapshot. Without a snapshot (an earlier boot
//!   failed before one was taken) the line is `dev reload launch failed; retrying on the next save`.
//! - `dev reload aborted; could not keep the snapshot at <path>, the old child is still serving` — the
//!   snapshot could not be written, so the old child was not stopped.
//! - `dev reload restore failed; running a fresh world, snapshot kept at <path> (…)` — the new child
//!   came up but the snapshot would not restore (the diagnostic is on stderr). The child fell back to
//!   loading the scene, so the fresh world runs, and dev keeps the snapshot at
//!   `<project>/.ludion/dev-snapshot.json`. The next save retries it, discarding the fresh world;
//!   deleting the file drops it.
//! - `dev scenes changed (world state is preserved; assets and data-docs reload live, scene edits
//!   need scene.load or restart)` — a `scenes/` file changed. No rebuild, no restart, no world
//!   mutation *by dev* — just the hint; only printed for a game that has a `scenes/` directory.
//! - `dev watching shaders <dir>` — once, if `dev` is running inside the engine workspace and found
//!   `crates/ludion-renderer/src/shaders/` to watch. Its absence means the WGSL fast path is off
//!   (a standalone game, or the engine installed as a dependency — there is no shader source on disk).
//! - `dev shader reloaded <name> in <ms> ms` — a watched `.wgsl` changed and was hot-reloaded over the
//!   wire (the WGSL fast path): no rebuild, no snapshot, no restart, so `<ms>` (watch → reload ack) is
//!   sub-second. A rejected edit (bad WGSL / a binding-shape change — `L2009`) prints to stderr
//!   instead; the child kept its last good pipeline and keeps serving.
//!
//! ## The build-status file
//!
//! The status lines above stream to dev's own stdout/stderr, where the attached Tauri editor never
//! sees them. So on every build lifecycle transition dev also writes a machine-readable status file —
//! `<project>/.ludion/dev-status.json` (the same `.ludion/` scratch dir the editor context lives in,
//! already gitignored/loreignored) — that the editor tails to show build state and rustc errors it
//! could not otherwise reach. It carries a `state` (`building`/`ok`/`failed`/`crashed`/
//! `restore_failed`), a per-process monotonic `seq` (so a tailing client detects a change by the
//! number, not a diff), an RFC3339 `timestamp`, and per-state extras: a build `duration_ms` on
//! `ok`/`failed`/`restore_failed`, a `restored_frame` and (when the migration was not clean) the full
//! per-entity `report` on a reload's `ok`, a `message` and the kept `snapshot` path on
//! `restore_failed`, a `message` on `crashed` and on the `ok` after it (the state was lost; no
//! `restored_frame`) and on a resume's `ok` (which carries `restored_frame` too), and the flat
//! `diagnostics` on `failed` (rustc's, or the `L0013` of a child that could not boot, with the kept
//! `snapshot` path). The write is atomic (`ludion_core::atomic`) and
//! always nonfatal — a failed status write prints one warning and never gates the reload loop. See
//! [`DevStatus`].
//!
//! ## The WGSL fast path
//!
//! Renderer shaders are compiled into the engine, so a game project has no shader source to watch —
//! the fast path targets the **engine developer** tuning shaders against a running scene. When `dev`
//! runs from inside the engine workspace it also watches `crates/ludion-renderer/src/shaders/` and
//! launches its children with `--dev-shaders`; a `.wgsl` edit there is forwarded as an `asset.reload`
//! over the live connection, which recreates just that shader's pipeline (keep-last-good on a bad
//! edit) — bypassing the whole rebuild → snapshot → restart cycle. Outside the engine workspace the
//! path is inert and `dev` behaves exactly as before.
//!
//! ## What reaches the child, and what does not
//!
//! A child's argv is exactly `--addr`, `--scene`, `--restore`, `--dev-shaders`, `--probes rebake`,
//! `--aa <mode>` and `--supervised` (see `try_launch_once`) — the things dev itself decides; `--supervised` is what makes the child
//! exit when dev's end of its stdin closes. Nothing else on `ludion dev`'s own
//! command line is forwarded, deliberately: dev supervises a game binary whose flags are the game's
//! (`ludion::run`'s `RunArgs`), not this CLI's, and translating between the two would make dev a
//! place where a flag can be silently dropped.
//!
//! The per-machine knobs reach the child anyway, by plain **environment inheritance**: a spawned
//! child inherits `LUDION_THREADS`, `LUDION_QUALITY` and `LUDION_RENDER_SIZE`, and resolves each for
//! itself through the same precedence chains a host uses. That is the whole mechanism, and it is why
//! those three are environment variables rather than flags — `LUDION_RENDER_SIZE=960x540 ludion dev`
//! streams a smaller viewport with no argv plumbing at all. A `ludion dev --quality low` (or
//! `--render-size`), by contrast, refuses to start on a malformed value and then reaches nothing: the
//! flags are resolved in dev's own process, which renders nothing.

use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded};
use ludion_client::Client;
use ludion_core::atomic::write_atomic;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_engine::AntiAliasing;
use ludion_protocol::command::AssetReloadParams;
use ludion_protocol::restore::{RestoreEntry, RestoreReport};
use ludion_protocol::{Command as WireCommand, ResponseBody};
use serde::Serialize;
use serde_json::Value;

use std::process::ExitCode;

/// How long to wait for a freshly launched child to print its `listening on …` line before treating
/// the launch as hung. It covers the boot and, for a `--restore` child, the in-process restore: vigil
/// does both in about a second, so the generous bound only matters on a loaded machine.
const LISTEN_TIMEOUT: Duration = Duration::from_secs(20);
/// How long to keep retrying the bind window when a restarted child dies immediately (a same-port
/// rebind race: `TIME_WAIT`, or the previous process not yet fully gone). A bind failure fails fast
/// (the child exits at once), so within this window dev spawns several fresh attempts.
const BIND_RETRY_WINDOW: Duration = Duration::from_secs(5);
/// The pause between bind-window retry attempts.
const BIND_RETRY_DELAY: Duration = Duration::from_millis(150);
/// How long to wait for a child to exit after `engine.shutdown` before hard-killing it.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
/// How long `engine.snapshot` may take before the child counts as wedged: a big world on a loaded box
/// stays well inside it, and it fires long before the server's own 120 s `L3023`.
const SNAPSHOT_CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// How long `engine.shutdown` may take to answer. The reply is ignored; dropping the child waits
/// [`SHUTDOWN_GRACE`] and kills.
const SHUTDOWN_CALL_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a shader `asset.reload` may take — a pipeline compile is slower than any other command.
const SHADER_RELOAD_CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// The `src/` watch debounce — long enough to coalesce an editor's multi-file save, short enough to
/// feel immediate.
const WATCH_DEBOUNCE: Duration = Duration::from_millis(300);

/// The verbatim status lines that carry no data. `restored`/`watching` are formatted below.
const LINE_BUILDING: &str = "dev reload building";
const LINE_BUILD_FAILED: &str = "dev reload build failed";
/// Printed when a snapshot failure turns out to be a crashed child (see [`snapshot_failure`]), right
/// before dev relaunches fresh from the just-built exe.
const LINE_CHILD_CRASHED: &str = "dev reload child exited; state lost, relaunching";
/// Printed when a `scenes/` file changes — a hint, never a trigger (see the module docs, point 4).
///
/// The line speaks for the whole content root, not just the scene file: the
/// watched directory also holds mesh/voxel assets and game data-docs, and the *child* reloads those
/// live through its own asset watch (`crates/ludion/src/lib.rs`) — dev cannot claim they need a
/// restart. It names both outcomes because a coalesced batch may carry either kind, and dev has no
/// way to tell them apart (a game's registered doc kinds live in the game binary, not here).
const LINE_SCENES_CHANGED: &str = "dev scenes changed (world state is preserved; assets and data-docs reload live, scene edits need scene.load or restart)";

/// Unique-per-launch counter for the throwaway binary copies (prefixes the copy's filename).
static COPY_SEQ: AtomicU64 = AtomicU64::new(0);

/// The prefix of a dev process's temp session directory. Distinct from the unit tests' own
/// `ludion-dev-*` scratch names, which the sweep must never touch.
const SESSION_DIR_PREFIX: &str = "ludion-dev-session-";
/// How long a session's `lock` file (or, without one, its directory) must sit untouched before the
/// sweep may judge it dead — a session between creating its lock and locking it looks unlocked.
const SESSION_GRACE: Duration = Duration::from_secs(10);
/// This process's session directory, set once by [`start_session`] at the top of [`run`]; where every
/// child's binary copy lives. Unset outside `run`, where a launch is an error.
static SESSION_DIR: OnceLock<PathBuf> = OnceLock::new();
/// The exclusive lock on the session's `lock` file, held for the whole process: the OS releasing it on
/// exit is the liveness signal [`sweep_dead_sessions`] reads.
static SESSION_LOCK: OnceLock<std::fs::File> = OnceLock::new();

/// The machine-readable build-status file dev writes on every build lifecycle transition, so the
/// attached editor can tail build state and rustc errors it otherwise never sees (they only stream to
/// dev's own stderr). Set once, at the top of [`run`], to `<project>/.ludion/dev-status.json` (the
/// same `.ludion/` scratch dir the editor context lives in — already gitignored/loreignored). Left
/// unset in tests and any path that never reaches `run`, where every status write is then a no-op.
static STATUS_PATH: OnceLock<PathBuf> = OnceLock::new();
/// The status file's `seq`: monotonically increasing per dev process, starting at 0, so a tailing
/// client detects a change by the number rather than diffing the whole file.
static STATUS_SEQ: AtomicU64 = AtomicU64::new(0);

/// The build lifecycle state written to `dev-status.json` (serialized lowercase).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum BuildState {
    /// A build started (the initial build, or a rebuild after a source change).
    Building,
    /// A build succeeded and its child is serving (with `restored_frame` set once a reload restored
    /// the snapshot; absent on the initial launch, which restores nothing).
    Ok,
    /// A build failed, or the rebuilt child could not boot; `diagnostics` carries the rustc errors, or
    /// the `L0013` with the child's output (and `snapshot` the kept snapshot's path, when one waits).
    Failed,
    /// A reload could not snapshot the running child — it had exited, or did not answer — so its state
    /// was lost and dev relaunches fresh (see [`SnapshotFailure`]); `message` is the cause.
    Crashed,
    /// A reload's new child came up but `engine.restore` failed: it is serving a fresh world (running,
    /// not paused) and the snapshot is kept on disk — `message` and `snapshot` say why and where.
    #[serde(rename = "restore_failed")]
    RestoreFailed,
}

/// One rustc diagnostic, flattened to the primary span — the shape the editor renders. `rendered` is
/// rustc's own full multi-line rendering (the exact text a human sees in the terminal); the flat
/// fields are for structured display (jump-to-file, error-list). `code` is the `E####`/lint name when
/// rustc attached one; `file`/`line`/`column` come from the primary span, absent for a span-less
/// diagnostic (e.g. an unresolved-crate error).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct StatusDiagnostic {
    level: String,
    code: Option<String>,
    message: String,
    file: Option<String>,
    line: Option<u64>,
    column: Option<u64>,
    rendered: String,
}

/// The `dev-status.json` document. Field order here is the serialized order (serde emits struct
/// fields in declaration order); the optional tail fields are omitted rather than written `null` when
/// they do not apply to the current `state`, so each state's document carries only its own facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct DevStatus {
    state: BuildState,
    seq: u64,
    timestamp: String,
    /// Build duration in milliseconds — present on `ok`/`failed`, absent while `building`/`crashed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u128>,
    /// The frame a successful reload restored — present on `ok` after a restore, absent on the initial
    /// launch (which restores nothing) and every other state.
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_frame: Option<u64>,
    /// The restore's full per-entity migration report — present on a reload's `ok` whose restore was
    /// not clean (dev's stdout carries the grouped summary).
    #[serde(skip_serializing_if = "Option::is_none")]
    report: Option<RestoreReport>,
    /// Why the restore failed (`restore_failed`), or why the session state was lost — a `crashed`
    /// reload, and the `ok` that follows once the fresh child serves (which has no `restored_frame`).
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    /// The kept snapshot's path — present on `restore_failed`, and on a `failed` whose child could not
    /// boot while a snapshot waits for the next save.
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot: Option<String>,
    /// The build's rustc diagnostics — present (possibly empty) on `failed`, absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostics: Option<Vec<StatusDiagnostic>>,
}

/// Writes `building` — a build has started. No-op until [`STATUS_PATH`] is set (i.e. outside `run`).
fn status_building() {
    write_status(BuildState::Building, None, None, None, None, None, None);
}

/// Writes `ok` — a build succeeded and a child is serving. `restored_frame` is the reload's restored
/// frame, or `None` on the initial launch and when the session state was lost (`message` then says
/// so). `report` is the restore's full migration report when it is not clean.
fn status_ok(
    duration_ms: Option<u128>,
    restored_frame: Option<u64>,
    message: Option<String>,
    report: Option<RestoreReport>,
) {
    write_status(
        BuildState::Ok,
        duration_ms,
        restored_frame,
        message,
        None,
        None,
        report,
    );
}

/// Writes `failed` — a build failed, or the rebuilt child could not boot — carrying its diagnostics
/// (possibly empty). `snapshot` is the kept snapshot's path when one waits for the next save.
fn status_failed(duration_ms: u128, diagnostics: Vec<StatusDiagnostic>, snapshot: Option<String>) {
    write_status(
        BuildState::Failed,
        Some(duration_ms),
        None,
        None,
        snapshot,
        Some(diagnostics),
        None,
    );
}

/// Writes `crashed` — a reload could not snapshot the running child (it had exited, or did not
/// answer), so its state is lost and dev relaunches fresh. `message` is the cause; the `ok` that
/// follows once the fresh child serves keeps saying the world was not restored.
fn status_crashed(message: String) {
    write_status(
        BuildState::Crashed,
        None,
        None,
        Some(message),
        None,
        None,
        None,
    );
}

/// Writes `restore_failed` — the new child is serving a fresh world because its restore failed;
/// `message` is the diagnostic and `snapshot` the path of the snapshot kept for a retry.
fn status_restore_failed(duration_ms: u128, message: String, snapshot: String) {
    write_status(
        BuildState::RestoreFailed,
        Some(duration_ms),
        None,
        Some(message),
        Some(snapshot),
        None,
        None,
    );
}

/// Allocates the next monotonic per-process `seq` (0 on the first call). A plain atomic fetch-add, so
/// concurrent writers (the windowed mode's supervisor runs on a worker thread) never collide on a
/// value.
fn next_seq() -> u64 {
    STATUS_SEQ.fetch_add(1, Ordering::Relaxed)
}

/// Assembles and atomically writes the status document, allocating the next monotonic `seq`. A failed
/// write (or an unset [`STATUS_PATH`]) never kills dev: the path being unset is a silent no-op (tests,
/// non-`run` paths), and any IO error prints a one-line nonfatal warning and returns. The status file
/// is a convenience for the editor, never a load-bearing part of the reload loop.
fn write_status(
    state: BuildState,
    duration_ms: Option<u128>,
    restored_frame: Option<u64>,
    message: Option<String>,
    snapshot: Option<String>,
    diagnostics: Option<Vec<StatusDiagnostic>>,
    report: Option<RestoreReport>,
) {
    let Some(path) = STATUS_PATH.get() else {
        return;
    };
    let status = DevStatus {
        state,
        seq: next_seq(),
        timestamp: clock::now_rfc3339_utc(),
        duration_ms,
        restored_frame,
        report,
        message,
        snapshot,
        diagnostics,
    };
    if let Err(error) = write_status_file(path, &status) {
        eprintln!(
            "{}",
            Diagnostic::new(
                code::DEV_DEGRADED,
                format!("could not write {}: {error}", path.display()),
            )
            .with_help("the editor will not see build state; the reload loop is unaffected")
        );
    }
}

/// Serializes `status` and writes it to `path` through [`ludion_core::atomic`], creating `.ludion/`
/// if missing. A reader tailing the file therefore always sees a complete document, never a
/// half-written one.
fn write_status_file(path: &Path, status: &DevStatus) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(status)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    write_atomic(path, json)
}

/// Extracts the flat [`StatusDiagnostic`] from a `compiler-message`'s inner rustc `message` object,
/// or `None` when the level is neither `error` nor `warning` (notes/help arrive as children inside a
/// parent's `rendered`, not as their own status entries). The primary span (`is_primary: true`) — if
/// any — supplies `file`/`line`/`column`; a span-less diagnostic leaves them absent.
fn diagnostic_from_message(message: &Value) -> Option<StatusDiagnostic> {
    let level = message.get("level").and_then(Value::as_str)?;
    if level != "error" && level != "warning" {
        return None;
    }
    let primary = message
        .get("spans")
        .and_then(Value::as_array)
        .and_then(|spans| {
            spans
                .iter()
                .find(|span| span.get("is_primary").and_then(Value::as_bool) == Some(true))
        });
    Some(StatusDiagnostic {
        level: level.to_string(),
        code: message
            .get("code")
            .and_then(|c| c.get("code"))
            .and_then(Value::as_str)
            .map(str::to_string),
        message: message
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        file: primary
            .and_then(|s| s.get("file_name"))
            .and_then(Value::as_str)
            .map(str::to_string),
        line: primary
            .and_then(|s| s.get("line_start"))
            .and_then(Value::as_u64),
        column: primary
            .and_then(|s| s.get("column_start"))
            .and_then(Value::as_u64),
        rendered: message
            .get("rendered")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

/// Formats a [`SystemTime`] as an RFC3339 UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`, second precision).
/// A pure function of its argument — no clock read — so the timestamp shape is unit-testable against a
/// fixed instant. Kept dependency-free (no `chrono`/`time` in the workspace) via Howard Hinnant's
/// `civil_from_days` calendar algorithm. A time before the Unix epoch (not reachable from a real
/// clock) clamps to the epoch rather than underflowing.
fn rfc3339_utc(time: SystemTime) -> String {
    let secs = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    let (hour, minute, second) = (tod / 3600, (tod % 3600) / 60, tod % 60);

    // civil_from_days: days since 1970-01-01 -> (year, month [1,12], day [1,31]).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // year of era [0, 399]
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year [0, 365]
    let mp = (5 * doy + 2) / 153; // month, shifted so March=0 [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Runs `ludion dev`. Blocks until Ctrl-C, a window close (with `--window`), or a fatal reload
/// failure, then returns.
///
/// `scene` and `addr` forward to the child. `scene` is optional — the child's builder may carry a
/// `default_scene`. The first child is a **plain start** (no `--restore`): its run-state comes from
/// the scene/child defaults, the same as `cargo run` would give. Only *reloaded* children boot from
/// the kept snapshot, whose run-state they install.
///
/// With `--window`, dev hosts **one persistent native window in this process** across every restart
/// (decision 6): the supervision loop moves to a worker thread so winit can keep the main thread, and
/// the window rides each reload's same-port blink via `ludion-window`'s reconnect. The child is
/// always headless — passing `--window` to the child would make the window die and reopen on every
/// reload.
pub fn run(
    scene: Option<&Path>,
    addr: &str,
    window: bool,
    rebake_probes: bool,
    anti_aliasing: Option<AntiAliasing>,
) -> ExitCode {
    let project = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(error) => {
            eprintln!(
                "{}",
                Diagnostic::new(
                    code::SCENE_IO,
                    format!("could not read the current directory: {error}"),
                )
                .with_help("run `ludion dev` from the game project's directory")
            );
            return ExitCode::FAILURE;
        }
    };

    if let Err(diagnostic) = check_runnable_game(&project) {
        eprintln!("{diagnostic}");
        return ExitCode::FAILURE;
    }

    // From here on, build lifecycle transitions are mirrored to `<project>/.ludion/dev-status.json`
    // (see [`STATUS_PATH`]) so the attached editor can tail build state and rustc errors. A failed
    // write is always nonfatal — the status file never gates the reload loop.
    let _ = STATUS_PATH.set(project.join(".ludion").join("dev-status.json"));

    if let Err(error) = start_session() {
        eprintln!(
            "{}",
            Diagnostic::new(
                code::OUTPUT_IO,
                format!("could not create the dev session directory in the temp dir: {error}"),
            )
            .with_help("check that the temp directory exists and is writable")
        );
        return ExitCode::FAILURE;
    }

    // The engine shader dir (present only when `dev` runs from inside the engine workspace) enables
    // The WGSL fast path: children launch with `--dev-shaders` and a `.wgsl` edit reloads over
    // the wire. A standalone game leaves this `None` and everything below is unchanged.
    let shader_dir = find_engine_shaders(&project);
    let flags = ChildFlags {
        dev_shaders: shader_dir.is_some(),
        rebake_probes,
        anti_aliasing,
    };

    // First start: sync the derived imports, build, then launch a plain (not paused) child and wait
    // for it to serve.
    status_building();
    let build_watch = Stopwatch::start();
    match crate::sync::ensure_fresh(&project, &crate::cook::CookRequest::default()) {
        Ok(summary) => report_sync(&summary),
        Err(diagnostics) => {
            for diagnostic in &diagnostics {
                eprintln!("{diagnostic}");
            }
            status_failed(
                build_watch.elapsed_ms(),
                status_from_diagnostics(&diagnostics),
                None,
            );
            eprintln!(
                "ludion dev: stopped — the derived-import sync failed; fix the errors above and \
                 rerun"
            );
            return ExitCode::FAILURE;
        }
    }
    let exe = match cargo_build(&project) {
        Ok(exe) => exe,
        Err(diagnostics) => {
            status_failed(build_watch.elapsed_ms(), diagnostics, None);
            eprintln!(
                "ludion dev: stopped — the initial build failed; fix the errors above and rerun"
            );
            return ExitCode::FAILURE;
        }
    };
    let build_ms = build_watch.elapsed_ms();
    // A snapshot a previous session left (a reload whose child never booted, or whose restore failed)
    // resumes here, through the same path a reload restores by; the file's presence is the whole signal.
    let resume = kept_snapshot_path(&project);
    let child = if resume.is_file() {
        let launch_watch = Stopwatch::start();
        let ctx = Relaunch {
            project: &project,
            addr,
            scene,
            exe: &exe,
            flags,
            build_ms,
            watch: &launch_watch,
            origin: RestoreOrigin::Resume,
        };
        match relaunch_and_restore(&ctx, &mut true) {
            Ok(child) => child,
            Err(diagnostic) => {
                eprintln!(
                    "{}",
                    diagnostic.append_help(format!(
                        "fix it and rerun `ludion dev`; the kept snapshot {} resumes then",
                        resume.display()
                    ))
                );
                return ExitCode::FAILURE;
            }
        }
    } else {
        match launch(&project, addr, scene, None, &exe, flags) {
            Ok(child) => {
                // The initial launch restores nothing, so `restored_frame` is absent on this first `ok`.
                status_ok(Some(build_ms), None, None, None);
                child
            }
            Err(diagnostic) => {
                eprintln!(
                    "{}",
                    diagnostic.append_help("fix it and rerun `ludion dev`")
                );
                return ExitCode::FAILURE;
            }
        }
    };
    warn_on_version_skew(&child.url);

    // The concrete address the first child bound (which may differ from `--addr` when it asked for an
    // OS-chosen port, `:0`). Every reload rebinds *this* exact address, so the viewer's connection
    // survives the restart on the same port.
    let concrete_addr = match child.url.strip_prefix("ws://") {
        Some(addr) => addr.to_string(),
        None => {
            eprintln!(
                "{}",
                Diagnostic::new(
                    code::DEV_CHILD_FAILED,
                    format!("the game announced a non-ws:// address: {}", child.url),
                )
                .with_help("the game must serve the wire protocol on a `ws://` address")
            );
            drop(child);
            return ExitCode::FAILURE;
        }
    };

    let src = project.join("src");
    let (changes, watch_handle) = match ludion_assets::watch(&src, WATCH_DEBOUNCE) {
        Ok(pair) => pair,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            drop(child);
            return ExitCode::FAILURE;
        }
    };
    emit(&format!("dev watching {}", src.display()));

    // A `scenes/` watch is independent of the `src/` one (point 4 in the module docs): it never
    // rebuilds, only hints. A game without a `scenes/` directory (or one whose watch fails to start)
    // gets `never()` — a receiver that simply never fires, so the supervise loop's select just never
    // takes that arm. A failed scenes watch is reported but does not end the session: `src` reload is
    // the load-bearing capability, the hint is a nicety.
    let mut watches = DevWatches::none();
    watches.dev_shaders = flags.dev_shaders;
    watches.rebake_probes = flags.rebake_probes;
    watches.anti_aliasing = flags.anti_aliasing;
    let scenes = project.join("scenes");
    if scenes.is_dir() {
        match ludion_assets::watch(&scenes, WATCH_DEBOUNCE) {
            Ok((rx, handle)) => {
                watches.scene_changes = rx;
                watches._scenes_handle = Some(handle);
            }
            Err(diagnostic) => eprintln!("{diagnostic}"),
        }
    }

    // The engine shader watch — the WGSL fast path. Present only inside the engine workspace; a
    // failed watch degrades to no fast path (rebuild still works), never a dead session.
    if let Some(dir) = &shader_dir {
        match ludion_assets::watch(dir, WATCH_DEBOUNCE) {
            Ok((rx, handle)) => {
                watches.shader_changes = rx;
                watches._shaders_handle = Some(handle);
                emit(&format!("dev watching shaders {}", dir.display()));
            }
            Err(diagnostic) => eprintln!("{diagnostic}"),
        }
    }

    if window {
        run_windowed(
            project,
            concrete_addr,
            scene.map(Path::to_path_buf),
            child,
            changes,
            watches,
            watch_handle,
        )
    } else {
        run_headless(
            &project,
            &concrete_addr,
            scene,
            child,
            changes,
            watches,
            watch_handle,
        )
    }
}

/// The `dev` supervisor's secondary watches, bundled into one value so they cost a single parameter
/// at each call site — `run_headless`/`run_windowed`/`supervise_loop` already sit at clippy's
/// too-many-arguments ceiling with just the `src/` watch's two pieces:
///
/// - the `scenes/` watch (a hint-only trigger — never rebuilds, never restores);
/// - the **engine shader watch** (the WGSL fast path): when `dev` runs from inside the engine
///   workspace, its `crates/ludion-renderer/src/shaders/` directory is watched, and a `.wgsl` edit
///   sends an `asset.reload` over the wire instead of rebuild→snapshot→restart. `dev_shaders` is true
///   exactly then, and the children launch with `--dev-shaders` so they honor the reload.
struct DevWatches {
    scene_changes: Receiver<Vec<String>>,
    _scenes_handle: Option<ludion_assets::WatchHandle>,
    /// Changed engine-shader paths (absolute), or a never-firing receiver when there is no engine
    /// shader dir to watch (a standalone game, or a failed watch).
    shader_changes: Receiver<Vec<String>>,
    _shaders_handle: Option<ludion_assets::WatchHandle>,
    /// Whether launched children get `--dev-shaders` — true iff an engine shader dir was found, so the
    /// WGSL fast path can actually round-trip a reload. A standalone game leaves this `false` and
    /// launches its children without `--dev-shaders` and watches no shader directory.
    dev_shaders: bool,
    /// Whether launched children get `--probes rebake`.
    rebake_probes: bool,
    /// The `--aa` mode launched children get, if dev was given one.
    anti_aliasing: Option<AntiAliasing>,
}

impl DevWatches {
    /// No secondary watches: both receivers never fire, no handles, no dev-shader children.
    fn none() -> DevWatches {
        DevWatches {
            scene_changes: crossbeam_channel::never(),
            _scenes_handle: None,
            shader_changes: crossbeam_channel::never(),
            _shaders_handle: None,
            dev_shaders: false,
            rebake_probes: false,
            anti_aliasing: None,
        }
    }
}

/// The flags dev decides for every child it launches, beyond the address, scene and restore path.
#[derive(Clone, Copy)]
struct ChildFlags {
    /// The child honors `asset.reload` on a renderer `.wgsl` so dev's WGSL fast path can round-trip
    /// an edit over the wire. Only set when an engine shader dir was found to watch.
    dev_shaders: bool,
    /// The child captures its reflection probes afresh instead of reading the probe store.
    rebake_probes: bool,
    /// The child hosts with this anti-aliasing mode; absent leaves its builder default.
    anti_aliasing: Option<AntiAliasing>,
}

impl ChildFlags {
    fn args(self) -> Vec<&'static str> {
        let mut args = Vec::new();
        if self.dev_shaders {
            args.push("--dev-shaders");
        }
        if self.rebake_probes {
            args.extend(["--probes", "rebake"]);
        }
        if let Some(mode) = self.anti_aliasing {
            args.extend(["--aa", mode.name()]);
        }
        args
    }
}

/// The engine's renderer-shader directory, if `dev` is running from inside the engine workspace:
/// walks up from `project` for a `crates/ludion-renderer/src/shaders` directory. `Some` marks the
/// engine-developer case — a WGSL edit there hot-reloads over the wire. `None` (a standalone
/// `ludion new` game, or the engine installed as a dependency) means no shaders are on disk to
/// watch, and the WGSL fast path is simply absent.
fn find_engine_shaders(project: &Path) -> Option<PathBuf> {
    let suffix = Path::new("crates")
        .join("ludion-renderer")
        .join("src")
        .join("shaders");
    project.ancestors().find_map(|dir| {
        let candidate = dir.join(&suffix);
        candidate.is_dir().then_some(candidate)
    })
}

/// Headless dev (no `--window`): supervise on the main thread until Ctrl-C.
fn run_headless(
    project: &Path,
    addr: &str,
    scene: Option<&Path>,
    mut child: GameChild,
    changes: Receiver<Vec<String>>,
    mut watches: DevWatches,
    _watch_handle: ludion_assets::WatchHandle,
) -> ExitCode {
    let stop = install_ctrl_c();
    supervise_loop(
        project,
        addr,
        scene,
        &mut child,
        &changes,
        &mut watches,
        &stop,
    );
    // `child` owns the last good process, or is a placeholder after a failed boot (a no-op drop).
    // `_watch_handle`/`watches` stay in scope until here so every watch lives as long as the loop.
    drop(child);
    ExitCode::SUCCESS
}

/// Windowed dev (`--window`): host one persistent window on the main thread while the supervision
/// loop runs on a worker thread. The window connects to the first child's concrete `ws://` URL and,
/// because every reload rebinds that same URL, rides each reload's blink via `ludion-window`'s
/// reconnect. The blink is shutdown → boot → restore (seconds); a slow rebuild does not lengthen it
/// because the *old* child keeps serving until the build has succeeded and its snapshot is on disk.
/// A child that cannot boot leaves the window showing "— reconnecting…" until the next good save
/// relaunches it; the window closes when dev stops (the supervisor ending closes it). The snapshot
/// stays on disk.
fn run_windowed(
    project: PathBuf,
    addr: String,
    scene: Option<PathBuf>,
    child: GameChild,
    changes: Receiver<Vec<String>>,
    watches: DevWatches,
    watch_handle: ludion_assets::WatchHandle,
) -> ExitCode {
    let url = child.url.clone();
    let title = window_title(&project);

    // Two stop signals. `window_stop` closes the window; `supervise_stop` ends the supervision loop.
    // Ctrl-C fires both (fanout handler). A window *close* fires `supervise_stop` after the fact
    // (below).
    let (supervise_stop_tx, supervise_stop_rx) = bounded::<()>(1);
    let (window_stop_tx, window_stop_rx) = bounded::<()>(1);
    ludion::ctrl_c_fan_out(vec![supervise_stop_tx.clone(), window_stop_tx.clone()]);

    // Supervision moves to a worker thread so the main thread can host winit (its event loop must
    // live on the main thread). The worker owns the child + both watchers and prints the same status
    // lines.
    let close_window = CloseOnDrop(window_stop_tx.clone());
    let supervisor = std::thread::spawn(move || {
        // Dropped last, on any exit including a panic: the window follows the supervisor down. An
        // explicit send, because the Ctrl-C fan-out holds the other senders for the process lifetime.
        let _close_window = close_window;
        let mut child = child;
        let mut watches = watches;
        supervise_loop(
            &project,
            &addr,
            scene.as_deref(),
            &mut child,
            &changes,
            &mut watches,
            &supervise_stop_rx,
        );
        drop(child); // a placeholder after a failed boot: a no-op
        drop(watch_handle); // keep every watch alive exactly as long as the loop
        drop(watches);
    });

    let options = ludion_window::WindowOptions {
        url,
        title,
        width: 1280,
        height: 720,
    };
    let window_result = ludion_window::run(options, window_stop_rx);

    // The window has closed (user close, Ctrl-C, or a lost connection). Ensure the supervision loop
    // stops too — on a user close it has not been signaled yet — then join it.
    let _ = supervise_stop_tx.try_send(());
    let supervised = supervisor.join();

    match window_result {
        Ok(()) if supervised.is_ok() => ExitCode::SUCCESS,
        Ok(()) => ExitCode::FAILURE,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            ExitCode::FAILURE
        }
    }
}

/// Closes the viewer window when dropped, so the supervisor thread ending for any reason ends it.
struct CloseOnDrop(Sender<()>);

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
}

/// The supervise loop, shared by both modes: a source-change batch drives a reload; a scenes-change
/// batch only prints the hint (point 4 in the module docs); a `stop` signal (or the src watcher thread
/// ending) stops it. No reload ends it: a rebuilt child that cannot boot waits for the next save.
///
/// `scene_changes` is `&mut` (unlike `changes`) so that if the `scenes/` watcher itself disconnects —
/// distinct from `never()`, which by design never fires this select arm at all — it can be swapped for
/// `never()` in place: a one-time reassignment stops `select!` from re-entering the disconnected arm
/// every iteration, rather than busy-spinning on a channel that will always report `Err` instantly.
fn supervise_loop(
    project: &Path,
    addr: &str,
    scene: Option<&Path>,
    child: &mut GameChild,
    changes: &Receiver<Vec<String>>,
    watches: &mut DevWatches,
    stop: &Receiver<()>,
) {
    let flags = ChildFlags {
        dev_shaders: watches.dev_shaders,
        rebake_probes: watches.rebake_probes,
        anti_aliasing: watches.anti_aliasing,
    };
    // A snapshot is kept on disk, waiting for a launch to restore it (see `reload`): the file exists
    // exactly while one is kept, so a resume that failed at startup arrives here with it set.
    let mut kept = kept_snapshot_path(project).is_file();
    // The `src/` fingerprint the last reload started from.
    let mut last_built: Option<SrcFingerprint> = None;
    loop {
        crossbeam_channel::select! {
            recv(stop) -> _ => return,
            recv(changes) -> batch => {
                if batch.is_err() {
                    // The watcher thread ended unexpectedly; stop cleanly rather than spin.
                    return;
                }
                // Settle every edit currently on disk, then keep reloading while the source keeps
                // changing *under the build*. A save can land after cargo has already read the files
                // (or coalesce into the batch that is already reloading); the reload that just finished
                // would then be serving a binary that predates that save, with no further batch to
                // trigger a retry. `reload` reports (via its own post-build mtime check) whether that
                // happened, and the loop reloads again — so a save during an in-flight rebuild is never
                // silently lost.
                //
                // A batch can also arrive for a save an earlier reload already built (one save
                // reaching the watcher as two batches more than the debounce apart, the second
                // landing during a long reload): with `src/` unchanged since the last build there is
                // nothing to rebuild, and a rebuild would replace the running world for nothing.
                loop {
                    // Coalesce any batches that piled up (during a previous build, or during the
                    // reload we are about to repeat) into this one reload.
                    while changes.try_recv().is_ok() {}
                    if let Some(now) = src_fingerprint(project)
                        && last_built.as_ref() == Some(&now)
                    {
                        break;
                    }
                    match reload(project, addr, scene, child, flags, &mut kept) {
                        ReloadOutcome::Continued {
                            source_changed_during_build,
                            built_from,
                        } => {
                            last_built = built_from;
                            if !source_changed_during_build {
                                break;
                            }
                        }
                    }
                }
            }
            recv(watches.scene_changes) -> batch => {
                match batch {
                    Ok(_) => {
                        // Coalesce the same way the src arm does, then hint — never rebuild, never
                        // restore: the running world is preserved by design.
                        while watches.scene_changes.try_recv().is_ok() {}
                        emit(LINE_SCENES_CHANGED);
                    }
                    Err(_) => watches.scene_changes = crossbeam_channel::never(),
                }
            }
            recv(watches.shader_changes) -> batch => {
                match batch {
                    Ok(mut paths) => {
                        // Coalesce piled-up shader batches into this one, then hot-reload each changed
                        // `.wgsl` over the wire — the WGSL fast path: no rebuild, no snapshot, no
                        // restart, so the edit → pixels round-trip is sub-second (well under the ~3.1 s
                        // code-reload path). The running world is untouched; only render-side shaders
                        // change (they are not sim state).
                        while let Ok(more) = watches.shader_changes.try_recv() {
                            paths.extend(more);
                        }
                        // After a failed boot there is no child to reload into.
                        if child.process.is_some() {
                            reload_shaders_over_wire(&child.url, &paths);
                        }
                    }
                    Err(_) => watches.shader_changes = crossbeam_channel::never(),
                }
            }
        }
    }
}

/// Hot-reloads each changed `.wgsl` in `paths` over the wire to the running child: one
/// `asset.reload` per shader file, on a fresh connection, timing the watch → reload-ack round-trip
/// (the fast path's edit→pixels latency less the child's next-frame render, which follows on the very
/// next egress pass because the reload trips `frame_dirty`). A non-`.wgsl` path is ignored; a failed
/// reload (a bad edit — the child kept its last good pipeline) prints the diagnostic and the session
/// keeps serving. Latency is a wall-clock measurement, legitimate here (dev is the CLI supervisor).
fn reload_shaders_over_wire(url: &str, paths: &[String]) {
    let shaders: Vec<&String> = paths.iter().filter(|p| p.ends_with(".wgsl")).collect();
    if shaders.is_empty() {
        return;
    }
    let mut client = match Client::connect(url) {
        Ok(client) => client.with_call_timeout(SHADER_RELOAD_CALL_TIMEOUT),
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return;
        }
    };
    for path in shaders {
        let watch = Stopwatch::start();
        let name = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        match client.call(WireCommand::AssetReload(AssetReloadParams {
            path: Some(path.clone()),
        })) {
            Ok(_) => emit(&format!(
                "dev shader reloaded {name} in {} ms",
                watch.elapsed_ms()
            )),
            // A rejected reload (bad WGSL, a binding-shape change, or a file the renderer compiles
            // once) — the child kept its last good pipeline (`L2009`), so report and carry on. A
            // disabled host would be `L2008`, but `dev` launches its children with `--dev-shaders`,
            // so that never happens here.
            Err(diagnostic) => eprintln!("{diagnostic}"),
        }
    }
}

/// The persistent window's title: `Ludion — <project> (dev)`. The `(dev)` marks it as the
/// dev-hosted window that survives reloads (distinct from a plain `serve --window`).
fn window_title(project: &Path) -> String {
    let name = project
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "game".to_string());
    format!("Ludion — {name} (dev)")
}

/// The result of one reload attempt: no reload ends the loop. A build failure leaves the old child up;
/// a rebuilt child that cannot boot or restore leaves a kept snapshot for the next save.
enum ReloadOutcome {
    /// `source_changed_during_build` is `true` when a save landed while this reload was building —
    /// the just-built binary may predate that save, so the loop reloads again. `built_from` is the
    /// `src/` fingerprint the reload started from, which a later watcher batch is compared against.
    Continued {
        source_changed_during_build: bool,
        built_from: Option<SrcFingerprint>,
    },
}

/// One reload: rebuild, and on success snapshot → keep it on disk → shutdown → relaunch on the same
/// port → restore.
///
/// `child` is the currently-serving child; on a successful reload it is replaced with the new one. A
/// build failure leaves it untouched (the designer keeps playing). A rebuilt child that cannot boot
/// leaves a placeholder and a kept snapshot; `kept` is the supervisor's memory that such a snapshot
/// waits to be restored by the next reload.
fn reload(
    project: &Path,
    addr: &str,
    scene: Option<&Path>,
    child: &mut GameChild,
    flags: ChildFlags,
    kept: &mut bool,
) -> ReloadOutcome {
    let watch = Stopwatch::start();

    // Sampled before the build: an edit whose mtime advances past this landed *during* the
    // build/restore, so the binary this reload produces may predate it and the caller reloads again.
    let source_stamp = src_fingerprint(project);

    emit(LINE_BUILDING);
    status_building();

    let build_watch = Stopwatch::start();
    match crate::sync::ensure_fresh(project, &crate::cook::CookRequest::default()) {
        Ok(summary) => report_sync(&summary),
        Err(diagnostics) => {
            for diagnostic in &diagnostics {
                eprintln!("{diagnostic}");
            }
            emit(LINE_BUILD_FAILED);
            status_failed(
                build_watch.elapsed_ms(),
                status_from_diagnostics(&diagnostics),
                None,
            );
            return continued_after(project, source_stamp);
        }
    }
    let exe = match cargo_build(project) {
        Ok(exe) => exe,
        Err(diagnostics) => {
            emit(LINE_BUILD_FAILED);
            status_failed(build_watch.elapsed_ms(), diagnostics, None);
            return continued_after(project, source_stamp);
        }
    };
    let build_ms = build_watch.elapsed_ms();
    let continued = || continued_after(project, source_stamp.clone());
    let ctx = Relaunch {
        project,
        addr,
        scene,
        exe: &exe,
        flags,
        build_ms,
        watch: &watch,
        origin: RestoreOrigin::Reload,
    };
    let kept_path = kept_snapshot_path(project);
    // Deleting the file is how a designer drops a kept snapshot.
    if *kept && !kept_path.is_file() {
        *kept = false;
    }

    // A snapshot kept by an earlier failed boot or restore wins over whatever world is up now: the
    // child standing in is a fresh world (or nothing), and snapshotting it would overwrite the
    // designer's state.
    if *kept {
        drop(std::mem::replace(child, GameChild::placeholder()));
        match relaunch_and_restore(&ctx, kept) {
            Ok(fresh) => *child = fresh,
            Err(diagnostic) => report_launch_failed(&diagnostic, build_ms, Some(&kept_path)),
        }
        return continued();
    }

    // Nothing is serving and nothing is kept (an earlier boot failed before any snapshot was taken):
    // there is nothing to restore, so start the new binary as a first start does.
    if child.process.is_none() {
        match launch(project, addr, scene, None, &exe, flags) {
            Ok(fresh) => {
                *child = fresh;
                status_ok(Some(build_ms), None, None, None);
            }
            Err(diagnostic) => report_launch_failed(&diagnostic, build_ms, None),
        }
        return continued();
    }

    let (client, mut snapshot) = match take_snapshot(&child.url) {
        Ok(taken) => taken,
        Err(diagnostic) => {
            let diagnostic = explain_version_skew(diagnostic);
            eprintln!("{diagnostic}");
            // Either way the designer's state cannot be had, and keeping a child that did not answer
            // would leave the edit never running: stop it and start the new build on a fresh world.
            let child_exited = child
                .process
                .as_mut()
                .is_none_or(|process| matches!(process.try_wait(), Ok(Some(_))));
            let (line, message) = match snapshot_failure(child_exited) {
                SnapshotFailure::ChildExited => (
                    LINE_CHILD_CRASHED.to_string(),
                    format!("the old child had exited, so its state was lost: {diagnostic}"),
                ),
                SnapshotFailure::ChildAlive => (
                    snapshot_failed_line(&diagnostic),
                    format!("the old child did not answer engine.snapshot: {diagnostic}"),
                ),
            };
            emit(&line);
            status_crashed(message);
            // Dropping stops the old child: stdin EOF, `SHUTDOWN_GRACE`, then a kill.
            drop(std::mem::replace(child, GameChild::placeholder()));
            match launch(project, addr, scene, None, &exe, flags) {
                Ok(fresh) => {
                    *child = fresh;
                    status_ok(
                        Some(build_ms),
                        None,
                        Some("the session state was lost: the old child could not be snapshotted, so the new build runs a fresh world".to_string()),
                        None,
                    );
                }
                Err(diagnostic) => report_launch_failed(&diagnostic, build_ms, None),
            }
            return continued();
        }
    };

    // The snapshot is on disk before the old child is told to stop: whatever happens to the next
    // launch, the designer's state survives it. If it cannot be kept, the old child keeps serving.
    prepare_for_reload(&mut snapshot);
    if let Err(error) = write_snapshot(&kept_path, &snapshot) {
        let diagnostic = Diagnostic::new(
            code::DEV_DEGRADED,
            format!(
                "could not keep the snapshot at {}: {error}",
                kept_path.display()
            ),
        )
        .with_help("the reload was abandoned so the running world is not lost; fix the path and save again");
        eprintln!("{diagnostic}");
        emit(&format!(
            "dev reload aborted; could not keep the snapshot at {}, the old child is still serving",
            kept_path.display()
        ));
        status_failed(build_ms, status_from_diagnostics(&[diagnostic]), None);
        return continued();
    }
    *kept = true;
    request_shutdown(client);

    // Take ownership of the old child to stop it (wait for exit, kill on timeout, delete its copy),
    // then launch the freshly built binary on the same port, booting from the file just written.
    drop(std::mem::replace(child, GameChild::placeholder()));
    match relaunch_and_restore(&ctx, kept) {
        Ok(fresh) => *child = fresh,
        Err(diagnostic) => report_launch_failed(&diagnostic, build_ms, Some(&kept_path)),
    }
    continued()
}

/// What [`relaunch_and_restore`] needs from the reload that called it.
struct Relaunch<'a> {
    project: &'a Path,
    addr: &'a str,
    scene: Option<&'a Path>,
    exe: &'a Path,
    flags: ChildFlags,
    /// The build's duration, for the status document.
    build_ms: u128,
    /// The reload's own stopwatch, for the `restored` line's source-change → restored time.
    watch: &'a Stopwatch,
    origin: RestoreOrigin,
}

/// Why a child boots from the kept snapshot: a reload relaunching after an edit, or a fresh `ludion dev`
/// start finding the file a previous session left.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RestoreOrigin {
    Reload,
    Resume,
}

/// Launches the new binary booting from the kept snapshot file (`--restore`): the child restores before
/// it binds and reports the outcome on stdout. A successful restore clears `kept` and deletes the file —
/// the file exists exactly while a snapshot is kept. A failed one leaves both — the file is the retry —
/// and the child, which fell back to the scene, serves a fresh world.
///
/// # Errors
/// The `L0013` of a child that could not boot; nothing is restored then, and `kept` is untouched.
fn relaunch_and_restore(ctx: &Relaunch<'_>, kept: &mut bool) -> Result<GameChild, Diagnostic> {
    let path = kept_snapshot_path(ctx.project);
    let fresh = launch(
        ctx.project,
        ctx.addr,
        ctx.scene,
        Some(&path),
        ctx.exe,
        ctx.flags,
    )?;
    match parse_boot_restore(fresh.boot_restore.as_deref()) {
        Ok((frame, report)) => {
            print_report(&report);
            let message = match ctx.origin {
                RestoreOrigin::Reload => {
                    emit(&format!(
                        "dev reload restored frame={frame} in {} ms",
                        ctx.watch.elapsed_ms()
                    ));
                    None
                }
                RestoreOrigin::Resume => {
                    emit(&format!(
                        "dev resumed frame={frame} from the kept snapshot {} in {} ms",
                        path.display(),
                        ctx.watch.elapsed_ms()
                    ));
                    Some(format!(
                        "resumed from the kept snapshot {}; delete that file before starting dev to start fresh",
                        path.display()
                    ))
                }
            };
            status_ok(
                Some(ctx.build_ms),
                Some(frame),
                message,
                (!report.is_clean()).then_some(report),
            );
            *kept = false;
            discard_kept_snapshot(&path);
        }
        Err(diagnostic) => {
            // The new child is up but was not restored; keep it as the current child so the next edit
            // can try again. The world it serves is fresh, not the designer's, so say so loudly. The
            // snapshot stays on disk and the next save retries it — which discards this fresh world —
            // unless the file is deleted.
            eprintln!("{diagnostic}");
            let failed = match ctx.origin {
                RestoreOrigin::Reload => "dev reload restore failed",
                RestoreOrigin::Resume => "dev resume failed",
            };
            emit(&format!(
                "{failed}; running a fresh world, snapshot kept at {} (the next save retries it; delete the file to drop it)",
                path.display()
            ));
            status_restore_failed(
                ctx.build_ms,
                format!(
                    "{diagnostic}\nthe snapshot is kept at {}; the next save retries it and discards \
                     this fresh world, delete the file to drop it",
                    path.display()
                ),
                path.display().to_string(),
            );
        }
    }
    Ok(fresh)
}

/// Deletes the kept snapshot once it has been restored, so a later `ludion dev` start does not resume
/// a state the designer has since moved past. A file that cannot be deleted is warned about, not fatal.
fn discard_kept_snapshot(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => eprintln!(
            "{}",
            Diagnostic::new(
                code::DEV_DEGRADED,
                format!(
                    "could not delete the restored snapshot {}: {error}",
                    path.display()
                ),
            )
            .with_help(
                "a later `ludion dev` start would resume it; delete it by hand to start fresh"
            )
        ),
    }
}

/// What a failed reload launch tells the designer to do: dev is still running and the next save
/// launches again, restoring the kept snapshot when one waits.
fn reload_retry_help(kept: bool) -> String {
    let restore = if kept {
        " and restores the kept snapshot"
    } else {
        ""
    };
    format!("fix it and save again: dev retries the launch on the next save{restore}")
}

/// Reports a reload whose rebuilt child could not boot: dev keeps running, the old child is already
/// gone, and the next save launches again (restoring `kept` when a snapshot waits).
fn report_launch_failed(diagnostic: &Diagnostic, build_ms: u128, kept: Option<&Path>) {
    let diagnostic = &diagnostic
        .clone()
        .append_help(reload_retry_help(kept.is_some()));
    eprintln!("{diagnostic}");
    match kept {
        Some(path) => emit(&format!(
            "dev reload launch failed; snapshot kept at {}, retrying on the next save",
            path.display()
        )),
        None => emit("dev reload launch failed; retrying on the next save"),
    }
    status_failed(
        build_ms,
        status_from_diagnostics(std::slice::from_ref(diagnostic)),
        kept.map(|path| path.display().to_string()),
    );
}

/// Why `engine.snapshot` failed to deliver the world. Both relaunch a fresh child from the just-built
/// exe (unpaused, unrestored); they differ in what dev tells the designer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapshotFailure {
    /// The child has already exited on its own (a crash): there is no state left to snapshot.
    ChildExited,
    /// The child is alive but did not deliver — a sim that never returns (`L3054` on the call
    /// timeout), a schema skew (`L3003`), any refusal. Keeping it would mean the edit never runs.
    ChildAlive,
}

/// Classifies a snapshot failure, given whether the child process has already exited on its own (a
/// non-blocking `try_wait`, checked by the caller). Split out as its own function — pure and
/// independent of `std::process::Child` — so it is unit-testable without spawning a child.
fn snapshot_failure(child_exited: bool) -> SnapshotFailure {
    if child_exited {
        SnapshotFailure::ChildExited
    } else {
        SnapshotFailure::ChildAlive
    }
}

/// The stdout line for a snapshot a live child did not deliver: the cause's code and first line, and
/// the loss. One line, so a client parsing dev's stdout sees one event.
fn snapshot_failed_line(diagnostic: &Diagnostic) -> String {
    let cause = diagnostic.message.lines().next().unwrap_or_default();
    format!(
        "dev reload snapshot failed ({}: {cause}); restarting fresh, session state lost",
        diagnostic.code
    )
}

/// Replaces the bare `L3003` a CLI built from another engine revision gets from `Client::connect`
/// with a message that names the skew and its fix; any other diagnostic passes through unchanged.
fn explain_version_skew(diagnostic: Diagnostic) -> Diagnostic {
    if diagnostic.code != code::PROTOCOL_UNSUPPORTED_VERSION {
        return diagnostic;
    }
    Diagnostic::new(
        diagnostic.code,
        format!(
            "this `ludion` CLI and the game were built from different engine revisions: {}",
            diagnostic.message
        ),
    )
    .with_help(
        "dev cannot snapshot a game whose wire schema differs, so every reload restarts a fresh \
         world; install the `ludion` CLI from the engine revision the game depends on (inside the \
         engine workspace, `cargo build -p ludion-cli`)",
    )
}

/// Warns, right after the first launch, when the CLI cannot speak to the game it just started (the
/// `L3003` skew): every reload would otherwise lose the session without saying why. Any other
/// outcome is ignored — dev keeps running either way.
fn warn_on_version_skew(url: &str) {
    if let Err(diagnostic) = Client::connect(url)
        && diagnostic.code == code::PROTOCOL_UNSUPPORTED_VERSION
    {
        eprintln!("{}", explain_version_skew(diagnostic));
    }
}

/// What a build read of the game's `src/` tree: the regular-file count, the newest modification time
/// and the total size. Any edit, added file or deleted file changes it, so comparing two of them says
/// whether the sources differ without reading them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SrcFingerprint {
    files: usize,
    newest: Option<SystemTime>,
    bytes: u64,
}

/// The [`SrcFingerprint`] of `<project>/src`, or `None` if the tree cannot be read or is empty. `src/`
/// is the only directory whose changes trigger a rebuild.
fn src_fingerprint(project: &Path) -> Option<SrcFingerprint> {
    fn walk(dir: &Path, fingerprint: &mut SrcFingerprint) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            match entry.file_type() {
                Ok(ft) if ft.is_dir() => walk(&entry.path(), fingerprint),
                Ok(_) => {
                    let Ok(metadata) = entry.metadata() else {
                        continue;
                    };
                    fingerprint.files += 1;
                    fingerprint.bytes += metadata.len();
                    if let Ok(mtime) = metadata.modified()
                        && fingerprint.newest.is_none_or(|n| mtime > n)
                    {
                        fingerprint.newest = Some(mtime);
                    }
                }
                Err(_) => {}
            }
        }
    }
    let mut fingerprint = SrcFingerprint {
        files: 0,
        newest: None,
        bytes: 0,
    };
    walk(&project.join("src"), &mut fingerprint);
    (fingerprint.files > 0).then_some(fingerprint)
}

/// Whether `src/` differs from `stamp` — a save landed during the build/restore. The reload that just
/// finished may be serving a binary that predates that save, so the caller reloads again: a save
/// during an in-flight rebuild is never silently lost. A missing `stamp` (unreadable tree at build
/// start) is treated as "changed" so a now-readable tree is picked up; if the tree is unreadable now
/// nothing can be built from it.
fn src_changed_since(project: &Path, stamp: Option<&SrcFingerprint>) -> bool {
    match (src_fingerprint(project), stamp) {
        (Some(now), Some(then)) => now != *then,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// The outcome of a reload that built from `built_from`: whether `src/` moved on since.
fn continued_after(project: &Path, built_from: Option<SrcFingerprint>) -> ReloadOutcome {
    ReloadOutcome::Continued {
        source_changed_during_build: src_changed_since(project, built_from.as_ref()),
        built_from,
    }
}

/// A running game child process plus what dev needs to talk to it and clean it up.
struct GameChild {
    process: Option<Child>,
    /// The `ws://host:port` URL the child announced it is listening on.
    url: String,
    /// The throwaway binary copy this child is running (deleted when the child is stopped).
    exe_copy: PathBuf,
    /// The relay threads draining the child's stdout/stderr — joined when the child is stopped so no
    /// thread outlives the process.
    stdout_thread: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<()>>,
    /// The write end of the child's stdin, held for the child's whole life: the `--supervised` child
    /// exits when it closes, and the OS closes it however dev dies. Never dropped early.
    stdin: Option<ChildStdin>,
    /// The restore outcome line a `--restore` child printed before it listened; `None` for a child
    /// that was not asked to restore.
    boot_restore: Option<String>,
}

impl GameChild {
    /// An inert stand-in used by `std::mem::replace` while ownership of the real child moves out — it
    /// owns no process, thread, or file, so stopping or dropping it is a no-op.
    fn placeholder() -> GameChild {
        GameChild {
            process: None,
            url: String::new(),
            exe_copy: PathBuf::new(),
            stdout_thread: None,
            stderr_thread: None,
            stdin: None,
            boot_restore: None,
        }
    }
}

/// Builds the game in `project` and returns the path to the built binary.
///
/// Before spawning cargo, the game package's fingerprints are purged (see [`purge_fingerprints`]):
/// dev only builds because a source changed, so cargo must actually recompile the package from the
/// bytes on disk — its mtime-based freshness guess is not trusted here, because it can wrongly
/// declare the package fresh (an edit landing within its granularity of the previous build, or a
/// shared target dir where an identically named package's fingerprint answers for this one) and
/// "succeed" with a binary that never saw the edit.
///
/// Uses `--message-format=json`: cargo's own progress (`Compiling …`/`Finished`) still goes to the
/// inherited stderr, while every diagnostic and artifact arrives as JSON on stdout. dev reads that
/// stream line by line — printing each rustc diagnostic's `rendered` string to its own stderr as it
/// arrives, so a human watching `ludion dev` sees exactly what plain cargo shows — while also
/// collecting the diagnostics structurally (for the status file) and the built executable's path (no
/// fragile guessing of the target directory or binary name).
///
/// # Errors
/// Returns the collected rustc diagnostics if the build failed (empty when cargo failed without a
/// parseable diagnostic — e.g. it could not be spawned, or produced no binary artifact); the human
/// rendering has already been printed. The caller records these on the `failed` status.
fn cargo_build(project: &Path) -> Result<PathBuf, Vec<StatusDiagnostic>> {
    if let Some(package) = package_name(project) {
        purge_fingerprints(&fingerprint_root(project), &package);
    }
    let mut process = match Command::new("cargo")
        .arg("build")
        .arg("--message-format=json")
        .current_dir(project)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(process) => process,
        Err(error) => {
            eprintln!(
                "{}",
                Diagnostic::new(
                    code::DEV_CARGO_FAILED,
                    format!("could not run `cargo build`: {error}"),
                )
                .with_help("check that `cargo` is on PATH (`cargo --version`)")
            );
            return Err(Vec::new());
        }
    };

    let mut executable = None;
    let mut diagnostics = Vec::new();
    if let Some(stdout) = process.stdout.take() {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(exe) = artifact_executable(&value) {
                executable = Some(exe);
            } else if value.get("reason").and_then(Value::as_str) == Some("compiler-message")
                && let Some(message) = value.get("message")
            {
                // Print rustc's own rendering live (cargo no longer does under plain `json`), then keep
                // the flat record for the status file.
                if let Some(rendered) = message.get("rendered").and_then(Value::as_str) {
                    eprint!("{rendered}");
                }
                if let Some(diagnostic) = diagnostic_from_message(message) {
                    diagnostics.push(diagnostic);
                }
            }
        }
    }
    let status = match process.wait() {
        Ok(status) => status,
        Err(error) => {
            eprintln!(
                "{}",
                Diagnostic::new(
                    code::DEV_CARGO_FAILED,
                    format!("`cargo build` could not be waited on: {error}"),
                )
                .with_help("check that `cargo` is on PATH (`cargo --version`)")
            );
            return Err(diagnostics);
        }
    };
    if !status.success() {
        return Err(diagnostics);
    }
    executable.ok_or(diagnostics)
}

/// The game package's name from its `Cargo.toml`, or `None` if the manifest cannot be read/parsed.
fn package_name(project: &Path) -> Option<String> {
    let text = std::fs::read_to_string(project.join("Cargo.toml")).ok()?;
    let manifest: toml::Value = text.parse().ok()?;
    Some(manifest.get("package")?.get("name")?.as_str()?.to_string())
}

/// Where cargo keeps this project's build fingerprints, laid out as
/// `<target-root>[/<triple>]/debug/.fingerprint`.
///
/// Three things move that path, and all three are followed:
/// - **`CARGO_TARGET_DIR`** relocates the root (relative values resolve against `project`, the cwd
///   of the `cargo build` spawns below — the way cargo resolves them).
/// - **The workspace and its config.** Cargo's `target/` sits at the *workspace* root, so a member
///   game builds into the engine's `target/`, not its own — and a detached game workspace can point
///   `build.target-dir` at that same directory from its `.cargo/config.toml`. `cargo metadata
///   --no-deps`'s `target_directory` is cargo's own answer to both.
/// - **`CARGO_BUILD_TARGET`** inserts the target triple a cross-compile builds under.
///
/// The profile segment is always `debug`: dev spawns a plain `cargo build` and never passes
/// `--profile`. A triple set only in `.cargo/config.toml` is the one remaining gap, and
/// [`purge_fingerprints`] reports a miss rather than degrading silently.
fn fingerprint_root(project: &Path) -> PathBuf {
    fingerprint_root_of(
        target_root(project),
        std::env::var_os("CARGO_BUILD_TARGET").as_deref(),
    )
}

/// [`fingerprint_root`]'s pure tail: the profile/triple layout under an already-resolved target root.
fn fingerprint_root_of(target_root: PathBuf, triple: Option<&OsStr>) -> PathBuf {
    let mut root = target_root;
    if let Some(triple) = triple.filter(|t| !t.is_empty()) {
        root.push(triple);
    }
    root.join("debug").join(".fingerprint")
}

/// The directory cargo builds `project` into, resolved once per process (it cannot change while dev
/// supervises one project).
fn target_root(project: &Path) -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
            let dir = PathBuf::from(dir);
            return if dir.is_absolute() {
                dir
            } else {
                project.join(dir)
            };
        }
        cargo_target_directory(project).unwrap_or_else(|| project.join("target"))
    })
    .clone()
}

/// The `target_directory` of `cargo metadata --no-deps` run in `project`, which honours the workspace
/// root and `.cargo/config.toml`'s `build.target-dir`. `None` if cargo cannot be run or its answer is
/// unreadable — the caller then assumes the package's own `target/`.
fn cargo_target_directory(project: &Path) -> Option<PathBuf> {
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(project)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let metadata: Value = serde_json::from_slice(&output.stdout).ok()?;
    metadata
        .get("target_directory")?
        .as_str()
        .map(PathBuf::from)
}

/// Deletes the cargo fingerprint directories of `package`'s units under `root`, forcing the next
/// `cargo build` to recompile the package from the actual bytes on disk (see [`cargo_build`]).
///
/// Cargo has no "force this package dirty" switch, and `cargo clean -p` also drops rustc's
/// incremental caches (slowing every reload); deleting only `.fingerprint/<unit>/` invalidates
/// cargo's freshness record while leaving incremental compilation intact. Only this package's dirs
/// are touched — the warm engine dependencies never rebuild.
///
/// The purge is best effort — a miss degrades to cargo's own freshness check — but it does not do so
/// silently: a `root` that exists yet holds no unit of `package` means [`fingerprint_root`] guessed a
/// layout cargo is not using (a target dir or `build.target` set in `.cargo/config.toml`, which no
/// env var reveals), so a one-time warning names the directory it looked in. A `root` that does not
/// exist at all is the ordinary cold-tree case and says nothing.
fn purge_fingerprints(root: &Path, package: &str) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut purged = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_unit_dir_for(&name, package) && std::fs::remove_dir_all(entry.path()).is_ok() {
            purged += 1;
        }
    }
    if purged == 0 {
        static WARNED: Once = Once::new();
        WARNED.call_once(|| {
            eprintln!(
                "{}",
                Diagnostic::new(
                    code::DEV_DEGRADED,
                    format!(
                        "no `{package}` fingerprints under `{}` — hot reload falls back to \
                         cargo's own freshness check, which can serve a binary that never saw \
                         your edit",
                        root.display()
                    ),
                )
                .with_help(
                    "if this project's target dir or `build.target` is set in \
                     `.cargo/config.toml`, export it as CARGO_TARGET_DIR / CARGO_BUILD_TARGET so \
                     `ludion dev` sees it too"
                )
            );
        });
    }
}

/// Whether fingerprint directory `dir` (shaped `<unit-name>-<hash>`) belongs to `package`. Unit
/// names use the package name for bins and the underscored crate name for libs, so both spellings
/// match; the suffix must look like cargo's metadata hash (hex) so a package named `foo` never
/// claims `foo-utils-…`.
fn is_unit_dir_for(dir: &str, package: &str) -> bool {
    let underscored = package.replace('-', "_");
    [package, underscored.as_str()].iter().any(|unit| {
        dir.strip_prefix(unit)
            .and_then(|rest| rest.strip_prefix('-'))
            .is_some_and(|hash| !hash.is_empty() && hash.bytes().all(|b| b.is_ascii_hexdigit()))
    })
}

/// The bin executable a single cargo message declares, or `None` if it is not a `compiler-artifact`
/// for a `bin` target that produced an `executable`. [`cargo_build`]'s streaming reader folds it over
/// every message and keeps the last hit (the "last bin artifact wins" rule).
fn artifact_executable(value: &Value) -> Option<PathBuf> {
    if value.get("reason").and_then(Value::as_str) != Some("compiler-artifact") {
        return None;
    }
    let is_bin = value
        .get("target")
        .and_then(|t| t.get("kind"))
        .and_then(Value::as_array)
        .is_some_and(|kinds| kinds.iter().any(|k| k.as_str() == Some("bin")));
    if !is_bin {
        return None;
    }
    value
        .get("executable")
        .and_then(Value::as_str)
        .map(PathBuf::from)
}

/// Launches the built binary as a game child on `addr`, retrying the bind window if it dies
/// immediately, and waits for it to announce `listening on …` (relayed verbatim to dev's stdout).
///
/// `restore` forwards `--restore <file>` (a reloaded child boots from the kept snapshot; the first
/// start loads the scene). The binary is copied to a throwaway path first so the canonical build
/// output is never locked — see the module docs.
///
/// # Errors
/// `L0013` if the child could not be launched, or died / hung before listening across the whole
/// bind-retry window, carrying the child's captured output.
fn launch(
    project: &Path,
    addr: &str,
    scene: Option<&Path>,
    restore: Option<&Path>,
    exe: &Path,
    flags: ChildFlags,
) -> Result<GameChild, Diagnostic> {
    let exe_copy = copy_binary(exe).map_err(|error| {
        Diagnostic::new(
            code::DEV_CHILD_FAILED,
            format!("could not copy the built binary to a runnable location: {error}"),
        )
    })?;

    let retry_deadline = Deadline::after(BIND_RETRY_WINDOW);
    let (last_reason, last_output) = loop {
        match try_launch_once(project, addr, scene, restore, &exe_copy, flags) {
            Ok(child) => return Ok(child),
            Err((reason, output)) => {
                let bind_race =
                    reason != ListenFailure::TimedOut && looks_like_bind_failure(&output);
                if !bind_race || retry_deadline.passed() {
                    break (reason, output);
                }
                std::thread::sleep(BIND_RETRY_DELAY);
            }
        }
    };

    remove_with_retry(&exe_copy);
    Err(child_failed(&last_output, last_reason))
}

/// One launch attempt: spawn the copy, relay its output, and wait for the listening line. On failure
/// the child is killed and reaped and why it failed plus its captured output returned (for a retry or
/// the final `L0013`).
fn try_launch_once(
    project: &Path,
    addr: &str,
    scene: Option<&Path>,
    restore: Option<&Path>,
    exe_copy: &Path,
    flags: ChildFlags,
) -> Result<GameChild, (ListenFailure, String)> {
    let mut command = Command::new(exe_copy);
    command
        .current_dir(project)
        .arg("--addr")
        .arg(addr)
        // The child exits when dev's end of this pipe closes — however dev dies (see `GameChild`).
        .arg("--supervised")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(scene) = scene {
        command.arg("--scene").arg(scene);
    }
    if let Some(restore) = restore {
        command.arg("--restore").arg(restore);
    }
    command.args(flags.args());

    let mut process = match command.spawn() {
        Ok(process) => process,
        Err(error) => {
            return Err((
                ListenFailure::SpawnFailed,
                format!("could not start the game binary: {error}"),
            ));
        }
    };

    let (listen_tx, listen_rx) = bounded::<String>(2);
    let stderr_tail = Arc::new(Mutex::new(StderrTail::default()));
    let stdin = process.stdin.take();
    let stdout_thread = spawn_stdout_relay(process.stdout.take(), listen_tx);
    let stderr_thread = spawn_stderr_relay(process.stderr.take(), stderr_tail.clone());

    match wait_for_listening(&mut process, &listen_rx) {
        Ok((url, boot_restore)) => {
            // Flush what the child printed while booting, then forward every later line live.
            if let Ok(mut tail) = stderr_tail.lock() {
                for line in tail.start_forwarding() {
                    eprintln!("[game] {line}");
                }
            }
            Ok(GameChild {
                process: Some(process),
                url,
                exe_copy: exe_copy.to_path_buf(),
                stdout_thread: Some(stdout_thread),
                stderr_thread: Some(stderr_thread),
                stdin,
                boot_restore,
            })
        }
        Err(reason) => {
            // Failed to come up: kill, reap, join the relays (so the pipes close and the buffer is
            // complete), then hand back whatever the child printed.
            let _ = process.kill();
            let _ = process.wait();
            let _ = stdout_thread.join();
            let _ = stderr_thread.join();
            let output = stderr_tail
                .lock()
                .map(|tail| tail.text())
                .unwrap_or_default();
            Err((reason, output))
        }
    }
}

/// Why a launched child never announced `listening on …`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListenFailure {
    /// The process could not be started.
    SpawnFailed,
    /// The process exited.
    Exited,
    /// Its stdout closed without a listening line.
    OutputClosed,
    /// It was still running after [`LISTEN_TIMEOUT`]; dev stops it.
    TimedOut,
}

/// Waits for the child to send its `listening on …` line (relaying it verbatim to dev's stdout) or to
/// fail. Returns the announced URL and the restore outcome line a `--restore` child printed before it
/// (`None` for a child that restored nothing), or why the child never listened.
fn wait_for_listening(
    process: &mut Child,
    listen_rx: &Receiver<String>,
) -> Result<(String, Option<String>), ListenFailure> {
    let deadline = Deadline::after(LISTEN_TIMEOUT);
    let mut boot_restore = None;
    loop {
        match listen_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(line) if is_boot_restore_line(&line) => boot_restore = Some(line),
            Ok(line) => {
                emit(&line); // the child's `listening on ws://…`, verbatim
                return parse_listening_url(&line)
                    .map(|url| (url.to_string(), boot_restore))
                    .ok_or(ListenFailure::OutputClosed);
            }
            Err(RecvTimeoutError::Timeout) => {
                let exited = matches!(process.try_wait(), Ok(Some(_)));
                if let Some(reason) = classify_silence(exited, deadline.passed()) {
                    return Err(reason);
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                // A child that dies at once (a bind failure) closes its stdout before `try_wait` sees
                // the exit; give the exit a moment so it is not misread as a closed pipe.
                return Err(classify_disconnect(wait_for_exit(
                    process,
                    Duration::from_secs(2),
                )));
            }
        }
    }
}

/// Why a child that printed nothing for a poll interval has failed, if it has: an exit outranks the
/// deadline.
fn classify_silence(exited: bool, deadline_passed: bool) -> Option<ListenFailure> {
    if exited {
        Some(ListenFailure::Exited)
    } else if deadline_passed {
        Some(ListenFailure::TimedOut)
    } else {
        None
    }
}

/// Why a child whose stdout closed without a listening line failed.
fn classify_disconnect(exited: bool) -> ListenFailure {
    if exited {
        ListenFailure::Exited
    } else {
        ListenFailure::OutputClosed
    }
}

/// Whether `line` is the outcome a `--restore` boot prints before `listening on …`.
fn is_boot_restore_line(line: &str) -> bool {
    line.starts_with("restored ") || line.starts_with("restore failed: ")
}

/// Parses the outcome line a `--restore` child printed: `restored frame=<N> report=<JSON>` is the
/// restored frame and the migration report, `restore failed: <JSON>` the child's diagnostic. A child
/// that printed neither (`None`, or any other text) is an `L0033`: dev cannot tell what world it serves.
fn parse_boot_restore(line: Option<&str>) -> Result<(u64, RestoreReport), Diagnostic> {
    let line = line.unwrap_or_default();
    let parsed = if let Some(rest) = line.strip_prefix("restored frame=") {
        rest.split_once(" report=").and_then(|(frame, report)| {
            Some(Ok((
                frame.parse().ok()?,
                serde_json::from_str(report).ok()?,
            )))
        })
    } else {
        line.strip_prefix("restore failed: ")
            .and_then(|json| serde_json::from_str::<Diagnostic>(json).ok())
            .map(Err)
    };
    parsed.unwrap_or_else(|| {
        Err(Diagnostic::new(
            code::DEV_DEGRADED,
            "the game did not report its restore",
        )
        .with_help("a game binary built from an older engine does not print the restore outcome; rebuild it"))
    })
}

/// Spawns a thread that reads the child's stdout and forwards only its `listening on …` line and the
/// restore outcome line before it (once each) to `listen_tx`. Every other line is discarded, so dev's
/// own stdout stays exactly its status lines plus the relayed listening line, while the child's pipe is
/// still drained (never stalls the child).
fn spawn_stdout_relay(
    stdout: Option<std::process::ChildStdout>,
    listen_tx: Sender<String>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let Some(stdout) = stdout else { return };
        let (mut sent_listening, mut sent_restore) = (false, false);
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if !sent_listening && parse_listening_url(&line).is_some() {
                let _ = listen_tx.send(line);
                sent_listening = true;
            } else if !sent_restore && !sent_listening && is_boot_restore_line(&line) {
                let _ = listen_tx.send(line);
                sent_restore = true;
            }
        }
    })
}

/// How many of a booting child's stderr lines dev keeps for a failed launch's `L0013` (oldest evicted).
const STDERR_TAIL_LINES: usize = 200;

/// A child's stderr as dev holds it: a bounded ring of the lines printed before the child announced
/// `listening on …` (what an `L0013` quotes when it never does), then, once it has, nothing — every
/// later line is forwarded to dev's stderr as it arrives.
#[derive(Default)]
struct StderrTail {
    lines: std::collections::VecDeque<String>,
    forwarding: bool,
}

impl StderrTail {
    /// Records one line, or returns it back when the tail is already forwarding.
    fn push(&mut self, line: String) -> Option<String> {
        if self.forwarding {
            return Some(line);
        }
        if self.lines.len() == STDERR_TAIL_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
        None
    }

    /// Switches to forwarding and hands back the lines buffered so far, oldest first.
    fn start_forwarding(&mut self) -> Vec<String> {
        self.forwarding = true;
        self.lines.drain(..).collect()
    }

    /// The buffered lines, one per line.
    fn text(&self) -> String {
        self.lines.iter().fold(String::new(), |mut text, line| {
            text.push_str(line);
            text.push('\n');
            text
        })
    }
}

/// Spawns a thread that reads the child's stderr line by line into `tail`: buffered until the child is
/// listening (so it is available, even partially, for an `L0013`), forwarded to dev's stderr with a
/// `[game]` prefix after. Printing happens under the tail's lock so the buffered lines flushed at the
/// flip stay ahead of every later one.
fn spawn_stderr_relay(
    stderr: Option<std::process::ChildStderr>,
    tail: Arc<Mutex<StderrTail>>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let Some(stderr) = stderr else { return };
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if let Ok(mut guard) = tail.lock()
                && let Some(line) = guard.push(line)
            {
                eprintln!("[game] {line}");
            }
        }
    })
}

/// Connects to the running child and captures an `engine.snapshot`. Returns the connection too, for
/// [`request_shutdown`] once the snapshot is safe.
///
/// # Errors
/// A transport or protocol diagnostic if the snapshot itself could not be taken.
fn take_snapshot(url: &str) -> Result<(Client, Value), Diagnostic> {
    let mut client = Client::connect(url)?.with_call_timeout(SNAPSHOT_CALL_TIMEOUT);
    let snapshot = match client.call(WireCommand::EngineSnapshot)?.body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => {
            return Err(ludion_client::unexpected_body(
                "engine.snapshot",
                "Snapshot",
                &other,
            ));
        }
    };
    Ok((client, snapshot))
}

/// Asks the child to stop gracefully. The reply may or may not arrive before the socket drops; dropping
/// the child afterwards waits for exit and hard-kills as a fallback, so the outcome is ignored.
fn request_shutdown(mut client: Client) {
    client.set_call_timeout(Some(SHUTDOWN_CALL_TIMEOUT));
    let _ = client.call(WireCommand::EngineShutdown);
}

/// Strips what must not cross a recompile from a snapshot, so the file is exactly what is restored.
///
/// - `input`: the old child's key-ups cannot reach it across a recompile, so a key held at snapshot
///   time and released during the rebuild would otherwise stay stuck in the new child. `engine.restore`
///   itself keeps carrying input for every other client; a document without `input` restores idle.
/// - `doc_reloads`: the session's live data-doc reloads. The new child just walked the disk, which is
///   newer than that log; replaying it would put stale (or no longer deserializable) docs over the files.
fn prepare_for_reload(snapshot: &mut Value) {
    if let Some(document) = snapshot.as_object_mut() {
        document.remove("input");
        document.remove("doc_reloads");
    }
}

/// Where the reload's snapshot is kept: written before the old child is stopped, restored from on the
/// next successful launch, and left in place after a failed restore.
fn kept_snapshot_path(project: &Path) -> PathBuf {
    project.join(".ludion").join("dev-snapshot.json")
}

/// Keeps the reload's snapshot at `path` (pretty-printed, atomically) so a failed restore can be
/// retried by the next launch.
fn write_snapshot(path: &Path, snapshot: &Value) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(snapshot)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    write_atomic(path, json)
}

/// Prints a non-empty restore migration report to stdout — the designer's honest migration story, one
/// indented line per group (see [`report_lines`]). An empty (clean) report prints nothing.
fn print_report(report: &RestoreReport) {
    for line in report_lines(report) {
        emit(&line);
    }
}

/// What a group of per-entity report entries share: the line they collapse into.
#[derive(PartialEq)]
enum ReportKey<'a> {
    Unregistered(&'a str),
    Undeserializable(&'a str),
    Migrated(&'a str, &'a [String], &'a [String]),
}

/// A group's first entity and entry count, plus the first entry's error line for the kinds that carry one.
struct Group<'a> {
    key: ReportKey<'a>,
    first: EntityId,
    count: usize,
    detail: Option<&'a str>,
}

/// The report as stdout lines: per-entity entries sharing a `(kind, component, fields)` key collapse
/// into one line with an entity count and the first entity, in first-appearance order, so a component
/// on thousands of entities is one line; the document-level entries are one line each. A
/// deserialize failure shows the first entry's error as an example. The full per-entity list rides
/// the `ok` status's `report`.
fn report_lines(report: &RestoreReport) -> Vec<String> {
    let mut groups: Vec<Group<'_>> = Vec::new();
    let mut lines = Vec::new();
    // `None` marks where a group's line goes, so group and document-level lines keep report order.
    let mut order: Vec<Option<usize>> = Vec::new();
    for entry in &report.entries {
        let (key, entity, detail) = match entry {
            RestoreEntry::ComponentUnregistered { entity, component } => {
                (ReportKey::Unregistered(component), *entity, None)
            }
            RestoreEntry::ComponentUndeserializable {
                entity,
                component,
                error,
            } => (
                ReportKey::Undeserializable(component),
                *entity,
                Some(error.lines().next().unwrap_or_default()),
            ),
            RestoreEntry::ComponentMigrated {
                entity,
                component,
                filled,
                removed,
            } => (
                ReportKey::Migrated(component, filled, removed),
                *entity,
                None,
            ),
            RestoreEntry::EngineVersionDiffers {
                snapshot_version,
                engine_version,
            } => {
                lines.push(format!(
                    "  [L3048] snapshot taken by engine {snapshot_version}, restored into {engine_version}"
                ));
                order.push(None);
                continue;
            }
            RestoreEntry::SnapshotFieldUnknown { field } => {
                lines.push(format!(
                    "  [L3049] snapshot field `{field}` is unknown to this engine, ignored"
                ));
                order.push(None);
                continue;
            }
        };
        match groups.iter_mut().position(|group| group.key == key) {
            Some(index) => groups[index].count += 1,
            None => {
                order.push(Some(groups.len()));
                groups.push(Group {
                    key,
                    first: entity,
                    count: 1,
                    detail,
                });
            }
        }
    }
    let mut singles = lines.into_iter();
    order
        .into_iter()
        .filter_map(|slot| match slot {
            None => singles.next(),
            Some(index) => Some(group_line(&groups[index])),
        })
        .collect()
}

/// One grouped report line: today's per-entity wording with the entity column replaced by the count
/// and the first entity.
fn group_line(group: &Group<'_>) -> String {
    let Group {
        key,
        first,
        count,
        detail,
    } = group;
    let on = format!(
        "{count} {}, first ({},{})",
        if *count == 1 { "entity" } else { "entities" },
        first.index(),
        first.generation(),
    );
    match key {
        ReportKey::Unregistered(component) => {
            format!("  [L3015] component `{component}` dropped (no longer registered) on {on}")
        }
        ReportKey::Undeserializable(component) => format!(
            "  [L3016] component `{component}` dropped (its value no longer deserializes) on {on}: {}",
            detail.unwrap_or_default()
        ),
        ReportKey::Migrated(component, filled, removed) => format!(
            "  component `{component}` migrated (filled: [{}], removed: [{}]) on {on}",
            filled.join(", "),
            removed.join(", "),
        ),
    }
}

/// Dropping a child stops it: close the supervision pipe (the `--supervised` child's graceful stop
/// signal), wait for it to exit, hard-kill on timeout, join the relay threads, and delete the
/// throwaway binary copy. A [`GameChild::placeholder`] is a no-op.
impl Drop for GameChild {
    fn drop(&mut self) {
        self.stdin.take();
        if let Some(mut process) = self.process.take()
            && !wait_for_exit(&mut process, SHUTDOWN_GRACE)
        {
            let _ = process.kill();
            let _ = process.wait();
        }
        if let Some(handle) = self.stdout_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr_thread.take() {
            let _ = handle.join();
        }
        if !self.exe_copy.as_os_str().is_empty() {
            remove_with_retry(&self.exe_copy);
        }
    }
}

/// Polls `process` for exit up to `grace`, returning whether it exited on its own within the window.
fn wait_for_exit(process: &mut Child, grace: Duration) -> bool {
    let deadline = Deadline::after(grace);
    loop {
        match process.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => {
                if deadline.passed() {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => return false,
        }
    }
}

/// Creates this process's session directory — where every child's binary copy lives — after sweeping
/// the directories dead sessions left behind. The session holds an exclusive lock on `<dir>/lock` for
/// as long as the process lives (the OS releases it however the process dies), which is how the next
/// sweep tells a live session from a dead one.
fn start_session() -> std::io::Result<()> {
    let temp = std::env::temp_dir();
    sweep_dead_sessions(&temp, SESSION_GRACE);
    let dir = temp.join(format!("{SESSION_DIR_PREFIX}{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let lock = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("lock"))?;
    lock.lock()?;
    let _ = SESSION_LOCK.set(lock);
    let _ = SESSION_DIR.set(dir);
    Ok(())
}

/// Removes every `ludion-dev-session-*` directory under `temp` whose owner is dead: its `lock` file
/// can be locked, or it has no `lock` at all. Anything touched less than `min_age` ago is left alone
/// — a session between creating its lock and locking it looks unlocked, and one mid-creation has no
/// lock yet — as is a directory whose age cannot be read and anything not matching the prefix. Best
/// effort: an undeletable directory is retried by the next session.
// Wall-clock by nature: a directory's age is file-system time, host housekeeping that never touches sim state.
#[allow(clippy::disallowed_methods)]
fn sweep_dead_sessions(temp: &Path, min_age: Duration) {
    let Ok(entries) = std::fs::read_dir(temp) else {
        return;
    };
    let settled = |path: &Path| {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age >= min_age)
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(SESSION_DIR_PREFIX)
        {
            continue;
        }
        let dir = entry.path();
        let lock_path = dir.join("lock");
        if !lock_path.exists() {
            if settled(&dir) {
                let _ = std::fs::remove_dir_all(&dir);
            }
            continue;
        }
        if !settled(&lock_path) {
            continue;
        }
        let Ok(lock) = std::fs::File::options()
            .read(true)
            .write(true)
            .open(&lock_path)
        else {
            continue;
        };
        if lock.try_lock().is_ok() {
            drop(lock);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// Copies the built binary into this session's directory under a unique name (see the module docs on
/// why the child never runs the canonical build output directly).
fn copy_binary(exe: &Path) -> std::io::Result<PathBuf> {
    let session = SESSION_DIR.get().ok_or_else(|| {
        std::io::Error::other("no dev session directory (copy_binary called outside `run`)")
    })?;
    let seq = COPY_SEQ.fetch_add(1, Ordering::Relaxed);
    let name = format!(
        "{seq}-{}",
        exe.file_name()
            .unwrap_or(OsStr::new("game"))
            .to_string_lossy()
    );
    let dest = session.join(name);
    std::fs::copy(exe, &dest)?;
    Ok(dest)
}

/// Deletes `path`, retrying a few times — on Windows the OS can hold a just-exited process's image
/// briefly, so the first `remove_file` can transiently fail with "access denied".
fn remove_with_retry(path: &Path) {
    for _ in 0..20 {
        match std::fs::remove_file(path) {
            Ok(()) => return,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) => std::thread::sleep(Duration::from_millis(25)),
        }
    }
}

/// Installs the facade's Ctrl-C handler and returns the receiver it signals once, so the supervise
/// loop can stop cleanly; a second press exits the process at once.
fn install_ctrl_c() -> Receiver<()> {
    let (tx, rx) = bounded::<()>(1);
    ludion::ctrl_c_fan_out(vec![tx]);
    rx
}

/// Verifies the current directory is a runnable game project. The honest check: a `Cargo.toml`
/// exists, and there is a binary target to run — either a `src/main.rs` (cargo's default bin) or an
/// explicit `[[bin]]` table in the manifest. This is intentionally lightweight (no `cargo metadata`
/// round-trip); it catches "ran from the wrong directory", which is the mistake `L0012` is for.
///
/// # Errors
/// `L0012` if the directory is not a runnable game project.
fn check_runnable_game(project: &Path) -> Result<(), Diagnostic> {
    let manifest = project.join("Cargo.toml");
    let manifest_text = std::fs::read_to_string(&manifest).map_err(|_| not_a_game(project))?;
    let has_bin =
        project.join("src").join("main.rs").is_file() || manifest_text.contains("[[bin]]");
    if has_bin {
        Ok(())
    } else {
        Err(not_a_game(project))
    }
}

/// Builds the `L0012` diagnostic for a directory that is not a runnable game project.
fn not_a_game(project: &Path) -> Diagnostic {
    Diagnostic::new(
        code::DEV_NOT_A_GAME,
        format!(
            "`{}` is not a runnable game project (no Cargo.toml with a binary target)",
            project.display()
        ),
    )
    .with_help(
        "run `ludion dev` from a game's project directory, or scaffold one with `ludion new`",
    )
}

/// Whether a failed boot's captured output looks like the same-port bind race — worth retrying within
/// [`BIND_RETRY_WINDOW`]. ludion-server's bind error says `could not bind`; an empty output is a silent
/// death, which may be the race too. Anything else (a scene diagnostic, a panic) will not clear itself.
fn looks_like_bind_failure(output: &str) -> bool {
    output.trim().is_empty() || output.contains("could not bind")
}

/// Builds the `L0013` diagnostic for a child that never came up, embedding its captured output.
/// `reason` words the message so a slow boot never reads as a crash; the help follows the cause: a bind
/// race usually clears on its own, a boot failure needs a fix, a timeout wants a look at the load.
fn child_failed(output: &str, reason: ListenFailure) -> Diagnostic {
    let what = match reason {
        ListenFailure::SpawnFailed => "the game process could not be started".to_string(),
        ListenFailure::Exited => "the game process exited before it started listening".to_string(),
        ListenFailure::OutputClosed => {
            "the game process closed its output before it started listening".to_string()
        }
        ListenFailure::TimedOut => format!(
            "the game did not print `listening on …` within {} s; dev stopped it (a slow boot on a loaded machine, or a hang)",
            LISTEN_TIMEOUT.as_secs()
        ),
    };
    let detail = if output.trim().is_empty() {
        format!("{what} (no output captured)")
    } else {
        format!("{what}; its output was:\n{}", output.trim_end())
    };
    let help = if reason == ListenFailure::TimedOut {
        "retry on a quieter machine; if it persists the game hangs while booting — run it directly \
         to see where, and `ludion explain L0013`"
    } else if looks_like_bind_failure(output) {
        "if this is a same-port bind race it usually clears on its own; a persistent failure is a \
         real crash — run `ludion explain L0013`"
    } else {
        "the game failed while booting; its output above names the cause — `ludion explain L0013`"
    };
    Diagnostic::new(code::DEV_CHILD_FAILED, detail).with_help(help)
}

/// The `ws://…` URL a `listening on ws://…` line announces, or `None` if the line is not one.
fn parse_listening_url(line: &str) -> Option<&str> {
    line.trim()
        .strip_prefix("listening on ")
        .filter(|rest| rest.starts_with("ws://"))
}

/// The status document's view of sync diagnostics: no source position, rustc-shaped otherwise.
fn status_from_diagnostics(diagnostics: &[Diagnostic]) -> Vec<StatusDiagnostic> {
    diagnostics
        .iter()
        .map(|diagnostic| StatusDiagnostic {
            level: diagnostic.severity.as_str().to_string(),
            code: Some(diagnostic.code.to_string()),
            message: diagnostic.message.clone(),
            file: None,
            line: None,
            column: None,
            rendered: diagnostic.to_string(),
        })
        .collect()
}

/// One line when a sync wrote anything; a fresh tree is silent.
fn report_sync(summary: &crate::sync::Summary) {
    let changed = summary.imported + summary.removed;
    if changed > 0 {
        emit(&format!("dev synced {changed} derived import(s)"));
    }
}

/// Prints one line to stdout and flushes — dev's stdout is block-buffered when piped (as in the exit
/// tests), so an unflushed status line would not be seen until the buffer filled or the process ended.
fn emit(line: &str) {
    println!("{line}");
    let _ = std::io::stdout().flush();
}

/// Wall-clock helpers. Isolated so the one legitimate use of the clock in dev — measuring the
/// reload loop time and bounding waits — carries a single, documented `allow` for the sim-side
/// determinism denylist (`ludion dev` is the CLI supervisor, not sim-side; §5).
#[allow(clippy::disallowed_methods)]
mod clock {
    use std::time::{Duration, Instant, SystemTime};

    /// The current wall-clock time as an RFC3339 UTC string (`YYYY-MM-DDTHH:MM:SSZ`) — the
    /// `timestamp` on each status write. The sole `SystemTime::now` in dev lives here beside the
    /// `Instant::now`s so the one documented determinism `allow` covers every wall-clock read.
    pub fn now_rfc3339_utc() -> String {
        super::rfc3339_utc(SystemTime::now())
    }

    /// A future point in wall-clock time, for bounding a wait.
    pub struct Deadline(Instant);

    impl Deadline {
        /// A deadline `after` from now.
        pub fn after(after: Duration) -> Deadline {
            Deadline(Instant::now() + after)
        }
        /// Whether the deadline has passed.
        pub fn passed(&self) -> bool {
            Instant::now() >= self.0
        }
    }

    /// Measures elapsed wall time (the reload loop's `touch → restored` number).
    pub struct Stopwatch(Instant);

    impl Stopwatch {
        /// Starts the stopwatch now.
        pub fn start() -> Stopwatch {
            Stopwatch(Instant::now())
        }
        /// Milliseconds elapsed since [`start`](Stopwatch::start).
        pub fn elapsed_ms(&self) -> u128 {
            self.0.elapsed().as_millis()
        }
    }
}

use clock::{Deadline, Stopwatch};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_flags_become_child_args() {
        let none = ChildFlags {
            dev_shaders: false,
            rebake_probes: false,
            anti_aliasing: None,
        };
        assert!(none.args().is_empty());
        let all = ChildFlags {
            dev_shaders: true,
            rebake_probes: true,
            anti_aliasing: Some(AntiAliasing::Smaa),
        };
        assert_eq!(
            all.args(),
            ["--dev-shaders", "--probes", "rebake", "--aa", "smaa"]
        );
    }

    #[test]
    fn status_from_diagnostics_keeps_the_severity() {
        let warning = Diagnostic::new(code::MAP_GATE_SEALED, "sealed");
        assert_eq!(
            warning.severity,
            ludion_core::diagnostics::Severity::Warning
        );
        let levels: Vec<String> =
            status_from_diagnostics(&[warning, child_failed("boom", ListenFailure::Exited)])
                .into_iter()
                .map(|s| s.level)
                .collect();
        assert_eq!(levels, ["warning", "error"]);
    }

    /// A reload restores with input released and the session's doc-reload log dropped: `input` and
    /// `doc_reloads` go, every other key stays as it was.
    #[test]
    fn a_reload_drops_the_snapshots_held_input_and_doc_reloads() {
        let mut held = serde_json::json!({
            "frame": 12,
            "input": { "keys": ["KeyW"] },
            "doc_reloads": [{ "kind": "tuning" }],
            "world": {}
        });
        prepare_for_reload(&mut held);
        assert_eq!(held, serde_json::json!({ "frame": 12, "world": {} }));

        let mut idle = serde_json::json!({ "frame": 12, "world": {} });
        prepare_for_reload(&mut idle);
        assert_eq!(idle, serde_json::json!({ "frame": 12, "world": {} }));

        let mut not_a_document = serde_json::json!([1, 2]);
        prepare_for_reload(&mut not_a_document);
        assert_eq!(not_a_document, serde_json::json!([1, 2]));
    }

    /// Only a bind failure (or a silent death, which may be one) is worth the bind-race retries.
    #[test]
    fn only_a_bind_failure_is_retried() {
        assert!(looks_like_bind_failure(
            "could not bind 127.0.0.1:9411: Only one usage of each socket address"
        ));
        assert!(looks_like_bind_failure(""));
        assert!(looks_like_bind_failure("  \n"));
        assert!(!looks_like_bind_failure(
            "error[L1013]: no component named `Player` is registered"
        ));
        assert!(!looks_like_bind_failure(
            "thread 'main' panicked at src/lib.rs:3:5:\nboom"
        ));
    }

    /// The fingerprint path follows cargo's layout under the resolved target root: plain `debug/`
    /// normally, and a triple segment ahead of it when `CARGO_BUILD_TARGET` names one.
    #[test]
    fn fingerprint_root_follows_cargos_layout() {
        let target = Path::new("/ws/target");
        let tail = Path::new("debug").join(".fingerprint");

        assert_eq!(
            fingerprint_root_of(target.to_path_buf(), None),
            target.join(&tail)
        );
        assert_eq!(
            fingerprint_root_of(
                target.to_path_buf(),
                Some(OsStr::new("aarch64-apple-darwin"))
            ),
            target.join("aarch64-apple-darwin").join(&tail)
        );
        // An exported-but-empty `CARGO_BUILD_TARGET` is cargo's "no triple", not a directory named "".
        assert_eq!(
            fingerprint_root_of(target.to_path_buf(), Some(OsStr::new(""))),
            target.join(&tail)
        );
    }

    /// A workspace member's fingerprints live under the *workspace* root's `target/`, not the
    /// package's own directory — the miss that made `ludion dev` on `games/*` purge nothing. This
    /// crate is such a member, so its own location is the fixture.
    #[test]
    fn cargo_target_directory_is_the_workspaces_not_the_packages() {
        let package = Path::new(env!("CARGO_MANIFEST_DIR"));
        let dir = cargo_target_directory(package).expect("cargo reports this crate's target dir");
        assert!(
            dir.is_absolute() && !dir.starts_with(package),
            "expected a target dir outside {}, got {}",
            package.display(),
            dir.display()
        );
    }

    #[test]
    fn parses_a_listening_line() {
        assert_eq!(
            parse_listening_url("listening on ws://127.0.0.1:9411"),
            Some("ws://127.0.0.1:9411"),
        );
        assert_eq!(
            parse_listening_url("  listening on ws://127.0.0.1:0\n"),
            Some("ws://127.0.0.1:0"),
        );
    }

    #[test]
    fn non_listening_lines_do_not_parse() {
        assert_eq!(parse_listening_url("engine version: 0.0.0"), None);
        assert_eq!(parse_listening_url("listening on http://x"), None);
        assert_eq!(parse_listening_url(""), None);
    }

    /// A `bin` `compiler-artifact` with an `executable` yields its path; a `lib` artifact, a
    /// non-artifact message, and a bin artifact without an `executable` all yield `None` — the streaming
    /// reader folds this over the output and keeps the last hit.
    #[test]
    fn artifact_executable_matches_only_a_bin_with_an_executable() {
        let bin = serde_json::json!({
            "reason": "compiler-artifact",
            "target": { "kind": ["bin"], "name": "game" },
            "executable": "/t/debug/game"
        });
        assert_eq!(
            artifact_executable(&bin),
            Some(PathBuf::from("/t/debug/game")),
        );

        let lib = serde_json::json!({
            "reason": "compiler-artifact",
            "target": { "kind": ["lib"], "name": "foo" },
            "executable": serde_json::Value::Null
        });
        assert_eq!(artifact_executable(&lib), None);

        let message = serde_json::json!({ "reason": "build-finished", "success": true });
        assert_eq!(artifact_executable(&message), None);

        let bin_no_exe = serde_json::json!({
            "reason": "compiler-artifact",
            "target": { "kind": ["bin"], "name": "game" },
            "executable": serde_json::Value::Null
        });
        assert_eq!(artifact_executable(&bin_no_exe), None);
    }

    /// An exit, a closed pipe and a timeout are told apart by the pure classifiers `wait_for_listening`
    /// calls; swapping any pair fails here.
    #[test]
    fn listen_failures_are_classified_by_cause() {
        assert_eq!(classify_silence(true, false), Some(ListenFailure::Exited));
        assert_eq!(classify_silence(true, true), Some(ListenFailure::Exited));
        assert_eq!(classify_silence(false, true), Some(ListenFailure::TimedOut));
        assert_eq!(classify_silence(false, false), None);
        assert_eq!(classify_disconnect(true), ListenFailure::Exited);
        assert_eq!(classify_disconnect(false), ListenFailure::OutputClosed);
    }

    /// A spawn failure is worded as one, never as an exit.
    #[test]
    fn child_failed_words_a_spawn_failure_as_one() {
        let diag = child_failed(
            "could not start the game binary: denied",
            ListenFailure::SpawnFailed,
        );
        assert!(
            diag.message.contains("could not be started"),
            "{}",
            diag.message
        );
        assert!(!diag.message.contains("exited"), "{}", diag.message);
    }

    #[test]
    fn child_failed_embeds_output() {
        let diag = child_failed(
            "error[L1013]: unknown component `Nope`",
            ListenFailure::Exited,
        );
        assert_eq!(diag.code.as_str(), "L0013");
        assert!(diag.message.contains("Nope"));
        assert!(diag.message.contains("exited"), "{}", diag.message);
    }

    /// A launch that outlived the listen window is worded as a timeout, never as an exit.
    #[test]
    fn child_failed_words_a_timeout_as_a_timeout() {
        let diag = child_failed("", ListenFailure::TimedOut);
        assert!(
            diag.message.contains("did not print `listening on"),
            "{}",
            diag.message
        );
        assert!(diag.message.contains("slow boot"), "{}", diag.message);
        assert!(!diag.message.contains("exited"), "{}", diag.message);
        let help = diag.help.as_deref().expect("a timeout has help");
        assert!(help.contains("quieter machine"), "{help}");
    }

    /// A bind failure whose closed stdout won the race with the exit still gets the bind-race help.
    #[test]
    fn child_failed_closed_output_keeps_the_bind_race_help() {
        let diag = child_failed(
            "could not bind 127.0.0.1:9411: in use",
            ListenFailure::OutputClosed,
        );
        let help = diag.help.as_deref().expect("a bind race has help");
        assert!(help.contains("bind race"), "{help}");
    }

    /// A closed stdout is its own reason, neither an exit nor a timeout.
    #[test]
    fn child_failed_words_closed_output_as_closed_output() {
        let diag = child_failed("partial", ListenFailure::OutputClosed);
        assert!(
            diag.message.contains("closed its output"),
            "{}",
            diag.message
        );
        assert!(!diag.message.contains("exited"), "{}", diag.message);
        assert!(!diag.message.contains("did not print"), "{}", diag.message);
    }

    /// A bind race says it may clear on its own; a boot failure says dev keeps the snapshot.
    #[test]
    fn child_failed_help_follows_the_cause() {
        let race = child_failed(
            "could not bind 127.0.0.1:9411: in use",
            ListenFailure::Exited,
        );
        let help = race.help.as_deref().expect("a bind race has help");
        assert!(help.contains("bind race"), "{help}");

        let boot = child_failed(
            "error[L1013]: unknown component `Nope`",
            ListenFailure::Exited,
        );
        let help = boot.help.as_deref().expect("a boot failure has help");
        assert!(help.contains("names the cause"), "{help}");
        assert!(!help.contains("bind race"), "{help}");
        assert!(!help.contains("retries"), "{help}");
    }

    /// A failed reload launch says dev retries on the next save, and restores only when a snapshot waits.
    #[test]
    fn reload_retry_help_names_the_kept_snapshot_only_when_one_waits() {
        assert!(reload_retry_help(true).contains("restores the kept snapshot"));
        assert!(!reload_retry_help(false).contains("snapshot"));
        assert!(reload_retry_help(false).contains("retries the launch on the next save"));
    }

    #[test]
    fn parse_boot_restore_reads_both_outcome_lines_and_refuses_the_rest() {
        let entry = RestoreEntry::ComponentUnregistered {
            entity: ludion_core::ids::EntityId::new(2, 0),
            component: "Player".into(),
        };
        let line = format!(
            "restored frame=42 report={}",
            serde_json::to_string(&RestoreReport {
                entries: vec![entry.clone()]
            })
            .unwrap()
        );
        let (frame, report) = parse_boot_restore(Some(&line)).expect("a restored line parses");
        assert_eq!(frame, 42);
        assert_eq!(report.entries, vec![entry]);

        let refused = Diagnostic::new(code::PROTOCOL_SNAPSHOT_INVALID, "no good");
        let line = format!(
            "restore failed: {}",
            serde_json::to_string(&refused).unwrap()
        );
        let diagnostic =
            parse_boot_restore(Some(&line)).expect_err("a failed line is the diagnostic");
        assert_eq!(diagnostic.code, code::PROTOCOL_SNAPSHOT_INVALID);
        assert_eq!(diagnostic.message, "no good");

        for line in [
            None,
            Some(""),
            Some("restored frame=x report={}"),
            Some("garbage"),
        ] {
            let diagnostic = parse_boot_restore(line).expect_err("not a restore outcome");
            assert_eq!(diagnostic.code, code::DEV_DEGRADED, "{line:?}");
        }
    }

    #[test]
    fn child_failed_handles_empty_output() {
        let diag = child_failed("   \n", ListenFailure::Exited);
        assert_eq!(diag.code.as_str(), "L0013");
        assert!(diag.message.contains("no output captured"));
    }

    /// The scenes-changed hint is a machine-parseable status line like the others (module docs, point
    /// 4): one line, no interpolated data, so a client can match it with plain equality.
    #[test]
    fn scenes_changed_line_is_the_documented_hint() {
        assert_eq!(
            LINE_SCENES_CHANGED,
            "dev scenes changed (world state is preserved; assets and data-docs reload live, scene edits need scene.load or restart)",
        );
        assert!(LINE_SCENES_CHANGED.starts_with("dev scenes changed"));
        assert!(!LINE_SCENES_CHANGED.contains('\n'));
    }

    #[test]
    fn unit_dirs_match_both_package_name_spellings() {
        assert!(is_unit_dir_for("dev_game-95259bf0ea11bdc3", "dev_game"));
        assert!(is_unit_dir_for("my-game-6c9c157dcd3759b8", "my-game"));
        assert!(is_unit_dir_for("my_game-6c9c157dcd3759b8", "my-game"));
    }

    #[test]
    fn unit_dirs_never_claim_a_prefixed_sibling_package() {
        // `foo-utils-<hash>` must not match package `foo`: the remainder is not a bare hex hash.
        assert!(!is_unit_dir_for("foo-utils-95259bf0ea11bdc3", "foo"));
        assert!(!is_unit_dir_for("foo", "foo"));
        assert!(!is_unit_dir_for("foo-", "foo"));
        assert!(!is_unit_dir_for("bar-95259bf0ea11bdc3", "foo"));
    }

    #[test]
    fn package_name_reads_the_manifest() {
        let dir = std::env::temp_dir().join(format!("ludion-dev-pkg-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"probe_game\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        assert_eq!(package_name(&dir).as_deref(), Some("probe_game"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An unchanged `src/` compares equal; an edit, an added file and a deleted file each change the
    /// fingerprint, so a watcher batch for an already-built save can be told from a real one.
    #[test]
    fn src_fingerprint_sees_edits_additions_and_deletions() {
        let project =
            std::env::temp_dir().join(format!("ludion-dev-fingerprint-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&project);
        let src = project.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("lib.rs"), "fn a() {}").unwrap();
        let base = src_fingerprint(&project).expect("a readable tree");
        assert_eq!(src_fingerprint(&project), Some(base.clone()));
        assert!(!src_changed_since(&project, Some(&base)));

        std::fs::write(src.join("lib.rs"), "fn a() { let _ = 1; }").unwrap();
        let edited = src_fingerprint(&project).unwrap();
        assert_ne!(edited, base, "an edit changes the size");

        std::fs::write(src.join("extra.rs"), "").unwrap();
        let added = src_fingerprint(&project).unwrap();
        assert_ne!(
            added, edited,
            "an added (even empty) file changes the count"
        );

        std::fs::remove_file(src.join("extra.rs")).unwrap();
        let deleted = src_fingerprint(&project).unwrap();
        assert_ne!(deleted, added, "a deleted file changes the count");
        assert!(src_changed_since(&project, Some(&added)));
        assert!(src_changed_since(&project, None));
        let _ = std::fs::remove_dir_all(&project);
    }

    /// A live child that did not deliver (a wedged sim, a refusal) is told apart from a dead one only
    /// by the message dev prints; both relaunch fresh.
    #[test]
    fn snapshot_failure_tells_a_live_child_from_a_dead_one() {
        assert_eq!(snapshot_failure(false), SnapshotFailure::ChildAlive);
        assert_eq!(snapshot_failure(true), SnapshotFailure::ChildExited);
    }

    /// The live-child line names the cause's code and first message line, and the loss — on one line.
    #[test]
    fn snapshot_failed_line_names_the_cause_on_one_line() {
        let diagnostic = Diagnostic::new(
            code::DEV_DEGRADED,
            "the call timed out\nsecond line of detail",
        );
        let line = snapshot_failed_line(&diagnostic);
        assert_eq!(
            line,
            format!(
                "dev reload snapshot failed ({}: the call timed out); restarting fresh, session state lost",
                code::DEV_DEGRADED
            )
        );
        assert!(!line.contains('\n'));
    }

    /// An `L3003` gains the skew explanation; any other diagnostic passes through unchanged.
    #[test]
    fn version_skew_is_explained_and_other_diagnostics_pass_through() {
        let skew = explain_version_skew(Diagnostic::new(
            code::PROTOCOL_UNSUPPORTED_VERSION,
            "schema hash differs",
        ));
        assert_eq!(skew.code, code::PROTOCOL_UNSUPPORTED_VERSION);
        assert!(
            skew.message.contains("different engine revisions")
                && skew.message.contains("schema hash differs"),
            "message: {}",
            skew.message
        );
        assert!(
            skew.help
                .is_some_and(|help| help.contains("cargo build -p ludion-cli"))
        );
        let other = Diagnostic::new(code::DEV_DEGRADED, "something else");
        assert_eq!(explain_version_skew(other.clone()), other);
    }

    fn unregistered(index: u32, component: &str) -> RestoreEntry {
        RestoreEntry::ComponentUnregistered {
            entity: EntityId::new(index, 0),
            component: component.to_string(),
        }
    }

    fn migrated(index: u32, component: &str, filled: &[&str]) -> RestoreEntry {
        RestoreEntry::ComponentMigrated {
            entity: EntityId::new(index, 0),
            component: component.to_string(),
            filled: filled.iter().map(ToString::to_string).collect(),
            removed: Vec::new(),
        }
    }

    /// Entries sharing a `(kind, component, fields)` key collapse to one line with the count and the
    /// first entity; the document-level entries keep their one line each, in report order.
    #[test]
    fn report_lines_group_per_entity_entries() {
        let report = RestoreReport {
            entries: vec![
                RestoreEntry::EngineVersionDiffers {
                    snapshot_version: "0.1.0".to_string(),
                    engine_version: "0.2.0".to_string(),
                },
                unregistered(4, "Player"),
                migrated(5, "Health", &["max"]),
                unregistered(6, "Player"),
                migrated(7, "Health", &["regen"]),
                unregistered(9, "Player"),
                RestoreEntry::SnapshotFieldUnknown {
                    field: "extras".to_string(),
                },
            ],
        };
        let lines = report_lines(&report);
        assert_eq!(
            lines,
            vec![
                "  [L3048] snapshot taken by engine 0.1.0, restored into 0.2.0".to_string(),
                "  [L3015] component `Player` dropped (no longer registered) on 3 entities, first (4,0)"
                    .to_string(),
                "  component `Health` migrated (filled: [max], removed: []) on 1 entity, first (5,0)"
                    .to_string(),
                "  component `Health` migrated (filled: [regen], removed: []) on 1 entity, first (7,0)"
                    .to_string(),
                "  [L3049] snapshot field `extras` is unknown to this engine, ignored".to_string(),
            ]
        );
        assert!(report_lines(&RestoreReport::default()).is_empty());
    }

    /// Deserialize failures group by component, not by the error text (which embeds the value): one
    /// line per component with the first entry's error as the example.
    #[test]
    fn report_lines_group_undeserializable_by_component() {
        let undeserializable =
            |index: u32, component: &str, error: &str| RestoreEntry::ComponentUndeserializable {
                entity: EntityId::new(index, 0),
                component: component.to_string(),
                error: error.to_string(),
            };
        let report = RestoreReport {
            entries: vec![
                undeserializable(2, "Health", "invalid type: string \"a\"\nat line 1"),
                undeserializable(3, "Mana", "missing field `max`"),
                undeserializable(4, "Health", "invalid type: string \"b\""),
            ],
        };
        assert_eq!(
            report_lines(&report),
            vec![
                "  [L3016] component `Health` dropped (its value no longer deserializes) on 2 entities, first (2,0): invalid type: string \"a\"".to_string(),
                "  [L3016] component `Mana` dropped (its value no longer deserializes) on 1 entity, first (3,0): missing field `max`".to_string(),
            ]
        );
    }

    /// An `error` diagnostic with a primary span yields `file`/`line`/`column` from that span, the
    /// `E####` code, and rustc's `rendered` text.
    #[test]
    fn diagnostic_error_with_span_is_flattened() {
        let message = serde_json::json!({
            "message": "mismatched types",
            "code": { "code": "E0308", "explanation": "…" },
            "level": "error",
            "spans": [
                { "file_name": "src/other.rs", "line_start": 3, "column_start": 1, "is_primary": false },
                { "file_name": "src/main.rs", "line_start": 12, "column_start": 5, "is_primary": true }
            ],
            "rendered": "error[E0308]: mismatched types\n --> src/main.rs:12:5\n"
        });
        let diag = diagnostic_from_message(&message).expect("an error is collected");
        assert_eq!(diag.level, "error");
        assert_eq!(diag.code.as_deref(), Some("E0308"));
        assert_eq!(diag.message, "mismatched types");
        // The primary span wins over the earlier non-primary one.
        assert_eq!(diag.file.as_deref(), Some("src/main.rs"));
        assert_eq!(diag.line, Some(12));
        assert_eq!(diag.column, Some(5));
        assert!(diag.rendered.starts_with("error[E0308]"));
    }

    /// A `warning` is collected too (the status file carries both errors and warnings), with a `null`
    /// code left as `None`.
    #[test]
    fn diagnostic_warning_is_collected_with_null_code() {
        let message = serde_json::json!({
            "message": "unused variable: `x`",
            "code": serde_json::Value::Null,
            "level": "warning",
            "spans": [
                { "file_name": "src/main.rs", "line_start": 4, "column_start": 9, "is_primary": true }
            ],
            "rendered": "warning: unused variable: `x`\n"
        });
        let diag = diagnostic_from_message(&message).expect("a warning is collected");
        assert_eq!(diag.level, "warning");
        assert_eq!(diag.code, None);
        assert_eq!(diag.line, Some(4));
    }

    /// A span-less error (e.g. an unresolved crate) still collects, with `file`/`line`/`column` absent.
    #[test]
    fn diagnostic_error_without_span_has_no_location() {
        let message = serde_json::json!({
            "message": "can't find crate for `nope`",
            "code": { "code": "E0463", "explanation": "…" },
            "level": "error",
            "spans": [],
            "rendered": "error[E0463]: can't find crate for `nope`\n"
        });
        let diag = diagnostic_from_message(&message).expect("a span-less error is collected");
        assert_eq!(diag.code.as_deref(), Some("E0463"));
        assert_eq!(diag.file, None);
        assert_eq!(diag.line, None);
        assert_eq!(diag.column, None);
    }

    /// A `note`/`help` top-level message is not its own status entry (it rides its parent's `rendered`).
    #[test]
    fn diagnostic_note_level_is_ignored() {
        let message = serde_json::json!({
            "message": "the note",
            "code": serde_json::Value::Null,
            "level": "note",
            "spans": [],
            "rendered": "note: …\n"
        });
        assert_eq!(diagnostic_from_message(&message), None);
    }

    /// The `failed` document's shape is stable: fields in declaration order, `duration_ms` and a
    /// (possibly empty) `diagnostics` array present, `restored_frame` omitted. A snapshot-style assert
    /// so a field-order or shape regression is caught.
    #[test]
    fn failed_status_serializes_to_the_stable_shape() {
        let status = DevStatus {
            state: BuildState::Failed,
            seq: 3,
            timestamp: "2026-07-19T12:34:56Z".to_string(),
            duration_ms: Some(742),
            restored_frame: None,
            report: None,
            message: None,
            snapshot: None,
            diagnostics: Some(vec![StatusDiagnostic {
                level: "error".to_string(),
                code: Some("E0308".to_string()),
                message: "mismatched types".to_string(),
                file: Some("src/main.rs".to_string()),
                line: Some(12),
                column: Some(5),
                rendered: "error[E0308]: mismatched types\n".to_string(),
            }]),
        };
        let json = serde_json::to_string_pretty(&status).unwrap();
        assert_eq!(
            json,
            r#"{
  "state": "failed",
  "seq": 3,
  "timestamp": "2026-07-19T12:34:56Z",
  "duration_ms": 742,
  "diagnostics": [
    {
      "level": "error",
      "code": "E0308",
      "message": "mismatched types",
      "file": "src/main.rs",
      "line": 12,
      "column": 5,
      "rendered": "error[E0308]: mismatched types\n"
    }
  ]
}"#
        );
    }

    /// The `ok` document after a reload: `duration_ms` and `restored_frame` present, `diagnostics`
    /// omitted. The initial-launch `ok` differs only in omitting `restored_frame`.
    #[test]
    fn ok_status_omits_diagnostics_and_carries_restored_frame() {
        let status = DevStatus {
            state: BuildState::Ok,
            seq: 1,
            timestamp: "2026-07-19T00:00:00Z".to_string(),
            duration_ms: Some(310),
            restored_frame: Some(1200),
            report: None,
            message: None,
            snapshot: None,
            diagnostics: None,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(
            json,
            r#"{"state":"ok","seq":1,"timestamp":"2026-07-19T00:00:00Z","duration_ms":310,"restored_frame":1200}"#
        );
    }

    /// A state-lost `ok`: `message` and the full `report` ride it after `restored_frame`'s slot, and
    /// no `restored_frame` is written.
    #[test]
    fn ok_status_carries_message_and_report() {
        let status = DevStatus {
            state: BuildState::Ok,
            seq: 2,
            timestamp: "2026-10-05T00:00:00Z".to_string(),
            duration_ms: Some(310),
            restored_frame: None,
            report: Some(RestoreReport {
                entries: vec![unregistered(4, "Player")],
            }),
            message: Some("the session state was lost".to_string()),
            snapshot: None,
            diagnostics: None,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(
            json,
            r#"{"state":"ok","seq":2,"timestamp":"2026-10-05T00:00:00Z","duration_ms":310,"report":{"entries":[{"kind":"component_unregistered","entity":{"index":4,"generation":0},"component":"Player"}]},"message":"the session state was lost"}"#
        );
    }

    /// The `building` document is the bare header: only `state`/`seq`/`timestamp`, every optional tail
    /// field omitted.
    #[test]
    fn building_status_is_the_bare_header() {
        let status = DevStatus {
            state: BuildState::Building,
            seq: 0,
            timestamp: "2026-07-19T00:00:00Z".to_string(),
            duration_ms: None,
            restored_frame: None,
            report: None,
            message: None,
            snapshot: None,
            diagnostics: None,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(
            json,
            r#"{"state":"building","seq":0,"timestamp":"2026-07-19T00:00:00Z"}"#
        );
    }

    /// `seq` is monotonic per process: successive [`next_seq`] calls strictly increase (other tests may
    /// consume values in between, so assert the relation, not exact numbers).
    #[test]
    fn seq_is_monotonic() {
        let a = next_seq();
        let b = next_seq();
        let c = next_seq();
        assert!(a < b && b < c, "seq must strictly increase: {a} {b} {c}");
    }

    /// A round-trip through the atomic file write: the JSON written to disk parses back to the same
    /// document (the editor's tail path).
    #[test]
    fn status_file_write_round_trips() {
        let path =
            std::env::temp_dir().join(format!("ludion-dev-status-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let status = DevStatus {
            state: BuildState::Crashed,
            seq: 9,
            timestamp: "2026-07-19T09:00:00Z".to_string(),
            duration_ms: None,
            restored_frame: None,
            report: None,
            message: None,
            snapshot: None,
            diagnostics: None,
        };
        write_status_file(&path, &status).unwrap();
        let read = std::fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&read).unwrap();
        assert_eq!(value["state"], "crashed");
        assert_eq!(value["seq"], 9);
        assert_eq!(value.get("duration_ms"), None);
        let _ = std::fs::remove_file(&path);
    }

    /// RFC3339 formatting is a pure function of the instant, second precision, `Z` (UTC) suffix.
    #[test]
    fn rfc3339_formats_known_instants() {
        let epoch = SystemTime::UNIX_EPOCH;
        assert_eq!(rfc3339_utc(epoch), "1970-01-01T00:00:00Z");
        // 2026-07-19T12:34:56Z == 1_784_464_496 seconds since the epoch.
        let instant = SystemTime::UNIX_EPOCH + Duration::from_secs(1_784_464_496);
        assert_eq!(rfc3339_utc(instant), "2026-07-19T12:34:56Z");
    }

    #[test]
    fn not_a_game_dir_is_l0012() {
        let dir = std::env::temp_dir().join("ludion-dev-not-a-game-probe");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        match check_runnable_game(&dir) {
            Ok(()) => panic!("an empty dir is not a runnable game"),
            Err(diag) => assert_eq!(diag.code.as_str(), "L0012"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The `restore_failed` document: the explicit snake_case state, `message` and `snapshot` after
    /// `restored_frame` and before `diagnostics`, the build duration carried, `restored_frame` omitted.
    #[test]
    fn restore_failed_status_serializes_to_the_stable_shape() {
        let status = DevStatus {
            state: BuildState::RestoreFailed,
            seq: 4,
            timestamp: "2026-09-26T08:00:00Z".to_string(),
            duration_ms: Some(512),
            restored_frame: None,
            report: None,
            message: Some("engine.restore failed".to_string()),
            snapshot: Some("game/.ludion/dev-snapshot.json".to_string()),
            diagnostics: None,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(
            json,
            r#"{"state":"restore_failed","seq":4,"timestamp":"2026-09-26T08:00:00Z","duration_ms":512,"message":"engine.restore failed","snapshot":"game/.ludion/dev-snapshot.json"}"#
        );
    }

    /// A `failed` document from a child that could not boot: `snapshot` sits between `duration_ms`
    /// and `diagnostics`, carrying the kept path.
    #[test]
    fn launch_failed_status_serializes_to_the_stable_shape() {
        let status = DevStatus {
            state: BuildState::Failed,
            seq: 7,
            timestamp: "2026-10-05T08:00:00Z".to_string(),
            duration_ms: Some(512),
            restored_frame: None,
            report: None,
            message: None,
            snapshot: Some("game/.ludion/dev-snapshot.json".to_string()),
            diagnostics: Some(status_from_diagnostics(&[child_failed(
                "boom",
                ListenFailure::Exited,
            )])),
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["state"], "failed");
        assert_eq!(json["snapshot"], "game/.ludion/dev-snapshot.json");
        assert_eq!(json["diagnostics"][0]["code"], "L0013");
        let text = serde_json::to_string(&status).unwrap();
        let at = |key: &str| text.find(&format!("\"{key}\":")).expect(key);
        assert!(at("duration_ms") < at("snapshot") && at("snapshot") < at("diagnostics"));
    }

    /// The tail keeps the newest [`STDERR_TAIL_LINES`] lines, then flips to forwarding: the buffer is
    /// handed back oldest first and every later line is returned to the caller to print.
    #[test]
    fn stderr_tail_evicts_the_oldest_then_forwards() {
        let mut tail = StderrTail::default();
        for n in 0..STDERR_TAIL_LINES + 5 {
            assert_eq!(tail.push(format!("line {n}")), None);
        }
        assert_eq!(tail.lines.len(), STDERR_TAIL_LINES);
        assert!(tail.text().starts_with("line 5\n"));

        let buffered = tail.start_forwarding();
        assert_eq!(buffered.len(), STDERR_TAIL_LINES);
        assert_eq!(buffered.first().map(String::as_str), Some("line 5"));
        assert_eq!(
            buffered.last().map(String::as_str),
            Some(format!("line {}", STDERR_TAIL_LINES + 4).as_str())
        );
        assert_eq!(tail.text(), "");
        assert_eq!(tail.push("late".to_string()), Some("late".to_string()));
        assert!(tail.lines.is_empty());
    }

    /// The sweep removes a dead session's directory (its lock is free), keeps a live one (its lock is
    /// held on another handle), removes a lockless directory once it is older than the grace and keeps a fresh one, skips a
    /// dead session whose lock was touched inside the grace, and never touches a name outside
    /// the session prefix.
    #[test]
    fn sweep_removes_only_dead_sessions() {
        let temp =
            std::env::temp_dir().join(format!("ludion-dev-sweep-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let session = |name: &str| {
            let dir = temp.join(format!("{SESSION_DIR_PREFIX}{name}"));
            std::fs::create_dir_all(&dir).unwrap();
            dir
        };
        let open_lock = |dir: &Path| {
            std::fs::File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(dir.join("lock"))
                .unwrap()
        };
        let dead = session("dead");
        std::fs::write(dead.join("0-game.exe"), b"stale").unwrap();
        drop(open_lock(&dead));
        let live = session("live");
        let live_lock = open_lock(&live);
        live_lock.lock().unwrap();
        let mid_creation = session("mid-creation");
        let fresh = session("fresh-lock");
        drop(open_lock(&fresh));
        let other = temp.join("ludion-dev-pkg-probe-x");
        std::fs::create_dir_all(&other).unwrap();

        // Inside the grace nothing settled yet: the lockless and fresh-lock directories survive.
        sweep_dead_sessions(&temp, Duration::from_secs(3600));
        assert!(mid_creation.exists(), "a fresh lockless directory is kept");
        assert!(fresh.exists(), "a lock touched inside the grace is kept");
        assert!(
            dead.exists(),
            "the grace protects a dead session too, while its lock is fresh"
        );

        std::thread::sleep(Duration::from_millis(100));
        sweep_dead_sessions(&temp, Duration::from_millis(50));

        assert!(!dead.exists(), "a dead session's directory is removed");
        assert!(live.exists(), "a live session's directory is kept");
        assert!(
            !mid_creation.exists(),
            "a settled lockless directory is removed"
        );
        assert!(!fresh.exists(), "a settled unlocked session is removed");
        assert!(other.exists(), "a non-session name is never touched");
        drop(live_lock);
        let _ = std::fs::remove_dir_all(&temp);
    }
}
