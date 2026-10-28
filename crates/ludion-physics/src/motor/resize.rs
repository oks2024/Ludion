//! [`resize_character`], [`fits_character`] and [`teleport_character`]: the motor's crouch and its
//! placement.

use ludion_core::ids::EntityId;
use ludion_core::math::sin_cos;
use ludion_ecs::World;
use ludion_spatial::Shape;

use super::component::{CharacterMotor, CharacterMotorState, check, height_in_force};
use super::mover::motor_filter;
use super::report::{MotorError, ResizeReport};
use crate::pose::PhysicsPose;
use crate::systems::teleport_kinematic;
use crate::world::PhysicsWorld;

/// A motor, its feet and the height in force, after the checks every function here shares.
struct Checked {
    motor: CharacterMotor,
    feet: [f32; 3],
    current: f32,
}

fn motor_of<P: PhysicsPose>(
    world: &World,
    entity: EntityId,
) -> Result<(CharacterMotor, [f32; 3]), MotorError> {
    let motor = world
        .get::<CharacterMotor>(entity)
        .ok()
        .flatten()
        .cloned()
        .ok_or(MotorError::NoMotor(entity))?;
    let feet = world
        .get::<P>(entity)
        .ok()
        .flatten()
        .map(|pose| pose.translation())
        .ok_or(MotorError::NoPose(entity))?;
    Ok((motor, feet))
}

fn validate(entity: EntityId, motor: &CharacterMotor, height: f32) -> Result<(), MotorError> {
    if let Err((pointer, reason)) = check(motor) {
        return Err(MotorError::InvalidMotor {
            entity,
            reason: format!("{pointer}: {reason}"),
        });
    }
    let resized = CharacterMotor {
        height,
        ..motor.clone()
    };
    check(&resized).map_err(|(_, reason)| MotorError::InvalidHeight {
        entity,
        height,
        reason,
    })
}

fn checked<P: PhysicsPose>(
    world: &World,
    entity: EntityId,
    height: f32,
) -> Result<Checked, MotorError> {
    let (motor, feet) = motor_of::<P>(world, entity)?;
    if world.resource::<PhysicsWorld>().is_none() {
        return Err(MotorError::NoPhysicsWorld);
    }
    validate(entity, &motor, height)?;
    let current = height_in_force(
        &motor,
        world.get::<CharacterMotorState>(entity).ok().flatten(),
    );
    Ok(Checked {
        motor,
        feet,
        current,
    })
}

/// The lowest-id collider overlapping a capsule of height `h` standing on `feet`, `None` when it
/// fits. The test capsule is lifted by the skin because a grounded motor rests a skin above its
/// floor: the floor never blocks, while a skin of clearance at the top is still required.
fn headroom(
    physics: &PhysicsWorld,
    entity: EntityId,
    motor: &CharacterMotor,
    feet: [f32; 3],
    h: f32,
) -> Option<EntityId> {
    let shape = Shape::Capsule {
        half_height: motor.half_height_for(h),
        radius: motor.radius,
    };
    let centre = [feet[0], feet[1] + motor.skin_width + h / 2.0, feet[2]];
    physics
        .overlap(
            &shape,
            centre,
            [0.0, 0.0, 0.0, 1.0],
            &motor_filter(physics, entity, motor),
        )
        .unwrap_or_default()
        .into_iter()
        .min()
}

/// Sets the height in force of `entity`'s motor capsule to `height`, metres, feet to crown.
///
/// A height at or below the current one always succeeds and leaves the feet where they are (the pose
/// `P` is not written). A taller one is first tested against the world: when a collider is inside
/// the taller capsule the height stays, and [`ResizeReport::blocked_by`] names the blocker with the
/// lowest entity id. The solver capsule is rebuilt by the next physics write pass. A height that
/// would make the motor invalid (below `2 * radius + 0.001`, not finite, or not above
/// `step_height`) is [`MotorError::InvalidHeight`]; growing past the authored `height` is allowed.
///
/// The test reads the state as of the last completed step, so a ceiling despawned in tick N still
/// blocks in tick N and is gone in tick N + 1.
///
/// Call it from a system registered **before** [`install`](crate::install)'s.
///
/// ```no_run
/// # use ludion_ecs::{SystemCtx, World};
/// # use ludion_physics::{resize_character, PhysicsPose};
/// # use ludion_core::ids::EntityId;
/// fn crouch<P: PhysicsPose>(world: &mut World, _ctx: &SystemCtx, hero: EntityId, down: bool) {
///     let height = if down { 0.6 } else { 0.9 };
///     if let Ok(report) = resize_character::<P>(world, hero, height) {
///         if report.blocked_by.is_some() { /* stay crouched under the low ceiling */ }
///     }
/// }
/// ```
pub fn resize_character<P: PhysicsPose>(
    world: &mut World,
    entity: EntityId,
    height: f32,
) -> Result<ResizeReport, MotorError> {
    let Checked {
        motor,
        feet,
        current,
    } = checked::<P>(world, entity, height)?;
    if height > current {
        let blocker = world
            .resource::<PhysicsWorld>()
            .and_then(|physics| headroom(physics, entity, &motor, feet, height));
        if blocker.is_some() {
            return Ok(ResizeReport {
                height: current,
                blocked_by: blocker,
            });
        }
    }
    let stored = (height.to_bits() != motor.height.to_bits()).then_some(height);
    if let Ok(Some(state)) = world.get_mut::<CharacterMotorState>(entity) {
        state.height = stored;
    } else {
        let _ = world.insert(
            entity,
            CharacterMotorState {
                height: stored,
                ..CharacterMotorState::default()
            },
        );
    }
    Ok(ResizeReport {
        height,
        blocked_by: None,
    })
}

/// Whether a capsule of `height` fits at `entity`'s feet: the test [`resize_character`] runs before
/// it grows, without changing anything. A height at or below the current one fits.
///
/// ```no_run
/// # use ludion_ecs::{SystemCtx, World};
/// # use ludion_physics::{fits_character, PhysicsPose};
/// # use ludion_core::ids::EntityId;
/// fn can_stand<P: PhysicsPose>(world: &mut World, _ctx: &SystemCtx, hero: EntityId) -> bool {
///     fits_character::<P>(world, hero, 0.9).unwrap_or(false)
/// }
/// ```
pub fn fits_character<P: PhysicsPose>(
    world: &World,
    entity: EntityId,
    height: f32,
) -> Result<bool, MotorError> {
    let Checked {
        motor,
        feet,
        current,
    } = checked::<P>(world, entity, height)?;
    if height <= current {
        return Ok(true);
    }
    Ok(world
        .resource::<PhysicsWorld>()
        .is_none_or(|physics| headroom(physics, entity, &motor, feet, height).is_none()))
}

/// Places `entity`'s feet at `feet` facing `yaw` (radians about +Y) without driving the capsule: no
/// prop is pushed on the way, and nothing is swept.
///
/// A plain `Transform` write is a move, not a teleport: it drives the capsule and pushes props on
/// the way. This clears the grounded state, the ground and the platform (no carry next tick), and
/// restores the authored height when the motor was crouched and the authored height fits at the
/// new spot; otherwise the crouch stays and the next move's `report.height` says so. The pose
/// alone is written when the world has no physics yet.
///
/// Call it from a system registered **before** [`install`](crate::install)'s; a snapshot does not
/// carry a queued teleport.
///
/// ```no_run
/// # use ludion_ecs::{SystemCtx, World};
/// # use ludion_physics::{teleport_character, PhysicsPose};
/// # use ludion_core::ids::EntityId;
/// fn respawn<P: PhysicsPose>(world: &mut World, _ctx: &SystemCtx, hero: EntityId) {
///     let _ = teleport_character::<P>(world, hero, [0.0, 0.5, 0.0], 0.0);
/// }
/// ```
pub fn teleport_character<P: PhysicsPose>(
    world: &mut World,
    entity: EntityId,
    feet: [f32; 3],
    yaw: f32,
) -> Result<(), MotorError> {
    if !feet.iter().all(|c| c.is_finite()) || !yaw.is_finite() {
        return Err(MotorError::NonFiniteTeleport { feet, yaw });
    }
    let (motor, _) = motor_of::<P>(world, entity)?;
    let state = world.get::<CharacterMotorState>(entity).ok().flatten();
    let crouched = height_in_force(&motor, state) < motor.height;
    let restores = crouched
        && check(&motor).is_ok()
        && world
            .resource::<PhysicsWorld>()
            .is_some_and(|physics| headroom(physics, entity, &motor, feet, motor.height).is_none());
    let (s, c) = sin_cos(yaw / 2.0);
    if let Ok(Some(pose)) = world.get_mut::<P>(entity) {
        pose.set_translation(feet);
        pose.set_rotation_quat([0.0, s, 0.0, c]);
    }
    let reset = |state: &mut CharacterMotorState| {
        state.grounded = false;
        state.ground = None;
        state.platform = None;
        state.carry_tick = None;
        if restores {
            state.height = None;
        }
    };
    if let Ok(Some(state)) = world.get_mut::<CharacterMotorState>(entity) {
        reset(state);
    } else {
        let mut state = CharacterMotorState::default();
        reset(&mut state);
        let _ = world.insert(entity, state);
    }
    teleport_kinematic(world, entity);
    Ok(())
}
