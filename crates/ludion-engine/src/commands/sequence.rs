//! The `sequence.*` handlers: shape the reply; the engine methods live in `sequences::wire`.

use ludion_core::diagnostics::Diagnostic;
use ludion_protocol::ResponseBody;
use ludion_protocol::command::{SequencePlayParams, SequenceSeekParams, SequenceSkipParams};

use crate::engine::Engine;

/// `sequence.play` — start the document with its roles bound; reply with the player.
pub(crate) fn play(
    engine: &mut Engine,
    params: SequencePlayParams,
) -> Result<ResponseBody, Diagnostic> {
    let player = engine.sequence_play(&params.sequence, &params.bindings)?;
    Ok(ResponseBody::Sequence {
        player,
        ended: None,
    })
}

/// `sequence.seek` — rewind to the last play and run to the playhead tick.
pub(crate) fn seek(
    engine: &mut Engine,
    params: SequenceSeekParams,
) -> Result<ResponseBody, Diagnostic> {
    engine.sequence_seek(params)
}

/// `sequence.skip` — cut the player to its end state at the next tick.
pub(crate) fn skip(
    engine: &mut Engine,
    params: SequenceSkipParams,
) -> Result<ResponseBody, Diagnostic> {
    engine.sequence_skip(params.player)?;
    Ok(ResponseBody::Ack)
}
