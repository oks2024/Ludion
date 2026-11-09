//! The character motor: a scene-authored capsule, its tunables and its engine-written state, and
//! the `move_character` that drives it.

mod batch;
mod carry;
mod component;
mod mover;
mod report;
mod resize;
mod surface;
mod sweep;

pub use batch::move_characters;
pub use component::{CharacterMotor, CharacterMotorState, PlatformPose, height_in_force};
pub(crate) use component::{check, motor_invalid};
pub use mover::{
    MoveConstraint, ground_probe_reach, move_character, move_character_with, probe_character_ground,
};
pub use report::{
    GroundHit, GroundProbe, LaneEnd, LaneFrame, LaneReport, MotorError, MoveReport, PlatformCarry,
    ResizeReport, SurfaceHit,
};
pub use resize::{fits_character, resize_character, teleport_character};
pub use surface::SurfaceVelocity;
