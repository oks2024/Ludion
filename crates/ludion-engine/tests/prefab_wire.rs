//! `prefab.spawn` over the command layer: the reply shape, all-or-nothing refusals, and the replay
//! gate (the text is recorded before the command, so a replay survives a changed or deleted file).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ludion_core::ids::EntityId;
use ludion_core::time::FrameIndex;
use ludion_ecs::Parent;
use ludion_engine::{Engine, EngineBuilder, Name, PrefabInstance, replay_hashes_with};
use ludion_protocol::command::{
    EntityDestroyParams, Placement, PrefabSpawnParams, QueryEntitiesParams, SimStepParams,
};
use ludion_protocol::replay::{ReplayReader, ReplayRecord, ReplayWriter};
use ludion_protocol::{Command, ResponseBody};
use ludion_sim::WorldHash;
use ludion_spatial::Joint;

const PATH: &str = "crate.prefab.yaml";

const V1: &str = "\
format: 1
kind: prefab
name: crate
entities:
  - name: base
    components:
      Transform:
        translation:
          - 1.0
          - 0.0
          - 0.0
  - name: lid
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

/// A different valid prefab: another translation and a third part.
const V2: &str = "\
format: 1
kind: prefab
name: crate
entities:
  - name: base
    components:
      Transform:
        translation:
          - 5.0
          - 0.0
          - 0.0
  - name: lid
    components:
      Transform:
        translation:
          - 0.0
          - 2.0
          - 0.0
      Parent:
        index: 0
        generation: 0
  - name: latch
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 3.0
";

/// A prefab with an unnamed part: `L1227`.
const UNNAMED: &str = "\
format: 1
kind: prefab
name: bad
entities:
  - components:
      Transform: {}
";

const SCENE: &str = "\
format: 1
seed: 3
entities:
  - name: camera
    components:
      Transform: {}
";

fn root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-prefab-wire-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(PATH), V1).unwrap();
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

type Trace = Vec<(FrameIndex, WorldHash)>;

fn spawn_command(path: &str, name: Option<&str>, translation: Option<[f32; 3]>) -> Command {
    Command::PrefabSpawn(PrefabSpawnParams {
        path: path.to_owned(),
        transform: translation.map(|translation| Placement {
            translation,
            rotation: [0.0; 3],
            scale: [1.0; 3],
        }),
        name: name.map(str::to_owned),
    })
}

fn spawn(engine: &mut Engine, name: Option<&str>, translation: Option<[f32; 3]>) -> Vec<EntityId> {
    match engine
        .execute(spawn_command(PATH, name, translation))
        .unwrap()
        .body
    {
        ResponseBody::PrefabSpawned { entities } => entities,
        other => panic!("unexpected reply {other:?}"),
    }
}

/// Steps `ticks` ticks one at a time, extending `trace` with each `(frame, hash)`.
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

/// A command between ticks restamps the last trace entry, as a replay's drive does.
fn restamp(engine: &mut Engine, trace: &mut Trace) {
    *trace.last_mut().unwrap() = (engine.frame(), engine.world_hash().unwrap());
}

fn query(engine: &mut Engine, world: bool) -> Vec<ludion_protocol::response::EntityMatch> {
    match engine
        .execute(Command::QueryEntities(QueryEntitiesParams {
            world,
            ..QueryEntitiesParams::default()
        }))
        .unwrap()
        .body
    {
        ResponseBody::QueryResult { entities } => entities,
        other => panic!("unexpected reply {other:?}"),
    }
}

fn start(engine: &mut Engine) -> Trace {
    vec![(engine.frame(), engine.world_hash().unwrap())]
}

/// Records `[step 2, spawn a, step 2, destroy a's root, step 2, spawn b, step 2]`.
fn record(root: &Path) -> (Trace, Vec<u8>) {
    let sink = SharedSink::default();
    let mut engine = engine(root);
    let mut trace = start(&mut engine);
    engine.set_recording(sink.clone()).unwrap();
    step(&mut engine, 2, &mut trace);
    let first = spawn(&mut engine, Some("a"), None);
    restamp(&mut engine, &mut trace);
    step(&mut engine, 2, &mut trace);
    engine
        .execute(Command::EntityDestroy(EntityDestroyParams {
            entity: first[0],
        }))
        .unwrap();
    restamp(&mut engine, &mut trace);
    step(&mut engine, 2, &mut trace);
    spawn(&mut engine, Some("b"), None);
    restamp(&mut engine, &mut trace);
    step(&mut engine, 2, &mut trace);
    engine.stop_recording().unwrap();
    (trace, sink.bytes())
}

fn records(bytes: &[u8]) -> Vec<ReplayRecord> {
    ReplayReader::new(bytes).unwrap().records().unwrap()
}

fn replay(root: &Path, bytes: &[u8]) -> Trace {
    replay_hashes_with(game(root), bytes, root).unwrap()
}

#[test]
fn the_reply_lists_the_parts_in_order_with_names_placement_and_instance_links() {
    let root = root("reply");
    let mut placed = engine(&root);
    let ids = spawn(&mut placed, Some("a"), Some([5.0, 0.0, 0.0]));
    assert_eq!(ids.len(), 2);
    let world = placed.world();
    assert_eq!(world.get::<Name>(ids[0]).unwrap().unwrap().0, "a/base");
    assert_eq!(world.get::<Name>(ids[1]).unwrap().unwrap().0, "a/lid");
    for id in &ids {
        let instance = world.get::<PrefabInstance>(*id).unwrap().unwrap();
        assert_eq!(instance.root, ids[0]);
    }
    assert_eq!(world.get::<Parent>(ids[1]).unwrap().unwrap().0, ids[0]);
    let joint = world.get::<Joint>(ids[1]).unwrap().unwrap();
    assert_eq!((joint.a, joint.b), (ids[0], ids[1]));

    // base is authored at x = 1 and the placement shifts it by 5; the lid rides one unit above.
    let matches = query(&mut placed, true);
    let lid = matches.iter().find(|m| m.entity == ids[1]).unwrap();
    assert_eq!(lid.world.as_ref().unwrap().translation, [6.0, 1.0, 0.0]);

    let mut bare = engine(&root);
    let ids = spawn(&mut bare, None, None);
    assert_eq!(bare.world().get::<Name>(ids[0]).unwrap().unwrap().0, "base");
    assert_eq!(bare.world().get::<Name>(ids[1]).unwrap().unwrap().0, "lid");
    let matches = query(&mut bare, true);
    let lid = matches.iter().find(|m| m.entity == ids[1]).unwrap();
    assert_eq!(lid.world.as_ref().unwrap().translation, [1.0, 1.0, 0.0]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_refused_spawn_changes_nothing() {
    let root = root("refuse");
    std::fs::write(root.join("bad.prefab.yaml"), UNNAMED).unwrap();
    let mut engine = engine(&root);
    let hash = engine.world_hash().unwrap();
    let count = query(&mut engine, false).len();
    for (path, code) in [
        ("missing.prefab.yaml", "L1230"),
        ("../x.prefab.yaml", "L3040"),
        ("/abs/x.prefab.yaml", "L3040"),
        ("bad.prefab.yaml", "L1227"),
    ] {
        let error = engine
            .execute(spawn_command(path, None, None))
            .expect_err(path);
        assert_eq!(error.code.to_string(), code, "{path}");
        assert_eq!(engine.world_hash().unwrap(), hash, "{path}");
        assert_eq!(query(&mut engine, false).len(), count, "{path}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_spawn_despawn_spawn_session_replays_from_a_changed_and_a_deleted_prefab() {
    let root = root("gate");
    let (live, bytes) = record(&root);

    let texts: Vec<_> = records(&bytes)
        .into_iter()
        .filter_map(|record| match record {
            ReplayRecord::PrefabText {
                frame, document, ..
            } => Some((frame, document)),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec![(FrameIndex::new(2), V1.to_owned())]);

    assert_eq!(replay(&root, &bytes), live, "unchanged checkout");
    std::fs::write(root.join(PATH), V2).unwrap();
    assert_eq!(replay(&root, &bytes), live, "a changed prefab file");
    std::fs::remove_file(root.join(PATH)).unwrap();
    assert_eq!(replay(&root, &bytes), live, "a deleted prefab file");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_paused_spawn_records_its_text_before_the_command() {
    let root = root("paused");
    let sink = SharedSink::default();
    let mut engine = engine(&root);
    let mut trace = start(&mut engine);
    engine.set_recording(sink.clone()).unwrap();
    step(&mut engine, 2, &mut trace);
    engine.execute(Command::SimPause).unwrap();
    spawn(&mut engine, Some("a"), None);
    engine.stop_recording().unwrap();

    let records = records(&sink.bytes());
    let text = records
        .iter()
        .position(|record| matches!(record, ReplayRecord::PrefabText { .. }))
        .expect("the text is recorded without a further tick");
    let command = records
        .iter()
        .position(|record| {
            matches!(record, ReplayRecord::Command { command, .. }
                if matches!(command, Command::PrefabSpawn(_)))
        })
        .expect("the spawn is recorded");
    assert!(text < command, "text {text} must precede command {command}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_recorded_spawn_without_its_text_diverges_instead_of_reading_disk() {
    let root = root("strip");
    let (live, bytes) = record(&root);

    let reader = ReplayReader::new(&bytes).unwrap();
    let mut stripped = Vec::new();
    let mut writer = ReplayWriter::new(&mut stripped, reader.header()).unwrap();
    for record in reader.records().unwrap() {
        if !matches!(record, ReplayRecord::PrefabText { .. }) {
            writer.write_record(&record).unwrap();
        }
    }
    writer.into_inner().unwrap();

    // The file is still on disk: only a library that read it would reproduce the live trace.
    let refused =
        replay_hashes_with(game(&root), &stripped, &root).expect_err("the spawn is refused");
    assert_eq!(refused.code.to_string(), "L1230");
    assert!(live.len() > 3, "the live run spawned");
    let _ = std::fs::remove_dir_all(&root);
}
