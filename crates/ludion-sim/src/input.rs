//! Input scripts and per-tick input state.
//!
//! Input reaches the simulation as **data addressed to frame indices**, never as live OS events:
//! an [`InputScript`] is a serializable list of [`ScriptedInput`]s ("frame 10: key `W` down"),
//! and the driver applies every event addressed to frame *N* at the start of tick *N*, before any
//! system runs. Systems then read the resulting [`InputState`] resource. Because the script is
//! plain data and application happens at a fixed point in the tick, a scripted run is exactly
//! reproducible — this is the substrate `ludion replay` and scripted gameplay tests stand on.
//!
//! This module is deliberately serde-only: no YAML, no OS input backend, no wall clock. Types
//! serialize to JSON-compatible shapes so scripts can travel through the wire protocol and the
//! replay format unchanged.

use ludion_core::collections::StableSet;
use ludion_core::time::FrameIndex;
use serde::{Deserialize, Serialize};

/// A key on the keyboard, as an abstract, layout-independent name.
///
/// This is the milestone-minimal set (letters, digits, arrows, and the few control keys a
/// Breakout/Sokoban-class game needs); variants are appended as games require them. The serde form
/// is the variant name as a string (`"W"`, `"Left"`, `"Space"`), which is what appears in scripts
/// and on the wire.
///
/// # Example
/// ```
/// use ludion_sim::Key;
/// assert_eq!(serde_json::to_string(&Key::Left).unwrap(), "\"Left\"");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum Key {
    /// The `A` key.
    A,
    /// The `B` key.
    B,
    /// The `C` key.
    C,
    /// The `D` key.
    D,
    /// The `E` key.
    E,
    /// The `F` key.
    F,
    /// The `G` key.
    G,
    /// The `H` key.
    H,
    /// The `I` key.
    I,
    /// The `J` key.
    J,
    /// The `K` key.
    K,
    /// The `L` key.
    L,
    /// The `M` key.
    M,
    /// The `N` key.
    N,
    /// The `O` key.
    O,
    /// The `P` key.
    P,
    /// The `Q` key.
    Q,
    /// The `R` key.
    R,
    /// The `S` key.
    S,
    /// The `T` key.
    T,
    /// The `U` key.
    U,
    /// The `V` key.
    V,
    /// The `W` key.
    W,
    /// The `X` key.
    X,
    /// The `Y` key.
    Y,
    /// The `Z` key.
    Z,
    /// The `0` key on the main row.
    Digit0,
    /// The `1` key on the main row.
    Digit1,
    /// The `2` key on the main row.
    Digit2,
    /// The `3` key on the main row.
    Digit3,
    /// The `4` key on the main row.
    Digit4,
    /// The `5` key on the main row.
    Digit5,
    /// The `6` key on the main row.
    Digit6,
    /// The `7` key on the main row.
    Digit7,
    /// The `8` key on the main row.
    Digit8,
    /// The `9` key on the main row.
    Digit9,
    /// The up arrow.
    Up,
    /// The down arrow.
    Down,
    /// The left arrow.
    Left,
    /// The right arrow.
    Right,
    /// The space bar.
    Space,
    /// The enter/return key.
    Enter,
    /// The escape key.
    Escape,
}

/// A mouse button, as an abstract name. The serde form is `snake_case`
/// (`"left"`, `"right"`, `"middle"`) — deliberately distinct from [`Key`]'s bare-variant-name form,
/// since these are new surface with no shipped-format compatibility to preserve.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    /// The primary (usually left) button.
    Left,
    /// The secondary (usually right) button.
    Right,
    /// The middle button, often the scroll wheel click.
    Middle,
}

/// A gamepad button, as an abstract, layout-independent name (the SDL/standard face, shoulder,
/// d-pad, stick-click, start/select set). Face buttons are named by position (`South`/`East`/
/// `West`/`North`), not by any one controller brand's letter, since the mapping is layout-agnostic.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GamepadButton {
    /// The bottom face button (Xbox `A`, PlayStation ✕).
    South,
    /// The right face button (Xbox `B`, PlayStation ○).
    East,
    /// The left face button (Xbox `X`, PlayStation ▢).
    West,
    /// The top face button (Xbox `Y`, PlayStation △).
    North,
    /// The left shoulder button.
    LeftShoulder,
    /// The right shoulder button.
    RightShoulder,
    /// The left stick click.
    LeftStick,
    /// The right stick click.
    RightStick,
    /// The start/menu button.
    Start,
    /// The select/back/share button.
    Select,
    /// D-pad up.
    DpadUp,
    /// D-pad down.
    DpadDown,
    /// D-pad left.
    DpadLeft,
    /// D-pad right.
    DpadRight,
}

/// A gamepad analog axis, as an abstract name. Each [`InputEvent::GamepadAxis`] names one of
/// these and carries its new level value — one generic event rather than six per-axis variants.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GamepadAxis {
    /// The left stick's horizontal axis, in `[-1, 1]`.
    LeftStickX,
    /// The left stick's vertical axis, in `[-1, 1]`.
    LeftStickY,
    /// The right stick's horizontal axis, in `[-1, 1]`.
    RightStickX,
    /// The right stick's vertical axis, in `[-1, 1]`.
    RightStickY,
    /// The left (analog) trigger, in `[0, 1]`.
    LeftTrigger,
    /// The right (analog) trigger, in `[0, 1]`.
    RightTrigger,
}

/// Serde helper: encodes an `f32` as its IEEE-754 bit pattern (a `u32`), never as a JSON number.
///
/// `serde_json` parses a JSON number through `f64` and casts down to `f32`; that decimal →
/// `f64` → `f32` double rounding is not guaranteed to reproduce the original bits. Analog input
/// must replay bit-exact, so every analog field crossing serde uses this instead of the natural
/// `f32` encoding — bit-exact by construction, not by a parser's luck.
mod f32_bits {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(value: &f32, serializer: S) -> Result<S::Ok, S::Error> {
        value.to_bits().serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
        u32::deserialize(deserializer).map(f32::from_bits)
    }
}

/// Serde helper: the two-`f32` sibling of [`f32_bits`], for `[f32; 2]` fields (mouse position and
/// wheel delta). Encodes as `[u32; 2]`, one bit pattern per component.
mod f32x2_bits {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(value: &[f32; 2], serializer: S) -> Result<S::Ok, S::Error> {
        [value[0].to_bits(), value[1].to_bits()].serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[f32; 2], D::Error> {
        let bits = <[u32; 2]>::deserialize(deserializer)?;
        Ok([f32::from_bits(bits[0]), f32::from_bits(bits[1])])
    }
}

/// One input event, as data.
///
/// The serde form is externally recognizable and hand-writable for keys:
/// `{"kind": "key_down", "key": "W"}`. Analog mouse/gamepad fields (positions, deltas, axis
/// values) encode as IEEE-754 bit patterns under a `*_bits` field name (the private `f32_bits`
/// serde helper) — not as
/// JSON numbers — so replay of analog input is bit-exact by construction, e.g.
/// `{"kind": "gamepad_axis", "axis": "left_stick_x", "value_bits": 1056964608}`.
///
/// # Example
/// ```
/// use ludion_sim::{InputEvent, Key};
///
/// let event = InputEvent::KeyDown { key: Key::W };
/// let json = serde_json::to_value(&event).unwrap();
/// assert_eq!(json, serde_json::json!({"kind": "key_down", "key": "W"}));
/// ```
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputEvent {
    /// A key transitioned (or was reasserted) down.
    KeyDown {
        /// Which key.
        key: Key,
    },
    /// A key was released.
    KeyUp {
        /// Which key.
        key: Key,
    },
    /// The mouse moved to a new render-target-pixel position (origin top-left, +Y down).
    MouseMove {
        /// The new position.
        #[serde(rename = "position_bits", with = "f32x2_bits")]
        position: [f32; 2],
    },
    /// A mouse button transitioned (or was reasserted) down.
    MouseDown {
        /// Which button.
        button: MouseButton,
    },
    /// A mouse button was released.
    MouseUp {
        /// Which button.
        button: MouseButton,
    },
    /// The mouse wheel moved; `delta` accumulates into this frame's
    /// [`MouseState::wheel`](crate::input::MouseState::wheel).
    MouseWheel {
        /// The wheel delta to accumulate.
        #[serde(rename = "delta_bits", with = "f32x2_bits")]
        delta: [f32; 2],
    },
    /// A gamepad analog axis was set to a new level value.
    GamepadAxis {
        /// Which axis.
        axis: GamepadAxis,
        /// The new value.
        #[serde(rename = "value_bits", with = "f32_bits")]
        value: f32,
    },
    /// A gamepad button transitioned (or was reasserted) down.
    GamepadButtonDown {
        /// Which button.
        button: GamepadButton,
    },
    /// A gamepad button was released.
    GamepadButtonUp {
        /// Which button.
        button: GamepadButton,
    },
}

/// One scripted input: an [`InputEvent`] addressed to the [`FrameIndex`] at which it applies.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct ScriptedInput {
    /// The frame at whose tick boundary the event applies (before that frame's systems run).
    pub frame: FrameIndex,
    /// The event itself.
    pub event: InputEvent,
}

/// A deterministic, serializable sequence of input events addressed to frame indices.
///
/// # Invariant
///
/// Events are always held **sorted by frame, stably** — events addressed to the same frame keep
/// their relative order. Every construction path (empty + [`push`](InputScript::push),
/// `From<Vec<ScriptedInput>>`, deserialization) canonicalizes to that order, so the driver can
/// apply events with a simple cursor and two scripts with the same events always behave — and
/// serialize — identically.
///
/// # Example
/// ```
/// use ludion_core::time::FrameIndex;
/// use ludion_sim::{InputEvent, InputScript, Key};
///
/// let mut script = InputScript::new();
/// script.push(FrameIndex::new(10), InputEvent::KeyDown { key: Key::W });
/// script.push(FrameIndex::new(30), InputEvent::KeyUp { key: Key::W });
///
/// // Round-trips through JSON as a plain array of {frame, event} records.
/// let json = serde_json::to_string(&script).unwrap();
/// let back: InputScript = serde_json::from_str(&json).unwrap();
/// assert_eq!(script, back);
/// ```
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(from = "Vec<ScriptedInput>", into = "Vec<ScriptedInput>")]
pub struct InputScript {
    events: Vec<ScriptedInput>,
}

impl InputScript {
    /// Creates an empty script.
    pub const fn new() -> Self {
        InputScript { events: Vec::new() }
    }

    /// Appends an event addressed to `frame`, keeping the script sorted.
    ///
    /// If the script already contains events for `frame`, the new event goes **after** them
    /// (stable order: push order is application order within a frame).
    pub fn push(&mut self, frame: FrameIndex, event: InputEvent) {
        let at = self.events.partition_point(|e| e.frame <= frame);
        self.events.insert(at, ScriptedInput { frame, event });
    }

    /// The events, sorted by frame (stable within a frame).
    pub fn events(&self) -> &[ScriptedInput] {
        &self.events
    }

    /// Removes the first `count` events — an already-applied prefix the driver's cursor has passed.
    ///
    /// The driver calls this at a tick boundary so a long-lived live session that injects an event
    /// every frame does not accumulate the script without bound. Only consumed events are dropped;
    /// the unapplied remainder keeps its sorted order, so the sim (and any replay of it) behaves
    /// identically with or without the compaction — the retained events and their meaning are
    /// unchanged.
    pub(crate) fn drain_applied(&mut self, count: usize) {
        self.events.drain(..count);
    }

    /// The number of events.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether the script has no events.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

impl From<Vec<ScriptedInput>> for InputScript {
    /// Canonicalizes an arbitrary event list: stable-sorts by frame, preserving the relative order
    /// of events addressed to the same frame.
    fn from(mut events: Vec<ScriptedInput>) -> Self {
        events.sort_by_key(|e| e.frame);
        InputScript { events }
    }
}

impl From<InputScript> for Vec<ScriptedInput> {
    fn from(script: InputScript) -> Self {
        script.events
    }
}

/// Mouse state: level position, edge button sets, and a per-frame wheel accumulator.
///
/// Mirrors [`InputState`]'s key vocabulary: [`is_held`](MouseState::is_held) is level,
/// [`was_pressed`](MouseState::was_pressed)/[`was_released`](MouseState::was_released) are
/// one-frame edges. [`wheel`](MouseState::wheel) accumulates every [`InputEvent::MouseWheel`]
/// delta applied this frame and resets to zero on the next [`InputState::begin_tick`] — it reads
/// like an edge (this frame's motion only), not a level.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MouseState {
    #[serde(with = "f32x2_bits")]
    position: [f32; 2],
    held: StableSet<MouseButton>,
    pressed: StableSet<MouseButton>,
    released: StableSet<MouseButton>,
    #[serde(with = "f32x2_bits")]
    wheel: [f32; 2],
}

impl MouseState {
    /// The current position, in render-target pixels (origin top-left, +Y down).
    pub fn position(&self) -> [f32; 2] {
        self.position
    }

    /// Whether `button` is currently down.
    pub fn is_held(&self, button: MouseButton) -> bool {
        self.held.contains(&button)
    }

    /// Whether `button` transitioned down this frame.
    pub fn was_pressed(&self, button: MouseButton) -> bool {
        self.pressed.contains(&button)
    }

    /// Whether `button` transitioned up this frame.
    pub fn was_released(&self, button: MouseButton) -> bool {
        self.released.contains(&button)
    }

    /// This frame's accumulated wheel delta (zero if the wheel did not move).
    pub fn wheel(&self) -> [f32; 2] {
        self.wheel
    }
}

/// Gamepad state: level analog sticks/triggers and edge button sets.
///
/// Mirrors [`InputState`]'s key vocabulary: sticks and triggers are level (persist across ticks,
/// set by [`InputEvent::GamepadAxis`]); buttons follow the same held/pressed/released edge rules
/// as keys.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GamepadState {
    #[serde(with = "f32x2_bits")]
    left_stick: [f32; 2],
    #[serde(with = "f32x2_bits")]
    right_stick: [f32; 2],
    #[serde(with = "f32_bits")]
    left_trigger: f32,
    #[serde(with = "f32_bits")]
    right_trigger: f32,
    held: StableSet<GamepadButton>,
    pressed: StableSet<GamepadButton>,
    released: StableSet<GamepadButton>,
}

impl GamepadState {
    /// The left stick's current `(x, y)`, each component in `[-1, 1]`.
    pub fn left_stick(&self) -> [f32; 2] {
        self.left_stick
    }

    /// The right stick's current `(x, y)`, each component in `[-1, 1]`.
    pub fn right_stick(&self) -> [f32; 2] {
        self.right_stick
    }

    /// The left trigger's current value, in `[0, 1]`.
    pub fn left_trigger(&self) -> f32 {
        self.left_trigger
    }

    /// The right trigger's current value, in `[0, 1]`.
    pub fn right_trigger(&self) -> f32 {
        self.right_trigger
    }

    /// Whether `button` is currently down.
    pub fn is_held(&self, button: GamepadButton) -> bool {
        self.held.contains(&button)
    }

    /// Whether `button` transitioned down this frame.
    pub fn was_pressed(&self, button: GamepadButton) -> bool {
        self.pressed.contains(&button)
    }

    /// Whether `button` transitioned up this frame.
    pub fn was_released(&self, button: GamepadButton) -> bool {
        self.released.contains(&button)
    }
}

/// The per-tick input state systems read, maintained by the driver as a `World` resource.
///
/// Before frame *N*'s systems run, the driver clears the edge sets and applies every scripted
/// event addressed to frame *N*. Systems then observe:
///
/// - [`is_held`](InputState::is_held) — the key is currently down (level).
/// - [`was_pressed`](InputState::was_pressed) — the key went down *this frame* (edge; a
///   `KeyDown` for an already-held key is **not** a new press).
/// - [`was_released`](InputState::was_released) — the key went up this frame (edge; a `KeyUp`
///   for a key that was not held is ignored).
///
/// [`mouse`](InputState::mouse) and [`gamepad`](InputState::gamepad) expose the same level/edge
/// vocabulary for those devices, under [`MouseState`] and [`GamepadState`].
///
/// # Example
/// ```
/// use ludion_sim::{InputEvent, InputState, Key};
///
/// let mut input = InputState::default();
/// input.begin_tick();
/// input.apply(InputEvent::KeyDown { key: Key::Space });
/// assert!(input.is_held(Key::Space));
/// assert!(input.was_pressed(Key::Space));
///
/// input.begin_tick(); // next frame: edges clear, level persists
/// assert!(input.is_held(Key::Space));
/// assert!(!input.was_pressed(Key::Space));
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InputState {
    held: StableSet<Key>,
    pressed: StableSet<Key>,
    released: StableSet<Key>,
    mouse: MouseState,
    gamepad: GamepadState,
}

impl InputState {
    /// Nothing held, no edges, analogs and cursor at rest — what a snapshot omits.
    pub fn is_idle(&self) -> bool {
        *self == Self::default()
    }

    /// Whether `key` is currently down.
    pub fn is_held(&self, key: Key) -> bool {
        self.held.contains(&key)
    }

    /// Whether `key` transitioned down this frame.
    pub fn was_pressed(&self, key: Key) -> bool {
        self.pressed.contains(&key)
    }

    /// Whether `key` transitioned up this frame.
    pub fn was_released(&self, key: Key) -> bool {
        self.released.contains(&key)
    }

    /// The mouse's state (level position, edge buttons, wheel accumulator).
    pub fn mouse(&self) -> &MouseState {
        &self.mouse
    }

    /// The gamepad's state (level sticks/triggers, edge buttons).
    pub fn gamepad(&self) -> &GamepadState {
        &self.gamepad
    }

    /// Starts a new tick: clears the pressed/released edge sets and the wheel accumulator. Held
    /// keys/buttons and level analogs (mouse position, gamepad sticks/triggers) persist.
    ///
    /// The driver calls this at every tick boundary before applying that frame's events; it is
    /// public so embedders and tests can drive an `InputState` by hand.
    pub fn begin_tick(&mut self) {
        self.pressed.clear();
        self.released.clear();
        self.mouse.pressed.clear();
        self.mouse.released.clear();
        self.mouse.wheel = [0.0, 0.0];
        self.gamepad.pressed.clear();
        self.gamepad.released.clear();
    }

    /// Applies one event.
    ///
    /// A `*Down` for an already-held key/button changes nothing (no repeat edges); a `*Up` for one
    /// that is not held is ignored. These rules make event application idempotent per level, so a
    /// script's meaning does not depend on OS-style auto-repeat quirks. `MouseMove`/`GamepadAxis`
    /// simply set the named level value; `MouseWheel` accumulates into this frame's wheel total.
    pub fn apply(&mut self, event: InputEvent) {
        match event {
            InputEvent::KeyDown { key } => {
                if self.held.insert(key) {
                    self.pressed.insert(key);
                }
            }
            InputEvent::KeyUp { key } => {
                if self.held.remove(&key) {
                    self.released.insert(key);
                }
            }
            InputEvent::MouseMove { position } => {
                self.mouse.position = position;
            }
            InputEvent::MouseDown { button } => {
                if self.mouse.held.insert(button) {
                    self.mouse.pressed.insert(button);
                }
            }
            InputEvent::MouseUp { button } => {
                if self.mouse.held.remove(&button) {
                    self.mouse.released.insert(button);
                }
            }
            InputEvent::MouseWheel { delta } => {
                self.mouse.wheel[0] += delta[0];
                self.mouse.wheel[1] += delta[1];
            }
            InputEvent::GamepadAxis { axis, value } => match axis {
                GamepadAxis::LeftStickX => self.gamepad.left_stick[0] = value,
                GamepadAxis::LeftStickY => self.gamepad.left_stick[1] = value,
                GamepadAxis::RightStickX => self.gamepad.right_stick[0] = value,
                GamepadAxis::RightStickY => self.gamepad.right_stick[1] = value,
                GamepadAxis::LeftTrigger => self.gamepad.left_trigger = value,
                GamepadAxis::RightTrigger => self.gamepad.right_trigger = value,
            },
            InputEvent::GamepadButtonDown { button } => {
                if self.gamepad.held.insert(button) {
                    self.gamepad.pressed.insert(button);
                }
            }
            InputEvent::GamepadButtonUp { button } => {
                if self.gamepad.held.remove(&button) {
                    self.gamepad.released.insert(button);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsorted_events_are_canonicalized_stably() {
        let down = |f: u64, key| ScriptedInput {
            frame: FrameIndex::new(f),
            event: InputEvent::KeyDown { key },
        };
        let up = |f: u64, key| ScriptedInput {
            frame: FrameIndex::new(f),
            event: InputEvent::KeyUp { key },
        };

        // Out of frame order, with two same-frame events whose relative order must survive.
        let script = InputScript::from(vec![down(30, Key::A), down(10, Key::W), up(10, Key::S)]);
        assert_eq!(
            script.events(),
            &[down(10, Key::W), up(10, Key::S), down(30, Key::A)]
        );
    }

    #[test]
    fn push_keeps_sorted_order_and_same_frame_push_order() {
        let mut script = InputScript::new();
        script.push(FrameIndex::new(20), InputEvent::KeyUp { key: Key::W });
        script.push(FrameIndex::new(5), InputEvent::KeyDown { key: Key::W });
        script.push(FrameIndex::new(5), InputEvent::KeyDown { key: Key::A });

        let frames: Vec<u64> = script.events().iter().map(|e| e.frame.as_u64()).collect();
        assert_eq!(frames, vec![5, 5, 20]);
        // Same-frame events stay in push order.
        assert_eq!(
            script.events()[1].event,
            InputEvent::KeyDown { key: Key::A }
        );
    }

    #[test]
    fn serde_form_is_a_plain_array_and_sorts_on_deserialize() {
        let json = r#"[
            {"frame": 30, "event": {"kind": "key_up", "key": "W"}},
            {"frame": 10, "event": {"kind": "key_down", "key": "W"}}
        ]"#;
        let script: InputScript = serde_json::from_str(json).unwrap();
        assert_eq!(script.events()[0].frame, FrameIndex::new(10));
        assert_eq!(script.events()[1].frame, FrameIndex::new(30));

        let back = serde_json::to_string(&script).unwrap();
        let reparsed: InputScript = serde_json::from_str(&back).unwrap();
        assert_eq!(script, reparsed);
    }

    #[test]
    fn press_and_release_edges() {
        let mut input = InputState::default();
        input.begin_tick();
        input.apply(InputEvent::KeyDown { key: Key::D });
        assert!(input.is_held(Key::D) && input.was_pressed(Key::D));
        assert!(!input.was_released(Key::D));

        // Repeated KeyDown while held: no new press edge.
        input.begin_tick();
        input.apply(InputEvent::KeyDown { key: Key::D });
        assert!(input.is_held(Key::D));
        assert!(!input.was_pressed(Key::D));

        input.begin_tick();
        input.apply(InputEvent::KeyUp { key: Key::D });
        assert!(!input.is_held(Key::D));
        assert!(input.was_released(Key::D));

        // KeyUp for a key that is not held: ignored.
        input.begin_tick();
        input.apply(InputEvent::KeyUp { key: Key::D });
        assert!(!input.was_released(Key::D));
    }

    #[test]
    fn mouse_button_press_and_release_edges() {
        let mut input = InputState::default();
        input.begin_tick();
        input.apply(InputEvent::MouseDown {
            button: MouseButton::Left,
        });
        assert!(input.mouse().is_held(MouseButton::Left));
        assert!(input.mouse().was_pressed(MouseButton::Left));
        assert!(!input.mouse().was_released(MouseButton::Left));

        // Repeated MouseDown while held: no new press edge.
        input.begin_tick();
        input.apply(InputEvent::MouseDown {
            button: MouseButton::Left,
        });
        assert!(input.mouse().is_held(MouseButton::Left));
        assert!(!input.mouse().was_pressed(MouseButton::Left));

        input.begin_tick();
        input.apply(InputEvent::MouseUp {
            button: MouseButton::Left,
        });
        assert!(!input.mouse().is_held(MouseButton::Left));
        assert!(input.mouse().was_released(MouseButton::Left));

        // MouseUp for a button that is not held: ignored.
        input.begin_tick();
        input.apply(InputEvent::MouseUp {
            button: MouseButton::Left,
        });
        assert!(!input.mouse().was_released(MouseButton::Left));
    }

    #[test]
    fn mouse_move_sets_position_and_persists_across_ticks() {
        let mut input = InputState::default();
        assert_eq!(input.mouse().position(), [0.0, 0.0]);

        input.begin_tick();
        input.apply(InputEvent::MouseMove {
            position: [12.5, -3.0],
        });
        assert_eq!(input.mouse().position(), [12.5, -3.0]);

        // Level state: still there next tick with no new event.
        input.begin_tick();
        assert_eq!(input.mouse().position(), [12.5, -3.0]);
    }

    #[test]
    fn mouse_wheel_accumulates_within_a_tick_and_clears_on_begin_tick() {
        let mut input = InputState::default();
        input.begin_tick();
        input.apply(InputEvent::MouseWheel { delta: [0.0, 1.0] });
        input.apply(InputEvent::MouseWheel { delta: [0.5, 2.0] });
        assert_eq!(input.mouse().wheel(), [0.5, 3.0]);

        // Next tick, with no new wheel event: cleared like an edge.
        input.begin_tick();
        assert_eq!(input.mouse().wheel(), [0.0, 0.0]);
    }

    #[test]
    fn gamepad_button_press_and_release_edges() {
        let mut input = InputState::default();
        input.begin_tick();
        input.apply(InputEvent::GamepadButtonDown {
            button: GamepadButton::South,
        });
        assert!(input.gamepad().is_held(GamepadButton::South));
        assert!(input.gamepad().was_pressed(GamepadButton::South));
        assert!(!input.gamepad().was_released(GamepadButton::South));

        // Repeated GamepadButtonDown while held: no new press edge.
        input.begin_tick();
        input.apply(InputEvent::GamepadButtonDown {
            button: GamepadButton::South,
        });
        assert!(input.gamepad().is_held(GamepadButton::South));
        assert!(!input.gamepad().was_pressed(GamepadButton::South));

        input.begin_tick();
        input.apply(InputEvent::GamepadButtonUp {
            button: GamepadButton::South,
        });
        assert!(!input.gamepad().is_held(GamepadButton::South));
        assert!(input.gamepad().was_released(GamepadButton::South));

        // GamepadButtonUp for a button that is not held: ignored.
        input.begin_tick();
        input.apply(InputEvent::GamepadButtonUp {
            button: GamepadButton::South,
        });
        assert!(!input.gamepad().was_released(GamepadButton::South));
    }

    #[test]
    fn gamepad_axis_sets_and_persists_across_ticks() {
        let mut input = InputState::default();
        input.begin_tick();
        input.apply(InputEvent::GamepadAxis {
            axis: GamepadAxis::LeftStickX,
            value: -0.75,
        });
        input.apply(InputEvent::GamepadAxis {
            axis: GamepadAxis::LeftStickY,
            value: 0.25,
        });
        input.apply(InputEvent::GamepadAxis {
            axis: GamepadAxis::RightTrigger,
            value: 1.0,
        });
        assert_eq!(input.gamepad().left_stick(), [-0.75, 0.25]);
        assert_eq!(input.gamepad().right_trigger(), 1.0);

        // Level state: still there next tick with no new event.
        input.begin_tick();
        assert_eq!(input.gamepad().left_stick(), [-0.75, 0.25]);
        assert_eq!(input.gamepad().right_trigger(), 1.0);
    }

    #[test]
    fn mouse_move_serde_form_is_position_bits() {
        let event = InputEvent::MouseMove {
            position: [1.5, -2.25],
        };
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "kind": "mouse_move",
                "position_bits": [1.5f32.to_bits(), (-2.25f32).to_bits()],
            })
        );
        let back: InputEvent = serde_json::from_value(json).unwrap();
        assert_eq!(back, event);
    }

    #[test]
    fn mouse_down_up_wheel_serde_round_trip() {
        for event in [
            InputEvent::MouseDown {
                button: MouseButton::Right,
            },
            InputEvent::MouseUp {
                button: MouseButton::Middle,
            },
            InputEvent::MouseWheel { delta: [0.0, -1.0] },
        ] {
            let json = serde_json::to_value(event).unwrap();
            let back: InputEvent = serde_json::from_value(json).unwrap();
            assert_eq!(back, event);
        }
        let json = serde_json::to_value(InputEvent::MouseDown {
            button: MouseButton::Right,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"kind": "mouse_down", "button": "right"})
        );
    }

    #[test]
    fn gamepad_axis_serde_form_is_value_bits() {
        let event = InputEvent::GamepadAxis {
            axis: GamepadAxis::LeftStickX,
            value: 0.5,
        };
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "kind": "gamepad_axis",
                "axis": "left_stick_x",
                "value_bits": 0.5f32.to_bits(),
            })
        );
        let back: InputEvent = serde_json::from_value(json).unwrap();
        assert_eq!(back, event);
    }

    #[test]
    fn gamepad_button_serde_round_trip() {
        for event in [
            InputEvent::GamepadButtonDown {
                button: GamepadButton::DpadUp,
            },
            InputEvent::GamepadButtonUp {
                button: GamepadButton::Start,
            },
        ] {
            let json = serde_json::to_value(event).unwrap();
            let back: InputEvent = serde_json::from_value(json).unwrap();
            assert_eq!(back, event);
        }
        let json = serde_json::to_value(InputEvent::GamepadButtonDown {
            button: GamepadButton::DpadUp,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"kind": "gamepad_button_down", "button": "dpad_up"})
        );
    }
}
