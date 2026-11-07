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
//! 4. **post-aim** — kick, roll and the field-of-view channels, then seeded trauma shake, on the
//!    output only; none feeds back into the state. [`resolved_camera`] reaches it, for the tick and
//!    for a presenter that latches fresher look.
//!
//! The `first_person` mode skips the path and the springs: the game writes look deltas
//! ([`add_look`]), the rig owns yaw and pitch, and [`CameraRig::aim`] reports the pre-shake aim.
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

mod first_person;
mod relative;
mod rig;
mod shake;
mod spring;
mod step;
mod track;
mod validate;

pub use first_person::first_person_eye;
pub use relative::{camera_relative, camera_relative_yaw};
pub use rig::{
    Aim, Blend, BlendCurve, BlendIn, CameraRig, FirstPersonRig, KickConfig, LookAhead, Pose,
    RigConfig, RigMode, RigState, ShakeConfig, Springs, TrackRig, VerticalAnchor, add_fov_kick,
    add_kick, add_look, add_look_counts, add_trauma, look_from_counts, set_look, snap_rig,
};
pub use step::{ResolvedCamera, RigInput, RigOutput, resolved_camera, step_rig};
pub use track::{rig_focus, track_eye};
pub use validate::{invalid_config, invalid_field};
