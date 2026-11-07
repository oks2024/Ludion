//! The authored and engine-written types of a camera rig.

use ludion_core::ids::EntityId;
use ludion_core::math::{Orbit, sin_cos};
use ludion_core::time::FrameIndex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::spring::wrap180;

const DEG_TO_RAD: f32 = core::f32::consts::PI / 180.0;

fn frame_zero() -> FrameIndex {
    FrameIndex::ZERO
}

fn look_is_zero(look: &[f32; 2]) -> bool {
    *look == [0.0; 2]
}

/// The rig on the camera entity. `rig` is the authored configuration (writing a different value
/// starts a timed blend), `shake` survives switches, `trauma`, `grounded` and `look_deg` are
/// game-written inputs, `state` is engine-written. Every field defaults, so older documents keep loading.
///
/// # Example
///
/// ```
/// use ludion_camera::{CameraRig, RigMode};
///
/// let rig: CameraRig = serde_json::from_value(serde_json::json!({
///     "rig": { "mode": { "track": { "path": "paths/behind-1.path.yaml", "offset": [0.0, 2.2, 4.5] } } },
///     "trauma": 0.25,
/// }))
/// .unwrap();
/// let RigMode::Track(track) = &rig.rig.mode else { unreachable!() };
/// assert_eq!(track.offset, [0.0, 2.2, 4.5]);
/// assert_eq!(rig.rig.springs.position_half_life, 0.15);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct CameraRig {
    /// The entity the rig follows; `None` holds the last pose.
    #[schemars(with = "Option<ludion_ecs::EntityRefSchema>")]
    pub target: Option<EntityId>,
    /// The authored part; a write that differs from the last applied value starts a blend.
    pub rig: RigConfig,
    /// The trauma shake; not part of `rig`, so a switch never blends or resets it.
    pub shake: ShakeConfig,
    /// Shake strength in `[0, 1]`; add through [`add_trauma`], decays on gameplay time.
    pub trauma: f32,
    /// Game-written hint: whether the target stands on ground (steers the vertical anchor).
    pub grounded: bool,
    /// First-person look input `[yaw, pitch]` in degrees (positive yaw turns left, positive pitch
    /// looks down). The game writes it, usually through [`add_look`]; the rig applies and zeroes it
    /// on each tick it poses.
    #[serde(skip_serializing_if = "look_is_zero")]
    pub look_deg: [f32; 2],
    /// Engine-written state: springs, projection, blend and shake clock.
    pub state: RigState,
}

impl Default for CameraRig {
    fn default() -> Self {
        Self {
            target: None,
            rig: RigConfig::default(),
            shake: ShakeConfig::default(),
            trauma: 0.0,
            grounded: true,
            look_deg: [0.0; 2],
            state: RigState::default(),
        }
    }
}

impl CameraRig {
    /// The yaw the rig last posed at, in degrees, before shake; `0.0` before the first pose.
    /// [`camera_relative`](crate::camera_relative) reads it.
    pub fn yaw_deg(&self) -> f32 {
        self.state.pose.as_ref().map_or(0.0, |pose| pose.yaw_deg)
    }

    /// The eye and orientation the rig last posed, before shake and any other post-aim effect;
    /// `None` before the first pose. Valid in every mode. It is the one source of truth for aim
    /// rays and anything that must follow where the player looks; it lags a game system by one tick,
    /// like [`camera_relative`](crate::camera_relative).
    pub fn aim(&self) -> Option<Aim> {
        self.state.pose.as_ref().map(|pose| Aim {
            eye: pose.eye,
            yaw_deg: pose.yaw_deg,
            pitch_deg: pose.pitch_deg,
        })
    }
}

/// The resolved pre-shake eye and orientation of a rig ([`CameraRig::aim`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aim {
    /// Eye position in world space.
    pub eye: [f32; 3],
    /// Degrees about +Y from +Z, wrapped to `(-180, 180]`; yaw 0 looks along -Z.
    pub yaw_deg: f32,
    /// Degrees looking down.
    pub pitch_deg: f32,
}

impl Aim {
    /// The unit look direction.
    ///
    /// # Example
    ///
    /// ```
    /// use ludion_camera::Aim;
    ///
    /// let aim = Aim { eye: [0.0; 3], yaw_deg: 0.0, pitch_deg: 0.0 };
    /// assert_eq!(aim.forward(), [0.0, 0.0, -1.0]);
    /// ```
    #[must_use]
    pub fn forward(&self) -> [f32; 3] {
        let (sy, cy) = sin_cos(self.yaw_deg * DEG_TO_RAD);
        let (sp, cp) = sin_cos(self.pitch_deg * DEG_TO_RAD);
        [-sy * cp, -sp, -cy * cp]
    }

    /// The camera `Transform` rotation (Euler XYZ degrees) for this aim.
    #[must_use]
    pub fn rotation_deg(&self) -> [f32; 3] {
        Orbit {
            target: self.eye,
            azimuth_deg: self.yaw_deg,
            pitch_deg: self.pitch_deg,
            distance: 1.0,
        }
        .rotation_deg()
    }
}

/// Cuts the next tick to the rig's desired pose: a global path search, springs, look-ahead and the
/// anchor at rest, any blend dropped. Use it for respawns and teleports.
pub fn snap_rig(rig: &mut CameraRig) {
    rig.state.snap = true;
}

/// Adds a look delta (degrees) to the rig's pending [`CameraRig::look_deg`]; a non-finite component
/// is ignored.
pub fn add_look(rig: &mut CameraRig, yaw_deg: f32, pitch_deg: f32) {
    if yaw_deg.is_finite() {
        rig.look_deg[0] += yaw_deg;
    }
    if pitch_deg.is_finite() {
        rig.look_deg[1] += pitch_deg;
    }
}

/// Sets the first-person facing outright (a spawn or respawn): the yaw wrapped, the pitch clamped
/// to the mode's limit on the next tick, any pending look dropped. A non-finite value is ignored.
pub fn set_look(rig: &mut CameraRig, yaw_deg: f32, pitch_deg: f32) {
    if yaw_deg.is_finite() && pitch_deg.is_finite() {
        rig.state.yaw_deg = wrap180(yaw_deg);
        rig.state.pitch_deg = pitch_deg;
        rig.look_deg = [0.0; 2];
    }
}

/// Adds `amount` to the rig's trauma, clamped to `[0, 1]`; a non-finite amount is ignored.
pub fn add_trauma(rig: &mut CameraRig, amount: f32) {
    if amount.is_finite() {
        rig.trauma = (rig.trauma + amount).clamp(0.0, 1.0);
    }
}

/// The authored configuration of a rig. A game component can embed one (a zone's rig) and copy it
/// into [`CameraRig::rig`] to switch.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct RigConfig {
    /// How the camera is placed.
    pub mode: RigMode,
    /// The look point relative to the target origin, in metres.
    pub focus_offset: [f32; 3],
    /// The position and rotation springs.
    pub springs: Springs,
    /// The focus shift toward the direction of travel.
    pub look_ahead: LookAhead,
    /// How the camera's height follows the target through jumps.
    pub vertical_anchor: VerticalAnchor,
    /// Vertical field of view in degrees; `None` leaves the camera's projection alone.
    pub fov_y_deg: Option<f32>,
    /// The blend used when this configuration becomes the active one.
    pub blend_in: BlendIn,
}

/// The placement mode of a rig. Externally tagged: `mode: { track: { .. } }`. Further modes
/// (`chase`, `follow`) join as variants.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RigMode {
    /// Ride a level-authored path: the target is projected onto it and the camera sits at an offset
    /// in the path's frame.
    Track(TrackRig),
    /// The eye rides the target rigidly; yaw and pitch are game-written look deltas on
    /// [`CameraRig::look_deg`], unsmoothed.
    FirstPerson(FirstPersonRig),
}

impl Default for RigMode {
    fn default() -> Self {
        Self::Track(TrackRig::default())
    }
}

/// The `track` mode.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct TrackRig {
    /// Content-relative `kind: path` file; empty is inert (the rig holds its pose).
    #[schemars(with = "ludion_ecs::AssetPathSchema")]
    pub path: String,
    /// The eye offset in the path frame, in metres: `+x` right, `+y` up, `+z` back (against the
    /// tangent).
    pub offset: [f32; 3],
    /// Metres of arc length either side of the last projection searched each tick.
    pub search_window: f32,
}

impl Default for TrackRig {
    fn default() -> Self {
        Self {
            path: String::new(),
            offset: [0.0, 2.2, 4.5],
            search_window: 3.0,
        }
    }
}

/// The `first_person` mode.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct FirstPersonRig {
    /// The eye offset from the target origin in the yaw frame (not pitched), in metres: `+x`
    /// right, `+y` up, `+z` back.
    pub eye_offset: [f32; 3],
    /// The pitch clamp in degrees either way; in `(0, 90)`.
    pub pitch_limit_deg: f32,
}

impl Default for FirstPersonRig {
    fn default() -> Self {
        Self {
            eye_offset: [0.0, 1.6, 0.0],
            pitch_limit_deg: 89.0,
        }
    }
}

/// Critically damped spring half-lives, in gameplay seconds (`0` is rigid).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Springs {
    /// Seconds for the eye position error to halve.
    pub position_half_life: f32,
    /// Seconds for the yaw and pitch error to halve.
    pub rotation_half_life: f32,
}

impl Default for Springs {
    fn default() -> Self {
        Self {
            position_half_life: 0.15,
            rotation_half_life: 0.10,
        }
    }
}

/// Focus shift toward the target's horizontal direction of travel. It moves the focus only, never
/// the path projection.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct LookAhead {
    /// Maximum horizontal shift of the focus, in metres.
    pub distance: f32,
    /// Horizontal target speed in metres per second at which the full distance applies.
    pub full_speed: f32,
    /// Seconds for the shift to halve toward its goal.
    pub half_life: f32,
}

impl Default for LookAhead {
    fn default() -> Self {
        Self {
            distance: 1.0,
            full_speed: 6.0,
            half_life: 0.3,
        }
    }
}

/// The vertical anchor: while grounded it follows the target's height; airborne it holds until the
/// target leaves the slack.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct VerticalAnchor {
    /// Metres the target may rise above the anchor while airborne before the anchor follows.
    pub up_slack: f32,
    /// Metres the target may drop below the anchor while airborne before the anchor follows.
    pub down_slack: f32,
    /// Seconds for the anchor to halve toward its goal.
    pub half_life: f32,
}

impl Default for VerticalAnchor {
    fn default() -> Self {
        Self {
            up_slack: 1.5,
            down_slack: 1.0,
            half_life: 0.25,
        }
    }
}

/// The blend a configuration plays when it becomes active.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct BlendIn {
    /// Gameplay seconds the blend takes; `0` cuts.
    pub seconds: f32,
    /// The easing of the blend.
    pub curve: BlendCurve,
}

impl Default for BlendIn {
    fn default() -> Self {
        Self {
            seconds: 1.0,
            curve: BlendCurve::EaseInOut,
        }
    }
}

/// The easing of a blend.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BlendCurve {
    /// Constant rate.
    Linear,
    /// Smoothstep: slow in, slow out.
    #[default]
    EaseInOut,
}

/// The trauma shake, applied to the output pose only.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ShakeConfig {
    /// Maximum camera-local offset at full strength, in metres.
    pub max_offset: [f32; 3],
    /// Maximum rotation at full strength in degrees: pitch, yaw, roll.
    pub max_angle_deg: [f32; 3],
    /// Noise frequency in Hz.
    pub frequency: f32,
    /// Trauma lost per gameplay second.
    pub decay_per_second: f32,
    /// Strength is `trauma^exponent`.
    pub exponent: f32,
    /// Seed of the noise phases.
    pub seed: u64,
}

impl Default for ShakeConfig {
    fn default() -> Self {
        Self {
            max_offset: [0.15, 0.15, 0.0],
            max_angle_deg: [1.5, 1.5, 2.0],
            frequency: 18.0,
            decay_per_second: 1.2,
            exponent: 2.0,
            seed: 1,
        }
    }
}

/// A camera pose before shake: eye, yaw and pitch (degrees, [`ludion_core::math::Orbit`]
/// convention) and the optional field of view.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Pose {
    /// Eye position in world space.
    pub eye: [f32; 3],
    /// Degrees about +Y from +Z, wrapped to `(-180, 180]`.
    pub yaw_deg: f32,
    /// Degrees looking down.
    pub pitch_deg: f32,
    /// Vertical field of view in degrees, when the rig sets one.
    pub fov_y_deg: Option<f32>,
}

/// A running blend: the frozen source pose and the clock it plays against.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Blend {
    /// The pose the camera had when the new configuration took over.
    pub from: Pose,
    /// The signed yaw arc in degrees chosen when the blend began; keeps the turn continuous when the
    /// live yaw drifts across the ±180° seam. Absent: the shortest arc each tick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yaw_span: Option<f32>,
    /// The gameplay frame the blend began at.
    #[serde(default = "frame_zero")]
    #[schemars(with = "u64")]
    pub started: FrameIndex,
    /// Blend length in seconds.
    pub seconds: f32,
    /// The easing.
    pub curve: BlendCurve,
}

/// Engine-written rig state; hashed, snapshotted and replayed with the component. A state without
/// `last_applied` snaps on its first tick.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct RigState {
    /// The configuration the last tick applied; a different `rig` starts a blend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_applied: Option<RigConfig>,
    /// Set by [`snap_rig`]; the next tick cuts and clears it.
    pub snap: bool,
    /// The projection onto the track path, in metres of path parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param: Option<f32>,
    /// The sprung eye position.
    pub eye: [f32; 3],
    /// The eye spring's velocity, per axis.
    pub eye_velocity: [f32; 3],
    /// The sprung yaw in degrees.
    pub yaw_deg: f32,
    /// The sprung pitch in degrees.
    pub pitch_deg: f32,
    /// The yaw spring's velocity in degrees per second.
    pub yaw_velocity: f32,
    /// The pitch spring's velocity in degrees per second.
    pub pitch_velocity: f32,
    /// The vertical anchor's height.
    pub anchor_y: f32,
    /// The smoothed look-ahead shift on x and z.
    pub look_ahead: [f32; 2],
    /// The target position the last advancing tick saw, for its velocity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_target: Option<[f32; 3]>,
    /// The running blend, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blend: Option<Blend>,
    /// The last posed pre-shake pose.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pose: Option<Pose>,
    /// Gameplay seconds the shake noise has advanced.
    pub shake_time: f64,
}
