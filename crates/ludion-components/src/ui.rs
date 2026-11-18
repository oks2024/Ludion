//! [`UiNav`], the menu intent a game feeds to [`UiList::navigate`], and the HUD motion components
//! [`UiTween`] and [`UiWorldAnchor`], which `extract` resolves onto a screen-space element.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::number::engine_number;
use ludion_ecs::SemanticValidate;
use ludion_sim::{GamepadButton, InputState, Key};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{UiList, UiListItem};

/// One frame's menu intent: three edge flags a game feeds to [`UiList::navigate`].
///
/// Built from an [`InputState`] by [`UiNav::from_input`] under the engine's fixed bindings, or by
/// hand — the fields are public precisely so a game with its own bindings never needs an engine
/// change. There is no action-mapping layer and no engine-side focus manager: exactly one list is
/// navigated per frame, the one the game chooses to call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiNav {
    /// Move the cursor to the previous selectable item.
    pub prev: bool,
    /// Move the cursor to the next selectable item.
    pub next: bool,
    /// Activate the item under the cursor.
    pub activate: bool,
}

impl UiNav {
    /// Reads this frame's menu intent under the engine's default bindings: `Up`/`DpadUp` is `prev`,
    /// `Down`/`DpadDown` is `next`, and `Enter`/`Space`/`GamepadButton::South` is `activate`.
    ///
    /// **Discrete edges only** (`was_pressed`, never `is_held`): one press is one step, a held
    /// direction does not repeat, and no hidden repeat-latch state exists to desynchronize. An analog
    /// stick is deliberately not read here for the same reason.
    ///
    /// # Example
    ///
    /// ```
    /// use ludion_components::UiNav;
    /// use ludion_sim::{InputEvent, InputState, Key};
    ///
    /// let mut input = InputState::default();
    /// input.begin_tick();
    /// input.apply(InputEvent::KeyDown { key: Key::Down });
    /// assert!(UiNav::from_input(&input).next);
    ///
    /// input.begin_tick(); // the key is still held, but the edge is gone
    /// assert!(!UiNav::from_input(&input).next);
    /// ```
    pub fn from_input(input: &InputState) -> Self {
        let pad = input.gamepad();
        UiNav {
            prev: input.was_pressed(Key::Up) || pad.was_pressed(GamepadButton::DpadUp),
            next: input.was_pressed(Key::Down) || pad.was_pressed(GamepadButton::DpadDown),
            activate: input.was_pressed(Key::Enter)
                || input.was_pressed(Key::Space)
                || pad.was_pressed(GamepadButton::South),
        }
    }
}

impl UiList {
    /// Applies one frame's [`UiNav`] to the cursor and reports an activation.
    ///
    /// Moves `selected` (also clamping a stale value into range) and returns `Some(index)` when
    /// `activate` fired over an **enabled** item — the activation is this call's return value, never a
    /// field, so there is no stale flag and no clearing protocol to forget.
    ///
    /// The cursor **wraps** at both ends and **never lands on a disabled item**; a list whose items
    /// are all disabled keeps its cursor where it is and refuses activation. An empty list moves
    /// nothing and returns `None`.
    ///
    /// # Example
    ///
    /// ```
    /// use ludion_components::{UiList, UiListItem};
    /// use ludion_components::UiNav;
    ///
    /// let item = |text: &str, enabled: bool| UiListItem { text: text.to_string(), enabled };
    /// let mut list = UiList {
    ///     items: vec![item("BUY", true), item("SOLD OUT", false), item("LEAVE", true)],
    ///     ..UiList::default()
    /// };
    ///
    /// let next = UiNav { next: true, ..UiNav::default() };
    /// assert_eq!(list.navigate(next), None); // the sold-out item is skipped
    /// assert_eq!(list.selected, 2);
    ///
    /// let activate = UiNav { activate: true, ..UiNav::default() };
    /// assert_eq!(list.navigate(activate), Some(2));
    /// ```
    pub fn navigate(&mut self, nav: UiNav) -> Option<u32> {
        let count = self.items.len();
        if count == 0 {
            return None;
        }
        let mut cursor = (self.selected as usize).min(count - 1);
        if nav.prev {
            cursor = step_cursor(&self.items, cursor, false);
        }
        if nav.next {
            cursor = step_cursor(&self.items, cursor, true);
        }
        self.selected = cursor as u32;
        (nav.activate && self.items[cursor].enabled).then_some(cursor as u32)
    }
}

/// The next enabled index from `from`, wrapping — or `from` itself when nothing else is selectable.
/// Bounded by one full lap, so an all-disabled list terminates with the cursor where it was.
fn step_cursor(items: &[UiListItem], from: usize, forward: bool) -> usize {
    let count = items.len();
    let mut index = from;
    for _ in 0..count {
        index = if forward {
            (index + 1) % count
        } else {
            (index + count - 1) % count
        };
        if items[index].enabled {
            return index;
        }
    }
    from
}

/// The easing curve of a [`UiTweenTrack`], applied to the linear progress `u` in `[0, 1]`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum UiEase {
    /// `u`.
    #[default]
    Linear,
    /// Smoothstep, `u * u * (3 - 2u)`: zero slope at both ends.
    Smooth,
    /// Holds the start value until `u` reaches 1.
    Step,
    /// `u^2`.
    InQuad,
    /// `u * (2 - u)`.
    OutQuad,
    /// `2u^2` then `1 - 2(1 - u)^2`.
    InOutQuad,
    /// `u^3`.
    InCubic,
    /// `1 - (1 - u)^3`.
    OutCubic,
    /// `4u^3` then `1 - 4(1 - u)^3`.
    InOutCubic,
    /// Overshoots, then settles: `1 + c3 (u-1)^3 + c1 (u-1)^2` with `c1 = 1.70158`, `c3 = c1 + 1`.
    OutBack,
}

impl UiEase {
    /// The eased value for progress `u`. `u <= 0` is exactly `0` and `u >= 1` exactly `1` for every
    /// curve (`step` is `0` below 1), so a tween's `from` and `to` are the values that draw at its ends.
    /// Arithmetic only, no transcendentals.
    ///
    /// ```
    /// use ludion_components::ui::UiEase;
    ///
    /// assert_eq!(UiEase::InQuad.apply(0.5), 0.25);
    /// assert_eq!(UiEase::OutBack.apply(1.0), 1.0);
    /// assert!(UiEase::OutBack.apply(0.8) > 1.0);
    /// ```
    pub fn apply(self, u: f32) -> f32 {
        if u <= 0.0 {
            return 0.0;
        }
        if u >= 1.0 {
            return 1.0;
        }
        let inv = 1.0 - u;
        match self {
            UiEase::Linear => u,
            UiEase::Smooth => u * u * (3.0 - 2.0 * u),
            UiEase::Step => 0.0,
            UiEase::InQuad => u * u,
            UiEase::OutQuad => u * (2.0 - u),
            UiEase::InOutQuad if u < 0.5 => 2.0 * u * u,
            UiEase::InOutQuad => 1.0 - 2.0 * inv * inv,
            UiEase::InCubic => u * u * u,
            UiEase::OutCubic => 1.0 - inv * inv * inv,
            UiEase::InOutCubic if u < 0.5 => 4.0 * u * u * u,
            UiEase::InOutCubic => 1.0 - 4.0 * inv * inv * inv,
            UiEase::OutBack => {
                const C1: f32 = 1.70158;
                const C3: f32 = C1 + 1.0;
                let d = u - 1.0;
                1.0 + C3 * d * d * d + C1 * d * d
            }
        }
    }
}

/// What a [`UiTweenTrack`] animates. The number is the arity of `from` and `to`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UiTweenField {
    /// Multiplied into the alpha of every colour the element draws (1). Every element.
    Opacity,
    /// Added to the element's `offset`, canvas units (2). Every element.
    Offset,
    /// Scales the drawn element about the centre of its box (1). `UiRect` and `UiText`.
    Scale,
    /// Replaces a `UiRect`'s `size`, canvas units (2). `UiRect` only.
    Size,
    /// Replaces the element's linear RGBA colour (4). `UiRect` and `UiText`.
    Color,
}

impl UiTweenField {
    /// How many numbers `from` and `to` hold for this field.
    pub fn arity(self) -> usize {
        match self {
            UiTweenField::Opacity | UiTweenField::Scale => 1,
            UiTweenField::Offset | UiTweenField::Size => 2,
            UiTweenField::Color => 4,
        }
    }
}

/// What a [`UiTweenTrack`] does when it reaches its end.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum UiTweenRepeat {
    /// Plays once, then holds `to`.
    #[default]
    Once,
    /// Restarts at `from` every `duration`.
    Loop,
    /// Plays forward, then backward, then forward again.
    PingPong,
}

/// Which frame count a [`UiTweenTrack`] measures against.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum UiClock {
    /// The sim frame: keeps running while a `GameClock` holds the game. The default, so a pause menu
    /// can animate over a held game.
    #[default]
    Real,
    /// The gameplay frame: stands still while a `GameClock` holds the game.
    Game,
}

/// One animated field of a [`UiTween`].
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct UiTweenTrack {
    /// What the track animates.
    pub field: UiTweenField,
    /// The value at `start`: `field.arity()` numbers.
    pub from: Vec<f32>,
    /// The value at `start + duration`: `field.arity()` numbers.
    pub to: Vec<f32>,
    /// The frame the track starts at, on its `clock` (default `0`). Before it the track holds `from`.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub start: u64,
    /// Length in seconds, `>= 0` (default `1`). `0` jumps to `to` at `start`.
    #[serde(default = "one_f32", skip_serializing_if = "is_one_f32")]
    #[schemars(range(min = 0.0))]
    pub duration: f32,
    /// The easing curve (default `linear`).
    #[serde(default, skip_serializing_if = "is_default_ease")]
    pub ease: UiEase,
    /// What happens after `duration` (default `once`).
    #[serde(default, skip_serializing_if = "is_default_repeat")]
    pub repeat: UiTweenRepeat,
    /// The frame count `start` is measured on (default `real`).
    #[serde(default, skip_serializing_if = "is_default_clock")]
    pub clock: UiClock,
}

/// Animates a screen-space element (`UiRect`, `UiText` or `UiList` on the same entity) closed-form
/// from a frame stamp. Nothing is written per tick: `extract` samples each track at the current
/// frame, so an element with no tween draws exactly as before.
///
/// Tracks apply in list order; a later track on the same field wins. A track whose field the element
/// cannot take is skipped with `L2300`. A wrong arity, non-finite value or negative `duration` is
/// `L1610`.
///
/// ```yaml
/// components:
///   UiRect: { anchor: center, size: [200, 40] }
///   UiTween:
///     tracks:
///       - { field: opacity, from: [0.0], to: [1.0], start: 0, duration: 0.25, ease: out_quad }
///       - { field: offset, from: [0.0, 24.0], to: [0.0, 0.0], duration: 0.25, ease: out_back }
/// ```
///
/// ```
/// use ludion_components::ui::{UiTween, UiTweenField};
///
/// let tween: UiTween = serde_json::from_value(serde_json::json!({
///     "tracks": [{ "field": "opacity", "from": [0.0], "to": [1.0] }]
/// })).unwrap();
/// assert_eq!(tween.tracks[0].field, UiTweenField::Opacity);
/// assert_eq!(tween.tracks[0].duration, 1.0);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
pub struct UiTween {
    /// The animated fields (default none).
    #[serde(default)]
    pub tracks: Vec<UiTweenTrack>,
}

/// When a [`UiWorldAnchor`] draws its element.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum UiAnchorShow {
    /// Always (default).
    #[default]
    Always,
    /// Only while the target projects inside the screen margin.
    OnScreen,
    /// Only while the target is off the screen, as an edge marker.
    OffScreen,
}

/// What a [`UiWorldAnchor`] does with a target behind the camera.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum UiAnchorBehind {
    /// Pins the element to the screen edge in the target's direction (default).
    #[default]
    Clamp,
    /// Draws nothing.
    Hide,
}

/// Pins a screen-space element to a 3D target: `extract` projects the target through the camera as
/// drawn and moves the element there, its `anchor` becoming its pivot on the projected point (the
/// `offset` still adds, in canvas units). Off screen it clamps to the edge, inset by `margin`.
/// A target that is dead or has no pose draws nothing. A `fill: true` rect ignores anchors (`L2300`).
///
/// ```yaml
/// components:
///   UiRect: { anchor: bottom_center, offset: [0, -12], size: [80, 8] }
///   UiWorldAnchor:
///     target: { index: 3, generation: 0 }
///     world_offset: [0.0, 2.0, 0.0]
///     fade: [20.0, 40.0]
/// ```
///
/// ```
/// use ludion_components::ui::{UiAnchorBehind, UiWorldAnchor};
///
/// let anchor: UiWorldAnchor = serde_json::from_value(serde_json::json!({ "target": { "index": 1, "generation": 0 } })).unwrap();
/// assert_eq!(anchor.margin, 48.0);
/// assert_eq!(anchor.behind, UiAnchorBehind::Clamp);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct UiWorldAnchor {
    /// The entity the element follows.
    #[schemars(with = "ludion_ecs::EntityRefSchema")]
    pub target: EntityId,
    /// Added to the target's world position, in world axes (default the origin).
    #[serde(default, skip_serializing_if = "is_zero_vec3")]
    pub world_offset: [f32; 3],
    /// When the element draws (default `always`).
    #[serde(default, skip_serializing_if = "is_default_show")]
    pub show: UiAnchorShow,
    /// What happens behind the camera (default `clamp`).
    #[serde(default, skip_serializing_if = "is_default_behind")]
    pub behind: UiAnchorBehind,
    /// Edge inset when clamping, canvas units, `>= 0` (default `48`).
    #[serde(default = "margin_default", skip_serializing_if = "is_margin_default")]
    #[schemars(range(min = 0.0))]
    pub margin: f32,
    /// `[near, far]` metres: opacity 1 at `near`, 0 at `far`, multiplied into alpha (default none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fade: Option<[f32; 2]>,
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}
fn one_f32() -> f32 {
    1.0
}
fn is_one_f32(value: &f32) -> bool {
    value.to_bits() == 1.0f32.to_bits()
}
fn margin_default() -> f32 {
    48.0
}
fn is_margin_default(value: &f32) -> bool {
    value.to_bits() == 48.0f32.to_bits()
}
fn is_zero_vec3(value: &[f32; 3]) -> bool {
    value.iter().all(|c| c.to_bits() == 0)
}
fn is_default_ease(value: &UiEase) -> bool {
    *value == UiEase::default()
}
fn is_default_repeat(value: &UiTweenRepeat) -> bool {
    *value == UiTweenRepeat::default()
}
fn is_default_clock(value: &UiClock) -> bool {
    *value == UiClock::default()
}
fn is_default_show(value: &UiAnchorShow) -> bool {
    *value == UiAnchorShow::default()
}
fn is_default_behind(value: &UiAnchorBehind) -> bool {
    *value == UiAnchorBehind::default()
}

fn non_finite(value: &Value) -> bool {
    value
        .as_f64()
        .map(engine_number)
        .is_some_and(|n| !n.is_finite())
}

fn arity_of(field: &str) -> Option<usize> {
    match field {
        "opacity" | "scale" => Some(1),
        "offset" | "size" => Some(2),
        "color" => Some(4),
        _ => None,
    }
}

impl SemanticValidate for UiTween {
    /// Rejects a track whose `from`/`to` length is not its field's arity, a non-finite number, or a
    /// negative `duration` (`L1610`, pointers `/tracks/{n}/from`, `/tracks/{n}/to/{i}`,
    /// `/tracks/{n}/duration`). A wrong *shape* is the schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut reject = |pointer: String, what: String| {
            out.push(
                Diagnostic::new(code::UI_TWEEN_INVALID, what)
                    .with_path(pointer)
                    .with_help("fix the track; `ludion explain L1610`"),
            );
        };
        let tracks = value.get("tracks").and_then(Value::as_array);
        for (n, track) in tracks.into_iter().flatten().enumerate() {
            let arity = track
                .get("field")
                .and_then(Value::as_str)
                .and_then(arity_of);
            for key in ["from", "to"] {
                let Some(items) = track.get(key).and_then(Value::as_array) else {
                    continue;
                };
                if let Some(arity) = arity
                    && items.len() != arity
                {
                    reject(
                        format!("/tracks/{n}/{key}"),
                        format!(
                            "track {n} `{key}` has {} numbers but its field takes {arity}",
                            items.len()
                        ),
                    );
                }
                for (i, _) in items.iter().enumerate().filter(|(_, v)| non_finite(v)) {
                    reject(
                        format!("/tracks/{n}/{key}/{i}"),
                        format!("track {n} `{key}` component {i} must be finite"),
                    );
                }
            }
            if let Some(duration) = track.get("duration").and_then(Value::as_f64) {
                let duration = engine_number(duration);
                if !duration.is_finite() || duration < 0.0 {
                    reject(
                        format!("/tracks/{n}/duration"),
                        format!("track {n} `duration` must be finite and at least 0 seconds"),
                    );
                }
            }
        }
        out
    }
}

impl SemanticValidate for UiWorldAnchor {
    /// Rejects a non-finite `world_offset` or `margin`, a negative `margin`, and a `fade` that is not
    /// `0 <= near < far` (`L1611`, pointers `/world_offset/{i}`, `/margin`, `/fade`). A wrong *shape*
    /// is the schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut reject = |pointer: String, what: String| {
            out.push(
                Diagnostic::new(code::UI_WORLD_ANCHOR_INVALID, what)
                    .with_path(pointer)
                    .with_help("fix the field; `ludion explain L1611`"),
            );
        };
        for (i, _) in value
            .get("world_offset")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
            .filter(|(_, v)| non_finite(v))
        {
            reject(
                format!("/world_offset/{i}"),
                format!("the `UiWorldAnchor` world_offset component {i} must be finite"),
            );
        }
        if let Some(margin) = value.get("margin").and_then(Value::as_f64) {
            let margin = engine_number(margin);
            if !margin.is_finite() || margin < 0.0 {
                reject(
                    "/margin".into(),
                    "the `UiWorldAnchor` margin must be finite and at least 0".into(),
                );
            }
        }
        if let Some(fade) = value.get("fade").and_then(Value::as_array)
            && let [near, far] = fade.as_slice()
            && let (Some(near), Some(far)) = (near.as_f64(), far.as_f64())
        {
            let (near, far) = (engine_number(near), engine_number(far));
            if !(near.is_finite() && far.is_finite() && near >= 0.0 && near < far) {
                reject(
                    "/fade".into(),
                    "the `UiWorldAnchor` fade must be `[near, far]` with 0 <= near < far".into(),
                );
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tween(track: Value) -> Value {
        json!({ "tracks": [track] })
    }

    #[test]
    fn tween_round_trips_and_skips_defaults() {
        let value =
            json!({ "tracks": [{ "field": "offset", "from": [0.0, 1.0], "to": [2.0, 3.0] }] });
        let parsed: UiTween = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(parsed.tracks[0].duration, 1.0);
        assert_eq!(parsed.tracks[0].clock, UiClock::Real);
        assert_eq!(serde_json::to_value(&parsed).unwrap(), value);
    }

    #[test]
    fn anchor_round_trips_and_skips_defaults() {
        let value = json!({ "target": { "index": 4, "generation": 0 } });
        let parsed: UiWorldAnchor = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), value);
        let full = json!({
            "target": { "index": 4, "generation": 0 }, "world_offset": [0.0, 2.0, 0.0], "show": "on_screen",
            "behind": "hide", "margin": 10.0, "fade": [1.0, 5.0]
        });
        let parsed: UiWorldAnchor = serde_json::from_value(full.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), full);
    }

    #[test]
    fn tween_arity_non_finite_and_duration_are_l1610() {
        let bad = [
            tween(json!({ "field": "color", "from": [0.0], "to": [1.0, 1.0, 1.0, 1.0] })),
            tween(json!({ "field": "opacity", "from": [0.0], "to": [1.0], "duration": -1.0 })),
        ];
        for value in bad {
            let out = UiTween::validate_semantics(&value);
            assert_eq!(out.len(), 1, "{value}");
            assert_eq!(out[0].code, code::UI_TWEEN_INVALID);
        }
        let out = UiTween::validate_semantics(&tween(
            json!({ "field": "opacity", "from": [0.0], "to": [1.0], "duration": 0.0 }),
        ));
        assert!(out.is_empty());
        let out = UiTween::validate_semantics(&tween(
            json!({ "field": "color", "from": [0.0], "to": [1.0, 1.0] }),
        ));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].path.as_deref(), Some("/tracks/0/from"));
    }

    #[test]
    fn anchor_margin_and_fade_are_l1611() {
        for value in [
            json!({ "target": { "index": 1, "generation": 0 }, "margin": -1.0 }),
            json!({ "target": { "index": 1, "generation": 0 }, "fade": [5.0, 5.0] }),
            json!({ "target": { "index": 1, "generation": 0 }, "fade": [-1.0, 5.0] }),
        ] {
            let out = UiWorldAnchor::validate_semantics(&value);
            assert_eq!(out.len(), 1, "{value}");
            assert_eq!(out[0].code, code::UI_WORLD_ANCHOR_INVALID);
        }
        assert!(
            UiWorldAnchor::validate_semantics(
                &json!({ "target": { "index": 1, "generation": 0 }, "fade": [0.0, 5.0] })
            )
            .is_empty()
        );
    }

    #[test]
    fn every_ease_is_exact_at_its_ends() {
        for ease in [
            UiEase::Linear,
            UiEase::Smooth,
            UiEase::Step,
            UiEase::InQuad,
            UiEase::OutQuad,
            UiEase::InOutQuad,
            UiEase::InCubic,
            UiEase::OutCubic,
            UiEase::InOutCubic,
            UiEase::OutBack,
        ] {
            assert_eq!(ease.apply(0.0), 0.0, "{ease:?}");
            assert_eq!(ease.apply(1.0), 1.0, "{ease:?}");
        }
        assert_eq!(UiEase::Step.apply(0.99), 0.0);
        assert_eq!(UiEase::Smooth.apply(0.5), 0.5);
        assert_eq!(UiEase::InOutQuad.apply(0.5), 0.5);
        assert_eq!(UiEase::InOutCubic.apply(0.5), 0.5);
    }
}
