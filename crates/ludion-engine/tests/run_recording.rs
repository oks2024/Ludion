//! In-game run recording: `EngineBuilder::run_recordings` installs a `RunRecording` resource; a
//! persisting host restarts the run at the next free-running boundary and records it to an ordinary
//! `.lrp` that replays to the live trace.

use ludion_core::rng::DeterministicRng;
use ludion_core::time::FrameIndex;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    Engine, EngineBuilder, Profile, RunRecording, RunRecordingConfig, RunRecordingRequest,
    replay_hashes_with,
};
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use ludion_protocol::{Command, command::InputInjectParams};
use ludion_sim::{InputState, Key, WorldHash};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

type Trace = Vec<(FrameIndex, WorldHash)>;

/// A component whose value depends on the RNG and on input, so a wrong restart diverges.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct Tally {
    sum: u64,
    edges: u32,
    held: u32,
}

const HUB: &str = "format: 1\nname: hub\nseed: 5\nentities:\n  - name: t\n    components:\n      Tally:\n        sum: 0\n        edges: 0\n        held: 0\n";

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-runrec-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn tally(world: &mut World, _ctx: &SystemCtx) {
    let roll = world
        .resource_mut::<DeterministicRng>()
        .map(DeterministicRng::next_u32)
        .unwrap_or(0);
    let (held, edge) = {
        let input = world.resource::<InputState>().unwrap();
        (input.is_held(Key::W), input.was_pressed(Key::W))
    };
    for (t,) in world.query::<(&mut Tally,)>() {
        t.sum = t.sum.wrapping_add(u64::from(roll));
        t.edges += u32::from(edge);
        t.held += u32::from(held);
    }
}

fn config(keyframe_every: u64) -> RunRecordingConfig {
    RunRecordingConfig {
        dir: "save/replays".to_string(),
        keyframe_every,
    }
}

fn game(dir: &Path, persist: bool, keyframe_every: u64) -> EngineBuilder {
    let mut builder = Engine::builder()
        .project_root(dir)
        .run_recordings(config(keyframe_every))
        .components(|registry| {
            registry.register::<Tally>("Tally")?;
            Ok(())
        })
        .system("tally", tally);
    if persist {
        builder = builder.persist_profile();
    }
    builder
}

fn boot(builder: EngineBuilder) -> Engine {
    let mut engine = builder.build().expect("boots");
    engine.load_inline_scene(HUB).unwrap();
    engine
}

fn press(engine: &mut Engine, kind: &str) {
    engine
        .execute(Command::InputInject(InputInjectParams {
            frame: None,
            event: json!({ "kind": kind, "key": "W" }),
        }))
        .unwrap();
}

fn recording(engine: &mut Engine) -> &mut RunRecording {
    engine.world_mut().resource_mut::<RunRecording>().unwrap()
}

fn start(engine: &mut Engine, name: &str) {
    recording(engine).start(name);
}

fn sample(engine: &mut Engine) -> (FrameIndex, WorldHash) {
    (engine.frame(), engine.world_hash().unwrap())
}

fn error_code(engine: &Engine) -> Option<String> {
    engine
        .run_recording_error()
        .map(|d| d.code.as_str().to_string())
}

fn replays_dir(dir: &Path) -> PathBuf {
    dir.join("save/replays")
}

fn leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(replays_dir(dir))
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|name| !name.ends_with(".lrp"))
                .collect()
        })
        .unwrap_or_default()
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
fn a_run_started_mid_session_replays_to_its_live_trace() {
    let dir = fresh_dir("acceptance");
    let mut engine = boot(game(&dir, true, 3600));
    press(&mut engine, "key_down");
    let mut earlier = Vec::new();
    for _ in 0..7 {
        engine.advance_live();
        earlier.push(sample(&mut engine));
    }
    assert_eq!(engine.frame().as_u64(), 7);

    start(&mut engine, "first");
    engine.advance_live();
    assert_eq!(engine.frame(), FrameIndex::ZERO, "the run restarted at 0");
    assert_eq!(
        engine.world().resource::<RunRecording>().unwrap().active(),
        Some("first")
    );
    let mut live: Trace = vec![sample(&mut engine)];
    for tick in 1..=20u64 {
        match tick {
            4 => press(&mut engine, "key_down"),
            12 => press(&mut engine, "key_up"),
            _ => {}
        }
        engine.advance_live();
        live.push(sample(&mut engine));
    }
    recording(&mut engine).stop();
    engine.advance_live();
    live.push(sample(&mut engine));

    let path = engine.run_recording_path("first");
    assert!(path.is_file(), "{path:?}");
    assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
    assert_eq!(error_code(&engine), None);
    let recording = engine.world().resource::<RunRecording>().unwrap();
    assert_eq!(
        (recording.active(), recording.last_finished()),
        (None, Some("first"))
    );

    let bytes = std::fs::read(&path).unwrap();
    let replayed = replay_hashes_with(game(&dir, false, 3600), &bytes, &dir).unwrap();
    assert_eq!(replayed, live);
    assert_ne!(
        live[3].1, earlier[2].1,
        "the run differs from the session at the same frame: the reset mattered"
    );
}

#[test]
fn keyframe_every_sets_the_interval_and_the_ends_stay() {
    let dir = fresh_dir("keyframes");
    let mut engine = boot(game(&dir, true, 4));
    start(&mut engine, "k");
    engine.advance_live();
    for _ in 0..9 {
        engine.advance_live();
    }
    recording(&mut engine).stop();
    engine.advance_live();
    assert_eq!(engine.frame().as_u64(), 10);

    let bytes = std::fs::read(engine.run_recording_path("k")).unwrap();
    let frames: Vec<u64> = ReplayReader::new(&bytes)
        .unwrap()
        .records()
        .unwrap()
        .into_iter()
        .filter_map(|record| match record {
            ReplayRecord::Keyframe { frame, .. } => Some(frame.as_u64()),
            _ => None,
        })
        .collect();
    assert_eq!(frames, vec![0, 4, 8, 10]);
}

#[test]
fn without_persistence_a_request_is_only_logged() {
    let dir = fresh_dir("no-persist");
    let mut engine = boot(game(&dir, false, 3600));
    for _ in 0..3 {
        engine.advance_live();
    }
    start(&mut engine, "x");
    engine.advance_live();
    assert_eq!(engine.frame().as_u64(), 4, "no reset");
    recording(&mut engine).stop();
    engine.advance_live();
    assert_eq!(engine.frame().as_u64(), 5);

    let log = engine.run_recording_log().unwrap();
    let requests: Vec<_> = log
        .iter()
        .map(|e| (e.frame.as_u64(), e.request.clone()))
        .collect();
    assert_eq!(
        requests,
        vec![
            (3, RunRecordingRequest::Start("x".to_string())),
            (4, RunRecordingRequest::Stop)
        ]
    );
    assert!(!dir.join("save").exists(), "no IO without persistence");
    assert!(!engine.is_recording());
}

#[test]
fn a_session_recording_refuses_a_run_start() {
    let dir = fresh_dir("session");
    let sink = SharedSink::default();
    let mut engine = boot(game(&dir, true, 3600));
    engine.set_recording(sink.clone()).unwrap();
    let mut live: Trace = vec![sample(&mut engine)];
    for _ in 0..3 {
        engine.advance_live();
        live.push(sample(&mut engine));
    }
    start(&mut engine, "nope");
    engine.advance_live();
    live.push(sample(&mut engine));
    assert_eq!(engine.frame().as_u64(), 4, "no reset");
    assert_eq!(error_code(&engine).as_deref(), Some("L1402"));
    assert!(engine.is_recording());
    assert!(!replays_dir(&dir).exists());
    for _ in 0..2 {
        engine.advance_live();
        live.push(sample(&mut engine));
    }
    engine.stop_recording().unwrap();

    let bytes = sink.0.lock().unwrap().clone();
    let replayed = replay_hashes_with(game(&dir, false, 3600), &bytes, &dir).unwrap();
    assert_eq!(replayed, live);
}

#[test]
fn a_bad_name_is_refused_without_a_reset() {
    let dir = fresh_dir("bad-name");
    let mut engine = boot(game(&dir, true, 3600));
    for _ in 0..3 {
        engine.advance_live();
    }
    start(&mut engine, "a/b");
    engine.advance_live();
    assert_eq!(engine.frame().as_u64(), 4);
    assert_eq!(error_code(&engine).as_deref(), Some("L1402"));
    assert!(
        engine
            .world()
            .resource::<RunRecording>()
            .unwrap()
            .active()
            .is_none()
    );
    assert!(!replays_dir(&dir).exists());
}

#[test]
fn starting_while_active_finishes_the_previous_run() {
    let dir = fresh_dir("restart");
    let mut engine = boot(game(&dir, true, 3600));
    start(&mut engine, "one");
    engine.advance_live();
    for _ in 0..3 {
        engine.advance_live();
    }
    start(&mut engine, "two");
    engine.advance_live();
    assert_eq!(engine.frame(), FrameIndex::ZERO);
    assert!(engine.run_recording_path("one").is_file());
    let recording = engine.world().resource::<RunRecording>().unwrap();
    assert_eq!(
        (recording.active(), recording.last_finished()),
        (Some("two"), Some("one"))
    );
    engine.advance_live();
    recording_stop_and_tick(&mut engine);
    assert!(engine.run_recording_path("two").is_file());
    assert_eq!(error_code(&engine), None);
}

fn recording_stop_and_tick(engine: &mut Engine) {
    recording(engine).stop();
    engine.advance_live();
}

#[test]
fn a_zero_interval_is_refused_at_build() {
    let error = game(&fresh_dir("zero"), false, 0)
        .build()
        .err()
        .expect("refused");
    assert_eq!(error.code.as_str(), "L1403");
}

#[derive(Serialize, Deserialize, JsonSchema, Default, Clone, Debug, PartialEq)]
struct Save {
    runs: u32,
    pending_run: u32,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Default)]
struct Probe {
    pending: u32,
}

fn project_pending(world: &mut World, _ctx: &SystemCtx) {
    let pending = world
        .resource::<Profile<Save>>()
        .map_or(0, |p| p.get().pending_run);
    for (probe,) in world.query::<(&mut Probe,)>() {
        probe.pending = pending;
    }
}

const PROBE_HUB: &str = "format: 1\nname: probe\nentities:\n  - name: p\n    components:\n      Probe:\n        pending: 0\n";

fn probe_game(dir: &Path, persist: bool) -> EngineBuilder {
    let mut builder = Engine::builder()
        .project_root(dir)
        .profile::<Save>()
        .run_recordings(config(3600))
        .components(|registry| registry.register::<Probe>("Probe"))
        .system("project_pending", project_pending);
    if persist {
        builder = builder.persist_profile();
    }
    builder
}

#[test]
fn the_run_starts_from_the_live_profile_with_unsaved_writes() {
    let dir = fresh_dir("profile");
    std::fs::create_dir_all(dir.join("save")).unwrap();
    std::fs::write(
        dir.join("save/profile.yaml"),
        "format: 1\nkind: profile\nruns: 1\npending_run: 7\n",
    )
    .unwrap();
    let mut engine = probe_game(&dir, true).build().unwrap();
    engine.load_inline_scene(PROBE_HUB).unwrap();
    engine.advance_live();
    // Unsaved: nothing calls `save()`.
    engine
        .world_mut()
        .resource_mut::<Profile<Save>>()
        .unwrap()
        .get_mut()
        .pending_run = 42;

    start(&mut engine, "p");
    engine.advance_live();
    assert_eq!(engine.frame(), FrameIndex::ZERO);
    assert_eq!(
        engine
            .world()
            .resource::<Profile<Save>>()
            .unwrap()
            .get()
            .pending_run,
        42
    );
    let mut live: Trace = vec![sample(&mut engine)];
    for _ in 0..3 {
        engine.advance_live();
        live.push(sample(&mut engine));
    }
    recording_stop_and_tick(&mut engine);
    live.push(sample(&mut engine));

    let bytes = std::fs::read(engine.run_recording_path("p")).unwrap();
    let header = ReplayReader::new(&bytes)
        .unwrap()
        .header()
        .profile
        .clone()
        .unwrap();
    assert!(header.contains("pending_run: 42"), "{header}");
    let replayed = replay_hashes_with(probe_game(&dir, false), &bytes, &dir).unwrap();
    assert_eq!(replayed, live);
}
