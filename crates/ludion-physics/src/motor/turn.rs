//! [`turn_character`]: turning a motor in place, tested in steps when it lies.

use core::f32::consts::PI;

use ludion_core::ids::EntityId;
use ludion_core::math::sin_cos;
use ludion_ecs::World;

use super::component::{CharacterMotorState, in_force};
use super::report::{MotorError, TurnReport};
use super::resize::{fit_blocker, motor_of};
use super::shape::yaw_of;
use crate::pose::PhysicsPose;
use crate::world::PhysicsWorld;

/// The largest yaw a lying turn is tested across in one go: 15 degrees.
const TURN_STEP: f32 = 15.0 * PI / 180.0;

/// Turns `entity`'s motor to face `yaw` (radians about +Y; `0` faces local `-Z`) by writing the pose
/// `P`'s rotation as a pure yaw.
///
/// An upright capsule is rotation-symmetric, so it turns at once and nothing is tested
/// ([`TurnReport::blocked_by`] is `None`). A lying capsule sweeps: the turn is split into steps of at
/// most 15 degrees and the capsule is tested at the end of each, shortest way round. The first
/// blocked step stops it: the pose keeps the last clear yaw and `blocked_by` names the blocker with
/// the lowest entity id ([`TurnReport::yaw`] is the yaw written either way). Only the destination of
/// each step is tested, so a thin collider can slip between two. Rigid props overlapping a step block
/// it like static colliders. A turn is a move, not a teleport: no body is placed and the grounded
/// state is kept.
///
/// The checks come in this order: [`MotorError::NoMotor`], [`MotorError::NoPose`],
/// [`MotorError::Suspended`], a non-finite `yaw` ([`MotorError::NonFiniteTeleport`]), and
/// [`MotorError::NoPhysicsWorld`] when a test is needed. Writing the `Transform` yaw directly turns a
/// lying motor too, but untested: the next tick's depenetration resolves any overlap.
///
/// Call it from a system registered **before** [`install`](crate::install)'s; the test reads the
/// state as of the last completed step.
///
/// ```no_run
/// # use ludion_ecs::{SystemCtx, World};
/// # use ludion_physics::{turn_character, PhysicsPose};
/// # use ludion_core::ids::EntityId;
/// fn face_east<P: PhysicsPose>(world: &mut World, _ctx: &SystemCtx, hero: EntityId) {
///     if let Ok(report) = turn_character::<P>(world, hero, -std::f32::consts::FRAC_PI_2) {
///         if report.blocked_by.is_some() { /* a wall is in the way of the prone body */ }
///     }
/// }
/// ```
pub fn turn_character<P: PhysicsPose>(
    world: &mut World,
    entity: EntityId,
    yaw: f32,
) -> Result<TurnReport, MotorError> {
    let (motor, feet) = motor_of::<P>(world, entity)?;
    let rotation = world
        .get::<P>(entity)
        .ok()
        .flatten()
        .map(|pose| pose.rotation_quat())
        .ok_or(MotorError::NoPose(entity))?;
    let state = world.get::<CharacterMotorState>(entity).ok().flatten();
    if state.is_some_and(|state| state.suspended) {
        return Err(MotorError::Suspended(entity));
    }
    if !yaw.is_finite() {
        return Err(MotorError::NonFiniteTeleport { feet, yaw });
    }
    let force = in_force(&motor, state);
    if force.lying.is_none() {
        write_yaw::<P>(world, entity, yaw);
        return Ok(TurnReport {
            yaw,
            blocked_by: None,
        });
    }
    let current = yaw_of(rotation);
    let delta = wrap(yaw - current);
    if delta == 0.0 {
        return Ok(TurnReport {
            yaw: current,
            blocked_by: None,
        });
    }
    let steps = (delta.abs() / TURN_STEP).ceil().max(1.0);
    let physics = world
        .resource::<PhysicsWorld>()
        .ok_or(MotorError::NoPhysicsWorld)?;
    let mut clear = current;
    let mut blocked_by = None;
    for step in 1..=steps as u32 {
        let at = current + delta * step as f32 / steps;
        if let Some(blocker) = fit_blocker(physics, entity, &force, feet, at) {
            blocked_by = Some(blocker);
            break;
        }
        clear = at;
    }
    let written = if blocked_by.is_some() { clear } else { yaw };
    if written != current {
        write_yaw::<P>(world, entity, written);
    }
    Ok(TurnReport {
        yaw: written,
        blocked_by,
    })
}

/// `angle` wrapped into `(-pi, pi]`.
fn wrap(angle: f32) -> f32 {
    let tau = 2.0 * PI;
    let mut wrapped = angle % tau;
    if wrapped > PI {
        wrapped -= tau;
    } else if wrapped <= -PI {
        wrapped += tau;
    }
    wrapped
}

fn write_yaw<P: PhysicsPose>(world: &mut World, entity: EntityId, yaw: f32) {
    let (sin, cos) = sin_cos(yaw / 2.0);
    if let Ok(Some(pose)) = world.get_mut::<P>(entity) {
        pose.set_rotation_quat([0.0, sin, 0.0, cos]);
    }
}
