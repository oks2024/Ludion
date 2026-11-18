//! [`set_character_posture`]: switching a motor between its named postures.

use ludion_core::ids::EntityId;
use ludion_ecs::World;

use super::component::{CharacterMotor, CharacterMotorState, check, motor_in_force, posture_motor};
use super::report::{MotorError, PostureReport};
use super::resize::{headroom, motor_of};
use crate::pose::PhysicsPose;
use crate::world::PhysicsWorld;

/// Puts `entity`'s motor in the posture `posture`, one of the names under its `postures`; `None` is
/// the motor's own shape.
///
/// A posture that is taller or wider than the shape in force is first tested against the world with
/// the capsule at the feet: when a collider is inside it, nothing changes and
/// [`PostureReport::blocked_by`] names the blocker with the lowest entity id (the report's `posture`
/// is the one still in force). Otherwise the posture is set and any
/// [`resize_character`](crate::resize_character) height override is cleared, because the posture
/// defines its own shape; the next physics write pass rebuilds the capsule. Setting the posture
/// already in force is a success. The checks come in this order: [`MotorError::NoMotor`],
/// [`MotorError::NoPose`], [`MotorError::Suspended`] ([`resume_character`](crate::resume_character)
/// takes a posture), [`MotorError::UnknownPosture`], [`MotorError::InvalidMotor`] (the posture's
/// effective motor is out of range), and [`MotorError::NoPhysicsWorld`] when a fit test is needed.
///
/// The test reads the state as of the last completed step, so a ceiling despawned in tick N still
/// blocks in tick N and is gone in tick N + 1. Call it from a system registered **before**
/// [`install`](crate::install)'s.
///
/// ```no_run
/// # use ludion_ecs::{SystemCtx, World};
/// # use ludion_physics::{set_character_posture, PhysicsPose};
/// # use ludion_core::ids::EntityId;
/// fn crouch<P: PhysicsPose>(world: &mut World, _ctx: &SystemCtx, hero: EntityId, down: bool) {
///     let posture = down.then_some("crouch");
///     if let Ok(report) = set_character_posture::<P>(world, hero, posture) {
///         if report.blocked_by.is_some() { /* stay crouched under the low ceiling */ }
///     }
/// }
/// ```
pub fn set_character_posture<P: PhysicsPose>(
    world: &mut World,
    entity: EntityId,
    posture: Option<&str>,
) -> Result<PostureReport, MotorError> {
    let (motor, feet) = motor_of::<P>(world, entity)?;
    let state = world.get::<CharacterMotorState>(entity).ok().flatten();
    if state.is_some_and(|state| state.suspended) {
        return Err(MotorError::Suspended(entity));
    }
    let target = match posture {
        Some(name) => posture_motor(&motor, name).ok_or_else(|| MotorError::UnknownPosture {
            entity,
            posture: name.to_owned(),
        })?,
        None => CharacterMotor {
            postures: Default::default(),
            ..motor.clone()
        },
    };
    if let Err((pointer, reason)) = check(&target) {
        return Err(MotorError::InvalidMotor {
            entity,
            reason: format!("{pointer}: {reason}"),
        });
    }
    let in_force = motor_in_force(&motor, state);
    let held = state.and_then(|state| state.posture.clone());
    if target.height > in_force.height || target.radius > in_force.radius {
        let physics = world
            .resource::<PhysicsWorld>()
            .ok_or(MotorError::NoPhysicsWorld)?;
        if let Some(blocker) = headroom(physics, entity, &target, feet, target.height) {
            return Ok(PostureReport {
                posture: held,
                blocked_by: Some(blocker),
            });
        }
    }
    let stored = posture.map(str::to_owned);
    if let Ok(Some(state)) = world.get_mut::<CharacterMotorState>(entity) {
        state.posture = stored.clone();
        state.height = None;
    } else if stored.is_some() {
        let _ = world.insert(
            entity,
            CharacterMotorState {
                posture: stored.clone(),
                ..CharacterMotorState::default()
            },
        );
    }
    Ok(PostureReport {
        posture: stored,
        blocked_by: None,
    })
}
