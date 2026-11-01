//! The [`PathFollower`] component: an entity that rides a `kind: path` document.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::number::engine_number;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a [`PathFollower`] does at the end of its path.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PathMode {
    /// Wraps: past the end it is back at the start (an open path jumps, a closed one is seamless).
    #[default]
    Loop,
    /// Runs to the end and back, forever.
    PingPong,
    /// Runs to the end and stays there (or at the start, going backwards).
    Once,
}

/// How a [`PathFollower`] turns to face its travel. Rotation is written into `Transform.rotation`
/// in the parent's frame, as Euler XYZ degrees; local −Z faces travel, as a camera's does.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PathOrient {
    /// Rotation is untouched.
    #[default]
    None,
    /// Rotation about +Y only, facing the horizontal travel direction. A vertical stretch keeps the
    /// last yaw (the rotation is not written).
    Yaw,
    /// The path's no-roll frame: −Z faces travel, +Y is up.
    Frame,
}

impl PathOrient {
    #[doc(hidden)]
    pub fn is_none(&self) -> bool {
        *self == PathOrient::None
    }
}

/// Moves an entity along a `kind: path` document: `path_follow` writes its `Transform.translation`
/// every tick (scale is untouched, and so is rotation unless `orient` is set), advancing `phase` by `speed` on gameplay time.
///
/// The pose is in the parent's frame — the path's points are scene coordinates for a root and local
/// to the `Parent` otherwise. A game drives the position
/// itself by writing `phase` with `speed: 0`; the write poses on the next tick.
/// `path_follower_motion` reads what the tick did, and a wrap or jump is a teleport for a
/// kinematic body.
///
/// # Example
/// ```
/// use ludion_components::{PathFollower, PathMode, PathOrient};
///
/// let lift: PathFollower = serde_json::from_value(serde_json::json!({
///     "path": "paths/lift.path.yaml",
///     "speed": 2.0,
///     "mode": "ping_pong",
/// }))
/// .unwrap();
/// assert_eq!(lift.mode, PathMode::PingPong);
/// assert_eq!(lift.phase, 0.0);
/// assert_eq!(lift.orient, PathOrient::None);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct PathFollower {
    /// The `kind: path` document, relative to the content root (default `""` — inert).
    #[serde(default)]
    #[schemars(with = "ludion_ecs::AssetPathSchema")]
    pub path: String,
    /// Arc-length metres per gameplay second; negative runs backwards (default `1.0`).
    #[serde(default = "one")]
    pub speed: f32,
    /// What happens at the end of the path (default `loop`).
    #[serde(default)]
    pub mode: PathMode,
    /// Whether and how the follower turns to face its travel (default `none`: rotation untouched).
    /// Omitted from a serialized follower while `none`, so existing scenes and hashes are unchanged.
    #[serde(default, skip_serializing_if = "PathOrient::is_none")]
    pub orient: PathOrient,
    /// Metres along the path (default `0.0`): the start offset as authored, rewritten every tick.
    #[serde(default)]
    pub phase: f32,
}

fn one() -> f32 {
    1.0
}

impl SemanticValidate for PathFollower {
    /// Rejects a `speed` or `phase` that is not a finite `f32` (`L1269`, pointers `/speed` and
    /// `/phase`): a non-finite number, or one past `f32` range, which the field would store as
    /// infinity. A wrong *shape* is the schema's job and adds nothing here.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        [
            ("speed", "metres per second"),
            ("phase", "metres along the path"),
        ]
        .into_iter()
        .filter_map(|(field, unit)| {
            let number = value.get(field)?.as_f64().map(engine_number)?;
            (!number.is_finite()).then(|| {
                Diagnostic::new(
                    code::PATH_FOLLOWER_INVALID,
                    format!("`PathFollower` `{field}` {number} is not a finite number"),
                )
                .with_path(format!("/{field}"))
                .with_help(format!("give `{field}` a finite number of {unit}"))
            })
        })
        .collect()
    }
}

impl Default for PathFollower {
    fn default() -> Self {
        PathFollower {
            path: String::new(),
            speed: 1.0,
            mode: PathMode::Loop,
            orient: PathOrient::None,
            phase: 0.0,
        }
    }
}
