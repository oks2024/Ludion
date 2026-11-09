//! A **suspended run**: save the running game at a "room boundary", stop, and resume it in a later
//! process.
//!
//! A game system calls [`RunSave::suspend`]; a live host (one that enabled persistence) writes
//! `save/suspend.json` at the frame boundary. The next boot reads it back as [`RunSave::pending`], and
//! `resume()` restores the world in place, strictly: a changed build or schema is refused with `L1401`.
//!
//! Everything above the `main` divider is copyable. Run it headless (writes only under a temp dir):
//!
//! ```text
//! cargo run -p ludion --example run_save
//! ```

use ludion::prelude::*;
use std::path::PathBuf;
use std::process::ExitCode;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// --- The game (copyable) -------------------------------------------------------------------------

/// Progress through the run; it changes every tick, so a wrong resume would show in the hash.
#[derive(Serialize, Deserialize, JsonSchema, Default, Clone, Debug)]
#[serde(default)]
struct Progress {
    steps: u32,
}

/// The "room boundary" frame at which the game suspends.
const BOUNDARY: u64 = 4;

fn advance(world: &mut World, ctx: &SystemCtx) {
    let entities: Vec<EntityId> = world.entities().collect();
    for entity in entities {
        if let Some(progress) = world.get_mut::<Progress>(entity).unwrap() {
            progress.steps += 1;
        }
    }
    if ctx.frame.as_u64() == BOUNDARY
        && let Some(run) = world.resource_mut::<RunSave>()
    {
        run.suspend();
    }
}

pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<Progress>("Progress"))
        .run_save()
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
    let root: PathBuf =
        std::env::temp_dir().join(format!("ludion-run-save-example-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);

    // Run 1: play past the boundary, which suspends, then "quit".
    let suspended_hash = {
        let mut engine = game()
            .project_root(&root)
            .persist_profile()
            .build()
            .unwrap();
        engine.load_inline_scene(SCENE).unwrap();
        for _ in 0..=BOUNDARY {
            engine.advance_live();
        }
        println!(
            "run 1: suspended at frame {}, steps {}",
            engine.frame().as_u64(),
            steps(&engine)
        );
        engine.world_hash().unwrap()
    };

    // Run 2: a new process on the same root sees the suspended run and resumes it.
    let mut engine = game()
        .project_root(&root)
        .persist_profile()
        .build()
        .unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    let info = engine
        .world()
        .resource::<RunSave>()
        .unwrap()
        .pending()
        .cloned()
        .expect("run 1 left a suspended run");
    println!("run 2: pending run from frame {}", info.frame.as_u64());
    engine
        .world_mut()
        .resource_mut::<RunSave>()
        .unwrap()
        .resume();
    engine.advance_live();
    let outcome = engine
        .world()
        .resource::<RunSave>()
        .unwrap()
        .last_resume()
        .cloned();
    println!("run 2: {outcome:?}; frame {}", engine.frame().as_u64());
    let resumed = matches!(outcome, Some(ResumeOutcome::Resumed { .. }));
    let same_world = engine.world_hash().unwrap() == suspended_hash;
    let file_gone = !root.join("save/suspend.json").exists();
    println!("run 2: world hash matches the suspended world: {same_world}");
    println!("run 2: suspend.json is gone: {file_gone}");

    let _ = std::fs::remove_dir_all(&root);
    if resumed && same_world && file_gone {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
