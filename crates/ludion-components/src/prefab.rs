//! The [`PrefabInstance`] component.

use ludion_core::ids::EntityId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where a prefab part came from: written onto every named part by `spawn_prefab`, hashed and
/// snapshotted like any component.
///
/// `root` is the instance's first part — a prefab's parts are one group, and the group's identity is
/// its first id — so it remaps like [`Joint`](ludion_spatial::Joint)'s `a`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct PrefabInstance {
    /// The prefab file, relative to the content root, normalized: forward slashes, no `.` segments.
    pub source: String,
    /// The part's name in the prefab, without the instance prefix.
    pub part: String,
    /// The instance's first part; the first part names itself.
    #[schemars(with = "ludion_ecs::EntityRefSelfSchema")]
    pub root: EntityId,
}
