//! `sim.pause` / `sim.resume` / `sim.step` / `sim.set_speed` / `sim.hash` / `input.inject` handlers.
//!
//! Run control and hashing delegate straight to the [`SimDriver`](ludion_sim::SimDriver); the
//! handlers are the thin routing layer above it.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_protocol::WireHash;
use ludion_protocol::command::{InputInjectParams, SimSetSpeedParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};
use ludion_sim::{InputEvent, Speed};

use crate::engine::Engine;

/// `sim.pause` — stop ticking (the queue keeps draining and answering).
pub(crate) fn pause(engine: &mut Engine) -> Result<ResponseBody, Diagnostic> {
    engine.driver_mut().pause();
    Ok(ResponseBody::Ack)
}

/// `sim.resume` — resume free-running ticks.
pub(crate) fn resume(engine: &mut Engine) -> Result<ResponseBody, Diagnostic> {
    engine.driver_mut().resume();
    Ok(ResponseBody::Ack)
}

/// `sim.step` — run exactly `ticks` ticks then pause; optionally return the resulting `WorldHash`.
///
/// A system that panics during the step stops it short and auto-pauses the sim, so this answers with
/// that `L4007` rather than an Ack that would claim the requested ticks all ran (the sim's own
/// `run_state` and the `log` channel report the same pause to anyone not holding this reply). The
/// step did advance the world, so the record holds the ticks that ran, the panicking tick included.
pub(crate) fn step(engine: &mut Engine, params: SimStepParams) -> Result<ResponseBody, Diagnostic> {
    let ran = engine.step_ticks(params.ticks);
    if let Some(panic) = engine.take_system_panic() {
        engine.note_applied_before_error(Command::SimStep(SimStepParams {
            ticks: ran,
            hash: params.hash,
        }));
        return Err(panic);
    }
    let hash = if params.hash {
        Some(WireHash::from_u64(engine.world_hash()?.as_u64()))
    } else {
        None
    };
    Ok(ResponseBody::Stepped { hash })
}

/// `sim.set_speed` — validate and store the wall-clock pacing factor (`L4001` if outside `[1/64, 64]`).
///
/// Pacing is applied only by the live host; this stores the factor and never changes `dt`.
pub(crate) fn set_speed(
    engine: &mut Engine,
    params: SimSetSpeedParams,
) -> Result<ResponseBody, Diagnostic> {
    let speed = Speed::new(params.speed)?;
    engine.driver_mut().set_speed(speed);
    Ok(ResponseBody::Ack)
}

/// `sim.hash` — the current `WorldHash` (the determinism verification primitive).
pub(crate) fn hash(engine: &mut Engine) -> Result<ResponseBody, Diagnostic> {
    Ok(ResponseBody::SimHash {
        hash: WireHash::from_u64(engine.world_hash()?.as_u64()),
    })
}

/// `input.inject` — deserialize the value into a sim input event and inject it (`L3005` if the value
/// is not a valid input event, or `frame` has already run).
pub(crate) fn inject(
    engine: &mut Engine,
    params: InputInjectParams,
) -> Result<ResponseBody, Diagnostic> {
    let event: InputEvent = serde_json::from_value(params.event).map_err(|error| {
        Diagnostic::new(
            code::PROTOCOL_INVALID_INPUT,
            format!("input.inject event is not a valid input event: {error}"),
        )
        .with_help("use e.g. {\"kind\": \"key_down\", \"key\": \"W\"} — see L3005 for the shape")
    })?;
    engine
        .driver_mut()
        .inject_input(params.frame, event)
        .map_err(|past| {
            Diagnostic::new(
                code::PROTOCOL_INVALID_INPUT,
                format!(
                    "input.inject frame {} has already run (the sim is at frame {})",
                    past.requested.as_u64(),
                    past.current.as_u64()
                ),
            )
            .with_help(
                "omit `frame` to apply at the next tick, or address a frame at or after the current one",
            )
        })?;
    Ok(ResponseBody::Ack)
}
