//! Headless-degrade proof (Phase 13 Wave 4, `Plan/phase-13-input.md` decision 5): mouse/gamepad
//! capture lives entirely out-of-sim in `ludion-window`. A headless engine — every agent/CI run, and
//! any `ludion serve` with no window attached — has nothing capturing input at all, so `InputState`
//! must stay at its quiescent default after ticking: mouse at `[0, 0]` with no buttons/wheel, zero
//! gamepad axes with no buttons held. This is the architectural half of the "capture out-of-sim, the
//! sim only ever sees injected data" guarantee: nothing here ever calls `input.inject`.

use ludion_engine::Engine;
use ludion_protocol::Command;
use ludion_protocol::command::SimStepParams;
use ludion_sim::{GamepadButton, InputState, Key, MouseButton};

fn step(engine: &mut Engine, ticks: u64) {
    engine
        .execute(Command::SimStep(SimStepParams { ticks, hash: false }))
        .expect("sim steps");
}

#[test]
fn headless_engine_leaves_input_state_at_default_after_ticking() {
    let mut engine = Engine::builder().build().expect("a bare engine builds");

    // Several seconds' worth of frames tick by with no window, no `input.inject` call at all —
    // exactly what every headless/CI run looks like.
    step(&mut engine, 180);

    let input = engine
        .world_mut()
        .resource::<InputState>()
        .expect("the driver seeds an InputState resource");

    // Keyboard: unchanged from Wave 1's already-true default.
    assert!(!input.is_held(Key::W));
    assert!(!input.was_pressed(Key::W));
    assert!(!input.was_released(Key::W));

    // Mouse: quiescent — no capture thread ever ran, so nothing moved or clicked.
    assert_eq!(input.mouse().position(), [0.0, 0.0]);
    assert!(!input.mouse().is_held(MouseButton::Left));
    assert!(!input.mouse().is_held(MouseButton::Right));
    assert!(!input.mouse().is_held(MouseButton::Middle));
    assert!(!input.mouse().was_pressed(MouseButton::Left));
    assert!(!input.mouse().was_released(MouseButton::Left));
    assert_eq!(input.mouse().wheel(), [0.0, 0.0]);

    // Gamepad: zero axes, no buttons — no `gilrs` poll ever ran.
    assert_eq!(input.gamepad().left_stick(), [0.0, 0.0]);
    assert_eq!(input.gamepad().right_stick(), [0.0, 0.0]);
    assert_eq!(input.gamepad().left_trigger(), 0.0);
    assert_eq!(input.gamepad().right_trigger(), 0.0);
    assert!(!input.gamepad().is_held(GamepadButton::South));
    assert!(!input.gamepad().was_pressed(GamepadButton::South));
}
