//! Camera rigs: the [`CameraRig`] component and the pure pipeline that advances one a tick and
//! returns the camera pose.
//!
//! A rig poses the scene camera from data. [`step_rig`] runs the stages in a fixed order:
//!
//! 1. **snap or switch** — an implicit snap on the first tick (no `state.last_applied`) or after
//!    [`snap_rig`]; a write to `rig` since the last tick starts a timed blend from the current pose;
//! 2. **advance** (gameplay time only) — trauma decay, target velocity, look-ahead, the vertical
//!    anchor, the `track` pose projected onto a path, then the critically damped springs;
//! 3. **blend** — an interpolation from the frozen source pose to the live rig pose;
//! 4. **shake** — seeded trauma noise on the output only; it never feeds back into the state.
//!
//! The crate owns the types and the maths. The engine registers the component, resolves the target
//! and the path (a [`ludion_scene::PathSampler`]), calls [`step_rig`] and writes the result to the
//! camera's `Transform` and `Camera`. [`camera_relative`] maps a stick to world directions from the
//! rig's last posed yaw.
//!
//! Component state is `f32` with transcendentals through [`ludion_core::math`], so a rig's state
//! hashes and replays bit-exact across platforms.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod relative;
mod rig;
mod shake;
mod spring;
mod step;
mod track;
mod validate;

pub use relative::{camera_relative, camera_relative_yaw};
pub use rig::{
    Blend, BlendCurve, BlendIn, CameraRig, LookAhead, Pose, RigConfig, RigMode, RigState,
    ShakeConfig, Springs, TrackRig, VerticalAnchor, add_trauma, snap_rig,
};
pub use step::{RigInput, RigOutput, step_rig};
pub use validate::invalid_field;
