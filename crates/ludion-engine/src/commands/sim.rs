//! `sim.pause` / `sim.resume` / `sim.step` / `sim.set_speed` / `sim.hash` / `input.inject` handlers.
//!
//! Run control and hashing delegate straight to the [`SimDriver`](ludion_sim::SimDriver); the
//! handlers are the thin routing layer above it.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_protocol::WireHash;
use ludion_protocol::command::{
    InputInjectParams, MAX_STEP_TICKS, SimSetSpeedParams, SimStepParams,
};
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

/// Steps `engine` exactly `ticks` fixed ticks through `sim.step`, in batches no longer than
/// [`MAX_STEP_TICKS`], then leaves it paused. Always issues at least one step, so `0` still pauses.
///
/// # Errors
/// The first batch's failure (a system panic, `L4007`, stops the run there).
///
/// # Example
/// ```
/// use ludion_engine::EngineBuilder;
///
/// let mut engine = ludion_testkit::loaded_engine(EngineBuilder::new(), "format: 1\nentities: []\n");
/// ludion_engine::step_in_batches(&mut engine, 5).unwrap();
/// assert_eq!(engine.frame().as_u64(), 5);
/// ```
pub fn step_in_batches(engine: &mut Engine, ticks: u64) -> Result<(), Diagnostic> {
    step_batched(engine, ticks, MAX_STEP_TICKS)
}

fn step_batched(engine: &mut Engine, ticks: u64, batch: u64) -> Result<(), Diagnostic> {
    let mut remaining = ticks;
    loop {
        let now = remaining.min(batch);
        engine.execute(Command::SimStep(SimStepParams {
            ticks: now,
            hash: false,
        }))?;
        remaining -= now;
        if remaining == 0 {
            return Ok(());
        }
    }
}

/// `sim.step` — run exactly `ticks` ticks (at most [`MAX_STEP_TICKS`], else `L3057`) then pause;
/// optionally return the resulting `WorldHash`.
///
/// A system that panics during the step stops it short and auto-pauses the sim, so this answers with
/// that `L4007` rather than an Ack that would claim the requested ticks all ran (the sim's own
/// `run_state` and the `log` channel report the same pause to anyone not holding this reply). The
/// step did advance the world, so the record holds the ticks that ran, the panicking tick included.
/// A hash that cannot be computed after the ticks ran is recorded the same way.
pub(crate) fn step(engine: &mut Engine, params: SimStepParams) -> Result<ResponseBody, Diagnostic> {
    if params.ticks > MAX_STEP_TICKS {
        return Err(Diagnostic::new(
            code::SIM_STEP_TOO_LONG,
            format!(
                "sim.step asked for {} ticks; the most one step may run is {MAX_STEP_TICKS}",
                params.ticks
            ),
        )
        .with_path("/params/ticks")
        .with_help(
            "step in smaller batches; the command runs synchronously on the sim thread, so one \
             very long step blocks every other client",
        ));
    }
    let ran = engine.step_ticks(params.ticks);
    if let Some(panic) = engine.take_system_panic() {
        engine.note_applied_before_error(Command::SimStep(SimStepParams {
            ticks: ran,
            hash: params.hash,
        }));
        return Err(panic);
    }
    let hash = if params.hash {
        match engine.world_hash() {
            Ok(hash) => Some(WireHash::from_u64(hash.as_u64())),
            Err(error) => {
                engine.note_applied_before_error(Command::SimStep(SimStepParams {
                    ticks: ran,
                    hash: params.hash,
                }));
                return Err(error);
            }
        }
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
/// is not a valid input event, an analog value is non-finite or out of range, or `frame` has
/// already run).
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
    event.validate()?;
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

#[cfg(test)]
mod tests {
    use super::{step, step_batched};
    use crate::Engine;
    use crate::components::Transform;
    use ludion_core::time::FrameIndex;
    use ludion_ecs::{SystemCtx, World};
    use ludion_protocol::Command;
    use ludion_protocol::command::{InputInjectParams, SimStepParams};
    use ludion_sim::{InputState, Key};
    use serde_json::json;

    const SCENE: &str = "\
format: 1
entities:
  - name: walker
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
";

    /// Moves every entity one unit a tick, and ten more while `W` is held.
    fn walk(world: &mut World, _ctx: &SystemCtx) {
        let boost = world
            .resource::<InputState>()
            .is_some_and(|input| input.is_held(Key::W));
        let ids: Vec<_> = world.entities().collect();
        for id in ids {
            if let Ok(Some(transform)) = world.get_mut::<Transform>(id) {
                transform.translation[0] += if boost { 11.0 } else { 1.0 };
            }
        }
    }

    fn walked(ticks: u64, batch: u64) -> (u64, u64) {
        let mut engine = Engine::builder()
            .system("walk", walk)
            .build()
            .expect("the test engine builds");
        engine.load_inline_scene(SCENE).expect("the scene loads");
        engine
            .execute(Command::InputInject(InputInjectParams {
                frame: Some(FrameIndex::new(20)),
                event: json!({ "kind": "key_down", "key": "W" }),
            }))
            .expect("the key is injected");
        step_batched(&mut engine, ticks, batch).expect("the steps run");
        (
            engine.frame().as_u64(),
            engine.world_hash().expect("hashes").as_u64(),
        )
    }

    #[test]
    fn batches_reach_the_same_world_as_one_step() {
        let one = walked(50, 50);
        assert_eq!(one.0, 50);
        assert_eq!(
            walked(50, 7),
            one,
            "the key lands on frame 20, past the first batch"
        );
    }

    #[test]
    fn zero_ticks_still_issue_a_step() {
        let mut engine = Engine::builder().build().expect("the test engine builds");
        engine
            .load_inline_scene("format: 1\nentities: []\n")
            .unwrap();
        step_batched(&mut engine, 0, 7).expect("a zero step is fine");
        assert_eq!(engine.frame().as_u64(), 0);
    }

    #[test]
    fn a_step_past_the_limit_is_refused_and_batching_reaches_it() {
        let mut engine = Engine::builder().build().expect("the test engine builds");
        engine
            .load_inline_scene("format: 1\nentities: []\n")
            .unwrap();
        let over = ludion_protocol::command::MAX_STEP_TICKS + 1;
        assert!(
            step(
                &mut engine,
                SimStepParams {
                    ticks: over,
                    hash: false
                }
            )
            .is_err()
        );
        super::step_in_batches(&mut engine, over).expect("batches stay under the limit");
        assert_eq!(engine.frame().as_u64(), over);
    }
}
