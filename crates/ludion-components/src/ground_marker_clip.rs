//! The [`GroundMarkerClip`] component: a line-of-sight clip for a `GroundMarker`.
//! `extract` in `ludion-engine` casts the profile; `ludion docs ground-telegraphs` is the guide.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::number::engine_number;
use ludion_ecs::SemanticValidate;
use ludion_physics::LayerSet;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The fewest rays a clip casts.
pub const MIN_CLIP_BINS: u32 = 8;
/// The most rays a clip casts.
pub const MAX_CLIP_BINS: u32 = 512;

/// Clips the `GroundMarker` beside it by the walls its apex can see. `bins` rays fan across the
/// marker's arc (the full circle for a disc or ring) at `height` above the entity origin; the marker
/// is drawn only out to the first hit per ray. A `partial_layers` hit does not clip but dims what lies
/// behind it to `partial_alpha`. `layers` empty means every layer; `partial_layers` empty means none.
/// Static colliders only unless `dynamic`, which casts every extract.
///
/// ```
/// use ludion_components::GroundMarkerClip;
///
/// let clip: GroundMarkerClip = serde_json::from_value(serde_json::json!({
///     "bins": 128,
///     "layers": ["sight_blocker"],
///     "partial_layers": ["low_cover"],
/// }))
/// .unwrap();
/// assert_eq!(clip.height, 1.6);
/// assert_eq!(clip.partial_alpha, 0.35);
/// assert!(!clip.dynamic);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
pub struct GroundMarkerClip {
    /// Ray height above the entity origin, in world units (default 1.6).
    #[serde(default = "default_height")]
    pub height: f32,
    /// Rays across the arc, 8 to 512 (default 96).
    #[serde(default = "default_bins")]
    pub bins: u32,
    /// Layers whose colliders clip the marker; empty means every layer.
    #[serde(default, skip_serializing_if = "LayerSet::is_empty")]
    pub layers: LayerSet,
    /// Layers whose colliders dim what lies behind them; empty means none.
    #[serde(default, skip_serializing_if = "LayerSet::is_empty")]
    pub partial_layers: LayerSet,
    /// The coverage multiplier behind a partial hit, in `[0, 1]` (default 0.35).
    #[serde(default = "default_partial_alpha")]
    pub partial_alpha: f32,
    /// Cast against dynamic colliders too, every extract; false means static colliders only.
    #[serde(default, skip_serializing_if = "is_false")]
    pub dynamic: bool,
}

impl Default for GroundMarkerClip {
    fn default() -> Self {
        Self {
            height: default_height(),
            bins: default_bins(),
            layers: LayerSet::default(),
            partial_layers: LayerSet::default(),
            partial_alpha: default_partial_alpha(),
            dynamic: false,
        }
    }
}

fn default_height() -> f32 {
    1.6
}

fn default_bins() -> u32 {
    96
}

fn default_partial_alpha() -> f32 {
    0.35
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl SemanticValidate for GroundMarkerClip {
    /// Rejects what a schema cannot (`L2342`, pointer into the field): `bins` outside 8 to 512, a
    /// non-finite `height`, a `partial_alpha` outside `[0, 1]`. A wrong *shape* is the schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut reject = |path: &str, what: String| {
            out.push(
                Diagnostic::new(
                    code::GROUND_MARKER_CLIP_INVALID,
                    format!("`GroundMarkerClip` {what}"),
                )
                .with_path(path)
                .with_help("see `ludion docs ground-telegraphs`; `ludion explain L2342`"),
            );
        };
        if let Some(bins) = value.get("bins").and_then(Value::as_u64)
            && !(u64::from(MIN_CLIP_BINS)..=u64::from(MAX_CLIP_BINS)).contains(&bins)
        {
            reject(
                "/bins",
                format!("`bins` is {bins}; it takes {MIN_CLIP_BINS} to {MAX_CLIP_BINS}"),
            );
        }
        if let Some(height) = value.get("height").and_then(Value::as_f64) {
            let height = engine_number(height) as f32;
            if !height.is_finite() {
                reject("/height", "`height` must be finite".into());
            }
        }
        if let Some(alpha) = value.get("partial_alpha").and_then(Value::as_f64) {
            let alpha = engine_number(alpha) as f32;
            if !(0.0..=1.0).contains(&alpha) {
                reject(
                    "/partial_alpha",
                    format!("`partial_alpha` is {alpha}; it takes a value in [0, 1]"),
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
        GroundMarkerClip::validate_semantics(&value)
            .into_iter()
            .map(|d| d.path.unwrap())
            .collect()
    }

    #[test]
    fn empty_object_takes_the_defaults_and_is_clean() {
        let clip: GroundMarkerClip = serde_json::from_value(json!({})).unwrap();
        assert_eq!(clip, GroundMarkerClip::default());
        assert_eq!(clip.bins, 96);
        let written = serde_json::to_value(&clip).unwrap();
        assert_eq!(written["bins"], 96);
        assert!(written.get("layers").is_none() && written.get("dynamic").is_none());
        assert!(paths(json!({})).is_empty());
    }

    #[test]
    fn a_full_clip_round_trips() {
        let value = json!({
            "height": 1.25,
            "bins": 128,
            "layers": ["sight_blocker"],
            "partial_layers": ["low_cover"],
            "partial_alpha": 0.5,
            "dynamic": true,
        });
        let clip: GroundMarkerClip = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&clip).unwrap(), value);
    }

    #[test]
    fn each_bad_field_names_its_pointer() {
        assert_eq!(paths(json!({ "bins": 7 })), ["/bins"]);
        assert_eq!(paths(json!({ "bins": 513 })), ["/bins"]);
        assert!(paths(json!({ "bins": 8 })).is_empty());
        assert!(paths(json!({ "bins": 512 })).is_empty());
        assert_eq!(paths(json!({ "partial_alpha": 1.5 })), ["/partial_alpha"]);
        assert_eq!(paths(json!({ "partial_alpha": -0.1 })), ["/partial_alpha"]);
        assert_eq!(
            paths(json!({ "bins": 2, "partial_alpha": 2.0 })),
            ["/bins", "/partial_alpha"]
        );
    }
}
