//! [`MeshBlend`]: how a blended [`Mesh`](crate::Mesh) combines with what is behind it
//! (`Plan/transparent-depth.md` decisions 5–9).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How a blended [`Mesh`](crate::Mesh) resolves its own overlapping surfaces (`Mesh.blend_depth`,
/// `Plan/transparent-depth.md` decision 15).
///
/// ```
/// use ludion_components::BlendDepth;
/// assert_eq!(serde_json::to_string(&BlendDepth::Nearest).unwrap(), "\"nearest\"");
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BlendDepth {
    /// Every surface blends, back faces and far limbs included (the default).
    #[default]
    None,
    /// Only the entity's front surface blends: a translucent figure no longer shows its far arm
    /// through its near one. An assembly is one figure. Costs a depth copy and two small passes per
    /// entity per frame, twice with `transparent_depth`: for a handful of figures.
    Nearest,
}

impl BlendDepth {
    /// Whether this is the default, for `skip_serializing_if`.
    pub fn is_none(&self) -> bool {
        *self == BlendDepth::None
    }
}

/// The blend of a [`Mesh`](crate::Mesh) that is seen through. `Mesh.color`'s alpha is the opacity;
/// `Fade` multiplies it. A blended mesh is lit like an opaque one, drawn back to front per entity after
/// the ground markers and before the sprites, tests depth and writes none, casts no shadow and is
/// invisible to every opaque consumer (ambient occlusion, GI, reflections, the silhouette).
///
/// ```yaml
/// components:
///   Mesh:
///     shape: cube
///     color: [0.4, 0.7, 1.0, 0.35]
///     blend: alpha
/// ```
///
/// ```
/// use ludion_components::MeshBlend;
/// assert_eq!(serde_json::to_string(&MeshBlend::Additive).unwrap(), "\"additive\"");
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MeshBlend {
    /// Premultiplied alpha: the mesh covers what is behind it by its opacity.
    Alpha,
    /// Adds the lit colour scaled by its opacity: it brightens and never darkens.
    Additive,
}
