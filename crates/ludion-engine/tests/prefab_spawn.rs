//! Runtime proof for `spawn_prefab`: a system spawns a prefab twice at two placements and every part
//! lands on its own copy's ids, the spawn is deterministic, the provenance component snapshots, and
//! `asset.reload` refreshes a held prefab while refusing a bad edit. Everything goes through the
//! public surface over a prefab file written to a temp root.

use std::path::{Path, PathBuf};

use ludion_core::ids::EntityId;
use ludion_ecs::{Parent, SystemCtx, World};
use ludion_engine::{Engine, EngineBuilder, Name, PrefabInstance, Transform, spawn_prefab};
use ludion_protocol::command::{AssetReloadParams, EngineRestoreParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};
use ludion_spatial::Joint;

const PREFAB: &str = "\
format: 1
kind: prefab
name: goblin
entities:
  - name: pelvis
    components:
      Transform:
        translation:
          - 1.0
          - 0.0
          - 0.0
  - name: thigh
    components:
      Transform:
        translation:
          - 0.0
          - 1.0
          - 0.0
      Parent:
        index: 0
        generation: 0
      Joint:
        a:
          index: 0
          generation: 0
        b:
          index: 1
          generation: 0
        kind:
          fixed: {}
";

const PATH: &str = "goblin.prefab.yaml";

const EMPTY_SCENE: &str = "\
format: 1
entities:
  - name: camera
    components:
      Transform: {}
";

/// The ids each spawn returned, in spawn order.
#[derive(Default)]
struct Spawned(Vec<Vec<EntityId>>);

fn at(x: f32, z: f32) -> Transform {
    Transform {
        translation: [x, 0.0, z],
        ..Transform::default()
    }
}

/// At frame 1: a named copy at `x = 10`, an unnamed one at `z = 5`.
fn spawner(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 1 {
        return;
    }
    let first =
        spawn_prefab(world, PATH, Some(at(10.0, 0.0)), Some("goblin-a"), false).expect("spawns");
    let second = spawn_prefab(world, PATH, Some(at(0.0, 5.0)), None, false).expect("spawns");
    world.insert_resource(Spawned(vec![first, second]));
}

fn fixture_root(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-prefab-spawn-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temp content root");
    std::fs::write(dir.join(PATH), PREFAB).unwrap();
    dir
}

fn game(root: &Path) -> EngineBuilder {
    Engine::builder().project_root(root)
}

fn engine(root: &Path, spawning: bool) -> Engine {
    let builder = game(root);
    let mut engine = if spawning {
        builder.system("spawner", spawner)
    } else {
        builder
    }
    .build()
    .expect("the game builds");
    engine
        .load_inline_scene(EMPTY_SCENE)
        .expect("the scene loads");
    engine
}

fn advance(engine: &mut Engine, ticks: u64) {
    engine
        .execute(Command::SimStep(SimStepParams { ticks, hash: false }))
        .expect("the sim steps");
}

fn reload(engine: &mut Engine, path: &str) -> Result<Vec<String>, String> {
    match engine.execute(Command::AssetReload(AssetReloadParams {
        path: Some(path.to_owned()),
    })) {
        Ok(response) => match response.body {
            ResponseBody::AssetsReloaded { paths } => Ok(paths),
            other => panic!("expected AssetsReloaded, got {other:?}"),
        },
        Err(error) => Err(error.code.as_str().to_owned()),
    }
}

fn name(world: &World, id: EntityId) -> String {
    world.get::<Name>(id).unwrap().unwrap().0.clone()
}

#[test]
fn a_system_spawns_two_copies_that_each_own_their_references() {
    let root = fixture_root("copies");
    let mut engine = engine(&root, true);
    advance(&mut engine, 2);
    let world = engine.world();
    let spawned = &world.resource::<Spawned>().expect("the spawner ran").0;
    let (first, second) = (&spawned[0], &spawned[1]);
    assert_eq!((first.len(), second.len()), (2, 2));
    assert_ne!(first[0], second[0]);

    assert_eq!(name(world, first[0]), "goblin-a/pelvis");
    assert_eq!(name(world, first[1]), "goblin-a/thigh");
    assert_eq!(name(world, second[0]), "pelvis");
    assert_eq!(name(world, second[1]), "thigh");

    for (ids, placement) in [(first, [10.0, 0.0, 0.0]), (second, [0.0, 0.0, 5.0])] {
        assert_eq!(world.get::<Parent>(ids[1]).unwrap().unwrap().0, ids[0]);
        let joint = world.get::<Joint>(ids[1]).unwrap().unwrap();
        assert_eq!((joint.a, joint.b), (ids[0], ids[1]));

        let root_pose = world.get::<Transform>(ids[0]).unwrap().unwrap();
        let want = [placement[0] + 1.0, placement[1], placement[2]];
        for (got, want) in root_pose.translation.iter().zip(want) {
            assert!((got - want).abs() < 1e-6, "{root_pose:?} vs {want:?}");
        }
        let child = world.get::<Transform>(ids[1]).unwrap().unwrap();
        assert_eq!(child.translation, [0.0, 1.0, 0.0]);

        for (id, part) in ids.iter().zip(["pelvis", "thigh"]) {
            assert_eq!(
                *world.get::<PrefabInstance>(*id).unwrap().unwrap(),
                PrefabInstance {
                    source: PATH.to_owned(),
                    part: part.to_owned(),
                    root: ids[0],
                }
            );
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_spawn_moves_the_hash_and_a_fresh_engine_reproduces_it() {
    let root = fixture_root("hash");
    let mut left = engine(&root, false);
    let before = left.world_hash().unwrap();
    spawn_prefab(left.world_mut(), PATH, Some(at(2.0, 0.0)), Some("g"), false).expect("spawns");
    let after = left.world_hash().unwrap();
    assert_ne!(before, after, "the spawned parts are hashed");

    let mut right = engine(&root, false);
    spawn_prefab(
        right.world_mut(),
        PATH,
        Some(at(2.0, 0.0)),
        Some("g"),
        false,
    )
    .expect("spawns");
    assert_eq!(right.world_hash().unwrap(), after);

    let (mut a, mut b) = (engine(&root, true), engine(&root, true));
    advance(&mut a, 4);
    advance(&mut b, 4);
    assert_eq!(a.world_hash().unwrap(), b.world_hash().unwrap());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn prefab_instance_survives_snapshot_and_restore() {
    let root = fixture_root("snapshot");
    let mut source = engine(&root, true);
    advance(&mut source, 2);
    let hash = source.world_hash().unwrap();
    let snapshot = match source.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };

    let mut restored = game(&root)
        .system("spawner", spawner)
        .build()
        .expect("the game builds");
    restored
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .expect("the snapshot restores");
    assert_eq!(restored.world_hash().unwrap(), hash);

    let ids = &source.world().resource::<Spawned>().unwrap().0[0];
    let instance = restored
        .world()
        .get::<PrefabInstance>(ids[1])
        .unwrap()
        .unwrap();
    assert_eq!(instance.root, ids[0]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn asset_reload_refreshes_a_held_prefab_and_keeps_it_on_a_bad_edit() {
    let root = fixture_root("reload");
    let mut engine = engine(&root, false);

    // Nothing has loaded the prefab yet: the reload ignores it.
    assert_eq!(reload(&mut engine, PATH), Ok(Vec::new()));

    let first = spawn_prefab(engine.world_mut(), PATH, None, None, false).expect("spawns");
    assert_eq!(first.len(), 2);

    let one_part =
        "format: 1\nkind: prefab\nname: goblin\nentities:\n  - name: pelvis\n    components: {}\n";
    std::fs::write(root.join(PATH), one_part).unwrap();
    assert_eq!(reload(&mut engine, PATH), Ok(vec![PATH.to_owned()]));
    let second = spawn_prefab(engine.world_mut(), PATH, None, None, false).expect("spawns");
    assert_eq!(second.len(), 1, "the next spawn uses the new text");
    assert_eq!(
        engine.world().entity_count(),
        3 + 1,
        "the first instance is untouched, with the camera"
    );

    let broken = "format: 1\nkind: prefab\nname: goblin\nentities:\n  - name: eye\n    components:\n      Camera: {}\n";
    std::fs::write(root.join(PATH), broken).unwrap();
    assert_eq!(reload(&mut engine, PATH), Err("L1229".to_owned()));
    let third =
        spawn_prefab(engine.world_mut(), PATH, None, None, false).expect("the old entry holds");
    assert_eq!(third.len(), 1, "a bad edit keeps the old text");

    // An unrelated path and a missing file are no-ops.
    assert_eq!(reload(&mut engine, "other.prefab.yaml"), Ok(Vec::new()));
    let _ = std::fs::remove_dir_all(&root);
}

/// A lint in a prefab body (`L1209`, an atlas sprite crossfading) is reported by `check-scene` on
/// the expanded scene; it must not make the prefab refuse to spawn.
#[test]
fn a_prefab_with_an_atlas_crossfade_lint_still_spawns() {
    let root = fixture_root("crossfade");
    let sprite = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../ludion-cli/tests/fixtures/sprite/"
    );
    for file in ["ship.atlas.yaml", "ship.png"] {
        std::fs::copy(format!("{sprite}{file}"), root.join(file)).unwrap();
    }
    std::fs::write(
        root.join("fader.prefab.yaml"),
        "format: 1\nkind: prefab\nname: fader\nentities:\n  - name: bolt\n    components:\n      \
         Sprite:\n        atlas: ship.atlas.yaml\n        frame: bolt/idle\n      Animation:\n        \
         clip: run\n        from: walk\n        fade: 0.5\n",
    )
    .unwrap();
    let mut engine = engine(&root, false);
    let ids = spawn_prefab(engine.world_mut(), "fader.prefab.yaml", None, None, false)
        .expect("a lint must not refuse the spawn");
    assert_eq!(ids.len(), 1);
    assert!(reload(&mut engine, "fader.prefab.yaml").is_ok());
    let _ = std::fs::remove_dir_all(&root);
}
