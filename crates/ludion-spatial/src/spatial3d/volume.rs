//! The [`SpatialVolume`] builtin component: the 3D bounds an entity files into the [`Index3d`].

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Handle, Item, Volume};

/// The bounds of a [`SpatialVolume`], serialized with serde's external tagging:
///
/// ```yaml
/// shape:
///   sphere:
///     radius: 0.5
/// shape:
///   aabb:
///     half_extents: [0.5, 1.0, 0.5]
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum VolumeShape {
    /// A sphere about the entity's world translation.
    Sphere {
        /// Radius in world units (must be `> 0`; default `0.5`).
        #[serde(default = "half")]
        #[schemars(range(min = 0.0))]
        radius: f32,
    },
    /// An axis-aligned box about the entity's world translation.
    Aabb {
        /// Half-extents on x, y and z, in world units (each must be `> 0`; default
        /// `[0.5, 0.5, 0.5]`).
        #[serde(default = "half3")]
        half_extents: [f32; 3],
    },
}

impl Default for VolumeShape {
    fn default() -> Self {
        VolumeShape::Sphere { radius: half() }
    }
}

/// A 3D bound and layer mask that files an entity into the [`Index3d`](super::Index3d) when a game
/// schedules [`rebuild`](super::rebuild).
///
/// Centred on the entity's world translation; rotation and scale are ignored (the bounds are
/// axis-aligned, like `Collider`). `layers` is a raw bitmask, one bit per game-defined layer, and
/// is unrelated to the named `physics.layers`. Every field defaults, so `{}` is a `0.5`-radius
/// sphere on layer bit 0.
///
/// ```yaml
/// SpatialVolume:
///   shape:
///     sphere:
///       radius: 0.5
///   layers: 1
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct SpatialVolume {
    /// The bounds (default a `0.5`-radius sphere).
    #[serde(default)]
    pub shape: VolumeShape,
    /// Layer bits a query filter matches against (must be non-zero; default `1`).
    #[serde(default = "one")]
    pub layers: u32,
}

impl Default for SpatialVolume {
    fn default() -> Self {
        Self {
            shape: VolumeShape::default(),
            layers: one(),
        }
    }
}

impl SpatialVolume {
    /// The index [`Item`] for this volume as `handle`, centred on `center`.
    pub fn item(&self, handle: Handle, center: [f32; 3]) -> Item {
        let volume = match self.shape {
            VolumeShape::Sphere { radius } => Volume::Sphere { center, radius },
            VolumeShape::Aabb { half_extents: h } => Volume::Aabb {
                min: [center[0] - h[0], center[1] - h[1], center[2] - h[2]],
                max: [center[0] + h[0], center[1] + h[1], center[2] + h[2]],
            },
        };
        Item {
            handle,
            volume,
            layers: self.layers,
        }
    }
}

impl SemanticValidate for SpatialVolume {
    /// Rejects a radius or half extent that is not a positive finite `f32` (`/shape/sphere/radius`,
    /// `/shape/aabb/half_extents/<i>`) and `layers == 0` (`/layers`), all `L1339`, with pointers
    /// relative to the component root.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        if let Some(radius) = value
            .pointer("/shape/sphere/radius")
            .and_then(Value::as_f64)
        {
            require_positive(radius, "/shape/sphere/radius".to_string(), &mut out);
        }
        if let Some(extents) = value
            .pointer("/shape/aabb/half_extents")
            .and_then(Value::as_array)
        {
            for (i, extent) in extents.iter().enumerate() {
                if let Some(extent) = extent.as_f64() {
                    require_positive(extent, format!("/shape/aabb/half_extents/{i}"), &mut out);
                }
            }
        }
        if value.get("layers").and_then(Value::as_u64) == Some(0) {
            out.push(
                Diagnostic::new(
                    code::SPATIAL_VOLUME_INVALID,
                    "a spatial volume with `layers: 0` is on no layer, so no query filter can see it"
                        .to_string(),
                )
                .with_path("/layers".to_string())
                .with_help("set at least one bit, e.g. `layers: 1`"),
            );
        }
        out
    }
}

fn require_positive(value: f64, pointer: String, out: &mut Vec<Diagnostic>) {
    if !ludion_core::number::positive_f32(value) {
        out.push(
            Diagnostic::new(
                code::SPATIAL_VOLUME_INVALID,
                format!(
                    "spatial volume size {value} is not valid; a radius or half-extent must be \
                     greater than 0 as an f32 (at least ~1.4e-45, at most ~3.4e38)"
                ),
            )
            .with_path(pointer)
            .with_help("give every radius and half-extent a value greater than 0"),
        );
    }
}

fn half() -> f32 {
    0.5
}

fn half3() -> [f32; 3] {
    [0.5, 0.5, 0.5]
}

fn one() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pointers(value: Value) -> Vec<String> {
        SpatialVolume::validate_semantics(&value)
            .into_iter()
            .inspect(|d| assert_eq!(d.code, code::SPATIAL_VOLUME_INVALID))
            .map(|d| d.path.expect("path"))
            .collect()
    }

    #[test]
    fn defaults_deserialize_from_empty() {
        let v: SpatialVolume = serde_json::from_value(json!({})).unwrap();
        assert_eq!(v, SpatialVolume::default());
        assert_eq!(v.shape, VolumeShape::Sphere { radius: 0.5 });
        assert_eq!(v.layers, 1);
        let a: VolumeShape = serde_json::from_value(json!({"aabb": {}})).unwrap();
        assert_eq!(
            a,
            VolumeShape::Aabb {
                half_extents: [0.5; 3]
            }
        );
    }

    #[test]
    fn valid_volume_has_no_diagnostics() {
        assert!(pointers(json!({"shape": {"sphere": {"radius": 2.0}}, "layers": 5})).is_empty());
        assert!(pointers(json!({"shape": {"aabb": {"half_extents": [1, 2, 3]}}})).is_empty());
    }

    #[test]
    fn bad_sizes_and_layers_point_at_the_field() {
        assert_eq!(
            pointers(json!({"shape": {"sphere": {"radius": 0.0}}})),
            ["/shape/sphere/radius"]
        );
        assert_eq!(
            pointers(json!({"shape": {"sphere": {"radius": -1.0}}})),
            ["/shape/sphere/radius"]
        );
        assert_eq!(
            pointers(json!({"shape": {"aabb": {"half_extents": [1.0, 0.0, -2.0]}}})),
            ["/shape/aabb/half_extents/1", "/shape/aabb/half_extents/2"]
        );
        assert_eq!(pointers(json!({"layers": 0})), ["/layers"]);
    }

    #[test]
    fn item_centres_the_volume() {
        let h = Handle::slot(0, 0);
        let v = SpatialVolume {
            shape: VolumeShape::Aabb {
                half_extents: [1.0, 2.0, 3.0],
            },
            layers: 2,
        };
        assert_eq!(
            v.item(h, [1.0, 1.0, 1.0]),
            Item {
                handle: h,
                volume: Volume::Aabb {
                    min: [0.0, -1.0, -2.0],
                    max: [2.0, 3.0, 4.0]
                },
                layers: 2
            }
        );
    }
}
