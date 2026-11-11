//! `map.spawn` over the command layer: the reply shape against the in-system `spawn_map`, refusals
//! that change nothing, and the replay gate (map and kit texts precede the command).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ludion_core::ids::EntityId;
use ludion_core::time::FrameIndex;
use ludion_engine::{
    Engine, EngineBuilder, MapSpawn, Mirror, Name, PrefabInstance, Transform, replay_hashes_with,
    spawn_map,
};
use ludion_protocol::command::{
    MapMirror, MapSpawnParams, Placement, PrefabDespawnParams, QueryEntitiesParams, SimStepParams,
};
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use ludion_protocol::{Command, ResponseBody};
use ludion_sim::WorldHash;

const MAP: &str = "rooms/crypt.map.yaml";
const KIT: &str = "rooms/ossuary.kit.yaml";
const KIT_B: &str = "rooms/vault.kit.yaml";

const KIT_TEXT: &str = "format: 1\nkind: map-kit\nname: k\ntiles:\n  \".\":\n    - name: floor\n      shape: cube\n  \"#\":\n    - name: wall\n      shape: cube\n";
const KIT_B_TEXT: &str = "format: 1\nkind: map-kit\nname: k\ntiles:\n  \".\":\n    - name: tile\n      shape: cube\n  \"#\":\n    - name: pillar\n      shape: cube\n";
const MAP_TEXT: &str = "format: 1\nkind: map\nname: crypt\nkit: ossuary.kit.yaml\ncell_size: 2.0\norigin:\n  - 0.0\n  - 0.0\nrows:\n  - \".#\"\n";
const PREFAB_TEXT: &str = "format: 1\nkind: prefab\nname: p\nentities:\n  - name: a\n    components:\n      Transform: {}\n";

const SCENE: &str =
    "format: 1\nseed: 3\nentities:\n  - name: camera\n    components:\n      Transform: {}\n";

fn root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-map-wire-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("rooms")).unwrap();
    std::fs::write(dir.join(KIT), KIT_TEXT).unwrap();
    std::fs::write(dir.join(KIT_B), KIT_B_TEXT).unwrap();
    std::fs::write(dir.join(MAP), MAP_TEXT).unwrap();
    std::fs::write(dir.join("rooms/not-a-map.yaml"), PREFAB_TEXT).unwrap();
    dir
}

fn game(root: &Path) -> EngineBuilder {
    Engine::builder().project_root(root)
}

fn engine(root: &Path) -> Engine {
    let mut engine = game(root).build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine
}

#[derive(Clone, Default)]
struct SharedSink(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

type Trace = Vec<(FrameIndex, WorldHash)>;

fn params(map: &str) -> MapSpawnParams {
    MapSpawnParams {
        map: map.to_owned(),
        kit: None,
        seed: None,
        mirror: None,
        placement: None,
        name: None,
    }
}

fn map_spawn(engine: &mut Engine, params: MapSpawnParams) -> Vec<EntityId> {
    match engine.execute(Command::MapSpawn(params)).unwrap().body {
        ResponseBody::PrefabSpawned { entities } => entities,
        other => panic!("unexpected reply {other:?}"),
    }
}

fn step(engine: &mut Engine, ticks: u64, trace: &mut Trace) {
    for _ in 0..ticks {
        engine
            .execute(Command::SimStep(SimStepParams {
                ticks: 1,
                hash: false,
            }))
            .unwrap();
        trace.push((engine.frame(), engine.world_hash().unwrap()));
    }
}

fn restamp(engine: &mut Engine, trace: &mut Trace) {
    *trace.last_mut().unwrap() = (engine.frame(), engine.world_hash().unwrap());
}

fn count(engine: &mut Engine) -> usize {
    match engine
        .execute(Command::QueryEntities(QueryEntitiesParams::default()))
        .unwrap()
        .body
    {
        ResponseBody::QueryResult { entities } => entities.len(),
        other => panic!("unexpected reply {other:?}"),
    }
}

fn names(engine: &Engine, ids: &[EntityId]) -> Vec<String> {
    ids.iter()
        .map(|id| engine.world().get::<Name>(*id).unwrap().unwrap().0.clone())
        .collect()
}

fn translations(engine: &Engine, ids: &[EntityId]) -> Vec<[f32; 3]> {
    ids.iter()
        .map(|id| {
            engine
                .world()
                .get::<Transform>(*id)
                .unwrap()
                .unwrap()
                .translation
        })
        .collect()
}

#[test]
fn the_reply_lists_the_parts_with_the_root_first_and_applies_every_field() {
    let root = root("reply");
    let mut engine = engine(&root);
    let ids = map_spawn(
        &mut engine,
        MapSpawnParams {
            name: Some("r1".into()),
            placement: Some(Placement {
                translation: [10.0, 0.0, 0.0],
                rotation: [0.0; 3],
                scale: [1.0; 3],
            }),
            ..params(MAP)
        },
    );
    assert_eq!(ids.len(), 2, "one part per tile: {ids:?}");
    for id in &ids {
        let instance = engine.world().get::<PrefabInstance>(*id).unwrap().unwrap();
        assert_eq!(instance.root, ids[0]);
        assert_eq!(instance.source, MAP);
    }
    let names = names(&engine, &ids);
    assert!(
        names.iter().all(|name| name.starts_with("r1/")),
        "{names:?}"
    );
    assert!(
        translations(&engine, &ids).iter().any(|t| t[0] >= 10.0),
        "the placement is baked"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_mirror_matches_the_in_system_spawn_and_a_kit_override_applies() {
    let root = root("mirror");
    let mut wire = engine(&root);
    let wire_ids = map_spawn(
        &mut wire,
        MapSpawnParams {
            mirror: Some(MapMirror::X),
            seed: Some(7),
            ..params(MAP)
        },
    );
    let mut rust = engine(&root);
    let spawn = MapSpawn {
        mirror: Mirror::X,
        seed: Some(7),
        ..MapSpawn::default()
    };
    let rust_ids = spawn_map(rust.world_mut(), MAP, spawn).unwrap();
    assert_eq!(names(&wire, &wire_ids), names(&rust, &rust_ids));
    assert_eq!(
        translations(&wire, &wire_ids),
        translations(&rust, &rust_ids)
    );
    assert_eq!(wire.world_hash().unwrap(), rust.world_hash().unwrap());

    let mut plain = engine(&root);
    let plain_ids = map_spawn(&mut plain, params(MAP));
    assert_ne!(
        translations(&plain, &plain_ids),
        translations(&wire, &wire_ids),
        "the mirror moved the tiles"
    );

    let mut overridden = engine(&root);
    let ids = map_spawn(
        &mut overridden,
        MapSpawnParams {
            kit: Some(KIT_B.into()),
            ..params(MAP)
        },
    );
    let names = names(&overridden, &ids);
    assert!(
        names.iter().any(|name| name.contains("pillar")),
        "{names:?}"
    );
    assert!(!names.iter().any(|name| name.contains("wall")), "{names:?}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_refused_spawn_changes_nothing() {
    let root = root("refuse");
    let mut engine = engine(&root);
    let hash = engine.world_hash().unwrap();
    let before = count(&mut engine);
    for (label, spawn, code) in [
        ("absolute map", params("/abs/x.map.yaml"), "L3040"),
        ("escaping map", params("../x.map.yaml"), "L3040"),
        (
            "escaping kit",
            MapSpawnParams {
                kit: Some("../k.kit.yaml".into()),
                ..params(MAP)
            },
            "L3040",
        ),
        ("missing map", params("rooms/none.map.yaml"), "L1406"),
        ("not a map", params("rooms/not-a-map.yaml"), "L1406"),
    ] {
        let error = engine.execute(Command::MapSpawn(spawn)).expect_err(label);
        assert_eq!(error.code.to_string(), code, "{label}");
        assert_eq!(engine.world_hash().unwrap(), hash, "{label}");
        assert_eq!(count(&mut engine), before, "{label}");
    }

    let unknown = serde_json::json!({"method": "map.spawn", "params": {"map": MAP, "bogus": 1}});
    assert!(
        serde_json::from_value::<Command>(unknown).is_err(),
        "unknown params are an envelope error"
    );
    let _ = std::fs::remove_dir_all(&root);
}

fn record(root: &Path) -> (Trace, Vec<u8>) {
    let sink = SharedSink::default();
    let mut engine = engine(root);
    let mut trace = vec![(engine.frame(), engine.world_hash().unwrap())];
    engine.set_recording(sink.clone()).unwrap();
    step(&mut engine, 2, &mut trace);
    let first = map_spawn(&mut engine, params(MAP));
    restamp(&mut engine, &mut trace);
    step(&mut engine, 2, &mut trace);
    engine
        .execute(Command::PrefabDespawn(PrefabDespawnParams {
            root: first[0],
        }))
        .unwrap();
    restamp(&mut engine, &mut trace);
    step(&mut engine, 2, &mut trace);
    map_spawn(
        &mut engine,
        MapSpawnParams {
            seed: Some(99),
            kit: Some(KIT_B.into()),
            ..params(MAP)
        },
    );
    restamp(&mut engine, &mut trace);
    step(&mut engine, 2, &mut trace);
    engine.stop_recording().unwrap();
    let bytes = sink.0.lock().unwrap().clone();
    (trace, bytes)
}

#[test]
fn a_map_spawn_session_replays_from_deleted_map_and_kit_files() {
    let root = root("gate");
    let (live, bytes) = record(&root);

    let records = ReplayReader::new(&bytes[..]).unwrap().records().unwrap();
    let first_command = records
        .iter()
        .position(|record| {
            matches!(record, ReplayRecord::Command { command, .. }
                if matches!(command, Command::MapSpawn(_)))
        })
        .expect("the spawn is recorded");
    let text_before = |path: &str| {
        records[..first_command]
            .iter()
            .any(|record| matches!(record, ReplayRecord::PrefabText { path: p, .. } if p == path))
    };
    assert!(text_before(MAP), "map text precedes the first map.spawn");
    assert!(text_before(KIT), "kit text precedes the first map.spawn");

    let replay = |bytes: &[u8]| replay_hashes_with(game(&root), bytes, &root).unwrap();
    assert_eq!(replay(&bytes), live, "unchanged checkout");
    for file in [MAP, KIT, KIT_B] {
        std::fs::remove_file(root.join(file)).unwrap();
    }
    assert_eq!(replay(&bytes), live, "deleted map and kit files");
    let _ = std::fs::remove_dir_all(&root);
}
