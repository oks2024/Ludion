//! [`MoveReport`] and [`MotorError`]: what one `move_character` call did, or why it did nothing.

use std::fmt;

use ludion_core::ids::EntityId;

/// The walkable-or-not surface a motor stands on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundHit {
    /// The entity whose collider is underfoot.
    pub entity: EntityId,
    /// World-space contact point.
    pub point: [f32; 3],
    /// World-space surface normal, pointing out of the ground. At a ledge rim it is the walkable
    /// face under the feet when one exists within the radius, not the edge's interpolated normal.
    pub normal: [f32; 3],
    /// Whether the normal is within the motor's slope limit.
    pub walkable: bool,
    /// The cast distance from the final pose down to the surface, in metres.
    pub distance: f32,
}

/// A wall or ceiling the move was blocked by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceHit {
    /// The entity whose collider blocked the move.
    pub entity: EntityId,
    /// World-space contact point.
    pub point: [f32; 3],
    /// World-space surface normal, pointing out of the obstacle.
    pub normal: [f32; 3],
}

/// What moving ground did to the motor this tick. Default (no `entity`) when no carry ran: the
/// motor was airborne, rode a rigid support, or this was not its first call of the tick. A static
/// floor reports its entity with zeros. The values are the intended carry, even when a wall
/// blocked it; a game's takeoff momentum is `(translation + surface) / dt`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlatformCarry {
    /// The platform that carried the motor.
    pub entity: Option<EntityId>,
    /// The translation the carry applied, in metres.
    pub translation: [f32; 3],
    /// The yaw the carry applied, in radians about +Y.
    pub yaw: f32,
    /// The displacement the platform's surface velocity added this tick, in metres: its
    /// `SurfaceVelocity` turned by the platform's rotation, times the gameplay `dt`.
    pub surface: [f32; 3],
}

/// What one `move_character` call did.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct MoveReport {
    /// Whether the motor stands on walkable ground at the end of the move.
    pub grounded: bool,
    /// Whether this move started airborne and ended grounded.
    pub landed: bool,
    /// The support found by the ground probe, walkable or not.
    pub ground: Option<GroundHit>,
    /// The first ceiling an upward motion hit.
    pub ceiling: Option<SurfaceHit>,
    /// The walls the slide hit, one per entity, in order of travel.
    pub walls: Vec<SurfaceHit>,
    /// The height climbed by a step-up, when one happened.
    pub stepped: Option<f32>,
    /// What platform carry did this tick.
    pub platform: PlatformCarry,
    /// The feet's total movement, depenetration included.
    pub moved: [f32; 3],
    /// Whether depenetration could not clear the capsule: it is crushed.
    pub stuck: bool,
    /// The capsule height the move used.
    pub height: f32,
}

/// Why `move_character` did nothing.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum MotorError {
    /// The entity has no `CharacterMotor`.
    NoMotor(EntityId),
    /// The entity has no pose component of the type the call names.
    NoPose(EntityId),
    /// No physics body has been written yet, which is the case on a world's first tick.
    NoPhysicsWorld,
    /// The displacement has a non-finite component.
    NonFiniteDisplacement([f32; 3]),
    /// The motor's fields are out of range (`L1294` at document time).
    InvalidMotor {
        /// The entity carrying the motor.
        entity: EntityId,
        /// The field and the range it broke.
        reason: String,
    },
}

impl fmt::Display for MotorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MotorError::NoMotor(entity) => {
                write!(f, "{entity:?} has no CharacterMotor; add one to move it")
            }
            MotorError::NoPose(entity) => write!(
                f,
                "{entity:?} has no pose component of the type passed to move_character"
            ),
            MotorError::NoPhysicsWorld => write!(
                f,
                "the physics world does not exist yet (it is created by the first write pass); \
                 call move_character from the second tick on"
            ),
            MotorError::NonFiniteDisplacement(displacement) => {
                write!(f, "the displacement {displacement:?} is not finite")
            }
            MotorError::InvalidMotor { entity, reason } => {
                write!(f, "the CharacterMotor on {entity:?} is invalid: {reason}")
            }
        }
    }
}

impl std::error::Error for MotorError {}
