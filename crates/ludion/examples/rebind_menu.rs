//! A **controls screen** that rebinds actions and persists them — the copyable pattern for a
//! key-rebinding menu (`Plan/input-actions.md`, `ludion docs input`).
//!
//! A game declares its actions in a `kind: input-actions` document and reads them through
//! [`Actions`]. A controls screen is a [`UiList`] with one row per rebindable action: activating a row
//! calls [`Actions::capture`], which takes the player's next keyboard press as the action's new
//! binding; once it lands the screen calls [`Actions::save`], and the engine writes the overrides to
//! `save/input-bindings.yaml` at the frame boundary. The next boot reads the file back, so the
//! rebind outlives the process. The menu itself navigates through [`UiNav::from_actions`], so the
//! player can rebind the menu keys too.
//!
//! Everything above the `main` divider is copyable into a real game as-is — `game()`, `Controls` and
//! `controls_screen` are the whole pattern. A real game's `main` is `ludion::run(game())`, which
//! enables the file (`persist_input_bindings()`); a replay, `--check-docs` or a test leaves it off, and
//! a recording carries the overrides it started with, so a replay never depends on this machine's file.
//!
//! The headless `main` plays **two sessions on one save directory**: the first rebinds `scan` to
//! `F`, the second boots, loads the file and shows the row reading `F`.
//!
//! ```text
//! cargo run -p ludion --example rebind_menu
//! ```

use ludion::prelude::*;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

// --- The game (copyable) -------------------------------------------------------------------------

/// The rebindable actions, one controls-screen row each, in row order.
const ROWS: [(&str, &str); 2] = [("scan", "SCAN"), ("jump", "JUMP")];

/// The game's actions: the menu keys (`ui_*`) and two gameplay actions with default bindings.
const ACTIONS: &str = "\
format: 1
kind: input-actions
actions:
  ui_prev:
    kind: button
    bindings:
      - key: Up
  ui_next:
    kind: button
    bindings:
      - key: Down
  ui_activate:
    kind: button
    bindings:
      - key: Enter
  scan:
    kind: button
    bindings:
      - key: M
  jump:
    kind: button
    bindings:
      - key: Space
";

/// The controls screen's own state: whether the last frame's capture was already bound, so the
/// screen saves once per rebind, not every frame the result stays `Bound`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
#[serde(default)]
struct Controls {
    was_bound: bool,
}

/// The whole pattern in one system: navigate the rows, start a capture on the activated one, show
/// each row's current binding, and save once a capture is bound.
fn controls_screen(world: &mut World, _ctx: &SystemCtx) {
    let nav = {
        let (Some(actions), Some(input)) =
            (world.resource::<Actions>(), world.resource::<InputState>())
        else {
            return;
        };
        UiNav::from_actions(actions, input)
    };
    let mut activated = None;
    for (list,) in world.query::<(&mut UiList,)>() {
        activated = list.navigate(nav);
    }
    let Some(actions) = world.resource_mut::<Actions>() else {
        return;
    };
    if let Some(index) = activated
        && let Some((name, _)) = ROWS.get(index as usize)
    {
        let _ = actions.capture(name, CaptureSlot::Keyboard, CaptureOptions::default());
    }
    let bound = *actions.capture_result() == CaptureResult::Bound;
    let rows: Vec<String> = ROWS
        .iter()
        .map(|(name, label)| {
            let key = actions
                .bindings(name)
                .and_then(|bindings| bindings.iter().find_map(|binding| binding.key));
            match key {
                Some(key) => format!("{label}  [{key:?}]"),
                None => format!("{label}  [unbound]"),
            }
        })
        .collect();
    let mut newly_bound = false;
    for (controls,) in world.query::<(&mut Controls,)>() {
        newly_bound = bound && !controls.was_bound;
        controls.was_bound = bound;
    }
    if newly_bound && let Some(actions) = world.resource_mut::<Actions>() {
        actions.save();
    }
    for (list,) in world.query::<(&mut UiList,)>() {
        for (item, row) in list.items.iter_mut().zip(rows.iter()) {
            item.text.clone_from(row);
        }
    }
}

/// The game: the action map, the controls component and the one screen system. A real game's `main`
/// is `ludion::run(game())`; `main` below adds a scratch save directory so it can play two sessions.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<Controls>("Controls"))
        .system("controls_screen", controls_screen)
        .input_actions()
        .doc_text("input/actions.yaml", ACTIONS)
}

/// A two-row controls list and the screen's state.
const SCENE: &str = concat!(
    "format: 1\n",
    "seed: 1\n",
    "entities:\n",
    "  - name: controls\n",
    "    components:\n",
    "      UiList:\n",
    "        anchor: center\n",
    "        title: CONTROLS\n",
    "        items:\n",
    "          - text: SCAN\n",
    "          - text: JUMP\n",
    "      Controls: {}\n",
);

// --- Headless demo `main` ------------------------------------------------------------------------

/// Plays two sessions on one save directory: session 1 rebinds `scan` to `F` and saves, session 2 is
/// a "process restart" that loads the file. Deterministic, no window, no wire client.
fn main() -> ExitCode {
    let root =
        std::env::temp_dir().join(format!("ludion-rebind-menu-example-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let result = play(&root);
    let _ = std::fs::remove_dir_all(&root);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn play(root: &Path) -> Result<(), String> {
    let mut first = boot(root)?;
    first.execute(step()).map_err(|error| error.to_string())?;
    println!("session 1 boots with:  {}", rows(&mut first).join(" | "));
    // Activate the SCAN row, release Enter, then press the new key.
    for command in [
        key_down(Key::Enter),
        step(),
        key_up(Key::Enter),
        step(),
        key_down(Key::F),
        step(),
        key_up(Key::F),
        step(),
    ] {
        first.execute(command).map_err(|error| error.to_string())?;
    }
    println!("session 1 rebinds to:  {}", rows(&mut first).join(" | "));
    let file = save_file(root);
    let text = std::fs::read_to_string(&file)
        .map_err(|error| format!("the rebind was not saved to {}: {error}", file.display()))?;
    println!(
        "
save/input-bindings.yaml:
{text}"
    );

    let mut second = boot(root)?;
    second.execute(step()).map_err(|error| error.to_string())?;
    let shown = rows(&mut second);
    println!("session 2 boots with:  {}", shown.join(" | "));
    if shown[0] != "SCAN  [F]" {
        return Err(format!("the saved rebind did not load: {shown:?}"));
    }
    Ok(())
}

/// Builds an engine on `root` with the file on — the one call `ludion::run` makes for a live host.
fn boot(root: &Path) -> Result<Engine, String> {
    let mut engine = game()
        .project_root(root)
        .save_root(root)
        .persist_input_bindings()
        .build()
        .map_err(|diagnostic| diagnostic.to_string())?;
    engine.load_inline_scene(SCENE).map_err(|diagnostics| {
        diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    Ok(engine)
}

fn save_file(root: &Path) -> PathBuf {
    root.join("save").join("input-bindings.yaml")
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

/// The list's row texts.
fn rows(engine: &mut Engine) -> Vec<String> {
    engine
        .world_mut()
        .query::<(&UiList,)>()
        .next()
        .map(|(list,)| list.items.iter().map(|item| item.text.clone()).collect())
        .expect("the scene has a UiList")
}
