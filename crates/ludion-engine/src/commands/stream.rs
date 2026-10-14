//! `stream.subscribe` / `stream.unsubscribe` handlers.
//!
//! These manage the *state* of subscriptions — allocating and freeing ids (and per-subscriber event
//! sequence counters) in the engine's subscription table. The channel is always valid once
//! deserialized (it is a closed enum), so subscribing on any channel succeeds — **except** a `max_fps`
//! outside [`MIN_MAX_FPS`, `MAX_MAX_FPS`] (`L3046`).
//!
//! `latest_wins` is the only delivery mode. Event delivery on it is ordered:
//! events ride their own unbounded lane out of the sim thread, so an agent verifying `sim.events`/`log`
//! sees them in `seq` order; a gap in `seq` appears only when the client stops reading and the
//! connection's bounded text lane overflows. Unsubscribe of an unknown id is `L3007`. The live loop
//! ([`run_live`](crate::live::run_live)) does the actual delivery.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_protocol::ResponseBody;
use ludion_protocol::command::{StreamSubscribeParams, StreamUnsubscribeParams, SubscribeMode};

use crate::engine::Engine;

/// The slowest `max_fps` a frame subscription may ask for: one frame every ten seconds.
pub(crate) const MIN_MAX_FPS: f32 = 0.1;
/// The fastest: far past any render rate, so a larger value is a mistake rather than a cap.
pub(crate) const MAX_MAX_FPS: f32 = 1000.0;

/// `stream.subscribe` — open a subscription on a channel; returns its `sub` id.
///
/// # Errors
/// - `L3046` if `latest_wins` carries a `max_fps` that is not finite or lies outside
///   [`MIN_MAX_FPS`, `MAX_MAX_FPS`] (at `/params/mode/max_fps`).
pub(crate) fn subscribe(
    engine: &mut Engine,
    params: StreamSubscribeParams,
) -> Result<ResponseBody, Diagnostic> {
    if let SubscribeMode::LatestWins { max_fps: Some(fps) } = params.mode
        && !(MIN_MAX_FPS..=MAX_MAX_FPS).contains(&fps)
    {
        return Err(Diagnostic::new(
            code::PROTOCOL_INVALID_MAX_FPS,
            format!("max_fps {fps} is not a finite number between {MIN_MAX_FPS} and {MAX_MAX_FPS}"),
        )
        .with_path("/params/mode/max_fps")
        .with_help(format!(
            "pass a max_fps in [{MIN_MAX_FPS}, {MAX_MAX_FPS}], or omit it for no cap"
        )));
    }
    let sub = engine.subscriptions_mut().open(params.channel);
    Ok(ResponseBody::Subscribed { sub })
}

/// `stream.unsubscribe` — close a subscription (`L3007` if the id is not open).
pub(crate) fn unsubscribe(
    engine: &mut Engine,
    params: StreamUnsubscribeParams,
) -> Result<ResponseBody, Diagnostic> {
    engine.subscriptions_mut().close(params.sub)?;
    Ok(ResponseBody::Ack)
}
