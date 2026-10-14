//! An in-run **scene transition** — the copyable G6 "districts" pattern (Phase 17's transition
//! wave, E7).
//!
//! This is the runnable proof of the scene-switch facade surface. A game splits its world into
//! districts authored as **separate scene files** and connects them with gates. When a system decides
//! the run should move on, it sets the [`SceneSwitch`] resource
//! (`request(scene_path, spawn_anchor, carry)`); the engine performs the switch at the next frame
//! boundary — rebuilding the world from the target district while **continuing** the run: the frame
//! counter keeps counting, the RNG stream keeps flowing, and an explicit carry set (here the player)
//! travels, serialized out of the old district and respawned at a `Name`d anchor in the new one.
//!
//! Everything above the `main` divider is copyable into a real game as-is — `game()`, `Player`,
//! `Gate`, `walk`, and `cross_gate` are the whole pattern. The two halves it always has:
//!
//! - **a gate carries where it leads.** A `Gate` component names the destination district and the
//!   anchor to drop the player at, so districts are wired in *data*, not code — the same system moves
//!   the player through any gate it reaches.
//! - **the run state rides carried components, not resources.** The player's `hp` is a component on the
//!   carried entity, so it survives the district change verbatim (resources do **not** carry — model
//!   run state as components, and the run clock as `FrameIndex` arithmetic).
//!
//! A gate never re-fires after a switch because the destination district simply has no `Gate` entity —
//! there is nothing to cross. The carried player lands at the anchor's `Transform.translation` (only
//! the translation is overridden; `hp`, `Name`, rotation, and scale travel untouched).
//!
//! Run it headless (writes the two district files under a process-unique temp directory, never the
//! repo tree, then walks the player across the gate and reports the carry):
//!
//! ```text
//! cargo run -p ludion --example scene_transition
//! ```
//!
//! A real game's `main` is just `ludion::run(game())` — the content root is then the start scene's own
//! directory, the other districts are scene files beside it, and a gate's `to:` resolves against that
//! root. `ludion::run` is the one host that reads a target district from disk on a live switch (and,
//! under `--record`, embeds its text in the `.lrp` so the session reproduces cross-machine).

use ludion::prelude::*;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// --- The game (copyable) -------------------------------------------------------------------------

/// The carried player. `hp` is ordinary run state that must survive a district change — so it lives on
/// this entity, which the switch carries, rather than in a resource (resources do not carry).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Player {
    hp: u32,
}

/// A district exit, authored on an entity in the scene: reaching its `Transform.translation` sends the
/// player to `to` (a scene file resolved against the project root) and respawns them at the `anchor`.
/// Gates are pure data — one `cross_gate` system drives every gate in the game.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
struct Gate {
    /// The target district's scene file, relative to the project root (the start scene's directory).
    to: String,
    /// The `name:` of the entity in the target district to drop the player at.
    anchor: String,
}

/// How far the player walks along +X each tick — the "approaching the gate" motion.
const WALK_STEP: f32 = 1.0;

/// Walks every player one step along +X each tick, so it eventually reaches a gate.
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

/// Requests a switch when the player reaches a gate, carrying `[player]`. The target district has no
/// `Gate` entity, so this never re-fires after the crossing — the switch is naturally one-shot.
fn cross_gate(world: &mut World, _ctx: &SystemCtx) {
    let Some(player) = world.single::<Player>() else {
        return;
    };
    let player_x = translation_x(world, player);

    // Collect the gates the player has reached (owned, so the world borrows end before the request).
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

/// The X translation of `entity` (0.0 if it has no `Transform`).
fn translation_x(world: &World, entity: EntityId) -> f32 {
    world
        .get::<Transform>(entity)
        .unwrap()
        .map(|transform| transform.translation[0])
        .unwrap_or(0.0)
}

/// The game: the two components plus the walk-and-cross systems. A real game's `main` is
/// `ludion::run(game())` with the start scene passed as `--scene` (or set as the default).
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| {
            reg.register::<Player>("Player")?;
            reg.register::<Gate>("Gate")
        })
        .system("walk", walk)
        .system("cross_gate", cross_gate)
}

/// District A (the start): the player at the origin with 100 HP, and a gate at X = 5 leading to
/// `next.yaml`'s `gate-from-start` anchor.
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

/// District B (the target): the `gate-from-start` anchor where the carried player lands, plus a native
/// prop. Its `seed:` differs deliberately — a switch ignores it, so the run's one RNG stream continues.
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

// --- Headless demo `main` ------------------------------------------------------------------------

/// Writes the two districts as scene files, boots on the start district, walks the player across the
/// gate, and reports the carry — the player's HP and position before and after the switch, showing the
/// run continue into district B. Deterministic, no window, no wire client. (A real game's `main` is
/// just `ludion::run(game())`.)
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

    // Walk until the switch swaps district B in (its anchor + native prop + the carried player = 3
    // entities, versus district A's 2). A generous tick budget — the crossing is at X = 5.
    let mut before = engine_state(&engine);
    for _ in 0..12 {
        if engine.world().entity_count() == 3 {
            break; // district B is loaded — the switch has happened
        }
        before = engine_state(&engine);
        engine.advance_live();
    }

    if let Some(error) = engine.last_transition_error() {
        eprintln!("the switch was rejected: {error}");
        return ExitCode::FAILURE;
    }
    let after = engine_state(&engine);

    println!(
        "district A  frame {:>2}  player hp {} at x {:.1}",
        before.frame, before.hp, before.x
    );
    println!(
        "district B  frame {:>2}  player hp {} at x {:.1}  (dropped at the gate-from-start anchor)",
        after.frame, after.hp, after.x
    );
    println!(
        "\nthe player crossed the gate carrying its {} HP; the run continued (the frame counter kept \
         counting, {} -> {}, and district B's seed was ignored).",
        after.hp, before.frame, after.frame
    );

    let _ = std::fs::remove_dir_all(&root);
    ExitCode::SUCCESS
}

/// A snapshot of the player for the demo output: the current frame, the player's HP, and its X.
struct DemoState {
    frame: u64,
    hp: u32,
    x: f32,
}

/// Reads the current player state off the engine.
fn engine_state(engine: &Engine) -> DemoState {
    let world = engine.world();
    let player = world.single::<Player>().expect("the district has a player");
    DemoState {
        frame: engine.frame().as_u64(),
        hp: world.get::<Player>(player).unwrap().unwrap().hp,
        x: translation_x(world, player),
    }
}

/// A fresh, process-unique temp directory for the demo's district files (removed first, so a re-run
/// starts clean, and removed again at the end — the example never leaves scenes under the repo tree).
fn demo_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-scene-transition-example-{}",
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
