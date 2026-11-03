//! Recording writes what a command actually applied (audit 2026-09-25 B2/B6).
//!
//! A failed `entity.spawn` mutates nothing, a `sim.step` stopped by a system panic records the ticks
//! that ran, and a free-run tick whose system panicked is batched like any other — so each session
//! replays to the live frame and hash instead of diverging or aborting on the recorded `L4007`. The
//! panicking tick carries a `system_panic` marker: a replay accepts `L4007` there and nowhere else,
//! and a live `drive_hashes` accepts it nowhere.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use ludion_core::time::FrameIndex;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Engine, EngineBuilder, drive_hashes, replay_hashes_with};
use ludion_protocol::command::{EntitySpawnParams, MAX_STEP_TICKS, SimStepParams};
use ludion_protocol::replay::{
    REPLAY_FORMAT_VERSION, ReplayHeader, ReplayReader, ReplayRecord, ReplayWriter, SceneRef,
};
use ludion_protocol::{Command, PROTOCOL_VERSION, ResponseBody, schema};
use ludion_sim::WorldHash;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// The frame the player is despawned on, and so the frame `damage_player` panics on: the 4th tick.
const DOOM: u64 = 3;

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug)]
struct Health {
    hp: i32,
}

const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: player
    components:
      Health:
        hp: 100
";

fn reap(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != DOOM {
        return;
    }
    let doomed: Vec<_> = world.query_ids::<(&Health,)>().map(|(id, _)| id).collect();
    for id in doomed {
        world.despawn(id).unwrap();
    }
}

fn damage_player(world: &mut World, _ctx: &SystemCtx) {
    let health = world
        .query::<(&mut Health,)>()
        .next()
        .expect("the player's Health")
        .0;
    health.hp -= 1;
}

fn game() -> EngineBuilder {
    EngineBuilder::new()
        .components(|registry| registry.register::<Health>("Health"))
        .system("reap", reap)
        .system("damage_player", damage_player)
}

/// The same components as [`game`], without `reap`: the player lives and nothing ever panics.
fn calm() -> EngineBuilder {
    EngineBuilder::new()
        .components(|registry| registry.register::<Health>("Health"))
        .system("damage_player", damage_player)
}

fn engine() -> Engine {
    let mut engine = game().build().expect("the test engine builds");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    engine
}

fn calm_engine() -> Engine {
    let mut engine = calm().build().expect("the calm engine builds");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    engine
}

/// A recording sink shared with the test so the recorded bytes can be read back.
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

fn step(ticks: u64) -> Command {
    Command::SimStep(SimStepParams { ticks, hash: false })
}

fn spawn(components: &[(&str, serde_json::Value)]) -> Command {
    Command::EntitySpawn(EntitySpawnParams {
        name: None,
        components: components
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone()))
            .collect::<BTreeMap<_, _>>(),
        presentation: false,
    })
}

/// The recording holds exactly one `system_panic` marker, on the live panic frame, right after a
/// `sim.step` record — read straight from the container, not through a replay.
fn assert_marker_after_step(sink: &SharedSink) {
    let bytes = sink.bytes();
    let records = ReplayReader::new(&bytes).unwrap().records().unwrap();
    let markers: Vec<usize> = (0..records.len())
        .filter(|&i| matches!(records[i], ReplayRecord::SystemPanic { .. }))
        .collect();
    assert_eq!(markers.len(), 1, "exactly one marker");
    assert_eq!(
        records[markers[0]],
        ReplayRecord::SystemPanic {
            frame: FrameIndex::new(DOOM)
        }
    );
    assert!(
        matches!(
            records[markers[0] - 1],
            ReplayRecord::Command {
                command: Command::SimStep(_),
                ..
            }
        ),
        "the marker follows a sim.step record"
    );
}

fn header() -> ReplayHeader {
    ReplayHeader {
        format_version: REPLAY_FORMAT_VERSION,
        engine_version: "0.0.0".into(),
        protocol_version: PROTOCOL_VERSION,
        schema_hash: schema::replay_schema_hash(),
        scene: SceneRef::Inline {
            document: SCENE.into(),
        },
        seed: 1,
        profile: None,
        render_extent: None,
        replay_content_hash: Some(ludion_engine::replay_content_hash()),
    }
}

/// The live engine's final `(frame, hash)` against the replay's last trace entry.
fn assert_replays_to_live(engine: &mut Engine, sink: &SharedSink) {
    let replayed = replay_hashes_with(game(), &sink.bytes(), std::env::temp_dir())
        .expect("the recording replays without error");
    let live: (FrameIndex, WorldHash) = (engine.frame(), engine.world_hash().unwrap());
    assert_eq!(
        replayed.last(),
        Some(&live),
        "replay ends where the live run did"
    );
}

#[test]
fn a_failed_spawn_while_recording_replays_bit_identically() {
    let sink = SharedSink::default();
    let mut engine = engine();
    engine.set_recording(sink.clone()).unwrap();
    engine.execute(step(1)).unwrap();

    let unknown = engine
        .execute(spawn(&[("Nope", json!({}))]))
        .expect_err("an unknown component");
    assert_eq!(unknown.code.as_str(), "L0004");
    let bad = engine
        .execute(spawn(&[("Health", json!({ "hp": "lots" }))]))
        .expect_err("a value that does not match the schema");
    assert_eq!(bad.code.as_str(), "L1014");

    let ResponseBody::Spawned { entity } = engine
        .execute(spawn(&[("Health", json!({ "hp": 5 }))]))
        .unwrap()
        .body
    else {
        panic!("entity.spawn answers Spawned");
    };
    assert_eq!(
        entity.generation(),
        0,
        "the failed spawns never touched the allocator"
    );

    engine.execute(step(1)).unwrap();
    engine.stop_recording().unwrap();
    assert_replays_to_live(&mut engine, &sink);
}

#[test]
fn a_presentation_spawn_while_recording_keeps_its_flag_and_replays_bit_identically() {
    let sink = SharedSink::default();
    let mut engine = engine();
    engine.set_recording(sink.clone()).unwrap();
    engine.execute(step(1)).unwrap();
    let Command::EntitySpawn(params) = spawn(&[("Health", json!({ "hp": 5 }))]) else {
        unreachable!("spawn builds an entity.spawn");
    };
    engine
        .execute(Command::EntitySpawn(EntitySpawnParams {
            presentation: true,
            ..params.clone()
        }))
        .unwrap();
    engine.execute(Command::EntitySpawn(params)).unwrap();
    engine.execute(step(1)).unwrap();
    engine.stop_recording().unwrap();

    let bytes = sink.bytes();
    let spawns: Vec<serde_json::Value> = ReplayReader::new(&bytes)
        .unwrap()
        .records()
        .unwrap()
        .into_iter()
        .filter_map(|record| match record {
            ReplayRecord::Command {
                command: command @ Command::EntitySpawn(_),
                ..
            } => Some(serde_json::to_value(command).unwrap()),
            _ => None,
        })
        .collect();
    assert_eq!(spawns.len(), 2);
    assert_eq!(spawns[0]["params"]["presentation"], json!(true));
    assert!(
        spawns[1]["params"].get("presentation").is_none(),
        "a sim spawn records no flag: {}",
        spawns[1]
    );
    assert_replays_to_live(&mut engine, &sink);
}

#[test]
fn an_over_long_step_is_refused_unrecorded_and_the_frame_stays() {
    let sink = SharedSink::default();
    let mut engine = calm_engine();
    engine.set_recording(sink.clone()).unwrap();
    engine.execute(step(1)).unwrap();
    let before = engine.frame();

    let refused = engine
        .execute(step(MAX_STEP_TICKS + 1))
        .expect_err("one tick past the limit");
    assert_eq!(refused.code.as_str(), "L3057");
    assert_eq!(refused.path.as_deref(), Some("/params/ticks"));
    assert_eq!(engine.frame(), before, "a refused step runs nothing");

    engine.execute(step(0)).unwrap();
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();
    let steps = ReplayReader::new(&bytes)
        .unwrap()
        .records()
        .unwrap()
        .iter()
        .filter(|record| matches!(record, ReplayRecord::Command { .. }))
        .count();
    assert_eq!(steps, 2, "only the accepted steps are recorded");
}

#[test]
fn a_step_stopped_by_a_system_panic_replays_to_the_same_frame() {
    let sink = SharedSink::default();
    let mut engine = engine();
    engine.set_recording(sink.clone()).unwrap();

    let error = engine
        .execute(step(10))
        .expect_err("the step is stopped by the panic");
    assert_eq!(error.code.as_str(), "L4007");
    assert_eq!(engine.frame().as_u64(), DOOM + 1, "the panicking tick ran");

    engine.stop_recording().unwrap();
    assert_replays_to_live(&mut engine, &sink);
    assert_marker_after_step(&sink);
}

#[test]
fn a_lockstep_single_tick_panic_is_marked_and_replays() {
    let sink = SharedSink::default();
    let mut engine = engine();
    engine.set_recording(sink.clone()).unwrap();
    for _ in 0..DOOM {
        engine.execute(step(1)).unwrap();
    }
    let error = engine.execute(step(1)).expect_err("the panicking tick");
    assert_eq!(error.code.as_str(), "L4007");

    engine.stop_recording().unwrap();
    assert_replays_to_live(&mut engine, &sink);
    assert_marker_after_step(&sink);
}

#[test]
fn drive_hashes_propagates_a_panic_on_a_steps_last_tick() {
    let error = drive_hashes(&mut engine(), vec![step(DOOM + 1)]).expect_err("the panic");
    assert_eq!(error.code.as_str(), "L4007");
}

#[test]
fn drive_hashes_propagates_a_panic_in_lockstep_single_ticks() {
    let commands = (0..=DOOM).map(|_| step(1)).collect();
    let error = drive_hashes(&mut engine(), commands).expect_err("the panic");
    assert_eq!(error.code.as_str(), "L4007");
}

#[test]
fn an_unrecorded_panic_fails_the_replay() {
    // Recorded on a build that never panics, replayed on one that does: no marker, so strict.
    let sink = SharedSink::default();
    let mut recorded = calm_engine();
    recorded.set_recording(sink.clone()).unwrap();
    for _ in 0..DOOM + 3 {
        recorded.execute(step(1)).unwrap();
    }
    recorded.stop_recording().unwrap();

    let error = replay_hashes_with(game(), &sink.bytes(), std::env::temp_dir())
        .expect_err("the unmarked panic fails the replay");
    assert_eq!(error.code.as_str(), "L4007");
}

#[test]
fn a_recorded_panic_that_no_longer_happens_is_l3050() {
    let sink = SharedSink::default();
    let mut recorded = engine();
    recorded.set_recording(sink.clone()).unwrap();
    recorded.execute(step(10)).expect_err("the panic");
    recorded.stop_recording().unwrap();

    let error = replay_hashes_with(calm(), &sink.bytes(), std::env::temp_dir())
        .expect_err("the marked tick no longer panics");
    assert_eq!(error.code.as_str(), "L3050");
}

#[test]
fn a_system_panic_record_that_follows_no_step_ending_on_its_frame_is_l3010() {
    for (step_ticks, marker_frame) in [(None, 0), (Some(2), 0), (Some(2), 2)] {
        let mut writer = ReplayWriter::new(Vec::new(), &header()).unwrap();
        let command = match step_ticks {
            Some(ticks) => step(ticks),
            None => Command::SimPause,
        };
        writer.write_command(FrameIndex::new(0), &command).unwrap();
        writer
            .write_system_panic(FrameIndex::new(marker_frame))
            .unwrap();
        let bytes = writer.into_inner().unwrap();

        let error = replay_hashes_with(game(), &bytes, std::env::temp_dir())
            .expect_err("a stray marker is refused");
        assert_eq!(
            error.code.as_str(),
            "L3010",
            "{step_ticks:?} {marker_frame}"
        );
    }
}

#[test]
fn a_free_run_panic_tick_is_recorded() {
    let sink = SharedSink::default();
    let mut engine = engine();
    engine.set_recording(sink.clone()).unwrap();
    engine.execute(Command::SimResume).unwrap();

    let mut ticks = 0;
    while engine.advance_live() {
        ticks += 1;
        assert!(ticks <= 100, "the panic must pause the sim");
    }
    assert_eq!(ticks, DOOM + 1, "the panicking tick counts");
    assert!(engine.take_system_panic().is_some(), "the panic is held");

    engine.stop_recording().unwrap();
    assert_replays_to_live(&mut engine, &sink);
    assert_marker_after_step(&sink);
    let bytes = sink.bytes();
    let records = ReplayReader::new(&bytes).unwrap().records().unwrap();
    assert!(
        matches!(records.last(), Some(ReplayRecord::SystemPanic { .. })),
        "no closing keyframe after a panic marker"
    );
}

/// The frame whose tick is the first to end on the 300-frame keyframe boundary.
const BOUNDARY: u64 = 299;

fn reap_at_boundary(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != BOUNDARY {
        return;
    }
    let doomed: Vec<_> = world.query_ids::<(&Health,)>().map(|(id, _)| id).collect();
    for id in doomed {
        world.despawn(id).unwrap();
    }
}

#[test]
fn a_lockstep_panic_on_the_keyframe_boundary_writes_no_keyframe() {
    let sink = SharedSink::default();
    let mut engine = EngineBuilder::new()
        .components(|registry| registry.register::<Health>("Health"))
        .system("reap", reap_at_boundary)
        .system("damage_player", damage_player)
        .build()
        .expect("the boundary engine builds");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    engine.set_recording(sink.clone()).unwrap();
    engine.execute(step(BOUNDARY)).unwrap();
    let error = engine.execute(step(1)).expect_err("the panicking tick");
    assert_eq!(error.code.as_str(), "L4007");
    assert_eq!(engine.frame().as_u64(), BOUNDARY + 1);

    engine.stop_recording().unwrap();
    let bytes = sink.bytes();
    let records = ReplayReader::new(&bytes).unwrap().records().unwrap();
    assert!(
        matches!(records.last(), Some(ReplayRecord::SystemPanic { .. })),
        "no keyframe follows a panic marker: {:?}",
        records.last()
    );
}

/// A spawn naming a dead `Parent`: refused by the wire's reference checks (`L0002`).
fn spawn_dead_parent() -> Command {
    spawn(&[("Parent", json!({ "index": 99, "generation": 0 }))])
}

#[test]
fn a_refused_reference_check_leaves_no_record() {
    let sink = SharedSink::default();
    let mut engine = engine();
    engine.set_recording(sink.clone()).unwrap();
    let error = engine
        .execute(spawn_dead_parent())
        .expect_err("a dead Parent is refused");
    assert_eq!(error.code.as_str(), "L0002");
    engine.stop_recording().unwrap();

    let bytes = sink.bytes();
    let records = ReplayReader::new(&bytes).unwrap().records().unwrap();
    assert!(
        !records
            .iter()
            .any(|record| matches!(record, ReplayRecord::Command { .. })),
        "a refused command is not recorded"
    );
}

#[test]
fn a_replayed_command_the_reference_checks_refuse_stops_the_replay_with_its_code() {
    let error = drive_hashes(&mut engine(), vec![step(1), spawn_dead_parent()])
        .expect_err("the refused command stops the drive");
    assert_eq!(error.code.as_str(), "L0002");
}
