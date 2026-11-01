//! Scene instances in the engine: a scene holding prefab instance entries loads with consecutive,
//! named, provenance-carrying parts, nested instances compose their placements, and the wire
//! `scene.check` expands too. Everything runs over a prefab tree written to a per-test temp root.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ludion_core::ids::EntityId;
use ludion_ecs::{Parent, World};
use ludion_engine::hierarchy::world_transform;
use ludion_engine::{Engine, Name, PrefabInstance, Transform, spawn_prefab};
use ludion_protocol::command::SceneCheckParams;
use ludion_protocol::{Command, ResponseBody};

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

const SCENE: &str = "\
format: 1
name: instances
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
  - name: pair-2
    prefab: ./pair.prefab.yaml
    transform:
      translation:
        - 10.0
        - 0.0
        - 0.0
    removed:
      - tip
  - name: rider
    components:
      Transform: {}
      Parent:
        index: 1
        generation: 0
  - name: turret-1
    prefab: turret.prefab.yaml
";

fn root(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-prefab-scene-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temp content root");
    for (file, text) in [
        ("pair.prefab.yaml", PAIR),
        ("barrel.prefab.yaml", BARREL),
        ("turret.prefab.yaml", TURRET),
    ] {
        std::fs::write(dir.join(file), text).unwrap();
    }
    dir
}

fn loaded(root: &Path) -> Engine {
    let mut engine = Engine::builder().project_root(root).build().unwrap();
    engine.load_inline_scene(SCENE).expect("the scene loads");
    engine
}

/// Every named entity, by name, in id order.
fn index(world: &mut World) -> BTreeMap<String, EntityId> {
    world
        .query_ids::<(&Name,)>()
        .map(|(id, (name,))| (name.0.clone(), id))
        .collect()
}

fn at(world: &World, ids: &BTreeMap<String, EntityId>, name: &str) -> [f32; 3] {
    world_transform(world, ids[name]).unwrap().trs.translation
}

fn near(got: [f32; 3], want: [f32; 3]) {
    for (got, want) in got.iter().zip(want) {
        assert!((got - want).abs() < 1e-5, "{got:?} vs {want:?}");
    }
}

#[test]
fn a_scene_with_instances_loads_named_parts_with_provenance() {
    let root = root("load");
    let mut engine = loaded(&root);
    let world = engine.world_mut();
    let ids = index(world);
    let names: Vec<&str> = world
        .query_ids::<(&Name,)>()
        .map(|(_, (name,))| name.0.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "pair-1/base",
            "pair-1/arm",
            "pair-1/tip",
            "pair-2/base",
            "pair-2/arm",
            "rider",
            "turret-1/base",
            "turret-1/barrel/tube",
            "turret-1/barrel/muzzle",
        ]
    );

    let root_id = ids["pair-2/base"];
    let instance = world
        .get::<PrefabInstance>(ids["pair-2/arm"])
        .unwrap()
        .unwrap();
    assert_eq!(
        *instance,
        PrefabInstance {
            source: "pair.prefab.yaml".to_owned(),
            part: "arm".to_owned(),
            root: root_id,
        }
    );
    let muzzle = world
        .get::<PrefabInstance>(ids["turret-1/barrel/muzzle"])
        .unwrap()
        .unwrap();
    assert_eq!(muzzle.source, "turret.prefab.yaml");
    assert_eq!(muzzle.part, "barrel/muzzle");
    assert_eq!(muzzle.root, ids["turret-1/base"]);
    assert!(world.get::<PrefabInstance>(ids["rider"]).unwrap().is_none());

    assert_eq!(
        world.get::<Parent>(ids["rider"]).unwrap().unwrap().0,
        root_id,
        "a scene entity's reference to an instance entry lands on its first part"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn placements_compose_and_overrides_apply() {
    let root = root("poses");
    let mut engine = loaded(&root);
    let world = engine.world_mut();
    let ids = index(world);

    near(at(world, &ids, "pair-1/base"), [1.0, 0.0, 0.0]);
    near(at(world, &ids, "pair-1/arm"), [1.0, 5.0, 0.0]);
    near(at(world, &ids, "pair-2/base"), [11.0, 0.0, 0.0]);
    near(at(world, &ids, "pair-2/arm"), [11.0, 1.0, 0.0]);
    near(at(world, &ids, "turret-1/barrel/tube"), [0.0, 2.0, 0.0]);
    near(at(world, &ids, "turret-1/barrel/muzzle"), [0.0, 2.5, 0.0]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_wire_scene_check_expands_and_reports_a_bad_override() {
    let root = root("wire");
    let mut engine = loaded(&root);
    let check = |engine: &mut Engine, file: &str| match engine
        .execute(Command::SceneCheck(SceneCheckParams {
            path: file.to_owned(),
        }))
        .unwrap()
        .body
    {
        ResponseBody::SceneChecked { valid, diagnostics } => (valid, diagnostics),
        other => panic!("expected SceneChecked, got {other:?}"),
    };
    std::fs::write(root.join("good.yaml"), SCENE).unwrap();
    assert_eq!(check(&mut engine, "good.yaml"), (true, Vec::new()));

    let bad = SCENE.replace("      arm:\n", "      arn:\n");
    std::fs::write(root.join("bad.yaml"), bad).unwrap();
    let (valid, diagnostics) = check(&mut engine, "bad.yaml");
    assert!(!valid);
    assert!(
        diagnostics.iter().any(|d| d.code.as_str() == "L1233"
            && d.path.as_deref() == Some("/entities/0/overrides/arn")),
        "{diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn spawn_prefab_expands_a_nested_instance() {
    let root = root("spawn");
    let mut engine = Engine::builder().project_root(&root).build().unwrap();
    engine
        .load_inline_scene(
            "format: 1\nentities:\n  - name: camera\n    components:\n      Transform: {}\n",
        )
        .unwrap();
    let placement = Transform {
        translation: [3.0, 0.0, 0.0],
        ..Transform::default()
    };
    let ids = spawn_prefab(
        engine.world_mut(),
        "turret.prefab.yaml",
        Some(placement),
        Some("t"),
        false,
    )
    .expect("spawns");
    assert_eq!(ids.len(), 3);
    let world = engine.world_mut();
    let found = index(world);
    for (id, name) in ids
        .iter()
        .zip(["t/base", "t/barrel/tube", "t/barrel/muzzle"])
    {
        assert_eq!(found[name], *id);
    }
    let muzzle = world.get::<PrefabInstance>(ids[2]).unwrap().unwrap();
    assert_eq!(
        *muzzle,
        PrefabInstance {
            source: "turret.prefab.yaml".to_owned(),
            part: "barrel/muzzle".to_owned(),
            root: ids[0],
        }
    );
    near(at(world, &found, "t/barrel/tube"), [3.0, 2.0, 0.0]);
    near(at(world, &found, "t/barrel/muzzle"), [3.0, 2.5, 0.0]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_nested_prefab_uses_the_text_the_library_already_holds() {
    let root = root("held");
    let mut engine = Engine::builder().project_root(&root).build().unwrap();
    spawn_prefab(engine.world_mut(), "barrel.prefab.yaml", None, None, false).expect("spawns");
    let edited = format!("{BARREL}  - name: cap\n    components:\n      Transform: {{}}\n");
    std::fs::write(root.join("barrel.prefab.yaml"), edited).unwrap();
    let ids =
        spawn_prefab(engine.world_mut(), "turret.prefab.yaml", None, None, false).expect("spawns");
    assert_eq!(
        ids.len(),
        3,
        "base plus the two parts of the first-read barrel"
    );
    let _ = std::fs::remove_dir_all(&root);
}

const CAM: &str = "\
format: 1
kind: prefab
name: cam
entities:
  - name: eye
    components:
      Transform: {}
      Camera: {}
";

const TWO_CAMS: &str = "\
format: 1
name: cams
entities:
  - name: cam-1
    prefab: cam.prefab.yaml
  - name: cam-2
    prefab: cam.prefab.yaml
";

fn l1229_entries(diagnostics: &[ludion_core::diagnostics::Diagnostic]) -> Vec<String> {
    diagnostics
        .iter()
        .filter(|d| d.code.as_str() == "L1229")
        .filter_map(|d| d.path.clone())
        .collect()
}

#[test]
fn a_singleton_in_an_expanded_instance_is_l1229_at_the_entry() {
    let root = root("singleton");
    std::fs::write(root.join("cam.prefab.yaml"), CAM).unwrap();
    let registry = ludion_engine::builtin_registry();
    let scene = ludion_scene::check(TWO_CAMS, &registry).unwrap();
    let errors = ludion_engine::expand_scene(&root, &registry, &scene).unwrap_err();
    assert_eq!(l1229_entries(&errors), ["/entities/0", "/entities/1"]);
    assert!(errors[0].message.contains("eye"), "{}", errors[0].message);

    let mut engine = Engine::builder().project_root(&root).build().unwrap();
    let errors = engine.load_inline_scene(TWO_CAMS).unwrap_err();
    assert_eq!(l1229_entries(&errors), ["/entities/0", "/entities/1"]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_override_adding_a_singleton_is_l1229() {
    let root = root("singleton-override");
    let registry = ludion_engine::builtin_registry();
    let scene = ludion_scene::check(
        "format: 1\nname: sun\nentities:\n  - name: pair-1\n    prefab: pair.prefab.yaml\n    \
         overrides:\n      arm:\n        DirectionalLight: {}\n",
        &registry,
    )
    .unwrap();
    let errors = ludion_engine::expand_scene(&root, &registry, &scene).unwrap_err();
    assert_eq!(l1229_entries(&errors), ["/entities/0"]);
    let _ = std::fs::remove_dir_all(&root);
}
