//! Headless gameplay + determinism tests for `hello-paddle`, all driven through the real `Engine`
//! (built from the game's own `EngineBuilder`), proving a game gets scene load, the sim, `WorldHash`,
//! and recording/replay from the engine with only its `Paddle` component and `move_paddle` system.

use std::fs::{self, File};
use std::path::PathBuf;

use hello_paddle::Paddle;
use ludion::prelude::*;
use ludion_testkit::GameplayDriver;
use serde_json::json;

/// The game scene, embedded so these tests are self-contained (an inline load is also what makes a
/// recording portable — the scene text travels in the replay).
const SCENE: &str = include_str!("../scenes/paddle.yaml");

/// A fresh driver over the game's config with the paddle scene loaded — the construction path every
/// test starts from. Loading succeeds only because the engine registers `Paddle` on top of the
/// built-ins (a bare engine would reject the scene with `L1013`).
fn driver() -> GameplayDriver {
    GameplayDriver::new(hello_paddle::game(), SCENE)
}

/// Reads the single paddle entity's `Paddle::x` out of the world.
fn paddle_x(d: &mut GameplayDriver) -> f32 {
    let mut x = None;
    for (paddle,) in d.engine_mut().world_mut().query::<(&Paddle,)>() {
        x = Some(paddle.x);
    }
    x.expect("the scene has a Paddle entity")
}

/// The command sequence the determinism and replay tests drive: press `D`, then run 60 ticks.
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

/// (a) Sim assertion: the custom system moves the custom component while its key is held.
/// 30 ticks × 6.0 units/s × (1/60 s) = 3.0 units.
#[test]
fn paddle_slides_right_while_d_is_held() {
    let mut d = driver();
    d.inject("key_down", "D");
    d.step(30);
    let x = paddle_x(&mut d);
    assert!(
        (x - 3.0).abs() < 1e-3,
        "paddle should have slid ~3.0 units after 30 ticks with D held, got {x}"
    );
}

/// (a, negative) With no input the system runs but mutates nothing — the paddle stays put.
#[test]
fn paddle_stays_put_without_input() {
    let mut d = driver();
    d.step(30);
    assert_eq!(
        paddle_x(&mut d),
        0.0,
        "with no key held the paddle must not move"
    );
}

/// (b) Determinism: two identical runs produce identical per-frame `WorldHash` sequences. `WorldHash`
/// is registry-driven, so the custom `Paddle` is hashed with no extra wiring.
#[test]
fn two_runs_produce_identical_worldhash_sequences() {
    let run = || {
        let mut engine = ludion_testkit::loaded_engine(hello_paddle::game(), SCENE);
        ludion_engine::drive_hashes(&mut engine, run_commands()).expect("drive succeeds")
    };
    assert_eq!(run(), run(), "the game's simulation must be deterministic");
}

/// The recording round-trip: record a session of the game (custom component + system), then
/// reproduce its exact `WorldHash` trace with `replay_hashes_with`, handing replay the *same* builder
/// config. Recording, replay, snapshot keyframes, and hashing are all registry-driven, so the custom
/// component travels through them with no extra wiring.
#[test]
fn recorded_session_replays_to_the_same_trace() {
    let session = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("hello_paddle_session.lrp");
    let _ = fs::remove_file(&session);

    // Live run, recorded. Arm *after* loading so the header captures the scene (inline) and seed.
    let mut engine = ludion_testkit::loaded_engine(hello_paddle::game(), SCENE);
    engine
        .set_recording(File::create(&session).expect("create the replay sink"))
        .expect("arm recording");
    let live = ludion_engine::drive_hashes(&mut engine, run_commands()).expect("drive live");
    engine.stop_recording().expect("flush the recording");

    // Replay through the game's builder (a bare engine could not re-instantiate the Paddle scene).
    let bytes = fs::read(&session).expect("read the recorded session");
    let root = session
        .parent()
        .expect("the session has a parent directory");
    let replay =
        ludion_engine::replay_hashes_with(hello_paddle::game(), &bytes, root).expect("replay");

    assert_eq!(
        live, replay,
        "replay must reproduce the recorded WorldHash trace exactly"
    );
    let _ = fs::remove_file(&session);
}
