//! The touch mapper: a game-owned `kind: touch-layout` document and the translation of touches into
//! the gamepad events the sim already understands.
//!
//! A native host forwards raw touches ([`Embedded::touch`](crate::embed::Embedded::touch)) and draws
//! the controls at the rects [`TouchMapper::controls`] resolves; it never hit-tests. The mapper
//! emits [`InputEvent`]s only — pad buttons, pad axes — so a recording made from a touch session is
//! indistinguishable from a pad's, and no protocol type or replay moves.
//!
//! The layout lives beside the hosted scene as [`TOUCH_LAYOUT_FILE`]. Sizes and offsets are
//! fractions of the safe rect's *height*: `y` from the safe top, `x` from the anchored edge to the
//! control's near edge. A stick's deflection is the touch's offset from the control's centre over its
//! radius (the smaller half-extent), clamped to unit length, +Y up. The game applies its own
//! deadzone; the mapper's `deadzone` decides only when the synthesised trigger holds and a flick
//! re-arms.
//!
//! ```
//! use ludion::touch::{TouchLayout, TouchMapper, TouchPhase};
//! use ludion_core::letterbox::HostRect;
//!
//! let layout = TouchLayout::parse(
//!     "format: 1\nkind: touch-layout\nbuttons:\n  - button: south\n    label: A\n    anchor: right\n    \
//!      x: 0.1\n    y: 0.1\n    width: 0.2\n    height: 0.2\n",
//!     std::path::Path::new("touch.yaml"),
//! )
//! .expect("a valid layout");
//! let safe = HostRect { x: 0, y: 0, width: 2000, height: 1000 };
//! let mut mapper = TouchMapper::new(layout);
//! // The button's rect is 200 px square, 100 px from the top and 100 px in from the right edge.
//! let events = mapper.touch(1, TouchPhase::Began, 1800.0, 200.0, safe);
//! assert_eq!(events.len(), 1);
//! ```

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::letterbox::HostRect;
use ludion_core::math::sqrt;
use ludion_sim::{GamepadAxis, GamepadButton, InputEvent};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The layout's file name, beside the scene the core hosts.
pub const TOUCH_LAYOUT_FILE: &str = "touch.yaml";

/// The `kind:` of a layout document.
const KIND: &str = "touch-layout";

/// A game's on-screen controls: analog sticks and buttons.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TouchLayout {
    /// The analog sticks; at most one per side.
    #[serde(default)]
    pub sticks: Vec<LayoutStick>,
    /// The buttons; each pad button at most once.
    #[serde(default)]
    pub buttons: Vec<LayoutButton>,
}

/// One on-screen stick.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayoutStick {
    /// Which pad stick it drives.
    pub stick: StickSide,
    /// The edge `x` is measured from.
    pub anchor: Anchor,
    /// Offset from the anchored edge to the control's near edge, in safe-rect heights.
    pub x: f32,
    /// Offset from the safe rect's top, in safe-rect heights.
    pub y: f32,
    /// Width, in safe-rect heights.
    pub width: f32,
    /// Height, in safe-rect heights.
    pub height: f32,
    /// Deflection under which the stick counts as at rest, in `[0, 1)`.
    pub deadzone: f32,
    /// A vertical deflection at or past this taps the D-pad (up or down) once per push.
    #[serde(default)]
    pub flick: Option<f32>,
    /// Past the deadzone the stick's side trigger holds at full.
    #[serde(default)]
    pub trigger: bool,
}

/// One on-screen button.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayoutButton {
    /// The pad button it presses.
    pub button: PadButton,
    /// The text the host draws on it.
    pub label: String,
    /// The edge `x` is measured from.
    pub anchor: Anchor,
    /// Offset from the anchored edge to the control's near edge, in safe-rect heights.
    pub x: f32,
    /// Offset from the safe rect's top, in safe-rect heights.
    pub y: f32,
    /// Width, in safe-rect heights.
    pub width: f32,
    /// Height, in safe-rect heights.
    pub height: f32,
}

/// A pad stick.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StickSide {
    /// The left stick.
    Left,
    /// The right stick.
    Right,
}

/// The safe rect's edge a control is positioned from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    /// The left edge.
    Left,
    /// The right edge.
    Right,
}

/// A gamepad button, by the names `GamepadButton` gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)] // mirrors `GamepadButton`, whose variants are documented
pub enum PadButton {
    South,
    East,
    West,
    North,
    LeftShoulder,
    RightShoulder,
    LeftStick,
    RightStick,
    Start,
    Select,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
}

impl From<PadButton> for GamepadButton {
    fn from(button: PadButton) -> Self {
        match button {
            PadButton::South => GamepadButton::South,
            PadButton::East => GamepadButton::East,
            PadButton::West => GamepadButton::West,
            PadButton::North => GamepadButton::North,
            PadButton::LeftShoulder => GamepadButton::LeftShoulder,
            PadButton::RightShoulder => GamepadButton::RightShoulder,
            PadButton::LeftStick => GamepadButton::LeftStick,
            PadButton::RightStick => GamepadButton::RightStick,
            PadButton::Start => GamepadButton::Start,
            PadButton::Select => GamepadButton::Select,
            PadButton::DpadUp => GamepadButton::DpadUp,
            PadButton::DpadDown => GamepadButton::DpadDown,
            PadButton::DpadLeft => GamepadButton::DpadLeft,
            PadButton::DpadRight => GamepadButton::DpadRight,
        }
    }
}

fn positive(value: f32) -> bool {
    value.is_finite() && value > 0.0
}

fn non_negative(value: f32) -> bool {
    value.is_finite() && value >= 0.0
}

/// A control's rect in surface pixels `[x, y, width, height]`, resolved against the safe rect.
fn rect(anchor: Anchor, x: f32, y: f32, width: f32, height: f32, safe: HostRect) -> [f32; 4] {
    let unit = safe.height as f32;
    let left = safe.x as f32;
    let px = match anchor {
        Anchor::Left => left + x * unit,
        Anchor::Right => left + safe.width as f32 - (x + width) * unit,
    };
    [px, safe.y as f32 + y * unit, width * unit, height * unit]
}

fn contains(rect: [f32; 4], x: f32, y: f32) -> bool {
    x >= rect[0] && x < rect[0] + rect[2] && y >= rect[1] && y < rect[1] + rect[3]
}

impl LayoutStick {
    fn rect(&self, safe: HostRect) -> [f32; 4] {
        rect(self.anchor, self.x, self.y, self.width, self.height, safe)
    }
}

impl LayoutButton {
    fn rect(&self, safe: HostRect) -> [f32; 4] {
        rect(self.anchor, self.x, self.y, self.width, self.height, safe)
    }
}

impl TouchLayout {
    /// Parses and validates a layout document; `file` only names it in diagnostics.
    ///
    /// # Errors
    /// An `L11xx` from the document checker for a malformed or mistyped document, else `L0021` for a
    /// control the mapper cannot use, with a JSON Pointer path (`/sticks/0/deadzone`). With several
    /// findings the first is returned and the rest ride along as `related`.
    pub fn parse(source: &str, file: &Path) -> Result<TouchLayout, Diagnostic> {
        let schema = serde_json::to_value(schemars::schema_for!(TouchLayout))
            .expect("a schemars JSON Schema always serializes to a JSON value");
        let fields = ludion_scene::check_doc(source, KIND, &schema).map_err(first_with_related)?;
        let layout: TouchLayout = serde_json::from_value(fields).map_err(|error| {
            invalid(
                file,
                "",
                format!("the document does not fit the layout: {error}"),
            )
        })?;
        let findings = layout.findings(file);
        if findings.is_empty() {
            Ok(layout)
        } else {
            Err(first_with_related(findings))
        }
    }

    /// Reads the layout beside `scene`: `Ok(None)` when there is no such file (a game without touch
    /// controls).
    ///
    /// # Errors
    /// As [`parse`](Self::parse), and `L0021` for a file that exists but cannot be read.
    pub fn load_beside(scene: &Path) -> Result<Option<TouchLayout>, Diagnostic> {
        let file = scene.with_file_name(TOUCH_LAYOUT_FILE);
        match std::fs::read_to_string(&file) {
            Ok(source) => Self::parse(&source, &file).map(Some),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(invalid(&file, "", format!("could not be read: {error}"))),
        }
    }

    fn findings(&self, file: &Path) -> Vec<Diagnostic> {
        let mut found = Vec::new();
        let mut bad = |path: String, what: String| found.push(invalid(file, &path, what));
        for (i, stick) in self.sticks.iter().enumerate() {
            let at = |field: &str| format!("/sticks/{i}/{field}");
            if !(0.0..1.0).contains(&stick.deadzone) {
                bad(
                    at("deadzone"),
                    format!("deadzone {} is outside [0, 1)", stick.deadzone),
                );
            }
            if let Some(flick) = stick.flick
                && !(flick.is_finite() && flick > stick.deadzone && flick <= 1.0)
            {
                bad(
                    at("flick"),
                    format!(
                        "flick {flick} must be above the deadzone {} and at most 1",
                        stick.deadzone
                    ),
                );
            }
            if !positive(stick.width) || !positive(stick.height) {
                bad(at("width"), "width and height must be positive".into());
            }
            if !non_negative(stick.x) || !non_negative(stick.y) {
                bad(at("x"), "x and y must not be negative".into());
            }
            if self.sticks[..i].iter().any(|s| s.stick == stick.stick) {
                bad(
                    at("stick"),
                    format!("{:?} stick is named twice", stick.stick),
                );
            }
        }
        for (i, button) in self.buttons.iter().enumerate() {
            let at = |field: &str| format!("/buttons/{i}/{field}");
            if button.label.trim().is_empty() {
                bad(at("label"), "label is empty".into());
            }
            if !positive(button.width) || !positive(button.height) {
                bad(at("width"), "width and height must be positive".into());
            }
            if !non_negative(button.x) || !non_negative(button.y) {
                bad(at("x"), "x and y must not be negative".into());
            }
            if self.buttons[..i].iter().any(|b| b.button == button.button) {
                bad(
                    at("button"),
                    format!("button {:?} is named twice", button.button),
                );
            }
        }
        found
    }
}

fn invalid(file: &Path, path: &str, what: impl std::fmt::Display) -> Diagnostic {
    let diagnostic = Diagnostic::new(
        code::TOUCH_LAYOUT_INVALID,
        format!("touch layout `{}`: {what}", file.display()),
    )
    .with_help(
        "sizes and offsets are fractions of the safe area's height; a deadzone is in [0, 1), a flick \
         lies above it, and each stick side and pad button appears once",
    );
    if path.is_empty() {
        diagnostic
    } else {
        diagnostic.with_path(path)
    }
}

fn first_with_related(diagnostics: Vec<Diagnostic>) -> Diagnostic {
    let mut all = diagnostics.into_iter();
    let first = all.next().expect("a failed check carries a diagnostic");
    all.fold(first, Diagnostic::with_related)
}

/// A touch's lifecycle stage, as UIKit reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchPhase {
    /// The finger went down.
    Began,
    /// The finger moved.
    Moved,
    /// The finger lifted.
    Ended,
    /// The system took the touch away.
    Cancelled,
}

/// What a control is, for a host that draws it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlKind {
    /// The left analog stick.
    LeftStick,
    /// The right analog stick.
    RightStick,
    /// A button.
    Button,
}

/// A control's resolved rect and label, for a host to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct TouchControl {
    /// What to draw.
    pub kind: ControlKind,
    /// `[x, y, width, height]` in surface pixels.
    pub rect: [f32; 4],
    /// A button's label; sticks have none.
    pub label: Option<String>,
}

/// Which control a touch id holds.
#[derive(Clone, Copy, Debug)]
enum Owner {
    /// A button by layout index; `on` is false once the finger slid off (dead until lifted).
    Button { index: usize, on: bool },
    /// A stick by layout index.
    Stick(usize),
    /// A touch the mapper ignores until it ends.
    Swallowed,
}

#[derive(Clone, Copy, Debug)]
struct StickState {
    /// The axes last emitted.
    axes: [f32; 2],
    trigger_on: bool,
    flick_armed: bool,
}

/// Turns touches into pad events over one [`TouchLayout`].
#[derive(Debug)]
pub struct TouchMapper {
    layout: TouchLayout,
    owners: BTreeMap<u64, Owner>,
    /// Fingers currently pressing each button, by layout index.
    holds: Vec<u32>,
    sticks: Vec<StickState>,
}

impl TouchMapper {
    /// A mapper at rest over `layout`.
    pub fn new(layout: TouchLayout) -> Self {
        let holds = vec![0; layout.buttons.len()];
        let sticks = vec![
            StickState {
                axes: [0.0, 0.0],
                trigger_on: false,
                flick_armed: true,
            };
            layout.sticks.len()
        ];
        TouchMapper {
            layout,
            owners: BTreeMap::new(),
            holds,
            sticks,
        }
    }

    /// The controls to draw, resolved against `safe`: sticks, then buttons, in layout order.
    pub fn controls(&self, safe: HostRect) -> Vec<TouchControl> {
        let sticks = self.layout.sticks.iter().map(|stick| TouchControl {
            kind: match stick.stick {
                StickSide::Left => ControlKind::LeftStick,
                StickSide::Right => ControlKind::RightStick,
            },
            rect: stick.rect(safe),
            label: None,
        });
        let buttons = self.layout.buttons.iter().map(|button| TouchControl {
            kind: ControlKind::Button,
            rect: button.rect(safe),
            label: Some(button.label.clone()),
        });
        sticks.chain(buttons).collect()
    }

    /// Feeds one touch (`x`/`y` in surface pixels) and returns the pad events it causes.
    pub fn touch(
        &mut self,
        id: u64,
        phase: TouchPhase,
        x: f32,
        y: f32,
        safe: HostRect,
    ) -> Vec<InputEvent> {
        let mut events = Vec::new();
        match phase {
            TouchPhase::Began => {
                self.end(id, &mut events);
                self.begin(id, x, y, safe, &mut events);
            }
            TouchPhase::Moved => self.moved(id, x, y, safe, &mut events),
            TouchPhase::Ended | TouchPhase::Cancelled => self.end(id, &mut events),
        }
        events
    }

    /// Returns every held control to rest and forgets every touch.
    pub fn release_all(&mut self) -> Vec<InputEvent> {
        let mut events = Vec::new();
        let ids: Vec<u64> = self.owners.keys().copied().collect();
        for id in ids {
            self.end(id, &mut events);
        }
        self.owners.clear();
        events
    }

    /// Ignores `id` until it ends; anything it held is released first.
    pub fn swallow(&mut self, id: u64) -> Vec<InputEvent> {
        let mut events = Vec::new();
        self.end(id, &mut events);
        self.owners.insert(id, Owner::Swallowed);
        events
    }

    fn begin(&mut self, id: u64, x: f32, y: f32, safe: HostRect, events: &mut Vec<InputEvent>) {
        let button = self
            .layout
            .buttons
            .iter()
            .position(|b| contains(b.rect(safe), x, y));
        if let Some(index) = button {
            self.owners.insert(id, Owner::Button { index, on: true });
            self.press(index, events);
            return;
        }
        let stick = self
            .layout
            .sticks
            .iter()
            .position(|s| contains(s.rect(safe), x, y));
        if let Some(index) = stick
            && !self
                .owners
                .values()
                .any(|o| matches!(o, Owner::Stick(i) if *i == index))
        {
            self.owners.insert(id, Owner::Stick(index));
            self.deflect(index, x, y, safe, events);
        }
    }

    fn moved(&mut self, id: u64, x: f32, y: f32, safe: HostRect, events: &mut Vec<InputEvent>) {
        match self.owners.get(&id).copied() {
            Some(Owner::Button { index, on: true }) => {
                if !contains(self.layout.buttons[index].rect(safe), x, y) {
                    self.owners.insert(id, Owner::Button { index, on: false });
                    self.lift(index, events);
                }
            }
            Some(Owner::Stick(index)) => self.deflect(index, x, y, safe, events),
            _ => {}
        }
    }

    fn end(&mut self, id: u64, events: &mut Vec<InputEvent>) {
        match self.owners.remove(&id) {
            Some(Owner::Button { index, on: true }) => self.lift(index, events),
            Some(Owner::Stick(index)) => self.set_deflection(index, [0.0, 0.0], events),
            _ => {}
        }
    }

    fn press(&mut self, index: usize, events: &mut Vec<InputEvent>) {
        self.holds[index] += 1;
        if self.holds[index] == 1 {
            events.push(InputEvent::GamepadButtonDown {
                button: self.layout.buttons[index].button.into(),
            });
        }
    }

    fn lift(&mut self, index: usize, events: &mut Vec<InputEvent>) {
        self.holds[index] -= 1;
        if self.holds[index] == 0 {
            events.push(InputEvent::GamepadButtonUp {
                button: self.layout.buttons[index].button.into(),
            });
        }
    }

    fn deflect(
        &mut self,
        index: usize,
        x: f32,
        y: f32,
        safe: HostRect,
        events: &mut Vec<InputEvent>,
    ) {
        let [rx, ry, width, height] = self.layout.sticks[index].rect(safe);
        let radius = width.min(height) / 2.0;
        let dx = (x - (rx + width / 2.0)) / radius;
        let dy = -(y - (ry + height / 2.0)) / radius;
        self.set_deflection(index, [dx, dy], events);
    }

    /// Applies a deflection (+Y up, any length): the clamped axes, the trigger and the flick, each
    /// emitted only on change.
    fn set_deflection(&mut self, index: usize, d: [f32; 2], events: &mut Vec<InputEvent>) {
        let stick = &self.layout.sticks[index];
        let (axis_x, axis_y, trigger_axis) = match stick.stick {
            StickSide::Left => (
                GamepadAxis::LeftStickX,
                GamepadAxis::LeftStickY,
                GamepadAxis::LeftTrigger,
            ),
            StickSide::Right => (
                GamepadAxis::RightStickX,
                GamepadAxis::RightStickY,
                GamepadAxis::RightTrigger,
            ),
        };
        let (deadzone, flick, trigger) = (stick.deadzone, stick.flick, stick.trigger);
        let length = sqrt(d[0] * d[0] + d[1] * d[1]);
        let d = if length > 1.0 {
            [d[0] / length, d[1] / length]
        } else {
            d
        };
        let length = length.min(1.0);
        let state = &mut self.sticks[index];
        for (i, axis) in [axis_x, axis_y].into_iter().enumerate() {
            if d[i] != state.axes[i] {
                state.axes[i] = d[i];
                events.push(InputEvent::GamepadAxis { axis, value: d[i] });
            }
        }
        let outside = length > deadzone;
        if trigger && outside != state.trigger_on {
            state.trigger_on = outside;
            events.push(InputEvent::GamepadAxis {
                axis: trigger_axis,
                value: if outside { 1.0 } else { 0.0 },
            });
        }
        if !outside {
            state.flick_armed = true;
        } else if let Some(flick) = flick
            && state.flick_armed
            && d[1].abs() >= flick
            && d[1].abs() > d[0].abs()
        {
            state.flick_armed = false;
            let button = if d[1] > 0.0 {
                GamepadButton::DpadUp
            } else {
                GamepadButton::DpadDown
            };
            events.push(InputEvent::GamepadButtonDown { button });
            events.push(InputEvent::GamepadButtonUp { button });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kind_is_an_engine_kind_the_walk_skips() {
        assert!(ludion_scene::kind_entry(KIND).is_some_and(|entry| !entry.doc_store));
    }

    const LAYOUT: &str = "\
format: 1
kind: touch-layout
sticks:
  - stick: left
    anchor: left
    x: 0.1
    y: 0.4
    width: 0.4
    height: 0.4
    deadzone: 0.2
    flick: 0.7
  - stick: right
    anchor: right
    x: 0.1
    y: 0.4
    width: 0.4
    height: 0.4
    deadzone: 0.2
    trigger: true
buttons:
  - button: south
    label: A
    anchor: right
    x: 0.6
    y: 0.2
    width: 0.2
    height: 0.2
  - button: east
    label: B
    anchor: right
    x: 0.6
    y: 0.5
    width: 0.2
    height: 0.2
";

    const SAFE: HostRect = HostRect {
        x: 0,
        y: 0,
        width: 2000,
        height: 1000,
    };

    fn mapper() -> TouchMapper {
        TouchMapper::new(TouchLayout::parse(LAYOUT, Path::new("touch.yaml")).expect("valid layout"))
    }

    /// Left stick: x 100..500, y 400..800 → centre (300, 600), radius 200.
    const LEFT: (f32, f32) = (300.0, 600.0);
    /// Right stick: x 1500..1900 → centre (1700, 600), radius 200.
    const RIGHT: (f32, f32) = (1700.0, 600.0);
    /// South: x 1200..1400, y 200..400. East: x 1200..1400, y 500..700.
    const SOUTH: (f32, f32) = (1300.0, 300.0);
    const EAST: (f32, f32) = (1300.0, 600.0);

    fn axis(axis: GamepadAxis, value: f32) -> InputEvent {
        InputEvent::GamepadAxis { axis, value }
    }

    fn down(button: GamepadButton) -> InputEvent {
        InputEvent::GamepadButtonDown { button }
    }

    fn up(button: GamepadButton) -> InputEvent {
        InputEvent::GamepadButtonUp { button }
    }

    fn touch(m: &mut TouchMapper, id: u64, phase: TouchPhase, at: (f32, f32)) -> Vec<InputEvent> {
        m.touch(id, phase, at.0, at.1, SAFE)
    }

    #[test]
    fn a_stick_maps_the_offset_over_the_radius_with_y_up_and_clamps_to_unit_length() {
        let mut m = mapper();
        assert!(
            touch(&mut m, 1, TouchPhase::Began, LEFT).is_empty(),
            "centre is rest"
        );
        assert_eq!(
            touch(
                &mut m,
                1,
                TouchPhase::Moved,
                (LEFT.0 + 100.0, LEFT.1 - 100.0)
            ),
            [
                axis(GamepadAxis::LeftStickX, 0.5),
                axis(GamepadAxis::LeftStickY, 0.5)
            ],
            "screen y is down, stick +Y is up"
        );
        assert_eq!(
            touch(&mut m, 1, TouchPhase::Moved, (LEFT.0 + 400.0, LEFT.1)),
            [
                axis(GamepadAxis::LeftStickX, 1.0),
                axis(GamepadAxis::LeftStickY, 0.0)
            ],
            "twice the radius clamps to exactly 1"
        );
        let diagonal = touch(
            &mut m,
            1,
            TouchPhase::Moved,
            (LEFT.0 + 400.0, LEFT.1 + 400.0),
        );
        let [
            InputEvent::GamepadAxis { value: x, .. },
            InputEvent::GamepadAxis { value: y, .. },
        ] = diagonal[..]
        else {
            panic!("both axes move: {diagonal:?}");
        };
        assert!(x > 0.0 && y < 0.0 && (x * x + y * y - 1.0).abs() < 1e-6);
        assert_eq!(
            touch(&mut m, 1, TouchPhase::Ended, LEFT),
            [
                axis(GamepadAxis::LeftStickX, 0.0),
                axis(GamepadAxis::LeftStickY, 0.0)
            ]
        );

        touch(&mut m, 2, TouchPhase::Began, RIGHT);
        assert_eq!(
            touch(&mut m, 2, TouchPhase::Moved, (RIGHT.0 - 200.0, RIGHT.1)),
            [
                axis(GamepadAxis::RightStickX, -1.0),
                axis(GamepadAxis::RightTrigger, 1.0)
            ],
            "the right stick drives the right axes; Y did not change"
        );
    }

    #[test]
    fn the_trigger_holds_past_the_deadzone_on_a_trigger_stick_only() {
        let mut m = mapper();
        touch(&mut m, 1, TouchPhase::Began, RIGHT);
        // Exactly at the deadzone (0.2 · 200 px) is still rest.
        let at_edge = touch(&mut m, 1, TouchPhase::Moved, (RIGHT.0 + 40.0, RIGHT.1));
        assert!(
            !at_edge.contains(&axis(GamepadAxis::RightTrigger, 1.0)),
            "{at_edge:?}"
        );
        let past = touch(&mut m, 1, TouchPhase::Moved, (RIGHT.0 + 60.0, RIGHT.1));
        assert!(
            past.contains(&axis(GamepadAxis::RightTrigger, 1.0)),
            "{past:?}"
        );
        let back = touch(&mut m, 1, TouchPhase::Moved, (RIGHT.0 + 20.0, RIGHT.1));
        assert!(
            back.contains(&axis(GamepadAxis::RightTrigger, 0.0)),
            "{back:?}"
        );
        touch(&mut m, 1, TouchPhase::Moved, (RIGHT.0 + 200.0, RIGHT.1));
        let released = touch(&mut m, 1, TouchPhase::Ended, RIGHT);
        assert!(
            released.contains(&axis(GamepadAxis::RightTrigger, 0.0)),
            "{released:?}"
        );

        touch(&mut m, 2, TouchPhase::Began, LEFT);
        let left = touch(&mut m, 2, TouchPhase::Moved, (LEFT.0 + 200.0, LEFT.1));
        assert!(
            left.iter()
                .all(|e| !matches!(e, InputEvent::GamepadAxis { axis, .. }
                if *axis == GamepadAxis::LeftTrigger)),
            "the left stick has no trigger: {left:?}"
        );
    }

    #[test]
    fn a_flick_taps_the_dpad_once_per_push_and_never_holds_it() {
        let mut m = mapper();
        touch(&mut m, 1, TouchPhase::Began, LEFT);
        let flick = touch(&mut m, 1, TouchPhase::Moved, (LEFT.0, LEFT.1 - 150.0));
        assert_eq!(
            flick,
            [
                axis(GamepadAxis::LeftStickY, 0.75),
                down(GamepadButton::DpadUp),
                up(GamepadButton::DpadUp)
            ],
            "one tap: Down then Up in the same batch"
        );
        let more = touch(&mut m, 1, TouchPhase::Moved, (LEFT.0, LEFT.1 - 200.0));
        assert_eq!(
            more,
            [axis(GamepadAxis::LeftStickY, 1.0)],
            "still pushed: no second tap"
        );
        // Back to a partial deflection above the deadzone does not re-arm.
        touch(&mut m, 1, TouchPhase::Moved, (LEFT.0, LEFT.1 - 60.0));
        let again = touch(&mut m, 1, TouchPhase::Moved, (LEFT.0, LEFT.1 - 180.0));
        assert!(!again.contains(&down(GamepadButton::DpadUp)), "{again:?}");
        // Under the deadzone re-arms; a downward push is DpadDown.
        touch(&mut m, 1, TouchPhase::Moved, (LEFT.0, LEFT.1 + 20.0));
        let down_tap = touch(&mut m, 1, TouchPhase::Moved, (LEFT.0, LEFT.1 + 180.0));
        assert!(
            down_tap.contains(&down(GamepadButton::DpadDown)),
            "{down_tap:?}"
        );
        assert!(
            down_tap.contains(&up(GamepadButton::DpadDown)),
            "{down_tap:?}"
        );
    }

    #[test]
    fn a_horizontal_dominant_push_does_not_flick() {
        let mut m = mapper();
        touch(&mut m, 1, TouchPhase::Began, LEFT);
        let events = touch(
            &mut m,
            1,
            TouchPhase::Moved,
            (LEFT.0 + 190.0, LEFT.1 - 160.0),
        );
        assert!(
            events
                .iter()
                .all(|e| !matches!(e, InputEvent::GamepadButtonDown { .. })),
            "{events:?}"
        );
    }

    #[test]
    fn touches_are_tracked_per_id_across_sticks_and_buttons() {
        let mut m = mapper();
        assert_eq!(
            touch(&mut m, 1, TouchPhase::Began, SOUTH),
            [down(GamepadButton::South)]
        );
        assert!(touch(&mut m, 2, TouchPhase::Began, LEFT).is_empty());
        touch(&mut m, 2, TouchPhase::Moved, (LEFT.0 + 200.0, LEFT.1));
        assert_eq!(
            touch(&mut m, 3, TouchPhase::Began, EAST),
            [down(GamepadButton::East)]
        );
        // A second finger on the owned left stick is ignored, and so is its lift.
        assert!(touch(&mut m, 4, TouchPhase::Began, (LEFT.0 - 50.0, LEFT.1)).is_empty());
        assert!(touch(&mut m, 4, TouchPhase::Moved, (LEFT.0 - 100.0, LEFT.1)).is_empty());
        assert!(touch(&mut m, 4, TouchPhase::Ended, LEFT).is_empty());
        // Ending one id releases only its control.
        assert_eq!(
            touch(&mut m, 1, TouchPhase::Ended, SOUTH),
            [up(GamepadButton::South)]
        );
        let stick_off = touch(&mut m, 2, TouchPhase::Ended, LEFT);
        assert_eq!(stick_off, [axis(GamepadAxis::LeftStickX, 0.0)]);
        assert_eq!(
            touch(&mut m, 3, TouchPhase::Cancelled, EAST),
            [up(GamepadButton::East)]
        );
    }

    #[test]
    fn two_fingers_on_one_button_release_on_the_last() {
        let mut m = mapper();
        assert_eq!(
            touch(&mut m, 1, TouchPhase::Began, SOUTH),
            [down(GamepadButton::South)]
        );
        assert!(touch(&mut m, 2, TouchPhase::Began, (1250.0, 250.0)).is_empty());
        assert!(touch(&mut m, 1, TouchPhase::Ended, SOUTH).is_empty());
        assert_eq!(
            touch(&mut m, 2, TouchPhase::Ended, SOUTH),
            [up(GamepadButton::South)]
        );
    }

    #[test]
    fn sliding_off_a_button_releases_it_and_sliding_back_does_not_press_again() {
        let mut m = mapper();
        touch(&mut m, 1, TouchPhase::Began, SOUTH);
        assert_eq!(
            touch(&mut m, 1, TouchPhase::Moved, (1500.0, 300.0)),
            [up(GamepadButton::South)]
        );
        assert!(
            touch(&mut m, 1, TouchPhase::Moved, SOUTH).is_empty(),
            "dead until lifted"
        );
        assert!(
            touch(&mut m, 1, TouchPhase::Moved, EAST).is_empty(),
            "nor onto another button"
        );
        assert!(touch(&mut m, 1, TouchPhase::Ended, EAST).is_empty());
    }

    #[test]
    fn an_unchanged_axis_value_is_not_emitted_again() {
        let mut m = mapper();
        touch(&mut m, 1, TouchPhase::Began, LEFT);
        assert_eq!(
            touch(&mut m, 1, TouchPhase::Moved, (LEFT.0 + 100.0, LEFT.1)).len(),
            1
        );
        assert!(touch(&mut m, 1, TouchPhase::Moved, (LEFT.0 + 100.0, LEFT.1)).is_empty());
        assert!(touch(&mut m, 1, TouchPhase::Moved, (LEFT.0 + 100.0, LEFT.1 + 0.0)).is_empty());
    }

    #[test]
    fn release_all_rests_every_control_and_swallow_ignores_a_touch_until_it_ends() {
        let mut m = mapper();
        touch(&mut m, 1, TouchPhase::Began, SOUTH);
        touch(&mut m, 2, TouchPhase::Began, RIGHT);
        touch(&mut m, 2, TouchPhase::Moved, (RIGHT.0 + 200.0, RIGHT.1));
        let released = m.release_all();
        assert!(released.contains(&up(GamepadButton::South)), "{released:?}");
        assert!(
            released.contains(&axis(GamepadAxis::RightTrigger, 0.0)),
            "{released:?}"
        );
        assert!(
            released.contains(&axis(GamepadAxis::RightStickX, 0.0)),
            "{released:?}"
        );
        assert!(
            touch(&mut m, 1, TouchPhase::Ended, SOUTH).is_empty(),
            "the owner is gone"
        );

        assert!(m.swallow(7).is_empty());
        assert!(touch(&mut m, 7, TouchPhase::Moved, SOUTH).is_empty());
        assert!(touch(&mut m, 7, TouchPhase::Ended, SOUTH).is_empty());
        assert_eq!(
            touch(&mut m, 7, TouchPhase::Began, SOUTH),
            [down(GamepadButton::South)],
            "a new touch with the same id is live again"
        );
    }

    #[test]
    fn the_rects_resolve_against_the_anchored_edge_of_an_offset_safe_rect() {
        let m = mapper();
        let safe = HostRect {
            x: 100,
            y: 50,
            width: 1800,
            height: 1000,
        };
        let controls = m.controls(safe);
        let kinds: Vec<_> = controls.iter().map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            [
                ControlKind::LeftStick,
                ControlKind::RightStick,
                ControlKind::Button,
                ControlKind::Button
            ]
        );
        assert_eq!(
            controls[0].rect,
            [200.0, 450.0, 400.0, 400.0],
            "left: safe.x + x·H"
        );
        assert_eq!(
            controls[1].rect,
            [1900.0 - 500.0, 450.0, 400.0, 400.0],
            "right: safe right − (x + w)·H"
        );
        assert_eq!(controls[2].label.as_deref(), Some("A"));
        assert_eq!(controls[0].label, None);
    }

    fn parse(source: &str) -> Result<TouchLayout, Diagnostic> {
        TouchLayout::parse(source, Path::new("scenes/touch.yaml"))
    }

    fn with(replace: &str, with: &str) -> String {
        assert!(LAYOUT.contains(replace), "the fixture has `{replace}`");
        LAYOUT.replacen(replace, with, 1)
    }

    #[test]
    fn a_layout_the_mapper_cannot_use_is_refused_with_a_pointer_and_the_file() {
        let bad_deadzone = parse(&with("deadzone: 0.2", "deadzone: 1.5")).unwrap_err();
        assert_eq!(bad_deadzone.code, code::TOUCH_LAYOUT_INVALID);
        assert_eq!(bad_deadzone.path.as_deref(), Some("/sticks/0/deadzone"));
        assert!(bad_deadzone.message.contains("scenes/touch.yaml"));

        let flick = parse(&with("flick: 0.7", "flick: 0.1")).unwrap_err();
        assert_eq!(flick.path.as_deref(), Some("/sticks/0/flick"));

        let duplicate = parse(&with("button: east", "button: south")).unwrap_err();
        assert_eq!(duplicate.code, code::TOUCH_LAYOUT_INVALID);
        assert_eq!(duplicate.path.as_deref(), Some("/buttons/1/button"));

        let side = parse(&with("stick: right", "stick: left")).unwrap_err();
        assert_eq!(side.path.as_deref(), Some("/sticks/1/stick"));

        let label = parse(&with("label: B", "label: ''")).unwrap_err();
        assert_eq!(label.path.as_deref(), Some("/buttons/1/label"));

        let size = parse(&with(
            "width: 0.2\n    height: 0.2",
            "width: 0.0\n    height: 0.2",
        ))
        .unwrap_err();
        assert_eq!(size.path.as_deref(), Some("/buttons/0/width"));

        let offset = parse(&with("x: 0.6", "x: -0.1")).unwrap_err();
        assert_eq!(offset.path.as_deref(), Some("/buttons/0/x"));
    }

    #[test]
    fn several_findings_ride_as_related() {
        let both = with("deadzone: 0.2", "deadzone: 2.0").replacen("label: B", "label: ''", 1);
        let error = parse(&both).unwrap_err();
        assert_eq!(error.path.as_deref(), Some("/sticks/0/deadzone"));
        let related: Vec<_> = error.related.iter().map(|r| r.path.as_deref()).collect();
        assert_eq!(related, [Some("/sticks/0/flick"), Some("/buttons/1/label")]);
    }

    #[test]
    fn an_unknown_key_or_a_bad_enum_is_a_document_check_error() {
        for source in [
            with("label: A", "label: A\n    colour: red"),
            with("button: south", "button: cross"),
        ] {
            let error = parse(&source).unwrap_err();
            assert!(
                error.code.as_str().starts_with("L11"),
                "the document checker owns it: {error}"
            );
        }
    }

    #[test]
    fn a_layout_beside_the_scene_loads_and_a_missing_one_is_no_controls() {
        let dir = std::env::temp_dir().join(format!("ludion-touch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let scene = dir.join("main.yaml");
        assert_eq!(TouchLayout::load_beside(&scene).unwrap(), None);
        std::fs::write(dir.join(TOUCH_LAYOUT_FILE), LAYOUT).unwrap();
        let layout = TouchLayout::load_beside(&scene)
            .unwrap()
            .expect("the layout");
        assert_eq!(layout.sticks.len(), 2);
        std::fs::write(
            dir.join(TOUCH_LAYOUT_FILE),
            "format: 1\nkind: touch-layout\nsticks: 3\n",
        )
        .unwrap();
        assert!(TouchLayout::load_beside(&scene).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
