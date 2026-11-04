//! A persistent **player profile** — the copyable memory pattern (Phase 16's text-profile
//! wave).
//!
//! This is the runnable proof of the profile facade surface. A game registers **one** serde
//! save-state type with [`EngineBuilder::profile`] and gets a [`Profile<SaveState>`] resource that
//! boot-loads `save/profile.yaml`, migrates leniently across schema drift, and — in a live host —
//! persists back to disk at the frame boundary. Unlike a data-doc (authored content the engine only
//! *reads*), a profile is a *singleton the sim mutates*: a system reads it to shape the run and writes
//! it back when the run ends.
//!
//! Everything above the `main` divider is copyable into a real game as-is — `game()`, `SaveState`,
//! `Fighter`, and `play_run` are the whole pattern. The two halves the pattern always has:
//!
//! - **read the profile-at-start → derive visible sim state.** `play_run` stamps each fighter's
//!   starting weapon from `unlocked_crossbow` on the first tick, so the profile-at-start reaches the
//!   `WorldHash` through an ordinary component — an unlocked save and a fresh one diverge from frame 0.
//! - **mutate the profile on a run-end condition → `profile.save()`.** At the run-end frame `play_run`
//!   records the run (`runs += 1`, unlock earned) and requests a save; the engine drains the request
//!   and (when a host enabled persistence) writes the canonical file.
//!
//! The headless `main` plays **two runs on the same save directory** — the second is a "process
//! restart" that loads what the first wrote — to show the whole point of a profile: run 1 starts with
//! `fists` and earns the crossbow, run 2 boots already unlocked and starts with `crossbow`. It prints
//! each run's boot profile, derived starting weapon, and frame-stamped save log.
//!
//! Run it headless (writes only under a process-unique temp directory, never the repo tree):
//!
//! ```text
//! cargo run -p ludion --example player_profile
//! ```
//!
//! A real game's `main` is just `ludion::run(game())` — the content root is then the scene file's own
//! directory, the profile lives at `save/profile.yaml` beside it, and `ludion::run` is the one host
//! that enables persistence (a replay, `--check-docs`, or `--profile` run leaves it off, so it never
//! clobbers the developer's real save).
//!
//! [`EngineBuilder::profile`]: ludion::prelude::EngineBuilder::profile

use ludion::prelude::*;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// --- The game (copyable) -------------------------------------------------------------------------

/// The game's one persistent save state, registered with `Engine::builder().profile::<SaveState>()`
/// and loaded from `save/profile.yaml`. It holds meta-progression that outlives a run and a process
/// restart: an unlock and a lifetime run counter.
///
/// Every field carries a serde default (`#[serde(default)]`) — the **growable-profile contract**. A
/// field added to this type in a later build fills from its default when an older save lacks it, and a
/// field removed is dropped, so a player's accumulated save survives the schema change rather than
/// being rejected. That leniency is the one way a profile differs from an authored data-doc.
#[derive(Serialize, Deserialize, JsonSchema, Default, Clone, Debug)]
#[serde(default)]
struct SaveState {
    /// An unlock earned by finishing a run: once set, every future run starts with the crossbow.
    unlocked_crossbow: bool,
    /// A lifetime stat: how many runs this profile has completed.
    runs: u32,
}

/// The scene's fighter. Its `weapon` is *derived from the profile at boot* (empty in the scene, filled
/// on the first tick), so the profile-at-start reaches the `WorldHash` through this ordinary hashed
/// component — the resource-only `Profile` never enters the hash itself.
#[derive(Serialize, Deserialize, JsonSchema, Default, Clone, Debug)]
#[serde(default)]
struct Fighter {
    /// The weapon this fighter starts the run with, stamped by `play_run` from the profile.
    weapon: String,
}

/// The frame a run "ends" (an extraction/death/timeout) and its result is written to the profile — a
/// small number so the headless demo is quick.
const RUN_LENGTH: u64 = 3;

/// The whole pattern in one system: read the profile to shape the run, then write it back when the run
/// ends.
///
/// (a) On the first tick, stamp each fighter's starting weapon from the profile's unlock — the
/// profile-at-start drives visible sim state. (b) At [`RUN_LENGTH`], record the run into the profile
/// (`runs += 1`, unlock earned) and call [`Profile::save`] once; the engine drains the request at the
/// frame boundary and persists it when a host enabled persistence.
fn play_run(world: &mut World, ctx: &SystemCtx) {
    // (a) Derive each fighter's starting weapon from the profile-at-start, on the first tick only.
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

    // (b) At the run-end frame, record the run and request a save — once (idempotent within a frame).
    if ctx.frame.as_u64() == RUN_LENGTH
        && let Some(profile) = world.resource_mut::<Profile<SaveState>>()
    {
        let save = profile.get_mut();
        save.runs += 1;
        save.unlocked_crossbow = true; // finishing a run earns the crossbow for every future run
        profile.save();
    }
}

/// The game: one component, one registered profile type, one system that couples them. A real game's
/// `main` is `ludion::run(game())`; `main` below adds a scratch save directory so it can play two runs
/// headlessly and read the persisted profile back.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<Fighter>("Fighter"))
        .profile::<SaveState>()
        .system("play_run", play_run)
}

/// One fighter with an unset weapon that `play_run` fills from the profile on the first tick. A fixed
/// `seed` makes the run reproducible (seed + profile-at-start + inputs fully determine the trace).
const SCENE: &str = concat!(
    "format: 1\n",
    "seed: 7\n",
    "entities:\n",
    "  - name: hero\n",
    "    components:\n",
    "      Fighter: {}\n",
);

// --- Headless demo `main` ------------------------------------------------------------------------

/// Plays two runs on one save directory, printing each run's boot profile, derived starting weapon,
/// and save log — showing the unlock earned in run 1 persist into run 2's boot. Deterministic, no
/// window, no wire client. (A real game's `main` is just `ludion::run(game())`.)
fn main() -> ExitCode {
    let root = demo_save_root();
    let _ = std::fs::remove_dir_all(&root);

    // Run 1: a fresh install — no save on disk yet, so the profile starts from `Default` (locked).
    let run1 = match play_one_run(&root) {
        Ok(run) => run,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    println!("{}", run1.describe(1));

    // Run 2: a "process restart" on the SAME save directory — the profile now loads what run 1 wrote,
    // so this run boots already unlocked and starts with the crossbow.
    let run2 = match play_one_run(&root) {
        Ok(run) => run,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    println!("{}", run2.describe(2));

    println!(
        "\nthe crossbow unlock and run count from run 1 persisted across the restart: run 2 booted \
         {} with {} completed run(s).",
        if run2.boot_unlocked {
            "unlocked"
        } else {
            "locked"
        },
        run2.boot_runs
    );

    let _ = std::fs::remove_dir_all(&root);
    ExitCode::SUCCESS
}

/// What one run observed — its boot-time profile, the weapon it derived, and the save requests it
/// drained.
struct RunOutcome {
    boot_unlocked: bool,
    boot_runs: u32,
    migration_clean: bool,
    starting_weapon: String,
    saved_frames: Vec<u64>,
}

impl RunOutcome {
    /// A one-line summary of the run for the demo output.
    fn describe(&self, run: u32) -> String {
        format!(
            "run {run}  boot: {:>8} ({} run(s), {})  ->  started with {:<8}  ->  saved at frame(s) {:?}",
            if self.boot_unlocked {
                "unlocked"
            } else {
                "locked"
            },
            self.boot_runs,
            if self.migration_clean {
                "clean load"
            } else {
                "migrated"
            },
            self.starting_weapon,
            self.saved_frames,
        )
    }
}

/// Builds an engine on `root` with persistence on (the one call `ludion::run` makes for a live host),
/// plays a full run, and returns what it observed. Booting a second engine on the same `root` loads
/// the profile the first wrote — the persistence round trip.
fn play_one_run(root: &Path) -> Result<RunOutcome, String> {
    // A live host enables persistence so a `profile.save()` reaches disk; here we call it by hand so
    // the NEXT run loads what this one wrote. `ludion::run` makes exactly this call.
    let mut engine = game()
        .project_root(root)
        .persist_profile()
        .build()
        .map_err(|diagnostic| diagnostic.to_string())?;

    engine
        .load_inline_scene(SCENE)
        .map_err(|diagnostics| join(&diagnostics))?;

    let boot = engine
        .world()
        .resource::<Profile<SaveState>>()
        .expect("the profile is registered")
        .get()
        .clone();
    let migration_clean = engine.profile_migration().is_clean();

    // Tick through the run-end frame so `play_run`'s save request (frame RUN_LENGTH) drains and writes.
    for _ in 0..=RUN_LENGTH {
        engine.advance_live();
    }

    let saved_frames = engine
        .profile_save_log()
        .iter()
        .map(|entry| entry.frame.as_u64())
        .collect();

    Ok(RunOutcome {
        boot_unlocked: boot.unlocked_crossbow,
        boot_runs: boot.runs,
        migration_clean,
        starting_weapon: fighter_weapon(&engine),
        saved_frames,
    })
}

/// A fresh, process-unique temp directory for the demo's save file (removed first, so a re-run starts
/// clean, and removed again at the end — the example never leaves a save under the repo tree).
fn demo_save_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "ludion-player-profile-example-{}",
        std::process::id()
    ))
}

/// The fighter's current derived weapon.
fn fighter_weapon(engine: &Engine) -> String {
    let world = engine.world();
    world
        .entities()
        .find_map(|entity| world.get::<Fighter>(entity).unwrap())
        .map(|fighter| fighter.weapon.clone())
        .expect("the scene has a Fighter entity")
}

/// Joins a scene-load diagnostic list into one error string.
fn join(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| diagnostic.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}
