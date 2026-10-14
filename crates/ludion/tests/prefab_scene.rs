//! Behavioral proof for prefab instances in scenes (`crates/ludion/examples/prefab_scene.rs`): a
//! scene places a lamp prefab twice, `scene_text` collapses the live parts back into the same
//! entries, and apply, revert and unpack through the facade each leave a save that reloads.
//!
//! The content here mirrors the example's.

use ludion::prelude::*;
use std::collections::BTreeMap;
use std::path::PathBuf;

const LAMP_PATH: &str = "prefabs/lamp.prefab.yaml";

const LAMP: &str = "\
format: 1
kind: prefab
name: lamp
entities:
  - name: post
    components:
      Transform: {}
  - name: bulb
    components:
      Parent:
        index: 0
        generation: 0
      Transform:
        translation:
          - 0.0
          - 2.0
          - 0.0
      PointLight:
        intensity: 1.0
";

const SCENE: &str = "\
format: 1
name: lamps
entities:
  - name: lamp-a
    prefab: prefabs/lamp.prefab.yaml
    transform:
      translation:
        - -3.0
        - 0.0
        - 0.0
  - name: lamp-b
    prefab: prefabs/lamp.prefab.yaml
    transform:
      translation:
        - 3.0
        - 0.0
        - 0.0
    overrides:
      bulb:
        PointLight:
          intensity: 9.0
";

fn content(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "ludion-prefab-scene-test-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("prefabs")).unwrap();
    std::fs::write(root.join(LAMP_PATH), LAMP).unwrap();
    root
}

fn load(root: &std::path::Path, scene: &str) -> Engine {
    let mut engine = Engine::builder().project_root(root).build().unwrap();
    engine.load_inline_scene(scene).expect("the scene loads");
    engine
}

fn named(engine: &mut Engine) -> BTreeMap<String, EntityId> {
    engine
        .world_mut()
        .query_ids::<(&Name,)>()
        .map(|(id, (name,))| (name.0.clone(), id))
        .collect()
}

fn intensity(engine: &mut Engine, name: &str) -> f32 {
    let id = named(engine)[name];
    engine
        .world_mut()
        .get::<PointLight>(id)
        .unwrap()
        .expect("a light")
        .intensity
}

#[test]
fn instances_expand_to_named_parts_and_collapse_back() {
    let root = content("expand");
    let mut engine = load(&root, SCENE);
    let names: Vec<String> = named(&mut engine).into_keys().collect();
    assert_eq!(
        names,
        ["lamp-a/bulb", "lamp-a/post", "lamp-b/bulb", "lamp-b/post"]
    );
    assert_eq!(intensity(&mut engine, "lamp-a/bulb"), 1.0);
    assert_eq!(intensity(&mut engine, "lamp-b/bulb"), 9.0);
    assert_eq!(engine.scene_text().unwrap(), SCENE);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn apply_writes_the_prefab_and_leaves_the_instance_clean() {
    let root = content("apply");
    let mut engine = load(&root, SCENE);
    let ids = named(&mut engine);
    let brighter = PointLight {
        intensity: 3.0,
        ..PointLight::default()
    };
    engine
        .world_mut()
        .insert(ids["lamp-a/bulb"], brighter)
        .unwrap();
    let applied = engine.apply_prefab(ids["lamp-a/post"], None).unwrap();
    assert_eq!(applied.parts, ["bulb"]);
    let prefab = std::fs::read_to_string(root.join(LAMP_PATH)).unwrap();
    assert!(prefab.contains("intensity: 3.0"), "{prefab}");

    let saved = engine.scene_text().unwrap();
    assert_eq!(saved, SCENE, "lamp-a is clean, lamp-b keeps its override");
    let mut reloaded = load(&root, &saved);
    assert_eq!(intensity(&mut reloaded, "lamp-a/bulb"), 3.0);
    assert_eq!(intensity(&mut reloaded, "lamp-b/bulb"), 9.0);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn revert_drops_an_override_and_unpack_saves_plain_entities() {
    let root = content("revert");
    let mut engine = load(&root, SCENE);
    let ids = named(&mut engine);

    let reverted = revert_prefab(engine.world_mut(), ids["lamp-b/post"], None).unwrap();
    assert_eq!(reverted.entities, [ids["lamp-b/bulb"]]);
    assert_eq!(intensity(&mut engine, "lamp-b/bulb"), 1.0);
    assert!(!engine.scene_text().unwrap().contains("overrides"));

    let parts = unpack_prefab(engine.world_mut(), ids["lamp-a/post"]).unwrap();
    assert_eq!(parts.len(), 2);
    let saved = engine.scene_text().unwrap();
    assert!(saved.contains("name: lamp-a/post"), "{saved}");
    assert!(saved.contains("name: lamp-a/bulb"), "{saved}");
    assert_eq!(saved.matches("prefab:").count(), 1, "{saved}");
    let _ = std::fs::remove_dir_all(root);
}
