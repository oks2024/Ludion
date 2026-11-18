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

fn kick_is_default(kick: &KickConfig) -> bool {
    *kick == KickConfig::default()
}

fn zero3(v: &[f32; 3]) -> bool {
    *v == [0.0; 3]
}

fn zero(v: &f32) -> bool {
    *v == 0.0
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

fn is_default_sensitivity(v: &f32) -> bool {
    *v == CameraRig::DEFAULT_LOOK_DEG_PER_COUNT
}

/// The rig on the camera entity. `rig` is the authored configuration (writing a different value
/// starts a timed blend), `shake` survives switches, `trauma`, `grounded` and `look_deg` are
/// game-written inputs, `state` is engine-written. The kick, roll and FOV channels are game-written
/// too (the engine decays only the two kicks) and apply after aim. Every field defaults, so older documents keep loading.
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
    /// Look input `[yaw, pitch]` in degrees (positive yaw turns left, positive pitch looks down)
    /// for `first_person` and `chase`; `follow` consumes and discards it. The game writes it,
    /// usually through [`add_look`]; the rig applies and zeroes it on each tick it poses.
    #[serde(skip_serializing_if = "look_is_zero")]
    pub look_deg: [f32; 2],
    /// Degrees of look per raw mouse count: the one sensitivity the sim ([`add_look_counts`]) and a
    /// late-latching presenter share.
    #[serde(skip_serializing_if = "is_default_sensitivity")]
    pub look_deg_per_count: f32,
    /// The kick decay; not part of `rig`, so a switch never blends or resets it.
    #[serde(skip_serializing_if = "kick_is_default")]
    pub kick: KickConfig,
    /// Recoil kick `[yaw, pitch, roll]` in degrees, post-aim (positive yaw turns left, positive
    /// pitch looks down, so recoil up is negative pitch). Add through [`add_kick`]; it decays to
    /// exactly `0` on gameplay time.
    #[serde(skip_serializing_if = "zero3")]
    pub kick_deg: [f32; 3],
    /// Field-of-view kick in degrees, added through [`add_fov_kick`]; decays like `kick_deg`.
    #[serde(skip_serializing_if = "zero")]
    pub fov_kick_deg: f32,
    /// Persistent roll in degrees, post-aim; the game writes it and the rig never changes it.
    /// Positive turns the camera's right vector upward.
    #[serde(skip_serializing_if = "zero")]
    pub roll_deg: f32,
    /// Persistent field-of-view offset in degrees, post-aim; the game writes it and the rig never
    /// changes it. Inert when the rig sets no `fov_y_deg`.
    #[serde(skip_serializing_if = "zero")]
    pub fov_offset_deg: f32,
    /// The game-written framing channel (corner peek, wall-hug, shoulder framing) for `chase` and
    /// `follow`: it moves the eye before collision, eased by `framing.half_life`.
    #[serde(skip_serializing_if = "is_default")]
    pub framing: Framing,
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
            look_deg_per_count: Self::DEFAULT_LOOK_DEG_PER_COUNT,
            kick: KickConfig::default(),
            kick_deg: [0.0; 3],
            fov_kick_deg: 0.0,
            roll_deg: 0.0,
            fov_offset_deg: 0.0,
            framing: Framing::default(),
            state: RigState::default(),
        }
    }
}

impl CameraRig {
    /// The default [`look_deg_per_count`](Self::look_deg_per_count).
    pub const DEFAULT_LOOK_DEG_PER_COUNT: f32 = 0.1;

    /// The yaw the rig last posed at, in degrees, before shake; `0.0` before the first pose.
    /// [`camera_relative`](crate::camera_relative) reads it.
    pub fn yaw_deg(&self) -> f32 {
        self.state.pose.as_ref().map_or(0.0, |pose| pose.yaw_deg)
    }

    /// The eye and orientation the rig last posed, before shake, kick, roll, the FOV channels and any other post-aim effect;
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

/// The `[yaw, pitch]` look in degrees for raw mouse counts through the rig's
/// [`look_deg_per_count`](CameraRig::look_deg_per_count): a mouse moving right turns right
/// (negative yaw) and moving down looks down (positive pitch). The one conversion the sim and a
/// presenter share.
///
/// # Example
///
/// ```
/// use ludion_camera::{CameraRig, look_from_counts};
///
/// let rig = CameraRig::default();
/// assert_eq!(look_from_counts(&rig, 10.0, 5.0), [-1.0, 0.5]);
/// ```
#[must_use]
pub fn look_from_counts(rig: &CameraRig, dx: f32, dy: f32) -> [f32; 2] {
    let s = rig.look_deg_per_count;
    [-dx * s, dy * s]
}

/// Adds raw mouse counts to the rig's pending look through [`look_from_counts`]; a non-finite
/// component is ignored.
pub fn add_look_counts(rig: &mut CameraRig, dx: f32, dy: f32) {
    let [yaw, pitch] = look_from_counts(rig, dx, dy);
    add_look(rig, yaw, pitch);
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

/// Adds a recoil kick (degrees) to [`CameraRig::kick_deg`]; a non-finite component is ignored.
/// Recoil up is a negative `pitch_deg`.
pub fn add_kick(rig: &mut CameraRig, yaw_deg: f32, pitch_deg: f32, roll_deg: f32) {
    for (slot, value) in rig.kick_deg.iter_mut().zip([yaw_deg, pitch_deg, roll_deg]) {
        if value.is_finite() {
            *slot += value;
        }
    }
}

/// Adds a field-of-view kick (degrees) to [`CameraRig::fov_kick_deg`]; a non-finite value is
/// ignored.
pub fn add_fov_kick(rig: &mut CameraRig, deg: f32) {
    if deg.is_finite() {
        rig.fov_kick_deg += deg;
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
    /// The sphere-cast collision stage that keeps the eye out of geometry; off by default.
    #[serde(skip_serializing_if = "is_default")]
    pub collision: RigCollisionConfig,
}

/// The collision stage of a rig: a sphere cast from the pivot (or focus, on `track`) to the eye
/// pulls the boom in at once and eases it back out after a hold. Ignored in `first_person`. It
/// lives on the config so a zone can swap it.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct RigCollisionConfig {
    /// Whether the stage runs.
    pub enabled: bool,
    /// Radius of the cast sphere in metres; above 0.
    pub radius: f32,
    /// Physics layer names the cast hits; empty hits every layer.
    pub include_layers: Vec<String>,
    /// Physics layer names the cast passes through.
    pub ignore_layers: Vec<String>,
    /// The shortest boom a hit can pull in to, in metres.
    pub min_distance: f32,
    /// Metres kept between the hit and the eye.
    pub skin: f32,
    /// Gameplay seconds the pulled-in boom holds before easing back out.
    pub recover_delay: f32,
    /// Seconds for the remaining boom error to halve while recovering.
    pub recover_half_life: f32,
}

impl Default for RigCollisionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            radius: 0.3,
            include_layers: Vec::new(),
            ignore_layers: Vec::new(),
            min_distance: 0.6,
            skin: 0.05,
            recover_delay: 0.15,
            recover_half_life: 0.35,
        }
    }
}

/// The game-written framing channel: at `weight` 1 the boom is built from the offset pivot, yaw,
/// pitch and distance below. It eases by `half_life`, applies before collision, and the framed
/// yaw and pitch show in [`CameraRig::aim`]; `state.yaw_deg` stays the unframed orbit yaw.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Framing {
    /// The goal weight, clamped to `[0, 1]` at use; `0` is no framing.
    pub weight: f32,
    /// Pivot offset in metres in the yaw frame: `+x` right, `+y` up, `+z` back.
    pub pivot_offset: [f32; 3],
    /// Yaw added at full weight, in degrees.
    pub yaw_offset_deg: f32,
    /// Pitch added at full weight, in degrees (positive looks down).
    pub pitch_offset_deg: f32,
    /// The boom length at full weight; `None` keeps the mode's.
    pub distance: Option<f32>,
    /// Seconds for the applied weight to halve toward `weight`.
    pub half_life: f32,
}

impl Default for Framing {
    fn default() -> Self {
        Self {
            weight: 0.0,
            pivot_offset: [0.0; 3],
            yaw_offset_deg: 0.0,
            pitch_offset_deg: 0.0,
            distance: None,
            half_life: 0.12,
        }
    }
}

/// The chase recenter: after `delay` gameplay seconds with no look, the yaw turns back behind the
/// target's horizontal motion.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Recenter {
    /// Whether the recenter runs.
    pub enabled: bool,
    /// Gameplay seconds without look input before it starts.
    pub delay: f32,
    /// Seconds for the yaw error to halve.
    pub half_life: f32,
    /// Horizontal target speed in metres per second below which it does not turn.
    pub min_speed: f32,
}

impl Default for Recenter {
    fn default() -> Self {
        Self {
            enabled: false,
            delay: 1.5,
            half_life: 0.6,
            min_speed: 1.0,
        }
    }
}

/// The placement mode of a rig. Externally tagged: `mode: { track: { .. } }`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RigMode {
    /// Ride a level-authored path: the target is projected onto it and the camera sits at an offset
    /// in the path's frame.
    Track(TrackRig),
    /// The eye rides the target rigidly; yaw and pitch are game-written look deltas on
    /// [`CameraRig::look_deg`], unsmoothed.
    FirstPerson(FirstPersonRig),
    /// The boom orbits the target: the pivot is sprung toward the focus, yaw and pitch are
    /// game-written look deltas on [`CameraRig::look_deg`], unsmoothed.
    Chase(ChaseRig),
    /// The boom holds a fixed world yaw, pitch and distance from the sprung pivot.
    Follow(FollowRig),
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
    /// The pitch in degrees a snap or switch into `first_person` starts at (clamped on the next
    /// look); `None` carries the current pitch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enter_pitch_deg: Option<f32>,
}

impl Default for FirstPersonRig {
    fn default() -> Self {
        Self {
            eye_offset: [0.0, 1.6, 0.0],
            pitch_limit_deg: 89.0,
            enter_pitch_deg: None,
        }
    }
}

/// The `chase` mode: a boom of `distance` metres from the pivot, orbited by look input.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ChaseRig {
    /// Boom length from the pivot to the eye, in metres; above 0.
    pub distance: f32,
    /// The pitch in degrees (positive looks down) a snap or switch into `chase` starts at.
    pub pitch_deg: f32,
    /// The lowest pitch look can reach, in degrees; in `(-90, 90)`.
    pub pitch_min_deg: f32,
    /// The highest pitch look can reach, in degrees; in `(-90, 90)` and at least `pitch_min_deg`
    /// (equal gives a fixed pitch).
    pub pitch_max_deg: f32,
    /// The yaw in degrees a snap or switch into `chase` starts at; `None` carries the current yaw.
    pub yaw_deg: Option<f32>,
    /// Whether `look_deg[1]` moves the pitch; `false` consumes and drops it.
    pub look_pitch: bool,
    /// Turns the yaw back behind the target's motion once look has been idle.
    #[serde(skip_serializing_if = "is_default")]
    pub recenter: Recenter,
}

impl Default for ChaseRig {
    fn default() -> Self {
        Self {
            distance: 6.0,
            pitch_deg: 20.0,
            pitch_min_deg: -30.0,
            pitch_max_deg: 70.0,
            yaw_deg: None,
            look_pitch: true,
            recenter: Recenter::default(),
        }
    }
}

/// The `follow` mode: a fixed world yaw, pitch and distance from the sprung pivot.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct FollowRig {
    /// World yaw in degrees (0 puts the eye on the pivot's +Z side).
    pub yaw_deg: f32,
    /// Pitch in degrees, positive looks down; in `(-90, 90)`.
    pub pitch_deg: f32,
    /// Boom length from the pivot to the eye, in metres; above 0.
    pub distance: f32,
}

impl Default for FollowRig {
    fn default() -> Self {
        Self {
            yaw_deg: 0.0,
            pitch_deg: 45.0,
            distance: 10.0,
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

/// How fast the kicks return to centre, in gameplay seconds (`0` is an instant return).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct KickConfig {
    /// Seconds for the `kick_deg` error to halve.
    pub half_life: f32,
    /// Seconds for the `fov_kick_deg` error to halve.
    pub fov_half_life: f32,
}

impl Default for KickConfig {
    fn default() -> Self {
        Self {
            half_life: 0.08,
            fov_half_life: 0.15,
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
    /// The sprung pivot of a `chase` or `follow` rig; `None` in other modes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pivot: Option<[f32; 3]>,
    /// The pivot spring's velocity, per axis.
    #[serde(skip_serializing_if = "zero3")]
    pub pivot_velocity: [f32; 3],
    /// The sprung eye position (the orbit eye in `chase` and `follow`).
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
    /// The collided boom length in metres; `None` when the eye is free.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boom: Option<f32>,
    /// Gameplay seconds left before a pulled-in boom eases back out.
    #[serde(skip_serializing_if = "zero")]
    pub boom_hold: f32,
    /// The applied framing weight in `[0, 1]`.
    #[serde(skip_serializing_if = "zero")]
    pub framing_weight: f32,
    /// Gameplay seconds since the last nonzero chase look.
    #[serde(skip_serializing_if = "zero")]
    pub look_idle: f32,
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
    /// The [`CameraZone`](crate::CameraZone) entity whose `rig` drives this rig; `None` outside
    /// every zone. Written by the engine each tick from the target's position.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<ludion_ecs::EntityRefSchema>")]
    pub zone: Option<EntityId>,
    /// The configuration in force before the first zone, restored when the target leaves every
    /// zone; `None` outside zones.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<RigConfig>,
}
