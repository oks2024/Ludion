//! The rumble, brightness and text settings: where each applies, which are host render state, and
//! that none moves a `WorldHash` (`Plan/settings.md`).
//!
//! The GPU test skips (printing why) when no adapter is available.

use std::path::{Path, PathBuf};

use ludion_assets::AssetRoot;
use ludion_core::rumble::RumbleCommand;
use ludion_core::time::FrameIndex;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::localization::{self, LocaleState};
use ludion_engine::{
    Diagnostics, DialogueConfig, Engine, EngineBuilder, PlaybackOutcome, ReplayPlayback,
    ReplayPlaybackConfig, Rumble, RumbleEffect, RunRecording, RunRecordingConfig, SceneSwitch,
    Settings, SettingsView, dialogue, dialogue_update, replay_hashes_with, replay_with,
};
use ludion_protocol::Command;
use ludion_protocol::command::SimStepParams;
use ludion_renderer::{RenderScene, UiInstance};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-settings-groups-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_settings(root: &Path, text: &str) {
    let path = root.join("save").join("settings.yaml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn persisted(root: &Path) -> EngineBuilder {
    Engine::builder().save_root(root).persist_settings()
}

fn requests(engine: &mut Engine) -> &mut Settings {
    engine.world_mut().resource_mut::<Settings>().unwrap()
}

/// Plays one full-scale effect every tick.
fn rumble_every_tick(world: &mut World, _ctx: &SystemCtx) {
    if let Some(rumble) = world.resource_mut::<Rumble>() {
        rumble.play(RumbleEffect {
            low: 0.8,
            high: 0.4,
            duration: 1.0,
        });
    }
}

#[test]
fn rumble_from_the_file_scales_a_play_and_a_request_of_zero_drops_the_next() {
    let root = scratch("rumble");
    write_settings(&root, "format: 1\nkind: settings\ninput:\n  rumble: 0.5\n");
    let mut engine = persisted(&root)
        .system("rumble", rumble_every_tick)
        .build()
        .unwrap();
    assert!(engine.boot_warnings().is_empty());
    assert_eq!(engine.rumble_intensity(), 0.5);
    let (tx, rx) = crossbeam_channel::unbounded();
    engine.set_rumble_sender(tx);
    engine.advance_live();
    assert_eq!(
        rx.try_iter().collect::<Vec<_>>(),
        [RumbleCommand::Play {
            low: 0.4,
            high: 0.2,
            seconds: 1.0
        }]
    );

    requests(&mut engine).request_rumble(0.0);
    // The boundary drains the pad outbox before the settings, so this tick's play is the old one.
    engine.advance_live();
    assert_eq!(rx.try_iter().count(), 1);
    assert_eq!(engine.rumble_intensity(), 0.0);
    let view = engine.world().resource::<SettingsView>().unwrap();
    assert_eq!(view.rumble(), 0.0);
    engine.advance_live();
    assert_eq!(rx.try_iter().count(), 0);
}

/// The recorded game picks its own brightness and rumble at tick 2, inside the recorded span.
fn late_request(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame == FrameIndex(2)
        && let Some(settings) = world.resource_mut::<Settings>()
    {
        settings.request_brightness(-0.75);
        settings.request_rumble(0.25);
    }
}

#[test]
fn brightness_and_rumble_are_host_state_that_no_header_installs_and_playback_restores() {
    let dir = scratch("playback");
    let hub = "format: 1\nname: hub\nseed: 5\nentities: []\n";
    std::fs::write(dir.join("hub.yaml"), hub).unwrap();
    write_settings(
        &dir,
        "format: 1\nkind: settings\ndisplay:\n  brightness: 0.5\ninput:\n  rumble: 0.5\n",
    );
    let mut engine = persisted(&dir)
        .project_root(&dir)
        .persist_profile()
        .run_recordings(RunRecordingConfig::default())
        .replay_playback(ReplayPlaybackConfig::default())
        .system("late_request", late_request)
        .build()
        .unwrap();
    assert_eq!(engine.brightness(), 0.5, "the file's value");

    // The recording is armed with brightness 0.5 in force, so its header carries it.
    engine.load_inline_scene(hub).unwrap();
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

    let settings = requests(&mut engine);
    settings.request_brightness(0.2);
    settings.request_rumble(0.8);
    engine.advance_live();
    assert_eq!(engine.brightness(), 0.2, "a request moves it");

    engine
        .world_mut()
        .resource_mut::<ReplayPlayback>()
        .unwrap()
        .play("p", "hub.yaml");
    engine.advance_live();
    let outcome = engine
        .world()
        .resource::<ReplayPlayback>()
        .unwrap()
        .last_outcome()
        .cloned();
    assert_eq!(engine.playing(), Some("p"), "{outcome:?}");
    let view = engine.world().resource::<SettingsView>().unwrap();
    assert_eq!(view.brightness(), 0.5, "the view is the header's");
    assert_eq!(
        engine.brightness(),
        0.2,
        "the render state is this machine's"
    );
    assert_eq!(engine.rumble_intensity(), 0.8);

    for _ in 0..8 {
        engine.advance_live();
    }
    assert_eq!(engine.brightness(), -0.75, "the replayed game's request");
    assert_eq!(engine.rumble_intensity(), 0.25);
    engine.stop_playback();
    assert!(matches!(
        engine
            .world()
            .resource::<ReplayPlayback>()
            .unwrap()
            .last_outcome(),
        Some(PlaybackOutcome::Stopped { .. })
    ));
    assert_eq!(engine.brightness(), 0.2);
    assert_eq!(engine.rumble_intensity(), 0.8);
}

/// A lit cube, so an exposure offset has something to brighten.
const LIT_SCENE: &str = "format: 1
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 4.0
      Camera:
        projection:
          type: perspective
          fov_y_deg: 50.0
  - name: key-light
    components:
      Transform:
        translation:
          - 1.0
          - 3.0
          - 3.0
      PointLight:
        color:
          - 1.0
          - 1.0
          - 1.0
        intensity: 12.0
        range: 20.0
  - name: cube
    components:
      Transform:
        rotation:
          - 20.0
          - 33.0
          - 0.0
      Mesh:
        shape: cube
        color:
          - 0.5
          - 0.5
          - 0.5
          - 1.0
";

const SIZE: (u32, u32) = (160, 90);

fn mean_luma(png: &[u8]) -> f64 {
    let decoder = png::Decoder::new(std::io::Cursor::new(png));
    let mut reader = decoder.read_info().expect("valid PNG");
    let mut buf = vec![0u8; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).expect("frame decodes");
    buf.truncate(info.buffer_size());
    let sum: f64 = buf
        .chunks_exact(4)
        .map(|p| 0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]))
        .sum();
    sum / f64::from(info.width * info.height)
}

#[test]
fn needs_gpu_brightness_zero_is_the_bare_frame_and_one_ev_brightens_it() {
    let root = scratch("gpu");
    write_settings(
        &root,
        "format: 1\nkind: settings\ndisplay:\n  brightness: 0.0\n",
    );
    let mut bare = Engine::new();
    bare.load_inline_scene(LIT_SCENE).unwrap();
    let mut engine = persisted(&root).build().unwrap();
    engine.load_inline_scene(LIT_SCENE).unwrap();
    bare.advance_live();
    engine.advance_live();
    let bare_png = match bare.render_png(SIZE.0, SIZE.1) {
        Ok(png) => png,
        Err(error) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&error)) => {
            eprintln!("[brightness] SKIPPED: no GPU adapter available");
            return;
        }
        Err(error) => panic!("[brightness] render failed: {error:?}"),
    };
    let zero = engine.render_png(SIZE.0, SIZE.1).unwrap();
    assert_eq!(zero, bare_png, "brightness 0 renders the bare frame");

    requests(&mut engine).request_brightness(1.0);
    bare.advance_live();
    engine.advance_live();
    let brighter = engine.render_png(SIZE.0, SIZE.1).unwrap();
    assert!(
        mean_luma(&brighter) > mean_luma(&zero),
        "+1 EV: {} vs {}",
        mean_luma(&brighter),
        mean_luma(&zero)
    );
    assert_eq!(engine.world_hash().unwrap(), bare.world_hash().unwrap());
}

fn trace(builder: EngineBuilder) -> Vec<u64> {
    let mut engine = builder.build().unwrap();
    assert!(
        engine.boot_warnings().is_empty(),
        "{:?}",
        engine.boot_warnings()
    );
    engine.load_inline_scene(LIT_SCENE).unwrap();
    (0..6)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

#[test]
fn neither_a_missing_file_nor_a_full_one_moves_the_trace() {
    let reference = trace(Engine::builder().save_root(scratch("trace-off")));
    assert_eq!(trace(persisted(&scratch("trace-none"))), reference);

    let full = scratch("trace-full");
    write_settings(
        &full,
        "format: 1\nkind: settings\ndisplay:\n  brightness: 0.5\ninput:\n  rumble: 0.5\ntext:\n  subtitles: false\n  subtitle_scale: 1.5\n  language: fr\n",
    );
    let engine = persisted(&full).build().unwrap();
    let view = engine.world().resource::<SettingsView>().unwrap();
    assert_eq!(view.brightness(), 0.5);
    assert_eq!(view.rumble(), 0.5);
    assert!(!view.subtitles());
    assert_eq!(view.subtitle_scale(), 1.5);
    assert_eq!(view.language(), Some("fr"));
    assert_eq!(trace(persisted(&full)), reference);
}

/// A subtitle block with no padding, so its panel is exactly one row of text tall.
const SUBTITLE_SCENE: &str = "format: 1
entities:
  - name: camera
    components:
      Transform: {}
      Camera: {}
  - name: subtitles
    components:
      SubtitleView:
        padding: 0.0
";

const STRINGS_EN: &str = "format: 1
kind: strings
locale: en
strings:
  speaker.guard: Guard
  t.line: Halt.
";

const DIALOGUE: &str = "format: 1
kind: dialogue
speakers:
  guard:
    name: speaker.guard
    channel: radio
lines:
  near:
    speaker: guard
    text: t.line
    duration_ticks: 60
";

const PANEL: [f32; 4] = [0.0, 0.0, 0.0, 0.55];
const VIEWPORT: (u32, u32) = (1920, 1080);

/// The subtitle panel's height in pixels and the number of text runs drawn.
fn subtitle_draws(engine: &Engine, root: &Path) -> (Option<f32>, usize) {
    let scene: RenderScene =
        ludion_engine::extract(engine.world(), VIEWPORT, AssetRoot::content(root)).unwrap();
    let panel = scene.ui.iter().find_map(|instance| match instance {
        UiInstance::Rect { rect_px, color, .. } if *color == PANEL => Some(rect_px[3]),
        _ => None,
    });
    let texts = scene
        .ui
        .iter()
        .filter(|instance| matches!(instance, UiInstance::Text { .. }))
        .count();
    (panel, texts)
}

fn step(engine: &mut Engine) {
    engine
        .execute(Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .unwrap();
}

#[test]
fn extract_hides_and_scales_subtitles_without_writing_the_view() {
    let root = scratch("subtitles");
    let mut engine = Engine::builder()
        .project_root(&root)
        .dialogue(DialogueConfig::default())
        .system("dialogue_update", dialogue_update)
        .doc_text("strings/en.yaml", STRINGS_EN)
        .doc_text("dialogue/main.yaml", DIALOGUE)
        .build()
        .unwrap();
    engine.load_inline_scene(SUBTITLE_SCENE).unwrap();
    dialogue::say(engine.world_mut(), "near", None).unwrap();
    step(&mut engine);
    let (height, texts) = subtitle_draws(&engine, &root);
    let height = height.expect("a line on air draws its panel");
    assert!(texts > 0);

    requests(&mut engine).request_subtitles(false);
    step(&mut engine);
    assert_eq!(subtitle_draws(&engine, &root), (None, 0));

    let settings = requests(&mut engine);
    settings.request_subtitles(true);
    settings.request_subtitle_scale(2.0);
    step(&mut engine);
    let (scaled, texts) = subtitle_draws(&engine, &root);
    assert_eq!(scaled, Some(height * 2.0));
    assert!(texts > 0);
    let authored = engine
        .world_mut()
        .query::<(&ludion_engine::SubtitleView,)>()
        .map(|(view,)| view.size)
        .collect::<Vec<_>>();
    assert_eq!(authored, [30.0], "the authored component is untouched");
}

const STRINGS_FR: &str = "format: 1
kind: strings
locale: fr
strings:
  hud.start: Demarrer
";

const STRINGS_DE: &str = "format: 1
kind: strings
locale: de
strings:
  hud.start: Starten
";

const KEYED_SCENE: &str = "format: 1
seed: 1
entities:
  - name: start
    components:
      UiText:
        text: Start (authored)
        key: hud.start
";

const FRENCH: &str = "format: 1\nkind: settings\ntext:\n  language: fr\n";

/// A root whose `content/` holds the French and German tables and a hub scene.
fn localized_root(name: &str, settings: Option<&str>) -> PathBuf {
    let root = scratch(name);
    let strings = root.join("content").join("strings");
    std::fs::create_dir_all(&strings).unwrap();
    std::fs::write(strings.join("fr.yaml"), STRINGS_FR).unwrap();
    std::fs::write(strings.join("de.yaml"), STRINGS_DE).unwrap();
    std::fs::write(root.join("content").join("hub.yaml"), KEYED_SCENE).unwrap();
    if let Some(text) = settings {
        write_settings(&root, text);
    }
    root
}

fn localized(root: &Path) -> EngineBuilder {
    persisted(root)
        .project_root(root.join("content"))
        .localization()
}

fn locale_states(engine: &mut Engine) -> usize {
    engine.world_mut().query_ids::<(&LocaleState,)>().count()
}

#[test]
fn the_file_language_is_the_locale_at_boot_load_and_restart_and_a_request_moves_it() {
    let root = localized_root("language", Some(FRENCH));
    let mut engine = localized(&root).build().unwrap();
    assert_eq!(localization::locale(engine.world_mut()), "fr", "boot");
    engine.load_inline_scene(KEYED_SCENE).unwrap();
    assert_eq!(localization::locale(engine.world_mut()), "fr", "load");
    assert_eq!(locale_states(&mut engine), 1);

    localization::set_locale(engine.world_mut(), "de");
    engine
        .world_mut()
        .resource_mut::<SceneSwitch>()
        .unwrap()
        .restart(Vec::new());
    step(&mut engine);
    assert_eq!(localization::locale(engine.world_mut()), "fr", "switch");

    requests(&mut engine).request_language("de");
    assert_eq!(localization::locale(engine.world_mut()), "fr");
    step(&mut engine);
    assert_eq!(localization::locale(engine.world_mut()), "de", "request");

    // The stored value is unchanged, but the game has set another locale since.
    localization::set_locale(engine.world_mut(), "fr");
    requests(&mut engine).request_language("de");
    step(&mut engine);
    assert_eq!(localization::locale(engine.world_mut()), "de");
}

#[test]
fn a_language_with_no_strings_warns_after_a_load() {
    let root = localized_root(
        "language-unknown",
        Some("format: 1\nkind: settings\ntext:\n  language: it\n"),
    );
    let mut engine = localized(&root).build().unwrap();
    engine.load_inline_scene(KEYED_SCENE).unwrap();
    let reported = engine
        .world()
        .resource::<Diagnostics>()
        .unwrap()
        .reported(ludion_core::codes::code::LOCALE_WITHOUT_STRINGS)
        .count();
    assert_eq!(reported, 1);
}

#[test]
fn a_file_without_a_language_spawns_no_locale_state() {
    let root = localized_root(
        "language-none",
        Some("format: 1\nkind: settings\ntext:\n  subtitles: false\n"),
    );
    let mut engine = localized(&root).build().unwrap();
    assert_eq!(locale_states(&mut engine), 0);
    engine.load_inline_scene(KEYED_SCENE).unwrap();
    step(&mut engine);
    assert_eq!(locale_states(&mut engine), 0);
}

/// The recorded game turns subtitles off, then picks German, inside the recorded span.
fn language_requests(world: &mut World, ctx: &SystemCtx) {
    let Some(settings) = world.resource_mut::<Settings>() else {
        return;
    };
    if ctx.frame == FrameIndex(2) {
        settings.request_subtitles(false);
    }
    if ctx.frame == FrameIndex(4) {
        settings.request_language("de");
    }
}

#[test]
fn a_recording_made_in_french_replays_identically_on_a_machine_with_no_file() {
    let recording = |root: &Path| localized(root).system("language_requests", language_requests);
    let a = localized_root("language-record", Some(FRENCH));
    let mut engine = recording(&a).build().unwrap();
    engine.load_inline_scene(KEYED_SCENE).unwrap();
    let path = a.join("session.lrp");
    engine
        .set_recording(std::fs::File::create(&path).unwrap())
        .unwrap();
    let live: Vec<_> = (0..8)
        .map(|_| {
            engine.advance_live();
            (engine.frame(), engine.world_hash().unwrap())
        })
        .collect();
    engine.stop_recording().unwrap();
    assert_eq!(localization::locale(engine.world_mut()), "de");
    let bytes = std::fs::read(&path).unwrap();

    let b = localized_root("language-replay", None);
    let trace = replay_hashes_with(recording(&b), &bytes, b.join("content")).unwrap();
    for expected in &live {
        assert!(trace.contains(expected), "{expected:?} not in {trace:?}");
    }
    let mut replayed = replay_with(recording(&b), &bytes, b.join("content"), |_, _| {
        Ok(std::ops::ControlFlow::Continue(()))
    })
    .unwrap();
    assert_eq!(localization::locale(replayed.world_mut()), "de");
    assert_eq!(
        replayed.world().resource::<SettingsView>(),
        engine.world().resource::<SettingsView>()
    );

    // A machine in another language reproduces it too.
    let c = localized_root(
        "language-replay-de",
        Some("format: 1\nkind: settings\ntext:\n  language: de\n"),
    );
    assert_eq!(
        replay_hashes_with(recording(&c), &bytes, c.join("content")).unwrap(),
        trace
    );
}

#[test]
fn in_place_playback_keeps_this_machines_language_out_of_the_replay() {
    let root = localized_root("language-playback", None);
    let mut engine = localized(&root)
        .persist_profile()
        .run_recordings(RunRecordingConfig::default())
        .replay_playback(ReplayPlaybackConfig::default())
        .build()
        .unwrap();
    engine.load_inline_scene(KEYED_SCENE).unwrap();
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

    requests(&mut engine).request_language("fr");
    engine.advance_live();
    assert_eq!(localization::locale(engine.world_mut()), "fr");

    engine
        .world_mut()
        .resource_mut::<ReplayPlayback>()
        .unwrap()
        .play("p", "hub.yaml");
    engine.advance_live();
    let outcome = engine
        .world()
        .resource::<ReplayPlayback>()
        .unwrap()
        .last_outcome()
        .cloned();
    assert_eq!(engine.playing(), Some("p"), "{outcome:?}");
    assert_eq!(
        locale_states(&mut engine),
        0,
        "the recording had no language"
    );

    engine.stop_playback();
    assert_eq!(localization::locale(engine.world_mut()), "fr");
}
