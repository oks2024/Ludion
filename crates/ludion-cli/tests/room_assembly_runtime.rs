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
    Engine, MapSpawn, Mirror, NavAgent, PrefabInstance, SceneSwitch, Transform, despawn_instance,
    prepare_map, replay_hashes_with, spawn_map, spawn_prefab,
};
use ludion_protocol::command::{
    AssetReloadParams, PrefabApplyParams, PrefabOverridesParams, PrefabRevertParams,
    SceneSaveParams, SimStepParams,
};
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use ludion_protocol::{Command, ResponseBody};
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

/// `spawner::<V>` that first prepares the same room `PREPARE_AHEAD` ticks before the spawn.
fn preparer<const V: u8>(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == SPAWN - PREPARE_AHEAD {
        prepare_map(world, MAP_PATH, &variant(V)).expect("the room compiles");
    }
    spawner::<V>(world, ctx);
}

const PREPARE_AHEAD: u64 = 4;

fn hash_trace(root: &Path, v: u8, ticks: usize) -> Vec<u64> {
    let spawner: fn(&mut World, &SystemCtx) = match v {
        0 => spawner::<0>,
        1 => spawner::<1>,
        _ => spawner::<2>,
    };
    trace_with(root, spawner, ticks)
}

fn trace_with(root: &Path, spawner: fn(&mut World, &SystemCtx), ticks: usize) -> Vec<u64> {
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
    replays_after_deleting_its_files("room_runtime_replay", spawner::<1>);
}

#[test]
fn a_prepared_runtime_room_replays_after_its_map_and_kit_are_deleted() {
    replays_after_deleting_its_files("room_runtime_prepare_replay", preparer::<1>);
}

/// Records `spawner`'s session, deletes the map and both kits, and replays it to the same hashes.
fn replays_after_deleting_its_files(tag: &str, spawner: fn(&mut World, &SystemCtx)) {
    let root = root(tag);
    let sink = SharedSink::default();
    let mut engine = booted_with(&root, spawner);
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
    let replayed = replay_hashes_with(
        crate::room_assembly::game_with(&root, spawner),
        &bytes,
        &root,
    )
    .unwrap();
    assert_eq!(replayed, live);
}

/// Tries the room on four ticks, each a different combination (so each compiles afresh), and
/// requests a restart between the second and the third.
fn retrier(world: &mut World, ctx: &SystemCtx) {
    let seed = match ctx.frame.as_u64() {
        25 => {
            world
                .resource_mut::<SceneSwitch>()
                .expect("the switch")
                .restart(Vec::new());
            return;
        }
        10 => 1,
        20 => 2,
        30 => 3,
        40 => 4,
        _ => return,
    };
    let _ = spawn_map(
        world,
        MAP_PATH,
        MapSpawn {
            seed: Some(seed),
            ..MapSpawn::default()
        },
    );
}

#[test]
fn a_failed_room_read_retries_only_through_a_reload_and_replays() {
    let root = root("room_runtime_unread");
    std::fs::remove_file(root.join("kit.kit.yaml")).unwrap();
    let sink = SharedSink::default();
    let mut engine = booted_with(&root, retrier);
    let base = engine.world().entity_count();
    let mut live = vec![(engine.frame(), engine.world_hash().unwrap())];
    let mut counts = Vec::new();
    engine.set_recording(sink.clone()).unwrap();
    while engine.frame().as_u64() < 50 {
        let frame = engine.frame().as_u64();
        if frame == 12 {
            std::fs::write(root.join("kit.kit.yaml"), KIT).unwrap();
        }
        if frame == 36 {
            let reloaded = engine
                .execute(Command::AssetReload(AssetReloadParams {
                    path: Some("kit.kit.yaml".to_owned()),
                }))
                .unwrap();
            assert!(
                matches!(reloaded.body, ResponseBody::AssetsReloaded { ref paths } if !paths.is_empty()),
                "the failed kit reloads: {reloaded:?}"
            );
        }
        engine
            .execute(Command::SimStep(SimStepParams {
                ticks: 1,
                hash: false,
            }))
            .unwrap();
        counts.push((frame, engine.world().entity_count()));
        live.push((engine.frame(), engine.world_hash().unwrap()));
    }
    engine.stop_recording().unwrap();
    let bytes = sink.0.lock().unwrap().clone();

    for (frame, count) in counts {
        assert_eq!(
            count > base,
            frame >= 40,
            "frame {frame}: the room spawns only after the reload, not once the file exists, nor across the restart ({count} entities, {base} before)"
        );
    }

    for gone in [MAP_PATH, "kit.kit.yaml"] {
        std::fs::remove_file(root.join(gone)).unwrap();
    }
    let replayed = replay_hashes_with(
        crate::room_assembly::game_with(&root, retrier),
        &bytes,
        &root,
    )
    .unwrap();
    assert_eq!(replayed, live, "the replay agrees tick for tick");
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
fn a_preloaded_map_routes_by_kind_not_name() {
    let root = root("room_runtime_preload_kind");
    std::fs::write(
        root.join("rooms/crypt.yaml"),
        MAP.replace("kit: kit.kit.yaml", "kit: k.yaml"),
    )
    .unwrap();
    std::fs::write(root.join("rooms/k.yaml"), KIT).unwrap();
    let with_preload = scene().replacen(
        "entities:",
        "preload:
  - rooms/crypt.yaml
  - rooms/k.yaml
entities:",
        1,
    );
    let mut engine = crate::room_assembly::game_with(&root, spawner::<9>)
        .build()
        .unwrap();
    engine
        .load_inline_scene(&with_preload)
        .expect("the preload warms a map and a kit whatever their names");

    for gone in ["rooms/crypt.yaml", "rooms/k.yaml"] {
        std::fs::remove_file(root.join(gone)).unwrap();
    }
    let ids = spawn_map(engine.world_mut(), "rooms/crypt.yaml", MapSpawn::default());
    assert!(ids.is_ok(), "the preload held the map and its kit: {ids:?}");
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

/// Every part's `translation.y` in thousandths, sorted.
fn heights(parts: &[BTreeMap<String, Value>]) -> Vec<i64> {
    let mut ys: Vec<i64> = parts
        .iter()
        .map(|p| (p["Transform"]["translation"][1].as_f64().unwrap() * 1000.0).round() as i64)
        .collect();
    ys.sort_unstable();
    ys
}

fn lamp(y: f32) -> String {
    let mut out = String::from(
        "format: 1
kind: prefab
name: lamp
entities:
",
    );
    out += "  - name: post
    components:
      Transform:
        translation:
";
    out += &crate::room_assembly::seq([0.0, y, 0.0], 10);
    out
}

#[test]
fn reloading_a_prefab_in_a_fragment_changes_the_next_room_and_no_live_one() {
    let root = root("room_runtime_nested_reload");
    std::fs::write(root.join("lamp.prefab.yaml"), lamp(70.0)).unwrap();
    std::fs::write(
        root.join("frag.scene.yaml"),
        "format: 1
entities:
  - name: lamp-1
    prefab: lamp.prefab.yaml
",
    )
    .unwrap();
    std::fs::write(
        root.join("frag.map.yaml"),
        format!(
            "{MAP}fragments:
  - frag.scene.yaml
"
        ),
    )
    .unwrap();
    let mut engine = bare(&root);
    let first = spawn_map(engine.world_mut(), "frag.map.yaml", MapSpawn::default()).unwrap();
    let first_dump = dump(&engine, &first);
    assert!(
        heights(&first_dump).contains(&70_000),
        "the lamp's first height"
    );

    std::fs::write(root.join("lamp.prefab.yaml"), lamp(90.0)).unwrap();
    let reloaded = engine
        .execute(Command::AssetReload(AssetReloadParams {
            path: Some("lamp.prefab.yaml".to_owned()),
        }))
        .unwrap();
    assert!(
        matches!(reloaded.body, ResponseBody::AssetsReloaded { ref paths } if !paths.is_empty()),
        "the nested prefab reloads: {reloaded:?}"
    );

    let second = spawn_map(engine.world_mut(), "frag.map.yaml", MapSpawn::default()).unwrap();
    let second_heights = heights(&dump(&engine, &second));
    assert!(
        second_heights.contains(&90_000),
        "the new room has the new lamp"
    );
    assert!(!second_heights.contains(&70_000), "and not the old one");
    assert_eq!(
        dump(&engine, &first),
        first_dump,
        "the live room keeps the old lamp"
    );
}

#[test]
fn a_room_whose_root_animates_is_refused() {
    let root = root("room_runtime_root_animation");
    std::fs::write(
        root.join("anim.scene.yaml"),
        "format: 1
entities:
  - name: lamp
    components:
      Animation:
        clip: walk
",
    )
    .unwrap();
    std::fs::write(
        root.join("anim.map.yaml"),
        format!(
            "{MAP}fragments:
  - anim.scene.yaml
"
        ),
    )
    .unwrap();
    let mut engine = bare(&root);
    let count = engine.world().entity_count();
    let refused = spawn_map(engine.world_mut(), "anim.map.yaml", MapSpawn::default()).unwrap_err();
    assert_eq!(codes(&refused), ["L1406"]);
    assert!(
        refused[0].message.contains("lamp") && refused[0].message.contains("no clips"),
        "{}",
        refused[0].message
    );
    assert_eq!(engine.world().entity_count(), count, "nothing spawned");
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
fn save_and_prefab_edits_on_a_named_map_instance() {
    let root = root("room_runtime_instance_ops");
    let mut engine = bare(&root);
    let named = MapSpawn {
        name: Some("f1-r04"),
        ..MapSpawn::default()
    };
    let ids = spawn_map(engine.world_mut(), MAP_PATH, named).unwrap();
    let root_id = ids[0];
    let part = *ids.last().unwrap();
    let prefab = spawn_prefab(
        engine.world_mut(),
        "rooms/room.prefab.yaml",
        Some(placement()),
        Some("p"),
        false,
    )
    .unwrap();

    let saved = engine.execute(Command::SceneSave(SceneSaveParams {
        path: "saved.scene.yaml".to_owned(),
        force: true,
    }));
    assert!(saved.is_ok(), "scene.save of a named room: {saved:?}");
    let text = std::fs::read_to_string(root.join("saved.scene.yaml")).unwrap();
    assert!(
        !text.contains(&format!("prefab: {MAP_PATH}")),
        "the room is not an instance entry:
{text}"
    );
    assert!(
        text.contains("prefab: rooms/room.prefab.yaml"),
        "the real prefab instance still saves as an instance:
{text}"
    );
    assert_eq!(
        text.matches("name: f1-r04/").count(),
        ids.len(),
        "one plain entity per room part:
{text}"
    );
    let mut fresh = bare(&root);
    fresh.load_inline_scene(&text).unwrap();
    assert_eq!(
        fresh.world().entity_count(),
        engine.world().entity_count(),
        "the saved scene reloads to the same entities"
    );

    let overrides = engine
        .execute(Command::PrefabOverrides(PrefabOverridesParams {
            instances: Some(vec![root_id, prefab[0]]),
        }))
        .unwrap();
    let ResponseBody::PrefabOverrides { instances } = overrides.body else {
        panic!("prefab.overrides body");
    };
    assert_eq!(instances.len(), 2);
    let refused = instances[0]
        .diagnostic
        .as_ref()
        .expect("the room is refused");
    assert_eq!(refused.code.to_string(), "L1235", "{refused:?}");
    assert!(
        instances[1].diagnostic.is_none(),
        "the prefab instance diffs"
    );

    let apply = engine
        .execute(Command::PrefabApply(PrefabApplyParams {
            instance: part,
            parts: None,
        }))
        .unwrap_err();
    assert_eq!(apply.code.to_string(), "L1237", "{apply:?}");
    assert!(apply.message.contains("runtime room"), "{}", apply.message);
    assert!(!apply.message.contains("  "), "{}", apply.message);
    let revert = engine
        .execute(Command::PrefabRevert(PrefabRevertParams {
            instance: part,
            pointer: None,
        }))
        .unwrap_err();
    assert_eq!(revert.code.to_string(), "L1238", "{revert:?}");
    assert_eq!(std::fs::read_to_string(root.join(MAP_PATH)).unwrap(), MAP);

    // The refused ops left the library clean: a map reload still lands, the next room is new.
    std::fs::write(root.join("kit.kit.yaml"), kit2()).unwrap();
    let reloaded = engine
        .execute(Command::AssetReload(AssetReloadParams {
            path: Some(MAP_PATH.to_owned()),
        }))
        .unwrap();
    assert!(
        matches!(reloaded.body, ResponseBody::AssetsReloaded { ref paths } if !paths.is_empty()),
        "the map reloads: {reloaded:?}"
    );
    std::fs::write(root.join("kit.kit.yaml"), kit2()).unwrap();
    engine
        .execute(Command::AssetReload(AssetReloadParams {
            path: Some("kit.kit.yaml".to_owned()),
        }))
        .unwrap();
    spawn_map(engine.world_mut(), MAP_PATH, MapSpawn::default()).unwrap();

    // The unnamed room saves too.
    let mut plain = bare(&root);
    spawn_map(plain.world_mut(), MAP_PATH, MapSpawn::default()).unwrap();
    plain
        .execute(Command::SceneSave(SceneSaveParams {
            path: "plain.scene.yaml".to_owned(),
            force: true,
        }))
        .unwrap();
}

#[test]
fn preparing_a_room_is_invisible_to_the_hash_and_the_spawn_after_it_is_equal() {
    let root = root("room_runtime_prepare");
    for (v, prepared) in [
        (0, preparer::<0> as fn(&mut World, &SystemCtx)),
        (1, preparer::<1>),
        (2, preparer::<2>),
    ] {
        let plain = hash_trace(&root, v, 340);
        assert_eq!(
            trace_with(&root, prepared, 340),
            plain,
            "variant {v}: a prepare {PREPARE_AHEAD} ticks ahead moves no hash"
        );
    }
}

#[test]
fn preparing_a_bad_map_returns_the_spawn_errors_and_spawns_nothing() {
    let root = root("room_runtime_prepare_errors");
    std::fs::write(
        root.join("phys.map.yaml"),
        format!(
            "{MAP}physics:
  space: free
"
        ),
    )
    .unwrap();
    let mut engine = bare(&root);
    let count = engine.world().entity_count();
    for map in ["phys.map.yaml", "../outside.map.yaml", "missing.map.yaml"] {
        let found = prepare_map(engine.world_mut(), map, &MapSpawn::default()).unwrap_err();
        assert_eq!(codes(&found), ["L1406"], "{map}");
        let spawned = spawn_map(engine.world_mut(), map, MapSpawn::default()).unwrap_err();
        assert_eq!(codes(&spawned), codes(&found), "{map}: the spawn agrees");
    }
    assert_eq!(engine.world().entity_count(), count, "nothing spawned");
    prepare_map(engine.world_mut(), MAP_PATH, &variant(1)).unwrap();
    assert_eq!(
        engine.world().entity_count(),
        count,
        "a good prepare spawns nothing"
    );
}
