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

/// What [`probe_character_ground`](crate::probe_character_ground) found.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct GroundProbe {
    /// The support, walkable or not; `distance` is from the probed feet (negative: above them).
    pub ground: GroundHit,
    /// The feet height the motor would settle at on it; `None` when it is not walkable.
    pub rest_y: Option<f32>,
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
/// motor was airborne, rode a rigid support, this was not its first call of the tick, or the move
/// was stuck (the carry was not applied). A static
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

/// The lane's frame at the character's `s`: the centreline pose the lane constrains against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaneFrame {
    /// Metres along the path the frame is evaluated at.
    pub s: f32,
    /// The lane point at `s`: the path point moved `offset` metres along `lateral`.
    pub point: [f32; 3],
    /// The path's unit tangent at `s` (not horizontal on a slope).
    pub tangent: [f32; 3],
    /// The horizontal unit direction of travel at `s`.
    pub forward: [f32; 3],
    /// The horizontal unit direction to the right of `forward` (`lateral = forward × up`).
    pub lateral: [f32; 3],
    /// The lane's blend weight at the call, `0..=1` (the eased, not the raw, blend).
    pub weight: f32,
}

/// Which end of an open lane path the character is at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneEnd {
    /// `s` is at the start of the path (`0`).
    Start,
    /// `s` is at the end of the path (its length).
    End,
}

/// What a `LaneConstraint` did to one `move_character` call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaneReport {
    /// Metres along the path after the move.
    pub s: f32,
    /// The change in `s` over the move, in metres.
    pub ds: f32,
    /// The blend weight the move used, `0..=1`.
    pub weight: f32,
    /// Whether the lane is engaged (the component's flag at the call).
    pub engaged: bool,
    /// Whether the weight is still moving toward its target.
    pub easing: bool,
    /// Signed metres from the lane centre line to the feet along `lateral`, after the move.
    pub lateral_error: f32,
    /// Whether a wall stopped the pull toward the lane.
    pub pull_blocked: bool,
    /// The frame at the final `s`.
    pub frame: LaneFrame,
    /// The end of an open path `s` is at or beyond, in either `ends` mode.
    pub at_end: Option<LaneEnd>,
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
    /// The walls the slide hit, one per entity and plane, in order of travel, then entity.
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
    /// The posture in force for the move, `None` for the motor's own shape.
    pub posture: Option<String>,
    /// Present when the entity has a `LaneConstraint` that is engaged or still easing; filled by
    /// the engine's `move_character`.
    pub lane: Option<LaneReport>,
}

/// What one `resize_character` call did.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct ResizeReport {
    /// The capsule height in force after the call.
    pub height: f32,
    /// The lowest-id collider that kept a growth from fitting; the height is unchanged then.
    pub blocked_by: Option<EntityId>,
}

/// What one `set_character_posture` call did.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct PostureReport {
    /// The posture in force after the call (`None` is the motor's own shape).
    pub posture: Option<String>,
    /// The lowest-id collider that kept a growth from fitting; the posture is unchanged then.
    pub blocked_by: Option<EntityId>,
}

/// What one `resume_character` call did.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct ResumeReport {
    /// Whether the motor is back: the spot fit, the pose was written and the motor is live again.
    pub resumed: bool,
    /// The lowest-id collider that kept the capsule from fitting; the motor stays suspended and
    /// the pose unchanged then.
    pub blocked_by: Option<EntityId>,
}

/// Why a motor function did nothing.
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
    /// The requested capsule height is out of range for the motor.
    InvalidHeight {
        /// The entity carrying the motor.
        entity: EntityId,
        /// The requested height.
        height: f32,
        /// The rule it broke.
        reason: String,
    },
    /// The teleport target has a non-finite component.
    NonFiniteTeleport {
        /// The requested feet position.
        feet: [f32; 3],
        /// The requested yaw.
        yaw: f32,
    },
    /// The probed feet position has a non-finite component.
    NonFiniteFeet([f32; 3]),
    /// The entity appears more than once in one `move_characters` batch.
    DuplicateInBatch(EntityId),
    /// The entity has a `LaneConstraint` gated active (even one whose path fails to load), which a
    /// batch does not apply.
    LaneInBatch(EntityId),
    /// The motor is suspended, so it takes no moves or teleports.
    Suspended(EntityId),
    /// `resume_character` was called on a motor that is not suspended.
    NotSuspended(EntityId),
    /// The entity has a `Parent`; a motor resumes only as a root.
    Parented(EntityId),
    /// The posture is not one the motor declares.
    UnknownPosture {
        /// The entity carrying the motor.
        entity: EntityId,
        /// The posture asked for.
        posture: String,
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
            MotorError::InvalidHeight {
                entity,
                height,
                reason,
            } => write!(
                f,
                "height {height} is not valid for the CharacterMotor on {entity:?}: {reason}; \
                 keep the radius and pick a height that is finite, at least 2 × radius + 0.001 \
                 and above step_height"
            ),
            MotorError::NonFiniteTeleport { feet, yaw } => write!(
                f,
                "the teleport target (feet {feet:?}, yaw {yaw}) is not finite"
            ),
            MotorError::NonFiniteFeet(feet) => {
                write!(f, "the probed feet {feet:?} are not finite")
            }
            MotorError::DuplicateInBatch(entity) => write!(
                f,
                "{entity:?} is listed twice in one move_characters batch; merge the displacements"
            ),
            MotorError::LaneInBatch(entity) => write!(
                f,
                "{entity:?} has an active LaneConstraint (a path and a non-zero blend or engaged), which \
                 move_characters does not apply; call move_character for it"
            ),
            MotorError::Suspended(entity) => write!(
                f,
                "the CharacterMotor on {entity:?} is suspended and takes no moves; call \
                 resume_character to bring it back"
            ),
            MotorError::NotSuspended(entity) => write!(
                f,
                "the CharacterMotor on {entity:?} is not suspended; call suspend_character first, \
                 or use teleport_character to place a live motor"
            ),
            MotorError::Parented(entity) => write!(
                f,
                "{entity:?} has a Parent, so its motor cannot resume; reparent it to the root \
                 first (remove Parent), then call resume_character"
            ),
            MotorError::UnknownPosture { entity, posture } => write!(
                f,
                "the CharacterMotor on {entity:?} has no posture {posture:?}; declare it under \
                 `postures` on the motor, or pass None for the motor's own shape"
            ),
        }
    }
}

impl std::error::Error for MotorError {}
