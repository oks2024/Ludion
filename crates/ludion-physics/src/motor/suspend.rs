//! [`suspend_character`], [`character_fit`] and [`resume_character`]: parking a motor (no body, no
//! moves) and bringing it back at a tested spot.

use ludion_core::ids::EntityId;
use ludion_ecs::{Parent, World};

use super::component::{CharacterMotorState, check_in_force, in_force, posture_motor};
use super::report::{MotorError, ResumeReport};
use super::resize::{fit_blocker, motor_of, place};
use super::shape::InForce;
use crate::pose::PhysicsPose;
use crate::world::PhysicsWorld;

/// Parks `entity`'s motor: it keeps its component and state but loses its physics body at the next
/// write pass, and every move or teleport is [`MotorError::Suspended`] until
/// [`resume_character`]. Use it while something else owns the entity: a ragdoll, a parent, a carry.
///
/// The grounded record, the ground, the platform and the carry tick are cleared; the capsule height
/// in force is kept. It is idempotent and inserts the state when the entity has none.
/// [`resize_character`](crate::resize_character), [`fits_character`](crate::fits_character) and
/// [`probe_character_ground`](crate::probe_character_ground) keep working on a suspended motor.
///
/// Call it from a system registered **before** [`install`](crate::install)'s.
///
/// ```no_run
/// # use ludion_ecs::{SystemCtx, World};
/// # use ludion_physics::suspend_character;
/// # use ludion_core::ids::EntityId;
/// fn knock_out(world: &mut World, _ctx: &SystemCtx, guard: EntityId) {
///     let _ = suspend_character(world, guard);
/// }
/// ```
pub fn suspend_character(world: &mut World, entity: EntityId) -> Result<(), MotorError> {
    if world
        .get::<super::CharacterMotor>(entity)
        .ok()
        .flatten()
        .is_none()
    {
        return Err(MotorError::NoMotor(entity));
    }
    let park = |state: &mut CharacterMotorState| {
        state.suspended = true;
        state.grounded = false;
        state.ground = None;
        state.platform = None;
        state.carry_tick = None;
    };
    if let Ok(Some(state)) = world.get_mut::<CharacterMotorState>(entity) {
        park(state);
    } else {
        let mut state = CharacterMotorState::default();
        park(&mut state);
        let _ = world.insert(entity, state);
    }
    Ok(())
}

/// The motor a fit or resume tests: the named posture's, which must be declared; `None` is the
/// base shape, wearing the height override only while no posture is in force.
fn target<P: PhysicsPose>(
    world: &World,
    entity: EntityId,
    posture: Option<&str>,
) -> Result<InForce, MotorError> {
    let (motor, _) = motor_of::<P>(world, entity)?;
    match posture {
        Some(name) => posture_motor(&motor, name).ok_or_else(|| MotorError::UnknownPosture {
            entity,
            posture: name.to_owned(),
        }),
        None => {
            let state = world.get::<CharacterMotorState>(entity).ok().flatten();
            let base = CharacterMotorState {
                posture: None,
                height: state
                    .filter(|state| state.posture.is_none())
                    .and_then(|state| state.height),
                ..CharacterMotorState::default()
            };
            Ok(in_force(&motor, Some(&base)))
        }
    }
}

/// The shared checks of [`character_fit`] and [`resume_character`] after the pose and posture
/// ones: the target motor valid, the physics world present, then the overlap test.
fn blocker(
    world: &World,
    entity: EntityId,
    feet: [f32; 3],
    yaw: f32,
    resting: &InForce,
) -> Result<Option<EntityId>, MotorError> {
    if let Err((pointer, reason)) = check_in_force(resting) {
        return Err(MotorError::InvalidMotor {
            entity,
            reason: format!("{pointer}: {reason}"),
        });
    }
    let physics = world
        .resource::<PhysicsWorld>()
        .ok_or(MotorError::NoPhysicsWorld)?;
    Ok(fit_blocker(physics, entity, resting, feet, yaw))
}

/// Whether `entity`'s capsule fits standing on `feet`: the lowest-id collider in the way, `None`
/// when it fits. It is the test [`resume_character`] runs, so a game can try several candidate
/// spots without changing anything.
///
/// `posture` is `None` for the motor's own shape (at the height override, while no posture is in
/// force) or the name of one of its `postures`, tested without a height override; a name the motor
/// does not declare is [`MotorError::UnknownPosture`]. A lying shape is tested at `yaw`, the facing the
/// motor would have there. Works on a live or suspended motor.
///
/// ```no_run
/// # use ludion_ecs::{SystemCtx, World};
/// # use ludion_physics::{character_fit, PhysicsPose};
/// # use ludion_core::ids::EntityId;
/// fn first_free<P: PhysicsPose>(world: &World, _ctx: &SystemCtx, guard: EntityId) -> Option<[f32; 3]> {
///     [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0]]
///         .into_iter()
///         .find(|spot| matches!(character_fit::<P>(world, guard, None, *spot, 0.0), Ok(None)))
/// }
/// ```
pub fn character_fit<P: PhysicsPose>(
    world: &World,
    entity: EntityId,
    posture: Option<&str>,
    feet: [f32; 3],
    yaw: f32,
) -> Result<Option<EntityId>, MotorError> {
    let _ = motor_of::<P>(world, entity)?;
    if !feet.iter().all(|c| c.is_finite()) {
        return Err(MotorError::NonFiniteFeet(feet));
    }
    let resting = target::<P>(world, entity, posture)?;
    if resting.lying.is_some() && !yaw.is_finite() {
        return Err(MotorError::NonFiniteTeleport { feet, yaw });
    }
    blocker(world, entity, feet, yaw, &resting)
}

/// Brings a suspended motor back with its feet at `feet`, facing `yaw` (radians about +Y), when the
/// capsule fits there.
///
/// A blocker makes this `Ok(ResumeReport { resumed: false, blocked_by })` with the motor still
/// suspended and the pose unchanged; try another spot. A fit writes the pose, clears the grounded
/// state like [`teleport_character`](crate::teleport_character), and places the capsule without pushing props. A motor under a `Parent` is
/// [`MotorError::Parented`]: reparent it to the root first; one that is not suspended is
/// [`MotorError::NotSuspended`]. `posture` is the shape to wake in: `Some(name)` tests that
/// declared posture and sets it (the height override is cleared); `None` tests the motor's own
/// shape and keeps a height override only if no posture was in force. `yaw` has no effect on the fit of an upright capsule; a lying one is tested at it.
///
/// Call it from a system registered **before** [`install`](crate::install)'s.
///
/// ```no_run
/// # use ludion_ecs::{SystemCtx, World};
/// # use ludion_physics::{resume_character, PhysicsPose};
/// # use ludion_core::ids::EntityId;
/// fn wake<P: PhysicsPose>(world: &mut World, _ctx: &SystemCtx, guard: EntityId) {
///     if let Ok(report) = resume_character::<P>(world, guard, [0.0, 0.0, 0.0], 0.0, None) {
///         if !report.resumed { /* report.blocked_by names the wall; try another spot */ }
///     }
/// }
/// ```
pub fn resume_character<P: PhysicsPose>(
    world: &mut World,
    entity: EntityId,
    feet: [f32; 3],
    yaw: f32,
    posture: Option<&str>,
) -> Result<ResumeReport, MotorError> {
    let _ = motor_of::<P>(world, entity)?;
    if !feet.iter().all(|c| c.is_finite()) || !yaw.is_finite() {
        return Err(MotorError::NonFiniteTeleport { feet, yaw });
    }
    let resting = target::<P>(world, entity, posture)?;
    if world.has::<Parent>(entity) {
        return Err(MotorError::Parented(entity));
    }
    let suspended = world
        .get::<CharacterMotorState>(entity)
        .ok()
        .flatten()
        .is_some_and(|state| state.suspended);
    if !suspended {
        return Err(MotorError::NotSuspended(entity));
    }
    if let Some(blocker) = blocker(world, entity, feet, yaw, &resting)? {
        return Ok(ResumeReport {
            resumed: false,
            blocked_by: Some(blocker),
        });
    }
    place::<P>(world, entity, feet, yaw, false);
    if let Ok(Some(state)) = world.get_mut::<CharacterMotorState>(entity) {
        state.suspended = false;
        match posture {
            Some(name) => {
                state.posture = Some(name.to_owned());
                state.height = None;
            }
            None => {
                if state.posture.take().is_some() {
                    state.height = None;
                }
            }
        }
    }
    Ok(ResumeReport {
        resumed: true,
        blocked_by: None,
    })
}
