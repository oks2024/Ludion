//! Dispatch-level tests: one happy path and at least one diagnostic path per command family, plus
//! the snapshot/restore determinism test and the replay round-trip hash-sequence test.
//!
//! These exercise the parity gate and handlers with **no GPU** (screenshot/timings determinism lives
//! in `gpu_dispatch.rs`, which graceful-skips on adapterless machines).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::rng::{DeterministicRng, WorldSeed};
use ludion_engine::{
    Engine, GiRebakeMode, Quality, QualityParams, QualitySetting, Transform, dispatch,
    drive_hashes, replay_hashes,
};
use ludion_protocol::command::SubscribeMode;
use ludion_protocol::command::{
    AssetReloadParams, ComponentGetParams, ComponentRemoveParams, ComponentSetParams,
    EngineRestoreParams, EntityDestroyParams, EntityReparentParams, EntitySpawnParams,
    InputInjectParams, QueryEntitiesParams, ReparentKeep, SceneCheckParams, SceneLoadParams,
    SceneSaveParams, SimSetSpeedParams, SimStepParams, StreamSubscribeParams,
    StreamUnsubscribeParams,
};
use ludion_protocol::frame::FrameFormat;
use ludion_protocol::restore::RestoreEntry;
use ludion_protocol::{Channel, Command, ResponseBody, SubId};
use serde_json::{Value, json};

// --- Helpers -------------------------------------------------------------------------------------

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh temp directory to use as a project root (for file-based commands).
fn temp_project() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("ludion-engine-dispatch-{}-{n}", std::process::id()));
    // nextest runs each test in its own process, so `{pid}-{n}` recurs once the OS recycles a pid:
    // a stale directory from an earlier run (an `out.yaml` file, say) must not leak into this one.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An engine with an empty, seeded scene loaded (world reset to frame 0).
fn engine() -> Engine {
    let mut engine = Engine::new();
    engine
        .load_inline_scene("format: 1\nseed: 7\nentities: []\n")
        .unwrap();
    engine
}

/// Dispatches a command, expecting success.
fn ok(engine: &mut Engine, command: Command) -> ResponseBody {
    dispatch(engine, command)
        .expect("command should succeed")
        .body
}

/// Dispatches a command, expecting a diagnostic, and returns its code string.
fn err_code(engine: &mut Engine, command: Command) -> String {
    dispatch(engine, command)
        .expect_err("command should fail")
        .code
        .as_str()
        .to_string()
}

/// Spawns an entity carrying one component, returning its id.
fn spawn_one(engine: &mut Engine, component: &str, value: Value) -> EntityId {
    let mut components = BTreeMap::new();
    components.insert(component.to_string(), value);
    match ok(
        engine,
        Command::EntitySpawn(EntitySpawnParams {
            name: None,
            components,
        }),
    ) {
        ResponseBody::Spawned { entity } => entity,
        other => panic!("expected Spawned, got {other:?}"),
    }
}

/// Spawns an entity with a `name` and no components.
fn spawn_named(engine: &mut Engine, name: &str) -> EntityId {
    match ok(
        engine,
        Command::EntitySpawn(EntitySpawnParams {
            name: Some(name.to_string()),
            components: BTreeMap::new(),
        }),
    ) {
        ResponseBody::Spawned { entity } => entity,
        other => panic!("expected Spawned, got {other:?}"),
    }
}

/// Spawns a named entity carrying `Parent` → `parent`.
fn spawn_child(engine: &mut Engine, name: &str, parent: EntityId) -> EntityId {
    let mut components = BTreeMap::new();
    components.insert(
        "Parent".to_string(),
        json!({ "index": parent.index, "generation": parent.generation }),
    );
    match ok(
        engine,
        Command::EntitySpawn(EntitySpawnParams {
            name: Some(name.to_string()),
            components,
        }),
    ) {
        ResponseBody::Spawned { entity } => entity,
        other => panic!("expected Spawned, got {other:?}"),
    }
}

/// Spawns a named `name` entity whose `Parent` names a destroyed ghost, returning `(child, ghost)`:
/// `entity.destroy` cascades, so a dangling parent can only be written by hand.
fn spawn_dangling_child(engine: &mut Engine, name: &str) -> (EntityId, EntityId) {
    let child = spawn_named(engine, name);
    let ghost = spawn_named(engine, "ghost");
    ok(
        engine,
        Command::EntityDestroy(EntityDestroyParams { entity: ghost }),
    );
    ok(
        engine,
        Command::ComponentSet(ComponentSetParams {
            entity: child,
            component: "Parent".into(),
            value: json!({ "index": ghost.index, "generation": ghost.generation }),
        }),
    );
    (child, ghost)
}

/// Spawns a named entity carrying `Joint` naming `a` and `b`, kind `fixed`.
fn spawn_joint(engine: &mut Engine, name: &str, a: EntityId, b: EntityId) -> EntityId {
    let mut components = BTreeMap::new();
    components.insert(
        "Joint".to_string(),
        json!({
            "a": { "index": a.index, "generation": a.generation },
            "b": { "index": b.index, "generation": b.generation },
            "kind": { "fixed": {} },
        }),
    );
    match ok(
        engine,
        Command::EntitySpawn(EntitySpawnParams {
            name: Some(name.to_string()),
            components,
        }),
    ) {
        ResponseBody::Spawned { entity } => entity,
        other => panic!("expected Spawned, got {other:?}"),
    }
}

/// Spawns a named entity carrying one component with the given JSON value.
fn spawn_with(engine: &mut Engine, name: &str, component: &str, value: Value) -> EntityId {
    let mut components = BTreeMap::new();
    components.insert(component.to_string(), value);
    match ok(
        engine,
        Command::EntitySpawn(EntitySpawnParams {
            name: Some(name.to_string()),
            components,
        }),
    ) {
        ResponseBody::Spawned { entity } => entity,
        other => panic!("expected Spawned, got {other:?}"),
    }
}

/// `scene.save` to `out.yaml`, then a fresh engine loaded from the result.
fn save_and_reload(engine: &mut Engine, root: &std::path::Path) -> Engine {
    ok(
        engine,
        Command::SceneSave(SceneSaveParams {
            path: "out.yaml".into(),
        }),
    );
    let saved = std::fs::read_to_string(root.join("out.yaml")).unwrap();
    let mut reloaded = Engine::with_project_root(root);
    reloaded.load_inline_scene(&saved).unwrap();
    reloaded
}

/// One component's JSON value on `entity`.
fn component_value(engine: &mut Engine, entity: EntityId, component: &str) -> Value {
    match ok(
        engine,
        Command::ComponentGet(ComponentGetParams {
            entity,
            component: component.into(),
        }),
    ) {
        ResponseBody::ComponentValue { value } => value,
        other => panic!("{other:?}"),
    }
}

/// Runs `scene.check` on inline YAML written to `root`, returning its diagnostics.
fn scene_check_diagnostics(root: &std::path::Path, file: &str, yaml: &str) -> Vec<Diagnostic> {
    std::fs::write(root.join(file), yaml).unwrap();
    let mut engine = Engine::with_project_root(root);
    match ok(
        &mut engine,
        Command::SceneCheck(SceneCheckParams { path: file.into() }),
    ) {
        ResponseBody::SceneChecked { diagnostics, .. } => diagnostics,
        other => panic!("expected SceneChecked, got {other:?}"),
    }
}

/// A recording sink shared with the test so the recorded bytes can be read back.
#[derive(Clone, Default)]
struct SharedSink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

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

// --- scene family --------------------------------------------------------------------------------

#[test]
fn scene_load_check_save() {
    let root = temp_project();
    let scene = "format: 1\nentities:\n  - components:\n      Transform:\n        translation:\n          - 1.0\n          - 0.0\n          - 0.0\n";
    std::fs::write(root.join("level.yaml"), scene).unwrap();

    let mut engine = Engine::with_project_root(&root);

    // scene.check — valid.
    match ok(
        &mut engine,
        Command::SceneCheck(SceneCheckParams {
            path: "level.yaml".into(),
        }),
    ) {
        ResponseBody::SceneChecked { valid, diagnostics } => {
            assert!(valid);
            assert!(diagnostics.is_empty());
        }
        other => panic!("{other:?}"),
    }

    // scene.load — happy.
    assert_eq!(
        ok(
            &mut engine,
            Command::SceneLoad(SceneLoadParams {
                path: "level.yaml".into(),
            }),
        ),
        ResponseBody::SceneLoaded { entity_count: 1 },
    );
    assert_eq!(engine.world().entity_count(), 1);
    assert_eq!(engine.frame().as_u64(), 0, "scene.load resets the frame");

    // scene.save — canonical YAML round-trips a Transform.
    ok(
        &mut engine,
        Command::SceneSave(SceneSaveParams {
            path: "out.yaml".into(),
        }),
    );
    let saved = std::fs::read_to_string(root.join("out.yaml")).unwrap();
    assert!(saved.contains("Transform"), "saved scene: {saved}");
    let stray: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(stray.is_empty(), "scene.save left temps: {stray:?}");
}

#[test]
fn scene_round_trips_byte_identically() {
    // A scene with a scene name, a seed, and a mix of named and unnamed entities. Components list
    // only some fields — the first save materializes the canonical (full) form; the guarantee under
    // test is that reloading and re-saving that canonical form reproduces it byte-for-byte, and that
    // entity `name:` fields (and the scene name) survive the load→save round trip.
    let root = temp_project();
    let source = "format: 1\n\
                  name: level-1\n\
                  seed: 9\n\
                  entities:\n\
                  \x20 - name: hero\n\
                  \x20   components:\n\
                  \x20     Transform:\n\
                  \x20       translation:\n\
                  \x20         - 1.0\n\
                  \x20         - 2.0\n\
                  \x20         - 3.0\n\
                  \x20 - components:\n\
                  \x20     Mesh: {}\n\
                  \x20 - name: key-light\n\
                  \x20   components:\n\
                  \x20     PointLight:\n\
                  \x20       intensity: 2.0\n";

    let mut e1 = Engine::with_project_root(&root);
    e1.load_inline_scene(source).unwrap();
    ok(
        &mut e1,
        Command::SceneSave(SceneSaveParams {
            path: "canon.yaml".into(),
        }),
    );
    let canonical = std::fs::read_to_string(root.join("canon.yaml")).unwrap();

    // Scene name and both entity names survive; the reserved `Name` never appears as a block.
    assert!(canonical.contains("name: level-1"), "scene: {canonical}");
    assert!(canonical.contains("- name: hero"), "scene: {canonical}");
    assert!(
        canonical.contains("- name: key-light"),
        "scene: {canonical}"
    );
    assert!(canonical.contains("seed: 9"), "scene: {canonical}");
    assert!(
        !canonical.contains("Name:"),
        "the reserved Name component must never be emitted as a block: {canonical}"
    );

    // Byte-identity: reloading the canonical form and re-saving reproduces it exactly.
    let mut e2 = Engine::with_project_root(&root);
    e2.load_inline_scene(&canonical).unwrap();
    ok(
        &mut e2,
        Command::SceneSave(SceneSaveParams {
            path: "canon2.yaml".into(),
        }),
    );
    let canonical2 = std::fs::read_to_string(root.join("canon2.yaml")).unwrap();
    assert_eq!(
        canonical, canonical2,
        "load→save is not a byte-exact fixed point"
    );
}

#[test]
fn name_survives_load_as_queryable_component() {
    // The entity `name:` field lands in the world as a queryable `Name` component.
    let mut engine = Engine::new();
    engine
        .load_inline_scene(
            "format: 1\nentities:\n  - name: paddle\n    components: {}\n  - components: {}\n",
        )
        .unwrap();

    let ids: Vec<EntityId> = engine.world().entities().collect();

    // The named entity carries Name = "paddle".
    match ok(
        &mut engine,
        Command::ComponentGet(ComponentGetParams {
            entity: ids[0],
            component: "Name".into(),
        }),
    ) {
        ResponseBody::ComponentValue { value } => assert_eq!(value, json!("paddle")),
        other => panic!("{other:?}"),
    }

    // The unnamed entity has no Name component (L3006 component-absent).
    assert_eq!(
        err_code(
            &mut engine,
            Command::ComponentGet(ComponentGetParams {
                entity: ids[1],
                component: "Name".into(),
            }),
        ),
        "L3006",
    );
}

#[test]
fn name_declared_as_a_component_block_is_l1016() {
    // The `name:` field is the one authoring channel; a `Name` component block is rejected at check.
    let root = temp_project();
    std::fs::write(
        root.join("dup.yaml"),
        "format: 1\nentities:\n  - components:\n      Name: hero\n",
    )
    .unwrap();
    let mut engine = Engine::with_project_root(&root);
    match ok(
        &mut engine,
        Command::SceneCheck(SceneCheckParams {
            path: "dup.yaml".into(),
        }),
    ) {
        ResponseBody::SceneChecked { valid, diagnostics } => {
            assert!(!valid);
            assert!(
                diagnostics.iter().any(|d| d.code.as_str() == "L1016"),
                "expected L1016, got {diagnostics:?}"
            );
        }
        other => panic!("{other:?}"),
    }
    // And the same scene fails to load (check gates the load path).
    assert_eq!(
        err_code(
            &mut engine,
            Command::SceneLoad(SceneLoadParams {
                path: "dup.yaml".into(),
            }),
        ),
        "L1016",
    );
}

#[test]
fn spawn_name_populates_the_name_component() {
    let mut engine = engine();
    let entity = spawn_named(&mut engine, "hero");
    match ok(
        &mut engine,
        Command::ComponentGet(ComponentGetParams {
            entity,
            component: "Name".into(),
        }),
    ) {
        ResponseBody::ComponentValue { value } => assert_eq!(value, json!("hero")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn spawn_with_name_and_name_component_is_l3014() {
    let mut engine = engine();
    let mut components = BTreeMap::new();
    components.insert("Name".to_string(), json!("hero"));
    assert_eq!(
        err_code(
            &mut engine,
            Command::EntitySpawn(EntitySpawnParams {
                name: Some("hero".into()),
                components,
            }),
        ),
        "L3014",
    );
    // Checked before spawning: no half-built entity is left behind.
    assert_eq!(engine.world().entity_count(), 0);
}

#[test]
fn a_non_relative_asset_path_is_refused_on_spawn_and_component_set() {
    let mut engine = engine();
    let mut components = BTreeMap::new();
    components.insert("Sprite".to_string(), json!({ "texture": "C:outside.png" }));
    assert_eq!(
        err_code(
            &mut engine,
            Command::EntitySpawn(EntitySpawnParams {
                name: None,
                components,
            }),
        ),
        "L1321",
    );
    assert_eq!(engine.world().entity_count(), 0);

    let entity = spawn_one(&mut engine, "Sprite", json!({ "texture": "ok.png" }));
    let diagnostic = dispatch(
        &mut engine,
        Command::ComponentSet(ComponentSetParams {
            entity,
            component: "Sprite".into(),
            value: json!({ "texture": "/etc/x.png" }),
        }),
    )
    .expect_err("an absolute asset path must be refused");
    assert_eq!(diagnostic.code.as_str(), "L1321");
    assert_eq!(diagnostic.path.as_deref(), Some("/texture"));
    assert_eq!(
        component_value(&mut engine, entity, "Sprite")["texture"],
        json!("ok.png")
    );
}

#[test]
fn spawn_without_name_carries_no_name_component() {
    let mut engine = engine();
    let entity = spawn_one(&mut engine, "Transform", json!({}));
    assert_eq!(
        err_code(
            &mut engine,
            Command::ComponentGet(ComponentGetParams {
                entity,
                component: "Name".into(),
            }),
        ),
        "L3006",
    );
}

#[test]
fn destroy_cascades_and_a_dangling_parent_save_is_l1018() {
    let root = temp_project();
    let mut engine = Engine::with_project_root(&root);
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .unwrap();

    // `entity.destroy` cascades: destroying a parent takes its child with it.
    let parent = spawn_named(&mut engine, "parent");
    let doomed = spawn_child(&mut engine, "doomed", parent);
    ok(
        &mut engine,
        Command::EntityDestroy(EntityDestroyParams { entity: parent }),
    );
    assert!(
        !engine.world().is_alive(doomed),
        "the child dies with its parent"
    );

    // A hand-written `Parent` naming a dead id is left verbatim.
    let (child, ghost) = spawn_dangling_child(&mut engine, "child");
    match ok(
        &mut engine,
        Command::ComponentGet(ComponentGetParams {
            entity: child,
            component: "Parent".into(),
        }),
    ) {
        ResponseBody::ComponentValue { value } => assert_eq!(
            value,
            json!({ "index": ghost.index, "generation": ghost.generation }),
            "the dangling Parent is left verbatim"
        ),
        other => panic!("{other:?}"),
    }

    // Save refuses to write a hierarchy that would reload corrupt.
    assert_eq!(
        err_code(
            &mut engine,
            Command::SceneSave(SceneSaveParams {
                path: "out.yaml".into(),
            }),
        ),
        "L1018",
    );
}

/// Spawns a named entity at `x` (a `Transform` translation), parented when `parent` is given.
fn spawn_at(engine: &mut Engine, name: &str, x: f32, parent: Option<EntityId>) -> EntityId {
    let mut components = BTreeMap::new();
    components.insert(
        "Transform".to_string(),
        json!({ "translation": [x, 0.0, 0.0] }),
    );
    if let Some(parent) = parent {
        components.insert(
            "Parent".to_string(),
            json!({ "index": parent.index(), "generation": parent.generation() }),
        );
    }
    match ok(
        engine,
        Command::EntitySpawn(EntitySpawnParams {
            name: Some(name.to_string()),
            components,
        }),
    ) {
        ResponseBody::Spawned { entity } => entity,
        other => panic!("expected Spawned, got {other:?}"),
    }
}

fn reparent_command(entity: EntityId, parent: Option<EntityId>, keep: ReparentKeep) -> Command {
    Command::EntityReparent(EntityReparentParams {
        entity,
        parent,
        keep,
    })
}

/// `query.entities {world: true}`: the entity's reported world translation.
fn world_translation(engine: &mut Engine, entity: EntityId) -> [f32; 3] {
    let body = ok(
        engine,
        Command::QueryEntities(QueryEntitiesParams {
            filter: vec!["Transform".into()],
            include: vec![],
            world: true,
            entities: None,
        }),
    );
    let ResponseBody::QueryResult { entities } = body else {
        panic!("expected QueryResult");
    };
    entities
        .iter()
        .find(|m| m.entity == entity)
        .and_then(|m| m.world)
        .expect("a world transform")
        .translation
}

fn assert_near(a: [f32; 3], b: [f32; 3]) {
    for axis in 0..3 {
        assert!((a[axis] - b[axis]).abs() < 1e-4, "{a:?} vs {b:?}");
    }
}

fn sim_hash(engine: &mut Engine) -> ludion_protocol::WireHash {
    match ok(engine, Command::SimHash) {
        ResponseBody::SimHash { hash } => hash,
        other => panic!("{other:?}"),
    }
}

#[test]
fn reparent_keeps_the_world_or_the_local_pose_and_its_reply_undoes_it() {
    let mut engine = engine();
    let parent = spawn_at(&mut engine, "parent", 10.0, None);
    let child = spawn_at(&mut engine, "child", 1.0, None);
    let original = match ok(
        &mut engine,
        Command::ComponentGet(ComponentGetParams {
            entity: child,
            component: "Transform".into(),
        }),
    ) {
        ResponseBody::ComponentValue { value } => value,
        other => panic!("{other:?}"),
    };

    let reply = ok(
        &mut engine,
        reparent_command(child, Some(parent), ReparentKeep::World),
    );
    assert_eq!(
        reply,
        ResponseBody::Reparented {
            previous_parent: None,
            previous_transform: Some(original.clone()),
        }
    );
    assert_near(world_translation(&mut engine, child), [1.0, 0.0, 0.0]);

    // Undo: the reply's parent with keep local, then its Transform.
    ok(
        &mut engine,
        reparent_command(child, None, ReparentKeep::Local),
    );
    ok(
        &mut engine,
        Command::ComponentSet(ComponentSetParams {
            entity: child,
            component: "Transform".into(),
            value: original.clone(),
        }),
    );
    assert_eq!(
        ok(
            &mut engine,
            Command::ComponentGet(ComponentGetParams {
                entity: child,
                component: "Transform".into(),
            }),
        ),
        ResponseBody::ComponentValue { value: original }
    );
    assert_eq!(
        err_code(
            &mut engine,
            Command::ComponentGet(ComponentGetParams {
                entity: child,
                component: "Parent".into(),
            }),
        ),
        "L3006"
    );

    // Keep local: the entity moves with its new parent.
    ok(
        &mut engine,
        reparent_command(child, Some(parent), ReparentKeep::Local),
    );
    assert_near(world_translation(&mut engine, child), [11.0, 0.0, 0.0]);
}

#[test]
fn a_cycle_reparent_is_l1224_and_changes_nothing() {
    let mut engine = engine();
    let a = spawn_at(&mut engine, "a", 1.0, None);
    let b = spawn_at(&mut engine, "b", 2.0, Some(a));
    let c = spawn_at(&mut engine, "c", 3.0, Some(b));
    let before = sim_hash(&mut engine);
    for (child, parent) in [(a, c), (a, b), (a, a)] {
        assert_eq!(
            err_code(
                &mut engine,
                reparent_command(child, Some(parent), ReparentKeep::World)
            ),
            "L1224"
        );
    }
    assert_eq!(sim_hash(&mut engine), before);
}

#[test]
fn destroy_returns_the_subtree_parent_first_and_the_undo_rebuilds_it() {
    let mut engine = engine();
    let root = spawn_at(&mut engine, "root", 1.0, None);
    let x = spawn_at(&mut engine, "x", 2.0, Some(root));
    let y = spawn_at(&mut engine, "y", 3.0, Some(root));
    let z = spawn_at(&mut engine, "z", 4.0, Some(x));

    let ResponseBody::EntityDestroyed {
        components,
        descendants,
    } = ok(
        &mut engine,
        Command::EntityDestroy(EntityDestroyParams { entity: root }),
    )
    else {
        panic!("expected EntityDestroyed");
    };
    for id in [root, x, y, z] {
        assert!(!engine.world().is_alive(id));
    }
    let order: Vec<EntityId> = descendants.iter().map(|d| d.entity).collect();
    let at = |id: EntityId| order.iter().position(|e| *e == id).unwrap();
    assert_eq!(order.len(), 3);
    assert!(at(x) < at(z), "a parent precedes its child: {order:?}");

    // Respawn root, then each descendant in order, remapping the old ids in their Parent values.
    let mut fresh: BTreeMap<EntityId, EntityId> = BTreeMap::new();
    let spawn = |engine: &mut Engine, components: BTreeMap<String, Value>| match ok(
        engine,
        Command::EntitySpawn(EntitySpawnParams {
            name: None,
            components,
        }),
    ) {
        ResponseBody::Spawned { entity } => entity,
        other => panic!("{other:?}"),
    };
    fresh.insert(root, spawn(&mut engine, components));
    for destroyed in descendants {
        let mut components = destroyed.components;
        let old = components["Parent"].clone();
        let old = EntityId::new(
            old["index"].as_u64().unwrap() as u32,
            old["generation"].as_u64().unwrap() as u32,
        );
        let new = fresh[&old];
        components.insert(
            "Parent".into(),
            json!({ "index": new.index(), "generation": new.generation() }),
        );
        fresh.insert(destroyed.entity, spawn(&mut engine, components));
    }

    // Same shape and the same Transforms, matched by name.
    let ResponseBody::QueryResult { entities } = ok(
        &mut engine,
        Command::QueryEntities(QueryEntitiesParams {
            filter: vec!["Name".into()],
            include: vec!["Name".into(), "Transform".into(), "Parent".into()],
            world: false,
            entities: None,
        }),
    ) else {
        panic!("expected QueryResult");
    };
    let by_id: BTreeMap<EntityId, &ludion_protocol::response::EntityMatch> =
        entities.iter().map(|m| (m.entity, m)).collect();
    let expect = [
        (root, None, 1.0, "root"),
        (x, Some(root), 2.0, "x"),
        (y, Some(root), 3.0, "y"),
        (z, Some(x), 4.0, "z"),
    ];
    for (old, parent, tx, label) in expect {
        let new = fresh[&old];
        assert_eq!(by_id[&new].components["Name"], json!(label));
        assert_eq!(
            by_id[&new].components["Transform"]["translation"],
            json!([tx, 0.0, 0.0])
        );
        match parent {
            None => assert!(!by_id[&new].components.contains_key("Parent")),
            Some(parent) => {
                let want = fresh[&parent];
                assert_eq!(
                    by_id[&new].components["Parent"],
                    json!({ "index": want.index(), "generation": want.generation() })
                );
            }
        }
    }
    let map = ludion_ecs::ChildMap::build(engine.world());
    assert_eq!(map.children(fresh[&root]).len(), 2);
    assert_eq!(map.descendants(fresh[&root]).len(), 3);
}

#[test]
fn parent_index_remaps_across_a_destroy_and_survives_reload() {
    let root = temp_project();
    let mut engine = Engine::with_project_root(&root);
    engine
        .load_inline_scene("format: 1\nseed: 5\nentities: []\n")
        .unwrap();

    // Document order A, B, C; C parents B.
    let a = spawn_named(&mut engine, "A");
    let b = spawn_named(&mut engine, "B");
    let _c = spawn_child(&mut engine, "C", b);

    // Destroy the unrelated earlier entity A, so live slot indices no longer equal document
    // positions (B and C shift down by one on save).
    ok(
        &mut engine,
        Command::EntityDestroy(EntityDestroyParams { entity: a }),
    );
    assert_ne!(
        b.index, 0,
        "precondition: B's live slot is not document position 0"
    );

    ok(
        &mut engine,
        Command::SceneSave(SceneSaveParams {
            path: "out.yaml".into(),
        }),
    );
    let saved = std::fs::read_to_string(root.join("out.yaml")).unwrap();
    // C's Parent must be remapped to B's new document position (0), not B's live slot index.
    assert!(saved.contains("Parent:"), "saved scene: {saved}");
    assert!(
        saved.contains("index: 0"),
        "Parent must point at B's document position 0: {saved}"
    );

    // Reload and confirm the hierarchy is intact: C (document position 1) parents B (position 0).
    let mut reloaded = Engine::with_project_root(&root);
    reloaded.load_inline_scene(&saved).unwrap();
    let ids: Vec<EntityId> = reloaded.world().entities().collect();
    assert_eq!(ids.len(), 2);
    match ok(
        &mut reloaded,
        Command::ComponentGet(ComponentGetParams {
            entity: ids[0],
            component: "Name".into(),
        }),
    ) {
        ResponseBody::ComponentValue { value } => assert_eq!(value, json!("B")),
        other => panic!("{other:?}"),
    }
    match ok(
        &mut reloaded,
        Command::ComponentGet(ComponentGetParams {
            entity: ids[1],
            component: "Parent".into(),
        }),
    ) {
        ResponseBody::ComponentValue { value } => assert_eq!(
            value,
            json!({ "index": 0, "generation": 0 }),
            "C's Parent resolves to B at its reloaded document position"
        ),
        other => panic!("{other:?}"),
    }
}

/// The four `Parent` structural checks in `check-scene`: out-of-range index, nonzero generation,
/// self-parent (each `L1017` with the pointer at the offending value), and a valid reference.
#[test]
fn scene_check_reports_a_missing_mesh_document() {
    let root = temp_project();
    let mut engine = Engine::with_project_root(&root);
    std::fs::write(
        root.join("rock.yaml"),
        "format: 1
entities:
  - name: rock
    components:
      Mesh:
        shape:
          asset: gone.mesh.yaml
",
    )
    .unwrap();
    match ok(
        &mut engine,
        Command::SceneCheck(SceneCheckParams {
            path: "rock.yaml".into(),
        }),
    ) {
        ResponseBody::SceneChecked { valid, diagnostics } => {
            assert!(!valid, "{diagnostics:?}");
            let d = diagnostics
                .iter()
                .find(|d| d.code.as_str() == "L1101")
                .expect("expected L1101");
            assert_eq!(
                d.path.as_deref(),
                Some("/entities/0/components/Mesh/shape/asset")
            );
        }
        other => panic!("expected SceneChecked, got {other:?}"),
    }
}

#[test]
fn check_scene_validates_parent_references() {
    let root = temp_project();

    let out_of_range = scene_check_diagnostics(
        &root,
        "range.yaml",
        "format: 1\nentities:\n  - components:\n      Parent:\n        index: 5\n        generation: 0\n",
    );
    let d = out_of_range
        .iter()
        .find(|d| d.code.as_str() == "L1017")
        .expect("expected L1017");
    assert_eq!(
        d.path.as_deref(),
        Some("/entities/0/components/Parent/index")
    );

    let bad_generation = scene_check_diagnostics(
        &root,
        "gen.yaml",
        "format: 1\nentities:\n  - components: {}\n  - components:\n      Parent:\n        index: 0\n        generation: 3\n",
    );
    let d = bad_generation
        .iter()
        .find(|d| d.code.as_str() == "L1017")
        .expect("expected L1017");
    assert_eq!(
        d.path.as_deref(),
        Some("/entities/1/components/Parent/generation")
    );

    let self_parent = scene_check_diagnostics(
        &root,
        "self.yaml",
        "format: 1\nentities:\n  - components:\n      Parent:\n        index: 0\n        generation: 0\n",
    );
    let d = self_parent
        .iter()
        .find(|d| d.code.as_str() == "L1017")
        .expect("expected L1017");
    assert_eq!(
        d.path.as_deref(),
        Some("/entities/0/components/Parent/index")
    );

    let valid = scene_check_diagnostics(
        &root,
        "ok.yaml",
        "format: 1\nentities:\n  - components: {}\n  - components:\n      Parent:\n        index: 0\n        generation: 0\n",
    );
    assert!(
        valid.iter().all(|d| d.code.as_str() != "L1017"),
        "a valid Parent must pass: {valid:?}"
    );
}

/// The generalized `scene.save` remap (`Plan/physics.md` W3): `Joint`'s `a`/`b` remap to document
/// positions across a destroy the same way `Parent` does, and survive a reload.
#[test]
fn joint_refs_remap_across_a_destroy_and_survive_reload() {
    let root = temp_project();
    let mut engine = Engine::with_project_root(&root);
    engine
        .load_inline_scene("format: 1\nseed: 5\nentities: []\n")
        .unwrap();

    // Document order A, B, C; the joint carrier D names A and B.
    let a = spawn_named(&mut engine, "A");
    let b = spawn_named(&mut engine, "B");
    let c = spawn_named(&mut engine, "C");
    let _d = spawn_joint(&mut engine, "D", a, b);

    // Destroy the unrelated middle entity C, so live slot indices no longer equal document
    // positions for D's remaining siblings.
    ok(
        &mut engine,
        Command::EntityDestroy(EntityDestroyParams { entity: c }),
    );

    ok(
        &mut engine,
        Command::SceneSave(SceneSaveParams {
            path: "out.yaml".into(),
        }),
    );
    let saved = std::fs::read_to_string(root.join("out.yaml")).unwrap();
    assert!(saved.contains("Joint:"), "saved scene: {saved}");

    // Reload and confirm D's `a`/`b` still resolve to A and B at their reloaded document positions.
    let mut reloaded = Engine::with_project_root(&root);
    reloaded.load_inline_scene(&saved).unwrap();
    let ids: Vec<EntityId> = reloaded.world().entities().collect();
    assert_eq!(ids.len(), 3, "A, B, D survive; C was destroyed");
    match ok(
        &mut reloaded,
        Command::ComponentGet(ComponentGetParams {
            entity: ids[2],
            component: "Joint".into(),
        }),
    ) {
        ResponseBody::ComponentValue { value } => {
            assert_eq!(
                value["a"],
                json!({ "index": 0, "generation": 0 }),
                "D's `a` resolves to A at its reloaded document position"
            );
            assert_eq!(
                value["b"],
                json!({ "index": 1, "generation": 0 }),
                "D's `b` resolves to B at its reloaded document position"
            );
        }
        other => panic!("{other:?}"),
    }
}

/// The optional entity references (`PatternPlayer.target`, `Volley.source`) remap across a destroy
/// like `Joint`'s, an absent target stays absent, and a dead one is saved as `null` rather than
/// refusing the save (a shooter dying with bullets in flight is ordinary gameplay).
#[test]
fn optional_entity_refs_remap_and_null_out_on_save() {
    let root = temp_project();
    let mut engine = Engine::with_project_root(&root);
    engine
        .load_inline_scene(
            "format: 1
seed: 5
entities: []
",
        )
        .unwrap();

    let a = spawn_named(&mut engine, "A");
    let b = spawn_named(&mut engine, "B");
    let shooter = spawn_named(&mut engine, "shooter");
    let ref_to = |id: EntityId| json!({ "index": id.index, "generation": id.generation });
    let _aimed = spawn_with(
        &mut engine,
        "aimed",
        "PatternPlayer",
        json!({ "pattern": "", "started": 0, "target": ref_to(b) }),
    );
    let _untargeted = spawn_with(
        &mut engine,
        "untargeted",
        "PatternPlayer",
        json!({ "pattern": "", "started": 0, "target": null }),
    );
    let _orphaned = spawn_with(
        &mut engine,
        "orphaned",
        "PatternPlayer",
        json!({ "pattern": "", "started": 0, "target": ref_to(shooter) }),
    );
    let _volley = spawn_with(
        &mut engine,
        "volley",
        "Volley",
        json!({ "source": ref_to(shooter) }),
    );
    let _kept = spawn_with(
        &mut engine,
        "kept",
        "Volley",
        json!({ "source": ref_to(b) }),
    );

    // Destroying A shifts every later document position down by one; destroying the shooter
    // leaves two references dangling.
    for entity in [a, shooter] {
        ok(
            &mut engine,
            Command::EntityDestroy(EntityDestroyParams { entity }),
        );
    }

    let mut reloaded = save_and_reload(&mut engine, &root);
    let ids: Vec<EntityId> = reloaded.world().entities().collect();
    // Document order after the destroys: B, aimed, untargeted, orphaned, volley, kept.
    assert_eq!(ids.len(), 6);
    let target =
        |engine: &mut Engine, id| component_value(engine, id, "PatternPlayer")["target"].clone();
    let source = |engine: &mut Engine, id| component_value(engine, id, "Volley")["source"].clone();
    assert_eq!(
        target(&mut reloaded, ids[1]),
        json!({ "index": 0, "generation": 0 }),
        "the aimed player's target follows B to its new document position"
    );
    assert_eq!(
        target(&mut reloaded, ids[2]),
        Value::Null,
        "no target stays none"
    );
    assert_eq!(
        target(&mut reloaded, ids[3]),
        Value::Null,
        "a dead target saves as none"
    );
    assert_eq!(
        source(&mut reloaded, ids[4]),
        Value::Null,
        "a dead source saves as none"
    );
    assert_eq!(
        source(&mut reloaded, ids[5]),
        json!({ "index": 0, "generation": 0 }),
        "a live source follows B to its new document position"
    );
}

/// A `Joint` naming a despawned entity cannot be expressed as a valid scene — `scene.save` refuses
/// it with the same `L1018` `Parent` uses, generalized.
#[test]
fn a_joint_naming_a_despawned_entity_is_l1018_on_save() {
    let root = temp_project();
    let mut engine = Engine::with_project_root(&root);
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .unwrap();

    let a = spawn_named(&mut engine, "a");
    let b = spawn_named(&mut engine, "b");
    let _carrier = spawn_joint(&mut engine, "carrier", a, b);

    ok(
        &mut engine,
        Command::EntityDestroy(EntityDestroyParams { entity: b }),
    );

    assert_eq!(
        err_code(
            &mut engine,
            Command::SceneSave(SceneSaveParams {
                path: "out.yaml".into(),
            }),
        ),
        "L1018",
    );
}

/// Two entities that `Parent` each other form a loop no scene can hold, so `scene.save` refuses it
/// with `L1018` and names the chain.
#[test]
fn a_parent_cycle_is_l1018_on_save() {
    let root = temp_project();
    let mut engine = Engine::with_project_root(&root);
    engine
        .load_inline_scene(
            "format: 1
entities: []
",
        )
        .unwrap();

    let a = spawn_named(&mut engine, "a");
    let b = spawn_child(&mut engine, "b", a);
    ok(
        &mut engine,
        Command::ComponentSet(ComponentSetParams {
            entity: a,
            component: "Parent".into(),
            value: json!({ "index": b.index, "generation": b.generation }),
        }),
    );

    let error = save_err(&mut engine);
    assert_eq!(
        error.message,
        format!(
            "cannot save: the `Parent` links form a cycle: entity {a} (`a`) -> entity {b} (`b`) -> entity {a} (`a`)"
        )
    );
    assert_eq!(
        error.help.as_deref(),
        Some(
            "a hierarchy is a tree — `component.set` one `Parent` in the loop to another entity, \
             or `component.remove` it to make that entity a root"
        )
    );
}

/// Points `entity`'s `component` at `entity` itself, through `component.set`.
fn set_self_reference(engine: &mut Engine, entity: EntityId, component: &str, field: Option<&str>) {
    let this = json!({ "index": entity.index, "generation": entity.generation });
    let mut value = component_value(engine, entity, component);
    match field {
        Some(field) => value[field] = this,
        None => value = this,
    }
    ok(
        engine,
        Command::ComponentSet(ComponentSetParams {
            entity,
            component: component.into(),
            value,
        }),
    );
}

fn save_err(engine: &mut Engine) -> Diagnostic {
    let error = dispatch(
        engine,
        Command::SceneSave(SceneSaveParams {
            path: "out.yaml".into(),
        }),
    )
    .expect_err("the save should be refused");
    assert_eq!(error.code.as_str(), "L1018");
    error
}

fn save_err_message(engine: &mut Engine) -> String {
    save_err(engine).message
}

/// `PatternPlayer.target` forbids naming its own carrier (aiming at yourself is a zero vector), and
/// a save refuses it with `L1018` even though the field is optional — nulling it would lose data.
#[test]
fn a_pattern_player_targeting_its_carrier_is_l1018_on_save() {
    let root = temp_project();
    let mut engine = Engine::with_project_root(&root);
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .unwrap();
    let player = spawn_with(
        &mut engine,
        "player",
        "PatternPlayer",
        json!({ "pattern": "", "started": 0, "target": null }),
    );
    set_self_reference(&mut engine, player, "PatternPlayer", Some("target"));

    let message = save_err_message(&mut engine);
    assert!(message.contains("`target` pointing at itself"), "{message}");
}

/// `Parent` never allows self-reference: a save refuses it rather than writing a scene that would
/// fail `L1017` on reload.
#[test]
fn a_parent_naming_its_carrier_is_l1018_on_save() {
    let root = temp_project();
    let mut engine = Engine::with_project_root(&root);
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .unwrap();
    let other = spawn_named(&mut engine, "other");
    let child = spawn_child(&mut engine, "child", other);
    set_self_reference(&mut engine, child, "Parent", None);

    let message = save_err_message(&mut engine);
    assert!(
        message.contains("has a `Parent` pointing at itself"),
        "{message}"
    );
}

/// `Volley.source` allows naming its own carrier: the save writes the carrier's own document
/// position and the reload resolves it to the same entity.
#[test]
fn a_volley_sourced_by_its_carrier_saves_and_round_trips() {
    let root = temp_project();
    let mut engine = Engine::with_project_root(&root);
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .unwrap();
    let _first = spawn_named(&mut engine, "first");
    let volley = spawn_with(&mut engine, "volley", "Volley", json!({ "source": null }));
    set_self_reference(&mut engine, volley, "Volley", Some("source"));

    let mut reloaded = save_and_reload(&mut engine, &root);
    let ids: Vec<EntityId> = reloaded.world().entities().collect();
    assert_eq!(ids.len(), 2);
    assert_eq!(
        component_value(&mut reloaded, ids[1], "Volley")["source"],
        json!({ "index": 1, "generation": 0 }),
        "the volley's source resolves to the volley itself"
    );
}

#[test]
fn scene_load_missing_file_is_l1015() {
    let mut engine = Engine::with_project_root(temp_project());
    assert_eq!(
        err_code(
            &mut engine,
            Command::SceneLoad(SceneLoadParams {
                path: "nope.yaml".into(),
            }),
        ),
        "L1015",
    );
}

#[test]
fn scene_save_onto_a_directory_is_output_io() {
    let root = temp_project();
    std::fs::create_dir_all(root.join("out.yaml")).unwrap();
    let mut engine = Engine::with_project_root(root);
    engine
        .load_inline_scene(
            "format: 1
seed: 7
entities: []
",
        )
        .unwrap();
    assert_eq!(
        err_code(
            &mut engine,
            Command::SceneSave(SceneSaveParams {
                path: "out.yaml".into(),
            }),
        ),
        "L1038",
    );
}

#[test]
fn scene_check_reports_invalid_without_erroring() {
    let root = temp_project();
    // Unknown component name → L1013 inside the diagnostics (the command still "succeeds").
    std::fs::write(
        root.join("bad.yaml"),
        "format: 1\nentities:\n  - components:\n      Nonsense: {}\n",
    )
    .unwrap();
    let mut engine = Engine::with_project_root(&root);
    match ok(
        &mut engine,
        Command::SceneCheck(SceneCheckParams {
            path: "bad.yaml".into(),
        }),
    ) {
        ResponseBody::SceneChecked { valid, diagnostics } => {
            assert!(!valid);
            assert!(!diagnostics.is_empty());
        }
        other => panic!("{other:?}"),
    }
}

// --- entity / component / query family -----------------------------------------------------------

#[test]
fn spawn_get_set_destroy() {
    let mut engine = engine();
    let entity = spawn_one(
        &mut engine,
        "Transform",
        json!({ "translation": [1.0, 2.0, 3.0] }),
    );
    assert_eq!(engine.world().entity_count(), 1);

    // component.get — happy.
    match ok(
        &mut engine,
        Command::ComponentGet(ComponentGetParams {
            entity,
            component: "Transform".into(),
        }),
    ) {
        ResponseBody::ComponentValue { value } => {
            assert_eq!(value["translation"], json!([1.0, 2.0, 3.0]));
        }
        other => panic!("{other:?}"),
    }

    // component.set — overwrite; the response carries the exact previous value (the undo inverse).
    match ok(
        &mut engine,
        Command::ComponentSet(ComponentSetParams {
            entity,
            component: "Transform".into(),
            value: json!({ "translation": [9.0, 0.0, 0.0] }),
        }),
    ) {
        ResponseBody::ComponentSet { previous } => {
            assert_eq!(
                previous,
                Some(json!({
                    "translation": [1.0, 2.0, 3.0],
                    "rotation": [0.0, 0.0, 0.0],
                    "scale": [1.0, 1.0, 1.0],
                })),
            );
        }
        other => panic!("{other:?}"),
    }

    // entity.destroy — returns the full component state at destruction, then the handle is dead.
    match ok(
        &mut engine,
        Command::EntityDestroy(EntityDestroyParams { entity }),
    ) {
        ResponseBody::EntityDestroyed { components, .. } => {
            assert_eq!(
                components["Transform"]["translation"],
                json!([9.0, 0.0, 0.0])
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        err_code(
            &mut engine,
            Command::ComponentGet(ComponentGetParams {
                entity,
                component: "Transform".into(),
            }),
        ),
        "L0002",
    );
}

#[test]
fn component_set_fresh_insert_returns_none_previous() {
    let mut engine = engine();
    let entity = spawn_one(&mut engine, "Transform", json!({}));

    // The entity does not carry Sprite yet — the set is a fresh insert.
    match ok(
        &mut engine,
        Command::ComponentSet(ComponentSetParams {
            entity,
            component: "Sprite".into(),
            value: json!({}),
        }),
    ) {
        ResponseBody::ComponentSet { previous } => assert_eq!(previous, None),
        other => panic!("{other:?}"),
    }
}

#[test]
fn component_set_previous_round_trips_the_original_value_byte_equal() {
    let mut engine = engine();
    let entity = spawn_one(
        &mut engine,
        "Transform",
        json!({ "translation": [1.0, 2.0, 3.0] }),
    );
    let original = match ok(
        &mut engine,
        Command::ComponentGet(ComponentGetParams {
            entity,
            component: "Transform".into(),
        }),
    ) {
        ResponseBody::ComponentValue { value } => value,
        other => panic!("{other:?}"),
    };

    let previous = match ok(
        &mut engine,
        Command::ComponentSet(ComponentSetParams {
            entity,
            component: "Transform".into(),
            value: json!({ "translation": [9.0, 9.0, 9.0] }),
        }),
    ) {
        ResponseBody::ComponentSet { previous } => {
            previous.expect("Transform existed before this set")
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(
        previous, original,
        "the captured inverse is byte-equal to the pre-set value"
    );

    // Undo: set the component back to the captured inverse.
    ok(
        &mut engine,
        Command::ComponentSet(ComponentSetParams {
            entity,
            component: "Transform".into(),
            value: previous,
        }),
    );
    let restored = match ok(
        &mut engine,
        Command::ComponentGet(ComponentGetParams {
            entity,
            component: "Transform".into(),
        }),
    ) {
        ResponseBody::ComponentValue { value } => value,
        other => panic!("{other:?}"),
    };
    assert_eq!(restored, original, "undo restores the exact original value");
}

#[test]
fn component_remove_returns_value_then_matches_component_get_error_codes() {
    let mut engine = engine();
    let entity = spawn_one(
        &mut engine,
        "Transform",
        json!({ "translation": [1.0, 2.0, 3.0] }),
    );

    // component.remove — happy: returns exactly the removed value.
    match ok(
        &mut engine,
        Command::ComponentRemove(ComponentRemoveParams {
            entity,
            component: "Transform".into(),
        }),
    ) {
        ResponseBody::ComponentRemoved { previous } => {
            assert_eq!(
                previous,
                json!({
                    "translation": [1.0, 2.0, 3.0],
                    "rotation": [0.0, 0.0, 0.0],
                    "scale": [1.0, 1.0, 1.0],
                }),
            );
        }
        other => panic!("{other:?}"),
    }

    // A second remove of the now-absent component matches component.get's L3006.
    assert_eq!(
        err_code(
            &mut engine,
            Command::ComponentRemove(ComponentRemoveParams {
                entity,
                component: "Transform".into(),
            }),
        ),
        "L3006",
    );
    // Unregistered name → L0004, matching component.get.
    assert_eq!(
        err_code(
            &mut engine,
            Command::ComponentRemove(ComponentRemoveParams {
                entity,
                component: "Ghost".into(),
            }),
        ),
        "L0004",
    );
    // Dead entity → L0002, matching component.get.
    ok(
        &mut engine,
        Command::EntityDestroy(EntityDestroyParams { entity }),
    );
    assert_eq!(
        err_code(
            &mut engine,
            Command::ComponentRemove(ComponentRemoveParams {
                entity,
                component: "Transform".into(),
            }),
        ),
        "L0002",
    );
}

#[test]
fn entity_destroy_returns_full_component_state_including_name() {
    let mut engine = Engine::new();
    engine
        .load_inline_scene(
            "format: 1\nentities:\n  - name: paddle\n    components:\n      Transform:\n        \
             translation:\n          - 1.0\n          - 2.0\n          - 3.0\n      Sprite: {}\n",
        )
        .unwrap();
    let entity = engine.world().entities().next().unwrap();

    match ok(
        &mut engine,
        Command::EntityDestroy(EntityDestroyParams { entity }),
    ) {
        ResponseBody::EntityDestroyed { components, .. } => {
            assert_eq!(components["Name"], json!("paddle"));
            assert_eq!(
                components["Transform"]["translation"],
                json!([1.0, 2.0, 3.0])
            );
            assert!(components.contains_key("Sprite"));
            assert_eq!(components.len(), 3, "components: {components:?}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(engine.world().entity_count(), 0);
}

#[test]
fn spawn_unknown_component_is_l0004_and_bad_value_is_l0006() {
    let mut engine = engine();

    let mut unknown = BTreeMap::new();
    unknown.insert("Nope".to_string(), json!({}));
    assert_eq!(
        err_code(
            &mut engine,
            Command::EntitySpawn(EntitySpawnParams {
                name: None,
                components: unknown,
            }),
        ),
        "L0004",
    );
    assert_eq!(
        engine.world().entity_count(),
        0,
        "failed spawn leaves no entity"
    );

    let mut bad = BTreeMap::new();
    bad.insert(
        "Transform".to_string(),
        json!({ "translation": "not-a-vector" }),
    );
    assert_eq!(
        err_code(
            &mut engine,
            Command::EntitySpawn(EntitySpawnParams {
                name: None,
                components: bad,
            }),
        ),
        "L1014",
    );
    assert_eq!(engine.world().entity_count(), 0);

    // Nothing was spawned and despawned: the allocator is untouched, so the next spawn is generation 0.
    let next = spawn_one(&mut engine, "Transform", json!({}));
    assert_eq!(
        next.generation(),
        0,
        "failed spawns leave the allocator alone"
    );
}

/// `component.set` of `value` on `entity`, expecting a diagnostic.
fn set_err(engine: &mut Engine, entity: EntityId, component: &str, value: Value) -> Diagnostic {
    dispatch(
        engine,
        Command::ComponentSet(ComponentSetParams {
            entity,
            component: component.into(),
            value,
        }),
    )
    .expect_err("an off-schema value must be rejected")
}

#[test]
fn component_set_with_a_typo_d_field_is_l1014_and_leaves_the_value() {
    let mut engine = engine();
    let entity = spawn_one(
        &mut engine,
        "Transform",
        json!({ "translation": [1.0, 2.0, 3.0] }),
    );
    let diagnostic = set_err(
        &mut engine,
        entity,
        "Transform",
        json!({ "translaton": [5.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0] }),
    );
    assert_eq!(diagnostic.code.as_str(), "L1014");
    assert_eq!(diagnostic.path.as_deref(), Some("/translaton"));
    assert!(
        diagnostic
            .help
            .as_deref()
            .unwrap_or_default()
            .contains("`translation`"),
        "help: {:?}",
        diagnostic.help
    );
    let kept = component_value(&mut engine, entity, "Transform");
    assert_eq!(kept["translation"], json!([1.0, 2.0, 3.0]));
}

#[test]
fn component_set_with_a_mistyped_field_is_l1014() {
    let mut engine = engine();
    let entity = spawn_one(&mut engine, "Transform", json!({}));
    let diagnostic = set_err(
        &mut engine,
        entity,
        "Transform",
        json!({ "translation": "x" }),
    );
    assert_eq!(diagnostic.code.as_str(), "L1014");
    assert_eq!(diagnostic.path.as_deref(), Some("/translation"));
}

#[test]
fn entity_spawn_with_a_typo_d_field_is_l1014_and_touches_nothing() {
    let mut engine = engine();
    let mut bad = BTreeMap::new();
    bad.insert(
        "Transform".to_string(),
        json!({ "translaton": [5.0, 0.0, 0.0] }),
    );
    let diagnostic = dispatch(
        &mut engine,
        Command::EntitySpawn(EntitySpawnParams {
            name: None,
            components: bad,
        }),
    )
    .expect_err("a typo'd field must be rejected");
    assert_eq!(diagnostic.code.as_str(), "L1014");
    assert_eq!(diagnostic.path.as_deref(), Some("/translaton"));
    assert_eq!(engine.world().entity_count(), 0);
    let next = spawn_one(&mut engine, "Transform", json!({}));
    assert_eq!(
        next.generation(),
        0,
        "a rejected spawn leaves the allocator alone"
    );
}

#[test]
fn component_get_absent_is_l3006_and_unknown_name_is_l0004() {
    let mut engine = engine();
    let entity = spawn_one(&mut engine, "Transform", json!({}));

    // Present entity, registered component, but not carried → L3006.
    assert_eq!(
        err_code(
            &mut engine,
            Command::ComponentGet(ComponentGetParams {
                entity,
                component: "Sprite".into(),
            }),
        ),
        "L3006",
    );
    // Unregistered name → L0004.
    assert_eq!(
        err_code(
            &mut engine,
            Command::ComponentGet(ComponentGetParams {
                entity,
                component: "Ghost".into(),
            }),
        ),
        "L0004",
    );
}

/// `query.entities` with `entities` equals the whole-world query kept to the ids asked for.
#[test]
fn query_entities_id_restriction_matches_the_whole_world_oracle() {
    let mut engine = engine();
    let spawn = |engine: &mut Engine| {
        spawn_one(
            engine,
            "Transform",
            json!({ "translation": [1.0, 0.0, 0.0] }),
        )
    };
    let a = spawn(&mut engine);
    let b = spawn(&mut engine);
    let destroyed = spawn(&mut engine);
    let d = spawn(&mut engine);
    ok(
        &mut engine,
        Command::ComponentSet(ComponentSetParams {
            entity: d,
            component: "Name".into(),
            value: json!("named"),
        }),
    );
    ok(
        &mut engine,
        Command::EntityDestroy(EntityDestroyParams { entity: destroyed }),
    );
    let reborn = spawn(&mut engine);
    assert_ne!(
        reborn, destroyed,
        "the freed slot comes back with a new generation"
    );

    let query = |engine: &mut Engine, filter: &[&str], entities: Option<Vec<EntityId>>| match ok(
        engine,
        Command::QueryEntities(QueryEntitiesParams {
            filter: filter.iter().map(|s| (*s).to_string()).collect(),
            include: vec!["Transform".into()],
            world: true,
            entities,
        }),
    ) {
        ResponseBody::QueryResult { entities } => entities,
        other => panic!("{other:?}"),
    };

    let ghost = EntityId::new(999_999, 0);
    let cases: Vec<Vec<EntityId>> = vec![
        vec![b, a],
        vec![a, a],
        vec![destroyed],
        vec![ghost],
        vec![],
        vec![d, ghost, destroyed, reborn, a, a, b],
    ];
    for filter in [&["Transform"][..], &["Name"][..]] {
        let all = query(&mut engine, filter, None);
        for ids in &cases {
            let expected: Vec<_> = all
                .iter()
                .filter(|m| ids.contains(&m.entity))
                .cloned()
                .collect();
            let got = query(&mut engine, filter, Some(ids.clone()));
            assert_eq!(got, expected, "filter {filter:?}, ids {ids:?}");
        }
    }
    // The stale handle matches nothing even though its slot is live again.
    assert!(query(&mut engine, &["Transform"], Some(vec![destroyed])).is_empty());
    // Both restrictions must hold: `a` has no Name.
    assert!(query(&mut engine, &["Name"], Some(vec![a])).is_empty());
}

#[test]
fn query_entities_filters_and_includes() {
    let mut engine = engine();
    let a = spawn_one(
        &mut engine,
        "Transform",
        json!({ "translation": [1.0, 0.0, 0.0] }),
    );
    let _b = spawn_one(
        &mut engine,
        "Transform",
        json!({ "translation": [2.0, 0.0, 0.0] }),
    );
    // Give `a` a Sprite too.
    ok(
        &mut engine,
        Command::ComponentSet(ComponentSetParams {
            entity: a,
            component: "Sprite".into(),
            value: json!({}),
        }),
    );

    // Filter [Transform], include [Transform] → both, with values.
    match ok(
        &mut engine,
        Command::QueryEntities(QueryEntitiesParams {
            filter: vec!["Transform".into()],
            include: vec!["Transform".into()],
            world: false,
            entities: None,
        }),
    ) {
        ResponseBody::QueryResult { entities } => {
            assert_eq!(entities.len(), 2);
            assert!(
                entities
                    .iter()
                    .all(|m| m.components.contains_key("Transform"))
            );
        }
        other => panic!("{other:?}"),
    }

    // Filter [Sprite] → only `a`.
    match ok(
        &mut engine,
        Command::QueryEntities(QueryEntitiesParams {
            filter: vec!["Sprite".into()],
            include: vec![],
            world: false,
            entities: None,
        }),
    ) {
        ResponseBody::QueryResult { entities } => {
            assert_eq!(entities.len(), 1);
            assert_eq!(entities[0].entity, a);
        }
        other => panic!("{other:?}"),
    }

    // Unknown filter name → L0004.
    assert_eq!(
        err_code(
            &mut engine,
            Command::QueryEntities(QueryEntitiesParams {
                filter: vec!["Ghost".into()],
                include: vec![],
                world: false,
                entities: None,
            }),
        ),
        "L0004",
    );
}

// --- sim family ----------------------------------------------------------------------------------

#[test]
fn pause_resume_step_and_frame_stamp() {
    let mut engine = engine();

    assert_eq!(ok(&mut engine, Command::SimPause), ResponseBody::Ack);

    // step 5 with hash → Stepped{Some}, frame advanced to 5.
    let dispatched = dispatch(
        &mut engine,
        Command::SimStep(SimStepParams {
            ticks: 5,
            hash: true,
        }),
    )
    .unwrap();
    assert_eq!(dispatched.frame.as_u64(), 5);
    match dispatched.body {
        ResponseBody::Stepped { hash } => assert!(hash.is_some()),
        other => panic!("{other:?}"),
    }
    assert_eq!(engine.frame().as_u64(), 5);

    // step without hash.
    match ok(
        &mut engine,
        Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        }),
    ) {
        ResponseBody::Stepped { hash } => assert!(hash.is_none()),
        other => panic!("{other:?}"),
    }

    assert_eq!(ok(&mut engine, Command::SimResume), ResponseBody::Ack);
}

#[test]
fn set_speed_validates_and_hash_is_stable() {
    let mut engine = engine();
    assert_eq!(
        ok(
            &mut engine,
            Command::SimSetSpeed(SimSetSpeedParams { speed: 2.0 }),
        ),
        ResponseBody::Ack,
    );
    for bad in [0.0, 1e-30, 65.0] {
        assert_eq!(
            err_code(
                &mut engine,
                Command::SimSetSpeed(SimSetSpeedParams { speed: bad }),
            ),
            "L4001",
            "speed {bad}",
        );
    }

    // sim.hash is stable for an unchanged world.
    let h1 = ok(&mut engine, Command::SimHash);
    let h2 = ok(&mut engine, Command::SimHash);
    assert_eq!(h1, h2);
}

#[test]
fn input_inject_valid_and_invalid() {
    let mut engine = engine();
    assert_eq!(
        ok(
            &mut engine,
            Command::InputInject(InputInjectParams {
                frame: None,
                event: json!({ "kind": "key_down", "key": "W" }),
            }),
        ),
        ResponseBody::Ack,
    );
    assert_eq!(
        err_code(
            &mut engine,
            Command::InputInject(InputInjectParams {
                frame: None,
                event: json!({ "kind": "wiggle" }),
            }),
        ),
        "L3005",
    );
}

// --- render (CPU-only), stream, asset, engine ----------------------------------------------------

#[test]
fn dump_graph_is_the_frame_graph() {
    let mut engine = engine();
    match ok(&mut engine, Command::RenderDumpGraph) {
        ResponseBody::GraphDump { graph } => {
            assert!(graph.to_string().contains("composite"), "graph: {graph}");
        }
        other => panic!("{other:?}"),
    }
}

/// The wire dump is masked by the engine's quality, like the CLI's: a sun schedules the shadow pass
/// at full quality and none at `low`.
#[test]
fn dump_graph_masks_by_the_engine_quality() {
    let mut engine = Engine::new();
    engine
        .load_inline_scene(
            r#"format: 1
seed: 7
entities:
  - name: sun
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      DirectionalLight:
        direction:
          - 0.55
          - -0.42
          - -0.3
        color:
          - 1.0
          - 0.96
          - 0.84
        intensity: 2.4
"#,
        )
        .unwrap();
    let scheduled = |engine: &mut Engine| -> Vec<String> {
        match ok(engine, Command::RenderDumpGraph) {
            ResponseBody::GraphDump { graph } => graph["schedule"]
                .as_array()
                .expect("the dump carries a schedule array")
                .iter()
                .map(|name| name.as_str().unwrap().to_string())
                .collect(),
            other => panic!("{other:?}"),
        }
    };

    assert!(scheduled(&mut engine).iter().any(|p| p == "shadow"));
    engine.set_quality(QualitySetting {
        quality: Quality::LOW,
        params: QualityParams::LOW,
        label: "low".into(),
    });
    let low = scheduled(&mut engine);
    assert!(!low.iter().any(|p| p == "shadow"), "schedule: {low:?}");
}

#[test]
fn stream_subscribe_and_unsubscribe() {
    let mut engine = engine();
    let sub = match ok(
        &mut engine,
        Command::StreamSubscribe(StreamSubscribeParams {
            channel: Channel::Frames,
            mode: Default::default(),
            format: FrameFormat::Qoi,
        }),
    ) {
        ResponseBody::Subscribed { sub } => sub,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        ok(
            &mut engine,
            Command::StreamUnsubscribe(StreamUnsubscribeParams { sub }),
        ),
        ResponseBody::Ack,
    );
    // Double unsubscribe → L3007.
    assert_eq!(
        err_code(
            &mut engine,
            Command::StreamUnsubscribe(StreamUnsubscribeParams { sub }),
        ),
        "L3007",
    );
    // Unknown id → L3007.
    assert_eq!(
        err_code(
            &mut engine,
            Command::StreamUnsubscribe(StreamUnsubscribeParams {
                sub: SubId::new(999)
            }),
        ),
        "L3007",
    );
}

#[test]
fn stream_subscribe_latest_wins_opens() {
    let mut engine = engine();
    // `latest_wins` (the default) opens fine.
    assert!(matches!(
        ok(
            &mut engine,
            Command::StreamSubscribe(StreamSubscribeParams {
                channel: Channel::Frames,
                mode: SubscribeMode::LatestWins { max_fps: None },
                format: FrameFormat::Qoi,
            }),
        ),
        ResponseBody::Subscribed { .. }
    ));
}

#[test]
fn stream_subscribe_validates_max_fps() {
    let mut engine = engine();
    let subscribe = |max_fps| {
        Command::StreamSubscribe(StreamSubscribeParams {
            channel: Channel::Frames,
            mode: SubscribeMode::LatestWins { max_fps },
            format: FrameFormat::Qoi,
        })
    };
    for fps in [0.1, 30.0, 1000.0] {
        assert!(
            matches!(
                ok(&mut engine, subscribe(Some(fps))),
                ResponseBody::Subscribed { .. }
            ),
            "{fps} is in range"
        );
    }
    assert!(matches!(
        ok(&mut engine, subscribe(None)),
        ResponseBody::Subscribed { .. }
    ));
    // NaN and infinity cannot travel as JSON numbers, so these are built directly.
    for fps in [1e-30, -1.0, 0.0, 1000.5, f32::NAN, f32::INFINITY] {
        assert_eq!(
            err_code(&mut engine, subscribe(Some(fps))),
            "L3046",
            "{fps} is out of range"
        );
    }
}

#[test]
fn asset_reload_ignores_uncached_paths() {
    let root = temp_project();
    std::fs::write(root.join("thing.bin"), b"payload").unwrap();
    let mut engine = Engine::with_project_root(&root);

    // Membership, not mere presence, gates a reload now (Plan/phase-7 decision 5): with nothing
    // rendered, the CPU asset cache is empty, so a reload is a cheap no-op — no event, no error, an
    // empty reply — whether the path exists on disk (`thing.bin`) or not (`ghost.bin`). This is what
    // lets a host forward every changed path under the asset root and have the engine ignore what it
    // does not hold, with no L1101 spam for editor temp files.
    for path in ["thing.bin", "ghost.bin"] {
        match ok(
            &mut engine,
            Command::AssetReload(AssetReloadParams {
                path: Some(path.into()),
            }),
        ) {
            ResponseBody::AssetsReloaded { paths } => {
                assert!(paths.is_empty(), "uncached `{path}` must be ignored");
            }
            other => panic!("{other:?}"),
        }
    }
    // No path → nothing to reload.
    match ok(
        &mut engine,
        Command::AssetReload(AssetReloadParams { path: None }),
    ) {
        ResponseBody::AssetsReloaded { paths } => assert!(paths.is_empty()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn engine_describe_carries_components_and_schema_hash() {
    let mut engine = engine();
    match ok(&mut engine, Command::EngineDescribe) {
        ResponseBody::EngineDescription { description } => {
            assert!(description["components"]["Transform"].is_object());
            assert!(description["protocol"]["command"].is_object());
            let hash = description["schema_hash"].as_str().unwrap();
            assert_eq!(hash.len(), 16);
            // A bare engine registers no doc kind, so `documents` is present and empty — a client
            // never has to distinguish "no kinds" from "an engine too old to say".
            assert_eq!(
                description["documents"].as_object().unwrap().len(),
                0,
                "a bare engine describes no document kinds"
            );
        }
        other => panic!("{other:?}"),
    }
}

/// `engine.describe` publishes every registered doc kind's JSON Schema under `documents` — the same
/// `schema_for!(T)` document the engine validates a `kind:` file against, so a client's authoring
/// form and the engine's validation read one truth.
#[test]
fn engine_describe_carries_registered_doc_kind_schemas() {
    let mut engine = Engine::builder()
        .patterns()
        .project_root(temp_project())
        .build()
        .unwrap();
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .unwrap();
    match ok(&mut engine, Command::EngineDescribe) {
        ResponseBody::EngineDescription { description } => {
            let pattern = &description["documents"]["pattern"];
            assert!(
                pattern["properties"]["bullets"].is_object(),
                "the pattern schema names its bullet vocabulary: {pattern}"
            );
            assert!(
                pattern["properties"]["timeline"].is_object(),
                "the pattern schema names its timeline: {pattern}"
            );
        }
        other => panic!("{other:?}"),
    }
}

fn noop_a(_world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {}
fn noop_b(_world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {}

#[test]
fn engine_describe_carries_ordered_systems() {
    let mut engine = Engine::builder()
        .system("first", noop_a)
        .system("second", noop_b)
        .build()
        .unwrap();
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .unwrap();
    match ok(&mut engine, Command::EngineDescribe) {
        ResponseBody::EngineDescription { description } => {
            let systems = description["systems"].as_array().unwrap();
            // The engine's own systems bracket the game's: `path_follow` first, `camera_rig` last.
            assert_eq!(systems.len(), 4);
            assert_eq!(systems[0]["name"], "path_follow");
            assert_eq!(systems[0]["order"], 0);
            assert_eq!(systems[1]["name"], "first");
            assert_eq!(systems[1]["order"], 1);
            assert_eq!(systems[2]["name"], "second");
            assert_eq!(systems[2]["order"], 2);
            assert_eq!(systems[3]["name"], "camera_rig");
            assert_eq!(systems[3]["order"], 3);
            // `fn_path` is now exposed: the Rust module path of the registered fn (this test module).
            let systems = &systems[1..];
            let fn_path = systems[0]["fn_path"].as_str().unwrap();
            assert!(
                fn_path.contains("noop_a"),
                "fn_path should name the registered fn: {fn_path}"
            );
            // A system that declared no queries carries an empty `queries` array.
            assert_eq!(systems[0]["queries"].as_array().unwrap().len(), 0);
            // The wiring call site is this test file; the path is forward-slash normalized, so a
            // suffix match holds on every OS.
            let file = systems[0]["registered_at"]["file"].as_str().unwrap();
            assert!(
                !file.contains('\\'),
                "path not forward-slash normalized: {file}"
            );
            assert!(file.ends_with("dispatch.rs"), "unexpected file: {file}");
            assert!(systems[0]["registered_at"]["line"].as_u64().unwrap() > 0);
        }
        other => panic!("{other:?}"),
    }
}

/// `Plan/physics.md` W2: the physics triple describes after every game system, only when the
/// loaded scene declared a `physics:` block.
#[test]
fn engine_describe_carries_the_physics_triple_after_game_systems() {
    let mut engine = Engine::builder()
        .system("first", noop_a)
        .system("second", noop_b)
        .build()
        .unwrap();
    engine
        .load_inline_scene("format: 1\nphysics:\n  space: free\nentities: []\n")
        .unwrap();
    match ok(&mut engine, Command::EngineDescribe) {
        ResponseBody::EngineDescription { description } => {
            let systems = description["systems"].as_array().unwrap();
            assert_eq!(systems.len(), 7);
            assert_eq!(systems[0]["name"], "path_follow");
            assert_eq!(systems[1]["name"], "first");
            assert_eq!(systems[2]["name"], "second");
            assert_eq!(systems[3]["name"], "physics_write");
            assert_eq!(systems[3]["order"], 3);
            assert!(systems[3]["registered_at"].is_null());
            assert_eq!(systems[4]["name"], "physics_step");
            assert_eq!(systems[4]["order"], 4);
            assert!(systems[4]["registered_at"].is_null());
            assert_eq!(systems[5]["name"], "physics_mirror");
            assert_eq!(systems[5]["order"], 5);
            assert!(systems[5]["registered_at"].is_null());
            assert_eq!(systems[6]["name"], "camera_rig");
            let mirror_queries = systems[5]["queries"].as_array().unwrap();
            assert!(
                mirror_queries.iter().any(|query| {
                    query["components"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|c| c["name"] == "Transform" && c["access"] == "write")
                }),
                "physics_mirror's queries must name Transform as write: {mirror_queries:?}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema, Clone, Copy)]
struct Loadout {
    rate: f32,
}
#[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema, Clone, Copy)]
struct Health {
    hp: f32,
}

fn fire(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
    for (_loadout,) in world.query::<(&Loadout,)>() {}
    for (_health,) in world.query::<(&mut Health,)>() {}
}

#[test]
fn engine_describe_carries_declared_queries() {
    let mut engine = Engine::builder()
        .components(|r| {
            r.register::<Loadout>("Loadout")?;
            r.register::<Health>("Health")
        })
        .system("fire", fire)
        .query::<(&Loadout, &Transform)>()
        .query::<(&mut Health,)>()
        .build()
        .unwrap();
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .unwrap();
    match ok(&mut engine, Command::EngineDescribe) {
        ResponseBody::EngineDescription { description } => {
            let queries = description["systems"][1]["queries"].as_array().unwrap();
            assert_eq!(
                queries.len(),
                2,
                "two declared queries, in declaration order"
            );
            // Query 0: (&Loadout, &Transform) — both reads, in tuple order.
            let q0 = queries[0]["components"].as_array().unwrap();
            assert_eq!(q0[0]["name"], "Loadout");
            assert_eq!(q0[0]["access"], "read");
            assert_eq!(q0[1]["name"], "Transform");
            assert_eq!(q0[1]["access"], "read");
            // Query 1: (&mut Health,) — a write.
            let q1 = queries[1]["components"].as_array().unwrap();
            assert_eq!(q1[0]["name"], "Health");
            assert_eq!(q1[0]["access"], "write");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn declared_query_naming_unregistered_component_fails_build_l1125() {
    // `Loadout` is never registered, so declaring a query over it is an L1125 build error.
    let err = match Engine::builder()
        .system("fire", fire)
        .query::<(&Loadout,)>()
        .build()
    {
        Err(err) => err,
        Ok(_) => panic!("a declared query over an unregistered component must fail the build"),
    };
    assert_eq!(err.code.as_str(), "L1125");
    assert!(
        err.message.contains("fire"),
        "names the system: {}",
        err.message
    );
    assert!(
        err.message.contains("Loadout"),
        "names the offending type: {}",
        err.message
    );
}

#[test]
fn engine_describe_bare_engine_lists_only_the_engine_systems() {
    let mut engine = engine();
    match ok(&mut engine, Command::EngineDescribe) {
        ResponseBody::EngineDescription { description } => {
            let names: Vec<&str> = description["systems"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s["name"].as_str().unwrap())
                .collect();
            assert_eq!(names, ["path_follow", "camera_rig"]);
        }
        other => panic!("{other:?}"),
    }
}

// --- snapshot / restore determinism --------------------------------------------------------------

#[test]
fn snapshot_restore_reproduces_world_hash_and_rng() {
    let mut e1 = Engine::new();
    e1.load_inline_scene("format: 1\nseed: 3\nentities: []\n")
        .unwrap();

    // Exercise generational identity: spawn, spawn, destroy the first, spawn again (reuses slot 0
    // at generation 1). WorldHash includes generations, so restore must reproduce them exactly.
    let a = spawn_one(
        &mut e1,
        "Transform",
        json!({ "translation": [1.0, 0.0, 0.0] }),
    );
    let _b = spawn_one(&mut e1, "Mesh", json!({}));
    ok(
        &mut e1,
        Command::EntityDestroy(EntityDestroyParams { entity: a }),
    );
    let _c = spawn_one(&mut e1, "Sprite", json!({}));

    // Advance the RNG to a non-initial state so its round-trip is meaningful.
    for _ in 0..5 {
        e1.world_mut()
            .resource_mut::<DeterministicRng>()
            .unwrap()
            .next_u64();
    }

    let snapshot = match ok(&mut e1, Command::EngineSnapshot) {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("{other:?}"),
    };

    let mut e2 = Engine::new();
    // A clean restore reports an empty migration (no component-schema drift between the two engines).
    match ok(
        &mut e2,
        Command::EngineRestore(EngineRestoreParams { snapshot }),
    ) {
        ResponseBody::Restored { report } => assert!(
            report.is_clean(),
            "restoring into the same build must be a clean migration: {report:?}",
        ),
        other => panic!("{other:?}"),
    }

    // Observable world is identical.
    assert_eq!(e1.world_hash().unwrap(), e2.world_hash().unwrap());
    assert_eq!(e1.frame(), e2.frame());

    // The run's seed comes back as a resource, so a derived stream taken after the restore matches
    // one taken before it. The snapshot's long-standing `seed` field carries it — no new bytes.
    assert_eq!(
        e2.world().resource::<WorldSeed>().copied(),
        Some(WorldSeed(3)),
    );
    assert_eq!(
        e1.world().resource::<WorldSeed>().copied(),
        e2.world().resource::<WorldSeed>().copied(),
    );

    // The RNG continues identically from the restore point.
    let r1 = e1
        .world_mut()
        .resource_mut::<DeterministicRng>()
        .unwrap()
        .next_u64();
    let r2 = e2
        .world_mut()
        .resource_mut::<DeterministicRng>()
        .unwrap()
        .next_u64();
    assert_eq!(r1, r2);

    // Stepping N ticks keeps them in lockstep (empty schedule → world static, hash equal).
    ok(
        &mut e1,
        Command::SimStep(SimStepParams {
            ticks: 8,
            hash: false,
        }),
    );
    ok(
        &mut e2,
        Command::SimStep(SimStepParams {
            ticks: 8,
            hash: false,
        }),
    );
    assert_eq!(e1.world_hash().unwrap(), e2.world_hash().unwrap());
}

/// **T14's snapshot half** (`Plan/gi-wave3-dynamic-injection.md` §11): `engine.snapshot` walks
/// every registered component generically (`for info in engine.registry().iter()`,
/// `crates/ludion-engine/src/snapshot.rs::capture`) — there is no per-component list a new
/// component must be added to, so `GiDynamic` round-trips for free. Proven rather than assumed: an
/// entity carrying the marker restores into a fresh `Engine` with the component still present and a
/// clean (drift-free) migration report.
#[test]
fn snapshot_restore_carries_the_gi_dynamic_marker() {
    let mut e1 = engine();
    let marked = spawn_one(&mut e1, "GiDynamic", json!({}));

    let snapshot = match ok(&mut e1, Command::EngineSnapshot) {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("{other:?}"),
    };

    let mut e2 = Engine::new();
    match ok(
        &mut e2,
        Command::EngineRestore(EngineRestoreParams { snapshot }),
    ) {
        ResponseBody::Restored { report } => {
            assert!(
                report.is_clean(),
                "GiDynamic must restore drift-free: {report:?}"
            )
        }
        other => panic!("{other:?}"),
    }

    match ok(
        &mut e2,
        Command::ComponentGet(ComponentGetParams {
            entity: marked,
            component: "GiDynamic".into(),
        }),
    ) {
        ResponseBody::ComponentValue { value } => {
            assert_eq!(value, json!({}), "the empty marker survives the round trip")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(e1.world_hash().unwrap(), e2.world_hash().unwrap());
}

/// **The GI rebake lever is host/session state, not world state.** It is a `Renderer`-side switch a
/// live host flips (the editor's Hold/Rebake affordance); nothing about it may reach the simulation,
/// or a session held while recording would replay differently than it ran.
///
/// Three claims, one fixture: an `engine.snapshot` taken while held carries no trace of the lever
/// (so a snapshot cannot smuggle it), a `restore` of that snapshot leaves the receiving engine's own
/// lever exactly as it was (in *both* directions — Auto stays Auto, Hold stays Hold), and the two
/// engines' `WorldHash`es agree throughout.
#[test]
fn the_gi_rebake_lever_is_host_state_and_no_snapshot_carries_it() {
    let mut held = engine();
    held.set_gi_rebake(GiRebakeMode::Hold);
    held.gi_rebake_now();
    spawn_one(
        &mut held,
        "Transform",
        json!({ "translation": [1.0, 2.0, 3.0] }),
    );

    let snapshot = match ok(&mut held, Command::EngineSnapshot) {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("{other:?}"),
    };
    let text = serde_json::to_string(&snapshot).unwrap();
    for word in ["gi_rebake", "giRebake", "hold", "Hold", "gi_stale"] {
        assert!(
            !text.contains(word),
            "a snapshot must carry no spelling of the lever, found `{word}` in {text}",
        );
    }

    // Restoring into an `Auto` engine leaves it at `Auto` — the snapshot cannot set the lever.
    let mut fresh = Engine::new();
    assert_eq!(fresh.gi_rebake_mode(), GiRebakeMode::Auto);
    match ok(
        &mut fresh,
        Command::EngineRestore(EngineRestoreParams {
            snapshot: snapshot.clone(),
        }),
    ) {
        ResponseBody::Restored { report } => assert!(report.is_clean(), "{report:?}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        fresh.gi_rebake_mode(),
        GiRebakeMode::Auto,
        "a restore must not import the source host's lever",
    );

    // And the other direction: a restore does not clear a receiving host's own hold.
    let mut receiving = Engine::new();
    receiving.set_gi_rebake(GiRebakeMode::Hold);
    match ok(
        &mut receiving,
        Command::EngineRestore(EngineRestoreParams { snapshot }),
    ) {
        ResponseBody::Restored { report } => assert!(report.is_clean(), "{report:?}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(receiving.gi_rebake_mode(), GiRebakeMode::Hold);

    let hash = held.world_hash().unwrap();
    assert_eq!(hash, fresh.world_hash().unwrap());
    assert_eq!(hash, receiving.world_hash().unwrap());
}

// --- engine.shutdown -----------------------------------------------------------------------------

/// In-process (no host) `engine.shutdown` acks and sets an observable flag — it never exits the test
/// process. A host (`run_live`) is what acts on the flag; a bare engine just records the request.
#[test]
fn engine_shutdown_acks_and_flags_without_exiting() {
    let mut engine = engine();
    assert!(!engine.shutdown_requested());
    assert_eq!(ok(&mut engine, Command::EngineShutdown), ResponseBody::Ack);
    assert!(
        engine.shutdown_requested(),
        "shutdown must be observable after the command",
    );
    // The engine is still fully usable in-process — the flag is advisory to a host, not a kill.
    assert!(engine.world_hash().is_ok());
}

// --- snapshot / restore migration ----------------------------------------------------------------

/// Helper: the `body` of a `component.get`, or `None` if the entity does not carry the component.
fn get_component(engine: &mut Engine, entity: EntityId, component: &str) -> Option<Value> {
    match dispatch(
        engine,
        Command::ComponentGet(ComponentGetParams {
            entity,
            component: component.to_string(),
        }),
    ) {
        Ok(dispatched) => match dispatched.body {
            ResponseBody::ComponentValue { value } => Some(value),
            other => panic!("expected ComponentValue, got {other:?}"),
        },
        Err(diag) => {
            // L3006 = the entity is alive and the component registered, but absent (dropped).
            assert_eq!(diag.code.as_str(), "L3006", "unexpected error: {diag}");
            None
        }
    }
}

/// Restore of a doctored snapshot: an unknown component drops with L3015, a value that no longer
/// deserializes drops with L3016 (carrying the serde error + entity id), and a component missing a
/// now-defaulted field is filled from the default with that field named in the report. The exact
/// report shape is asserted, and the world comes up running (drops applied, defaults filled).
#[test]
fn restore_migrates_drift_into_a_report_and_a_running_world() {
    let mut e1 = engine();
    // Three entities, each carrying one Transform. Ascending slot order 0,1,2 is the report order.
    let a = spawn_one(
        &mut e1,
        "Transform",
        json!({ "translation": [1.0, 2.0, 3.0] }),
    );
    let b = spawn_one(
        &mut e1,
        "Transform",
        json!({ "translation": [4.0, 5.0, 6.0] }),
    );
    let c = spawn_one(
        &mut e1,
        "Transform",
        json!({ "translation": [7.0, 8.0, 9.0] }),
    );

    let mut snapshot = match ok(&mut e1, Command::EngineSnapshot) {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("{other:?}"),
    };

    // Doctor the snapshot to simulate a recompiled build's schema drift.
    let entities = snapshot["entities"].as_array_mut().unwrap();
    // (a) entity A: a component this build no longer registers -> dropped, L3015.
    entities[0]["components"]
        .as_object_mut()
        .unwrap()
        .insert("OldGizmo".to_string(), json!({ "wobble": 3 }));
    // (b) entity B: a Transform value that no longer deserializes -> dropped, L3016.
    entities[1]["components"]["Transform"]["translation"] = json!("not-a-vector");
    // (c) entity C: a Transform missing its now-defaulted `scale` -> filled from default.
    entities[2]["components"]["Transform"]
        .as_object_mut()
        .unwrap()
        .remove("scale");

    let mut e2 = Engine::new();
    let report = match ok(
        &mut e2,
        Command::EngineRestore(EngineRestoreParams { snapshot }),
    ) {
        ResponseBody::Restored { report } => report,
        other => panic!("{other:?}"),
    };

    // The exact report shape: one entry per divergence, in restore order.
    assert_eq!(report.entries.len(), 3, "report: {report:?}");
    assert_eq!(
        report.entries[0],
        RestoreEntry::ComponentUnregistered {
            entity: a,
            component: "OldGizmo".to_string(),
        },
    );
    match &report.entries[1] {
        RestoreEntry::ComponentUndeserializable {
            entity,
            component,
            error,
        } => {
            assert_eq!(*entity, b);
            assert_eq!(component, "Transform");
            assert!(!error.is_empty(), "L3016 must carry the serde error text");
        }
        other => panic!("expected ComponentUndeserializable, got {other:?}"),
    }
    assert_eq!(
        report.entries[2],
        RestoreEntry::ComponentMigrated {
            entity: c,
            component: "Transform".to_string(),
            filled: vec!["scale".to_string()],
            removed: vec![],
        },
    );

    // The codes are the documented ones (an agent can `ludion explain` them).
    assert_eq!(report.entries[0].code(), Some("L3015"));
    assert_eq!(report.entries[1].code(), Some("L3016"));
    assert_eq!(report.entries[2].code(), None);

    // The world came up running: the drops applied, and the missing field filled from its default.
    assert!(e2.world_hash().is_ok(), "restored world must be valid");
    assert!(
        get_component(&mut e2, a, "Transform").is_some(),
        "A keeps its Transform; only the unknown OldGizmo was dropped",
    );
    assert!(
        get_component(&mut e2, b, "Transform").is_none(),
        "B's undeserializable Transform was dropped entirely (no invented state)",
    );
    let c_transform = get_component(&mut e2, c, "Transform").expect("C keeps its Transform");
    assert_eq!(
        c_transform["scale"],
        json!([1.0, 1.0, 1.0]),
        "C's missing scale filled from the serde default",
    );
    assert_eq!(
        c_transform["translation"],
        json!([7.0, 8.0, 9.0]),
        "C's other fields restored verbatim",
    );
}

#[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema, Clone, Copy, Default)]
struct NestedInner {
    a: u32,
    #[serde(default)]
    b: u32,
}
#[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema, Clone, Copy, Default)]
struct Nested {
    inner: NestedInner,
}

/// A nested field serde filled or dropped shows in the restore report as a dotted path.
#[test]
fn restore_reports_nested_field_drift_as_dotted_paths() {
    let build = || {
        let mut engine = Engine::builder()
            .components(|r| r.register::<Nested>("Nested"))
            .build()
            .unwrap();
        engine
            .load_inline_scene("format: 1\nseed: 7\nentities: []\n")
            .unwrap();
        engine
    };
    let mut e1 = build();
    let entity = spawn_one(&mut e1, "Nested", json!({ "inner": { "a": 1, "b": 2 } }));
    let mut snapshot = match ok(&mut e1, Command::EngineSnapshot) {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("{other:?}"),
    };
    let inner = snapshot["entities"][0]["components"]["Nested"]["inner"]
        .as_object_mut()
        .unwrap();
    inner.remove("b");
    inner.insert("legacy".to_string(), json!(9));

    let mut e2 = build();
    let report = match ok(
        &mut e2,
        Command::EngineRestore(EngineRestoreParams { snapshot }),
    ) {
        ResponseBody::Restored { report } => report,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        report.entries,
        [RestoreEntry::ComponentMigrated {
            entity,
            component: "Nested".to_string(),
            filled: vec!["inner.b".to_string()],
            removed: vec!["inner.legacy".to_string()],
        }],
    );
}

/// A snapshot from another engine version, or one with a top-level field this build does not read,
/// restores with a warning entry each — never a refusal — and the world comes up intact.
#[test]
fn restore_warns_on_engine_version_and_unknown_snapshot_fields() {
    let mut e1 = engine();
    let entity = spawn_one(
        &mut e1,
        "Transform",
        json!({ "translation": [1.0, 2.0, 3.0] }),
    );
    let snapshot = match ok(&mut e1, Command::EngineSnapshot) {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("{other:?}"),
    };
    let restore = |snapshot: Value| {
        let mut e2 = Engine::new();
        let report = match ok(
            &mut e2,
            Command::EngineRestore(EngineRestoreParams { snapshot }),
        ) {
            ResponseBody::Restored { report } => report,
            other => panic!("{other:?}"),
        };
        assert!(
            get_component(&mut e2, entity, "Transform").is_some(),
            "the world restores intact"
        );
        report
    };

    let mut other_version = snapshot.clone();
    other_version["engine_version"] = json!("9.9.9");
    let report = restore(other_version);
    assert_eq!(report.entries.len(), 1, "report: {report:?}");
    match &report.entries[0] {
        RestoreEntry::EngineVersionDiffers {
            snapshot_version, ..
        } => assert_eq!(snapshot_version, "9.9.9"),
        other => panic!("expected EngineVersionDiffers, got {other:?}"),
    }
    assert_eq!(report.entries[0].code(), Some("L3048"));

    let mut with_unknown = snapshot.clone();
    with_unknown["from_the_future"] = json!(1);
    let report = restore(with_unknown);
    assert_eq!(
        report.entries,
        [RestoreEntry::SnapshotFieldUnknown {
            field: "from_the_future".to_string()
        }],
    );
    assert_eq!(report.entries[0].code(), Some("L3049"));

    assert!(restore(snapshot).is_clean(), "a clean round-trip is empty");
}

/// A snapshot restored into the very build that produced it is a clean migration: an empty report.
#[test]
fn restore_of_an_undoctored_snapshot_reports_empty() {
    let mut e1 = engine();
    spawn_one(
        &mut e1,
        "Transform",
        json!({ "translation": [1.0, 0.0, 0.0] }),
    );
    spawn_one(&mut e1, "Sprite", json!({ "billboard": true }));

    let snapshot = match ok(&mut e1, Command::EngineSnapshot) {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("{other:?}"),
    };

    let mut e2 = Engine::new();
    match ok(
        &mut e2,
        Command::EngineRestore(EngineRestoreParams { snapshot }),
    ) {
        ResponseBody::Restored { report } => {
            assert!(report.is_clean(), "clean restore must be empty: {report:?}");
        }
        other => panic!("{other:?}"),
    }
}

/// A genuinely malformed snapshot document (not per-component drift) is still the hard `L3008` error —
/// the narrowing kept L3008 for real corruption, it did not remove it.
#[test]
fn restore_of_a_malformed_document_still_errors_l3008() {
    let mut engine = Engine::new();
    let code = err_code(
        &mut engine,
        Command::EngineRestore(EngineRestoreParams {
            snapshot: json!({ "not": "a snapshot" }),
        }),
    );
    assert_eq!(code, "L3008");
}

// --- replay round-trip ---------------------------------------------------------------------------

#[test]
fn replay_reproduces_the_world_hash_sequence() {
    // A session with a spawn, single-tick steps, a component.set, a component.remove, and an
    // injected input.
    let entity0 = EntityId::new(0, 0);
    let mut spawn_components = BTreeMap::new();
    spawn_components.insert(
        "Transform".to_string(),
        json!({ "translation": [1.0, 0.0, 0.0] }),
    );
    spawn_components.insert("Sprite".to_string(), json!({}));
    let commands = vec![
        Command::EntitySpawn(EntitySpawnParams {
            name: None,
            components: spawn_components,
        }),
        Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        }),
        Command::ComponentSet(ComponentSetParams {
            entity: entity0,
            component: "Transform".into(),
            value: json!({ "translation": [2.0, 0.0, 0.0] }),
        }),
        Command::ComponentRemove(ComponentRemoveParams {
            entity: entity0,
            component: "Sprite".into(),
        }),
        Command::InputInject(InputInjectParams {
            frame: None,
            event: json!({ "kind": "key_down", "key": "W" }),
        }),
        Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        }),
        Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        }),
    ];

    // Record the session while producing its per-frame hash trace.
    let sink = SharedSink::default();
    let mut engine = Engine::new();
    engine
        .load_inline_scene("format: 1\nseed: 7\nentities: []\n")
        .unwrap();
    engine.set_recording(sink.clone()).unwrap();
    let original = drive_hashes(&mut engine, commands).unwrap();
    engine.stop_recording().unwrap();

    // Replay it and compare the FULL per-frame WorldHash sequence.
    let replayed = replay_hashes(&sink.bytes(), ".").unwrap();
    assert_eq!(original, replayed);
    assert!(original.len() >= 4, "trace should span several frames");
}

#[test]
fn a_recorded_reparent_replays_to_the_same_hash_sequence() {
    let at = |x: f32| {
        let mut components = BTreeMap::new();
        components.insert(
            "Transform".to_string(),
            json!({ "translation": [x, 0.0, 0.0], "rotation": [0.0, 30.0, 0.0] }),
        );
        Command::EntitySpawn(EntitySpawnParams {
            name: None,
            components,
        })
    };
    let (first, second) = (EntityId::new(0, 0), EntityId::new(1, 0));
    let commands = vec![
        at(5.0),
        at(1.0),
        reparent_command(second, Some(first), ReparentKeep::World),
        Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        }),
        reparent_command(second, None, ReparentKeep::Local),
        Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        }),
    ];
    let sink = SharedSink::default();
    let mut engine = Engine::new();
    engine
        .load_inline_scene("format: 1\nseed: 7\nentities: []\n")
        .unwrap();
    engine.set_recording(sink.clone()).unwrap();
    let original = drive_hashes(&mut engine, commands).unwrap();
    engine.stop_recording().unwrap();
    assert_eq!(original, replay_hashes(&sink.bytes(), ".").unwrap());
}

/// The `L1018` message and help are user-facing text: `Parent`'s dangling case is byte-identical to
/// the wording from before the diagnostic was generalized to other reference fields.
#[test]
fn l1018_wording_is_pinned() {
    let fresh = || {
        let root = temp_project();
        let mut engine = Engine::with_project_root(&root);
        engine
            .load_inline_scene("format: 1\nentities: []\n")
            .unwrap();
        engine
    };

    let mut engine = fresh();
    let (child, parent) = spawn_dangling_child(&mut engine, "child");
    let error = save_err(&mut engine);
    assert_eq!(
        error.message,
        format!(
            "cannot save: entity {child} (`child`) has a `Parent` pointing at {parent}, which is not in the scene"
        )
    );
    assert_eq!(
        error.help.as_deref(),
        Some(
            format!(
                "repair the reference first — `component.set` the `Parent` on {child} to a live entity, or `component.remove` it"
            )
            .as_str()
        )
    );

    let mut engine = fresh();
    let a = spawn_named(&mut engine, "a");
    let b = spawn_named(&mut engine, "b");
    let carrier = spawn_joint(&mut engine, "carrier", a, b);
    ok(
        &mut engine,
        Command::EntityDestroy(EntityDestroyParams { entity: b }),
    );
    assert_eq!(
        save_err(&mut engine).help.as_deref(),
        Some(
            format!(
                "repair the reference first — `component.set` the `Joint` on {carrier} so `b` names a live entity, or `component.remove` it"
            )
            .as_str()
        )
    );

    let mut engine = fresh();
    let other = spawn_named(&mut engine, "other");
    let child = spawn_child(&mut engine, "child", other);
    set_self_reference(&mut engine, child, "Parent", None);
    assert_eq!(
        save_err(&mut engine).help.as_deref(),
        Some(
            format!(
                "repair the reference first — `component.set` the `Parent` on {child} to a live entity other than its carrier, or `component.remove` it"
            )
            .as_str()
        )
    );
}

/// A required entity reference whose serializer skips the field, so the saved value lacks it.
#[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema, Clone, Copy)]
struct Skipped {
    #[schemars(with = "ludion_ecs::EntityRefSchema")]
    #[serde(skip_serializing_if = "always")]
    target: EntityId,
}

fn always(_: &EntityId) -> bool {
    true
}

/// A component whose serialized value omits a required entity reference is a coded refusal on save
/// (`L0007`), not a panic in the remap.
#[test]
fn a_required_reference_missing_from_the_serialized_value_is_l0007_on_save() {
    let root = temp_project();
    let mut engine = Engine::builder()
        .components(|r| r.register::<Skipped>("Skipped"))
        .project_root(&root)
        .build()
        .unwrap();
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .unwrap();
    let info = engine.registry().get("Skipped").expect("registered");
    assert!(
        !info.entity_refs()[0].optional,
        "the field must reach the required branch"
    );
    let other = spawn_named(&mut engine, "other");
    spawn_with(
        &mut engine,
        "holder",
        "Skipped",
        json!({ "target": { "index": other.index, "generation": other.generation } }),
    );
    let error = dispatch(
        &mut engine,
        Command::SceneSave(SceneSaveParams {
            path: "out.yaml".into(),
        }),
    )
    .expect_err("the save should be refused");
    assert_eq!(error.code.as_str(), "L0007");
    assert!(error.message.contains("`Skipped`"), "{}", error.message);
}
