//! Mapping `gilrs` gamepad samples to the engine's abstract gamepad vocabulary, mirroring
//! `keymap.rs`/`mouse.rs`: hand-built event `Value`s, no dependency on `ludion_sim`.
//!
//! `gilrs` names buttons/axes after the SDL game-controller model, which the engine's
//! `GamepadButton`/`GamepadAxis` (Plan/phase-13-input.md decision 1) already mirrors, with one
//! naming wrinkle: `gilrs` reports the analog triggers *twice* — as a digital `Button::LeftTrigger2`/
//! `RightTrigger2` (pressed past a threshold) and as a continuous `Axis::LeftZ`/`RightZ`. The engine's
//! vocabulary takes each trigger as an axis (`GamepadAxis::LeftTrigger`/`RightTrigger`, level
//! `[0, 1]`) and synthesises its own `GamepadButton::LeftTrigger`/`RightTrigger` from that axis at
//! `TRIGGER_THRESHOLD`, so gilrs's digital trigger buttons (its own threshold) are deliberately
//! unmapped here — one source per trigger. `Button::LeftTrigger`/
//! `RightTrigger` (the *shoulder* buttons, SDL `LB`/`RB`) map to `"left_shoulder"`/`"right_shoulder"`
//! instead, which is the standard gilrs/SDL naming (shoulder = `Trigger`, analog trigger =
//! `Trigger2`). `Axis::DPadX`/`DPadY` (a hat reported as an axis pair on some backends) are also
//! unmapped: the engine has no d-pad axis, only d-pad buttons, and the button path is what every
//! platform this crate targets (XInput on Windows, IOKit on macOS) actually reports.
//!
//! Only the first connected pad is forwarded (a settled out-of-scope, `Plan/phase-13-input.md`); this
//! module maps a single sample at a time and has no notion of which pad is "first" — that bookkeeping
//! is the poll loop's job in `lib.rs`.

use gilrs::{Axis, Button};

/// Maps a `gilrs` axis to the engine's abstract axis name, or `None` for an axis the engine's vocabulary
/// has no name for (`LeftZ`/`RightZ` are handled by the caller as triggers via [`clamp_axis_value`],
/// not through this map; `DPadX`/`DPadY`/`Unknown` have no engine equivalent).
fn axis_name(axis: Axis) -> Option<&'static str> {
    match axis {
        Axis::LeftStickX => Some("left_stick_x"),
        Axis::LeftStickY => Some("left_stick_y"),
        Axis::RightStickX => Some("right_stick_x"),
        Axis::RightStickY => Some("right_stick_y"),
        Axis::LeftZ => Some("left_trigger"),
        Axis::RightZ => Some("right_trigger"),
        Axis::DPadX | Axis::DPadY | Axis::Unknown => None,
    }
}

/// Clamps a raw axis sample to the wire's documented range: sticks to `[-1, 1]`, triggers
/// (`LeftZ`/`RightZ`) to `[0, 1]` — defensive, since driver/platform normalization is not guaranteed
/// exact at the extremes.
fn clamp_axis_value(axis: Axis, value: f32) -> f32 {
    match axis {
        Axis::LeftZ | Axis::RightZ => value.clamp(0.0, 1.0),
        _ => value.clamp(-1.0, 1.0),
    }
}

/// Maps a `gilrs` button to the engine's abstract button name, or `None` for one with no engine
/// equivalent (`C`, `Z`, `Mode`, the analog trigger buttons `LeftTrigger2`/`RightTrigger2`, which the
/// engine synthesises from the axis — see the module docs — and `Unknown`).
fn button_name(button: Button) -> Option<&'static str> {
    match button {
        Button::South => Some("south"),
        Button::East => Some("east"),
        Button::West => Some("west"),
        Button::North => Some("north"),
        Button::LeftTrigger => Some("left_shoulder"),
        Button::RightTrigger => Some("right_shoulder"),
        Button::LeftThumb => Some("left_stick"),
        Button::RightThumb => Some("right_stick"),
        Button::Start => Some("start"),
        Button::Select => Some("select"),
        Button::DPadUp => Some("dpad_up"),
        Button::DPadDown => Some("dpad_down"),
        Button::DPadLeft => Some("dpad_left"),
        Button::DPadRight => Some("dpad_right"),
        Button::C
        | Button::Z
        | Button::Mode
        | Button::LeftTrigger2
        | Button::RightTrigger2
        | Button::Unknown => None,
    }
}

/// Builds the `input.inject` event `Value` for an axis sample, in the Wave 1 pinned shape, or `None`
/// for an axis the engine has no name for.
pub fn axis_event(axis: Axis, value: f32) -> Option<serde_json::Value> {
    let name = axis_name(axis)?;
    let clamped = clamp_axis_value(axis, value);
    Some(serde_json::json!({
        "kind": "gamepad_axis",
        "axis": name,
        "value_bits": clamped.to_bits(),
    }))
}

/// Builds the `input.inject` event `Value` for a button press, or `None` for an unmapped button.
pub fn button_down_event(button: Button) -> Option<serde_json::Value> {
    let name = button_name(button)?;
    Some(serde_json::json!({"kind": "gamepad_button_down", "button": name}))
}

/// Builds the `input.inject` event `Value` for a button release, or `None` for an unmapped button.
pub fn button_up_event(button: Button) -> Option<serde_json::Value> {
    let name = button_name(button)?;
    Some(serde_json::json!({"kind": "gamepad_button_up", "button": name}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stick_axes_map_to_the_pinned_shape() {
        assert_eq!(
            axis_event(Axis::LeftStickX, -0.75),
            Some(serde_json::json!({
                "kind": "gamepad_axis",
                "axis": "left_stick_x",
                "value_bits": (-0.75f32).to_bits(),
            }))
        );
        assert_eq!(
            axis_event(Axis::RightStickY, 0.5),
            Some(serde_json::json!({
                "kind": "gamepad_axis",
                "axis": "right_stick_y",
                "value_bits": 0.5f32.to_bits(),
            }))
        );
    }

    #[test]
    fn trigger_axes_map_and_clamp_to_zero_one() {
        assert_eq!(
            axis_event(Axis::LeftZ, 1.0),
            Some(serde_json::json!({
                "kind": "gamepad_axis",
                "axis": "left_trigger",
                "value_bits": 1.0f32.to_bits(),
            }))
        );
        // Out-of-range on the low side (a driver quirk): clamps to 0.0, never a negative trigger.
        assert_eq!(
            axis_event(Axis::RightZ, -0.2),
            Some(serde_json::json!({
                "kind": "gamepad_axis",
                "axis": "right_trigger",
                "value_bits": 0.0f32.to_bits(),
            }))
        );
    }

    #[test]
    fn stick_axes_clamp_to_minus_one_one() {
        assert_eq!(
            axis_event(Axis::LeftStickX, 1.5),
            Some(serde_json::json!({
                "kind": "gamepad_axis",
                "axis": "left_stick_x",
                "value_bits": 1.0f32.to_bits(),
            }))
        );
        assert_eq!(
            axis_event(Axis::LeftStickY, -1.5),
            Some(serde_json::json!({
                "kind": "gamepad_axis",
                "axis": "left_stick_y",
                "value_bits": (-1.0f32).to_bits(),
            }))
        );
    }

    #[test]
    fn dpad_and_unknown_axes_are_unmapped() {
        assert_eq!(axis_event(Axis::DPadX, 1.0), None);
        assert_eq!(axis_event(Axis::DPadY, 1.0), None);
        assert_eq!(axis_event(Axis::Unknown, 1.0), None);
    }

    #[test]
    fn face_and_shoulder_buttons_map_to_the_pinned_shape() {
        assert_eq!(
            button_down_event(Button::South),
            Some(serde_json::json!({"kind": "gamepad_button_down", "button": "south"}))
        );
        assert_eq!(
            button_up_event(Button::LeftTrigger),
            Some(serde_json::json!({"kind": "gamepad_button_up", "button": "left_shoulder"}))
        );
        assert_eq!(
            button_down_event(Button::DPadUp),
            Some(serde_json::json!({"kind": "gamepad_button_down", "button": "dpad_up"}))
        );
    }

    #[test]
    fn analog_trigger_buttons_and_unknown_are_unmapped() {
        // The digital trigger-button events are dropped; the axis events (Axis::LeftZ/RightZ) carry
        // the trigger's value instead (see the module docs).
        assert_eq!(button_down_event(Button::LeftTrigger2), None);
        assert_eq!(button_down_event(Button::RightTrigger2), None);
        assert_eq!(button_down_event(Button::Mode), None);
        assert_eq!(button_down_event(Button::Unknown), None);
    }
}
