//! The character motor: a scene-authored capsule, its tunables and its engine-written state, and
//! the `move_character` that drives it.

mod component;
mod mover;
mod report;
mod sweep;

pub use component::{CharacterMotor, CharacterMotorState, PlatformPose, height_in_force};
pub(crate) use component::{check, motor_invalid};
pub use mover::{SlideConstraint, move_character, move_character_with};
pub use report::{GroundHit, MotorError, MoveReport, PlatformCarry, SurfaceHit};
