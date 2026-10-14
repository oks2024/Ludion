//! Record/replay round-trip: a session recorded in-process replays through a fresh engine to the
//! identical `WorldHash` sequence. Recordings embed the scene text, so the `.lrp` is
//! self-contained.

use std::path::{Path, PathBuf};

use ludion::prelude::*;
use serde_json::json;

const SCENE: &str = include_str!("../scenes/main.yaml");

/// The same scripted solve `tests/gameplay.rs` drives (duplicated — integration tests are separate
/// crates): tap Up, Down, Right, Right, then run 60 ticks through the win celebration.
fn solve_commands() -> Vec<Command> {
    let mut commands = Vec::new();
    let mut frame = 0u64;
    for key in ["Up", "Down", "Right", "Right"] {
        commands.push(Command::InputInject(command::InputInjectParams {
            frame: Some(FrameIndex::new(frame)),
            event: json!({"kind": "key_down", "key": key}),
        }));
        commands.push(Command::SimStep(command::SimStepParams {
            ticks: 1,
            hash: false,
        }));
        frame += 1;
        commands.push(Command::InputInject(command::InputInjectParams {
            frame: Some(FrameIndex::new(frame)),
            event: json!({"kind": "key_up", "key": key}),
        }));
        commands.push(Command::SimStep(command::SimStepParams {
            ticks: 1,
            hash: false,
        }));
        frame += 1;
    }
    commands.push(Command::SimStep(command::SimStepParams {
        ticks: 60,
        hash: false,
    }));
    commands
}

#[test]
fn a_recorded_session_replays_identically() {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("sokoban-run.lrp");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();

    let mut engine = ludion_testkit::loaded_engine(sokoban::game(), SCENE);
    engine
        .set_recording(std::fs::File::create(&out).expect("recording file creates"))
        .expect("recording arms");
    let live = ludion_engine::drive_hashes(&mut engine, solve_commands()).expect("drive succeeds");
    engine.stop_recording().expect("recording flushes");

    let bytes = std::fs::read(&out).expect("recording reads back");
    let replayed = ludion_engine::replay_hashes_with(
        sokoban::game(),
        &bytes,
        Path::new(env!("CARGO_MANIFEST_DIR")),
    )
    .expect("replay succeeds");
    assert_eq!(live, replayed, "the recording is the session");
}
