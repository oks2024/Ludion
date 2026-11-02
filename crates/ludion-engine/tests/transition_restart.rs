//! `SceneSwitch::restart`: the current scene rebuilt from the text the session holds, at the frame
//! boundary, through the switch's transactional path. No anchor, no reseed, no record: the frame and
//! the RNG continue, the prefab epoch stays, and a replay reproduces every restart from the text it
//! already holds.

use ludion_core::ids::EntityId;
use ludion_core::time::FrameIndex;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    Engine, EngineBuilder, Name, SceneSwitch, Transform, replay_hashes_with, spawn_prefab,
};
use ludion_protocol::command::{
    AssetReloadParams, EngineRestoreParams, InputInjectParams, SceneLoadParams, SimStepParams,
};
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use ludion_protocol::{Command, ResponseBody};
use ludion_sim::WorldHash;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// --- Components -------------------------------------------------------------------------------

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Player {
    hp: u32,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
struct RngProbe {
    last: u32,
}

/// A counter every tick moves, on authored and spawned entities alike.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Coin {
    value: u32,
}

/// A play-once latch: a resource a game carries across a rebuild.
struct Latch;

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct Weapon {
    damage: f32,
}

// --- Scenes -----------------------------------------------------------------------------------

const ARENA: &str = "\
format: 1
name: arena
seed: 7
entities:
  - name: player
    components:
      Transform:
        translation:
          - 1.0
          - 2.0
          - 3.0
      Player:
        hp: 100
      RngProbe:
        last: 0
  - name: door
    components:
      Residue: {}
      Coin:
        value: 0
  - name: crate
    components:
      Coin:
        value: 0
";

const NEXT: &str = "\
format: 1
name: next
seed: 999
entities:
  - name: gate-from-start
    components:
      Transform:
        translation:
          - 40.0
          - 0.0
          - -10.0
  - name: beacon
    components:
      Transform:
        translation:
          - 7.0
          - 0.0
          - 0.0
";

const NEXT_EDITED: &str = "\
format: 1
name: next
seed: 999
entities:
  - name: gate-from-start
    components:
      Transform:
        translation:
          - 40.0
          - 0.0
          - -10.0
  - name: beacon
    components:
      Transform:
        translation:
          - 999.0
          - 0.0
          - 0.0
";

const PREFAB: &str = "crate.prefab.yaml";
const LATE: &str = "late.prefab.yaml";
const WEAPON_KEY: &str = "weapons/crossbow.yaml";

const V1: &str = "\
format: 1
kind: prefab
name: crate
entities:
  - name: base
    components:
      Transform: {}
  - name: lid
    components:
      Transform: {}
      Parent:
        index: 0
        generation: 0
";

const V2: &str = "\
format: 1
kind: prefab
name: crate
entities:
  - name: base
    components:
      Transform: {}
  - name: lid
    components:
      Transform: {}
      Parent:
        index: 0
        generation: 0
  - name: latch
    components:
      Transform: {}
      Parent:
        index: 0
        generation: 0
";

const PREFAB_SCENE: &str = "\
format: 1
name: yard
seed: 3
entities:
  - name: camera
    components:
      Transform: {}
  - name: placed
    prefab: crate.prefab.yaml
";

// --- Systems ----------------------------------------------------------------------------------

const NEVER: u64 = u64::MAX;

fn draw_rng(world: &mut World, _ctx: &SystemCtx) {
    let value = world
        .resource_mut::<ludion_core::rng::DeterministicRng>()
        .map(ludion_core::rng::DeterministicRng::next_u32)
        .unwrap_or(0);
    for (probe,) in world.query::<(&mut RngProbe,)>() {
        probe.last = value;
    }
}

/// Moves every counter, the player and the latch; frame 2 also spawns a stray coin.
fn mutate(world: &mut World, ctx: &SystemCtx) {
    for (coin,) in world.query::<(&mut Coin,)>() {
        coin.value += 1;
    }
    for (player, transform) in world.query::<(&mut Player, &mut Transform)>() {
        player.hp += 1;
        transform.translation[0] += 1.0;
    }
    match ctx.frame.as_u64() {
        1 => drop(world.insert_resource(Latch)),
        2 => {
            let id = world.spawn();
            world.insert(id, Name("stray".to_string())).unwrap();
            world.insert(id, Coin { value: 50 }).unwrap();
        }
        _ => {}
    }
}

fn player_of(world: &World) -> Option<EntityId> {
    world
        .entities()
        .filter(|&id| matches!(world.get::<Player>(id), Ok(Some(_))))
        .last()
}

/// Requests a restart on frames `A`, `B` and `C`, carrying the player when `CARRY`.
fn restarter<const A: u64, const B: u64, const C: u64, const CARRY: bool>(
    world: &mut World,
    ctx: &SystemCtx,
) {
    let frame = ctx.frame.as_u64();
    if frame != A && frame != B && frame != C {
        return;
    }
    let carry = if CARRY {
        player_of(world).into_iter().collect()
    } else {
        Vec::new()
    };
    if let Some(switch) = world.resource_mut::<SceneSwitch>() {
        switch.restart(carry);
    }
}

fn request_next(world: &mut World) {
    if let Some(switch) = world.resource_mut::<SceneSwitch>() {
        switch.request("next.yaml", "gate-from-start", Vec::new());
    }
}

fn switch_at_two(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 2 {
        request_next(world);
    }
}

fn restart_then_request(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 2 {
        world
            .resource_mut::<SceneSwitch>()
            .unwrap()
            .restart(Vec::new());
        request_next(world);
    }
}

fn request_then_restart(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 2 {
        request_next(world);
        world
            .resource_mut::<SceneSwitch>()
            .unwrap()
            .restart(Vec::new());
    }
}

/// Spawns the prefab `path` at frames 2, 6 and 10; a refusal is a no-op.
fn spawn_at(world: &mut World, ctx: &SystemCtx, path: &str) {
    if matches!(ctx.frame.as_u64(), 2 | 6 | 10) {
        let _ = spawn_prefab(world, path, None, Some("runtime"), false);
    }
}

fn spawn_crate(world: &mut World, ctx: &SystemCtx) {
    spawn_at(world, ctx, PREFAB);
}

fn spawn_late(world: &mut World, ctx: &SystemCtx) {
    spawn_at(world, ctx, LATE);
}

// --- Harness ----------------------------------------------------------------------------------

type Trace = Vec<(FrameIndex, WorldHash)>;

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-transition-restart-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("next.yaml"), NEXT).unwrap();
    std::fs::write(dir.join(PREFAB), V1).unwrap();
    dir
}

fn game(root: &Path, restart: fn(&mut World, &SystemCtx)) -> EngineBuilder {
    Engine::builder()
        .components(|reg| {
            reg.register::<Player>("Player")?;
            reg.register::<RngProbe>("RngProbe")?;
            reg.register::<Coin>("Coin")
        })
        .carry_resource::<Latch>()
        .system("draw_rng", draw_rng)
        .system("mutate", mutate)
        .system("restart", restart)
        .project_root(root)
}

fn booted(root: &Path, restart: fn(&mut World, &SystemCtx)) -> Engine {
    let mut engine = game(root, restart).build().unwrap();
    engine.load_inline_scene(ARENA).unwrap();
    engine
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

fn tick(engine: &mut Engine) {
    engine
        .execute(Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .unwrap();
}

fn tick_traced(engine: &mut Engine, trace: &mut Trace) {
    tick(engine);
    trace.push((engine.frame(), engine.world_hash().unwrap()));
}

fn ticks(engine: &mut Engine, count: usize) {
    for _ in 0..count {
        tick(engine);
    }
}

fn start_trace(engine: &mut Engine) -> Trace {
    vec![(engine.frame(), engine.world_hash().unwrap())]
}

fn arrived(engine: &Engine) -> bool {
    engine
        .world()
        .resource::<SceneSwitch>()
        .expect("the resource is installed")
        .arrived_by_restart()
}

/// Ticks until the world has just been rebuilt by a restart, returning the ticks it took.
fn until_restarted(engine: &mut Engine, trace: Option<&mut Trace>) -> usize {
    let mut trace = trace;
    for count in 1..=40 {
        match trace.as_deref_mut() {
            Some(trace) => tick_traced(engine, trace),
            None => tick(engine),
        }
        if arrived(engine) {
            return count;
        }
    }
    panic!("no restart landed within 40 ticks");
}

fn error_code(engine: &Engine) -> Option<String> {
    engine
        .last_transition_error()
        .map(|error| error.code.as_str().to_string())
}

fn named(engine: &Engine, name: &str) -> Vec<EntityId> {
    let world = engine.world();
    world
        .entities()
        .filter(|&id| matches!(world.get::<Name>(id), Ok(Some(found)) if found.0 == name))
        .collect()
}

/// How many entities are a prefab part named `part` (`<instance>/<part>`).
fn parts(engine: &Engine, part: &str) -> usize {
    let world = engine.world();
    world
        .entities()
        .filter(|&id| {
            matches!(world.get::<Name>(id), Ok(Some(found))
                if found.0 == part || found.0.ends_with(&format!("/{part}")))
        })
        .count()
}

fn coin_of(engine: &Engine, name: &str) -> u32 {
    let id = named(engine, name)[0];
    engine.world().get::<Coin>(id).unwrap().unwrap().value
}

fn beacon_x(engine: &Engine) -> f32 {
    let id = named(engine, "beacon")[0];
    engine
        .world()
        .get::<Transform>(id)
        .unwrap()
        .unwrap()
        .translation[0]
}

fn records(bytes: &[u8]) -> Vec<ReplayRecord> {
    ReplayReader::new(bytes).unwrap().records().unwrap()
}

fn switch_records(bytes: &[u8]) -> usize {
    records(bytes)
        .iter()
        .filter(|record| matches!(record, ReplayRecord::SceneSwitch { .. }))
        .count()
}

fn prefab_text_records(bytes: &[u8]) -> Vec<String> {
    records(bytes)
        .into_iter()
        .filter_map(|record| match record {
            ReplayRecord::PrefabText { document, .. } => Some(document),
            _ => None,
        })
        .collect()
}

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

fn restore(engine: &mut Engine, snapshot: Value) {
    engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
}

fn replay(builder: EngineBuilder, bytes: &[u8], dir: &Path) -> Trace {
    replay_hashes_with(builder, bytes, dir).unwrap()
}

// --- The rebuild ------------------------------------------------------------------------------

#[test]
fn a_restart_brings_back_the_authored_world_and_drops_what_a_system_spawned() {
    let dir = fresh_dir("authored");
    let mut engine = booted(&dir, restarter::<4, NEVER, NEVER, false>);
    ticks(&mut engine, 3);
    assert_eq!(named(&engine, "stray").len(), 1);
    assert!(coin_of(&engine, "crate") > 0);

    until_restarted(&mut engine, None);
    assert_eq!(error_code(&engine), None);
    assert!(
        named(&engine, "stray").is_empty(),
        "a spawned entity is gone"
    );
    assert_eq!(engine.world().entity_count(), 3);
    assert_eq!(
        coin_of(&engine, "crate"),
        0,
        "a mutated entity is as authored"
    );
}

#[test]
fn the_frame_and_the_rng_stream_continue_through_a_restart() {
    let dir_a = fresh_dir("rng-a");
    let dir_b = fresh_dir("rng-b");
    let mut restarted = booted(&dir_a, restarter::<3, NEVER, NEVER, false>);
    let mut control = booted(&dir_b, restarter::<NEVER, NEVER, NEVER, false>);
    ticks(&mut restarted, 8);
    ticks(&mut control, 8);

    assert_eq!(restarted.frame(), control.frame());
    let last = |engine: &Engine| {
        let id = player_of(engine.world()).unwrap();
        engine.world().get::<RngProbe>(id).unwrap().unwrap().last
    };
    assert_ne!(last(&control), 0);
    assert_eq!(
        last(&restarted),
        last(&control),
        "the draw count is unchanged"
    );
}

#[test]
fn a_carried_entity_keeps_its_pose_and_components_and_holds_the_highest_id() {
    let dir_a = fresh_dir("carry-a");
    let dir_b = fresh_dir("carry-b");
    let mut restarted = booted(&dir_a, restarter::<4, NEVER, NEVER, true>);
    let mut control = booted(&dir_b, restarter::<NEVER, NEVER, NEVER, false>);
    let count = until_restarted(&mut restarted, None);
    ticks(&mut control, count);

    assert_eq!(error_code(&restarted), None, "no anchor, so no L3019");
    let carried = player_of(restarted.world()).unwrap();
    let before = player_of(control.world()).unwrap();
    assert_eq!(
        restarted.world().get::<Player>(carried).unwrap(),
        control.world().get::<Player>(before).unwrap()
    );
    assert_eq!(
        restarted
            .world()
            .get::<Transform>(carried)
            .unwrap()
            .unwrap()
            .translation,
        control
            .world()
            .get::<Transform>(before)
            .unwrap()
            .unwrap()
            .translation
    );
    assert_eq!(restarted.world().entities().last(), Some(carried));
}

#[test]
fn a_carried_resource_and_held_input_survive_a_restart() {
    let dir = fresh_dir("latch");
    let mut engine = booted(&dir, restarter::<4, NEVER, NEVER, false>);
    engine
        .execute(Command::InputInject(InputInjectParams {
            frame: None,
            event: json!({ "kind": "key_down", "key": "W" }),
        }))
        .unwrap();
    until_restarted(&mut engine, None);

    assert!(
        engine.world().resource::<Latch>().is_some(),
        "the latch crossed"
    );
    let input = engine.world().resource::<ludion_sim::InputState>().unwrap();
    assert!(input.is_held(ludion_sim::Key::W), "held input carries");
}

#[test]
fn the_last_request_wins_whichever_order_they_arrive_in() {
    let dir = fresh_dir("last-wins-a");
    let mut engine = booted(&dir, restart_then_request);
    ticks(&mut engine, 4);
    assert_eq!(named(&engine, "beacon").len(), 1, "the switch won");
    assert!(
        !arrived(&engine),
        "a switch arrival is not a restart arrival"
    );

    let dir = fresh_dir("last-wins-b");
    let mut engine = booted(&dir, request_then_restart);
    until_restarted(&mut engine, None);
    assert!(named(&engine, "beacon").is_empty(), "the restart won");
    assert_eq!(named(&engine, "crate").len(), 1);
}

#[test]
fn a_door_marked_residue_keeps_its_state_and_an_unmarked_crate_is_reauthored() {
    let dir_a = fresh_dir("residue-a");
    let dir_b = fresh_dir("residue-b");
    let mut restarted = booted(&dir_a, restarter::<4, NEVER, NEVER, false>);
    let mut control = booted(&dir_b, restarter::<NEVER, NEVER, NEVER, false>);
    let count = until_restarted(&mut restarted, None);
    ticks(&mut control, count);

    assert_eq!(coin_of(&restarted, "door"), coin_of(&control, "door"));
    assert!(coin_of(&restarted, "door") > 0);
    assert_eq!(coin_of(&restarted, "crate"), 0);
}

// --- Rejections -------------------------------------------------------------------------------

#[test]
fn a_restart_with_no_scene_is_rejected() {
    let dir = fresh_dir("no-scene");
    let mut engine = game(&dir, restarter::<1, NEVER, NEVER, false>)
        .build()
        .unwrap();
    ticks(&mut engine, 4);
    assert_eq!(error_code(&engine).as_deref(), Some("L3018"));
    assert!(!arrived(&engine));
}

#[test]
fn a_data_doc_gone_bad_rejects_the_restart_and_leaves_the_world_and_the_recording_alone() {
    let dir = fresh_dir("bad-doc");
    std::fs::create_dir_all(dir.join("weapons")).unwrap();
    std::fs::write(
        dir.join(WEAPON_KEY),
        "format: 1\nkind: weapon\ndamage: 12.0\n",
    )
    .unwrap();
    let mut engine = game(&dir, restarter::<4, NEVER, NEVER, false>)
        .doc_kind::<Weapon>("weapon")
        .build()
        .unwrap();
    engine.load_inline_scene(ARENA).unwrap();
    let sink = SharedSink::default();
    engine.set_recording(sink.clone()).unwrap();
    ticks(&mut engine, 2);
    std::fs::write(
        dir.join(WEAPON_KEY),
        "format: 1\nkind: weapon\ndamage: nope\n",
    )
    .unwrap();
    ticks(&mut engine, 3);

    assert_eq!(
        error_code(&engine).as_deref(),
        Some("L1120"),
        "the doc walk's diagnostic, as a switch reports it"
    );
    assert_eq!(
        named(&engine, "stray").len(),
        1,
        "the live world is untouched"
    );
    assert!(!arrived(&engine));
    assert_eq!(engine.frame().as_u64(), 5, "the sim ran on");
    engine.stop_recording().unwrap();
    assert_eq!(switch_records(&sink.bytes()), 0);
}

// --- The arrival fact -------------------------------------------------------------------------

#[test]
fn arrived_by_restart_lasts_one_tick_and_rides_a_snapshot() {
    let dir = fresh_dir("flag");
    let mut engine = booted(&dir, restarter::<4, NEVER, NEVER, false>);
    until_restarted(&mut engine, None);

    let on_arrival = snapshot_of(&mut engine);
    assert_eq!(on_arrival["arrived_by_restart"], json!(true));
    let mut restored = booted(
        &fresh_dir("flag-restored"),
        restarter::<NEVER, NEVER, NEVER, false>,
    );
    restore(&mut restored, on_arrival);
    assert!(arrived(&restored), "the snapshot carries the fact");
    tick(&mut restored);
    assert!(!arrived(&restored), "and it clears after one tick");

    tick(&mut engine);
    assert!(!arrived(&engine));
    assert!(snapshot_of(&mut engine).get("arrived_by_restart").is_none());
}

#[test]
fn a_restart_after_engine_restore_rebuilds_the_snapshotted_scene() {
    let dir = fresh_dir("after-restore");
    let mut original = booted(&dir, restarter::<NEVER, NEVER, NEVER, false>);
    ticks(&mut original, 3);
    let snapshot = snapshot_of(&mut original);

    let mut engine = game(&dir, restarter::<5, NEVER, NEVER, false>)
        .build()
        .unwrap();
    restore(&mut engine, snapshot);
    until_restarted(&mut engine, None);
    assert_eq!(error_code(&engine), None);
    assert_eq!(engine.world().entity_count(), 3);
    assert_eq!(coin_of(&engine, "crate"), 0);
}

// --- Replay -----------------------------------------------------------------------------------

#[test]
fn restarts_replay_from_held_text_and_the_recording_does_not_grow() {
    let run = |name: &str, restart: fn(&mut World, &SystemCtx)| {
        let dir = fresh_dir(name);
        std::fs::write(dir.join("main.yaml"), ARENA).unwrap();
        let sink = SharedSink::default();
        let mut engine = game(&dir, restart).build().unwrap();
        engine
            .execute(Command::SceneLoad(SceneLoadParams {
                path: "main.yaml".to_string(),
            }))
            .unwrap();
        engine.set_recording(sink.clone()).unwrap();
        let mut trace = start_trace(&mut engine);
        for _ in 0..6 {
            tick_traced(&mut engine, &mut trace);
        }
        // The scene file changes under a live session: a restart still rebuilds the held text.
        std::fs::write(dir.join("main.yaml"), NEXT).unwrap();
        for _ in 0..10 {
            tick_traced(&mut engine, &mut trace);
        }
        engine.stop_recording().unwrap();
        assert_eq!(named(&engine, "crate").len(), 1, "{name}: still the arena");
        (dir, trace, sink.bytes())
    };

    let (dir, live, bytes) = run("replay-3", restarter::<4, 8, 12, false>);
    assert_eq!(switch_records(&bytes), 0, "a restart writes no record");
    std::fs::write(dir.join("main.yaml"), "not a scene").unwrap();
    assert_eq!(
        replay(game(&dir, restarter::<4, 8, 12, false>), &bytes, &dir),
        live
    );
    std::fs::remove_file(dir.join("main.yaml")).unwrap();
    assert_eq!(
        replay(game(&dir, restarter::<4, 8, 12, false>), &bytes, &dir),
        live
    );

    let (_, _, one) = run("replay-1", restarter::<4, NEVER, NEVER, false>);
    let plain = |bytes: &[u8]| records(bytes).iter().filter(|r| !r.is_keyframe()).count();
    assert_eq!(plain(&bytes), plain(&one), "restarts add no records");
    assert!(
        bytes.len() <= one.len() + 256,
        "{} bytes against {}",
        bytes.len(),
        one.len()
    );
}

#[test]
fn restarts_after_a_real_switch_rebuild_the_text_the_switch_read() {
    let dir = fresh_dir("after-switch");
    let sink = SharedSink::default();
    let build = || game(&dir, restarter::<4, 8, NEVER, false>).system("switch", switch_at_two);
    let mut engine = build().build().unwrap();
    engine.load_inline_scene(ARENA).unwrap();
    engine.set_recording(sink.clone()).unwrap();
    let mut trace = start_trace(&mut engine);
    until_restarted(&mut engine, Some(&mut trace));
    assert_eq!(beacon_x(&engine), 7.0);
    std::fs::write(dir.join("next.yaml"), NEXT_EDITED).unwrap();
    until_restarted(&mut engine, Some(&mut trace));
    assert_eq!(
        beacon_x(&engine),
        7.0,
        "the originally read text, not the edited file"
    );
    for _ in 0..3 {
        tick_traced(&mut engine, &mut trace);
    }
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();

    assert_eq!(switch_records(&bytes), 1);
    assert_eq!(replay(build(), &bytes, &dir), trace);
}

#[test]
fn a_restart_after_a_mid_session_scene_load_rebuilds_the_loaded_scene() {
    let dir = fresh_dir("after-load");
    let sink = SharedSink::default();
    let mut engine = booted(&dir, restarter::<4, NEVER, NEVER, false>);
    engine.set_recording(sink.clone()).unwrap();
    let mut trace = start_trace(&mut engine);
    tick_traced(&mut engine, &mut trace);
    engine
        .execute(Command::SceneLoad(SceneLoadParams {
            path: "next.yaml".to_string(),
        }))
        .unwrap();
    *trace.last_mut().unwrap() = (engine.frame(), engine.world_hash().unwrap());
    until_restarted(&mut engine, Some(&mut trace));
    assert_eq!(beacon_x(&engine), 7.0);
    assert!(named(&engine, "crate").is_empty());
    engine.stop_recording().unwrap();

    assert_eq!(
        replay(
            game(&dir, restarter::<4, NEVER, NEVER, false>),
            &sink.bytes(),
            &dir
        ),
        trace
    );
}

#[test]
fn a_restart_after_engine_restore_replays_with_the_prefab_read_filed_in_the_restore_epoch() {
    let dir = fresh_dir("restore-replay");
    let sink = SharedSink::default();
    let build = || game(&dir, restarter::<6, NEVER, NEVER, false>);
    let mut engine = build().build().unwrap();
    engine.load_inline_scene(PREFAB_SCENE).unwrap();
    engine.set_recording(sink.clone()).unwrap();
    let mut trace = start_trace(&mut engine);
    for _ in 0..3 {
        tick_traced(&mut engine, &mut trace);
    }
    let snapshot = snapshot_of(&mut engine);
    restore(&mut engine, snapshot);
    // A command between ticks restamps the last entry, as the replay's drive does.
    *trace.last_mut().unwrap() = (engine.frame(), engine.world_hash().unwrap());
    until_restarted(&mut engine, Some(&mut trace));
    for _ in 0..3 {
        tick_traced(&mut engine, &mut trace);
    }
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();

    assert_eq!(
        parts(&engine, "lid"),
        1,
        "the restart rebuilt the placed instance"
    );
    assert_eq!(switch_records(&bytes), 0);
    std::fs::remove_file(dir.join(PREFAB)).unwrap();
    assert_eq!(replay(build(), &bytes, &dir), trace);
}

// --- Prefabs ----------------------------------------------------------------------------------

/// Runs the yard scene with `restart`, `edit` called after the first restart lands.
fn yard_run(
    name: &str,
    restart: fn(&mut World, &SystemCtx),
    spawn: fn(&mut World, &SystemCtx),
    edit: impl FnOnce(&mut Engine, &Path),
) -> (PathBuf, Engine, Trace, Vec<u8>) {
    let dir = fresh_dir(name);
    let sink = SharedSink::default();
    let mut engine = game(&dir, restart).system("spawn", spawn).build().unwrap();
    engine.load_inline_scene(PREFAB_SCENE).unwrap();
    engine.set_recording(sink.clone()).unwrap();
    let mut trace = start_trace(&mut engine);
    until_restarted(&mut engine, Some(&mut trace));
    edit(&mut engine, &dir);
    for _ in 0..8 {
        tick_traced(&mut engine, &mut trace);
    }
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();
    (dir, engine, trace, bytes)
}

fn yard_game(
    dir: &Path,
    restart: fn(&mut World, &SystemCtx),
    spawn: fn(&mut World, &SystemCtx),
) -> EngineBuilder {
    game(dir, restart).system("spawn", spawn)
}

#[test]
fn a_prefab_file_edited_between_restarts_does_not_reach_the_second_one() {
    let (dir, engine, trace, bytes) = yard_run(
        "prefab-edit",
        restarter::<4, 8, NEVER, false>,
        spawn_crate,
        |_, dir| std::fs::write(dir.join(PREFAB), V2).unwrap(),
    );
    assert_eq!(
        parts(&engine, "latch"),
        0,
        "the held text, not the edited file"
    );
    assert!(
        parts(&engine, "lid") >= 2,
        "the placed and the spawned instance"
    );
    assert_eq!(
        prefab_text_records(&bytes),
        vec![V1.to_string()],
        "one read per path"
    );

    std::fs::remove_file(dir.join(PREFAB)).unwrap();
    assert_eq!(
        replay(
            yard_game(&dir, restarter::<4, 8, NEVER, false>, spawn_crate),
            &bytes,
            &dir
        ),
        trace
    );
}

#[test]
fn an_asset_reload_between_restarts_survives_the_next_restart() {
    let (dir, engine, trace, bytes) = yard_run(
        "prefab-reload",
        restarter::<4, 9, NEVER, false>,
        spawn_crate,
        |engine, dir| {
            std::fs::write(dir.join(PREFAB), V2).unwrap();
            engine
                .execute(Command::AssetReload(AssetReloadParams {
                    path: Some(PREFAB.to_string()),
                }))
                .unwrap();
        },
    );
    assert!(parts(&engine, "latch") >= 2, "the reloaded text is held");

    std::fs::remove_file(dir.join(PREFAB)).unwrap();
    assert_eq!(
        replay(
            yard_game(&dir, restarter::<4, 9, NEVER, false>, spawn_crate),
            &bytes,
            &dir
        ),
        trace
    );
}

#[test]
fn a_prefab_refused_before_a_restart_stays_refused_after_it() {
    let (dir, engine, trace, bytes) = yard_run(
        "prefab-refused",
        restarter::<4, 8, NEVER, false>,
        spawn_late,
        |_, dir| std::fs::write(dir.join(LATE), V2).unwrap(),
    );
    assert_eq!(
        parts(&engine, "base"),
        1,
        "only the placed one, the spawn stays refused"
    );
    assert!(
        !prefab_text_records(&bytes).contains(&V2.to_string()),
        "the late file was never read"
    );

    std::fs::remove_file(dir.join(LATE)).unwrap();
    assert_eq!(
        replay(
            yard_game(&dir, restarter::<4, 8, NEVER, false>, spawn_late),
            &bytes,
            &dir
        ),
        trace
    );
}
