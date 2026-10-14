//! Behavioral proof for the Phase 16 player-profile facade wave (E5): a game registers one
//! `profile::<SaveState>` save type, boots load-or-default from `save/profile.yaml`, and a system
//! derives a hashed `Fighter.weapon` from the profile-at-start while writing the profile back on a
//! run-end condition. Verified three ways — the profile-at-start drives the `WorldHash` (an unlocked
//! save diverges from a fresh one), two boots from the same save reproduce identical traces, and a
//! `profile.save()` produces a frame-stamped log entry.
//!
//! The game mirrors the `player_profile` example: `SaveState` is the save, `Fighter` the derived
//! component, `play_run` the coupling system (types duplicated minimally, the `data_docs` test mold).

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
const UNLOCKED: &str = "format: 1\nkind: profile\nunlocked_crossbow: true\nruns: 5\n";

/// A fresh, process-unique temp content root (removed first, so a re-run starts clean).
fn fresh_root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-player-profile-test-{}-{name}",
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

fn engine_at(root: &Path) -> Engine {
    let mut engine = game(root).build().expect("the game builds");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    engine
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

/// Drives one run's per-frame `WorldHash` trace.
fn trace(engine: &mut Engine) -> Vec<(ludion_core::time::FrameIndex, ludion_sim::WorldHash)> {
    ludion_engine::drive_hashes(
        engine,
        vec![Command::SimStep(command::SimStepParams {
            ticks: 6,
            hash: false,
        })],
    )
    .expect("the session drives")
}

/// Two boots from the same profile-at-start + same seed produce identical `WorldHash` traces: the
/// whole profile-load → derive path is deterministic, since the resource-only profile reaches the hash
/// only through the `Fighter.weapon` the system derives.
#[test]
fn two_runs_from_the_same_profile_produce_identical_traces() {
    let root = fresh_root("determinism");
    write(&root, "save/profile.yaml", UNLOCKED);

    let mut first = engine_at(&root);
    let mut second = engine_at(&root);
    assert_eq!(
        trace(&mut first),
        trace(&mut second),
        "same profile-at-start + same seed must reproduce the trace"
    );
}

/// A run from an unlocked profile diverges from a default-profile run: the profile-at-start changes the
/// derived starting weapon, which is a hashed component, so the traces differ from the first tick.
#[test]
fn an_unlocked_profile_diverges_from_a_default_run() {
    let unlocked_root = fresh_root("diverge-unlocked");
    write(&unlocked_root, "save/profile.yaml", UNLOCKED);
    // The default root has no save file, so the profile boots from Default (locked).
    let default_root = fresh_root("diverge-default");

    // The unlocked profile derives the crossbow; the default (locked) one derives fists — the visible
    // sim divergence the traces below then observe as a hash difference.
    let mut unlocked = engine_at(&unlocked_root);
    let mut default = engine_at(&default_root);
    unlocked.advance_live();
    default.advance_live();
    assert_eq!(fighter_weapon(&unlocked), "crossbow");
    assert_eq!(fighter_weapon(&default), "fists");

    // Fresh engines for the trace comparison (the peek above ticked these two).
    let mut unlocked = engine_at(&unlocked_root);
    let mut default = engine_at(&default_root);
    let unlocked_trace = trace(&mut unlocked);
    let default_trace = trace(&mut default);
    assert_eq!(
        unlocked_trace[0].1, default_trace[0].1,
        "frame 0 (before any tick) matches — both scenes start with an unset weapon"
    );
    assert_ne!(
        unlocked_trace[1].1, default_trace[1].1,
        "the profile-at-start moves the hash from the first tick (crossbow vs fists)"
    );
    assert_ne!(unlocked_trace, default_trace);
}

/// A `profile.save()` in a system produces exactly one frame-stamped `ProfileSaveEntry` at the
/// requesting frame — the run-end write, regenerable and never recorded (the audio-log mold).
#[test]
fn a_run_end_save_produces_one_frame_stamped_entry() {
    let root = fresh_root("save-log");
    let mut engine = engine_at(&root);

    // Tick through the run-end frame so `play_run`'s save request (frame RUN_LENGTH) drains.
    for _ in 0..=RUN_LENGTH {
        engine.advance_live();
    }

    let log = engine.profile_save_log();
    assert_eq!(log.len(), 1, "exactly one run-end save was requested");
    assert_eq!(
        log[0].frame.as_u64(),
        RUN_LENGTH,
        "the entry is stamped with the run-end frame"
    );
}
