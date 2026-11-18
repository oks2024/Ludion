//! The [`UiRadial`] component: a wheel of up to 16 items selected by an angle, with the deterministic
//! [`UiRadial::navigate`] a game feeds one frame of intent. `ludion docs ui-radial` is the guide.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math;
use ludion_core::number::engine_number;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Anchor, TextOutline};

/// The most items a wheel holds.
pub const MAX_RADIAL_ITEMS: usize = 16;

/// Degrees per radian, written out so no `std` conversion is involved.
const DEGREES_PER_RADIAN: f32 = 57.295_78;

/// One choice on a [`UiRadial`]: a label, an optional second line and icon, and whether it can be picked.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct UiRadialItem {
    /// The label, drawn at the item's mid-radius (default `""`). Always one row.
    #[serde(default)]
    pub text: String,
    /// A smaller line under the label (default none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The atlas an icon is drawn from, relative to the content root (default none: no icon).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub atlas: Option<String>,
    /// The atlas frame the icon shows (default the whole atlas).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
    /// Whether the item can be selected and activated (default `true`). A disabled item is drawn in
    /// `disabled_color` and never selected by aim or step.
    #[serde(default = "crate::yes", skip_serializing_if = "crate::is_true")]
    pub enabled: bool,
}

impl Default for UiRadialItem {
    fn default() -> Self {
        UiRadialItem {
            text: String::new(),
            detail: None,
            atlas: None,
            frame: None,
            enabled: true,
        }
    }
}

/// A tool wheel: up to 16 items on a ring, the selected one chosen by aim. Screen space, like
/// [`UiList`](crate::UiList): it shares `UiRect`'s 1080-unit canvas and pivot rule, over a square box
/// `2 * radius_outer` wide, so the wheel centre is the box centre.
///
/// Angles are clock degrees: 0 is 12 o'clock and positive is clockwise. Item `i` is centred at
/// `start_deg + i * 360 / n`. [`navigate`](UiRadial::navigate) moves `selected` and `pointer`, the only
/// fields it writes, and reports an activation as its return value.
///
/// ```
/// use ludion_components::{UiRadial, UiRadialItem};
///
/// let radial: UiRadial = serde_json::from_value(serde_json::json!({
///     "items": [{ "text": "AXE" }, { "text": "BOW" }]
/// })).unwrap();
/// assert_eq!(radial.radius_outer, 220.0);
/// assert_eq!(radial.selected, None);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct UiRadial {
    /// Which canvas point (and box pivot) the wheel is placed at (default [`Anchor::TopLeft`]).
    #[serde(default)]
    pub anchor: Anchor,
    /// Offset from the anchor point, in canvas units, **+Y down** (default `[0, 0]`).
    #[serde(default)]
    pub offset: [f32; 2],
    /// The [`UiGroup`](crate::UiGroup) the wheel anchors inside, instead of the canvas (default none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<ludion_ecs::EntityRefSchema>")]
    pub group: Option<EntityId>,
    /// Inner radius in canvas units (default `90`).
    #[serde(default = "ninety")]
    pub radius_inner: f32,
    /// Outer radius in canvas units, above `radius_inner` (default `220`).
    #[serde(default = "two_twenty")]
    pub radius_outer: f32,
    /// Clock degrees of item 0's centre (default `0`).
    #[serde(default)]
    pub start_deg: f32,
    /// Degrees left empty between sectors, in `[0, 360 / items)`; drawing and `radial_hit` only
    /// (default `2`).
    #[serde(default = "two")]
    pub gap_deg: f32,
    /// The choices, clockwise from `start_deg` (default empty: an empty wheel draws nothing).
    #[serde(default)]
    pub items: Vec<UiRadialItem>,
    /// The selected item (default none). An index past the end reads as none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<u32>,
    /// The accumulated pointer offset of a delta aim, in canvas units, +Y down (default `[0, 0]`).
    #[serde(default)]
    pub pointer: [f32; 2],
    /// Aim magnitude below which `selected` holds, in `[0, 1)` (default `0.45`).
    #[serde(default = "deadzone_default")]
    pub deadzone: f32,
    /// Degrees past a sector's edge, per side, that the selected item is kept (default `6`, total
    /// widening; `3` each side).
    #[serde(default = "six")]
    pub hysteresis_deg: f32,
    /// Label glyph cell height in canvas units (default `24`).
    #[serde(default = "crate::twenty_four")]
    pub size: f32,
    /// Logical face name from `fonts.yaml` (default the table's default face).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// Icon edge in canvas units (default `48`).
    #[serde(default = "forty_eight")]
    pub icon_size: f32,
    /// Linear RGBA of an ordinary item's text (default opaque white).
    #[serde(default = "crate::white4")]
    pub color: [f32; 4],
    /// Linear RGBA of the selected item's text (default a warm highlight).
    #[serde(default = "crate::ui_list_selected_color_default")]
    pub selected_color: [f32; 4],
    /// Linear RGBA of a disabled item's text; outranks `selected_color` (default a dimmed grey).
    #[serde(default = "crate::ui_list_disabled_color_default")]
    pub disabled_color: [f32; 4],
    /// Draw order: higher draws later (default `0`).
    #[serde(default)]
    pub order: i32,
    /// Whether the wheel draws at all (default `true`).
    #[serde(default = "crate::yes")]
    pub visible: bool,
    /// An outline under every text line's glyphs (default none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline: Option<TextOutline>,
}

fn ninety() -> f32 {
    90.0
}
fn two_twenty() -> f32 {
    220.0
}
fn two() -> f32 {
    2.0
}
fn six() -> f32 {
    6.0
}
fn deadzone_default() -> f32 {
    0.45
}
fn forty_eight() -> f32 {
    48.0
}

impl Default for UiRadial {
    fn default() -> Self {
        UiRadial {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            group: None,
            radius_inner: 90.0,
            radius_outer: 220.0,
            start_deg: 0.0,
            gap_deg: 2.0,
            items: Vec::new(),
            selected: None,
            pointer: [0.0, 0.0],
            deadzone: 0.45,
            hysteresis_deg: 6.0,
            size: 24.0,
            font: None,
            icon_size: 48.0,
            color: [1.0; 4],
            selected_color: crate::ui_list_selected_color_default(),
            disabled_color: crate::ui_list_disabled_color_default(),
            order: 0,
            visible: true,
            outline: None,
        }
    }
}

/// How [`UiRadialNav::aim`] is read.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum AimKind {
    /// A stick, `+y` up, magnitude in `[0, 1]`: compared to the wheel's `deadzone`.
    #[default]
    Stick,
    /// A cursor offset from the wheel centre in pixels, `+y` down: inside `dead_radius_px` holds.
    Cursor {
        /// The radius, in pixels, inside which the cursor selects nothing.
        dead_radius_px: f32,
    },
    /// A pointer delta (a locked mouse), `+y` down: summed into `pointer`, clamped to `radius_outer`.
    Delta,
}

/// One frame's wheel intent, fed to [`UiRadial::navigate`].
///
/// There is no engine focus manager: the game fills `step`, `activate` and `cancel` from its own
/// bindings, and navigates one wheel per frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UiRadialNav {
    /// The aim vector, read per `aim_kind`.
    pub aim: [f32; 2],
    /// How `aim` is read.
    pub aim_kind: AimKind,
    /// Items to walk, wrapping and skipping disabled ones; `0` is no step.
    pub step: i32,
    /// Activate the selected item.
    pub activate: bool,
    /// Close without activating; beats `activate`.
    pub cancel: bool,
}

impl UiRadialNav {
    /// An aim from a stick, `+y` up (`InputState::gamepad().left_stick()` as it is).
    pub fn from_stick(stick: [f32; 2]) -> Self {
        UiRadialNav {
            aim: stick,
            aim_kind: AimKind::Stick,
            ..Self::default()
        }
    }

    /// An aim from a cursor offset in pixels from the wheel centre, `+y` down.
    pub fn from_cursor(offset_px: [f32; 2], dead_radius_px: f32) -> Self {
        UiRadialNav {
            aim: offset_px,
            aim_kind: AimKind::Cursor { dead_radius_px },
            ..Self::default()
        }
    }

    /// An aim from a pointer delta (a locked mouse), `+y` down.
    pub fn from_delta(delta: [f32; 2]) -> Self {
        UiRadialNav {
            aim: delta,
            aim_kind: AimKind::Delta,
            ..Self::default()
        }
    }
}

/// What a [`UiRadial::navigate`] call decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RadialEvent {
    /// The selected, enabled item was activated.
    Activated(u32),
    /// The wheel was cancelled.
    Cancelled,
}

/// The sector index of clock angle `theta_deg` on a wheel of `count` sectors whose item 0 is centred at
/// `start_deg`. An exact boundary belongs to the clockwise sector.
pub(crate) fn sector_of(theta_deg: f32, start_deg: f32, count: usize) -> usize {
    let span = 360.0 / count as f32;
    let relative = (theta_deg - start_deg + span * 0.5).rem_euclid(360.0);
    ((relative / span).floor() as usize) % count
}

/// The clock angle in `[0, 360)` of an up-positive vector `(x, y)`: 0 at 12 o'clock, clockwise.
pub(crate) fn clock_degrees(x: f32, y: f32) -> f32 {
    (math::atan2(x, y) * DEGREES_PER_RADIAN).rem_euclid(360.0)
}

impl UiRadial {
    /// The item whose sector, gap excluded, the up-positive direction `(x, y)` points into, or `None`
    /// for an empty wheel or a direction inside a gap. Angle only: the caller tests the radii. An edge
    /// of the gap belongs to the sector.
    ///
    /// ```
    /// use ludion_components::{UiRadial, UiRadialItem};
    ///
    /// let wheel = UiRadial { items: vec![UiRadialItem::default(); 4], ..UiRadial::default() };
    /// assert_eq!(wheel.sector_hit(0.0, 1.0), Some(0)); // 12 o'clock
    /// assert_eq!(wheel.sector_hit(1.0, 1.0), None); // on the 0/1 boundary, inside the 2-degree gap
    /// assert_eq!(wheel.sector_hit(1.0, 0.0), Some(1));
    /// ```
    pub fn sector_hit(&self, x: f32, y: f32) -> Option<usize> {
        let count = self.items.len();
        if count == 0 {
            return None;
        }
        let span = 360.0 / count as f32;
        let theta = clock_degrees(x, y);
        let sector = sector_of(theta, self.start_deg, count);
        let centre = self.start_deg + sector as f32 * span;
        let offset = (theta - centre + 180.0).rem_euclid(360.0) - 180.0;
        let edge = (span * 0.5 - offset).min(span * 0.5 + offset);
        (self.gap_deg <= 0.0 || edge >= self.gap_deg * 0.5).then_some(sector)
    }

    /// Applies one frame's [`UiRadialNav`] and reports an activation or a cancel.
    ///
    /// In order: the aim moves `selected` (inside `deadzone` or `dead_radius_px` it holds; the selected
    /// item is kept while the angle stays within its sector widened by `hysteresis_deg / 2` a side; a
    /// disabled target changes nothing), then `step` walks enabled items with wrap (from no selection
    /// the first step lands on the first enabled item), then `cancel` beats `activate`. A cancel or an
    /// activation resets `pointer` to the centre and keeps `selected`.
    ///
    /// `selected` and `pointer` are the only fields written, and only on change, so an idle wheel
    /// stays bit-identical. An empty wheel returns `None` and writes nothing.
    ///
    /// ```
    /// use ludion_components::{RadialEvent, UiRadial, UiRadialItem, UiRadialNav};
    ///
    /// let item = |t: &str| UiRadialItem { text: t.into(), ..UiRadialItem::default() };
    /// let mut wheel = UiRadial {
    ///     items: vec![item("N"), item("E"), item("S"), item("W")],
    ///     ..UiRadial::default()
    /// };
    /// wheel.navigate(UiRadialNav::from_stick([1.0, 0.0])); // stick right: three o'clock
    /// assert_eq!(wheel.selected, Some(1));
    /// let activate = UiRadialNav { activate: true, ..UiRadialNav::default() };
    /// assert_eq!(wheel.navigate(activate), Some(RadialEvent::Activated(1)));
    /// ```
    pub fn navigate(&mut self, nav: UiRadialNav) -> Option<RadialEvent> {
        let count = self.items.len();
        if count == 0 {
            return None;
        }
        let span = 360.0 / count as f32;
        let current = self.selected.filter(|&s| (s as usize) < count);
        let mut selected = current;

        if let Some(up) = self.aim(nav) {
            let theta = clock_degrees(up[0], up[1]);
            let held = selected.is_some_and(|s| {
                let centre = self.start_deg + s as f32 * span;
                let offset = (theta - centre + 180.0).rem_euclid(360.0) - 180.0;
                offset.abs() <= span * 0.5 + self.hysteresis_deg * 0.5
            });
            if !held {
                let target = sector_of(theta, self.start_deg, count);
                if self.items[target].enabled {
                    selected = Some(target as u32);
                }
            }
        }

        if nav.step != 0 {
            selected = self.step(selected, nav.step);
        }
        if selected != current {
            self.selected = selected;
        }

        let event = if nav.cancel {
            Some(RadialEvent::Cancelled)
        } else if nav.activate {
            selected
                .filter(|&s| self.items[s as usize].enabled)
                .map(RadialEvent::Activated)
        } else {
            None
        };
        if event.is_some() && self.pointer != [0.0, 0.0] {
            self.pointer = [0.0, 0.0];
        }
        event
    }

    /// The aim as an up-positive vector, or `None` when it holds the selection. A delta aim
    /// accumulates into `pointer` first.
    fn aim(&mut self, nav: UiRadialNav) -> Option<[f32; 2]> {
        let [x, y] = nav.aim;
        match nav.aim_kind {
            AimKind::Stick => (math::hypot(x, y) >= self.deadzone).then_some([x, y]),
            AimKind::Cursor { dead_radius_px } => {
                (math::hypot(x, y) > dead_radius_px).then_some([x, -y])
            }
            AimKind::Delta => {
                let mut pointer = [self.pointer[0] + x, self.pointer[1] + y];
                let length = math::hypot(pointer[0], pointer[1]);
                if length > self.radius_outer {
                    let k = self.radius_outer / length;
                    pointer = [pointer[0] * k, pointer[1] * k];
                }
                if pointer != self.pointer {
                    self.pointer = pointer;
                }
                let length = math::hypot(pointer[0], pointer[1]);
                (length / self.radius_outer >= self.deadzone).then_some([pointer[0], -pointer[1]])
            }
        }
    }

    /// Walks `|steps|` enabled items from `from`, wrapping; `None` stays `None` when nothing is enabled.
    fn step(&self, from: Option<u32>, steps: i32) -> Option<u32> {
        let count = self.items.len();
        let forward = steps > 0;
        let mut at = from.map(|s| s as usize);
        for _ in 0..steps.unsigned_abs() {
            let enabled = |i: &usize| self.items[*i].enabled;
            let next = match at {
                None => (0..count).find(enabled),
                Some(from) if forward => (1..=count).map(|k| (from + k) % count).find(enabled),
                Some(from) => (1..=count)
                    .map(|k| (from + count - k) % count)
                    .find(enabled),
            };
            match next {
                Some(i) => at = Some(i),
                None => break,
            }
        }
        at.map(|i| i as u32)
    }
}

impl SemanticValidate for UiRadial {
    /// Rejects (`L1619`) a non-finite number; a `radius_inner` below 0 or a `radius_outer` not above it;
    /// a `gap_deg` or `hysteresis_deg` outside `[0, 360 / items)`; a `deadzone` outside `[0, 1)`; a
    /// `size` or `icon_size` that is not positive and finite; and more than [`MAX_RADIAL_ITEMS`] items.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut reject = |pointer: String, what: String| {
            out.push(
                Diagnostic::new(code::UI_RADIAL_INVALID, what)
                    .with_path(pointer)
                    .with_help("fix the field; `ludion explain L1619`"),
            );
        };
        let number = |key: &str, default: f64| -> f64 {
            value
                .get(key)
                .and_then(Value::as_f64)
                .map_or(default, engine_number)
        };

        let mut finite = true;
        for key in [
            "start_deg",
            "radius_inner",
            "radius_outer",
            "gap_deg",
            "deadzone",
            "hysteresis_deg",
            "size",
            "icon_size",
        ] {
            if !number(key, 0.0).is_finite() {
                finite = false;
                reject(
                    format!("/{key}"),
                    format!("the `UiRadial` {key} must be a finite number"),
                );
            }
        }
        for key in [
            "offset",
            "pointer",
            "color",
            "selected_color",
            "disabled_color",
        ] {
            let Some(array) = value.get(key).and_then(Value::as_array) else {
                continue;
            };
            for (i, element) in array.iter().enumerate() {
                if element
                    .as_f64()
                    .is_some_and(|n| !engine_number(n).is_finite())
                {
                    finite = false;
                    reject(
                        format!("/{key}/{i}"),
                        format!("the `UiRadial` {key} must have finite channels"),
                    );
                }
            }
        }
        if !finite {
            return out;
        }

        let inner = number("radius_inner", 90.0);
        let outer = number("radius_outer", 220.0);
        if inner < 0.0 {
            reject(
                "/radius_inner".into(),
                "the `UiRadial` radius_inner must be at least 0".into(),
            );
        }
        if outer <= inner {
            reject(
                "/radius_outer".into(),
                "the `UiRadial` radius_outer must be above radius_inner".into(),
            );
        }
        let deadzone = number("deadzone", 0.45);
        if !(0.0..1.0).contains(&deadzone) {
            reject(
                "/deadzone".into(),
                "the `UiRadial` deadzone must be within [0, 1)".into(),
            );
        }
        for (key, default) in [("size", 24.0), ("icon_size", 48.0)] {
            if !ludion_core::number::positive_f32(number(key, default)) {
                reject(
                    format!("/{key}"),
                    format!("the `UiRadial` {key} must be positive and finite"),
                );
            }
        }
        let items = value
            .get("items")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        if items > MAX_RADIAL_ITEMS {
            reject(
                "/items".into(),
                format!("a `UiRadial` holds at most {MAX_RADIAL_ITEMS} items, found {items}"),
            );
        }
        if items > 0 {
            let span = 360.0 / items as f64;
            for (key, default) in [("gap_deg", 2.0), ("hysteresis_deg", 6.0)] {
                if !(0.0..span).contains(&number(key, default)) {
                    reject(
                        format!("/{key}"),
                        format!(
                            "the `UiRadial` {key} must be within [0, {span}) for {items} items"
                        ),
                    );
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wheel(n: usize) -> UiRadial {
        UiRadial {
            items: (0..n)
                .map(|i| UiRadialItem {
                    text: format!("i{i}"),
                    ..UiRadialItem::default()
                })
                .collect(),
            ..UiRadial::default()
        }
    }

    fn aim(wheel: &mut UiRadial, x: f32, y: f32) -> Option<RadialEvent> {
        wheel.navigate(UiRadialNav::from_stick([x, y]))
    }

    /// An up-positive stick at clock angle `deg`, full tilt.
    fn at(deg: f32) -> [f32; 2] {
        let r = deg / DEGREES_PER_RADIAN;
        [math::sin(r), math::cos(r)]
    }

    #[test]
    fn an_exact_boundary_goes_clockwise() {
        for (n, start) in [(4usize, 0.0f32), (4, 30.0), (6, 0.0), (6, 30.0)] {
            let span = 360.0 / n as f32;
            for i in 0..n {
                // The boundary between item i and i + 1.
                let boundary = start + i as f32 * span + span * 0.5;
                assert_eq!(
                    sector_of(boundary.rem_euclid(360.0), start, n),
                    (i + 1) % n,
                    "n={n} start={start} i={i}"
                );
                assert_eq!(sector_of(boundary - 0.01, start, n), i);
            }
        }
    }

    #[test]
    fn a_stick_selects_by_clock_angle() {
        let mut w = wheel(4);
        aim(&mut w, 0.0, 1.0);
        assert_eq!(w.selected, Some(0));
        aim(&mut w, 1.0, 0.0);
        assert_eq!(w.selected, Some(1));
        aim(&mut w, 0.0, -1.0);
        assert_eq!(w.selected, Some(2));
        aim(&mut w, -1.0, 0.0);
        assert_eq!(w.selected, Some(3));
    }

    #[test]
    fn hysteresis_holds_at_a_boundary_and_releases_past_it() {
        let mut w = wheel(4);
        aim(&mut w, 0.0, 1.0);
        assert_eq!(w.selected, Some(0));
        // The 0/1 boundary is at 45 degrees; hysteresis 6 keeps item 0 up to 48.
        for deg in [43.0, 47.0, 45.0, 47.5] {
            let [x, y] = at(deg);
            aim(&mut w, x, y);
            assert_eq!(w.selected, Some(0), "at {deg}");
        }
        let [x, y] = at(49.0);
        aim(&mut w, x, y);
        assert_eq!(w.selected, Some(1));
        // And back: item 1 holds down to 42 degrees.
        let [x, y] = at(43.0);
        aim(&mut w, x, y);
        assert_eq!(w.selected, Some(1));
        let [x, y] = at(41.0);
        aim(&mut w, x, y);
        assert_eq!(w.selected, Some(0));
    }

    #[test]
    fn the_deadzone_holds() {
        let mut w = wheel(4);
        aim(&mut w, 0.0, 1.0);
        aim(&mut w, 0.4, 0.0);
        assert_eq!(w.selected, Some(0));
        aim(&mut w, 0.46, 0.0);
        assert_eq!(w.selected, Some(1));
    }

    #[test]
    fn a_disabled_sector_changes_nothing() {
        let mut w = wheel(4);
        w.items[1].enabled = false;
        aim(&mut w, 0.0, 1.0);
        aim(&mut w, 1.0, 0.0);
        assert_eq!(w.selected, Some(0));
        let mut fresh = wheel(4);
        fresh.items[1].enabled = false;
        aim(&mut fresh, 1.0, 0.0);
        assert_eq!(fresh.selected, None);
    }

    #[test]
    fn step_wraps_and_skips_disabled_from_none_too() {
        let mut w = wheel(4);
        w.items[0].enabled = false;
        w.items[2].enabled = false;
        let step = |w: &mut UiRadial, n: i32| {
            w.navigate(UiRadialNav {
                step: n,
                ..UiRadialNav::default()
            })
        };
        step(&mut w, 1);
        assert_eq!(w.selected, Some(1), "from none: the first enabled item");
        step(&mut w, 1);
        assert_eq!(w.selected, Some(3));
        step(&mut w, 1);
        assert_eq!(w.selected, Some(1), "wraps");
        step(&mut w, -1);
        assert_eq!(w.selected, Some(3), "wraps backwards");
        step(&mut w, 2);
        assert_eq!(w.selected, Some(3), "two enabled items, two steps is a lap");
        let mut none = wheel(3);
        none.items.iter_mut().for_each(|i| i.enabled = false);
        step(&mut none, 1);
        assert_eq!(none.selected, None);
    }

    #[test]
    fn delta_accumulates_clamps_and_resets() {
        let mut w = wheel(4);
        w.navigate(UiRadialNav::from_delta([60.0, 0.0]));
        assert_eq!(w.pointer, [60.0, 0.0]);
        assert_eq!(w.selected, None, "60 / 220 is inside the deadzone");
        w.navigate(UiRadialNav::from_delta([60.0, 0.0]));
        assert_eq!(w.pointer, [120.0, 0.0]);
        assert_eq!(w.selected, Some(1), "pointing right");
        w.navigate(UiRadialNav::from_delta([500.0, 0.0]));
        assert!(
            (w.pointer[0] - 220.0).abs() < 1e-3 && w.pointer[1] == 0.0,
            "{:?}",
            w.pointer
        );
        w.navigate(UiRadialNav {
            activate: true,
            ..UiRadialNav::default()
        });
        assert_eq!(w.pointer, [0.0, 0.0]);
        assert_eq!(w.selected, Some(1), "selected is kept");
        // A delta down (+y) points at six o'clock.
        w.navigate(UiRadialNav::from_delta([0.0, 200.0]));
        assert_eq!(w.selected, Some(2));
        w.navigate(UiRadialNav {
            cancel: true,
            ..UiRadialNav::default()
        });
        assert_eq!(w.pointer, [0.0, 0.0]);
    }

    #[test]
    fn a_cursor_inside_the_dead_radius_holds() {
        let mut w = wheel(4);
        w.navigate(UiRadialNav::from_cursor([0.0, 100.0], 30.0));
        assert_eq!(w.selected, Some(2), "+y down is six o'clock");
        w.navigate(UiRadialNav::from_cursor([100.0, 0.0], 150.0));
        assert_eq!(w.selected, Some(2));
        w.navigate(UiRadialNav::from_cursor([100.0, 0.0], 30.0));
        assert_eq!(w.selected, Some(1));
    }

    #[test]
    fn cancel_beats_activate_and_activation_needs_an_enabled_selection() {
        let mut w = wheel(4);
        let both = UiRadialNav {
            activate: true,
            cancel: true,
            ..UiRadialNav::default()
        };
        aim(&mut w, 0.0, 1.0);
        assert_eq!(w.navigate(both), Some(RadialEvent::Cancelled));
        let activate = UiRadialNav {
            activate: true,
            ..UiRadialNav::default()
        };
        assert_eq!(w.navigate(activate), Some(RadialEvent::Activated(0)));
        w.items[0].enabled = false;
        assert_eq!(w.navigate(activate), None);
        let mut idle = wheel(4);
        assert_eq!(idle.navigate(activate), None);
    }

    #[test]
    fn an_empty_wheel_does_nothing() {
        let mut w = UiRadial::default();
        let before = w.clone();
        let nav = UiRadialNav {
            aim: [1.0, 0.0],
            step: 1,
            activate: true,
            ..UiRadialNav::default()
        };
        assert_eq!(w.navigate(nav), None);
        assert_eq!(w, before);
    }

    #[test]
    fn an_out_of_range_selection_acts_as_none() {
        let mut w = wheel(4);
        w.selected = Some(9);
        let activate = UiRadialNav {
            activate: true,
            ..UiRadialNav::default()
        };
        assert_eq!(w.navigate(activate), None);
        assert_eq!(w.selected, Some(9), "an idle call writes nothing");
        w.navigate(UiRadialNav {
            step: 1,
            ..UiRadialNav::default()
        });
        assert_eq!(w.selected, Some(0));
    }

    #[test]
    fn an_idle_navigate_writes_nothing() {
        let mut w = wheel(4);
        aim(&mut w, 0.0, 1.0);
        w.pointer = [1.5, -2.5];
        let before = serde_json::to_vec(&w).unwrap();
        for nav in [
            UiRadialNav::default(),
            UiRadialNav::from_stick([0.1, 0.1]),
            UiRadialNav::from_stick([0.0, 1.0]),
            UiRadialNav::from_cursor([2.0, 2.0], 30.0),
        ] {
            assert_eq!(w.navigate(nav), None);
            assert_eq!(serde_json::to_vec(&w).unwrap(), before);
        }
    }

    #[test]
    fn serde_round_trip_skips_defaults() {
        let value = serde_json::to_value(wheel(2)).unwrap();
        for absent in ["group", "selected", "font", "outline"] {
            assert!(value.get(absent).is_none(), "{absent}");
        }
        assert!(value["items"][0].get("enabled").is_none());
        assert!(value["items"][0].get("detail").is_none());
        let full = UiRadial {
            group: Some(EntityId::new(3, 0)),
            selected: Some(1),
            font: Some("title".into()),
            outline: Some(TextOutline {
                width: 2.0,
                color: [0.0, 0.0, 0.0, 1.0],
            }),
            items: vec![UiRadialItem {
                text: "AXE".into(),
                detail: Some("x3".into()),
                atlas: Some("icons.atlas.yaml".into()),
                frame: Some("axe".into()),
                enabled: false,
            }],
            ..UiRadial::default()
        };
        let json = serde_json::to_value(&full).unwrap();
        assert_eq!(serde_json::from_value::<UiRadial>(json).unwrap(), full);
        let empty: UiRadial = serde_json::from_value(json!({})).unwrap();
        assert_eq!(empty, UiRadial::default());
    }

    fn pointers(value: Value) -> Vec<String> {
        UiRadial::validate_semantics(&value)
            .into_iter()
            .inspect(|d| assert_eq!(d.code.as_str(), "L1619"))
            .map(|d| d.path.unwrap())
            .collect()
    }

    fn items(n: usize) -> Value {
        Value::Array((0..n).map(|_| json!({})).collect())
    }

    #[test]
    fn each_l1619_case_names_its_pointer() {
        assert!(pointers(json!({ "items": items(4) })).is_empty());
        assert!(pointers(json!({})).is_empty());
        let cases = [
            (json!({ "radius_inner": -1.0 }), "/radius_inner"),
            (json!({ "radius_outer": 90.0 }), "/radius_outer"),
            (json!({ "radius_inner": 300.0 }), "/radius_outer"),
            (json!({ "gap_deg": -1.0, "items": items(4) }), "/gap_deg"),
            (json!({ "gap_deg": 90.0, "items": items(4) }), "/gap_deg"),
            (
                json!({ "hysteresis_deg": 90.0, "items": items(4) }),
                "/hysteresis_deg",
            ),
            (json!({ "deadzone": 1.0 }), "/deadzone"),
            (json!({ "deadzone": -0.1 }), "/deadzone"),
            (json!({ "size": 0.0 }), "/size"),
            (json!({ "icon_size": -4.0 }), "/icon_size"),
            (json!({ "items": items(17) }), "/items"),
            (json!({ "start_deg": 1e300 }), "/start_deg"),
            (json!({ "offset": [0.0, 1e300] }), "/offset/1"),
            (json!({ "color": [1.0, 1.0, 1e300, 1.0] }), "/color/2"),
        ];
        for (value, pointer) in cases {
            let got = pointers(value.clone());
            assert!(got.contains(&pointer.to_string()), "{value}: {got:?}");
        }
        // The span is only computed with items: a wide gap on an empty wheel is fine.
        assert!(pointers(json!({ "gap_deg": 90.0 })).is_empty());
        // One item is allowed, with a gap below 360.
        assert!(
            pointers(json!({ "gap_deg": 10.0, "hysteresis_deg": 10.0, "items": items(1) }))
                .is_empty()
        );
    }
}
