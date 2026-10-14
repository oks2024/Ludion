//! Audio event-stream tests for `hello-paddle` (Phase 8 Wave 4).
//!
//! Audio is verified text-first, not with goldens: a scripted input produces an exact, replayable
//! stream of `(frame, sound)` pairs in the engine's in-process audio log. These tests assert that
//! stream, that it is deterministic across runs, that a recorded session regenerates it on replay,
//! and that the committed WAV actually decodes.

use std::fs::{self, File};
use std::path::PathBuf;

use hello_paddle::BOUNCE_SOUND;
use ludion::prelude::*;
use ludion_engine::{AudioMessage, LoopId};
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use ludion_testkit::audio::{audio_messages, audio_stream};
use serde_json::json;

const SCENE: &str = include_str!("../scenes/paddle.yaml");

fn loaded_engine() -> Engine {
    ludion_testkit::loaded_engine(hello_paddle::game(), SCENE)
}

/// Press `D` at frame 0, then run 60 ticks — the same script the determinism/replay tests drive.
fn run_commands() -> Vec<Command> {
    vec![
        Command::InputInject(command::InputInjectParams {
            frame: Some(FrameIndex::new(0)),
            event: json!({"kind": "key_down", "key": "D"}),
        }),
        Command::SimStep(command::SimStepParams {
            ticks: 60,
            hash: false,
        }),
    ]
}

fn drive(engine: &mut Engine, commands: &[Command]) {
    for command in commands {
        engine.execute(command.clone()).expect("command dispatches");
    }
}

/// The event-stream assertion: holding `D` slides the paddle across the arena edge (`x = 2.05`) on
/// frame 20 — 0.1 units/tick from 0 — emitting exactly one bounce sound, stamped with that frame.
#[test]
fn crossing_the_arena_edge_emits_one_bounce_at_frame_20() {
    let mut engine = loaded_engine();
    drive(&mut engine, &run_commands());
    assert_eq!(
        audio_stream(&engine),
        vec![(20, BOUNCE_SOUND.to_string())],
        "one bounce sound, on the frame the paddle crosses the arena edge"
    );
}

/// No input → the paddle never moves → no sound.
#[test]
fn no_input_emits_no_audio() {
    let mut engine = loaded_engine();
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks: 60,
            hash: false,
        }))
        .expect("sim steps");
    assert!(audio_stream(&engine).is_empty());
}

/// Determinism: two identical runs produce byte-identical audio streams.
#[test]
fn two_runs_produce_identical_audio_streams() {
    let run = || {
        let mut engine = loaded_engine();
        drive(&mut engine, &run_commands());
        audio_stream(&engine)
    };
    assert_eq!(run(), run());
}

/// Replay regenerates the stream: record a session, reconstruct its commands from the `.lrp`, drive a
/// fresh engine, and confirm it emits the identical audio stream — proving the stream is a pure
/// function of the sim (nothing extra is recorded, and none needs to be).
#[test]
fn recorded_session_replays_to_the_same_audio_stream() {
    let session = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("hello_paddle_audio.lrp");
    let _ = fs::remove_file(&session);

    // Live run, recorded.
    let mut engine = loaded_engine();
    engine
        .set_recording(File::create(&session).expect("create the replay sink"))
        .expect("arm recording");
    drive(&mut engine, &run_commands());
    engine.stop_recording().expect("flush the recording");
    let live = audio_stream(&engine);

    // Replay: reconstruct the command list from the recorded deltas and re-drive a fresh engine.
    let bytes = fs::read(&session).expect("read the recorded session");
    let reader = ReplayReader::new(&bytes).expect("open the replay");
    let mut commands = Vec::new();
    for record in reader.records().expect("read replay records") {
        match record {
            ReplayRecord::Command { command, .. } => commands.push(command),
            ReplayRecord::Input { frame, event } => {
                commands.push(Command::InputInject(command::InputInjectParams {
                    frame: Some(frame),
                    event,
                }));
            }
            ReplayRecord::Keyframe { .. } => {}
            // This game registers no doc kinds and never switches scenes, so its sessions contain
            // neither doc reloads, scene switches, mid-session scene loads, nor prefab texts, and
            // its systems never panic.
            ReplayRecord::DocReload { .. }
            | ReplayRecord::SceneSwitch { .. }
            | ReplayRecord::SceneLoad { .. }
            | ReplayRecord::PrefabText { .. }
            | ReplayRecord::SystemPanic { .. } => {}
        }
    }
    let mut replay_engine = loaded_engine();
    drive(&mut replay_engine, &commands);

    assert_eq!(live, audio_stream(&replay_engine));
    assert_eq!(live, vec![(20, BOUNCE_SOUND.to_string())]);
    let _ = fs::remove_file(&session);
}

/// The loop primitive from a game's side: an ambient bed started, resent at a new gain, and stopped
/// is one ordered run of messages in the stream, with the bed sounding at every frame in between —
/// the generic second-consumer proof, outside vigil.
#[test]
fn a_bed_started_and_stopped_reads_as_one_ordered_run_of_messages() {
    let mut engine = loaded_engine();
    let bed = LoopId::Sound(BOUNCE_SOUND.to_string());
    let step = |engine: &mut Engine| {
        engine
            .execute(Command::SimStep(command::SimStepParams {
                ticks: 1,
                hash: false,
            }))
            .expect("sim steps");
    };

    engine
        .world_mut()
        .resource_mut::<AudioQueue>()
        .expect("the engine installs AudioQueue")
        .loop_start(BOUNCE_SOUND, 0.3);
    step(&mut engine);
    engine
        .world_mut()
        .resource_mut::<AudioQueue>()
        .unwrap()
        .loop_set_gain(BOUNCE_SOUND, 0.6);
    step(&mut engine);
    engine
        .world_mut()
        .resource_mut::<AudioQueue>()
        .unwrap()
        .loop_stop(BOUNCE_SOUND);
    step(&mut engine);

    let messages = audio_messages(&engine);
    let gains: Vec<f32> = messages
        .iter()
        .filter_map(|(_, message)| match message {
            AudioMessage::LoopSync { id, gain, .. } if *id == bed => Some(*gain),
            _ => None,
        })
        .collect();
    assert_eq!(
        gains,
        vec![0.3, 0.6],
        "the resend is a gain change, in order"
    );
    assert_eq!(
        messages.last().map(|(_, message)| message),
        Some(&AudioMessage::LoopStop { id: bed }),
        "the stop is last; stream was {messages:?}"
    );
    // A loop sounds, so it shows up in the flattened stream; a stop names an id, not a path.
    assert_eq!(
        audio_stream(&engine),
        vec![(0, BOUNCE_SOUND.to_string()), (1, BOUNCE_SOUND.to_string())]
    );
}

/// One more scene entity, appended to `paddle.yaml`: a bell looping the bounce sound over a 40-unit
/// field, 20 units east of the camera. `<sound>` is filled in from `BOUNCE_SOUND`.
const BELL_ENTITY: &str = r#"  - name: bell
    components:
      Transform:
        translation:
          - 20.0
          - 0.0
          - 0.0
      AudioEmitter:
        sound: "<sound>"
        gain: 0.8
        range: 40.0
"#;

/// The one live entity carrying `C`; panics if the scene places none or more than one.
fn only_entity_with<C: Component>(engine: &mut Engine) -> EntityId {
    let ids: Vec<EntityId> = engine
        .world_mut()
        .query_ids::<(&C,)>()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids.len(), 1, "expected exactly one such entity");
    ids[0]
}

/// The declarative half from a game's side: an `AudioEmitter` placed in the scene, with the listener
/// walked toward it and then away again. The engine syncs it with no system registered, and the gain
/// in the stream rises then falls — the generic second-consumer proof, outside vigil.
#[test]
fn walking_toward_a_placed_emitter_and_away_again_raises_then_lowers_its_gain() {
    // The scene's camera is the listener; the bell sits 20 units east of it, inside a 40-unit field.
    let scene = format!("{SCENE}{}", BELL_ENTITY.replace("<sound>", BOUNCE_SOUND));
    let mut engine = ludion_testkit::loaded_engine(hello_paddle::game(), &scene);
    let camera = only_entity_with::<Camera>(&mut engine);
    let bell = only_entity_with::<AudioEmitter>(&mut engine);

    // Stand still, walk halfway to the bell, then retreat past where we started.
    for x in [0.0, 10.0, -10.0] {
        engine
            .world_mut()
            .get_mut::<Transform>(camera)
            .expect("the camera is alive")
            .expect("the camera has a Transform")
            .translation[0] = x;
        engine
            .execute(Command::SimStep(command::SimStepParams {
                ticks: 1,
                hash: false,
            }))
            .expect("sim steps");
    }

    let gains: Vec<f32> = audio_messages(&engine)
        .iter()
        .filter_map(|(_, message)| match message {
            AudioMessage::LoopSync { id, gain, .. } if *id == LoopId::Entity(bell) => Some(*gain),
            _ => None,
        })
        .collect();
    assert_eq!(
        gains.len(),
        3,
        "one sync per gain change while the bell is audible"
    );
    assert!(
        gains[0] < gains[1] && gains[1] > gains[2],
        "the bell gets louder on approach and quieter on retreat: {gains:?}"
    );
}

/// The committed bounce WAV decodes as 16-bit PCM mono through `ludion_assets::load_wav`.
#[test]
fn committed_bounce_wav_decodes() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenes");
    let pcm = ludion_assets::load_wav(&root, BOUNCE_SOUND).expect("the committed WAV decodes");
    assert_eq!(pcm.channels, 1);
    assert!(!pcm.samples.is_empty());
    assert!(pcm.samples.iter().all(|s| (-1.0..=1.0).contains(s)));
}
