//! `prefab.create` / `apply` / `revert` / `unpack` over the command layer: the replies, refusals that
//! change nothing, and the replay gate (revert and unpack are recorded commands, apply is recorded
//! as its prefab reload, create is not recorded).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ludion_core::ids::EntityId;
use ludion_core::time::FrameIndex;
use ludion_engine::{Engine, EngineBuilder, PrefabInstance, replay_hashes_with};
use ludion_protocol::command::{
    ComponentSetParams, EntitySpawnParams, PrefabApplyParams, PrefabCreateParams,
    PrefabRevertParams, PrefabSpawnParams, PrefabUnpackParams, QueryEntitiesParams, SimStepParams,
};
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use ludion_protocol::{Command, ResponseBody};
use ludion_sim::WorldHash;
use serde_json::json;

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

const SCENE: &str = "\
format: 1
seed: 3
entities:
  - name: camera
    components:
      Transform: {}
";

fn root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-prefab-edit-wire-{tag}-{}",
        std::process::id()
    ));
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

fn spawn(engine: &mut Engine, name: &str) -> Vec<EntityId> {
    let command = Command::PrefabSpawn(PrefabSpawnParams {
        path: PATH.to_owned(),
        transform: None,
        name: Some(name.to_owned()),
        presentation: false,
    });
    match engine.execute(command).unwrap().body {
        ResponseBody::PrefabSpawned { entities } => entities,
        other => panic!("unexpected reply {other:?}"),
    }
}

/// A plain entity named `name`, carrying an identity `Transform` and the given extra components.
fn plain(engine: &mut Engine, name: &str, extra: &[(&str, serde_json::Value)]) -> EntityId {
    let mut components = BTreeMap::new();
    components.insert("Transform".to_owned(), json!({}));
    for (component, value) in extra {
        components.insert((*component).to_owned(), value.clone());
    }
    let command = Command::EntitySpawn(EntitySpawnParams {
        name: Some(name.to_owned()),
        components,
    });
    match engine.execute(command).unwrap().body {
        ResponseBody::Spawned { entity } => entity,
        other => panic!("unexpected reply {other:?}"),
    }
}

fn lift(engine: &mut Engine, entity: EntityId, height: f32) {
    engine
        .execute(Command::ComponentSet(ComponentSetParams {
            entity,
            component: "Transform".to_owned(),
            value: json!({"translation": [0.0, height, 0.0]}),
        }))
        .unwrap();
}

fn create(
    engine: &mut Engine,
    entities: &[EntityId],
    path: &str,
) -> Result<ResponseBody, ludion_core::diagnostics::Diagnostic> {
    engine
        .execute(Command::PrefabCreate(PrefabCreateParams {
            entities: entities.to_vec(),
            path: path.to_owned(),
        }))
        .map(|reply| reply.body)
}

fn apply(
    engine: &mut Engine,
    instance: EntityId,
) -> Result<ResponseBody, ludion_core::diagnostics::Diagnostic> {
    engine
        .execute(Command::PrefabApply(PrefabApplyParams {
            instance,
            parts: None,
        }))
        .map(|reply| reply.body)
}

fn revert(
    engine: &mut Engine,
    instance: EntityId,
    pointer: Option<&str>,
) -> Result<ResponseBody, ludion_core::diagnostics::Diagnostic> {
    engine
        .execute(Command::PrefabRevert(PrefabRevertParams {
            instance,
            pointer: pointer.map(str::to_owned),
        }))
        .map(|reply| reply.body)
}

fn unpack(
    engine: &mut Engine,
    instance: EntityId,
) -> Result<ResponseBody, ludion_core::diagnostics::Diagnostic> {
    engine
        .execute(Command::PrefabUnpack(PrefabUnpackParams { instance }))
        .map(|reply| reply.body)
}

fn entity_count(engine: &mut Engine) -> usize {
    match engine
        .execute(Command::QueryEntities(QueryEntitiesParams::default()))
        .unwrap()
        .body
    {
        ResponseBody::QueryResult { entities } => entities.len(),
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

fn start(engine: &mut Engine) -> Trace {
    vec![(engine.frame(), engine.world_hash().unwrap())]
}

fn records(bytes: &[u8]) -> Vec<ReplayRecord> {
    ReplayReader::new(bytes).unwrap().records().unwrap()
}

fn replay(root: &Path, bytes: &[u8]) -> Trace {
    replay_hashes_with(game(root), bytes, root).unwrap()
}

/// Records `[step 2, spawn a, step 1, set a/lid, revert a, step 1, spawn b + set + apply, spawn c,
/// step 1, unpack a, step 2]`.
fn record(root: &Path) -> (Trace, Vec<u8>) {
    let sink = SharedSink::default();
    let mut engine = engine(root);
    let mut trace = start(&mut engine);
    engine.set_recording(sink.clone()).unwrap();
    step(&mut engine, 2, &mut trace);
    let a = spawn(&mut engine, "a");
    restamp(&mut engine, &mut trace);
    step(&mut engine, 1, &mut trace);
    lift(&mut engine, a[1], 7.0);
    revert(&mut engine, a[0], None).unwrap();
    restamp(&mut engine, &mut trace);
    step(&mut engine, 1, &mut trace);
    let b = spawn(&mut engine, "b");
    lift(&mut engine, b[1], 9.0);
    apply(&mut engine, b[0]).unwrap();
    spawn(&mut engine, "c");
    restamp(&mut engine, &mut trace);
    step(&mut engine, 1, &mut trace);
    unpack(&mut engine, a[0]).unwrap();
    restamp(&mut engine, &mut trace);
    step(&mut engine, 2, &mut trace);
    engine.stop_recording().unwrap();
    (trace, sink.bytes())
}

fn code(error: &ludion_core::diagnostics::Diagnostic) -> String {
    error.code.to_string()
}

#[test]
fn create_apply_revert_and_unpack_reply_with_what_they_did() {
    let root = root("reply");
    let mut engine = engine(&root);

    let x = plain(&mut engine, "x", &[]);
    let y = plain(&mut engine, "y", &[]);
    let made = create(&mut engine, &[x, y], "made.prefab.yaml").unwrap();
    assert_eq!(
        made,
        ResponseBody::PrefabCreated {
            path: "made.prefab.yaml".to_owned()
        }
    );
    assert!(
        std::fs::read_to_string(root.join("made.prefab.yaml"))
            .unwrap()
            .contains("name: y")
    );
    let error = create(&mut engine, &[x], "../x.prefab.yaml").unwrap_err();
    assert_eq!(code(&error), "L3040");
    let dangling = plain(
        &mut engine,
        "z",
        &[("Parent", serde_json::to_value(x).unwrap())],
    );
    let error = create(&mut engine, &[dangling], "z.prefab.yaml").unwrap_err();
    assert_eq!(code(&error), "L1236");

    let a = spawn(&mut engine, "a");
    lift(&mut engine, a[1], 7.0);
    let applied = apply(&mut engine, a[0]).unwrap();
    assert_eq!(
        applied,
        ResponseBody::PrefabApplied {
            path: PATH.to_owned(),
            parts: vec!["lid".to_owned()],
            removed: Vec::new(),
        }
    );
    assert!(
        std::fs::read_to_string(root.join(PATH))
            .unwrap()
            .contains("- 7.0")
    );

    let b = spawn(&mut engine, "b");
    lift(&mut engine, b[1], 9.0);
    let reverted = revert(&mut engine, b[0], None).unwrap();
    assert_eq!(
        reverted,
        ResponseBody::PrefabReverted {
            entities: vec![b[1]],
            not_restored: Vec::new(),
        }
    );

    let unpacked = unpack(&mut engine, b[0]).unwrap();
    assert_eq!(
        unpacked,
        ResponseBody::PrefabUnpacked {
            entities: b.clone()
        }
    );
    for id in &b {
        assert!(engine.world().get::<PrefabInstance>(*id).unwrap().is_none());
    }

    for error in [
        apply(&mut engine, x).unwrap_err(),
        revert(&mut engine, x, None).unwrap_err(),
        unpack(&mut engine, x).unwrap_err(),
    ] {
        assert_eq!(code(&error), "L1235");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_refused_edit_changes_nothing() {
    let root = root("refuse");
    let mut engine = engine(&root);
    let a = spawn(&mut engine, "a");
    let plain = plain(&mut engine, "x", &[]);
    let hash = engine.world_hash().unwrap();
    let count = entity_count(&mut engine);

    let error = revert(&mut engine, a[0], Some("/nowhere")).unwrap_err();
    assert_eq!(code(&error), "L1238");
    assert_eq!(engine.world_hash().unwrap(), hash);
    assert_eq!(entity_count(&mut engine), count);

    let error = unpack(&mut engine, plain).unwrap_err();
    assert_eq!(code(&error), "L1235");
    assert_eq!(engine.world_hash().unwrap(), hash);
    assert_eq!(entity_count(&mut engine), count);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_edit_session_replays_from_a_changed_and_a_deleted_prefab() {
    let root = root("gate");
    let (live, bytes) = record(&root);

    let stream = records(&bytes);
    let commands: Vec<&str> = stream
        .iter()
        .filter_map(|record| match record {
            ReplayRecord::Command { command, .. } => Some(command.method()),
            _ => None,
        })
        .collect();
    assert!(commands.contains(&"prefab.revert"), "{commands:?}");
    assert!(commands.contains(&"prefab.unpack"), "{commands:?}");
    assert!(
        !commands
            .iter()
            .any(|method| matches!(*method, "prefab.create" | "prefab.apply")),
        "{commands:?}"
    );
    assert!(
        stream
            .iter()
            .any(|record| matches!(record, ReplayRecord::PrefabText { reload: true, .. })),
        "the apply's reload is recorded"
    );

    std::fs::write(root.join(PATH), V2).unwrap();
    assert_eq!(replay(&root, &bytes), live, "a changed prefab file");
    std::fs::remove_file(root.join(PATH)).unwrap();
    assert_eq!(replay(&root, &bytes), live, "a deleted prefab file");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_paused_revert_and_unpack_record_their_text_before_the_command() {
    let root = root("paused");
    let sink = SharedSink::default();
    let mut engine = engine(&root);
    let mut trace = start(&mut engine);
    engine.set_recording(sink.clone()).unwrap();
    step(&mut engine, 1, &mut trace);
    let a = spawn(&mut engine, "a");
    engine.execute(Command::SimPause).unwrap();
    revert(&mut engine, a[0], None).unwrap();
    unpack(&mut engine, a[0]).unwrap();
    engine.stop_recording().unwrap();

    let records = records(&sink.bytes());
    let first_command = |method: &str| {
        records
            .iter()
            .position(|record| {
                matches!(record, ReplayRecord::Command { command, .. } if command.method() == method)
            })
            .unwrap_or_else(|| panic!("`{method}` is recorded"))
    };
    let text = records
        .iter()
        .position(|record| matches!(record, ReplayRecord::PrefabText { .. }))
        .expect("the text is recorded without a further tick");
    let revert = first_command("prefab.revert");
    let unpack = first_command("prefab.unpack");
    assert!(text < first_command("prefab.spawn"));
    assert!(text < revert && revert < unpack, "{text} {revert} {unpack}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn apply_refuses_an_instance_whose_prefab_lies_outside_the_root() {
    let base = std::env::temp_dir().join(format!(
        "ludion-prefab-edit-wire-outside-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    let content = base.join("content");
    std::fs::create_dir_all(&content).unwrap();
    std::fs::create_dir_all(base.join("shared")).unwrap();
    std::fs::write(base.join("shared").join(PATH), V1).unwrap();
    let mut engine = game(&content).build().unwrap();
    engine
        .load_inline_scene(&format!(
            "{SCENE}  - name: o\n    prefab: ../shared/{PATH}\n"
        ))
        .unwrap();
    let root = engine
        .world_mut()
        .query::<(&PrefabInstance,)>()
        .next()
        .map(|(instance,)| instance.root)
        .expect("the instance loaded");
    let before = std::fs::read(base.join("shared").join(PATH)).unwrap();

    let error = apply(&mut engine, root).unwrap_err();
    assert_eq!(code(&error), "L3040");
    assert_eq!(
        std::fs::read(base.join("shared").join(PATH)).unwrap(),
        before
    );
    let _ = std::fs::remove_dir_all(&base);
}
