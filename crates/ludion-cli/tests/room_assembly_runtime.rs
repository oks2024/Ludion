//! `spawn_map`: a room compiled from its `kind: map` at runtime, with a kit, seed and mirror the
//! caller picks. It equals the `generate-map --prefab` file spawned with `spawn_prefab`, hashes the
//! same at any thread count, replays after its map and kit are deleted, and refreshes on reload.

use std::collections::BTreeMap;
use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::diagnostics::Diagnostics;
use ludion_engine::{
    Engine, MapSpawn, Mirror, NavAgent, PrefabInstance, Transform, despawn_instance,
    replay_hashes_with, spawn_map, spawn_prefab,
};
use ludion_protocol::Command;
use ludion_protocol::command::{
    AssetReloadParams, PrefabApplyParams, PrefabOverridesParams, PrefabRevertParams,
    SceneSaveParams, SimStepParams,
};
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use serde_json::Value;

use crate::room_assembly::{
    KIT, MAP, PLACEMENT, RESPAWN, SPAWN, SharedSink, Spawned, T, TEARDOWN, WALKERS,
    assert_no_query_drift, booted_with, floor_map, generate_with, node_at_t, room_root, scene,
    spawned, tick_to,
};

const MAP_PATH: &str = "room.map.yaml";
const KIT2_PATH: &str = "kit2.kit.yaml";

/// The default kit with the walls two cells higher: a different room from the same map.
fn kit2() -> String {
    let taller = KIT.replace("      y: 1.0\n", "      y: 3.0\n");
    assert_ne!(taller, KIT, "the wall height changed");
    taller
}

/// A scratch project holding the map, the default kit and the taller one.
fn root(tag: &str) -> std::path::PathBuf {
    let dir = room_root(tag);
    std::fs::write(dir.join(KIT2_PATH), kit2()).unwrap();
    dir
}

fn placement() -> Transform {
    Transform {
        translation: PLACEMENT,
        ..Transform::default()
    }
}

/// The spawn variants: the map as authored, a kit and seed override, and a mirrored override.
fn variant(v: u8) -> MapSpawn<'static> {
    let base = MapSpawn {
        placement: Some(placement()),
        name: Some("r"),
        ..MapSpawn::default()
    };
    match v {
        0 => base,
        1 => MapSpawn {
            kit: Some(KIT2_PATH),
            seed: Some(5),
            ..base
        },
        _ => MapSpawn {
            kit: Some(KIT2_PATH),
            seed: Some(5),
            mirror: Mirror::X,
            ..base
        },
    }
}

fn spawn_variant(world: &mut World, v: u8) {
    if let Ok(ids) = spawn_map(world, MAP_PATH, variant(v)) {
        let mut spawned = world.remove_resource::<Spawned>().unwrap_or_default();
        spawned.0.push(ids);
        world.insert_resource(spawned);
    }
}

/// The spawner system for variant `V`: spawn, tear down, respawn.
fn spawner<const V: u8>(world: &mut World, ctx: &SystemCtx) {
    match ctx.frame.as_u64() {
        SPAWN | RESPAWN => spawn_variant(world, V),
        TEARDOWN => {
            if let Some(root) = world.resource::<Spawned>().map(|s| s.0[0][0]) {
                despawn_instance(world, root).expect("the room's root is alive");
            }
        }
        _ => {}
    }
}

fn hash_trace(root: &Path, v: u8, ticks: usize) -> Vec<u64> {
    let spawner: fn(&mut World, &SystemCtx) = match v {
        0 => spawner::<0>,
        1 => spawner::<1>,
        _ => spawner::<2>,
    };
    let mut engine = booted_with(root, spawner);
    (0..ticks)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

/// Every registered component of each part, with the prefab key left out of `PrefabInstance`.
fn dump(engine: &Engine, ids: &[EntityId]) -> Vec<BTreeMap<String, Value>> {
    ids.iter()
        .map(|&id| {
            engine
                .registry()
                .iter()
                .filter_map(|info| {
                    let mut value = info.serialize(engine.world(), id).unwrap()?;
                    if info.name() == "PrefabInstance" {
                        value.as_object_mut().unwrap().remove("source");
                    }
                    Some((info.name().to_owned(), value))
                })
                .collect()
        })
        .collect()
}

fn bare(root: &Path) -> Engine {
    Engine::builder().project_root(root).build().expect("boots")
}

fn codes(found: &[Diagnostic]) -> Vec<String> {
    found.iter().map(|d| d.code.to_string()).collect()
}

#[test]
fn a_runtime_room_equals_its_generated_prefab_entity_for_entity() {
    let root = root("room_runtime_equivalence");
    let variants: [(&[&str], u8); 4] = [
        (&[], 0),
        (&["--kit", KIT2_PATH, "--seed", "5"], 1),
        (&["--mirror", "x"], 3),
        (&["--mirror", "z"], 4),
    ];
    for (flags, v) in variants {
        let mut args: Vec<String> = flags.iter().map(|f| (*f).to_owned()).collect();
        if let Some(at) = args.iter().position(|f| f == KIT2_PATH) {
            args[at] = root.join(KIT2_PATH).to_str().unwrap().to_owned();
        }
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let prefab = format!("rooms/v{v}.prefab.yaml");
        generate_with(&root, MAP_PATH, &prefab, &args);

        let mut engine = bare(&root);
        let from_prefab = spawn_prefab(
            engine.world_mut(),
            &prefab,
            Some(placement()),
            Some("r"),
            false,
        )
        .unwrap();
        let expected = dump(&engine, &from_prefab);

        let mut fresh = bare(&root);
        let spawn = MapSpawn {
            placement: Some(placement()),
            name: Some("r"),
            kit: flags.contains(&"--kit").then_some(KIT2_PATH),
            seed: flags.contains(&"--seed").then_some(5),
            mirror: match flags.last() {
                Some(&"x") => Mirror::X,
                Some(&"z") => Mirror::Z,
                _ => Mirror::None,
            },
        };
        let from_map = spawn_map(fresh.world_mut(), MAP_PATH, spawn).unwrap();
        assert_eq!(from_map.len(), from_prefab.len(), "{flags:?}: part count");
        assert_eq!(dump(&fresh, &from_map), expected, "{flags:?}: components");
        let sources: Vec<_> = from_map
            .iter()
            .map(|&id| {
                fresh
                    .world()
                    .get::<PrefabInstance>(id)
                    .unwrap()
                    .unwrap()
                    .source
                    .clone()
            })
            .collect();
        assert!(
            sources.iter().all(|s| s == MAP_PATH),
            "source is the map key"
        );
    }
}

/// The box colliders of a room: `(translation, scale, half_extents)` in thousandths.
fn colliders(engine: &mut Engine, ids: &[EntityId]) -> Vec<[i64; 9]> {
    let milli = |v: &Value| -> [i64; 3] {
        let a = v.as_array().unwrap();
        [0, 1, 2].map(|i| (a[i].as_f64().unwrap() * 1000.0).round() as i64)
    };
    let mut out = Vec::new();
    for part in dump(engine, ids) {
        let Some(collider) = part.get("Collider") else {
            continue;
        };
        let transform = &part["Transform"];
        let half = &collider["shape"]["box"]["half_extents"];
        let [tx, ty, tz] = milli(&transform["translation"]);
        let [sx, sy, sz] = milli(&transform["scale"]);
        let [hx, hy, hz] = milli(half);
        out.push([tx, ty, tz, sx, sy, sz, hx, hy, hz]);
    }
    out
}

#[test]
fn a_mirrored_room_keeps_unit_scale_colliders_at_the_reflected_places() {
    let root = root("room_runtime_mirror");
    let plain_spawn = MapSpawn::default();
    let mut engine = bare(&root);
    let plain = spawn_map(engine.world_mut(), MAP_PATH, plain_spawn).unwrap();
    let mut plain = colliders(&mut engine, &plain);

    let mut mirrored_engine = bare(&root);
    let mirrored_spawn = MapSpawn {
        mirror: Mirror::X,
        ..MapSpawn::default()
    };
    let mirrored = spawn_map(mirrored_engine.world_mut(), MAP_PATH, mirrored_spawn).unwrap();
    let mut mirrored = colliders(&mut mirrored_engine, &mirrored);

    assert!(!plain.is_empty(), "the room has box colliders");
    assert!(
        mirrored.iter().all(|c| c[3..6] == [1000, 1000, 1000]),
        "every collider sits on a unit-scale entity: {mirrored:?}"
    );
    // A 6-cell, 2-unit grid reflects x across 6.0.
    for c in &mut plain {
        c[0] = 12_000 - c[0];
    }
    plain.sort_unstable();
    mirrored.sort_unstable();
    assert_eq!(
        mirrored, plain,
        "the mirrored colliders are the reflected ones"
    );

    let root = room_root("room_runtime_mirror_ticks");
    std::fs::write(root.join(KIT2_PATH), kit2()).unwrap();
    let mut engine = booted_with(&root, spawner::<2>);
    tick_to(&mut engine, SPAWN + 5);
    assert_eq!(spawned(&engine).len(), 1, "the mirrored room spawned");
    let diagnostics = engine.world().resource::<Diagnostics>().unwrap();
    assert_eq!(
        diagnostics.reported(code::COLLIDER_SCALE_IGNORED).count(),
        0,
        "L1290: a mirrored collider kept a non-unit world scale"
    );
}

#[test]
fn a_runtime_room_hashes_the_same_at_one_and_eight_threads() {
    let root = root("room_runtime_threads");
    let one = par::with_threads(1, || hash_trace(&root, 2, 340));
    let again = par::with_threads(1, || hash_trace(&root, 2, 340));
    let eight = par::with_threads(8, || hash_trace(&root, 2, 340));
    assert_eq!(one, again, "two 1-thread runs");
    assert_eq!(one, eight, "1 and 8 threads");
    let spawn_tick = SPAWN as usize;
    assert!(
        (spawn_tick - 1..=spawn_tick + 1).any(|i| one[i] != one[i - 1]),
        "the spawn moves the trace"
    );

    let mut engine = par::with_threads(1, || booted_with(&root, spawner::<2>));
    par::with_threads(1, || tick_to(&mut engine, 340));
    let spawns = spawned(&engine);
    assert_eq!(spawns.len(), 2);
    assert_eq!(spawns[0].len(), spawns[1].len(), "same part count");
    assert_no_query_drift(&engine);
}

#[test]
fn walkers_path_into_a_runtime_room_and_not_into_a_torn_down_one() {
    let root = root("room_runtime_nav");
    let mut engine = booted_with(&root, spawner::<0>);

    tick_to(&mut engine, SPAWN + 1);
    assert_eq!(spawned(&engine).len(), 1, "the room spawned on this tick");
    let before = floor_map(&engine);
    assert!(node_at_t(&engine).is_none(), "no floor on the spawn tick");

    tick_to(&mut engine, SPAWN + 2);
    let at = node_at_t(&engine).expect("the floor is in the grid the tick after the spawn");
    assert!(at[1].abs() < 0.1, "floor top at y = 0: {at:?}");

    tick_to(&mut engine, TEARDOWN);
    let near = engine
        .world_mut()
        .query_ids::<(&Transform, &NavAgent)>()
        .filter(|(_, (t, _))| {
            let d = [t.translation[0] - T[0], t.translation[2] - T[2]];
            (d[0] * d[0] + d[1] * d[1]).sqrt() < 1.5
        })
        .count();
    assert!(near >= 16, "{near} of {WALKERS} walkers reached the target");

    tick_to(&mut engine, TEARDOWN + 2);
    assert!(
        node_at_t(&engine).is_none(),
        "the torn-down floor leaves the grid"
    );
    assert_eq!(
        floor_map(&engine),
        before,
        "the grid is back to its pre-spawn floor"
    );
    assert_no_query_drift(&engine);
}

#[test]
fn a_runtime_room_replays_after_its_map_and_kit_are_deleted() {
    let root = root("room_runtime_replay");
    let sink = SharedSink::default();
    let mut engine = booted_with(&root, spawner::<1>);
    let mut live = vec![(engine.frame(), engine.world_hash().unwrap())];
    engine.set_recording(sink.clone()).unwrap();
    while engine.frame().as_u64() < TEARDOWN + 30 {
        engine
            .execute(Command::SimStep(SimStepParams {
                ticks: 1,
                hash: false,
            }))
            .unwrap();
        live.push((engine.frame(), engine.world_hash().unwrap()));
    }
    engine.stop_recording().unwrap();
    let bytes = sink.0.lock().unwrap().clone();

    let texts: BTreeMap<String, String> = ReplayReader::new(&bytes[..])
        .unwrap()
        .records()
        .unwrap()
        .into_iter()
        .filter_map(|record| match record {
            ReplayRecord::PrefabText { path, document, .. } => Some((path, document)),
            _ => None,
        })
        .collect();
    assert_eq!(texts.get(MAP_PATH).map(String::as_str), Some(MAP));
    assert_eq!(texts.get(KIT2_PATH), Some(&kit2()));

    for gone in [MAP_PATH, KIT2_PATH, "kit.kit.yaml"] {
        std::fs::remove_file(root.join(gone)).unwrap();
    }
    let spawner: fn(&mut World, &SystemCtx) = spawner::<1>;
    let replayed = replay_hashes_with(
        crate::room_assembly::game_with(&root, spawner),
        &bytes,
        &root,
    )
    .unwrap();
    assert_eq!(replayed, live);
}

#[test]
fn a_preloaded_map_spawns_after_its_files_are_deleted() {
    let root = root("room_runtime_preload");
    let with_preload = scene().replacen("entities:", "preload:\n  - room.map.yaml\nentities:", 1);
    let mut engine = crate::room_assembly::game_with(&root, spawner::<9>)
        .build()
        .unwrap();
    engine.load_inline_scene(&with_preload).unwrap();
    let mut cold = crate::room_assembly::game_with(&root, spawner::<9>)
        .build()
        .unwrap();
    cold.load_inline_scene(&scene()).unwrap();

    for gone in [MAP_PATH, "kit.kit.yaml"] {
        std::fs::remove_file(root.join(gone)).unwrap();
    }
    let ids = spawn_map(engine.world_mut(), MAP_PATH, MapSpawn::default());
    assert!(ids.is_ok(), "preload warmed the map and its kit: {ids:?}");
    let refused = spawn_map(cold.world_mut(), MAP_PATH, MapSpawn::default()).unwrap_err();
    assert_eq!(
        codes(&refused),
        ["L1406"],
        "without preload the file is needed"
    );
}

#[test]
fn reloading_a_kit_changes_the_next_runtime_room_and_no_live_one() {
    let root = root("room_runtime_reload");
    let mut engine = bare(&root);
    let first = spawn_map(engine.world_mut(), MAP_PATH, MapSpawn::default()).unwrap();
    let first_dump = dump(&engine, &first);

    std::fs::write(root.join("kit.kit.yaml"), kit2()).unwrap();
    engine
        .execute(Command::AssetReload(AssetReloadParams {
            path: Some("kit.kit.yaml".to_owned()),
        }))
        .unwrap();

    let second = spawn_map(engine.world_mut(), MAP_PATH, MapSpawn::default()).unwrap();
    let second_dump = dump(&engine, &second);
    let top = |parts: &[BTreeMap<String, Value>]| {
        parts
            .iter()
            .map(|p| (p["Transform"]["translation"][1].as_f64().unwrap() * 1000.0) as i64)
            .max()
            .unwrap()
    };
    assert!(
        top(&second_dump) > top(&first_dump),
        "the new kit's taller walls"
    );
    assert_eq!(
        dump(&engine, &first),
        first_dump,
        "the live room is unchanged"
    );
}

#[test]
fn a_bad_map_is_refused_and_spawns_nothing() {
    let root = root("room_runtime_errors");
    std::fs::write(
        root.join("phys.map.yaml"),
        format!("{MAP}physics:\n  space: free\n"),
    )
    .unwrap();
    std::fs::write(
        root.join("frag.scene.yaml"),
        "format: 1\nentities:\n  - name: lamp\n    components: {}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("frag.map.yaml"),
        format!("{MAP}fragments:\n  - frag.scene.yaml\n"),
    )
    .unwrap();
    let mut engine = bare(&root);
    let count = engine.world().entity_count();

    let refused = |engine: &mut Engine, map: &str, spawn: MapSpawn<'_>| {
        let found = spawn_map(engine.world_mut(), map, spawn).unwrap_err();
        assert_eq!(
            engine.world().entity_count(),
            count,
            "{map}: nothing spawned"
        );
        codes(&found)
    };
    let abs = root.join(MAP_PATH).to_str().unwrap().replace('\\', "/");
    assert_eq!(refused(&mut engine, &abs, MapSpawn::default()), ["L1406"]);
    assert_eq!(
        refused(&mut engine, "../outside.map.yaml", MapSpawn::default()),
        ["L1406"]
    );
    assert_eq!(
        refused(&mut engine, "rooms/room.prefab.yaml", MapSpawn::default()),
        ["L1406"],
        "a prefab is not a map"
    );
    let kit_is_a_map = MapSpawn {
        kit: Some(MAP_PATH),
        ..MapSpawn::default()
    };
    assert_eq!(refused(&mut engine, MAP_PATH, kit_is_a_map), ["L1406"]);
    assert_eq!(
        refused(&mut engine, "phys.map.yaml", MapSpawn::default()),
        ["L1406"],
        "a map carrying physics"
    );
    let mirrored = MapSpawn {
        mirror: Mirror::X,
        ..MapSpawn::default()
    };
    assert_eq!(refused(&mut engine, "frag.map.yaml", mirrored), ["L1399"]);
    assert!(
        spawn_map(engine.world_mut(), "frag.map.yaml", MapSpawn::default()).is_ok(),
        "the same map spawns unmirrored, fragments included"
    );
}

#[test]
fn save_and_prefab_edits_on_a_map_instance_error_cleanly() {
    let root = root("room_runtime_instance_ops");
    let mut engine = bare(&root);
    let ids = spawn_map(engine.world_mut(), MAP_PATH, MapSpawn::default()).unwrap();
    let root_id = ids[0];
    let part = *ids.last().unwrap();

    let outcomes = [
        engine.execute(Command::SceneSave(SceneSaveParams {
            path: "saved.scene.yaml".to_owned(),
            force: true,
        })),
        engine.execute(Command::PrefabOverrides(PrefabOverridesParams {
            instances: Some(vec![root_id]),
        })),
        engine.execute(Command::PrefabApply(PrefabApplyParams {
            instance: part,
            parts: None,
        })),
        engine.execute(Command::PrefabRevert(PrefabRevertParams {
            instance: part,
            pointer: None,
        })),
    ];
    // A map instance is not a prefab text: applying or reverting it is refused, not a panic.
    assert!(outcomes[2].is_err(), "prefab.apply is refused");
    assert!(outcomes[3].is_err(), "prefab.revert is refused");
    assert!(
        std::fs::read_to_string(root.join(MAP_PATH)).unwrap() == MAP,
        "the map file is untouched"
    );
}
