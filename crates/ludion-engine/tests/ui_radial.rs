//! Behavioural proof for `UiRadial` (`Plan/ui-hud.md` decisions 37-44, Neon Shroud request 0020 W6a): a
//! scripted stick sweep driven through a game system that reads the left stick and the south button and
//! calls `UiRadial::navigate`. Verified four ways: the selection and activation reach game state; two
//! runs, 1, 2 and 8 threads, and a snapshot restore all give one `WorldHash` trace; an idle wheel leaves
//! the hash unmoved; and extract draws the labels where a scaled group puts them, in the right colours.
//!
//! The system is the shape every consumer copies: read the input, drop the borrow, query the wheel
//! mutably, act on the returned event.

use ludion_ecs::{SystemCtx, World};
use ludion_engine::components::{Anchor, UiGroup, UiRadial, UiRadialItem, UiRadialNav};
use ludion_engine::{Engine, EngineBuilder, RadialEvent};
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody, command};
use ludion_renderer::UiInstance;
use ludion_sim::{GamepadButton, InputState, WorldHash};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// What the game recorded: `-1` until an activation lands.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Choice {
    activated: i32,
}

fn drive_wheel(world: &mut World, _ctx: &SystemCtx) {
    let nav = {
        let input = world
            .resource::<InputState>()
            .expect("the driver seeds an InputState resource");
        let pad = input.gamepad();
        UiRadialNav {
            activate: pad.was_pressed(GamepadButton::South),
            ..UiRadialNav::from_stick(pad.left_stick())
        }
    };
    let mut event = None;
    for (wheel,) in world.query::<(&mut UiRadial,)>() {
        event = wheel.navigate(nav);
    }
    if let Some(RadialEvent::Activated(index)) = event {
        for (choice,) in world.query::<(&mut Choice,)>() {
            choice.activated = index as i32;
        }
    }
}

fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<Choice>("Choice"))
        .system("drive_wheel", drive_wheel)
}

/// A four-item wheel, item 3 disabled, plus the slot the choice lands in.
const SCENE: &str = concat!(
    "format: 1\n",
    "seed: 1\n",
    "entities:\n",
    "  - name: wheel\n",
    "    components:\n",
    "      UiRadial:\n",
    "        anchor: center\n",
    "        items:\n",
    "          - text: AXE\n",
    "          - text: BOW\n",
    "          - text: SPEAR\n",
    "          - text: SHIELD\n",
    "            enabled: false\n",
    "  - name: choice\n",
    "    components:\n",
    "      Choice:\n",
    "        activated: -1\n",
);

fn engine_with_wheel() -> Engine {
    let mut engine = game().build().expect("the game builds");
    engine
        .load_inline_scene(SCENE)
        .expect("the wheel scene loads");
    engine
}

fn axis(name: &str, value: f32) -> Command {
    Command::InputInject(command::InputInjectParams {
        frame: None,
        event: json!({ "kind": "gamepad_axis", "axis": name, "value_bits": value.to_bits() }),
    })
}

fn stick(x: f32, y: f32) -> Vec<Command> {
    vec![axis("left_stick_x", x), axis("left_stick_y", y), step()]
}

fn south(down: bool) -> Vec<Command> {
    let kind = if down {
        "gamepad_button_down"
    } else {
        "gamepad_button_up"
    };
    vec![
        Command::InputInject(command::InputInjectParams {
            frame: None,
            event: json!({ "kind": kind, "button": "south" }),
        }),
        step(),
    ]
}

fn step() -> Command {
    Command::SimStep(SimStepParams {
        ticks: 1,
        hash: false,
    })
}

/// Stick up (item 0), right (item 1), down (item 2), then the south button.
fn first_half() -> Vec<Command> {
    [stick(0.0, 1.0), stick(1.0, 0.0)].concat()
}

fn second_half() -> Vec<Command> {
    [stick(0.0, -1.0), south(true), south(false)].concat()
}

fn selected(engine: &mut Engine) -> Option<u32> {
    let mut found = None;
    for (wheel,) in engine.world_mut().query::<(&UiRadial,)>() {
        found = Some(wheel.selected);
    }
    found.expect("the scene has a UiRadial")
}

fn activated(engine: &mut Engine) -> i32 {
    let mut found = None;
    for (choice,) in engine.world_mut().query::<(&Choice,)>() {
        found = Some(choice.activated);
    }
    found.expect("the scene has a Choice")
}

fn full_session() -> Vec<Command> {
    [first_half(), second_half()].concat()
}

fn trace_of(commands: Vec<Command>) -> Vec<(ludion_core::time::FrameIndex, WorldHash)> {
    let mut engine = engine_with_wheel();
    ludion_engine::drive_hashes(&mut engine, commands).expect("the session drives")
}

#[test]
fn a_stick_sweep_passes_three_items_then_activates() {
    let mut engine = engine_with_wheel();
    assert_eq!(selected(&mut engine), None);
    ludion_engine::drive_hashes(&mut engine, stick(0.0, 1.0)).unwrap();
    assert_eq!(selected(&mut engine), Some(0));
    ludion_engine::drive_hashes(&mut engine, stick(1.0, 0.0)).unwrap();
    assert_eq!(selected(&mut engine), Some(1));
    ludion_engine::drive_hashes(&mut engine, stick(0.0, -1.0)).unwrap();
    assert_eq!(selected(&mut engine), Some(2));
    assert_eq!(activated(&mut engine), -1, "nothing activated yet");
    ludion_engine::drive_hashes(&mut engine, south(true)).unwrap();
    assert_eq!(activated(&mut engine), 2);
    assert_eq!(selected(&mut engine), Some(2), "selected is kept");
    // The disabled item is never taken by aim.
    ludion_engine::drive_hashes(&mut engine, stick(-1.0, 0.0)).unwrap();
    assert_eq!(selected(&mut engine), Some(2));
}

#[test]
fn two_runs_produce_identical_worldhash_traces() {
    let first = trace_of(full_session());
    assert!(first.len() > 1, "the session actually advanced frames");
    assert_eq!(first, trace_of(full_session()));
}

#[test]
fn the_trace_is_identical_at_1_2_and_8_threads() {
    let runs: Vec<_> = [1, 2, 8]
        .into_iter()
        .map(|n| ludion_core::par::with_threads(n, || trace_of(full_session())))
        .collect();
    assert_eq!(runs[0], runs[1], "1 vs 2 threads");
    assert_eq!(runs[0], runs[2], "1 vs 8 threads");
}

#[test]
fn a_snapshot_mid_sweep_replays_the_same_trace_tail() {
    let mut engine = engine_with_wheel();
    ludion_engine::drive_hashes(&mut engine, first_half()).unwrap();
    let snapshot = match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    let tail = ludion_engine::drive_hashes(&mut engine, second_half()).unwrap();
    assert_eq!(activated(&mut engine), 2);
    match engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    assert_eq!(activated(&mut engine), -1, "the restore rewinds the choice");
    assert_eq!(selected(&mut engine), Some(1), "and the selection");
    let replayed = ludion_engine::drive_hashes(&mut engine, second_half()).unwrap();
    assert_eq!(tail, replayed, "the tail after a restore diverged");
}

#[test]
fn an_idle_wheel_leaves_the_world_hash_unmoved() {
    let mut engine = engine_with_wheel();
    let trace = ludion_engine::drive_hashes(
        &mut engine,
        vec![Command::SimStep(command::SimStepParams {
            ticks: 30,
            hash: false,
        })],
    )
    .expect("idle frames drive");
    for pair in trace.windows(2) {
        assert_eq!(pair[0].1, pair[1].1, "an idle wheel churned the hash");
    }
}

#[test]
fn extract_draws_a_wheel_in_a_scaled_group_at_the_composed_label_points() {
    let mut engine = Engine::builder().build().expect("the engine builds");
    engine
        .load_inline_scene("format: 1\nseed: 3\nentities:\n  - name: cam\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - 0.0\n          - 10.0\n      Camera: {}\n")
        .expect("the scene loads");
    let group = engine.world_mut().spawn_presentation();
    engine
        .world_mut()
        .insert(
            group,
            UiGroup {
                anchor: Anchor::TopRight,
                offset: [-24.0, 24.0],
                size: [600.0, 600.0],
                scale: 0.5,
                ..UiGroup::default()
            },
        )
        .unwrap();
    let item = |text: &str, enabled: bool| UiRadialItem {
        text: text.into(),
        enabled,
        ..UiRadialItem::default()
    };
    let wheel = engine.world_mut().spawn_presentation();
    engine
        .world_mut()
        .insert(
            wheel,
            UiRadial {
                anchor: Anchor::Center,
                group: Some(group),
                items: vec![
                    item("AA", true),
                    item("BB", true),
                    item("CC", false),
                    item("DD", true),
                ],
                selected: Some(1),
                ..UiRadial::default()
            },
        )
        .unwrap();
    let scene = engine.extract_present_scene().expect("extracts").0;
    let ps = engine.render_size().1 as f32 / 1080.0;
    let viewport = engine.render_size();
    let box_px = ludion_engine::ui::resolve_ui_rect(
        Anchor::TopRight,
        [-24.0, 24.0],
        [600.0, 600.0],
        viewport,
    );
    let centre = [box_px[0] + box_px[2] * 0.5, box_px[1] + box_px[3] * 0.5];

    let labels: Vec<_> = scene
        .ui
        .iter()
        .filter_map(|instance| match instance {
            UiInstance::Text {
                text, color, quads, ..
            } => Some((text.as_str(), *color, quads)),
            UiInstance::Rect { .. } | UiInstance::Shape { .. } | UiInstance::Polygon { .. } => None,
        })
        .collect();
    assert_eq!(labels.len(), 4);
    let defaults = UiRadial::default();
    assert_eq!(labels[0].1, defaults.color);
    assert_eq!(labels[1].1, defaults.selected_color);
    assert_eq!(labels[2].1, defaults.disabled_color);
    assert_eq!(labels[3].1, defaults.color);
    // Item 0 is 12 o'clock at the mid-radius 155; the group halves the wheel about its box centre.
    let quads = labels[0].2;
    let x = (quads[0].pos[0] + quads[quads.len() - 1].pos[2]) * 0.5;
    let y = (quads[0].pos[1] + quads[0].pos[3]) * 0.5;
    assert!((x - centre[0]).abs() < 1e-2, "x {x} vs {}", centre[0]);
    assert!(
        (y - (centre[1] - 155.0 * 0.5 * ps)).abs() < 1e-2,
        "y {y} vs {}",
        centre[1] - 155.0 * 0.5 * ps
    );
    // The glyphs are half size: a 24-unit label is 12 canvas units tall.
    assert!(((quads[0].pos[3] - quads[0].pos[1]) - 12.0 * ps).abs() < 1e-2);
}
