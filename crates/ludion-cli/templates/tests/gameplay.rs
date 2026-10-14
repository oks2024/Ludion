//! Headless gameplay + determinism tests, driven through the real `Engine` built from this game's
//! own `EngineBuilder`. These need no GPU and pass out of the box. See `LUDION-GUIDE.md` for the
//! verification loop these are the start of.

use {{name_ident}}::Player;
use ludion::prelude::*;
use serde_json::json;

/// The game scene, embedded so these tests are self-contained.
const SCENE: &str = include_str!("../scenes/main.yaml");

/// A fresh engine built from this game's config with the scene loaded — the path every test starts
/// from. Loading succeeds because the engine registers `Player` on top of the built-ins.
fn loaded_engine() -> Engine {
    let mut engine = {{name_ident}}::game().build().expect("the game builds");
    engine
        .load_inline_scene(SCENE)
        .expect("the scene loads against the game registry");
    engine
}

/// Reads the single player entity's `Player::x` out of the world.
fn player_x(engine: &mut Engine) -> f32 {
    let mut x = None;
    for (player,) in engine.world_mut().query::<(&Player,)>() {
        x = Some(player.x);
    }
    x.expect("the scene has a Player entity")
}

/// Steps the engine `ticks` fixed ticks.
fn step(engine: &mut Engine, ticks: u64) {
    engine
        .execute(Command::SimStep(command::SimStepParams { ticks, hash: false }))
        .expect("sim steps");
}

/// The command sequence the determinism test drives: press Right, then run 60 ticks.
fn run_commands() -> Vec<Command> {
    vec![
        Command::InputInject(command::InputInjectParams {
            frame: Some(FrameIndex::new(0)),
            event: json!({"kind": "key_down", "key": "Right"}),
        }),
        Command::SimStep(command::SimStepParams {
            ticks: 60,
            hash: false,
        }),
    ]
}

/// Sim assertion: the custom system moves the custom component while its key is held.
/// 30 ticks * 6.0 units/s * (1/60 s) = 3.0 units.
#[test]
fn player_slides_right_while_key_is_held() {
    let mut engine = loaded_engine();
    engine
        .execute(Command::InputInject(command::InputInjectParams {
            frame: Some(FrameIndex::new(0)),
            event: json!({"kind": "key_down", "key": "Right"}),
        }))
        .expect("input injects");
    step(&mut engine, 30);
    let x = player_x(&mut engine);
    assert!(
        (x - 3.0).abs() < 1e-3,
        "player should have slid ~3.0 units after 30 ticks, got {x}"
    );
}

/// Sim assertion (negative): with no input the system runs but mutates nothing.
#[test]
fn player_stays_put_without_input() {
    let mut engine = loaded_engine();
    step(&mut engine, 30);
    assert_eq!(
        player_x(&mut engine),
        0.0,
        "with no key held the player must not move"
    );
}

/// Determinism: two identical runs produce identical per-frame `WorldHash` sequences. `WorldHash` is
/// registry-driven, so the custom `Player` component is hashed with no extra wiring.
#[test]
fn two_runs_produce_identical_worldhash_sequences() {
    let run = || {
        let mut engine = loaded_engine();
        ludion_engine::drive_hashes(&mut engine, run_commands()).expect("drive succeeds")
    };
    assert_eq!(run(), run(), "the simulation must be deterministic");
}
