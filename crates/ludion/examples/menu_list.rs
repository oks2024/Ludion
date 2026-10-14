//! A three-item menu driven by scripted input — the copyable pattern for every choice screen
//! (`Plan/ui-widgets.md` Wave 1: shops, hub screens, a pre-run loadout pick) and the runnable
//! proof of the [`UiList`] facade surface.
//!
//! This is the whole idiom `Plan/ui-widgets.md` decision 4 describes: a game system reads
//! [`InputState`], builds this frame's intent with [`UiNav::from_input`], drops the input borrow,
//! then queries `&mut UiList` and calls [`UiList::navigate`] — which moves the cursor and returns
//! the activated index as a plain value, never a stale field to clear. The menu below has one
//! disabled item (`CONTINUE` — no save yet), so the scripted session also exercises decision 9's two
//! navigation rules in two keypresses: the first `Down` skips over the disabled item, and the second
//! wraps past the last item back to the first.
//!
//! Everything above the `main` divider is copyable into a real game as-is — `game()`, `Choice`, and
//! `drive_menu` are the whole pattern. `Choice` is where a real game would branch (start a run, load
//! a save, quit); here it just records the index `main` prints.
//!
//! Run it headless (drives three scripted ticks of keyboard input and prints the activated index):
//!
//! ```text
//! cargo run -p ludion --example menu_list
//! ```

use ludion::prelude::*;
use std::process::ExitCode;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

// --- The game (copyable) -------------------------------------------------------------------------

/// Where the game records what the menu chose: `-1` until an activation lands. A real game would
/// act on the index directly instead of storing it (start a run, load a save, quit).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Choice {
    activated: i32,
}

/// The one system every `UiList` consumer copies: this frame's menu intent applied to the one live
/// list, and the activation — a return value, never a component field — written into game state.
fn drive_menu(world: &mut World, _ctx: &SystemCtx) {
    let nav = {
        let input = world
            .resource::<InputState>()
            .expect("the driver seeds an InputState resource");
        UiNav::from_input(input)
    };

    let mut activated = None;
    for (list,) in world.query::<(&mut UiList,)>() {
        activated = list.navigate(nav);
    }

    if let Some(index) = activated {
        for (choice,) in world.query::<(&mut Choice,)>() {
            choice.activated = index as i32;
        }
    }
}

/// The game: the `Choice` component plus the one navigation system. A real game's `main` is just
/// `ludion::run(game())`.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<Choice>("Choice"))
        .system("drive_menu", drive_menu)
}

/// A three-item main menu — `CONTINUE` disabled (no save yet) — plus the slot the choice lands in.
const SCENE: &str = concat!(
    "format: 1\n",
    "seed: 1\n",
    "entities:\n",
    "  - name: menu\n",
    "    components:\n",
    "      UiList:\n",
    "        anchor: center\n",
    "        title: MAIN MENU\n",
    "        items:\n",
    "          - text: NEW GAME\n",
    "          - text: CONTINUE\n",
    "            enabled: false\n",
    "          - text: QUIT\n",
    "  - name: choice\n",
    "    components:\n",
    "      Choice:\n",
    "        activated: -1\n",
);

// --- Headless demo `main` ------------------------------------------------------------------------

/// Scripts `Down`, `Down`, `Enter` and reports the resulting cursor and activation. No window, no
/// wire client, deterministic. (A real game's `main` is just `ludion::run(game())`.)
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

    for command in session_commands() {
        engine.execute(command).expect("the session drives");
    }

    println!(
        "cursor landed on item {} ; activated index {}",
        selected(&mut engine),
        activated(&mut engine)
    );

    ExitCode::SUCCESS
}

/// `Down`, release, `Down`, `Enter` — the release between the two `Down`s is load-bearing:
/// `UiNav::from_input` reads **edges**, so a re-asserted down for an already-held key is not a new
/// step. The first `Down` skips the disabled `CONTINUE` and lands on `QUIT`; the second wraps past
/// the end back to `NEW GAME`, which `Enter` then activates.
fn session_commands() -> Vec<Command> {
    vec![
        key_down(Key::Down),
        step(),
        key_up(Key::Down),
        step(),
        key_down(Key::Down),
        step(),
        key_down(Key::Enter),
        step(),
    ]
}

fn key_down(key: Key) -> Command {
    Command::InputInject(command::InputInjectParams {
        frame: None,
        event: json!({ "kind": "key_down", "key": key }),
    })
}

fn key_up(key: Key) -> Command {
    Command::InputInject(command::InputInjectParams {
        frame: None,
        event: json!({ "kind": "key_up", "key": key }),
    })
}

fn step() -> Command {
    Command::SimStep(command::SimStepParams {
        ticks: 1,
        hash: false,
    })
}

/// The menu's current cursor.
fn selected(engine: &mut Engine) -> u32 {
    engine
        .world_mut()
        .query::<(&UiList,)>()
        .next()
        .map(|(list,)| list.selected)
        .expect("the scene has a UiList")
}

/// The recorded activation (`-1` if nothing has activated yet).
fn activated(engine: &mut Engine) -> i32 {
    engine
        .world_mut()
        .query::<(&Choice,)>()
        .next()
        .map(|(choice,)| choice.activated)
        .expect("the scene has a Choice")
}
