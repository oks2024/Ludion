//! The character motor: a scene-authored capsule, its tunables and its engine-written state.

mod component;

pub use component::{CharacterMotor, CharacterMotorState, PlatformPose, height_in_force};
pub(crate) use component::{check, motor_invalid};
