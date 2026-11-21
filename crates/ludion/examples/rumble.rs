//! **Gamepad rumble** — a system asks for a force-feedback effect through [`Rumble`]
//! (`Plan/input-actions.md`, `ludion docs input`).
//!
//! The request is host feel, not sim state: the engine validates it at the frame boundary and sends
//! it to the window host's pad thread, which mixes overlapping effects per motor and stops them on
//! focus loss or disconnect. Headless (and in the out-of-process viewer) nothing is listening and the
//! request is a silent no-op, so a game never branches on whether a pad exists.
//!
//! On macOS a pad only rumbles with the `apple-rumble` feature, which drives GameController haptics
//! (without it the pad is silent; on Windows the feature is inert):
//!
//! ```toml
//! ludion = { version = "0.1", features = ["apple-rumble"] }
//! ```
//!
//! The headless `main` wires its own receiver in place of the pad thread, presses `Space`, advances
//! one tick and prints the command that arrived. A real game's `main` is `ludion::run(game())`.
//!
//! ```text
//! cargo run -p ludion --example rumble
//! ```

use ludion::prelude::*;
use serde_json::json;
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// A short thump on every `Space` press: strong motor hard, weak motor light, for 0.18 s.
fn thump_on_space(world: &mut World, _ctx: &SystemCtx) {
    let pressed = world
        .resource::<InputState>()
        .is_some_and(|input| input.was_pressed(Key::Space));
    if pressed && let Some(rumble) = world.resource_mut::<Rumble>() {
        rumble.play(RumbleEffect {
            low: 0.8,
            high: 0.3,
            duration: 0.18,
        });
    }
}

/// The game: one system. A real game's `main` is just `ludion::run(game())`.
pub fn game() -> EngineBuilder {
    Engine::builder().system("thump_on_space", thump_on_space)
}

// --- Headless demo `main` ------------------------------------------------------------------------

fn main() -> ExitCode {
    let mut engine = match game().build() {
        Ok(engine) => engine,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    let (tx, rx) = crossbeam_channel::unbounded();
    engine.set_rumble_sender(tx);

    engine
        .execute(Command::InputInject(command::InputInjectParams {
            frame: None,
            event: json!({ "kind": "key_down", "key": Key::Space }),
        }))
        .expect("the press injects");
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .expect("the sim steps");

    match rx.try_recv() {
        Ok(command) => {
            println!("the pad thread would receive: {command:?}");
            ExitCode::SUCCESS
        }
        Err(_) => {
            eprintln!("no rumble command arrived");
            ExitCode::FAILURE
        }
    }
}
