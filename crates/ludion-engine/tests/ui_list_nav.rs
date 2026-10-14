//! Behavioural proof for the `UiList` cursor (`Plan/ui-widgets.md` decisions 4 and 9): a scripted
//! gamepad session driven through a game system that reads `UiNav::from_input` and calls
//! `UiList::navigate`, verified two ways — the activation actually reaches game state, and a two-run
//! `WorldHash` trace (the menu is deterministic and does not churn the hash while it sits idle).
//!
//! A menu has no visible surface in a static frame and no engine-owned system anywhere, so this —
//! not a golden — is the feature's proof. The system here is the shape every consumer will copy:
//! read the input resource, drop the borrow, query the list mutably, act on the returned index.

use ludion_ecs::{SystemCtx, World};
use ludion_engine::components::UiList;
use ludion_engine::ui::UiNav;
use ludion_engine::{Engine, EngineBuilder};
use ludion_protocol::{Command, command};
use ludion_sim::InputState;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Where the game records what the menu chose: `-1` until an activation lands.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Choice {
    activated: i32,
}

/// The one system: this frame's menu intent applied to the one live list, and the activation — a
/// return value, never a component field — written into game state.
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

    // Write only on change: an idle menu touches nothing, which is what keeps a variable-length
    // component out of the world hash's motion.
    if let Some(index) = activated {
        for (choice,) in world.query::<(&mut Choice,)>() {
            choice.activated = index as i32;
        }
    }
}

fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<Choice>("Choice"))
        .system("drive_menu", drive_menu)
}

/// A three-item menu, every item selectable, plus the slot the choice lands in.
const SCENE: &str = concat!(
    "format: 1\n",
    "seed: 1\n",
    "entities:\n",
    "  - name: menu\n",
    "    components:\n",
    "      UiList:\n",
    "        anchor: center\n",
    "        title: CHOOSE\n",
    "        items:\n",
    "          - text: FIRST\n",
    "          - text: SECOND\n",
    "          - text: THIRD\n",
    "  - name: choice\n",
    "    components:\n",
    "      Choice:\n",
    "        activated: -1\n",
);

fn engine_with_menu() -> Engine {
    let mut engine = game().build().expect("the game builds");
    engine
        .load_inline_scene(SCENE)
        .expect("the menu scene loads");
    engine
}

fn pad_down(button: &str) -> Command {
    Command::InputInject(command::InputInjectParams {
        frame: None,
        event: json!({ "kind": "gamepad_button_down", "button": button }),
    })
}

fn pad_up(button: &str) -> Command {
    Command::InputInject(command::InputInjectParams {
        frame: None,
        event: json!({ "kind": "gamepad_button_up", "button": button }),
    })
}

fn step() -> Command {
    Command::SimStep(command::SimStepParams {
        ticks: 1,
        hash: false,
    })
}

/// D-pad Down, Down, then the south face button. The release between the two Downs is load-bearing:
/// navigation reads **edges**, and a re-asserted down for an already-held button is not a new press.
fn session_commands() -> Vec<Command> {
    vec![
        pad_down("dpad_down"),
        step(),
        pad_up("dpad_down"),
        step(),
        pad_down("dpad_down"),
        step(),
        pad_down("south"),
        step(),
    ]
}

fn selected(engine: &mut Engine) -> u32 {
    let mut found = None;
    for (list,) in engine.world_mut().query::<(&UiList,)>() {
        found = Some(list.selected);
    }
    found.expect("the scene has a UiList")
}

fn activated(engine: &mut Engine) -> i32 {
    let mut found = None;
    for (choice,) in engine.world_mut().query::<(&Choice,)>() {
        found = Some(choice.activated);
    }
    found.expect("the scene has a Choice")
}

/// The headless driver proof: two d-pad Downs move the cursor two items, and the south face button
/// activates that item — the index the game receives is the one under the cursor.
#[test]
fn a_scripted_gamepad_session_moves_the_cursor_and_activates() {
    let mut engine = engine_with_menu();
    assert_eq!(selected(&mut engine), 0, "the cursor starts at the top");
    assert_eq!(activated(&mut engine), -1, "nothing activated yet");

    ludion_engine::drive_hashes(&mut engine, session_commands()).expect("the session drives");

    assert_eq!(selected(&mut engine), 2, "Down twice, two items down");
    assert_eq!(
        activated(&mut engine),
        2,
        "the south button activates the item under the cursor"
    );
}

/// Two identical scripted runs produce identical per-frame `WorldHash` traces: the cursor is
/// ordinary hashed state and the whole navigation path is deterministic.
#[test]
fn two_runs_produce_identical_worldhash_traces() {
    let run = || {
        let mut engine = engine_with_menu();
        ludion_engine::drive_hashes(&mut engine, session_commands()).expect("the session drives")
    };
    let first = run();
    assert!(first.len() > 1, "the session actually advanced frames");
    assert_eq!(first, run(), "menu navigation must be deterministic");
}

/// The write-only-on-change discipline, in the hash: a menu nobody is touching contributes no
/// motion at all — every idle frame's hash is the previous frame's. This is the property a game
/// rebuilding its `items` from fresh `String`s every frame would break.
#[test]
fn an_idle_menu_leaves_the_world_hash_unmoved() {
    let mut engine = engine_with_menu();
    let trace = ludion_engine::drive_hashes(
        &mut engine,
        vec![Command::SimStep(command::SimStepParams {
            ticks: 30,
            hash: false,
        })],
    )
    .expect("idle frames drive");

    for pair in trace.windows(2) {
        assert_eq!(
            pair[0].1, pair[1].1,
            "an untouched menu must not churn the world hash: {trace:?}"
        );
    }
}
