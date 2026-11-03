//! `engine.describe` / `engine.snapshot` / `engine.restore` handlers.

use ludion_core::diagnostics::Diagnostic;
use ludion_protocol::ResponseBody;
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::event::{Event, SceneChangeKind, SceneChangedData};

use crate::engine::Engine;

/// `engine.describe` — the full machine-readable surface: engine/protocol versions, the schema hash,
/// every component's JSON Schema, and the protocol schema.
pub(crate) fn describe(engine: &mut Engine) -> Result<ResponseBody, Diagnostic> {
    Ok(ResponseBody::EngineDescription {
        description: engine.describe_document(),
    })
}

/// `engine.snapshot` — the serialized engine state (world + RNG + frame + scene/seed), which restore
/// can reconstruct exactly.
pub(crate) fn snapshot(engine: &mut Engine) -> Result<ResponseBody, Diagnostic> {
    let snapshot = crate::snapshot::capture(engine)?;
    engine.write_scene_session();
    Ok(ResponseBody::Snapshot { snapshot })
}

/// `engine.restore` — restore engine state from a snapshot document, migrating over any
/// component-schema drift and returning the report of what changed (empty = a clean restore). `L3008`
/// if the snapshot document itself is malformed, `L0025`/`L3041` if its entity allocator state or
/// entity list is inconsistent.
pub(crate) fn restore(
    engine: &mut Engine,
    params: EngineRestoreParams,
) -> Result<ResponseBody, Diagnostic> {
    let report = engine.restore_from_value(&params.snapshot)?;
    // The restored world is not the file's: a later restart names no path.
    engine.set_current_scene_path(None);
    // The world was replaced — notify `scene.changed` subscribers (dropped if none are open).
    engine.push_event(Event::SceneChanged(SceneChangedData {
        kind: SceneChangeKind::Restore,
        path: None,
        request_id: engine.current_request(),
    }));
    Ok(ResponseBody::Restored { report })
}

/// `engine.shutdown` — request a graceful stop of the hosting process. This flags shutdown on the
/// engine and returns `Ack`; the sim loop ([`run_live`](crate::live::run_live)) observes the flag and
/// returns *after* this reply is sent, so the host runs the same graceful teardown Ctrl-C triggers
/// (flush recordings, close connections, exit 0). In-process (no host) it just sets the observable
/// flag and acks — nothing exits the process.
pub(crate) fn shutdown(engine: &mut Engine) -> Result<ResponseBody, Diagnostic> {
    engine.request_shutdown();
    Ok(ResponseBody::Ack)
}
