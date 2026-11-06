//! Held input across the three rebuilds: a scene switch carries the held
//! keys, the cursor and the pending injected events; `engine.restore` continues the snapshot's held
//! input; `scene.load` starts a new session with none. Edges never re-fire after a carry.

use ludion_core::ids::EntityId;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Engine, EngineBuilder, SceneSwitch};
use ludion_protocol::command::{EngineRestoreParams, InputInjectParams, SceneLoadParams};
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use ludion_protocol::{Command, ResponseBody};
use ludion_sim::{InputState, Key};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Player {
    hp: u32,
}

const START: &str = "\
format: 1
seed: 7
entities:
  - name: player
    components:
      Transform:
        translation:
          - 1.0
          - 2.0
          - 3.0
      Player:
        hp: 100
";

const NEXT: &str = "\
format: 1
seed: 999
entities:
  - name: gate-from-start
    components:
      Transform:
        translation:
          - 40.0
          - 0.0
          - -10.0
";

/// Requests a switch to `next.yaml` on frame 2, carrying the `Player`.
fn switch_to_next(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 2 {
        return;
    }
    let ids: Vec<EntityId> = world.entities().collect();
    let Some(player) = ids
        .into_iter()
        .find(|&id| matches!(world.get::<Player>(id), Ok(Some(_))))
    else {
        return;
    };
    if let Some(switch) = world.resource_mut::<SceneSwitch>() {
        switch.request(
            "next.yaml".to_string(),
            "gate-from-start".to_string(),
            vec![player],
        );
    }
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-held-input-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("start.yaml"), START).unwrap();
    std::fs::write(dir.join("next.yaml"), NEXT).unwrap();
    dir
}

fn switch_game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .components(|reg| {
            reg.register::<Player>("Player")?;
            Ok(())
        })
        .system("switch_to_next", switch_to_next)
        .project_root(root)
}

fn inject(engine: &mut Engine, frame: Option<u64>, event: Value) {
    engine
        .execute(Command::InputInject(InputInjectParams {
            frame: frame.map(ludion_core::time::FrameIndex::new),
            event,
        }))
        .unwrap();
}

fn input(engine: &Engine) -> &InputState {
    engine.world().resource::<InputState>().unwrap()
}

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

fn restore(engine: &mut Engine, snapshot: Value) {
    match engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
}

fn key_down(key: &str) -> Value {
    json!({ "kind": "key_down", "key": key })
}

fn booted(dir: &Path) -> Engine {
    let mut engine = switch_game(dir).build().expect("boots");
    engine.load_inline_scene(START).unwrap();
    engine
}

#[test]
fn a_switch_carries_held_keys_and_the_cursor_without_a_new_press() {
    let dir = fresh_dir("carry");
    let mut engine = booted(&dir);
    // `W` goes down on the switch frame itself, so its press edge is live when the world is swapped.
    inject(&mut engine, Some(2), key_down("W"));
    inject(
        &mut engine,
        Some(0),
        json!({ "kind": "mouse_move", "position_bits": [1094713344u32, 1107820544u32] }),
    );

    for _ in 0..3 {
        engine.advance_live();
    }
    assert!(engine.last_transition_error().is_none());
    assert!(input(&engine).is_held(Key::W), "W survives the switch");

    engine.advance_live(); // the first arrival tick
    let state = input(&engine);
    assert!(state.is_held(Key::W));
    assert!(
        !state.was_pressed(Key::W),
        "a carried key is not a new press"
    );
    assert_eq!(state.mouse().position(), [12.0, 34.0]);
}

#[test]
fn a_switch_carries_events_addressed_past_it() {
    let dir = fresh_dir("script");
    let mut engine = booted(&dir);
    inject(&mut engine, Some(5), key_down("Space"));

    for _ in 0..5 {
        engine.advance_live(); // frames 0..=4: the switch happens after frame 2
    }
    assert!(engine.last_transition_error().is_none());
    assert!(!input(&engine).is_held(Key::Space));

    engine.advance_live(); // frame 5, in the arriving district
    assert!(input(&engine).is_held(Key::Space));
    assert!(input(&engine).was_pressed(Key::Space));
}

#[test]
fn a_restore_continues_the_snapshots_held_input() {
    let dir = fresh_dir("restore");
    let mut engine = booted(&dir);
    inject(&mut engine, None, key_down("W"));
    engine.advance_live();
    let snapshot = snapshot_of(&mut engine);
    assert!(snapshot.get("input").is_some());

    let mut fresh = switch_game(&dir).build().expect("boots");
    restore(&mut fresh, snapshot);
    assert!(input(&fresh).is_held(Key::W));

    // A live restore over a session that has since released everything brings the key back too.
    inject(&mut engine, None, json!({ "kind": "key_up", "key": "W" }));
    engine.advance_live();
    assert!(!input(&engine).is_held(Key::W));
    restore(&mut engine, snapshot_of(&mut fresh));
    assert!(input(&engine).is_held(Key::W));
}

#[test]
fn an_idle_snapshot_has_no_input_key_and_an_older_document_restores_idle() {
    let dir = fresh_dir("idle");
    let mut engine = booted(&dir);
    engine.advance_live();
    let snapshot = snapshot_of(&mut engine);
    assert!(snapshot.get("input").is_none(), "idle input is omitted");
    assert!(
        snapshot.get("pending_input").is_none(),
        "an empty script is omitted"
    );

    // A document with the field removed (an older snapshot) restores idle even into a session
    // that is holding a key.
    let mut holder = booted(&dir);
    inject(&mut holder, None, key_down("W"));
    holder.advance_live();
    assert!(input(&holder).is_held(Key::W));
    restore(&mut holder, snapshot);
    assert!(input(&holder).is_idle());
}

#[test]
fn a_scene_load_starts_with_no_held_input() {
    let dir = fresh_dir("load");
    let mut engine = booted(&dir);
    inject(&mut engine, None, key_down("W"));
    engine.advance_live();
    assert!(input(&engine).is_held(Key::W));

    engine
        .execute(Command::SceneLoad(SceneLoadParams {
            path: "start.yaml".into(),
        }))
        .unwrap();
    assert!(input(&engine).is_idle());
}

#[test]
fn a_restore_keeps_input_injected_for_a_later_frame() {
    let dir = fresh_dir("pending");
    let plain = |dir: &Path| {
        let mut engine = Engine::builder()
            .components(|reg| {
                reg.register::<Player>("Player")?;
                Ok(())
            })
            .project_root(dir)
            .build()
            .expect("boots");
        engine.load_inline_scene(START).unwrap();
        engine
    };
    let mut engine = plain(&dir);
    inject(&mut engine, Some(6), key_down("Space"));
    for _ in 0..3 {
        engine.advance_live();
    }
    let snapshot = snapshot_of(&mut engine);

    // A fresh engine, as `ludion dev` restarts into.
    let mut fresh = plain(&dir);
    restore(&mut fresh, snapshot);
    for _ in 0..3 {
        fresh.advance_live();
        assert!(!input(&fresh).is_held(Key::Space), "not before frame 6");
    }
    fresh.advance_live(); // frame 6
    assert!(input(&fresh).was_pressed(Key::Space));
}

#[test]
fn a_snapshot_with_a_pending_input_for_a_past_frame_is_refused() {
    let dir = fresh_dir("pending-past");
    let mut engine = booted(&dir);
    for _ in 0..3 {
        engine.advance_live();
    }
    let mut snapshot = snapshot_of(&mut engine);
    snapshot["pending_input"] = json!([{
        "frame": 1,
        "event": { "kind": "key_down", "key": "Space" }
    }]);
    let err = engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap_err();
    assert_eq!(
        err.code,
        ludion_core::codes::code::PROTOCOL_SNAPSHOT_INVALID
    );
    assert!(
        err.path
            .as_deref()
            .is_some_and(|p| p.starts_with("/pending_input/0"))
    );
}

/// A recording sink shared with the test so the recorded bytes can be read back.
#[derive(Clone, Default)]
struct SharedSink(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn injecting_at_a_frame_that_has_already_run_is_refused_and_not_recorded() {
    let dir = fresh_dir("inject-past");
    let mut engine = booted(&dir);
    let sink = SharedSink::default();
    engine.set_recording(sink.clone()).unwrap();
    for _ in 0..3 {
        engine.advance_live();
    }

    let inject_w = |engine: &mut Engine, frame: u64| {
        engine.execute(Command::InputInject(InputInjectParams {
            frame: Some(ludion_core::time::FrameIndex::new(frame)),
            event: key_down("W"),
        }))
    };
    let refused = inject_w(&mut engine, 1).unwrap_err();
    assert_eq!(
        refused.code,
        ludion_core::codes::code::PROTOCOL_INVALID_INPUT
    );
    assert!(
        refused.message.contains("frame 1") && refused.message.contains("frame 3"),
        "{}",
        refused.message
    );
    engine.advance_live();
    assert!(
        !input(&engine).is_held(Key::W),
        "the refused event applied nowhere"
    );

    // The current frame is "now" and is accepted.
    inject_w(&mut engine, 4).expect("the current frame is accepted");
    engine.advance_live();
    assert!(input(&engine).is_held(Key::W));

    engine.stop_recording().unwrap();
    let bytes = sink.0.lock().unwrap().clone();
    let inputs: Vec<_> = ReplayReader::new(&bytes)
        .unwrap()
        .records()
        .unwrap()
        .into_iter()
        .filter_map(|r| match r {
            ReplayRecord::Input { frame, .. } => Some(frame.as_u64()),
            _ => None,
        })
        .collect();
    assert_eq!(inputs, vec![4], "only the accepted inject is recorded");
}

#[test]
fn folded_modifier_keys_inject_and_edge_like_any_key() {
    let dir = fresh_dir("modifiers");
    let mut engine = booted(&dir);
    for (name, key) in [
        ("Shift", Key::Shift),
        ("Ctrl", Key::Ctrl),
        ("Alt", Key::Alt),
    ] {
        inject(&mut engine, None, key_down(name));
        engine.advance_live();
        assert!(input(&engine).is_held(key) && input(&engine).was_pressed(key));
        engine.advance_live();
        assert!(input(&engine).is_held(key) && !input(&engine).was_pressed(key));
        inject(&mut engine, None, json!({ "kind": "key_up", "key": name }));
        engine.advance_live();
        assert!(!input(&engine).is_held(key) && input(&engine).was_released(key));
    }

    let refused = engine.execute(Command::InputInject(InputInjectParams {
        frame: None,
        event: key_down("LeftShift"),
    }));
    assert!(refused.is_err(), "there is no per-side modifier key");
}

#[test]
fn injected_mouse_motion_accumulates_within_a_frame_and_clears_the_next() {
    let dir = fresh_dir("motion");
    let mut engine = booted(&dir);
    let motion = |x: f32, y: f32| json!({ "kind": "mouse_motion", "delta_bits": [x.to_bits(), y.to_bits()] });
    inject(&mut engine, Some(1), motion(3.0, -1.5));
    inject(&mut engine, Some(1), motion(0.25, 4.0));

    engine.advance_live(); // frame 0
    assert_eq!(input(&engine).mouse().delta(), [0.0, 0.0]);
    engine.advance_live(); // frame 1
    assert_eq!(input(&engine).mouse().delta(), [3.25, 2.5]);
    assert_eq!(input(&engine).mouse().position(), [0.0, 0.0]);
    engine.advance_live(); // frame 2
    assert_eq!(input(&engine).mouse().delta(), [0.0, 0.0]);

    let error = engine
        .execute(Command::InputInject(InputInjectParams {
            frame: None,
            event: motion(f32::NAN, 0.0),
        }))
        .unwrap_err();
    assert_eq!(error.code, ludion_core::codes::code::PROTOCOL_INVALID_INPUT);
}

#[test]
fn non_finite_or_out_of_range_analogs_are_refused_and_leave_input_unchanged() {
    let dir = fresh_dir("analog-validation");
    let mut engine = booted(&dir);
    let stick =
        |bits: u32| json!({ "kind": "gamepad_axis", "axis": "left_stick_x", "value_bits": bits });
    let refused = [
        stick(f32::NAN.to_bits()),
        stick(1.5f32.to_bits()),
        json!({
            "kind": "mouse_move",
            "position_bits": [f32::INFINITY.to_bits(), 0.0f32.to_bits()],
        }),
    ];
    for event in refused {
        let error = engine
            .execute(Command::InputInject(InputInjectParams {
                frame: None,
                event,
            }))
            .unwrap_err();
        assert_eq!(error.code, ludion_core::codes::code::PROTOCOL_INVALID_INPUT);
    }
    engine.advance_live();
    assert_eq!(input(&engine).gamepad().left_stick(), [0.0, 0.0]);
    assert_eq!(input(&engine).mouse().position(), [0.0, 0.0]);

    inject(&mut engine, None, stick(1.0f32.to_bits()));
    engine.advance_live();
    assert_eq!(input(&engine).gamepad().left_stick(), [1.0, 0.0]);
}
