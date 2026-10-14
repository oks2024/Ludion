//! **World-space debug draw** — a game system draws a capsule and a ray under a named category, and
//! the host chooses to see them.
//!
//! **The pen always records; the host filters.** `debug_draw(world, "motor")` costs a few pushes and
//! never reads the host's choice, so no system can branch on whether anything is being shown. Which
//! categories reach the frame is [`Engine::set_debug_draw`] — host render state, never hashed,
//! snapshotted or recorded. The frame shows what the systems drew on the **last completed tick**.
//!
//! Everything above the `main` divider is copyable into a real game — `game()` and `draw_motor` are
//! the whole pattern.
//!
//! Run it headless (renders a PNG to a temp dir when a GPU is available):
//!
//! ```text
//! cargo run -p ludion --example debug_draw
//! ```

use ludion::prelude::*;
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// A character-shaped capsule standing at the origin and a ray along its facing, drawn every tick
/// under the `motor` category. Colors are linear RGBA.
fn draw_motor(world: &mut World, _ctx: &SystemCtx) {
    debug_draw(world, "motor")
        .capsule([0.0, 0.5, 0.0], [0.0, 1.5, 0.0], 0.5, [0.1, 1.0, 0.3, 1.0])
        .ray([0.0, 1.0, 0.0], [2.0, 0.0, 0.0], [1.0, 0.8, 0.1, 1.0]);
}

/// The game: one system. A real game's `main` is `ludion::run(game())`; pass `--render-out frame.png
/// --debug-draw motor` to it to see the lines.
pub fn game() -> EngineBuilder {
    Engine::builder().system("draw_motor", draw_motor)
}

/// A camera looking at the origin. The scene draws nothing itself: every line comes from the system.
const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 2.0
          - 8.0
      Camera: {}
";

// --- Headless demo `main` ------------------------------------------------------------------------

/// Ticks once, then renders the frame with and without the `motor` category enabled.
fn main() -> ExitCode {
    let mut engine = match game().build() {
        Ok(engine) => engine,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(diagnostics) = engine.load_inline_scene(SCENE) {
        for diagnostic in diagnostics {
            eprintln!("{diagnostic}");
        }
        return ExitCode::FAILURE;
    }
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .expect("the sim steps");

    // The pen recorded whether or not anything is shown.
    println!("drawn under: {:?}", engine.debug_draw_drawn());

    let plain = match engine.render_png(320, 180) {
        Ok(png) => png,
        Err(diagnostic) if diagnostic.code.as_str() == "L2003" => {
            println!("no GPU adapter (L2003): skipping the render");
            return ExitCode::SUCCESS;
        }
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    engine.set_debug_draw(DebugCategories::parse("motor"));
    let drawn = engine
        .render_png(320, 180)
        .expect("the second render works");
    if plain == drawn {
        eprintln!("enabling `motor` changed no pixel");
        return ExitCode::FAILURE;
    }

    let out = std::env::temp_dir().join(format!(
        "ludion-debug-draw-example-{}.png",
        std::process::id()
    ));
    if let Err(error) = std::fs::write(&out, &drawn) {
        eprintln!("could not write `{}`: {error}", out.display());
        return ExitCode::FAILURE;
    }
    println!("wrote {} with the motor category shown", out.display());
    ExitCode::SUCCESS
}
