//! Prompt glyph names: what to draw for a binding on the device the player is using.
//!
//! A glyph name is an atlas frame name. The engine ships the names only; the art, and the atlas
//! that holds it, are the game's. Read an action's glyphs with
//! [`Actions::glyphs`](crate::Actions::glyphs), or name one binding with [`glyph_name`].
//!
//! | source | name |
//! |---|---|
//! | `key` | `key.<snake>`: the [`Key`] serde name in `snake_case` (`key.e`, `key.digit1`, `key.page_up`, `key.f12`) |
//! | `mouse` | `mouse.left`, `right`, `middle`, `back` or `forward` |
//! | `wheel` | `mouse.wheel_up`, `wheel_down`, `wheel_x` or `wheel_y` |
//! | `mouse_delta` | `mouse.move` |
//! | `pad`, `trigger`, `stick` | `pad.<family>.<label>`, family `xbox`, `playstation`, `nintendo` or `generic` |
//!
//! Key names are physical (QWERTY) positions, not the labels of the player's keyboard layout.
//!
//! The face buttons are labelled per family:
//!
//! | button | xbox | playstation | nintendo | generic |
//! |---|---|---|---|---|
//! | `South` | `a` | `cross` | `b` | `south` |
//! | `East` | `b` | `circle` | `a` | `east` |
//! | `West` | `x` | `square` | `y` | `west` |
//! | `North` | `y` | `triangle` | `x` | `north` |
//!
//! Every other label is the same in each family: `lb`, `rb`, `lt`, `rt` (the trigger buttons and
//! `trigger: left|right` alike), `ls`, `rs` (stick clicks), `start`, `select`, `dpad_up`,
//! `dpad_down`, `dpad_left`, `dpad_right`, and for an analog stick `left_stick` or `right_stick`.
//!
//! A composite binding is one glyph per physical input: `keys` gives up, down, left, right, and
//! `key_pair` and `pad_pair` give negative, then positive.

use crate::actions::{ActionKind, BindingDoc, Gesture, Side, Source, WheelSource, sources};
use crate::input::{Device, GamepadButton, GamepadFamily, Key, MouseButton};

/// One thing to draw in a prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glyph {
    /// The atlas frame name.
    pub name: &'static str,
    /// How a button binding is read ([`Gesture::Press`] when the binding names none), so a prompt
    /// can say "hold"; `None` for an analog or composite axis binding.
    pub gesture: Option<Gesture>,
}

/// The glyph name of a binding that is one physical input, with pad labels for `family`.
///
/// `None` for a composite (`keys`, `key_pair`, `pad_pair`) and for a binding that does not name
/// exactly one source.
///
/// # Example
/// ```
/// use ludion_sim::actions::BindingDoc;
/// use ludion_sim::{GamepadButton, GamepadFamily, glyph_name};
///
/// let south = BindingDoc { pad: Some(GamepadButton::South), ..BindingDoc::default() };
/// assert_eq!(glyph_name(&south, GamepadFamily::Xbox), Some("pad.xbox.a"));
/// assert_eq!(glyph_name(&south, GamepadFamily::Nintendo), Some("pad.nintendo.b"));
/// ```
pub fn glyph_name(binding: &BindingDoc, family: GamepadFamily) -> Option<&'static str> {
    match single_source(binding)? {
        Source::Keys(_) | Source::KeyPair(_) | Source::PadPair(_) => None,
        _ => binding_glyph_names(binding, family).next(),
    }
}

fn single_source(binding: &BindingDoc) -> Option<Source> {
    match sources(binding)[..] {
        [(_, source)] => Some(source),
        _ => None,
    }
}

/// Every glyph name of a binding, composites expanded; nothing for an invalid binding.
pub(crate) fn binding_glyph_names(
    binding: &BindingDoc,
    family: GamepadFamily,
) -> impl Iterator<Item = &'static str> + use<> {
    let one = |name| [Some(name), None, None, None];
    let two = |negative, positive| [Some(negative), Some(positive), None, None];
    let names = match single_source(binding) {
        None => [None; 4],
        Some(Source::Key(key)) => one(key_glyph(key)),
        Some(Source::Mouse(button)) => one(mouse_glyph(button)),
        Some(Source::Pad(button)) => one(pad_glyph(button, family)),
        Some(Source::Trigger(side)) => one(pad_glyph(
            match side {
                Side::Left => GamepadButton::LeftTrigger,
                Side::Right => GamepadButton::RightTrigger,
            },
            family,
        )),
        Some(Source::Wheel(wheel)) => one(match wheel {
            WheelSource::Up => "mouse.wheel_up",
            WheelSource::Down => "mouse.wheel_down",
            WheelSource::X => "mouse.wheel_x",
            WheelSource::Y => "mouse.wheel_y",
        }),
        Some(Source::Stick(side)) => one(stick_glyph(side, family)),
        Some(Source::Keys(c)) => [c.up, c.down, c.left, c.right].map(|k| Some(key_glyph(k))),
        Some(Source::KeyPair([negative, positive])) => {
            two(key_glyph(negative), key_glyph(positive))
        }
        Some(Source::PadPair([negative, positive])) => {
            two(pad_glyph(negative, family), pad_glyph(positive, family))
        }
        Some(Source::MouseDelta) => one("mouse.move"),
    };
    names.into_iter().flatten()
}

/// The [`Glyph::gesture`] of a binding.
pub(crate) fn binding_gesture(binding: &BindingDoc) -> Option<Gesture> {
    single_source(binding)?
        .fits(ActionKind::Button)
        .then(|| binding.gesture.unwrap_or(Gesture::Press))
}

/// The device a binding is read from, or `None` for an invalid binding.
pub(crate) fn binding_device(binding: &BindingDoc) -> Option<Device> {
    Some(match single_source(binding)? {
        Source::Key(_)
        | Source::Mouse(_)
        | Source::Wheel(_)
        | Source::Keys(_)
        | Source::KeyPair(_)
        | Source::MouseDelta => Device::KeyboardMouse,
        Source::Pad(_) | Source::Trigger(_) | Source::Stick(_) | Source::PadPair(_) => {
            Device::Gamepad
        }
    })
}

/// `pad.<family>.<label>` for a label every family shares.
macro_rules! shared_label {
    ($family:expr, $label:literal) => {
        match $family {
            GamepadFamily::Xbox => concat!("pad.xbox.", $label),
            GamepadFamily::Playstation => concat!("pad.playstation.", $label),
            GamepadFamily::Nintendo => concat!("pad.nintendo.", $label),
            GamepadFamily::Generic => concat!("pad.generic.", $label),
        }
    };
}

fn pad_glyph(button: GamepadButton, family: GamepadFamily) -> &'static str {
    match button {
        GamepadButton::South => match family {
            GamepadFamily::Xbox => "pad.xbox.a",
            GamepadFamily::Playstation => "pad.playstation.cross",
            GamepadFamily::Nintendo => "pad.nintendo.b",
            GamepadFamily::Generic => "pad.generic.south",
        },
        GamepadButton::East => match family {
            GamepadFamily::Xbox => "pad.xbox.b",
            GamepadFamily::Playstation => "pad.playstation.circle",
            GamepadFamily::Nintendo => "pad.nintendo.a",
            GamepadFamily::Generic => "pad.generic.east",
        },
        GamepadButton::West => match family {
            GamepadFamily::Xbox => "pad.xbox.x",
            GamepadFamily::Playstation => "pad.playstation.square",
            GamepadFamily::Nintendo => "pad.nintendo.y",
            GamepadFamily::Generic => "pad.generic.west",
        },
        GamepadButton::North => match family {
            GamepadFamily::Xbox => "pad.xbox.y",
            GamepadFamily::Playstation => "pad.playstation.triangle",
            GamepadFamily::Nintendo => "pad.nintendo.x",
            GamepadFamily::Generic => "pad.generic.north",
        },
        GamepadButton::LeftShoulder => shared_label!(family, "lb"),
        GamepadButton::RightShoulder => shared_label!(family, "rb"),
        GamepadButton::LeftTrigger => shared_label!(family, "lt"),
        GamepadButton::RightTrigger => shared_label!(family, "rt"),
        GamepadButton::LeftStick => shared_label!(family, "ls"),
        GamepadButton::RightStick => shared_label!(family, "rs"),
        GamepadButton::Start => shared_label!(family, "start"),
        GamepadButton::Select => shared_label!(family, "select"),
        GamepadButton::DpadUp => shared_label!(family, "dpad_up"),
        GamepadButton::DpadDown => shared_label!(family, "dpad_down"),
        GamepadButton::DpadLeft => shared_label!(family, "dpad_left"),
        GamepadButton::DpadRight => shared_label!(family, "dpad_right"),
    }
}

fn stick_glyph(side: Side, family: GamepadFamily) -> &'static str {
    match side {
        Side::Left => shared_label!(family, "left_stick"),
        Side::Right => shared_label!(family, "right_stick"),
    }
}

fn mouse_glyph(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "mouse.left",
        MouseButton::Right => "mouse.right",
        MouseButton::Middle => "mouse.middle",
        MouseButton::Back => "mouse.back",
        MouseButton::Forward => "mouse.forward",
    }
}

fn key_glyph(key: Key) -> &'static str {
    match key {
        Key::A => "key.a",
        Key::B => "key.b",
        Key::C => "key.c",
        Key::D => "key.d",
        Key::E => "key.e",
        Key::F => "key.f",
        Key::G => "key.g",
        Key::H => "key.h",
        Key::I => "key.i",
        Key::J => "key.j",
        Key::K => "key.k",
        Key::L => "key.l",
        Key::M => "key.m",
        Key::N => "key.n",
        Key::O => "key.o",
        Key::P => "key.p",
        Key::Q => "key.q",
        Key::R => "key.r",
        Key::S => "key.s",
        Key::T => "key.t",
        Key::U => "key.u",
        Key::V => "key.v",
        Key::W => "key.w",
        Key::X => "key.x",
        Key::Y => "key.y",
        Key::Z => "key.z",
        Key::Digit0 => "key.digit0",
        Key::Digit1 => "key.digit1",
        Key::Digit2 => "key.digit2",
        Key::Digit3 => "key.digit3",
        Key::Digit4 => "key.digit4",
        Key::Digit5 => "key.digit5",
        Key::Digit6 => "key.digit6",
        Key::Digit7 => "key.digit7",
        Key::Digit8 => "key.digit8",
        Key::Digit9 => "key.digit9",
        Key::Up => "key.up",
        Key::Down => "key.down",
        Key::Left => "key.left",
        Key::Right => "key.right",
        Key::Space => "key.space",
        Key::Enter => "key.enter",
        Key::Escape => "key.escape",
        Key::Shift => "key.shift",
        Key::Ctrl => "key.ctrl",
        Key::Alt => "key.alt",
        Key::Tab => "key.tab",
        Key::Backspace => "key.backspace",
        Key::CapsLock => "key.caps_lock",
        Key::F1 => "key.f1",
        Key::F2 => "key.f2",
        Key::F3 => "key.f3",
        Key::F4 => "key.f4",
        Key::F5 => "key.f5",
        Key::F6 => "key.f6",
        Key::F7 => "key.f7",
        Key::F8 => "key.f8",
        Key::F9 => "key.f9",
        Key::F10 => "key.f10",
        Key::F11 => "key.f11",
        Key::F12 => "key.f12",
        Key::Minus => "key.minus",
        Key::Equal => "key.equal",
        Key::BracketLeft => "key.bracket_left",
        Key::BracketRight => "key.bracket_right",
        Key::Backslash => "key.backslash",
        Key::Semicolon => "key.semicolon",
        Key::Quote => "key.quote",
        Key::Backquote => "key.backquote",
        Key::Comma => "key.comma",
        Key::Period => "key.period",
        Key::Slash => "key.slash",
        Key::Insert => "key.insert",
        Key::Delete => "key.delete",
        Key::Home => "key.home",
        Key::End => "key.end",
        Key::PageUp => "key.page_up",
        Key::PageDown => "key.page_down",
        Key::Numpad0 => "key.numpad0",
        Key::Numpad1 => "key.numpad1",
        Key::Numpad2 => "key.numpad2",
        Key::Numpad3 => "key.numpad3",
        Key::Numpad4 => "key.numpad4",
        Key::Numpad5 => "key.numpad5",
        Key::Numpad6 => "key.numpad6",
        Key::Numpad7 => "key.numpad7",
        Key::Numpad8 => "key.numpad8",
        Key::Numpad9 => "key.numpad9",
        Key::NumpadAdd => "key.numpad_add",
        Key::NumpadSubtract => "key.numpad_subtract",
        Key::NumpadMultiply => "key.numpad_multiply",
        Key::NumpadDivide => "key.numpad_divide",
        Key::NumpadDecimal => "key.numpad_decimal",
        Key::NumpadEnter => "key.numpad_enter",
    }
}
