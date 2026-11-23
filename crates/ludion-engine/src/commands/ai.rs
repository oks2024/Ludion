//! The `ai.describe` handler.

use ludion_core::diagnostics::Diagnostic;
use ludion_protocol::ResponseBody;

use crate::engine::Engine;

/// `ai.describe` — the machines and agents `engine.describe`'s `ai` key carries; empty before any
/// machine has ticked.
pub(crate) fn describe(engine: &mut Engine) -> Result<ResponseBody, Diagnostic> {
    Ok(ResponseBody::AiDescribe {
        machines: crate::ai::machines(engine.world()).unwrap_or_default(),
    })
}
