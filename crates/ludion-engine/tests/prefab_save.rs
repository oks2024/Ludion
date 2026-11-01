//! Saving live prefab instances: `scene_text` writes each named instance back as an instance entry
//! that loads to the same world, and `L1234` refuses what an entry cannot express. Everything runs
//! over a prefab tree written to a per-test temp root.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_ecs::{Parent, World};
use ludion_engine::hierarchy::world_transform;
use ludion_engine::{
    Engine, Name, PointLight, PrefabInstance, Transform, builtin_registry, spawn_prefab,
};
use ludion_protocol::command::{AssetReloadParams, EngineRestoreParams};
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
      PointLight:
        intensity: 2.0
";

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
          - 2.0
          - 0.0
          - 0.0
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

const ROOTS: &str = "\
format: 1
kind: prefab
name: roots
entities:
  - name: a
    components: {}
  - name: b
    components: {}
";

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
      Transform: {}
";

const TURRET_TURNED: &str = "\
format: 1
kind: prefab
name: turret-turned
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
      rotation:
        - 0.0
        - 37.3
        - 0.0
";

const FADING: &str = "\
format: 1
kind: prefab
name: fading
entities:
  - name: fader
    components:
      Animation:
        from: walk
";

const PLACED_SPLIT: &str = "format: 1\nname: s\nentities:\n  - name: s-1\n    prefab: split.prefab.yaml\n    transform:\n      translation:\n        - 7.0\n        - 1.0\n        - 0.0\n      rotation:\n        - 10.0\n        - 37.3\n        - 0.0\n";

fn root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-prefab-save-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temp content root");
    for (file, text) in [
        ("pair.prefab.yaml", PAIR),
        ("split.prefab.yaml", SPLIT),
        ("barrel.prefab.yaml", BARREL),
        ("turret.prefab.yaml", TURRET),
        ("roots.prefab.yaml", ROOTS),
        ("lamp.prefab.yaml", LAMP),
        ("turret-turned.prefab.yaml", TURRET_TURNED),
        ("fading.prefab.yaml", FADING),
    ] {
        std::fs::write(dir.join(file), text).unwrap();
    }
    dir
}

fn loaded(root: &Path, scene: &str) -> Engine {
    let mut engine = Engine::builder().project_root(root).build().unwrap();
    engine.load_inline_scene(scene).expect("the scene loads");
    engine
}

/// `source` in the canonical form the writer produces.
fn canonical(source: &str) -> String {
    let scene = ludion_scene::parse(source).expect("the source parses");
    ludion_scene::emit(&scene, &builtin_registry())
}

/// Every named entity, by name.
fn index(world: &mut World) -> BTreeMap<String, EntityId> {
    world
        .query_ids::<(&Name,)>()
        .map(|(id, (name,))| (name.0.clone(), id))
        .collect()
}

/// Loads `source`, saves it, and checks the save is `expected` (the source, canonical, when `None`)
/// and a fixed point of load then save.
fn round_trip(root: &Path, source: &str, expected: Option<&str>) -> String {
    let saved = loaded(root, source).scene_text().expect("the world saves");
    assert_eq!(saved, canonical(expected.unwrap_or(source)));
    assert_eq!(
        loaded(root, &saved).scene_text().expect("it saves again"),
        saved
    );
    saved
}

fn near(got: [f32; 3], want: [f32; 3]) {
    for (got, want) in got.iter().zip(want) {
        assert!((got - want).abs() < 1e-4, "{got:?} vs {want:?}");
    }
}

/// An `f32` component saves as its short decimal, not the widened `0.30000001192092896`.
#[test]
fn f32_components_and_placements_save_as_short_decimals() {
    let root = root("f32-short");
    let saved = round_trip(
        &root,
        "format: 1\nname: s\nentities:\n  - name: crate\n    components:\n      Transform:\n        translation:\n          - 0.3\n          - 0.9\n          - 1.0e-7\n        rotation:\n          - 0.0\n          - 0.0\n          - 0.0\n        scale:\n          - 0.3\n          - 1.1\n          - 1.0\n",
        None,
    );
    assert!(
        saved.contains("- 0.3\n") && saved.contains("- 1e-7\n"),
        "{saved}"
    );
    assert!(!saved.contains("0.3000000"), "{saved}");
    let placed = round_trip(
        &root,
        "format: 1\nname: s\nentities:\n  - name: pair-1\n    prefab: pair.prefab.yaml\n    transform:\n      translation:\n        - 0.3\n        - 0.0\n        - 0.0\n",
        None,
    );
    assert!(!placed.contains("0.3000000"), "{placed}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_instance_without_overrides_saves_as_its_entry() {
    let root = root("plain");
    round_trip(
        &root,
        "format: 1\nname: s\nentities:\n  - name: pair-1\n    prefab: pair.prefab.yaml\n",
        None,
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_placement_overrides_and_removals_round_trip() {
    let root = root("authored");
    round_trip(
        &root,
        "\
format: 1
name: s
entities:
  - name: pair-1
    prefab: pair.prefab.yaml
    transform:
      translation:
        - 4.0
        - 0.0
        - 0.0
      rotation:
        - 0.0
        - 90.0
        - 0.0
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
    prefab: ./pair.prefab.yaml
    removed:
      - tip
  - name: rider
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
      Parent:
        index: 1
        generation: 0
",
        None,
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_nested_instance_keeps_an_override_on_an_inner_part() {
    let root = root("nested");
    round_trip(
        &root,
        "\
format: 1
name: s
entities:
  - name: turret-1
    prefab: turret.prefab.yaml
    transform:
      translation:
        - 0.0
        - 0.0
        - 6.0
    overrides:
      barrel/muzzle:
        PointLight:
          intensity: 4.0
",
        None,
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_runtime_component_change_becomes_an_override() {
    let root = root("component");
    let source = "format: 1\nname: s\nentities:\n  - name: pair-1\n    prefab: pair.prefab.yaml\n";
    let mut engine = loaded(&root, source);
    let world = engine.world_mut();
    let tip = index(world)["pair-1/tip"];
    world
        .insert(
            tip,
            PointLight {
                intensity: 9.0,
                ..PointLight::default()
            },
        )
        .unwrap();
    let saved = engine.scene_text().expect("saves");
    let expected = canonical(&format!(
        "{source}    overrides:\n      tip:\n        PointLight:\n          intensity: 9.0\n"
    ));
    assert_eq!(saved, expected);

    let mut again = loaded(&root, &saved);
    let tip = index(again.world_mut())["pair-1/tip"];
    let light = again.world_mut().get::<PointLight>(tip).unwrap().copied();
    assert_eq!(light.map(|l| l.intensity), Some(9.0));
    assert_eq!(again.scene_text().unwrap(), saved);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_moved_instance_saves_a_new_transform_and_reaches_a_fixed_point() {
    let root = root("moved");
    let source = "format: 1\nname: s\nentities:\n  - name: pair-1\n    prefab: pair.prefab.yaml\n";
    let mut engine = loaded(&root, source);
    let parts = ["pair-1/base", "pair-1/arm", "pair-1/tip"];
    let mut live_poses = Vec::new();
    let before: Vec<[f32; 3]> = {
        let world = engine.world_mut();
        let ids = index(world);
        world
            .insert(
                ids["pair-1/base"],
                Transform {
                    translation: [5.0, 0.0, 2.0],
                    rotation: [0.0, 90.0, 0.0],
                    scale: [1.0; 3],
                },
            )
            .unwrap();
        for name in parts {
            live_poses.push(world.get::<Transform>(ids[name]).unwrap().copied());
        }
        parts
            .map(|name| world_transform(world, ids[name]).unwrap().trs.translation)
            .to_vec()
    };
    let saved = engine.scene_text().expect("saves");
    assert!(saved.contains("transform:"), "{saved}");
    assert!(
        !saved.contains("overrides:"),
        "the anchor is explained by the placement: {saved}"
    );

    let mut second = loaded(&root, &saved);
    let world = second.world_mut();
    let ids = index(world);
    for ((name, want), pose) in parts.iter().zip(before).zip(&live_poses) {
        near(
            world_transform(world, ids[*name]).unwrap().trs.translation,
            want,
        );
        assert_eq!(
            &world.get::<Transform>(ids[*name]).unwrap().copied(),
            pose,
            "{name}: the reloaded local pose equals the live one bitwise"
        );
    }
    let resaved = second.scene_text().expect("saves again");
    assert_eq!(resaved, saved);
    assert_eq!(loaded(&root, &resaved).scene_text().unwrap(), resaved);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_destroyed_part_is_removed_and_the_next_part_becomes_first() {
    let root = root("removed");
    let pair = "format: 1\nname: s\nentities:\n  - name: pair-1\n    prefab: pair.prefab.yaml\n";
    let mut engine = loaded(&root, pair);
    let world = engine.world_mut();
    let tip = index(world)["pair-1/tip"];
    world.despawn(tip).unwrap();
    assert_eq!(
        engine.scene_text().expect("saves"),
        canonical(&format!("{pair}    removed:\n      - tip\n"))
    );

    let split = "format: 1\nname: s\nentities:\n  - name: s-1\n    prefab: split.prefab.yaml\n";
    let mut engine = loaded(&root, split);
    let world = engine.world_mut();
    let left = index(world)["s-1/left"];
    world.despawn(left).unwrap();
    let saved = engine.scene_text().expect("saves");
    assert_eq!(
        saved,
        canonical(&format!("{split}    removed:\n      - left\n"))
    );
    let mut again = loaded(&root, &saved);
    assert_eq!(again.world_mut().entity_count(), 1);
    assert_eq!(again.scene_text().unwrap(), saved);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_named_spawn_saves_as_an_entry() {
    let root = root("spawn");
    let camera = "format: 1\nentities:\n  - name: camera\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - 0.0\n          - 0.0\n        rotation:\n          - 0.0\n          - 0.0\n          - 0.0\n        scale:\n          - 1.0\n          - 1.0\n          - 1.0\n";
    let mut engine = loaded(&root, camera);
    let placement = Transform {
        translation: [3.0, 0.0, 0.0],
        ..Transform::default()
    };
    spawn_prefab(
        engine.world_mut(),
        "pair.prefab.yaml",
        Some(placement),
        Some("p"),
        false,
    )
    .expect("spawns");
    let saved = engine.scene_text().expect("saves");
    assert_eq!(
        saved,
        canonical(&format!(
            "{camera}  - name: p\n    prefab: pair.prefab.yaml\n    transform:\n      translation:\n        - 3.0\n        - 0.0\n        - 0.0\n"
        ))
    );
    assert_eq!(loaded(&root, &saved).scene_text().unwrap(), saved);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_unnamed_spawn_saves_and_reloads() {
    let root = root("unnamed");
    let mut engine = loaded(&root, "format: 1\nentities: []\n");
    spawn_prefab(engine.world_mut(), "pair.prefab.yaml", None, None, false).expect("spawns");
    let saved = engine.scene_text().expect("an unnamed spawn saves flat");
    assert!(saved.contains("PrefabInstance:"), "{saved}");

    let mut reloaded = loaded(&root, &saved);
    let world = reloaded.world_mut();
    let ids = index(world);
    for part in ["base", "arm", "tip"] {
        let instance = world
            .get::<PrefabInstance>(ids[part])
            .unwrap()
            .expect("the part keeps its PrefabInstance")
            .clone();
        assert_eq!(instance.root, ids["base"], "part `{part}`");
    }
    assert_eq!(reloaded.scene_text().unwrap(), saved);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn what_an_entry_cannot_express_is_refused_with_l1234() {
    let root = root("refused");
    let pair = "format: 1\nname: s\nentities:\n  - name: pair-1\n    prefab: pair.prefab.yaml\n  - name: x\n    components:\n      Transform: {}\n";

    let mut engine = loaded(&root, pair);
    let world = engine.world_mut();
    let ids = index(world);
    world.insert(ids["x"], Parent(ids["pair-1/arm"])).unwrap();
    let error = engine.scene_text().unwrap_err();
    assert_eq!(error.code, code::PREFAB_INSTANCE_SAVE);
    assert!(
        error.message.contains("entity `x`")
            && error.message.contains("part `arm` of instance `pair-1`"),
        "{}",
        error.message
    );
    assert!(error.help.as_deref().unwrap().contains("unpack"));

    let mut engine = loaded(&root, pair);
    let world = engine.world_mut();
    let tip = index(world)["pair-1/tip"];
    world.remove::<PointLight>(tip).unwrap();
    let error = engine.scene_text().unwrap_err();
    assert_eq!(error.code, code::PREFAB_INSTANCE_SAVE);
    assert!(error.message.contains("`PointLight`"), "{}", error.message);

    let mut engine = loaded(&root, "format: 1\nentities: []\n");
    for _ in 0..2 {
        spawn_prefab(
            engine.world_mut(),
            "pair.prefab.yaml",
            None,
            Some("p"),
            false,
        )
        .expect("spawns");
    }
    let error = engine.scene_text().unwrap_err();
    assert_eq!(error.code, code::PREFAB_INSTANCE_SAVE);
    assert!(
        error.message.contains("both named `p`"),
        "{}",
        error.message
    );
    let _ = std::fs::remove_dir_all(&root);
}

const TWO: &str = "format: 1\nname: s\nentities:\n  - name: a\n    prefab: pair.prefab.yaml\n  - name: b\n    prefab: pair.prefab.yaml\n";

/// Moves `a`'s arm, gives it a light and destroys `a`'s tip, then applies the instance to the file.
fn edit_and_apply_a(engine: &mut Engine) {
    let world = engine.world_mut();
    let ids = index(world);
    let moved = Transform {
        translation: [0.0, 3.0, 0.0],
        ..Transform::default()
    };
    world.insert(ids["a/arm"], moved).unwrap();
    let light = PointLight {
        intensity: 4.0,
        ..PointLight::default()
    };
    world.insert(ids["a/arm"], light).unwrap();
    world.despawn(ids["a/tip"]).unwrap();
    engine.apply_prefab(ids["a/base"], None).expect("applies");
}

#[test]
fn an_untouched_instance_saves_clean_after_an_apply_and_gets_the_edit_on_reload() {
    let root = root("after-apply");
    let mut engine = loaded(&root, TWO);
    edit_and_apply_a(&mut engine);

    let saved = engine.scene_text().expect("saves");
    assert_eq!(
        saved,
        canonical(TWO),
        "neither entry carries overrides or removals"
    );

    let mut again = loaded(&root, &saved);
    let world = again.world_mut();
    let ids = index(world);
    assert!(!ids.contains_key("b/tip"), "the applied removal reached b");
    let arm = world
        .get::<Transform>(ids["b/arm"])
        .unwrap()
        .copied()
        .unwrap();
    assert_eq!(arm.translation, [0.0, 3.0, 0.0]);
    let light = world.get::<PointLight>(ids["b/arm"]).unwrap().copied();
    assert_eq!(light.map(|l| l.intensity), Some(4.0));
    assert_eq!(again.scene_text().unwrap(), saved);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_external_edit_and_reload_leaves_a_spawned_instance_clean() {
    let root = root("external");
    let mut engine = loaded(&root, "format: 1\nentities: []\n");
    spawn_prefab(
        engine.world_mut(),
        "pair.prefab.yaml",
        None,
        Some("x"),
        false,
    )
    .expect("spawns");
    std::fs::write(
        root.join("pair.prefab.yaml"),
        PAIR.replace("intensity: 2.0", "intensity: 5.0"),
    )
    .unwrap();
    let reloaded = engine.execute(Command::AssetReload(AssetReloadParams {
        path: Some("pair.prefab.yaml".to_owned()),
    }));
    assert!(matches!(
        reloaded.map(|r| r.body),
        Ok(ResponseBody::AssetsReloaded { .. })
    ));

    let saved = engine.scene_text().expect("saves");
    assert_eq!(
        saved,
        canonical("format: 1\nentities:\n  - name: x\n    prefab: pair.prefab.yaml\n")
    );
    let mut again = loaded(&root, &saved);
    let tip = index(again.world_mut())["x/tip"];
    let light = again.world_mut().get::<PointLight>(tip).unwrap().copied();
    assert_eq!(light.map(|l| l.intensity), Some(5.0));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_override_on_a_part_the_applied_file_lost_is_refused_and_a_stale_removal_is_dropped() {
    let root = root("vanished");
    let mut engine = loaded(&root, TWO);
    let b_tip = index(engine.world_mut())["b/tip"];
    let light = PointLight {
        intensity: 9.0,
        ..PointLight::default()
    };
    engine.world_mut().insert(b_tip, light).unwrap();
    edit_and_apply_a(&mut engine);
    let error = engine.scene_text().unwrap_err();
    assert_eq!(error.code, code::PREFAB_INSTANCE_SAVE);
    assert!(
        error
            .message
            .contains("instance `b` overrides part `tip`, which `pair.prefab.yaml` no longer has"),
        "{}",
        error.message
    );

    let _ = std::fs::remove_dir_all(&root);
    let root = self::root("vanished-removal");
    let mut engine = loaded(&root, TWO);
    let b_tip = index(engine.world_mut())["b/tip"];
    engine.world_mut().despawn(b_tip).unwrap();
    edit_and_apply_a(&mut engine);
    let saved = engine.scene_text().expect("the stale removal is dropped");
    assert_eq!(saved, canonical(TWO));
    assert_eq!(loaded(&root, &saved).scene_text().unwrap(), saved);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_genuine_loss_after_an_apply_that_did_not_touch_it_is_still_refused() {
    let root = root("loss");
    let mut engine = loaded(&root, TWO);
    let world = engine.world_mut();
    let ids = index(world);
    let moved = Transform {
        translation: [0.0, 3.0, 0.0],
        ..Transform::default()
    };
    world.insert(ids["a/arm"], moved).unwrap();
    engine.apply_prefab(ids["a/base"], None).expect("applies");
    engine
        .world_mut()
        .remove::<PointLight>(ids["b/tip"])
        .unwrap();
    let error = engine.scene_text().unwrap_err();
    assert_eq!(error.code, code::PREFAB_INSTANCE_SAVE);
    assert!(error.message.contains("`PointLight`"), "{}", error.message);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_unplaced_spawn_without_a_root_transform_saves_and_applies() {
    let root = root("bare");
    std::fs::write(
        root.join("bare.prefab.yaml"),
        "format: 1\nkind: prefab\nname: bare\nentities:\n  - name: knob\n    components: {}\n",
    )
    .unwrap();
    let mut engine = loaded(&root, "format: 1\nentities: []\n");
    let ids = spawn_prefab(
        engine.world_mut(),
        "bare.prefab.yaml",
        None,
        Some("n"),
        false,
    )
    .unwrap();
    let saved = engine.scene_text().expect("saves");
    assert_eq!(
        saved,
        canonical("format: 1\nentities:\n  - name: n\n    prefab: bare.prefab.yaml\n")
    );
    let mut again = loaded(&root, &saved);
    let knob = index(again.world_mut())["n/knob"];
    assert!(again.world_mut().get::<Transform>(knob).unwrap().is_none());
    engine.apply_prefab(ids[0], None).expect("applies");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_restored_world_saves_a_scene_loaded_instance_from_disk() {
    let root = root("restore");
    let mut engine = loaded(&root, TWO);
    let snapshot = match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    let mut restored = Engine::builder().project_root(&root).build().unwrap();
    restored
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .expect("restores");
    let saved = restored.scene_text().expect("saves");
    assert!(saved.contains("prefab: pair.prefab.yaml"), "{saved}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Every named part's local `Transform`, by name.
fn poses(engine: &mut Engine) -> BTreeMap<String, Option<Transform>> {
    let world = engine.world_mut();
    index(world)
        .into_iter()
        .map(|(name, id)| (name, world.get::<Transform>(id).unwrap().copied()))
        .collect()
}

/// Every field's bits, so a signed zero is a difference.
fn bits(pose: &Option<Transform>) -> Option<[u32; 9]> {
    pose.map(|pose| {
        let mut bits = [0; 9];
        for (slot, value) in bits.iter_mut().zip(
            pose.translation
                .iter()
                .chain(&pose.rotation)
                .chain(&pose.scale),
        ) {
            *slot = value.to_bits();
        }
        bits
    })
}

/// The live poses and the reloaded ones agree bit for bit.
fn bitwise(
    live: &BTreeMap<String, Option<Transform>>,
    loaded: &BTreeMap<String, Option<Transform>>,
) {
    let bits = |poses: &BTreeMap<String, Option<Transform>>| -> Vec<_> {
        poses
            .iter()
            .map(|(name, pose)| (name.clone(), bits(pose)))
            .collect()
    };
    assert_eq!(bits(live), bits(loaded));
}

#[test]
fn a_signed_zero_in_a_part_pose_survives_a_save_and_reload() {
    let root = root("signed-zero");
    for source in [
        "format: 1\nname: s\nentities:\n  - name: pair-1\n    prefab: pair.prefab.yaml\n",
        PLACED_SPLIT,
    ] {
        let mut engine = loaded(&root, source);
        let ids = index(engine.world_mut());
        let (root_name, part_name) = match ids.contains_key("pair-1/base") {
            true => ("pair-1/base", "pair-1/arm"),
            false => ("s-1/left", "s-1/right"),
        };
        for name in [root_name, part_name] {
            let world = engine.world_mut();
            let mut pose = *world.get::<Transform>(ids[name]).unwrap().unwrap();
            pose.translation[0] = -0.0;
            world.insert(ids[name], pose).unwrap();
        }
        let live = poses(&mut engine);
        let saved = engine.scene_text().expect("saves");
        let mut again = loaded(&root, &saved);
        bitwise(&live, &poses(&mut again));
        assert_eq!(again.scene_text().unwrap(), saved, "{source}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_transform_added_to_an_unplaced_root_without_one_survives_a_save() {
    let root = root("added-root-transform");
    let source = "format: 1\nname: s\nentities:\n  - name: i\n    prefab: roots.prefab.yaml\n";
    let mut engine = loaded(&root, source);
    let b = index(engine.world_mut())["i/b"];
    let pose = Transform {
        translation: [0.0, 5.0, 0.0],
        ..Transform::default()
    };
    engine.world_mut().insert(b, pose).unwrap();
    let live = poses(&mut engine);
    let saved = engine.scene_text().expect("saves");
    let mut again = loaded(&root, &saved);
    bitwise(&live, &poses(&mut again));
    assert_eq!(again.scene_text().unwrap(), saved);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_parent_added_between_parts_saves_and_reloads() {
    let root = root("added-parent");
    let source = "format: 1\nname: s\nentities:\n  - name: i\n    prefab: lamp.prefab.yaml\n";
    let mut engine = loaded(&root, source);
    let ids = index(engine.world_mut());
    engine
        .world_mut()
        .insert(ids["i/bulb"], Parent(ids["i/post"]))
        .unwrap();
    let saved = engine.scene_text().expect("saves");
    let mut again = loaded(&root, &saved);
    let world = again.world_mut();
    let ids = index(world);
    assert_eq!(
        world.get::<Parent>(ids["i/bulb"]).unwrap().map(|p| p.0),
        Some(ids["i/post"])
    );
    assert_eq!(again.scene_text().unwrap(), saved);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_placed_multi_root_instance_round_trips_byte_identical() {
    let root = root("placed-split");
    round_trip(&root, PLACED_SPLIT, None);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_moved_anchor_of_a_placed_multi_root_instance_reloads_bitwise() {
    let root = root("placed-split-moved");
    let mut engine = loaded(&root, PLACED_SPLIT);
    let left = index(engine.world_mut())["s-1/left"];
    let pose = Transform {
        translation: [3.0, 0.0, 1.0],
        rotation: [10.0, 37.3, 0.0],
        scale: [1.0; 3],
    };
    engine.world_mut().insert(left, pose).unwrap();
    let live = poses(&mut engine);
    let saved = engine.scene_text().expect("saves");
    let mut again = loaded(&root, &saved);
    bitwise(&live, &poses(&mut again));
    assert_eq!(again.scene_text().unwrap(), saved);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_rotated_moved_instance_keeps_its_transform_form() {
    let root = root("moved-rotated");
    let source = "format: 1\nname: s\nentities:\n  - name: pair-1\n    prefab: pair.prefab.yaml\n";
    let mut engine = loaded(&root, source);
    let base = index(engine.world_mut())["pair-1/base"];
    let pose = Transform {
        translation: [5.0, 0.0, 2.0],
        rotation: [0.0, 90.0, 0.0],
        scale: [1.0; 3],
    };
    engine.world_mut().insert(base, pose).unwrap();
    let live = poses(&mut engine);
    let saved = engine.scene_text().expect("saves");
    assert!(saved.contains("transform:"), "{saved}");
    bitwise(&live, &poses(&mut loaded(&root, &saved)));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_nested_root_under_a_turned_placement_settles_after_one_save() {
    let root = root("nested-settle");
    let source =
        "format: 1\nname: s\nentities:\n  - name: t\n    prefab: turret-turned.prefab.yaml\n";
    let mut engine = loaded(&root, source);
    let base = index(engine.world_mut())["t/base"];
    let pose = Transform {
        translation: [4.0, 0.0, 0.0],
        rotation: [0.0, 12.0, 0.0],
        scale: [1.0; 3],
    };
    engine.world_mut().insert(base, pose).unwrap();
    let saved = engine.scene_text().expect("saves without L1234");
    let resaved = loaded(&root, &saved).scene_text().expect("saves again");
    assert_eq!(
        loaded(&root, &resaved).scene_text().unwrap(),
        resaved,
        "settled after one save"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_unnamed_spawn_whose_first_part_died_saves_with_the_next_part_as_root() {
    let root = root("unnamed-first-died");
    let mut engine = loaded(&root, "format: 1\nentities: []\n");
    let ids =
        spawn_prefab(engine.world_mut(), "split.prefab.yaml", None, None, false).expect("spawns");
    engine.world_mut().despawn(ids[0]).unwrap();
    let saved = engine.scene_text().expect("saves");
    let mut again = loaded(&root, &saved);
    let world = again.world_mut();
    let ids = index(world);
    let held = world.get::<PrefabInstance>(ids["right"]).unwrap().unwrap();
    assert_eq!(held.root, ids["right"]);
    assert_eq!(again.scene_text().unwrap(), saved);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_component_field_returned_to_its_default_saves_and_reloads() {
    let root = root("merge-null");
    let source = "format: 1\nname: s\nentities:\n  - name: f\n    prefab: fading.prefab.yaml\n";
    let mut engine = loaded(&root, source);
    let fader = index(engine.world_mut())["f/fader"];
    let mut animation = engine
        .world_mut()
        .get::<ludion_engine::Animation>(fader)
        .unwrap()
        .cloned()
        .expect("the prefab gives it an Animation");
    assert_eq!(animation.from, "walk");
    animation.from = String::new();
    engine.world_mut().insert(fader, animation).unwrap();
    let saved = engine.scene_text().expect("saves");
    let mut again = loaded(&root, &saved);
    let fader = index(again.world_mut())["f/fader"];
    let held = again
        .world_mut()
        .get::<ludion_engine::Animation>(fader)
        .unwrap()
        .cloned()
        .unwrap();
    assert_eq!(held.from, "");
    assert_eq!(again.scene_text().unwrap(), saved);
    let _ = std::fs::remove_dir_all(&root);
}
