//! The character motor: a scene-authored capsule, its tunables and its engine-written state, and
//! the `move_character` that drives it.

mod batch;
mod carry;
mod component;
mod gather;
mod mover;
mod posture;
mod report;
mod resize;
mod shape;
mod surface;
mod suspend;
mod sweep;
mod turn;

pub use batch::move_characters;
pub use component::{
    CharacterMotor, CharacterMotorState, PlatformPose, Posture, PostureShape, height_in_force,
    motor_in_force,
};
pub(crate) use component::{
    check_in_force, check_postures, in_force, motor_invalid, posture_unknown,
};
pub(crate) use gather::Gather;
pub use mover::{
    MoveConstraint, ground_probe_reach, move_character, move_character_with, probe_character_ground,
};
pub use posture::set_character_posture;
pub use report::{
    GroundHit, GroundProbe, LaneEnd, LaneFrame, LaneReport, MotorError, MoveReport, PlatformCarry,
    PostureReport, ResizeReport, ResumeReport, SurfaceHit, TurnReport,
};
pub use resize::{fits_character, resize_character, teleport_character};
pub use shape::motor_capsule_segment;
pub(crate) use shape::{lying_centre, lying_rotation, yaw_of};
pub use surface::SurfaceVelocity;
pub use suspend::{character_fit, resume_character, suspend_character};
pub use turn::turn_character;
