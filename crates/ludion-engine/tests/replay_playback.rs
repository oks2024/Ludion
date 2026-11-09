//! In-window replay playback: `EngineBuilder::replay_playback` installs a `ReplayPlayback` resource; a
//! persisting host turns its own live engine into the replay of a recording and back.

use ludion_core::rng::DeterministicRng;
use ludion_core::time::FrameIndex;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    Engine, EngineBuilder, PlaybackInput, PlaybackOutcome, PlaybackSpeed, Profile, ReplayPlayback,
    ReplayPlaybackConfig, RunRecording, RunRecordingConfig, Shutdown, replay_hashes_with,
};
use ludion_protocol::{
    Command, ResponseBody,
    command::{InputInjectParams, RenderSizeParams},
};
use ludion_sim::{InputState, Key, WorldHash};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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

/// A presentation system that counts the ticks the live W key was held.
fn probed(dir: &Path) -> (EngineBuilder, Arc<AtomicUsize>) {
    let seen = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&seen);
    let builder = game(dir, true, None).presentation_system("probe", move |world, _ctx| {
        if let Some(input) = world.resource::<PlaybackInput>()
            && input.input().is_held(Key::W)
        {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    });
    (builder, seen)
}

fn live_error(engine: &mut Engine, command: Command) -> String {
    engine
        .execute(command)
        .expect_err("refused while a replay plays")
        .code
        .as_str()
        .to_string()
}

fn start(engine: &mut Engine, name: &str) {
    playback(engine).play(name, "hub.yaml");
    engine.advance_live();
    assert_eq!(engine.playing(), Some(name));
}

fn expected_trace(dir: &Path, name: &str) -> Trace {
    let bytes = std::fs::read(dir.join(format!("save/replays/{name}.lrp"))).unwrap();
    replay_hashes_with(game(dir, false, None), &bytes, dir).unwrap()
}

#[test]
fn live_input_and_mutations_never_reach_the_replay() {
    let dir = fresh_dir("flood");
    record(&dir, "run", None);
    let expected = expected_trace(&dir, "run");

    let mut engine = boot(probed(&dir).0);
    start(&mut engine, "run");
    let mut played: Trace = vec![sample(&mut engine)];
    loop {
        press(&mut engine, "key_down");
        press(&mut engine, "key_up");
        press(&mut engine, "key_down");
        assert_eq!(live_error(&mut engine, Command::SimPause), "L1405");
        assert_eq!(live_error(&mut engine, Command::SimResume), "L1405");
        for command in [
            Command::SceneSave(ludion_protocol::command::SceneSaveParams {
                path: "saved.yaml".to_string(),
                force: true,
            }),
            Command::PrefabCreate(ludion_protocol::command::PrefabCreateParams {
                entities: Vec::new(),
                path: "made.prefab.yaml".to_string(),
            }),
            Command::PrefabApply(ludion_protocol::command::PrefabApplyParams {
                instance: ludion_core::ids::EntityId::new(1, 0),
                parts: None,
            }),
        ] {
            assert_eq!(live_error(&mut engine, command), "L1405");
        }
        let reply = engine
            .execute(Command::RenderSize(RenderSizeParams {
                width: Some(640),
                height: Some(360),
            }))
            .unwrap();
        assert_eq!(
            reply.body,
            ResponseBody::RenderSize {
                width: 640,
                height: 360
            }
        );
        if !engine.advance_live() {
            break;
        }
        played.push(sample(&mut engine));
    }
    assert_eq!(played, expected);
    assert_eq!(engine.render_size(), (640, 360), "applied at the return");
    assert!(!dir.join("saved.yaml").exists() && !dir.join("made.prefab.yaml").exists());
}

#[test]
fn presentation_systems_see_live_input() {
    let dir = fresh_dir("presentation");
    record(&dir, "run", None);
    let (builder, seen) = probed(&dir);
    let mut engine = boot(builder);
    start(&mut engine, "run");
    press(&mut engine, "key_down");
    assert!(engine.advance_live());
    assert_eq!(seen.load(Ordering::Relaxed), 1);
    assert!(engine.advance_live());
    assert_eq!(seen.load(Ordering::Relaxed), 2, "held until released");
    while engine.advance_live() {}
    assert!(engine.world().resource::<PlaybackInput>().is_none());
}

#[test]
fn pause_holds_and_resume_continues_the_trace() {
    let dir = fresh_dir("pause");
    record(&dir, "run", None);
    let expected = expected_trace(&dir, "run");

    let mut engine = boot(game(&dir, true, None));
    start(&mut engine, "run");
    let mut played: Trace = vec![sample(&mut engine)];
    for _ in 0..3 {
        assert!(engine.advance_live());
        played.push(sample(&mut engine));
    }
    engine.pause_playback();
    assert!(engine.playback_paused());
    let held = sample(&mut engine);
    for _ in 0..5 {
        assert!(!engine.advance_live());
        assert_eq!(sample(&mut engine), held);
        assert_eq!(engine.playing(), Some("run"));
    }
    engine.resume_playback();
    while engine.advance_live() {
        played.push(sample(&mut engine));
    }
    assert_eq!(played, expected);
}

#[test]
fn the_hook_drives_the_controls() {
    let dir = fresh_dir("hook");
    record(&dir, "run", None);
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let builder = game(&dir, true, None).playback_controls(move |input, controls| {
        counter.fetch_add(1, Ordering::Relaxed);
        if input.was_pressed(Key::Space) {
            if controls.paused() {
                controls.resume();
            } else {
                controls.pause();
            }
        }
        if input.was_pressed(Key::Escape) {
            controls.stop();
        }
    });
    let mut engine = boot(builder);
    start(&mut engine, "run");
    let key = |engine: &mut Engine, key: &str| {
        engine
            .execute(Command::InputInject(InputInjectParams {
                frame: None,
                event: json!({ "kind": "key_down", "key": key }),
            }))
            .unwrap();
    };
    key(&mut engine, "Space");
    assert!(!engine.advance_live());
    assert!(engine.playback_paused());
    let at = engine.frame();
    let before = calls.load(Ordering::Relaxed);
    for _ in 0..3 {
        assert!(!engine.advance_live());
    }
    assert_eq!(
        calls.load(Ordering::Relaxed),
        before + 3,
        "hook runs paused"
    );
    assert_eq!(engine.frame(), at);

    engine
        .execute(Command::InputInject(InputInjectParams {
            frame: None,
            event: json!({ "kind": "key_up", "key": "Space" }),
        }))
        .unwrap();
    assert!(!engine.advance_live());
    key(&mut engine, "Space");
    assert!(engine.advance_live());
    assert!(!engine.playback_paused());

    key(&mut engine, "Escape");
    assert!(!engine.advance_live());
    assert_eq!(engine.playing(), None);
    assert!(matches!(
        outcome(&engine),
        Some(PlaybackOutcome::Stopped { .. })
    ));
    assert_eq!(engine.frame(), FrameIndex::ZERO, "back at the hub");
}

#[test]
fn speed_sets_the_driver_pacing_and_returns() {
    let dir = fresh_dir("speed");
    record(&dir, "run", None);
    let mut engine = boot(game(&dir, true, None));
    start(&mut engine, "run");
    assert_eq!(engine.playback_speed(), Some(PlaybackSpeed::X1));
    engine.set_playback_speed(PlaybackSpeed::X4);
    assert_eq!(engine.speed().factor(), 4.0);
    assert!(engine.advance_live());
    assert_eq!(engine.speed().factor(), 4.0, "a tick keeps the pace");
    engine.stop_playback();
    assert_eq!(engine.speed().factor(), 1.0, "the saved speed is back");
    assert_eq!(engine.playback_speed(), None);
}

#[test]
fn run_live_keeps_ticking_a_playback() {
    use ludion_engine::live::{StampedCommand, run_live, shutdown_channel};
    use std::time::Duration;

    let dir = fresh_dir("run-live");
    record(&dir, "run", None);
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let mut engine = boot(
        game(&dir, true, None).playback_controls(move |_, controls| {
            let n = counter.fetch_add(1, Ordering::Relaxed);
            if n == 5 {
                controls.pause();
            }
        }),
    );
    playback(&mut engine).play("run", "hub.yaml");

    let (intake, intake_rx) = crossbeam_channel::unbounded();
    let (shutdown, shutdown_rx) = shutdown_channel();
    let handle = std::thread::spawn(move || run_live(engine, intake_rx, shutdown_rx, None));
    let send = |command: Command| {
        let (stamped, reply) = StampedCommand::new(command);
        intake.send(stamped).unwrap();
        reply.recv_timeout(Duration::from_secs(5)).unwrap()
    };

    let deadline = crossbeam_channel::after(Duration::from_secs(10));
    while calls.load(Ordering::Relaxed) < 40 {
        assert!(
            deadline.try_recv().is_err(),
            "the hook stopped being called"
        );
        let ack = send(Command::InputInject(InputInjectParams {
            frame: None,
            event: json!({ "kind": "key_down", "key": "W" }),
        }));
        assert_eq!(ack.unwrap().body, ResponseBody::Ack);
        let refused = send(Command::SimPause).expect_err("refused while it plays");
        assert_eq!(refused.code.as_str(), "L1405");
        std::thread::sleep(Duration::from_millis(5));
    }
    shutdown.signal();
    handle.join().unwrap().unwrap();
}

static PLAY_AND_SWITCH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn play_and_switch(world: &mut World, _ctx: &SystemCtx) {
    if PLAY_AND_SWITCH.swap(false, Ordering::SeqCst) {
        world
            .resource_mut::<ReplayPlayback>()
            .unwrap()
            .play("run", "hub.yaml");
        if let Some(switch) = world.resource_mut::<ludion_engine::SceneSwitch>() {
            switch.request("next.yaml".to_string(), "gate".to_string(), Vec::new());
        }
    }
}

#[test]
fn a_play_in_a_switch_tick_still_plays() {
    let dir = fresh_dir("play-switch");
    std::fs::write(
        dir.join("next.yaml"),
        "format: 1\nname: next\nentities:\n  - name: gate\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - 0.0\n          - 0.0\n",
    )
    .unwrap();
    record(&dir, "run", None);
    let mut engine = boot(game(&dir, true, None).system("play_and_switch", play_and_switch));
    for _ in 0..3 {
        engine.advance_live();
    }
    PLAY_AND_SWITCH.store(true, Ordering::SeqCst);
    engine.advance_live();
    assert!(engine.last_transition_error().is_none());
    assert_eq!(engine.playing(), Some("run"));
    assert_eq!(outcome(&engine), None);
}

/// Branches the sim on disk state a replay never sees.
fn disk_branch(world: &mut World, _ctx: &SystemCtx) {
    let suspended = world
        .resource::<ludion_engine::RunSave>()
        .is_some_and(|run| run.pending().is_some());
    let listed = world
        .resource::<ReplayPlayback>()
        .is_some_and(|playback| !playback.entries().is_empty());
    let finished = world
        .resource::<RunRecording>()
        .is_some_and(|recording| recording.last_finished().is_some());
    for (t,) in world.query::<(&mut Tally,)>() {
        t.held += u32::from(suspended) + 2 * u32::from(listed) + 4 * u32::from(finished);
    }
}

#[test]
fn playback_sees_the_host_state_a_replay_verify_sees() {
    let dir = fresh_dir("host-state");
    record(&dir, "run", None);
    let bytes = std::fs::read(dir.join("save/replays/run.lrp")).unwrap();
    let build = |persist| {
        game(&dir, persist, None)
            .run_save()
            .system("disk", disk_branch)
    };
    let expected: Trace = replay_hashes_with(build(false), &bytes, &dir).unwrap();

    let mut engine = boot(build(true));
    engine
        .world_mut()
        .resource_mut::<ludion_engine::RunSave>()
        .unwrap()
        .suspend();
    playback(&mut engine).refresh();
    engine.advance_live();
    engine.advance_live();
    let resource = engine.world().resource::<ludion_engine::RunSave>().unwrap();
    assert!(
        resource.pending().is_some(),
        "a live suspend file is pending"
    );
    assert!(
        !engine
            .world()
            .resource::<ReplayPlayback>()
            .unwrap()
            .entries()
            .is_empty()
    );

    playback(&mut engine).play("run", "hub.yaml");
    engine.advance_live();
    assert_eq!(engine.playing(), Some("run"));
    assert_eq!(
        engine
            .world()
            .resource::<ReplayPlayback>()
            .unwrap()
            .playing(),
        None,
        "the replayed world sees no playback, as under --replay-verify"
    );
    let mut played: Trace = vec![sample(&mut engine)];
    while engine.advance_live() {
        played.push(sample(&mut engine));
    }
    assert_eq!(played, expected);

    let resource = engine.world().resource::<ludion_engine::RunSave>().unwrap();
    assert!(resource.pending().is_some(), "the live mirror is back");
    assert!(
        !engine
            .world()
            .resource::<ReplayPlayback>()
            .unwrap()
            .entries()
            .is_empty()
    );
}
