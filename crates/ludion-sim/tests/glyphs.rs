//! Prompt glyph names, `Actions::glyphs` and `Actions::source`.

use ludion_core::par;
use ludion_core::time::{FrameIndex, TickRate};
use ludion_ecs::{Registry, Schedule, SystemCtx, World};
use ludion_sim::actions::{
    ActionMap, ActionName, Actions, BindingDoc, Gesture, InputActionsDoc, KeyCluster, Side,
    WheelSource,
};
use ludion_sim::{
    Device, GamepadAxis, GamepadButton, GamepadFamily, Glyph, InputEvent, InputScript, InputState,
    Key, MouseButton, SimConfig, SimDriver, WorldHash, glyph_name,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const FAMILIES: [(GamepadFamily, &str); 4] = [
    (GamepadFamily::Xbox, "xbox"),
    (GamepadFamily::Playstation, "playstation"),
    (GamepadFamily::Nintendo, "nintendo"),
    (GamepadFamily::Generic, "generic"),
];

fn pad(button: GamepadButton) -> BindingDoc {
    BindingDoc {
        pad: Some(button),
        ..BindingDoc::default()
    }
}

fn key(key: Key) -> BindingDoc {
    BindingDoc {
        key: Some(key),
        ..BindingDoc::default()
    }
}

#[test]
fn every_pad_button_has_the_tables_name_in_every_family() {
    use GamepadButton::*;
    // Labels in the order of `FAMILIES`.
    let faces = [
        (South, ["a", "cross", "b", "south"]),
        (East, ["b", "circle", "a", "east"]),
        (West, ["x", "square", "y", "west"]),
        (North, ["y", "triangle", "x", "north"]),
    ];
    let shared = [
        (LeftShoulder, "lb"),
        (RightShoulder, "rb"),
        (LeftTrigger, "lt"),
        (RightTrigger, "rt"),
        (LeftStick, "ls"),
        (RightStick, "rs"),
        (Start, "start"),
        (Select, "select"),
        (DpadUp, "dpad_up"),
        (DpadDown, "dpad_down"),
        (DpadLeft, "dpad_left"),
        (DpadRight, "dpad_right"),
    ];
    let table: Vec<(GamepadButton, [&str; 4])> = faces
        .into_iter()
        .chain(shared.into_iter().map(|(b, label)| (b, [label; 4])))
        .collect();
    assert_eq!(table.len(), variants::<GamepadButton>().len());
    for (button, labels) in table {
        for ((family, prefix), label) in FAMILIES.into_iter().zip(labels) {
            assert_eq!(
                glyph_name(&pad(button), family),
                Some(format!("pad.{prefix}.{label}").as_str()),
                "{button:?} {family:?}"
            );
        }
    }
}

/// Every variant of a unit-only enum, read from its schema so a new one cannot be missed.
fn variants<T: JsonSchema + for<'de> Deserialize<'de>>() -> Vec<T> {
    fn collect(node: &Value, out: &mut Vec<Value>) {
        match node {
            Value::Object(map) => {
                for (field, value) in map {
                    match (field.as_str(), value) {
                        ("const", name) => out.push(name.clone()),
                        ("enum", Value::Array(names)) => out.extend(names.iter().cloned()),
                        _ => collect(value, out),
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|item| collect(item, out)),
            _ => {}
        }
    }
    let schema = serde_json::to_value(schemars::schema_for!(T)).unwrap();
    let mut names = Vec::new();
    collect(&schema, &mut names);
    names
        .into_iter()
        .map(|name| serde_json::from_value(name).unwrap())
        .collect()
}

fn snake_case(name: &str) -> String {
    let mut out = String::new();
    for (at, c) in name.char_indices() {
        if c.is_ascii_uppercase() && at > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

#[test]
fn every_key_is_named_by_its_serde_name_in_snake_case() {
    let keys = variants::<Key>();
    assert!(keys.len() >= 94, "only {} keys", keys.len());
    for k in keys {
        let serde = serde_json::to_value(k).unwrap();
        let expected = format!("key.{}", snake_case(serde.as_str().unwrap()));
        for (family, _) in FAMILIES {
            assert_eq!(
                glyph_name(&key(k), family),
                Some(expected.as_str()),
                "{k:?}"
            );
        }
    }
    for (k, name) in [
        (Key::E, "key.e"),
        (Key::Tab, "key.tab"),
        (Key::Space, "key.space"),
        (Key::Digit1, "key.digit1"),
        (Key::PageUp, "key.page_up"),
        (Key::CapsLock, "key.caps_lock"),
        (Key::NumpadAdd, "key.numpad_add"),
        (Key::F12, "key.f12"),
    ] {
        assert_eq!(glyph_name(&key(k), GamepadFamily::Generic), Some(name));
    }
}

#[test]
fn mouse_wheel_trigger_and_stick_names() {
    let name = |binding: BindingDoc, family| glyph_name(&binding, family).unwrap();
    let buttons = [
        (MouseButton::Left, "mouse.left"),
        (MouseButton::Right, "mouse.right"),
        (MouseButton::Middle, "mouse.middle"),
        (MouseButton::Back, "mouse.back"),
        (MouseButton::Forward, "mouse.forward"),
    ];
    assert_eq!(buttons.len(), variants::<MouseButton>().len());
    for (button, expected) in buttons {
        let binding = BindingDoc {
            mouse: Some(button),
            ..BindingDoc::default()
        };
        assert_eq!(name(binding, GamepadFamily::Xbox), expected);
    }
    for (wheel, expected) in [
        (WheelSource::Up, "mouse.wheel_up"),
        (WheelSource::Down, "mouse.wheel_down"),
        (WheelSource::X, "mouse.wheel_x"),
        (WheelSource::Y, "mouse.wheel_y"),
    ] {
        let binding = BindingDoc {
            wheel: Some(wheel),
            ..BindingDoc::default()
        };
        assert_eq!(name(binding, GamepadFamily::Xbox), expected);
    }
    let motion = BindingDoc {
        mouse_delta: true,
        ..BindingDoc::default()
    };
    assert_eq!(name(motion, GamepadFamily::Xbox), "mouse.move");

    for (family, prefix) in FAMILIES {
        for (side, trigger, stick) in [
            (Side::Left, "lt", "left_stick"),
            (Side::Right, "rt", "right_stick"),
        ] {
            let binding = BindingDoc {
                trigger: Some(side),
                ..BindingDoc::default()
            };
            assert_eq!(name(binding, family), format!("pad.{prefix}.{trigger}"));
            let binding = BindingDoc {
                stick: Some(side),
                ..BindingDoc::default()
            };
            assert_eq!(name(binding, family), format!("pad.{prefix}.{stick}"));
        }
    }
}

#[test]
fn composites_and_invalid_bindings_have_no_single_name() {
    let cluster = BindingDoc {
        keys: Some(KeyCluster {
            up: Key::W,
            down: Key::S,
            left: Key::A,
            right: Key::D,
        }),
        ..BindingDoc::default()
    };
    let key_pair = BindingDoc {
        key_pair: Some([Key::Q, Key::E]),
        ..BindingDoc::default()
    };
    let pad_pair = BindingDoc {
        pad_pair: Some([GamepadButton::DpadLeft, GamepadButton::DpadRight]),
        ..BindingDoc::default()
    };
    let two = BindingDoc {
        pad: Some(GamepadButton::South),
        ..key(Key::E)
    };
    for binding in [cluster, key_pair, pad_pair, two, BindingDoc::default()] {
        assert_eq!(
            glyph_name(&binding, GamepadFamily::Xbox),
            None,
            "{binding:?}"
        );
    }
}

// ───────────────────────────── the action layer ─────────────────────────────

const TAKEDOWN: ActionName = ActionName::new("takedown");
const ZOOM: ActionName = ActionName::new("zoom");
const MOVE: ActionName = ActionName::new("move");
const LOOK: ActionName = ActionName::new("look");

fn actions() -> Actions {
    let doc: InputActionsDoc = serde_json::from_value(json!({
        "actions": {
            "takedown": { "kind": "button", "bindings": [
                { "key": "E", "gesture": "tap" },
                { "pad": "south" },
                { "key": "E", "gesture": "hold" },
                { "trigger": "left" },
                { "wheel": "up" },
            ]},
            "zoom": { "kind": "axis1", "bindings": [
                { "pad_pair": ["dpad_down", "dpad_up"] },
                { "key_pair": ["Q", "R"] },
                { "wheel": "y" },
            ]},
            "move": { "kind": "axis2", "bindings": [
                { "stick": "left" },
                { "keys": { "up": "W", "down": "S", "left": "A", "right": "D" } },
            ]},
            "look": { "kind": "axis2", "bindings": [
                { "mouse_delta": true },
                { "stick": "right" },
            ]},
        }
    }))
    .unwrap();
    Actions::new(ActionMap::compile(&doc, TickRate::DEFAULT).unwrap())
}

fn tick(actions: &mut Actions, input: &mut InputState, events: &[InputEvent]) {
    input.begin_tick();
    for &event in events {
        input.apply(event);
    }
    actions.resolve(input);
}

fn glyphs(actions: &Actions, name: ActionName, device: Device) -> Vec<Glyph> {
    actions.glyphs(name, device).collect()
}

fn names(actions: &Actions, name: ActionName, device: Device) -> Vec<&'static str> {
    actions.glyphs(name, device).map(|g| g.name).collect()
}

fn glyph(name: &'static str, gesture: impl Into<Option<Gesture>>) -> Glyph {
    Glyph {
        name,
        gesture: gesture.into(),
    }
}

#[test]
fn glyphs_keep_binding_order_filter_by_device_and_carry_gestures() {
    let actions = actions();
    assert_eq!(
        glyphs(&actions, TAKEDOWN, Device::KeyboardMouse),
        [
            glyph("key.e", Gesture::Tap),
            glyph("key.e", Gesture::Hold),
            glyph("mouse.wheel_up", Gesture::Press),
        ]
    );
    assert_eq!(
        glyphs(&actions, TAKEDOWN, Device::Gamepad),
        [
            glyph("pad.generic.south", Gesture::Press),
            glyph("pad.generic.lt", Gesture::Press),
        ]
    );
}

#[test]
fn composites_expand_and_axis_glyphs_have_no_gesture() {
    let actions = actions();
    assert_eq!(
        glyphs(&actions, ZOOM, Device::Gamepad),
        [
            glyph("pad.generic.dpad_down", None),
            glyph("pad.generic.dpad_up", None),
        ]
    );
    assert_eq!(
        glyphs(&actions, ZOOM, Device::KeyboardMouse),
        [
            glyph("key.q", None),
            glyph("key.r", None),
            glyph("mouse.wheel_y", None),
        ]
    );
    assert_eq!(
        glyphs(&actions, MOVE, Device::KeyboardMouse),
        ["key.w", "key.s", "key.a", "key.d"].map(|n| glyph(n, None))
    );
    assert_eq!(
        glyphs(&actions, MOVE, Device::Gamepad),
        [glyph("pad.generic.left_stick", None)]
    );
    assert_eq!(
        glyphs(&actions, LOOK, Device::KeyboardMouse),
        [glyph("mouse.move", None)]
    );
}

#[test]
fn the_family_follows_the_connected_pad() {
    let mut actions = actions();
    let mut input = InputState::default();
    let pad_names = |actions: &Actions| names(actions, TAKEDOWN, Device::Gamepad);

    for (family, south) in [
        (GamepadFamily::Xbox, "pad.xbox.a"),
        (GamepadFamily::Playstation, "pad.playstation.cross"),
        (GamepadFamily::Nintendo, "pad.nintendo.b"),
    ] {
        let connected = InputEvent::GamepadConnected { family };
        tick(&mut actions, &mut input, &[connected]);
        assert_eq!(pad_names(&actions)[0], south);
    }
    tick(&mut actions, &mut input, &[InputEvent::GamepadDisconnected]);
    assert_eq!(pad_names(&actions), ["pad.generic.south", "pad.generic.lt"]);
}

#[test]
fn a_rebind_shows_at_once() {
    let mut actions = actions();
    actions.rebind("takedown", 0, key(Key::F)).unwrap();
    assert_eq!(
        glyphs(&actions, TAKEDOWN, Device::KeyboardMouse)[0],
        glyph("key.f", Gesture::Press)
    );
    let overrides = actions.overrides().clone();

    let mut fresh = self::actions();
    assert!(fresh.apply_overrides(&overrides).is_empty());
    assert_eq!(names(&fresh, TAKEDOWN, Device::KeyboardMouse)[0], "key.f");
    fresh.reset_all();
    assert_eq!(names(&fresh, TAKEDOWN, Device::KeyboardMouse)[0], "key.e");
}

#[test]
fn an_unknown_name_yields_nothing_and_is_reported_once() {
    let mut actions = actions();
    const NOPE: ActionName = ActionName::new("nope");
    assert!(glyphs(&actions, NOPE, Device::Gamepad).is_empty());
    assert!(glyphs(&actions, NOPE, Device::KeyboardMouse).is_empty());
    assert_eq!(actions.source(NOPE), None);
    assert_eq!(actions.take_unknown(), ["nope"]);
    assert_eq!(actions.source(NOPE), None);
    assert!(actions.take_unknown().is_empty());

    const OTHER: ActionName = ActionName::new("other");
    assert_eq!(actions.source(OTHER), None);
    assert_eq!(actions.take_unknown(), ["other"]);
}

fn key_down(key: Key) -> InputEvent {
    InputEvent::KeyDown { key }
}

fn key_up(key: Key) -> InputEvent {
    InputEvent::KeyUp { key }
}

fn button_down(button: GamepadButton) -> InputEvent {
    InputEvent::GamepadButtonDown { button }
}

fn button_up(button: GamepadButton) -> InputEvent {
    InputEvent::GamepadButtonUp { button }
}

fn stick_x(value: f32) -> InputEvent {
    InputEvent::GamepadAxis {
        axis: GamepadAxis::LeftStickX,
        value,
    }
}

#[test]
fn source_follows_the_device_that_drove_the_read() {
    let mut actions = actions();
    let mut input = InputState::default();
    tick(&mut actions, &mut input, &[]);
    for name in [TAKEDOWN, ZOOM, MOVE, LOOK] {
        assert_eq!(actions.source(name), None);
    }

    tick(
        &mut actions,
        &mut input,
        &[button_down(GamepadButton::South)],
    );
    assert_eq!(actions.source(TAKEDOWN), Some(Device::Gamepad));
    tick(&mut actions, &mut input, &[button_up(GamepadButton::South)]);
    // The release tick still has a read, from the pad.
    assert!(actions.released(TAKEDOWN));
    assert_eq!(actions.source(TAKEDOWN), Some(Device::Gamepad));
    tick(&mut actions, &mut input, &[]);
    assert_eq!(actions.source(TAKEDOWN), None);

    let wheel = InputEvent::MouseWheel { delta: [0.0, 1.0] };
    tick(&mut actions, &mut input, &[wheel]);
    assert_eq!(actions.source(TAKEDOWN), Some(Device::KeyboardMouse));
}

#[test]
fn a_tap_reports_its_device_on_the_release_tick_and_a_hold_keeps_it() {
    let mut actions = actions();
    let mut input = InputState::default();
    // A press short of the hold: neither gesture has fired.
    tick(&mut actions, &mut input, &[key_down(Key::E)]);
    assert_eq!(actions.source(TAKEDOWN), None);
    tick(&mut actions, &mut input, &[key_up(Key::E)]);
    assert!(actions.pressed(TAKEDOWN));
    assert_eq!(actions.source(TAKEDOWN), Some(Device::KeyboardMouse));

    tick(&mut actions, &mut input, &[key_down(Key::E)]);
    for _ in 0..20 {
        tick(&mut actions, &mut input, &[]);
    }
    assert!(actions.pressed(TAKEDOWN));
    for _ in 0..5 {
        assert!(actions.held(TAKEDOWN));
        assert_eq!(actions.source(TAKEDOWN), Some(Device::KeyboardMouse));
        tick(&mut actions, &mut input, &[]);
    }
}

#[test]
fn binding_order_decides_between_two_driving_devices() {
    let mut actions = actions();
    let mut input = InputState::default();
    // The wheel (binding 4) and the pad (binding 1) fire together.
    let wheel = InputEvent::MouseWheel { delta: [0.0, 1.0] };
    tick(
        &mut actions,
        &mut input,
        &[wheel, button_down(GamepadButton::South)],
    );
    assert_eq!(actions.source(TAKEDOWN), Some(Device::Gamepad));
}

#[test]
fn an_axis_takes_the_device_of_the_first_non_zero_binding() {
    let mut actions = actions();
    let mut input = InputState::default();
    tick(&mut actions, &mut input, &[key_down(Key::D)]);
    assert_eq!(actions.source(MOVE), Some(Device::KeyboardMouse));
    tick(&mut actions, &mut input, &[stick_x(1.0)]);
    assert_eq!(actions.source(MOVE), Some(Device::Gamepad));
    tick(&mut actions, &mut input, &[stick_x(0.0)]);
    assert_eq!(actions.source(MOVE), Some(Device::KeyboardMouse));
    tick(&mut actions, &mut input, &[key_up(Key::D)]);
    assert_eq!(actions.source(MOVE), None);

    tick(&mut actions, &mut input, &[key_down(Key::R)]);
    assert_eq!(actions.source(ZOOM), Some(Device::KeyboardMouse));
    tick(
        &mut actions,
        &mut input,
        &[button_down(GamepadButton::DpadUp)],
    );
    assert_eq!(actions.source(ZOOM), Some(Device::Gamepad));

    let motion = InputEvent::MouseMotion { delta: [3.0, 0.0] };
    tick(&mut actions, &mut input, &[motion]);
    assert_eq!(actions.source(LOOK), Some(Device::KeyboardMouse));
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, PartialEq, Debug, Default)]
struct Sources {
    takedown: u32,
    movement: u32,
    log: u32,
}

fn observe(world: &mut World, _ctx: &SystemCtx) {
    let actions = world.resource::<Actions>().unwrap();
    let code = |name| match actions.source(name) {
        None => 0,
        Some(Device::KeyboardMouse) => 1,
        Some(Device::Gamepad) => 2,
    };
    let (takedown, movement) = (code(TAKEDOWN), code(MOVE));
    for (seen,) in world.query::<(&mut Sources,)>() {
        seen.takedown = takedown;
        seen.movement = movement;
        // A base-3 digest, so the final value remembers the order of the reads.
        seen.log = seen
            .log
            .wrapping_mul(9)
            .wrapping_add(takedown * 3 + movement);
    }
}

fn trace(threads: usize) -> Vec<WorldHash> {
    par::with_threads(threads, || {
        let mut registry = Registry::new();
        registry.register::<Sources>("Sources").unwrap();
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Sources::default()).unwrap();
        world.insert_resource(actions());
        let mut schedule = Schedule::new();
        schedule.add_system("observe", observe);
        let mut script = InputScript::new();
        for (frame, event) in [
            (1, button_down(GamepadButton::South)),
            (3, button_up(GamepadButton::South)),
            (5, key_down(Key::D)),
            (7, stick_x(1.0)),
            (9, stick_x(0.0)),
            (11, key_up(Key::D)),
            (12, key_down(Key::E)),
            (14, key_up(Key::E)),
        ] {
            script.push(FrameIndex::new(frame), event);
        }
        let config = SimConfig {
            input: script,
            ..SimConfig::default()
        };
        let mut driver = SimDriver::new(world, schedule, config);
        let hashes: Vec<_> = (0..18)
            .map(|_| {
                driver.run_ticks(1);
                WorldHash::compute(driver.world(), &registry).unwrap()
            })
            .collect();
        let seen = driver.world().get::<Sources>(entity).unwrap().unwrap();
        assert_ne!(seen.log, 0, "the system never saw a source");
        hashes
    })
}

#[test]
fn sources_give_identical_hash_traces_at_any_thread_count() {
    let first = trace(1);
    for threads in [1, 2, 8] {
        assert_eq!(trace(threads), first, "{threads} threads");
    }
}
