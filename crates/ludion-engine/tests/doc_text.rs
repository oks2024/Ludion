//! `EngineBuilder::doc_text`: a data-doc handed in as text replaces (or adds to) the content
//! root's file docs, takes the same checks, lives for the engine, and travels in a recording.

use ludion_core::time::FrameIndex;
use ludion_engine::{DocStore, Engine, EngineBuilder, replay_hashes_with};
use ludion_protocol::command::{AssetReloadParams, EngineRestoreParams, SimStepParams};
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use ludion_protocol::{Command, ResponseBody};
use ludion_sim::WorldHash;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct Weapon {
    damage: f32,
}

/// The sim-visible surface of the `weapon` doc, so a store change moves the `WorldHash`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct DamageReadout {
    value: f32,
}

const KEY: &str = "weapons/crossbow.yaml";
const FILE_DOC: &str = "format: 1\nkind: weapon\ndamage: 12.0\n";
const TEXT_DOC: &str = "format: 1\nkind: weapon\ndamage: 99.0\n";
const SCENE: &str =
    "format: 1\nseed: 1\nentities:\n  - components:\n      DamageReadout:\n        value: 0.0\n";

fn apply_weapon(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
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

fn game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<DamageReadout>("DamageReadout"))
        .system("apply_weapon", apply_weapon)
        .doc_kind::<Weapon>("weapon")
        .project_root(root)
}

fn singleton_game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .doc_singleton::<Weapon>("weapon")
        .project_root(root)
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-doctext-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn build_err(builder: EngineBuilder) -> String {
    match builder.build() {
        Ok(_) => panic!("the build should have failed"),
        Err(err) => format!("{err:?}"),
    }
}

fn store(engine: &Engine) -> &DocStore<Weapon> {
    engine.world().resource::<DocStore<Weapon>>().unwrap()
}

fn step(engine: &mut Engine) -> (FrameIndex, WorldHash) {
    engine
        .execute(Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .unwrap();
    (engine.frame(), engine.world_hash().unwrap())
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

#[test]
fn doc_text_overrides_a_file_doc_and_survives_scene_load() {
    let dir = fresh_dir("override");
    write(&dir, KEY, FILE_DOC);
    let mut engine = game(&dir).doc_text(KEY, TEXT_DOC).build().unwrap();
    assert_eq!(store(&engine).get(KEY).unwrap().damage, 99.0);
    assert_eq!(store(&engine).len(), 1);

    engine.load_inline_scene(SCENE).unwrap();
    assert_eq!(store(&engine).get(KEY).unwrap().damage, 99.0);

    let ResponseBody::Snapshot { snapshot } = engine.execute(Command::EngineSnapshot).unwrap().body
    else {
        panic!("expected a snapshot");
    };
    engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    assert_eq!(store(&engine).get(KEY).unwrap().damage, 99.0);

    let added = game(&dir)
        .doc_text(KEY, TEXT_DOC)
        .doc_text("weapons/axe.yaml", TEXT_DOC)
        .build()
        .unwrap();
    assert_eq!(store(&added).len(), 2);
}

#[test]
fn bad_doc_text_fails_the_build_with_l1120() {
    let dir = fresh_dir("bad");
    let err = build_err(game(&dir).doc_text(KEY, "format: 1\nkind: weapon\ndamage: fast\n"));
    assert!(err.contains("L1120"), "{err}");
    assert!(err.contains(KEY), "{err}");
}

#[test]
fn a_doc_text_key_must_be_a_relative_yaml_path() {
    let dir = fresh_dir("keys");
    for key in ["../x.yaml", "x.txt"] {
        let err = build_err(game(&dir).doc_text(key, TEXT_DOC));
        assert!(err.contains("L1119"), "{key}: {err}");
    }
    let err = build_err(game(&dir).doc_text(KEY, "format: 1\ndamage: 1.0\n"));
    assert!(err.contains("L1119"), "{err}");
}

#[test]
fn a_second_key_for_a_singleton_kind_is_l1315() {
    let dir = fresh_dir("singleton");
    write(&dir, "a.yaml", FILE_DOC);
    let err = build_err(singleton_game(&dir).doc_text("b.yaml", TEXT_DOC));
    assert!(err.contains("L1315"), "{err}");
    assert!(err.contains("a.yaml") && err.contains("b.yaml"), "{err}");

    let engine = singleton_game(&dir)
        .doc_text("a.yaml", TEXT_DOC)
        .build()
        .unwrap();
    assert_eq!(store(&engine).single().unwrap().damage, 99.0);
}

#[test]
fn a_recording_with_doc_text_replays_to_the_same_hashes() {
    let dir = fresh_dir("record");
    write(&dir, KEY, FILE_DOC);
    let sink = SharedSink::default();
    let mut engine = game(&dir).doc_text(KEY, TEXT_DOC).build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine.set_recording(sink.clone()).unwrap();
    let mut original = vec![(engine.frame(), engine.world_hash().unwrap())];
    for _ in 0..3 {
        original.push(step(&mut engine));
    }
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();

    let records = ReplayReader::new(&bytes).unwrap().records().unwrap();
    assert!(
        matches!(&records[0], ReplayRecord::DocReload { path, .. } if path == KEY),
        "the first record is the inline doc"
    );
    assert!(matches!(records[1], ReplayRecord::Keyframe { .. }));

    let with_text = replay_hashes_with(game(&dir).doc_text(KEY, TEXT_DOC), &bytes, &dir).unwrap();
    assert_eq!(original, with_text);
    let plain = replay_hashes_with(game(&dir), &bytes, &dir).unwrap();
    assert_eq!(original, plain);
}

#[test]
fn asset_reload_leaves_a_doc_text_override_in_place() {
    let dir = fresh_dir("reload");
    write(&dir, KEY, FILE_DOC);
    let mut engine = game(&dir).doc_text(KEY, TEXT_DOC).build().unwrap();
    write(&dir, KEY, "format: 1\nkind: weapon\ndamage: 5.0\n");
    engine
        .execute(Command::AssetReload(AssetReloadParams {
            path: Some(KEY.to_string()),
        }))
        .unwrap();
    assert_eq!(store(&engine).get(KEY).unwrap().damage, 99.0);
    assert_eq!(store(&engine).generation(), 0);
}
