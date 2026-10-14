//! Mapping a winit keyboard event to the engine's abstract key name.
//!
//! The sim's input model (`ludion_sim::Key`, mirrored here without a dependency on that crate) is
//! deliberately small and layout-independent: a key is an abstract name (`"W"`, `"Left"`, `"Space"`,
//! `"Digit1"`) and an event is `{"kind": "key_down" | "key_up", "key": "<name>"}`. winit reports the
//! *physical* key as a [`KeyCode`] — the same layout-independent identity — so this maps `KeyCode`,
//! never the produced character. Keys the engine has no name for are ignored, and OS auto-repeat is
//! suppressed so a held key produces exactly one `key_down` edge, matching the engine's own no-repeat
//! edge semantics (the TS viewer's `keymap.ts` does the same).

use winit::event::ElementState;
use winit::keyboard::KeyCode;

/// A keyboard edge translated to the wire's key vocabulary: a press/release kind plus the engine key
/// name. Both fields are the exact strings the `input.inject` event carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WireKey {
    /// `"key_down"` or `"key_up"`.
    pub kind: &'static str,
    /// The engine's abstract key name (a `ludion_sim::Key` serde name).
    pub key: &'static str,
}

impl WireKey {
    /// The `input.inject` event `Value` for this edge — the point where the input thread's channel
    /// (generalized in Phase 13 Wave 4 to carry any device's pre-built event `Value`) stops caring
    /// that this edge came from a keyboard at all.
    pub fn to_value(self) -> serde_json::Value {
        serde_json::json!({ "kind": self.kind, "key": self.key })
    }
}

/// Translates a physical key transition into the wire event to inject, or `None` when nothing should
/// be sent: an unmapped key (the engine has no such key), or an auto-repeat press (the engine treats
/// a re-asserted held key as no new edge, so forwarding repeats would be pure noise). A repeat
/// *release* is impossible from winit but, if seen, still maps — only presses are repeat-suppressed.
pub fn wire_key_event(code: KeyCode, state: ElementState, repeat: bool) -> Option<WireKey> {
    if state == ElementState::Pressed && repeat {
        return None;
    }
    let key = key_name(code)?;
    let kind = match state {
        ElementState::Pressed => "key_down",
        ElementState::Released => "key_up",
    };
    Some(WireKey { kind, key })
}

/// Maps a winit [`KeyCode`] to the engine's abstract key name, or `None` if the engine's
/// milestone-minimal key set has no such key. Covers the four families the set defines: `KeyA`–`KeyZ`,
/// `Digit0`–`Digit9`, the four arrows, and Space/Enter/Escape.
fn key_name(code: KeyCode) -> Option<&'static str> {
    let name = match code {
        KeyCode::KeyA => "A",
        KeyCode::KeyB => "B",
        KeyCode::KeyC => "C",
        KeyCode::KeyD => "D",
        KeyCode::KeyE => "E",
        KeyCode::KeyF => "F",
        KeyCode::KeyG => "G",
        KeyCode::KeyH => "H",
        KeyCode::KeyI => "I",
        KeyCode::KeyJ => "J",
        KeyCode::KeyK => "K",
        KeyCode::KeyL => "L",
        KeyCode::KeyM => "M",
        KeyCode::KeyN => "N",
        KeyCode::KeyO => "O",
        KeyCode::KeyP => "P",
        KeyCode::KeyQ => "Q",
        KeyCode::KeyR => "R",
        KeyCode::KeyS => "S",
        KeyCode::KeyT => "T",
        KeyCode::KeyU => "U",
        KeyCode::KeyV => "V",
        KeyCode::KeyW => "W",
        KeyCode::KeyX => "X",
        KeyCode::KeyY => "Y",
        KeyCode::KeyZ => "Z",
        KeyCode::Digit0 => "Digit0",
        KeyCode::Digit1 => "Digit1",
        KeyCode::Digit2 => "Digit2",
        KeyCode::Digit3 => "Digit3",
        KeyCode::Digit4 => "Digit4",
        KeyCode::Digit5 => "Digit5",
        KeyCode::Digit6 => "Digit6",
        KeyCode::Digit7 => "Digit7",
        KeyCode::Digit8 => "Digit8",
        KeyCode::Digit9 => "Digit9",
        KeyCode::ArrowUp => "Up",
        KeyCode::ArrowDown => "Down",
        KeyCode::ArrowLeft => "Left",
        KeyCode::ArrowRight => "Right",
        KeyCode::Space => "Space",
        KeyCode::Enter => "Enter",
        KeyCode::Escape => "Escape",
        _ => return None,
    };
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn down(code: KeyCode) -> Option<WireKey> {
        wire_key_event(code, ElementState::Pressed, false)
    }

    #[test]
    fn every_letter_maps_to_its_uppercase_name() {
        let letters = [
            (KeyCode::KeyA, "A"),
            (KeyCode::KeyB, "B"),
            (KeyCode::KeyC, "C"),
            (KeyCode::KeyD, "D"),
            (KeyCode::KeyE, "E"),
            (KeyCode::KeyF, "F"),
            (KeyCode::KeyG, "G"),
            (KeyCode::KeyH, "H"),
            (KeyCode::KeyI, "I"),
            (KeyCode::KeyJ, "J"),
            (KeyCode::KeyK, "K"),
            (KeyCode::KeyL, "L"),
            (KeyCode::KeyM, "M"),
            (KeyCode::KeyN, "N"),
            (KeyCode::KeyO, "O"),
            (KeyCode::KeyP, "P"),
            (KeyCode::KeyQ, "Q"),
            (KeyCode::KeyR, "R"),
            (KeyCode::KeyS, "S"),
            (KeyCode::KeyT, "T"),
            (KeyCode::KeyU, "U"),
            (KeyCode::KeyV, "V"),
            (KeyCode::KeyW, "W"),
            (KeyCode::KeyX, "X"),
            (KeyCode::KeyY, "Y"),
            (KeyCode::KeyZ, "Z"),
        ];
        for (code, name) in letters {
            assert_eq!(
                down(code),
                Some(WireKey {
                    kind: "key_down",
                    key: name
                })
            );
        }
    }

    #[test]
    fn every_digit_maps_to_its_digit_name() {
        let digits = [
            (KeyCode::Digit0, "Digit0"),
            (KeyCode::Digit1, "Digit1"),
            (KeyCode::Digit2, "Digit2"),
            (KeyCode::Digit3, "Digit3"),
            (KeyCode::Digit4, "Digit4"),
            (KeyCode::Digit5, "Digit5"),
            (KeyCode::Digit6, "Digit6"),
            (KeyCode::Digit7, "Digit7"),
            (KeyCode::Digit8, "Digit8"),
            (KeyCode::Digit9, "Digit9"),
        ];
        for (code, name) in digits {
            assert_eq!(
                down(code),
                Some(WireKey {
                    kind: "key_down",
                    key: name
                })
            );
        }
    }

    #[test]
    fn arrows_and_named_keys_map() {
        let named = [
            (KeyCode::ArrowUp, "Up"),
            (KeyCode::ArrowDown, "Down"),
            (KeyCode::ArrowLeft, "Left"),
            (KeyCode::ArrowRight, "Right"),
            (KeyCode::Space, "Space"),
            (KeyCode::Enter, "Enter"),
            (KeyCode::Escape, "Escape"),
        ];
        for (code, name) in named {
            assert_eq!(
                down(code),
                Some(WireKey {
                    kind: "key_down",
                    key: name
                })
            );
        }
    }

    #[test]
    fn release_maps_to_key_up() {
        assert_eq!(
            wire_key_event(KeyCode::Space, ElementState::Released, false),
            Some(WireKey {
                kind: "key_up",
                key: "Space"
            }),
        );
    }

    #[test]
    fn unmapped_keys_are_ignored() {
        for code in [
            KeyCode::Tab,
            KeyCode::F1,
            KeyCode::ShiftLeft,
            KeyCode::NumpadEnter,
        ] {
            assert_eq!(down(code), None);
        }
    }

    #[test]
    fn auto_repeat_press_is_suppressed_but_release_is_not() {
        // A held key re-asserts as a repeat press: no new edge should be forwarded.
        assert_eq!(
            wire_key_event(KeyCode::KeyW, ElementState::Pressed, true),
            None
        );
        // Repeat only suppresses presses; a release always maps.
        assert_eq!(
            wire_key_event(KeyCode::KeyW, ElementState::Released, true),
            Some(WireKey {
                kind: "key_up",
                key: "W"
            }),
        );
    }
}
