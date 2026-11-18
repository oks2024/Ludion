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
/// At most 8 distinct styles are drawn per frame.
///
/// ```
/// use ludion_components::{OccludedSilhouette, SilhouetteWhen};
///
/// let silhouette: OccludedSilhouette = serde_json::from_value(serde_json::json!({
///     "outline": { "px": 2.0 },
/// }))
/// .unwrap();
/// assert_eq!(silhouette.color, [1.0, 1.0, 1.0, 0.55]);
/// assert_eq!(silhouette.when, SilhouetteWhen::Hidden);
/// assert_eq!(silhouette.outline.unwrap().alpha, 1.0);
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
        }
    }
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
        if let Some(px) = value.pointer("/outline/px").and_then(Value::as_f64) {
            let px = engine_number(px) as f32;
            if !(px > 0.0 && px <= MAX_OUTLINE_PX) {
                reject(
                    "/outline/px".into(),
                    format!("`outline.px` is {px}; it takes a value in (0, {MAX_OUTLINE_PX}]"),
                );
            }
        }
        if let Some(alpha) = value.pointer("/outline/alpha").and_then(Value::as_f64) {
            let alpha = engine_number(alpha) as f32;
            if !(0.0..=1.0).contains(&alpha) {
                reject(
                    "/outline/alpha".into(),
                    format!("`outline.alpha` is {alpha}; it takes a value in [0, 1]"),
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
        assert!(paths(json!({})).is_empty());
    }

    #[test]
    fn a_full_silhouette_round_trips() {
        let value = json!({
            "color": [0.0, 1.0, 0.0, 0.75],
            "outline": { "px": 3.0, "alpha": 0.5 },
            "when": "always",
            "bias": 0.125,
        });
        let s: OccludedSilhouette = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(s.when, SilhouetteWhen::Always);
        assert_eq!(serde_json::to_value(s).unwrap(), value);
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
        assert_eq!(paths(json!({ "bias": -0.01 })), ["/bias"]);
        assert!(paths(json!({ "bias": 0.0 })).is_empty());
        assert_eq!(
            paths(json!({ "bias": -1.0, "outline": { "px": 99.0 } })),
            ["/outline/px", "/bias"]
        );
    }
}
