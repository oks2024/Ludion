//! Behavioral proof for the Phase 17 scene-transition facade wave (E7): a game splits its world into
//! districts authored as separate scene files, and a `cross_gate` system requests a `SceneSwitch` when
//! the player reaches a `Gate`, carrying `[player]`. Verified two ways — two identical drives across
//! the gate produce identical `WorldHash` traces (the switch is fully deterministic and regenerates),
//! and the carried player's components are byte-equal after the respawn at the anchor (its
//! `Transform.translation` aside, which becomes the anchor's).
//!
//! The game mirrors the `scene_transition` example: `Player` carries the HP, `Gate` names the
//! destination district and anchor, and the walk-and-cross systems drive the crossing (types
//! duplicated minimally, the `data_docs`/`player_profile` test mold).

use ludion::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Player {
    hp: u32,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
struct Gate {
    to: String,
    anchor: String,
}

const WALK_STEP: f32 = 1.0;

fn walk(world: &mut World, _ctx: &SystemCtx) {
    let players: Vec<EntityId> = world
        .entities()
        .filter(|&e| world.get::<Player>(e).unwrap().is_some())
        .collect();
    for player in players {
        if let Some(transform) = world.get_mut::<Transform>(player).unwrap() {
            transform.translation[0] += WALK_STEP;
        }
    }
}

fn cross_gate(world: &mut World, _ctx: &SystemCtx) {
    let Some(player) = world.single::<Player>() else {
        return;
    };
    let player_x = translation_x(world, player);

    let mut reached: Option<(String, String)> = None;
    let gates: Vec<EntityId> = world
        .entities()
        .filter(|&e| world.get::<Gate>(e).unwrap().is_some())
        .collect();
    for gate in gates {
        if player_x >= translation_x(world, gate) {
            let g = world.get::<Gate>(gate).unwrap().unwrap();
            reached = Some((g.to.clone(), g.anchor.clone()));
            break;
        }
    }

    if let Some((to, anchor)) = reached
        && let Some(switch) = world.resource_mut::<SceneSwitch>()
    {
        switch.request(to, anchor, vec![player]);
    }
}

fn translation_x(world: &World, entity: EntityId) -> f32 {
    world
        .get::<Transform>(entity)
        .unwrap()
        .map(|transform| transform.translation[0])
        .unwrap_or(0.0)
}

fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| {
            reg.register::<Player>("Player")?;
            reg.register::<Gate>("Gate")
        })
        .system("walk", walk)
        .system("cross_gate", cross_gate)
}

/// District A: the player (100 HP) at the origin and a gate at X = 5 leading to `next.yaml`.
const START: &str = "\
format: 1
seed: 1
entities:
  - name: player
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      Player:
        hp: 100
  - name: gate
    components:
      Transform:
        translation:
          - 5.0
          - 0.0
          - 0.0
      Gate:
        to: next.yaml
        anchor: gate-from-start
";

/// District B: the `gate-from-start` anchor (a distinctive translation) plus a native prop. Its seed
/// differs deliberately — a switch ignores it.
const NEXT: &str = "\
format: 1
seed: 2
entities:
  - name: gate-from-start
    components:
      Transform:
        translation:
          - 40.0
          - 0.0
          - 0.0
  - name: beacon
    components:
      Transform:
        translation:
          - 42.0
          - 0.0
          - 0.0
";

/// The anchor's translation in [`NEXT`] — where the carried player lands.
const ANCHOR_XYZ: [f32; 3] = [40.0, 0.0, 0.0];

/// A fresh, process-unique temp content root holding the two districts (removed first, so a re-run
/// starts clean).
fn fresh_root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-scene-transition-test-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("start.yaml"), START).unwrap();
    std::fs::write(dir.join("next.yaml"), NEXT).unwrap();
    dir
}

fn engine_at(root: &Path) -> Engine {
    game()
        .load_scene_file(&root.join("start.yaml"))
        .expect("the start district loads")
}

/// The player entity, located by its `Player` component.
fn player_of(engine: &Engine) -> EntityId {
    engine
        .world()
        .single::<Player>()
        .expect("the district has a player")
}

fn serialized(engine: &Engine, id: EntityId, component: &str) -> Option<Value> {
    engine
        .registry()
        .serialize(component, engine.world(), id)
        .unwrap()
}

/// Two identical drives across the gate produce identical per-frame `WorldHash` traces: the switch is
/// fully deterministic and regenerates from the sim (a fresh boot, same on-disk districts, same seed).
#[test]
fn two_runs_across_the_gate_produce_identical_traces() {
    let drive = |name: &str| -> Vec<u64> {
        let mut engine = engine_at(&fresh_root(name));
        ludion_engine::drive_hashes(
            &mut engine,
            vec![Command::SimStep(command::SimStepParams {
                ticks: 12,
                hash: false,
            })],
        )
        .expect("the session drives")
        .into_iter()
        .map(|(_, hash)| hash.as_u64())
        .collect()
    };
    assert_eq!(
        drive("trace-a"),
        drive("trace-b"),
        "the switch is fully deterministic — two drives hash identically across the gate"
    );

    // Teeth: a single drive actually crosses into district B (its anchor + prop + carried player = 3).
    let mut engine = engine_at(&fresh_root("teeth"));
    for _ in 0..12 {
        engine.advance_live();
    }
    assert!(
        engine.last_transition_error().is_none(),
        "the switch reported no error: {:?}",
        engine.last_transition_error()
    );
    assert_eq!(
        engine.world().entity_count(),
        3,
        "district B was swapped in (its two natives plus the carried player)"
    );
}

/// The carried player's components are byte-equal across the switch — `Player` and `Name` round-trip
/// verbatim through the serialize→respawn — and it lands at the anchor's translation.
#[test]
fn the_carried_player_is_byte_equal_after_the_switch() {
    let mut engine = engine_at(&fresh_root("carry"));

    // Walk tick by tick until district B is present, remembering the player's pre-switch component
    // bytes from the last district-A tick.
    let mut before_player = None;
    let mut before_name = None;
    for _ in 0..12 {
        if engine.world().entity_count() == 3 {
            break; // district B is loaded — the switch has happened
        }
        let player = player_of(&engine);
        before_player = serialized(&engine, player, "Player");
        before_name = serialized(&engine, player, "Name");
        engine.advance_live();
    }
    assert_eq!(
        engine.world().entity_count(),
        3,
        "the switch happened within the tick budget"
    );
    assert!(engine.last_transition_error().is_none());

    let player = player_of(&engine);
    assert_eq!(
        serialized(&engine, player, "Player"),
        before_player,
        "the carried Player component round-trips byte-equal (hp carried verbatim)"
    );
    assert_eq!(
        serialized(&engine, player, "Name"),
        before_name,
        "the carried Name round-trips byte-equal"
    );

    // Only the translation changed — the player landed at the anchor's (decision 8).
    let transform = serialized(&engine, player, "Transform").unwrap();
    let translation = &transform["translation"];
    assert_eq!(translation[0], ANCHOR_XYZ[0] as f64);
    assert_eq!(translation[1], ANCHOR_XYZ[1] as f64);
    assert_eq!(translation[2], ANCHOR_XYZ[2] as f64);
}
