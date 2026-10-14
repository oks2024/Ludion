//! Headless gameplay + determinism tests, driven through the real `Engine` built from this game's
//! own `EngineBuilder`. No GPU needed.
//!
//! Level reference (`scenes/main.yaml`), cols left-to-right, rows top-down:
//!
//! ```text
//!   col 01234567
//! row 0 ########
//! row 1 #......#
//! row 2 #..x...#     x = target
//! row 3 #..a...#     a = crate-a start (3,3)
//! row 4 #..@.bx#     @ = player start (3,4), b = crate-b start (5,4)
//! row 5 #......#
//! row 6 ########
//! ```
//!
//! Solution used by the win tests: Up (push a onto (3,2)), Down, Right, Right (push b onto (6,4)).

use ludion::prelude::*;
use ludion_testkit::GameplayDriver;
use serde_json::json;
use sokoban::{GameState, GridPos, Player, Pushable};

/// The game scene, embedded so these tests are self-contained.
const SCENE: &str = include_str!("../scenes/main.yaml");

fn driver() -> GameplayDriver {
    GameplayDriver::new(sokoban::game(), SCENE)
}

fn player_pos(d: &mut GameplayDriver) -> (i32, i32) {
    let world = d.engine_mut().world_mut();
    let e = world.single::<Player>().expect("the scene has a player");
    let pos = world.get::<GridPos>(e).unwrap().unwrap();
    (pos.col, pos.row)
}

/// Crate cells in deterministic entity order.
fn crate_positions(d: &mut GameplayDriver) -> Vec<(i32, i32)> {
    let world = d.engine_mut().world_mut();
    let crates: Vec<EntityId> = world
        .entities()
        .filter(|&e| world.has::<Pushable>(e))
        .collect();
    crates
        .into_iter()
        .map(|e| {
            let pos = world.get::<GridPos>(e).unwrap().unwrap();
            (pos.col, pos.row)
        })
        .collect()
}

fn state(d: &mut GameplayDriver) -> GameState {
    let world = d.engine_mut().world_mut();
    let e = world
        .single::<GameState>()
        .expect("the scene has a GameState");
    *world.get::<GameState>(e).unwrap().unwrap()
}

fn status_text(d: &mut GameplayDriver) -> String {
    let world = d.engine_mut().world_mut();
    let e = world
        .single::<GameState>()
        .expect("the scene has a GameState");
    world.get::<Text>(e).unwrap().unwrap().text.clone()
}

fn player_translation(d: &mut GameplayDriver) -> [f32; 3] {
    let world = d.engine_mut().world_mut();
    let e = world.single::<Player>().expect("the scene has a player");
    world.get::<Transform>(e).unwrap().unwrap().translation
}

/// Solves the level: Up, Down, Right, Right (see the module doc's map).
fn solve(d: &mut GameplayDriver) {
    for key in ["Up", "Down", "Right", "Right"] {
        d.tap(key);
    }
}

#[test]
fn player_moves_one_cell_per_key_press() {
    let mut d = driver();
    assert_eq!(player_pos(&mut d), (3, 4));
    d.tap("Right");
    assert_eq!(
        player_pos(&mut d),
        (4, 4),
        "one press moves exactly one cell"
    );
    // Holding across many ticks must not repeat the move: the edge fired once.
    d.inject("key_down", "Right");
    d.step(30);
    assert_eq!(
        player_pos(&mut d),
        (5, 4),
        "a held key is one edge, one move"
    );
}

#[test]
fn transform_snaps_to_the_cell_center() {
    let mut d = driver();
    d.tap("Right");
    let t = player_translation(&mut d);
    // Grid origin (-3.5, 3.0); cell (4,4) center = (-3.5 + 4, 3.0 - 4) = (0.5, -1.0). z is the
    // scene's layering choice and must survive untouched.
    assert!(
        (t[0] - 0.5).abs() < 1e-5 && (t[1] + 1.0).abs() < 1e-5 && (t[2] - 0.5).abs() < 1e-5,
        "player transform should snap to (0.5, -1.0, 0.5), got {t:?}"
    );
}

#[test]
fn walls_block_movement() {
    let mut d = driver();
    for _ in 0..4 {
        d.tap("Left");
    }
    assert_eq!(
        player_pos(&mut d),
        (1, 4),
        "col 0 is wall — the fourth Left must be swallowed"
    );
    assert_eq!(
        state(&mut d).moves,
        2,
        "blocked presses don't count as moves"
    );
}

#[test]
fn pushing_a_crate_moves_it_one_cell() {
    let mut d = driver();
    d.tap("Up");
    assert_eq!(
        player_pos(&mut d),
        (3, 3),
        "player takes the crate's old cell"
    );
    assert_eq!(
        crate_positions(&mut d)[0],
        (3, 2),
        "crate-a is pushed onto the target"
    );
}

#[test]
fn a_crate_cannot_be_pushed_into_a_wall() {
    let mut d = driver();
    d.tap("Up"); // crate-a -> (3,2), player -> (3,3)
    d.tap("Up"); // crate-a -> (3,1), player -> (3,2)
    d.tap("Up"); // beyond is the top wall: nothing moves
    assert_eq!(player_pos(&mut d), (3, 2));
    assert_eq!(crate_positions(&mut d)[0], (3, 1));
    assert_eq!(state(&mut d).moves, 2);
}

#[test]
fn a_crate_cannot_push_another_crate() {
    let mut d = driver();
    // Push crate-b left twice so it lands at (3,4), directly below crate-a (3,3): walk under the
    // level's right side to (6,4) (the target cell is walkable), then shove left.
    for key in ["Down", "Right", "Right", "Right", "Up"] {
        d.tap(key); // (3,5) -> (4,5) -> (5,5) -> (6,5) -> (6,4)
    }
    assert_eq!(player_pos(&mut d), (6, 4));
    d.tap("Left"); // crate-b (5,4) -> (4,4), player (5,4)
    d.tap("Left"); // crate-b (4,4) -> (3,4), player (4,4)
    assert_eq!(crate_positions(&mut d), vec![(3, 3), (3, 4)]);
    // Stand below the stack and push up: crate-b's beyond is crate-a — the whole move is swallowed.
    let moves_before = state(&mut d).moves;
    d.tap("Down"); // player (4,5)
    d.tap("Left"); // player (3,5)
    d.tap("Up"); // push attempt into the crate stack
    assert_eq!(
        player_pos(&mut d),
        (3, 5),
        "the push into a crate is swallowed"
    );
    assert_eq!(crate_positions(&mut d), vec![(3, 3), (3, 4)]);
    assert_eq!(
        state(&mut d).moves,
        moves_before + 2,
        "only the two walk steps counted"
    );
}

#[test]
fn solving_the_level_wins_and_r_resets() {
    let mut d = driver();
    assert_eq!(status_text(&mut d), "Moves: 0");
    solve(&mut d);
    let s = state(&mut d);
    assert!(s.won, "both crates on targets must win");
    assert_eq!(s.moves, 4);
    assert_eq!(status_text(&mut d), "Solved in 4 moves!");

    // After the win, movement is ignored...
    d.tap("Left");
    assert_eq!(player_pos(&mut d), (5, 4));
    assert_eq!(state(&mut d).moves, 4);

    // ...but R restores the start layout.
    d.tap("R");
    assert_eq!(player_pos(&mut d), (3, 4));
    assert_eq!(crate_positions(&mut d), vec![(3, 3), (5, 4)]);
    let s = state(&mut d);
    assert!(!s.won);
    assert_eq!(s.moves, 0);
    assert_eq!(status_text(&mut d), "Moves: 0");
}

#[test]
fn crates_turn_gold_on_targets() {
    let mut d = driver();
    d.tap("Up"); // crate-a onto the (3,2) target
    let world = d.engine_mut().world_mut();
    let crates: Vec<EntityId> = world
        .entities()
        .filter(|&e| world.has::<Pushable>(e))
        .collect();
    let colors: Vec<[f32; 4]> = crates
        .iter()
        .map(|&e| world.get::<Sprite>(e).unwrap().unwrap().color)
        .collect();
    assert_eq!(colors[0], sokoban::CRATE_ON_TARGET_COLOR);
    assert_eq!(colors[1], sokoban::CRATE_COLOR);
}

/// The scripted solve as a raw command list, for the determinism test (and the replay test, which
/// includes this file's sibling module — see `tests/replay.rs`).
pub fn solve_commands() -> Vec<Command> {
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
    // Run on through the win so the celebration light's animation is hashed too.
    commands.push(Command::SimStep(command::SimStepParams {
        ticks: 60,
        hash: false,
    }));
    commands
}

/// Determinism: two identical runs produce identical per-frame `WorldHash` sequences — including
/// the post-win pulsing light, which must be pure sim state.
#[test]
fn two_runs_produce_identical_worldhash_sequences() {
    let run = || {
        let mut engine = ludion_testkit::loaded_engine(sokoban::game(), SCENE);
        ludion_engine::drive_hashes(&mut engine, solve_commands()).expect("drive succeeds")
    };
    assert_eq!(run(), run(), "the simulation must be deterministic");
}
