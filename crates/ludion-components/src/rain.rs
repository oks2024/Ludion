//! The [`Rain`] component: a world-anchored field of velocity-stretched streaks around the camera.
//! Render-only; `ludion docs weather` is the guide.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::number::engine_number;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The widest vertical band the drops fill, in metres.
pub const MAX_RAIN_HEIGHT_SPAN: f32 = 64.0;

/// Falling rain drawn as streaks around the camera. A singleton: the lowest entity id wins. The
/// drops live on a world-space lattice, so nothing swims when the camera moves, and they move with
/// game time. `intensity: 0` draws nothing and leaves the frame identical to a scene without it.
///
/// ```
/// use ludion_components::Rain;
///
/// let rain: Rain = serde_json::from_value(serde_json::json!({
///     "intensity": 0.5,
///     "wind": [1.2, 0.0, 0.4],
/// }))
/// .unwrap();
/// assert_eq!(rain.density, 1.5);
/// assert_eq!(rain.fall_speed, [7.0, 9.0]);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq)]
pub struct Rain {
    /// Scales the drop count and fades the rain in and out, in `[0, 1]` (default 1).
    #[serde(default = "default_intensity")]
    pub intensity: f32,
    /// Streaks per cubic metre at intensity 1, in `(0, 16]` (default 1.5).
    #[serde(default = "default_density")]
    pub density: f32,
    /// Horizontal reach around the camera in metres, in `[2, 64]` (default 16).
    #[serde(default = "default_radius")]
    pub radius: f32,
    /// The vertical band `[low, high]` relative to the camera, low < high, span at most 64 (default `[-6, 14]`).
    #[serde(default = "default_height")]
    pub height: [f32; 2],
    /// The lattice cube size in metres, in `[1, 8]` (default 4).
    #[serde(default = "default_cell")]
    pub cell: f32,
    /// Fall speed range `[min, max]` in m/s, `0 < min <= max <= 40` (default `[7, 9]`).
    #[serde(default = "default_fall_speed")]
    pub fall_speed: [f32; 2],
    /// Horizontal wind in m/s `[x, 0, z]`; `y` must be 0, `|x|` and `|z|` at most 20 (default none).
    #[serde(default)]
    pub wind: [f32; 3],
    /// Per-drop wind variation, in `[0, 1]` (default 0.15).
    #[serde(default = "default_gust")]
    pub gust: f32,
    /// Streak width in metres, in `(0, 0.05]`; widened to one output pixel when thinner (default 0.006).
    #[serde(default = "default_drop_width")]
    pub drop_width: f32,
    /// Streak length scale, in `[0, 8]`; length is `speed * streak / 60` metres (default 1).
    #[serde(default = "default_streak")]
    pub streak: f32,
    /// Display-referred colour `[r, g, b, a]`, each in `[0, 1]` (default `[0.55, 0.6, 0.65, 0.25]`).
    #[serde(default = "default_color")]
    pub color: [f32; 4],
    /// Light each streak from the scene's lights, unshadowed (default true).
    #[serde(default = "default_true")]
    pub lit: bool,
    /// How much fog and the transparent layer's medium reach the streaks, in `[0, 1]` (default 1).
    #[serde(default = "default_medium")]
    pub medium: f32,
    /// Hide streaks under roofs; read by the occlusion pass (default true).
    #[serde(default = "default_true")]
    pub occlusion: bool,
    /// How wet exposed surfaces look, in `[0, 1]`: 0 leaves every surface dry, 1 darkens and glosses what the
    /// rain reaches (default 1). Games write it to dry a street.
    #[serde(default = "default_wetness")]
    pub wetness: f32,
    /// Varies the lattice hash (default 0).
    #[serde(default)]
    pub seed: u32,
}

impl Default for Rain {
    fn default() -> Self {
        Self {
            intensity: default_intensity(),
            density: default_density(),
            radius: default_radius(),
            height: default_height(),
            cell: default_cell(),
            fall_speed: default_fall_speed(),
            wind: [0.0; 3],
            gust: default_gust(),
            drop_width: default_drop_width(),
            streak: default_streak(),
            color: default_color(),
            lit: true,
            medium: default_medium(),
            occlusion: true,
            wetness: default_wetness(),
            seed: 0,
        }
    }
}

fn default_intensity() -> f32 {
    1.0
}
fn default_density() -> f32 {
    1.5
}
fn default_radius() -> f32 {
    16.0
}
fn default_height() -> [f32; 2] {
    [-6.0, 14.0]
}
fn default_cell() -> f32 {
    4.0
}
fn default_fall_speed() -> [f32; 2] {
    [7.0, 9.0]
}
fn default_gust() -> f32 {
    0.15
}
fn default_drop_width() -> f32 {
    0.006
}
fn default_streak() -> f32 {
    1.0
}
fn default_color() -> [f32; 4] {
    [0.55, 0.6, 0.65, 0.25]
}
fn default_wetness() -> f32 {
    1.0
}
fn default_true() -> bool {
    true
}
fn default_medium() -> f32 {
    1.0
}

/// Every out-of-range field of `rain` as `(JSON Pointer, what is wrong)`; empty when valid.
/// Shared by the component validator and the engine's extract.
pub fn rain_problems(rain: &Rain) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut check = |ok: bool, path: String, what: String| {
        if !ok {
            out.push((path, what));
        }
    };
    let unit = |v: f32| v.is_finite() && (0.0..=1.0).contains(&v);
    check(
        unit(rain.intensity),
        "/intensity".into(),
        format!(
            "`intensity` is {}; it takes a value in [0, 1]",
            rain.intensity
        ),
    );
    check(
        rain.density.is_finite() && rain.density > 0.0 && rain.density <= 16.0,
        "/density".into(),
        format!("`density` is {}; it takes a value in (0, 16]", rain.density),
    );
    check(
        rain.radius.is_finite() && (2.0..=64.0).contains(&rain.radius),
        "/radius".into(),
        format!("`radius` is {}; it takes a value in [2, 64]", rain.radius),
    );
    let [h0, h1] = rain.height;
    check(
        h0.is_finite() && h1.is_finite() && h0 < h1 && h1 - h0 <= MAX_RAIN_HEIGHT_SPAN,
        "/height".into(),
        format!(
            "`height` is [{h0}, {h1}]; it needs low < high and a span of at most {MAX_RAIN_HEIGHT_SPAN}"
        ),
    );
    check(
        rain.cell.is_finite() && (1.0..=8.0).contains(&rain.cell),
        "/cell".into(),
        format!("`cell` is {}; it takes a value in [1, 8]", rain.cell),
    );
    let [a, b] = rain.fall_speed;
    check(
        a.is_finite() && b.is_finite() && a > 0.0 && a <= b && b <= 40.0,
        "/fall_speed".into(),
        format!("`fall_speed` is [{a}, {b}]; it needs 0 < min <= max <= 40"),
    );
    let [wx, wy, wz] = rain.wind;
    check(
        wx.is_finite() && wx.abs() <= 20.0,
        "/wind/0".into(),
        format!("`wind.x` is {wx}; it takes a value in [-20, 20]"),
    );
    check(
        wy == 0.0,
        "/wind/1".into(),
        format!("`wind.y` is {wy}; it must be 0 (rain falls straight down)"),
    );
    check(
        wz.is_finite() && wz.abs() <= 20.0,
        "/wind/2".into(),
        format!("`wind.z` is {wz}; it takes a value in [-20, 20]"),
    );
    check(
        unit(rain.gust),
        "/gust".into(),
        format!("`gust` is {}; it takes a value in [0, 1]", rain.gust),
    );
    check(
        rain.drop_width.is_finite() && rain.drop_width > 0.0 && rain.drop_width <= 0.05,
        "/drop_width".into(),
        format!(
            "`drop_width` is {}; it takes a value in (0, 0.05]",
            rain.drop_width
        ),
    );
    check(
        rain.streak.is_finite() && (0.0..=8.0).contains(&rain.streak),
        "/streak".into(),
        format!("`streak` is {}; it takes a value in [0, 8]", rain.streak),
    );
    for (i, c) in rain.color.iter().enumerate() {
        check(
            unit(*c),
            format!("/color/{i}"),
            format!("colour channel {i} is {c}; it takes a value in [0, 1]"),
        );
    }
    check(
        unit(rain.medium),
        "/medium".into(),
        format!("`medium` is {}; it takes a value in [0, 1]", rain.medium),
    );
    check(
        unit(rain.wetness),
        "/wetness".into(),
        format!("`wetness` is {}; it takes a value in [0, 1]", rain.wetness),
    );
    out
}

impl SemanticValidate for Rain {
    /// Rejects what a schema cannot (`L2380`, pointer into the field): a non-finite or out-of-range
    /// value. A wrong *shape* is the schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        // Overlay the authored numbers on the defaults so each field is checked on its own.
        let Ok(rain) = serde_json::from_value::<Rain>(sanitize(value)) else {
            return Vec::new();
        };
        rain_problems(&rain)
            .into_iter()
            .map(|(path, what)| {
                Diagnostic::new(code::RAIN_INVALID, format!("`Rain` {what}"))
                    .with_path(path)
                    .with_help("see `ludion docs weather`; `ludion explain L2380`")
            })
            .collect()
    }
}

/// The authored value with each float rounded the way the engine reads it.
fn sanitize(value: &Value) -> Value {
    match value {
        Value::Number(n) if n.is_f64() => Value::from(engine_number(n.as_f64().unwrap_or(0.0))),
        Value::Array(a) => Value::Array(a.iter().map(sanitize).collect()),
        Value::Object(o) => {
            Value::Object(o.iter().map(|(k, v)| (k.clone(), sanitize(v))).collect())
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn paths(value: Value) -> Vec<String> {
        Rain::validate_semantics(&value)
            .into_iter()
            .map(|d| d.path.unwrap())
            .collect()
    }

    #[test]
    fn empty_object_takes_the_defaults_and_is_clean() {
        let r: Rain = serde_json::from_value(json!({})).unwrap();
        assert_eq!(r, Rain::default());
        assert_eq!(r.height, [-6.0, 14.0]);
        assert!(paths(json!({})).is_empty());
    }

    #[test]
    fn a_full_rain_round_trips() {
        let r = Rain {
            intensity: 0.5,
            wind: [1.25, 0.0, -0.5],
            seed: 7,
            lit: false,
            ..Rain::default()
        };
        let back: Rain = serde_json::from_value(serde_json::to_value(r).unwrap()).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn each_bad_field_names_its_pointer() {
        assert_eq!(paths(json!({ "intensity": 1.5 })), ["/intensity"]);
        assert_eq!(paths(json!({ "density": 0.0 })), ["/density"]);
        assert_eq!(paths(json!({ "density": 17.0 })), ["/density"]);
        assert_eq!(paths(json!({ "radius": 1.0 })), ["/radius"]);
        assert_eq!(paths(json!({ "height": [4.0, 4.0] })), ["/height"]);
        assert_eq!(paths(json!({ "height": [-40.0, 40.0] })), ["/height"]);
        assert_eq!(paths(json!({ "cell": 0.5 })), ["/cell"]);
        assert_eq!(paths(json!({ "fall_speed": [9.0, 7.0] })), ["/fall_speed"]);
        assert_eq!(paths(json!({ "fall_speed": [0.0, 7.0] })), ["/fall_speed"]);
        assert_eq!(paths(json!({ "wind": [0.0, 1.0, 0.0] })), ["/wind/1"]);
        assert_eq!(
            paths(json!({ "wind": [25.0, 0.0, -25.0] })),
            ["/wind/0", "/wind/2"]
        );
        assert_eq!(paths(json!({ "gust": -0.1 })), ["/gust"]);
        assert_eq!(paths(json!({ "drop_width": 0.1 })), ["/drop_width"]);
        assert_eq!(paths(json!({ "streak": 9.0 })), ["/streak"]);
        assert_eq!(
            paths(json!({ "color": [0.0, 2.0, 0.0, 0.5] })),
            ["/color/1"]
        );
        assert_eq!(paths(json!({ "medium": 2.0 })), ["/medium"]);
        assert_eq!(paths(json!({ "wetness": 1.5 })), ["/wetness"]);
        assert_eq!(paths(json!({ "wetness": -0.1 })), ["/wetness"]);
        assert!(paths(json!({ "wetness": 0.0 })).is_empty());
        assert!(paths(json!({ "height": [-6.0, 58.0], "cell": 8.0 })).is_empty());
    }
}
