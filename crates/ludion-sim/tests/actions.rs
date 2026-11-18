//! The action layer: every gesture at its exact tick boundary, trigger hysteresis, axes, every
//! compile error with its pointer, and determinism through a `SimDriver` and a state round trip.
//!
//! At 60 Hz the default thresholds are `tap_max` 13 ticks, `hold` 21, `double_tap_window` 15.

use core::f32::consts::FRAC_1_SQRT_2;
use ludion_core::time::{FrameIndex, TickRate};
use ludion_ecs::{Schedule, World};
use ludion_sim::actions::{ActionMap, ActionName, Actions, InputActionsDoc};
use ludion_sim::{
    GamepadAxis, GamepadButton, InputEvent, InputScript, InputState, Key, MouseButton, SimConfig,
    SimDriver,
};
use serde_json::{Value, json};

const T: u32 = 13;
const H: u32 = 21;
const W: u32 = 15;

fn doc(value: Value) -> InputActionsDoc {
    serde_json::from_value(value).expect("the document parses")
}

fn compile(value: Value) -> Result<ActionMap, Vec<ludion_core::diagnostics::Diagnostic>> {
    ActionMap::compile(&doc(value), TickRate::DEFAULT)
}

fn button(bindings: Value) -> Value {
    json!({ "kind": "button", "bindings": bindings })
}

/// A tick-by-tick harness: `tick` is one `begin_tick`, the events, then `resolve`.
struct Rig {
    actions: Actions,
    input: InputState,
}

impl Rig {
    fn new(actions: Value) -> Self {
        let map = compile(json!({ "actions": actions })).expect("the map compiles");
        Rig {
            actions: Actions::new(map),
            input: InputState::default(),
        }
    }

    fn tick(&mut self, events: &[InputEvent]) {
        self.input.begin_tick();
        for event in events {
            self.input.apply(*event);
        }
        self.actions.resolve(&self.input);
    }

    fn idle(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.tick(&[]);
        }
    }

    fn down(&mut self, key: Key) {
        self.tick(&[InputEvent::KeyDown { key }]);
    }

    fn up(&mut self, key: Key) {
        self.tick(&[InputEvent::KeyUp { key }]);
    }
}

fn name(n: &'static str) -> ActionName {
    ActionName::new(n)
}

#[test]
fn press_fires_on_the_edge_holds_a_level_and_releases() {
    let mut rig = Rig::new(json!({ "jump": button(json!([{ "key": "Space" }])) }));
    rig.down(Key::Space);
    assert!(rig.actions.pressed(name("jump")) && rig.actions.held(name("jump")));
    rig.idle(1);
    assert!(!rig.actions.pressed(name("jump")) && rig.actions.held(name("jump")));
    rig.up(Key::Space);
    assert!(rig.actions.released(name("jump")) && !rig.actions.held(name("jump")));
}

#[test]
fn tap_fires_on_release_at_n_equal_t_and_not_at_t_plus_one() {
    for (n, taps) in [(1, true), (T, true), (T + 1, false)] {
        let mut rig =
            Rig::new(json!({ "dodge": button(json!([{ "key": "C", "gesture": "tap" }])) }));
        rig.down(Key::C); // n = 1 on this tick
        rig.idle(n - 1);
        assert!(!rig.actions.pressed(name("dodge")), "n = {n}");
        rig.up(Key::C);
        assert_eq!(rig.actions.pressed(name("dodge")), taps, "n = {n}");
        assert!(!rig.actions.held(name("dodge")), "a tap is an edge only");
    }
}

#[test]
fn hold_fires_at_exactly_h_ticks_and_stays_level_until_release() {
    let mut rig = Rig::new(json!({ "prone": button(json!([{ "key": "C", "gesture": "hold" }])) }));
    rig.down(Key::C);
    for n in 2..H {
        rig.idle(1);
        assert!(!rig.actions.pressed(name("prone")), "n = {n}");
        assert_eq!(rig.actions.held_ticks(name("prone")), n);
    }
    rig.idle(1); // n == H
    assert!(rig.actions.pressed(name("prone")) && rig.actions.held(name("prone")));
    assert_eq!(rig.actions.hold_progress(name("prone")), 1.0);
    rig.idle(1);
    assert!(!rig.actions.pressed(name("prone")) && rig.actions.held(name("prone")));
    rig.up(Key::C);
    assert!(rig.actions.released(name("prone")));
    assert_eq!(rig.actions.held_ticks(name("prone")), 0);
}

#[test]
fn hold_progress_is_the_fraction_of_the_threshold() {
    let mut rig = Rig::new(json!({ "prone": button(json!([{ "key": "C", "gesture": "hold" }])) }));
    rig.down(Key::C);
    rig.idle(9); // n = 10
    let progress = rig.actions.hold_progress(name("prone"));
    assert!((progress - 10.0 / H as f32).abs() < 1e-6, "{progress}");
}

fn tap_and_double() -> Rig {
    Rig::new(json!({
        "slide": button(json!([{ "key": "C", "gesture": "tap" }])),
        "roll": button(json!([{ "key": "C", "gesture": "double_tap" }])),
    }))
}

#[test]
fn a_deferred_tap_fires_at_up_plus_w() {
    let mut rig = tap_and_double();
    rig.down(Key::C);
    rig.up(Key::C); // up tick
    for k in 1..W {
        rig.idle(1);
        assert!(!rig.actions.pressed(name("slide")), "k = {k}");
    }
    rig.idle(1); // up + W
    assert!(rig.actions.pressed(name("slide")));
    assert!(!rig.actions.pressed(name("roll")));
    rig.idle(1);
    assert!(!rig.actions.pressed(name("slide")), "fires once");
}

#[test]
fn a_second_press_at_up_plus_w_minus_one_is_a_double_tap_and_cancels_the_tap() {
    let mut rig = tap_and_double();
    rig.down(Key::C);
    rig.up(Key::C);
    rig.idle(W - 2);
    rig.down(Key::C); // up + W - 1
    assert!(rig.actions.pressed(name("roll")) && rig.actions.held(name("roll")));
    rig.idle(1);
    assert!(rig.actions.held(name("roll")));
    rig.up(Key::C);
    assert!(rig.actions.released(name("roll")));
    for _ in 0..W + 2 {
        rig.idle(1);
        assert!(
            !rig.actions.pressed(name("slide")),
            "the cancelled tap never fires"
        );
    }
}

#[test]
fn a_second_press_at_up_plus_w_is_too_late() {
    let mut rig = tap_and_double();
    rig.down(Key::C);
    rig.up(Key::C);
    rig.idle(W - 1);
    rig.tick(&[InputEvent::KeyDown { key: Key::C }]); // up + W: the tap fires, this press is fresh
    assert!(rig.actions.pressed(name("slide")));
    assert!(!rig.actions.pressed(name("roll")));
}

#[test]
fn a_hold_cancels_the_tap_on_the_same_input() {
    let mut rig = Rig::new(json!({
        "dodge": button(json!([{ "key": "C", "gesture": "tap" }])),
        "prone": button(json!([{ "key": "C", "gesture": "hold" }])),
    }));
    rig.down(Key::C);
    rig.idle(H - 1);
    assert!(rig.actions.pressed(name("prone")));
    rig.up(Key::C);
    assert!(
        !rig.actions.pressed(name("dodge")),
        "a held press is not a tap"
    );
}

#[test]
fn press_and_hold_on_one_input_fire_in_two_actions_by_their_own_rules() {
    let mut rig = Rig::new(json!({
        "aim": button(json!([{ "mouse": "right" }])),
        "zoom": button(json!([{ "mouse": "right", "gesture": "hold" }])),
    }));
    rig.tick(&[InputEvent::MouseDown {
        button: MouseButton::Right,
    }]);
    assert!(rig.actions.pressed(name("aim")) && !rig.actions.pressed(name("zoom")));
    rig.idle(H - 1);
    assert!(rig.actions.pressed(name("zoom")) && !rig.actions.pressed(name("aim")));
    assert!(rig.actions.held(name("aim")) && rig.actions.held(name("zoom")));
}

#[test]
fn a_double_tap_press_does_not_tap_on_release() {
    let mut rig = tap_and_double();
    rig.down(Key::C);
    rig.up(Key::C);
    rig.down(Key::C); // double
    rig.up(Key::C); // would be a tap if the press were ordinary
    rig.idle(W + 2);
    assert!(!rig.actions.pressed(name("slide")));
}

fn trigger_rig() -> Rig {
    Rig::new(json!({
        "aim": button(json!([{ "trigger": "right", "press": 0.5, "release": 0.35 }])),
        "legacy": button(json!([{ "pad": "right_trigger" }])),
    }))
}

fn trigger(value: f32) -> InputEvent {
    InputEvent::GamepadAxis {
        axis: GamepadAxis::RightTrigger,
        value,
    }
}

#[test]
fn a_trigger_binding_has_hysteresis_and_the_legacy_button_does_not() {
    let mut rig = trigger_rig();
    rig.idle(1); // a trigger already past `press` on the first tick would be latched
    rig.tick(&[trigger(0.6)]);
    assert!(rig.actions.pressed(name("aim")) && rig.actions.pressed(name("legacy")));
    rig.tick(&[trigger(0.49)]);
    assert!(rig.actions.held(name("aim")), "above release: still held");
    assert!(!rig.actions.held(name("legacy")), "legacy flips at 0.5");
    rig.tick(&[trigger(0.36)]);
    assert!(rig.actions.held(name("aim")));
    rig.tick(&[trigger(0.34)]);
    assert!(rig.actions.released(name("aim")));
    rig.tick(&[trigger(0.4)]);
    assert!(
        !rig.actions.held(name("aim")),
        "re-press needs the press level"
    );
    rig.tick(&[trigger(0.5)]);
    assert!(rig.actions.pressed(name("aim")));
}

#[test]
fn a_key_held_before_the_first_resolve_is_latched_until_released() {
    let mut rig = Rig::new(json!({
        "go": button(json!([{ "key": "C" }])),
        "hop": button(json!([{ "key": "C", "gesture": "hold" }])),
    }));
    // Held across a load: the down edge came on an earlier tick, before this layer existed.
    rig.input.begin_tick();
    rig.input.apply(InputEvent::KeyDown { key: Key::C });
    rig.input.begin_tick();
    rig.actions.resolve(&rig.input);
    for _ in 0..H + 2 {
        assert!(!rig.actions.pressed(name("go")) && !rig.actions.held(name("go")));
        assert!(!rig.actions.pressed(name("hop")) && rig.actions.held_ticks(name("hop")) == 0);
        rig.tick(&[]);
    }
    rig.up(Key::C);
    assert!(
        !rig.actions.released(name("go")),
        "a latched release yields nothing"
    );
    rig.down(Key::C);
    assert!(rig.actions.pressed(name("go")), "a fresh press fires");
}

#[test]
fn a_key_pressed_on_the_first_tick_fires_normally() {
    let mut rig = Rig::new(json!({ "go": button(json!([{ "key": "C" }])) }));
    rig.down(Key::C);
    assert!(rig.actions.pressed(name("go")) && rig.actions.held(name("go")));
}

#[test]
fn a_trigger_past_press_at_creation_is_latched() {
    let mut rig = trigger_rig();
    rig.tick(&[trigger(0.9)]);
    assert!(!rig.actions.pressed(name("aim")) && !rig.actions.held(name("aim")));
    rig.tick(&[trigger(0.1)]);
    assert!(!rig.actions.released(name("aim")));
    rig.tick(&[trigger(0.6)]);
    assert!(rig.actions.pressed(name("aim")));
}

#[test]
fn wheel_steps_are_one_tick_buttons() {
    let mut rig = Rig::new(json!({ "next": button(json!([{ "wheel": "up" }])) }));
    rig.tick(&[InputEvent::MouseWheel { delta: [0.0, 1.0] }]);
    assert!(rig.actions.pressed(name("next")));
    rig.tick(&[InputEvent::MouseWheel { delta: [0.0, 1.0] }]);
    assert!(
        rig.actions.pressed(name("next")),
        "a second notch on the next tick presses again"
    );
    assert!(rig.actions.held(name("next")) && !rig.actions.released(name("next")));
    rig.idle(1);
    assert!(!rig.actions.held(name("next")));
    assert!(rig.actions.released(name("next")));
}

#[test]
fn axis2_keys_stick_and_the_first_non_zero_binding_wins() {
    let mut rig = Rig::new(json!({
        "move": { "kind": "axis2", "bindings": [
            { "keys": { "up": "W", "down": "S", "left": "A", "right": "D" } },
            { "stick": "left", "deadzone": 0.25, "invert_y": true },
        ]},
    }));
    rig.tick(&[
        InputEvent::KeyDown { key: Key::W },
        InputEvent::KeyDown { key: Key::D },
    ]);
    let [x, y] = rig.actions.axis2(name("move"));
    assert!((x - FRAC_1_SQRT_2).abs() < 1e-6 && (y - FRAC_1_SQRT_2).abs() < 1e-6);
    rig.tick(&[
        InputEvent::KeyUp { key: Key::W },
        InputEvent::KeyUp { key: Key::D },
    ]);
    assert_eq!(rig.actions.axis2(name("move")), [0.0, 0.0]);

    let axis = |axis, value| InputEvent::GamepadAxis { axis, value };
    rig.tick(&[
        axis(GamepadAxis::LeftStickX, 0.2),
        axis(GamepadAxis::LeftStickY, 0.1),
    ]);
    assert_eq!(
        rig.actions.axis2(name("move")),
        [0.0, 0.0],
        "inside the deadzone"
    );
    rig.tick(&[
        axis(GamepadAxis::LeftStickX, 1.0),
        axis(GamepadAxis::LeftStickY, 0.0),
    ]);
    assert_eq!(rig.actions.axis2(name("move")), [1.0, 0.0]);
    rig.tick(&[
        axis(GamepadAxis::LeftStickX, 0.0),
        axis(GamepadAxis::LeftStickY, 1.0),
    ]);
    let [x, y] = rig.actions.axis2(name("move"));
    assert!(x == 0.0 && (y + 1.0).abs() < 1e-6, "invert_y flips: {y}");
    // Keys come first in binding order and win while non-zero.
    rig.tick(&[InputEvent::KeyDown { key: Key::A }]);
    assert_eq!(rig.actions.axis2(name("move")), [-1.0, 0.0]);
}

#[test]
fn axis1_pairs_and_the_wheel_and_mouse_delta_scale() {
    let mut rig = Rig::new(json!({
        "lean": { "kind": "axis1", "bindings": [{ "key_pair": ["Q", "E"] }, { "pad_pair": ["dpad_left", "dpad_right"] }] },
        "zoom": { "kind": "axis1", "bindings": [{ "wheel": "y", "scale": 2.0 }] },
        "look": { "kind": "axis2", "bindings": [{ "mouse_delta": true, "scale": 0.5, "invert_y": true }] },
    }));
    rig.tick(&[
        InputEvent::KeyDown { key: Key::E },
        InputEvent::MouseWheel { delta: [0.0, 1.5] },
        InputEvent::MouseMotion { delta: [4.0, 2.0] },
    ]);
    assert_eq!(rig.actions.axis(name("lean")), 1.0);
    assert_eq!(rig.actions.axis(name("zoom")), 3.0);
    assert_eq!(rig.actions.axis2(name("look")), [2.0, -1.0]);
    rig.tick(&[
        InputEvent::KeyUp { key: Key::E },
        InputEvent::GamepadButtonDown {
            button: GamepadButton::DpadLeft,
        },
    ]);
    assert_eq!(rig.actions.axis(name("lean")), -1.0);
}

#[test]
fn seconds_round_half_up_to_ticks_with_a_floor_of_one() {
    let hold_at = |hold: f32| {
        let map = compile(json!({
            "gestures": { "hold": hold },
            "actions": { "a": button(json!([{ "key": "C", "gesture": "hold" }])) },
        }))
        .unwrap();
        let mut rig = Rig {
            actions: Actions::new(map),
            input: InputState::default(),
        };
        rig.down(Key::C);
        let mut n = 1;
        while !rig.actions.pressed(name("a")) {
            rig.idle(1);
            n += 1;
        }
        n
    };
    assert_eq!(hold_at(0.0001), 1, "never below one tick");
    assert_eq!(hold_at(0.0249), 1);
    assert_eq!(hold_at(0.025), 2, "1.5 ticks rounds up");
    assert_eq!(hold_at(0.35), H);
}

#[test]
fn an_unknown_name_reads_inactive_and_is_reported_once() {
    let mut rig = Rig::new(json!({ "jump": button(json!([{ "key": "Space" }])) }));
    rig.down(Key::Space);
    assert!(!rig.actions.pressed(name("juump")));
    assert_eq!(rig.actions.axis(name("juump")), 0.0);
    assert!(!rig.actions.held(name("nope")));
    assert_eq!(rig.actions.take_unknown(), vec!["juump", "nope"]);
    assert!(!rig.actions.pressed(name("juump")));
    assert!(
        rig.actions.take_unknown().is_empty(),
        "reported once per name"
    );
    assert!(rig.actions.pressed(name("jump")));
}

// ── compile errors ──

fn errors(value: Value) -> Vec<(String, String)> {
    compile(value)
        .expect_err("the document is invalid")
        .into_iter()
        .map(|d| (d.code.to_string(), d.path.clone().unwrap_or_default()))
        .collect()
}

fn one(bindings: Value, kind: &str) -> Value {
    json!({ "actions": { "a": { "kind": kind, "bindings": bindings } } })
}

#[test]
fn l1630_zero_or_several_sources() {
    assert_eq!(
        errors(one(json!([{}]), "button")),
        [("L1630".into(), "/actions/a/bindings/0".into())]
    );
    assert_eq!(
        errors(one(json!([{ "key": "A", "mouse": "left" }]), "button")),
        [("L1630".into(), "/actions/a/bindings/0".into())]
    );
}

#[test]
fn l1631_source_does_not_fit_the_kind() {
    for (kind, binding, field) in [
        ("button", json!({ "stick": "left" }), "stick"),
        ("button", json!({ "wheel": "x" }), "wheel"),
        ("axis1", json!({ "key": "A" }), "key"),
        ("axis1", json!({ "stick": "left" }), "stick"),
        ("axis2", json!({ "key_pair": ["A", "B"] }), "key_pair"),
        ("axis2", json!({ "wheel": "up" }), "wheel"),
    ] {
        assert_eq!(
            errors(one(json!([binding]), kind)),
            [("L1631".into(), format!("/actions/a/bindings/0/{field}"))],
            "{kind} {field}"
        );
    }
}

#[test]
fn l1632_modifier_does_not_fit_the_source() {
    for (kind, binding, field) in [
        (
            "axis2",
            json!({ "stick": "left", "gesture": "tap" }),
            "gesture",
        ),
        (
            "button",
            json!({ "wheel": "up", "gesture": "tap" }),
            "gesture",
        ),
        ("button", json!({ "key": "A", "press": 0.5 }), "press"),
        ("button", json!({ "key": "A", "release": 0.2 }), "release"),
        (
            "axis2",
            json!({ "mouse_delta": true, "deadzone": 0.1 }),
            "deadzone",
        ),
        ("button", json!({ "key": "A", "scale": 2.0 }), "scale"),
        (
            "axis2",
            json!({ "keys": { "up": "W", "down": "S", "left": "A", "right": "D" }, "invert_x": true }),
            "invert_x",
        ),
        (
            "button",
            json!({ "pad": "south", "invert_y": true }),
            "invert_y",
        ),
    ] {
        assert_eq!(
            errors(one(json!([binding]), kind)),
            [("L1632".into(), format!("/actions/a/bindings/0/{field}"))],
            "{field}"
        );
    }
}

#[test]
fn l1633_out_of_range_values() {
    for (binding, field) in [
        (json!({ "trigger": "left", "press": 1.5 }), "press"),
        (
            json!({ "trigger": "left", "press": 0.5, "release": 0.6 }),
            "release",
        ),
        (
            json!({ "trigger": "left", "press": 0.5, "release": -0.1 }),
            "release",
        ),
        (
            json!({ "trigger": "left", "press": 0.2, "release": 0.3 }),
            "release",
        ),
    ] {
        assert_eq!(
            errors(one(json!([binding]), "button")),
            [("L1633".into(), format!("/actions/a/bindings/0/{field}"))],
            "{field}"
        );
    }
    assert_eq!(
        errors(one(json!([{ "stick": "left", "deadzone": 1.0 }]), "axis2")),
        [("L1633".into(), "/actions/a/bindings/0/deadzone".into())]
    );
    assert_eq!(
        errors(json!({
            "gestures": { "hold": 0.0 },
            "actions": { "a": button(json!([{ "key": "A" }])) },
        })),
        [("L1633".into(), "/gestures/hold".into())]
    );
}

#[test]
fn l1634_duplicate_source_and_gesture_in_one_context() {
    let dup = json!({ "actions": {
        "a": button(json!([{ "key": "C" }])),
        "b": button(json!([{ "key": "C" }])),
    }});
    assert_eq!(
        errors(dup),
        [("L1634".into(), "/actions/b/bindings/0".into())]
    );
    let ok = json!({ "actions": {
        "a": button(json!([{ "key": "C" }, { "key": "C", "gesture": "hold" }])),
        "b": { "kind": "button", "context": "menu", "bindings": [{ "key": "C" }] },
    }});
    assert!(
        compile(ok).is_ok(),
        "another gesture or another context is fine"
    );
}

#[test]
fn l1635_empty_bindings_and_bad_names() {
    assert_eq!(
        errors(json!({ "actions": { "a": { "kind": "button", "bindings": [] } } })),
        [("L1635".into(), "/actions/a/bindings".into())]
    );
    assert_eq!(
        errors(json!({ "actions": { "Bad-Name": button(json!([{ "key": "A" }])) } })),
        [("L1635".into(), "/actions/Bad-Name".into())]
    );
}

#[test]
fn every_error_is_collected_not_just_the_first() {
    let all = errors(json!({ "actions": {
        "a": button(json!([{}, { "stick": "left" }])),
        "b": { "kind": "button", "bindings": [] },
    }}));
    assert_eq!(all.len(), 3);
}

#[test]
fn unknown_fields_are_rejected_by_the_document() {
    let err = serde_json::from_value::<InputActionsDoc>(json!({
        "actions": { "a": button(json!([{ "key": "A", "sensitivity": 2 }])) }
    }))
    .unwrap_err()
    .to_string();
    assert!(err.contains("sensitivity"), "{err}");
}

#[test]
fn the_compiled_map_is_canonical() {
    let a = compile(json!({ "actions": {
        "b": button(json!([{ "key": "Z" }])),
        "a": button(json!([{ "key": "A" }, { "key": "Z", "gesture": "hold" }])),
    }}))
    .unwrap();
    assert_eq!(a.names().collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(
        a.inputs().len(),
        2,
        "a shared physical input is tracked once"
    );
}

#[test]
fn l4270_and_the_explain_codes_are_registered() {
    for code in [
        "L1630", "L1631", "L1632", "L1633", "L1634", "L1635", "L4270", "L4271",
    ] {
        assert!(ludion_core::codes::explain(code).is_some(), "{code}");
    }
}

// ── determinism ──

fn script() -> InputScript {
    let mut s = InputScript::new();
    let at = FrameIndex::new;
    let key = |key| (InputEvent::KeyDown { key }, InputEvent::KeyUp { key });
    let (down, up) = key(Key::C);
    // A lone tap (deferred: the key also has a double-tap binding), a double-tap whose second
    // press becomes a hold.
    s.push(at(1), down);
    s.push(at(3), up);
    s.push(at(30), down);
    s.push(at(32), up);
    s.push(at(38), down);
    s.push(at(70), up);
    // The pad's south button has no double-tap: its tap fires on release, then it holds.
    let pad = |button| {
        (
            InputEvent::GamepadButtonDown { button },
            InputEvent::GamepadButtonUp { button },
        )
    };
    let (down, up) = pad(GamepadButton::South);
    s.push(at(75), down);
    s.push(at(77), up);
    s.push(at(80), down);
    s.push(at(120), up);
    s.push(at(50), trigger(0.7));
    s.push(at(60), trigger(0.4));
    s.push(at(70), trigger(0.1));
    s
}

fn actions_doc() -> Value {
    json!({ "actions": {
        "slide": button(json!([{ "key": "C", "gesture": "tap" }, { "pad": "south", "gesture": "tap" }])),
        "roll": button(json!([{ "key": "C", "gesture": "double_tap" }])),
        "prone": button(json!([{ "key": "C", "gesture": "hold" }, { "pad": "south", "gesture": "hold" }])),
        "aim": button(json!([{ "trigger": "right", "press": 0.5, "release": 0.35 }])),
    }})
}

fn driver() -> SimDriver {
    let mut world = World::new();
    world.insert_resource(Actions::new(compile(actions_doc()).unwrap()));
    SimDriver::new(
        world,
        Schedule::new(),
        SimConfig {
            input: script(),
            ..SimConfig::default()
        },
    )
}

const NAMES: [&str; 4] = ["slide", "roll", "prone", "aim"];

fn reads(driver: &SimDriver) -> Vec<(bool, bool, bool, u32)> {
    let actions = driver.world().resource::<Actions>().unwrap();
    NAMES
        .iter()
        .map(|n| {
            let n = ActionName::new(n);
            (
                actions.pressed(n),
                actions.held(n),
                actions.released(n),
                actions.held_ticks(n),
            )
        })
        .collect()
}

fn run(driver: &mut SimDriver, ticks: usize) -> Vec<Vec<(bool, bool, bool, u32)>> {
    (0..ticks)
        .map(|_| {
            driver.advance();
            reads(driver)
        })
        .collect()
}

#[test]
fn the_driver_resolves_each_tick_and_two_runs_agree() {
    let (mut a, mut b) = (driver(), driver());
    let (ra, rb) = (run(&mut a, 130), run(&mut b, 130));
    assert_eq!(ra, rb);
    assert!(ra.iter().any(|t| t[0].0), "a tap fired");
    assert!(ra.iter().any(|t| t[1].0), "a double-tap fired");
    assert!(ra.iter().any(|t| t[2].0), "a hold fired");
    assert!(ra.iter().any(|t| t[3].0), "the trigger pressed");
}

#[test]
fn a_world_without_actions_is_untouched() {
    let mut d = SimDriver::new(
        World::new(),
        Schedule::new(),
        SimConfig {
            input: script(),
            ..SimConfig::default()
        },
    );
    d.run_ticks(20);
    assert!(d.world().resource::<Actions>().is_none());
}

#[test]
fn a_mid_hold_state_round_trip_fires_the_hold_on_the_same_tick() {
    let mut whole = driver();
    let uninterrupted = run(&mut whole, 130);

    let mut first = driver();
    let _ = run(&mut first, 50); // the double press is mid-hold (n = 12 of 21)
    let state = first.world().resource::<Actions>().unwrap().state();
    let json = serde_json::to_string(&state).unwrap();
    let restored: ludion_sim::ActionState = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, state);

    // A fresh driver at the same input position (frame 50) with a restored Actions.
    let mut second = driver();
    let mut resumed = Vec::new();
    {
        let mut fresh = Actions::new(compile(actions_doc()).unwrap());
        fresh.restore_state(restored).unwrap();
        // Rebuild the same driver but replace the resource after 25 ticks of the same script.
        let _ = run(&mut second, 50);
        second.world_mut().insert_resource(fresh);
        // The resolve of tick 50 happens inside the next advance, from the restored state.
        for _ in 50..130 {
            second.advance();
            resumed.push(reads(&second));
        }
    }
    assert_eq!(resumed, uninterrupted[50..]);
}

#[test]
fn restoring_a_corrupt_state_is_l4271_and_unknown_inputs_are_dropped() {
    let map = compile(actions_doc()).unwrap();
    let mut actions = Actions::new(map);
    let bad: ludion_sim::ActionState =
        serde_json::from_value(json!([[{ "Key": "C" }, { "down": true, "n": 0 }]])).unwrap();
    assert_eq!(
        actions.restore_state(bad).unwrap_err().code.to_string(),
        "L4271"
    );
    let stray: ludion_sim::ActionState =
        serde_json::from_value(json!([[{ "Key": "Q" }, { "down": true, "n": 3 }]])).unwrap();
    actions.restore_state(stray).unwrap();
    assert_eq!(actions.state(), ludion_sim::ActionState::default());
}
