//! In-window replay playback: `EngineBuilder::replay_playback` installs a `ReplayPlayback` resource; a
//! persisting host turns its own live engine into the replay of a recording and back.

use ludion_core::rng::DeterministicRng;
use ludion_core::time::FrameIndex;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    Engine, EngineBuilder, PlaybackOutcome, Profile, ReplayPlayback, ReplayPlaybackConfig,
    RunRecording, RunRecordingConfig, Shutdown, replay_hashes_with,
};
use ludion_protocol::{Command, command::InputInjectParams};
use ludion_sim::{InputState, Key, WorldHash};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};

type Trace = Vec<(FrameIndex, WorldHash)>;

/// A component whose value depends on the RNG and on input, so a wrong replay diverges.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct Tally {
    sum: u64,
    edges: u32,
    held: u32,
}

const HUB: &str = "format: 1\nname: hub\nseed: 5\nentities:\n  - name: t\n    components:\n      Tally:\n        sum: 0\n        edges: 0\n        held: 0\n";

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-playback-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("hub.yaml"), HUB).unwrap();
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

/// Asks the host to quit once the tally has seen a key edge.
fn quit_on_edge(world: &mut World, _ctx: &SystemCtx) {
    let edged = world.query::<(&Tally,)>().any(|(t,)| t.edges > 0);
    if edged && let Some(quit) = world.resource_mut::<Shutdown>() {
        quit.request();
    }
}

fn game(dir: &Path, persist: bool, version: Option<&str>) -> EngineBuilder {
    let mut builder = Engine::builder()
        .project_root(dir)
        .run_recordings(RunRecordingConfig::default())
        .replay_playback(ReplayPlaybackConfig::default())
        .components(|registry| {
            registry.register::<Tally>("Tally")?;
            Ok(())
        })
        .system("tally", tally);
    if persist {
        builder = builder.persist_profile();
    }
    if let Some(version) = version {
        builder = builder.game_version(version);
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

fn sample(engine: &mut Engine) -> (FrameIndex, WorldHash) {
    (engine.frame(), engine.world_hash().unwrap())
}

fn playback(engine: &mut Engine) -> &mut ReplayPlayback {
    engine.world_mut().resource_mut::<ReplayPlayback>().unwrap()
}

fn outcome(engine: &Engine) -> Option<PlaybackOutcome> {
    engine
        .world()
        .resource::<ReplayPlayback>()
        .unwrap()
        .last_outcome()
        .cloned()
}

/// Records a 20-tick run called `name` on a persisting engine of `version` in `dir`.
fn record(dir: &Path, name: &str, version: Option<&str>) {
    let mut engine = boot(game(dir, true, version));
    engine
        .world_mut()
        .resource_mut::<RunRecording>()
        .unwrap()
        .start(name);
    engine.advance_live();
    for tick in 1..=20u64 {
        match tick {
            4 => press(&mut engine, "key_down"),
            12 => press(&mut engine, "key_up"),
            _ => {}
        }
        engine.advance_live();
    }
    engine
        .world_mut()
        .resource_mut::<RunRecording>()
        .unwrap()
        .stop();
    engine.advance_live();
    assert!(engine.run_recording_path(name).is_file());
}

fn tally_sum(engine: &mut Engine) -> u64 {
    engine
        .world_mut()
        .query::<(&Tally,)>()
        .map(|(t,)| t.sum)
        .next()
        .unwrap()
}

#[test]
fn playback_reaches_the_replay_hashes_of_the_file() {
    let dir = fresh_dir("acceptance");
    record(&dir, "run", None);
    let bytes = std::fs::read(dir.join("save/replays/run.lrp")).unwrap();
    let expected: Trace = replay_hashes_with(game(&dir, false, None), &bytes, &dir).unwrap();

    let mut engine = boot(game(&dir, true, None));
    for _ in 0..5 {
        engine.advance_live();
    }
    // Held input in the live world must not leak into the replay.
    press(&mut engine, "key_down");
    playback(&mut engine).play("run", "hub.yaml");
    engine.advance_live();
    assert_eq!(engine.playing(), Some("run"));

    let mut played: Trace = vec![sample(&mut engine)];
    while engine.advance_live() {
        assert!(engine.playing().is_some());
        played.push(sample(&mut engine));
    }
    assert_eq!(played, expected);

    assert_eq!(engine.playing(), None);
    assert_eq!(
        outcome(&engine),
        Some(PlaybackOutcome::Finished {
            name: "run".into(),
            frames: expected.len() as u64 - 1
        })
    );
    assert_eq!(engine.frame(), FrameIndex::ZERO, "back at a fresh hub");
    assert_eq!(tally_sum(&mut engine), 0);
}

#[test]
fn a_game_mismatch_is_refused_and_never_starts() {
    let dir = fresh_dir("mismatch");
    record(&dir, "run", Some("1"));
    let mut engine = boot(game(&dir, true, Some("2")));
    for _ in 0..3 {
        engine.advance_live();
    }
    playback(&mut engine).play("run", "hub.yaml");
    engine.advance_live();
    let (frame, hash) = sample(&mut engine);
    assert_eq!(frame.as_u64(), 4, "the drain tick is the live world's own");
    assert_eq!(engine.playing(), None);
    match outcome(&engine) {
        Some(PlaybackOutcome::Refused { name, diagnostic }) => {
            assert_eq!((name.as_str(), diagnostic.code.as_str()), ("run", "L1404"));
        }
        other => panic!("{other:?}"),
    }
    engine.advance_live();
    assert_eq!(engine.frame().as_u64(), 5, "the live world ticks on");
    assert_ne!(hash, sample(&mut engine).1);
}

#[test]
fn a_missing_file_is_refused_with_l1405() {
    let dir = fresh_dir("missing");
    let mut engine = boot(game(&dir, true, None));
    playback(&mut engine).play("nope", "hub.yaml");
    engine.advance_live();
    match outcome(&engine) {
        Some(PlaybackOutcome::Refused { diagnostic, .. }) => {
            assert_eq!(diagnostic.code.as_str(), "L1405");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        engine.replay_playback_error().map(|d| d.code.as_str()),
        Some("L1405")
    );
}

#[derive(Serialize, Deserialize, JsonSchema, Default, Clone, Debug, PartialEq)]
struct Save {
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

fn probe_game(dir: &Path) -> EngineBuilder {
    Engine::builder()
        .project_root(dir)
        .profile::<Save>()
        .persist_profile()
        .run_recordings(RunRecordingConfig::default())
        .replay_playback(ReplayPlaybackConfig::default())
        .components(|registry| registry.register::<Probe>("Probe"))
        .system("project_pending", project_pending)
}

fn set_pending(engine: &mut Engine, value: u32) {
    engine
        .world_mut()
        .resource_mut::<Profile<Save>>()
        .unwrap()
        .get_mut()
        .pending_run = value;
}

fn pending(engine: &Engine) -> u32 {
    engine
        .world()
        .resource::<Profile<Save>>()
        .unwrap()
        .get()
        .pending_run
}

#[test]
fn stop_returns_to_the_named_scene_with_the_live_profile() {
    let dir = fresh_dir("stop");
    std::fs::write(dir.join("probe.yaml"), PROBE_HUB).unwrap();
    let mut engine = probe_game(&dir).build().unwrap();
    engine.load_inline_scene(PROBE_HUB).unwrap();
    engine.advance_live();
    set_pending(&mut engine, 42);
    engine
        .world_mut()
        .resource_mut::<RunRecording>()
        .unwrap()
        .start("p");
    engine.advance_live();
    for _ in 0..3 {
        engine.advance_live();
    }
    engine
        .world_mut()
        .resource_mut::<RunRecording>()
        .unwrap()
        .stop();
    engine.advance_live();

    set_pending(&mut engine, 99);
    playback(&mut engine).play("p", "probe.yaml");
    engine.advance_live();
    assert_eq!(engine.playing(), Some("p"));
    assert_eq!(
        pending(&engine),
        42,
        "the replay runs on the recorded profile"
    );
    for _ in 0..2 {
        assert!(engine.advance_live());
    }
    let at = engine.frame();
    engine.stop_playback();
    assert_eq!(engine.playing(), None);
    assert_eq!(
        outcome(&engine),
        Some(PlaybackOutcome::Stopped {
            name: "p".into(),
            frame: at
        })
    );
    assert_eq!(pending(&engine), 99, "the unsaved live write came back");
    assert_eq!(engine.frame(), FrameIndex::ZERO);
}

#[test]
fn requests_without_persistence_are_inert() {
    let dir = fresh_dir("inert");
    record(&dir, "run", None);
    let mut engine = boot(game(&dir, false, None));
    playback(&mut engine).play("run", "hub.yaml");
    playback(&mut engine).refresh();
    engine.advance_live();
    engine.advance_live();
    assert_eq!(engine.playing(), None);
    assert_eq!(outcome(&engine), None);
    assert!(
        engine
            .world()
            .resource::<ReplayPlayback>()
            .unwrap()
            .entries()
            .is_empty()
    );
}

#[test]
fn refresh_lists_and_delete_removes() {
    let dir = fresh_dir("list");
    record(&dir, "a", Some("1"));
    record(&dir, "b", Some("1"));
    record(&dir, "c", Some("2"));
    let mut engine = boot(game(&dir, true, Some("1")));
    playback(&mut engine).refresh();
    engine.advance_live();
    let entries = engine
        .world()
        .resource::<ReplayPlayback>()
        .unwrap()
        .entries()
        .to_vec();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["a", "b", "c"]);
    assert!(entries.iter().all(|e| e.bytes > 0));
    assert!(entries[0].refusal.is_none() && entries[1].refusal.is_none());
    assert_eq!(
        entries[2].refusal.as_ref().map(|d| d.code.as_str()),
        Some("L1404")
    );

    playback(&mut engine).delete("a");
    engine.advance_live();
    assert!(!engine.replay_path("a").exists());
    let count = engine
        .world()
        .resource::<ReplayPlayback>()
        .unwrap()
        .entries()
        .len();
    assert_eq!(count, 2);
}

#[test]
fn a_replayed_quit_does_not_shut_the_host() {
    let dir = fresh_dir("quit");
    let build = |persist| game(&dir, persist, None).system("quit_on_edge", quit_on_edge);
    let mut recorder = boot(build(true));
    recorder
        .world_mut()
        .resource_mut::<RunRecording>()
        .unwrap()
        .start("q");
    recorder.advance_live();
    for tick in 1..=8u64 {
        if tick == 3 {
            press(&mut recorder, "key_down");
        }
        recorder.advance_live();
    }
    recorder
        .world_mut()
        .resource_mut::<RunRecording>()
        .unwrap()
        .stop();
    recorder.advance_live();
    assert!(recorder.shutdown_requested(), "the live game does quit");

    let mut host = boot(build(true));
    playback(&mut host).play("q", "hub.yaml");
    host.advance_live();
    while host.advance_live() {}
    assert_eq!(host.playing(), None);
    assert!(!host.shutdown_requested(), "the replayed quit was dropped");
}
