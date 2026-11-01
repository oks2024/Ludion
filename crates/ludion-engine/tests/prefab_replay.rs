//! Replay proof for runtime prefabs: a recording carries every prefab text the library read as an
//! inline `prefab_text` record, so a spawn/despawn/spawn session reproduces its hash trace from a
//! checkout where the prefab file has since changed or vanished, and a replay never falls back to disk.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ludion_core::ids::EntityId;
use ludion_core::time::FrameIndex;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Engine, EngineBuilder, replay_hashes_with, spawn_prefab};
use ludion_protocol::command::{
    AssetReloadParams, EngineRestoreParams, Placement, PrefabSpawnParams, SceneLoadParams,
    SimStepParams,
};
use ludion_protocol::replay::{ReplayReader, ReplayRecord, ReplayWriter};
use ludion_protocol::{Command, ResponseBody};
use ludion_sim::WorldHash;

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

const V3: &str = "\
format: 1
kind: prefab
name: crate
entities:
  - name: base
    components:
      Transform:
        translation:
          - 9.0
          - 9.0
          - 9.0
";

const SCENE: &str = "\
format: 1
seed: 3
entities:
  - name: camera
    components:
      Transform: {}
";

/// The ids each spawn returned, in spawn order.
#[derive(Default)]
struct Spawned(Vec<Vec<EntityId>>);

/// Spawns at 2 (named `a`), despawns the first spawn's root at 4 (the cascade takes the child), and
/// spawns again at 6 (named `b`).
fn spawner(world: &mut World, ctx: &SystemCtx) {
    match ctx.frame.as_u64() {
        2 | 6 => {
            let name = if ctx.frame.as_u64() == 2 { "a" } else { "b" };
            // A refused spawn (a replay whose record is missing) is a no-op, not a panic.
            if let Ok(ids) = spawn_prefab(world, PATH, None, Some(name), false) {
                let mut spawned = world.remove_resource::<Spawned>().unwrap_or_default();
                spawned.0.push(ids);
                world.insert_resource(spawned);
            }
        }
        4 => {
            if let Some(root) = world.resource::<Spawned>().map(|spawned| spawned.0[0][0]) {
                world.despawn(root).expect("the root is alive");
            }
        }
        _ => {}
    }
}

fn root(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-prefab-replay-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(PATH), V1).unwrap();
    dir
}

fn game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .system("spawner", spawner)
        .project_root(root)
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

/// Steps `engine` one tick at a time until `until`, extending `trace` with each `(frame, hash)`.
fn run_to(engine: &mut Engine, until: u64, trace: &mut Trace) {
    while engine.frame().as_u64() < until {
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
    engine.load_inline_scene(SCENE).unwrap();
    vec![(engine.frame(), engine.world_hash().unwrap())]
}

/// Records `steps` ticks of the spawner game over `root`, returning the live trace and the bytes.
fn record(root: &Path, steps: u64) -> (Trace, Vec<u8>) {
    let sink = SharedSink::default();
    let mut engine = game(root).build().unwrap();
    let mut trace = start(&mut engine);
    engine.set_recording(sink.clone()).unwrap();
    run_to(&mut engine, steps, &mut trace);
    engine.stop_recording().unwrap();
    (trace, sink.bytes())
}

fn prefab_texts(bytes: &[u8]) -> Vec<(FrameIndex, String)> {
    ReplayReader::new(bytes)
        .unwrap()
        .records()
        .unwrap()
        .into_iter()
        .filter_map(|record| match record {
            ReplayRecord::PrefabText {
                frame, document, ..
            } => Some((frame, document)),
            _ => None,
        })
        .collect()
}

fn replay(root: &Path, bytes: &[u8]) -> Trace {
    replay_hashes_with(game(root), bytes, root).unwrap()
}

#[test]
fn a_spawn_despawn_spawn_session_replays_from_a_changed_and_a_deleted_prefab() {
    let root = root("gate");
    let (live, bytes) = record(&root, 8);

    let texts = prefab_texts(&bytes);
    assert_eq!(texts.len(), 1, "one disk read, one record");
    assert_eq!(texts[0], (FrameIndex::new(2), V1.to_owned()));

    assert_eq!(replay(&root, &bytes), live, "unchanged checkout");
    std::fs::write(root.join(PATH), V2).unwrap();
    assert_eq!(replay(&root, &bytes), live, "a changed prefab file");
    std::fs::remove_file(root.join(PATH)).unwrap();
    assert_eq!(replay(&root, &bytes), live, "a deleted prefab file");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_reload_is_recorded_and_the_later_spawn_replays_the_new_text() {
    let root = root("reload");
    let sink = SharedSink::default();
    let mut engine = game(&root).build().unwrap();
    let mut trace = start(&mut engine);
    engine.set_recording(sink.clone()).unwrap();
    run_to(&mut engine, 4, &mut trace);
    std::fs::write(root.join(PATH), V2).unwrap();
    let reload_frame = engine.frame();
    engine
        .execute(Command::AssetReload(AssetReloadParams {
            path: Some(PATH.to_owned()),
        }))
        .unwrap();
    run_to(&mut engine, 8, &mut trace);
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();

    let texts = prefab_texts(&bytes);
    assert_eq!(
        texts,
        vec![
            (FrameIndex::new(2), V1.to_owned()),
            (reload_frame, V2.to_owned())
        ]
    );

    std::fs::write(root.join(PATH), V3).unwrap();
    assert_eq!(replay(&root, &bytes), trace);
    std::fs::remove_file(root.join(PATH)).unwrap();
    assert_eq!(replay(&root, &bytes), trace);

    // The frame-6 spawn is the three-part v2 on the live run too.
    let spawned = engine.world().resource::<Spawned>().unwrap();
    assert_eq!(spawned.0[0].len(), 2);
    assert_eq!(spawned.0[1].len(), 3);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn arming_after_the_first_read_is_refused_and_records_nothing() {
    let root = root("arm");
    let sink = SharedSink::default();
    let mut engine = game(&root).build().unwrap();
    let mut trace = start(&mut engine);
    run_to(&mut engine, 3, &mut trace);

    // The held prefab text was read at frame 2; a recording armed now would start mid-session.
    let refused = engine.set_recording(sink.clone()).unwrap_err();
    assert_eq!(refused.code.as_str(), "L3047");
    assert!(!engine.is_recording());
    assert!(sink.bytes().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_recording_without_its_prefab_record_is_refused_not_read_from_disk() {
    let root = root("miss");
    let (live, bytes) = record(&root, 8);

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
    let replayed = replay(&root, &stripped);
    assert_ne!(replayed, live, "the spawn must be refused");
    assert_eq!(
        replayed[1], live[1],
        "the traces agree until the spawn frame"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Steps `ticks` ticks one at a time, extending `trace` with each `(frame, hash)`.
fn run_ticks(engine: &mut Engine, ticks: u64, trace: &mut Trace) {
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

fn wire_spawn(engine: &mut Engine, path: &str, name: &str) -> Vec<EntityId> {
    let reply = engine
        .execute(Command::PrefabSpawn(PrefabSpawnParams {
            path: path.to_owned(),
            transform: None::<Placement>,
            name: Some(name.to_owned()),
            presentation: false,
        }))
        .unwrap();
    match reply.body {
        ResponseBody::PrefabSpawned { entities } => entities,
        other => panic!("unexpected reply {other:?}"),
    }
}

#[test]
fn a_spawn_after_a_rebuild_with_the_file_gone_replays_as_refused() {
    let root = root("rebuild");
    std::fs::write(root.join("scene.yaml"), SCENE).unwrap();
    let sink = SharedSink::default();
    let mut engine = game(&root).build().unwrap();
    let mut trace = start(&mut engine);
    engine.set_recording(sink.clone()).unwrap();
    run_ticks(&mut engine, 4, &mut trace);
    assert!(
        engine.world().resource::<Spawned>().is_some(),
        "the first spawn read the file"
    );

    std::fs::remove_file(root.join(PATH)).unwrap();
    engine
        .execute(Command::SceneLoad(SceneLoadParams {
            path: "scene.yaml".to_owned(),
        }))
        .unwrap();
    restamp(&mut engine, &mut trace);
    run_ticks(&mut engine, 4, &mut trace);
    assert!(
        engine.world().resource::<Spawned>().is_none(),
        "the rebuilt library re-reads the missing file and refuses"
    );
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();

    assert_eq!(replay(&root, &bytes), trace);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_restore_rewinding_past_a_read_replays_the_text_read_after_it() {
    let root = root("restore");
    let sink = SharedSink::default();
    let mut engine = game(&root).build().unwrap();
    let mut trace = start(&mut engine);
    engine.set_recording(sink.clone()).unwrap();
    run_ticks(&mut engine, 1, &mut trace);
    let snapshot = match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("unexpected reply {other:?}"),
    };
    // One tick past the spawn, so its hash is not the entry the restore restamps.
    run_ticks(&mut engine, 3, &mut trace);
    assert_eq!(engine.world().resource::<Spawned>().unwrap().0[0].len(), 2);

    std::fs::write(root.join(PATH), V2).unwrap();
    engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    restamp(&mut engine, &mut trace);
    run_ticks(&mut engine, 2, &mut trace);
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();

    assert_eq!(replay(&root, &bytes), trace, "the file as recorded");
    std::fs::write(root.join(PATH), V3).unwrap();
    assert_eq!(replay(&root, &bytes), trace, "a changed file");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_same_frame_reload_between_two_wire_spawns_replays_in_order() {
    let root = root("same-frame");
    let sink = SharedSink::default();
    let mut engine = Engine::builder().project_root(&root).build().unwrap();
    let mut trace = start(&mut engine);
    engine.set_recording(sink.clone()).unwrap();
    run_ticks(&mut engine, 2, &mut trace);
    assert_eq!(wire_spawn(&mut engine, PATH, "a").len(), 2);
    restamp(&mut engine, &mut trace);
    std::fs::write(root.join(PATH), V2).unwrap();
    engine
        .execute(Command::AssetReload(AssetReloadParams {
            path: Some(PATH.to_owned()),
        }))
        .unwrap();
    assert_eq!(wire_spawn(&mut engine, PATH, "b").len(), 3);
    restamp(&mut engine, &mut trace);
    run_ticks(&mut engine, 2, &mut trace);
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();

    // Both texts carry one stamp: a stamp-only lookup cannot order them, the stream position can.
    let texts = prefab_texts(&bytes);
    assert_eq!(texts.len(), 2);
    assert_eq!(texts[0].0, texts[1].0);
    assert_eq!((texts[0].1.as_str(), texts[1].1.as_str()), (V1, V2));

    let bare = |root: &Path| replay_hashes_with(Engine::builder(), &bytes, root).unwrap();
    std::fs::write(root.join(PATH), V3).unwrap();
    assert_eq!(bare(&root), trace);
    std::fs::remove_file(root.join(PATH)).unwrap();
    assert_eq!(bare(&root), trace);
    let _ = std::fs::remove_dir_all(&root);
}

/// Spawns once, at frame 3.
fn spawn_at_three(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 3 {
        spawn_prefab(world, PATH, None, Some("late"), false).expect("the prefab is readable");
    }
}

#[test]
fn a_free_run_read_mid_batch_is_stamped_at_its_tick() {
    let root = root("free-run");
    let sink = SharedSink::default();
    let mut engine = Engine::builder()
        .system("spawn_at_three", spawn_at_three)
        .project_root(&root)
        .build()
        .unwrap();
    let mut trace = start(&mut engine);
    engine.set_recording(sink.clone()).unwrap();
    engine.execute(Command::SimResume).unwrap();
    restamp(&mut engine, &mut trace);
    for _ in 0..6 {
        assert!(engine.advance_live());
        trace.push((engine.frame(), engine.world_hash().unwrap()));
    }
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();

    assert_eq!(
        prefab_texts(&bytes),
        vec![(FrameIndex::new(3), V1.to_owned())]
    );
    let replayed = replay_hashes_with(
        Engine::builder()
            .system("spawn_at_three", spawn_at_three)
            .project_root(&root),
        &bytes,
        &root,
    )
    .unwrap();
    assert_eq!(replayed, trace);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_reload_through_a_dotted_path_refreshes_the_held_prefab() {
    let root = root("dotted");
    let mut engine = Engine::builder().project_root(&root).build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    assert_eq!(wire_spawn(&mut engine, "./crate.prefab.yaml", "a").len(), 2);

    std::fs::write(root.join(PATH), V2).unwrap();
    let reply = engine
        .execute(Command::AssetReload(AssetReloadParams {
            path: Some("./crate.prefab.yaml".to_owned()),
        }))
        .unwrap();
    match reply.body {
        ResponseBody::AssetsReloaded { paths } => assert!(!paths.is_empty()),
        other => panic!("unexpected reply {other:?}"),
    }
    assert_eq!(wire_spawn(&mut engine, PATH, "b").len(), 3);
    let _ = std::fs::remove_dir_all(&root);
}

const OUTER: &str = "\
format: 1
kind: prefab
name: outer
entities:
  - name: hull
    components:
      Transform: {}
  - name: inner
    prefab: inner.prefab.yaml
    transform:
      translation:
        - 0.0
        - 2.0
        - 0.0
";

const INNER: &str = "\
format: 1
kind: prefab
name: inner
entities:
  - name: gun
    components:
      Transform: {}
";

fn nested_spawner(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 2 {
        // A refused spawn (a replay whose record is missing) is a no-op, not a panic.
        let _ = spawn_prefab(world, "outer.prefab.yaml", None, Some("o"), false);
    }
}

#[test]
fn a_nested_spawn_records_both_texts_and_replays_from_deleted_files() {
    let root = root("nested");
    std::fs::write(root.join("outer.prefab.yaml"), OUTER).unwrap();
    std::fs::write(root.join("inner.prefab.yaml"), INNER).unwrap();
    let game = |root: &Path| {
        Engine::builder()
            .system("nested_spawner", nested_spawner)
            .project_root(root)
    };

    let sink = SharedSink::default();
    let mut engine = game(&root).build().unwrap();
    let mut live = start(&mut engine);
    engine.set_recording(sink.clone()).unwrap();
    run_to(&mut engine, 5, &mut live);
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();

    let mut texts: Vec<String> = prefab_texts(&bytes)
        .into_iter()
        .map(|(_, document)| document)
        .collect();
    texts.sort();
    let mut want = vec![OUTER.to_owned(), INNER.to_owned()];
    want.sort();
    assert_eq!(
        texts, want,
        "the outer and the nested read are both recorded"
    );

    std::fs::remove_file(root.join("outer.prefab.yaml")).unwrap();
    std::fs::remove_file(root.join("inner.prefab.yaml")).unwrap();
    assert_eq!(
        replay_hashes_with(game(&root), &bytes, &root).unwrap(),
        live
    );
    let _ = std::fs::remove_dir_all(&root);
}

// --- Scene-load reads and nested reloads -------------------------------------------------------

/// A different valid inner prefab: the gun sits elsewhere.
const INNER_MOVED: &str = "\
format: 1
kind: prefab
name: inner
entities:
  - name: gun
    components:
      Transform:
        translation:
          - 3.0
          - 0.0
          - 0.0
";

/// The inner prefab with its only part renamed, so an override on `gun` has nothing to land on.
const INNER_RENAMED: &str = "\
format: 1
kind: prefab
name: inner
entities:
  - name: barrel
    components:
      Transform: {}
";

/// An outer prefab whose nested instance overrides the inner `gun`.
const OUTER_OVERRIDING: &str = "\
format: 1
kind: prefab
name: outer
entities:
  - name: hull
    components:
      Transform: {}
  - name: inner
    prefab: inner.prefab.yaml
    overrides:
      gun:
        Transform:
          translation:
            - 0.0
            - 0.0
            - 4.0
";

/// A scene whose `o` entry is an instance of the nesting outer prefab.
const INSTANCE_SCENE: &str = "\
format: 1
seed: 4
entities:
  - name: camera
    components:
      Transform: {}
  - name: o
    prefab: outer.prefab.yaml
";

/// A district whose `o` entry is an instance of the nesting outer prefab, with the switch's anchor.
const INSTANCE_DISTRICT: &str = "\
format: 1
seed: 4
entities:
  - name: gate-from-start
    components:
      Transform:
        translation:
          - 40.0
          - 0.0
          - -10.0
  - name: o
    prefab: outer.prefab.yaml
";

/// A root holding the nesting prefab pair and the instance scene files.
fn nested_root(tag: &str) -> PathBuf {
    let root = root(tag);
    std::fs::write(root.join("outer.prefab.yaml"), OUTER).unwrap();
    std::fs::write(root.join("inner.prefab.yaml"), INNER).unwrap();
    std::fs::write(root.join("scene.yaml"), INSTANCE_SCENE).unwrap();
    std::fs::write(root.join("next.yaml"), INSTANCE_DISTRICT).unwrap();
    root
}

/// The record stream as markers, in stream order: `None` for a scene load or switch, the document
/// for a prefab text.
fn stream(bytes: &[u8]) -> Vec<Option<String>> {
    ReplayReader::new(bytes)
        .unwrap()
        .records()
        .unwrap()
        .into_iter()
        .filter_map(|record| match record {
            ReplayRecord::PrefabText { document, .. } => Some(Some(document)),
            ReplayRecord::SceneLoad { .. } | ReplayRecord::SceneSwitch { .. } => Some(None),
            _ => None,
        })
        .collect()
}

/// Replays `bytes` from `root` unchanged, after the nesting files are edited, then deleted.
fn assert_replays_without_the_prefab_files(root: &Path, bytes: &[u8], live: &Trace) {
    let bare = |root: &Path| replay_hashes_with(Engine::builder(), bytes, root).unwrap();
    assert_eq!(&bare(root), live, "unchanged checkout");
    std::fs::write(root.join("outer.prefab.yaml"), V3).unwrap();
    std::fs::write(root.join("inner.prefab.yaml"), INNER_MOVED).unwrap();
    assert_eq!(&bare(root), live, "changed prefab files");
    std::fs::remove_file(root.join("outer.prefab.yaml")).unwrap();
    std::fs::remove_file(root.join("inner.prefab.yaml")).unwrap();
    assert_eq!(&bare(root), live, "deleted prefab files");
}

#[test]
fn a_scene_load_of_an_instance_scene_records_its_reads_in_the_new_epoch() {
    let root = nested_root("load-reads");
    let sink = SharedSink::default();
    let mut engine = Engine::builder().project_root(&root).build().unwrap();
    let mut trace = start(&mut engine);
    engine.set_recording(sink.clone()).unwrap();
    run_ticks(&mut engine, 2, &mut trace);
    engine
        .execute(Command::SceneLoad(SceneLoadParams {
            path: "scene.yaml".to_owned(),
        }))
        .unwrap();
    restamp(&mut engine, &mut trace);
    run_ticks(&mut engine, 3, &mut trace);
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();

    let records = stream(&bytes);
    assert_eq!(records[0], None, "the scene load comes first");
    let mut texts: Vec<String> = records[1..].iter().flatten().cloned().collect();
    texts.sort();
    let mut want = vec![OUTER.to_owned(), INNER.to_owned()];
    want.sort();
    assert_eq!(texts, want, "both reads follow the scene load");

    assert_replays_without_the_prefab_files(&root, &bytes, &trace);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_header_scene_with_instances_replays_from_deleted_prefab_files() {
    let root = nested_root("header");
    let sink = SharedSink::default();
    let mut engine = Engine::builder().project_root(&root).build().unwrap();
    engine.load_inline_scene(INSTANCE_SCENE).unwrap();
    let mut trace = vec![(engine.frame(), engine.world_hash().unwrap())];
    engine.set_recording(sink.clone()).unwrap();
    run_to(&mut engine, 5, &mut trace);
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();

    let texts = prefab_texts(&bytes);
    assert_eq!(texts.len(), 2);
    assert!(texts.iter().all(|(frame, _)| *frame == FrameIndex::ZERO));

    assert_replays_without_the_prefab_files(&root, &bytes, &trace);
    let _ = std::fs::remove_dir_all(&root);
}

/// Requests a switch to `next.yaml` on frame 2, carrying nothing.
fn switch_at_two(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 2
        && let Some(switch) = world.resource_mut::<ludion_engine::SceneSwitch>()
    {
        switch.request("next.yaml", "gate-from-start", Vec::new());
    }
}

#[test]
fn a_district_switch_into_instances_replays_from_deleted_prefab_files() {
    let root = nested_root("switch");
    let sink = SharedSink::default();
    let mut engine = Engine::builder()
        .system("switch_at_two", switch_at_two)
        .project_root(&root)
        .build()
        .unwrap();
    let mut trace = start(&mut engine);
    engine.set_recording(sink.clone()).unwrap();
    run_to(&mut engine, 6, &mut trace);
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();

    let texts: Vec<String> = stream(&bytes).into_iter().flatten().collect();
    assert_eq!(texts.len(), 2, "the switch recorded both reads: {texts:?}");

    std::fs::remove_file(root.join("next.yaml")).unwrap();
    let replayed = |root: &Path| {
        replay_hashes_with(
            Engine::builder().system("switch_at_two", switch_at_two),
            &bytes,
            root,
        )
        .unwrap()
    };
    assert_eq!(replayed(&root), trace, "unchanged prefab files");
    std::fs::write(root.join("outer.prefab.yaml"), V3).unwrap();
    std::fs::write(root.join("inner.prefab.yaml"), INNER_MOVED).unwrap();
    assert_eq!(replayed(&root), trace, "changed prefab files");
    std::fs::remove_file(root.join("outer.prefab.yaml")).unwrap();
    std::fs::remove_file(root.join("inner.prefab.yaml")).unwrap();
    assert_eq!(replayed(&root), trace, "deleted prefab files");
    let _ = std::fs::remove_dir_all(&root);
}

/// Spawns the outer prefab at frames 2 and 6, named `first` and `second`.
fn twice_spawner(world: &mut World, ctx: &SystemCtx) {
    let name = match ctx.frame.as_u64() {
        2 => "first",
        6 => "second",
        _ => return,
    };
    let _ = spawn_prefab(world, "outer.prefab.yaml", None, Some(name), false);
}

/// The translation of the entity named `name`.
fn translation_of(world: &mut World, name: &str) -> [f32; 3] {
    world
        .query::<(&ludion_engine::Name, &ludion_engine::Transform)>()
        .find(|(named, _)| named.0 == name)
        .map(|(_, transform)| transform.translation)
        .unwrap_or_else(|| panic!("no entity named `{name}`"))
}

fn reload(engine: &mut Engine, path: &str) -> Result<(), String> {
    engine
        .execute(Command::AssetReload(AssetReloadParams {
            path: Some(path.to_owned()),
        }))
        .map(|_| ())
        .map_err(|error| error.message)
}

#[test]
fn reloading_a_nested_prefab_refreshes_the_outer_live_and_in_replay() {
    let root = nested_root("nested-reload");
    let sink = SharedSink::default();
    let mut engine = Engine::builder()
        .system("twice_spawner", twice_spawner)
        .project_root(&root)
        .build()
        .unwrap();
    let mut trace = start(&mut engine);
    engine.set_recording(sink.clone()).unwrap();
    run_to(&mut engine, 4, &mut trace);
    std::fs::write(root.join("inner.prefab.yaml"), INNER_MOVED).unwrap();
    reload(&mut engine, "inner.prefab.yaml").unwrap();
    run_to(&mut engine, 8, &mut trace);
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();

    let world = engine.world_mut();
    assert_eq!(translation_of(world, "first/inner/gun"), [0.0, 2.0, 0.0]);
    assert_eq!(translation_of(world, "second/inner/gun"), [3.0, 2.0, 0.0]);

    let replayed = |root: &Path| {
        replay_hashes_with(
            Engine::builder().system("twice_spawner", twice_spawner),
            &bytes,
            root,
        )
        .unwrap()
    };
    std::fs::remove_file(root.join("outer.prefab.yaml")).unwrap();
    std::fs::remove_file(root.join("inner.prefab.yaml")).unwrap();
    assert_eq!(replayed(&root), trace, "both prefab files deleted");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn arming_after_a_reload_relogs_the_text_the_load_read_then_the_reload() {
    let root = nested_root("arm-reload");
    let sink = SharedSink::default();
    let mut engine = Engine::builder()
        .system("twice_spawner", twice_spawner)
        .project_root(&root)
        .build()
        .unwrap();
    engine.load_inline_scene(INSTANCE_SCENE).unwrap();
    std::fs::write(root.join("inner.prefab.yaml"), INNER_MOVED).unwrap();
    reload(&mut engine, "inner.prefab.yaml").unwrap();
    let mut trace = vec![(engine.frame(), engine.world_hash().unwrap())];
    engine.set_recording(sink.clone()).unwrap();
    run_to(&mut engine, 8, &mut trace);
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();

    let world = engine.world_mut();
    assert_eq!(translation_of(world, "o/inner/gun"), [0.0, 2.0, 0.0]);
    assert_eq!(translation_of(world, "second/inner/gun"), [3.0, 2.0, 0.0]);

    let replayed = |root: &Path| {
        replay_hashes_with(
            Engine::builder().system("twice_spawner", twice_spawner),
            &bytes,
            root,
        )
        .unwrap()
    };
    std::fs::remove_file(root.join("outer.prefab.yaml")).unwrap();
    std::fs::remove_file(root.join("inner.prefab.yaml")).unwrap();
    assert_eq!(replayed(&root), trace, "both prefab files deleted");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_nested_edit_that_breaks_the_outer_is_refused_naming_it() {
    let root = nested_root("nested-break");
    std::fs::write(root.join("outer.prefab.yaml"), OUTER_OVERRIDING).unwrap();
    let mut engine = Engine::builder()
        .system("twice_spawner", twice_spawner)
        .project_root(&root)
        .build()
        .unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    let mut trace = Trace::new();
    run_to(&mut engine, 4, &mut trace);
    assert_eq!(
        translation_of(engine.world_mut(), "first/inner/gun"),
        [0.0, 0.0, 4.0]
    );

    std::fs::write(root.join("inner.prefab.yaml"), INNER_RENAMED).unwrap();
    let refusal = reload(&mut engine, "inner.prefab.yaml").unwrap_err();
    assert!(
        refusal.contains("outer.prefab.yaml") && refusal.contains("breaks"),
        "{refusal}"
    );

    run_to(&mut engine, 8, &mut trace);
    assert_eq!(
        translation_of(engine.world_mut(), "second/inner/gun"),
        [0.0, 0.0, 4.0],
        "a later spawn still composes the old inner"
    );
    let _ = std::fs::remove_dir_all(&root);
}
