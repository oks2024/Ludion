//! `prefab.overrides` over the command layer: what `scene.save` would write for each instance, read
//! without recording, hashing or changing anything.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_ecs::{Parent, World};
use ludion_engine::{Engine, Name};
use ludion_protocol::command::{
    ComponentSetParams, Placement, PrefabOverridesParams, PrefabRevertParams, PrefabSpawnParams,
};
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use ludion_protocol::response::PrefabInstanceOverrides;
use ludion_protocol::{Command, ResponseBody};
use serde_json::json;
use std::collections::BTreeMap;

const PAIR: &str = "\
format: 1
kind: prefab
name: pair
entities:
  - name: base
    components:
      Transform:
        translation:
          - 1.0
          - 0.0
          - 0.0
  - name: arm
    components:
      Transform:
        translation:
          - 0.0
          - 1.0
          - 0.0
      Parent:
        index: 0
        generation: 0
  - name: tip
    components:
      Transform: {}
      Parent:
        index: 1
        generation: 0
      PointLight:
        intensity: 2.0
";

const BARREL: &str = "\
format: 1
kind: prefab
name: barrel
entities:
  - name: tube
    components:
      Transform: {}
  - name: muzzle
    components:
      Transform:
        translation:
          - 0.0
          - 0.5
          - 0.0
      Parent:
        index: 0
        generation: 0
      PointLight:
        intensity: 1.0
";

const TURRET: &str = "\
format: 1
kind: prefab
name: turret
entities:
  - name: base
    components:
      Transform: {}
  - name: barrel
    prefab: barrel.prefab.yaml
    transform:
      translation:
        - 0.0
        - 2.0
        - 0.0
";

const TWO: &str = "format: 1\nname: s\nentities:\n  - name: pair-1\n    prefab: pair.prefab.yaml\n  - name: pair-2\n    prefab: pair.prefab.yaml\n";

const EDITED: &str = "\
format: 1
name: s
entities:
  - name: pair-1
    prefab: pair.prefab.yaml
    overrides:
      arm:
        Transform:
          translation:
            - 0.0
            - 5.0
            - 0.0
      tip:
        PointLight:
          intensity: 3.0
  - name: pair-2
    prefab: pair.prefab.yaml
    removed:
      - tip
";

fn root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-prefab-overrides-wire-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("pair.prefab.yaml"), PAIR).unwrap();
    dir
}

fn loaded(root: &Path, scene: &str) -> Engine {
    let mut engine = Engine::builder().project_root(root).build().unwrap();
    engine.load_inline_scene(scene).expect("the scene loads");
    engine
}

fn index(world: &mut World) -> BTreeMap<String, EntityId> {
    world
        .query_ids::<(&Name,)>()
        .map(|(id, (name,))| (name.0.clone(), id))
        .collect()
}

fn spawn(engine: &mut Engine, name: Option<&str>, placement: Option<Placement>) -> Vec<EntityId> {
    let command = Command::PrefabSpawn(PrefabSpawnParams {
        path: "pair.prefab.yaml".to_owned(),
        transform: placement,
        name: name.map(str::to_owned),
    });
    match engine.execute(command).unwrap().body {
        ResponseBody::PrefabSpawned { entities } => entities,
        other => panic!("unexpected reply {other:?}"),
    }
}

fn set(engine: &mut Engine, entity: EntityId, component: &str, value: serde_json::Value) {
    engine
        .execute(Command::ComponentSet(ComponentSetParams {
            entity,
            component: component.to_owned(),
            value,
        }))
        .unwrap();
}

fn read(
    engine: &mut Engine,
    instances: Option<Vec<EntityId>>,
) -> Result<Vec<PrefabInstanceOverrides>, ludion_core::diagnostics::Diagnostic> {
    let reply = engine.execute(Command::PrefabOverrides(PrefabOverridesParams {
        instances,
    }))?;
    match reply.body {
        ResponseBody::PrefabOverrides { instances } => Ok(instances),
        other => panic!("unexpected reply {other:?}"),
    }
}

#[test]
fn a_loaded_entry_reads_back_as_its_overrides_and_removals() {
    let root = root("loaded");
    let mut engine = loaded(&root, EDITED);
    let read = read(&mut engine, None).unwrap();
    assert_eq!(read.len(), 2);

    let first = &read[0];
    assert_eq!(first.name.as_deref(), Some("pair-1"));
    assert_eq!(first.prefab, "pair.prefab.yaml");
    assert!(!first.placement_differs && first.diagnostic.is_none());
    assert_eq!(first.overrides["arm"], ["/Transform/translation"]);
    assert_eq!(first.overrides["tip"], ["/PointLight/intensity"]);
    assert!(first.removed.is_empty());
    assert_eq!(first.root_parts, ["base"]);

    let second = &read[1];
    assert_eq!(second.root_parts, ["base"]);
    assert_eq!(second.name.as_deref(), Some("pair-2"));
    assert!(second.overrides.is_empty() && !second.placement_differs);
    assert_eq!(second.removed, ["tip"]);

    // One source of truth: the saved entry carries the same override keys.
    let saved = ludion_scene::parse(&engine.scene_text().unwrap()).unwrap();
    let entry = |name: &str| {
        saved
            .entities
            .iter()
            .find(|entity| entity.name.as_deref() == Some(name))
            .and_then(|entity| entity.instance.as_deref())
            .expect("an instance entry")
            .clone()
    };
    let saved_first = entry("pair-1");
    assert_eq!(
        saved_first.overrides.keys().collect::<Vec<_>>(),
        first.overrides.keys().collect::<Vec<_>>()
    );
    assert_eq!(entry("pair-2").removed, second.removed);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_untouched_rotated_spawn_has_no_overrides_and_edits_show() {
    let root = root("spawn");
    let mut engine = loaded(&root, "format: 1\nentities: []\n");
    let parts = spawn(
        &mut engine,
        Some("a"),
        Some(Placement {
            translation: [3.0, 0.0, 2.0],
            rotation: [0.0, 37.0, 0.0],
            scale: [1.0, 1.0, 1.0],
        }),
    );
    let read_one = |engine: &mut Engine| read(engine, None).unwrap().remove(0);

    let clean = read_one(&mut engine);
    assert_eq!(clean.name.as_deref(), Some("a"));
    assert_eq!(clean.root, parts[0]);
    assert!(clean.overrides.is_empty() && clean.removed.is_empty());
    assert!(
        !clean.placement_differs,
        "an unmoved spawn keeps its placement"
    );
    assert!(clean.diagnostic.is_none());

    set(
        &mut engine,
        parts[1],
        "Transform",
        json!({"translation": [0.0, 9.0, 0.0]}),
    );
    let edited = read_one(&mut engine);
    assert_eq!(edited.overrides["arm"], ["/Transform/translation"]);
    assert!(!edited.placement_differs);

    set(
        &mut engine,
        parts[0],
        "Transform",
        json!({"translation": [8.0, 0.0, 0.0]}),
    );
    assert!(read_one(&mut engine).placement_differs);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_unnamed_spawn_reads_as_unnamed_and_empty() {
    let root = root("unnamed");
    let mut engine = loaded(&root, "format: 1\nentities: []\n");
    spawn(&mut engine, None, None);
    let read = read(&mut engine, None).unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].name, None);
    assert!(read[0].overrides.is_empty() && read[0].removed.is_empty());
    assert!(!read[0].placement_differs && read[0].diagnostic.is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_refused_instance_carries_its_own_diagnostic_and_the_call_succeeds() {
    let root = root("refused");
    let mut engine = loaded(
        &root,
        &format!("{TWO}  - name: x\n    components:\n      Transform: {{}}\n"),
    );
    let ids = index(engine.world_mut());
    set(
        &mut engine,
        ids["pair-1/tip"],
        "PointLight",
        json!({"intensity": 9.0}),
    );
    engine
        .world_mut()
        .insert(ids["x"], Parent(ids["pair-2/arm"]))
        .unwrap();

    let read = read(&mut engine, None).unwrap();
    assert_eq!(read.len(), 2);
    assert_eq!(read[0].name.as_deref(), Some("pair-1"));
    assert!(read[0].diagnostic.is_none());
    assert_eq!(read[0].overrides["tip"], ["/PointLight/intensity"]);
    let refusal = read[1].diagnostic.as_ref().expect("pair-2 is refused");
    assert_eq!(refusal.code, code::PREFAB_INSTANCE_SAVE);
    assert!(read[1].overrides.is_empty() && !read[1].placement_differs);
    assert_eq!(read[0].root_parts, ["base"]);
    assert!(read[1].root_parts.is_empty());

    let saved = engine.scene_text().unwrap_err();
    assert_eq!(saved.code, code::PREFAB_INSTANCE_SAVE);
    assert_eq!(
        refusal.message, saved.message,
        "the read speaks the save's words"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_scoped_refusal_names_an_unnamed_entity_as_the_save_does() {
    let root = root("unnamed-position");
    let plain = "  - components:\n      Transform: {}\n";
    let scene = TWO.replacen("entities:\n", &format!("entities:\n{plain}{plain}"), 1);
    let mut engine = loaded(&root, &scene);
    let ids = index(engine.world_mut());
    let tip = *spawn(&mut engine, None, None).last().unwrap();
    engine.world_mut().remove::<Name>(tip).unwrap();
    engine
        .world_mut()
        .insert(tip, Parent(ids["pair-2/arm"]))
        .unwrap();
    let position = engine
        .world_mut()
        .entities()
        .position(|id| id == tip)
        .unwrap();

    let read = read(&mut engine, None).unwrap();
    let refusal = read[1].diagnostic.as_ref().expect("pair-2 is refused");
    assert_eq!(refusal.code, code::PREFAB_INSTANCE_SAVE);
    let saved = engine.scene_text().unwrap_err();
    assert_eq!(refusal.message, saved.message);
    assert!(
        refusal
            .message
            .contains(&format!("the unnamed entity at position {position}")),
        "{}",
        refusal.message
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_cross_instance_reference_is_blamed_on_the_instance_that_holds_it() {
    let root = root("cross");
    let mut engine = loaded(&root, TWO);
    let ids = index(engine.world_mut());
    engine
        .world_mut()
        .insert(ids["pair-1/tip"], Parent(ids["pair-2/arm"]))
        .unwrap();

    let read = read(&mut engine, None).unwrap();
    let refusal = read[0].diagnostic.as_ref().expect("pair-1 is refused");
    assert_eq!(refusal.code, code::PREFAB_INSTANCE_SAVE);
    let saved = engine.scene_text().unwrap_err();
    assert_eq!(refusal.message, saved.message);
    assert!(read[1].diagnostic.is_none() && read[1].overrides.is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_unreadable_prefab_refuses_only_its_instance() {
    let root = root("unreadable");
    std::fs::write(root.join("pair2.prefab.yaml"), PAIR).unwrap();
    let scene = TWO.replacen("pair.prefab.yaml", "pair2.prefab.yaml", 2);
    let scene = scene.replacen("pair2.prefab.yaml", "pair.prefab.yaml", 1);
    let mut engine = loaded(&root, &scene);
    std::fs::remove_file(root.join("pair2.prefab.yaml")).unwrap();

    let read = read(&mut engine, None).unwrap();
    assert!(read[0].diagnostic.is_none());
    let refusal = read[1].diagnostic.as_ref().expect("pair-2 is refused");
    assert_eq!(refusal.code, code::PREFAB_UNRESOLVED);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_instance_hung_on_a_plain_entity_reads_clean() {
    let root = root("plain-parent");
    let mut engine = loaded(
        &root,
        &format!(
            "{TWO}  - name: x
    components:
      Transform: {{}}
"
        ),
    );
    let ids = index(engine.world_mut());
    engine
        .world_mut()
        .insert(ids["pair-1/base"], Parent(ids["x"]))
        .unwrap();

    let read = read(&mut engine, None).unwrap();
    assert!(read.iter().all(|held| held.diagnostic.is_none()));
    assert_eq!(read[0].overrides["base"], ["/Parent"]);
    assert!(read[1].overrides.is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn instances_are_named_by_any_part_and_a_plain_entity_is_refused() {
    let root = root("named");
    let mut engine = loaded(
        &root,
        &format!("{TWO}  - name: x\n    components:\n      Transform: {{}}\n"),
    );
    let ids = index(engine.world_mut());

    let one = read(
        &mut engine,
        Some(vec![ids["pair-1/arm"], ids["pair-1/tip"]]),
    )
    .unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].name.as_deref(), Some("pair-1"));
    assert_eq!(one[0].root, ids["pair-1/base"]);

    let error = read(&mut engine, Some(vec![ids["x"]])).unwrap_err();
    assert_eq!(error.code, code::PREFAB_NOT_INSTANCE);
    let _ = std::fs::remove_dir_all(&root);
}

fn revert(engine: &mut Engine, instance: EntityId, pointer: &str) {
    engine
        .execute(Command::PrefabRevert(PrefabRevertParams {
            instance,
            pointer: Some(pointer.to_owned()),
        }))
        .unwrap_or_else(|refusal| panic!("`{pointer}` is refused: {}", refusal.message));
}

#[test]
fn a_gained_component_reads_as_one_revertable_pointer() {
    let root = root("gained");
    let mut engine = loaded(&root, TWO);
    let ids = index(engine.world_mut());
    set(
        &mut engine,
        ids["pair-1/base"],
        "PointLight",
        json!({"intensity": 4.0, "color": [1.0, 0.5, 0.25]}),
    );

    let first = read(&mut engine, None).unwrap().remove(0);
    assert_eq!(first.overrides["base"], ["/PointLight"]);

    revert(&mut engine, ids["pair-1/base"], "/base/PointLight");
    let first = read(&mut engine, None).unwrap().remove(0);
    assert!(first.overrides.is_empty(), "{:?}", first.overrides);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_root_transform_override_is_listed_and_revert_leaves_it() {
    let root = root("root-pose");
    let scene = TWO.replacen(
        "    prefab: pair.prefab.yaml\n",
        "    prefab: pair.prefab.yaml\n    overrides:\n      base:\n        Transform:\n          translation:\n            - 7.0\n            - 0.0\n            - 0.0\n",
        1,
    );
    let mut engine = loaded(&root, &scene);
    let ids = index(engine.world_mut());
    let first = read(&mut engine, None).unwrap().remove(0);
    assert_eq!(first.overrides["base"], ["/Transform/translation"]);

    revert(&mut engine, ids["pair-1/base"], "/base/Transform");
    let first = read(&mut engine, None).unwrap().remove(0);
    assert_eq!(first.overrides["base"], ["/Transform/translation"]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn root_parts_are_the_prefabs_not_the_live_trees() {
    let root = root("root-parts");
    let mut engine = loaded(&root, TWO);
    let ids = index(engine.world_mut());
    let (base, arm, tip) = (ids["pair-1/base"], ids["pair-1/arm"], ids["pair-1/tip"]);
    // A save refuses a part that lost its Parent, so the live tree is rearranged by reparenting.
    engine.world_mut().insert(tip, Parent(base)).unwrap();

    let first = read(&mut engine, None).unwrap().remove(0);
    assert!(first.diagnostic.is_none(), "{:?}", first.diagnostic);
    assert_eq!(first.root_parts, ["base"]);
    assert!(
        first.overrides["tip"]
            .iter()
            .all(|p| p.starts_with("/Parent"))
    );

    revert(&mut engine, tip, "/tip/Parent");
    assert_eq!(
        engine.world_mut().get::<Parent>(tip).unwrap().unwrap().0,
        arm
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_nested_prefabs_roots_include_the_inner_root() {
    let root = root("root-parts-nested");
    std::fs::write(root.join("barrel.prefab.yaml"), BARREL).unwrap();
    std::fs::write(root.join("turret.prefab.yaml"), TURRET).unwrap();
    let scene = "format: 1\nname: s\nentities:\n  - name: t-1\n    prefab: turret.prefab.yaml\n";
    let mut engine = loaded(&root, scene);
    let first = read(&mut engine, None).unwrap().remove(0);
    assert_eq!(first.root_parts, ["base", "barrel/tube"]);
    let _ = std::fs::remove_dir_all(&root);
}

const TAGGED: &str = "\
format: 1
kind: prefab
name: tagged
entities:
  - name: base
    components:
      Transform: {}
  - name: tag
    components:
      Parent:
        index: 0
        generation: 0
      PointLight:
        intensity: 2.0
";

#[test]
fn a_transform_gained_by_a_part_reads_as_one_revertable_pointer() {
    let root = root("gained-pose");
    std::fs::write(root.join("tagged.prefab.yaml"), TAGGED).unwrap();
    let scene = "format: 1\nname: s\nentities:\n  - name: b-1\n    prefab: tagged.prefab.yaml\n";
    let mut engine = loaded(&root, scene);
    let ids = index(engine.world_mut());
    set(
        &mut engine,
        ids["b-1/tag"],
        "Transform",
        json!({"translation": [0.0, 2.0, 0.0]}),
    );

    let first = read(&mut engine, None).unwrap().remove(0);
    assert_eq!(first.overrides["tag"], ["/Transform"]);

    revert(&mut engine, ids["b-1/tag"], "/tag/Transform");
    let first = read(&mut engine, None).unwrap().remove(0);
    assert!(first.overrides.is_empty(), "{:?}", first.overrides);
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
fn the_read_is_unhashed_and_unrecorded() {
    let root = root("pure");
    let mut engine = loaded(&root, EDITED);
    let sink = SharedSink::default();
    engine.set_recording(sink.clone()).unwrap();
    let before = engine.world_hash().unwrap();
    read(&mut engine, None).unwrap();
    assert_eq!(engine.world_hash().unwrap(), before);
    engine.stop_recording().unwrap();

    let bytes = sink.0.lock().unwrap().clone();
    let records = ReplayReader::new(&bytes[..]).unwrap().records().unwrap();
    assert!(!records.iter().any(|record| matches!(
        record,
        ReplayRecord::Command {
            command: Command::PrefabOverrides(_),
            ..
        }
    )));

    // The root-parts lookup may fall back to reading prefab text; it must log none.
    let texts = |read_it: bool| {
        let mut engine = loaded(&root, EDITED);
        let sink = SharedSink::default();
        engine.set_recording(sink.clone()).unwrap();
        if read_it {
            read(&mut engine, None).unwrap();
        }
        engine.stop_recording().unwrap();
        let bytes = sink.0.lock().unwrap().clone();
        let records = ReplayReader::new(&bytes[..]).unwrap().records().unwrap();
        records
            .iter()
            .filter(|record| matches!(record, ReplayRecord::PrefabText { .. }))
            .count()
    };
    assert_eq!(texts(true), texts(false), "the read records no prefab text");
    let _ = std::fs::remove_dir_all(&root);
}

/// `prefab.overrides` on a vigil-sized scene (50 instances beside 3000 plain entities), the OK case and
/// the refused one, where the read collapses once more per instance. Run with
/// `cargo test --release -p ludion-engine --test prefab_overrides_wire -- --ignored --nocapture`.
#[test]
#[ignore = "timing probe"]
#[allow(clippy::disallowed_methods)] // wall clock: a host-side timing probe, not sim
fn cost_of_the_read_on_a_vigil_sized_scene() {
    let root = root("cost");
    let mut scene = String::from("format: 1\nname: s\nentities:\n");
    for n in 0..50 {
        scene.push_str(&format!(
            "  - name: pair-{n}\n    prefab: pair.prefab.yaml\n    transform:\n      translation:\n        - {n}.0\n        - 0.0\n        - 0.0\n"
        ));
    }
    for n in 0..3000 {
        scene.push_str(&format!(
            "  - name: plain-{n}\n    components:\n      Transform:\n        translation:\n          - {n}.0\n          - 0.0\n          - 0.0\n"
        ));
    }
    let mut engine = loaded(&root, &scene);
    let timed = |engine: &mut Engine| {
        let runs = 10;
        let start = std::time::Instant::now();
        for _ in 0..runs {
            assert_eq!(read(engine, None).unwrap().len(), 50);
        }
        start.elapsed().as_secs_f64() * 1000.0 / f64::from(runs)
    };
    let cold = std::time::Instant::now();
    assert_eq!(read(&mut engine, None).unwrap().len(), 50);
    println!(
        "prefab.overrides, first read after load: {:.2} ms",
        cold.elapsed().as_secs_f64() * 1000.0
    );
    println!("prefab.overrides, ok: {:.2} ms", timed(&mut engine));

    let ids = index(engine.world_mut());
    engine
        .world_mut()
        .insert(ids["plain-0"], Parent(ids["pair-49/arm"]))
        .unwrap();
    println!("prefab.overrides, refused: {:.2} ms", timed(&mut engine));
    let _ = std::fs::remove_dir_all(&root);
}
