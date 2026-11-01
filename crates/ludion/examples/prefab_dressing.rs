//! **An imported prefab placed as set dressing** — `presentation: true` on a prefab instance entry,
//! and `spawn_prefab(..., true)` from a presentation system, end to end.
//!
//! `IMPORTED` is shaped like a `ludion import` of a glTF: a root and a mesh child, every part sim.
//! The scene places it as `prop` with `presentation: true`, so both parts spawn from the presentation
//! allocator and the scene hashes exactly as it does without the entry. A presentation system then
//! spawns a second copy with the flag at runtime, and `scene_text` writes the load-time instance back
//! as one entry with `presentation: true`.
//!
//! Everything above the `main` divider is copyable into a real game (`ludion docs presentation`,
//! `ludion docs prefab-instances`).
//!
//! Run it headless (prints one line per step, self-checks each):
//!
//! ```text
//! cargo run -p ludion --example prefab_dressing
//! ```

use ludion::prelude::*;
use ludion_ecs::is_presentation;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// The prefab, by content-root-relative path.
const PROP_PATH: &str = "prefabs/imported.prefab.yaml";

/// The game: one presentation system that dresses the scene once. A real game's `main` is
/// `ludion::run(game())`.
pub fn game() -> EngineBuilder {
    Engine::builder().presentation_system("dress", dress)
}

/// Spawns a second copy of the prop as presentation parts, once. `true` makes every part
/// presentation, which is the only kind a presentation system may spawn.
fn dress(world: &mut World, _ctx: &SystemCtx) {
    let dressed = world
        .query_ids::<(&Name,)>()
        .any(|(_, (name,))| name.0 == "extra/root");
    if !dressed && let Err(found) = spawn_prefab(world, PROP_PATH, None, Some("extra"), true) {
        for diagnostic in found {
            report_full_diagnostic(world, "dress", diagnostic);
        }
    }
}

/// An imported-style prefab: a root and a mesh child, every part sim.
const IMPORTED: &str = "\
format: 1
kind: prefab
name: imported
entities:
  - name: root
    components:
      Transform: {}
  - name: child
    components:
      Transform:
        translation:
          - 0.0
          - 0.5
          - 0.0
      Mesh:
        shape: cube
      Parent:
        index: 0
        generation: 0
";

/// A sim ground, the scene's only hashed entity.
const GROUND: &str = "\
format: 1
name: dressing
seed: 3
entities:
  - name: ground
    components:
      Transform: {}
";

/// The ground and the prop placed as dressing.
const SCENE: &str = "\
format: 1
name: dressing
seed: 3
entities:
  - name: ground
    components:
      Transform: {}
  - name: prop
    prefab: prefabs/imported.prefab.yaml
    presentation: true
";

// --- Headless demo `main` ------------------------------------------------------------------------

/// Runs every step, printing one line each. Deterministic, no window.
fn main() -> ExitCode {
    let root = demo_content_root();
    let result = write_prefab(&root)
        .map_err(|error| format!("could not write the demo prefab: {error}"))
        .and_then(|()| run(&root));
    let _ = std::fs::remove_dir_all(&root);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

/// Fails with `message` unless `ok`, otherwise prints `step`.
fn step(ok: bool, step: &str, message: &str) -> Result<(), String> {
    if ok {
        println!("ok  {step}");
        Ok(())
    } else {
        Err(format!("FAILED {step}: {message}"))
    }
}

/// Loads `scene` over `root`.
fn load(root: &Path, scene: &str) -> Result<Engine, String> {
    let mut engine = game()
        .project_root(root)
        .build()
        .map_err(|diagnostic| diagnostic.to_string())?;
    engine.load_inline_scene(scene).map_err(|diagnostics| {
        diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    Ok(engine)
}

/// Advances `ticks` fixed ticks.
fn advance(engine: &mut Engine, ticks: u32) -> Result<(), String> {
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks: u64::from(ticks),
            hash: false,
        }))
        .map(|_| ())
        .map_err(|diagnostic| diagnostic.to_string())
}

/// Every named entity, by name.
fn named(engine: &mut Engine) -> std::collections::BTreeMap<String, EntityId> {
    engine
        .world_mut()
        .query_ids::<(&Name,)>()
        .map(|(id, (name,))| (name.0.clone(), id))
        .collect()
}

fn run(root: &Path) -> Result<(), String> {
    let mut engine = load(root, SCENE)?;
    let ids = named(&mut engine);
    step(
        ["prop/root", "prop/child"]
            .iter()
            .all(|part| ids.get(*part).is_some_and(|&id| is_presentation(id))),
        "every part of the instance is presentation",
        &format!("{ids:?}"),
    )?;

    let mut plain = load(root, GROUND)?;
    advance(&mut engine, 3)?;
    advance(&mut plain, 3)?;
    let (with, without) = (
        engine.world_hash().map_err(|d| d.to_string())?,
        plain.world_hash().map_err(|d| d.to_string())?,
    );
    step(
        with == without,
        "sim.hash equals the scene without the entry",
        &format!("{with:?} != {without:?}"),
    )?;

    let ids = named(&mut engine);
    step(
        ["extra/root", "extra/child"]
            .iter()
            .all(|part| ids.get(*part).is_some_and(|&id| is_presentation(id))),
        "a presentation system spawned a second copy with the flag",
        &format!("{ids:?}"),
    )?;

    let saved = engine.scene_text().map_err(|d| d.to_string())?;
    step(
        saved.matches("presentation: true").count() == 2 && saved.contains("name: prop"),
        "scene_text writes the load-time instance as one entry with presentation: true",
        &saved,
    )
}

/// A fresh, process-unique temp directory for the demo's content.
fn demo_content_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-prefab-dressing-example-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Writes the prefab under `root/prefabs/`.
fn write_prefab(root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(root.join("prefabs"))?;
    std::fs::write(root.join(PROP_PATH), IMPORTED)
}
