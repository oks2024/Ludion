//! `replay_with`: the public replay entry point that hands back the driven engine and calls an
//! observer at every settled frame.

use std::ops::ControlFlow;

use ludion_core::time::FrameIndex;
use ludion_engine::{Engine, EngineBuilder, drive_hashes, replay_hashes_with, replay_with};
use ludion_protocol::Command;
use ludion_protocol::command::{InputInjectParams, SimStepParams};
use ludion_sim::{InputState, Key, WorldHash};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Advances every `Mover` one unit per tick while `W` is held.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct Mover {
    x: f32,
}

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

fn step(ticks: u64) -> Command {
    Command::SimStep(SimStepParams { ticks, hash: false })
}

fn key(kind: &str) -> Command {
    Command::InputInject(InputInjectParams {
        frame: None,
        event: json!({ "kind": kind, "key": "W" }),
    })
}

fn mover_x(engine: &mut Engine) -> f32 {
    engine
        .world_mut()
        .query::<(&Mover,)>()
        .next()
        .map(|(mover,)| mover.x)
        .unwrap()
}

/// Records a session (hold `W` for 4 of 7 ticks); returns the live engine, its trace and the bytes.
fn record(name: &str) -> (Engine, Vec<(FrameIndex, WorldHash)>, Vec<u8>) {
    let path = std::env::temp_dir().join(format!(
        "ludion-replay-with-{}-{name}.lrp",
        std::process::id()
    ));
    let mut engine = game().build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine
        .set_recording(std::fs::File::create(&path).unwrap())
        .unwrap();
    let trace = drive_hashes(
        &mut engine,
        vec![step(2), key("key_down"), step(4), key("key_up"), step(1)],
    )
    .unwrap();
    engine.stop_recording().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    (engine, trace, bytes)
}

#[test]
fn a_replay_hands_back_the_driven_engine() {
    let (mut live, trace, bytes) = record("engine");
    assert_eq!(mover_x(&mut live), 4.0);

    let mut replayed = replay_with(game(), &bytes, std::env::temp_dir(), |_, _| {
        Ok(ControlFlow::Continue(()))
    })
    .unwrap();

    assert_eq!(mover_x(&mut replayed), 4.0);
    assert_eq!(replayed.frame(), live.frame());
    assert_eq!(replayed.world_hash().unwrap(), trace.last().unwrap().1);
}

#[test]
fn the_observer_sees_every_settled_frame_and_matches_the_hash_trace() {
    let (_, _, bytes) = record("observer");
    let mut seen = Vec::new();
    replay_with(game(), &bytes, std::env::temp_dir(), |engine, frame| {
        seen.push((frame, engine.world_hash()?));
        Ok(ControlFlow::Continue(()))
    })
    .unwrap();

    let hashes = replay_hashes_with(game(), &bytes, std::env::temp_dir()).unwrap();
    assert_eq!(seen, hashes);
    assert_eq!(seen.len(), 8, "frames 0..=7");
}

#[test]
fn break_stops_the_drive_at_that_frame() {
    let (_, _, bytes) = record("break");
    let mut calls = 0;
    let engine = replay_with(game(), &bytes, std::env::temp_dir(), |_, frame| {
        calls += 1;
        Ok(if frame.as_u64() == 3 {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        })
    })
    .unwrap();

    assert_eq!(engine.frame(), FrameIndex::new(3));
    assert_eq!(calls, 4, "frames 0..=3, and no call after the Break");
}
