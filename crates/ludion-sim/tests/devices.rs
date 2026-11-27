//! The device and focus facts: the three recorded events, the `last_device` fold, and what a
//! rebuilt world carries over.

use ludion_core::par;
use ludion_core::time::FrameIndex;
use ludion_ecs::{Registry, Schedule, SystemCtx, World};
use ludion_sim::{
    Device, DeviceFacts, GamepadAxis, GamepadButton, GamepadFamily, InputEvent, InputScript,
    InputState, Key, MOUSE_DEVICE_THRESHOLD, MouseButton, STICK_DEADZONE, SimConfig, SimDriver,
    TRIGGER_THRESHOLD, WorldHash,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

fn tick(input: &mut InputState, events: &[InputEvent]) {
    input.begin_tick();
    for &event in events {
        input.apply(event);
    }
}

/// A state whose last device is `device`, one tick old.
fn on(device: Device) -> InputState {
    let mut input = InputState::default();
    if device == Device::Gamepad {
        tick(&mut input, &[button_down(GamepadButton::South)]);
    }
    input.begin_tick();
    input
}

fn button_down(button: GamepadButton) -> InputEvent {
    InputEvent::GamepadButtonDown { button }
}

fn axis(axis: GamepadAxis, value: f32) -> InputEvent {
    InputEvent::GamepadAxis { axis, value }
}

fn motion(x: f32, y: f32) -> InputEvent {
    InputEvent::MouseMotion { delta: [x, y] }
}

fn connected(family: GamepadFamily) -> InputEvent {
    InputEvent::GamepadConnected { family }
}

#[track_caller]
fn assert_selects(from: Device, events: &[InputEvent], to: Device) {
    let mut input = on(from);
    tick(&mut input, events);
    assert_eq!(input.last_device(), to, "{events:?} from {from:?}");
}

#[test]
fn keyboard_and_mouse_actions_select_keyboard_mouse() {
    use Device::{Gamepad, KeyboardMouse};
    let key = InputEvent::KeyDown { key: Key::W };
    let click = InputEvent::MouseDown {
        button: MouseButton::Left,
    };
    let wheel = InputEvent::MouseWheel { delta: [0.0, -1.0] };
    for event in [key, click, wheel, motion(5.0, 0.0)] {
        assert_selects(Gamepad, &[event], KeyboardMouse);
    }
    // A repeat of a held key counts too.
    let mut input = InputState::default();
    tick(&mut input, &[key, button_down(GamepadButton::South), key]);
    assert_eq!(input.last_device(), KeyboardMouse);
}

#[test]
fn the_cursor_a_still_wheel_and_releases_select_nothing() {
    use Device::Gamepad;
    for event in [
        InputEvent::MouseMove {
            position: [640.0, 360.0],
        },
        InputEvent::MouseWheel { delta: [0.0, -0.0] },
        InputEvent::KeyUp { key: Key::W },
        InputEvent::MouseUp {
            button: MouseButton::Left,
        },
    ] {
        assert_selects(Gamepad, &[event], Gamepad);
    }
    for event in [
        InputEvent::GamepadButtonUp {
            button: GamepadButton::South,
        },
        InputEvent::GamepadButtonUp {
            button: GamepadButton::LeftTrigger,
        },
        axis(GamepadAxis::LeftStickX, 0.0),
        axis(GamepadAxis::RightTrigger, 0.0),
        InputEvent::Focus { gained: false },
    ] {
        assert_selects(Device::KeyboardMouse, &[event], Device::KeyboardMouse);
    }
}

#[test]
fn motion_must_exceed_the_threshold_over_the_tick() {
    use Device::{Gamepad, KeyboardMouse};
    assert_eq!(MOUSE_DEVICE_THRESHOLD, 4.0);
    assert_selects(Gamepad, &[motion(4.0, 0.0)], Gamepad);
    assert_selects(
        Gamepad,
        &[motion(3.0, 0.0), motion(3.0, 0.0)],
        KeyboardMouse,
    );
    // The sum is a vector: out and back is no motion.
    assert_selects(Gamepad, &[motion(3.0, 0.0), motion(-3.0, 0.0)], Gamepad);
    // The accumulator is per tick.
    let mut input = on(Gamepad);
    tick(&mut input, &[motion(3.0, 0.0)]);
    tick(&mut input, &[motion(3.0, 0.0)]);
    assert_eq!(input.last_device(), Gamepad);
}

#[test]
fn pad_actions_select_the_gamepad() {
    use Device::{Gamepad, KeyboardMouse};
    for event in [
        button_down(GamepadButton::South),
        button_down(GamepadButton::LeftTrigger),
        axis(GamepadAxis::LeftStickX, -STICK_DEADZONE),
        axis(GamepadAxis::RightStickY, 1.0),
        axis(GamepadAxis::LeftTrigger, TRIGGER_THRESHOLD),
        axis(GamepadAxis::RightTrigger, 1.0),
        connected(GamepadFamily::Generic),
    ] {
        assert_selects(KeyboardMouse, &[event], Gamepad);
    }
    for event in [
        axis(GamepadAxis::LeftStickX, STICK_DEADZONE * 0.9),
        axis(GamepadAxis::RightStickY, -STICK_DEADZONE * 0.9),
        axis(GamepadAxis::LeftTrigger, TRIGGER_THRESHOLD * 0.9),
    ] {
        assert_selects(KeyboardMouse, &[event], KeyboardMouse);
    }
    // The deadzone is radial over the stick's two axes, and per stick.
    let part = STICK_DEADZONE * 0.8;
    assert_selects(
        KeyboardMouse,
        &[
            axis(GamepadAxis::LeftStickX, part),
            axis(GamepadAxis::LeftStickY, part),
        ],
        Gamepad,
    );
    assert_selects(
        KeyboardMouse,
        &[
            axis(GamepadAxis::LeftStickX, part),
            axis(GamepadAxis::RightStickY, part),
        ],
        KeyboardMouse,
    );
}

#[test]
fn device_changed_compares_with_the_tick_start() {
    let mut input = InputState::default();
    tick(&mut input, &[button_down(GamepadButton::South)]);
    assert!(input.device_changed());
    tick(&mut input, &[]);
    assert!(!input.device_changed());
    // There and back inside one tick is no change.
    tick(
        &mut input,
        &[
            InputEvent::KeyDown { key: Key::W },
            button_down(GamepadButton::East),
        ],
    );
    assert!(!input.device_changed());
    assert_eq!(input.last_device(), Device::Gamepad);
}

#[test]
fn connect_and_disconnect_track_the_pad_and_its_family() {
    let mut input = InputState::default();
    assert!(!input.gamepad().connected());
    assert_eq!(input.gamepad().family(), None);

    tick(&mut input, &[connected(GamepadFamily::Playstation)]);
    assert!(input.gamepad().connected());
    assert_eq!(input.gamepad().family(), Some(GamepadFamily::Playstation));
    assert_eq!(input.last_device(), Device::Gamepad);

    // No release is synthesized: the host sends them before the event.
    tick(&mut input, &[button_down(GamepadButton::South)]);
    tick(&mut input, &[InputEvent::GamepadDisconnected]);
    assert!(!input.gamepad().connected());
    assert_eq!(input.gamepad().family(), None);
    assert!(input.gamepad().is_held(GamepadButton::South));
    assert_eq!(input.last_device(), Device::KeyboardMouse);
    assert!(input.device_changed());
}

#[test]
fn a_disconnect_leaves_keyboard_mouse_alone() {
    let mut input = InputState::default();
    tick(
        &mut input,
        &[
            connected(GamepadFamily::Xbox),
            InputEvent::KeyDown { key: Key::W },
        ],
    );
    tick(&mut input, &[InputEvent::GamepadDisconnected]);
    assert_eq!(input.last_device(), Device::KeyboardMouse);
    assert!(!input.device_changed());
}

#[test]
fn focus_is_a_level_with_a_one_tick_edge() {
    let mut input = InputState::default();
    assert!(input.focused() && !input.focus_changed());

    tick(&mut input, &[InputEvent::Focus { gained: false }]);
    assert!(!input.focused() && input.focus_changed());
    assert!(!input.is_idle());
    tick(&mut input, &[]);
    assert!(!input.focused() && !input.focus_changed());

    tick(&mut input, &[InputEvent::Focus { gained: true }]);
    assert!(input.focused() && input.focus_changed());
    // The tick-start copy still differs from the default until the next tick.
    assert!(!input.is_idle());
    tick(&mut input, &[]);
    assert!(input.is_idle());

    tick(
        &mut input,
        &[
            InputEvent::Focus { gained: false },
            InputEvent::Focus { gained: true },
        ],
    );
    assert!(!input.focus_changed());
}

#[test]
fn the_three_events_have_pinned_wire_forms() {
    let cases = [
        (
            connected(GamepadFamily::Xbox),
            json!({"kind": "gamepad_connected", "family": "xbox"}),
        ),
        (
            connected(GamepadFamily::Playstation),
            json!({"kind": "gamepad_connected", "family": "playstation"}),
        ),
        (
            connected(GamepadFamily::Nintendo),
            json!({"kind": "gamepad_connected", "family": "nintendo"}),
        ),
        (
            connected(GamepadFamily::Generic),
            json!({"kind": "gamepad_connected", "family": "generic"}),
        ),
        (
            InputEvent::GamepadDisconnected,
            json!({"kind": "gamepad_disconnected"}),
        ),
        (
            InputEvent::Focus { gained: false },
            json!({"kind": "focus", "gained": false}),
        ),
    ];
    for (event, wire) in cases {
        assert_eq!(serde_json::to_value(event).unwrap(), wire);
        assert_eq!(serde_json::from_value::<InputEvent>(wire).unwrap(), event);
        assert!(event.validate().is_ok());
    }
    assert_eq!(
        serde_json::to_value(Device::KeyboardMouse).unwrap(),
        json!("keyboard_mouse")
    );
    let unknown = json!({"kind": "gamepad_connected", "family": "sega"});
    assert!(serde_json::from_value::<InputEvent>(unknown).is_err());

    let mut script = InputScript::new();
    script.push(FrameIndex::new(1), connected(GamepadFamily::Nintendo));
    script.push(FrameIndex::new(2), InputEvent::Focus { gained: false });
    script.push(FrameIndex::new(3), InputEvent::GamepadDisconnected);
    let text = serde_json::to_string(&script).unwrap();
    assert_eq!(serde_json::from_str::<InputScript>(&text).unwrap(), script);
}

#[test]
fn a_default_state_serializes_without_the_device_fields() {
    let input = InputState::default();
    assert!(input.is_idle());
    assert_eq!(
        serde_json::to_string(&input).unwrap(),
        concat!(
            r#"{"held":[],"pressed":[],"released":[],"#,
            r#""mouse":{"position":[0,0],"held":[],"pressed":[],"released":[],"wheel":[0,0]},"#,
            r#""gamepad":{"left_stick":[0,0],"right_stick":[0,0],"left_trigger":0,"#,
            r#""right_trigger":0,"held":[],"pressed":[],"released":[]}}"#,
        )
    );
}

#[test]
fn device_facts_round_trip_through_serde() {
    let mut input = InputState::default();
    tick(
        &mut input,
        &[
            connected(GamepadFamily::Nintendo),
            InputEvent::Focus { gained: false },
        ],
    );
    let value = serde_json::to_value(&input).unwrap();
    assert_eq!(value["gamepad"]["family"], json!("nintendo"));
    assert_eq!(serde_json::from_value::<InputState>(value).unwrap(), input);
}

#[test]
fn setting_the_device_facts_takes_them_without_edges_or_held_input() {
    let mut live = InputState::default();
    tick(
        &mut live,
        &[
            InputEvent::KeyDown { key: Key::W },
            connected(GamepadFamily::Playstation),
            axis(GamepadAxis::LeftStickX, 1.0),
            InputEvent::Focus { gained: false },
        ],
    );
    assert!(live.device_changed() && live.focus_changed());
    assert_eq!(
        live.devices(),
        DeviceFacts {
            pad: Some(GamepadFamily::Playstation),
            last_device: Device::Gamepad,
            focused: false,
        }
    );

    let mut fresh = InputState::default();
    fresh.apply(InputEvent::KeyDown { key: Key::A });
    fresh.set_devices(live.devices());
    assert_eq!(fresh.devices(), live.devices());
    assert!(fresh.gamepad().connected());
    assert_eq!(fresh.gamepad().family(), Some(GamepadFamily::Playstation));
    assert!(!fresh.device_changed() && !fresh.focus_changed());
    assert!(!fresh.is_held(Key::W) && fresh.is_held(Key::A));
    assert_eq!(fresh.gamepad().left_stick(), [0.0, 0.0]);

    // And back to nothing connected.
    fresh.set_devices(DeviceFacts::default());
    assert!(!fresh.gamepad().connected() && fresh.focused());
    assert_eq!(fresh.last_device(), Device::KeyboardMouse);
    assert!(!fresh.device_changed() && !fresh.focus_changed());
}

#[test]
fn the_default_facts_are_those_of_a_default_state() {
    assert_eq!(InputState::default().devices(), DeviceFacts::default());
    let mut input = InputState::default();
    input.set_devices(DeviceFacts::default());
    assert!(input.is_idle());
}

#[test]
fn device_facts_have_a_strict_serde_form() {
    // A connected pad under a keyboard player, which no single event says.
    let facts = DeviceFacts {
        pad: Some(GamepadFamily::Xbox),
        last_device: Device::KeyboardMouse,
        focused: true,
    };
    let text = serde_json::to_string(&facts).unwrap();
    assert_eq!(
        text,
        r#"{"pad":"xbox","last_device":"keyboard_mouse","focused":true}"#
    );
    assert_eq!(serde_json::from_str::<DeviceFacts>(&text).unwrap(), facts);
    for bad in [
        r#"{"pad":null,"focused":true}"#,
        r#"{"pad":null,"last_device":"gamepad","focused":true,"extra":1}"#,
        "pad",
    ] {
        assert!(serde_json::from_str::<DeviceFacts>(bad).is_err(), "{bad}");
    }
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, PartialEq, Debug, Default)]
struct Seen {
    gamepad: bool,
    connected: bool,
    focused: bool,
    changes: u32,
}

fn observe(world: &mut World, _ctx: &SystemCtx) {
    let input = world.resource::<InputState>().unwrap();
    let (gamepad, pad, focused) = (
        input.last_device() == Device::Gamepad,
        input.gamepad().connected(),
        input.focused(),
    );
    let changed = u32::from(input.device_changed()) + u32::from(input.focus_changed());
    for (seen,) in world.query::<(&mut Seen,)>() {
        seen.gamepad = gamepad;
        seen.connected = pad;
        seen.focused = focused;
        seen.changes += changed;
    }
}

fn trace(threads: usize) -> (Vec<WorldHash>, Seen) {
    par::with_threads(threads, || {
        let mut registry = Registry::new();
        registry.register::<Seen>("Seen").unwrap();
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Seen::default()).unwrap();
        let mut schedule = Schedule::new();
        schedule.add_system("observe", observe);
        let mut script = InputScript::new();
        script.push(FrameIndex::new(2), connected(GamepadFamily::Xbox));
        script.push(FrameIndex::new(4), motion(9.0, 0.0));
        script.push(FrameIndex::new(6), axis(GamepadAxis::RightStickX, 1.0));
        script.push(FrameIndex::new(8), InputEvent::Focus { gained: false });
        script.push(FrameIndex::new(10), InputEvent::GamepadDisconnected);
        script.push(FrameIndex::new(12), InputEvent::Focus { gained: true });
        let config = SimConfig {
            input: script,
            ..SimConfig::default()
        };
        let mut driver = SimDriver::new(world, schedule, config);
        let hashes = (0..16)
            .map(|_| {
                driver.run_ticks(1);
                WorldHash::compute(driver.world(), &registry).unwrap()
            })
            .collect();
        let seen = driver.world().get::<Seen>(entity).unwrap();
        (hashes, *seen.unwrap())
    })
}

#[test]
fn device_facts_give_identical_hash_traces_at_any_thread_count() {
    let (first, seen) = trace(1);
    assert_eq!(
        seen,
        Seen {
            gamepad: false,
            connected: false,
            focused: true,
            changes: 6,
        }
    );
    for threads in [1, 2, 8] {
        assert_eq!(trace(threads).0, first, "{threads} threads");
    }
}
