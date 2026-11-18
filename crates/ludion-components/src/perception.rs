//! The [`Perceiver`] and [`SightTarget`] components: who looks, through which sight zones, and what
//! can be seen, from which sample points. `perception_update` in `ludion-engine` reads them. A
//! perceiver with [`PerceiverMemory`] keeps one [`StimulusRecord`] entity per thing it sensed.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::number::engine_number;
use ludion_ecs::SemanticValidate;
use ludion_physics::LayerSet;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Most sight zones a [`Perceiver`] holds, and most sample points a [`SightTarget`] holds.
pub const PERCEPTION_MAX_PARTS: usize = 8;

/// Most `max_records` a [`PerceiverMemory`] allows.
pub const PERCEIVER_MEMORY_MAX_RECORDS: u32 = 64;

/// A facing offset from the entity's own rotation, in degrees. Yaw 0 looks along the entity's −Z,
/// **positive yaw turns left** and **positive pitch looks down**: the camera rig's convention.
///
/// ```
/// use ludion_components::Look;
///
/// let look: Look = serde_json::from_value(serde_json::json!({ "pitch_deg": 10.0 })).unwrap();
/// assert_eq!(look, Look { yaw_deg: 0.0, pitch_deg: 10.0 });
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
pub struct Look {
    /// Turn about +Y in degrees; positive turns left (default `0.0`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub yaw_deg: f32,
    /// Tilt in degrees; positive looks down (default `0.0`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pitch_deg: f32,
}

/// How a [`ZoneSweep`] moves the facing's yaw over gameplay time.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SweepShape {
    /// Constant-speed travel from `-amplitude_deg` to `+amplitude_deg`, a dwell, the way back, a dwell.
    #[default]
    PingPongLinear,
    /// `amplitude_deg * sin(2π t / period_s)`; `dwell_s` must be 0.
    Sine,
}

/// A closed-form yaw sweep of a [`Perceiver`]'s zones: a camera or searchlight panning to and fro.
/// It is a pure function of gameplay time (a held `GameClock` holds it).
///
/// ```
/// use ludion_components::{SweepShape, ZoneSweep};
///
/// let sweep: ZoneSweep = serde_json::from_value(serde_json::json!({
///     "amplitude_deg": 40.0, "period_s": 8.0, "dwell_s": 1.0,
/// }))
/// .unwrap();
/// assert_eq!(sweep.shape, SweepShape::PingPongLinear);
/// assert_eq!(sweep.phase_s, 0.0);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct ZoneSweep {
    /// Degrees either side of the look yaw (finite).
    pub amplitude_deg: f32,
    /// Seconds for one full cycle, above `2 * dwell_s`.
    pub period_s: f32,
    /// Seconds held at each end of a `ping_pong_linear` sweep (default `0.0`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub dwell_s: f32,
    /// Seconds added to gameplay time before the cycle position is taken (default `0.0`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub phase_s: f32,
    /// The motion (default `ping_pong_linear`).
    #[serde(default, skip_serializing_if = "is_default_shape")]
    pub shape: SweepShape,
}

/// One wedge of sight: a range, horizontal and vertical half-angles, and the classes it detects. A
/// point is in the zone when its horizontal and vertical angles are inside the half-angles and its
/// distance from the eye is within the effective range (a wedge, not a cone).
///
/// ```
/// use ludion_components::SightZone;
///
/// let zone: SightZone = serde_json::from_value(serde_json::json!({
///     "name": "inner", "range": 6.0, "half_angle_deg": 60.0,
///     "detects": ["player"], "see_through": ["cover_overhead"], "see_through_max_range": 3.0,
/// }))
/// .unwrap();
/// assert_eq!(zone.vertical_half_angle_deg, 45.0);
/// assert!(!zone.range_by_visibility);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct SightZone {
    /// The zone's name; unique within the perceiver (default `""`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Metres from the eye (finite and above 0).
    pub range: f32,
    /// Horizontal half-angle in degrees, in (0, 180].
    pub half_angle_deg: f32,
    /// Vertical half-angle in degrees, in (0, 90] (default `45.0`).
    #[serde(
        default = "default_vertical_half_angle_deg",
        skip_serializing_if = "is_default_vertical"
    )]
    pub vertical_half_angle_deg: f32,
    /// The `PerceptionConfig` classes the zone detects; empty means every class (default `[]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detects: Vec<String>,
    /// Layers a sample point's `blocked_by` loses while it is within `see_through_max_range` of the
    /// eye (default `[]`).
    #[serde(default, skip_serializing_if = "LayerSet::is_empty")]
    pub see_through: LayerSet,
    /// Metres from the eye within which `see_through` applies (default `0.0`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub see_through_max_range: f32,
    /// Scales `range` by the target's visibility (default `false`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub range_by_visibility: bool,
    /// Clamps the visibility that scales the range, `[lo, hi]` with `lo <= hi`; unset clamps only
    /// below 0 (default unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility_clamp: Option<[f32; 2]>,
}

/// An entity that sees: up to eight [`SightZone`]s from an eye point, a facing, and an optional
/// closed-form [`ZoneSweep`]. The entity's world rotation, then `look`, then the sweep's yaw orient
/// every zone. `perception_update` fills the `PerceptionResults` resource from it.
///
/// ```yaml
/// components:
///   Perceiver:
///     eye:
///       - 0.0
///       - 1.6
///       - 0.0
///     look:
///       pitch_deg: 5.0
///     sight:
///       - name: inner
///         range: 6.0
///         half_angle_deg: 60.0
///         detects:
///           - player
///       - name: outer
///         range: 14.0
///         half_angle_deg: 30.0
///         range_by_visibility: true
/// ```
///
/// ```
/// use ludion_components::Perceiver;
///
/// let guard: Perceiver = serde_json::from_value(serde_json::json!({
///     "eye": [0.0, 1.6, 0.0],
///     "sight": [{ "name": "inner", "range": 6.0, "half_angle_deg": 60.0 }],
/// }))
/// .unwrap();
/// assert!(guard.enabled);
/// assert_eq!(guard.sight.len(), 1);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Perceiver {
    /// Whether the perceiver sees; a disabled one produces no samples (default `true`).
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// The eye in the entity's local frame, metres (default `[0, 0, 0]`).
    #[serde(default, skip_serializing_if = "is_zero_vec3")]
    pub eye: [f32; 3],
    /// The facing the zones share, from the entity's rotation (default none).
    #[serde(default, skip_serializing_if = "is_default_look")]
    pub look: Look,
    /// The sight zones, one to eight.
    #[serde(default)]
    pub sight: Vec<SightZone>,
    /// A yaw sweep over gameplay time, applied to every zone (default none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sweep: Option<ZoneSweep>,
    /// Keeps a [`StimulusRecord`] per sensed source; unset remembers nothing (default none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<PerceiverMemory>,
    /// Hears noises emitted with `emit_noise`; unset hears nothing (default none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hearing: Option<Hearing>,
}

/// How a [`Perceiver`] hears: an ear offset, a sensitivity and the noise kinds it listens for.
///
/// ```
/// use ludion_components::Hearing;
///
/// let ears: Hearing = serde_json::from_value(serde_json::json!({ "hears": ["footstep"] })).unwrap();
/// assert_eq!(ears.sensitivity, 1.0);
/// assert_eq!(ears.ear, [0.0; 3]);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Hearing {
    /// The ear in the entity's local frame, metres (default `[0, 0, 0]`).
    #[serde(default, skip_serializing_if = "is_zero_vec3")]
    pub ear: [f32; 3],
    /// Scales every noise's radius for this listener (default `1.0`; finite, not negative).
    #[serde(
        default = "default_visibility",
        skip_serializing_if = "is_default_visibility"
    )]
    pub sensitivity: f32,
    /// The `PerceptionConfig` noise kinds the listener hears, one or more, each once.
    #[serde(default)]
    pub hears: Vec<String>,
}

/// The volume of a [`NoiseZone`], in the zone entity's world pose.
///
/// ```
/// use ludion_components::NoiseZoneShape;
///
/// let shape: NoiseZoneShape = serde_json::from_value(serde_json::json!({ "sphere": { "radius": 4.0 } })).unwrap();
/// assert_eq!(shape, NoiseZoneShape::Sphere { radius: 4.0 });
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum NoiseZoneShape {
    /// A box oriented by the entity's rotation.
    Box {
        /// Half the box's size along each local axis, metres (finite and above 0).
        half_extents: [f32; 3],
    },
    /// A sphere about the entity's origin.
    Sphere {
        /// Metres (finite and above 0).
        radius: f32,
    },
}

/// A volume that scales the radius of the noises emitted inside it: rain, a loud street, a muffled
/// carpet. Among the zones containing a noise and matching its kind the highest `priority` wins;
/// multipliers never stack.
///
/// ```yaml
/// components:
///   NoiseZone:
///     shape:
///       box:
///         half_extents:
///           - 20.0
///           - 5.0
///           - 20.0
///     multiplier: 0.5
///     kinds:
///       - footstep
/// ```
///
/// ```
/// use ludion_components::NoiseZone;
///
/// let rain: NoiseZone = serde_json::from_value(serde_json::json!({
///     "shape": { "sphere": { "radius": 30.0 } }, "multiplier": 0.5,
/// }))
/// .unwrap();
/// assert!(rain.kinds.is_empty() && rain.priority == 0);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct NoiseZone {
    /// The volume.
    pub shape: NoiseZoneShape,
    /// Scales the radius of a noise inside (default `1.0`; finite, not negative).
    #[serde(
        default = "default_visibility",
        skip_serializing_if = "is_default_visibility"
    )]
    pub multiplier: f32,
    /// The noise kinds the zone applies to; empty means every kind (default `[]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<String>,
    /// The higher one wins where zones overlap; ties go to the lowest entity (default `0`).
    #[serde(default, skip_serializing_if = "is_zero_priority")]
    pub priority: i32,
}

/// A noise `emit_noise` has queued and `perception_update` has not yet delivered. It is a runtime
/// entity, so a snapshot between emission and delivery loses nothing.
///
/// ```
/// use ludion_components::PendingNoise;
///
/// let noise: PendingNoise = serde_json::from_value(serde_json::json!({
///     "id": 3, "at": [1.0, 0.0, 2.0], "radius": 8.0, "loudness": 1.0, "kind": "footstep",
/// }))
/// .unwrap();
/// assert!(noise.source.is_none() && !noise.occlusion);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct PendingNoise {
    /// The monotonic noise id; delivery order.
    pub id: u64,
    /// Where it was made, world metres.
    pub at: [f32; 3],
    /// Metres it carries before zones, surface and sensitivity.
    pub radius: f32,
    /// How loud it was, passed to the listener.
    pub loudness: f32,
    /// A `PerceptionConfig` noise kind.
    pub kind: String,
    /// What made it (default none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<ludion_ecs::EntityRefSchema>")]
    pub source: Option<EntityId>,
    /// The surface it was made on, a physics `SurfaceId` (default none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<u16>,
    /// Whether sound occluders between the noise and a listener shrink its radius (default `false`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub occlusion: bool,
}

/// The singleton `perception_update` and `emit_noise` keep their counters in, spawned on the first
/// noise. Hashed and snapshotted.
///
/// ```
/// use ludion_components::PerceptionState;
///
/// assert_eq!(PerceptionState::default().next_noise_id, 0);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PerceptionState {
    /// The id the next emitted noise gets (default `0`).
    #[serde(default, skip_serializing_if = "is_zero_tick")]
    pub next_noise_id: u64,
}

/// How long a [`Perceiver`] remembers what it sensed, and how much. Times are gameplay seconds, so a
/// held `GameClock` stops the forgetting.
///
/// ```
/// use ludion_components::PerceiverMemory;
///
/// let memory: PerceiverMemory = serde_json::from_value(serde_json::json!({})).unwrap();
/// assert_eq!(memory, PerceiverMemory::default());
/// assert_eq!((memory.forget_sight_s, memory.forget_hearing_s, memory.max_records), (12.0, 8.0, 8));
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct PerceiverMemory {
    /// Seconds after the last sighting before a sight record is forgotten (default `12.0`).
    #[serde(
        default = "default_forget_sight_s",
        skip_serializing_if = "is_default_forget_sight"
    )]
    pub forget_sight_s: f32,
    /// Seconds after the last report before a hearing or custom record is forgotten (default `8.0`).
    #[serde(
        default = "default_forget_hearing_s",
        skip_serializing_if = "is_default_forget_hearing"
    )]
    pub forget_hearing_s: f32,
    /// Most records kept; the least recently sensed is evicted first, 1 to 64 (default `8`).
    #[serde(
        default = "default_max_records",
        skip_serializing_if = "is_default_max_records"
    )]
    pub max_records: u32,
}

impl Default for PerceiverMemory {
    fn default() -> Self {
        Self {
            forget_sight_s: default_forget_sight_s(),
            forget_hearing_s: default_forget_hearing_s(),
            max_records: default_max_records(),
        }
    }
}

/// The channel a stimulus arrived on. Orders `sight`, `hearing`, then `custom` by number.
///
/// ```
/// use ludion_components::Sense;
///
/// let custom: Sense = serde_json::from_value(serde_json::json!({ "custom": 3 })).unwrap();
/// assert_eq!(custom, Sense::Custom(3));
/// assert!(Sense::Sight < Sense::Hearing && Sense::Hearing < custom);
/// ```
#[derive(
    Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum Sense {
    /// Seen by a [`Perceiver`]'s sight zones.
    Sight,
    /// Heard, reported by the game.
    Hearing,
    /// A game-defined channel, numbered by the game.
    Custom(u32),
}

/// One thing a [`Perceiver`] sensed and still remembers: the ghost marker. `perception_update` and
/// `report_stimulus` write it; the game reads `last_position` of a record whose `sensed_now` is
/// `false` to search where the source was last seen. It is a runtime entity, hashed and snapshotted
/// like any other, and despawns with its perceiver or when forgotten.
///
/// ```
/// use ludion_components::{Sense, StimulusRecord};
///
/// let record: StimulusRecord = serde_json::from_value(serde_json::json!({
///     "perceiver": { "index": 0, "generation": 0 }, "sense": "sight", "last_position": [1.0, 0.0, 2.0],
///     "first_sensed_tick": 10, "last_sensed_tick": 40,
/// }))
/// .unwrap();
/// assert_eq!(record.sense, Sense::Sight);
/// assert!(record.source.is_none() && !record.sensed_now);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct StimulusRecord {
    /// The perceiver that remembers.
    #[schemars(with = "ludion_ecs::EntityRefSchema")]
    pub perceiver: EntityId,
    /// What was sensed; none for an anonymous stimulus (default none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<ludion_ecs::EntityRefSchema>")]
    pub source: Option<EntityId>,
    /// The channel.
    pub sense: Sense,
    /// Where the source was when last sensed, world metres.
    pub last_position: [f32; 3],
    /// The unit direction the source last moved, or `[0, 0, 0]` when unknown (default `[0, 0, 0]`).
    #[serde(default, skip_serializing_if = "is_zero_vec3")]
    pub last_direction: [f32; 3],
    /// The gameplay tick the record was created on (default `0`).
    #[serde(default, skip_serializing_if = "is_zero_tick")]
    pub first_sensed_tick: u64,
    /// The gameplay tick the source was last sensed on (default `0`).
    #[serde(default, skip_serializing_if = "is_zero_tick")]
    pub last_sensed_tick: u64,
    /// Whether the source was sensed this tick: true iff `last_sensed_tick` is the current tick (default `false`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub sensed_now: bool,
    /// How strong the last stimulus was; sight is always `1.0` (default `1.0`).
    #[serde(
        default = "default_visibility",
        skip_serializing_if = "is_default_visibility"
    )]
    pub strength: f32,
    /// The lowest sight zone index that saw the source on the last sighting (default `0`).
    #[serde(default, skip_serializing_if = "is_zero_zone")]
    pub zone: u8,
    /// The noise kind of a hearing record; empty for every other sense (default `""`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
}

/// How many of a [`SightTarget`]'s sample points must be seen for the target to count as seen.
///
/// ```
/// use ludion_components::Require;
///
/// let rule: Require = serde_json::from_value(serde_json::json!({ "fraction": 0.5 })).unwrap();
/// assert_eq!(rule, Require::Fraction(0.5));
/// let all: Require = serde_json::from_value(serde_json::json!("all")).unwrap();
/// assert_eq!(all, Require::All);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Require {
    /// One visible point is enough.
    #[default]
    Any,
    /// Every point must be visible.
    All,
    /// At least `ceil(fraction * points)` points, with `fraction` in (0, 1].
    Fraction(f32),
}

/// One place on a target a ray is cast to, and the layers that hide it.
///
/// ```
/// use ludion_components::SightPoint;
///
/// let head: SightPoint = serde_json::from_value(serde_json::json!({
///     "offset": [0.0, 1.6, 0.0], "blocked_by": ["wall", "cover_high"],
/// }))
/// .unwrap();
/// assert_eq!(head.blocked_by.as_slice(), ["wall", "cover_high"]);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct SightPoint {
    /// The point in the target's local frame, metres.
    pub offset: [f32; 3],
    /// The layers that block the line of sight to this point; empty means nothing blocks it (a
    /// ray is not even cast; default `[]`).
    #[serde(default, skip_serializing_if = "LayerSet::is_empty")]
    pub blocked_by: LayerSet,
}

/// An entity a [`Perceiver`] can see: a `class` the zones detect, a `visibility` that scales
/// visibility-ranged zones, and one to eight sample [`SightPoint`]s with the rule that turns their
/// results into one answer.
///
/// ```yaml
/// components:
///   SightTarget:
///     class: player
///     visibility: 0.5
///     points:
///       - offset:
///           - 0.0
///           - 1.6
///           - 0.0
///         blocked_by:
///           - wall
///           - cover_high
///       - offset:
///           - 0.0
///           - 0.4
///           - 0.0
///         blocked_by:
///           - wall
///           - cover_low
///           - cover_high
///     require: any
/// ```
///
/// ```
/// use ludion_components::{Require, SightTarget};
///
/// let target: SightTarget = serde_json::from_value(serde_json::json!({
///     "class": "player",
///     "points": [{ "offset": [0.0, 1.0, 0.0] }],
/// }))
/// .unwrap();
/// assert_eq!(target.visibility, 1.0);
/// assert_eq!(target.require, Require::Any);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct SightTarget {
    /// A `PerceptionConfig` class name.
    pub class: String,
    /// Scales the range of `range_by_visibility` zones (default `1.0`; finite, not negative).
    #[serde(
        default = "default_visibility",
        skip_serializing_if = "is_default_visibility"
    )]
    pub visibility: f32,
    /// The sample points, one to eight.
    #[serde(default)]
    pub points: Vec<SightPoint>,
    /// How many points must be seen (default `any`).
    #[serde(default, skip_serializing_if = "is_default_require")]
    pub require: Require,
}

fn is_zero(value: &f32) -> bool {
    *value == 0.0
}
fn is_zero_tick(value: &u64) -> bool {
    *value == 0
}
fn is_zero_zone(value: &u8) -> bool {
    *value == 0
}
fn is_zero_priority(value: &i32) -> bool {
    *value == 0
}
fn default_forget_sight_s() -> f32 {
    12.0
}
fn is_default_forget_sight(value: &f32) -> bool {
    *value == 12.0
}
fn default_forget_hearing_s() -> f32 {
    8.0
}
fn is_default_forget_hearing(value: &f32) -> bool {
    *value == 8.0
}
fn default_max_records() -> u32 {
    8
}
fn is_default_max_records(value: &u32) -> bool {
    *value == 8
}
fn is_zero_vec3(value: &[f32; 3]) -> bool {
    *value == [0.0; 3]
}
fn is_false(value: &bool) -> bool {
    !*value
}
fn is_true(value: &bool) -> bool {
    *value
}
fn default_true() -> bool {
    true
}
fn is_default_shape(value: &SweepShape) -> bool {
    *value == SweepShape::default()
}
fn is_default_look(value: &Look) -> bool {
    *value == Look::default()
}
fn is_default_require(value: &Require) -> bool {
    *value == Require::default()
}
fn default_vertical_half_angle_deg() -> f32 {
    45.0
}
fn is_default_vertical(value: &f32) -> bool {
    *value == 45.0
}
fn default_visibility() -> f32 {
    1.0
}
fn is_default_visibility(value: &f32) -> bool {
    *value == 1.0
}

fn number(value: &Value) -> Option<f32> {
    value.as_f64().map(|n| engine_number(n) as f32)
}

/// A collector of diagnostics at one code, so each check is one line.
struct Found {
    code: ludion_core::diagnostics::DiagnosticCode,
    component: &'static str,
    out: Vec<Diagnostic>,
}

impl Found {
    fn new(code: ludion_core::diagnostics::DiagnosticCode, component: &'static str) -> Self {
        Self {
            code,
            component,
            out: Vec::new(),
        }
    }

    fn reject(&mut self, pointer: String, problem: String, help: &str) {
        self.out.push(
            Diagnostic::new(self.code, format!("`{}` {problem}", self.component))
                .with_path(pointer)
                .with_help(help.to_string()),
        );
    }

    /// Rejects an empty or repeated name in `value[field]`, and an empty list when `needs_one`.
    fn names_listed(
        &mut self,
        value: &Value,
        base: &str,
        field: &str,
        needs_one: bool,
        help: &str,
    ) {
        let names = value.get(field).and_then(Value::as_array);
        if needs_one && names.is_none_or(Vec::is_empty) {
            self.reject(
                format!("{base}/{field}"),
                format!("`{field}` needs at least one name"),
                help,
            );
        }
        let mut seen: Vec<&str> = Vec::new();
        for (i, name) in names.into_iter().flatten().enumerate() {
            let Some(name) = name.as_str() else { continue };
            if name.is_empty() || seen.contains(&name) {
                self.reject(
                    format!("{base}/{field}/{i}"),
                    format!("`{field}[{i}]` is empty or repeats `{name}`"),
                    help,
                );
            }
            seen.push(name);
        }
    }

    /// Rejects `value[field]` when it is present, a number, and `fails` it.
    fn number_where(
        &mut self,
        value: &Value,
        base: &str,
        field: &str,
        fails: impl Fn(f32) -> bool,
        needs: &str,
        help: &str,
    ) {
        if let Some(n) = value.get(field).and_then(number)
            && fails(n)
        {
            self.reject(
                format!("{base}/{field}"),
                format!("`{field}` {n} {needs}"),
                help,
            );
        }
    }
}

impl SemanticValidate for Perceiver {
    /// Rejects what a schema cannot (`L1570`, pointer into the field): a zone count outside 1..=8,
    /// a `range` that is not finite and above 0, a `half_angle_deg` outside (0, 180], a
    /// `vertical_half_angle_deg` outside (0, 90], a repeated zone `name`, a negative or non-finite
    /// `see_through_max_range`, a `visibility_clamp` with `lo > hi`, a non-finite `eye` or `look`,
    /// and a sweep whose `period_s` does not exceed `2 * dwell_s`, whose sine carries a dwell, or
    /// whose numbers are not finite. A wrong *shape* is the schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut found = Found::new(code::PERCEIVER_INVALID, "Perceiver");
        let help = "see `ludion docs perception`; `ludion explain L1570`";
        if let Some(eye) = value.get("eye").and_then(Value::as_array) {
            for (i, c) in eye.iter().enumerate() {
                if number(c).is_some_and(|n| !n.is_finite()) {
                    found.reject(format!("/eye/{i}"), "`eye` must be finite".into(), help);
                }
            }
        }
        if let Some(look) = value.get("look") {
            for field in ["yaw_deg", "pitch_deg"] {
                found.number_where(
                    look,
                    "/look",
                    field,
                    |n| !n.is_finite(),
                    "is not finite",
                    help,
                );
            }
        }
        if let Some(zones) = value.get("sight").and_then(Value::as_array) {
            if zones.is_empty() || zones.len() > PERCEPTION_MAX_PARTS {
                found.reject(
                    "/sight".into(),
                    format!(
                        "`sight` holds {} zones; it needs 1 to {PERCEPTION_MAX_PARTS}",
                        zones.len()
                    ),
                    help,
                );
            }
            let mut names: Vec<&str> = Vec::new();
            for (i, zone) in zones.iter().enumerate() {
                let base = format!("/sight/{i}");
                if let Some(name) = zone.get("name").and_then(Value::as_str) {
                    if names.contains(&name) {
                        found.reject(
                            format!("{base}/name"),
                            format!("zone name `{name}` is repeated"),
                            help,
                        );
                    }
                    names.push(name);
                }
                found.number_where(
                    zone,
                    &base,
                    "range",
                    |n| !(n.is_finite() && n > 0.0),
                    "must be finite and above 0",
                    help,
                );
                found.number_where(
                    zone,
                    &base,
                    "half_angle_deg",
                    |n| !(n > 0.0 && n <= 180.0),
                    "must be in (0, 180]",
                    help,
                );
                found.number_where(
                    zone,
                    &base,
                    "vertical_half_angle_deg",
                    |n| !(n > 0.0 && n <= 90.0),
                    "must be in (0, 90]",
                    help,
                );
                found.number_where(
                    zone,
                    &base,
                    "see_through_max_range",
                    |n| !(n.is_finite() && n >= 0.0),
                    "must be finite and not negative",
                    help,
                );
                if let Some(clamp) = zone.get("visibility_clamp").and_then(Value::as_array) {
                    let ends: Vec<f32> = clamp.iter().filter_map(number).collect();
                    if let [lo, hi] = ends[..]
                        && (lo.is_nan() || hi.is_nan() || lo < 0.0 || lo > hi)
                    {
                        found.reject(
                            format!("{base}/visibility_clamp"),
                            format!("`visibility_clamp` [{lo}, {hi}] needs 0 <= lo <= hi"),
                            help,
                        );
                    }
                }
            }
        }
        if let Some(sweep) = value.get("sweep").filter(|s| s.is_object()) {
            for field in ["amplitude_deg", "phase_s"] {
                found.number_where(
                    sweep,
                    "/sweep",
                    field,
                    |n| !n.is_finite(),
                    "is not finite",
                    help,
                );
            }
            let period = sweep.get("period_s").and_then(number);
            let dwell = sweep.get("dwell_s").and_then(number).unwrap_or(0.0);
            if let Some(period) = period
                && !(period.is_finite() && dwell >= 0.0 && period > 2.0 * dwell)
            {
                found.reject(
                    "/sweep/period_s".into(),
                    format!("`period_s` {period} must be finite and exceed 2 * `dwell_s` ({dwell}), which is not negative"),
                    help,
                );
            }
            if sweep.get("shape").and_then(Value::as_str) == Some("sine") && dwell != 0.0 {
                found.reject(
                    "/sweep/dwell_s".into(),
                    format!("a `sine` sweep has no dwell, found {dwell}"),
                    help,
                );
            }
        }
        if let Some(memory) = value.get("memory").filter(|m| m.is_object()) {
            let mut bad = Found::new(code::PERCEIVER_MEMORY_INVALID, "Perceiver");
            let help = "see `ludion docs perception`; `ludion explain L1573`";
            for field in ["forget_sight_s", "forget_hearing_s"] {
                bad.number_where(
                    memory,
                    "/memory",
                    field,
                    |n| !(n.is_finite() && n >= 0.0),
                    "must be finite and not negative",
                    help,
                );
            }
            if let Some(n) = memory.get("max_records").and_then(Value::as_u64)
                && !(1..=u64::from(PERCEIVER_MEMORY_MAX_RECORDS)).contains(&n)
            {
                bad.reject(
                    "/memory/max_records".into(),
                    format!("`max_records` {n} must be in 1 to {PERCEIVER_MEMORY_MAX_RECORDS}"),
                    help,
                );
            }
            found.out.extend(bad.out);
        }
        if let Some(hearing) = value.get("hearing").filter(|h| h.is_object()) {
            let mut bad = Found::new(code::HEARING_INVALID, "Perceiver");
            let help = "see `ludion docs perception`; `ludion explain L1576`";
            if let Some(ear) = hearing.get("ear").and_then(Value::as_array) {
                for (i, c) in ear.iter().enumerate() {
                    if number(c).is_some_and(|n| !n.is_finite()) {
                        bad.reject(
                            format!("/hearing/ear/{i}"),
                            "`ear` must be finite".into(),
                            help,
                        );
                    }
                }
            }
            bad.number_where(
                hearing,
                "/hearing",
                "sensitivity",
                |n| !(n.is_finite() && n >= 0.0),
                "must be finite and not negative",
                help,
            );
            bad.names_listed(hearing, "/hearing", "hears", true, help);
            found.out.extend(bad.out);
        }
        found.out
    }
}

impl SemanticValidate for NoiseZone {
    /// Rejects what a schema cannot (`L1575`, pointer into the field): a box half extent or sphere
    /// radius that is not finite and above 0, a negative or non-finite `multiplier`, and an empty or
    /// repeated `kinds` entry.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut found = Found::new(code::NOISE_ZONE_INVALID, "NoiseZone");
        let help = "see `ludion docs perception`; `ludion explain L1575`";
        if let Some(shape) = value.get("shape") {
            if let Some(extents) = shape
                .get("box")
                .and_then(|b| b.get("half_extents"))
                .and_then(Value::as_array)
            {
                for (i, c) in extents.iter().enumerate() {
                    if number(c).is_some_and(|n| !(n.is_finite() && n > 0.0)) {
                        found.reject(
                            format!("/shape/box/half_extents/{i}"),
                            "`half_extents` must be finite and above 0".into(),
                            help,
                        );
                    }
                }
            }
            if let Some(sphere) = shape.get("sphere") {
                found.number_where(
                    sphere,
                    "/shape/sphere",
                    "radius",
                    |n| !(n.is_finite() && n > 0.0),
                    "must be finite and above 0",
                    help,
                );
            }
        }
        found.number_where(
            value,
            "",
            "multiplier",
            |n| !(n.is_finite() && n >= 0.0),
            "must be finite and not negative",
            help,
        );
        found.names_listed(value, "", "kinds", false, help);
        found.out
    }
}

impl SemanticValidate for StimulusRecord {
    /// Rejects a non-finite `last_position`, `last_direction` or `strength` (`L1573`); a record is
    /// engine-written, so only a hand-edited scene or snapshot can carry one.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut found = Found::new(code::PERCEIVER_MEMORY_INVALID, "StimulusRecord");
        let help = "see `ludion docs perception`; `ludion explain L1573`";
        for field in ["last_position", "last_direction"] {
            if let Some(vector) = value.get(field).and_then(Value::as_array) {
                for (i, c) in vector.iter().enumerate() {
                    if number(c).is_some_and(|n| !n.is_finite()) {
                        found.reject(
                            format!("/{field}/{i}"),
                            format!("`{field}` must be finite"),
                            help,
                        );
                    }
                }
            }
        }
        found.number_where(
            value,
            "",
            "strength",
            |n| !n.is_finite(),
            "is not finite",
            help,
        );
        found.out
    }
}

impl SemanticValidate for SightTarget {
    /// Rejects what a schema cannot (`L1571`, pointer into the field): an empty `class`, a point
    /// count outside 1..=8, a non-finite `offset`, a negative or non-finite `visibility`, and a
    /// `require: { fraction }` outside (0, 1]. A wrong *shape* is the schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut found = Found::new(code::SIGHT_TARGET_INVALID, "SightTarget");
        let help = "see `ludion docs perception`; `ludion explain L1571`";
        if value.get("class").and_then(Value::as_str) == Some("") {
            found.reject("/class".into(), "`class` is empty".into(), help);
        }
        found.number_where(
            value,
            "",
            "visibility",
            |n| !(n.is_finite() && n >= 0.0),
            "must be finite and not negative",
            help,
        );
        if let Some(points) = value.get("points").and_then(Value::as_array) {
            if points.is_empty() || points.len() > PERCEPTION_MAX_PARTS {
                found.reject(
                    "/points".into(),
                    format!(
                        "`points` holds {} sample points; it needs 1 to {PERCEPTION_MAX_PARTS}",
                        points.len()
                    ),
                    help,
                );
            }
            for (i, point) in points.iter().enumerate() {
                let Some(offset) = point.get("offset").and_then(Value::as_array) else {
                    continue;
                };
                for (j, c) in offset.iter().enumerate() {
                    if number(c).is_some_and(|n| !n.is_finite()) {
                        found.reject(
                            format!("/points/{i}/offset/{j}"),
                            "`offset` must be finite".into(),
                            help,
                        );
                    }
                }
            }
        }
        if let Some(fraction) = value
            .get("require")
            .and_then(|r| r.get("fraction"))
            .and_then(number)
            && !(fraction > 0.0 && fraction <= 1.0)
        {
            found.reject(
                "/require/fraction".into(),
                format!("`fraction` {fraction} must be in (0, 1]"),
                help,
            );
        }
        found.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn zone() -> Value {
        json!({ "name": "inner", "range": 6.0, "half_angle_deg": 60.0 })
    }

    fn perceiver(extra: Value) -> Value {
        let mut base = json!({ "sight": [zone()] });
        for (k, v) in extra.as_object().unwrap() {
            base[k] = v.clone();
        }
        base
    }

    fn paths(found: Vec<Diagnostic>) -> Vec<String> {
        found
            .iter()
            .map(|d| d.path.clone().unwrap_or_default())
            .collect()
    }

    #[test]
    fn a_minimal_perceiver_is_clean_and_round_trips_without_defaults() {
        let value = perceiver(json!({}));
        assert!(Perceiver::validate_semantics(&value).is_empty());
        let parsed: Perceiver = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), value);
    }

    #[test]
    fn zone_counts_outside_one_to_eight_name_the_sight_pointer() {
        assert_eq!(
            paths(Perceiver::validate_semantics(&json!({ "sight": [] }))),
            ["/sight"]
        );
        let nine = json!({ "sight": vec![zone(); 9] });
        let found = Perceiver::validate_semantics(&nine);
        assert_eq!(found[0].path.as_deref(), Some("/sight"));
        assert_eq!(found[0].code, code::PERCEIVER_INVALID);
    }

    #[test]
    fn zone_fields_out_of_range_name_their_pointers() {
        let bad = json!({ "sight": [
            { "name": "a", "range": 0.0, "half_angle_deg": 60.0 },
            { "name": "b", "range": 5.0, "half_angle_deg": 181.0, "vertical_half_angle_deg": 91.0 },
            { "name": "b", "range": 5.0, "half_angle_deg": 0.0, "see_through_max_range": -1.0,
              "visibility_clamp": [2.0, 1.0] },
        ] });
        assert_eq!(
            paths(Perceiver::validate_semantics(&bad)),
            [
                "/sight/0/range",
                "/sight/1/half_angle_deg",
                "/sight/1/vertical_half_angle_deg",
                "/sight/2/name",
                "/sight/2/half_angle_deg",
                "/sight/2/see_through_max_range",
                "/sight/2/visibility_clamp",
            ]
        );
    }

    #[test]
    fn the_half_angle_and_vertical_edges_are_accepted() {
        let edge = json!({ "sight": [
            { "range": 1.0, "half_angle_deg": 180.0, "vertical_half_angle_deg": 90.0,
              "visibility_clamp": [0.0, 0.0] },
        ] });
        assert!(Perceiver::validate_semantics(&edge).is_empty());
    }

    #[test]
    fn sweep_rules() {
        let sweep = |s: Value| {
            paths(Perceiver::validate_semantics(&perceiver(
                json!({ "sweep": s }),
            )))
        };
        assert!(
            sweep(json!({ "amplitude_deg": 30.0, "period_s": 8.0, "dwell_s": 1.0 })).is_empty()
        );
        assert_eq!(
            sweep(json!({ "amplitude_deg": 30.0, "period_s": 2.0, "dwell_s": 1.0 })),
            ["/sweep/period_s"]
        );
        assert_eq!(
            sweep(
                json!({ "amplitude_deg": 30.0, "period_s": 8.0, "dwell_s": 1.0, "shape": "sine" })
            ),
            ["/sweep/dwell_s"]
        );
        assert_eq!(
            sweep(json!({ "amplitude_deg": 1e39, "period_s": 8.0 })),
            ["/sweep/amplitude_deg"]
        );
    }

    #[test]
    fn a_non_finite_eye_or_look_is_rejected() {
        let found = Perceiver::validate_semantics(&perceiver(
            json!({ "eye": [0.0, 1e39, 0.0], "look": { "yaw_deg": 1e39 } }),
        ));
        assert_eq!(paths(found), ["/eye/1", "/look/yaw_deg"]);
    }

    fn target(extra: Value) -> Value {
        let mut base = json!({ "class": "player", "points": [{ "offset": [0.0, 1.0, 0.0] }] });
        for (k, v) in extra.as_object().unwrap() {
            base[k] = v.clone();
        }
        base
    }

    #[test]
    fn a_minimal_target_is_clean_and_round_trips() {
        let value = target(json!({}));
        assert!(SightTarget::validate_semantics(&value).is_empty());
        let parsed: SightTarget = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), value);
    }

    #[test]
    fn target_fields_out_of_range_name_their_pointers() {
        let found = SightTarget::validate_semantics(&json!({
            "class": "", "visibility": -1.0,
            "points": [{ "offset": [0.0, 1e39, 0.0] }],
            "require": { "fraction": 1.5 },
        }));
        assert_eq!(found[0].code, code::SIGHT_TARGET_INVALID);
        assert_eq!(
            paths(found),
            [
                "/class",
                "/visibility",
                "/points/0/offset/1",
                "/require/fraction"
            ]
        );
        assert_eq!(
            paths(SightTarget::validate_semantics(
                &json!({ "class": "a", "points": [] })
            )),
            ["/points"]
        );
        let nine = target(json!({ "points": vec![json!({ "offset": [0, 0, 0] }); 9] }));
        assert_eq!(paths(SightTarget::validate_semantics(&nine)), ["/points"]);
    }

    #[test]
    fn fraction_edges() {
        let fraction = |f: f64| {
            SightTarget::validate_semantics(&target(json!({ "require": { "fraction": f } }))).len()
        };
        assert_eq!(fraction(1.0), 0);
        assert_eq!(fraction(0.0), 1);
        assert_eq!(fraction(0.001), 0);
    }

    #[test]
    fn memory_fields_out_of_range_name_their_pointers() {
        let bad = Perceiver::validate_semantics(&perceiver(json!({
            "memory": { "forget_sight_s": -1.0, "forget_hearing_s": 1e39, "max_records": 0 },
        })));
        assert_eq!(bad[0].code, code::PERCEIVER_MEMORY_INVALID);
        assert_eq!(
            paths(bad),
            [
                "/memory/forget_sight_s",
                "/memory/forget_hearing_s",
                "/memory/max_records"
            ]
        );
        let many = perceiver(json!({ "memory": { "max_records": 65 } }));
        assert_eq!(
            paths(Perceiver::validate_semantics(&many)),
            ["/memory/max_records"]
        );
        let edges = perceiver(json!({ "memory": { "forget_sight_s": 0.0, "max_records": 64 } }));
        assert!(Perceiver::validate_semantics(&edges).is_empty());
    }

    #[test]
    fn a_default_memory_round_trips_as_an_empty_map() {
        let value = perceiver(json!({ "memory": {} }));
        let parsed: Perceiver = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(parsed.memory, Some(PerceiverMemory::default()));
        assert_eq!(serde_json::to_value(&parsed).unwrap(), value);
    }

    #[test]
    fn a_record_round_trips_and_refuses_non_finite_numbers() {
        let value = json!({
            "perceiver": { "index": 1, "generation": 0 }, "sense": { "custom": 2 }, "last_position": [1.0, 2.0, 3.0],
            "last_sensed_tick": 5, "strength": 0.5,
        });
        let parsed: StimulusRecord = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), value);
        let bad = StimulusRecord::validate_semantics(&json!({
            "last_position": [0.0, 1e39, 0.0], "strength": 1e39,
        }));
        assert_eq!(paths(bad), ["/last_position/1", "/strength"]);
    }

    #[test]
    fn hearing_fields_out_of_range_name_their_pointers() {
        let ok = perceiver(json!({ "hearing": { "hears": ["footstep"] } }));
        assert!(Perceiver::validate_semantics(&ok).is_empty());
        let parsed: Perceiver = serde_json::from_value(ok.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), ok);
        let bad = Perceiver::validate_semantics(&perceiver(json!({
            "hearing": { "ear": [0.0, 1e39, 0.0], "sensitivity": -1.0, "hears": ["a", "a", ""] },
        })));
        assert_eq!(bad[0].code, code::HEARING_INVALID);
        assert_eq!(
            paths(bad),
            [
                "/hearing/ear/1",
                "/hearing/sensitivity",
                "/hearing/hears/1",
                "/hearing/hears/2"
            ]
        );
        let none = perceiver(json!({ "hearing": { "hears": [] } }));
        assert_eq!(
            paths(Perceiver::validate_semantics(&none)),
            ["/hearing/hears"]
        );
    }

    #[test]
    fn zone_fields_out_of_range_name_their_pointers_for_noise_zones() {
        let clean = json!({ "shape": { "box": { "half_extents": [1.0, 2.0, 3.0] } } });
        assert!(NoiseZone::validate_semantics(&clean).is_empty());
        let parsed: NoiseZone = serde_json::from_value(clean.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), clean);
        let bad = NoiseZone::validate_semantics(&json!({
            "shape": { "box": { "half_extents": [1.0, 0.0, 1.0] } },
            "multiplier": -0.5, "kinds": ["a", "a"],
        }));
        assert_eq!(bad[0].code, code::NOISE_ZONE_INVALID);
        assert_eq!(
            paths(bad),
            ["/shape/box/half_extents/1", "/multiplier", "/kinds/1"]
        );
        let sphere = json!({ "shape": { "sphere": { "radius": 0.0 } }, "kinds": [""] });
        assert_eq!(
            paths(NoiseZone::validate_semantics(&sphere)),
            ["/shape/sphere/radius", "/kinds/0"]
        );
    }

    #[test]
    fn require_reads_the_three_forms() {
        for (text, want) in [
            (json!("any"), Require::Any),
            (json!("all"), Require::All),
            (json!({ "fraction": 0.5 }), Require::Fraction(0.5)),
        ] {
            let parsed: Require = serde_json::from_value(text.clone()).unwrap();
            assert_eq!(parsed, want);
            assert_eq!(serde_json::to_value(parsed).unwrap(), text);
        }
    }
}
