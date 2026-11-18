//! [`MeshBlend`]: how a blended [`Mesh`](crate::Mesh) combines with what is behind it
//! (`Plan/transparent-depth.md` decisions 5–9).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
