//! Headless gameplay + determinism tests, driven through the real `Engine` built from this game's
//! own `EngineBuilder`. These need no GPU and pass out of the box. See `LUDION-GUIDE.md` for the
//! verification loop these are the start of.

use {{name_ident}}::Player;
use ludion::prelude::*;
use ludion_testkit::GameplayDriver;
use serde_json::json;

/// The game scene, embedded so these tests are self-contained.
const SCENE: &str = include_str!("../scenes/main.yaml");

/// Reads the single player entity's `Player::x` out of the world.
fn player_x(engine: &mut Engine) -> f32 {
    let mut x = None;
    for (player,) in engine.world_mut().query::<(&Player,)>() {
        x = Some(player.x);
    }
    x.expect("the scene has a Player entity")
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
    let mut driver = GameplayDriver::new({{name_ident}}::game(), SCENE);
    driver.hold("Right", 30);
    let x = player_x(driver.engine_mut());
    assert!(
        (x - 3.0).abs() < 1e-3,
        "player should have slid ~3.0 units after 30 ticks, got {x}"
    );
}

/// Sim assertion (negative): with no input the system runs but mutates nothing. This one keeps a bare
/// `Engine` and the free `step`.
#[test]
fn player_stays_put_without_input() {
    let mut engine = ludion_testkit::loaded_engine({{name_ident}}::game(), SCENE);
    ludion_testkit::step(&mut engine, 30);
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
        let mut engine = ludion_testkit::loaded_engine({{name_ident}}::game(), SCENE);
        ludion_engine::drive_hashes(&mut engine, run_commands()).expect("drive succeeds")
    };
    assert_eq!(run(), run(), "the simulation must be deterministic");
}
