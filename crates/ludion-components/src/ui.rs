//! [`UiNav`], the menu intent a game feeds to [`UiList::navigate`], and the HUD motion components
//! [`UiTween`] and [`UiWorldAnchor`], which `extract` resolves onto a screen-space element.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::number::engine_number;
use ludion_ecs::SemanticValidate;
use ludion_sim::actions::{ActionName, Actions};
use ludion_sim::{GamepadButton, InputState, Key};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{UiList, UiListItem};

/// One frame's menu intent: three edge flags a game feeds to [`UiList::navigate`].
///
/// Built from an [`InputState`] by [`UiNav::from_input`] under the engine's fixed bindings, or by
/// hand — the fields are public precisely so a game with its own bindings never needs an engine
/// change. [`UiNav::from_actions`] reads the action layer instead. There is no engine-side focus
/// manager: exactly one list is navigated per frame, the one the game chooses to call.
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

    /// Reads this frame's menu intent from the action layer, so a player's rebinds reach the menus.
    ///
    /// The reserved action names are `ui_prev`, `ui_next`, `ui_activate` and `ui_back`. Each field
    /// whose action the map defines is that action's [`pressed`](Actions::pressed); a field whose
    /// action is undefined keeps [`from_input`](UiNav::from_input)'s default, so a map may define
    /// only some. `ui_back` has no field: read it with `actions.pressed(ActionName::new("ui_back"))`.
    ///
    /// # Example
    ///
    /// ```
    /// use ludion_components::UiNav;
    /// use ludion_core::time::TickRate;
    /// use ludion_sim::actions::{ActionMap, Actions, InputActionsDoc};
    /// use ludion_sim::{InputEvent, InputState, Key};
    ///
    /// let doc: InputActionsDoc = serde_json::from_value(serde_json::json!({
    ///     "actions": { "ui_next": { "kind": "button", "bindings": [{ "key": "S" }] } }
    /// })).unwrap();
    /// let mut actions = Actions::new(ActionMap::compile(&doc, TickRate::DEFAULT).unwrap());
    /// let mut input = InputState::default();
    /// input.begin_tick();
    /// input.apply(InputEvent::KeyDown { key: Key::S });
    /// actions.resolve(&input);
    /// assert!(UiNav::from_actions(&actions, &input).next);
    /// ```
    pub fn from_actions(actions: &Actions, input: &InputState) -> Self {
        let default = Self::from_input(input);
        let read = |name: &'static str, fallback: bool| {
            let name = ActionName::new(name);
            if actions.contains(name) {
                actions.pressed(name)
            } else {
                fallback
            }
        };
        UiNav {
            prev: read("ui_prev", default.prev),
            next: read("ui_next", default.next),
            activate: read("ui_activate", default.activate),
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
    /// Added to the element's `rotation`, degrees (1). `UiRect`, `UiText` and `UiGroup`.
    Rotation,
}

impl UiTweenField {
    /// How many numbers `from` and `to` hold for this field.
    pub fn arity(self) -> usize {
        match self {
            UiTweenField::Opacity | UiTweenField::Scale | UiTweenField::Rotation => 1,
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

/// What a nine-slice does with its middle piece.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum UiSliceCenter {
    /// The centre stretches over the rect (default).
    #[default]
    Stretch,
    /// The centre is not drawn, leaving a frame.
    Hidden,
}

/// A nine-slice frame for a textured [`UiRect`](crate::UiRect): the corners keep their size, the
/// edges stretch along one axis and the centre stretches or is hidden.
///
/// ```
/// use ludion_components::ui::{UiSlice, UiSliceCenter};
///
/// let slice: UiSlice = serde_json::from_value(serde_json::json!({ "border": [4, 4, 4, 4] })).unwrap();
/// assert_eq!(slice.scale, 1.0);
/// assert_eq!(slice.center, UiSliceCenter::Stretch);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct UiSlice {
    /// `[left, top, right, bottom]` in source texels of the texture or atlas region, each `>= 0`.
    pub border: [f32; 4],
    /// Canvas units per source texel for the corners, `> 0` (default `1`).
    #[serde(default = "one_f32", skip_serializing_if = "is_one_f32")]
    pub scale: f32,
    /// What the middle piece does (default `stretch`).
    #[serde(default, skip_serializing_if = "is_default_center")]
    pub center: UiSliceCenter,
}

/// A screen-space box that other UI elements anchor inside, scale, fade and hide as one unit. It
/// draws nothing: it resolves like a [`UiRect`](crate::UiRect) (`anchor`, `offset`, `size`), and a
/// `UiRect`, `UiText`, `UiList` or another `UiGroup` with `group: <this entity>` anchors against that
/// box instead of the canvas. `scale` (about the box centre), `opacity` and `visible: false` compose
/// onto every descendant through nesting of up to 8 groups. `order` stays global; a group never
/// reorders its children. `UiTween` (offset, size, scale, opacity) and `UiWorldAnchor` work on a
/// group.
///
/// ```yaml
/// components:
///   UiGroup: { anchor: top_right, offset: [-24, 24], size: [360, 360], opacity: 0.9 }
/// ```
///
/// ```
/// use ludion_components::UiGroup;
///
/// let group: UiGroup = serde_json::from_value(serde_json::json!({})).unwrap();
/// assert_eq!(group, UiGroup::default());
/// assert_eq!(group.size, [100.0, 100.0]);
/// assert!(group.visible);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct UiGroup {
    /// Which canvas point (or outer group box point) the group is placed at (default `top_left`).
    #[serde(default)]
    pub anchor: crate::Anchor,
    /// Offset from the anchor point, in canvas units, **+Y down** (default `[0, 0]`).
    #[serde(default)]
    pub offset: [f32; 2],
    /// Size `[width, height]` in canvas units (default `[100, 100]`).
    #[serde(default = "crate::hundred2")]
    pub size: [f32; 2],
    /// Uniform scale about the box centre, composed onto every descendant, `> 0` (default `1`).
    #[serde(default = "one_f32", skip_serializing_if = "is_one_f32")]
    pub scale: f32,
    /// Rotation about the box centre in degrees, clockwise on screen, composed onto every descendant and
    /// its clip (default `0`).
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub rotation: f32,
    /// Opacity multiplied into every descendant's alpha, `0..=1` (default `1`).
    #[serde(default = "one_f32", skip_serializing_if = "is_one_f32")]
    pub opacity: f32,
    /// Whether the group's descendants draw at all (default `true`).
    #[serde(default = "crate::yes", skip_serializing_if = "is_true")]
    pub visible: bool,
    /// The outer group this one nests in (default none: it sits on the canvas).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<ludion_ecs::EntityRefSchema>")]
    pub group: Option<EntityId>,
    /// Cuts every descendant's pixels to the group's box, scaled with the group (default `none`).
    #[serde(default, skip_serializing_if = "UiClipShape::is_none")]
    pub clip: UiClipShape,
}

/// The shape a [`UiGroup`] clips its descendants to, externally tagged: `clip: rect`,
/// `clip: circle` (the circle inscribed in the group's box) or
/// `clip: { rounded_rect: { corner_radius: 12.0 } }` (canvas units, `>= 0`).
///
/// ```
/// use ludion_components::UiClipShape;
///
/// let clip: UiClipShape = serde_json::from_value(serde_json::json!("rect")).unwrap();
/// assert_eq!(clip, UiClipShape::Rect);
/// assert!(UiClipShape::default().is_none());
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum UiClipShape {
    /// No clip.
    #[default]
    None,
    /// The group's box.
    Rect,
    /// The circle inscribed in the group's box.
    Circle,
    /// The group's box with rounded corners.
    RoundedRect {
        /// Corner radius in canvas units, finite and `>= 0`.
        corner_radius: f32,
    },
}

impl UiClipShape {
    /// Whether this is [`UiClipShape::None`].
    pub fn is_none(&self) -> bool {
        matches!(self, UiClipShape::None)
    }
}

impl Default for UiGroup {
    fn default() -> Self {
        UiGroup {
            anchor: crate::Anchor::TopLeft,
            offset: [0.0, 0.0],
            size: [100.0, 100.0],
            scale: 1.0,
            rotation: 0.0,
            opacity: 1.0,
            visible: true,
            group: None,
            clip: UiClipShape::None,
        }
    }
}

fn is_true(value: &bool) -> bool {
    *value
}

impl SemanticValidate for UiGroup {
    /// Rejects a `size` axis that is not strictly positive and finite (`L1118`, `/size/{i}`), a
    /// `scale` that is non-finite or not above 0, an `opacity` that is non-finite or outside
    /// `0..=1`, and a `rounded_rect` `corner_radius` that is non-finite or negative (`L1614`,
    /// `/scale`, `/opacity`, `/clip/rounded_rect/corner_radius`).
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        if let Some(size) = value.get("size").and_then(Value::as_array) {
            for (index, element) in size.iter().enumerate().take(2) {
                if let Some(extent) = element.as_f64().map(engine_number) {
                    crate::require_positive_finite_ui_size(
                        extent,
                        &format!("/size/{index}"),
                        &mut out,
                    );
                }
            }
        }
        let mut reject = |pointer: &str, what: &str| {
            out.push(
                Diagnostic::new(code::UI_GROUP_INVALID, what.to_string())
                    .with_path(pointer)
                    .with_help("fix the field; `ludion explain L1614`"),
            );
        };
        if let Some(scale) = value
            .get("scale")
            .and_then(Value::as_f64)
            .map(engine_number)
            && !(scale.is_finite() && scale > 0.0)
        {
            reject("/scale", "the `UiGroup` scale must be finite and above 0");
        }
        if let Some(opacity) = value
            .get("opacity")
            .and_then(Value::as_f64)
            .map(engine_number)
            && !(opacity.is_finite() && (0.0..=1.0).contains(&opacity))
        {
            reject(
                "/opacity",
                "the `UiGroup` opacity must be finite and within 0..1",
            );
        }
        if let Some(radius) = value
            .get("clip")
            .and_then(|clip| clip.get("rounded_rect"))
            .and_then(|shape| shape.get("corner_radius"))
            .and_then(Value::as_f64)
            .map(engine_number)
            && !(radius.is_finite() && radius >= 0.0)
        {
            reject(
                "/clip/rounded_rect/corner_radius",
                "the `UiGroup` clip corner_radius must be finite and at least 0",
            );
        }
        validate_rotation(value, &mut out);
        out
    }
}

/// An outline drawn under the glyphs of a [`UiText`](crate::UiText) or [`UiList`]. A `width` of `0`,
/// or a `color` alpha of `0`, draws nothing.
///
/// ```
/// use ludion_components::TextOutline;
///
/// let outline: TextOutline =
///     serde_json::from_value(serde_json::json!({ "width": 2.0, "color": [0.0, 0.0, 0.0, 1.0] })).unwrap();
/// assert_eq!(outline.width, 2.0);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct TextOutline {
    /// Outline thickness in canvas units, finite and `>= 0`. Clamped to what the face can draw
    /// (`L2304`).
    pub width: f32,
    /// Linear straight RGBA of the outline.
    pub color: [f32; 4],
}

/// Rejects an `outline` with a `width` that is non-finite or negative, or a non-finite `color`
/// channel (`L1618`, `/outline/width`, `/outline/color`). Shared by `UiText` and `UiList`.
pub(crate) fn validate_text_outline(value: &Value, out: &mut Vec<Diagnostic>) {
    let Some(outline) = value.get("outline") else {
        return;
    };
    let mut reject = |pointer: &str, what: &str| {
        out.push(
            Diagnostic::new(code::UI_TEXT_OUTLINE_INVALID, what.to_string())
                .with_path(pointer)
                .with_help("fix the field; `ludion explain L1618`"),
        );
    };
    if let Some(width) = outline
        .get("width")
        .and_then(Value::as_f64)
        .map(engine_number)
        && !(width.is_finite() && width >= 0.0)
    {
        reject(
            "/outline/width",
            "the text outline width must be finite and at least 0",
        );
    }
    if let Some(color) = outline.get("color").and_then(Value::as_array)
        && color
            .iter()
            .filter_map(Value::as_f64)
            .map(engine_number)
            .any(|channel| !channel.is_finite())
    {
        reject(
            "/outline/color",
            "the text outline color must have finite channels",
        );
    }
}

/// Rejects a `rotation` that is not finite (`L1620`, `/rotation`). Shared by `UiRect`, `UiText` and
/// `UiGroup`.
pub(crate) fn validate_rotation(value: &Value, out: &mut Vec<Diagnostic>) {
    if let Some(rotation) = value
        .get("rotation")
        .and_then(Value::as_f64)
        .map(engine_number)
        && !rotation.is_finite()
    {
        out.push(
            Diagnostic::new(
                code::UI_ROTATION_INVALID,
                "the UI rotation must be finite".to_string(),
            )
            .with_path("/rotation")
            .with_help("give `rotation` a finite number of degrees; `ludion explain L1620`"),
        );
    }
}

fn is_zero_f32(value: &f32) -> bool {
    value.to_bits() == 0.0f32.to_bits()
}

fn is_default_center(value: &UiSliceCenter) -> bool {
    *value == UiSliceCenter::default()
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

/// `L1612` / `L1613` for a `UiRect`'s `uv_rect` and `slice`, pushed onto `out`. Pointers are relative
/// to the component (`/uv_rect/{i}`, `/slice/border/{i}`, `/slice/scale`, `/uv_rect`, `/slice`).
pub(crate) fn ui_rect_sampling_diagnostics(
    object: &serde_json::Map<String, Value>,
    out: &mut Vec<Diagnostic>,
) {
    let present = |field: &str| object.get(field).is_some_and(|v| !v.is_null());
    let number = |value: &Value| value.as_f64().map(engine_number);
    let mut invalid = |pointer: String, what: String| {
        out.push(
            Diagnostic::new(code::UI_RECT_SAMPLING_INVALID, what)
                .with_path(pointer)
                .with_help("fix the field; `ludion explain L1612`"),
        );
    };
    if let Some(uv) = object.get("uv_rect").and_then(Value::as_array) {
        let values: Vec<Option<f64>> = uv.iter().map(number).collect();
        for (i, value) in values.iter().enumerate() {
            if let Some(v) = value
                && !(v.is_finite() && (0.0..=1.0).contains(v))
            {
                invalid(
                    format!("/uv_rect/{i}"),
                    format!("`uv_rect` component {i} must be finite and within 0..1"),
                );
            }
        }
        if let [Some(u0), Some(v0), Some(u1), Some(v1)] = values.as_slice() {
            for (axis, lo, hi, at) in [("u", u0, u1, 0), ("v", v0, v1, 1)] {
                if lo >= hi {
                    invalid(
                        format!("/uv_rect/{at}"),
                        format!("`uv_rect` needs {axis}0 < {axis}1 so the sub-rect has an area"),
                    );
                }
            }
        }
    }
    if let Some(slice) = object.get("slice").and_then(Value::as_object) {
        let border = slice.get("border").and_then(Value::as_array);
        for (i, v) in border.into_iter().flatten().enumerate() {
            if let Some(v) = number(v)
                && !(v.is_finite() && v >= 0.0)
            {
                invalid(
                    format!("/slice/border/{i}"),
                    format!("`slice.border` component {i} must be finite and at least 0"),
                );
            }
        }
        if let Some(scale) = slice.get("scale").and_then(number)
            && !(scale.is_finite() && scale > 0.0)
        {
            invalid(
                "/slice/scale".into(),
                "`slice.scale` must be finite and above 0".into(),
            );
        }
    }
    let mut ignored = |pointer: &str, what: String, help: &str| {
        out.push(
            Diagnostic::new(code::UI_RECT_SAMPLING_IGNORED, what)
                .with_path(pointer)
                .with_help(help),
        );
    };
    if present("uv_rect") && present("slice") {
        ignored(
            "/uv_rect",
            "this `UiRect` has both `uv_rect` and `slice`; `slice` wins and `uv_rect` is ignored"
                .into(),
            "keep one of the two",
        );
    }
    if !present("texture") && !present("atlas") && !present("view") {
        for field in ["uv_rect", "slice"].into_iter().filter(|f| present(f)) {
            ignored(
                &format!("/{field}"),
                format!("this `UiRect` has no `texture` or `atlas`, so `{field}` is ignored"),
                "add a `texture`, or an `atlas` and `frame`, or remove the field",
            );
        }
    }
}

/// `L1617` for a `UiRect`'s `fill_group` and `fill` beside `group`, pushed onto `out`. Pointers are
/// relative to the component (`/fill_group`, `/fill`).
pub(crate) fn ui_rect_group_diagnostics(
    object: &serde_json::Map<String, Value>,
    out: &mut Vec<Diagnostic>,
) {
    let present = |field: &str| object.get(field).is_some_and(|v| !v.is_null());
    let flag = |field: &str| object.get(field).and_then(Value::as_bool) == Some(true);
    let mut ignored = |pointer: &str, what: &str, help: &str| {
        out.push(
            Diagnostic::new(code::UI_GROUP_IGNORED, what.to_string())
                .with_path(pointer)
                .with_help(help.to_string()),
        );
    };
    if flag("fill_group") && !present("group") {
        ignored(
            "/fill_group",
            "this `UiRect` has `fill_group: true` but no `group`, so it is ignored",
            "set `group`, or drop `fill_group`",
        );
    }
    if flag("fill") && (present("group") || flag("fill_group")) {
        ignored(
            "/fill",
            "this `UiRect` has `fill: true`, so its `group` and `fill_group` are ignored",
            "keep either `fill` or the group fields",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn group_diag(value: Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        ui_rect_group_diagnostics(value.as_object().unwrap(), &mut out);
        out
    }

    #[test]
    fn group_round_trips_and_skips_defaults() {
        let empty: UiGroup = serde_json::from_value(json!({})).unwrap();
        assert_eq!(empty, UiGroup::default());
        assert_eq!(
            serde_json::to_value(&empty).unwrap(),
            json!({ "anchor": "top_left", "offset": [0.0, 0.0], "size": [100.0, 100.0] })
        );
        let full = json!({
            "anchor": "top_right", "offset": [-24.0, 24.0], "size": [360.0, 360.0],
            "scale": 0.5, "opacity": 0.25, "visible": false,
            "group": { "index": 2, "generation": 0 }
        });
        let parsed: UiGroup = serde_json::from_value(full.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), full);
    }

    #[test]
    fn group_clip_round_trips_in_both_spellings() {
        for clip in [
            json!("rect"),
            json!("circle"),
            json!({ "rounded_rect": { "corner_radius": 12.0 } }),
        ] {
            let value = json!({
                "anchor": "top_left", "offset": [0.0, 0.0], "size": [100.0, 100.0], "clip": clip
            });
            let parsed: UiGroup = serde_json::from_value(value.clone()).unwrap();
            assert!(!parsed.clip.is_none());
            assert_eq!(serde_json::to_value(&parsed).unwrap(), value);
        }
        let absent = serde_json::to_value(UiGroup::default()).unwrap();
        assert!(absent.get("clip").is_none());
    }

    #[test]
    fn group_clip_corner_radius_is_l1614() {
        for radius in [-1.0, -0.001] {
            let value = json!({ "clip": { "rounded_rect": { "corner_radius": radius } } });
            let out = UiGroup::validate_semantics(&value);
            assert_eq!(
                pointers(&out, "L1614"),
                ["/clip/rounded_rect/corner_radius"],
                "{value}"
            );
        }
        let ok = json!({ "clip": { "rounded_rect": { "corner_radius": 0.0 } } });
        assert!(UiGroup::validate_semantics(&ok).is_empty());
    }

    #[test]
    fn rect_without_group_fields_serializes_as_before() {
        let rect = serde_json::to_value(crate::UiRect::default()).unwrap();
        assert!(rect.get("group").is_none() && rect.get("fill_group").is_none());
        let text = serde_json::to_value(crate::UiText::default()).unwrap();
        assert!(text.get("group").is_none());
        let list = serde_json::to_value(crate::UiList::default()).unwrap();
        assert!(list.get("group").is_none());
    }

    #[test]
    fn group_scale_and_opacity_are_l1614() {
        for (value, pointer) in [
            (json!({ "scale": 0.0 }), "/scale"),
            (json!({ "scale": -1.0 }), "/scale"),
            (json!({ "opacity": 1.5 }), "/opacity"),
            (json!({ "opacity": -0.1 }), "/opacity"),
        ] {
            let out = UiGroup::validate_semantics(&value);
            assert_eq!(pointers(&out, "L1614"), [pointer], "{value}");
        }
        let out = UiGroup::validate_semantics(&json!({ "size": [0.0, 10.0] }));
        assert_eq!(pointers(&out, "L1118"), ["/size/0"]);
        assert!(UiGroup::validate_semantics(&json!({ "scale": 2.0, "opacity": 0.0 })).is_empty());
        assert!(UiGroup::validate_semantics(&json!({})).is_empty());
    }

    #[test]
    fn fill_group_without_group_and_fill_beside_group_are_l1617() {
        let d = group_diag(json!({ "fill_group": true }));
        assert_eq!(pointers(&d, "L1617"), ["/fill_group"]);
        let d = group_diag(json!({ "fill": true, "group": { "index": 1, "generation": 0 } }));
        assert_eq!(pointers(&d, "L1617"), ["/fill"]);
        let d = group_diag(json!({
            "fill_group": true, "group": { "index": 1, "generation": 0 }
        }));
        assert!(d.is_empty());
        assert!(group_diag(json!({})).is_empty());
    }

    fn sampling(value: Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        ui_rect_sampling_diagnostics(value.as_object().unwrap(), &mut out);
        out
    }

    fn pointers(diagnostics: &[Diagnostic], code: &str) -> Vec<String> {
        diagnostics
            .iter()
            .filter(|d| d.code.as_str() == code)
            .map(|d| d.path.clone().unwrap_or_default())
            .collect()
    }

    #[test]
    fn slice_round_trips_and_skips_defaults() {
        let parsed: UiSlice =
            serde_json::from_value(json!({ "border": [1.0, 2.0, 3.0, 4.0] })).unwrap();
        assert_eq!(parsed.scale, 1.0);
        assert_eq!(parsed.center, UiSliceCenter::Stretch);
        assert_eq!(
            serde_json::to_value(&parsed).unwrap(),
            json!({ "border": [1.0, 2.0, 3.0, 4.0] })
        );
        let full = json!({ "border": [1.0, 2.0, 3.0, 4.0], "scale": 2.0, "center": "hidden" });
        let parsed: UiSlice = serde_json::from_value(full.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), full);
    }

    #[test]
    fn sampling_valid_fields_add_nothing() {
        assert!(
            sampling(json!({ "texture": "a.png", "uv_rect": [0.0, 0.0, 0.5, 1.0] })).is_empty()
        );
        assert!(
            sampling(
                json!({ "atlas": "a.atlas", "slice": { "border": [4, 4, 4, 4], "scale": 2.0 } })
            )
            .is_empty()
        );
        assert!(sampling(json!({})).is_empty());
    }

    #[test]
    fn sampling_invalid_pointers() {
        let d = sampling(json!({ "texture": "a.png", "uv_rect": [0.0, 0.0, 1.5, 1.0] }));
        assert_eq!(pointers(&d, "L1612"), ["/uv_rect/2"]);
        let d = sampling(json!({ "texture": "a.png", "uv_rect": [0.5, 0.0, 0.5, 0.2] }));
        assert_eq!(pointers(&d, "L1612"), ["/uv_rect/0"]);
        let d = sampling(json!({ "texture": "a.png", "uv_rect": [0.0, 0.5, 1.0, 0.25] }));
        assert_eq!(pointers(&d, "L1612"), ["/uv_rect/1"]);
        let d = sampling(
            json!({ "texture": "a.png", "slice": { "border": [1, -1, 1, 1], "scale": 0.0 } }),
        );
        assert_eq!(pointers(&d, "L1612"), ["/slice/border/1", "/slice/scale"]);
    }

    #[test]
    fn sampling_ignored_cases() {
        let d = sampling(json!({
            "texture": "a.png",
            "uv_rect": [0.0, 0.0, 0.5, 1.0],
            "slice": { "border": [1, 1, 1, 1] }
        }));
        assert_eq!(pointers(&d, "L1613"), ["/uv_rect"]);
        let d = sampling(
            json!({ "uv_rect": [0.0, 0.0, 0.5, 1.0], "slice": { "border": [1, 1, 1, 1] } }),
        );
        assert_eq!(pointers(&d, "L1613"), ["/uv_rect", "/uv_rect", "/slice"]);
    }

    #[test]
    fn from_actions_reads_the_map_and_falls_back_per_field() {
        use ludion_core::time::TickRate;
        use ludion_sim::InputEvent;
        use ludion_sim::actions::{ActionMap, InputActionsDoc};

        let doc: InputActionsDoc = serde_json::from_value(json!({
            "actions": { "ui_next": { "kind": "button", "bindings": [{ "key": "S" }] } }
        }))
        .unwrap();
        let mut actions = Actions::new(ActionMap::compile(&doc, TickRate::DEFAULT).unwrap());
        let mut input = InputState::default();
        input.begin_tick();
        input.apply(InputEvent::KeyDown { key: Key::S });
        input.apply(InputEvent::KeyDown { key: Key::Up });
        actions.resolve(&input);
        let nav = UiNav::from_actions(&actions, &input);
        assert!(nav.next, "the rebound key reads");
        assert!(
            nav.prev,
            "an undefined `ui_prev` falls back to the default Up key"
        );
        assert!(!nav.activate);
        assert!(
            actions.take_unknown().is_empty(),
            "from_actions never reports L4270"
        );
    }

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
