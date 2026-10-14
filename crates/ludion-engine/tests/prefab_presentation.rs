//! Static set dressing as presentation parts of a prefab (`ludion docs presentation`): a prefab whose
//! root carries the `Collider` stays sim and its visual parts are `presentation: true` children with a
//! sim `Parent`. Every path is checked against the same scene with the presentation parts removed —
//! load, save, `prefab.spawn`, `prefab.overrides`, snapshot/restore and replay.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ludion_core::ids::EntityId;
use ludion_core::time::FrameIndex;
use ludion_ecs::{Parent, SystemCtx, World, is_presentation};
use ludion_engine::hierarchy::world_transform;
use ludion_engine::{
    Engine, EngineBuilder, Name, PrefabInstance, builtin_registry, replay_hashes_with, spawn_prefab,
};
use ludion_protocol::command::{
    EngineRestoreParams, Placement, PrefabOverridesParams, PrefabSpawnParams, SimStepParams,
};
use ludion_protocol::replay::ReplayReader;
use ludion_protocol::{Command, ResponseBody};
use ludion_sim::WorldHash;

const LAMP: &str = "\
format: 1
kind: prefab
name: lamp
entities:
  - name: post
    components:
      Transform:
        translation:
          - 0.0
          - 1.0
          - 0.0
      Mesh:
        shape: cube
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 0.5
              - 0.5
  - name: bulb
    presentation: true
    components:
      Transform:
        translation:
          - 0.0
          - 0.5
          - 0.0
      Mesh:
        shape: cube
      Parent:
        index: 0
        generation: 0
  - name: glow
    presentation: true
    components:
      Transform: {}
      Mesh:
        shape: cube
      Parent:
        index: 0
        generation: 0
";

/// The same prefab with its presentation parts removed.
const LAMP_SIM: &str = "\
format: 1
kind: prefab
name: lamp
entities:
  - name: post
    components:
      Transform:
        translation:
          - 0.0
          - 1.0
          - 0.0
      Mesh:
        shape: cube
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 0.5
              - 0.5
";

const BENCH: &str = "\
format: 1
kind: prefab
name: bench
entities:
  - name: seat
    presentation: true
    components:
      Transform: {}
      Mesh:
        shape: cube
";

const SCENE: &str = "\
format: 1
name: s
seed: 3
entities:
  - name: ground
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
        rotation:
          - 0.0
          - 0.0
          - 0.0
        scale:
          - 1.0
          - 1.0
          - 1.0
  - name: lamp-1
    prefab: lamp.prefab.yaml
    transform:
      translation:
        - 4.0
        - 0.0
        - 0.0
    overrides:
      bulb:
        Mesh:
          color:
            - 1.0
            - 0.0
            - 0.0
            - 1.0
  - name: lamp-2
    prefab: lamp.prefab.yaml
    transform:
      translation:
        - -4.0
        - 0.0
        - 0.0
  - name: bench-1
    prefab: bench.prefab.yaml
";

/// The scene a sim-only project loads: no bench, no overrides on a part that is not there.
const SCENE_SIM: &str = "\
format: 1
name: s
seed: 3
entities:
  - name: ground
    components:
      Transform: {}
  - name: lamp-1
    prefab: lamp.prefab.yaml
    transform:
      translation:
        - 4.0
        - 0.0
        - 0.0
  - name: lamp-2
    prefab: lamp.prefab.yaml
    transform:
      translation:
        - -4.0
        - 0.0
        - 0.0
";

const EMPTY: &str =
    "format: 1\nseed: 3\nentities:\n  - name: ground\n    components:\n      Transform: {}\n";

type Trace = Vec<(FrameIndex, WorldHash)>;

fn root(tag: &str, lamp: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-prefab-presentation-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lamp.prefab.yaml"), lamp).unwrap();
    std::fs::write(dir.join("bench.prefab.yaml"), BENCH).unwrap();
    dir
}

fn game(root: &Path) -> EngineBuilder {
    Engine::builder().project_root(root)
}

fn loaded(root: &Path, scene: &str) -> Engine {
    let mut engine = game(root).build().unwrap();
    engine.load_inline_scene(scene).expect("the scene loads");
    engine
}

fn index(engine: &mut Engine) -> BTreeMap<String, EntityId> {
    engine
        .world_mut()
        .query_ids::<(&Name,)>()
        .map(|(id, (name,))| (name.0.clone(), id))
        .collect()
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

fn start(engine: &mut Engine) -> Trace {
    vec![(engine.frame(), engine.world_hash().unwrap())]
}

fn spawn(engine: &mut Engine, path: &str, name: &str) -> Vec<EntityId> {
    let command = Command::PrefabSpawn(PrefabSpawnParams {
        path: path.to_owned(),
        transform: Some(Placement {
            translation: [0.0, 0.0, 9.0],
            rotation: [0.0; 3],
            scale: [1.0; 3],
        }),
        name: Some(name.to_owned()),
    });
    match engine.execute(command).unwrap().body {
        ResponseBody::PrefabSpawned { entities } => entities,
        other => panic!("unexpected reply {other:?}"),
    }
}

#[test]
fn scene_load_keeps_sim_ids_and_hash_and_places_presentation_parts() {
    let full_root = root("load-full", LAMP);
    let sim_root = root("load-sim", LAMP_SIM);
    let mut full = loaded(&full_root, SCENE);
    let mut sim = loaded(&sim_root, SCENE_SIM);

    let (full_ids, sim_ids) = (index(&mut full), index(&mut sim));
    for (name, id) in &sim_ids {
        assert_eq!(full_ids.get(name), Some(id), "sim id of `{name}` moved");
    }
    for name in [
        "lamp-1/bulb",
        "lamp-1/glow",
        "lamp-2/bulb",
        "lamp-2/glow",
        "bench-1/seat",
    ] {
        assert!(is_presentation(full_ids[name]), "{name}");
        assert!(!sim_ids.contains_key(name));
    }

    let (mut a, mut b) = (start(&mut full), start(&mut sim));
    assert_eq!(a, b);
    step(&mut full, 5, &mut a);
    step(&mut sim, 5, &mut b);
    assert_eq!(a, b, "presentation parts moved the hash trace");

    let world = full.world();
    let bulb = world_transform(world, full_ids["lamp-1/bulb"]).unwrap();
    assert_eq!(bulb.trs.translation, [4.0, 1.5, 0.0]);
    let post = full_ids["lamp-1/post"];
    for part in ["lamp-1/post", "lamp-1/bulb", "lamp-1/glow"] {
        let instance = world
            .get::<PrefabInstance>(full_ids[part])
            .unwrap()
            .unwrap();
        assert_eq!(instance.root, post, "{part}");
    }
    assert_eq!(
        world
            .get::<Parent>(full_ids["lamp-2/glow"])
            .unwrap()
            .unwrap()
            .0,
        full_ids["lamp-2/post"]
    );
    let _ = std::fs::remove_dir_all(&full_root);
    let _ = std::fs::remove_dir_all(&sim_root);
}

fn canonical(source: &str) -> String {
    let scene = ludion_scene::parse(source).expect("the source parses");
    ludion_scene::emit(&scene, &builtin_registry())
}

#[test]
fn save_round_trips_instances_and_the_override_byte_for_byte() {
    let root = root("save", LAMP);
    let saved = loaded(&root, SCENE).scene_text().expect("the world saves");
    assert_eq!(saved, canonical(SCENE));
    let mut again = loaded(&root, &saved);
    assert_eq!(again.scene_text().expect("it saves again"), saved);
    let mut original = loaded(&root, SCENE);
    assert_eq!(
        again.world_hash().unwrap(),
        original.world_hash().unwrap(),
        "a reload moved the sim hash"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn prefab_spawn_from_the_host_hashes_like_the_sim_only_spawn() {
    let full_root = root("spawn-full", LAMP);
    let sim_root = root("spawn-sim", LAMP_SIM);
    let mut full = loaded(&full_root, EMPTY);
    let mut sim = loaded(&sim_root, EMPTY);

    let ids = spawn(&mut full, "lamp.prefab.yaml", "late");
    let sim_ids = spawn(&mut sim, "lamp.prefab.yaml", "late");
    assert_eq!((ids.len(), sim_ids.len()), (3, 1));
    assert_eq!(ids.iter().filter(|id| is_presentation(**id)).count(), 2);
    assert_eq!(ids[0], sim_ids[0]);
    assert_eq!(full.world_hash().unwrap(), sim.world_hash().unwrap());
    let _ = std::fs::remove_dir_all(&full_root);
    let _ = std::fs::remove_dir_all(&sim_root);
}

#[test]
fn prefab_overrides_reports_the_presentation_part_override() {
    let root = root("overrides", LAMP);
    let mut engine = loaded(&root, SCENE);
    let reply = engine
        .execute(Command::PrefabOverrides(PrefabOverridesParams {
            instances: None,
        }))
        .unwrap();
    let ResponseBody::PrefabOverrides { instances } = reply.body else {
        panic!("unexpected reply");
    };
    let lamp = instances
        .iter()
        .find(|i| i.name.as_deref() == Some("lamp-1"))
        .expect("lamp-1");
    assert!(lamp.diagnostic.is_none(), "{:?}", lamp.diagnostic);
    assert_eq!(
        lamp.overrides.get("bulb"),
        Some(&vec!["/Mesh/color".to_owned()])
    );
    let plain = instances
        .iter()
        .find(|i| i.name.as_deref() == Some("lamp-2"))
        .unwrap();
    assert!(plain.overrides.is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn snapshot_restore_brings_the_presentation_parts_back() {
    let root = root("snapshot", LAMP);
    let mut engine = loaded(&root, SCENE);
    let snapshot = match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("unexpected reply {other:?}"),
    };
    let mut first = start(&mut engine);
    step(&mut engine, 3, &mut first);

    engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    let ids = index(&mut engine);
    for lamp in ["lamp-1", "lamp-2"] {
        let post = ids[&format!("{lamp}/post")];
        for part in ["bulb", "glow"] {
            let id = ids[&format!("{lamp}/{part}")];
            assert!(is_presentation(id));
            assert_eq!(engine.world().get::<Parent>(id).unwrap().unwrap().0, post);
        }
    }
    let mut second = start(&mut engine);
    step(&mut engine, 3, &mut second);
    assert_eq!(first, second, "the restored world diverged");
    let _ = std::fs::remove_dir_all(&root);
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

#[test]
fn a_recorded_mixed_spawn_replays_to_the_same_hash_trace() {
    let root = root("replay", LAMP);
    let sink = SharedSink::default();
    let mut engine = loaded(&root, SCENE);
    let mut trace = start(&mut engine);
    engine.set_recording(sink.clone()).unwrap();
    step(&mut engine, 2, &mut trace);
    spawn(&mut engine, "lamp.prefab.yaml", "late");
    *trace.last_mut().unwrap() = (engine.frame(), engine.world_hash().unwrap());
    step(&mut engine, 3, &mut trace);
    engine.stop_recording().unwrap();
    let bytes = sink.0.lock().unwrap().clone();
    ReplayReader::new(bytes.as_slice()).unwrap();

    let replayed = replay_hashes_with(game(&root), &bytes, &root).unwrap();
    assert_eq!(replayed, trace);
    let _ = std::fs::remove_dir_all(&root);
}

#[derive(Default)]
struct Outcome(Option<Result<usize, Vec<String>>>);

fn spawner(world: &mut World, _ctx: &SystemCtx) {
    if world.resource::<Outcome>().is_some() {
        return;
    }
    let result = spawn_prefab(world, "lamp.prefab.yaml", None, Some("sys"))
        .map(|ids| ids.len())
        .map_err(|found| found.iter().map(|d| d.code.to_string()).collect());
    world.insert_resource(Outcome(Some(result)));
}

#[test]
fn a_sim_system_cannot_spawn_a_prefab_with_presentation_parts() {
    let root = root("system", LAMP);
    let mut engine = game(&root).system("spawner", spawner).build().unwrap();
    engine.load_inline_scene(EMPTY).unwrap();
    let before = index(&mut engine).len();
    let mut trace = Vec::new();
    step(&mut engine, 1, &mut trace);
    let outcome = engine.world().resource::<Outcome>().unwrap().0.clone();
    assert_eq!(
        outcome,
        Some(Err(vec!["L0028".to_owned(), "L0028".to_owned()]))
    );
    assert_eq!(
        index(&mut engine).len(),
        before,
        "a refused spawn left entities behind"
    );
    let _ = std::fs::remove_dir_all(&root);
}
