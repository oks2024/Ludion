//! Live free-running recording round-trip (audit finding 1.2).
//!
//! A live host (`ludion::run --record`) free-runs through [`Engine::advance_live`] — it never
//! dispatches a `sim.step`, so before this fix its recording captured no frame-advancing records and
//! the `.lrp` replayed to a ~1-frame trace. This test drives a *free-running* (Running) engine exactly
//! as the live loop does (`advance_live` for ticks, `execute(input.inject)` for input), records it, and
//! asserts the recording replays to the identical per-frame `WorldHash` trace — the guarantee the
//! `--record` flag makes and nothing exercised before.

use ludion_core::time::FrameIndex;
use ludion_engine::{Engine, EngineBuilder, replay_hashes_with};
use ludion_protocol::Command;
use ludion_protocol::command::InputInjectParams;
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use ludion_sim::{InputState, Key, WorldHash};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// A component a system drives from input, so the `WorldHash` moves exactly when `W` is held — the
/// proof the replay reproduces the input, not just the tick count.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct Mover {
    x: f32,
}

/// Advances every `Mover` one unit per tick while `W` is held.
fn drift(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
    let held = world
        .resource::<InputState>()
        .is_some_and(|input| input.is_held(Key::W));
    if held {
        for (mover,) in world.query::<(&mut Mover,)>() {
            mover.x += 1.0;
        }
    }
}

fn game() -> EngineBuilder {
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

/// One free-running live tick, returning the `(frame, hash)` sample after it — the caller pushes it,
/// mirroring how `replay::drive_actions` samples a `sim.step` (one hash per tick).
fn tick(engine: &mut Engine) -> (FrameIndex, WorldHash) {
    assert!(engine.advance_live(), "a running engine ticks");
    (engine.frame(), engine.world_hash().unwrap())
}

/// Inject an input at the current frame (host sends `frame: None`), returning the refined
/// `(frame, hash)` — the caller overwrites the last sample, mirroring how `drive_actions` refines the
/// current frame's hash for a non-stepping command.
fn inject(engine: &mut Engine, event: serde_json::Value) -> (FrameIndex, WorldHash) {
    engine
        .execute(Command::InputInject(InputInjectParams {
            frame: None,
            event,
        }))
        .unwrap();
    (engine.frame(), engine.world_hash().unwrap())
}

#[test]
fn live_free_running_recording_replays_to_the_live_worldhash_trace() {
    let path = std::env::temp_dir().join(format!("ludion-live-record-{}.lrp", std::process::id()));

    let mut engine = game().build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine
        .set_recording(std::fs::File::create(&path).unwrap())
        .unwrap();

    // The live run: a stretch of free-run ticks, hold `W`, more ticks, release, more ticks — all via
    // `advance_live` (never a `sim.step`), which is exactly what the free-running host does.
    let mut live: Vec<(FrameIndex, WorldHash)> =
        vec![(engine.frame(), engine.world_hash().unwrap())];
    for _ in 0..5 {
        live.push(tick(&mut engine));
    }
    *live.last_mut().unwrap() = inject(&mut engine, json!({ "kind": "key_down", "key": "W" }));
    for _ in 0..10 {
        live.push(tick(&mut engine));
    }
    *live.last_mut().unwrap() = inject(&mut engine, json!({ "kind": "key_up", "key": "W" }));
    for _ in 0..5 {
        live.push(tick(&mut engine));
    }
    engine.stop_recording().unwrap();

    // The session genuinely advanced 20 frames and the input moved the world (10 ticks of held `W`).
    assert_eq!(live.len(), 21, "the live trace samples every frame");
    let moved = engine
        .world_mut()
        .query::<(&Mover,)>()
        .next()
        .map(|(mover,)| mover.x)
        .unwrap();
    assert_eq!(moved, 10.0, "10 ticks of held `W` advanced the Mover");

    // The recording replays to the identical per-frame trace — the guarantee the old ~1-frame live
    // recording could not make.
    let bytes = std::fs::read(&path).unwrap();
    let replayed = replay_hashes_with(game(), &bytes, std::env::temp_dir()).unwrap();
    assert!(
        replayed.len() > 1,
        "the replay advanced past frame 0 (the bug produced a ~1-frame trace): {} frames",
        replayed.len()
    );
    assert_eq!(
        live, replayed,
        "a free-running live recording must replay to the live WorldHash trace exactly"
    );
}

#[test]
fn dropping_a_recording_engine_flushes_the_trailing_ticks() {
    // The live host ends its recording by *dropping* the engine — the sim thread returns from
    // `run_live` on shutdown and the owned `Engine` goes out of scope, with no explicit
    // `stop_recording`. The `Drop` impl must flush the batched trailing free-run ticks (and the sink),
    // or the tail of the session — everything since the last keyframe — is silently lost.
    let path = std::env::temp_dir().join(format!("ludion-live-drop-{}.lrp", std::process::id()));

    let live = {
        let mut engine = game().build().unwrap();
        engine.load_inline_scene(SCENE).unwrap();
        engine
            .set_recording(std::fs::File::create(&path).unwrap())
            .unwrap();
        let mut live: Vec<(FrameIndex, WorldHash)> =
            vec![(engine.frame(), engine.world_hash().unwrap())];
        for _ in 0..12 {
            live.push(tick(&mut engine));
        }
        live
        // engine dropped here — no stop_recording(); Drop must finalize the file.
    };

    let bytes = std::fs::read(&path).unwrap();
    let keyframes: Vec<u64> = ReplayReader::new(&bytes)
        .unwrap()
        .records()
        .unwrap()
        .iter()
        .filter_map(|record| match record {
            ReplayRecord::Keyframe { frame, .. } => Some(frame.as_u64()),
            _ => None,
        })
        .collect();
    assert_eq!(keyframes, [0, 12], "the drop writes the closing keyframe");
    let replayed = replay_hashes_with(game(), &bytes, std::env::temp_dir()).unwrap();
    assert_eq!(
        replayed.len(),
        live.len(),
        "every trailing tick survived the drop-finalized recording",
    );
    assert_eq!(
        live, replayed,
        "a recording finalized by dropping the engine must replay to the live trace exactly",
    );
}
