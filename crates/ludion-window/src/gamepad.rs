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
//! Only the first connected pad is forwarded (a settled out-of-scope, `Plan/phase-13-input.md`); the
//! poll loop in `lib.rs` owns which pad that is, and announces it with `gamepad_connected` and its
//! [`family_name`]. When it unplugs, [`on_disconnect`] releases whatever it held (the OS reports no
//! release for an unplugged pad), says `gamepad_disconnected` and promotes the next connected pad.

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

/// The engine axis name and the clamped value for a `gilrs` axis sample, or `None` for an unmapped axis.
fn axis_sample(axis: Axis, value: f32) -> Option<(&'static str, f32)> {
    Some((axis_name(axis)?, clamp_axis_value(axis, value)))
}

/// The `input.inject` event `Value` for an axis level, in the Wave 1 pinned shape.
fn axis_value(name: &str, value: f32) -> serde_json::Value {
    serde_json::json!({
        "kind": "gamepad_axis",
        "axis": name,
        "value_bits": value.to_bits(),
    })
}

/// The `input.inject` event `Value` for a button edge (`kind` is `gamepad_button_down`/`_up`).
fn button_value(kind: &str, name: &str) -> serde_json::Value {
    serde_json::json!({"kind": kind, "button": name})
}

/// The engine's pad family name for a USB vendor id.
pub fn family_name(vendor: Option<u16>) -> &'static str {
    match vendor {
        Some(0x045E) => "xbox",
        Some(0x054C) => "playstation",
        Some(0x057E) => "nintendo",
        _ => "generic",
    }
}

/// The `input.inject` event `Value` announcing the primary pad.
pub fn connected_value(family: &str) -> serde_json::Value {
    serde_json::json!({"kind": "gamepad_connected", "family": family})
}

/// The `input.inject` event `Value` for the primary pad leaving.
pub fn disconnected_value() -> serde_json::Value {
    serde_json::json!({"kind": "gamepad_disconnected"})
}

/// The axes and buttons the primary pad last forwarded as non-neutral, so an unplug (the OS reports
/// no release for what the pad was holding) can neutralise them. Trigger buttons are not tracked: the
/// sim synthesises them from the trigger axis, so zeroing the axis releases them.
#[derive(Default)]
pub struct PadHeld {
    /// The last forwarded (clamped) value per engine axis name.
    axes: Vec<(&'static str, f32)>,
    /// The buttons forwarded as down, in press order.
    buttons: Vec<&'static str>,
}

impl PadHeld {
    /// Maps a primary-pad `gilrs` event to the event `Value` to inject (`None`: nothing to forward)
    /// and records what it leaves held.
    pub fn forward(&mut self, event: &gilrs::EventType) -> Option<serde_json::Value> {
        match *event {
            gilrs::EventType::AxisChanged(axis, value, _) => {
                let (name, clamped) = axis_sample(axis, value)?;
                match self.axes.iter_mut().find(|(n, _)| *n == name) {
                    Some(slot) => slot.1 = clamped,
                    None => self.axes.push((name, clamped)),
                }
                Some(axis_value(name, clamped))
            }
            gilrs::EventType::ButtonPressed(button, _) => {
                let name = button_name(button)?;
                if !self.buttons.contains(&name) {
                    self.buttons.push(name);
                }
                Some(button_value("gamepad_button_down", name))
            }
            gilrs::EventType::ButtonReleased(button, _) => {
                let name = button_name(button)?;
                self.buttons.retain(|&held| held != name);
                Some(button_value("gamepad_button_up", name))
            }
            _ => None,
        }
    }

    /// A zero for every axis last forwarded non-zero (fixed vocabulary order), then a `button_up` per
    /// held button in press order; forgets them all.
    fn release(&mut self) -> Vec<serde_json::Value> {
        const ORDER: [&str; 6] = [
            "left_stick_x",
            "left_stick_y",
            "right_stick_x",
            "right_stick_y",
            "left_trigger",
            "right_trigger",
        ];
        let mut out = Vec::new();
        for name in ORDER {
            if self
                .axes
                .iter()
                .any(|&(axis, value)| axis == name && value != 0.0)
            {
                out.push(axis_value(name, 0.0));
            }
        }
        self.axes.clear();
        out.extend(
            self.buttons
                .drain(..)
                .map(|name| button_value("gamepad_button_up", name)),
        );
        out
    }
}

/// Handles pad `gone` disconnecting. When it is the primary, promotes the first `connected` pad
/// other than `gone` (or none) and returns the events to inject: the releases for everything it
/// held, then `gamepad_disconnected`, then the promoted pad's `gamepad_connected` with the family
/// `family_of` gives it. Any other pad changes nothing. Generic over the pad id so tests need no
/// real `gilrs::GamepadId`.
pub fn on_disconnect<Id: Copy + PartialEq>(
    primary: &mut Option<Id>,
    gone: Id,
    held: &mut PadHeld,
    connected: impl IntoIterator<Item = Id>,
    family_of: impl Fn(Id) -> &'static str,
) -> Vec<serde_json::Value> {
    if *primary != Some(gone) {
        return Vec::new();
    }
    let mut events = held.release();
    events.push(disconnected_value());
    *primary = connected.into_iter().find(|&id| id != gone);
    events.extend(primary.map(|id| connected_value(family_of(id))));
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use gilrs::EventType;
    use gilrs::ev::Code;

    /// `PadHeld` ignores the raw code; any constructible one will do.
    fn code() -> Code {
        Button::South.to_nec().unwrap()
    }

    fn axis(axis: Axis, value: f32) -> EventType {
        EventType::AxisChanged(axis, value, code())
    }

    // The mapping, observed through the one production path.
    fn axis_event(a: Axis, value: f32) -> Option<serde_json::Value> {
        PadHeld::default().forward(&axis(a, value))
    }

    fn button_down_event(button: Button) -> Option<serde_json::Value> {
        PadHeld::default().forward(&EventType::ButtonPressed(button, code()))
    }

    fn button_up_event(button: Button) -> Option<serde_json::Value> {
        PadHeld::default().forward(&EventType::ButtonReleased(button, code()))
    }

    fn zero(name: &str) -> serde_json::Value {
        axis_value(name, 0.0)
    }

    /// A pad holding stick X full right, the right trigger at 0.8 and South.
    fn busy_pad() -> PadHeld {
        let mut held = PadHeld::default();
        held.forward(&axis(Axis::LeftStickX, 1.0));
        held.forward(&axis(Axis::RightZ, 0.8));
        held.forward(&EventType::ButtonPressed(Button::South, code()));
        held
    }

    /// Pad 2 is a Nintendo pad; any other is generic.
    fn family_of(id: i32) -> &'static str {
        if id == 2 { "nintendo" } else { "generic" }
    }

    #[test]
    fn primary_unplug_releases_then_disconnects_then_announces_the_promoted_pad() {
        let mut held = busy_pad();
        let mut primary = Some(1);
        let out = on_disconnect(&mut primary, 1, &mut held, [1, 2], family_of);
        assert_eq!(
            out,
            vec![
                zero("left_stick_x"),
                zero("right_trigger"),
                serde_json::json!({"kind": "gamepad_button_up", "button": "south"}),
                serde_json::json!({"kind": "gamepad_disconnected"}),
                serde_json::json!({"kind": "gamepad_connected", "family": "nintendo"}),
            ]
        );
        assert_eq!(primary, Some(2));
        assert!(held.release().is_empty());
    }

    #[test]
    fn a_non_primary_unplug_changes_nothing() {
        let mut held = busy_pad();
        let mut primary = Some(1);
        assert!(on_disconnect(&mut primary, 2, &mut held, [1, 2], family_of).is_empty());
        assert_eq!(primary, Some(1));
        assert_eq!(held.release().len(), 3);
    }

    #[test]
    fn promotion_is_none_when_no_pad_is_left() {
        let mut held = busy_pad();
        let mut primary = Some(1);
        let out = on_disconnect(&mut primary, 1, &mut held, [1], family_of);
        assert_eq!(primary, None);
        assert_eq!(out.len(), 4);
        assert_eq!(out.last(), Some(&disconnected_value()));
    }

    #[test]
    fn an_axis_returned_to_zero_is_not_resent() {
        let mut held = PadHeld::default();
        held.forward(&axis(Axis::LeftStickX, 1.0));
        held.forward(&axis(Axis::LeftStickX, 0.0));
        let mut primary = Some(1);
        assert_eq!(
            on_disconnect(&mut primary, 1, &mut held, [], family_of),
            vec![disconnected_value()]
        );
    }

    #[test]
    fn the_vendor_id_names_the_family() {
        for (vendor, family) in [
            (Some(0x045E), "xbox"),
            (Some(0x054C), "playstation"),
            (Some(0x057E), "nintendo"),
            (Some(0x046D), "generic"),
            (None, "generic"),
        ] {
            assert_eq!(family_name(vendor), family, "{vendor:?}");
        }
    }

    #[test]
    fn a_released_button_is_not_released_again() {
        let mut held = PadHeld::default();
        held.forward(&EventType::ButtonPressed(Button::East, code()));
        held.forward(&EventType::ButtonReleased(Button::East, code()));
        assert!(held.release().is_empty());
    }

    #[test]
    fn trigger_buttons_are_never_forwarded_or_released() {
        let mut held = PadHeld::default();
        assert_eq!(
            held.forward(&EventType::ButtonPressed(Button::RightTrigger2, code())),
            None
        );
        held.forward(&axis(Axis::LeftZ, 1.0));
        let out = held.release();
        assert_eq!(out, vec![zero("left_trigger")]);
        assert!(out.iter().all(|e| e["kind"] == "gamepad_axis"));
    }

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
