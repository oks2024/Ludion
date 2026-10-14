//! **Scene residue** — the district you left remembers what you did to it.
//!
//! `scene_transition` shows the half of a district change that travels *with* the player. This is the
//! other half: an entity marked [`Residue`] stays with its scene, and the engine captures its
//! components on the way out and re-applies them when the run comes back — within one session, with no
//! save file, no wire method, and no bookkeeping in the game.
//!
//! The demo is one door. The player walks east through district A, opens the door on the way past,
//! crosses into district B, turns round and comes back: the door is still open. Its unmarked twin, the
//! `shutter`, carries the identical component and no marker, so it is rebuilt from the scene text and
//! is shut again — which is what makes the marker, not the engine, the thing doing the remembering.
//!
//! It also reads that memory from the *other* district: `preview_home` asks the [`ResidueStore`]
//! mirror what district A kept while the player is standing in district B — the reading a map screen
//! needs before the run walks back in.
//!
//! Everything above the `main` divider is copyable into a real game — `game()`, `Openable`, `open`,
//! `cross_gate`, and `preview_home` are the whole pattern. What it is built on:
//!
//! - **the marker is opt-in and per entity.** `Residue: {}` in the scene text (or inserted from code
//!   on an entity a system spawns). A scene that marks nothing is rebuilt exactly as before residue
//!   existed.
//! - **identity is the entity's `name:`.** A named record lands back on its own entity, replacing its
//!   authored state in place — the `EntityId` does not move. An entity with no name is *anonymous*:
//!   never matched, always respawned, which is what a dropped pile of loot wants.
//! - **residue is keyed by the scene's `name:`.** A scene that declares none keeps nothing.
//! - **what must survive a visit stays alive.** Capture reads the world that is there, so an entity
//!   despawned before the switch leaves nothing behind. A thing that has been used up keeps its entity
//!   and says so in a component (`open: true` here), rather than being deleted.
//!
//! Run it headless (writes the two districts under a process-unique temp directory, never the repo
//! tree, then walks the round trip and reports both doors):
//!
//! ```text
//! cargo run -p ludion --example scene_residue
//! ```
//!
//! Residue lives for the session: `ludion::run` keeps it across every gate of a run, and a fresh
//! `scene.load` — a new run — starts the city blank again.

use ludion::prelude::*;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// --- The game (copyable) -------------------------------------------------------------------------

/// The player, carried across every gate. `previewed` is demo bookkeeping — the flag that keeps
/// `preview_home` to one line per departure — and it rides the player because a resource would not
/// survive the very switch that makes the reading interesting.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
struct Player {
    #[serde(default)]
    previewed: bool,
}

/// A thing in the world that changes state once and should stay that way: a door, a lever, a
/// destroyed spawner. Ordinary game data — the engine never interprets it, it only stores it.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
struct Openable {
    /// What to call this one in the demo's output.
    #[serde(default)]
    label: String,
    #[serde(default)]
    open: bool,
}

/// A district exit: reaching its `Transform.translation` sends the player to `to`, respawned at
/// `anchor`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
struct Gate {
    to: String,
    anchor: String,
}

/// How far the player walks along +X each tick.
const WALK_STEP: f32 = 1.0;

/// How close the player must come to open something.
const REACH: f32 = 1.5;

/// Walks every player one step along +X each tick.
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

/// Opens anything the player walks past. Nothing here knows about districts or persistence: the state
/// change is an ordinary component write, and the marker on the entity is what makes it stick.
fn open(world: &mut World, _ctx: &SystemCtx) {
    let Some(player_x) = player_x(world) else {
        return;
    };
    let reached: Vec<EntityId> = world
        .entities()
        .filter(|&e| world.get::<Openable>(e).unwrap().is_some())
        .filter(|&e| (translation_x(world, e) - player_x).abs() <= REACH)
        .collect();
    for entity in reached {
        if let Some(openable) = world.get_mut::<Openable>(entity).unwrap() {
            openable.open = true;
        }
    }
}

/// Requests a switch when the player reaches a gate, carrying `[player]` — and only the player. A
/// marked entity must never appear in the carry list: it would be asked to leave and to stay behind
/// at once, which the engine reports as `L3027`.
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

/// Reports district A's door from *outside* district A, through the [`ResidueStore`] mirror.
///
/// This is what the mirror is for: residue is engine-side session state, so a system can read what a
/// district kept while standing somewhere else entirely — the reading a map screen or a district
/// tracker needs before the run walks back in. `district-a` is the scene's `name:`, `door` the
/// entity's; a scene that has kept nothing simply reads `None`.
fn preview_home(world: &mut World, _ctx: &SystemCtx) {
    let Some(player) = world.single::<Player>() else {
        return;
    };
    // District A is the one that authors the doors; anywhere else, the run is away from home.
    if world
        .entities()
        .any(|e| world.get::<Openable>(e).unwrap().is_some())
    {
        if let Some(player) = world.get_mut::<Player>(player).unwrap() {
            player.previewed = false;
        }
        return;
    }
    if world.get::<Player>(player).unwrap().unwrap().previewed {
        return;
    }
    match world
        .resource::<ResidueStore>()
        .and_then(|store| store.get::<Openable>("district-a", "door"))
    {
        Some(door) if door.open => {
            println!("away in B    district-a kept its door open (read through the mirror)")
        }
        Some(_) => println!("away in B    district-a kept its door shut"),
        None => println!("away in B    district-a has kept nothing"),
    }
    if let Some(player) = world.get_mut::<Player>(player).unwrap() {
        player.previewed = true;
    }
}

/// The X translation of `entity` (0.0 if it has no `Transform`).
fn translation_x(world: &World, entity: EntityId) -> f32 {
    world
        .get::<Transform>(entity)
        .unwrap()
        .map(|transform| transform.translation[0])
        .unwrap_or(0.0)
}

/// The player's X, if a player is in this district.
fn player_x(world: &World) -> Option<f32> {
    world
        .single::<Player>()
        .map(|player| translation_x(world, player))
}

/// The game: three components and the three systems. A real game's `main` is `ludion::run(game())`.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| {
            reg.register::<Player>("Player")?;
            reg.register::<Openable>("Openable")?;
            reg.register::<Gate>("Gate")
        })
        .system("walk", walk)
        .system("open", open)
        .system("cross_gate", cross_gate)
        .system("preview_home", preview_home)
}

/// District A: a marked `door` and an unmarked `shutter` carrying the identical component, the gate
/// east to district B, and the anchor the player comes back to. Its `name:` is the residue key.
const START: &str = "\
format: 1
name: district-a
seed: 1
entities:
  - name: player
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      Player: {}
  - name: door
    components:
      Residue: {}
      Transform:
        translation:
          - 3.0
          - 0.0
          - 0.0
      Openable:
        label: door
        open: false
  - name: shutter
    components:
      Transform:
        translation:
          - 3.0
          - 0.0
          - 2.0
      Openable:
        label: shutter
        open: false
  - name: gate-east
    components:
      Transform:
        translation:
          - 6.0
          - 0.0
          - 0.0
      Gate:
        to: next.yaml
        anchor: gate-from-start
  - name: gate-from-next
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
";

/// District B: the anchor the player lands on, and the gate straight back — so the demo is a round
/// trip rather than a one-way crossing.
const NEXT: &str = "\
format: 1
name: district-b
seed: 2
entities:
  - name: gate-from-start
    components:
      Transform:
        translation:
          - 40.0
          - 0.0
          - 0.0
  - name: gate-west
    components:
      Transform:
        translation:
          - 43.0
          - 0.0
          - 0.0
      Gate:
        to: start.yaml
        anchor: gate-from-next
";

// --- Headless demo `main` ------------------------------------------------------------------------

/// Writes the two districts, walks the player east through district A (opening both doors), across
/// into district B, and back — then reports which door remembered. Deterministic, no window.
fn main() -> ExitCode {
    let root = demo_root();
    if let Err(error) = write_districts(&root) {
        eprintln!("could not write the demo districts: {error}");
        return ExitCode::FAILURE;
    }

    let mut engine = match game().load_scene_file(&root.join("start.yaml")) {
        Ok(engine) => engine,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };

    // Walk east until both doors are open and the gate is crossed.
    let mut opened = (false, false);
    for _ in 0..8 {
        engine.advance_live();
        if in_district_a(&engine) {
            opened = (is_open(&engine, "door"), is_open(&engine, "shutter"));
        }
    }
    println!(
        "district A   door {}   shutter {}   (both opened on the way past)",
        state(opened.0),
        state(opened.1)
    );

    // Now in district B: walk to its west gate and come home.
    for _ in 0..8 {
        engine.advance_live();
        if in_district_a(&engine) {
            break;
        }
    }
    if let Some(error) = engine.last_transition_error() {
        eprintln!("a switch was rejected: {error}");
        return ExitCode::FAILURE;
    }
    if !in_district_a(&engine) {
        eprintln!("the player never made it back to district A");
        return ExitCode::FAILURE;
    }

    println!(
        "back in A    door {}   shutter {}",
        state(is_open(&engine, "door")),
        state(is_open(&engine, "shutter")),
    );
    println!(
        "\nthe `door` is marked `Residue: {{}}`, so district A kept it open while the run was away; \
         the unmarked `shutter` was rebuilt from the scene text and is shut again."
    );

    let _ = std::fs::remove_dir_all(&root);
    ExitCode::SUCCESS
}

/// Whether the loaded district is A (it is the one that authors the doors).
fn in_district_a(engine: &Engine) -> bool {
    labelled(engine, "door").is_some()
}

/// Whether the labelled `Openable` is open (false if it is not in this district).
fn is_open(engine: &Engine, label: &str) -> bool {
    labelled(engine, label).is_some_and(|openable| openable.open)
}

/// The `Openable` carrying `label` in the loaded district.
fn labelled<'w>(engine: &'w Engine, label: &str) -> Option<&'w Openable> {
    let world = engine.world();
    world
        .entities()
        .filter_map(|id| world.get::<Openable>(id).unwrap())
        .find(|openable| openable.label == label)
}

/// `open` / `shut`, for the demo's two-column output.
fn state(open: bool) -> &'static str {
    if open { "open" } else { "shut" }
}

/// A fresh, process-unique temp directory for the demo's district files.
fn demo_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-scene-residue-example-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Writes the two districts as scene files beside each other under `root`.
fn write_districts(root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(root)?;
    std::fs::write(root.join("start.yaml"), START)?;
    std::fs::write(root.join("next.yaml"), NEXT)?;
    Ok(())
}
