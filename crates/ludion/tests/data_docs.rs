//! Behavioral proof for the Phase 15 game data-doc facade wave (E4): a game registers a `kind: weapon`
//! doc type, boots on a content root of weapon docs, and a system derives a hashed `WeaponStats`
//! component from the `DocStore`. Verified two ways — the derived stats actually track the doc (and
//! react to a hot reload's generation bump, not before), and a two-run `WorldHash` trace (the store is
//! resource-only, so its numbers reach the hash only through `WeaponStats`, which two identical runs
//! hash identically).
//!
//! The game here mirrors the `data_docs` example: `Loadout` names a weapon, `equip_weapons` resolves
//! it from `DocStore<WeaponDoc>` into `WeaponStats`, keyed off the store `generation`.

use ludion::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
struct WeaponDoc {
    name: String,
    damage: f32,
    cooldown: f32,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default)]
struct Loadout {
    weapon: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default)]
#[serde(default)]
struct WeaponStats {
    damage: f32,
    cooldown: f32,
    applied_generation: Option<u64>,
}

fn equip_weapons(world: &mut World, _ctx: &SystemCtx) {
    let Some(generation) = world
        .resource::<DocStore<WeaponDoc>>()
        .map(|store| store.generation())
    else {
        return;
    };
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
            continue;
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

fn game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .components(|reg| {
            reg.register::<Loadout>("Loadout")?;
            reg.register::<WeaponStats>("WeaponStats")
        })
        .doc_kind::<WeaponDoc>("weapon")
        .system("equip_weapons", equip_weapons)
        .project_root(root)
}

const SCENE: &str = "format: 1\nentities:\n  - components:\n      Loadout:\n        weapon: crossbow\n      WeaponStats: {}\n";
const CROSSBOW_V1: &str = "format: 1\nkind: weapon\nname: crossbow\ndamage: 12.0\ncooldown: 0.8\n";
const CROSSBOW_V2: &str = "format: 1\nkind: weapon\nname: crossbow\ndamage: 30.0\ncooldown: 0.5\n";

/// A fresh, process-unique temp content root holding one crossbow doc (removed first, so a re-run
/// starts clean).
fn fresh_root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-data-docs-test-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("weapons")).unwrap();
    std::fs::write(dir.join("weapons/crossbow.yaml"), CROSSBOW_V1).unwrap();
    dir
}

fn engine_at(root: &Path) -> Engine {
    let mut engine = game(root).build().expect("the game builds on valid docs");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    engine
}

fn step(engine: &mut Engine) {
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .expect("the sim steps");
}

/// The equipped fighter's derived `(damage, cooldown)`.
fn stats(engine: &Engine) -> (f32, f32) {
    let world = engine.world();
    world
        .single::<WeaponStats>()
        .map(|e| {
            let s = world.get::<WeaponStats>(e).unwrap().unwrap();
            (s.damage, s.cooldown)
        })
        .expect("the scene has a WeaponStats entity")
}

/// The weapon `DocStore`'s current generation.
fn store_generation(engine: &Engine) -> u64 {
    engine
        .world()
        .resource::<DocStore<WeaponDoc>>()
        .unwrap()
        .generation()
}

/// `WeaponStats` is derived from the equipped weapon's doc on the first tick, and re-derived after a
/// hot reload bumps the store generation — the generation signal driving a real component.
#[test]
fn weapon_stats_derive_from_the_doc_and_react_to_a_reload() {
    let root = fresh_root("derive");
    let mut engine = engine_at(&root);

    step(&mut engine);
    assert_eq!(
        stats(&engine),
        (12.0, 0.8),
        "the boot doc's numbers land in WeaponStats"
    );
    assert_eq!(store_generation(&engine), 0, "a fresh boot is generation 0");

    // Retune the crossbow on disk and reload it — the `ludion dev` tuning loop.
    std::fs::write(root.join("weapons/crossbow.yaml"), CROSSBOW_V2).unwrap();
    engine
        .execute(Command::AssetReload(command::AssetReloadParams {
            path: Some("weapons/crossbow.yaml".to_string()),
        }))
        .expect("the doc reload validates and applies");
    assert_eq!(
        store_generation(&engine),
        1,
        "the reload bumped the generation"
    );

    step(&mut engine);
    assert_eq!(
        stats(&engine),
        (30.0, 0.5),
        "the retuned numbers land after the reload"
    );
}

/// Two identical runs from the same content root produce identical per-frame `WorldHash` traces: the
/// whole doc-load → derive path is deterministic, since the resource-only store surfaces in the hash
/// only through the `WeaponStats` component the system derives.
#[test]
fn two_runs_produce_identical_worldhash_traces() {
    let root = fresh_root("determinism");
    let run = || {
        let mut engine = engine_at(&root);
        ludion_engine::drive_hashes(
            &mut engine,
            vec![Command::SimStep(command::SimStepParams {
                ticks: 30,
                hash: false,
            })],
        )
        .expect("the session drives")
    };
    assert_eq!(
        run(),
        run(),
        "the data-doc derive path must be deterministic across two boots"
    );
}
