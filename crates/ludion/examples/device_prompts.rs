//! **Prompt glyphs** — a prompt names the action on the device the player is using
//! (`ludion docs input`, "Prompt glyphs").
//!
//! [`Actions::glyphs`] reads the action's effective bindings for [`InputState::last_device`] and
//! yields atlas frame names (`pad.xbox.b`, `key.c`). The engine ships names only: a game draws
//! each one as `UiRect { atlas, frame }` from its own atlas. This one keeps text labels, so it
//! needs no art.
//!
//! The headless `main` writes the action map to a temp content root, connects an Xbox pad,
//! presses its `East` button, then presses `C`, and prints the prompt after each:
//! `pad.xbox.b crouch`, then `key.c crouch`.
//!
//! ```text
//! cargo run -p ludion --example device_prompts
//! ```

use ludion::prelude::*;
use serde_json::{Value, json};
use std::path::Path;
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

const CROUCH: ActionName = ActionName::new("crouch");

/// The prompt on screen. A resource: it is derived from sim state, so it is not hashed.
struct Prompt(String);

/// Names `crouch` on the last-touched device. A game with an atlas spawns one
/// `UiRect { atlas, frame: glyph.name }` per glyph instead of joining the names.
fn crouch_prompt(world: &mut World, _ctx: &SystemCtx) {
    let (Some(input), Some(actions)) =
        (world.resource::<InputState>(), world.resource::<Actions>())
    else {
        return;
    };
    let names: Vec<&str> = actions
        .glyphs(CROUCH, input.last_device())
        .map(|glyph| glyph.name)
        .collect();
    let prompt = Prompt(format!("{} crouch", names.join(" ")));
    world.insert_resource(prompt);
}

/// The game: the action map, the prompt and its system. A real game's `main` is just
/// `ludion::run(game())`.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .system("crouch_prompt", crouch_prompt)
        .input_actions()
}

/// The committed action map (`input/actions.yaml` in a real game's content root).
const ACTIONS: &str = "\
format: 1
kind: input-actions
actions:
  crouch:
    kind: button
    bindings:
      - key: C
        gesture: tap
      - pad: east
        gesture: tap
  takedown:
    kind: button
    bindings:
      - key: E
        gesture: tap
      - key: E
        gesture: hold
      - pad: south
        gesture: tap
      - pad: south
        gesture: hold
";

// --- Headless demo `main` ------------------------------------------------------------------------

fn main() -> ExitCode {
    let root = std::env::temp_dir().join(format!(
        "ludion-device-prompts-example-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let result = demo(&root);
    let _ = std::fs::remove_dir_all(&root);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn demo(root: &Path) -> Result<(), String> {
    let dir = root.join("input");
    std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(dir.join("actions.yaml"), ACTIONS))
        .map_err(|error| format!("could not write the demo action map: {error}"))?;
    let mut engine = game()
        .project_root(root)
        .build()
        .map_err(|diagnostic| diagnostic.to_string())?;
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .map_err(|diagnostics| format!("{diagnostics:?}"))?;

    let pad = [
        json!({ "kind": "gamepad_connected", "family": "xbox" }),
        json!({ "kind": "gamepad_button_down", "button": "east" }),
    ];
    println!("{}", prompt_after(&mut engine, &pad)?);

    let keyboard = [
        json!({ "kind": "gamepad_button_up", "button": "east" }),
        json!({ "kind": "key_down", "key": Key::C }),
    ];
    println!("{}", prompt_after(&mut engine, &keyboard)?);
    Ok(())
}

/// Injects the events, advances one tick and returns the prompt.
fn prompt_after(engine: &mut Engine, events: &[Value]) -> Result<String, String> {
    for event in events {
        engine
            .execute(Command::InputInject(command::InputInjectParams {
                frame: None,
                event: event.clone(),
            }))
            .map_err(|diagnostic| diagnostic.to_string())?;
    }
    engine.advance_live();
    engine
        .world()
        .resource::<Prompt>()
        .map(|prompt| prompt.0.clone())
        .ok_or_else(|| "the prompt resource is missing".to_owned())
}
