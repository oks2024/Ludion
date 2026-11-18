//! An **in-memory checkpoint**: capture the world at a threshold, "die", restore, and check the
//! world is the captured one while the frame keeps counting.
//!
//! A game system calls [`Checkpoints::capture`] and [`Checkpoints::restore`] from hashed state; the
//! engine drains them at the frame boundary. The slot is kept by a restore, so the same checkpoint
//! serves every death.
//!
//! Everything above the `main` divider is copyable. Run it headless:
//!
//! ```text
//! cargo run -p ludion --example checkpoints
//! ```

use ludion::prelude::*;
use std::process::ExitCode;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// --- The game (copyable) -------------------------------------------------------------------------

/// Progress through the run; it changes every tick, so a wrong restore would show in the hash.
#[derive(Serialize, Deserialize, JsonSchema, Default, Clone, Debug)]
#[serde(default)]
struct Progress {
    steps: u32,
}

/// The frame at which the player crosses the threshold, and the frame at which they die.
const THRESHOLD: u64 = 4;
const DEATH: u64 = 9;

fn advance(world: &mut World, ctx: &SystemCtx) {
    let entities: Vec<EntityId> = world.entities().collect();
    for entity in entities {
        if let Some(progress) = world.get_mut::<Progress>(entity).unwrap() {
            progress.steps += 1;
        }
    }
    let Some(checkpoints) = world.resource_mut::<Checkpoints>() else {
        return;
    };
    match ctx.frame.as_u64() {
        THRESHOLD => checkpoints.capture("threshold"),
        DEATH => checkpoints.restore("threshold"),
        _ => {}
    }
}

pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<Progress>("Progress"))
        .checkpoints(CheckpointConfig::default())
        .system("advance", advance)
}

const SCENE: &str = concat!(
    "format: 1\n",
    "seed: 7\n",
    "entities:\n",
    "  - name: hero\n",
    "    components:\n",
    "      Progress: {}\n",
);

// --- Headless demo `main` ------------------------------------------------------------------------

fn steps(engine: &Engine) -> u32 {
    let world = engine.world();
    world
        .entities()
        .find_map(|entity| world.get::<Progress>(entity).unwrap())
        .map(|progress| progress.steps)
        .expect("the scene has a Progress entity")
}

fn main() -> ExitCode {
    let mut engine = game().build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();

    let mut captured_hash = None;
    for _ in 0..=DEATH {
        engine.advance_live();
        if engine.frame().as_u64() == THRESHOLD + 1 {
            captured_hash = Some(engine.world_hash().unwrap());
            println!(
                "captured at frame {}, steps {}",
                engine.frame().as_u64(),
                steps(&engine)
            );
        }
    }
    println!(
        "restored at frame {}, steps {}",
        engine.frame().as_u64(),
        steps(&engine)
    );

    let outcome = engine
        .world()
        .resource::<Checkpoints>()
        .unwrap()
        .last_restore()
        .cloned();
    println!("last restore: {outcome:?}");
    let same_world = Some(engine.world_hash().unwrap()) == captured_hash;
    let frame_kept = engine.frame().as_u64() == DEATH + 1;
    let slot_kept = engine
        .world()
        .resource::<Checkpoints>()
        .unwrap()
        .slot("threshold")
        .is_some();
    println!("world hash matches the captured world: {same_world}");
    println!("the frame kept counting: {frame_kept}; the slot is kept: {slot_kept}");

    if same_world && frame_kept && slot_kept {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
