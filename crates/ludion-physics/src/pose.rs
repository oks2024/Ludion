//! [`PhysicsPose`]: the one seam between a game's position component and the solver.

use ludion_ecs::Component;

/// A game's position component, seen by the solver.
///
/// `ludion-physics` never names a concrete pose type: the engine's `Transform`, a test's stub and a
/// game's own component all reach the solver through this trait, which is why this crate does not
/// depend on `ludion-engine`. A game registers the systems for its own pose component once —
/// [`install::<MyPose, NoParent>`](crate::install) — and the write/mirror pair reads and writes it through
/// these four methods and nothing else.
///
/// # Rotation is a quaternion at this boundary
///
/// `[x, y, z, w]`, not Euler angles, and deliberately: the solver's rotation *is* a quaternion, and
/// every Euler convention question (order, intrinsic or extrinsic, degrees or radians) then lives in
/// exactly one place — the implementor — instead of being re-answered on each side of the mirror.
/// A component that stores Euler angles converts with
/// [`ludion_core::math::euler_xyz_degrees_to_quat`] /
/// [`ludion_core::math::quat_to_euler_xyz_degrees`], which are the engine's convention.
///
/// # The round trip need not be lossless, but it must be stable
///
/// The mirror compares the component against the value it *reads back* after its own write, so an
/// implementor only needs `set` then `get` to be stable: a second `set` of what `get` returned must
/// read back the same bits. An untouched component is then bit-equal to its baseline, and any
/// difference is an edit.
pub trait PhysicsPose: Component {
    /// World-space position, `[x, y, z]`.
    fn translation(&self) -> [f32; 3];
    /// Sets the world-space position.
    fn set_translation(&mut self, translation: [f32; 3]);
    /// World-space orientation as a unit quaternion, `[x, y, z, w]`.
    fn rotation_quat(&self) -> [f32; 4];
    /// Sets the world-space orientation from a unit quaternion, `[x, y, z, w]`.
    fn set_rotation_quat(&mut self, rotation: [f32; 4]);
}
