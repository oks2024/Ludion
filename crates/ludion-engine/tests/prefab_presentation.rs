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
    EngineRestoreParams, Placement, PrefabApplyParams, PrefabOverridesParams, PrefabRevertParams,
    PrefabSpawnParams, PrefabUnpackParams, SimStepParams,
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

/// An imported-style prefab: a root and a mesh child, every part sim.
const IMPORTED: &str = "\
format: 1
kind: prefab
name: imported
entities:
  - name: root
    components:
      Transform: {}
  - name: child
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
";

/// A sim post with an imported body hung on it as a presentation instance.
const POST_BODY: &str = "\
format: 1
kind: prefab
name: post-body
entities:
  - name: post
    components:
      Transform: {}
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 0.5
              - 0.5
  - name: body
    prefab: imported.prefab.yaml
    presentation: true
    overrides:
      root:
        Parent:
          index: 0
          generation: 0
";

const POST_ONLY: &str = "\
format: 1
kind: prefab
name: post-body
entities:
  - name: post
    components:
      Transform: {}
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 0.5
              - 0.5
";

fn root_with(tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = root(tag, LAMP_SIM);
    for (name, text) in files {
        std::fs::write(dir.join(name), text).unwrap();
    }
    dir
}

const GROUND: &str = "format: 1\nname: s\nseed: 3\nentities:\n  - name: ground\n    components:\n      Transform: {}\n";

#[test]
fn a_presentation_instance_of_sim_parts_never_touches_the_hash() {
    let full_root = root_with("inst-full", &[("imported.prefab.yaml", IMPORTED)]);
    let sim_root = root_with("inst-sim", &[]);
    let with = format!(
        "{GROUND}  - name: prop\n    prefab: imported.prefab.yaml\n    presentation: true\n"
    );
    let mut full = loaded(&full_root, &with);
    let mut sim = loaded(&sim_root, GROUND);

    let (full_ids, sim_ids) = (index(&mut full), index(&mut sim));
    for (name, id) in &sim_ids {
        assert_eq!(full_ids.get(name), Some(id), "sim id of `{name}` moved");
    }
    for name in ["prop/root", "prop/child"] {
        assert!(is_presentation(full_ids[name]), "{name}");
    }

    let (mut a, mut b) = (start(&mut full), start(&mut sim));
    assert_eq!(a, b);
    step(&mut full, 5, &mut a);
    step(&mut sim, 5, &mut b);
    assert_eq!(a, b, "a presentation instance moved the hash trace");
    let _ = std::fs::remove_dir_all(&full_root);
    let _ = std::fs::remove_dir_all(&sim_root);
}

#[test]
fn a_sim_post_carries_a_presentation_body_through_an_override() {
    let full_root = root_with(
        "body-full",
        &[
            ("imported.prefab.yaml", IMPORTED),
            ("post.prefab.yaml", POST_BODY),
        ],
    );
    let sim_root = root_with("body-sim", &[("post.prefab.yaml", POST_ONLY)]);
    let scene = format!("{GROUND}  - name: pole\n    prefab: post.prefab.yaml\n");
    let mut full = loaded(&full_root, &scene);
    let mut sim = loaded(&sim_root, &scene);

    let (full_ids, sim_ids) = (index(&mut full), index(&mut sim));
    for (name, id) in &sim_ids {
        assert_eq!(full_ids.get(name), Some(id), "sim id of `{name}` moved");
    }
    let post = full_ids["pole/post"];
    assert!(!is_presentation(post));
    for name in ["pole/body/root", "pole/body/child"] {
        assert!(is_presentation(full_ids[name]), "{name}");
    }

    let world = full.world();
    assert_eq!(
        world
            .get::<Parent>(full_ids["pole/body/root"])
            .unwrap()
            .unwrap()
            .0,
        post
    );
    for part in ["pole/post", "pole/body/root", "pole/body/child"] {
        let instance = world
            .get::<PrefabInstance>(full_ids[part])
            .unwrap()
            .unwrap();
        assert_eq!(instance.root, post, "{part}");
    }

    let (mut a, mut b) = (start(&mut full), start(&mut sim));
    assert_eq!(a, b);
    step(&mut full, 5, &mut a);
    step(&mut sim, 5, &mut b);
    assert_eq!(a, b, "the presentation body moved the hash trace");
    let _ = std::fs::remove_dir_all(&full_root);
    let _ = std::fs::remove_dir_all(&sim_root);
}

#[test]
fn a_presentation_instance_over_a_collider_part_is_l1320() {
    let dir = root("l1320", LAMP_SIM);
    let scene =
        format!("{GROUND}  - name: pole\n    prefab: lamp.prefab.yaml\n    presentation: true\n");
    let mut engine = game(&dir).build().unwrap();
    let diagnostics = engine.load_inline_scene(&scene).unwrap_err();
    let refusal = diagnostics
        .iter()
        .find(|d| d.code.as_str() == "L1320")
        .unwrap_or_else(|| panic!("{diagnostics:?}"));
    assert!(
        refusal.message.contains("`post`") && refusal.message.contains("`Collider`"),
        "{}",
        refusal.message
    );
    let _ = std::fs::remove_dir_all(&dir);
}

const OVERRIDE_CHILD: &str = "    overrides:
      child:
        Mesh:
          color:
            - 1.0
            - 0.0
            - 0.0
            - 1.0
";

/// A sim part with a nested imported instance (no key).
const PAIR: &str = "\
format: 1
kind: prefab
name: pair
entities:
  - name: base
    components:
      Transform: {}
  - name: dressing
    prefab: imported.prefab.yaml
";

/// A variant: one imported instance with an override.
const VARIANT: &str = "\
format: 1
kind: prefab
name: variant
entities:
  - name: inner
    prefab: imported.prefab.yaml
    overrides:
      child:
        Mesh:
          color:
            - 0.0
            - 1.0
            - 0.0
            - 1.0
";

/// A prefab that already makes every surviving part presentation.
const DRESSING: &str = "\
format: 1
kind: prefab
name: dressing
entities:
  - name: inner
    prefab: imported.prefab.yaml
    presentation: true
";

const FILES: &[(&str, &str)] = &[
    ("imported.prefab.yaml", IMPORTED),
    ("pair.prefab.yaml", PAIR),
    ("variant.prefab.yaml", VARIANT),
    ("dressing.prefab.yaml", DRESSING),
    ("post.prefab.yaml", POST_BODY),
];

/// Saves `scene` twice and reloads it: the first save is `canonical(scene)`, the second equals the
/// first, and the reload's hash equals the original's. Returns the saved text.
fn round_trip(tag: &str, scene: &str) -> String {
    let dir = root_with(tag, FILES);
    let saved = loaded(&dir, scene).scene_text().expect("the world saves");
    assert_eq!(saved, canonical(scene));
    let mut again = loaded(&dir, &saved);
    assert_eq!(again.scene_text().expect("it saves again"), saved);
    let mut original = loaded(&dir, scene);
    assert_eq!(
        again.world_hash().unwrap(),
        original.world_hash().unwrap(),
        "a reload moved the sim hash"
    );
    let _ = std::fs::remove_dir_all(&dir);
    saved
}

fn entry(prefab: &str, key: bool, extra: &str) -> String {
    let flag = if key { "    presentation: true\n" } else { "" };
    format!("{GROUND}  - name: prop\n    prefab: {prefab}\n{flag}{extra}")
}

fn key_count(saved: &str) -> usize {
    saved.matches("presentation: true").count()
}

#[test]
fn save_writes_presentation_true_for_a_flat_instance() {
    let saved = round_trip("save-flat", &entry("imported.prefab.yaml", true, ""));
    assert_eq!(key_count(&saved), 1, "{saved}");
}

#[test]
fn save_writes_presentation_true_beside_an_override() {
    let saved = round_trip(
        "save-override",
        &entry("imported.prefab.yaml", true, OVERRIDE_CHILD),
    );
    assert!(saved.contains("presentation: true") && saved.contains("overrides:"));
}

#[test]
fn save_writes_presentation_true_for_a_nested_instance() {
    let saved = round_trip("save-nested", &entry("pair.prefab.yaml", true, ""));
    assert_eq!(key_count(&saved), 1, "{saved}");
}

#[test]
fn save_writes_presentation_true_for_a_variant_without_the_key() {
    let saved = round_trip("save-variant", &entry("variant.prefab.yaml", true, ""));
    assert_eq!(key_count(&saved), 1, "{saved}");
}

#[test]
fn an_all_presentation_prefab_saves_without_the_key() {
    let plain = entry("dressing.prefab.yaml", false, "");
    let saved = round_trip("save-dressing", &plain);
    assert_eq!(key_count(&saved), 0, "{saved}");

    let keyed = entry("dressing.prefab.yaml", true, "");
    let dir = root_with("save-dressing-keyed", FILES);
    let first = loaded(&dir, &keyed).scene_text().unwrap();
    assert_eq!(first, canonical(&plain), "the redundant key stays");
    assert_eq!(loaded(&dir, &first).scene_text().unwrap(), first);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_sim_post_with_a_presentation_body_saves_without_the_outer_key() {
    let scene = format!("{GROUND}  - name: pole\n    prefab: post.prefab.yaml\n");
    let saved = round_trip("save-post", &scene);
    assert_eq!(key_count(&saved), 0, "{saved}");
}

#[test]
fn a_live_sim_presentation_mix_the_prefab_does_not_make_is_l1234() {
    let scene = format!(
        "{GROUND}  - name: x/root
    components:
      Transform: {{}}
      PrefabInstance:
        source: imported.prefab.yaml
        part: root
        root:
          index: 1
          generation: 0
  - name: x/child
    presentation: true
    components:
      Transform: {{}}
      Parent:
        index: 1
        generation: 0
      PrefabInstance:
        source: imported.prefab.yaml
        part: child
        root:
          index: 1
          generation: 0
"
    );
    let dir = root_with("mixed", FILES);
    let mut engine = loaded(&dir, &scene);
    let refusal = engine.scene_text().unwrap_err();
    assert_eq!(refusal.code.as_str(), "L1234", "{refusal:?}");
    assert!(
        refusal.message.contains("x/child")
            && refusal.message.contains("presentation")
            && refusal.message.contains("sim"),
        "{}",
        refusal.message
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn tool_scene() -> String {
    entry("imported.prefab.yaml", true, OVERRIDE_CHILD)
}

fn instance_of(engine: &mut Engine) -> (BTreeMap<String, EntityId>, EntityId) {
    let ids = index(engine);
    let root = ids["prop/root"];
    (ids, root)
}

fn all_presentation(ids: &BTreeMap<String, EntityId>) {
    for name in ["prop/root", "prop/child"] {
        assert!(is_presentation(ids[name]), "{name}");
    }
}

#[test]
fn prefab_apply_never_writes_presentation_into_the_prefab() {
    let dir = root_with("apply", FILES);
    let mut engine = loaded(&dir, &tool_scene());
    let (before, root) = instance_of(&mut engine);
    engine
        .execute(Command::PrefabApply(PrefabApplyParams {
            instance: root,
            parts: None,
        }))
        .unwrap();
    let prefab = std::fs::read_to_string(dir.join("imported.prefab.yaml")).unwrap();
    assert!(prefab.contains("color:"), "{prefab}");
    assert!(!prefab.contains("presentation"), "{prefab}");
    let (after, _) = instance_of(&mut engine);
    assert_eq!(before, after);
    all_presentation(&after);
    let saved = engine.scene_text().unwrap();
    assert!(saved.contains("presentation: true") && !saved.contains("overrides:"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn prefab_revert_keeps_the_parts_presentation() {
    let dir = root_with("revert", FILES);
    let mut engine = loaded(&dir, &tool_scene());
    let (before, root) = instance_of(&mut engine);
    let hash = engine.world_hash().unwrap();
    engine
        .execute(Command::PrefabRevert(PrefabRevertParams {
            instance: root,
            pointer: None,
        }))
        .unwrap();
    let (after, _) = instance_of(&mut engine);
    assert_eq!(before, after);
    all_presentation(&after);
    assert_eq!(engine.world_hash().unwrap(), hash, "revert moved the hash");
    let saved = engine.scene_text().unwrap();
    assert!(saved.contains("presentation: true") && !saved.contains("overrides:"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn prefab_unpack_keeps_the_parts_presentation() {
    let dir = root_with("unpack", FILES);
    let mut engine = loaded(&dir, &tool_scene());
    let (before, root) = instance_of(&mut engine);
    let hash = engine.world_hash().unwrap();
    engine
        .execute(Command::PrefabUnpack(PrefabUnpackParams { instance: root }))
        .unwrap();
    let (after, _) = instance_of(&mut engine);
    assert_eq!(before, after);
    all_presentation(&after);
    assert!(
        engine
            .world()
            .get::<PrefabInstance>(after["prop/root"])
            .unwrap()
            .is_none()
    );
    let saved = engine.scene_text().unwrap();
    assert!(
        !saved.contains("prefab:") && key_count(&saved) == 2,
        "{saved}"
    );
    assert!(saved.contains("prop/root") && saved.contains("prop/child"));
    let mut again = loaded(&dir, &saved);
    assert_eq!(again.world_hash().unwrap(), hash);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn prefab_overrides_reports_the_override_of_a_presentation_instance() {
    let dir = root_with("tool-overrides", FILES);
    let mut engine = loaded(&dir, &tool_scene());
    let reply = engine
        .execute(Command::PrefabOverrides(PrefabOverridesParams {
            instances: None,
        }))
        .unwrap();
    let ResponseBody::PrefabOverrides { instances } = reply.body else {
        panic!("unexpected reply");
    };
    let prop = instances
        .iter()
        .find(|i| i.name.as_deref() == Some("prop"))
        .expect("prop");
    assert!(prop.diagnostic.is_none(), "{:?}", prop.diagnostic);
    assert_eq!(
        prop.overrides.get("child"),
        Some(&vec!["/Mesh/color".to_owned()])
    );
    let _ = std::fs::remove_dir_all(&dir);
}
