//! A world rebuild is transactional: `scene.load`, `engine.restore` and a scene switch either land
//! completely or leave the engine exactly as it was.
//!
//! Every test boots cleanly, then breaks a data-doc **on disk without reloading it** — the state a
//! `git checkout` or a stray edit leaves behind — so the rebuild's own doc walk is what fails. The
//! failure used to surface after the new world was already swapped in.

use ludion_core::ids::EntityId;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{DocStore, Engine, EngineBuilder, Profile, SceneSwitch};
use ludion_protocol::command::{EngineRestoreParams, SceneLoadParams, SimStepParams};
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use ludion_protocol::{Command, ResponseBody};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The `weapon` doc: one tunable number.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct Weapon {
    damage: f32,
}

/// The doc's sim-visible surface, carried across a switch.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct DamageReadout {
    value: f32,
}

/// A tiny player profile, so a failed switch can be shown to leave `Profile<P>` installed.
#[derive(Serialize, Deserialize, JsonSchema, Default, Clone, Debug, PartialEq)]
struct Save {
    unlocked: bool,
}

const KEY: &str = "weapons/crossbow.yaml";
const DOC_V1: &str = "format: 1\nkind: weapon\ndamage: 12.0\n";
/// A schema-invalid edit (`damage` is not a number) → `L1120`.
const DOC_BAD: &str = "format: 1\nkind: weapon\ndamage: fast\n";

/// District A: the start. One named entity carrying the readout.
const HUB: &str = "\
format: 1
seed: 5
name: hub
entities:
  - name: player
    components:
      DamageReadout:
        value: 0.0
";

/// District B: seed 77, the anchor a switch drops the player at, and one native prop.
const CELLAR: &str = "\
format: 1
seed: 77
name: cellar
entities:
  - name: gate
    components:
      Transform:
        translation:
          - 4.0
          - 0.0
          - 0.0
  - name: beacon
    components:
      Transform:
        translation:
          - 1.0
          - 0.0
          - 0.0
";

/// Copies the current `weapon` damage into every `DamageReadout` each tick.
fn apply_weapon(world: &mut World, _ctx: &SystemCtx) {
    let damage = world
        .resource::<DocStore<Weapon>>()
        .and_then(|store| store.get(KEY))
        .map(|weapon| weapon.damage);
    if let Some(damage) = damage {
        for (readout,) in world.query::<(&mut DamageReadout,)>() {
            readout.value = damage;
        }
    }
}

/// Requests a switch to the cellar, carrying the readout entity, on frames 2 and 6 — the second
/// attempt is the one a repaired doc lets through.
fn switch_to_cellar(world: &mut World, ctx: &SystemCtx) {
    if !matches!(ctx.frame.as_u64(), 2 | 6) {
        return;
    }
    let ids: Vec<EntityId> = world.entities().collect();
    let Some(player) = ids
        .into_iter()
        .find(|&id| matches!(world.get::<DamageReadout>(id), Ok(Some(_))))
    else {
        return;
    };
    if let Some(switch) = world.resource_mut::<SceneSwitch>() {
        switch.request("cellar.yaml".to_string(), "gate".to_string(), vec![player]);
    }
}

fn game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<DamageReadout>("DamageReadout"))
        .system("apply_weapon", apply_weapon)
        .system("switch_to_cellar", switch_to_cellar)
        .doc_kind::<Weapon>("weapon")
        .profile::<Save>()
        .project_root(root)
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-rebuild-tx-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("weapons")).unwrap();
    std::fs::write(dir.join(KEY), DOC_V1).unwrap();
    std::fs::write(dir.join("cellar.yaml"), CELLAR).unwrap();
    dir
}

fn write_doc(root: &Path, text: &str) {
    std::fs::write(root.join(KEY), text).unwrap();
}

/// A booted hub with its recording armed to an in-memory sink.
fn booted(name: &str) -> (Engine, PathBuf, SharedSink) {
    let dir = fresh_dir(name);
    let mut engine = game(&dir).build().expect("boots");
    engine.load_inline_scene(HUB).unwrap();
    let sink = SharedSink::default();
    engine.set_recording(sink.clone()).unwrap();
    (engine, dir, sink)
}

#[derive(Clone, Default)]
struct SharedSink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl SharedSink {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().clone()
    }
}

impl std::io::Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn has_entity(engine: &Engine, name: &str) -> bool {
    engine.world().entities().any(|id| {
        engine
            .world()
            .get::<ludion_engine::Name>(id)
            .ok()
            .flatten()
            .is_some_and(|n| n.0 == name)
    })
}

fn damage(engine: &Engine) -> f32 {
    engine
        .world()
        .resource::<DocStore<Weapon>>()
        .expect("the weapon store is installed")
        .get(KEY)
        .unwrap()
        .damage
}

fn scene_switches(bytes: &[u8]) -> usize {
    ReplayReader::new(bytes)
        .unwrap()
        .records()
        .unwrap()
        .iter()
        .filter(|record| matches!(record, ReplayRecord::SceneSwitch { .. }))
        .count()
}

fn step(engine: &mut Engine, ticks: u64) {
    engine
        .execute(Command::SimStep(SimStepParams { ticks, hash: false }))
        .unwrap();
}

#[test]
fn a_switch_onto_a_broken_doc_is_rejected_and_the_old_district_stays_live() {
    let (mut engine, dir, sink) = booted("switch");
    engine.advance_live(); // frame 0
    engine.advance_live(); // frame 1
    let hub_text = engine.scene_text().unwrap();

    // The doc goes bad on disk with no reload; frame 2 requests the switch.
    write_doc(&dir, DOC_BAD);
    engine.advance_live(); // frame 2 -> the switch is attempted and rejected
    engine.advance_live();
    engine.advance_live();

    let error = engine
        .last_transition_error()
        .expect("the rejected switch is reported");
    assert!(
        error.message.contains("L1120") || error.code.to_string().contains("1120"),
        "the doc failure is the reported cause: {error:?}"
    );
    assert!(engine.frame().as_u64() >= 5, "the sim kept advancing");
    assert!(
        has_entity(&engine, "player") && !has_entity(&engine, "beacon"),
        "still in the hub"
    );
    assert_eq!(engine.scene_text().unwrap(), hub_text, "same scene");
    assert_eq!(engine.seed(), 5, "the hub's seed");
    assert_eq!(damage(&engine), 12.0, "the store still holds v1");
    assert!(
        engine.world().resource::<Profile<Save>>().is_some(),
        "the profile is still installed"
    );

    engine.stop_recording().unwrap();
    assert_eq!(scene_switches(&sink.bytes()), 0, "no SceneSwitch record");
}

#[test]
fn a_scene_load_onto_a_broken_doc_leaves_the_engine_untouched() {
    let (mut engine, dir, _sink) = booted("load");
    step(&mut engine, 1);
    let hash = engine.world_hash().unwrap();
    let seed = engine.seed();
    let frame = engine.frame();
    let scene = engine.scene_text().unwrap();

    write_doc(&dir, DOC_BAD);
    let other = HUB.replace("seed: 5", "seed: 91").replace("hub", "annex");
    assert!(
        engine.load_inline_scene(&other).is_err(),
        "the doc walk fails"
    );

    assert_eq!(engine.world_hash().unwrap(), hash);
    assert_eq!(engine.seed(), seed);
    assert_eq!(engine.frame(), frame);
    assert_eq!(engine.scene_text().unwrap(), scene);
    assert_eq!(damage(&engine), 12.0);
}

#[test]
fn a_restore_onto_a_broken_doc_leaves_the_engine_untouched() {
    let (mut engine, dir, _sink) = booted("restore");
    step(&mut engine, 1);
    let snapshot = match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    step(&mut engine, 2);
    let hash = engine.world_hash().unwrap();
    let frame = engine.frame();

    write_doc(&dir, DOC_BAD);
    let result = engine.execute(Command::EngineRestore(EngineRestoreParams {
        snapshot: Value::clone(&snapshot),
    }));
    assert!(result.is_err(), "the doc walk fails the restore");

    assert_eq!(engine.world_hash().unwrap(), hash);
    assert_eq!(engine.frame(), frame);
    assert_eq!(damage(&engine), 12.0);
}

#[test]
fn a_repaired_doc_lets_the_next_switch_through() {
    let (mut engine, dir, sink) = booted("repaired");
    engine.advance_live(); // frame 0
    engine.advance_live(); // frame 1
    write_doc(&dir, DOC_BAD);
    engine.advance_live(); // frame 2: rejected
    assert!(engine.last_transition_error().is_some());

    write_doc(&dir, DOC_V1);
    for _ in 0..5 {
        engine.advance_live(); // frame 6 requests again and lands
    }

    assert!(
        engine.last_transition_error().is_none(),
        "a clean switch clears the report: {:?}",
        engine.last_transition_error()
    );
    assert!(
        has_entity(&engine, "beacon") && has_entity(&engine, "player"),
        "in the cellar, the player carried over"
    );
    assert_eq!(damage(&engine), 12.0);

    engine.stop_recording().unwrap();
    assert_eq!(scene_switches(&sink.bytes()), 1, "exactly the one switch");
}

/// The frames of the keyframes a recording holds, in file order.
fn keyframe_frames(bytes: &[u8]) -> Vec<u64> {
    ReplayReader::new(bytes)
        .unwrap()
        .records()
        .unwrap()
        .iter()
        .filter_map(|record| match record {
            ReplayRecord::Keyframe { frame, .. } => Some(frame.as_u64()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_load_that_rewinds_the_frame_keeps_the_keyframe_cadence() {
    let (mut engine, _dir, sink) = booted("keyframes");
    step(&mut engine, 301); // a keyframe at frame 301, after the opening one at 0
    engine
        .execute(Command::SceneLoad(SceneLoadParams {
            path: "cellar.yaml".to_string(),
        }))
        .unwrap();
    assert_eq!(engine.frame().as_u64(), 0, "the load rewinds the frame");
    step(&mut engine, 300);
    engine.stop_recording().unwrap();

    assert_eq!(
        keyframe_frames(&sink.bytes()),
        vec![0, 301, 300],
        "the next keyframe is one interval after the rebuilt frame, not after the old boundary"
    );
}
