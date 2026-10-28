//! The numeric trace on a game-driven motor: `trace_with` runs the game's system, the `motor`
//! channel reports the tick's `MoveReport`, the `sensors` channel reports the enter, and two runs
//! write the same bytes.

use std::path::{Path, PathBuf};

use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    Engine, EngineBuilder, Name, TraceChannel, move_character, trace_with, write_jsonl,
};
use ludion_sim::InputScript;
use serde_json::Value;

const FRAMES: u64 = 40;

/// A floor, a sensor box the hero walks into along +x, the hero, and a second motor nothing moves.
const SCENE: &str = "format: 1
physics:
  space: free
entities:
  - name: floor
    components:
      Transform:
        translation:
          - 0.0
          - -0.5
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 20.0
              - 0.5
              - 20.0
        body: static
  - name: zone
    components:
      Transform:
        translation:
          - 1.5
          - 1.0
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 0.5
              - 0.5
        body: static
        sensor: true
  - name: hero
    components:
      Transform:
        translation:
          - 0.0
          - 0.6
          - 0.0
      CharacterMotor:
        radius: 0.28
        height: 0.9
  - name: idle
    components:
      Transform:
        translation:
          - 0.0
          - 0.6
          - 5.0
      CharacterMotor:
        radius: 0.28
        height: 0.9
";

fn walk(world: &mut World, _ctx: &SystemCtx) {
    let hero = world
        .entities()
        .find(|entity| matches!(world.get::<Name>(*entity), Ok(Some(name)) if name.0 == "hero"));
    if let Some(hero) = hero {
        let _ = move_character(world, hero, [0.05, -0.1, 0.0]);
    }
}

fn scene_file() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-trace-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scene.yaml");
    std::fs::write(&path, SCENE).unwrap();
    path
}

fn game() -> EngineBuilder {
    Engine::builder().system("walk", walk)
}

fn run(scene: &Path) -> Vec<u8> {
    let records = trace_with(
        game(),
        scene,
        &InputScript::new(),
        FRAMES,
        &TraceChannel::ALL,
    )
    .expect("the trace runs");
    let mut bytes = Vec::new();
    write_jsonl(&records, &mut bytes).unwrap();
    bytes
}

fn lines(bytes: &[u8]) -> Vec<Value> {
    std::str::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).expect("every line is JSON"))
        .collect()
}

#[test]
fn two_runs_write_the_same_bytes() {
    let scene = scene_file();
    let first = run(&scene);
    assert!(!first.is_empty());
    assert_eq!(first, run(&scene));
}

#[test]
fn the_motor_channel_reports_each_tick_and_only_what_was_moved() {
    let lines = lines(&run(&scene_file()));
    let motor = |name: &str| -> Vec<&Value> {
        lines
            .iter()
            .filter(|line| line["channel"] == "motor" && line["entity"] == name)
            .collect()
    };

    let hero = motor("hero");
    assert_eq!(hero.len(), FRAMES as usize, "one hero line per tick");
    for (tick, line) in hero.iter().enumerate() {
        assert_eq!(line["tick"], tick as u64);
    }
    let last = hero.last().unwrap();
    assert_eq!(last["grounded"], true, "the hero landed on the floor");
    assert!(last["feet"][0].as_f64().unwrap() > 1.5, "and walked on");

    let idle = motor("idle");
    assert_eq!(idle.len(), FRAMES as usize);
    for line in idle {
        let mut keys: Vec<&str> = line
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["channel", "entity", "feet", "tick"],
            "no move, no report"
        );
    }
}

#[test]
fn the_sensors_channel_names_the_zone_and_the_hero_once() {
    let lines = lines(&run(&scene_file()));
    let entered: Vec<&Value> = lines
        .iter()
        .filter(|line| line["channel"] == "sensors" && line["phase"] == "entered")
        .collect();
    assert_eq!(entered.len(), 1, "{entered:?}");
    assert_eq!(entered[0]["sensor"], "zone");
    assert_eq!(entered[0]["other"], "hero");
}
