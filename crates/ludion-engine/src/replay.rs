//! Replay playback: re-drive a fresh engine through a recorded session and expose its per-frame
//! `WorldHash` sequence.
//!
//! [`replay_hashes`] reads a replay's header, loads the scene it names (seeded from the header), and
//! replays every delta record through the **same dispatch path** the live session used — so a
//! recorded session and its replay produce an identical `WorldHash` sequence (Phase 3 exit criterion
//! 3). Recording and replay share one drive loop: replay simply reconstructs the command list from
//! the records and feeds it to that loop, so there is no second, drifting reproduction of the
//! stepping logic. The loop calls a hook at every *settled* frame (all actions at it applied);
//! [`replay_with`], the hash trace and the frame renderer are all hooks on it.
//!
//! A plain drive never decodes keyframe records; the bisect paths keep them in the action list as
//! the alignment points a keyframe verifier compares at. Seeks use `ludion-protocol`'s [`ReplayReader::seek`].

use std::collections::BTreeMap;
use std::ops::ControlFlow;
use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::FrameIndex;
use ludion_protocol::command::{InputInjectParams, RenderSizeParams, SimStepParams};
use ludion_protocol::replay::{ReplayHeader, ReplayReader, ReplayRecord, SceneRef};
use ludion_protocol::{Command, PROTOCOL_VERSION, schema};
use ludion_sim::WorldHash;
use serde_json::Value;

use crate::builder::EngineBuilder;
use crate::commands::{collapse, render};
use crate::engine::Engine;

/// Applies `commands` to `engine` in order, returning the per-frame `WorldHash` trace.
///
/// The trace holds one `(frame, hash)` per *settled* frame: the world after the frame's tick and
/// every non-step command applied at it. A `sim.step` is decomposed into single ticks, so the trace
/// is genuinely per-frame.
///
/// This is used both to *drive* a live/recording session and, via [`replay_hashes`], to reproduce
/// one — the single source of the stepping-and-hashing logic.
///
/// # Errors
/// Any dispatch error — every system panic (`L4007`) included, since a live drive has no recorded
/// panics — or a `WorldHash` computation failure (`L0007`).
///
/// # Example
/// ```
/// use ludion_engine::{drive_hashes, Engine};
/// use ludion_protocol::Command;
/// use ludion_protocol::command::SimStepParams;
///
/// let mut engine = Engine::new();
/// engine.load_inline_scene("format: 1\nseed: 1\nentities: []\n").unwrap();
/// // Two single-tick steps → a per-frame hash at frames 0, 1, and 2.
/// let trace = drive_hashes(&mut engine, vec![
///     Command::SimStep(SimStepParams { ticks: 2, hash: false }),
/// ]).unwrap();
/// assert_eq!(trace.len(), 3);
/// assert_eq!(trace[2].0.as_u64(), 2);
/// ```
pub fn drive_hashes(
    engine: &mut Engine,
    commands: Vec<Command>,
) -> Result<Vec<(FrameIndex, WorldHash)>, Diagnostic> {
    drive_actions(
        engine,
        commands.into_iter().map(ReplayAction::Command).collect(),
    )
}

/// One step of a replay drive: a protocol command, or a recorded game data-doc reload or scene load
/// reapplied from its inline text. A live/recording session only ever produces
/// [`ReplayAction::Command`]s (that is what [`drive_hashes`] wraps); a *replay* also reconstructs
/// [`ReplayAction::DocReload`] and [`ReplayAction::SceneLoad`] from their records (see
/// [`replay_hashes_with`]), so both feed the one stepping-and-hashing core ([`drive_actions`]) and
/// cannot drift.
pub(crate) enum ReplayAction {
    /// A protocol command applied at the current frame.
    Command(Command),
    /// A recorded doc reload reapplied at the current frame from its inline canonical text — no
    /// on-disk read, so it reproduces on a checkout whose doc differs (decision 9).
    DocReload {
        kind: String,
        path: String,
        document: String,
    },
    /// A recorded prefab reload reapplied at the current frame from its inline text — the library
    /// swaps the entry without reading the disk.
    PrefabReload { path: String, document: String },
    /// A recorded mid-session `scene.load` reapplied from the text it resolved — never the path, so
    /// it reproduces on a checkout whose scene file differs or is gone. `frame` and `path` only
    /// name the record in a failure.
    SceneLoad {
        frame: FrameIndex,
        path: String,
        document: String,
    },
    /// A recorded `render.size` reapplied with the validity check alone: the render scale that
    /// [`Command::RenderSize`] also checks is machine-local, so it must not refuse recorded input.
    /// `frame` only names the record in a failure.
    RenderSize {
        frame: FrameIndex,
        params: RenderSizeParams,
    },
    /// A recorded `sim.step` whose last tick, at `frame`, panicked live: the replay expects that
    /// `L4007` there and nowhere else.
    PanickedStep {
        frame: FrameIndex,
        params: SimStepParams,
    },
    /// A recorded keyframe, kept **in record order** so a verifier knows exactly when the live state
    /// is supposed to equal it: the keyframe was captured right after the preceding delta record was
    /// written, so applying the actions up to here reproduces the snapshotted state. Inert for a
    /// plain drive (a linear replay reconstructs from the deltas, never from a keyframe). Present
    /// only when the caller kept keyframes.
    Keyframe { frame: FrameIndex, snapshot: Value },
}

/// What a drive does at the boundaries a plain replay ignores. The default implementations *are* the
/// plain replay, so [`PlainDrive`] is the whole of a plain drive; an instrumented caller overrides
/// only the boundary it cares about and cannot drift from the shared stepping logic.
pub(crate) trait DriveHook {
    /// Called at each settled frame — before the tick that leaves it, and once when the drive ends —
    /// with every action at that frame applied. Returning `false` ends the drive here.
    fn settled(&mut self, _engine: &mut Engine) -> Result<bool, Diagnostic> {
        Ok(true)
    }

    /// Called at each recorded keyframe, in record order, with the engine at the state the keyframe
    /// captured. Returning `false` ends the drive here.
    fn keyframe(
        &mut self,
        _engine: &Engine,
        _frame: FrameIndex,
        _snapshot: &Value,
    ) -> Result<bool, Diagnostic> {
        Ok(true)
    }

    /// Performs one tick. Returning `false` ends the drive after it. The default is the single
    /// `sim.step` a replay drives with.
    fn tick(&mut self, engine: &mut Engine) -> Result<bool, Diagnostic> {
        engine.execute(Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        }))?;
        Ok(true)
    }
}

/// The plain replay drive: every hook at its default.
pub(crate) struct PlainDrive;
impl DriveHook for PlainDrive {}

/// A recorded marked tick that the replay ticked without a panic: the build no longer panics where
/// the recorded session did.
fn panic_not_reproduced(frame: FrameIndex) -> Diagnostic {
    Diagnostic::new(
        code::REPLAY_PANIC_NOT_REPRODUCED,
        format!(
            "the recording marks a system panic on frame {}'s tick, but the replay ticked it \
             without one",
            frame.as_u64()
        ),
    )
    .with_help(
        "the build no longer panics where the recorded session did, so the replay is not that \
         session; re-record it with this build",
    )
}

/// Drives one `sim.step`'s ticks through `hook`. `expected_panic` is the frame a recording marks as
/// panicking: the `L4007` of that step's last tick is the reproduction, and every other `L4007` — an
/// earlier tick, an unmarked step, a live drive — propagates. Returns `None` to carry on, or the
/// answer [`run_actions`] must return when a hook ended the drive.
fn run_step(
    engine: &mut Engine,
    params: &SimStepParams,
    expected_panic: Option<FrameIndex>,
    hook: &mut dyn DriveHook,
) -> Result<Option<bool>, Diagnostic> {
    for tick in 0..params.ticks {
        if !hook.settled(engine)? {
            return Ok(Some(false));
        }
        let expected = expected_panic.filter(|_| tick + 1 == params.ticks);
        match hook.tick(engine) {
            // The hook ended the drive; it may not have ticked at all.
            Ok(false) => return Ok(Some(true)),
            Ok(true) => {
                if let Some(frame) = expected {
                    return Err(panic_not_reproduced(frame));
                }
            }
            Err(diagnostic) if expected.is_some() && diagnostic.code == code::SYSTEM_PANICKED => {}
            Err(diagnostic) => return Err(diagnostic),
        }
    }
    Ok(None)
}

/// The per-frame `WorldHash` drive behind [`drive_hashes`] and [`replay_hashes_with`]: the one drive
/// loop ([`drive_actions_with`]) with a [`HashTrace`] sampling every settled frame.
pub(crate) fn drive_actions(
    engine: &mut Engine,
    actions: Vec<ReplayAction>,
) -> Result<Vec<(FrameIndex, WorldHash)>, Diagnostic> {
    let mut trace = HashTrace::default();
    drive_actions_with(engine, actions, &mut trace)?;
    Ok(trace.hashes)
}

/// Samples `(frame, WorldHash)` at every settled frame.
#[derive(Default)]
struct HashTrace {
    hashes: Vec<(FrameIndex, WorldHash)>,
}

impl DriveHook for HashTrace {
    fn settled(&mut self, engine: &mut Engine) -> Result<bool, Diagnostic> {
        self.hashes.push((engine.frame(), engine.world_hash()?));
        Ok(true)
    }
}

/// The single drive loop: a `sim.step` decomposes into single ticks, every other action applies at the
/// current frame. `hook` decides what happens at each settled frame, keyframe and tick; the default
/// hook is the plain replay, so an instrumented drive cannot step differently from a plain one.
pub(crate) fn drive_actions_with(
    engine: &mut Engine,
    actions: Vec<ReplayAction>,
    hook: &mut dyn DriveHook,
) -> Result<(), Diagnostic> {
    if run_actions(engine, actions, hook)? {
        hook.settled(engine)?;
    }
    Ok(())
}

/// Runs the actions; returns `false` when the settled hook itself asked to stop (the drive has ended
/// and must not announce a final settled frame again).
fn run_actions(
    engine: &mut Engine,
    actions: Vec<ReplayAction>,
    hook: &mut dyn DriveHook,
) -> Result<bool, Diagnostic> {
    for action in actions {
        match action {
            ReplayAction::Command(Command::SimStep(params)) => {
                if let Some(done) = run_step(engine, &params, None, hook)? {
                    return Ok(done);
                }
            }
            ReplayAction::PanickedStep { frame, params } => {
                if let Some(done) = run_step(engine, &params, Some(frame), hook)? {
                    return Ok(done);
                }
            }
            ReplayAction::Command(other) => {
                engine.execute(other)?;
            }
            ReplayAction::DocReload {
                kind,
                path,
                document,
            } => {
                engine.apply_doc_reload_inline(&kind, &path, &document)?;
            }
            ReplayAction::PrefabReload { path, document } => {
                engine.apply_prefab_reload_inline(&path, &document)?;
            }
            ReplayAction::SceneLoad {
                frame,
                path,
                document,
            } => {
                apply_scene_load(engine, frame, &path, &document)?;
            }
            ReplayAction::RenderSize { frame, params } => {
                apply_render_size(engine, frame, &params)?;
            }
            ReplayAction::Keyframe { frame, snapshot } => {
                if !hook.keyframe(engine, frame, &snapshot)? {
                    return Ok(true);
                }
            }
        }
    }
    Ok(true)
}

/// Replays the recorded session in `bytes`, returning its per-frame `WorldHash` trace.
///
/// Loads the scene from the header (an inline document loads verbatim, so the replay is
/// self-contained and portable; a path resolves against `project_root`), seeds from the header, then
/// replays every delta through the drive loop. Compare the returned trace to the original session's
/// trace (also from [`drive_hashes`]) for an exact round-trip check.
///
/// # Errors
/// - `L3010`/`L3011` if the replay bytes are malformed (from [`ReplayReader`]).
/// - `L3010` if the replay was written by an incompatible protocol version or schema.
/// - `L3013` if the header names a scene by path that cannot be loaded under `project_root`.
/// - Any scene-load or dispatch diagnostic encountered while replaying.
pub fn replay_hashes(
    bytes: &[u8],
    project_root: impl AsRef<Path>,
) -> Result<Vec<(FrameIndex, WorldHash)>, Diagnostic> {
    replay_hashes_with(Engine::builder(), bytes, project_root)
}

/// Replays a recorded session against a *game* engine, returning its per-frame `WorldHash` trace.
///
/// Identical to [`replay_hashes`] except the engine is built from `builder` (the game's registry and
/// systems) rather than the bare built-in engine. This is the seam a game recording needs: its scene
/// names custom components, so it can only be re-instantiated by an engine that registers them, and
/// its ticks only reproduce if the same systems run. Recording, replay, and `WorldHash` are otherwise
/// entirely registry-driven, so no other wiring is required.
///
/// The `builder`'s project root is overridden with `project_root` (the replay file's own directory,
/// like [`replay_hashes`]); its registry and systems are what matter here. Pass the *same* builder
/// configuration the session was recorded with — a fresh one each call, since building consumes it.
///
/// # Errors
/// A registration [`Diagnostic`], plus every error [`replay_hashes`] can raise.
pub fn replay_hashes_with(
    builder: EngineBuilder,
    bytes: &[u8],
    project_root: impl AsRef<Path>,
) -> Result<Vec<(FrameIndex, WorldHash)>, Diagnostic> {
    let (mut engine, actions) = prepare_replay(builder, bytes, project_root, Keyframes::Skip)?;
    drive_actions(&mut engine, actions)
}

/// Drives a recorded session against a *game* engine, calling `on_frame` at every settled frame, and
/// hands back the driven [`Engine`] — the seam for reading game state (a component, a resource) off
/// the end of a replay, or off any frame of it.
///
/// A frame is *settled* once every action recorded at it has applied: `on_frame` runs right before
/// the tick that leaves the frame, and once more when the drive ends, so it sees every frame from the
/// initial post-load world through the last. Returning `ControlFlow::Break(())` ends the drive early;
/// the engine is returned at that frame. To only want the end state, pass
/// `|_, _| Ok(ControlFlow::Continue(()))`.
///
/// The observer gets `&mut Engine` (rendering needs it), but mutating sim state there diverges the
/// replay from the recording. Setup is that of [`replay_hashes_with`]: pass the same builder
/// configuration the session was recorded with.
///
/// # Errors
/// Every error [`replay_hashes_with`] can raise, and any error `on_frame` returns (the drive stops
/// there and the engine is dropped).
///
/// # Example
/// ```
/// use std::ops::ControlFlow;
/// use ludion_engine::{drive_hashes, replay_with, Engine};
/// use ludion_protocol::Command;
/// use ludion_protocol::command::SimStepParams;
///
/// // Record two ticks of an inline scene into memory.
/// let sink = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
/// struct Sink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
/// impl std::io::Write for Sink {
///     fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
///         self.0.lock().unwrap().extend_from_slice(buf);
///         Ok(buf.len())
///     }
///     fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
/// }
/// let mut live = Engine::new();
/// live.load_inline_scene("format: 1\nseed: 1\nentities: []\n").unwrap();
/// live.set_recording(Sink(sink.clone())).unwrap();
/// drive_hashes(&mut live, vec![Command::SimStep(SimStepParams { ticks: 2, hash: false })]).unwrap();
/// live.stop_recording().unwrap();
/// let bytes = sink.lock().unwrap().clone();
///
/// // Replay it and read the final frame off the returned engine.
/// let engine = replay_with(Engine::builder(), &bytes, ".", |_, _| Ok(ControlFlow::Continue(())))
///     .unwrap();
/// assert_eq!(engine.frame().as_u64(), 2);
/// ```
pub fn replay_with<F>(
    builder: EngineBuilder,
    bytes: &[u8],
    project_root: impl AsRef<Path>,
    on_frame: F,
) -> Result<Engine, Diagnostic>
where
    F: FnMut(&mut Engine, FrameIndex) -> Result<ControlFlow<()>, Diagnostic>,
{
    let (mut engine, actions) = prepare_replay(builder, bytes, project_root, Keyframes::Skip)?;
    drive_actions_with(&mut engine, actions, &mut Observer(on_frame))?;
    Ok(engine)
}

/// Adapts [`replay_with`]'s closure to the settled boundary.
struct Observer<F>(F);

impl<F> DriveHook for Observer<F>
where
    F: FnMut(&mut Engine, FrameIndex) -> Result<ControlFlow<()>, Diagnostic>,
{
    fn settled(&mut self, engine: &mut Engine) -> Result<bool, Diagnostic> {
        let frame = engine.frame();
        Ok((self.0)(engine, frame)?.is_continue())
    }
}

/// A replay-to-frames render request: the inclusive frame window and the output resolution passed to
/// [`replay_render_frames`].
#[derive(Clone, Debug)]
pub struct ReplayFramesRequest {
    /// First sim frame to render (inclusive). Frame 0 is the initial post-load world, before any
    /// `sim.step`.
    pub from: u64,
    /// Last sim frame to render (inclusive). `None` renders through the replay's final frame (the sum
    /// of every recorded `sim.step`'s ticks).
    pub to: Option<u64>,
    /// Render width in pixels.
    pub width: u32,
    /// Render height in pixels.
    pub height: u32,
    /// Debug-draw categories drawn over every rendered frame; `DebugCategories::None` renders
    /// byte-identically to an engine without debug draw.
    pub debug: crate::DebugCategories,
}

/// What [`replay_render_frames`] did: the frames it rendered and the debug categories that drew.
#[derive(Clone, Debug, Default)]
pub struct ReplayFramesOutcome {
    /// Frames rendered and handed to the sink.
    pub rendered: u64,
    /// The categories that drew on any rendered frame, whether enabled or not — sorted and
    /// de-duplicated, the input `DebugCategories::summary` expects.
    pub debug_drawn: Vec<&'static str>,
}

/// The resolved (non-optional) window a [`ReplayFramesRequest`] drives against, plus its resolution.
#[derive(Clone, Copy)]
struct RenderWindow {
    from: u64,
    to: u64,
    width: u32,
    height: u32,
}

/// The per-frame sink [`replay_render_frames`] hands each rendered PNG to: `(frame_index, png_bytes)`,
/// called once per rendered frame in ascending order. Aliased so the drive helpers can name it without
/// tripping `clippy::type_complexity`.
type FrameSink<'a> = dyn FnMut(u64, &[u8]) -> Result<(), Diagnostic> + 'a;

/// Drives a recorded `.lrp` replay frame-by-frame against a *game* engine and renders each stepped sim
/// frame in the `[from, to]` window (inclusive) to a PNG, handing the bytes to `on_frame` — the
/// offline batch render behind `cargo run -p <game> -- --replay-frames …`, from which ffmpeg
/// assembles a clip.
///
/// The sim always drives from frame 0 (replay state is cumulative), so a `from` past 0 costs the same
/// setup — only the render is skipped below the window. Frame naming and disk I/O are the caller's
/// policy: `on_frame(frame_index, png_bytes)` is invoked once per rendered frame, in ascending frame
/// order. Rendering needs a GPU adapter; on an adapterless machine the first render returns `L2003`
/// and the caller graceful-skips exactly as golden tests do.
///
/// The engine is built through `prepare_replay`, so it never enables profile persistence — an
/// offline render can no more clobber the developer's `save/profile.yaml` than a plain replay can. The
/// header's embedded profile-at-start is installed, so the clip reproduces bit-exactly regardless of
/// the on-disk save.
///
/// Returns a [`ReplayFramesOutcome`]: the number of frames rendered (and handed to `on_frame`) and
/// the debug-draw categories that drew on them.
///
/// # Errors
/// - `L3021` if the window falls outside the replay's recorded range (`from > to`, or `to` past the
///   final frame).
/// - `L2003` (and other render diagnostics) surfaced from [`Engine::render_png`].
/// - Every error [`replay_hashes_with`] can raise (a malformed `.lrp`, an unresolved scene, a
///   dispatch failure while driving).
///
/// # Example
/// ```no_run
/// use ludion_engine::{replay_render_frames, DebugCategories, Engine, ReplayFramesRequest};
///
/// let bytes = std::fs::read("run.lrp").unwrap();
/// let request = ReplayFramesRequest {
///     from: 0,
///     to: Some(60),
///     width: 1920,
///     height: 1080,
///     debug: DebugCategories::None,
/// };
/// // A game passes its own `Engine::builder()...` here; the built-in engine stands in for the doc.
/// let outcome = replay_render_frames(Engine::builder(), &bytes, ".", request, |frame, png| {
///     std::fs::write(format!("frame-{frame:05}.png"), png)
///         .map_err(|e| ludion_core::diagnostics::Diagnostic::new(
///             ludion_core::codes::code::RENDER_PNG_IO, e.to_string()))
/// }).unwrap();
/// eprintln!("rendered {} frames", outcome.rendered);
/// ```
pub fn replay_render_frames<F>(
    builder: EngineBuilder,
    bytes: &[u8],
    project_root: impl AsRef<Path>,
    request: ReplayFramesRequest,
    mut on_frame: F,
) -> Result<ReplayFramesOutcome, Diagnostic>
where
    F: FnMut(u64, &[u8]) -> Result<(), Diagnostic>,
{
    let (mut engine, actions) = prepare_replay(builder, bytes, project_root, Keyframes::Skip)?;
    engine.set_debug_draw(request.debug.clone());

    require_monotonic_frames("replay-to-frames", &actions)?;
    let total = total_frames(&actions)?;
    let to = resolve_window("replay-to-frames", request.from, request.to, total)?;

    let window = RenderWindow {
        from: request.from,
        to,
        width: request.width,
        height: request.height,
    };
    let mut hook = RenderHook {
        window,
        on_frame: &mut on_frame,
        rendered: 0,
        last_rendered: None,
        drawn: Vec::new(),
    };
    drive_actions_with(&mut engine, actions, &mut hook)?;
    let mut debug_drawn = hook.drawn;
    debug_drawn.sort_unstable();
    debug_drawn.dedup();
    Ok(ReplayFramesOutcome {
        rendered: hook.rendered,
        debug_drawn,
    })
}

/// The action a recorded delta command replays as: a `render.size` skips the host's render-scale
/// check, everything else goes back through dispatch.
pub(crate) fn command_action(frame: FrameIndex, command: Command) -> ReplayAction {
    match command {
        Command::RenderSize(params) => ReplayAction::RenderSize { frame, params },
        other => ReplayAction::Command(other),
    }
}

/// Reapplies a recorded `scene.load` from its inline text; a failure names the record it came from.
fn apply_scene_load(
    engine: &mut Engine,
    frame: FrameIndex,
    path: &str,
    document: &str,
) -> Result<(), Diagnostic> {
    engine
        .load_inline_scene(document)
        .map(drop)
        .map_err(|diagnostics| {
            let mut diagnostic = collapse(diagnostics);
            diagnostic.message = format!(
                "replayed SceneLoad record at frame {} (`{path}`): {}",
                frame.as_u64(),
                diagnostic.message
            );
            diagnostic
        })
}

/// Reapplies a recorded `render.size` without the render-scale check; a failure names its frame.
fn apply_render_size(
    engine: &mut Engine,
    frame: FrameIndex,
    params: &RenderSizeParams,
) -> Result<(), Diagnostic> {
    render::apply_recorded(engine, params).map_err(|mut diagnostic| {
        diagnostic.message = format!(
            "replayed render.size record at frame {}: {}",
            frame.as_u64(),
            diagnostic.message
        );
        diagnostic
    })
}

/// Resolves an inclusive `[from, to]` frame window against a replay recording frames `0..=total`,
/// returning the concrete `to` (`total` when omitted). `label` names the mode in the `L3021` message.
pub(crate) fn resolve_window(
    label: &str,
    from: u64,
    to: Option<u64>,
    total: u64,
) -> Result<u64, Diagnostic> {
    let to = to.unwrap_or(total);
    if from > to {
        return Err(Diagnostic::new(
            code::REPLAY_FRAME_WINDOW,
            format!(
                "{label} was asked for the empty window `--from {from} --to {to}` (from is past to)"
            ),
        )
        .with_help(
            "pass a `--from` no greater than `--to`; the window is inclusive on both ends",
        ));
    }
    if to > total {
        return Err(Diagnostic::new(
            code::REPLAY_FRAME_WINDOW,
            format!("{label} was asked for frame {to}, but this replay records frames 0..={total}"),
        )
        .with_help(format!(
            "request a `--to` no greater than {total} (or omit `--to` to render through the final \
             frame)"
        )));
    }
    Ok(to)
}

/// The replay's final frame index: the sum of every recorded `sim.step`'s ticks. Frame states run
/// `0..=total`, matching the per-frame hash trace [`drive_actions`] produces.
///
/// # Errors
/// `L3010` if the records total more ticks than a frame index can hold.
pub(crate) fn total_frames(actions: &[ReplayAction]) -> Result<u64, Diagnostic> {
    actions.iter().try_fold(0u64, |total, action| match action {
        ReplayAction::Command(Command::SimStep(params))
        | ReplayAction::PanickedStep { params, .. } => {
            total.checked_add(params.ticks).ok_or_else(|| {
                tick_overflow(
                    "replay sim.step records total more ticks than a frame index can hold \
                     (overflow past u64::MAX)"
                        .to_owned(),
                )
            })
        }
        _ => Ok(total),
    })
}

/// The `L3010` for tick arithmetic that overflows `u64`: a corrupt or hand-written recording.
fn tick_overflow(message: String) -> Diagnostic {
    Diagnostic::new(code::REPLAY_FORMAT, message).with_help(
        "the file is corrupt or was not written by the engine's recorder; re-record the session",
    )
}

/// Refuses a replay whose drive actions reset or jump the sim frame, which makes a frame window or a
/// divergence frame ambiguous: a `scene.load` after any tick (a load before the first tick resets
/// 0 → 0), or an `engine.restore` anywhere. `label` names the mode in the `L3021` message.
///
/// # Errors
/// `L3021` naming the first offending action.
pub(crate) fn require_monotonic_frames(
    label: &str,
    actions: &[ReplayAction],
) -> Result<(), Diagnostic> {
    let mut ticks = 0u64;
    // Numbered like the recording's drive actions: keyframes are not actions a drive performs.
    let drive_actions = actions
        .iter()
        .filter(|action| !matches!(action, ReplayAction::Keyframe { .. }));
    for (index, action) in drive_actions.enumerate() {
        let what = match action {
            ReplayAction::Command(Command::SimStep(params))
            | ReplayAction::PanickedStep { params, .. } => {
                ticks = ticks.saturating_add(params.ticks);
                continue;
            }
            ReplayAction::SceneLoad { frame, path, .. } if ticks > 0 => format!(
                "a `scene.load` of `{path}` recorded at frame {}",
                frame.as_u64()
            ),
            ReplayAction::Command(Command::SceneLoad(params)) if ticks > 0 => {
                format!("a `scene.load` of `{}`", params.path)
            }
            ReplayAction::Command(Command::EngineRestore(_)) => "an `engine.restore`".to_owned(),
            _ => continue,
        };
        return Err(Diagnostic::new(
            code::REPLAY_FRAME_WINDOW,
            format!(
                "{label} cannot use this replay: drive action #{} ({what}) after {ticks} recorded \
                 ticks resets the sim frame, so frame numbers repeat or jump and a frame window or \
                 divergence frame is ambiguous",
                index + 1
            ),
        )
        .with_help(
            "window or bisect a recording that has no engine.restore and no scene.load after its \
             first tick (record the session after the load), or replay it whole with \
             --replay-verify",
        ));
    }
    Ok(())
}

/// Renders each settled frame in `window` through the one drive loop. Frames are monotonic —
/// [`require_monotonic_frames`] refuses a replay that resets or jumps the frame — so once the sim
/// passes `to` the drive stops early; `last_rendered` keeps the end-of-drive call from
/// rendering a frame the last tick's settled call already did.
struct RenderHook<'a, 'b> {
    window: RenderWindow,
    on_frame: &'a mut FrameSink<'b>,
    rendered: u64,
    last_rendered: Option<u64>,
    drawn: Vec<&'static str>,
}

impl DriveHook for RenderHook<'_, '_> {
    fn settled(&mut self, engine: &mut Engine) -> Result<bool, Diagnostic> {
        let frame = engine.frame().as_u64();
        if frame > self.window.to {
            return Ok(false);
        }
        if frame < self.window.from || self.last_rendered == Some(frame) {
            return Ok(true);
        }
        let png = engine.render_png(self.window.width, self.window.height)?;
        (self.on_frame)(frame, &png)?;
        self.drawn.extend(engine.debug_draw_drawn());
        self.rendered += 1;
        self.last_rendered = Some(frame);
        Ok(true)
    }
}

/// Whether [`prepare_replay`] decodes the replay's keyframes into [`ReplayAction::Keyframe`]s. Only
/// the bisect paths consume them; a plain drive never decompresses a keyframe body.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Keyframes {
    Keep,
    Skip,
}

/// Builds the game engine from `builder`, loads the replay's scene/seed/profile, and reconstructs the
/// drive actions from its records — everything [`replay_hashes_with`] does *up to* driving. Split out
/// so a caller that needs to instrument the drive (the query-drift verifier arms recording between
/// this and [`drive_actions`]) shares the exact same setup and cannot drift from a plain replay.
///
/// # Errors
/// Every error [`replay_hashes_with`] can raise.
pub(crate) fn prepare_replay(
    builder: EngineBuilder,
    bytes: &[u8],
    project_root: impl AsRef<Path>,
    keyframes: Keyframes,
) -> Result<(Engine, Vec<ReplayAction>), Diagnostic> {
    let reader = ReplayReader::new(bytes)?;
    check_compatible(reader.header())?;

    let mut engine = builder
        .replay_profile()
        .build_at(project_root.as_ref().to_path_buf())?;
    // The extent is sim input (a game unprojects the cursor through it), so it is installed before the
    // scene loads — with `render.size`'s validity check, which refuses a hostile header value, but not
    // the host's render-scale check. A header without one is a pre-field `.lrp`, recorded at the
    // default extent.
    if let Some(extent) = &reader.header().render_extent {
        let params = RenderSizeParams {
            width: Some(extent.width),
            height: Some(extent.height),
        };
        render::apply_recorded(&mut engine, &params).map_err(|mut diagnostic| {
            diagnostic.message = format!("replay header render_extent: {}", diagnostic.message);
            if let Some(path) = &diagnostic.path {
                diagnostic.path = Some(path.replace("/params/", "/render_extent/"));
            }
            diagnostic
        })?;
    }
    // Reconstruct the action list from the delta records; `Keep` also yields keyframe actions, the
    // verifier's alignment marker.
    // A `SceneLoad` record replays from its inline text, never its path. `SceneSwitch` records are
    // not drive actions — the switch *regenerates* from the sim's re-set request under replay (same
    // seed/inputs/profile → same frame). They are pre-scanned into an off-disk scene-text map
    // instead, so the regenerated drain resolves its target from the replay, never disk
    // (cross-machine purity, decision 6). Always installed, even empty, as the prefab texts are: a
    // replay never reads a switch target off disk.
    let mut actions = Vec::new();
    let mut scene_text: BTreeMap<String, String> = BTreeMap::new();
    let mut prefabs = PrefabScan::default();
    // The last tick of the `sim.step` record just before this one — what a `SystemPanic` must name.
    let mut step_end: Option<FrameIndex> = None;
    let records = match keyframes {
        Keyframes::Keep => reader.records()?,
        Keyframes::Skip => reader.deltas()?,
    };
    for record in records {
        actions.extend(prefabs.observe(&record));
        let prior_step_end = step_end.take();
        match record {
            ReplayRecord::Command { frame, command } => {
                if let Command::SimStep(params) = &command
                    && params.ticks > 0
                {
                    let last = frame
                        .as_u64()
                        .checked_add(params.ticks - 1)
                        .ok_or_else(|| {
                            tick_overflow(format!(
                                "replay sim.step record at frame {} runs past the largest frame \
                                 index (overflow past u64::MAX)",
                                frame.as_u64()
                            ))
                        })?;
                    step_end = Some(FrameIndex::new(last));
                }
                actions.push(command_action(frame, command));
            }
            ReplayRecord::Input { frame, event } => {
                actions.push(ReplayAction::Command(Command::InputInject(
                    InputInjectParams {
                        frame: Some(frame),
                        event,
                    },
                )));
            }
            ReplayRecord::DocReload {
                doc_kind,
                path,
                document,
                ..
            } => actions.push(ReplayAction::DocReload {
                kind: doc_kind,
                path,
                document,
            }),
            ReplayRecord::SceneLoad {
                frame,
                path,
                document,
            } => {
                actions.push(ReplayAction::SceneLoad {
                    frame,
                    path,
                    document,
                });
            }
            ReplayRecord::SystemPanic { frame } => {
                let params = match actions.last() {
                    Some(ReplayAction::Command(Command::SimStep(params)))
                        if prior_step_end == Some(frame) =>
                    {
                        params.clone()
                    }
                    _ => {
                        return Err(Diagnostic::new(
                            code::REPLAY_FORMAT,
                            format!(
                                "replay system_panic record at frame {} follows no sim.step \
                                 record ending on that frame",
                                frame.as_u64()
                            ),
                        )
                        .with_help(
                            "the file was not written by the engine's recorder; re-record the \
                             session",
                        ));
                    }
                };
                actions.pop();
                actions.push(ReplayAction::PanickedStep { frame, params });
            }
            // Present only under `Keyframes::Keep`, in record order: the alignment marker a keyframe
            // verifier needs (`crate::bisect`) — the state right here is what the keyframe captured.
            ReplayRecord::Keyframe { frame, snapshot } => {
                actions.push(ReplayAction::Keyframe { frame, snapshot });
            }
            // Pre-scan the target text (last write wins if a district is visited more than once); the
            // switch itself regenerates, so this record drives nothing.
            ReplayRecord::SceneSwitch {
                scene_path,
                document,
                ..
            } => {
                scene_text.insert(scene_path, document);
            }
            // Filed or driven by `PrefabScan` above.
            ReplayRecord::PrefabText { .. } => {}
        }
    }
    engine.install_replay_scene_text(scene_text);
    // Always installed, even empty: a replay never reads a prefab off disk — the header scene's own
    // instances included, so the texts are armed before it loads.
    engine.arm_replay_prefab_text(prefabs.reads);

    load_header_scene(&mut engine, reader.header())?;
    engine.reseed(reader.header().seed);

    // Install the header's embedded profile-at-start, overriding whatever the boot disk-load put in
    // place — so a committed `.lrp` reproduces bit-exactly on a machine whose `save/profile.yaml`
    // differs or is absent (Phase 16 / E5, decision 6). A header without one (an old replay or a
    // profile-free game) leaves the boot profile untouched.
    if let Some(profile) = &reader.header().profile {
        engine.install_profile_from_text(profile);
    }

    Ok((engine, actions))
}

/// Files a recording's prefab texts per rebuild epoch and turns its reload records into drive actions.
#[derive(Default)]
pub(crate) struct PrefabScan {
    /// The epoch the records scanned so far are in; after a full scan, the last one.
    pub(crate) epoch: u32,
    /// The read texts, keyed by `(epoch, path)`.
    pub(crate) reads: crate::prefab::PrefabTextSource,
}

impl PrefabScan {
    /// Call for every record in stream order; returns the action a reload record drives.
    pub(crate) fn observe(&mut self, record: &ReplayRecord) -> Option<ReplayAction> {
        if starts_epoch(record) {
            self.epoch += 1;
        }
        let ReplayRecord::PrefabText {
            path,
            document,
            reload,
            ..
        } = record
        else {
            return None;
        };
        if *reload {
            return Some(ReplayAction::PrefabReload {
                path: path.clone(),
                document: document.clone(),
            });
        }
        self.reads
            .entry((self.epoch, path.clone()))
            .or_insert_with(|| document.clone());
        None
    }
}

/// A record whose replay rebuilds the world — and with it the prefab library: a restore, a load or
/// a switch.
pub(crate) fn starts_epoch(record: &ReplayRecord) -> bool {
    match record {
        ReplayRecord::Command { command, .. } => matches!(command, Command::EngineRestore(_)),
        ReplayRecord::SceneLoad { .. } | ReplayRecord::SceneSwitch { .. } => true,
        ReplayRecord::Input { .. }
        | ReplayRecord::Keyframe { .. }
        | ReplayRecord::DocReload { .. }
        | ReplayRecord::PrefabText { .. }
        | ReplayRecord::SystemPanic { .. } => false,
    }
}

/// Loads the header's scene into `engine`.
///
/// Recordings this engine makes embed the scene inline ([`SceneRef::Inline`]) — the scene text
/// travels in the replay, so it loads verbatim with no filesystem dependency and the replay is
/// portable. A [`SceneRef::Path`] header (a hand-authored or externally produced replay) is still
/// honored: the path resolves against the engine's project root (the replay file's own directory, set
/// by the caller), so it follows a moved tree — but a path that cannot be loaded is reported as
/// [`code::REPLAY_SCENE_UNRESOLVED`], distinguishing "the replay's external scene is missing" from a
/// live `scene.load` typo. A path-named scene holding prefab instances cannot replay: its prefab
/// texts were never recorded, so the library refuses them (`not recorded`) instead of reading disk.
fn load_header_scene(engine: &mut Engine, header: &ReplayHeader) -> Result<(), Diagnostic> {
    match &header.scene {
        SceneRef::Path { path } => engine
            .execute(Command::SceneLoad(
                ludion_protocol::command::SceneLoadParams { path: path.clone() },
            ))
            .map(|_| ())
            .map_err(|error| {
                Diagnostic::new(
                    code::REPLAY_SCENE_UNRESOLVED,
                    format!(
                        "replay names scene `{path}` by path, which could not be loaded: {}",
                        error.message
                    ),
                )
                .with_help(
                    "place the scene file beside the replay (matching the header's relative path), \
                     or re-record the session so the scene is embedded inline and the replay is \
                     self-contained (a path-named scene holding prefab instances cannot replay: \
                     its prefab texts were never recorded)",
                )
            }),
        SceneRef::Inline { document } => {
            engine.load_inline_scene(document).map_err(collapse)?;
            Ok(())
        }
    }
}

/// Rejects a replay from an incompatible protocol version or schema (drift detection, like the
/// handshake's schema hash).
fn check_compatible(header: &ReplayHeader) -> Result<(), Diagnostic> {
    if header.protocol_version != PROTOCOL_VERSION || header.schema_hash != schema::schema_hash() {
        return Err(Diagnostic::new(
            code::REPLAY_FORMAT,
            format!(
                "replay was recorded against protocol version {} / schema {}, but this engine \
                 speaks version {PROTOCOL_VERSION} / schema {}",
                header.protocol_version,
                header.schema_hash,
                schema::schema_hash()
            ),
        )
        .with_help(
            "re-record the session with this engine build; replay formats are version-locked",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod seek_transition_tests {
    //! A replay **seek** whose keyframe lands *before* a district switch, with a target *after* it,
    //! reconstructs by restoring the keyframe and re-driving the deltas — which cross the switch. The
    //! switch **regenerates** from the sim's re-set request, so it needs the target district's text;
    //! on a checkout whose district file has since changed the only source is the pre-scanned
    //! scene-text map. This proves the reconstruction seams compose: `restore_from_value` (the
    //! keyframe) + `install_replay_scene_text` (the off-disk map) + `drive_actions` (the deltas). It
    //! is an in-crate test because those seams are `pub(crate)`; the engine has no production seek
    //! consumer (the CLI's `ludion replay` re-drives linearly via [`replay_hashes`], never
    //! [`ReplayReader::seek`]), so the composition is spelled out here.
    //!
    //! The self-contained *post-switch* keyframe restore (item 3(a)) and the same-machine seek-from-boot
    //! reconstruction are proven over the public API in `tests/transition_snapshot.rs`.

    use super::*;
    use crate::components::Name;
    use crate::transition::SceneSwitch;
    use ludion_ecs::{SystemCtx, World};
    use ludion_protocol::command::SimStepParams;
    use ludion_protocol::replay::{ReplayReader, ReplayRecord};
    use ludion_sim::WorldHash;
    use std::sync::{Arc, Mutex};

    const START: &str = "\
format: 1
seed: 7
entities:
  - name: player
    components:
      Transform:
        translation:
          - 1.0
          - 2.0
          - 3.0
";

    /// District B as recorded (beacon at x=7) — the text the `SceneSwitch` record carries inline.
    const NEXT: &str = "\
format: 1
seed: 999
entities:
  - name: gate-from-start
    components:
      Transform:
        translation:
          - 40.0
          - 0.0
          - -10.0
  - name: beacon
    components:
      Transform:
        translation:
          - 7.0
          - 0.0
          - 0.0
";

    /// A reverted checkout: district B on disk now places the beacon at x=999. A reconstruction that
    /// (wrongly) read disk would diverge; the embedded map (beacon at 7) must win.
    const NEXT_REVERTED: &str = "\
format: 1
seed: 999
entities:
  - name: gate-from-start
    components:
      Transform:
        translation:
          - 40.0
          - 0.0
          - -10.0
  - name: beacon
    components:
      Transform:
        translation:
          - 999.0
          - 0.0
          - 0.0
";

    /// Requests a switch to `next.yaml` on frame 2, carrying the `player` entity.
    fn switch_to_next(world: &mut World, ctx: &SystemCtx) {
        if ctx.frame.as_u64() != 2 {
            return;
        }
        let player = world.entities().find(|&id| {
            world
                .get::<Name>(id)
                .ok()
                .flatten()
                .is_some_and(|n| n.0 == "player")
        });
        if let (Some(player), Some(switch)) = (player, world.resource_mut::<SceneSwitch>()) {
            switch.request(
                "next.yaml".to_string(),
                "gate-from-start".to_string(),
                vec![player],
            );
        }
    }

    fn game(dir: &std::path::Path) -> EngineBuilder {
        Engine::builder()
            .system("switch_to_next", switch_to_next)
            .project_root(dir)
    }

    #[derive(Clone, Default)]
    struct SharedSink(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for SharedSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn fresh_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludion-seek-transition-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("next.yaml"), NEXT).unwrap();
        dir
    }

    /// Reconstructs an engine at `target` from `bytes` via `ReplayReader::seek`: restore the latest
    /// keyframe at or before `target`, install the off-disk scene-text map (pre-scanned from every
    /// `SceneSwitch` record) so a switch regenerated by the re-driven deltas resolves its district text
    /// without touching disk, then drive the seek's deltas. This is the reconstruction a seek consumer
    /// performs, spelled out to prove the seams compose.
    fn seek_reconstruct(
        builder: EngineBuilder,
        bytes: &[u8],
        target: FrameIndex,
        dir: &std::path::Path,
    ) -> Vec<(FrameIndex, WorldHash)> {
        let reader = ReplayReader::new(bytes).unwrap();
        let mut engine = builder.build_at(dir.to_path_buf()).unwrap();

        // Pre-scan every visited district's text into the off-disk map (the `replay_hashes_with` mold).
        let mut scene_text = BTreeMap::new();
        let mut prefabs = PrefabScan::default();
        for record in reader.records().unwrap() {
            prefabs.observe(&record);
            if let ReplayRecord::SceneSwitch {
                scene_path,
                document,
                ..
            } = record
            {
                scene_text.insert(scene_path, document);
            }
        }

        let seek = reader.seek(target).unwrap();
        match &seek.keyframe {
            Some((_, snapshot)) => {
                engine.restore_from_value(snapshot).unwrap();
            }
            None => {
                load_header_scene(&mut engine, reader.header()).unwrap();
                engine.reseed(reader.header().seed);
            }
        }
        // The map must be installed BEFORE driving the deltas — the switch a delta regenerates resolves
        // its target from it (never disk), so a reverted/absent district file cannot corrupt the seek.
        engine.install_replay_scene_text(scene_text);
        // The deltas start in the epoch the scan ended in, less the rebuilds the deltas themselves
        // replay. A seek consumer also re-applies the reload records of that start epoch which
        // precede the keyframe; this test has no prefabs.
        let start_epoch = prefabs.epoch
            - seek
                .deltas
                .iter()
                .filter(|record| starts_epoch(record))
                .count() as u32;
        engine.install_replay_prefab_text(prefabs.reads, start_epoch);

        let mut actions = Vec::new();
        for record in seek.deltas {
            if let ReplayRecord::PrefabText {
                path,
                document,
                reload: true,
                ..
            } = &record
            {
                actions.push(ReplayAction::PrefabReload {
                    path: path.clone(),
                    document: document.clone(),
                });
            }
            match record {
                ReplayRecord::Command { frame, command } => {
                    actions.push(command_action(frame, command));
                }
                ReplayRecord::Input { frame, event } => {
                    actions.push(ReplayAction::Command(Command::InputInject(
                        InputInjectParams {
                            frame: Some(frame),
                            event,
                        },
                    )));
                }
                ReplayRecord::DocReload {
                    doc_kind,
                    path,
                    document,
                    ..
                } => actions.push(ReplayAction::DocReload {
                    kind: doc_kind,
                    path,
                    document,
                }),
                ReplayRecord::SceneLoad {
                    frame,
                    path,
                    document,
                } => {
                    actions.push(ReplayAction::SceneLoad {
                        frame,
                        path,
                        document,
                    });
                }
                // The switch regenerates from the re-driven sim; its record only fed the map above.
                ReplayRecord::SceneSwitch { .. }
                | ReplayRecord::Keyframe { .. }
                | ReplayRecord::PrefabText { .. }
                | ReplayRecord::SystemPanic { .. } => {}
            }
        }
        drive_actions(&mut engine, actions).unwrap()
    }

    #[test]
    fn a_seek_from_a_pre_switch_keyframe_regenerates_the_switch_off_disk() {
        let dir = fresh_dir("pre-switch-keyframe");

        // Record a short session that crosses the gate at frame 2.
        let sink = SharedSink::default();
        let mut engine = game(&dir).build().unwrap();
        engine.load_inline_scene(START).unwrap();
        engine.set_recording(sink.clone()).unwrap();
        for _ in 0..6 {
            engine
                .execute(Command::SimStep(SimStepParams {
                    ticks: 1,
                    hash: false,
                }))
                .unwrap();
        }
        engine.stop_recording().unwrap();
        let bytes = sink.0.lock().unwrap().clone();

        // The reference: a linear replay (which installs the same map internally). Computed from the
        // embedded bytes, so it is independent of what the district file on disk says.
        let linear = replay_hashes_with(game(&dir), &bytes, &dir).unwrap();

        // A reverted checkout: the district file on disk now differs. Only the embedded map is correct.
        std::fs::write(dir.join("next.yaml"), NEXT_REVERTED).unwrap();

        // Seek to frame 5, before the closing keyframe at 6: the latest keyframe at or before it is the
        // boot keyframe at frame 0 (before the switch), so the deltas cross the gate and the switch
        // regenerates — resolving its district text from the installed map, never the reverted disk
        // file.
        let seek_trace = seek_reconstruct(game(&dir), &bytes, FrameIndex::new(5), &dir);

        // The step stamped 5 is at the target, so it is re-driven and its tick lands on frame 6.
        assert_eq!(
            seek_trace.last().unwrap().0,
            FrameIndex::new(6),
            "the reconstruction reaches the target, past the switch frame (2), so it exercised the regeneration"
        );
        assert_eq!(
            seek_trace,
            linear[..seek_trace.len()],
            "a seek reconstruction whose deltas cross the switch reproduces the linear trace off-disk \
             — the pre-scanned scene-text map wins over the reverted district file"
        );
    }

    #[test]
    fn a_zero_tick_replay_yields_the_frame_zero_hash() {
        let dir = fresh_dir("zero-tick");
        let sink = SharedSink::default();
        let mut engine = game(&dir).build().unwrap();
        engine.load_inline_scene(START).unwrap();
        engine.set_recording(sink.clone()).unwrap();
        engine.stop_recording().unwrap();
        let bytes = sink.0.lock().unwrap().clone();

        let trace = replay_hashes_with(game(&dir), &bytes, &dir).unwrap();
        assert_eq!(
            trace,
            vec![(FrameIndex::ZERO, engine.world_hash().unwrap())]
        );
    }
}

#[cfg(test)]
mod recorded_extent_tests {
    use super::*;
    use crate::RenderScale;

    fn resize(width: u32, height: u32) -> RenderSizeParams {
        RenderSizeParams {
            width: Some(width),
            height: Some(height),
        }
    }

    /// The host's render scale refuses a live `render.size`, never a recorded one: the scale is a
    /// per-machine knob, and the recording's machine may have run at another.
    #[test]
    fn a_recorded_extent_ignores_the_hosts_render_scale() {
        let mut engine = Engine::builder().build().unwrap();
        engine.set_render_scale(RenderScale::new(2.0).expect("an in-range scale"));

        let live = engine
            .execute(Command::RenderSize(resize(7680, 4320)))
            .unwrap_err();
        assert_eq!(live.code, code::RENDER_EXTENT_INVALID);
        assert_ne!(engine.render_size(), (7680, 4320));

        let action = command_action(FrameIndex::new(0), Command::RenderSize(resize(7680, 4320)));
        drive_actions(&mut engine, vec![action]).unwrap();
        assert_eq!(engine.render_size(), (7680, 4320));
    }

    /// A recorded extent still has to be a valid one.
    #[test]
    fn a_recorded_extent_is_still_validated() {
        let mut engine = Engine::builder().build().unwrap();
        let error = render::apply_recorded(&mut engine, &resize(0, 540)).unwrap_err();
        assert_eq!(error.code, code::PROTOCOL_RENDER_SIZE_INVALID);

        let action = command_action(FrameIndex::new(3), Command::RenderSize(resize(0, 540)));
        let error = drive_actions(&mut engine, vec![action]).unwrap_err();
        assert_eq!(error.code, code::PROTOCOL_RENDER_SIZE_INVALID);
        assert!(
            error.message.contains("render.size record at frame 3"),
            "{}",
            error.message
        );
    }
}
