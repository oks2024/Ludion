//! The [`UiShape`] component: an analytic screen-space shape (line, circle, ring, arc, wedge, rounded
//! rectangle) with a fill and a stroke. `ludion docs ui-shapes` is the guide.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::number::engine_number;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Anchor;

/// How a [`UiShapeType::Line`] ends.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UiLineCap {
    /// Flush with the end points (the default).
    #[default]
    Butt,
    /// A half disc past each end point, as wide as the stroke.
    Round,
}

impl UiLineCap {
    fn is_butt(&self) -> bool {
        *self == UiLineCap::Butt
    }
}

/// The geometry of a [`UiShape`]. Every length is in canvas units; angles are clock degrees (0 is
/// 12 o'clock, positive is clockwise). Everything is positioned on the shape's origin.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiShapeType {
    /// A segment from `from` to `to`, drawn as its stroke only.
    Line {
        /// The first end point, relative to the origin, +Y down.
        from: [f32; 2],
        /// The second end point, relative to the origin, +Y down.
        to: [f32; 2],
        /// How the ends are drawn (default butt).
        #[serde(default, skip_serializing_if = "UiLineCap::is_butt")]
        cap: UiLineCap,
    },
    /// A disc centred on the origin.
    Circle {
        /// The radius, above 0.
        radius: f32,
    },
    /// The band `[radius - thickness, radius]` over a sweep.
    Ring {
        /// The outer radius, above 0.
        radius: f32,
        /// The band width, in `(0, radius]`.
        thickness: f32,
        /// Clock degrees of the sweep's start (default `0`).
        #[serde(default, skip_serializing_if = "is_zero")]
        start_deg: f32,
        /// Degrees the sweep covers, in `[0, 360]` (default `360`).
        #[serde(default = "full_sweep", skip_serializing_if = "is_full_sweep")]
        sweep_deg: f32,
    },
    /// An arc of a circle drawn as its stroke only: the band `radius ± width / 2`.
    Arc {
        /// The radius of the arc's centre line, above 0.
        radius: f32,
        /// Clock degrees of the sweep's start (default `0`).
        #[serde(default, skip_serializing_if = "is_zero")]
        start_deg: f32,
        /// Degrees the sweep covers, in `[0, 360]` (default `360`).
        #[serde(default = "full_sweep", skip_serializing_if = "is_full_sweep")]
        sweep_deg: f32,
    },
    /// A pie slice, or a band slice with an `inner_radius`.
    Wedge {
        /// The outer radius, above 0.
        radius: f32,
        /// The hole radius, in `[0, radius)` (default `0`: a slice to the centre).
        #[serde(default, skip_serializing_if = "is_zero")]
        inner_radius: f32,
        /// Clock degrees of the sweep's start (default `0`).
        #[serde(default, skip_serializing_if = "is_zero")]
        start_deg: f32,
        /// Degrees the sweep covers, in `[0, 360]` (default `360`).
        #[serde(default = "full_sweep", skip_serializing_if = "is_full_sweep")]
        sweep_deg: f32,
    },
    /// A rectangle with rounded corners, centred on the origin.
    RoundedRect {
        /// Width and height, non-negative.
        size: [f32; 2],
        /// The corner radius, non-negative (default `0`; clamped to half the shorter side when drawn).
        #[serde(default, skip_serializing_if = "is_zero")]
        corner_radius: f32,
    },
}

impl Default for UiShapeType {
    /// A circle of radius 16.
    fn default() -> Self {
        UiShapeType::Circle { radius: 16.0 }
    }
}

fn full_sweep() -> f32 {
    360.0
}

fn is_full_sweep(value: &f32) -> bool {
    *value == 360.0
}

fn is_zero(value: &f32) -> bool {
    value.to_bits() == 0.0f32.to_bits()
}

/// An outline centred on a [`UiShape`]'s edge.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct UiStroke {
    /// The width in canvas units, above 0.
    pub width: f32,
    /// Linear RGBA.
    pub color: [f32; 4],
}

/// An analytic screen-space shape: one SDF-drawn primitive with a fill and a stroke. Screen space, like
/// [`UiRect`](crate::UiRect): the 1080-unit canvas, `anchor` + `offset`, `group`, `UiTween` and
/// `UiWorldAnchor`.
///
/// The shape's local `(0, 0)` sits at the anchor point plus `offset`; scale and `rotation` pivot there,
/// not on a bounds centre. `line` and `arc` draw only their `stroke`; the other kinds draw `fill`,
/// `stroke` or both. A shape with neither draws nothing.
///
/// ```
/// use ludion_components::{UiShape, UiShapeType};
///
/// let shape: UiShape = serde_json::from_value(serde_json::json!({
///     "shape": { "type": "ring", "radius": 40, "thickness": 6, "sweep_deg": 270 },
///     "fill": [1, 1, 1, 1]
/// })).unwrap();
/// assert!(matches!(shape.shape, UiShapeType::Ring { sweep_deg, .. } if sweep_deg == 270.0));
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct UiShape {
    /// Which canvas point the shape's origin is placed at (default [`Anchor::TopLeft`]).
    #[serde(default)]
    pub anchor: Anchor,
    /// Offset from the anchor point, in canvas units, **+Y down** (default `[0, 0]`).
    #[serde(default)]
    pub offset: [f32; 2],
    /// The [`UiGroup`](crate::UiGroup) the shape anchors inside, instead of the canvas (default none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<ludion_ecs::EntityRefSchema>")]
    pub group: Option<EntityId>,
    /// The geometry (required; the default is a circle of radius 16).
    #[serde(default)]
    pub shape: UiShapeType,
    /// Linear RGBA of the interior (default none). Not allowed on `line` or `arc`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<[f32; 4]>,
    /// The edge outline (default none). Required on `line` and `arc`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<UiStroke>,
    /// Degrees clockwise on screen about the origin (default `0`; non-finite is `L1620`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f32,
    /// Draw order: higher draws later (default `0`).
    #[serde(default)]
    pub order: i32,
    /// Whether the shape draws at all (default `true`).
    #[serde(default = "crate::yes")]
    pub visible: bool,
}

impl Default for UiShape {
    fn default() -> Self {
        UiShape {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            group: None,
            shape: UiShapeType::default(),
            fill: None,
            stroke: None,
            rotation: 0.0,
            order: 0,
            visible: true,
        }
    }
}

impl SemanticValidate for UiShape {
    /// Rejects (`L1621`) a non-finite number; a `radius` not above 0; a ring `thickness` outside
    /// `(0, radius]`; a wedge `inner_radius` outside `[0, radius)`; a `sweep_deg` outside `[0, 360]`; a
    /// negative `size` or `corner_radius`; a stroke `width` not above 0; and a `fill` on, or no `stroke`
    /// for, a `line` or `arc`. A non-finite `rotation` is `L1620`.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        crate::ui::validate_rotation(value, &mut out);
        let mut reject = |pointer: String, what: String| {
            out.push(
                Diagnostic::new(code::UI_SHAPE_INVALID, what)
                    .with_path(pointer)
                    .with_help("fix the field; `ludion explain L1621`"),
            );
        };

        let mut finite = true;
        let mut check_finite =
            |pointer: &str, what: &str, v: &Value, reject: &mut dyn FnMut(String, String)| {
                let bad = |n: &Value| n.as_f64().is_some_and(|n| !engine_number(n).is_finite());
                let any_bad = match v.as_array() {
                    Some(array) => array
                        .iter()
                        .enumerate()
                        .find(|(_, n)| bad(n))
                        .map(|(i, _)| i),
                    None => bad(v).then_some(0),
                };
                if let Some(i) = any_bad {
                    finite = false;
                    let at = if v.is_array() {
                        format!("{pointer}/{i}")
                    } else {
                        pointer.to_string()
                    };
                    reject(at, format!("the `UiShape` {what} must be finite"));
                }
            };

        for key in ["offset", "fill"] {
            if let Some(v) = value.get(key) {
                check_finite(&format!("/{key}"), key, v, &mut reject);
            }
        }
        if let Some(stroke) = value.get("stroke") {
            for key in ["width", "color"] {
                if let Some(v) = stroke.get(key) {
                    check_finite(
                        &format!("/stroke/{key}"),
                        &format!("stroke {key}"),
                        v,
                        &mut reject,
                    );
                }
            }
        }
        let shape = value.get("shape");
        if let Some(shape) = shape {
            for key in [
                "from",
                "to",
                "radius",
                "thickness",
                "inner_radius",
                "start_deg",
                "sweep_deg",
                "size",
                "corner_radius",
            ] {
                if let Some(v) = shape.get(key) {
                    check_finite(&format!("/shape/{key}"), key, v, &mut reject);
                }
            }
        }
        if !finite {
            return out;
        }

        let number = |holder: Option<&Value>, key: &str, default: f64| -> f64 {
            holder
                .and_then(|h| h.get(key))
                .and_then(Value::as_f64)
                .map_or(default, engine_number)
        };
        let kind = shape
            .and_then(|s| s.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("circle");
        let radius = number(shape, "radius", 16.0);
        if matches!(kind, "circle" | "ring" | "arc" | "wedge") && radius <= 0.0 {
            reject(
                "/shape/radius".into(),
                "the `UiShape` radius must be above 0".into(),
            );
        }
        if kind == "ring" {
            let thickness = number(shape, "thickness", 0.0);
            if thickness <= 0.0 || thickness > radius {
                reject(
                    "/shape/thickness".into(),
                    format!(
                        "the ring `thickness` ({thickness}) must be above 0 and at most its radius ({radius})"
                    ),
                );
            }
        }
        if kind == "wedge" {
            let inner = number(shape, "inner_radius", 0.0);
            if inner < 0.0 || inner >= radius {
                reject(
                    "/shape/inner_radius".into(),
                    format!(
                        "the wedge `inner_radius` ({inner}) must be in [0, radius) with radius {radius}"
                    ),
                );
            }
        }
        if matches!(kind, "ring" | "arc" | "wedge") {
            let sweep = number(shape, "sweep_deg", 360.0);
            if !(0.0..=360.0).contains(&sweep) {
                reject(
                    "/shape/sweep_deg".into(),
                    format!("`sweep_deg` ({sweep}) must be in [0, 360]"),
                );
            }
        }
        if kind == "rounded_rect" {
            if let Some(size) = shape.and_then(|s| s.get("size")).and_then(Value::as_array) {
                for (i, n) in size.iter().enumerate() {
                    if n.as_f64().is_some_and(|n| engine_number(n) < 0.0) {
                        reject(
                            format!("/shape/size/{i}"),
                            "the rounded_rect `size` must not be negative".into(),
                        );
                    }
                }
            }
            if number(shape, "corner_radius", 0.0) < 0.0 {
                reject(
                    "/shape/corner_radius".into(),
                    "the rounded_rect `corner_radius` must not be negative".into(),
                );
            }
        }
        let stroke = value.get("stroke").filter(|s| !s.is_null());
        if let Some(stroke) = stroke
            && let Some(width) = stroke
                .get("width")
                .and_then(Value::as_f64)
                .map(engine_number)
            && width <= 0.0
        {
            reject(
                "/stroke/width".into(),
                format!("the stroke `width` ({width}) must be above 0"),
            );
        }
        if matches!(kind, "line" | "arc") {
            if value.get("fill").is_some_and(|f| !f.is_null()) {
                reject(
                    "/fill".into(),
                    format!("a `{kind}` draws only its stroke; remove `fill`"),
                );
            }
            if stroke.is_none() {
                reject(
                    "/stroke".into(),
                    format!("a `{kind}` draws only its stroke; give it a `stroke`"),
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

    fn codes_at(value: Value) -> Vec<(String, String)> {
        UiShape::validate_semantics(&value)
            .into_iter()
            .map(|d| (d.code.as_str().to_string(), d.path.unwrap_or_default()))
            .collect()
    }

    #[test]
    fn every_kind_round_trips() {
        for shape in [
            json!({ "type": "line", "from": [0.0, 0.0], "to": [10.0, 4.0], "cap": "round" }),
            json!({ "type": "circle", "radius": 8.0 }),
            json!({ "type": "ring", "radius": 20.0, "thickness": 4.0, "start_deg": 30.0, "sweep_deg": 90.0 }),
            json!({ "type": "arc", "radius": 20.0, "start_deg": 10.0 }),
            json!({ "type": "wedge", "radius": 20.0, "inner_radius": 5.0, "sweep_deg": 45.0 }),
            json!({ "type": "rounded_rect", "size": [40.0, 20.0], "corner_radius": 6.0 }),
        ] {
            let kind: UiShapeType = serde_json::from_value(shape.clone()).unwrap();
            assert_eq!(serde_json::to_value(kind).unwrap(), shape);
        }
    }

    #[test]
    fn the_default_round_trips() {
        let shape = UiShape::default();
        let value = serde_json::to_value(&shape).unwrap();
        assert_eq!(value["shape"], json!({ "type": "circle", "radius": 16.0 }));
        assert_eq!(serde_json::from_value::<UiShape>(value).unwrap(), shape);
    }

    #[test]
    fn a_valid_shape_has_no_diagnostics() {
        assert!(
            codes_at(json!({
                "shape": { "type": "arc", "radius": 20, "sweep_deg": 0 },
                "stroke": { "width": 2, "color": [1, 1, 1, 1] }
            }))
            .is_empty()
        );
    }

    #[test]
    fn each_bad_field_names_its_pointer() {
        let l1621 = |at: &str| ("L1621".to_string(), at.to_string());
        let stroke = json!({ "width": 2, "color": [1, 1, 1, 1] });
        for (value, at) in [
            (
                json!({ "shape": { "type": "circle", "radius": 0 }, "fill": [1, 1, 1, 1] }),
                "/shape/radius",
            ),
            (
                json!({ "shape": { "type": "ring", "radius": 10, "thickness": 11 } }),
                "/shape/thickness",
            ),
            (
                json!({ "shape": { "type": "ring", "radius": 10, "thickness": 0 } }),
                "/shape/thickness",
            ),
            (
                json!({ "shape": { "type": "wedge", "radius": 10, "inner_radius": 10 } }),
                "/shape/inner_radius",
            ),
            (
                json!({ "shape": { "type": "wedge", "radius": 10, "sweep_deg": 361 } }),
                "/shape/sweep_deg",
            ),
            (
                json!({ "shape": { "type": "rounded_rect", "size": [-1, 2] } }),
                "/shape/size/0",
            ),
            (
                json!({ "shape": { "type": "rounded_rect", "size": [1, 2], "corner_radius": -1 } }),
                "/shape/corner_radius",
            ),
            (
                json!({ "shape": { "type": "circle", "radius": 4 }, "stroke": { "width": 0, "color": [1, 1, 1, 1] } }),
                "/stroke/width",
            ),
            (
                json!({ "shape": { "type": "circle", "radius": 4 }, "fill": [1e300, 1, 1, 1] }),
                "/fill/0",
            ),
        ] {
            let got = codes_at(value.clone());
            assert!(got.contains(&l1621(at)), "{value}: {got:?}");
        }
        let _ = stroke;
    }

    #[test]
    fn a_fill_on_an_arc_or_a_strokeless_line_is_rejected() {
        let got = codes_at(json!({
            "shape": { "type": "arc", "radius": 10 },
            "fill": [1, 1, 1, 1],
            "stroke": { "width": 2, "color": [1, 1, 1, 1] }
        }));
        assert_eq!(got, vec![("L1621".to_string(), "/fill".to_string())]);
        let got = codes_at(json!({ "shape": { "type": "line", "from": [0, 0], "to": [4, 0] } }));
        assert_eq!(got, vec![("L1621".to_string(), "/stroke".to_string())]);
    }

    #[test]
    fn a_nan_rotation_is_l1620() {
        let got = codes_at(json!({
            "shape": { "type": "circle", "radius": 4 },
            "rotation": 1e300
        }));
        assert_eq!(got, vec![("L1620".to_string(), "/rotation".to_string())]);
    }
}
