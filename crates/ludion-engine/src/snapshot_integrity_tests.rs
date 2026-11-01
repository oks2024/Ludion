//! Snapshot, restore and recording integrity: a restore refuses a frame it cannot step from, keeps
//! (and reports) a value that fails its schema, a capture refuses a non-finite world, and a recording
//! skips the keyframe such a world cannot give instead of dying. These live in the crate because they
//! read the pending `Diagnostics` and the event queue, which are not public.

use std::sync::{Arc, Mutex};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::{SystemCtx, World};
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use ludion_protocol::{Command, ResponseBody};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::components::{Mesh, Transform};
use crate::{Engine, EngineBuilder, KEYFRAME_INTERVAL, replay_hashes_with};

const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: cube
    components:
      Transform: {}
      Mesh: {}
";

/// Writes a NaN pose for frames `[299, 310)` and a finite one otherwise: a deterministic fault
/// straddling the first keyframe boundary, so a replay sees the same world.
fn poison(world: &mut World, ctx: &SystemCtx) {
    let frame = ctx.frame.as_u64();
    for (transform,) in world.query::<(&mut Transform,)>() {
        transform.translation[0] = if (299..310).contains(&frame) {
            f32::NAN
        } else {
            0.0
        };
    }
}

fn builder() -> EngineBuilder {
    EngineBuilder::new().system("poison", poison)
}

fn engine() -> Engine {
    let mut engine = builder().build().expect("boots");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    engine
}

#[derive(Clone, Default)]
struct SharedSink(Arc<Mutex<Vec<u8>>>);

impl SharedSink {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().clone()
    }
}

impl std::io::Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn step(ticks: u64, hash: bool) -> Command {
    Command::SimStep(SimStepParams { ticks, hash })
}

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

fn restore(engine: &mut Engine, snapshot: Value) -> Result<(), Diagnostic> {
    engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .map(|_| ())
}

/// Everything reported to `Diagnostics` and not yet drained, plus what a frame boundary already
/// sent to the `log` channel.
fn reported(engine: &mut Engine) -> Vec<Diagnostic> {
    let mut found: Vec<Diagnostic> = engine
        .take_events()
        .into_iter()
        .filter_map(|event| match event {
            ludion_protocol::Event::Log(log) => log.diagnostic,
            _ => None,
        })
        .collect();
    if let Some(pending) = engine
        .world_mut()
        .resource_mut::<crate::Diagnostics>()
        .map(|d| d.take())
    {
        found.extend(pending);
    }
    found
}

fn keyframe_frames(bytes: &[u8]) -> Vec<u64> {
    ReplayReader::new(bytes)
        .unwrap()
        .records()
        .unwrap()
        .iter()
        .filter_map(|record| match record {
            ReplayRecord::Keyframe { frame, .. } => Some(frame.as_u64()),
            _ => None,
        })
        .collect()
}

fn assert_replays_to_live(engine: &mut Engine, sink: &SharedSink) {
    let replayed = replay_hashes_with(builder(), &sink.bytes(), std::env::temp_dir())
        .expect("the recording replays without error");
    let live = (engine.frame(), engine.world_hash().unwrap());
    assert_eq!(
        replayed.last(),
        Some(&live),
        "replay ends where the live run did"
    );
}

#[test]
fn a_restore_frame_past_the_headroom_is_refused_untouched() {
    let mut engine = engine();
    let mut snapshot = snapshot_of(&mut engine);
    snapshot["frame"] = json!(u64::MAX);
    let refused = restore(&mut engine, snapshot).expect_err("no headroom to tick from");
    assert_eq!(refused.code, code::PROTOCOL_SNAPSHOT_INVALID);
    assert_eq!(refused.path.as_deref(), Some("/frame"));
    assert_eq!(engine.frame().as_u64(), 0, "the engine was not touched");
}

#[test]
fn a_value_that_fails_its_schema_is_kept_and_reported() {
    let mut engine = engine();
    let mut snapshot = snapshot_of(&mut engine);
    let entities = snapshot["entities"].as_array_mut().unwrap();
    let at = entities
        .iter()
        .position(|e| e["components"].get("Mesh").is_some())
        .expect("the cube");
    entities[at]["components"]["Mesh"]["roughness"] = json!(5.0);

    restore(&mut engine, snapshot).expect("a schema failure never refuses a restore");
    let cube = engine
        .world()
        .entities()
        .find(|&id| matches!(engine.world().get::<Mesh>(id), Ok(Some(_))))
        .unwrap();
    assert_eq!(
        engine.world().get::<Mesh>(cube).unwrap().unwrap().roughness,
        5.0,
        "the live value is restored exactly"
    );
    let found = reported(&mut engine);
    let diagnostic = found
        .iter()
        .find(|d| d.code.as_str() == "L1014")
        .unwrap_or_else(|| panic!("the schema failure is reported: {found:?}"));
    let path = diagnostic.path.as_deref().unwrap();
    assert!(
        path.starts_with(&format!("/entities/{at}/components/Mesh")) && path.contains("roughness"),
        "{path}"
    );
}

#[test]
fn a_non_finite_component_refuses_the_snapshot_naming_it() {
    let mut engine = engine();
    let cube = engine.world().entities().next().unwrap();
    engine
        .world_mut()
        .get_mut::<Transform>(cube)
        .unwrap()
        .unwrap()
        .translation[1] = f32::INFINITY;
    let refused = engine
        .execute(Command::EngineSnapshot)
        .expect_err("an infinity cannot be stored");
    assert_eq!(refused.code, code::COMPONENT_SERIALIZE_FAILED);
    assert!(
        refused.message.contains("`Transform`"),
        "{}",
        refused.message
    );
    assert!(
        refused.message.contains(&cube.to_string()),
        "{}",
        refused.message
    );
}

#[test]
fn a_free_run_recording_skips_the_keyframe_a_non_finite_world_cannot_give() {
    let sink = SharedSink::default();
    let mut engine = engine();
    engine.set_recording(sink.clone()).unwrap();
    for _ in 0..(2 * KEYFRAME_INTERVAL + 20) {
        engine.advance_live();
    }
    assert!(engine.is_recording(), "the skip never stops the recording");
    let found = reported(&mut engine);
    assert!(
        found.iter().any(
            |d| d.code == code::COMPONENT_SERIALIZE_FAILED && d.message.contains("`Transform`")
        ),
        "the skip is reported: {found:?}"
    );
    engine.stop_recording().unwrap();

    let frames = keyframe_frames(&sink.bytes());
    assert!(!frames.contains(&KEYFRAME_INTERVAL), "{frames:?}");
    assert!(frames.contains(&(2 * KEYFRAME_INTERVAL)), "{frames:?}");
    assert_replays_to_live(&mut engine, &sink);
}

#[test]
fn a_lockstep_recording_skips_the_same_keyframe() {
    let sink = SharedSink::default();
    let mut engine = engine();
    engine.set_recording(sink.clone()).unwrap();
    engine.execute(step(KEYFRAME_INTERVAL + 5, false)).unwrap();
    assert!(engine.is_recording());
    assert!(
        reported(&mut engine)
            .iter()
            .any(|d| d.code == code::COMPONENT_SERIALIZE_FAILED),
        "the skip is reported"
    );
    engine.execute(step(2 * KEYFRAME_INTERVAL, false)).unwrap();
    engine.stop_recording().unwrap();

    let frames = keyframe_frames(&sink.bytes());
    assert!(!frames.contains(&(KEYFRAME_INTERVAL + 5)), "{frames:?}");
    assert!(
        frames.len() >= 2,
        "a later keyframe was written: {frames:?}"
    );
    assert_replays_to_live(&mut engine, &sink);
}

#[test]
fn a_closing_keyframe_of_a_non_finite_world_is_skipped_and_stop_succeeds() {
    let sink = SharedSink::default();
    let mut engine = engine();
    engine.set_recording(sink.clone()).unwrap();
    engine.execute(step(300, false)).unwrap();
    // Frame 300 is a boundary that already skipped; the closing keyframe sees the same NaN world.
    engine.execute(step(1, false)).unwrap();
    engine
        .stop_recording()
        .expect("the closing skip is not an error");
    assert_eq!(keyframe_frames(&sink.bytes()), vec![0]);
}

/// A component whose `Serialize` fails once armed — a hash failure on demand.
#[derive(Deserialize, JsonSchema, Clone, Copy, Debug)]
struct Brittle(bool);

impl Serialize for Brittle {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.0 {
            Err(serde::ser::Error::custom("brittle"))
        } else {
            serializer.serialize_bool(false)
        }
    }
}

fn arm_brittle(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 2 {
        for (brittle,) in world.query::<(&mut Brittle,)>() {
            brittle.0 = true;
        }
    }
}

#[test]
fn a_step_whose_hash_fails_is_recorded_with_the_ticks_that_ran() {
    let sink = SharedSink::default();
    let mut engine = EngineBuilder::new()
        .components(|registry| registry.register::<Brittle>("Brittle"))
        .system("arm", arm_brittle)
        .build()
        .unwrap();
    engine
        .load_inline_scene(
            "format: 1\nseed: 1\nentities:\n  - name: b\n    components:\n      Brittle: false\n",
        )
        .unwrap();
    engine.set_recording(sink.clone()).unwrap();

    let failure = engine
        .execute(step(5, true))
        .expect_err("the hash cannot be computed");
    assert_eq!(failure.code, code::COMPONENT_SERIALIZE_FAILED);
    assert_eq!(engine.frame().as_u64(), 5, "the ticks ran");
    engine
        .stop_recording()
        .expect("the closing keyframe is skipped");

    let records = ReplayReader::new(&sink.bytes()).unwrap().records().unwrap();
    assert!(
        records.iter().any(|record| matches!(
            record,
            ReplayRecord::Command {
                command: Command::SimStep(SimStepParams { ticks: 5, .. }),
                ..
            }
        )),
        "the step that advanced the world is in the recording: {records:?}"
    );
}

#[test]
fn a_command_panic_stops_the_recording_and_keeps_the_ticks_before_it() {
    let sink = SharedSink::default();
    let mut engine = EngineBuilder::new().build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine.set_recording(sink.clone()).unwrap();
    for _ in 0..3 {
        engine.advance_live();
    }

    let diagnostic = engine.command_panicked("component.set", "the fuse blew");
    assert_eq!(diagnostic.code, code::COMMAND_PANICKED);
    assert!(
        diagnostic
            .help
            .as_deref()
            .unwrap()
            .contains("recording stops here"),
        "{diagnostic:?}"
    );
    assert!(!engine.is_recording());
    let stopped = engine
        .stop_recording()
        .expect_err("the death is returned once");
    assert_eq!(stopped.code, code::RECORDING_STOPPED);
    assert!(stopped.message.contains("L4016"), "{}", stopped.message);

    let records = ReplayReader::new(&sink.bytes()).unwrap().records().unwrap();
    assert!(
        records.iter().any(|record| matches!(
            record,
            ReplayRecord::Command {
                command: Command::SimStep(SimStepParams { ticks: 3, .. }),
                ..
            }
        )),
        "the ticks that ran before the command are present: {records:?}"
    );
}
