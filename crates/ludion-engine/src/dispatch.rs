//! `dispatch` — THE parity gate.
//!
//! One exhaustive `match` over every [`Command`] variant, with **no wildcard arm**: a capability that
//! has no handler is a compile error (architecture §2). Each arm is a one-line delegation — it
//! destructures the params and calls a plain handler in [`crate::commands`]; there is **no logic**
//! here, only routing. When a handler grows, its logic lives in the owning module (or a domain
//! crate), never in this file.
//!
//! The gate returns a [`Dispatched`]: the command's [`ResponseBody`] plus the [`FrameIndex`] at which
//! it applied and the gameplay frame at the same boundary — the values a response envelope stamps as
//! its `frame` and `game_frame`.

use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::FrameIndex;
use ludion_protocol::{Command, ResponseBody};

use crate::commands::{
    ai, asset, checkpoint, engine_cmds, entity, perf, prefab, render, scene, sequence, sim, stream,
};
use crate::engine::Engine;

/// A dispatched command's result: the success body and the frames it applied at.
///
/// The frame is the engine's frame *after* the command ran (a `sim.step` advances it); a response
/// envelope carries it as `frame` so causality and replay stitching are always available, beside
/// `game_frame`, the same boundary read through the world's `GameClock`.
#[derive(Clone, Debug, PartialEq)]
pub struct Dispatched {
    /// The command's success payload.
    pub body: ResponseBody,
    /// The frame the command applied at.
    pub frame: FrameIndex,
    /// The gameplay frame at the same boundary ([`Engine::game_frame`]); what a `started` stamp uses.
    pub game_frame: FrameIndex,
}

/// Routes `command` to its handler and stamps the applied frame — the single dispatch surface.
///
/// This is the pure parity gate. [`Engine::execute`](crate::Engine::execute) wraps it to record
/// mutating commands into an active session; call this directly when recording is not wanted.
///
/// # Errors
/// The handler's diagnostic (see each handler in the `commands` module).
///
/// # Example
/// ```
/// use ludion_engine::{dispatch, Engine};
/// use ludion_protocol::Command;
///
/// let mut engine = Engine::new();
/// engine.load_inline_scene("format: 1\nentities: []\n").unwrap();
/// let dispatched = dispatch(&mut engine, Command::SimHash).unwrap();
/// assert_eq!(dispatched.frame.as_u64(), 0);
/// ```
pub fn dispatch(engine: &mut Engine, command: Command) -> Result<Dispatched, Diagnostic> {
    let body = match command {
        Command::SceneLoad(params) => scene::load(engine, params)?,
        Command::SceneSave(params) => scene::save(engine, params)?,
        Command::SceneCheck(params) => scene::check(engine, params)?,
        Command::EntitySpawn(params) => entity::spawn(engine, params)?,
        Command::EntityDestroy(params) => entity::destroy(engine, params)?,
        Command::EntityReparent(params) => entity::reparent(engine, params)?,
        Command::PrefabSpawn(params) => prefab::spawn(engine, params)?,
        Command::PrefabCreate(params) => prefab::create(engine, params)?,
        Command::PrefabApply(params) => prefab::apply(engine, params)?,
        Command::PrefabRevert(params) => prefab::revert(engine, params)?,
        Command::PrefabUnpack(params) => prefab::unpack(engine, params)?,
        Command::PrefabOverrides(params) => prefab::overrides(engine, params)?,
        Command::MapSpawn(params) => prefab::spawn_map(engine, params)?,
        Command::PrefabDespawn(params) => prefab::despawn(engine, params)?,
        Command::ComponentSet(params) => entity::set(engine, params)?,
        Command::ComponentGet(params) => entity::get(engine, params)?,
        Command::ComponentRemove(params) => entity::remove(engine, params)?,
        Command::QueryEntities(params) => entity::query(engine, params)?,
        Command::SimPause => sim::pause(engine)?,
        Command::SimResume => sim::resume(engine)?,
        Command::SimStep(params) => sim::step(engine, params)?,
        Command::SimSetSpeed(params) => sim::set_speed(engine, params)?,
        Command::SimHash => sim::hash(engine)?,
        Command::InputInject(params) => sim::inject(engine, params)?,
        Command::RenderScreenshot(params) => render::screenshot(engine, params)?,
        Command::RenderDumpGraph => render::dump_graph(engine)?,
        Command::RenderDumpTimings => render::dump_timings(engine)?,
        Command::RenderBounds(params) => render::bounds(engine, params)?,
        Command::RenderGiRebake(params) => render::gi_rebake(engine, params)?,
        Command::RenderSize(params) => render::size(engine, params)?,
        Command::RenderView(params) => render::view(engine, params)?,
        Command::RenderDebug(params) => render::debug(engine, params)?,
        Command::StreamSubscribe(params) => stream::subscribe(engine, params)?,
        Command::StreamUnsubscribe(params) => stream::unsubscribe(engine, params)?,
        Command::AssetReload(params) => asset::reload(engine, params)?,
        Command::EngineDescribe => engine_cmds::describe(engine)?,
        Command::EngineSnapshot => engine_cmds::snapshot(engine)?,
        Command::EngineRestore(params) => engine_cmds::restore(engine, params)?,
        Command::EngineShutdown => engine_cmds::shutdown(engine)?,
        Command::ProfileCapture(params) => perf::capture(engine, params)?,
        Command::AiDescribe => ai::describe(engine)?,
        Command::CheckpointList => checkpoint::list(engine)?,
        Command::CheckpointCapture(params) => checkpoint::capture(engine, params)?,
        Command::CheckpointRestore(params) => checkpoint::restore(engine, params)?,
        Command::CheckpointDiscard(params) => checkpoint::discard(engine, params)?,
        Command::SequencePlay(params) => sequence::play(engine, params)?,
        Command::SequenceSeek(params) => sequence::seek(engine, params)?,
        Command::SequenceSkip(params) => sequence::skip(engine, params)?,
    };
    Ok(Dispatched {
        body,
        frame: engine.frame(),
        game_frame: engine.game_frame(),
    })
}
