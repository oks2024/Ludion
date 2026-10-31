//! Headless gameplay + determinism tests, driven through the real `Engine` built from this game's
//! own `EngineBuilder`. These need no GPU and pass out of the box. See `LUDION-GUIDE.md` for the
//! verification loop these are the start of.

use {{name_ident}}::Player;
use ludion::prelude::*;
use ludion_testkit::GameplayDriver;

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

/// Determinism: two runs of the same inputs reach the same `WorldHash` on every tick. On a mismatch
/// the failure names the frame, the first system and the component fields that differ.
#[test]
fn two_runs_are_deterministic() {
    ludion_testkit::assert_deterministic(
        || ludion_testkit::loaded_engine({{name_ident}}::game(), SCENE),
        |engine| {
            ludion_testkit::inject(engine, InputEvent::KeyDown { key: Key::Right });
            ludion_testkit::step(engine, 60);
        },
    );
}
