//! Game identity in the replay header: a run recording carries the schema hash and version of the
//! game build that wrote it, and a replay under a different build is refused with `L1404`. Session
//! recordings carry none and replay as before.

use ludion_core::codes::code;
use ludion_core::time::FrameIndex;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Engine, EngineBuilder, RunRecording, RunRecordingConfig, replay_hashes_with};
use ludion_protocol::replay::ReplayReader;
use ludion_sim::WorldHash;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

type Trace = Vec<(FrameIndex, WorldHash)>;

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct Tally {
    sum: u64,
}

/// The same component name with one more defaulted field: a schema change that still decodes.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct TallyV2 {
    sum: u64,
    #[serde(default)]
    bonus: u32,
}

const HUB: &str = "format: 1\nname: hub\nseed: 5\nentities:\n  - name: t\n    components:\n      Tally:\n        sum: 0\n";

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-gameid-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn bump(world: &mut World, _ctx: &SystemCtx) {
    for (t,) in world.query::<(&mut Tally,)>() {
        t.sum += 1;
    }
}

fn base(dir: &Path, persist: bool) -> EngineBuilder {
    let builder = Engine::builder()
        .project_root(dir)
        .run_recordings(RunRecordingConfig {
            dir: "save/replays".to_string(),
            keyframe_every: 3600,
        });
    if persist {
        builder.persist_profile()
    } else {
        builder
    }
}

fn game(dir: &Path, persist: bool, version: Option<&str>) -> EngineBuilder {
    let mut builder = base(dir, persist)
        .components(|registry| {
            registry.register::<Tally>("Tally")?;
            Ok(())
        })
        .system("bump", bump);
    if let Some(version) = version {
        builder = builder.game_version(version);
    }
    builder
}

fn game_v2(dir: &Path) -> EngineBuilder {
    base(dir, false).components(|registry| {
        registry.register::<TallyV2>("Tally")?;
        Ok(())
    })
}

/// Records a 5-tick run under `builder`, returning the engine, the file bytes and the live trace.
fn record_run(builder: EngineBuilder) -> (Engine, Vec<u8>, Trace) {
    let mut engine = builder.build().expect("boots");
    engine.load_inline_scene(HUB).unwrap();
    engine
        .world_mut()
        .resource_mut::<RunRecording>()
        .unwrap()
        .start("run");
    engine.advance_live();
    let mut live = vec![(engine.frame(), engine.world_hash().unwrap())];
    for _ in 0..5 {
        engine.advance_live();
        live.push((engine.frame(), engine.world_hash().unwrap()));
    }
    engine
        .world_mut()
        .resource_mut::<RunRecording>()
        .unwrap()
        .stop();
    engine.advance_live();
    live.push((engine.frame(), engine.world_hash().unwrap()));
    let bytes = std::fs::read(engine.run_recording_path("run")).unwrap();
    (engine, bytes, live)
}

#[test]
fn a_changed_schema_refuses_and_the_same_build_replays() {
    let dir = fresh_dir("schema");
    let (_, bytes, live) = record_run(game(&dir, true, None));

    let err = replay_hashes_with(game_v2(&dir), &bytes, &dir).unwrap_err();
    assert_eq!(err.code, code::REPLAY_GAME_MISMATCH, "{}", err.message);

    let replayed = replay_hashes_with(game(&dir, false, None), &bytes, &dir).unwrap();
    assert_eq!(replayed, live);
}

#[test]
fn a_changed_version_refuses_naming_both_and_the_header_carries_the_identity() {
    let dir = fresh_dir("version");
    let (engine, bytes, _) = record_run(game(&dir, true, Some("1.0")));

    let header = ReplayReader::new(&bytes).unwrap().header().clone();
    assert_eq!(header.game, Some(engine.game_identity()));

    let err = replay_hashes_with(game(&dir, false, Some("1.1")), &bytes, &dir).unwrap_err();
    assert_eq!(err.code, code::REPLAY_GAME_MISMATCH);
    assert!(
        err.message.contains("1.0") && err.message.contains("1.1"),
        "{}",
        err.message
    );
}

#[derive(Clone, Default)]
struct SharedSink(Arc<Mutex<Vec<u8>>>);

impl Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_session_recording_carries_no_identity_and_replays_under_a_changed_schema() {
    let dir = fresh_dir("session");
    let mut engine = game(&dir, false, Some("1.0")).build().expect("boots");
    engine.load_inline_scene(HUB).unwrap();
    let sink = SharedSink::default();
    engine.set_recording(sink.clone()).unwrap();
    for _ in 0..5 {
        engine.advance_live();
    }
    engine.stop_recording().unwrap();
    let bytes = sink.0.lock().unwrap().clone();

    assert_eq!(ReplayReader::new(&bytes).unwrap().header().game, None);
    let replayed = replay_hashes_with(game_v2(&dir), &bytes, &dir);
    assert!(replayed.is_ok(), "{:?}", replayed.err().map(|d| d.message));
}
