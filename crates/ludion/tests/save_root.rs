//! Behavioral proof for the save-root seam: `EngineBuilder::save_root(dir)`
//! moves the player profile off the project (content) root onto a separate, writable directory —
//! the seam a shipped build needs when its content root is read-only. Verified two ways — an
//! explicit save root receives the write and reads it back on a second boot, and the profile stays
//! on the project root when no save root is given (the pre-existing behavior, pinned).
//!
//! The game mirrors `player_profile.rs` (itself mirroring the `player_profile` example): `SaveState`
//! is the save, `Fighter` the derived component, `play_run` the coupling system (types duplicated
//! minimally, the `data_docs` test mold).

use ludion::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, JsonSchema, Default, Clone, Debug)]
#[serde(default)]
struct SaveState {
    unlocked_crossbow: bool,
    runs: u32,
}

#[derive(Serialize, Deserialize, JsonSchema, Default, Clone, Debug)]
#[serde(default)]
struct Fighter {
    weapon: String,
}

const RUN_LENGTH: u64 = 3;

fn play_run(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 0 {
        let unlocked = world
            .resource::<Profile<SaveState>>()
            .is_some_and(|profile| profile.get().unlocked_crossbow);
        let starting_weapon = if unlocked { "crossbow" } else { "fists" };
        let entities: Vec<EntityId> = world.entities().collect();
        for entity in entities {
            if let Some(fighter) = world.get_mut::<Fighter>(entity).unwrap() {
                fighter.weapon = starting_weapon.to_string();
            }
        }
    }
    if ctx.frame.as_u64() == RUN_LENGTH
        && let Some(profile) = world.resource_mut::<Profile<SaveState>>()
    {
        let save = profile.get_mut();
        save.runs += 1;
        save.unlocked_crossbow = true;
        profile.save();
    }
}

fn game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<Fighter>("Fighter"))
        .profile::<SaveState>()
        .system("play_run", play_run)
        .project_root(root)
}

const SCENE: &str = "format: 1\nseed: 7\nentities:\n  - components:\n      Fighter: {}\n";

/// A fresh, process-unique temp directory (removed first, so a re-run starts clean).
fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-save-root-test-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes `contents` to `root`/`rel`, creating parent directories.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// The equipped fighter's derived weapon.
fn fighter_weapon(engine: &Engine) -> String {
    let world = engine.world();
    world
        .entities()
        .find_map(|entity| world.get::<Fighter>(entity).unwrap())
        .map(|fighter| fighter.weapon.clone())
        .expect("the scene has a Fighter entity")
}

/// Drives frames through the run-end save request (`play_run`'s save fires at `RUN_LENGTH`).
fn play_to_run_end(engine: &mut Engine) {
    for _ in 0..=RUN_LENGTH {
        engine.advance_live();
    }
}

/// An explicit `save_root` receives the profile write, not the content root — and a second engine
/// booted against the same save root reads it back, proving load and save resolve the same path.
#[test]
fn profile_persists_under_the_save_root_not_the_content_root() {
    let content = fresh_dir("content-a");
    let save = fresh_dir("save-b");
    let scene_path = content.join("main.yaml");
    write(&content, "main.yaml", SCENE);

    let mut engine = game(&content)
        .save_root(&save)
        .persist_profile()
        .load_scene_file(&scene_path)
        .expect("the game builds and the scene loads");
    assert_eq!(engine.project_root(), content);
    assert_eq!(engine.save_root(), save);

    play_to_run_end(&mut engine);

    assert!(
        save.join("save/profile.yaml").exists(),
        "the profile was written under the save root"
    );
    assert!(
        !content.join("save/profile.yaml").exists(),
        "the profile was NOT written under the content root"
    );

    // A second boot against the same save root loads the saved state back: the unlocked profile
    // derives the crossbow at frame 0, the same divergence `player_profile.rs` uses as its proof.
    let mut second = game(&content)
        .save_root(&save)
        .load_scene_file(&scene_path)
        .expect("the second boot builds and loads");
    second.advance_live();
    assert_eq!(
        fighter_weapon(&second),
        "crossbow",
        "the second boot's profile-at-start came from the save root's file"
    );
}

/// With no `save_root` call, the profile lands beside the content root — the pre-existing behavior,
/// pinned so the new seam is additive.
#[test]
fn default_save_root_is_the_content_root() {
    let content = fresh_dir("content-default");
    let scene_path = content.join("main.yaml");
    write(&content, "main.yaml", SCENE);

    let mut engine = game(&content)
        .persist_profile()
        .load_scene_file(&scene_path)
        .expect("the game builds and the scene loads");
    assert_eq!(engine.project_root(), content);
    assert_eq!(engine.save_root(), content);

    play_to_run_end(&mut engine);

    assert!(
        content.join("save/profile.yaml").exists(),
        "with no save root, the profile lands beside the content root"
    );
}
