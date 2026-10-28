//! The committed session replay (`run.lrp`): the scripted route recorded in-process must replay,
//! through a fresh engine built from this game's own `EngineBuilder`, to the live per-frame
//! `WorldHash` trace, and the committed artifact must still replay to that same trace. After an
//! intentional change to the game, the scene or the engine's hash-visible behaviour, re-record
//! with `LUDION_BLESS=1 cargo test -p character-course --test replay` and commit the new `run.lrp`.

mod common;

use std::fs;
use std::path::PathBuf;

use common::{FRAMES, ROUTE};
use ludion::prelude::*;
use serde_json::json;

const MAIN_SCENE: &str = include_str!("../scenes/main.yaml");

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scenes_dir() -> PathBuf {
    project_root().join("scenes")
}

/// The route as wire commands: step up to each key transition, inject it, run out the session.
fn session_commands() -> Vec<Command> {
    let mut commands = Vec::new();
    let mut at = 0;
    for (frame, key, down) in ROUTE {
        if frame > at {
            commands.push(Command::SimStep(command::SimStepParams {
                ticks: frame - at,
                hash: false,
            }));
            at = frame;
        }
        commands.push(Command::InputInject(command::InputInjectParams {
            frame: None,
            event: json!({"kind": if down { "key_down" } else { "key_up" }, "key": format!("{key:?}")}),
        }));
    }
    commands.push(Command::SimStep(command::SimStepParams {
        ticks: FRAMES - at,
        hash: false,
    }));
    commands
}

#[test]
fn recorded_session_replays_to_identical_worldhash_trace() {
    let committed = project_root().join("run.lrp");
    // Crate-prefixed: CARGO_TARGET_TMPDIR is the workspace-shared `target/tmp`.
    let fresh = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("character-course-run.lrp");

    let mut engine = ludion_testkit::loaded_engine(
        character_course::game().project_root(scenes_dir()),
        MAIN_SCENE,
    );
    engine
        .set_recording(fs::File::create(&fresh).expect("the fresh recording is writable"))
        .expect("recording arms");
    let live =
        ludion_engine::drive_hashes(&mut engine, session_commands()).expect("the session runs");
    engine.stop_recording().expect("recording flushes");

    // The session was a real run: the hero ended past the lane section's exit.
    let x = engine
        .world_mut()
        .query::<(&character_course::Runner, &Transform)>()
        .next()
        .map(|(_, pose)| pose.translation[0])
        .expect("the hero exists");
    assert!(
        x >= 50.0,
        "the recorded session must clear the course, hero at x = {x}"
    );

    let bytes = fs::read(&fresh).expect("the fresh recording reads back");
    let replayed =
        ludion_engine::replay_hashes_with(character_course::game(), &bytes, scenes_dir())
            .expect("the fresh recording replays");
    assert_eq!(
        live, replayed,
        "the replayed session must reproduce the live WorldHash trace exactly"
    );

    if ludion_testkit::bless_requested() {
        fs::copy(&fresh, &committed).expect("adopting the fresh recording as run.lrp");
        eprintln!(
            "re-recorded {} — review the diff, then commit it",
            committed.display()
        );
        return;
    }

    let committed_bytes = fs::read(&committed).expect("the committed run.lrp reads back");
    let committed_replayed =
        ludion_engine::replay_hashes_with(character_course::game(), &committed_bytes, scenes_dir())
            .expect("the committed run.lrp replays");
    assert_eq!(
        live, committed_replayed,
        "the committed run.lrp no longer matches the live session — after an intentional \
         game/scene/engine change, re-record it with `LUDION_BLESS=1` and commit it"
    );
}
