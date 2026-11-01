//! Mapping a winit keyboard event to the engine's abstract key name.
//!
//! The sim's input model (`ludion_sim::Key`, mirrored here without a dependency on that crate) is
//! deliberately small and layout-independent: a key is an abstract name (`"W"`, `"Left"`, `"Space"`,
//! `"Digit1"`) and an event is `{"kind": "key_down" | "key_up", "key": "<name>"}`. winit reports the
//! *physical* key as a [`KeyCode`] — the same layout-independent identity — so this maps `KeyCode`,
//! never the produced character. Keys the engine has no name for are ignored, and OS auto-repeat is
//! suppressed so a held key produces exactly one `key_down` edge, matching the engine's own no-repeat
//! edge semantics (the TS viewer's `keymap.ts` does the same).
//!
//! The modifiers (`"Shift"`, `"Ctrl"`, `"Alt"`) are folded: left and right are one engine key, held
//! while either side is down. That makes [`KeyMapper`] the one stateful piece — it remembers which
//! sides are down so a second side or the release of one of two never sends a spurious edge, and
//! which other keys it forwarded as down so a focus loss can release them all.

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

/// The three folded modifiers, in the order `release_held` reports them.
const MODIFIERS: [&str; 3] = ["Shift", "Ctrl", "Alt"];

/// The modifier a [`KeyCode`] belongs to (index into [`MODIFIERS`]) and which physical side it is.
fn modifier_side(code: KeyCode) -> Option<(usize, usize)> {
    Some(match code {
        KeyCode::ShiftLeft => (0, 0),
        KeyCode::ShiftRight => (0, 1),
        KeyCode::ControlLeft => (1, 0),
        KeyCode::ControlRight => (1, 1),
        KeyCode::AltLeft => (2, 0),
        KeyCode::AltRight => (2, 1),
        _ => return None,
    })
}

/// Translates physical key transitions into the wire events to inject. Stateful for what a focus
/// loss must release: the modifiers (left and right fold into one engine key, so the mapper tracks
/// which sides are down and emits an edge only when the folded key changes — releasing one of two
/// held Shifts sends nothing) and the other keys it forwarded as down.
#[derive(Default)]
pub struct KeyMapper {
    /// Per modifier (Shift, Ctrl, Alt), whether its left / right side is down.
    down: [[bool; 2]; 3],
    /// The non-modifier keys forwarded as down, in press order.
    keys: Vec<&'static str>,
}

impl KeyMapper {
    /// Translates a physical key transition into the wire event to inject, or `None` when nothing
    /// should be sent: an unmapped key (the engine has no such key), an auto-repeat press (the engine
    /// treats a re-asserted held key as no new edge, so forwarding repeats would be pure noise), or a
    /// modifier edge that does not change the folded key. A repeat *release* is impossible from winit
    /// but, if seen, still maps — only presses are repeat-suppressed.
    pub fn map(&mut self, code: KeyCode, state: ElementState, repeat: bool) -> Option<WireKey> {
        if state == ElementState::Pressed && repeat {
            return None;
        }
        let pressed = state == ElementState::Pressed;
        if let Some((modifier, side)) = modifier_side(code) {
            let sides = &mut self.down[modifier];
            if sides[side] == pressed {
                return None;
            }
            sides[side] = pressed;
            if sides[1 - side] {
                return None;
            }
            let kind = if pressed { "key_down" } else { "key_up" };
            return Some(WireKey {
                kind,
                key: MODIFIERS[modifier],
            });
        }
        let key = key_name(code)?;
        if pressed {
            if !self.keys.contains(&key) {
                self.keys.push(key);
            }
        } else {
            self.keys.retain(|&held| held != key);
        }
        let kind = if pressed { "key_down" } else { "key_up" };
        Some(WireKey { kind, key })
    }

    /// Releases every held key (window focus lost: the OS delivers no key-up for a key let go
    /// elsewhere, e.g. Alt-Tab) and forgets them: one `key_up` per held folded modifier, then one per
    /// held key in press order.
    pub fn release_held(&mut self) -> Vec<WireKey> {
        let mut out = Vec::new();
        for (modifier, sides) in self.down.iter_mut().enumerate() {
            if sides.iter().any(|&down| down) {
                out.push(WireKey {
                    kind: "key_up",
                    key: MODIFIERS[modifier],
                });
            }
            *sides = [false; 2];
        }
        out.extend(self.keys.drain(..).map(|key| WireKey {
            kind: "key_up",
            key,
        }));
        out
    }
}

/// Maps a winit [`KeyCode`] to the engine's abstract key name, or `None` if the engine's
/// milestone-minimal key set has no such key. Covers the four families the set defines: `KeyA`–`KeyZ`,
/// `Digit0`–`Digit9`, the four arrows, and Space/Enter/Escape. Modifiers go through [`KeyMapper`].
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
        KeyMapper::default().map(code, ElementState::Pressed, false)
    }

    fn edge(kind: &'static str, key: &'static str) -> Option<WireKey> {
        Some(WireKey { kind, key })
    }

    #[test]
    fn each_modifier_side_maps_to_its_folded_name() {
        for (code, name) in [
            (KeyCode::ShiftLeft, "Shift"),
            (KeyCode::ShiftRight, "Shift"),
            (KeyCode::ControlLeft, "Ctrl"),
            (KeyCode::ControlRight, "Ctrl"),
            (KeyCode::AltLeft, "Alt"),
            (KeyCode::AltRight, "Alt"),
        ] {
            assert_eq!(down(code), edge("key_down", name));
        }
    }

    #[test]
    fn two_shift_sides_fold_into_one_held_key() {
        let mut keys = KeyMapper::default();
        let press = ElementState::Pressed;
        let release = ElementState::Released;
        assert_eq!(
            keys.map(KeyCode::ShiftLeft, press, false),
            edge("key_down", "Shift")
        );
        assert_eq!(keys.map(KeyCode::ShiftRight, press, false), None);
        assert_eq!(keys.map(KeyCode::ShiftLeft, release, false), None);
        assert_eq!(
            keys.map(KeyCode::ShiftRight, release, false),
            edge("key_up", "Shift")
        );
        // A release of a side never recorded down sends nothing.
        assert_eq!(keys.map(KeyCode::ShiftLeft, release, false), None);
    }

    #[test]
    fn focus_loss_releases_held_modifiers_once() {
        let mut keys = KeyMapper::default();
        keys.map(KeyCode::ShiftLeft, ElementState::Pressed, false);
        keys.map(KeyCode::AltRight, ElementState::Pressed, false);
        assert_eq!(
            keys.release_held(),
            vec![
                WireKey {
                    kind: "key_up",
                    key: "Shift"
                },
                WireKey {
                    kind: "key_up",
                    key: "Alt"
                }
            ]
        );
        assert_eq!(
            keys.map(KeyCode::AltRight, ElementState::Released, false),
            None
        );
        assert!(keys.release_held().is_empty());
    }

    #[test]
    fn focus_loss_releases_held_keys_in_press_order() {
        let mut keys = KeyMapper::default();
        keys.map(KeyCode::KeyW, ElementState::Pressed, false);
        keys.map(KeyCode::Space, ElementState::Pressed, false);
        assert_eq!(
            keys.release_held(),
            vec![
                WireKey {
                    kind: "key_up",
                    key: "W"
                },
                WireKey {
                    kind: "key_up",
                    key: "Space"
                }
            ]
        );
        assert!(keys.release_held().is_empty());
    }

    #[test]
    fn a_key_released_before_focus_loss_is_not_released_again() {
        let mut keys = KeyMapper::default();
        keys.map(KeyCode::KeyW, ElementState::Pressed, false);
        keys.map(KeyCode::KeyW, ElementState::Released, false);
        assert!(keys.release_held().is_empty());
    }

    #[test]
    fn focus_loss_releases_modifiers_before_keys() {
        let mut keys = KeyMapper::default();
        keys.map(KeyCode::KeyD, ElementState::Pressed, false);
        keys.map(KeyCode::ControlLeft, ElementState::Pressed, false);
        let names: Vec<_> = keys.release_held().iter().map(|k| k.key).collect();
        assert_eq!(names, ["Ctrl", "D"]);
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
            KeyMapper::default().map(KeyCode::Space, ElementState::Released, false),
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
            KeyCode::CapsLock,
            KeyCode::NumpadEnter,
        ] {
            assert_eq!(down(code), None);
        }
    }

    #[test]
    fn auto_repeat_press_is_suppressed_but_release_is_not() {
        // A held key re-asserts as a repeat press: no new edge should be forwarded.
        assert_eq!(
            KeyMapper::default().map(KeyCode::KeyW, ElementState::Pressed, true),
            None
        );
        // Repeat only suppresses presses; a release always maps.
        assert_eq!(
            KeyMapper::default().map(KeyCode::KeyW, ElementState::Released, true),
            Some(WireKey {
                kind: "key_up",
                key: "W"
            }),
        );
    }
}
