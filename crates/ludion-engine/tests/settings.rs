//! The player settings: requests apply at the frame boundary, persist only under
//! `persist_settings()`, survive rebuilds and never move a `WorldHash` (`Plan/settings.md`).

use std::path::{Path, PathBuf};

use ludion_core::audio::{AudioMessage, Bus, MixLayer};
use ludion_core::time::FrameIndex;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    AntiAliasing, Engine, EngineBuilder, PlaybackOutcome, QualityPreset, QualitySetting,
    ReplayPlayback, ReplayPlaybackConfig, RunRecording, RunRecordingConfig, Settings, SettingsView,
    WindowMode, replay_hashes_with,
};
use ludion_protocol::replay::ReplayReader;
use ludion_protocol::{Command, ResponseBody};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-settings-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn file(root: &Path) -> PathBuf {
    root.join("save").join("settings.yaml")
}

/// The options menu: tick 0 sets and saves, tick 1 repeats the window mode, tick 2 changes it.
fn menu(world: &mut World, ctx: &SystemCtx) {
    let Some(settings) = world.resource_mut::<Settings>() else {
        return;
    };
    match ctx.frame {
        FrameIndex(0) => {
            settings.request_bus_volume(Bus::Music, 0.6);
            settings.request_quality(QualityPreset::Medium);
            settings.request_anti_aliasing(AntiAliasing::Smaa);
            settings.request_window_mode(WindowMode::BorderlessFullscreen);
            settings.save();
            settings.save();
        }
        FrameIndex(1) => settings.request_window_mode(WindowMode::BorderlessFullscreen),
        FrameIndex(2) => settings.request_window_mode(WindowMode::Windowed),
        _ => {}
    }
}

fn quiet(_world: &mut World, _ctx: &SystemCtx) {}

fn game(root: &Path, persist: bool) -> EngineBuilder {
    let builder = Engine::builder().save_root(root).system("menu", menu);
    if persist {
        builder.persist_settings()
    } else {
        builder
    }
}

fn bus_gains(engine: &Engine) -> Vec<(Bus, MixLayer, f32)> {
    engine
        .audio_log()
        .iter()
        .filter_map(|entry| match entry.message {
            AudioMessage::BusGain {
                bus, layer, gain, ..
            } => Some((bus, layer, gain)),
            _ => None,
        })
        .collect()
}

#[test]
fn requests_apply_persist_and_boot_back() {
    let root = scratch("roundtrip");
    let mut engine = game(&root, true).build().unwrap();
    assert!(engine.boot_warnings().is_empty());
    engine.advance_live();
    assert_eq!(bus_gains(&engine), [(Bus::Music, MixLayer::User, 0.6)]);
    assert_eq!(engine.anti_aliasing(), AntiAliasing::Smaa);
    assert_eq!(engine.quality().label, "medium");
    let view = engine.world().resource::<SettingsView>().unwrap();
    assert_eq!(view.volume(Bus::Music), 0.6);
    assert_eq!(view.quality(), Some(QualityPreset::Medium));
    assert_eq!(view.anti_aliasing(), Some(AntiAliasing::Smaa));
    assert!(file(&root).exists(), "save() wrote the file");

    let booted = Engine::builder()
        .save_root(&root)
        .persist_settings()
        .build()
        .unwrap();
    assert!(
        booted.boot_warnings().is_empty(),
        "{:?}",
        booted.boot_warnings()
    );
    let view = booted.world().resource::<SettingsView>().unwrap();
    assert_eq!(view.volume(Bus::Music), 0.6);
    assert_eq!(view.anti_aliasing(), Some(AntiAliasing::Smaa));
    assert_eq!(booted.anti_aliasing(), AntiAliasing::Smaa);
    assert_eq!(booted.quality().label, "medium");
    assert_eq!(
        booted.settings_window_mode(),
        Some(WindowMode::BorderlessFullscreen)
    );
}

#[test]
fn without_persist_settings_the_file_is_neither_read_nor_written() {
    let root = scratch("off");
    let seeded = "format: 1\nkind: settings\naudio:\n  master: 0.5\n";
    std::fs::create_dir_all(root.join("save")).unwrap();
    std::fs::write(file(&root), seeded).unwrap();
    let mut engine = game(&root, false).build().unwrap();
    let view = engine.world().resource::<SettingsView>().unwrap();
    assert_eq!(view.volume(Bus::Master), 1.0, "the file was not read");
    engine.advance_live();
    assert_eq!(
        engine.quality().label,
        "medium",
        "the request still applies to the session"
    );
    assert_eq!(std::fs::read_to_string(file(&root)).unwrap(), seeded);
}

#[test]
fn a_pinned_quality_persists_but_does_not_apply() {
    let root = scratch("pinned");
    let mut engine = game(&root, true).build().unwrap();
    engine.pin_quality();
    let before = engine.quality().clone();
    engine.advance_live();
    assert_eq!(engine.quality(), &before);
    let view = engine.world().resource::<SettingsView>().unwrap();
    assert_eq!(view.quality(), Some(QualityPreset::Medium));
    assert!(
        std::fs::read_to_string(file(&root))
            .unwrap()
            .contains("quality: medium")
    );
}

#[test]
fn the_audio_sender_is_seeded_without_a_log_entry() {
    let root = scratch("seed");
    std::fs::create_dir_all(root.join("save")).unwrap();
    std::fs::write(
        file(&root),
        "format: 1\nkind: settings\naudio:\n  master: 1.0\n  buses:\n    music: 0.5\n",
    )
    .unwrap();
    let mut engine = Engine::builder()
        .save_root(&root)
        .persist_settings()
        .build()
        .unwrap();
    let logged = engine.audio_log().len();
    let (tx, rx) = crossbeam_channel::unbounded();
    engine.set_audio_sender(tx);
    let seeded: Vec<_> = rx.try_iter().collect();
    assert_eq!(
        seeded,
        [AudioMessage::BusGain {
            bus: Bus::Music,
            layer: MixLayer::User,
            gain: 0.5,
            fade_frames: 0
        }]
    );
    assert_eq!(engine.audio_log().len(), logged, "the seed is not logged");
    assert!(bus_gains(&engine).is_empty());

    let (tx, rx) = crossbeam_channel::unbounded();
    let mut default = Engine::new();
    default.set_audio_sender(tx);
    assert!(
        rx.try_iter().next().is_none(),
        "a default file sends nothing"
    );
}

#[test]
fn a_window_request_reaches_the_sender_once_per_change() {
    let root = scratch("window");
    let mut engine = game(&root, false).build().unwrap();
    let (tx, rx) = crossbeam_channel::unbounded();
    engine.set_window_mode_sender(tx);
    for _ in 0..4 {
        engine.advance_live();
    }
    assert_eq!(
        rx.try_iter().collect::<Vec<_>>(),
        [WindowMode::BorderlessFullscreen, WindowMode::Windowed]
    );
}

/// Tick 1 asks for borderless fullscreen and a save, tick 2 only for a save.
fn ask_borderless(world: &mut World, ctx: &SystemCtx) {
    let Some(settings) = world.resource_mut::<Settings>() else {
        return;
    };
    match ctx.frame {
        FrameIndex(1) => settings.request_window_mode(WindowMode::BorderlessFullscreen),
        FrameIndex(2) => settings.save(),
        _ => {}
    }
}

fn save_at_one(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame == FrameIndex(1)
        && let Some(settings) = world.resource_mut::<Settings>()
    {
        settings.save();
    }
}

#[test]
fn headless_the_view_has_no_live_window_mode() {
    let root = scratch("live-headless");
    let mut engine = game(&root, false).build().unwrap();
    engine.advance_live();
    let view = engine.world().resource::<SettingsView>().unwrap();
    assert_eq!(view.live_window_mode(), None);
}

#[test]
fn a_reported_window_mode_reaches_the_view_without_becoming_the_saved_choice() {
    let root = scratch("live-report");
    let mut engine = Engine::builder()
        .save_root(&root)
        .persist_settings()
        .system("save", save_at_one)
        .build()
        .unwrap();
    let (report, reports) = crossbeam_channel::unbounded();
    engine.set_window_mode_reports(reports);
    report.send(WindowMode::BorderlessFullscreen).unwrap();
    for _ in 0..3 {
        engine.advance_live();
    }
    let view = engine.world().resource::<SettingsView>().unwrap();
    assert_eq!(
        view.live_window_mode(),
        Some(WindowMode::BorderlessFullscreen)
    );
    assert_eq!(
        view.window_mode(),
        None,
        "the chord is not the saved choice"
    );
    let text = std::fs::read_to_string(file(&root)).unwrap();
    assert!(!text.contains("window"), "{text}");
}

#[test]
fn a_menu_request_for_the_files_own_mode_reaches_the_sender_after_a_chord() {
    let root = scratch("live-chord");
    write_settings(
        &root,
        "format: 1\nkind: settings\ndisplay:\n  window: borderless_fullscreen\n",
    );
    let mut engine = Engine::builder()
        .save_root(&root)
        .persist_settings()
        .system("ask", ask_borderless)
        .build()
        .unwrap();
    assert_eq!(
        engine
            .world()
            .resource::<SettingsView>()
            .unwrap()
            .window_mode(),
        Some(WindowMode::BorderlessFullscreen)
    );
    let (tx, rx) = crossbeam_channel::unbounded();
    engine.set_window_mode_sender(tx);
    let (report, reports) = crossbeam_channel::unbounded();
    engine.set_window_mode_reports(reports);
    // The window was chorded out of fullscreen before the menu asks for the file's own mode.
    report.send(WindowMode::Windowed).unwrap();
    for _ in 0..3 {
        engine.advance_live();
    }
    assert_eq!(
        rx.try_iter().collect::<Vec<_>>(),
        [WindowMode::BorderlessFullscreen]
    );
}

fn trace(builder: EngineBuilder) -> Vec<u64> {
    let mut engine = builder.build().unwrap();
    (0..6)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

#[test]
fn settings_are_not_sim_state() {
    let root = scratch("hash");
    let with = trace(game(&root, false));
    let without = trace(Engine::builder().save_root(&root).system("menu", quiet));
    assert_eq!(with, without);
}

#[test]
fn a_rebuild_keeps_the_view_and_the_engine_values() {
    let root = scratch("rebuild");
    let mut engine = game(&root, false).build().unwrap();
    engine.advance_live();
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .expect("scene loads");
    let view = engine.world().resource::<SettingsView>().unwrap();
    assert_eq!(view.volume(Bus::Music), 0.6);
    assert_eq!(view.quality(), Some(QualityPreset::Medium));
    assert_eq!(engine.anti_aliasing(), AntiAliasing::Smaa);
    assert_eq!(engine.quality().label, "medium");
}

// --- W2: the header carries the settings; a replay installs the view and never touches disk -------

const SCENE: &str = "format: 1\nseed: 1\nentities:\n  - name: ear\n    components:\n      Heard:\n        music: 0.0\n";

/// The music volume a system read from the settings view, so it is visible to the `WorldHash`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
struct Heard {
    music: f32,
}

fn hear(world: &mut World, _ctx: &SystemCtx) {
    let music = world
        .resource::<SettingsView>()
        .map_or(0.0, |view| view.volume(Bus::Music));
    for (heard,) in world.query::<(&mut Heard,)>() {
        heard.music = music;
    }
}

/// Asks for a save every tick, so a replay that honored it would write the file.
fn save_every_tick(world: &mut World, _ctx: &SystemCtx) {
    if let Some(settings) = world.resource_mut::<Settings>() {
        settings.save();
    }
}

fn root(name: &str) -> PathBuf {
    let dir = scratch(name);
    std::fs::create_dir_all(dir.join("content")).unwrap();
    dir
}

fn music_file(music: f32) -> String {
    format!("format: 1\nkind: settings\naudio:\n  master: 1.0\n  buses:\n    music: {music}\n")
}

fn write_settings(root: &Path, text: &str) {
    std::fs::create_dir_all(file(root).parent().unwrap()).unwrap();
    std::fs::write(file(root), text).unwrap();
}

fn heard_builder(root: &Path, saver: bool) -> EngineBuilder {
    let mut builder = Engine::builder()
        .components(|reg| reg.register::<Heard>("Heard"))
        .system("hear", hear)
        .project_root(root.join("content"))
        .save_root(root)
        .persist_settings();
    if saver {
        builder = builder.system("save_every_tick", save_every_tick);
    }
    builder
}

fn backups(root: &Path) -> usize {
    std::fs::read_dir(file(root).parent().unwrap())
        .map(|dir| {
            dir.flatten()
                .filter(|entry| entry.file_name().to_string_lossy().contains(".bak."))
                .count()
        })
        .unwrap_or(0)
}

fn record_session(engine: &mut Engine, path: &Path) {
    engine
        .set_recording(std::fs::File::create(path).unwrap())
        .unwrap();
    for _ in 0..6 {
        engine.advance_live();
    }
    engine.stop_recording().unwrap();
}

fn recorded_on(root: &Path, text: &str) -> Vec<u8> {
    write_settings(root, text);
    let mut engine = heard_builder(root, false).build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    let path = root.join("session.lrp");
    record_session(&mut engine, &path);
    std::fs::read(path).unwrap()
}

fn heard_of(engine: &mut Engine) -> f32 {
    engine
        .world_mut()
        .query::<(&Heard,)>()
        .map(|(heard,)| heard.music)
        .next()
        .unwrap()
}

#[test]
fn the_header_wins_over_the_replaying_machines_file() {
    let a = root("header-a");
    let b = root("header-b");
    let bytes = recorded_on(&a, &music_file(0.5));
    let header = ReplayReader::new(&bytes).unwrap().header().clone();
    let carried = header.save_docs.get("settings").expect("carried");
    assert!(carried.contains("music: 0.5"), "{carried}");

    let b_text = "format: 1\nkind: settings\n  audio: [\n";
    write_settings(&b, b_text);
    let mut replayed = ludion_engine::replay_with(
        heard_builder(&b, true),
        &bytes,
        b.join("content"),
        |_, _| Ok(std::ops::ControlFlow::Continue(())),
    )
    .unwrap();
    assert_eq!(heard_of(&mut replayed), 0.5, "the header's value, not B's");
    let view = replayed.world().resource::<SettingsView>().unwrap();
    assert_eq!(view.volume(Bus::Music), 0.5);
    assert!(
        replayed.boot_warnings().is_empty(),
        "the file was never read: {:?}",
        replayed.boot_warnings()
    );
    assert_eq!(std::fs::read_to_string(file(&b)).unwrap(), b_text);
    assert_eq!(backups(&b), 0, "a replay never backs the file up");

    let live = replay_hashes_with(heard_builder(&a, false), &bytes, a.join("content")).unwrap();
    let elsewhere =
        replay_hashes_with(heard_builder(&b, false), &bytes, b.join("content")).unwrap();
    assert_eq!(live, elsewhere, "the hash trace is identical");
    let c = root("header-c");
    let bare = replay_hashes_with(heard_builder(&c, false), &bytes, c.join("content")).unwrap();
    assert_eq!(live, bare, "no file at all reproduces too");
    assert!(!file(&c).exists(), "and none is created");
}

#[test]
fn a_replay_never_touches_the_disk() {
    let a = root("disk-a");
    let bytes = recorded_on(&a, &music_file(0.5));

    // A `save()` every tick on a root with no file writes nothing.
    let empty = root("disk-empty");
    let mut replayed = ludion_engine::replay_with(
        heard_builder(&empty, true),
        &bytes,
        empty.join("content"),
        |_, _| Ok(std::ops::ControlFlow::Continue(())),
    )
    .unwrap();
    assert_eq!(heard_of(&mut replayed), 0.5);
    assert!(!file(&empty).exists(), "the replayed save() wrote nothing");

    // And a malformed file is never read: no warning, no backup, bytes unchanged.
    let bad = root("disk-bad");
    let bad_text = "format: 1\nkind: settings\n  audio: [\n";
    write_settings(&bad, bad_text);
    let replayed = ludion_engine::replay_with(
        heard_builder(&bad, true),
        &bytes,
        bad.join("content"),
        |_, _| Ok(std::ops::ControlFlow::Continue(())),
    )
    .unwrap();
    assert!(replayed.boot_warnings().is_empty());
    assert_eq!(std::fs::read_to_string(file(&bad)).unwrap(), bad_text);
    assert_eq!(backups(&bad), 0);
}

#[test]
fn default_settings_leave_the_header_without_a_settings_key() {
    let root = root("default-header");
    let mut engine = heard_builder(&root, false).build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    let path = root.join("session.lrp");
    record_session(&mut engine, &path);
    let bytes = std::fs::read(&path).unwrap();
    let header = ReplayReader::new(&bytes).unwrap().header().clone();
    assert!(header.save_docs.is_empty());
    let json = serde_json::to_string(&header).unwrap();
    assert!(!json.contains("settings"), "{json}");
    let mut reference = header.clone();
    reference.save_docs = std::collections::BTreeMap::new();
    assert_eq!(json, serde_json::to_string(&reference).unwrap());
    assert!(!json.contains("save_docs"), "no key at all: {json}");
}

fn request_music(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame == FrameIndex(0)
        && let Some(settings) = world.resource_mut::<Settings>()
    {
        settings.request_bus_volume(Bus::Music, 0.7);
    }
}

#[test]
fn a_recording_armed_after_a_request_carries_the_requested_value() {
    let root = root("armed-after");
    let mut engine = Engine::builder()
        .components(|reg| reg.register::<Heard>("Heard"))
        .system("request_music", request_music)
        .project_root(root.join("content"))
        .save_root(&root)
        .build()
        .unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine.advance_live();
    // A reload puts the frame back at zero, where a recording can arm; the settings stay.
    engine.load_inline_scene(SCENE).unwrap();
    let path = root.join("session.lrp");
    record_session(&mut engine, &path);
    let bytes = std::fs::read(&path).unwrap();
    let header = ReplayReader::new(&bytes).unwrap().header().clone();
    let carried = header.save_docs.get("settings").expect("carried");
    assert!(carried.contains("music: 0.7"), "{carried}");
}

#[test]
fn describe_reports_the_settings_section() {
    let root = root("describe");
    write_settings(&root, &music_file(0.5));
    let describe = |engine: &mut Engine| match engine.execute(Command::EngineDescribe).unwrap().body
    {
        ResponseBody::EngineDescription { description } => description["settings"].clone(),
        other => panic!("expected a description, got {other:?}"),
    };
    let mut engine = heard_builder(&root, false).build().unwrap();
    let section = describe(&mut engine);
    assert_eq!(section["persisted"], true);
    assert_eq!(section["quality_pinned"], false);
    assert_eq!(section["values"]["audio"]["buses"]["music"], 0.5);
    engine.pin_quality();
    assert_eq!(describe(&mut engine)["quality_pinned"], true);
}

/// Asks for `full` at tick 2, a value the recording's header (`medium`) does not already hold.
fn late_full(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame == FrameIndex(2)
        && let Some(settings) = world.resource_mut::<Settings>()
    {
        settings.request_quality(QualityPreset::Full);
    }
}

fn playback_game(dir: &Path) -> EngineBuilder {
    Engine::builder()
        .project_root(dir)
        .save_root(dir)
        .persist_profile()
        .run_recordings(RunRecordingConfig::default())
        .replay_playback(ReplayPlaybackConfig::default())
        .system("menu", menu)
        .system("late_full", late_full)
}

#[test]
fn in_place_playback_returns_the_pre_play_settings_and_render_state() {
    let dir = scratch("playback");
    let hub = "format: 1\nname: hub\nseed: 5\nentities: []\n";
    std::fs::write(dir.join("hub.yaml"), hub).unwrap();
    let mut engine = playback_game(&dir).build().unwrap();
    engine.load_inline_scene(hub).unwrap();
    engine.advance_live();
    engine
        .world_mut()
        .resource_mut::<RunRecording>()
        .unwrap()
        .start("p");
    for _ in 0..20 {
        engine.advance_live();
    }
    engine
        .world_mut()
        .resource_mut::<RunRecording>()
        .unwrap()
        .stop();
    engine.advance_live();

    // The live player then picks different values than the recorded game's menu does.
    let settings = engine.world_mut().resource_mut::<Settings>().unwrap();
    settings.request_bus_volume(Bus::Music, 0.2);
    settings.request_quality(QualityPreset::High);
    settings.request_anti_aliasing(AntiAliasing::Off);
    engine.advance_live();
    assert_eq!(engine.quality().label, "high");

    engine
        .world_mut()
        .resource_mut::<ReplayPlayback>()
        .unwrap()
        .play("p", "hub.yaml");
    for _ in 0..6 {
        engine.advance_live();
    }
    let outcome = engine
        .world()
        .resource::<ReplayPlayback>()
        .unwrap()
        .last_outcome()
        .cloned();
    assert_eq!(engine.playing(), Some("p"), "{outcome:?}");
    assert_eq!(
        engine.quality().label,
        "full",
        "the replayed game's request changed render state"
    );
    engine.stop_playback();
    assert!(matches!(
        engine
            .world()
            .resource::<ReplayPlayback>()
            .unwrap()
            .last_outcome(),
        Some(PlaybackOutcome::Stopped { .. })
    ));
    let view = engine.world().resource::<SettingsView>().unwrap();
    assert_eq!(view.volume(Bus::Music), 0.2, "the pre-play view is back");
    assert_eq!(view.quality(), Some(QualityPreset::High));
    assert_eq!(engine.quality().label, "high");
    assert_eq!(engine.anti_aliasing(), AntiAliasing::Off);
}

/// Asks for borderless fullscreen at tick 2, inside the recorded span.
fn late_borderless(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame == FrameIndex(2)
        && let Some(settings) = world.resource_mut::<Settings>()
    {
        settings.request_window_mode(WindowMode::BorderlessFullscreen);
    }
}

#[test]
fn a_replay_never_moves_the_players_window() {
    let dir = scratch("playback-window");
    let hub = "format: 1\nname: hub\nseed: 5\nentities: []\n";
    std::fs::write(dir.join("hub.yaml"), hub).unwrap();
    let mut engine = Engine::builder()
        .project_root(&dir)
        .save_root(&dir)
        .persist_profile()
        .run_recordings(RunRecordingConfig::default())
        .replay_playback(ReplayPlaybackConfig::default())
        .system("late_borderless", late_borderless)
        .build()
        .unwrap();
    engine.load_inline_scene(hub).unwrap();
    engine.advance_live();
    engine
        .world_mut()
        .resource_mut::<RunRecording>()
        .unwrap()
        .start("p");
    for _ in 0..20 {
        engine.advance_live();
    }
    engine
        .world_mut()
        .resource_mut::<RunRecording>()
        .unwrap()
        .stop();
    engine.advance_live();

    // Live, the player picks windowed; the armed channel then sees only playback traffic.
    engine
        .world_mut()
        .resource_mut::<Settings>()
        .unwrap()
        .request_window_mode(WindowMode::Windowed);
    engine.advance_live();
    let (tx, rx) = crossbeam_channel::unbounded();
    engine.set_window_mode_sender(tx);
    assert!(rx.try_recv().is_err());

    engine
        .world_mut()
        .resource_mut::<ReplayPlayback>()
        .unwrap()
        .play("p", "hub.yaml");
    for _ in 0..6 {
        engine.advance_live();
    }
    assert_eq!(engine.playing(), Some("p"));
    assert!(
        rx.try_recv().is_err(),
        "a replayed window request reached the live window"
    );
    engine.stop_playback();
    assert!(rx.try_recv().is_err());
    assert_eq!(engine.settings_window_mode(), Some(WindowMode::Windowed));

    // Live again, the same request still reaches the sender (the gate is not over-broad).
    engine
        .world_mut()
        .resource_mut::<Settings>()
        .unwrap()
        .request_window_mode(WindowMode::BorderlessFullscreen);
    engine.advance_live();
    assert_eq!(
        rx.try_iter().collect::<Vec<_>>(),
        [WindowMode::BorderlessFullscreen]
    );
}

/// Every combination of named/unnamed × file preset/no file × packaged/full.
#[test]
fn startup_quality_precedence_holds_for_every_combination() {
    for named in [false, true] {
        for file_preset in [None, Some("low")] {
            for resolved in ["full", "high"] {
                let root = scratch("startup");
                if let Some(preset) = file_preset {
                    write_settings(
                        &root,
                        &format!("format: 1\nkind: settings\ngraphics:\n  quality: {preset}\n"),
                    );
                }
                let mut engine = Engine::builder()
                    .save_root(&root)
                    .persist_settings()
                    .build()
                    .unwrap();
                engine.apply_startup_quality(QualitySetting::preset(resolved).unwrap(), named);
                let expected = match (named, file_preset) {
                    (true, _) => resolved,
                    (false, Some(preset)) => preset,
                    (false, None) => resolved,
                };
                assert_eq!(
                    engine.quality().label,
                    expected,
                    "named={named} file={file_preset:?} resolved={resolved}"
                );
                // Pinned exactly when named: a later request applies only when unpinned.
                let settings = engine.world_mut().resource_mut::<Settings>().unwrap();
                settings.request_quality(QualityPreset::Medium);
                engine.advance_live();
                let after = if named { expected } else { "medium" };
                assert_eq!(engine.quality().label, after, "pin after named={named}");
            }
        }
    }
}

// --- Anti-aliasing: the game's runtime request is `Settings::request_anti_aliasing` -------------------

fn aa_twice(world: &mut World, _ctx: &SystemCtx) {
    if let Some(settings) = world.resource_mut::<Settings>() {
        settings.request_anti_aliasing(AntiAliasing::Smaa);
        settings.request_anti_aliasing(AntiAliasing::Off);
    }
}

fn aa_toggle(world: &mut World, ctx: &SystemCtx) {
    let mode = match ctx.frame {
        FrameIndex(1) => AntiAliasing::Smaa,
        FrameIndex(2) => AntiAliasing::Off,
        _ => return,
    };
    if let Some(settings) = world.resource_mut::<Settings>() {
        settings.request_anti_aliasing(mode);
    }
}

fn aa_smaa_and_save(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame == FrameIndex(0)
        && let Some(settings) = world.resource_mut::<Settings>()
    {
        settings.request_anti_aliasing(AntiAliasing::Smaa);
        settings.save();
    }
}

#[test]
fn the_last_aa_request_in_a_tick_wins() {
    let mut engine = Engine::builder().system("aa", aa_twice).build().unwrap();
    engine.set_anti_aliasing(AntiAliasing::Smaa);
    engine.advance_live();
    assert_eq!(engine.anti_aliasing(), AntiAliasing::Off);
}

#[test]
fn the_game_request_beats_the_host_and_toggles_per_frame() {
    let mut engine = Engine::builder()
        .system("toggle", aa_toggle)
        .build()
        .unwrap();
    engine.set_anti_aliasing(AntiAliasing::Off);
    let mut seen = Vec::new();
    for _ in 0..3 {
        engine.advance_live();
        seen.push(engine.anti_aliasing());
    }
    assert_eq!(
        seen,
        [AntiAliasing::Off, AntiAliasing::Smaa, AntiAliasing::Off]
    );
}

#[test]
fn the_aa_choice_survives_a_world_rebuild() {
    let mut engine = Engine::builder()
        .system("toggle", aa_toggle)
        .build()
        .unwrap();
    engine.advance_live();
    engine.advance_live();
    assert_eq!(engine.anti_aliasing(), AntiAliasing::Smaa);
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .expect("scene loads");
    assert_eq!(engine.anti_aliasing(), AntiAliasing::Smaa);
}

#[test]
fn describe_reports_the_anti_aliasing_mode_and_the_builder_default_holds() {
    let described =
        |engine: &mut Engine| match engine.execute(Command::EngineDescribe).unwrap().body {
            ResponseBody::EngineDescription { description } => description["anti_aliasing"].clone(),
            other => panic!("expected a description, got {other:?}"),
        };
    assert_eq!(described(&mut Engine::new()), "off");
    let mut engine = Engine::builder()
        .anti_aliasing(AntiAliasing::Smaa)
        .build()
        .unwrap();
    assert_eq!(engine.anti_aliasing(), AntiAliasing::Smaa);
    assert_eq!(described(&mut engine), "smaa");
}

#[test]
fn a_saved_aa_request_beats_the_host_this_session_and_sits_below_it_at_the_next_boot() {
    let root = scratch("aa-persist");
    let mut engine = Engine::builder()
        .save_root(&root)
        .persist_settings()
        .system("aa", aa_smaa_and_save)
        .build()
        .unwrap();
    engine.advance_live();
    engine.set_anti_aliasing(AntiAliasing::Off);
    assert_eq!(
        engine.anti_aliasing(),
        AntiAliasing::Smaa,
        "this session's request still beats the host"
    );

    let mut next = Engine::builder()
        .save_root(&root)
        .persist_settings()
        .build()
        .unwrap();
    assert_eq!(
        next.anti_aliasing(),
        AntiAliasing::Smaa,
        "the file persists"
    );
    next.set_anti_aliasing(AntiAliasing::Off);
    assert_eq!(
        next.anti_aliasing(),
        AntiAliasing::Off,
        "the file sits below --aa"
    );
}

// --- R14: a snapshot carries the settings view ----------------------------------------------------

fn snapshot_of(engine: &mut Engine) -> serde_json::Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

fn restore(engine: &mut Engine, snapshot: serde_json::Value) {
    engine
        .execute(Command::EngineRestore(
            ludion_protocol::command::EngineRestoreParams { snapshot },
        ))
        .unwrap();
}

fn requesting_engine(root: &Path) -> Engine {
    let mut engine = Engine::builder()
        .components(|reg| reg.register::<Heard>("Heard"))
        .system("hear", hear)
        .project_root(root.join("content"))
        .save_root(root)
        .build()
        .unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine
}

#[test]
fn a_restore_installs_the_snapshots_view_and_leaves_render_state() {
    let root = root("restore-view");
    let mut engine = requesting_engine(&root);
    engine
        .world_mut()
        .resource_mut::<Settings>()
        .unwrap()
        .request_bus_volume(Bus::Music, 0.4);
    engine.advance_live();
    let snapshot = snapshot_of(&mut engine);
    assert!(snapshot.get("settings").is_some());

    let settings = engine.world_mut().resource_mut::<Settings>().unwrap();
    settings.request_bus_volume(Bus::Music, 0.9);
    settings.request_quality(QualityPreset::High);
    settings.request_anti_aliasing(AntiAliasing::Off);
    engine.advance_live();
    engine.advance_live();
    assert_eq!(heard_of(&mut engine), 0.9);

    restore(&mut engine, snapshot);
    let view = engine.world().resource::<SettingsView>().unwrap();
    assert_eq!(view.volume(Bus::Music), 0.4, "the snapshot's view");
    assert_eq!(
        engine.quality().label,
        "high",
        "render quality is untouched"
    );
    assert_eq!(engine.anti_aliasing(), AntiAliasing::Off);
    engine.advance_live();
    assert_eq!(
        heard_of(&mut engine),
        0.4,
        "the game reads the restored view"
    );
}

#[test]
fn default_settings_leave_the_snapshot_without_a_settings_key() {
    let root = root("default-snapshot");
    let mut engine = requesting_engine(&root);
    engine.advance_live();
    assert!(snapshot_of(&mut engine).get("settings").is_none());
}

#[test]
fn a_replay_seek_keyframe_carries_the_view_after_a_mid_session_request() {
    let root = root("seek-view");
    let mut live = Engine::builder()
        .components(|reg| reg.register::<Heard>("Heard"))
        .system("hear", hear)
        .system("request_music", request_music)
        .project_root(root.join("content"))
        .save_root(&root)
        .build()
        .unwrap();
    live.load_inline_scene(SCENE).unwrap();
    let path = root.join("seek.lrp");
    record_session(&mut live, &path);
    let live_view = live.world().resource::<SettingsView>().unwrap().clone();
    assert_eq!(live_view.volume(Bus::Music), 0.7);

    let bytes = std::fs::read(&path).unwrap();
    let seek = ReplayReader::new(&bytes)
        .unwrap()
        .seek(live.frame())
        .unwrap();
    let (_, snapshot) = seek.keyframe.expect("the closing keyframe");
    let mut fresh = requesting_engine(&root);
    assert_eq!(
        fresh
            .world()
            .resource::<SettingsView>()
            .unwrap()
            .volume(Bus::Music),
        1.0
    );
    restore(&mut fresh, snapshot);
    assert_eq!(
        fresh.world().resource::<SettingsView>().unwrap(),
        &live_view,
        "identical SettingsView at the keyframe"
    );
}
