//! Runtime rebinding: direct edits, capture with conflict resolution, locked actions, lenient
//! overrides, the save flag, the latch after a binding, and determinism through a `SimDriver`.

use ludion_core::codes::code;
use ludion_core::time::{FrameIndex, TickRate};
use ludion_ecs::{Schedule, SystemCtx, World};
use ludion_sim::actions::{ActionMap, ActionName, Actions, BindingDoc, InputActionsDoc};
use ludion_sim::rebind::{CaptureOptions, CaptureResult, CaptureSlot, Resolve};
use ludion_sim::{
    GamepadAxis, GamepadButton, InputBindingsDoc, InputEvent, InputScript, InputState, Key,
    MouseButton, SimConfig, SimDriver,
};
use serde_json::{Value, json};

fn compile(value: Value) -> ActionMap {
    let doc: InputActionsDoc = serde_json::from_value(json!({ "actions": value })).unwrap();
    ActionMap::compile(&doc, TickRate::DEFAULT).expect("the map compiles")
}

fn button(bindings: Value) -> Value {
    json!({ "kind": "button", "bindings": bindings })
}

fn base() -> Value {
    json!({
        "jump": button(json!([{ "key": "Space" }, { "pad": "south", "gesture": "tap" }])),
        "fire": button(json!([{ "key": "F" }])),
        "use": button(json!([{ "key": "E" }, { "pad": "west" }])),
        "menu": { "kind": "button", "locked": true, "bindings": [{ "key": "Y" }] },
        "move": { "kind": "axis2", "bindings": [{ "stick": "left" }] },
    })
}

fn key(key: Key) -> BindingDoc {
    BindingDoc {
        key: Some(key),
        ..BindingDoc::default()
    }
}

struct Rig {
    actions: Actions,
    input: InputState,
}

impl Rig {
    fn new() -> Self {
        Rig {
            actions: Actions::new(compile(base())),
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
}

fn down(key: Key) -> InputEvent {
    InputEvent::KeyDown { key }
}

fn pad(button: GamepadButton) -> InputEvent {
    InputEvent::GamepadButtonDown { button }
}

fn codes(errors: &[ludion_core::diagnostics::Diagnostic]) -> Vec<&str> {
    errors.iter().map(|d| d.code.as_str()).collect()
}

#[test]
fn rebind_unbind_and_reset_round_trip_with_minimal_overrides() {
    let mut a = Actions::new(compile(base()));
    a.rebind("fire", 0, key(Key::G)).unwrap();
    assert_eq!(a.bindings("fire").unwrap(), [key(Key::G)]);
    assert_eq!(a.overrides().actions.len(), 1);

    a.rebind("jump", 2, key(Key::J)).unwrap(); // append
    assert_eq!(a.bindings("jump").unwrap().len(), 3);
    a.unbind("jump", 2).unwrap();
    assert!(
        a.overrides().actions.get(&"jump".to_owned()).is_none(),
        "equal to the default"
    );

    a.reset("fire").unwrap();
    assert!(a.overrides().actions.is_empty());

    a.rebind("fire", 0, key(Key::G)).unwrap();
    a.rebind("use", 0, key(Key::U)).unwrap();
    a.reset_all();
    assert!(a.overrides().actions.is_empty());
    assert_eq!(a.bindings("fire").unwrap(), [key(Key::F)]);
}

#[test]
fn rebind_acts_on_the_next_tick() {
    let mut rig = Rig::new();
    rig.actions.rebind("fire", 0, key(Key::G)).unwrap();
    rig.tick(&[down(Key::F)]);
    assert!(!rig.actions.pressed(ActionName::new("fire")));
    rig.tick(&[InputEvent::KeyUp { key: Key::F }, down(Key::G)]);
    assert!(rig.actions.pressed(ActionName::new("fire")));
}

#[test]
fn an_invalid_rebind_leaves_the_map_unchanged() {
    let mut a = Actions::new(compile(base()));
    let before = a.map().clone();
    // A duplicate of `use`'s E in the same context.
    let err = a.rebind("fire", 0, key(Key::E)).unwrap_err();
    assert_eq!(codes(&err), ["L1634"]);
    // A stick on a button action.
    let stick = BindingDoc {
        stick: Some(ludion_sim::actions::Side::Left),
        ..BindingDoc::default()
    };
    assert_eq!(codes(&a.rebind("fire", 0, stick).unwrap_err()), ["L1631"]);
    // The last binding cannot go.
    assert_eq!(codes(&a.unbind("fire", 0).unwrap_err()), ["L1635"]);
    assert_eq!(
        codes(&a.rebind("fire", 5, key(Key::H)).unwrap_err()),
        ["L4275"]
    );
    assert_eq!(
        codes(&a.rebind("nope", 0, key(Key::H)).unwrap_err()),
        ["L4270"]
    );
    assert_eq!(a.map(), &before);
    assert!(a.overrides().actions.is_empty());
}

#[test]
fn every_edit_of_a_locked_action_is_l4272() {
    let mut a = Actions::new(compile(base()));
    assert_eq!(
        codes(&a.rebind("menu", 0, key(Key::M)).unwrap_err()),
        ["L4272"]
    );
    assert_eq!(
        codes(&a.rebind("menu", 1, key(Key::M)).unwrap_err()),
        ["L4272"]
    );
    assert_eq!(codes(&a.unbind("menu", 0).unwrap_err()), ["L4272"]);
    assert_eq!(codes(&a.reset("menu").unwrap_err()), ["L4272"]);
    let err = a
        .capture("menu", CaptureSlot::Keyboard, CaptureOptions::default())
        .unwrap_err();
    assert_eq!(err.code.as_str(), "L4272");
    assert!(a.overrides().actions.is_empty());
}

#[test]
fn capture_refuses_axis_actions_and_bad_slots() {
    let mut a = Actions::new(compile(base()));
    let axis = a
        .capture("move", CaptureSlot::Keyboard, CaptureOptions::default())
        .unwrap_err();
    assert_eq!(axis.code.as_str(), "L4275");
    let slot = a
        .capture("fire", CaptureSlot::Index(2), CaptureOptions::default())
        .unwrap_err();
    assert_eq!(slot.code.as_str(), "L4275");
    assert_eq!(*a.capture_result(), CaptureResult::Idle);
}

#[test]
fn pad_capture_binds_east_as_tap_keeping_the_gesture() {
    let mut rig = Rig::new();
    rig.actions
        .capture("jump", CaptureSlot::Pad, CaptureOptions::default())
        .unwrap();
    assert_eq!(*rig.actions.capture_result(), CaptureResult::Pending);
    rig.tick(&[down(Key::X)]); // a key is not a pad press
    assert_eq!(*rig.actions.capture_result(), CaptureResult::Pending);
    rig.tick(&[pad(GamepadButton::East)]);
    assert_eq!(*rig.actions.capture_result(), CaptureResult::Bound);
    let bound = &rig.actions.bindings("jump").unwrap()[1];
    assert_eq!(bound.pad, Some(GamepadButton::East));
    assert_eq!(bound.gesture, Some(ludion_sim::actions::Gesture::Tap));
}

#[test]
fn keyboard_capture_ignores_held_keys_binds_the_next_press_and_latches_it() {
    let mut rig = Rig::new();
    rig.tick(&[down(Key::Q)]); // held before the capture starts
    rig.actions
        .capture("fire", CaptureSlot::Keyboard, CaptureOptions::default())
        .unwrap();
    rig.tick(&[]);
    assert_eq!(
        *rig.actions.capture_result(),
        CaptureResult::Pending,
        "Q has no edge"
    );
    rig.tick(&[down(Key::Z)]);
    assert_eq!(*rig.actions.capture_result(), CaptureResult::Bound);
    assert_eq!(rig.actions.bindings("fire").unwrap(), [key(Key::Z)]);
    assert!(!rig.actions.pressed(ActionName::new("fire")));

    // Z is still held: latched until released and pressed again.
    rig.tick(&[]);
    assert!(!rig.actions.pressed(ActionName::new("fire")));
    assert!(!rig.actions.held(ActionName::new("fire")));
    rig.tick(&[InputEvent::KeyUp { key: Key::Z }]);
    rig.tick(&[down(Key::Z)]);
    assert!(rig.actions.pressed(ActionName::new("fire")));
}

#[test]
fn escape_cancels_a_capture() {
    let mut rig = Rig::new();
    rig.actions
        .capture("fire", CaptureSlot::Keyboard, CaptureOptions::default())
        .unwrap();
    rig.tick(&[down(Key::Escape)]);
    assert_eq!(*rig.actions.capture_result(), CaptureResult::Cancelled);
    assert_eq!(rig.actions.bindings("fire").unwrap(), [key(Key::F)]);
    // A later press binds nothing.
    rig.tick(&[down(Key::Z)]);
    assert_eq!(rig.actions.bindings("fire").unwrap(), [key(Key::F)]);
}

#[test]
fn a_trigger_pull_binds_a_trigger() {
    let mut rig = Rig::new();
    rig.actions
        .capture("fire", CaptureSlot::Pad, CaptureOptions::default())
        .unwrap();
    rig.tick(&[InputEvent::GamepadAxis {
        axis: GamepadAxis::RightTrigger,
        value: 1.0,
    }]);
    assert_eq!(*rig.actions.capture_result(), CaptureResult::Bound);
    let bound = rig.actions.bindings("fire").unwrap();
    assert_eq!(bound.len(), 2);
    assert_eq!(bound[1].trigger, Some(ludion_sim::actions::Side::Right));
}

#[test]
fn two_presses_in_one_tick_pick_the_smaller_by_ord() {
    let mut rig = Rig::new();
    rig.actions
        .capture("fire", CaptureSlot::Index(0), CaptureOptions::default())
        .unwrap();
    let mouse = InputEvent::MouseDown {
        button: MouseButton::Left,
    };
    // Keys sort before mouse buttons, and `A` before `Z`.
    rig.tick(&[down(Key::Z), down(Key::A), mouse, pad(GamepadButton::North)]);
    assert_eq!(rig.actions.bindings("fire").unwrap(), [key(Key::A)]);
}

#[test]
fn a_conflict_swaps_both_bindings() {
    let mut rig = Rig::new();
    rig.actions
        .capture("fire", CaptureSlot::Keyboard, CaptureOptions::default())
        .unwrap();
    rig.tick(&[down(Key::E)]);
    assert_eq!(
        *rig.actions.capture_result(),
        CaptureResult::Conflict { with: "use".into() }
    );
    assert_eq!(
        rig.actions.bindings("fire").unwrap(),
        [key(Key::F)],
        "nothing applied yet"
    );
    rig.actions.resolve_conflict(Resolve::Swap).unwrap();
    assert_eq!(*rig.actions.capture_result(), CaptureResult::Bound);
    assert_eq!(rig.actions.bindings("fire").unwrap(), [key(Key::E)]);
    assert_eq!(rig.actions.bindings("use").unwrap()[0], key(Key::F));
    assert_eq!(rig.actions.overrides().actions.len(), 2);
}

#[test]
fn replace_removes_the_other_binding_unless_it_was_the_last() {
    let mut rig = Rig::new();
    rig.actions
        .capture("fire", CaptureSlot::Keyboard, CaptureOptions::default())
        .unwrap();
    rig.tick(&[down(Key::E)]);
    rig.actions.resolve_conflict(Resolve::Replace).unwrap();
    assert_eq!(rig.actions.bindings("use").unwrap().len(), 1);
    assert_eq!(
        rig.actions.bindings("use").unwrap()[0].pad,
        Some(GamepadButton::West)
    );

    // `use` now has only West; capturing West for `fire` would strip its last binding.
    rig.tick(&[InputEvent::KeyUp { key: Key::E }]);
    rig.actions
        .capture("fire", CaptureSlot::Pad, CaptureOptions::default())
        .unwrap();
    rig.tick(&[pad(GamepadButton::West)]);
    assert!(matches!(
        rig.actions.capture_result(),
        CaptureResult::Conflict { .. }
    ));
    let err = rig.actions.resolve_conflict(Resolve::Replace).unwrap_err();
    assert_eq!(codes(&err), ["L4275"]);
    // Still pending: Swap works (fire had no pad binding, so it appended; use would lose it too).
    let err = rig.actions.resolve_conflict(Resolve::Swap).unwrap_err();
    assert_eq!(codes(&err), ["L4275"]);
    rig.actions.resolve_conflict(Resolve::Keep).unwrap();
    assert_eq!(*rig.actions.capture_result(), CaptureResult::Cancelled);
    assert_eq!(
        codes(&rig.actions.resolve_conflict(Resolve::Keep).unwrap_err()),
        ["L4276"]
    );
}

#[test]
fn keep_cancels_and_a_locked_conflict_allows_only_keep() {
    let mut rig = Rig::new();
    rig.actions
        .capture("fire", CaptureSlot::Keyboard, CaptureOptions::default())
        .unwrap();
    rig.tick(&[down(Key::Y)]); // `menu`'s locked key
    assert_eq!(
        *rig.actions.capture_result(),
        CaptureResult::Conflict {
            with: "menu".into()
        }
    );
    assert_eq!(
        codes(&rig.actions.resolve_conflict(Resolve::Swap).unwrap_err()),
        ["L4272"]
    );
    assert_eq!(
        codes(&rig.actions.resolve_conflict(Resolve::Replace).unwrap_err()),
        ["L4272"]
    );
    rig.actions.resolve_conflict(Resolve::Keep).unwrap();
    assert_eq!(*rig.actions.capture_result(), CaptureResult::Cancelled);
    assert!(rig.actions.overrides().actions.is_empty());
}

#[test]
fn pressing_an_existing_binding_is_unchanged() {
    let mut rig = Rig::new();
    rig.actions
        .capture("fire", CaptureSlot::Keyboard, CaptureOptions::default())
        .unwrap();
    rig.tick(&[down(Key::F)]);
    assert_eq!(*rig.actions.capture_result(), CaptureResult::Unchanged);
}

#[test]
fn resolving_with_no_capture_is_l4276() {
    let mut a = Actions::new(compile(base()));
    assert_eq!(
        codes(&a.resolve_conflict(Resolve::Swap).unwrap_err()),
        ["L4276"]
    );
}

#[test]
fn apply_overrides_is_lenient_per_action() {
    let mut a = Actions::new(compile(base()));
    let doc: InputBindingsDoc = serde_json::from_value(json!({ "actions": {
        "fire": [{ "key": "G" }],
        "ghost": [{ "key": "H" }],
        "menu": [{ "key": "M" }],
        "use": [{ "key": "E", "stick": "left" }],
    }}))
    .unwrap();
    let warnings = a.apply_overrides(&doc);
    let mut got: Vec<(&str, &str)> = warnings
        .iter()
        .map(|w| (w.code.as_str(), w.path.as_deref().unwrap()))
        .collect();
    got.sort_unstable();
    assert_eq!(
        got,
        [
            ("L4272", "/actions/menu"),
            ("L4273", "/actions/ghost"),
            ("L4274", "/actions/use"),
        ]
    );
    assert_eq!(a.bindings("fire").unwrap(), [key(Key::G)]);
    assert_eq!(
        a.bindings("use").unwrap(),
        compile(base())
            .doc()
            .actions
            .get(&"use".to_owned())
            .unwrap()
            .bindings
    );
    assert_eq!(a.overrides().actions.len(), 1);
}

#[test]
fn a_duplicate_between_overrides_drops_one_action() {
    let mut a = Actions::new(compile(base()));
    let doc: InputBindingsDoc = serde_json::from_value(json!({ "actions": {
        "fire": [{ "key": "G" }],
        "use": [{ "key": "G" }],
    }}))
    .unwrap();
    let warnings = a.apply_overrides(&doc);
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code, code::INPUT_OVERRIDE_INVALID);
    assert_eq!(a.overrides().actions.len(), 1);
}

#[test]
fn save_is_a_one_shot_flag() {
    let mut a = Actions::new(compile(base()));
    assert!(!a.take_save_request());
    a.save();
    a.save();
    assert!(a.take_save_request());
    assert!(!a.take_save_request());
}

#[test]
fn contains_never_reports_unknown() {
    let mut a = Actions::new(compile(base()));
    assert!(a.contains(ActionName::new("fire")));
    assert!(!a.contains(ActionName::new("nope")));
    assert!(a.take_unknown().is_empty());
}

fn capture_system(world: &mut World, ctx: &SystemCtx) {
    let actions = world.resource_mut::<Actions>().unwrap();
    match ctx.frame.as_u64() {
        5 => actions
            .capture("fire", CaptureSlot::Keyboard, CaptureOptions::default())
            .unwrap(),
        7 => {
            if matches!(actions.capture_result(), CaptureResult::Conflict { .. }) {
                actions.resolve_conflict(Resolve::Swap).unwrap();
            }
        }
        _ => {}
    }
}

fn trace() -> Vec<(bool, bool)> {
    let mut script = InputScript::new();
    script.push(FrameIndex::new(6), down(Key::E));
    script.push(FrameIndex::new(9), InputEvent::KeyUp { key: Key::E });
    script.push(FrameIndex::new(12), down(Key::E));
    script.push(FrameIndex::new(14), down(Key::F));
    let mut world = World::new();
    world.insert_resource(Actions::new(compile(base())));
    let mut schedule = Schedule::new();
    schedule.add_system("capture", capture_system);
    let mut driver = SimDriver::new(
        world,
        schedule,
        SimConfig {
            input: script,
            ..SimConfig::default()
        },
    );
    (0..20)
        .map(|_| {
            driver.advance();
            let a = driver.world().resource::<Actions>().unwrap();
            (
                a.pressed(ActionName::new("fire")),
                a.pressed(ActionName::new("use")),
            )
        })
        .collect()
}

#[test]
fn a_scripted_capture_and_swap_replays_identically() {
    let (a, b) = (trace(), trace());
    assert_eq!(a, b);
    assert!(a.iter().any(|t| t.0), "fire fired on its new key");
    assert!(a.iter().any(|t| t.1), "use fired on its swapped key");
}
