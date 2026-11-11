//! `prefab.despawn` over the command layer: every part goes (including unparented second roots),
//! refusals change nothing, and the contrast with `entity.destroy` on a root.

use std::path::{Path, PathBuf};

use ludion_core::ids::EntityId;
use ludion_engine::Engine;
use ludion_protocol::command::{
    EntityDestroyParams, MapSpawnParams, PrefabDespawnParams, PrefabSpawnParams,
    QueryEntitiesParams,
};
use ludion_protocol::{Command, ResponseBody};

const PATH: &str = "pair.prefab.yaml";

/// Two unparented root parts (`a`, `b`) and a child of `a`.
const PAIR: &str = "\
format: 1
kind: prefab
name: pair
entities:
  - name: a
    components:
      Transform: {}
  - name: b
    components:
      Transform: {}
  - name: c
    components:
      Transform: {}
      Parent:
        index: 0
        generation: 0
";

const KIT: &str =
    "format: 1\nkind: map-kit\nname: k\ntiles:\n  \".\":\n    - name: floor\n      shape: cube\n";
const MAP: &str = "format: 1\nkind: map\nname: crypt\nkit: ossuary.kit.yaml\ncell_size: 2.0\norigin:\n  - 0.0\n  - 0.0\nrows:\n  - \"..\"\n";

const SCENE: &str =
    "format: 1\nseed: 3\nentities:\n  - name: camera\n    components:\n      Transform: {}\n";

fn root(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-despawn-wire-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("rooms")).unwrap();
    std::fs::write(dir.join(PATH), PAIR).unwrap();
    std::fs::write(dir.join("rooms/ossuary.kit.yaml"), KIT).unwrap();
    std::fs::write(dir.join("rooms/crypt.map.yaml"), MAP).unwrap();
    dir
}

fn engine(root: &Path) -> Engine {
    let mut engine = Engine::builder().project_root(root).build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine
}

fn spawn_pair(engine: &mut Engine) -> Vec<EntityId> {
    let spawn = Command::PrefabSpawn(PrefabSpawnParams {
        path: PATH.to_owned(),
        transform: None,
        name: None,
        presentation: false,
    });
    match engine.execute(spawn).unwrap().body {
        ResponseBody::PrefabSpawned { entities } => entities,
        other => panic!("unexpected reply {other:?}"),
    }
}

fn despawn(
    engine: &mut Engine,
    root: EntityId,
) -> Result<Vec<EntityId>, ludion_core::diagnostics::Diagnostic> {
    engine
        .execute(Command::PrefabDespawn(PrefabDespawnParams { root }))
        .map(|reply| match reply.body {
            ResponseBody::PrefabDespawned { entities } => entities,
            other => panic!("unexpected reply {other:?}"),
        })
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

#[test]
fn despawn_removes_every_part_including_a_second_unparented_root() {
    let root = root("all");
    let mut engine = engine(&root);
    let before = count(&mut engine);
    let ids = spawn_pair(&mut engine);
    assert_eq!(ids.len(), 3);

    let removed = despawn(&mut engine, ids[0]).unwrap();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(removed, sorted, "every part, sorted");
    for id in &ids {
        assert!(!engine.world().is_alive(*id));
    }
    assert_eq!(count(&mut engine), before);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn entity_destroy_on_the_root_leaks_the_unparented_second_root() {
    let root = root("leak");
    let mut engine = engine(&root);
    let ids = spawn_pair(&mut engine);
    engine
        .execute(Command::EntityDestroy(EntityDestroyParams {
            entity: ids[0],
        }))
        .unwrap();
    assert!(!engine.world().is_alive(ids[0]));
    assert!(
        !engine.world().is_alive(ids[2]),
        "the child goes with its parent"
    );
    assert!(engine.world().is_alive(ids[1]), "the second root survives");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn despawn_removes_a_map_room() {
    let root = root("map");
    let mut engine = engine(&root);
    let before = count(&mut engine);
    let spawn = Command::MapSpawn(MapSpawnParams {
        map: "rooms/crypt.map.yaml".into(),
        kit: None,
        seed: None,
        mirror: None,
        placement: None,
        name: None,
    });
    let ids = match engine.execute(spawn).unwrap().body {
        ResponseBody::PrefabSpawned { entities } => entities,
        other => panic!("unexpected reply {other:?}"),
    };
    let removed = despawn(&mut engine, ids[0]).unwrap();
    assert_eq!(removed.len(), ids.len());
    assert_eq!(count(&mut engine), before);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn refusals_change_nothing() {
    let root = root("refuse");
    let mut engine = engine(&root);
    let ids = spawn_pair(&mut engine);
    let camera = engine
        .world_mut()
        .query_ids::<(&ludion_engine::Name,)>()
        .find(|(_, (name,))| name.0 == "camera")
        .map(|(id, _)| id)
        .unwrap();
    let dead = {
        let again = spawn_pair(&mut engine);
        despawn(&mut engine, again[0]).unwrap();
        again[0]
    };
    let hash = engine.world_hash().unwrap();
    let before = count(&mut engine);
    for (label, target, code) in [
        ("a non-root part", ids[1], "L1235"),
        ("a plain entity", camera, "L1235"),
        ("a dead id", dead, "L0002"),
    ] {
        let error = despawn(&mut engine, target).expect_err(label);
        assert_eq!(error.code.to_string(), code, "{label}");
        assert_eq!(engine.world_hash().unwrap(), hash, "{label}");
        assert_eq!(count(&mut engine), before, "{label}");
    }
    let _ = std::fs::remove_dir_all(&root);
}
