#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `ludion-spatial` — deterministic 2D collision on the two in-plane axes ([`Plane`]; x/z by
//! default).
//!
//! This crate promotes the collision logic Breakout and Sokoban each hand-rolled into one sim-side
//! library: the [`Collider`] builtin component, an explicitly-scheduled per-frame spatial hash
//! ([`SpatialIndex`] / [`spatial::rebuild`]), and pinned-order queries
//! ([`spatial::overlaps`] / [`spatial::raycast`] /
//! [`spatial::k_nearest`]). It deliberately has **no rigid-body dynamics** —
//! games write their own collision response on top of the queries.
//!
//! [`spatial3d`] is the 3D counterpart: a generic, bit-exact bounding-volume hierarchy over
//! caller-fed spheres and boxes (overlap, k-nearest, first-hit swept-sphere cast), independent of
//! [`Collider`] and the ground plane.
//!
//! # Boundary
//!
//! This crate sits beside `ludion-sim` on the CPU-only sim side and depends only on `ludion-core`
//! (ids, collections) and `ludion-ecs` (`World`, `Component`, `SystemCtx`, `SemanticValidate`). It
//! never names a renderer type and never depends on `ludion-engine` (that edge runs the other way:
//! `ludion-engine` names [`Collider`] to register it). Because [`Collider`] carries only *shape*,
//! the position a collider sits at comes from a companion component the game already updates —
//! provided through the [`GroundPosition`] trait so this crate stays free of any concrete
//! `Transform` type (which lives in `ludion-engine`, above this crate). `ludion-engine` implements
//! [`GroundPosition`] for its `Transform`.
//!
//! # Determinism
//!
//! The index is a uniform sparse grid keyed by cell, backed by
//! [`StableMap`](ludion_core::collections::StableMap) (sorted iteration). Queries visit candidates
//! in a **pinned total order** — sorted cell key, then `EntityId` ascending — so results are
//! reproducible across runs and platforms. `k_nearest` orders by *squared* distance (exact in
//! `f32`, no `sqrt`, no last-bit ambiguity), breaking ties by `EntityId`.

mod collider;
mod joint;
mod joint_motor;
mod parent_pose;
mod plane;
mod pose_blend;
pub mod spatial;
pub mod spatial3d;
mod velocity;

pub use collider::{
    AxisLock, Body, Collider, CombineRule, CompoundPart, ContactMaterial, RigidBody, Shape,
};
pub use joint::{Joint, JointKind};
pub use joint_motor::{JointMotor, expected_targets};
pub use parent_pose::{NoParent, ParentPose};
pub use plane::Plane;
pub use pose_blend::{JointPoses, PoseBlend};
pub use spatial::{Entry, RayHit, SpatialIndex};
pub use spatial3d::{SpatialVolume, VolumeShape};
pub use velocity::Velocity;

/// A component that yields a collider's center on the two in-plane axes ([`Plane`]; x/z by default).
///
/// [`Collider`] stores only a shape; where that shape sits comes from a companion component the game
/// already maintains (typically `ludion-engine`'s `Transform`). [`spatial::rebuild`] is generic over
/// this trait so `ludion-spatial` need not name any concrete position type — keeping the crate below
/// `ludion-engine` in the graph. `ludion-engine` implements it for `Transform` as the two
/// [`Plane::axes`] components of `translation` (the off-plane axis, rotation, and scale are ignored
/// — the 2D-sim / 3D-eye pillar).
pub trait GroundPosition {
    /// The collider's center on `plane`'s two in-plane axes.
    fn ground_position(&self, plane: Plane) -> [f32; 2];
}

/// A component that yields an entity's world translation, for the 3D index
/// ([`spatial3d::rebuild`]); the sibling of [`GroundPosition`] without the plane projection.
///
/// `ludion-components` implements it for its `Transform` as `translation`. Under a `Parent` the
/// resolver's composed pose replaces it.
pub trait WorldPosition {
    /// The entity's world translation.
    fn world_position(&self) -> [f32; 3];
}
