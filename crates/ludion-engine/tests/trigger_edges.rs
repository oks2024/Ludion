//! Trigger edges: a trigger pulled past `TRIGGER_THRESHOLD` presses its synthesised button, the
//! edges feed the world hash and replay identically, a held trigger survives snapshot/restore, and
//! a trigger button event is accepted over `input.inject`.

use ludion_engine::{Engine, EngineBuilder, drive_hashes, replay_hashes_with};
use ludion_protocol::command::{EngineRestoreParams, InputInjectParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};
use ludion_sim::{GamepadButton, InputState};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Counts right-trigger press and release edges.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct Shots {
    pressed: u32,
    released: u32,
}

fn count(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
    let (pressed, released) = world
        .resource::<InputState>()
        .map(|input| {
            let pad = input.gamepad();
            (
                pad.was_pressed(GamepadButton::RightTrigger),
                pad.was_released(GamepadButton::RightTrigger),
            )
        })
        .unwrap_or_default();
    for (shots,) in world.query::<(&mut Shots,)>() {
        shots.pressed += u32::from(pressed);
        shots.released += u32::from(released);
    }
}

fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<Shots>("Shots"))
        .system("count", count)
}

const SCENE: &str = "\
format: 1
seed: 1
entities:
  - components:
      Shots:
        pressed: 0
        released: 0
";

fn step(ticks: u64) -> Command {
    Command::SimStep(SimStepParams { ticks, hash: false })
}

fn inject(event: Value) -> Command {
    Command::InputInject(InputInjectParams { frame: None, event })
}

fn trigger(value: f32) -> Command {
    inject(json!({
        "kind": "gamepad_axis",
        "axis": "right_trigger",
        "value_bits": value.to_bits(),
    }))
}

fn shots(engine: &mut Engine) -> (u32, u32) {
    engine
        .world_mut()
        .query::<(&Shots,)>()
        .next()
        .map(|(s,)| (s.pressed, s.released))
        .unwrap()
}

fn pad(engine: &Engine) -> &ludion_sim::GamepadState {
    engine.world().resource::<InputState>().unwrap().gamepad()
}

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

#[test]
fn trigger_edges_feed_the_hash_and_replay_identically() {
    let path =
        std::env::temp_dir().join(format!("ludion-trigger-edges-{}.lrp", std::process::id()));
    let mut engine = game().build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine
        .set_recording(std::fs::File::create(&path).unwrap())
        .unwrap();
    let mut commands = Vec::new();
    for v in [0.3, 0.6, 0.55, 0.4, 0.9, 0.0] {
        commands.push(trigger(v));
        commands.push(step(1));
    }
    let trace = drive_hashes(&mut engine, commands).unwrap();
    engine.stop_recording().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);

    assert_eq!(shots(&mut engine), (2, 2));

    let replayed = replay_hashes_with(game(), &bytes, std::env::temp_dir()).unwrap();
    assert_eq!(replayed, trace);
}

#[test]
fn a_held_trigger_survives_snapshot_and_restore() {
    let mut engine = game().build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine.execute(trigger(1.0)).unwrap();
    engine.advance_live();
    let snapshot = snapshot_of(&mut engine);
    let held = snapshot
        .pointer("/input/gamepad/held")
        .and_then(Value::as_array)
        .expect("held list");
    assert!(held.contains(&json!("right_trigger")), "{held:?}");

    let mut fresh = game().build().unwrap();
    fresh
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    fresh.advance_live();
    assert!(pad(&fresh).is_held(GamepadButton::RightTrigger));
    assert!(!pad(&fresh).was_pressed(GamepadButton::RightTrigger));
}

#[test]
fn a_trigger_button_event_is_accepted_and_sets_the_axis() {
    let mut engine = game().build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine
        .execute(inject(
            json!({"kind": "gamepad_button_down", "button": "left_trigger"}),
        ))
        .unwrap();
    engine.advance_live();
    assert_eq!(pad(&engine).left_trigger(), 1.0);
    assert!(pad(&engine).is_held(GamepadButton::LeftTrigger));
}
