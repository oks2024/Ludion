//! Operations over live prefab instances: `create_prefab`, `revert_prefab`, `unpack_prefab` and the
//! `L1235`–`L1238` refusals. Everything runs over a prefab tree written to a per-test temp root.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_ecs::{Parent, World};
use ludion_engine::{
    Engine, Mesh, MeshSource, Name, PointLight, PrefabInstance, Transform, builtin_registry,
    revert_prefab, spawn_prefab, unpack_prefab,
};

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

const SCENE: &str =
    "format: 1\nname: s\nentities:\n  - name: pair-1\n    prefab: pair.prefab.yaml\n";

fn root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-prefab-ops-{tag}-{}", std::process::id()));
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

fn set_readonly(path: &Path, readonly: bool) {
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(readonly);
    std::fs::set_permissions(path, permissions).unwrap();
}

fn loaded(root: &Path, scene: &str) -> Engine {
    let mut engine = Engine::builder().project_root(root).build().unwrap();
    engine.load_inline_scene(scene).expect("the scene loads");
    engine
}

/// Every named entity, by name.
fn index(world: &mut World) -> BTreeMap<String, EntityId> {
    world
        .query_ids::<(&Name,)>()
        .map(|(id, (name,))| (name.0.clone(), id))
        .collect()
}

/// `source` in the canonical form the prefab writer produces.
fn canonical(source: &str) -> String {
    let registry = builtin_registry();
    let doc = ludion_scene::parse_prefab(source).expect("the source parses");
    ludion_scene::emit_prefab(&doc, &registry)
}

fn light(world: &World, id: EntityId) -> Option<PointLight> {
    world.get::<PointLight>(id).unwrap().copied()
}

fn translation(world: &World, id: EntityId) -> [f32; 3] {
    world.get::<Transform>(id).unwrap().unwrap().translation
}

fn bright(intensity: f32) -> PointLight {
    PointLight {
        intensity,
        ..PointLight::default()
    }
}

const SET: &str = "\
format: 1
name: s
entities:
  - name: post
    components:
      Transform:
        translation:
          - 1.0
          - 0.0
          - 0.0
  - name: arm
    components:
      Parent:
        index: 0
        generation: 0
      PointLight:
        intensity: 3.0
  - name: weld
    components:
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

/// What `create_prefab` writes for `SET`: every component in full, parts in the order given, the
/// references renumbered to positions in the set.
const SET_PREFAB: &str = "\
format: 1
kind: prefab
entities:
  - name: post
    components:
      Transform:
        translation:
          - 1.0
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
  - name: arm
    components:
      PointLight:
        color:
          - 1.0
          - 1.0
          - 1.0
        intensity: 3.0
        range: 10.0
      Parent:
        index: 0
        generation: 0
  - name: weld
    components:
      Joint:
        a:
          index: 0
          generation: 0
        b:
          index: 1
          generation: 0
        kind:
          fixed: {}
        anchor:
          - 0.0
          - 0.0
          - 0.0
        collide: false
";

#[test]
fn create_writes_a_prefab_that_spawns_the_same_values() {
    let root = root("create");
    let mut engine = loaded(&root, SET);
    let ids = index(engine.world_mut());
    let set = [ids["post"], ids["arm"], ids["weld"]];
    let before = engine.scene_text().unwrap();

    let key = engine
        .create_prefab(&set, "set.prefab.yaml")
        .expect("creates");
    assert_eq!(key, "set.prefab.yaml");
    assert_eq!(
        engine.scene_text().unwrap(),
        before,
        "the world is unchanged"
    );

    let text = std::fs::read_to_string(root.join(&key)).unwrap();
    assert_eq!(text, canonical(SET_PREFAB));

    let parts = spawn_prefab(engine.world_mut(), &key, None, Some("set-1")).expect("spawns");
    let world = engine.world_mut();
    assert_eq!(translation(world, parts[0]), [1.0, 0.0, 0.0]);
    assert_eq!(world.get::<Parent>(parts[1]).unwrap().unwrap().0, parts[0]);
    assert_eq!(light(world, parts[1]).map(|l| l.intensity), Some(3.0));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn create_refuses_what_a_prefab_cannot_hold_with_l1236() {
    let root = root("create-refused");
    let mut engine = loaded(&root, SET);
    let ids = index(engine.world_mut());
    let (post, arm, weld) = (ids["post"], ids["arm"], ids["weld"]);
    let before = engine.scene_text().unwrap();

    let refused = |engine: &Engine, set: &[EntityId], path: &str| {
        let found = engine.create_prefab(set, path).unwrap_err();
        assert_eq!(found[0].code, code::PREFAB_CREATE, "{}", found[0].message);
        found[0].message.clone()
    };
    // `arm` is parented to `post`, which is not in the set.
    let message = refused(&engine, &[arm], "x.prefab.yaml");
    assert!(message.contains("not in `entities`"), "{message}");
    assert!(message.contains("`arm`"), "{message}");
    assert!(refused(&engine, &[], "x.prefab.yaml").contains("no entities"));
    assert!(refused(&engine, &[post, post], "x.prefab.yaml").contains("twice"));
    assert!(refused(&engine, &[post], "x.yaml").contains(".prefab.yaml"));

    let anonymous = engine.world_mut().spawn();
    assert!(refused(&engine, &[anonymous], "x.prefab.yaml").contains("no `Name`"));
    engine
        .world_mut()
        .insert(anonymous, Name("post".into()))
        .unwrap();
    assert!(refused(&engine, &[post, anonymous], "x.prefab.yaml").contains("two entities"));
    engine
        .world_mut()
        .insert(anonymous, Name("a/b".into()))
        .unwrap();
    assert!(refused(&engine, &[anonymous], "x.prefab.yaml").contains("`/`"));
    let _ = weld;

    assert!(!root.join("x.prefab.yaml").exists());
    engine.world_mut().despawn(anonymous).unwrap();
    assert_eq!(engine.scene_text().unwrap(), before);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn create_refuses_to_overwrite_a_generated_prefab_with_l1236() {
    let root = root("create-generated");
    let generated = "format: 1\nkind: prefab\nname: set\ngenerated_from: ../set.gltf\nentities:\n  - name: old\n    components: {}\n";
    std::fs::write(root.join("set.prefab.yaml"), generated).unwrap();
    let mut engine = loaded(&root, SET);
    let ids = index(engine.world_mut());
    let found = engine
        .create_prefab(&[ids["post"], ids["arm"], ids["weld"]], "set.prefab.yaml")
        .unwrap_err();
    assert_eq!(found[0].code, code::PREFAB_CREATE, "{}", found[0].message);
    assert!(
        found[0].message.contains("../set.gltf"),
        "{}",
        found[0].message
    );
    assert_eq!(
        std::fs::read_to_string(root.join("set.prefab.yaml")).unwrap(),
        generated
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn create_overwrites_an_existing_file() {
    let root = root("create-overwrite");
    let mut engine = loaded(&root, SET);
    let post = index(engine.world_mut())["post"];
    std::fs::write(root.join("set.prefab.yaml"), "stale").unwrap();
    engine.create_prefab(&[post], "set.prefab.yaml").unwrap();
    let text = std::fs::read_to_string(root.join("set.prefab.yaml")).unwrap();
    assert!(text.contains("kind: prefab"), "{text}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn revert_restores_prefab_values_and_removes_a_gained_component() {
    let root = root("revert");
    let mut engine = loaded(&root, SCENE);
    let world = engine.world_mut();
    let ids = index(world);
    let (base, arm, tip) = (ids["pair-1/base"], ids["pair-1/arm"], ids["pair-1/tip"]);
    world
        .insert(
            arm,
            Transform {
                translation: [9.0, 9.0, 9.0],
                rotation: [0.0; 3],
                scale: [1.0; 3],
            },
        )
        .unwrap();
    world.insert(arm, bright(5.0)).unwrap();
    world.insert(tip, bright(9.0)).unwrap();

    let reverted = revert_prefab(world, tip, None).expect("reverts");
    assert_eq!(reverted.entities, vec![arm, tip]);
    assert!(reverted.not_restored.is_empty());
    assert_eq!(translation(world, arm), [0.0, 1.0, 0.0]);
    assert_eq!(light(world, arm), None, "the gained component is removed");
    assert_eq!(light(world, tip).map(|l| l.intensity), Some(2.0));
    assert!(world.get::<PrefabInstance>(base).unwrap().is_some());

    let again = revert_prefab(world, base, None).unwrap();
    assert!(again.entities.is_empty(), "nothing differs: {again:?}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn revert_keeps_the_placement_of_a_root() {
    let root = root("revert-root");
    let source = "format: 1\nname: s\nentities:\n  - name: pair-1\n    prefab: pair.prefab.yaml\n    transform:\n      translation:\n        - 4.0\n        - 0.0\n        - 0.0\n";
    let mut engine = loaded(&root, source);
    let world = engine.world_mut();
    let ids = index(world);
    let (base, arm) = (ids["pair-1/base"], ids["pair-1/arm"]);
    let placed = translation(world, base);
    world.insert(arm, bright(4.0)).unwrap();
    let reverted = revert_prefab(world, base, None).unwrap();
    assert_eq!(reverted.entities, vec![arm]);
    assert_eq!(translation(world, base), placed);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn revert_pointers_narrow_to_a_part_a_component_and_a_field() {
    let root = root("revert-pointer");
    let mut engine = loaded(&root, SCENE);
    let world = engine.world_mut();
    let ids = index(world);
    let (arm, tip) = (ids["pair-1/arm"], ids["pair-1/tip"]);
    let dirty = |world: &mut World| {
        world.insert(arm, bright(5.0)).unwrap();
        world
            .insert(
                tip,
                PointLight {
                    intensity: 9.0,
                    range: 99.0,
                    ..PointLight::default()
                },
            )
            .unwrap();
    };

    dirty(world);
    let reverted = revert_prefab(world, arm, Some("/arm")).unwrap();
    assert_eq!(reverted.entities, vec![arm]);
    assert_eq!(light(world, arm), None);
    assert_eq!(light(world, tip).map(|l| l.intensity), Some(9.0));

    let reverted = revert_prefab(world, arm, Some("/tip/PointLight/intensity")).unwrap();
    assert_eq!(reverted.entities, vec![tip]);
    let restored = light(world, tip).unwrap();
    assert_eq!((restored.intensity, restored.range), (2.0, 99.0));

    dirty(world);
    let reverted = revert_prefab(world, arm, Some("/arm/PointLight")).unwrap();
    assert_eq!(reverted.entities, vec![arm]);
    assert_eq!(light(world, tip).map(|l| l.intensity), Some(9.0));
    let reverted = revert_prefab(world, arm, Some("/tip/PointLight")).unwrap();
    assert_eq!(reverted.entities, vec![tip]);
    assert_eq!(
        light(world, tip),
        Some(PointLight {
            intensity: 2.0,
            ..PointLight::default()
        })
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn revert_resolves_a_pointer_into_a_nested_instance() {
    let root = root("revert-nested");
    let source =
        "format: 1\nname: s\nentities:\n  - name: turret-1\n    prefab: turret.prefab.yaml\n";
    let mut engine = loaded(&root, source);
    let world = engine.world_mut();
    let ids = index(world);
    let (tube, muzzle) = (ids["turret-1/barrel/tube"], ids["turret-1/barrel/muzzle"]);
    world.insert(muzzle, bright(7.0)).unwrap();
    world.insert(tube, bright(7.0)).unwrap();

    let reverted = revert_prefab(world, tube, Some("/barrel/muzzle/PointLight/intensity")).unwrap();
    assert_eq!(reverted.entities, vec![muzzle]);
    assert_eq!(light(world, muzzle).map(|l| l.intensity), Some(1.0));
    assert!(
        light(world, tube).is_some(),
        "the pointer left the tube alone"
    );

    let reverted = revert_prefab(world, tube, Some("/barrel/tube")).unwrap();
    assert_eq!(reverted.entities, vec![tube]);
    assert_eq!(light(world, tube), None);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn revert_does_not_respawn_a_removed_part() {
    let root = root("revert-removed");
    let mut engine = loaded(&root, SCENE);
    let world = engine.world_mut();
    let ids = index(world);
    let (base, tip) = (ids["pair-1/base"], ids["pair-1/tip"]);
    world.despawn(tip).unwrap();
    world.insert(base, bright(5.0)).unwrap();

    let reverted = revert_prefab(world, base, None).unwrap();
    assert_eq!(reverted.entities, vec![base]);
    assert_eq!(reverted.not_restored, vec!["tip".to_string()]);

    let found = revert_prefab(world, base, Some("/tip")).unwrap_err();
    assert_eq!(found[0].code, code::PREFAB_REVERT);
    assert!(
        found[0].message.contains("not re-spawned"),
        "{}",
        found[0].message
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_bad_pointer_is_l1238_and_changes_nothing() {
    let root = root("revert-bad");
    let mut engine = loaded(&root, SCENE);
    let world = engine.world_mut();
    let ids = index(world);
    let (arm, tip) = (ids["pair-1/arm"], ids["pair-1/tip"]);
    world.insert(tip, bright(9.0)).unwrap();
    let hash = engine.world_hash().unwrap();

    for pointer in [
        "arm",
        "/nope",
        "/tip/Nope",
        "/tip/PointLight/nope",
        "/arm/PointLight/intensity",
        "/arm/Transform/nope",
    ] {
        let found = revert_prefab(engine.world_mut(), arm, Some(pointer)).unwrap_err();
        assert_eq!(
            found[0].code,
            code::PREFAB_REVERT,
            "{pointer}: {}",
            found[0].message
        );
        assert!(found[0].help.is_some(), "{pointer}");
    }
    assert_eq!(engine.world_hash().unwrap(), hash);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn unpack_leaves_plain_named_entities() {
    let root = root("unpack");
    let mut engine = loaded(&root, SCENE);
    let world = engine.world_mut();
    let ids = index(world);
    let tip = ids["pair-1/tip"];

    let parts = unpack_prefab(world, tip).expect("unpacks");
    assert_eq!(
        parts,
        vec![ids["pair-1/base"], ids["pair-1/arm"], ids["pair-1/tip"]]
    );
    for id in &parts {
        assert!(world.get::<PrefabInstance>(*id).unwrap().is_none());
    }
    assert_eq!(index(world), ids, "names stay");
    let saved = engine.scene_text().expect("plain entities save");
    assert!(!saved.contains("prefab:"), "{saved}");
    assert!(saved.contains("name: pair-1/arm"), "{saved}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_plain_or_dead_entity_is_l1235_on_revert_and_unpack() {
    let root = root("not-instance");
    let source = "format: 1\nname: s\nentities:\n  - name: plain\n    components: {}\n";
    let mut engine = loaded(&root, source);
    let world = engine.world_mut();
    let plain = index(world)["plain"];
    let dead = world.spawn();
    world.despawn(dead).unwrap();

    for id in [plain, dead] {
        let found = revert_prefab(world, id, None).unwrap_err();
        assert_eq!(found[0].code, code::PREFAB_NOT_INSTANCE);
        assert!(found[0].help.is_some());
        let found = unpack_prefab(world, id).unwrap_err();
        assert_eq!(found[0].code, code::PREFAB_NOT_INSTANCE);
    }
    let _ = std::fs::remove_dir_all(&root);
}

mod apply {
    use std::sync::{Arc, Mutex};

    use ludion_core::time::FrameIndex;
    use ludion_engine::{PrefabApplied, replay_hashes_with};
    use ludion_protocol::command::{
        ComponentSetParams, Placement, PrefabSpawnParams, SimStepParams,
    };
    use ludion_protocol::replay::{ReplayReader, ReplayRecord};
    use ludion_protocol::{Command, ResponseBody};
    use ludion_sim::WorldHash;
    use serde_json::{Value, json};

    use super::*;

    const PAIR_FILE: &str = "pair.prefab.yaml";

    /// Two instances of `pair`: `a` placed, edited by overrides and missing `tip`, and `b` missing
    /// `tip` only.
    const TWO: &str = "\
format: 1
name: s
entities:
  - name: a
    prefab: pair.prefab.yaml
    transform:
      translation:
        - 3.0
        - 0.0
        - 0.0
    overrides:
      arm:
        Transform:
          translation:
            - 0.0
            - 2.0
            - 0.0
    removed:
      - tip
  - name: b
    prefab: pair.prefab.yaml
    removed:
      - tip
";

    fn pose(y: f32) -> Value {
        json!({"translation": [0.0, y, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]})
    }

    fn set_pose(engine: &mut Engine, entity: EntityId, y: f32) {
        engine
            .execute(Command::ComponentSet(ComponentSetParams {
                entity,
                component: "Transform".into(),
                value: pose(y),
            }))
            .unwrap();
    }

    fn applied(engine: &mut Engine, any: EntityId, parts: Option<&[String]>) -> PrefabApplied {
        engine.apply_prefab(any, parts).expect("applies")
    }

    fn file(root: &Path, name: &str) -> String {
        std::fs::read_to_string(root.join(name)).unwrap()
    }

    fn prefab_doc(root: &Path, name: &str) -> ludion_scene::PrefabDocument {
        ludion_scene::parse_prefab(&file(root, name)).unwrap()
    }

    fn component(doc: &ludion_scene::PrefabDocument, part: &str, name: &str) -> Option<Value> {
        let entity = doc
            .entities
            .iter()
            .find(|entity| entity.name.as_deref() == Some(part))
            .unwrap_or_else(|| panic!("no part {part}"));
        entity
            .components
            .iter()
            .find(|(held, _)| held == name)
            .map(|(_, value)| value.clone())
    }

    /// Every named entity's components, by name; references show as the target's name.
    fn snapshot(engine: &mut Engine) -> BTreeMap<String, BTreeMap<String, Value>> {
        let registry = builtin_registry();
        let names: BTreeMap<EntityId, String> = index(engine.world_mut())
            .into_iter()
            .map(|(name, id)| (id, name))
            .collect();
        let mut out = BTreeMap::new();
        for (id, name) in &names {
            let mut components = BTreeMap::new();
            for info in registry.iter() {
                if info.reserved() || info.name() == "PrefabInstance" {
                    continue;
                }
                if let Ok(Some(mut value)) = info.serialize(engine.world(), *id) {
                    for field in info.entity_refs() {
                        let slot = if field.suffix.is_empty() {
                            Some(&mut value)
                        } else {
                            value.pointer_mut(&field.suffix)
                        };
                        if let Some(slot) = slot
                            && !slot.is_null()
                        {
                            let target = EntityId::new(
                                slot["index"].as_u64().unwrap() as u32,
                                slot["generation"].as_u64().unwrap() as u32,
                            );
                            *slot = json!(names[&target]);
                        }
                    }
                    components.insert(info.name().to_owned(), value);
                }
            }
            out.insert(name.clone(), components);
        }
        out
    }

    #[test]
    fn apply_writes_the_instance_into_the_prefab_and_save_reloads_to_the_same_world() {
        let root = root("apply-gate");
        let mut engine = loaded(&root, TWO);
        let ids = index(engine.world_mut());
        set_pose(&mut engine, ids["a/arm"], 3.0);
        let before = snapshot(&mut engine);

        let report = applied(&mut engine, ids["a/base"], None);
        assert_eq!(report.path, PAIR_FILE);
        assert_eq!(report.parts, vec!["arm".to_string()]);
        assert_eq!(report.removed, vec!["tip".to_string()]);

        let doc = prefab_doc(&root, PAIR_FILE);
        assert_eq!(doc.entities.len(), 2, "tip is removed from the prefab");
        let arm = component(&doc, "arm", "Transform").unwrap();
        assert_eq!(arm["translation"], json!([0.0, 3.0, 0.0]));
        let base = component(&doc, "base", "Transform").unwrap();
        assert_eq!(
            base["translation"],
            json!([1.0, 0.0, 0.0]),
            "the placement is not written"
        );

        let text = engine.scene_text().expect("saves");
        let scene = ludion_scene::check(&text, &builtin_registry()).unwrap();
        let entry = |name: &str| {
            scene
                .entities
                .iter()
                .find(|entity| entity.name.as_deref() == Some(name))
                .and_then(|entity| entity.instance.as_ref())
                .unwrap_or_else(|| panic!("no instance {name}\n{text}"))
        };
        let a = entry("a");
        assert!(a.overrides.is_empty(), "{:?}", a.overrides);
        assert!(a.removed.is_empty(), "{:?}", a.removed);
        assert_eq!(a.transform.translation, [3.0, 0.0, 0.0]);
        let b = entry("b");
        assert!(
            b.overrides.is_empty() && b.removed.is_empty(),
            "b was expanded from the old text, so it saves clean: {:?} {:?}",
            b.overrides,
            b.removed
        );

        let mut reloaded = loaded(&root, &text);
        let after = snapshot(&mut reloaded);
        for (name, components) in &before {
            if !name.starts_with("b/") {
                assert_eq!(&after[name], components, "{name}");
            }
        }
        assert_eq!(
            after["b/arm"]["Transform"]["translation"],
            json!([0.0, 3.0, 0.0]),
            "the applied edit reaches b at the next load"
        );

        let spawned = spawn_prefab(engine.world_mut(), PAIR_FILE, None, Some("c")).unwrap();
        assert_eq!(spawned.len(), 2);
        assert_eq!(translation(engine.world(), spawned[1]), [0.0, 3.0, 0.0]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unnamed_spawn_applies() {
        let root = root("apply-unnamed");
        let mut engine = loaded(&root, "format: 1\nname: s\nentities: []\n");
        let parts = spawn_prefab(engine.world_mut(), PAIR_FILE, None, None).unwrap();
        set_pose(&mut engine, parts[1], 4.0);
        let report = applied(&mut engine, parts[2], None);
        assert_eq!(report.parts, vec!["arm".to_string()]);
        let arm = component(&prefab_doc(&root, PAIR_FILE), "arm", "Transform").unwrap();
        assert_eq!(arm["translation"], json!([0.0, 4.0, 0.0]));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_nested_override_lands_in_the_outer_prefabs_entry() {
        let root = root("apply-nested");
        let source =
            "format: 1\nname: s\nentities:\n  - name: turret-1\n    prefab: turret.prefab.yaml\n";
        let mut engine = loaded(&root, source);
        let ids = index(engine.world_mut());
        engine
            .world_mut()
            .insert(ids["turret-1/barrel/muzzle"], bright(7.0))
            .unwrap();

        let report = applied(&mut engine, ids["turret-1/base"], None);
        assert_eq!(report.path, "turret.prefab.yaml");
        assert_eq!(report.parts, vec!["barrel/muzzle".to_string()]);
        let doc = prefab_doc(&root, "turret.prefab.yaml");
        let entry = doc.entities[1].instance.as_ref().unwrap();
        let light = &entry.overrides["muzzle"];
        assert_eq!(light.len(), 1, "{light:?}");
        assert_eq!(light[0].0, "PointLight");
        assert_eq!(light[0].1["intensity"], json!(7.0));
        assert_eq!(file(&root, "barrel.prefab.yaml"), BARREL);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn parts_limits_the_write_and_an_unknown_path_is_refused() {
        let root = root("apply-parts");
        let mut engine = loaded(&root, SCENE);
        let ids = index(engine.world_mut());
        let (base, arm, tip) = (ids["pair-1/base"], ids["pair-1/arm"], ids["pair-1/tip"]);
        set_pose(&mut engine, arm, 6.0);
        engine.world_mut().insert(tip, bright(9.0)).unwrap();

        let found = engine
            .apply_prefab(base, Some(&["nope".to_string()]))
            .unwrap_err();
        assert_eq!(found[0].code, code::PREFAB_APPLY, "{}", found[0].message);
        assert!(found[0].message.contains("nope"), "{}", found[0].message);
        assert_eq!(file(&root, PAIR_FILE), PAIR, "nothing was written");

        let report = applied(&mut engine, base, Some(&["arm".to_string()]));
        assert_eq!(report.parts, vec!["arm".to_string()]);
        let doc = prefab_doc(&root, PAIR_FILE);
        assert_eq!(
            component(&doc, "arm", "Transform").unwrap()["translation"],
            json!([0.0, 6.0, 0.0])
        );
        let tip_light = component(&doc, "tip", "PointLight").unwrap();
        assert_eq!(tip_light["intensity"], json!(2.0), "tip was not applied");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_instance_under_a_plain_entity_applies_without_its_parent() {
        let root = root("apply-parented");
        let source = "format: 1\nname: s\nentities:\n  - name: holder\n    components:\n      Transform: {}\n  - name: pair-1\n    prefab: pair.prefab.yaml\n    overrides:\n      base:\n        Parent:\n          index: 0\n          generation: 0\n";
        let mut engine = loaded(&root, source);
        let ids = index(engine.world_mut());
        assert_eq!(
            engine
                .world()
                .get::<Parent>(ids["pair-1/base"])
                .unwrap()
                .unwrap()
                .0,
            ids["holder"]
        );
        set_pose(&mut engine, ids["pair-1/arm"], 5.0);

        applied(&mut engine, ids["pair-1/arm"], None);
        let doc = prefab_doc(&root, PAIR_FILE);
        assert_eq!(component(&doc, "base", "Parent"), None);
        assert_eq!(
            component(&doc, "arm", "Transform").unwrap()["translation"],
            json!([0.0, 5.0, 0.0])
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_moved_instance_writes_no_transform_for_its_anchor() {
        let root = root("apply-moved");
        let source = "format: 1\nname: s\nentities:\n  - name: pair-1\n    prefab: pair.prefab.yaml\n    transform:\n      translation:\n        - 7.0\n        - 1.0\n        - 0.0\n      rotation:\n        - 0.0\n        - 30.0\n        - 0.0\n";
        let mut engine = loaded(&root, source);
        let ids = index(engine.world_mut());
        engine
            .world_mut()
            .insert(ids["pair-1/tip"], bright(3.0))
            .unwrap();
        applied(&mut engine, ids["pair-1/base"], None);
        let doc = prefab_doc(&root, PAIR_FILE);
        let base = component(&doc, "base", "Transform").unwrap();
        assert_eq!(base["translation"], json!([1.0, 0.0, 0.0]));
        assert!(
            base.get("rotation").is_none(),
            "the prefab never gained a rotation: {base}"
        );
        assert_eq!(
            component(&doc, "tip", "PointLight").unwrap()["intensity"],
            json!(3.0)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn apply_refusals_are_l1235_and_l1237_and_change_nothing() {
        let root = root("apply-refused");
        let mut engine = loaded(
            &root,
            "format: 1\nname: s\nentities:\n  - name: plain\n    components: {}\n  - name: pair-1\n    prefab: pair.prefab.yaml\n",
        );
        let ids = index(engine.world_mut());
        let found = engine.apply_prefab(ids["plain"], None).unwrap_err();
        assert_eq!(found[0].code, code::PREFAB_NOT_INSTANCE);
        let dead = engine.world_mut().spawn();
        engine.world_mut().despawn(dead).unwrap();
        let found = engine.apply_prefab(dead, None).unwrap_err();
        assert_eq!(found[0].code, code::PREFAB_NOT_INSTANCE);

        // tip re-parented to base: an override carrying an entity reference.
        engine
            .world_mut()
            .insert(ids["pair-1/tip"], Parent(ids["pair-1/base"]))
            .unwrap();
        let found = engine.apply_prefab(ids["pair-1/tip"], None).unwrap_err();
        assert_eq!(found[0].code, code::PREFAB_APPLY, "{}", found[0].message);
        assert!(found[0].message.contains("Parent"), "{}", found[0].message);
        assert!(found[0].help.is_some());
        assert_eq!(file(&root, PAIR_FILE), PAIR);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn apply_refuses_a_generated_prefab_with_l1237() {
        let root = root("apply-generated");
        let generated = PAIR.replace("name: pair\n", "name: pair\ngenerated_from: ../pair.gltf\n");
        std::fs::write(root.join(PAIR_FILE), &generated).unwrap();
        let mut engine = loaded(
            &root,
            "format: 1\nname: s\nentities:\n  - name: pair-1\n    prefab: pair.prefab.yaml\n",
        );
        let ids = index(engine.world_mut());
        set_pose(&mut engine, ids["pair-1/arm"], 3.0);
        let found = engine.apply_prefab(ids["pair-1/arm"], None).unwrap_err();
        assert_eq!(found[0].code, code::PREFAB_APPLY, "{}", found[0].message);
        assert!(
            found[0].message.contains("generated from `../pair.gltf`"),
            "{}",
            found[0].message
        );
        assert!(!found[0].message.contains("  "), "{}", found[0].message);
        assert_eq!(file(&root, PAIR_FILE), generated);
        let _ = std::fs::remove_dir_all(&root);
    }

    const SPLIT_FILE: &str = "split.prefab.yaml";

    const SPLIT: &str = "\
format: 1
kind: prefab
name: split
entities:
  - name: left
    components:
      Transform: {}
  - name: right
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 3.0
";

    fn split_root(tag: &str) -> PathBuf {
        let dir = root(tag);
        std::fs::write(dir.join(SPLIT_FILE), SPLIT).unwrap();
        dir
    }

    fn placed() -> Transform {
        Transform {
            translation: [7.0, 1.0, 0.0],
            rotation: [10.0, 37.3, 0.0],
            scale: [1.0; 3],
        }
    }

    /// Applies an untouched two-root instance, then one with a light gained on `right`.
    fn split_applies_at_its_placement(root: &Path, engine: &mut Engine) {
        let ids = index(engine.world_mut());
        let report = applied(engine, ids["s/left"], None);
        assert!(report.parts.is_empty(), "{:?}", report.parts);
        assert!(report.removed.is_empty(), "{:?}", report.removed);
        assert_eq!(
            file(root, SPLIT_FILE),
            SPLIT,
            "an untouched instance is a no-op"
        );

        engine
            .world_mut()
            .insert(ids["s/right"], bright(4.0))
            .unwrap();
        let report = applied(engine, ids["s/left"], None);
        assert_eq!(report.parts, vec!["right".to_string()]);
        let doc = prefab_doc(root, SPLIT_FILE);
        assert_eq!(
            component(&doc, "right", "PointLight").unwrap()["intensity"],
            json!(4.0)
        );
        assert_eq!(
            component(&doc, "right", "Transform").unwrap()["translation"],
            json!([0.0, 0.0, 3.0])
        );
        assert_eq!(
            component(&doc, "right", "Transform")
                .unwrap()
                .get("rotation"),
            None
        );
        assert_eq!(component(&doc, "left", "Transform"), Some(json!({})));
    }

    #[test]
    fn a_rotated_multi_root_spawn_applies_at_its_recorded_placement() {
        let root = split_root("apply-split-spawn");
        let mut engine = loaded(&root, "format: 1\nname: e\nentities: []\n");
        spawn_prefab(engine.world_mut(), SPLIT_FILE, Some(placed()), Some("s")).unwrap();
        split_applies_at_its_placement(&root, &mut engine);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_rotated_multi_root_scene_instance_applies_at_its_entry_transform() {
        let root = split_root("apply-split-scene");
        let scene = "format: 1\nname: e\nentities:\n  - name: s\n    prefab: split.prefab.yaml\n    transform:\n      translation:\n        - 7.0\n        - 1.0\n        - 0.0\n      rotation:\n        - 10.0\n        - 37.3\n        - 0.0\n";
        let mut engine = loaded(&root, scene);
        split_applies_at_its_placement(&root, &mut engine);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn removing_every_part_of_a_nested_entry_removes_the_entry() {
        let root = root("apply-nested-gone");
        std::fs::write(
            root.join("inner.prefab.yaml"),
            "format: 1\nkind: prefab\nname: inner\nentities:\n  - name: x\n    components:\n      Transform: {}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("outer.prefab.yaml"),
            "format: 1\nkind: prefab\nname: outer\nentities:\n  - name: base\n    components:\n      Transform: {}\n  - name: sub\n    prefab: inner.prefab.yaml\n",
        )
        .unwrap();
        let mut engine = loaded(
            &root,
            "format: 1\nname: e\nentities:\n  - name: o\n    prefab: outer.prefab.yaml\n",
        );
        let ids = index(engine.world_mut());
        engine.world_mut().despawn(ids["o/sub/x"]).unwrap();
        let report = applied(&mut engine, ids["o/base"], None);
        assert_eq!(report.removed, vec!["sub/x".to_string()]);
        let doc = prefab_doc(&root, "outer.prefab.yaml");
        assert_eq!(doc.entities.len(), 1, "the sub entry is gone");
        let mut engine = loaded(&root, "format: 1\nname: e\nentities: []\n");
        let spawned =
            spawn_prefab(engine.world_mut(), "outer.prefab.yaml", None, Some("n")).unwrap();
        assert_eq!(spawned.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn emptying_a_doubly_nested_entry_inside_a_live_one_keeps_its_removed_part_paths() {
        let root = root("apply-nested-deep");
        std::fs::write(
            root.join("inner.prefab.yaml"),
            "format: 1
kind: prefab
name: inner
entities:
  - name: y
    components:
      Transform: {}
",
        )
        .unwrap();
        std::fs::write(
            root.join("mid.prefab.yaml"),
            "format: 1
kind: prefab
name: mid
entities:
  - name: x
    components:
      Transform: {}
  - name: deep
    prefab: inner.prefab.yaml
",
        )
        .unwrap();
        std::fs::write(
            root.join("outer.prefab.yaml"),
            "format: 1
kind: prefab
name: outer
entities:
  - name: base
    components:
      Transform: {}
  - name: sub
    prefab: mid.prefab.yaml
",
        )
        .unwrap();
        let mut engine = loaded(
            &root,
            "format: 1
name: e
entities:
  - name: o
    prefab: outer.prefab.yaml
",
        );
        let ids = index(engine.world_mut());
        engine.world_mut().despawn(ids["o/sub/deep/y"]).unwrap();
        let report = applied(&mut engine, ids["o/base"], None);
        assert_eq!(report.removed, vec!["sub/deep/y".to_string()]);
        let text = file(&root, "outer.prefab.yaml");
        assert!(
            text.contains(
                "removed:
      - deep/y"
            ),
            "{text}"
        );
        let mut engine = loaded(
            &root,
            "format: 1
name: e
entities: []
",
        );
        let spawned =
            spawn_prefab(engine.world_mut(), "outer.prefab.yaml", None, Some("n")).unwrap();
        assert_eq!(spawned.len(), 2, "base and sub/x remain");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failed_write_leaves_the_library_and_the_recording_as_they_were() {
        let root = root("apply-rollback");
        let sink = SharedSink::default();
        let mut engine = loaded(&root, SCENE);
        let ids = index(engine.world_mut());
        engine.set_recording(sink.clone()).unwrap();
        set_pose(&mut engine, ids["pair-1/arm"], 6.0);

        // The apply writes through a temp sibling and a rename. A read-only file refuses the rename
        // on Windows but not on Unix, where only a read-only directory refuses it; locking both
        // fails the write everywhere.
        let path = root.join(PAIR_FILE);
        set_readonly(&path, true);
        set_readonly(&root, true);
        // A privileged user (root in a container) writes through the read-only bit, so the failure
        // this test provokes cannot happen there. The probe rewrites the same bytes.
        if ludion_core::atomic::write_atomic(&path, PAIR).is_ok() {
            set_readonly(&root, false);
            set_readonly(&path, false);
            eprintln!("SKIPPED: the read-only bit does not block writes for this user");
            return;
        }
        let found = engine.apply_prefab(ids["pair-1/base"], None);
        set_readonly(&root, false);
        set_readonly(&path, false);
        let found = found.unwrap_err();
        assert_eq!(found[0].code, code::PREFAB_APPLY, "{}", found[0].message);
        assert_eq!(file(&root, PAIR_FILE), PAIR);

        engine.stop_recording().unwrap();
        let bytes = sink.0.lock().unwrap().clone();
        let reloads = ReplayReader::new(&bytes[..])
            .unwrap()
            .records()
            .unwrap()
            .into_iter()
            .filter(|record| matches!(record, ReplayRecord::PrefabText { reload: true, .. }))
            .count();
        assert_eq!(reloads, 0, "a refused apply records no reload");

        let text = engine.scene_text().expect("saves");
        let scene = ludion_scene::check(&text, &builtin_registry()).unwrap();
        let entry = scene.entities[0].instance.as_ref().unwrap();
        assert!(entry.removed.is_empty(), "{:?}", entry.removed);
        assert_eq!(entry.overrides.len(), 1, "{:?}", entry.overrides);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn apply_merges_into_an_externally_edited_file() {
        let root = root("apply-mixed");
        let mut engine = loaded(&root, SCENE);
        let ids = index(engine.world_mut());
        std::fs::write(
            root.join(PAIR_FILE),
            PAIR.replace("intensity: 2.0", "intensity: 5.0"),
        )
        .unwrap();
        set_pose(&mut engine, ids["pair-1/arm"], 6.0);
        applied(&mut engine, ids["pair-1/base"], None);
        let doc = prefab_doc(&root, PAIR_FILE);
        assert_eq!(
            component(&doc, "tip", "PointLight").unwrap()["intensity"],
            json!(5.0),
            "the external edit survives"
        );
        assert_eq!(
            component(&doc, "arm", "Transform").unwrap()["translation"],
            json!([0.0, 6.0, 0.0])
        );
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

    type Trace = Vec<(FrameIndex, WorldHash)>;

    fn step(engine: &mut Engine, trace: &mut Trace) {
        for _ in 0..2 {
            engine
                .execute(Command::SimStep(SimStepParams {
                    ticks: 1,
                    hash: false,
                }))
                .unwrap();
            trace.push((engine.frame(), engine.world_hash().unwrap()));
        }
    }

    fn restamp(engine: &mut Engine, trace: &mut Trace) {
        *trace.last_mut().unwrap() = (engine.frame(), engine.world_hash().unwrap());
    }

    fn record(root: &Path) -> (Trace, Vec<u8>, String) {
        let sink = SharedSink::default();
        let mut engine = loaded(root, TWO);
        let ids = index(engine.world_mut());
        let mut trace = vec![(engine.frame(), engine.world_hash().unwrap())];
        engine.set_recording(sink.clone()).unwrap();
        step(&mut engine, &mut trace);
        set_pose(&mut engine, ids["a/arm"], 3.0);
        restamp(&mut engine, &mut trace);
        step(&mut engine, &mut trace);
        applied(&mut engine, ids["a/base"], None);
        step(&mut engine, &mut trace);
        let command = Command::PrefabSpawn(PrefabSpawnParams {
            path: PAIR_FILE.to_owned(),
            transform: Some(Placement {
                translation: [0.0; 3],
                rotation: [0.0; 3],
                scale: [1.0; 3],
            }),
            name: Some("c".to_owned()),
        });
        let spawned = engine.execute(command).unwrap().body;
        assert!(
            matches!(spawned, ResponseBody::PrefabSpawned { ref entities } if entities.len() == 2)
        );
        restamp(&mut engine, &mut trace);
        step(&mut engine, &mut trace);
        engine.stop_recording().unwrap();
        let applied_text = file(root, PAIR_FILE);
        (trace, sink.0.lock().unwrap().clone(), applied_text)
    }

    #[test]
    fn an_apply_session_replays_from_the_original_and_a_deleted_prefab() {
        let root = root("apply-replay");
        let (live, bytes, applied_text) = record(&root);

        let reloads: Vec<String> = ReplayReader::new(&bytes[..])
            .unwrap()
            .records()
            .unwrap()
            .into_iter()
            .filter_map(|record| match record {
                ReplayRecord::PrefabText {
                    path,
                    document,
                    reload: true,
                    ..
                } if path == PAIR_FILE => Some(document),
                _ => None,
            })
            .collect();
        assert_eq!(reloads, vec![applied_text]);

        let replay = |root: &Path| {
            replay_hashes_with(Engine::builder().project_root(root), &bytes, root).unwrap()
        };
        assert_eq!(replay(&root), live, "the applied file on disk");
        std::fs::write(root.join(PAIR_FILE), PAIR).unwrap();
        assert_eq!(replay(&root), live, "the original file restored");
        std::fs::remove_file(root.join(PAIR_FILE)).unwrap();
        assert_eq!(replay(&root), live, "a deleted file");
        let _ = std::fs::remove_dir_all(&root);
    }
}

fn mesh_asset(world: &World, id: EntityId) -> Option<String> {
    match world
        .get::<Mesh>(id)
        .unwrap()
        .map(|mesh| mesh.shape.clone())
    {
        Some(MeshSource::Asset(path)) => Some(path),
        _ => None,
    }
}

fn mesh_of(path: &str) -> Mesh {
    serde_json::from_value(serde_json::json!({ "shape": { "asset": path } })).unwrap()
}

#[test]
fn create_writes_asset_paths_relative_to_the_prefab() {
    let root = root("create-assets");
    let mut engine = loaded(
        &root,
        "format: 1\nname: s\nentities:\n  - name: lamp\n    components:\n      Mesh:\n        shape:\n          asset: kits/meshes/l.mesh.yaml\n",
    );
    let lamp = index(engine.world_mut())["lamp"];
    let key = engine
        .create_prefab(&[lamp], "kits/lamp.prefab.yaml")
        .expect("creates");
    let text = std::fs::read_to_string(root.join(&key)).unwrap();
    assert!(text.contains("meshes/l.mesh.yaml"), "{text}");
    assert!(!text.contains("kits/meshes"), "{text}");

    let parts = spawn_prefab(engine.world_mut(), &key, None, Some("lamp-1")).expect("spawns");
    assert_eq!(
        mesh_asset(engine.world_mut(), parts[0]).as_deref(),
        Some("kits/meshes/l.mesh.yaml")
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn apply_writes_asset_paths_relative_to_the_prefab() {
    let root = root("apply-assets");
    std::fs::create_dir_all(root.join("kits")).unwrap();
    std::fs::write(
        root.join("kits/lamp.prefab.yaml"),
        "format: 1\nkind: prefab\nentities:\n  - name: lamp\n    components:\n      Mesh:\n        shape:\n          asset: meshes/a.mesh.yaml\n",
    )
    .unwrap();
    let scene = "format: 1\nname: s\nentities:\n  - name: l\n    prefab: kits/lamp.prefab.yaml\n";
    let mut engine = loaded(&root, scene);
    let ids = index(engine.world_mut());
    engine
        .world_mut()
        .insert(ids["l/lamp"], mesh_of("other/m.mesh.yaml"))
        .unwrap();
    engine.apply_prefab(ids["l/lamp"], None).expect("applies");

    let text = std::fs::read_to_string(root.join("kits/lamp.prefab.yaml")).unwrap();
    assert!(text.contains("../other/m.mesh.yaml"), "{text}");

    let mut again = loaded(&root, scene);
    let lamp = index(again.world_mut())["l/lamp"];
    assert_eq!(
        mesh_asset(again.world_mut(), lamp).as_deref(),
        Some("other/m.mesh.yaml")
    );
    assert_eq!(again.scene_text().unwrap(), canonical_scene(scene));
    let _ = std::fs::remove_dir_all(&root);
}

fn canonical_scene(source: &str) -> String {
    let registry = builtin_registry();
    let doc = ludion_scene::parse(source).expect("the source parses");
    ludion_scene::emit(&doc, &registry)
}
