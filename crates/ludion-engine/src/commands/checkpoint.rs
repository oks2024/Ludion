//! The `checkpoint.*` handlers.
//!
//! Each mutating verb applies at the boundary it is dispatched at, through the engine code the
//! `Checkpoints` request drain runs, and replies with the store after it applied.

use ludion_core::diagnostics::Diagnostic;
use ludion_protocol::ResponseBody;
use ludion_protocol::command::CheckpointNameParams;
use ludion_protocol::response::{CheckpointRestore, CheckpointSlot};

use crate::checkpoints::{CheckpointInfo, RestoreOutcome};
use crate::engine::Engine;

/// `checkpoint.list` — the store as it stands.
pub(crate) fn list(engine: &mut Engine) -> Result<ResponseBody, Diagnostic> {
    store(engine)
}

/// `checkpoint.capture` — capture the world into slot `name`.
pub(crate) fn capture(
    engine: &mut Engine,
    params: CheckpointNameParams,
) -> Result<ResponseBody, Diagnostic> {
    engine.wire_capture(&params.name)?;
    store(engine)
}

/// `checkpoint.restore` — replace the world with slot `name`'s capture.
pub(crate) fn restore(
    engine: &mut Engine,
    params: CheckpointNameParams,
) -> Result<ResponseBody, Diagnostic> {
    engine.wire_restore(&params.name)?;
    store(engine)
}

/// `checkpoint.discard` — drop slot `name`; an unknown name is not an error.
pub(crate) fn discard(
    engine: &mut Engine,
    params: CheckpointNameParams,
) -> Result<ResponseBody, Diagnostic> {
    engine.wire_discard(&params.name)?;
    store(engine)
}

fn slot(info: CheckpointInfo) -> CheckpointSlot {
    CheckpointSlot {
        name: info.name,
        frame: info.frame.as_u64(),
        game_frame: info.game_frame.as_u64(),
        scene: info.scene,
    }
}

fn store(engine: &Engine) -> Result<ResponseBody, Diagnostic> {
    let (slots, disk, last_restore) = engine.checkpoint_view()?;
    Ok(ResponseBody::Checkpoints {
        slots: slots.into_iter().map(slot).collect(),
        disk: disk.map(slot),
        last_restore: last_restore.map(|outcome| match outcome {
            RestoreOutcome::Restored { name, frame } => CheckpointRestore::Restored {
                name,
                frame: frame.as_u64(),
            },
            RestoreOutcome::Refused { name, reason } => CheckpointRestore::Refused { name, reason },
        }),
    })
}
