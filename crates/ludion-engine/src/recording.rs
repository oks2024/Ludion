//! Session recording: which commands are written, and the [`Recorder`] that writes them.
//!
//! Recording is explicit and opt-in ([`Engine::set_recording`](crate::Engine::set_recording)). At
//! drain time, [`Engine::execute`](crate::Engine::execute) hands each command *as it applied* — a
//! failed command that changed nothing writes nothing — to [`write_delta`], stamped with the frame it
//! was drained at. The whole policy of *what* gets written is one obvious, exhaustive predicate —
//! [`record_kind`] — not a decision scattered across handlers:
//!
//! - **Injected inputs** ([`RecordKind::Input`]) are written as replay `Input` records so replay
//!   re-injects them at their apply frame.
//! - **World/sim/run-state-mutating commands** ([`RecordKind::Command`]) are written as replay
//!   `Command` records.
//! - **A mid-session `scene.load`** ([`RecordKind::SceneLoad`]) is written as a replay `SceneLoad`
//!   record carrying the text it resolved, so replay never re-reads the path.
//! - **Pure reads and transport/lifecycle/export commands** (component.get, query.entities, sim.hash,
//!   scene.check, scene.save, render.* except an applied `render.size`, profile.capture,
//!   engine.describe, engine.snapshot, engine.shutdown, and stream.subscribe/unsubscribe) leave **no
//!   trace** — they do not affect the `WorldHash` sequence a replay must reproduce.
//!
//! Keyframes (`engine.snapshot` documents, at the start, every
//! [`KEYFRAME_INTERVAL`](crate::engine::KEYFRAME_INTERVAL) frames and at the stop) are written by the engine around
//! [`write_delta`]; the seek machinery in `ludion-protocol` uses them. An engine holding builder
//! `doc_text` docs writes them first, as `DocReload` records ahead of the arm keyframe
//! ([`write_inline_docs`]); replay folds those back into its builder.

use std::io::{self, Write};

use ludion_core::time::FrameIndex;
use ludion_protocol::Command;
use ludion_protocol::command::{RenderSizeParams, SimStepParams};
use ludion_protocol::replay::ReplayWriter;
use serde_json::Value;

/// How a command participates in a recorded session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecordKind {
    /// A world/sim/run-state mutation: written as a replay `Command` record.
    Command,
    /// An injected input: written as a replay `Input` record (re-injected on replay).
    Input,
    /// A mid-session `scene.load`: written as a replay `SceneLoad` record carrying the resolved text.
    SceneLoad,
}

/// A game data-doc reload the `asset.reload` handler *applied* this frame — the state
/// [`record_kind`] cannot see (a doc reload rides `Command::AssetReload`, indistinguishable from a
/// texture/mesh/voxel reload by the command alone). The engine hands this alongside the command to
/// [`write_delta`], which writes it as a frame-stamped `DocReload` record carrying the new canonical
/// text inline. `None` for a plain asset reload (or any other command).
pub(crate) struct AppliedDocReload {
    /// The registered doc kind.
    pub kind: String,
    /// The content-root-relative key that was reloaded (the `DocStore` entry / record path).
    pub key: String,
    /// The new canonical doc text, recorded inline so a replay reproduces the swap off-disk.
    pub document: String,
}

/// The single, exhaustive predicate deciding whether — and as what — a command is recorded.
///
/// There is no wildcard arm: a new [`Command`] variant forces an explicit recording decision here,
/// the same way it forces a dispatch handler.
pub(crate) fn record_kind(command: &Command) -> Option<RecordKind> {
    match command {
        // Injected input — replay re-injects it at the stamped apply frame.
        Command::InputInject(_) => Some(RecordKind::Input),

        // Pure reads and transport/lifecycle/export commands: never recorded (they cannot change the
        // WorldHash sequence a replay reproduces). `engine.shutdown` belongs here, not with the sim
        // run-state controls below: the sim controls (pause/resume/step/set_speed) gate *when* ticks
        // happen relative to injected inputs, so replay must reproduce them; shutdown only stops the
        // host process — it mutates no world/sim state and has no post-shutdown trace to reproduce, so
        // like `stream.subscribe` it leaves no record. `asset.reload` is `None` *as a command*: a
        // texture/mesh/voxel reload invalidates a CPU cache and re-renders but touches no world/sim
        // state (asset bytes never enter the WorldHash), so recording it would only make a replay
        // depend on assets it does not carry. A *game data-doc* reload also rides `AssetReload`, but it
        // IS sim-visible (systems derive components from the `DocStore`), so it is recorded — as a
        // `DocReload` record driven by engine state (`AppliedDocReload`), not by this command-only
        // predicate, which cannot tell the two apart (Phase 15 / E4, decision 9; see `write_delta`).
        // `profile.capture` belongs here too: it arms a host-side observer whose measurements never
        // enter the world, so a profiled recording is byte-identical to an unprofiled one — the
        // property `profiled_recording_is_byte_identical` (in `perf_live`) asserts. `render.gi_rebake`
        // is the same shape of thing one layer down: the lever it moves is host state that never
        // enters a `WorldHash`, so a session held while recording replays on an `Auto` engine
        // bit-for-bit — and recording it would make a replay depend on a host setting it has no
        // business reproducing. (`render.size` is deliberately not here: the extent never enters a
        // `WorldHash`, but it is sim *input* — a game unprojects the cursor through `RenderExtent` —
        // so an applied resize is recorded, and the header carries the arm-time extent. Only the
        // read form, with neither dimension, is a no-trace read.)
        Command::SceneCheck(_)
        | Command::SceneSave(_)
        | Command::PrefabCreate(_)
        | Command::PrefabApply(_)
        | Command::PrefabOverrides(_)
        | Command::ComponentGet(_)
        | Command::QueryEntities(_)
        | Command::SimHash
        | Command::RenderScreenshot(_)
        | Command::RenderDumpGraph
        | Command::RenderDumpTimings
        | Command::RenderBounds(_)
        | Command::RenderGiRebake(_)
        | Command::RenderSize(RenderSizeParams {
            width: None,
            height: None,
        })
        | Command::RenderView(_)
        | Command::RenderDebug(_)
        | Command::StreamSubscribe(_)
        | Command::StreamUnsubscribe(_)
        | Command::AssetReload(_)
        | Command::EngineDescribe
        | Command::EngineSnapshot
        | Command::EngineShutdown
        | Command::ProfileCapture(_)
        | Command::AiDescribe
        | Command::CheckpointList => None,

        // A mid-session load is its own record kind: it carries the text it resolved, so a replay
        // never re-reads the path off disk (see `write_delta`).
        Command::SceneLoad(_) => Some(RecordKind::SceneLoad),

        // Everything else mutates world / sim / run state — recorded as a command delta. An applied
        // `render.size` is one: the extent is sim input.
        Command::RenderSize(_)
        | Command::EntitySpawn(_)
        | Command::EntityDestroy(_)
        | Command::EntityReparent(_)
        | Command::PrefabSpawn(_)
        | Command::MapSpawn(_)
        | Command::PrefabDespawn(_)
        | Command::PrefabRevert(_)
        | Command::PrefabUnpack(_)
        | Command::ComponentSet(_)
        | Command::ComponentRemove(_)
        | Command::SimPause
        | Command::SimResume
        | Command::SimStep(_)
        | Command::SimSetSpeed(_)
        | Command::EngineRestore(_)
        | Command::CheckpointCapture(_)
        | Command::CheckpointRestore(_)
        | Command::CheckpointDiscard(_) => Some(RecordKind::Command),
    }
}

/// An active recording sink plus its keyframe bookkeeping.
pub(crate) struct Recorder {
    /// The framed replay writer over the caller-supplied sink.
    pub writer: ReplayWriter<Box<dyn Write + Send>>,
    /// How often (in frames) to write a keyframe snapshot.
    pub keyframe_interval: u64,
    /// The frame of the most recently written keyframe.
    pub last_keyframe: FrameIndex,
    /// The writer's record count right after the last keyframe or `system_panic` marker; a closing
    /// keyframe is written only when the stream has moved past it.
    pub records_at_mark: u64,
    /// Free-running live ticks advanced since the last flush, not yet written to the file. A live
    /// host free-runs through [`advance_live`](crate::Engine::advance_live) — which never dispatches
    /// a `sim.step` — so these ticks have no command record of their own. They are batched here and
    /// flushed as a single `SimStep` command record ([`flush_pending_ticks`]) just before any delta
    /// (input / command / doc-reload), at each keyframe and at a system panic (with
    /// its marker), so a replay advances the frame the same way a lockstep recording's explicit `sim.step` records do — without emitting one
    /// tiny record per tick. Always `0` for a lockstep session (its ticks arrive as `sim.step`
    /// commands, recorded through [`RecordKind::Command`], so this batching path never fires). A
    /// `SceneSwitch` record is written mid-tick without a flush, so it can precede the `SimStep` that
    /// reaches its frame; replay only pre-scans those records.
    pub pending_ticks: u64,
}

impl Recorder {
    /// Wraps an opened [`ReplayWriter`] with keyframe bookkeeping.
    pub(crate) fn new(
        writer: ReplayWriter<Box<dyn Write + Send>>,
        keyframe_interval: u64,
        last_keyframe: FrameIndex,
    ) -> Self {
        Recorder {
            writer,
            keyframe_interval,
            last_keyframe,
            records_at_mark: 0,
            pending_ticks: 0,
        }
    }
}

/// Flushes the recorder's batched free-run ticks as one `SimStep` command record, stamped at the
/// frame the batch began (`batch_end - pending_ticks`), then clears the counter. A no-op when nothing
/// is pending.
///
/// `batch_end` is the frame the last batched tick advanced *to*, so the batch spans
/// `[batch_end - pending_ticks, batch_end)`. On replay [`drive_actions`](crate::replay) decomposes the
/// `SimStep(n)` back into `n` single ticks — identical to a lockstep recording's per-tick `sim.step`
/// records — so the frame advances and every input scheduled at a batched frame applies at its tick.
pub(crate) fn flush_pending_ticks(
    recorder: &mut Recorder,
    batch_end: FrameIndex,
) -> io::Result<()> {
    if recorder.pending_ticks == 0 {
        return Ok(());
    }
    let start = FrameIndex::new(batch_end.as_u64() - recorder.pending_ticks);
    let command = Command::SimStep(SimStepParams {
        ticks: recorder.pending_ticks,
        hash: false,
    });
    recorder.writer.write_command(start, &command)?;
    recorder.pending_ticks = 0;
    Ok(())
}

/// Writes the `system_panic` marker for the tick at `frame`. A free-run batch is flushed first, ending
/// at the panicking tick, so the marker always sits right after the `SimStep` whose last tick it is;
/// a lockstep session has nothing pending and only writes the marker.
pub(crate) fn write_system_panic(recorder: &mut Recorder, frame: FrameIndex) -> io::Result<()> {
    flush_pending_ticks(recorder, FrameIndex::new(frame.as_u64() + 1))?;
    recorder.writer.write_system_panic(frame)?;
    recorder.records_at_mark = recorder.writer.records_written();
    Ok(())
}

/// Writes a keyframe of `snapshot` at `frame` and marks it as the stream's latest keyframe.
pub(crate) fn write_keyframe(
    recorder: &mut Recorder,
    frame: FrameIndex,
    snapshot: &Value,
) -> io::Result<()> {
    recorder.writer.write_keyframe(frame, snapshot)?;
    recorder.last_keyframe = frame;
    recorder.records_at_mark = recorder.writer.records_written();
    Ok(())
}

/// Writes one `DocReload` per builder inline doc (`doc_text`), in key order at `frame`. They sit
/// between the header and the arm keyframe, which is how replay tells them from a live reload.
pub(crate) fn write_inline_docs(
    recorder: &mut Recorder,
    frame: FrameIndex,
    docs: &std::collections::BTreeMap<String, crate::docs::InlineDoc>,
) -> io::Result<()> {
    for (key, doc) in docs {
        recorder
            .writer
            .write_doc_reload(frame, &doc.kind, key, &doc.text)?;
    }
    recorder.records_at_mark = recorder.writer.records_written();
    Ok(())
}

/// Writes one delta record for `command` (drained at `drain_frame`), per [`record_kind`]. `command` is
/// the command *as it mutated the world* (see [`Engine::execute`](crate::Engine::execute)); `None`
/// when a failed command left the world untouched.
///
/// When `applied_doc` is `Some`, the drained command applied a game data-doc reload this frame: it is
/// written as a `DocReload` record carrying the new canonical text inline, regardless of what
/// [`record_kind`] says about the command (which is `AssetReload` → `None`). This is the seam decision
/// 9 requires — a doc reload is distinguishable from an asset reload only by engine state, not by the
/// `Command`.
///
/// Otherwise: an [`RecordKind::Input`] command is written as an `Input` record stamped at the event's
/// *apply* frame — the requested frame, or `drain_frame` when none — which is what
/// [`SimDriver::inject_input`](ludion_sim::SimDriver::inject_input) applies (it refuses a past
/// frame, so a refused inject records nothing), so replay injects it at the same frame. A
/// `scene.load` is a `SceneLoad` record carrying `scene_text` — the engine's current inline scene,
/// which the load just installed. Everything else recordable is a `Command` record stamped at
/// `drain_frame`.
pub(crate) fn write_delta(
    recorder: &mut Recorder,
    command: Option<&Command>,
    drain_frame: FrameIndex,
    applied_doc: Option<&AppliedDocReload>,
    scene_text: Option<&str>,
) -> io::Result<()> {
    if let Some(doc) = applied_doc {
        // Flush the batched free-run ticks first so the doc reload lands at its frame in the stream.
        flush_pending_ticks(recorder, drain_frame)?;
        return recorder
            .writer
            .write_doc_reload(drain_frame, &doc.kind, &doc.key, &doc.document);
    }
    let Some(command) = command else {
        return Ok(());
    };
    match record_kind(command) {
        // A pure read writes nothing — and must not flush the pending-tick batch, so a client
        // polling `component.get` every frame during a live free-run does not split the batch into
        // one `SimStep` record per poll.
        None => Ok(()),
        Some(RecordKind::Input) => {
            let Command::InputInject(params) = command else {
                unreachable!("record_kind returns Input only for InputInject");
            };
            // Flush the free-run ticks up to this frame before the input, so replay advances to the
            // input's frame and then applies it at the matching tick (the live ordering).
            flush_pending_ticks(recorder, drain_frame)?;
            let apply = params.frame.unwrap_or(drain_frame);
            debug_assert!(
                apply >= drain_frame,
                "a past-frame inject is refused at dispatch"
            );
            recorder.writer.write_input(apply, &params.event)
        }
        Some(RecordKind::Command) => {
            flush_pending_ticks(recorder, drain_frame)?;
            recorder.writer.write_command(drain_frame, command)
        }
        Some(RecordKind::SceneLoad) => {
            let Command::SceneLoad(params) = command else {
                unreachable!("record_kind returns SceneLoad only for SceneLoad");
            };
            portable(&params.path)?;
            flush_pending_ticks(recorder, drain_frame)?;
            match scene_text {
                Some(text) => recorder
                    .writer
                    .write_scene_load(drain_frame, &params.path, text),
                None => {
                    debug_assert!(
                        false,
                        "an applied scene.load leaves the engine's scene inline"
                    );
                    recorder.writer.write_command(drain_frame, command)
                }
            }
        }
    }
}

/// Refuses a path a replay could not carry between machines: absolute, drive-prefixed or reserved.
/// A relative climb is portable; the read bounds it at the project root. Upstream validation
/// already refuses these; this is the backstop that keeps one out of a `.lrp`.
fn portable(path: &str) -> io::Result<()> {
    match ludion_core::paths::climbing_refusal(path, usize::MAX) {
        Some(refusal) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "refusing to record the non-portable path `{path}`: it {}",
                refusal.describe()
            ),
        )),
        None => Ok(()),
    }
}

/// Writes a `SceneSwitch` record for a district switch performed at `frame`, carrying the target
/// district's canonical `document` text inline — the `write_doc_reload` mold for the switch's
/// external text (Phase 17 / E7, decision 6).
///
/// Unlike [`write_delta`], a switch is not a `Command`: it is requested through the [`SceneSwitch`]
/// resource and performed at a frame-boundary drain, never dispatched. So [`record_kind`] never sees
/// it, and the engine's `drain_scene_switch` calls this directly after a *live* switch succeeds. A
/// replay never reaches here (it resolves the target off these records, and never records); the record
/// supplies only the off-disk text source the regenerated switch needs, so a replay reproduces the
/// district change on a checkout whose target scene has since changed or is gone.
///
/// [`SceneSwitch`]: crate::SceneSwitch
pub(crate) fn write_scene_switch(
    recorder: &mut Recorder,
    frame: FrameIndex,
    scene_path: &str,
    document: &str,
) -> io::Result<()> {
    portable(scene_path)?;
    recorder
        .writer
        .write_scene_switch(frame, scene_path, document)
}

/// Writes a `PrefabText` record for a prefab source read at `frame`. The batched free-run ticks are
/// flushed first, as a doc reload does, so the stream's frames stay ordered.
pub(crate) fn write_prefab_text(
    recorder: &mut Recorder,
    frame: FrameIndex,
    path: &str,
    document: &str,
    reload: bool,
) -> io::Result<()> {
    portable(path)?;
    flush_pending_ticks(recorder, frame)?;
    recorder
        .writer
        .write_prefab_text(frame, path, document, reload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_core::ids::EntityId;
    use ludion_protocol::command::{
        ComponentGetParams, ComponentRemoveParams, EntitySpawnParams, InputInjectParams,
        RenderSizeParams, SceneLoadParams,
    };

    fn recorder() -> Recorder {
        let header = ludion_protocol::replay::ReplayHeader {
            format_version: ludion_protocol::replay::REPLAY_FORMAT_VERSION,
            engine_version: "0.0.0".into(),
            protocol_version: ludion_protocol::PROTOCOL_VERSION,
            schema_hash: ludion_protocol::schema::schema_hash(),
            scene: ludion_protocol::replay::SceneRef::Inline {
                document: "format: 1\n".into(),
            },
            seed: 0,
            profile: None,
            render_extent: None,
            replay_content_hash: None,
            game: None,
            save_docs: Default::default(),
        };
        let sink = Box::new(Vec::new()) as Box<dyn Write + Send>;
        let writer = ludion_protocol::replay::ReplayWriter::new(sink, &header).unwrap();
        Recorder::new(writer, 60, FrameIndex::ZERO)
    }

    #[test]
    fn a_non_portable_path_is_never_written_to_a_replay() {
        for path in ["/abs/x.yaml", "C:x.yaml"] {
            let mut recorder = recorder();
            let before = recorder.writer.records_written();
            let load = Command::SceneLoad(SceneLoadParams { path: path.into() });
            let errors = [
                write_delta(
                    &mut recorder,
                    Some(&load),
                    FrameIndex::ZERO,
                    None,
                    Some("x"),
                ),
                write_scene_switch(&mut recorder, FrameIndex::ZERO, path, "x"),
                write_prefab_text(&mut recorder, FrameIndex::ZERO, path, "x", false),
            ];
            for error in errors {
                let error = error.expect_err(path);
                assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{path}");
                assert!(error.to_string().contains(path), "{error}");
            }
            assert_eq!(recorder.writer.records_written(), before, "{path}");
        }
    }

    #[test]
    fn reads_are_never_recorded() {
        assert_eq!(
            record_kind(&Command::ComponentGet(ComponentGetParams {
                entity: EntityId::new(0, 0),
                component: "Transform".into(),
            })),
            None,
        );
        assert_eq!(record_kind(&Command::SimHash), None);
        assert_eq!(record_kind(&Command::EngineSnapshot), None);
    }

    /// The extent is sim input, so an applied resize is a record; the read form (no dimension) is
    /// not, since the editor may poll it.
    #[test]
    fn an_applied_resize_is_recorded_and_a_read_is_not() {
        for (width, height) in [(Some(960), Some(540)), (Some(960), None), (None, Some(540))] {
            assert_eq!(
                record_kind(&Command::RenderSize(RenderSizeParams { width, height })),
                Some(RecordKind::Command),
            );
        }
        assert_eq!(
            record_kind(&Command::RenderSize(RenderSizeParams {
                width: None,
                height: None,
            })),
            None,
        );
    }

    #[test]
    fn scene_load_is_its_own_record_kind() {
        assert_eq!(
            record_kind(&Command::SceneLoad(SceneLoadParams {
                path: "scenes/arena.yaml".into(),
            })),
            Some(RecordKind::SceneLoad),
        );
    }

    #[test]
    fn injected_input_is_an_input_record() {
        assert_eq!(
            record_kind(&Command::InputInject(InputInjectParams {
                frame: None,
                event: serde_json::json!({ "kind": "key_down", "key": "W" }),
            })),
            Some(RecordKind::Input),
        );
    }

    #[test]
    fn mutations_are_command_records() {
        assert_eq!(
            record_kind(&Command::EntitySpawn(EntitySpawnParams {
                name: None,
                components: Default::default(),
                presentation: false,
            })),
            Some(RecordKind::Command),
        );
        assert_eq!(record_kind(&Command::SimPause), Some(RecordKind::Command));
        assert_eq!(
            record_kind(&Command::ComponentRemove(ComponentRemoveParams {
                entity: EntityId::new(0, 0),
                component: "Transform".into(),
            })),
            Some(RecordKind::Command),
        );
    }
}
