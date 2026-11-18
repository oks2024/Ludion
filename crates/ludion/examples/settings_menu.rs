//! A player **settings menu** — the copyable options-screen pattern.
//!
//! A game's menu system writes the [`Settings`] resource (volumes, quality, anti-aliasing, window
//! mode, `save()`) and reads the [`SettingsView`] back. With `persist_settings()` the engine keeps
//! the choices in `save/settings.yaml`, so a second boot starts where the player left off.
//!
//! Everything above the `main` divider is copyable into a real game. The headless `main` boots twice
//! on one scratch save directory: the first boot's menu sets music to 60%, the `medium` preset and
//! SMAA and saves; the second boot reads them back.
//!
//! ```text
//! cargo run -p ludion --example settings_menu
//! ```

use ludion::prelude::*;
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// The options menu: on the first tick, set what the player picked and save once. A real menu does
/// this from its input handling, one request per changed row.
fn options_menu(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 0 {
        return;
    }
    let Some(settings) = world.resource_mut::<Settings>() else {
        return;
    };
    settings.request_bus_volume(Bus::Music, 0.6);
    settings.request_quality(QualityPreset::Medium);
    settings.request_anti_aliasing(AntiAliasing::Smaa);
    settings.save();
}

/// The game: one system. A real game's `main` is `ludion::run(game())`.
pub fn game() -> EngineBuilder {
    Engine::builder().system("options_menu", options_menu)
}

// --- Headless demo `main` ------------------------------------------------------------------------

fn describe(engine: &Engine) -> String {
    let view = engine
        .world()
        .resource::<SettingsView>()
        .expect("the engine installs the view");
    format!(
        "music {:.1}, quality {:?}, anti-aliasing {:?}",
        view.volume(Bus::Music),
        view.quality(),
        view.anti_aliasing()
    )
}

fn main() -> ExitCode {
    let root = std::env::temp_dir().join(format!("ludion-settings-menu-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);

    let boot = |menu: bool| {
        let builder = if menu { game() } else { Engine::builder() };
        builder.save_root(&root).persist_settings().build()
    };

    let mut first = match boot(true) {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    println!("first boot:  {}", describe(&first));
    first.advance_live();
    println!("after menu:  {}", describe(&first));

    let second = match boot(false) {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    println!("second boot: {}", describe(&second));
    let kept = second.anti_aliasing() == AntiAliasing::Smaa && second.quality().label == "medium";

    let _ = std::fs::remove_dir_all(&root);
    if kept {
        ExitCode::SUCCESS
    } else {
        eprintln!("the second boot did not read the saved settings back");
        ExitCode::FAILURE
    }
}
