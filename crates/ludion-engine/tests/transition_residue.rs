//! Scene residue: what a district keeps while the run is away.
//!
//! A switch already answers "what travels with you" (the carry set). These tests cover its mirror
//! image: an entity marked `Residue` has its component bag captured out of the departing world and
//! re-applied when the run comes back to that scene in the same session — a door opened in district A
//! is still open when the player walks back in, while an unmarked sibling is rebuilt from the scene
//! text. Named records land back on their own entity in place (no `EntityId` moves); unnamed ones are
//! anonymous and respawn. The two conflicts the engine refuses to guess at are reported: two marked
//! entities sharing a `Name` (`L3026`) and an entity that is both carried and marked (`L3027`).
//!
//! The second half of the file covers **reproducibility**: residue is regenerated, never recorded, so
//! a recorded round trip replays bit-identically with the replay record-kind set unmoved; and residue
//! is *carried* by a snapshot, so the two reconstructions that resume mid-session — a replay seek onto
//! a keyframe taken away from home, and a `ludion dev` snapshot → restart → restore — reproduce a
//! return that lands after the capture. A game that marks nothing pays nothing there too: its snapshot
//! omits the field entirely, and a snapshot written before the field existed restores as empty.

use ludion_core::ids::EntityId;
use ludion_core::time::FrameIndex;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    Engine, EngineBuilder, Name, RESIDUE_MAX_RECORDS_PER_SCENE, RESIDUE_MAX_SCENES, Residue,
    ResidueStore, SceneSwitch, replay_hashes_with,
};
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::replay::{REPLAY_FORMAT_VERSION, ReplayReader, ReplayRecord};
use ludion_protocol::{Command, ResponseBody};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The carried player.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Player {
    hp: u32,
}

/// A piece of district state: authored shut, opened by a system, and expected to stay open across a
/// visit to another district — on the entity that is marked, and only there.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Door {
    open: bool,
}

/// What a system standing in district B read out of district A's residue — a component, so the
/// reading is in the `WorldHash` and a replay has to reproduce it rather than merely not crash.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Memo {
    open: bool,
}

/// A dropped pile: spawned at runtime with no `Name`, so its record is anonymous.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Echo {
    amount: u32,
}

// --- Scenes -----------------------------------------------------------------------------------

/// District A: a marked `door` and an unmarked `lamp` carrying the identical component, the player,
/// and the anchor the return switch drops them at.
const DISTRICT_A: &str = "\
format: 1
name: district-a
seed: 1
entities:
  - name: player
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      Player:
        hp: 100
  - name: door
    components:
      Residue: {}
      Door:
        open: false
  - name: lamp
    components:
      Door:
        open: false
  - name: gate-from-b
    components:
      Transform:
        translation:
          - 5.0
          - 0.0
          - 0.0
";

/// District B: the outbound anchor and one native prop. Nothing here is marked.
const DISTRICT_B: &str = "\
format: 1
name: district-b
seed: 2
entities:
  - name: gate-from-a
    components:
      Transform:
        translation:
          - 40.0
          - 0.0
          - 0.0
  - name: beacon
    components:
      Transform:
        translation:
          - 42.0
          - 0.0
          - 0.0
";

/// District A with no `name:` — the scene shape that cannot be keyed, so it keeps no residue.
const DISTRICT_A_UNNAMED: &str = "\
format: 1
seed: 1
entities:
  - name: player
    components:
      Player:
        hp: 100
  - name: door
    components:
      Residue: {}
      Door:
        open: false
  - name: gate-from-b
    components:
      Transform:
        translation:
          - 5.0
          - 0.0
          - 0.0
";

// --- Systems ----------------------------------------------------------------------------------

/// The live player (the entity a switch carries).
fn player_id(world: &World) -> Option<EntityId> {
    let ids: Vec<EntityId> = world.entities().collect();
    ids.into_iter()
        .find(|&id| matches!(world.get::<Player>(id), Ok(Some(_))))
}

/// On frame 1 only: open every door in the district — the mutation that must survive on the marked
/// entity and be forgotten on the unmarked one.
fn open_doors(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 1 {
        return;
    }
    for (door,) in world.query::<(&mut Door,)>() {
        door.open = true;
    }
}

/// On frame 1 only: drop an unnamed, marked pile — the anonymous-record case.
fn drop_echo(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 1 {
        return;
    }
    let id = world.spawn();
    world.insert(id, Echo { amount: 3 }).unwrap();
    world.insert(id, Residue {}).unwrap();
}

/// On frame 1 only: spawn a second marked entity named `door` — the duplicate-name case.
fn duplicate_door(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 1 {
        return;
    }
    let id = world.spawn();
    world.insert(id, Name("door".to_string())).unwrap();
    world.insert(id, Residue {}).unwrap();
    world.insert(id, Door { open: false }).unwrap();
}

/// Requests the outbound switch on frame 2 and the return on frame 5, carrying whatever `carry`
/// selects for that frame.
fn travel_with(world: &mut World, ctx: &SystemCtx, carry: fn(&World, u64) -> Vec<EntityId>) {
    let frame = ctx.frame.as_u64();
    let (path, anchor) = match frame {
        2 => ("b.yaml", "gate-from-a"),
        5 => ("a.yaml", "gate-from-b"),
        _ => return,
    };
    let carried = carry(world, frame);
    if let Some(switch) = world.resource_mut::<SceneSwitch>() {
        switch.request(path.to_string(), anchor.to_string(), carried);
    }
}

/// The ordinary round trip: the player travels, everything else stays with its district.
fn travel(world: &mut World, ctx: &SystemCtx) {
    travel_with(world, ctx, |world, _| {
        player_id(world).into_iter().collect()
    });
}

/// The contradictory round trip: the outbound leg also carries the marked `door`, so it is asked to
/// leave and to stay behind at once (`L3027`). The return carries the player alone, leaving the door
/// where the conflict sent it.
fn travel_carrying_the_door(world: &mut World, ctx: &SystemCtx) {
    travel_with(world, ctx, |world, frame| {
        let mut carried: Vec<EntityId> = player_id(world).into_iter().collect();
        if frame == 2
            && let Some(door) = named_entity(world, "door")
        {
            carried.push(door);
        }
        carried
    });
}

/// The frame the late return fires on. It sits past the default keyframe cadence (300) while the
/// outbound leg stays at frame 2, so a recording's frame-300 keyframe is taken in district B — after
/// district A's residue was captured and before it is re-applied. A reconstruction from that keyframe
/// can only be right if the snapshot carried the residue.
const LATE_RETURN_FRAME: u64 = 310;

/// The round trip stretched around a keyframe boundary: out on frame 2, home on
/// [`LATE_RETURN_FRAME`], carrying the player both ways.
fn travel_late(world: &mut World, ctx: &SystemCtx) {
    let (path, anchor) = match ctx.frame.as_u64() {
        2 => ("b.yaml", "gate-from-a"),
        LATE_RETURN_FRAME => ("a.yaml", "gate-from-b"),
        _ => return,
    };
    let carried: Vec<EntityId> = player_id(world).into_iter().collect();
    if let Some(switch) = world.resource_mut::<SceneSwitch>() {
        switch.request(path.to_string(), anchor.to_string(), carried);
    }
}

/// While the run is away from district A, read that district's `door` out of the mirror and record
/// the reading on the carried player. The read happens for a scene that is not loaded — the whole
/// point of the mirror — and lands in a component, so the value travels in the `WorldHash`.
fn remember_home(world: &mut World, _ctx: &SystemCtx) {
    if named_entity(world, "door").is_some() {
        return;
    }
    let Some(player) = player_id(world) else {
        return;
    };
    let reading = world
        .resource::<ResidueStore>()
        .and_then(|store| store.get::<Door>("district-a", "door"));
    if let Some(door) = reading {
        world.insert(player, Memo { open: door.open }).unwrap();
    }
}

/// Asks the session to forget every district it has visited.
fn request_clear(world: &mut World) {
    if let Some(store) = world.resource_mut::<ResidueStore>() {
        store.clear();
    }
}

/// Requests the clear on frame 4 — standing in district B, after the capture and before the return.
fn forget_city_mid_visit(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 4 {
        request_clear(world);
    }
}

/// Requests the clear on frame 5 — the very frame `travel` requests the return switch, so the two
/// drains meet at one frame boundary.
fn forget_city_on_departure(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 5 {
        request_clear(world);
    }
}

// --- Harness ----------------------------------------------------------------------------------

/// The first entity carrying `Name(name)`.
fn named_entity(world: &World, name: &str) -> Option<EntityId> {
    let ids: Vec<EntityId> = world.entities().collect();
    ids.into_iter().find(|&id| {
        world
            .get::<Name>(id)
            .ok()
            .flatten()
            .is_some_and(|found| found.0 == name)
    })
}

/// Writes the two districts under a process-unique directory, so a switch resolves them off disk.
fn fresh_dir(name: &str, district_a: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-residue-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.yaml"), district_a).unwrap();
    std::fs::write(dir.join("b.yaml"), DISTRICT_B).unwrap();
    dir
}

/// The base game: the three test components plus the district-state systems. A caller adds the
/// travel system that decides what the switch carries.
fn base_game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .components(|reg| {
            reg.register::<Player>("Player")?;
            reg.register::<Door>("Door")?;
            reg.register::<Echo>("Echo")?;
            reg.register::<Memo>("Memo")?;
            Ok(())
        })
        .system("open_doors", open_doors)
        .project_root(root)
}

/// The world's residue mirror.
fn store(engine: &Engine) -> &ResidueStore {
    engine
        .world()
        .resource::<ResidueStore>()
        .expect("the engine installs a mirror into every world it builds")
}

/// Drives the six ticks of a full round trip: frame 1 mutates district A, frame 2 leaves it, frame 5
/// comes back, and the engine is left in district A on frame 6.
fn round_trip(engine: &mut Engine) {
    for _ in 0..6 {
        engine.advance_live();
    }
    assert_eq!(engine.frame().as_u64(), 6, "six ticks ran");
}

/// A named entity's `Door.open` in the current world.
fn door_open(engine: &Engine, name: &str) -> bool {
    let id = named_entity(engine.world(), name).unwrap_or_else(|| panic!("`{name}` is present"));
    engine
        .world()
        .get::<Door>(id)
        .unwrap()
        .unwrap_or_else(|| panic!("`{name}` carries a Door"))
        .open
}

/// A recording sink the test can read back after `stop_recording`.
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

/// One recorded tick: the command path a live session records, as opposed to `advance_live`.
fn sim_step(engine: &mut Engine) {
    engine
        .execute(Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .unwrap();
}

/// The engine's current `(frame, WorldHash)` pair.
fn stamp(engine: &mut Engine) -> (u64, u64) {
    (
        engine.frame().as_u64(),
        engine.world_hash().unwrap().as_u64(),
    )
}

/// Drives `n` recorded ticks, returning the `(frame, hash)` trace seeded with the pre-step state —
/// the shape [`replay_hashes_with`] returns (it stamps the loaded world before the first tick), so the
/// two compare directly.
fn record_trace(engine: &mut Engine, n: usize) -> Vec<(u64, u64)> {
    let mut trace = vec![stamp(engine)];
    for _ in 0..n {
        sim_step(engine);
        trace.push(stamp(engine));
    }
    trace
}

/// Advances `n` live ticks, returning the per-tick `WorldHash` trace.
fn advance_trace(engine: &mut Engine, n: usize) -> Vec<u64> {
    let mut trace = Vec::with_capacity(n);
    for _ in 0..n {
        engine.advance_live();
        trace.push(engine.world_hash().unwrap().as_u64());
    }
    trace
}

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

fn restore(engine: &mut Engine, snapshot: Value) {
    match engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
}

/// Replays a seek's delta records over the public API, returning the reconstructed `(frame, hash)`
/// trace seeded with the restored keyframe's own stamp. `SceneSwitch`/`Keyframe` records are not drive
/// actions — the switch regenerates from the re-driven sim, and with it the residue application.
fn drive_deltas(engine: &mut Engine, deltas: Vec<ReplayRecord>) -> Vec<(u64, u64)> {
    let mut trace = vec![stamp(engine)];
    for record in deltas {
        if let ReplayRecord::Command { command, .. } = record {
            engine.execute(command).unwrap();
            trace.push(stamp(engine));
        }
    }
    trace
}

/// The linear replay's `frame -> hash` reference for a recording.
fn linear_hashes(builder: EngineBuilder, bytes: &[u8], dir: &Path) -> BTreeMap<u64, u64> {
    replay_hashes_with(builder, bytes, dir)
        .unwrap()
        .into_iter()
        .map(|(frame, hash)| (frame.as_u64(), hash.as_u64()))
        .collect()
}

/// The snapshot document's residue records for `scene`, or `None` if it carries none for it.
fn snapshot_records<'a>(snapshot: &'a Value, scene: &str) -> Option<&'a Vec<Value>> {
    snapshot
        .get("residue")?
        .as_array()?
        .iter()
        .find(|entry| entry["scene"].as_str() == Some(scene))?["records"]
        .as_array()
}

// --- Tests ------------------------------------------------------------------------------------

#[test]
fn a_marked_entity_returns_mutated_while_an_unmarked_sibling_resets() {
    let dir = fresh_dir("round-trip", DISTRICT_A);
    let mut engine = base_game(&dir)
        .system("travel", travel)
        .build()
        .expect("boots");
    engine.load_inline_scene(DISTRICT_A).unwrap();

    // Frame 1 opens both doors; frame 2 leaves for district B.
    engine.advance_live();
    engine.advance_live();
    assert!(door_open(&engine, "door"), "the marked door opened");
    assert!(door_open(&engine, "lamp"), "the unmarked lamp opened too");

    for _ in 0..4 {
        engine.advance_live();
    }
    assert!(
        engine.last_transition_error().is_none(),
        "a clean round trip reports nothing: {:?}",
        engine.last_transition_error()
    );

    assert!(
        door_open(&engine, "door"),
        "the marked door came back open — its captured state was re-applied verbatim"
    );
    assert!(
        !door_open(&engine, "lamp"),
        "the unmarked lamp came back shut — an unmarked entity is rebuilt from the scene text"
    );
}

#[test]
fn a_matched_record_replaces_in_place_and_keeps_its_entity_id() {
    let dir = fresh_dir("in-place", DISTRICT_A);
    let mut engine = base_game(&dir)
        .system("travel", travel)
        .build()
        .expect("boots");
    engine.load_inline_scene(DISTRICT_A).unwrap();

    let authored_door = named_entity(engine.world(), "door").expect("district A authors a door");
    let authored_count = engine.world().entity_count();

    round_trip(&mut engine);

    assert_eq!(
        named_entity(engine.world(), "door"),
        Some(authored_door),
        "the returning record landed on the entity the scene minted, at the same id"
    );
    assert_eq!(
        engine.world().entity_count(),
        authored_count + 1,
        "district A's authored set plus the one carried player — a matched record spawns nothing"
    );
}

#[test]
fn an_anonymous_record_respawns_on_return() {
    let dir = fresh_dir("anonymous", DISTRICT_A);
    let mut engine = base_game(&dir)
        .system("drop_echo", drop_echo)
        .system("travel", travel)
        .build()
        .expect("boots");
    engine.load_inline_scene(DISTRICT_A).unwrap();

    round_trip(&mut engine);

    let echoes: Vec<EntityId> = engine
        .world()
        .entities()
        .filter(|&id| matches!(engine.world().get::<Echo>(id), Ok(Some(_))))
        .collect();
    assert_eq!(
        echoes.len(),
        1,
        "the dropped pile came back exactly once (district A authors none)"
    );
    assert_eq!(
        engine.world().get::<Echo>(echoes[0]).unwrap(),
        Some(&Echo { amount: 3 }),
        "the anonymous record's components came back verbatim"
    );
    assert!(
        engine.world().has::<Residue>(echoes[0]),
        "the respawned pile is still marked, so leaving again captures it again"
    );
    assert!(
        engine.world().get::<Name>(echoes[0]).unwrap().is_none(),
        "an anonymous record comes back anonymous — it is never given an identity"
    );
}

#[test]
fn two_marked_entities_sharing_a_name_report_l3026_and_the_first_is_captured() {
    let dir = fresh_dir("duplicate-name", DISTRICT_A);
    let mut engine = base_game(&dir)
        // Registered after `open_doors`, so the duplicate it spawns stays shut while the authored
        // door is already open — the two records are then distinguishable on return.
        .system("duplicate_door", duplicate_door)
        .system("travel", travel)
        .build()
        .expect("boots");
    engine.load_inline_scene(DISTRICT_A).unwrap();

    // Frames 0–2: the duplicate appears, then the outbound switch captures the district.
    for _ in 0..3 {
        engine.advance_live();
    }
    let warning = engine
        .last_transition_error()
        .expect("the duplicate name is reported");
    assert_eq!(warning.code.as_str(), "L3026", "{warning}");
    assert!(
        warning.message.contains("`door`"),
        "the warning names the repeated name: {warning}"
    );

    // The switch itself happened: the world is district B.
    assert!(
        named_entity(engine.world(), "gate-from-a").is_some(),
        "a duplicate name is a warning, not a rejection"
    );

    for _ in 0..3 {
        engine.advance_live();
    }
    let doors: Vec<EntityId> = engine
        .world()
        .entities()
        .filter(|&id| matches!(engine.world().get::<Door>(id), Ok(Some(_))))
        .filter(|&id| {
            engine
                .world()
                .get::<Name>(id)
                .unwrap()
                .is_some_and(|name| name.0 == "door")
        })
        .collect();
    assert_eq!(doors.len(), 1, "only one record can answer to one name");
    assert!(
        door_open(&engine, "door"),
        "the record kept is the first in entity order — the authored, opened door"
    );
}

#[test]
fn an_entity_both_carried_and_marked_reports_l3027_and_the_carry_wins() {
    let dir = fresh_dir("carry-conflict", DISTRICT_A);
    let mut engine = base_game(&dir)
        .system("travel_carrying_the_door", travel_carrying_the_door)
        .build()
        .expect("boots");
    engine.load_inline_scene(DISTRICT_A).unwrap();

    for _ in 0..3 {
        engine.advance_live();
    }
    let warning = engine
        .last_transition_error()
        .expect("the carried-and-marked entity is reported");
    assert_eq!(warning.code.as_str(), "L3027", "{warning}");

    // The carry won: the door crossed into district B, opened, with the player.
    assert!(
        door_open(&engine, "door"),
        "the marked door travelled — the carry is what the engine honors"
    );

    for _ in 0..3 {
        engine.advance_live();
    }
    assert_eq!(
        engine
            .world()
            .entities()
            .filter(|&id| engine
                .world()
                .get::<Name>(id)
                .unwrap()
                .is_some_and(|name| name.0 == "door"))
            .count(),
        1,
        "the door that travelled stayed in district B; district A rebuilt its own"
    );
    assert!(
        !door_open(&engine, "door"),
        "no residue was captured for the carried entity, so district A's door is authored-fresh"
    );
}

#[test]
fn a_residue_free_round_trip_rebuilds_the_district_from_its_text() {
    let unmarked = DISTRICT_A.replace("      Residue: {}\n", "");
    assert!(!unmarked.contains("Residue"), "the marker really is gone");
    let dir = fresh_dir("unmarked", &unmarked);
    let mut engine = base_game(&dir)
        .system("travel", travel)
        .build()
        .expect("boots");
    engine.load_inline_scene(&unmarked).unwrap();

    let authored_count = engine.world().entity_count();
    round_trip(&mut engine);

    assert!(
        engine.last_transition_error().is_none(),
        "an unmarked round trip reports nothing: {:?}",
        engine.last_transition_error()
    );
    assert!(
        !door_open(&engine, "door"),
        "a district that marks nothing spawns fresh — the opened door is forgotten"
    );
    assert_eq!(
        engine.world().entity_count(),
        authored_count + 1,
        "the authored set plus the carried player, exactly as before residue existed"
    );
}

#[test]
fn a_scene_without_a_name_keeps_no_residue() {
    let dir = fresh_dir("unnamed", DISTRICT_A_UNNAMED);
    let mut engine = base_game(&dir)
        .system("travel", travel)
        .build()
        .expect("boots");
    engine.load_inline_scene(DISTRICT_A_UNNAMED).unwrap();

    round_trip(&mut engine);

    assert!(
        engine.last_transition_error().is_none(),
        "an unkeyable scene is not an error: {:?}",
        engine.last_transition_error()
    );
    assert!(
        !door_open(&engine, "door"),
        "the scene's `name:` is the residue key, so a scene without one remembers nothing"
    );
}

#[test]
fn residue_does_not_survive_a_fresh_scene_load() {
    let dir = fresh_dir("reload", DISTRICT_A);
    let mut engine = base_game(&dir)
        .system("travel", travel)
        .build()
        .expect("boots");
    engine.load_inline_scene(DISTRICT_A).unwrap();

    // Open the door and leave district A, so the session holds its residue.
    for _ in 0..3 {
        engine.advance_live();
    }
    assert!(
        named_entity(engine.world(), "gate-from-a").is_some(),
        "in B"
    );

    // A `scene.load` starts a run, and a new run does not inherit the last one's city: the drive
    // below walks back into district A on a session that has forgotten it.
    engine.load_inline_scene(DISTRICT_B).unwrap();
    round_trip(&mut engine);

    assert!(
        !door_open(&engine, "door"),
        "the load cleared the session's residue, so district A rebuilt from its text"
    );
}

// --- Reproducibility: replay ------------------------------------------------------------------

/// Residue is **regenerated, never recorded**. Capture is a pure function of the departing world and
/// the registry, application a pure function of the residue map and the target text — so a replay,
/// driving the same ticks, re-requests the same switches at the same frames and rebuilds the same
/// residue. This is the claim that keeps `REPLAY_FORMAT_VERSION` at `1` and the record-kind set at
/// five: a recorded A→B→A round trip whose return depends on residue replays to a bit-identical
/// per-frame hash trace, and the recording contains nothing but the kinds that existed before.
#[test]
fn a_recorded_round_trip_replays_bit_identically_with_no_new_record_kind() {
    let dir = fresh_dir("recorded-round-trip", DISTRICT_A);
    let sink = SharedSink::default();
    let mut engine = base_game(&dir)
        .system("travel", travel)
        .build()
        .expect("boots");
    engine.load_inline_scene(DISTRICT_A).unwrap();
    engine.set_recording(sink.clone()).unwrap();

    // The full round trip (out on frame 2, home on frame 5) plus a tail of ordinary ticks in the
    // returned-to district, all over the recorded command path.
    let live = record_trace(&mut engine, 10);
    engine.stop_recording().unwrap();

    assert!(
        engine.last_transition_error().is_none(),
        "the recorded round trip is clean: {:?}",
        engine.last_transition_error()
    );
    assert!(
        door_open(&engine, "door"),
        "the recorded session really did exercise residue — the door came home open"
    );

    let bytes = sink.0.lock().unwrap().clone();
    let replayed: Vec<(u64, u64)> =
        replay_hashes_with(base_game(&dir).system("travel", travel), &bytes, &dir)
            .unwrap()
            .into_iter()
            .map(|(frame, hash)| (frame.as_u64(), hash.as_u64()))
            .collect();
    assert_eq!(
        replayed, live,
        "the replayed trace is bit-identical to the live one, residue and all"
    );

    // The format is unmoved: no version bump, and every record still answers to one of the six kinds
    // (five existed before residue; `scene_load` came later and this run never emits it). The exhaustive match below (no wildcard arm) is the compile-time
    // half of that claim; parsing the whole recording without error is the runtime half.
    assert_eq!(
        REPLAY_FORMAT_VERSION, 1,
        "residue needed no replay-format bump"
    );
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for record in ReplayReader::new(&bytes).unwrap().records().unwrap() {
        let kind = match record {
            ReplayRecord::Command { .. } => "command",
            ReplayRecord::Input { .. } => "input",
            ReplayRecord::Keyframe { .. } => "keyframe",
            ReplayRecord::DocReload { .. } => "doc_reload",
            ReplayRecord::SceneSwitch { .. } => "scene_switch",
            ReplayRecord::SceneLoad { .. } => "scene_load",
            ReplayRecord::PrefabText { .. } => "prefab_text",
            ReplayRecord::SystemPanic { .. } => "system_panic",
        };
        *counts.entry(kind).or_default() += 1;
    }
    assert_eq!(
        counts.get("scene_switch").copied(),
        Some(2),
        "both legs of the round trip recorded their target text: {counts:?}"
    );
    assert_eq!(
        counts.keys().copied().collect::<Vec<_>>(),
        vec!["command", "keyframe", "scene_switch"],
        "no kind outside the five residue-era kinds appears: {counts:?}"
    );
}

/// The decisive seek. A keyframe is a snapshot, and a seek reconstructs the run from it — so a
/// keyframe taken *away from home*, after district A's residue was captured and before the return
/// re-applies it, can only reconstruct correctly if the snapshot carried the residue and the restore
/// adopted it. The return is pushed past the default keyframe cadence (300) to arrange exactly that:
/// the frame-300 keyframe lands in district B holding district A's memory, the re-driven deltas cross
/// the return, and the reconstruction must match the linear replay frame for frame — with the door
/// open, which is only possible if the memory travelled through the snapshot.
#[test]
fn a_keyframe_seek_landing_away_from_home_reproduces_the_return() {
    let dir = fresh_dir("keyframe-seek", DISTRICT_A);
    let sink = SharedSink::default();
    let mut engine = base_game(&dir)
        .system("travel_late", travel_late)
        .build()
        .expect("boots");
    engine.load_inline_scene(DISTRICT_A).unwrap();
    engine.set_recording(sink.clone()).unwrap();
    record_trace(&mut engine, (LATE_RETURN_FRAME + 5) as usize);
    engine.stop_recording().unwrap();
    assert!(
        door_open(&engine, "door"),
        "the live session came home to an open door"
    );

    let bytes = sink.0.lock().unwrap().clone();
    let linear = linear_hashes(
        base_game(&dir).system("travel_late", travel_late),
        &bytes,
        &dir,
    );

    let target = FrameIndex::new(LATE_RETURN_FRAME + 2);
    let seek = ReplayReader::new(&bytes).unwrap().seek(target).unwrap();
    let (keyframe_frame, keyframe) = seek
        .keyframe
        .clone()
        .expect("a keyframe at or before the target");
    assert!(
        keyframe_frame.as_u64() > 2 && keyframe_frame.as_u64() < LATE_RETURN_FRAME,
        "the chosen keyframe (frame {}) sits in district B, after the capture and before the return",
        keyframe_frame.as_u64()
    );

    let mut reconstructed = base_game(&dir)
        .system("travel_late", travel_late)
        .build()
        .expect("boots");
    restore(&mut reconstructed, keyframe);
    assert!(
        named_entity(reconstructed.world(), "gate-from-a").is_some(),
        "the keyframe reconstructs district B — the run is away from home"
    );
    assert_eq!(
        reconstructed.world_hash().unwrap().as_u64(),
        linear[&keyframe_frame.as_u64()],
        "the reconstructed keyframe matches the linear replay at that frame"
    );

    for (frame, hash) in drive_deltas(&mut reconstructed, seek.deltas) {
        assert_eq!(
            hash, linear[&frame],
            "post-seek frame {frame} matches linear"
        );
    }

    assert!(
        reconstructed.frame().as_u64() > LATE_RETURN_FRAME,
        "the reconstruction ran past the return"
    );
    assert!(
        door_open(&reconstructed, "door"),
        "the re-driven return re-applied the residue the keyframe carried"
    );
    assert!(
        !door_open(&reconstructed, "lamp"),
        "and only that: the unmarked lamp came back from the scene text, shut"
    );
}

// --- Reproducibility: snapshot ----------------------------------------------------------------

/// The `ludion dev` restart, taken mid-trip. The snapshot is captured in district B — the mutation
/// that must survive exists nowhere in the world at that moment, only in the session's residue — and a
/// brand-new engine built from the same config restores it and drives on through the return. The
/// snapshot document's shape is asserted directly (one scene entry, keyed by district A's `name:`,
/// holding the opened `Door` under the record named `door`), because that field is the entire
/// mechanism: without it the restarted run walks home to an authored-fresh district.
#[test]
fn a_dev_style_restore_across_a_return_preserves_residue() {
    let dir = fresh_dir("dev-restart", DISTRICT_A);
    let mut original = base_game(&dir)
        .system("travel", travel)
        .build()
        .expect("boots");
    original.load_inline_scene(DISTRICT_A).unwrap();

    // Frame 1 opens the doors, frame 2 leaves; the snapshot is taken standing in district B, before
    // the frame-5 return.
    for _ in 0..3 {
        original.advance_live();
    }
    assert!(
        named_entity(original.world(), "gate-from-a").is_some(),
        "the snapshot is taken away from home"
    );
    let snapshot = snapshot_of(&mut original);

    let records = snapshot_records(&snapshot, "district-a")
        .expect("the snapshot carries district A's residue under its scene name");
    assert_eq!(
        snapshot["residue"].as_array().unwrap().len(),
        1,
        "one visited scene kept something behind"
    );
    assert_eq!(records.len(), 1, "district A marks exactly one entity");
    assert_eq!(
        records[0]["name"].as_str(),
        Some("door"),
        "the record is keyed by the `Name` a return matches on"
    );
    let bag = records[0]["bag"].as_array().expect("the record has a bag");
    let door = bag
        .iter()
        .find(|entry| entry[0].as_str() == Some("Door"))
        .expect("the bag holds the game's own state component");
    assert_eq!(
        door[1]["open"],
        Value::Bool(true),
        "the bag holds the mutated value, not the authored one"
    );
    assert!(
        !bag.iter().any(|entry| entry[0].as_str() == Some("Name")),
        "identity is the key, never payload"
    );
    assert_eq!(
        snapshot["residue"][0]["last_visit"].as_u64(),
        Some(3),
        "the entry carries the frame the switch left the scene on — the eviction key"
    );

    // The uninterrupted original crosses the return and carries on — the reference trace.
    let original_continuation = advance_trace(&mut original, 5);
    assert!(door_open(&original, "door"));

    // The restart: a fresh engine from the same builder config restores the snapshot and drives the
    // same ticks. The return fires on the restored timeline (frame 5), rebuilding district A.
    let mut restarted = base_game(&dir)
        .system("travel", travel)
        .build()
        .expect("boots");
    restore(&mut restarted, snapshot);
    assert_eq!(restarted.frame().as_u64(), 3, "the frame counter continues");
    assert_eq!(
        snapshot_of(&mut restarted)["residue"][0]["last_visit"].as_u64(),
        Some(3),
        "the last-visit frame survives the round trip, so a restored session evicts what the          uninterrupted one would"
    );

    let restarted_continuation = advance_trace(&mut restarted, 5);
    assert_eq!(
        restarted_continuation, original_continuation,
        "the restarted run is bit-identical to the uninterrupted original across the return"
    );
    assert!(
        restarted.last_transition_error().is_none(),
        "the post-restore return is clean: {:?}",
        restarted.last_transition_error()
    );
    assert!(
        door_open(&restarted, "door"),
        "the mutation survived snapshot -> restart -> restore -> return"
    );
    assert!(
        !door_open(&restarted, "lamp"),
        "the unmarked sibling reset, exactly as it does without a restart"
    );
}

/// The opt-in guarantee, at the snapshot layer. A session that marks nothing captures nothing, so the
/// `residue` field is skipped entirely and the document is what it was before the field existed —
/// which a restore-and-recapture byte comparison pins.
#[test]
fn a_residue_free_snapshot_omits_the_field_and_round_trips_byte_identically() {
    let unmarked = DISTRICT_A.replace("      Residue: {}\n", "");
    assert!(!unmarked.contains("Residue"), "the marker really is gone");
    let dir = fresh_dir("residue-free-snapshot", &unmarked);
    let mut engine = base_game(&dir)
        .system("travel", travel)
        .build()
        .expect("boots");
    engine.load_inline_scene(&unmarked).unwrap();

    // Leave district A, so a marking game would be holding its residue right now.
    for _ in 0..3 {
        engine.advance_live();
    }
    assert!(named_entity(engine.world(), "gate-from-a").is_some());

    let snapshot = snapshot_of(&mut engine);
    assert_eq!(
        snapshot["format"].as_u64(),
        Some(1),
        "SNAPSHOT_FORMAT stays 1 — the field is additive"
    );
    assert!(
        !snapshot.as_object().unwrap().contains_key("residue"),
        "a game that marks nothing writes no `residue` key: {:?}",
        snapshot.as_object().unwrap().keys().collect::<Vec<_>>()
    );

    let mut restored = base_game(&dir)
        .system("travel", travel)
        .build()
        .expect("boots");
    restore(&mut restored, snapshot.clone());
    let recaptured = snapshot_of(&mut restored);
    assert_eq!(
        serde_json::to_string(&snapshot).unwrap(),
        serde_json::to_string(&recaptured).unwrap(),
        "a residue-free snapshot round-trips byte-identically"
    );
}

/// The other direction of the same additive rule: a document written before `residue` existed has no
/// such key, `#[serde(default)]` reads it as an empty set, and the restore adopts nothing. The run
/// resumes with a session that has forgotten district A, so the return rebuilds it from its text — no
/// error, no memory. Simulated by stripping the field from a snapshot that has one.
#[test]
fn an_old_snapshot_without_residue_restores_with_no_memory() {
    let dir = fresh_dir("old-snapshot", DISTRICT_A);
    let mut engine = base_game(&dir)
        .system("travel", travel)
        .build()
        .expect("boots");
    engine.load_inline_scene(DISTRICT_A).unwrap();
    for _ in 0..3 {
        engine.advance_live();
    }

    let mut snapshot = snapshot_of(&mut engine);
    assert!(
        snapshot
            .as_object_mut()
            .unwrap()
            .remove("residue")
            .is_some(),
        "the snapshot carried the field to strip"
    );

    let mut restored = base_game(&dir)
        .system("travel", travel)
        .build()
        .expect("boots");
    restore(&mut restored, snapshot);
    for _ in 0..5 {
        restored.advance_live();
    }

    assert!(
        restored.last_transition_error().is_none(),
        "an older document is not an error: {:?}",
        restored.last_transition_error()
    );
    assert!(
        !door_open(&restored, "door"),
        "no residue was adopted, so the return rebuilt district A from its text"
    );
}

// --- The mirror: reading residue from another district ----------------------------------------

/// The mirror's reason to exist: a system reads a scene the run is **not in**. Standing in district
/// B, `remember_home` asks the store what district A kept and writes the answer onto the carried
/// player — so the reading is sim state, hashed like any other component. The recorded session and
/// its replay then produce the same per-frame trace, which is the claim: the mirror is a copy of
/// engine-owned session state, identical live and under replay, and it is still nothing a recording
/// carries.
#[test]
fn a_system_reads_a_scene_it_is_not_in_identically_live_and_under_replay() {
    let dir = fresh_dir("mirror-read", DISTRICT_A);
    let sink = SharedSink::default();
    let game = || {
        base_game(&dir)
            .system("travel", travel)
            .system("remember_home", remember_home)
    };
    let mut engine = game().build().expect("boots");
    engine.load_inline_scene(DISTRICT_A).unwrap();
    engine.set_recording(sink.clone()).unwrap();

    let live = record_trace(&mut engine, 10);
    engine.stop_recording().unwrap();

    // District A authors a player of its own, so the returned world holds two; the reading rode home
    // on the *carried* one, and it is the only entity that carries a memo at all.
    let memos: Vec<Memo> = engine
        .world()
        .entities()
        .filter_map(|id| engine.world().get::<Memo>(id).unwrap().copied())
        .collect();
    assert_eq!(
        memos,
        vec![Memo { open: true }],
        "the system in district B read district A's opened door out of the mirror"
    );

    let bytes = sink.0.lock().unwrap().clone();
    let replayed: Vec<(u64, u64)> = replay_hashes_with(game(), &bytes, &dir)
        .unwrap()
        .into_iter()
        .map(|(frame, hash)| (frame.as_u64(), hash.as_u64()))
        .collect();
    assert_eq!(
        replayed, live,
        "the replay read the same values out of its own mirror — the memo is in the hash"
    );
}

/// The mirror's readout matches the session map it copies: empty at a fresh load, and after one
/// departure holding exactly that scene, its last-visit frame, its record count, and the record's
/// component. Nothing that was not captured is readable through it.
#[test]
fn the_mirror_reports_the_session_map_and_is_empty_on_a_fresh_load() {
    let dir = fresh_dir("mirror-shape", DISTRICT_A);
    let mut engine = base_game(&dir)
        .system("travel", travel)
        .build()
        .expect("boots");
    engine.load_inline_scene(DISTRICT_A).unwrap();
    assert_eq!(
        store(&engine).scenes().count(),
        0,
        "a run that has left no district remembers nothing"
    );

    // Frame 1 opens the doors, frame 2 leaves — the boundary commits the capture and reinstalls the
    // mirror, so the district just left is readable from the district just entered.
    for _ in 0..3 {
        engine.advance_live();
    }
    let mirror = store(&engine);
    assert_eq!(
        mirror.scenes().collect::<Vec<_>>(),
        vec!["district-a"],
        "one visited scene, keyed by its `name:`"
    );
    assert_eq!(
        mirror.last_visit("district-a").map(FrameIndex::as_u64),
        Some(3),
        "the last-visit frame the engine evicts on is what the mirror reports"
    );
    assert_eq!(
        mirror.record_count("district-a"),
        Some(1),
        "district A marks exactly one entity"
    );
    assert_eq!(
        mirror.get::<Door>("district-a", "door"),
        Some(Door { open: true }),
        "the record's component reads back as the departing world left it"
    );
    assert!(
        mirror.get::<Door>("district-a", "lamp").is_none(),
        "the unmarked sibling was never captured, so it is not readable"
    );
    assert!(
        mirror.get::<Player>("district-a", "door").is_none(),
        "the record carried no `Player`"
    );
    assert!(
        mirror.get::<Door>("district-b", "door").is_none(),
        "a scene that kept nothing reads as nothing"
    );
    assert!(
        mirror.last_visit("district-b").is_none() && mirror.record_count("district-b").is_none()
    );

    // A `scene.load` starts a new run, and the mirror is rebuilt from the emptied session map.
    engine.load_inline_scene(DISTRICT_A).unwrap();
    assert_eq!(
        store(&engine).scenes().count(),
        0,
        "the load cleared the session, and the fresh world's mirror with it"
    );
}

/// `clear()` is the only write, and the engine honors it at the frame boundary: a system asks the
/// session to forget the city mid-visit, the mirror empties without a world rebuild, and the return
/// walks into a district loaded fresh from its text — the shape a run restarting in place needs.
#[test]
fn a_requested_clear_empties_the_mirror_at_the_boundary_and_the_return_loads_fresh() {
    let dir = fresh_dir("mirror-clear", DISTRICT_A);
    let mut engine = base_game(&dir)
        .system("travel", travel)
        .system("forget_city_mid_visit", forget_city_mid_visit)
        .build()
        .expect("boots");
    engine.load_inline_scene(DISTRICT_A).unwrap();

    // Frames 0–3: the door opens, the run leaves, and district A's memory is standing in the mirror.
    for _ in 0..4 {
        engine.advance_live();
    }
    assert_eq!(
        store(&engine).scenes().collect::<Vec<_>>(),
        vec!["district-a"],
        "the memory the clear is about to drop"
    );

    // Frame 4 requests the clear; the boundary honors it.
    engine.advance_live();
    assert_eq!(
        store(&engine).scenes().count(),
        0,
        "the drain emptied the session and reinstalled the mirror — no world rebuild involved"
    );
    assert!(
        named_entity(engine.world(), "gate-from-a").is_some(),
        "and the world is untouched: still district B, same entities"
    );

    // Frame 5 goes home.
    engine.advance_live();
    assert!(
        engine.last_transition_error().is_none(),
        "a return onto a forgotten district is not an error: {:?}",
        engine.last_transition_error()
    );
    assert!(
        !door_open(&engine, "door"),
        "the city was forgotten, so district A rebuilt from its text with the door shut"
    );
}

/// District B with a marked prop of its own — the departing scene of the same-frame clear-then-switch
/// test, whose capture has to survive the clear it shares a frame boundary with.
fn district_b_marked() -> String {
    const RELAY: &str = "  - name: relay
    components:
      Residue: {}
      Door:
        open: false
";
    format!("{DISTRICT_B}{RELAY}")
}

/// The drain order, stated as a test: a clear and a switch requested on the same frame compose, with
/// the clear first. The run forgets district A while standing in district B and departs in the same
/// breath — so the return finds A authored-fresh (the clear ran) while B's own capture, taken after
/// it, is in the mirror (the switch ran second, into the emptied map).
#[test]
fn a_clear_and_a_switch_on_one_frame_forget_the_old_city_and_keep_the_departing_district() {
    let dir = fresh_dir("mirror-clear-then-switch", DISTRICT_A);
    std::fs::write(dir.join("b.yaml"), district_b_marked()).unwrap();
    let mut engine = base_game(&dir)
        // Opens every door every tick, so district B's `relay` is captured mutated on the way out.
        .system("open_doors_always", open_doors_always)
        .system("travel", travel)
        .system("forget_city_on_departure", forget_city_on_departure)
        .build()
        .expect("boots");
    engine.load_inline_scene(DISTRICT_A).unwrap();

    // Out on frame 2, home on frame 5 — the frame that also requests the clear.
    round_trip(&mut engine);
    assert!(
        engine.last_transition_error().is_none(),
        "the round trip is clean: {:?}",
        engine.last_transition_error()
    );

    assert!(
        !door_open(&engine, "door"),
        "the clear ran first, so the return re-applied nothing and district A is authored-fresh"
    );
    let mirror = store(&engine);
    assert_eq!(
        mirror.scenes().collect::<Vec<_>>(),
        vec!["district-b"],
        "and the switch ran second, capturing the district being left into the emptied map"
    );
    assert_eq!(
        mirror.get::<Door>("district-b", "relay"),
        Some(Door { open: true }),
        "the departing capture survived the clear it shared a boundary with, mutations and all"
    );
}

// --- Bounds -----------------------------------------------------------------------------------

/// The scenes the bounds drive walks: two more than the map holds, so 33 departures overflow it and
/// the last switch still has somewhere to land.
const BOUNDS_SCENES: usize = RESIDUE_MAX_SCENES + 2;

/// One numbered bounds scene: a marked `mark` entity the drive mutates, and the `gate` anchor every
/// switch drops into.
fn bounds_scene(index: usize) -> String {
    format!(
        "\
format: 1
name: scene-{index:02}
seed: 1
entities:
  - name: mark
    components:
      Residue: {{}}
      Door:
        open: false
  - name: gate
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
"
    )
}

/// Writes every bounds scene under a process-unique directory.
fn bounds_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-residue-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for index in 0..BOUNDS_SCENES {
        std::fs::write(
            dir.join(format!("scene-{index:02}.yaml")),
            bounds_scene(index),
        )
        .unwrap();
    }
    dir
}

/// Opens every door every tick, so whichever scene a switch leaves is captured mutated and an
/// arriving scene's authored `open: false` is only ever visible before its first tick.
fn open_doors_always(world: &mut World, _ctx: &SystemCtx) {
    for (door,) in world.query::<(&mut Door,)>() {
        door.open = true;
    }
}

/// The bounds route, keyed by the frame whose systems request it: walk `scene-00` to `scene-33` one
/// switch per tick (33 departures, one past what the map holds), then back into the evicted
/// `scene-00`, then on to `scene-31`, which the map still remembers.
fn bounds_target(frame: u64) -> Option<usize> {
    match frame {
        0..=32 => Some(frame as usize + 1),
        33 => Some(0),
        34 => Some(31),
        _ => None,
    }
}

fn travel_bounds(world: &mut World, ctx: &SystemCtx) {
    let Some(target) = bounds_target(ctx.frame.as_u64()) else {
        return;
    };
    if let Some(switch) = world.resource_mut::<SceneSwitch>() {
        switch.request(
            format!("scene-{target:02}.yaml"),
            "gate".to_string(),
            Vec::new(),
        );
    }
}

/// The bounds game: `Door` alone, opened every tick, walking [`bounds_target`]'s route. Nothing is
/// carried — the drive is about the map, not the player.
fn bounds_game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .components(|reg| {
            reg.register::<Door>("Door")?;
            Ok(())
        })
        .system("open_doors_always", open_doors_always)
        .system("travel_bounds", travel_bounds)
        .project_root(root)
}

/// The `(scene, last_visit)` pairs a snapshot carries, in the document's own (ascending scene-key)
/// order. Empty when the session kept nothing.
fn residue_visits(snapshot: &Value) -> Vec<(String, u64)> {
    snapshot["residue"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        entry["scene"].as_str().expect("a scene key").to_string(),
                        entry["last_visit"].as_u64().unwrap_or(0),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `(code, message)` of the switch report standing on the engine.
fn transition_report(engine: &Engine) -> (String, String) {
    let warning = engine
        .last_transition_error()
        .expect("a switch reported something");
    (warning.code.as_str().to_string(), warning.message.clone())
}

/// The session's residue map is bounded, and the bound is held by dropping the scene the run visited
/// longest ago — the one whose last departure sits on the lowest frame. The drive walks 33 distinct
/// districts one switch per tick, exactly one more than the map holds.
#[test]
fn the_scene_bound_evicts_the_least_recently_visited_and_reports_l3028() {
    let dir = bounds_dir("scene-bound");
    let mut engine = bounds_game(&dir).build().expect("boots");
    engine.load_inline_scene(&bounds_scene(0)).unwrap();

    // 32 departures fill the map to exactly its bound, reporting nothing.
    for _ in 0..RESIDUE_MAX_SCENES {
        engine.advance_live();
    }
    assert!(
        engine.last_transition_error().is_none(),
        "a drive that stays inside the bound is clean: {:?}",
        engine.last_transition_error()
    );
    let visits = residue_visits(&snapshot_of(&mut engine));
    assert_eq!(
        visits.len(),
        RESIDUE_MAX_SCENES,
        "the map filled to its bound"
    );
    assert_eq!(
        visits[0].0, "scene-00",
        "the first district visited is held"
    );

    // The 33rd departure overflows it.
    engine.advance_live();
    let (code, message) = transition_report(&engine);
    assert_eq!(code, "L3028", "{message}");
    assert!(
        message.contains("`scene-00`"),
        "the report names the evicted scene: {message}"
    );

    let visits = residue_visits(&snapshot_of(&mut engine));
    assert_eq!(
        visits.len(),
        RESIDUE_MAX_SCENES,
        "the map is still exactly at its bound"
    );
    assert!(
        !visits.iter().any(|(scene, _)| scene == "scene-00"),
        "the district left longest ago is the one dropped: {visits:?}"
    );
    assert_eq!(
        visits[0].0, "scene-01",
        "and the next-oldest is now the head"
    );
    let frames: Vec<u64> = visits.iter().map(|(_, frame)| *frame).collect();
    assert!(
        frames.windows(2).all(|pair| pair[0] < pair[1]),
        "one switch per tick, so last-visit ascends with the walk — which is what makes the dropped \
         entry provably the minimum: {frames:?}"
    );

    // Walking back into the evicted district finds it authored: eviction is forgetting, not hiding.
    engine.advance_live();
    assert!(
        !door_open(&engine, "mark"),
        "the evicted scene loads fresh from its text on return"
    );

    // A district still inside the bound remembers, so the drop was surgical.
    engine.advance_live();
    assert!(
        door_open(&engine, "mark"),
        "a scene the map still holds returns mutated"
    );

    // The same drive twice drops the same districts: the eviction key is a frame index, sim state.
    let mut twin = bounds_game(&dir).build().expect("boots");
    twin.load_inline_scene(&bounds_scene(0)).unwrap();
    for _ in 0..RESIDUE_MAX_SCENES + 3 {
        twin.advance_live();
    }
    assert_eq!(
        residue_visits(&snapshot_of(&mut twin)),
        residue_visits(&snapshot_of(&mut engine)),
        "eviction is deterministic"
    );
}

/// The scene the per-scene overflow drive leaves. It marks nothing itself — the flood system supplies
/// every record.
const OVERFLOW_A: &str = "\
format: 1
name: overflow-a
seed: 1
entities:
  - name: post
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
";

/// On frame 0: drop one anonymous `Residue` record more than a scene's residue holds — the shape a
/// long visit reaches by despawning nothing.
fn flood_residue(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 0 {
        return;
    }
    for amount in 0..=RESIDUE_MAX_RECORDS_PER_SCENE {
        let id = world.spawn();
        world
            .insert(
                id,
                Echo {
                    amount: amount as u32,
                },
            )
            .unwrap();
        world.insert(id, Residue {}).unwrap();
    }
}

/// Leaves for district B on frame 0, carrying nothing.
fn travel_once(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 0 {
        return;
    }
    if let Some(switch) = world.resource_mut::<SceneSwitch>() {
        switch.request("b.yaml".to_string(), "gate-from-a".to_string(), Vec::new());
    }
}

/// The per-scene bound's twin: one scene's capture is truncated in capture order rather than allowed
/// to grow, and the drop is reported under the same code.
#[test]
fn the_per_scene_record_bound_drops_the_tail_and_reports_l3028() {
    let dir = fresh_dir("record-bound", OVERFLOW_A);
    let mut engine = base_game(&dir)
        .system("flood_residue", flood_residue)
        .system("travel_once", travel_once)
        .build()
        .expect("boots");
    engine.load_inline_scene(OVERFLOW_A).unwrap();
    engine.advance_live();

    let (code, message) = transition_report(&engine);
    assert_eq!(code, "L3028", "{message}");
    assert!(
        message.contains("`overflow-a`"),
        "the report names the scene: {message}"
    );
    assert!(
        message.contains(&(RESIDUE_MAX_RECORDS_PER_SCENE + 1).to_string()),
        "and how many it tried to keep: {message}"
    );

    let snapshot = snapshot_of(&mut engine);
    let records =
        snapshot_records(&snapshot, "overflow-a").expect("the scene kept what fits under its name");
    assert_eq!(
        records.len(),
        RESIDUE_MAX_RECORDS_PER_SCENE,
        "the scene keeps exactly its bound"
    );
    let amount = |record: &Value| {
        record["bag"]
            .as_array()
            .expect("a bag")
            .iter()
            .find(|entry| entry[0].as_str() == Some("Echo"))
            .expect("the flooded component")[1]["amount"]
            .as_u64()
            .expect("a count")
    };
    assert_eq!(amount(&records[0]), 0, "the head of the capture is kept");
    assert_eq!(
        amount(records.last().unwrap()),
        RESIDUE_MAX_RECORDS_PER_SCENE as u64 - 1,
        "the tail is what is dropped — capture order decides, so the drop is deterministic"
    );
}

// --- Records the engine refuses to guess at --------------------------------------------------

/// District A with its unmarked `lamp` renamed to `door`, so two authored entities answer to the one
/// name a record addresses — the return-side collision.
fn twin_doors() -> String {
    let twins = DISTRICT_A.replace("- name: lamp", "- name: door");
    assert!(!twins.contains("lamp"), "the lamp really was renamed");
    twins
}

/// On frame 1 only: spawn a *named*, marked entity the scene document never names — the code-spawned
/// census a game keeps for a district (`sector-spawners`), whose only home is its own record.
fn drop_named_echo(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 1 {
        return;
    }
    let id = world.spawn();
    world.insert(id, Name("echo-1".to_string())).unwrap();
    world.insert(id, Residue {}).unwrap();
    world.insert(id, Echo { amount: 7 }).unwrap();
}

/// The record a system spawned has no counterpart in the scene text and never will — respawning it is
/// the only way its state survives a visit, and a game that keeps a per-district census this way (a
/// cleared sector staying cleared) depends on it. It is not an `L3031`.
#[test]
fn a_code_spawned_named_record_respawns_on_return() {
    let dir = fresh_dir("code-spawned-name", DISTRICT_A);
    let mut engine = base_game(&dir)
        .system("drop_named_echo", drop_named_echo)
        .system("travel", travel)
        .build()
        .expect("boots");
    engine.load_inline_scene(DISTRICT_A).unwrap();
    round_trip(&mut engine);

    assert!(
        engine.last_transition_error().is_none(),
        "a record the scene never authored is not a mismatch: {:?}",
        engine.last_transition_error()
    );
    let echo = named_entity(engine.world(), "echo-1").expect("the census came back with the run");
    assert_eq!(
        engine.world().get::<Echo>(echo).unwrap(),
        Some(&Echo { amount: 7 }),
        "and it came back carrying the state it was captured with"
    );
}

/// The other half of the same distinction: a record captured off an entity the *scene* authored, whose
/// name the scene no longer carries. The scene authors that entity under its new name, so spawning the
/// record would duplicate it — the site draw would see one slot too many. It is dropped and reported.
/// The rename is applied to the file on disk while the run is away, the way a scene edit reaches a
/// live session.
#[test]
fn a_renamed_authored_record_reports_l3031_and_is_not_respawned() {
    let dir = fresh_dir("renamed-authored", DISTRICT_A);
    let mut engine = base_game(&dir)
        .system("travel", travel)
        .build()
        .expect("boots");
    engine.load_inline_scene(DISTRICT_A).unwrap();

    // Frame 1 opens the door, frame 2 leaves district A — its record is captured as authored.
    for _ in 0..3 {
        engine.advance_live();
    }
    let renamed = DISTRICT_A.replace("- name: door", "- name: hatch");
    assert!(
        !renamed.contains("name: door"),
        "the door really was renamed"
    );
    std::fs::write(dir.join("a.yaml"), &renamed).unwrap();

    // Frames 3, 4, then the frame-5 return, which instantiates the renamed district.
    for _ in 0..3 {
        engine.advance_live();
    }

    let (code, message) = transition_report(&engine);
    assert_eq!(code, "L3031", "{message}");
    assert!(
        message.contains("`door`"),
        "the report names the record: {message}"
    );
    assert!(
        named_entity(engine.world(), "door").is_none(),
        "the record is dropped for this visit, never spawned beside the entity it was renamed to"
    );
    assert!(
        !door_open(&engine, "hatch"),
        "the renamed entity stands as the scene authors it"
    );
}

/// The mirror of the capture-side `L3026`: a record addresses one entity, so a name two entities of
/// the arriving scene share makes which one it lands on an accident of entity order. The first still
/// wins — that is the behaviour games already read — but both ids are named.
#[test]
fn a_name_two_target_entities_share_reports_l3026_and_the_first_wins() {
    let twins = twin_doors();
    let dir = fresh_dir("twin-doors", &twins);
    let mut engine = base_game(&dir)
        .system("travel", travel)
        .build()
        .expect("boots");
    engine.load_inline_scene(&twins).unwrap();
    round_trip(&mut engine);

    let (code, message) = transition_report(&engine);
    assert_eq!(code, "L3026", "{message}");
    assert!(
        message.contains("matches 2 entities"),
        "the report says how many it could have chosen: {message}"
    );

    let doors: Vec<EntityId> = engine
        .world()
        .entities()
        .filter(|&id| matches!(engine.world().get::<Door>(id), Ok(Some(_))))
        .collect();
    assert_eq!(doors.len(), 2, "the scene authored both doors afresh");
    let open = |id: EntityId| engine.world().get::<Door>(id).unwrap().unwrap().open;
    assert!(
        open(doors[0]),
        "the record landed on the first in entity order"
    );
    assert!(
        !open(doors[1]),
        "the second is authored-fresh — it lost the state it kept, which is why this is reported"
    );
}

/// B2's case, at the layer that produces it: a snapshot carries residue across a rebuild, so a bag can
/// outlive the shape of its components. The bag is hand-broken in the snapshot document to stand for a
/// component whose fields moved between builds; the record is dropped whole and the district stays
/// reachable, where re-applying it verbatim used to reject the switch on every return, forever.
#[test]
fn a_bag_this_build_cannot_read_reports_l3030_and_the_record_is_dropped() {
    let dir = fresh_dir("stale-bag", DISTRICT_A);
    let mut original = base_game(&dir)
        .system("travel", travel)
        .build()
        .expect("boots");
    original.load_inline_scene(DISTRICT_A).unwrap();

    // Frame 1 opens the door, frame 2 leaves; the snapshot is taken standing in district B, where the
    // mutation exists only in the session's residue.
    for _ in 0..3 {
        original.advance_live();
    }
    let mut snapshot = snapshot_of(&mut original);
    let bag = snapshot["residue"][0]["records"][0]["bag"]
        .as_array_mut()
        .expect("the record carries a bag");
    let door = bag
        .iter_mut()
        .find(|entry| entry[0].as_str() == Some("Door"))
        .expect("the bag holds the game's own state component");
    door[1] = serde_json::json!({ "ajar": true });

    let mut restarted = base_game(&dir)
        .system("travel", travel)
        .build()
        .expect("boots");
    restore(&mut restarted, snapshot);
    // Frames 3, 4, then the frame-5 return that re-applies district A's residue.
    for _ in 0..3 {
        restarted.advance_live();
    }

    let (code, message) = transition_report(&restarted);
    assert_eq!(code, "L3030", "{message}");
    assert!(
        message.contains("`door`") && message.contains("`Door`"),
        "the report names the record and the component that failed: {message}"
    );
    assert!(
        named_entity(restarted.world(), "gate-from-b").is_some(),
        "the switch still happened — a stale record must not strand the run outside a district"
    );
    assert!(
        !door_open(&restarted, "door"),
        "the record was dropped whole, so the scene's authored door stands"
    );
}

/// The ordering contract `perform` states and vigil's `dedup_to_highest_id` depends on: instantiate
/// and `apply_residue` only ever spawn, so every carried entity holds an `EntityId` above every
/// authored and residue-spawned one. Nothing else says so — a despawn added inside instantiation would
/// silently hand a game the district's blank state instead of the run's.
#[test]
fn carried_entities_hold_the_highest_ids_after_a_switch() {
    let dir = fresh_dir("highest-ids", DISTRICT_A);
    let mut engine = base_game(&dir)
        .system("drop_echo", drop_echo)
        .system("remember_home", remember_home)
        .system("travel", travel)
        .build()
        .expect("boots");
    engine.load_inline_scene(DISTRICT_A).unwrap();
    round_trip(&mut engine);

    // District A authored four entities, its anonymous record respawned the echo, and the carried
    // player arrived on top — wearing the memo it wrote while away, which is what names it.
    let ids: Vec<EntityId> = engine.world().entities().collect();
    assert_eq!(
        ids.len(),
        6,
        "four authored, one residue-spawned echo, one carried player"
    );
    let carried = ids
        .iter()
        .copied()
        .find(|&id| matches!(engine.world().get::<Memo>(id), Ok(Some(_))))
        .expect("the carried player wrote a memo while standing in district B");
    for id in ids {
        if id == carried {
            continue;
        }
        assert!(
            id < carried,
            "{id} was minted after the carried entity — the carry no longer spawns last"
        );
    }
}
