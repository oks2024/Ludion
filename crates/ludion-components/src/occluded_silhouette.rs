//! The [`OccludedSilhouette`] component: a tint and outline for an entity's meshes where something
//! stands in front of them. Render-only; `ludion docs occluded-silhouette` is the guide.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::number::engine_number;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The widest outline, in 1080p pixels.
pub const MAX_OUTLINE_PX: f32 = 8.0;

/// Draws the entity's meshes in `color` wherever a nearer surface hides them, with an optional outline
/// around the hidden region. A mesh without its own takes its nearest ancestor's, like `Fade`. The tint
/// is blended after the tonemap, so it is not fogged, blurred or bloomed; vignette and grain still apply.
/// The fill can carry a screen-space line `pattern`, and an `inner_outline` draws a band just inside the
/// edge of the filled region. `film: false` fades the vignette and grain under the silhouette. At most 8 distinct styles are drawn per frame.
///
/// ```
/// use ludion_components::{OccludedSilhouette, SilhouettePatternKind, SilhouetteWhen};
///
/// let silhouette: OccludedSilhouette = serde_json::from_value(serde_json::json!({
///     "outline": { "px": 2.0 },
///     "pattern": { "kind": "scanlines" },
///     "inner_outline": {},
/// }))
/// .unwrap();
/// assert_eq!(silhouette.color, [1.0, 1.0, 1.0, 0.55]);
/// assert_eq!(silhouette.when, SilhouetteWhen::Hidden);
/// assert_eq!(silhouette.outline.unwrap().alpha, 1.0);
/// let pattern = silhouette.pattern.unwrap();
/// assert_eq!((pattern.kind, pattern.period), (SilhouettePatternKind::Scanlines, 4.0));
/// assert_eq!(silhouette.inner_outline.unwrap().px, 2.0);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq)]
pub struct OccludedSilhouette {
    /// Display-referred colour `[r, g, b, a]`, each in `[0, 1]` (default white at 0.55).
    #[serde(default = "default_color")]
    pub color: [f32; 4],
    /// An outer outline around the filled region; absent means none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline: Option<SilhouetteOutline>,
    /// `hidden` (default) tints only occluded pixels; `always` tints every pixel of the meshes.
    #[serde(default, skip_serializing_if = "SilhouetteWhen::is_hidden")]
    pub when: SilhouetteWhen,
    /// How far behind the occluder a pixel must be to count as hidden, in view-space units (default 0.05).
    #[serde(default = "default_bias")]
    pub bias: f32,
    /// A screen-space line pattern on the fill only; absent means a solid fill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<SilhouettePattern>,
    /// A band just inside the edge of the filled region; absent means none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inner_outline: Option<SilhouetteOutline>,
    /// `false` keeps the vignette and the grain off the silhouette, in proportion to its opacity at each pixel.
    #[serde(default = "default_film", skip_serializing_if = "is_true")]
    pub film: bool,
}

/// The widest pattern period, in 1080p pixels.
pub const MAX_PATTERN_PERIOD: f32 = 64.0;
/// The narrowest pattern period, in 1080p pixels.
pub const MIN_PATTERN_PERIOD: f32 = 2.0;

/// The line pattern of an [`OccludedSilhouette`] fill, anchored to the screen.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq)]
pub struct SilhouettePattern {
    /// Horizontal lines or 45° lines.
    pub kind: SilhouettePatternKind,
    /// Line spacing in pixels at 1080p, in `[2, 64]`; scales with the render height (default 4).
    #[serde(default = "default_period")]
    pub period: f32,
    /// The lit fraction of each period, in `(0, 1)` (default 0.5).
    #[serde(default = "default_duty")]
    pub duty: f32,
}

/// The shape of a [`SilhouettePattern`].
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SilhouettePatternKind {
    /// Horizontal lines.
    Scanlines,
    /// Diagonal lines at 45°.
    Hatch,
}

/// The outline of an [`OccludedSilhouette`].
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq)]
pub struct SilhouetteOutline {
    /// Width in pixels at 1080p, in `(0, 8]`; scales with the render height (default 2).
    #[serde(default = "default_px")]
    pub px: f32,
    /// Outline opacity in `[0, 1]` (default 1).
    #[serde(default = "default_alpha")]
    pub alpha: f32,
}

/// When an [`OccludedSilhouette`] draws.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SilhouetteWhen {
    /// Only where a nearer surface hides the mesh.
    #[default]
    Hidden,
    /// Everywhere the mesh is drawn.
    Always,
}

impl SilhouetteWhen {
    fn is_hidden(&self) -> bool {
        *self == Self::Hidden
    }
}

impl Default for OccludedSilhouette {
    fn default() -> Self {
        Self {
            color: default_color(),
            outline: None,
            when: SilhouetteWhen::Hidden,
            bias: default_bias(),
            pattern: None,
            inner_outline: None,
            film: true,
        }
    }
}

fn default_film() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

fn default_period() -> f32 {
    4.0
}

fn default_duty() -> f32 {
    0.5
}

fn default_color() -> [f32; 4] {
    [1.0, 1.0, 1.0, 0.55]
}

fn default_bias() -> f32 {
    0.05
}

fn default_px() -> f32 {
    2.0
}

fn default_alpha() -> f32 {
    1.0
}

impl SemanticValidate for OccludedSilhouette {
    /// Rejects what a schema cannot (`L2350`, pointer into the field): a colour channel outside
    /// `[0, 1]`, `outline.px` outside `(0, 8]`, `outline.alpha` outside `[0, 1]`, a negative or
    /// non-finite `bias`. A wrong *shape* is the schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut reject = |path: String, what: String| {
            out.push(
                Diagnostic::new(
                    code::OCCLUDED_SILHOUETTE_INVALID,
                    format!("`OccludedSilhouette` {what}"),
                )
                .with_path(path)
                .with_help("see `ludion docs occluded-silhouette`; `ludion explain L2350`"),
            );
        };
        if let Some(channels) = value.get("color").and_then(Value::as_array) {
            for (i, channel) in channels.iter().enumerate() {
                if let Some(c) = channel.as_f64() {
                    let c = engine_number(c) as f32;
                    if !(0.0..=1.0).contains(&c) {
                        reject(
                            format!("/color/{i}"),
                            format!("colour channel {i} is {c}; it takes a value in [0, 1]"),
                        );
                    }
                }
            }
        }
        for field in ["outline", "inner_outline"] {
            if let Some(px) = value
                .pointer(&format!("/{field}/px"))
                .and_then(Value::as_f64)
            {
                let px = engine_number(px) as f32;
                if !(px > 0.0 && px <= MAX_OUTLINE_PX) {
                    reject(
                        format!("/{field}/px"),
                        format!("`{field}.px` is {px}; it takes a value in (0, {MAX_OUTLINE_PX}]"),
                    );
                }
            }
            if let Some(alpha) = value
                .pointer(&format!("/{field}/alpha"))
                .and_then(Value::as_f64)
            {
                let alpha = engine_number(alpha) as f32;
                if !(0.0..=1.0).contains(&alpha) {
                    reject(
                        format!("/{field}/alpha"),
                        format!("`{field}.alpha` is {alpha}; it takes a value in [0, 1]"),
                    );
                }
            }
        }
        if let Some(period) = value.pointer("/pattern/period").and_then(Value::as_f64) {
            let period = engine_number(period) as f32;
            if !(MIN_PATTERN_PERIOD..=MAX_PATTERN_PERIOD).contains(&period) {
                reject(
                    "/pattern/period".into(),
                    format!(
                        "`pattern.period` is {period}; it takes a value in [{MIN_PATTERN_PERIOD}, {MAX_PATTERN_PERIOD}]"
                    ),
                );
            }
        }
        if let Some(duty) = value.pointer("/pattern/duty").and_then(Value::as_f64) {
            let duty = engine_number(duty) as f32;
            if !(duty > 0.0 && duty < 1.0) {
                reject(
                    "/pattern/duty".into(),
                    format!("`pattern.duty` is {duty}; it takes a value in (0, 1), exclusive"),
                );
            }
        }
        if let Some(bias) = value.get("bias").and_then(Value::as_f64) {
            let bias = engine_number(bias) as f32;
            if !(bias.is_finite() && bias >= 0.0) {
                reject(
                    "/bias".into(),
                    format!("`bias` is {bias}; it takes a finite value of 0 or more"),
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

    fn paths(value: Value) -> Vec<String> {
        OccludedSilhouette::validate_semantics(&value)
            .into_iter()
            .map(|d| d.path.unwrap())
            .collect()
    }

    #[test]
    fn empty_object_takes_the_defaults_and_is_clean() {
        let s: OccludedSilhouette = serde_json::from_value(json!({})).unwrap();
        assert_eq!(s, OccludedSilhouette::default());
        assert_eq!(s.color, [1.0, 1.0, 1.0, 0.55]);
        assert_eq!(s.bias, 0.05);
        let written = serde_json::to_value(s).unwrap();
        assert!(written.get("outline").is_none() && written.get("when").is_none());
        assert!(written.get("pattern").is_none() && written.get("inner_outline").is_none());
        assert!(written.get("film").is_none() && s.film);
        assert!(paths(json!({})).is_empty());
    }

    #[test]
    fn a_full_silhouette_round_trips() {
        let value = json!({
            "color": [0.0, 1.0, 0.0, 0.75],
            "outline": { "px": 3.0, "alpha": 0.5 },
            "when": "always",
            "bias": 0.125,
            "pattern": { "kind": "hatch", "period": 6.0, "duty": 0.25 },
            "inner_outline": { "px": 1.0, "alpha": 0.5 },
            "film": false,
        });
        let s: OccludedSilhouette = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(s.when, SilhouetteWhen::Always);
        assert_eq!(serde_json::to_value(s).unwrap(), value);
    }

    #[test]
    fn a_pattern_and_inner_outline_take_their_defaults() {
        let s: OccludedSilhouette = serde_json::from_value(json!({
            "pattern": { "kind": "scanlines" },
            "inner_outline": {},
        }))
        .unwrap();
        assert_eq!(
            s.pattern,
            Some(SilhouettePattern {
                kind: SilhouettePatternKind::Scanlines,
                period: 4.0,
                duty: 0.5
            })
        );
        assert_eq!(
            s.inner_outline,
            Some(SilhouetteOutline {
                px: 2.0,
                alpha: 1.0
            })
        );
    }

    #[test]
    fn an_outline_takes_its_defaults() {
        let s: OccludedSilhouette = serde_json::from_value(json!({ "outline": {} })).unwrap();
        assert_eq!(
            s.outline,
            Some(SilhouetteOutline {
                px: 2.0,
                alpha: 1.0
            })
        );
    }

    #[test]
    fn each_bad_field_names_its_pointer() {
        assert_eq!(
            paths(json!({ "color": [1.0, 1.5, 1.0, 1.0] })),
            ["/color/1"]
        );
        assert_eq!(
            paths(json!({ "color": [-0.1, 1.0, 1.0, 2.0] })),
            ["/color/0", "/color/3"]
        );
        assert_eq!(paths(json!({ "outline": { "px": 0.0 } })), ["/outline/px"]);
        assert_eq!(paths(json!({ "outline": { "px": 8.5 } })), ["/outline/px"]);
        assert!(paths(json!({ "outline": { "px": 8.0 } })).is_empty());
        assert_eq!(
            paths(json!({ "outline": { "alpha": 1.5 } })),
            ["/outline/alpha"]
        );
        assert_eq!(
            paths(json!({ "inner_outline": { "px": 0.0 } })),
            ["/inner_outline/px"]
        );
        assert!(paths(json!({ "inner_outline": { "px": 8.0 } })).is_empty());
        assert_eq!(
            paths(json!({ "inner_outline": { "alpha": 1.5 } })),
            ["/inner_outline/alpha"]
        );
        assert!(paths(json!({ "inner_outline": { "alpha": 0.0 } })).is_empty());
        assert_eq!(
            paths(json!({ "pattern": { "kind": "hatch", "period": 1.0 } })),
            ["/pattern/period"]
        );
        assert_eq!(
            paths(json!({ "pattern": { "kind": "hatch", "period": 65.0 } })),
            ["/pattern/period"]
        );
        assert!(paths(json!({ "pattern": { "kind": "hatch", "period": 2.0 } })).is_empty());
        assert!(paths(json!({ "pattern": { "kind": "hatch", "period": 64.0 } })).is_empty());
        assert_eq!(
            paths(json!({ "pattern": { "kind": "scanlines", "duty": 0.0 } })),
            ["/pattern/duty"]
        );
        assert_eq!(
            paths(json!({ "pattern": { "kind": "scanlines", "duty": 1.0 } })),
            ["/pattern/duty"]
        );
        assert!(paths(json!({ "pattern": { "kind": "scanlines", "duty": 0.999 } })).is_empty());
        assert_eq!(paths(json!({ "bias": -0.01 })), ["/bias"]);
        assert!(paths(json!({ "bias": 0.0 })).is_empty());
        assert_eq!(
            paths(json!({ "bias": -1.0, "outline": { "px": 99.0 } })),
            ["/outline/px", "/bias"]
        );
    }
}
