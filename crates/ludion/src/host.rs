//! The shared host lifecycle around the embedding core ([`crate::embed`]), which owns it: audio wiring,
//! `--paused`, the surface attach, `Server::bind`, the header block, the asset watcher and the one
//! graceful teardown.
//!
//! Every host — `ludion serve` (the CLI), [`ludion::run`](crate::run) (a game's own `main`),
//! [`launch`](crate::launch) — differs only in how it *builds* the engine and parses its options; from
//! there each starts an [`Embedded`](crate::embed::Embedded), so a shutdown-ordering fix lands in all of
//! them at once. This module holds what the core's callers share: [`host_scene`] (a built engine, for
//! `serve`), [`host_until_stopped`] (the headless wait), and the core's own pieces — the audio wiring,
//! [`announce`] and [`watch_project`]. Two cargo features shape what is compiled: `wire` (the listener
//! and `host_scene` itself) and `window` (the windowed shell in [`crate::present`]; without it
//! `--window` is refused up front).
//!
//! `--window` hands the core a surface the main thread created: the engine must take it before any
//! renderer exists, which the core does on its sim thread ahead of the bind.

#[cfg(feature = "wire")]
use std::io::Write;
#[cfg(feature = "wire")]
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "wire")]
use std::time::Duration;

#[cfg(all(feature = "wire", not(feature = "window")))]
use ludion_core::codes::code;
#[cfg(all(feature = "wire", not(feature = "window")))]
use ludion_core::diagnostics::Diagnostic;
use ludion_engine::Engine;
#[cfg(feature = "wire")]
use ludion_server::Server;

use crate::embed::Embedded;
#[cfg(feature = "wire")]
use crate::embed::{Source, StartOptions};

/// The asset-watch debounce: long enough to coalesce an image editor's multi-write save into one
/// reload, short enough to feel immediate. Matches `ludion dev`'s `src/` watch.
#[cfg(feature = "wire")]
const ASSET_WATCH_DEBOUNCE: Duration = Duration::from_millis(300);

/// The host-specific presentation choices [`host_scene`] cannot derive from the engine itself — the
/// small set that legitimately differs between `ludion serve` and [`run`](crate::run). Everything
/// else (audio, bind, watch, window, teardown) is identical and owned by `host_scene`.
#[cfg(feature = "wire")]
pub struct HostOptions<'a> {
    /// The scene path, printed in the header (`scene: …`) and used for the `--window` title.
    pub scene: &'a Path,
    /// The armed recording's path, if any — drives only the header line
    /// (`recording: armed -> … | off`). The recording must already be armed on the engine by the
    /// caller (each host arms it its own way); this never arms it.
    pub record_path: Option<&'a Path>,
    /// Start the engine paused (`sim.pause`) before binding — a deterministic attach point.
    pub paused: bool,
    /// Open the game's own OS window and present the engine's rendered frames into it in process,
    /// instead of blocking headless. The listener still binds, so an agent, the CLI or the editor
    /// attaches exactly as it does to a headless host — the window is a second sink for the very
    /// frame they stream, not a client of it. Needs the crate's default `window` feature: a build
    /// without it refuses this (`L0018`) before binding.
    pub window: bool,
    /// Print the `schema hash: …` header line. `ludion serve` does (it is the attach point external
    /// clients read); [`run`](crate::run) does not.
    pub print_schema_hash: bool,
    /// The host's command line has a `--window` flag, so the headless banner may suggest it. Only
    /// [`run`](crate::run) and `ludion serve` parse one; a [`launch`](crate::launch) host has no argv.
    pub window_flag: bool,
}

/// Hosts a built `engine` over the wire protocol at `addr` and runs it until Ctrl-C, a wire
/// `engine.shutdown`, or (in `--window` mode) the window closing — then shuts down cleanly, flushing
/// any armed recording. The caller has already built the engine and done its own prep (recording arm,
/// quality, render size); the [embedding core](crate::embed) owns everything from `--paused` on.
///
/// On success it prints exactly one machine-parseable line first — `listening on ws://<addr>`,
/// flushed immediately, so a test or agent can read the bound address when `addr` asked for an
/// OS-chosen port (`:0`) — then a short human-readable block whose exact lines
/// [`HostOptions`] selects.
///
/// Without `--window`, it blocks until Ctrl-C or a wire `engine.shutdown` and returns
/// [`ExitCode::SUCCESS`]. With `--window`, it hands the lifecycle to `crate::present::host_windowed`,
/// which opens the game's **own** window on **this thread** (a winit requirement — the ultimate caller
/// must therefore be `main`, not a spawned thread), gives the core that window's surface, and blocks
/// there while every rendered frame is blitted straight into the window: no encode, no decode, no
/// client in between. The listener still binds, so agents and the editor attach as they always did;
/// the out-of-process viewer remains only for `ludion dev --window`, whose window must outlive its
/// child process. Closing the window, Ctrl-C, or a wire `engine.shutdown` all end it the same way.
/// If `--window` was asked for but the window or its GPU surface fails (headless session, no
/// adapter), the diagnostic is printed to stderr and a non-zero code returned.
///
/// A failed `--paused` (`sim.pause`), a failed `Server::bind`, or `--window` in a build without the
/// `window` feature (`L0018`) prints a diagnostic to stderr and returns a non-zero code; nothing is
/// printed on stdout in that case.
#[cfg(feature = "wire")]
pub fn host_scene(engine: Engine, addr: &str, opts: HostOptions) -> ExitCode {
    // Refused before anything binds or plays: a windowless build cannot honour the request, and
    // falling back to headless would host something nobody asked for.
    #[cfg(not(feature = "window"))]
    if opts.window {
        eprintln!("{}", window_feature_off());
        return ExitCode::FAILURE;
    }

    let source = Source::Built {
        engine,
        scene: opts.scene.to_path_buf(),
        record_path: opts.record_path.map(Path::to_path_buf),
    };
    let options = StartOptions {
        paused: opts.paused,
        listen: Some(addr.to_string()),
        print_schema_hash: opts.print_schema_hash,
        window_flag: opts.window_flag,
        ..StartOptions::default()
    };

    #[cfg(feature = "window")]
    if opts.window {
        return crate::present::host_windowed(source, options);
    }

    match Embedded::start_from(source, None, options) {
        Ok(embedded) => host_until_stopped(embedded, crossbeam_channel::never()),
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            ExitCode::FAILURE
        }
    }
}

/// Blocks until Ctrl-C (or platform equivalent, e.g. a Windows console control event) or the host
/// ending on its own — a failed boot, a wire or in-process `engine.shutdown` — then stops it and
/// returns its exit code. The one wait every headless shell shares; it installs the process's Ctrl-C
/// handler, so a host calls it at most once. A `supervisor` message ([`stdin_closed`], or a channel
/// that never fires) stops it the same way. A fatal render fault (a lost device, a dead surface)
/// is printed once and stops it with a failure exit code.
pub(crate) fn host_until_stopped(
    embedded: Embedded,
    supervisor: crossbeam_channel::Receiver<()>,
) -> ExitCode {
    host_until(embedded, ctrl_c_stop(), supervisor)
}

/// [`host_until_stopped`] with the Ctrl-C wait injected, so a test can host without installing the
/// process's one handler.
pub(crate) fn host_until(
    embedded: Embedded,
    ctrl_c: crossbeam_channel::Receiver<()>,
    supervisor: crossbeam_channel::Receiver<()>,
) -> ExitCode {
    let ended = embedded.ended();
    let render_fault = embedded.render_fault();
    let fault = crossbeam_channel::select! {
        recv(ctrl_c) -> _ => None,
        recv(ended) -> _ => None,
        recv(supervisor) -> _ => None,
        // `Err` means the engine dropped its sender: the host is ending, like `ended`.
        recv(render_fault) -> fault => fault.ok(),
    };
    if let Some(diagnostic) = &fault {
        eprintln!("{diagnostic}");
    }
    let code = embedded.stop();
    if fault.is_some() {
        ExitCode::FAILURE
    } else {
        code
    }
}

/// Fires once the process's stdin reaches EOF — the supervising process holding the write end died
/// or dropped it. The `--supervised` host waits on this so a child never outlives `ludion dev`,
/// however dev ends: the OS closes the pipe on every death.
#[cfg(feature = "wire")]
pub(crate) fn stdin_closed() -> crossbeam_channel::Receiver<()> {
    let (tx, rx) = crossbeam_channel::bounded::<()>(1);
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
        let _ = tx.try_send(());
    });
    rx
}

/// Prints the bound host's header, the moment after `Server::bind` succeeds — the one place either
/// shell writes it, so a human sees the same block whether or not a window opened.
///
/// The first line is the only machine-parseable one and is flushed on its own: `listening on
/// ws://<addr>`, which a test or agent parses to learn the port when `addr` asked for an OS-chosen
/// one (`:0`). The rest is the human block [`HostOptions`] selects.
#[cfg(feature = "wire")]
pub(crate) fn announce(server: &Server, opts: &HostOptions<'_>) {
    println!("listening on {}", server.url());
    let _ = std::io::stdout().flush();

    println!("engine version: {}", ludion_engine::ENGINE_VERSION);
    if opts.print_schema_hash {
        println!("schema hash: {}", ludion_protocol::schema::schema_hash());
    }
    println!("scene: {}", opts.scene.display());
    println!(
        "recording: {}",
        match opts.record_path {
            Some(path) => format!("armed -> {}", path.display()),
            None => "off".to_string(),
        }
    );
    if opts.window {
        println!("close the window or press Ctrl-C to stop");
    } else if opts.window_flag && cfg!(feature = "window") {
        println!("headless — pass --window to open a window; press Ctrl-C to stop");
    } else {
        println!("press Ctrl-C to stop");
    }
    let _ = std::io::stdout().flush();
}

/// Watches the project root for asset edits, forwarding each into the one intake queue as an
/// `asset.reload` — so a texture edited on disk reloads live, even while the engine is paused.
///
/// A watcher that fails to start degrades to explicit `asset.reload` commands with a warning, never
/// a dead host. The returned handle must stay in scope for as long as the server: its drop stops the
/// watch.
#[cfg(feature = "wire")]
pub(crate) fn watch_project(server: &Server, root: &Path) -> Option<ludion_assets::WatchHandle> {
    match server.watch_assets(root, ASSET_WATCH_DEBOUNCE) {
        Ok(handle) => Some(handle),
        Err(diagnostic) => {
            eprintln!(
                "{}",
                diagnostic.append_help(
                    "asset hot-reload is disabled for this run; `asset.reload` still works"
                )
            );
            None
        }
    }
}

/// The refusal a windowless build gives `--window`: the windowed shell is compiled out with the
/// `window` feature, so the flag cannot be honoured and the host does not start.
#[cfg(all(feature = "wire", not(feature = "window")))]
pub(crate) fn window_feature_off() -> Diagnostic {
    Diagnostic::new(
        code::WINDOW_FEATURE_OFF,
        "this build has no native window: `--window` needs the `window` cargo feature, which is off",
    )
    .with_help(
        "build the game with the `ludion` crate's default features (`window` is on by default), or \
         host without `--window` and attach a viewer to the listener instead",
    )
}

/// Installs the process's one Ctrl-C handler. `ctrlc` allows one handler per process, so every shell
/// — each facade host and `ludion dev` — installs exactly one, through here. The first press
/// `try_send`s a stop request to every target; a second exits the process at once (status 130), so a
/// host that will not stop never needs a kill from outside.
///
/// # Panics
///
/// When a handler is already installed.
pub fn ctrl_c_fan_out(targets: Vec<crossbeam_channel::Sender<()>>) {
    let pressed = AtomicBool::new(false);
    ctrlc::set_handler(move || {
        if second_press(&pressed) {
            eprintln!("Ctrl-C again: exiting without waiting for the host to stop");
            std::process::exit(130);
        }
        for target in &targets {
            let _ = target.try_send(());
        }
    })
    .expect("installing the Ctrl-C handler");
}

/// The stop channel every facade shell waits on: a capacity-1 receiver [`ctrl_c_fan_out`] signals.
pub(crate) fn ctrl_c_stop() -> crossbeam_channel::Receiver<()> {
    let (tx, rx) = crossbeam_channel::bounded::<()>(1);
    ctrl_c_fan_out(vec![tx]);
    rx
}

/// Records a Ctrl-C press; `true` when an earlier one already happened.
fn second_press(pressed: &AtomicBool) -> bool {
    pressed.swap(true, Ordering::Relaxed)
}

/// Wires `engine`'s audio-event sender to a playback sink rooted at `project_root` and returns the
/// sink, which the caller keeps in scope for the whole host (dropping it waits for its session to
/// end) and declares *before* the engine, so the engine's sender is gone by the time that wait runs.
///
/// The engine emits `AudioEvent`s on the channel's sending half; the sink resolves and plays them
/// under the content root, mixed down through the game's authored bus balance
/// (`EngineBuilder::category_gains`, carried on the engine) — so every host honours it without a
/// knob of its own. A device-less machine makes the sink silent, never a failure — CI serving stays
/// green. The channel is unbounded on purpose: the change-only protocol never resends a stop or a
/// sync, so a full channel would drop one for good, and the sink drains it in every state. The engine
/// never depends on the sink — a host is the one place they meet, and both hosts
/// meet them here so their wiring cannot drift. `caps` bounds the sink's voices ([`voice_caps`]).
pub(crate) fn wire_audio(
    engine: &mut Engine,
    project_root: PathBuf,
    caps: ludion_audio::VoiceCaps,
) -> ludion_audio::AudioSink {
    let gains = engine.category_gains();
    let (audio_tx, audio_rx) = crossbeam_channel::unbounded();
    engine.set_audio_sender(audio_tx);
    ludion_audio::AudioSink::spawn(
        audio_rx,
        project_root,
        gains,
        caps,
        engine.seconds_per_tick(),
    )
}

/// The voice caps a game at `scene_dir` plays under: the `[audio]` table of the nearest `ludion.toml`
/// at or above the scene's directory, the engine defaults when there is none. Every host boots from
/// a scene directory, which is the one anchor they share (a launch's content root is not a project
/// root).
///
/// # Errors
/// `L0023`/`L5004` when that manifest does not load: the game refuses to boot on a bad manifest.
pub(crate) fn voice_caps(
    scene_dir: &std::path::Path,
) -> Result<ludion_audio::VoiceCaps, ludion_core::diagnostics::Diagnostic> {
    match ludion_engine::manifest::find(scene_dir) {
        Some(project) => ludion_engine::manifest::load(&project)?.voice_caps(),
        None => Ok(ludion_audio::VoiceCaps::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_later_ctrl_c_press_is_a_second() {
        let pressed = AtomicBool::new(false);
        assert!(!second_press(&pressed));
        assert!(second_press(&pressed));
        assert!(second_press(&pressed));
    }

    fn project(name: &str, manifest: Option<&str>) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ludion-host-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scenes")).unwrap();
        if let Some(text) = manifest {
            std::fs::write(dir.join("ludion.toml"), text).unwrap();
        }
        dir
    }

    const HEAD: &str = "name = \"g\"
default_scene = \"scenes/main.yaml\"
";

    #[test]
    fn the_voice_caps_come_from_the_manifest_above_the_scene() {
        let dir = project(
            "caps",
            Some(&format!(
                "{HEAD}[audio]
voices_per_sound = 6
voices_total = 96
"
            )),
        );
        let caps = voice_caps(&dir.join("scenes")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            caps,
            ludion_audio::VoiceCaps {
                per_sound: 6,
                total: 96
            }
        );
    }

    #[test]
    fn a_bad_audio_table_refuses_the_boot() {
        let dir = project(
            "bad",
            Some(&format!(
                "{HEAD}[audio]
voices_total = 0
"
            )),
        );
        let err = voice_caps(&dir.join("scenes")).unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(err.code.as_str(), "L5004");
    }

    #[test]
    fn no_manifest_means_the_default_caps() {
        let dir = project("none", None);
        let caps = voice_caps(&dir.join("scenes")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(caps, ludion_audio::VoiceCaps::default());
    }
}
