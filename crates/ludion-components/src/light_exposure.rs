//! The [`LightExposure`] component: the points on a subject whose light the engine reads each tick.
//! `light_exposure_update` in `ludion-engine` reads it; `ludion docs light-exposure` is the guide.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::number::engine_number;
use ludion_ecs::SemanticValidate;
use ludion_physics::LayerSet;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The most points one exposure query may carry.
pub const MAX_EXPOSURE_POINTS: usize = 16;

/// The surface orientation a point is lit as.
///
/// ```
/// use ludion_components::Facing;
///
/// let omni: Facing = serde_json::from_value(serde_json::json!("omni")).unwrap();
/// assert_eq!(omni, Facing::Omni);
/// let up: Facing = serde_json::from_value(serde_json::json!({ "normal": [0.0, 1.0, 0.0] })).unwrap();
/// assert_eq!(up, Facing::Normal([0.0, 1.0, 0.0]));
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Facing {
    /// `n·l = 1` for every light: the light a body of unknown orientation could receive at most.
    #[default]
    Omni,
    /// A surface with this normal (any non-zero length): `max(n·l, 0)`.
    Normal([f32; 3]),
}

/// How several points collapse into one reading.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Combine {
    /// The point with the largest `total`, whole; ties go to the first in authored order.
    #[default]
    Max,
    /// The componentwise mean of the per-point readings.
    Mean,
}

/// A subject whose light exposure `light_exposure_update` reads every tick into
/// `LightExposureResults`. `points` are **entity-local offsets**, mapped through the entity's world
/// pose; `facing.normal` is world-space. `occluder_layers` absent means static colliders only (the
/// subject excluded); present means exactly those layers.
///
/// ```
/// use ludion_components::{Combine, Facing, LightExposure};
///
/// let subject: LightExposure = serde_json::from_value(serde_json::json!({
///     "points": [[0.0, 0.3, 0.0], [0.0, 1.6, 0.0]],
///     "combine": "mean",
///     "occluder_layers": ["light_occluder"],
/// }))
/// .unwrap();
/// assert_eq!(subject.points.len(), 2);
/// assert_eq!(subject.facing, Facing::Omni);
/// assert_eq!(subject.combine, Combine::Mean);
/// assert_eq!(subject.occluder_layers.unwrap().as_slice(), ["light_occluder"]);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
pub struct LightExposure {
    /// 1 to 16 entity-local sample points (default `[[0, 0, 0]]`).
    #[serde(default = "default_points")]
    pub points: Vec<[f32; 3]>,
    /// The surface orientation at every point (default `omni`).
    #[serde(default, skip_serializing_if = "is_default")]
    pub facing: Facing,
    /// How the points collapse (default `max`).
    #[serde(default, skip_serializing_if = "is_default")]
    pub combine: Combine,
    /// The layers whose colliders block a shadow ray; absent means static colliders only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occluder_layers: Option<LayerSet>,
}

impl Default for LightExposure {
    fn default() -> Self {
        Self {
            points: default_points(),
            facing: Facing::default(),
            combine: Combine::default(),
            occluder_layers: None,
        }
    }
}

fn default_points() -> Vec<[f32; 3]> {
    vec![[0.0, 0.0, 0.0]]
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

fn number(value: &Value) -> Option<f32> {
    value.as_f64().map(|n| engine_number(n) as f32)
}

impl SemanticValidate for LightExposure {
    /// Rejects what a schema cannot (`L4250`, pointer into the field): a `points` list that is
    /// empty or holds more than 16, a non-finite point coordinate, a `facing.normal` that is zero
    /// or non-finite, and an empty or repeated `occluder_layers` entry. A wrong *shape* is the
    /// schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut reject = |path: String, what: String| {
            out.push(
                Diagnostic::new(
                    code::LIGHT_EXPOSURE_INVALID,
                    format!("`LightExposure` {what}"),
                )
                .with_path(path)
                .with_help("see `ludion docs light-exposure`; `ludion explain L4250`"),
            );
        };
        if let Some(points) = value.get("points").and_then(Value::as_array) {
            if points.is_empty() || points.len() > MAX_EXPOSURE_POINTS {
                reject(
                    "/points".into(),
                    format!(
                        "`points` holds {} entries; it takes 1 to {MAX_EXPOSURE_POINTS}",
                        points.len()
                    ),
                );
            }
            for (i, point) in points.iter().enumerate() {
                let finite = point
                    .as_array()
                    .is_none_or(|c| c.iter().all(|c| number(c).is_none_or(f32::is_finite)));
                if !finite {
                    reject(
                        format!("/points/{i}"),
                        format!("`points[{i}]` has a non-finite coordinate"),
                    );
                }
            }
        }
        if let Some(normal) = value
            .get("facing")
            .and_then(|f| f.get("normal"))
            .and_then(Value::as_array)
        {
            let c: Vec<f32> = normal.iter().filter_map(number).collect();
            if c.len() == 3 && (c.iter().any(|c| !c.is_finite()) || c.iter().all(|c| *c == 0.0)) {
                reject(
                    "/facing/normal".into(),
                    "`facing.normal` must be non-zero and finite".into(),
                );
            }
        }
        if let Some(layers) = value.get("occluder_layers").and_then(Value::as_array) {
            let names: Vec<Option<&str>> = layers.iter().map(Value::as_str).collect();
            for (j, name) in names.iter().enumerate() {
                let Some(name) = name else { continue };
                if name.is_empty() {
                    reject(
                        format!("/occluder_layers/{j}"),
                        format!("`occluder_layers[{j}]` is empty"),
                    );
                } else if names[..j].contains(&Some(name)) {
                    reject(
                        format!("/occluder_layers/{j}"),
                        format!("`occluder_layers[{j}]` repeats `{name}`"),
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

    fn paths(value: Value) -> Vec<String> {
        LightExposure::validate_semantics(&value)
            .into_iter()
            .map(|d| d.path.unwrap())
            .collect()
    }

    #[test]
    fn a_default_component_is_clean_and_serializes_empty_but_points() {
        let value = serde_json::to_value(LightExposure::default()).unwrap();
        assert_eq!(value, json!({ "points": [[0.0, 0.0, 0.0]] }));
        assert!(paths(value).is_empty());
    }

    #[test]
    fn points_must_number_one_to_sixteen() {
        assert_eq!(paths(json!({ "points": [] })), ["/points"]);
        let many: Vec<[f32; 3]> = vec![[0.0; 3]; 17];
        assert_eq!(paths(json!({ "points": many })), ["/points"]);
        assert!(paths(json!({ "points": vec![[0.0; 3]; 16] })).is_empty());
    }

    #[test]
    fn a_normal_must_be_non_zero() {
        assert_eq!(
            paths(json!({ "facing": { "normal": [0.0, 0.0, 0.0] } })),
            ["/facing/normal"]
        );
        assert!(paths(json!({ "facing": { "normal": [0.0, 2.0, 0.0] } })).is_empty());
        assert!(paths(json!({ "facing": "omni" })).is_empty());
    }

    #[test]
    fn occluder_layers_reject_empty_and_repeated_entries() {
        assert_eq!(
            paths(json!({ "occluder_layers": ["a", "", "a"] })),
            ["/occluder_layers/1", "/occluder_layers/2"]
        );
    }
}
