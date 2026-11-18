//! The player settings: requests apply at the frame boundary, persist only under
//! `persist_settings()`, survive rebuilds and never move a `WorldHash` (`Plan/settings.md`).

use std::path::{Path, PathBuf};

use ludion_core::audio::{AudioMessage, Bus, MixLayer};
use ludion_core::time::FrameIndex;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    AntiAliasing, Engine, EngineBuilder, QualityPreset, Settings, SettingsView, WindowMode,
};

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
