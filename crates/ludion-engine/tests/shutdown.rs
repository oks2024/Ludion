//! The quit a game system asks for: the [`Shutdown`] resource a title screen's back binding sets,
//! honored at the frame boundary through the same path the wire `engine.shutdown` takes.
//!
//! Three claims, over the ordinary public surface (a system, `Engine`, a recording):
//!
//! - **The request lands at the boundary, not inside the tick.** `shutdown_requested()` is false
//!   before the requesting frame's boundary and true after it — and false on every frame before.
//! - **An idle engine never quits.** A session whose systems leave the resource alone runs on.
//! - **A recorded session that quits replays to the same trace, ending at the quit frame.** A quit
//!   mutates no world state, so it writes no record; the recording simply stops there, and the
//!   replay reproduces every frame up to it and no more.

use ludion_core::time::FrameIndex;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Engine, EngineBuilder, Shutdown, replay_hashes_with};
use ludion_sim::WorldHash;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The frame the quitting system asks on — far enough in that a replay that stopped early would show
/// it as a short trace.
const QUIT_FRAME: u64 = 7;

/// A component a system moves every tick, so the `WorldHash` differs frame to frame and a replay that
/// ends one frame early is visible in the trace.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct Mover {
    x: f32,
}

fn drift(world: &mut World, _ctx: &SystemCtx) {
    for (mover,) in world.query::<(&mut Mover,)>() {
        mover.x += 1.0;
    }
}

/// The title screen's back binding, reduced to its one line: on [`QUIT_FRAME`], ask the host to stop.
fn quit_on_cue(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == QUIT_FRAME
        && let Some(quit) = world.resource_mut::<Shutdown>()
    {
        quit.request();
    }
}

fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<Mover>("Mover"))
        .system("drift", drift)
        .system("quit_on_cue", quit_on_cue)
}

/// The same game with the quitting system removed — the idle control.
fn idle_game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<Mover>("Mover"))
        .system("drift", drift)
}

const SCENE: &str = "\
format: 1
seed: 1
entities:
  - components:
      Mover:
        x: 0.0
";

#[test]
fn a_system_request_flags_shutdown_at_that_frames_boundary() {
    let mut engine = game().build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();

    // Every frame before the cue leaves the flag alone, and the resource is empty at rest.
    for _ in 0..QUIT_FRAME {
        assert!(
            !engine.shutdown_requested(),
            "frame {} must not have requested a quit",
            engine.frame().as_u64()
        );
        assert!(engine.advance_live(), "a running engine ticks");
        assert!(
            !engine
                .world()
                .resource::<Shutdown>()
                .expect("the engine installs the resource on every world it builds")
                .is_pending(),
            "the boundary drains the request, so it is never left pending"
        );
    }

    // The tick that runs `quit_on_cue` is the one that flips it — at its boundary, after the systems.
    assert!(!engine.shutdown_requested());
    assert!(engine.advance_live());
    assert_eq!(engine.frame(), FrameIndex::new(QUIT_FRAME + 1));
    assert!(
        engine.shutdown_requested(),
        "the frame-boundary drain routes the request into the engine's shutdown path"
    );
}

#[test]
fn an_idle_engine_never_requests_shutdown() {
    let mut engine = idle_game().build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    for _ in 0..(QUIT_FRAME + 5) {
        assert!(engine.advance_live());
    }
    assert!(
        !engine.shutdown_requested(),
        "a game that never touches the resource keeps running"
    );
}

#[test]
fn a_recorded_session_that_quits_replays_to_the_quit_frame() {
    let path = std::env::temp_dir().join(format!("ludion-shutdown-{}.lrp", std::process::id()));

    let mut engine = game().build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine
        .set_recording(std::fs::File::create(&path).unwrap())
        .unwrap();

    // Drive it exactly as `run_live` does: tick, then leave the loop the moment the boundary flags a
    // quit. The trace is the live one the replay must reproduce.
    let mut live: Vec<(FrameIndex, WorldHash)> =
        vec![(engine.frame(), engine.world_hash().unwrap())];
    while !engine.shutdown_requested() {
        assert!(engine.advance_live(), "a running engine ticks");
        live.push((engine.frame(), engine.world_hash().unwrap()));
        assert!(
            live.len() < 64,
            "the quitting system must end the session long before this"
        );
    }
    engine.stop_recording().unwrap();

    assert_eq!(
        engine.frame(),
        FrameIndex::new(QUIT_FRAME + 1),
        "the session stopped on the frame the system asked to quit"
    );

    let bytes = std::fs::read(&path).unwrap();
    let replayed = replay_hashes_with(game(), &bytes, std::env::temp_dir()).unwrap();
    assert_eq!(
        live, replayed,
        "a recording ended by a system's quit must replay to the live trace, ending at the same frame"
    );
    let _ = std::fs::remove_file(&path);
}
