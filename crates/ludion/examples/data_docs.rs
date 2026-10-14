//! A weapon table authored as game **data-docs** and tuned live — the copyable G4 armory pattern
//! (Phase 15's data-doc wave, E4).
//!
//! This is the runnable proof of the data-doc facade surface. A game registers a serde `kind:` type
//! with [`EngineBuilder::doc_kind`] and every matching `*.yaml` under the content root loads,
//! strict-YAML- and schema-validated, into a typed [`DocStore<WeaponDoc>`] resource. A game system
//! reads that store and derives an ordinary component from it — here `equip_weapons` resolves each
//! entity's named weapon into a hashed `WeaponStats`. The store's [`generation`](DocStore::generation)
//! is the "something changed" signal: the system re-derives only when a hot reload bumps it, not every
//! tick, so a designer can edit `weapons/crossbow.yaml` mid-run and watch the number land.
//!
//! Everything above the `main` divider is copyable into a real game as-is — `game()`, `WeaponDoc`,
//! and `equip_weapons` are the whole pattern. Two deliberate simplicities keep the wiring the point:
//! the weapon-name → doc lookup is a plain scan the game does itself (docs carry no cross-references
//! — name resolution is the game's job, by design), and the "combat" that would read `WeaponStats`
//! is left out.
//!
//! Run it headless (boots on the committed weapon docs, prints the equipped stats, then edits a doc
//! on disk + reloads it and prints the retuned stats):
//!
//! ```text
//! cargo run -p ludion --example data_docs
//! ```
//!
//! To host it live for a viewer or a windowed play instead, a real game's `main` is just
//! `ludion::run(game())` — the content root is then the scene file's own directory (where the
//! `weapons/*.yaml` live), and `ludion dev`'s watcher turns every save into the same `asset.reload`
//! this demo drives by hand.

use ludion::prelude::*;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// --- The game (copyable) -------------------------------------------------------------------------

/// One weapon, authored as a `kind: weapon` data-doc (`weapons/*.yaml`). A game registers this with
/// `doc_kind::<WeaponDoc>("weapon")`; the engine validates every matching file against its schema and
/// loads it into a [`DocStore<WeaponDoc>`]. Behavior stays in Rust — a doc names a Rust archetype (the
/// `name` a system resolves) plus the numbers to tune.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
struct WeaponDoc {
    /// The name a `Loadout` entity equips this weapon by — the game resolves it against the store
    /// itself (docs hold no ids/links).
    name: String,
    /// Damage per hit — a plain tunable number.
    damage: f32,
    /// Seconds between hits — another tunable.
    cooldown: f32,
}

/// Names which weapon an entity wields, by the weapon doc's own `name`. A plain string the game
/// resolves against the [`DocStore`] — the docs do no cross-referencing themselves.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default)]
struct Loadout {
    weapon: String,
}

/// The combat stats `equip_weapons` derives from an entity's equipped weapon doc — an ordinary hashed
/// component, so the doc's numbers reach the `WorldHash` exactly here. `applied_generation` caches the
/// store generation these numbers were derived at (`None` until the first derive), so a reload
/// re-derives them and an unchanged store does not.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default)]
#[serde(default)]
struct WeaponStats {
    damage: f32,
    cooldown: f32,
    applied_generation: Option<u64>,
}

/// Derives every `Loadout` entity's `WeaponStats` from the weapon doc it names, but only when the
/// store's [`generation`](DocStore::generation) differs from the stats' cached `applied_generation` —
/// the canonical use of the reload signal. Plain typed world access: read the store, resolve the
/// name, write the derived numbers.
fn equip_weapons(world: &mut World, _ctx: &SystemCtx) {
    let Some(generation) = world
        .resource::<DocStore<WeaponDoc>>()
        .map(|store| store.generation())
    else {
        return; // the `weapon` kind isn't registered — nothing to equip from
    };

    // Resolve every stale entity's weapon into owned numbers first, so the DocStore + read borrows
    // all end before the mutable write pass below.
    let entities: Vec<EntityId> = world.entities().collect();
    let mut updates: Vec<(EntityId, f32, f32)> = Vec::new();
    for entity in entities {
        let Some(loadout) = world.get::<Loadout>(entity).unwrap() else {
            continue;
        };
        let already_current = world
            .get::<WeaponStats>(entity)
            .unwrap()
            .is_some_and(|stats| stats.applied_generation == Some(generation));
        if already_current {
            continue; // stats already reflect this store generation — skip the re-derive
        }
        let weapon = loadout.weapon.clone();
        let store = world.resource::<DocStore<WeaponDoc>>().unwrap();
        if let Some(doc) = store
            .iter()
            .map(|(_, doc)| doc)
            .find(|doc| doc.name == weapon)
        {
            updates.push((entity, doc.damage, doc.cooldown));
        }
    }

    for (entity, damage, cooldown) in updates {
        if let Some(stats) = world.get_mut::<WeaponStats>(entity).unwrap() {
            stats.damage = damage;
            stats.cooldown = cooldown;
            stats.applied_generation = Some(generation);
        }
    }
}

/// The game: the two gameplay components plus the `weapon` doc kind and the system that couples them.
/// A real game's `main` is `ludion::run(game())`; `main` below adds a demo content root on top so it
/// can write and tune fixture docs headlessly.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| {
            reg.register::<Loadout>("Loadout")?;
            reg.register::<WeaponStats>("WeaponStats")
        })
        .doc_kind::<WeaponDoc>("weapon")
        .system("equip_weapons", equip_weapons)
}

/// One fighter equipping the `crossbow` weapon by name, with default (zero) stats that `equip_weapons`
/// fills in from the doc on the first tick.
const SCENE: &str = concat!(
    "format: 1\n",
    "entities:\n",
    "  - name: fighter\n",
    "    components:\n",
    "      Loadout:\n",
    "        weapon: crossbow\n",
    "      WeaponStats: {}\n",
);

/// The committed weapon docs the demo boots on — two `kind: weapon` files under `weapons/`. In a real
/// game these are version-controlled content the agent (or a designer) authors and tunes; here `main`
/// writes them to a temp content root so the example is self-contained.
const CROSSBOW_V1: &str = "format: 1\nkind: weapon\nname: crossbow\ndamage: 12.0\ncooldown: 0.8\n";
const LONGBOW: &str = "format: 1\nkind: weapon\nname: longbow\ndamage: 20.0\ncooldown: 1.4\n";
/// The retuned crossbow the demo hot-reloads mid-run — the same doc, one number changed.
const CROSSBOW_V2: &str = "format: 1\nkind: weapon\nname: crossbow\ndamage: 30.0\ncooldown: 0.5\n";

// --- Headless demo `main` ------------------------------------------------------------------------

/// Boots on the committed weapon docs, prints the equipped fighter's derived stats, then edits the
/// crossbow doc on disk and drives an `asset.reload` — the tuning loop — and prints the retuned stats,
/// showing the store generation bump. No window, no wire client, deterministic. (A real game's `main`
/// is just `ludion::run(game())`.)
fn main() -> ExitCode {
    let root = demo_content_root();
    if let Err(error) = write_weapon_docs(&root) {
        eprintln!("could not write the demo weapon docs: {error}");
        return ExitCode::FAILURE;
    }

    let mut engine = match game().project_root(&root).build() {
        Ok(engine) => engine,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(diagnostics) = engine.load_inline_scene(SCENE) {
        for diagnostic in diagnostics {
            eprintln!("{diagnostic}");
        }
        return ExitCode::FAILURE;
    }

    // One tick derives WeaponStats from the boot docs (generation 0).
    step(&mut engine);
    let (damage, cooldown) = fighter_stats(&engine);
    println!(
        "boot        crossbow -> damage {damage:5.1} cooldown {cooldown:4.2}  (store gen {})",
        store_generation(&engine)
    );

    // Tune the crossbow on disk and reload it, exactly as `ludion dev`'s watcher would on a save.
    if let Err(error) = std::fs::write(root.join("weapons/crossbow.yaml"), CROSSBOW_V2) {
        eprintln!("could not retune the crossbow doc: {error}");
        return ExitCode::FAILURE;
    }
    if let Err(diagnostic) = reload(&mut engine, "weapons/crossbow.yaml") {
        eprintln!("{diagnostic}");
        return ExitCode::FAILURE;
    }

    // The next tick re-derives, because the store generation moved.
    step(&mut engine);
    let (damage, cooldown) = fighter_stats(&engine);
    println!(
        "hot reload  crossbow -> damage {damage:5.1} cooldown {cooldown:4.2}  (store gen {})",
        store_generation(&engine)
    );

    ExitCode::SUCCESS
}

/// A fresh, process-unique temp directory for the demo's weapon docs (removed first, so a re-run
/// starts clean).
fn demo_content_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-data-docs-example-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Writes the two committed weapon docs under `root/weapons/`.
fn write_weapon_docs(root: &Path) -> std::io::Result<()> {
    let weapons = root.join("weapons");
    std::fs::create_dir_all(&weapons)?;
    std::fs::write(weapons.join("crossbow.yaml"), CROSSBOW_V1)?;
    std::fs::write(weapons.join("longbow.yaml"), LONGBOW)?;
    Ok(())
}

/// Advances one fixed tick, running `equip_weapons`.
fn step(engine: &mut Engine) {
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .expect("the sim steps");
}

/// Reloads one data-doc through the same `asset.reload` command the `ludion dev` watcher forwards on
/// a save — re-validating the file and upserting it into the `DocStore` (bumping its generation).
fn reload(engine: &mut Engine, path: &str) -> Result<(), Diagnostic> {
    engine
        .execute(Command::AssetReload(command::AssetReloadParams {
            path: Some(path.to_string()),
        }))
        .map(|_| ())
}

/// The fighter's current derived `(damage, cooldown)`.
fn fighter_stats(engine: &Engine) -> (f32, f32) {
    let world = engine.world();
    world
        .single::<WeaponStats>()
        .map(|e| {
            let stats = world.get::<WeaponStats>(e).unwrap().unwrap();
            (stats.damage, stats.cooldown)
        })
        .expect("the scene has a WeaponStats entity")
}

/// The weapon `DocStore`'s current generation — 0 at boot, bumped by each applied reload.
fn store_generation(engine: &Engine) -> u64 {
    engine
        .world()
        .resource::<DocStore<WeaponDoc>>()
        .map(|store| store.generation())
        .unwrap_or(0)
}
