//! A data-driven action map: `crouch` and `prone` bound to a key and a pad button by gesture, and a
//! `move` axis from WASD and the left stick — the copyable pattern for `ludion docs input`.
//!
//! A game opts in with [`EngineBuilder::input_actions`]; the engine loads the one
//! `kind: input-actions` file under the content root, validates it, and keeps an [`Actions`]
//! resource a system reads by [`ActionName`]. Here `stance` turns a tap on `east` (or `C`) into a
//! crouch and a hold on `east` (or `Z`) into prone — a hashed `Stance` component, so the gestures
//! reach the `WorldHash`.
//!
//! Everything above the `main` divider is copyable into a real game. Run it headless (writes the
//! action map to a temp content root, scripts a tap and a hold, prints each stance change):
//!
//! ```text
//! cargo run -p ludion --example input_actions
//! ```

use ludion::prelude::*;
use std::path::PathBuf;
use std::process::ExitCode;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// --- The game (copyable) -------------------------------------------------------------------------

const CROUCH: ActionName = ActionName::new("crouch");
const PRONE: ActionName = ActionName::new("prone");

/// 0 standing, 1 crouching, 2 prone. A hashed component, so the gestures are part of the run.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default)]
struct Stance {
    value: u32,
}

/// Reads the actions once per tick and writes the stance. `pressed` is the edge: a completed tap, or
/// the tick a hold is reached.
fn stance(world: &mut World, _ctx: &SystemCtx) {
    let Some(actions) = world.resource::<Actions>() else {
        return;
    };
    let next = if actions.pressed(PRONE) {
        Some(2)
    } else if actions.pressed(CROUCH) {
        Some(1)
    } else {
        None
    };
    if let Some(next) = next {
        for (s,) in world.query::<(&mut Stance,)>() {
            s.value = next;
        }
    }
}

/// The game: its component, its system, and the one opt-in line.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<Stance>("Stance"))
        .system("stance", stance)
        .input_actions()
}

const SCENE: &str =
    "format: 1\nentities:\n  - name: hero\n    components:\n      Stance:\n        value: 0\n";

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
  prone:
    kind: button
    bindings:
      - key: Z
        gesture: hold
      - pad: east
        gesture: hold
  move:
    kind: axis2
    bindings:
      - keys:
          up: W
          down: S
          left: A
          right: D
      - stick: left
";

// --- Headless demo `main` ------------------------------------------------------------------------

fn main() -> ExitCode {
    let root = std::env::temp_dir().join(format!(
        "ludion-input-actions-example-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    if let Err(error) = write_actions(&root) {
        eprintln!("could not write the demo action map: {error}");
        return ExitCode::FAILURE;
    }
    let mut engine = match game().project_root(&root).build() {
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

    // A pad tap on frame 3..6, then a hold from frame 30: the hold fires 20 ticks after the press.
    let script = [
        (3, "gamepad_button_down", "east"),
        (6, "gamepad_button_up", "east"),
        (30, "gamepad_button_down", "east"),
        (70, "gamepad_button_up", "east"),
    ];
    for (frame, kind, button) in script {
        let event = serde_json::json!({ "kind": kind, "button": button });
        let params = command::InputInjectParams {
            frame: Some(FrameIndex::new(frame)),
            event,
        };
        if let Err(diagnostic) = engine.execute(Command::InputInject(params)) {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    }

    let mut last = 0;
    for frame in 0..80 {
        engine.advance_live();
        let now = current_stance(&engine);
        if now != last {
            let name = ["standing", "crouching", "prone"][now as usize];
            println!("frame {frame:3}  {name}");
            last = now;
        }
    }
    ExitCode::SUCCESS
}

fn write_actions(root: &std::path::Path) -> std::io::Result<()> {
    let dir: PathBuf = root.join("input");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("actions.yaml"), ACTIONS)
}

fn current_stance(engine: &Engine) -> u32 {
    let world = engine.world();
    world
        .single::<Stance>()
        .and_then(|hero| world.get::<Stance>(hero).unwrap())
        .map_or(0, |s| s.value)
}
