//! Projectile patterns: the `kind: pattern` document, the two hashed components, the closed-form
//! bullet evaluator, and the two systems a game registers
//! (`Plan/projectile-patterns.md` decisions 1–8).
//!
//! **A gameplay bullet is an index, not an entity.** The entity is the *volley*: one [`Volley`]
//! component holding who fired, which document and bullet kind, the frame it fired on, the shape,
//! the resolved angle and speed range, a seed, and a 256-bit kill set. Bullet `i`'s trajectory is a
//! pure function of `(volley, bullet kind, i)` — `bullet_motion` builds the same [`Motion`] the
//! particle kernel evaluates — so the hit test the sim runs ([`volley_hits`]) and the quads
//! `extract` draws are literally the same arithmetic and cannot drift. Nothing per bullet is
//! stored, so a volley of 256 costs a few hundred hashed bytes, a whole-field cancel is a handful
//! of despawns, and the same `(volley, now)` yields the same field in any process, on any thread
//! count, after any `engine.restore`.
//!
//! **Timing is in frames, speeds in units per second.** A fire offset, `every`, `loop`, a motion
//! segment's `frames` and a bullet's `life` are exact integers at the fixed tick — the genre's own
//! unit and what a census closed form needs. Speeds are absolute: an omitted `from`/`speed` on a
//! segment carries the speed in force when that segment starts, which is the volley's fire speed
//! for the first segment and the previous segment's end speed after.
//!
//! **The engine ships the vocabulary; the game owns every rule.** [`play_patterns`] stands volleys
//! up on schedule and [`advance_volleys`] reaps the dead ones — both `pub fn`s a game registers in
//! its own schedule order. Hits, sides, damage, cancel and what appears in a cancelled bullet's
//! place are game code calling [`volley_hits`] and setting kill bits.
//!
//! **Every clock here is the gameplay one.** A timeline walks `ctx.game_frame`, a volley's
//! `started` is a gameplay frame, and every evaluator takes `ctx.game_instant` — so a game holding its
//! [`GameClock`](ludion_core::time::GameClock) freezes the whole field: no entry fires, no bullet
//! moves, nothing expires, and what a system hit-tests is exactly what the frame draws. A game that
//! never holds is unaffected, `game_*` being sim time exactly.

use std::collections::BTreeMap;
use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::glam::Vec3;
use ludion_core::math::{atan2, cos, sin};
use ludion_core::rng::DeterministicRng;
use ludion_core::suggest;
use ludion_core::time::{FrameIndex, GameTime, Q16_PER_TICK, TickRate};
use ludion_ecs::{SystemCtx, World};
use ludion_spatial::Plane;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use ludion_components::patterns::{PatternPlayer, VOLLEY_MAX_BULLETS, Volley, VolleyShape};

use crate::components::{SpriteBlend, Transform};
use crate::diagnostics::report;
use crate::docs::DocStore;
use crate::extract::AtlasAsset;
use crate::particles::{MOTION_MAX_SEGMENTS, Motion, Segment};

/// The most motion segments a bullet kind may author (`L1149`); one [`Motion`] slot is the tail.
pub const BULLET_MAX_SEGMENTS: usize = MOTION_MAX_SEGMENTS - 1;

/// Names the random stream of a `random`-shape volley's bullet `i`.
///
/// A distinct constant per draw site, [`crate::particles::PARTICLE_SALT_CONTINUOUS`]'s reason: a
/// volley and an emitter that happen to share a seed must not draw the same numbers.
pub const VOLLEY_SALT: u64 = 0x766f_6c6c_5f62_6c74;

/// Names the stream [`Volley::seed`] itself is drawn from when [`play_patterns`] stands a volley
/// up. See [`VOLLEY_SALT`].
pub const VOLLEY_SEED_SALT: u64 = 0x766f_6c6c_5f73_6564;

// ---------------------------------------------------------------------------------------------
// The document
// ---------------------------------------------------------------------------------------------

/// A `kind: pattern` document: a named set of bullet kinds and a timeline that fires them
/// (`Plan/projectile-patterns.md` decision 4).
///
/// One level deep, no expressions and no conditions — the vocabulary is a fixed enumerated set, and
/// anything beyond it is Rust. A game opts in with [`EngineBuilder::patterns`](crate::EngineBuilder::patterns),
/// which registers both the kind and [`validate_pattern_docs`].
///
/// ```yaml
/// kind: pattern
/// name: boss-phase-1
/// bullets:
///   needle:
///     atlas: art/bullets.atlas.yaml
///     frame: needle
///     size:
///       - 0.3
///       - 0.3
///     blend: additive
///     hit_radius: 0.12
///     life: 240
///     motion:
///       - ramp:
///           frames: 30
///           from: 6.0
///           to: 2.0
///       - turn:
///           frames: 90
///           rate_deg: 45.0
/// timeline:
///   - at: 0
///     repeat: 8
///     every: 12
///     shape: ring
///     count: 24
///     bullet: needle
///     speed: 4.0
///     angle_step_deg: 7.5
///   - at: 120
///     play: patterns/sweep-left.yaml
/// loop: 240
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
pub struct PatternDoc {
    /// The pattern's own name — identity for a designer and for a report; the engine resolves a
    /// pattern by its [`DocStore`] key (its content-root-relative path), never by this.
    #[serde(default)]
    pub name: String,
    /// The bullet kinds this document's timeline may name, keyed by the short name a timeline entry
    /// writes under `bullet:`. The vocabulary is local: a name from another document is exactly as
    /// unknown as a typo (`L1148`).
    #[serde(default)]
    pub bullets: BTreeMap<String, BulletKind>,
    /// What fires when, in frames after the [`PatternPlayer`]'s `started`.
    #[serde(default)]
    pub timeline: Vec<TimelineEntry>,
    /// How many frames the timeline spans before repeating (default `0` — play once). A boss phase
    /// is a looping pattern; a phase change is the game setting a different `pattern` on the player.
    #[serde(default, rename = "loop")]
    pub loop_frames: u32,
}

/// One kind of bullet: how it looks, how big its hit circle is, how long it lives, and the ≤ 4
/// motion segments its trajectory is made of.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct BulletKind {
    /// A `kind: atlas` document path (relative to the content root), or `None` for an untextured
    /// quad tinted by [`color`](BulletKind::color). Paired with [`frame`](BulletKind::frame) exactly
    /// as a [`Sprite`](crate::Sprite)'s is — both or neither (`L1151`); the pair is resolved when the
    /// document loads and reloads (`L1151`/`L1152`).
    #[serde(default)]
    pub atlas: Option<String>,
    /// The region name inside [`atlas`](BulletKind::atlas).
    #[serde(default)]
    pub frame: Option<String>,
    /// Quad size `[width, height]` in world units (default `[0.25, 0.25]`).
    #[serde(default = "bullet_size_default")]
    pub size: [f32; 2],
    /// RGBA tint multiplied into the sampled texel (default opaque white).
    #[serde(default = "white4")]
    pub color: [f32; 4],
    /// How the quad composites (default [`SpriteBlend::Alpha`]) — the sprite path's own flag, so a
    /// bullet swarm never splits a batch.
    #[serde(default)]
    pub blend: SpriteBlend,
    /// The sprite sort key (default `0`).
    #[serde(default)]
    pub layer: i16,
    /// Whether the quad faces the camera (default `false` — it lies in the game's
    /// [`Plane`], which is what a top-down or side-on field wants).
    #[serde(default)]
    pub billboard: bool,
    /// The bullet's collision radius in world units (default `0.1`), added to whatever radius a
    /// [`volley_hits`] caller asks about.
    #[serde(default = "bullet_hit_radius_default")]
    pub hit_radius: f32,
    /// How many frames the bullet lives after its volley fires (default `240` — four seconds).
    #[serde(default = "bullet_life_default")]
    pub life: u32,
    /// The trajectory's phases, in order (default empty — straight travel at the fire speed
    /// forever). At most four (`L1149`); past the last one the bullet continues straight at its end
    /// speed until `life`.
    #[serde(default)]
    pub motion: Vec<MotionSegment>,
}

impl Default for BulletKind {
    fn default() -> Self {
        BulletKind {
            atlas: None,
            frame: None,
            size: bullet_size_default(),
            color: white4(),
            blend: SpriteBlend::Alpha,
            layer: 0,
            billboard: false,
            hit_radius: bullet_hit_radius_default(),
            life: bullet_life_default(),
            motion: Vec::new(),
        }
    }
}

/// One phase of a bullet's trajectory: exactly one of `ramp` and `turn` (both, or neither, is
/// `L1149` — a ramping turn has no elementary closed form).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
pub struct MotionSegment {
    /// Straight travel with the speed sweeping linearly.
    #[serde(default)]
    pub ramp: Option<RampSpec>,
    /// A circular arc at constant speed.
    #[serde(default)]
    pub turn: Option<TurnSpec>,
}

/// `ramp { frames, from, to }` — straight along the current heading, speed sweeping from `from` to
/// `to`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
pub struct RampSpec {
    /// How long the phase lasts, in frames.
    #[serde(default)]
    pub frames: u32,
    /// Speed at the start, in units per second, or `None` to carry the speed in force when the
    /// phase begins.
    #[serde(default)]
    pub from: Option<f32>,
    /// Speed at the end, in units per second.
    #[serde(default)]
    pub to: f32,
}

/// `turn { frames, speed, rate_deg }` — a circular arc at constant speed, the heading rotating at
/// `rate_deg` degrees per second. A positive rate always turns toward increasing angle, on both
/// planes.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
pub struct TurnSpec {
    /// How long the phase lasts, in frames.
    #[serde(default)]
    pub frames: u32,
    /// Speed along the arc, in units per second, or `None` to carry the speed in force when the
    /// phase begins.
    #[serde(default)]
    pub speed: Option<f32>,
    /// Turn rate in **degrees** per second; negative turns the other way.
    #[serde(default)]
    pub rate_deg: f32,
}

/// What an `arc` volley's angle is measured from.
///
/// Doc comments on both variants, for [`VolleyShape`]'s reason.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum VolleyCenter {
    /// The authored `angle_deg`, as a world angle.
    #[default]
    Absolute,
    /// The direction from the player to its [`PatternPlayer::target`], captured at fire time, plus
    /// the authored `angle_deg`. A missing or dead target degrades to `absolute` with one `L2108`.
    Aim,
}

/// One timeline entry: either a volley (`bullet` and its shape parameters) or a `play` of another
/// pattern's timeline, offset by this entry's `at`.
///
/// One flat struct rather than a tagged enum, because the doc form stays one level deep and a
/// schema-driven form must be able to render it. `play` alongside any volley field, and neither set,
/// are both `L1149`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct TimelineEntry {
    /// The frame this entry fires on, counted from the [`PatternPlayer`]'s `started` (and, inside a
    /// played pattern, offset by the playing entry's own `at`).
    #[serde(default)]
    pub at: u32,
    /// Another pattern document's [`DocStore`] key, whose timeline plays offset by
    /// [`at`](TimelineEntry::at). Nesting reaches exactly one level (`L1150`).
    #[serde(default)]
    pub play: Option<String>,
    /// Which of the document's own `bullets:` this volley fires. Required for a volley (`L1148`
    /// when it names no kind).
    #[serde(default)]
    pub bullet: Option<String>,
    /// How many times the volley repeats (default `1`).
    #[serde(default = "one_u32")]
    pub repeat: u32,
    /// Frames between repeats (default `0`, which is only legal with `repeat: 1`).
    #[serde(default)]
    pub every: u32,
    /// How the bullets are laid out (default [`VolleyShape::Ring`]).
    #[serde(default)]
    pub shape: VolleyShape,
    /// What an `arc`'s angle is measured from (default [`VolleyCenter::Absolute`]).
    #[serde(default)]
    pub center: VolleyCenter,
    /// The cone width in degrees for `arc` and `random` (default `0`).
    #[serde(default)]
    pub spread_deg: f32,
    /// How many bullets the volley holds (default `1`, at most [`VOLLEY_MAX_BULLETS`]).
    #[serde(default = "one_u32")]
    pub count: u32,
    /// The fire speed in units per second, used when [`speed_range`](TimelineEntry::speed_range) is
    /// absent.
    #[serde(default)]
    pub speed: f32,
    /// A per-bullet-index speed range that **replaces** [`speed`](TimelineEntry::speed): bullet `i`
    /// of `count` fires at `lerp(range[0], range[1], i / (count − 1))`, and a single bullet at
    /// `range[0]`.
    #[serde(default)]
    pub speed_range: Option<[f32; 2]>,
    /// The volley's angle in degrees (default `0`), measured per [`center`](TimelineEntry::center).
    #[serde(default)]
    pub angle_deg: f32,
    /// Added to the angle once per repeat (default `0`) — the spiral.
    #[serde(default)]
    pub angle_step_deg: f32,
    /// Added to both ends of the speed range once per repeat (default `0`).
    #[serde(default)]
    pub speed_step: f32,
}

impl Default for TimelineEntry {
    fn default() -> Self {
        TimelineEntry {
            at: 0,
            play: None,
            bullet: None,
            repeat: 1,
            every: 0,
            shape: VolleyShape::Ring,
            center: VolleyCenter::Absolute,
            spread_deg: 0.0,
            count: 1,
            speed: 0.0,
            speed_range: None,
            angle_deg: 0.0,
            angle_step_deg: 0.0,
            speed_step: 0.0,
        }
    }
}

impl TimelineEntry {
    /// Whether this entry authors anything only a volley has — the test that turns "a `play` with a
    /// `count` on it" from a silently-ignored field into an `L1149`.
    fn names_a_volley(&self) -> bool {
        let default = TimelineEntry::default();
        self.bullet.is_some()
            || self.speed_range.is_some()
            || self.repeat != default.repeat
            || self.every != default.every
            || self.shape != default.shape
            || self.center != default.center
            || self.spread_deg != default.spread_deg
            || self.count != default.count
            || self.speed != default.speed
            || self.angle_deg != default.angle_deg
            || self.angle_step_deg != default.angle_step_deg
            || self.speed_step != default.speed_step
    }

    /// The `[min, max]` speed range repeat `r` fires with: the authored range (or the flat
    /// `[speed, speed]`) shifted by `speed_step × r`.
    fn speed_range_at(&self, repeat: u32) -> [f32; 2] {
        let base = self.speed_range.unwrap_or([self.speed, self.speed]);
        let shift = self.speed_step * repeat as f32;
        [base[0] + shift, base[1] + shift]
    }
}

fn bullet_size_default() -> [f32; 2] {
    [0.25, 0.25]
}

fn white4() -> [f32; 4] {
    [1.0; 4]
}

fn bullet_hit_radius_default() -> f32 {
    0.1
}

fn bullet_life_default() -> u32 {
    240
}

fn one_u32() -> u32 {
    1
}

// ---------------------------------------------------------------------------------------------
// The components
// ---------------------------------------------------------------------------------------------

// ---------------------------------------------------------------------------------------------
// The evaluator
// ---------------------------------------------------------------------------------------------

/// How many seconds `frames` ticks are at `rate`.
fn frames_seconds(frames: u32, rate: TickRate) -> f32 {
    (f64::from(frames) * rate.seconds_per_tick()) as f32
}

/// Q16 ticks from `started` to `now`, or `None` before `started` — the exact integer a bullet's
/// `life` (in whole ticks, times [`Q16_PER_TICK`]) is compared with, so a frame count never
/// round-trips through seconds.
fn age_q16(now: GameTime, started: FrameIndex) -> Option<u64> {
    now.as_q16()
        .checked_sub(started.as_u64().saturating_mul(Q16_PER_TICK))
}

/// Whether a bullet of `life` ticks has outlived `age` (a Q16 age).
fn expired(age: u64, life: u32) -> bool {
    age >= u64::from(life) * Q16_PER_TICK
}

/// Signed seconds between `started` and `now` — [`crate::particles`]'s own helper, mirrored so the
/// two evaluators measure age the same way.
fn seconds_since(now: GameTime, started: FrameIndex) -> f64 {
    now.ticks_since(started) * now.rate().seconds_per_tick()
}

/// How long `volley` has been in flight at `game_now`, in seconds — **negative** before it fires.
///
/// `game_now` is **gameplay** time: a system passes `ctx.game_instant`, never `ctx.time`, so a bullet
/// stops ageing while the game is held (a volley's `started` is a gameplay frame too).
pub fn bullet_age(volley: &Volley, game_now: impl Into<GameTime>) -> f32 {
    seconds_since(game_now.into(), volley.started) as f32
}

/// The launch angle (radians, `0` along the plane's first axis) and speed of bullet `index`.
///
/// The three shapes differ only here: a ring spreads over the full turn, an arc over `spread_deg`
/// centred on the volley's angle, and a random volley draws both numbers — **angle first, then
/// speed**, an order that is load-bearing because changing it re-rolls every random volley in the
/// repo.
fn bullet_launch(volley: &Volley, index: u32) -> (f32, f32) {
    let base = volley.angle_deg.to_radians();
    let spread = volley.spread_deg.to_radians();
    match volley.shape {
        VolleyShape::Ring => {
            let step = if volley.count == 0 {
                0.0
            } else {
                std::f32::consts::TAU / volley.count as f32
            };
            (base + step * index as f32, indexed_speed(volley, index))
        }
        VolleyShape::Arc => {
            let offset = if volley.count <= 1 {
                0.0
            } else {
                -0.5 * spread + spread * index as f32 / (volley.count - 1) as f32
            };
            (base + offset, indexed_speed(volley, index))
        }
        VolleyShape::Random => {
            let mut rng = DeterministicRng::derived_salted(
                volley.seed,
                FrameIndex::new(u64::from(index)),
                VOLLEY_SALT,
            );
            let angle = base - 0.5 * spread + spread * rng.next_f32();
            let speed = lerp(volley.speed[0], volley.speed[1], rng.next_f32());
            (angle, speed)
        }
    }
}

/// Bullet `index`'s speed on the deterministic shapes: the volley's range walked linearly over the
/// bullet indices, a single bullet taking the range's low end.
fn indexed_speed(volley: &Volley, index: u32) -> f32 {
    if volley.count <= 1 {
        return volley.speed[0];
    }
    lerp(
        volley.speed[0],
        volley.speed[1],
        index as f32 / (volley.count - 1) as f32,
    )
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// The trajectory of bullet `index`, ready for [`Motion::position_at`].
///
/// The heading starts at `bullet_launch`'s angle turned into a world direction —
/// `plane.right()·cos θ + plane.forward()·sin θ` — and each of the kind's segments carries the
/// heading and speed the previous one ended at, so an omitted `from`/`speed` means "keep going at
/// whatever we are doing". A `turn` rotates about `right × forward`, which is the plane's normal
/// *with the sign that makes a positive `rate_deg` turn toward increasing θ on both planes*
/// ([`Plane::normal`] is `+Y` on `Xz`, which would turn the other way).
///
/// A trailing open-ended straight leg keeps the bullet moving after its last authored segment. The
/// [`Motion`] capacity always leaves room for it: authored segments past [`BULLET_MAX_SEGMENTS`]
/// (which the validator refuses from a document) are ignored.
pub(crate) fn bullet_motion(
    volley: &Volley,
    origin: Vec3,
    kind: &BulletKind,
    index: u32,
    plane: Plane,
    rate: TickRate,
) -> Motion {
    let (angle, launch_speed) = bullet_launch(volley, index);
    let right = Vec3::from(plane.right());
    let forward = Vec3::from(plane.forward());
    let normal = right.cross(forward);

    let mut heading = right * cos(angle) + forward * sin(angle);
    let mut speed = launch_speed;
    let mut motion = Motion::new(origin);
    for segment in kind.motion.iter().take(BULLET_MAX_SEGMENTS) {
        let phase = match (&segment.ramp, &segment.turn) {
            (Some(ramp), None) => Segment::Ramp {
                heading,
                from: ramp.from.unwrap_or(speed),
                to: ramp.to,
                duration: frames_seconds(ramp.frames, rate),
            },
            (None, Some(turn)) => Segment::Turn {
                heading,
                normal,
                speed: turn.speed.unwrap_or(speed),
                rate: turn.rate_deg.to_radians(),
                duration: frames_seconds(turn.frames, rate),
            },
            // Both or neither is `L1149` at document time; a kind built in Rust degrades to a
            // phase that changes nothing rather than to a panic.
            _ => continue,
        };
        let pushed = motion.push(phase);
        debug_assert!(
            pushed,
            "authored phases are capped below the motion capacity"
        );
        heading = phase.end_heading();
        speed = phase.end_speed();
    }
    let pushed = motion.push(Segment::straight(heading, speed));
    debug_assert!(pushed, "the motion capacity leaves a slot for the tail");
    motion
}

/// Where bullet `index` of `volley` is at `game_now`, given the volley's captured `origin`.
///
/// `game_now` is **gameplay** time: a system passes `ctx.game_instant`, never `ctx.time`, so the
/// position it computes is the one the frame draws — held or not. `origin` is the `Transform` the
/// volley fired from, the plain-array idiom every other game-facing call uses (`volley_hits`,
/// `spatial::overlaps`, `Transform.translation`) — a game never needs `glam` for this.
pub fn bullet_position(
    volley: &Volley,
    origin: &Transform,
    kind: &BulletKind,
    index: u32,
    game_now: impl Into<GameTime>,
    plane: Plane,
) -> [f32; 3] {
    let game_now = game_now.into();
    bullet_motion(
        volley,
        Vec3::from(origin.translation),
        kind,
        index,
        plane,
        game_now.rate(),
    )
    .position_at(bullet_age(volley, game_now))
    .to_array()
}

/// Whether bullet `index` is on the field at `game_now`: inside the volley's `count`, within its
/// kind's `life`, not yet fired-in-the-future, and carrying no kill bit.
///
/// `game_now` is **gameplay** time: a system passes `ctx.game_instant`, never `ctx.time`, so a held
/// game's bullets neither expire nor appear.
pub fn bullet_is_alive(
    volley: &Volley,
    kind: &BulletKind,
    index: u32,
    game_now: impl Into<GameTime>,
) -> bool {
    if index >= volley.count || volley.is_killed(index) {
        return false;
    }
    age_q16(game_now.into(), volley.started).is_some_and(|age| !expired(age, kind.life))
}

/// Every live bullet of `volley` whose in-plane position at `game_now` lies within
/// `radius + kind.hit_radius` of `centre` — the pure overlap query a game's hit systems run, and
/// the one that must agree with what the frame draws (it evaluates the same `bullet_motion`).
///
/// `game_now` is **gameplay** time: a system passes `ctx.game_instant`, never `ctx.time`. Passing sim
/// time would hit-test a field the frozen frame is not drawing.
///
/// `centre` is a point in the plane's own two axes, the pair
/// [`GroundPosition`](ludion_spatial::GroundPosition) speaks in. Volleys hold no collider and never
/// enter the spatial index; this walk is `count` closed-form evaluations and nothing else.
pub fn volley_hits(
    volley: &Volley,
    origin: &Transform,
    kind: &BulletKind,
    game_now: impl Into<GameTime>,
    plane: Plane,
    centre: [f32; 2],
    radius: f32,
) -> impl Iterator<Item = u32> {
    let game_now = game_now.into();
    let origin = Vec3::from(origin.translation);
    let age = bullet_age(volley, game_now);
    let axes = plane.axes();
    let reach = radius + kind.hit_radius;
    (0..volley.count).filter(move |&index| {
        if !bullet_is_alive(volley, kind, index, game_now) {
            return false;
        }
        let position = bullet_motion(volley, origin, kind, index, plane, game_now.rate())
            .position_at(age)
            .to_array();
        let (dx, dy) = (position[axes[0]] - centre[0], position[axes[1]] - centre[1]);
        dx * dx + dy * dy <= reach * reach
    })
}

// ---------------------------------------------------------------------------------------------
// The systems
// ---------------------------------------------------------------------------------------------

/// Names a volley's own random stream: a fold of the firing player's generational id, the gameplay
/// frame the volley fired on, and its ordinal in that frame's timeline walk — so two volleys fired
/// by one player on one frame, and the same volley fired on two loops, all draw independently. The
/// frame is the gameplay one, so a volley's stream does not depend on how long the game was held
/// before it fired.
fn volley_seed(player: EntityId, frame: FrameIndex, ordinal: u32) -> u64 {
    let player_word = (u64::from(player.index) << 32) | u64::from(player.generation);
    DeterministicRng::derived_salted(player_word, frame, VOLLEY_SEED_SALT ^ u64::from(ordinal))
        .next_u64()
}

/// Which pattern systems have run in this world. Observation only: a resource, so never hashed or
/// snapshotted; [`report_unrun_patterns`] reads it to tell a forgotten system from a held frame.
#[derive(Default, Clone, Copy)]
pub(crate) struct PatternSystemsRan {
    play: bool,
    advance: bool,
}

/// Records that a pattern system ran. Only the first call inserts the resource, so an ordinary tick
/// allocates nothing.
fn mark_ran(world: &mut World, flag: fn(&mut PatternSystemsRan) -> &mut bool) {
    if let Some(ran) = world.resource_mut::<PatternSystemsRan>() {
        *flag(ran) = true;
        return;
    }
    let mut ran = PatternSystemsRan::default();
    *flag(&mut ran) = true;
    world.insert_resource(ran);
}

/// Reports a pattern system the world needs and the schedule never ran, once per system (`L1394`):
/// a [`PatternPlayer`] naming a pattern with no `play_patterns`, a [`Volley`] with no
/// `advance_volleys`. A bare host's empty schedule is not a game that forgot a system, so
/// `game_systems` gates both.
///
/// It runs only after a tick (every diagnostics drain is behind one), so a paused or just-loaded
/// engine never reaches it. **Known limit:** a wrapper that calls `play_patterns` only behind its
/// own guard, such as a menu state, warns until its first call.
pub(crate) fn report_unrun_patterns(world: &mut World, game_systems: bool) {
    if !game_systems {
        return;
    }
    let ran = world
        .resource::<PatternSystemsRan>()
        .copied()
        .unwrap_or_default();
    if !ran.play
        && world.component_count::<PatternPlayer>() > 0
        && !crate::diagnostics::is_latched(world, code::PATTERN_SYSTEM_NOT_RUN, "play_patterns")
    {
        let player = world
            .query_ids::<(&PatternPlayer,)>()
            .find(|(_, (player,))| !player.pattern.is_empty())
            .map(|(id, _)| id);
        if let Some(id) = player {
            let named = crate::scene_physics::entity_label(world, id);
            report(
                world,
                code::PATTERN_SYSTEM_NOT_RUN,
                "play_patterns",
                format!(
                    "{named} has a `PatternPlayer`, but no `play_patterns` system ever ran, so it \
                     fires nothing: register `.system(\"play_patterns\", play_patterns)` on the \
                     `EngineBuilder`"
                ),
            );
        }
    }
    if !ran.advance
        && world.component_count::<Volley>() > 0
        && !crate::diagnostics::is_latched(world, code::PATTERN_SYSTEM_NOT_RUN, "advance_volleys")
    {
        let volley = world.query_ids::<(&Volley,)>().next().map(|(id, _)| id);
        if let Some(id) = volley {
            let named = crate::scene_physics::entity_label(world, id);
            report(
                world,
                code::PATTERN_SYSTEM_NOT_RUN,
                "advance_volleys",
                format!(
                    "{named} is a `Volley`, but no `advance_volleys` system ever ran, so spent \
                     volleys are never reaped: register \
                     `.system(\"advance_volleys\", advance_volleys)` after the hit systems"
                ),
            );
        }
    }
}

/// Stands volleys up on schedule: for every [`PatternPlayer`] + [`Transform`], walks its
/// [`PatternDoc`]'s timeline at `t = gameplay frame − started` (modulo `loop`) and spawns one
/// [`Volley`] entity per entry whose frame has come (`Plan/projectile-patterns.md` decision 6).
///
/// A game registers it in its own schedule order, the `spatial_rebuild` precedent. A missing
/// document, a missing bullet kind, an unresolvable `play` and a missing or dead aim target each
/// degrade — the volley is skipped, or fires at its absolute angle — and report `L2108` once per
/// `(player entity, reason)` through the [`Diagnostics`](crate::Diagnostics) latch.
///
/// **The timeline runs on gameplay time.** A frame the game held is one it has already fired (the
/// same gameplay frame reaches the system again), so this returns immediately on it: a held run
/// neither re-fires an entry nor silently passes one whose frame went by while it was frozen.
pub fn play_patterns(world: &mut World, ctx: &SystemCtx) {
    mark_ran(world, |ran| &mut ran.play);
    if ctx.game_frames_crossed() == 0 {
        return;
    }
    let mut spawns: Vec<(Transform, Volley)> = Vec::new();
    // `(offender, key suffix, message)`: the key is built from the offender at the report loop.
    let mut warnings: Vec<(EntityId, String, String)> = Vec::new();

    // The players come out of the world before the [`DocStore`] does: the store is a shared borrow
    // held across the whole walk, and a typed query needs `&mut World`. A player's record is one
    // string and there are few of them, so copying them out costs far less than probing every entity
    // in the world for a component almost none of them carry.
    let players: Vec<(EntityId, PatternPlayer)> = world
        .query_ids::<(&PatternPlayer, &Transform)>()
        .filter(|(_, (player, _))| !player.pattern.is_empty())
        .map(|(id, (player, _))| (id, player.clone()))
        .collect();
    // A volley fires from the player's world position, however deep in a hierarchy it sits.
    let players: Vec<(EntityId, PatternPlayer, [f32; 3])> = players
        .into_iter()
        .filter_map(|(id, player)| {
            let at = crate::hierarchy::world_transform(world, id)?
                .trs
                .translation;
            Some((id, player, at))
        })
        .collect();

    {
        let plane = Plane::of(world);
        let store = world.resource::<DocStore<PatternDoc>>();

        for (id, player, origin) in &players {
            let id = *id;
            let Some(doc) = store.and_then(|store| store.get(&player.pattern)) else {
                warnings.push((
                    id,
                    "pattern".to_string(),
                    format!(
                        "PatternPlayer on {id} names pattern `{}`, which is not a loaded \
                         `kind: pattern` document; the player fires nothing",
                        player.pattern
                    ),
                ));
                continue;
            };

            let Some(t) = elapsed_frames(ctx.game_frame, player.started, doc.loop_frames) else {
                continue;
            };

            // Resolved once per player per frame: an aimed volley captures this angle, and a target
            // that is gone degrades the whole player to its absolute angles.
            let aim_deg = player.target.and_then(|target| {
                let target_at = crate::hierarchy::world_transform(world, target)?
                    .trs
                    .translation;
                Some(aim_degrees(plane, *origin, target_at))
            });

            let mut walk = Walk {
                ctx,
                t,
                player: id,
                translation: *origin,
                aim_deg,
                spawns: &mut spawns,
                warnings: &mut warnings,
                ordinal: 0,
            };
            for entry in &doc.timeline {
                match entry.play.as_deref() {
                    None => walk.consider(&player.pattern, doc, entry, entry.at),
                    Some(play) => {
                        let Some(played) = store.and_then(|store| store.get(play)) else {
                            walk.warnings.push((
                                id,
                                format!("play:{play}"),
                                format!(
                                    "pattern `{}` plays `{play}`, which is not a loaded \
                                     `kind: pattern` document; the entry fires nothing",
                                    player.pattern
                                ),
                            ));
                            continue;
                        };
                        // One level deep (`L1150`): a played pattern's own `play` entries are not
                        // followed, so a cycle cannot run even if one somehow loaded.
                        for nested in played.timeline.iter().filter(|e| e.play.is_none()) {
                            walk.consider(play, played, nested, entry.at.saturating_add(nested.at));
                        }
                    }
                }
            }
        }
    }

    for (transform, volley) in spawns {
        let entity = world.spawn();
        let _ = world.insert(entity, transform);
        let _ = world.insert(entity, volley);
    }
    for (offender, suffix, message) in warnings {
        let key = format!(
            "{}/{suffix}",
            crate::diagnostics::offender_key(world, offender)
        );
        report(world, code::VOLLEY_INVALID, key, message);
    }
}

/// Where a player's timeline is at `frame` — a **gameplay** frame — in frames since `started`, or
/// `None` before it starts.
fn elapsed_frames(frame: FrameIndex, started: FrameIndex, loop_frames: u32) -> Option<u64> {
    let elapsed = frame.as_u64().checked_sub(started.as_u64())?;
    Some(match loop_frames {
        0 => elapsed,
        period => elapsed % u64::from(period),
    })
}

/// The angle in degrees from `from` to `to`, measured in `plane`'s own two axes — what a
/// `center: aim` volley captures at fire time.
fn aim_degrees(plane: Plane, from: [f32; 3], to: [f32; 3]) -> f32 {
    let axes = plane.axes();
    atan2(to[axes[1]] - from[axes[1]], to[axes[0]] - from[axes[0]]).to_degrees()
}

/// One player's timeline walk: the state [`play_patterns`] threads through every entry it considers,
/// including a played pattern's.
struct Walk<'a> {
    ctx: &'a SystemCtx,
    /// Gameplay frames since the player's `started`, already folded into `loop`.
    t: u64,
    player: EntityId,
    translation: [f32; 3],
    /// The captured aim angle in degrees, or `None` when the player has no live target.
    aim_deg: Option<f32>,
    spawns: &'a mut Vec<(Transform, Volley)>,
    warnings: &'a mut Vec<(EntityId, String, String)>,
    /// Counts every candidate volley the walk passes — fired or not, an entry's `repeat` candidates
    /// in order — so a volley's seed does not depend on which of its siblings happened to be due
    /// this frame.
    ordinal: u32,
}

impl Walk<'_> {
    /// Considers one timeline entry of the document keyed `pattern`, whose fire offset is `at`
    /// (already carrying the offset of the `play` entry that reached it), standing a [`Volley`] up
    /// for the repeat whose frame is now — in constant time, however long the train. An entry with
    /// `every: 0` fires only its first repeat, on frame `at`.
    fn consider(&mut self, pattern: &str, doc: &PatternDoc, entry: &TimelineEntry, at: u32) {
        let base = self.ordinal;
        self.ordinal = self.ordinal.wrapping_add(entry.repeat);
        let Some(since) = self.t.checked_sub(u64::from(at)) else {
            return;
        };
        if entry.every == 0 {
            // Authored `every: 0` is `L1149`; a doc that got past the validator fires its one volley
            // rather than looping `repeat` times in a tick.
            if since == 0 && entry.repeat > 0 {
                self.fire(pattern, doc, entry, 0, base);
            }
            return;
        }
        let every = u64::from(entry.every);
        if since % every != 0 {
            return;
        }
        let repeat = since / every;
        if repeat < u64::from(entry.repeat) {
            let repeat = repeat as u32;
            self.fire(pattern, doc, entry, repeat, base.wrapping_add(repeat));
        }
    }

    /// Stands up the volley of `entry`'s repeat number `repeat`, whose candidate ordinal is `ordinal`.
    fn fire(
        &mut self,
        pattern: &str,
        doc: &PatternDoc,
        entry: &TimelineEntry,
        repeat: u32,
        ordinal: u32,
    ) {
        let Some(name) = entry.bullet.as_deref() else {
            return;
        };
        if !doc.bullets.contains_key(name) {
            self.warnings.push((
                self.player,
                format!("bullet:{name}"),
                format!(
                    "pattern `{pattern}` fires bullet kind `{name}`, which its own `bullets:` \
                     map does not define; the volley is skipped"
                ),
            ));
            return;
        }

        let aim = match entry.center {
            VolleyCenter::Absolute => 0.0,
            VolleyCenter::Aim => match self.aim_deg {
                Some(aim) => aim,
                None => {
                    self.warnings.push((
                        self.player,
                        "target".to_string(),
                        format!(
                            "pattern `{pattern}` aims a volley at the player's `target`, which \
                             is missing or dead; it fires at its authored absolute angle instead"
                        ),
                    ));
                    0.0
                }
            },
        };

        self.spawns.push((
            Transform {
                translation: self.translation,
                rotation: [0.0; 3],
                scale: [1.0; 3],
            },
            Volley {
                source: Some(self.player),
                pattern: pattern.to_string(),
                bullet: name.to_string(),
                started: self.ctx.game_frame,
                shape: entry.shape,
                count: entry.count,
                angle_deg: aim + entry.angle_deg + entry.angle_step_deg * repeat as f32,
                spread_deg: entry.spread_deg,
                speed: entry.speed_range_at(repeat),
                seed: volley_seed(self.player, self.ctx.game_frame, ordinal),
                killed: [0; 4],
            },
        ));
    }
}

/// Reaps spent volleys: despawns every [`Volley`] entity whose bullets are all killed or whose age
/// has reached its bullet kind's `life`.
///
/// A game registers it after its own hit systems, so a volley emptied this frame is gone by the
/// next. A volley naming a document or bullet kind that is not loaded cannot be evaluated at all
/// and is reaped with one `L2108` per `(volley entity, reason)`; so is one built in Rust with a
/// non-finite angle, spread or speed, which no validator saw.
///
/// Ages are measured on gameplay time, so nothing expires while the game is frozen. Unlike
/// [`play_patterns`] this runs on every frame — reaping is idempotent, and a volley killed during a
/// hold should leave the field as soon as the hit that emptied it lands.
pub fn advance_volleys(world: &mut World, ctx: &SystemCtx) {
    mark_ran(world, |ran| &mut ran.advance);
    let mut reap: Vec<EntityId> = Vec::new();
    let mut warnings: Vec<(String, String)> = Vec::new();

    // Only the ids come out of the typed query, and each volley is read back inside the [`DocStore`]
    // borrow that a `&mut World` query cannot coexist with: a volley holds two strings, so copying
    // the component out per entity would cost more than the read-back does. Both forms enumerate the
    // same volleys in the same ascending order.
    let ids: Vec<EntityId> = world.query_ids::<(&Volley,)>().map(|(id, _)| id).collect();

    {
        let store = world.resource::<DocStore<PatternDoc>>();
        for id in ids {
            let Some(volley) = world.get::<Volley>(id).ok().flatten() else {
                continue;
            };
            if let Some(field) = volley_non_finite_field(volley) {
                warnings.push((
                    format!("volley:{}/{}/{field}", volley.pattern, volley.bullet),
                    format!(
                        "volley {id} has a non-finite `{field}`, so every bullet position would \
                         be NaN; it is reaped"
                    ),
                ));
                reap.push(id);
                continue;
            }
            let kind = store
                .and_then(|store| store.get(&volley.pattern))
                .and_then(|doc| doc.bullets.get(&volley.bullet));
            let Some(kind) = kind else {
                warnings.push((
                    format!("volley:{}/{}/kind", volley.pattern, volley.bullet),
                    format!(
                        "volley {id} names bullet kind `{}` of pattern `{}`, which is not loaded; \
                         it cannot be drawn or hit-tested and is reaped",
                        volley.bullet, volley.pattern
                    ),
                ));
                reap.push(id);
                continue;
            };
            if volley.all_killed()
                || age_q16(ctx.game_instant, volley.started)
                    .is_some_and(|age| expired(age, kind.life))
            {
                reap.push(id);
            }
        }
    }

    for id in reap {
        let _ = world.despawn(id);
    }
    for (key, message) in warnings {
        report(world, code::VOLLEY_INVALID, key, message);
    }
}

/// The first of a volley's `angle_deg`, `spread_deg` or `speed` that is not finite, by name.
fn volley_non_finite_field(volley: &Volley) -> Option<&'static str> {
    if !volley.angle_deg.is_finite() {
        Some("angle_deg")
    } else if !volley.spread_deg.is_finite() {
        Some("spread_deg")
    } else if !volley.speed.iter().all(|s| s.is_finite()) {
        Some("speed")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------------------------
// The document validator
// ---------------------------------------------------------------------------------------------

/// Every rule a `kind: pattern` document must satisfy that its JSON Schema cannot state
/// ([`EngineBuilder::patterns`](crate::EngineBuilder::patterns) registers it):
///
/// - `L1148` — a timeline entry's `bullet` names no kind in the document's own `bullets:` map,
///   with a did-you-mean over those names.
/// - `L1149` — a bound the schema cannot express: a motion list over
///   [`BULLET_MAX_SEGMENTS`], a segment with both `ramp` and `turn` or neither, a `count` of zero
///   or above [`VOLLEY_MAX_BULLETS`], `repeat: 0`, `every: 0` with `repeat > 1`, a `play` that also
///   authors volley fields, an entry that is neither, a `life` of zero, a non-positive
///   `hit_radius`, and any non-finite or negative speed (or non-finite angle).
/// - `L1150` — a `play` that does not resolve: an unloaded key, the document itself, or a document
///   whose own timeline holds a `play` (nesting is one level deep).
///
/// Every diagnostic carries the document path `pattern/<doc key>/<field…>`, which is also the
/// once-per-offender print key.
pub fn validate_pattern_docs(world: &World) -> Vec<Diagnostic> {
    let Some(store) = world.resource::<DocStore<PatternDoc>>() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, doc) in store.iter() {
        validate_doc(store, key, doc, &mut out);
    }
    out
}

/// The `L1148`–`L1150`/`L1265` set of one document, `play` leaves resolved from `store`.
fn validate_doc(
    store: &DocStore<PatternDoc>,
    key: &str,
    doc: &PatternDoc,
    out: &mut Vec<Diagnostic>,
) {
    for (name, kind) in &doc.bullets {
        validate_bullet_kind(key, name, kind, out);
    }
    for (index, entry) in doc.timeline.iter().enumerate() {
        validate_entry(store, key, doc, index, entry, out);
    }
}

/// `L1151`/`L1152` for every bullet kind whose `atlas`/`frame` pair does not resolve — the same
/// resolution a render performs, so a kind that loads clean is textured on the field.
///
/// Separate from [`validate_pattern_docs`] because resolving an atlas reads the content root, which
/// a registered `fn(&World)` validator never sees; [`run_validators`](crate::docs::run_validators)
/// runs this pass after the registered ones. A live reload that newly breaks an atlas is refused by
/// the reload gate; one that breaks underneath a live doc degrades at extract.
pub(crate) fn validate_bullet_atlases(project_root: &Path, world: &World) -> Vec<Diagnostic> {
    let Some(store) = world.resource::<DocStore<PatternDoc>>() else {
        return Vec::new();
    };
    // One decode per atlas for the whole pass, however many kinds name it.
    let mut loaded = BTreeMap::new();
    store
        .iter()
        .flat_map(|(key, doc)| bullet_atlas_diagnostics(project_root, key, doc, &mut loaded))
        .collect()
}

type LoadedAtlases = BTreeMap<String, Result<AtlasAsset, Vec<Diagnostic>>>;

/// The `L1151`/`L1152` set of one pattern document's bullet kinds, at
/// `pattern/<key>/bullets/<name>/…`; `loaded` memoises each atlas's load across documents.
fn bullet_atlas_diagnostics(
    project_root: &Path,
    key: &str,
    doc: &PatternDoc,
    loaded: &mut LoadedAtlases,
) -> Vec<Diagnostic> {
    let root = ludion_assets::AssetRoot::content(project_root);
    let mut out = Vec::new();
    for (name, kind) in &doc.bullets {
        let base = format!("pattern/{key}/bullets/{name}");
        match (&kind.atlas, &kind.frame) {
            (Some(_), None) => out.push(crate::extract::atlas_without_frame(&base)),
            (Some(atlas), Some(frame)) => {
                let result = loaded
                    .entry(atlas.clone())
                    .or_insert_with(|| crate::extract::load_atlas(root, atlas));
                out.extend(crate::extract::check_loaded_atlas(
                    root, &base, atlas, frame, result,
                ));
            }
            (None, Some(_)) => out.push(crate::extract::frame_without_atlas(&base)),
            (None, None) => {}
        }
    }
    out
}

/// The frame of an entry's last repeat, relative to its pattern's start — what `Walk::consider`
/// compares against the folded time.
fn last_fire(entry: &TimelineEntry) -> u64 {
    u64::from(entry.at) + u64::from(entry.repeat.saturating_sub(1)) * u64::from(entry.every)
}

/// `L1265`, pointed at `pattern/<key>/<field…>`.
fn authoring_lint(path: String, message: String, help: &str) -> Diagnostic {
    Diagnostic::new(code::PATTERN_AUTHORING_LINT, message)
        .with_path(path)
        .with_help(help.to_string())
}

/// `L1149`, pointed at `pattern/<key>/<field…>`.
fn entry_invalid(path: String, message: String, help: &str) -> Diagnostic {
    Diagnostic::new(code::PATTERN_ENTRY_INVALID, message)
        .with_path(path)
        .with_help(help.to_string())
}

/// The `L1149` set a bullet kind can trip: its motion list's length and shape, and the three
/// scalars a schemars bound cannot reach.
fn validate_bullet_kind(key: &str, name: &str, kind: &BulletKind, out: &mut Vec<Diagnostic>) {
    let base = format!("pattern/{key}/bullets/{name}");
    if kind.motion.len() > BULLET_MAX_SEGMENTS {
        out.push(entry_invalid(
            format!("{base}/motion"),
            format!(
                "bullet kind `{name}` authors {} motion segments; a trajectory holds at most \
                 {BULLET_MAX_SEGMENTS}",
                kind.motion.len()
            ),
            "merge phases, or split the bullet into two kinds fired by two volleys",
        ));
    }
    for (index, segment) in kind.motion.iter().enumerate() {
        let at = format!("{base}/motion/{index}");
        match (&segment.ramp, &segment.turn) {
            (Some(_), Some(_)) => out.push(entry_invalid(
                at,
                format!(
                    "bullet kind `{name}` segment {index} authors both `ramp` and `turn`; a \
                     ramping arc has no elementary closed form"
                ),
                "keep one of `ramp`/`turn` per segment — a ramp then a turn is two segments",
            )),
            (None, None) => out.push(entry_invalid(
                at,
                format!("bullet kind `{name}` segment {index} authors neither `ramp` nor `turn`"),
                "give the segment exactly one `ramp:` (frames, to) or one `turn:` (frames, \
                 rate_deg)",
            )),
            (Some(ramp), None) => {
                if let Some(from) = ramp.from {
                    check_speed(&format!("{at}/ramp/from"), from, out);
                }
                check_speed(&format!("{at}/ramp/to"), ramp.to, out);
            }
            (None, Some(turn)) => {
                if let Some(speed) = turn.speed {
                    check_speed(&format!("{at}/turn/speed"), speed, out);
                }
                if !turn.rate_deg.is_finite() {
                    out.push(entry_invalid(
                        format!("{at}/turn/rate_deg"),
                        format!("turn rate {} is not a finite number", turn.rate_deg),
                        "give the turn a finite rate in degrees per second",
                    ));
                }
            }
        }
    }
    out.extend(crate::components::color_diagnostics(
        "pattern bullet kind",
        &format!("{base}/color"),
        &kind.color.map(f64::from),
    ));
    if kind.life == 0 {
        out.push(entry_invalid(
            format!("{base}/life"),
            format!("bullet kind `{name}` lives 0 frames, so it is never on the field"),
            "give `life` a positive frame count (240 is four seconds)",
        ));
    }
    if kind.hit_radius <= 0.0 || !kind.hit_radius.is_finite() {
        out.push(entry_invalid(
            format!("{base}/hit_radius"),
            format!(
                "bullet kind `{name}` has hit_radius {}, so it can never hit anything",
                kind.hit_radius
            ),
            "give `hit_radius` a positive world-unit radius",
        ));
    }
}

/// Pushes an `L1149` if `speed` is not a finite, non-negative number.
fn check_speed(path: &str, speed: f32, out: &mut Vec<Diagnostic>) {
    if !speed.is_finite() || speed < 0.0 {
        out.push(entry_invalid(
            path.to_string(),
            format!("speed {speed} is not valid; a speed is a finite, non-negative units-per-second value"),
            "give the field a value of 0 or more (a bullet travelling backwards is a negative angle)",
        ));
    }
}

/// The `L1148`/`L1149`/`L1150` set one timeline entry can trip.
fn validate_entry(
    store: &DocStore<PatternDoc>,
    key: &str,
    doc: &PatternDoc,
    index: usize,
    entry: &TimelineEntry,
    out: &mut Vec<Diagnostic>,
) {
    let base = format!("pattern/{key}/timeline/{index}");

    if let Some(play) = entry.play.as_deref() {
        if entry.names_a_volley() {
            out.push(entry_invalid(
                format!("{base}/play"),
                format!(
                    "timeline entry {index} plays `{play}` and also authors volley fields; an \
                     entry is one or the other"
                ),
                "drop the volley fields, or drop `play` and give the entry a `bullet`",
            ));
        }
        if let Some(played) = validate_play(store, key, play, &base, index, out) {
            validate_played_loop(doc, played, entry, &base, index, out);
        }
        return;
    }

    let Some(bullet) = entry.bullet.as_deref() else {
        out.push(entry_invalid(
            format!("{base}/bullet"),
            format!("timeline entry {index} is neither a volley nor a `play`"),
            "give the entry a `bullet: <kind>` from this document's `bullets:` map, or a \
             `play: <pattern key>`",
        ));
        return;
    };

    if !doc.bullets.contains_key(bullet) {
        let names: Vec<&str> = doc.bullets.keys().map(String::as_str).collect();
        let help = match suggest::nearest(bullet, names.iter().copied()) {
            Some(nearest) => format!("did you mean `{nearest}`?"),
            None => "this document's `bullets:` map lists every legal name, sorted".to_string(),
        };
        out.push(
            Diagnostic::new(
                code::PATTERN_BULLET_UNKNOWN,
                format!(
                    "timeline entry {index} fires bullet kind `{bullet}`, which this document's \
                     `bullets:` map does not define"
                ),
            )
            .with_path(format!("{base}/bullet"))
            .with_help(help),
        );
    }

    if entry.count == 0 {
        out.push(entry_invalid(
            format!("{base}/count"),
            format!("timeline entry {index} fires 0 bullets"),
            "give `count` a value of 1 or more",
        ));
    }
    if entry.count > VOLLEY_MAX_BULLETS {
        out.push(entry_invalid(
            format!("{base}/count"),
            format!(
                "timeline entry {index} fires {} bullets, above the kill bitset's width of \
                 {VOLLEY_MAX_BULLETS}",
                entry.count
            ),
            "split the volley into several entries of 256 or fewer",
        ));
    }
    if entry.repeat == 0 {
        out.push(entry_invalid(
            format!("{base}/repeat"),
            format!("timeline entry {index} repeats 0 times, so it never fires"),
            "give `repeat` a value of 1 or more, or delete the entry",
        ));
    }
    if entry.every == 0 && entry.repeat > 1 {
        out.push(entry_invalid(
            format!("{base}/every"),
            format!(
                "timeline entry {index} repeats {} times with `every: 0`, so every repeat fires on \
                 one frame",
                entry.repeat
            ),
            "give `every` a positive frame gap, or set `repeat: 1` and raise `count`",
        ));
    }
    check_speed(&format!("{base}/speed"), entry.speed, out);
    if let Some(range) = entry.speed_range {
        for (slot, speed) in range.iter().enumerate() {
            check_speed(&format!("{base}/speed_range/{slot}"), *speed, out);
        }
    }
    let mut finite = true;
    for (field, value) in [
        ("angle_deg", entry.angle_deg),
        ("angle_step_deg", entry.angle_step_deg),
        ("spread_deg", entry.spread_deg),
        ("speed_step", entry.speed_step),
    ] {
        if !value.is_finite() {
            finite = false;
            out.push(entry_invalid(
                format!("{base}/{field}"),
                format!("timeline entry {index} `{field}` {value} is not a finite number"),
                "give the field a finite value",
            ));
        }
    }

    validate_against_loop(doc, entry, &base, index, out);

    let [low, high] = entry.speed_range.unwrap_or([entry.speed, entry.speed]);
    let valid_speeds = [low, high].iter().all(|v| v.is_finite() && *v >= 0.0);
    if !finite || !valid_speeds {
        return;
    }
    if entry.shape == VolleyShape::Arc && entry.count > 1 && entry.spread_deg.abs() >= 360.0 {
        out.push(authoring_lint(
            format!("{base}/spread_deg"),
            format!(
                "timeline entry {index} spreads an arc over {} degrees; the arc's ends are \
                 inclusive, so its first and last bullet share a heading",
                entry.spread_deg
            ),
            "use `shape: ring` for an even circle, or keep the spread below 360",
        ));
    }
    if entry.speed_step != 0.0 && entry.repeat > 1 {
        let last = entry.repeat - 1;
        let [low, high] = entry.speed_range_at(last);
        if !low.is_finite() || !high.is_finite() || low < 0.0 || high < 0.0 {
            out.push(entry_invalid(
                format!("{base}/speed_step"),
                format!(
                    "timeline entry {index} steps its speed by {} per repeat, so repeat {last} \
                     fires at {} units per second",
                    entry.speed_step,
                    if low < 0.0 || !low.is_finite() {
                        low
                    } else {
                        high
                    }
                ),
                "lower `speed_step`, raise the starting speed, or cut `repeat`",
            ));
        }
    }
    let last_angle = entry.angle_deg + entry.angle_step_deg * entry.repeat.saturating_sub(1) as f32;
    if !last_angle.is_finite() {
        out.push(entry_invalid(
            format!("{base}/angle_step_deg"),
            format!(
                "timeline entry {index} steps its angle by {} per repeat, which overflows to \
                 {last_angle} by repeat {}",
                entry.angle_step_deg,
                entry.repeat - 1
            ),
            "give `angle_step_deg` a smaller value or cut `repeat`",
        ));
    }
}

/// `L1149` for a volley entry that fires at or past its document's `loop`: the timeline wraps first,
/// so those repeats never fire.
fn validate_against_loop(
    doc: &PatternDoc,
    entry: &TimelineEntry,
    base: &str,
    index: usize,
    out: &mut Vec<Diagnostic>,
) {
    let loop_frames = u64::from(doc.loop_frames);
    if loop_frames == 0 {
        return;
    }
    if u64::from(entry.at) >= loop_frames {
        out.push(entry_invalid(
            format!("{base}/at"),
            format!(
                "timeline entry {index} fires at frame {}, at or past the pattern's `loop: {}`, so it \
                 never fires",
                entry.at, doc.loop_frames
            ),
            "give `at` a frame before `loop`, or raise `loop`",
        ));
    } else if last_fire(entry) >= loop_frames {
        let first_dropped = (loop_frames - u64::from(entry.at)).div_ceil(u64::from(entry.every));
        out.push(entry_invalid(
            format!("{base}/repeat"),
            format!(
                "timeline entry {index} repeats until frame {}, at or past the pattern's `loop: {}`; \
                 repeat {first_dropped} onward never fire",
                last_fire(entry),
                doc.loop_frames
            ),
            "cut `repeat`, shorten `every`, or raise `loop`",
        ));
    }
}

/// The `loop` rules of a resolved `play`: the leaf's own `loop` is ignored (a lint), and its last
/// fire shifted by the entry's `at` must land inside the playing pattern's `loop` (`L1149`).
fn validate_played_loop(
    doc: &PatternDoc,
    played: &PatternDoc,
    entry: &TimelineEntry,
    base: &str,
    index: usize,
    out: &mut Vec<Diagnostic>,
) {
    if played.loop_frames > 0 {
        out.push(authoring_lint(
            format!("{base}/play"),
            format!(
                "timeline entry {index} plays a pattern with `loop: {}`; a played pattern runs \
                 its timeline once and ignores its own `loop`",
                played.loop_frames
            ),
            "drop the played pattern's `loop`, or repeat the `play` entry in this timeline",
        ));
    }
    let loop_frames = u64::from(doc.loop_frames);
    let last = played
        .timeline
        .iter()
        .filter(|leaf| leaf.bullet.is_some())
        .map(last_fire)
        .max();
    if let Some(last) = last
        && loop_frames > 0
        && u64::from(entry.at) + last >= loop_frames
    {
        out.push(entry_invalid(
            format!("{base}/play"),
            format!(
                "timeline entry {index} plays a pattern whose last volley fires {last} frames in; \
                 started at frame {}, that is at or past this pattern's `loop: {}`, so the tail \
                 never fires",
                entry.at, doc.loop_frames
            ),
            "move the `play` earlier, shorten the played pattern, or raise `loop`",
        ));
    }
}

/// The three ways a `play` fails to resolve, all `L1150`; the resolved document otherwise.
fn validate_play<'a>(
    store: &'a DocStore<PatternDoc>,
    key: &str,
    play: &str,
    base: &str,
    index: usize,
    out: &mut Vec<Diagnostic>,
) -> Option<&'a PatternDoc> {
    let unresolved = |message: String, help: &str| {
        Diagnostic::new(code::PATTERN_PLAY_UNRESOLVED, message)
            .with_path(format!("{base}/play"))
            .with_help(help.to_string())
    };
    if play == key {
        out.push(unresolved(
            format!("timeline entry {index} plays its own document `{play}`"),
            "inline the volleys instead, or play a different pattern",
        ));
        return None;
    }
    let Some(played) = store.get(play) else {
        let keys: Vec<&str> = store.iter().map(|(key, _)| key.as_str()).collect();
        let help = match suggest::nearest(play, keys.iter().copied()) {
            Some(nearest) => format!("did you mean `{nearest}`?"),
            None => "name a loaded `kind: pattern` document by its content-root-relative path"
                .to_string(),
        };
        out.push(unresolved(
            format!(
                "timeline entry {index} plays `{play}`, which is not a loaded `kind: pattern` \
                 document"
            ),
            &help,
        ));
        return None;
    };
    if played.timeline.iter().any(|entry| entry.play.is_some()) {
        out.push(unresolved(
            format!(
                "timeline entry {index} plays `{play}`, whose own timeline holds a `play`; nesting \
                 reaches one level"
            ),
            "play a leaf pattern — one whose timeline is volleys only",
        ));
    }
    Some(played)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;
    use ludion_core::time::SimTime;
    use ludion_ecs::SemanticValidate;
    use std::path::{Path, PathBuf};

    const TICK: TickRate = TickRate::DEFAULT;

    fn at_frame(frame: u64) -> SimTime {
        SimTime::from_ticks(frame, TICK)
    }

    fn ctx_at(frame: u64) -> SystemCtx {
        SystemCtx::new(FrameIndex::new(frame), TICK)
    }

    fn ring(count: u32, angle_deg: f32, speed: f32) -> Volley {
        Volley {
            shape: VolleyShape::Ring,
            count,
            angle_deg,
            speed: [speed, speed],
            ..Volley::default()
        }
    }

    #[test]
    fn a_negative_bullet_colour_is_l1263() {
        let mut out = Vec::new();
        let kind = BulletKind {
            color: [1.0, -0.5, 1.0, 2.0],
            ..BulletKind::default()
        };
        validate_bullet_kind("boss", "orb", &kind, &mut out);
        let at: Vec<(&str, &str)> = out
            .iter()
            .map(|d| (d.code.as_str(), d.path.as_deref().unwrap_or("")))
            .collect();
        assert!(
            at.contains(&("L1263", "pattern/boss/bullets/orb/color/1")),
            "{at:?}"
        );
        assert!(
            at.contains(&("L1263", "pattern/boss/bullets/orb/color/3")),
            "{at:?}"
        );
        let mut clean = Vec::new();
        validate_bullet_kind("boss", "orb", &BulletKind::default(), &mut clean);
        assert!(clean.is_empty(), "{clean:?}");
    }

    // --- the shapes ---

    #[test]
    fn a_ring_spreads_its_bullets_evenly_over_the_full_turn() {
        let volley = ring(8, 0.0, 1.0);
        let step = std::f32::consts::TAU / 8.0;
        for index in 0..8 {
            let (angle, speed) = bullet_launch(&volley, index);
            assert!(
                (angle - step * index as f32).abs() < 1e-5,
                "bullet {index} sits at {angle}"
            );
            assert_eq!(speed, 1.0);
        }
    }

    #[test]
    fn an_arc_spans_its_spread_centred_on_the_volley_angle() {
        let volley = Volley {
            shape: VolleyShape::Arc,
            count: 5,
            angle_deg: 10.0,
            spread_deg: 40.0,
            speed: [3.0, 3.0],
            ..Volley::default()
        };
        let degrees: Vec<f32> = (0..5)
            .map(|index| bullet_launch(&volley, index).0.to_degrees())
            .collect();
        for (index, expected) in [-10.0_f32, 0.0, 10.0, 20.0, 30.0].iter().enumerate() {
            assert!(
                (degrees[index] - expected).abs() < 1e-3,
                "bullet {index} at {} deg, expected {expected}",
                degrees[index]
            );
        }
        // A single bullet sits at the centre rather than at the cone's low edge.
        let single = Volley { count: 1, ..volley };
        assert!((bullet_launch(&single, 0).0.to_degrees() - 10.0).abs() < 1e-3);
    }

    #[test]
    fn a_random_bullet_is_a_pure_function_of_seed_and_index() {
        let volley = Volley {
            shape: VolleyShape::Random,
            count: 16,
            angle_deg: 0.0,
            spread_deg: 60.0,
            speed: [2.0, 6.0],
            seed: 99,
            ..Volley::default()
        };
        let mut launches = Vec::new();
        for index in 0..16 {
            let first = bullet_launch(&volley, index);
            let second = bullet_launch(&volley, index);
            assert_eq!(first, second, "bullet {index} re-draws differently");
            launches.push(first);
            let degrees = first.0.to_degrees();
            assert!(
                (-30.0..=30.0).contains(&degrees),
                "bullet {index} left the cone at {degrees} deg"
            );
            assert!((2.0..=6.0).contains(&first.1));
        }
        for (i, a) in launches.iter().enumerate() {
            for (j, b) in launches.iter().enumerate().skip(i + 1) {
                assert_ne!(a, b, "bullets {i} and {j} drew the same launch");
            }
        }
        // A different seed is a different field, and index 0 of one is not index 0 of the other.
        let other = Volley {
            seed: 100,
            ..volley.clone()
        };
        assert_ne!(bullet_launch(&volley, 0), bullet_launch(&other, 0));
    }

    // --- the trajectory ---

    #[test]
    fn a_ramp_then_turn_bullet_matches_the_hand_built_motion() {
        let kind = BulletKind {
            motion: vec![
                MotionSegment {
                    ramp: Some(RampSpec {
                        frames: 30,
                        from: Some(6.0),
                        to: 2.0,
                    }),
                    turn: None,
                },
                MotionSegment {
                    ramp: None,
                    turn: Some(TurnSpec {
                        frames: 90,
                        speed: None,
                        rate_deg: 45.0,
                    }),
                },
            ],
            ..BulletKind::default()
        };
        let volley = ring(1, 0.0, 6.0);
        let built = bullet_motion(&volley, Vec3::ZERO, &kind, 0, Plane::Xz, TICK);

        let mut expected = Motion::new(Vec3::ZERO);
        let heading = Vec3::X;
        let ramp = Segment::Ramp {
            heading,
            from: 6.0,
            to: 2.0,
            duration: 0.5,
        };
        expected.push(ramp);
        let turn = Segment::Turn {
            heading,
            normal: Vec3::X.cross(Vec3::Z),
            speed: 2.0,
            rate: 45.0_f32.to_radians(),
            duration: 1.5,
        };
        expected.push(turn);
        expected.push(Segment::straight(turn.end_heading(), 2.0));

        for age in [0.0_f32, 0.25, 0.5, 1.0, 2.0, 3.5] {
            assert_eq!(
                built.position_at(age),
                expected.position_at(age),
                "the two motions disagree at {age} s"
            );
        }
    }

    #[test]
    fn four_authored_segments_still_leave_the_tail() {
        let ramp = || MotionSegment {
            ramp: Some(RampSpec {
                frames: 6,
                from: None,
                to: 2.0,
            }),
            turn: None,
        };
        let kind = BulletKind {
            motion: vec![ramp(), ramp(), ramp(), ramp()],
            ..BulletKind::default()
        };
        let volley = ring(1, 0.0, 6.0);
        let motion = bullet_motion(&volley, Vec3::ZERO, &kind, 0, Plane::Xz, TICK);
        assert_eq!(motion.len(), BULLET_MAX_SEGMENTS + 1);
        // The four ramps end at 0.4 s; past that the bullet still travels at speed 2.
        let (a, b) = (motion.position_at(1.0), motion.position_at(2.0));
        assert!(
            (b.x - a.x - 2.0).abs() < 1e-4,
            "the bullet stopped after its fourth segment: {a} then {b}"
        );
    }

    #[test]
    fn an_omitted_speed_carries_the_speed_in_force() {
        let kind = BulletKind {
            motion: vec![MotionSegment {
                turn: Some(TurnSpec {
                    frames: 60,
                    speed: None,
                    rate_deg: 0.0,
                }),
                ramp: None,
            }],
            ..BulletKind::default()
        };
        // A rate of 0 is a straight line, so one second of it is exactly the fire speed travelled.
        let volley = ring(1, 0.0, 5.0);
        let motion = bullet_motion(&volley, Vec3::ZERO, &kind, 0, Plane::Xz, TICK);
        assert!((motion.position_at(1.0).x - 5.0).abs() < 1e-4);
        // And the tail keeps that speed rather than stopping.
        assert!((motion.position_at(2.0).x - 10.0).abs() < 1e-4);
    }

    #[test]
    fn the_plane_decides_where_ninety_degrees_points_and_which_way_a_turn_goes() {
        let volley = ring(1, 90.0, 4.0);
        let kind = BulletKind::default();
        let xz = bullet_motion(&volley, Vec3::ZERO, &kind, 0, Plane::Xz, TICK).position_at(1.0);
        assert!(xz.z > 3.9 && xz.x.abs() < 1e-4, "Xz theta=90 went to {xz}");
        let xy = bullet_motion(&volley, Vec3::ZERO, &kind, 0, Plane::Xy, TICK).position_at(1.0);
        assert!(xy.y > 3.9 && xy.x.abs() < 1e-4, "Xy theta=90 went to {xy}");

        // A positive rate turns toward increasing theta on both planes: launched along the first
        // axis, a quarter turn arrives on the second.
        let turning = BulletKind {
            motion: vec![MotionSegment {
                ramp: None,
                turn: Some(TurnSpec {
                    frames: 60,
                    speed: Some(1.0),
                    rate_deg: 90.0,
                }),
            }],
            ..BulletKind::default()
        };
        let along_axis = ring(1, 0.0, 1.0);
        for (plane, second) in [(Plane::Xz, 2_usize), (Plane::Xy, 1_usize)] {
            let end = bullet_motion(&along_axis, Vec3::ZERO, &turning, 0, plane, TICK)
                .position_at(1.0)
                .to_array();
            assert!(
                end[second] > 0.0 && end[0] > 0.0,
                "{plane:?} turned the wrong way: {end:?}"
            );
        }
    }

    // --- the query ---

    #[test]
    fn volley_hits_finds_the_bullet_on_the_circle_it_passes_through() {
        let kind = BulletKind {
            hit_radius: 0.1,
            ..BulletKind::default()
        };
        let mut volley = ring(1, 0.0, 4.0);
        volley.started = FrameIndex::new(0);
        let origin = Transform::default();
        // One second in, the bullet is 4 units along +x.
        let now = at_frame(60);
        let hits: Vec<u32> =
            volley_hits(&volley, &origin, &kind, now, Plane::Xz, [4.0, 0.0], 0.2).collect();
        assert_eq!(hits, vec![0]);

        let missed: Vec<u32> =
            volley_hits(&volley, &origin, &kind, now, Plane::Xz, [1.0, 0.0], 0.2).collect();
        assert!(
            missed.is_empty(),
            "the circle it is nowhere near: {missed:?}"
        );

        volley.kill(0);
        assert!(volley.is_killed(0));
        let after_kill: Vec<u32> =
            volley_hits(&volley, &origin, &kind, now, Plane::Xz, [4.0, 0.0], 0.2).collect();
        assert!(after_kill.is_empty(), "a killed bullet still hits");
    }

    #[test]
    fn a_bullet_is_alive_only_inside_its_life() {
        let kind = BulletKind {
            life: 120,
            ..BulletKind::default()
        };
        let volley = ring(4, 0.0, 1.0);
        assert!(bullet_is_alive(&volley, &kind, 0, at_frame(0)));
        assert!(bullet_is_alive(&volley, &kind, 3, at_frame(119)));
        assert!(!bullet_is_alive(&volley, &kind, 3, at_frame(120)));
        assert!(
            !bullet_is_alive(&volley, &kind, 4, at_frame(0)),
            "past `count`"
        );
    }

    #[test]
    fn a_non_default_tick_rate_measures_life_and_segments_in_its_own_frames() {
        let rate = TickRate::hz_of(30);
        let now = |ticks: u64| SimTime::from_ticks(ticks, rate);
        let kind = BulletKind {
            life: 120,
            motion: vec![MotionSegment {
                ramp: Some(RampSpec {
                    frames: 30,
                    from: Some(6.0),
                    to: 2.0,
                }),
                turn: None,
            }],
            ..BulletKind::default()
        };
        let volley = ring(1, 0.0, 6.0);
        assert!(bullet_is_alive(&volley, &kind, 0, now(119)));
        assert!(!bullet_is_alive(&volley, &kind, 0, now(120)));

        // The ramp lasts one second at 30 Hz: 4 units, then 2 u/s for the second second.
        let origin = Transform::default();
        let at = bullet_position(&volley, &origin, &kind, 0, now(60), Plane::Xz);
        assert!((at[0] - 6.0).abs() < 1e-4, "{at:?}");
    }

    #[test]
    fn a_fractional_instant_lies_between_its_whole_tick_neighbours() {
        const Q: u64 = ludion_core::time::Q16_PER_TICK;
        let kind = BulletKind {
            life: 100,
            ..BulletKind::default()
        };
        let volley = ring(1, 0.0, 6.0);
        let origin = Transform::default();
        let pos = |g: GameTime| bullet_position(&volley, &origin, &kind, 0, g, Plane::Xz)[0];
        let whole = |t: u64| GameTime::from_ticks(t, TICK);
        let mid = GameTime::from_q16(10 * Q + Q / 2, TICK);
        assert!(pos(whole(10)) < pos(mid) && pos(mid) < pos(whole(11)));
        assert_eq!(
            bullet_age(&volley, mid),
            (10.5 * TICK.seconds_per_tick()) as f32
        );
    }

    #[test]
    fn integer_instants_match_the_whole_tick_calls_and_the_life_boundary() {
        let kind = BulletKind {
            life: 20,
            ..BulletKind::default()
        };
        let volley = ring(3, 0.0, 2.0);
        let origin = Transform::default();
        for t in 0..40u64 {
            let g = GameTime::from_ticks(t, TICK);
            assert_eq!(
                bullet_age(&volley, g).to_bits(),
                bullet_age(&volley, at_frame(t)).to_bits()
            );
            assert_eq!(
                bullet_position(&volley, &origin, &kind, 1, g, Plane::Xz),
                bullet_position(&volley, &origin, &kind, 1, at_frame(t), Plane::Xz)
            );
            assert_eq!(bullet_is_alive(&volley, &kind, 0, g), t < 20, "tick {t}");
        }
        // The last Q16 before the boundary is alive, the boundary itself is not.
        let q = ludion_core::time::Q16_PER_TICK;
        assert!(bullet_is_alive(
            &volley,
            &kind,
            0,
            GameTime::from_q16(20 * q - 1, TICK)
        ));
        assert!(!bullet_is_alive(
            &volley,
            &kind,
            0,
            GameTime::from_q16(20 * q, TICK)
        ));
    }

    // --- the components ---

    #[test]
    fn a_volley_over_the_bitset_width_fails_validation() {
        let out = Volley::validate_semantics(&serde_json::json!({ "count": 300 }));
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].code, code::VOLLEY_INVALID);
        assert_eq!(out[0].path.as_deref(), Some("/count"));
        assert!(Volley::validate_semantics(&serde_json::json!({ "count": 256 })).is_empty());
    }

    #[test]
    fn a_volley_number_past_f32_range_fails_validation() {
        let out = Volley::validate_semantics(&serde_json::json!({ "angle_deg": 1e39 }));
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].code, code::VOLLEY_INVALID);
        assert_eq!(out[0].path.as_deref(), Some("/angle_deg"));
    }

    #[test]
    fn an_empty_volley_deserializes_to_the_default() {
        let volley: Volley = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(volley, Volley::default());
        let player: PatternPlayer = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(player, PatternPlayer::default());
    }

    // --- the validator ---

    /// A content root holding `patterns/<name>.yaml` for each `(name, body)`.
    fn root(test: &str, docs: &[(&str, &str)]) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ludion-patterns-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("patterns")).unwrap();
        for (name, body) in docs {
            std::fs::write(dir.join("patterns").join(format!("{name}.yaml")), body).unwrap();
        }
        dir
    }

    fn engine_at(dir: &Path) -> Result<Engine, Diagnostic> {
        Engine::builder().patterns().project_root(dir).build()
    }

    /// The diagnostics a content root's pattern documents produce at build — a failed build
    /// collapses them into one diagnostic carrying the rest as `related`, so both halves count.
    fn diagnostics_for(test: &str, docs: &[(&str, &str)]) -> Vec<Diagnostic> {
        let dir = root(test, docs);
        let out = match engine_at(&dir) {
            Ok(engine) => engine.doc_validation_warnings().to_vec(),
            Err(mut collapsed) => {
                let related = std::mem::take(&mut collapsed.related);
                std::iter::once(collapsed).chain(related).collect()
            }
        };
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    fn has(diagnostics: &[Diagnostic], expected: &str, path: &str) -> bool {
        diagnostics
            .iter()
            .any(|d| d.code.as_str() == expected && d.path.as_deref() == Some(path))
    }

    const ONE_BULLET: &str = "\
format: 1
kind: pattern
name: probe
bullets:
  needle:
    life: 60
timeline:
  - at: 0
    bullet: needle
    speed: 4.0
";

    #[test]
    fn a_clean_document_raises_nothing() {
        assert!(diagnostics_for("clean", &[("probe", ONE_BULLET)]).is_empty());
    }

    #[test]
    fn an_unknown_bullet_kind_is_l1148_with_a_did_you_mean() {
        let doc = "\
format: 1
kind: pattern
bullets:
  needle:
    life: 60
timeline:
  - at: 0
    bullet: neelde
    speed: 1.0
";
        let out = diagnostics_for("l1148", &[("probe", doc)]);
        assert!(
            has(
                &out,
                "L1148",
                "pattern/patterns/probe.yaml/timeline/0/bullet"
            ),
            "{out:?}"
        );
        assert!(
            out.iter()
                .any(|d| d.help.as_deref().is_some_and(|h| h.contains("needle"))),
            "no did-you-mean: {out:?}"
        );
    }

    #[test]
    fn the_l1149_set_is_raised_at_the_offending_field() {
        let doc = "\
format: 1
kind: pattern
bullets:
  wide:
    life: 0
    hit_radius: 0.0
    motion:
      - ramp:
          frames: 1
          to: 1.0
        turn:
          frames: 1
          rate_deg: 1.0
      - {}
      - ramp:
          frames: 1
          to: 1.0
      - ramp:
          frames: 1
          to: 1.0
      - ramp:
          frames: 1
          to: 1.0
timeline:
  - at: 0
    bullet: wide
    count: 0
    speed: -1.0
  - at: 1
    bullet: wide
    count: 300
    repeat: 0
    speed: 1.0
  - at: 2
    bullet: wide
    repeat: 4
    every: 0
    speed: 1.0
  - at: 3
    play: patterns/other.yaml
    count: 9
";
        let other = "format: 1\nkind: pattern\ntimeline: []\n";
        let out = diagnostics_for("l1149", &[("probe", doc), ("other", other)]);
        let base = "pattern/patterns/probe.yaml";
        for path in [
            format!("{base}/bullets/wide/motion"),
            format!("{base}/bullets/wide/motion/0"),
            format!("{base}/bullets/wide/motion/1"),
            format!("{base}/bullets/wide/life"),
            format!("{base}/bullets/wide/hit_radius"),
            format!("{base}/timeline/0/count"),
            format!("{base}/timeline/0/speed"),
            format!("{base}/timeline/1/count"),
            format!("{base}/timeline/1/repeat"),
            format!("{base}/timeline/2/every"),
            format!("{base}/timeline/3/play"),
        ] {
            assert!(has(&out, "L1149", &path), "no L1149 at {path}: {out:?}");
        }
    }

    #[test]
    fn an_entry_that_is_neither_a_volley_nor_a_play_is_l1149() {
        let doc = "format: 1\nkind: pattern\nbullets: {}\ntimeline:\n  - at: 0\n";
        let out = diagnostics_for("l1149-empty", &[("probe", doc)]);
        assert!(
            has(
                &out,
                "L1149",
                "pattern/patterns/probe.yaml/timeline/0/bullet"
            ),
            "{out:?}"
        );
    }

    #[test]
    fn a_play_that_does_not_resolve_is_l1150() {
        let missing =
            "format: 1\nkind: pattern\ntimeline:\n  - at: 0\n    play: patterns/gone.yaml\n";
        let out = diagnostics_for("l1150-missing", &[("probe", missing)]);
        assert!(
            has(&out, "L1150", "pattern/patterns/probe.yaml/timeline/0/play"),
            "{out:?}"
        );

        let itself =
            "format: 1\nkind: pattern\ntimeline:\n  - at: 0\n    play: patterns/probe.yaml\n";
        let out = diagnostics_for("l1150-self", &[("probe", itself)]);
        assert!(
            has(&out, "L1150", "pattern/patterns/probe.yaml/timeline/0/play"),
            "{out:?}"
        );

        // Two levels of nesting: `probe` plays `mid`, which itself plays `leaf`.
        let probe = "format: 1\nkind: pattern\ntimeline:\n  - at: 0\n    play: patterns/mid.yaml\n";
        let mid = "format: 1\nkind: pattern\ntimeline:\n  - at: 0\n    play: patterns/leaf.yaml\n";
        let leaf = ONE_BULLET;
        let out = diagnostics_for(
            "l1150-nested",
            &[("probe", probe), ("mid", mid), ("leaf", leaf)],
        );
        assert!(
            has(&out, "L1150", "pattern/patterns/probe.yaml/timeline/0/play"),
            "{out:?}"
        );
    }

    /// A `loop:` document of one `needle` kind and the given timeline entries, each written
    /// `field: value, field: value` and expanded to block style (flow mappings are refused).
    fn looped(loop_frames: u32, entries: &[&str]) -> String {
        let mut doc = String::from(
            "format: 1\nkind: pattern\nbullets:\n  needle:\n    life: 60\ntimeline:\n",
        );
        for entry in entries {
            for (line, field) in entry.split(", ").enumerate() {
                doc.push_str(if line == 0 { "  - " } else { "    " });
                doc.push_str(field);
                doc.push('\n');
            }
        }
        doc.push_str(&format!("loop: {loop_frames}\n"));
        doc
    }

    #[test]
    fn an_entry_at_or_past_loop_is_l1149() {
        let doc = looped(
            240,
            &[
                "at: 240, bullet: needle, speed: 1.0",
                "at: 200, repeat: 8, every: 8, bullet: needle, speed: 1.0",
                "at: 0, repeat: 30, every: 8, bullet: needle, speed: 1.0",
            ],
        );
        let out = diagnostics_for("loop-entries", &[("probe", &doc)]);
        let base = "pattern/patterns/probe.yaml/timeline";
        assert!(has(&out, "L1149", &format!("{base}/0/at")), "{out:?}");
        assert!(has(&out, "L1149", &format!("{base}/1/repeat")), "{out:?}");
        assert!(
            !out.iter()
                .any(|d| d.path.as_deref().is_some_and(|p| p.contains("/timeline/2"))),
            "last fire 232 is inside the loop: {out:?}"
        );
    }

    #[test]
    fn a_play_past_loop_is_l1149_and_its_own_loop_is_l1265() {
        let leaf = looped(
            200,
            &[
                "at: 0, bullet: needle, speed: 1.0",
                "at: 150, bullet: needle, speed: 1.0",
            ],
        );
        let playing_at = |at: u32| looped(240, &[&format!("at: {at}, play: patterns/leaf.yaml")]);
        let late = diagnostics_for(
            "loop-play-late",
            &[("probe", &playing_at(120)), ("leaf", &leaf)],
        );
        assert!(
            has(
                &late,
                "L1149",
                "pattern/patterns/probe.yaml/timeline/0/play"
            ),
            "{late:?}"
        );
        let early = diagnostics_for(
            "loop-play-early",
            &[("probe", &playing_at(80)), ("leaf", &leaf)],
        );
        let lint = early
            .iter()
            .find(|d| d.code.as_str() == "L1265")
            .unwrap_or_else(|| panic!("{early:?}"));
        assert_eq!(lint.severity, ludion_core::diagnostics::Severity::Warning);
        assert_eq!(
            lint.path.as_deref(),
            Some("pattern/patterns/probe.yaml/timeline/0/play")
        );
        assert!(
            !early.iter().any(|d| d.code.as_str() == "L1149"),
            "{early:?}"
        );
    }

    #[test]
    fn an_entry_whose_last_repeat_is_loop_minus_one_fires_every_repeat() {
        let doc = looped(
            240,
            &["at: 14, repeat: 4, every: 75, bullet: needle, speed: 1.0"],
        );
        let clean = diagnostics_for("loop-edge", &[("probe", &doc)]);
        assert!(clean.is_empty(), "{clean:?}");
        let (dir, mut engine) = playing("loop-edge-run", &[("probe", &doc)], SCENE);
        let mut fired: Vec<u64> = Vec::new();
        for frame in 0..240u64 {
            let before = volleys(&engine).len();
            play_patterns(engine.world_mut(), &ctx_at(frame));
            if volleys(&engine).len() > before {
                fired.push(frame);
            }
        }
        assert_eq!(fired, vec![14, 89, 164, 239]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A one-entry document with the given volley fields (comma-separated, block-expanded).
    fn one_entry(fields: &str) -> String {
        looped(0, &[&format!("at: 0, bullet: needle, {fields}")])
    }

    #[test]
    fn a_full_turn_arc_is_l1265() {
        let flagged = |label: &str, fields: &str| {
            let out = diagnostics_for(label, &[("probe", &one_entry(fields))]);
            let lint = out.iter().find(|d| d.code.as_str() == "L1265");
            let lint = lint.unwrap_or_else(|| panic!("{label}: {out:?}"));
            assert_eq!(lint.severity, ludion_core::diagnostics::Severity::Warning);
            assert_eq!(
                lint.path.as_deref(),
                Some("pattern/patterns/probe.yaml/timeline/0/spread_deg")
            );
        };
        flagged(
            "arc-360",
            "shape: arc, count: 5, spread_deg: 360.0, speed: 1.0",
        );
        flagged(
            "arc-neg",
            "shape: arc, count: 5, spread_deg: -360.0, speed: 1.0",
        );
        for (label, fields) in [
            (
                "arc-359",
                "shape: arc, count: 5, spread_deg: 359.0, speed: 1.0",
            ),
            (
                "arc-one",
                "shape: arc, count: 1, spread_deg: 360.0, speed: 1.0",
            ),
            (
                "random-360",
                "shape: random, count: 5, spread_deg: 360.0, speed: 1.0",
            ),
        ] {
            let out = diagnostics_for(label, &[("probe", &one_entry(fields))]);
            assert!(out.is_empty(), "{label}: {out:?}");
        }
    }

    #[test]
    fn a_speed_step_that_drives_a_repeat_negative_is_l1149() {
        let path = "pattern/patterns/probe.yaml/timeline/0/speed_step";
        let out = diagnostics_for(
            "step-flat",
            &[(
                "probe",
                &one_entry("repeat: 8, every: 2, speed: 4.0, speed_step: -1.0"),
            )],
        );
        assert!(has(&out, "L1149", path), "{out:?}");
        let out = diagnostics_for(
            "step-zero",
            &[(
                "probe",
                &one_entry("repeat: 8, every: 2, speed: 7.0, speed_step: -1.0"),
            )],
        );
        assert!(out.is_empty(), "speed 7 reaches 0: {out:?}");
        let out = diagnostics_for(
            "step-range",
            &[(
                "probe",
                &one_entry(
                    "repeat: 6, every: 2, count: 3, speed_step: -0.5, speed_range:\n      - 2.0\n      - 6.0",
                ),
            )],
        );
        assert!(has(&out, "L1149", path), "{out:?}");
    }

    // --- the systems ---

    const SCENE: &str = "\
format: 1
seed: 7
entities:
  - name: turret
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      PatternPlayer:
        pattern: patterns/probe.yaml
        started: 0
";

    /// An engine rooted at a content root holding `docs`, with the scene loaded.
    fn playing(test: &str, docs: &[(&str, &str)], scene: &str) -> (PathBuf, Engine) {
        let dir = root(test, docs);
        let mut engine = engine_at(&dir).expect("the pattern documents validate");
        engine.load_inline_scene(scene).expect("the scene loads");
        (dir, engine)
    }

    /// Every live volley, in ascending entity order.
    fn volleys(engine: &Engine) -> Vec<Volley> {
        let world = engine.world();
        world
            .entities()
            .filter_map(|id| world.get::<Volley>(id).ok().flatten().cloned())
            .collect()
    }

    #[test]
    fn a_timeline_stands_volleys_up_on_the_exact_frames() {
        let doc = "\
format: 1
kind: pattern
bullets:
  needle:
    life: 600
timeline:
  - at: 2
    repeat: 3
    every: 5
    shape: ring
    count: 4
    bullet: needle
    speed: 3.0
    angle_deg: 10.0
    angle_step_deg: 7.5
    speed_step: 1.0
";
        let (dir, mut engine) = playing("timeline", &[("probe", doc)], SCENE);
        let mut fired: Vec<u64> = Vec::new();
        for frame in 0..16u64 {
            let before = volleys(&engine).len();
            play_patterns(engine.world_mut(), &ctx_at(frame));
            if volleys(&engine).len() > before {
                fired.push(frame);
            }
        }
        assert_eq!(fired, vec![2, 7, 12], "fired on {fired:?}");

        let out = volleys(&engine);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].count, 4);
        assert_eq!(out[0].angle_deg, 10.0);
        assert_eq!(out[1].angle_deg, 17.5);
        assert_eq!(out[2].angle_deg, 25.0);
        assert_eq!(out[0].speed, [3.0, 3.0]);
        assert_eq!(out[2].speed, [5.0, 5.0]);
        assert_eq!(out[0].pattern, "patterns/probe.yaml");
        assert_eq!(out[0].bullet, "needle");
        assert_eq!(out[0].started, FrameIndex::new(2));
        assert_ne!(out[0].seed, out[1].seed, "two volleys share a stream");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_loop_replays_the_timeline() {
        let doc = "\
format: 1
kind: pattern
bullets:
  needle:
    life: 600
timeline:
  - at: 1
    bullet: needle
    speed: 1.0
loop: 4
";
        let (dir, mut engine) = playing("loop", &[("probe", doc)], SCENE);
        let mut fired: Vec<u64> = Vec::new();
        for frame in 0..10u64 {
            let before = volleys(&engine).len();
            play_patterns(engine.world_mut(), &ctx_at(frame));
            if volleys(&engine).len() > before {
                fired.push(frame);
            }
        }
        assert_eq!(fired, vec![1, 5, 9]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_play_entry_fires_the_played_patterns_volleys_at_its_own_offset() {
        let probe = "\
format: 1
kind: pattern
timeline:
  - at: 10
    play: patterns/leaf.yaml
";
        let leaf = "\
format: 1
kind: pattern
bullets:
  spark:
    life: 600
timeline:
  - at: 3
    bullet: spark
    speed: 2.0
    count: 2
";
        let (dir, mut engine) = playing("play", &[("probe", probe), ("leaf", leaf)], SCENE);
        for frame in 0..16u64 {
            play_patterns(engine.world_mut(), &ctx_at(frame));
        }
        let out = volleys(&engine);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].started, FrameIndex::new(13), "10 + 3");
        assert_eq!(
            out[0].pattern, "patterns/leaf.yaml",
            "the played document owns the kind"
        );
        assert_eq!(out[0].bullet, "spark");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_aimed_arc_folds_the_direction_to_the_target_in() {
        let doc = "\
format: 1
kind: pattern
bullets:
  spark:
    life: 600
timeline:
  - at: 0
    bullet: spark
    shape: arc
    center: aim
    spread_deg: 40.0
    count: 5
    speed: 2.0
";
        let scene = "\
format: 1
seed: 7
entities:
  - name: turret
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      PatternPlayer:
        pattern: patterns/probe.yaml
        started: 0
        target:
          index: 1
          generation: 0
  - name: prey
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 5.0
";
        let (dir, mut engine) = playing("aim", &[("probe", doc)], scene);
        play_patterns(engine.world_mut(), &ctx_at(0));
        let out = volleys(&engine);
        assert_eq!(out.len(), 1, "{out:?}");
        // +z on the ground plane is 90 degrees.
        assert!(
            (out[0].angle_deg - 90.0).abs() < 1e-3,
            "{}",
            out[0].angle_deg
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn advance_volleys_reaps_at_life_and_when_every_bullet_is_killed() {
        let doc = "\
format: 1
kind: pattern
bullets:
  needle:
    life: 6
timeline:
  - at: 0
    bullet: needle
    count: 2
    speed: 1.0
";
        let (dir, mut engine) = playing("reap", &[("probe", doc)], SCENE);
        play_patterns(engine.world_mut(), &ctx_at(0));
        assert_eq!(volleys(&engine).len(), 1);
        advance_volleys(engine.world_mut(), &ctx_at(5));
        assert_eq!(volleys(&engine).len(), 1, "still inside `life`");
        advance_volleys(engine.world_mut(), &ctx_at(6));
        assert!(volleys(&engine).is_empty(), "reaped at `life`");

        // A fully-killed volley goes before its life runs out.
        play_patterns(engine.world_mut(), &ctx_at(0));
        let world = engine.world_mut();
        let ids: Vec<EntityId> = world.entities().collect();
        for id in ids {
            if let Ok(Some(volley)) = world.get_mut::<Volley>(id) {
                volley.kill(0);
                volley.kill(1);
            }
        }
        advance_volleys(engine.world_mut(), &ctx_at(1));
        assert!(volleys(&engine).is_empty(), "an emptied volley is reaped");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A volley built in Rust with a NaN angle never saw a validator: it is reaped with `L2108`.
    #[test]
    fn advance_volleys_reaps_a_non_finite_volley_with_one_warning() {
        let doc = "\
format: 1
kind: pattern
bullets:
  needle:
    life: 600
timeline:
  - at: 0
    bullet: needle
    count: 2
    speed: 1.0
";
        let (dir, mut engine) = playing("nan_volley", &[("probe", doc)], SCENE);
        play_patterns(engine.world_mut(), &ctx_at(0));
        let world = engine.world_mut();
        let ids: Vec<EntityId> = world.query_ids::<(&Volley,)>().map(|(id, _)| id).collect();
        assert_eq!(ids.len(), 1);
        let id = ids[0];
        world.get_mut::<Volley>(id).unwrap().unwrap().angle_deg = f32::NAN;
        advance_volleys(engine.world_mut(), &ctx_at(1));
        assert!(volleys(&engine).is_empty(), "a non-finite volley is reaped");
        assert!(crate::diagnostics::is_latched(
            engine.world(),
            code::VOLLEY_INVALID,
            "volley:patterns/probe.yaml/needle/angle_deg"
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A pattern reload whose bullet `atlas`/`frame` fails is refused (`L1151`) with the old
    /// document still live; the generation, the recording and the store do not move.
    #[test]
    fn a_bad_bullet_atlas_on_reload_is_refused_and_the_old_doc_stays_live() {
        use ludion_protocol::Command;
        use ludion_protocol::command::AssetReloadParams;

        let key = "patterns/probe.yaml";
        for (label, edit) in [
            (
                "absent",
                "    atlas: art/absent.atlas.yaml\n    frame: ship/idle\n",
            ),
            ("half", "    atlas: art/absent.atlas.yaml\n"),
        ] {
            let dir = root(&format!("reload-atlas-{label}"), &[("probe", ONE_BULLET)]);
            let mut engine = engine_at(&dir).expect("an untextured kind builds");
            let generation = |engine: &Engine| {
                engine
                    .world()
                    .resource::<DocStore<PatternDoc>>()
                    .unwrap()
                    .generation()
            };
            let before = generation(&engine);

            let textured = ONE_BULLET.replace("    life: 60\n", &format!("    life: 60\n{edit}"));
            assert_ne!(textured, ONE_BULLET, "the edit must change the document");
            std::fs::write(dir.join(key), textured).unwrap();
            let refused = engine
                .execute(Command::AssetReload(AssetReloadParams {
                    path: Some(key.to_string()),
                }))
                .expect_err("a reload that cannot resolve its atlas is refused");
            let path = "pattern/patterns/probe.yaml/bullets/needle/atlas";
            let named: Vec<&Diagnostic> =
                std::iter::once(&refused).chain(&refused.related).collect();
            assert!(
                named
                    .iter()
                    .any(|d| d.code.as_str() == "L1151" && d.path.as_deref() == Some(path)),
                "{label}: {refused:?}"
            );
            let store = engine.world().resource::<DocStore<PatternDoc>>().unwrap();
            assert_eq!(store.get(key).unwrap().bullets["needle"].atlas, None);
            assert_eq!(generation(&engine), before, "{label}: the store moved");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// A `ship` atlas document whose regions are `frames`.
    fn ship_atlas(frames: &[&str]) -> ludion_scene::AtlasDocument {
        use ludion_scene::{AtlasDocument, AtlasFilter, AtlasRegion};
        AtlasDocument {
            generated_from: "ship.atlas-source.yaml".to_string(),
            pixels_per_unit: 4.0,
            color_map: "ship.png".to_string(),
            filter: AtlasFilter::Nearest,
            regions: frames
                .iter()
                .map(|name| {
                    (
                        name.to_string(),
                        AtlasRegion {
                            rect: [0, 0, 4, 2],
                            pivot: [2.0, 1.0],
                        },
                    )
                })
                .collect(),
            flipbooks: Vec::new(),
        }
    }

    /// An atlas that breaks under a live volley draws the kind untextured, every frame, and is
    /// reported once — the frame is never failed.
    #[test]
    fn an_atlas_that_breaks_under_a_live_volley_draws_untextured_and_reports_once() {
        use ludion_protocol::Command;
        use ludion_protocol::command::{AssetReloadParams, SimStepParams};
        use ludion_protocol::event::Event;
        use ludion_renderer::SpriteInstance;

        let doc = ONE_BULLET.replace(
            "    life: 60\n",
            "    life: 60\n    atlas: art/ship.atlas.yaml\n    frame: ship/idle\n",
        );
        let dir = root("break-under-volley", &[("probe", &doc)]);
        std::fs::create_dir_all(dir.join("art")).unwrap();
        let png = ludion_assets::encode_png(8, 8, &[255; 8 * 8 * 4]).unwrap();
        std::fs::write(dir.join("art/ship.png"), png).unwrap();
        let atlas = dir.join("art/ship.atlas.yaml");
        std::fs::write(
            &atlas,
            ludion_scene::emit_atlas(&ship_atlas(&["ship/idle"])),
        )
        .unwrap();

        let mut engine = engine_at(&dir).expect("the atlas resolves at build");
        engine.load_inline_scene(SCENE).expect("the scene loads");
        play_patterns(engine.world_mut(), &ctx_at(0));
        let scene = engine
            .extract_render_scene_through(320, 180, false)
            .unwrap();
        assert!(scene.sprites[0].texture.is_some(), "the bullet is textured");

        std::fs::write(&atlas, ludion_scene::emit_atlas(&ship_atlas(&["ship/hit"]))).unwrap();
        engine
            .execute(Command::AssetReload(AssetReloadParams {
                path: Some("art/ship.atlas.yaml".to_string()),
            }))
            .unwrap();
        for _ in 0..3 {
            let scene = engine
                .extract_render_scene_through(320, 180, false)
                .expect("a broken bullet atlas never fails the frame");
            assert_eq!(scene.sprites[0].texture, None);
            assert_eq!(scene.sprites[0].uv, SpriteInstance::FULL_UV);
        }
        engine.take_events();
        engine
            .execute(Command::SimStep(SimStepParams {
                ticks: 1,
                hash: false,
            }))
            .unwrap();
        let logged: Vec<_> = engine
            .take_events()
            .into_iter()
            .filter_map(|event| match event {
                Event::Log(log) => log.diagnostic,
                _ => None,
            })
            .filter(|d| d.code.as_str() == "L1152")
            .collect();
        assert_eq!(logged.len(), 1, "{logged:?}");
        assert_eq!(
            logged[0].path.as_deref(),
            Some("pattern/patterns/probe.yaml/bullets/needle/frame")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A reload whose document carries any validator error is refused, not only an unresolvable
    /// atlas: the old document stays live and the store does not move.
    #[test]
    fn a_pattern_reload_with_an_l1149_is_refused_and_the_old_doc_stays_live() {
        use ludion_protocol::Command;
        use ludion_protocol::command::AssetReloadParams;

        let key = "patterns/probe.yaml";
        for (label, edit) in [
            (
                "count",
                "    count: 0
",
            ),
            (
                "every",
                "    every: 0
    repeat: 3
",
            ),
        ] {
            let dir = root(&format!("reload-l1149-{label}"), &[("probe", ONE_BULLET)]);
            let mut engine = engine_at(&dir).expect("a clean document builds");
            let generation = |engine: &Engine| {
                engine
                    .world()
                    .resource::<DocStore<PatternDoc>>()
                    .unwrap()
                    .generation()
            };
            let before = generation(&engine);

            let edited = format!("{ONE_BULLET}{edit}");
            std::fs::write(dir.join(key), edited).unwrap();
            let refused = engine
                .execute(Command::AssetReload(AssetReloadParams {
                    path: Some(key.to_string()),
                }))
                .expect_err("a reload the validator rejects is refused");
            let named: Vec<&Diagnostic> =
                std::iter::once(&refused).chain(&refused.related).collect();
            assert!(
                named.iter().any(|d| d.code.as_str() == "L1149"),
                "{label}: {refused:?}"
            );
            let store = engine.world().resource::<DocStore<PatternDoc>>().unwrap();
            assert_eq!(store.get(key).unwrap().timeline[0].count, 1, "{label}");
            assert_eq!(generation(&engine), before, "{label}: the store moved");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// A reload that pushes *another* document into an error is refused too: `probe` loops 240
    /// frames around `leaf`, and a `leaf` whose entry moves to frame 300 overruns that loop.
    #[test]
    fn a_reload_that_breaks_another_pattern_is_refused() {
        use ludion_protocol::Command;
        use ludion_protocol::command::AssetReloadParams;

        let probe = looped(240, &["at: 0, play: patterns/leaf.yaml"]);
        let leaf = looped(0, &["at: 0, bullet: needle, speed: 1.0"]);
        let dir = root("reload-breaks-other", &[("probe", &probe), ("leaf", &leaf)]);
        let mut engine = engine_at(&dir).expect("both documents are clean");
        let key = "patterns/leaf.yaml";
        let generation = |engine: &Engine| {
            engine
                .world()
                .resource::<DocStore<PatternDoc>>()
                .unwrap()
                .generation()
        };
        let before = generation(&engine);

        std::fs::write(
            dir.join(key),
            looped(0, &["at: 300, bullet: needle, speed: 1.0"]),
        )
        .unwrap();
        let refused = engine
            .execute(Command::AssetReload(AssetReloadParams {
                path: Some(key.to_string()),
            }))
            .expect_err("a reload that breaks a playing document is refused");
        let named: Vec<&Diagnostic> = std::iter::once(&refused).chain(&refused.related).collect();
        assert!(
            named.iter().any(|d| d.code.as_str() == "L1149"
                && d.path.as_deref() == Some("pattern/patterns/probe.yaml/timeline/0/play")),
            "{refused:?}"
        );
        let store = engine.world().resource::<DocStore<PatternDoc>>().unwrap();
        assert_eq!(store.get(key).unwrap().timeline[0].at, 0);
        assert_eq!(generation(&engine), before, "the store moved");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An atlas reference the reload leaves unchanged is reported, not refused, even while it is
    /// broken: tuning the same document keeps applying and the kind draws untextured.
    #[test]
    fn an_unchanged_broken_bullet_reference_does_not_block_a_reload() {
        use ludion_protocol::Command;
        use ludion_protocol::command::{AssetReloadParams, SimStepParams};
        use ludion_protocol::event::Event;

        let key = "patterns/probe.yaml";
        let doc = ONE_BULLET.replace(
            "    life: 60
",
            "    life: 60
    atlas: art/ship.atlas.yaml
    frame: ship/idle
",
        );
        let dir = root("unchanged-broken", &[("probe", &doc)]);
        std::fs::create_dir_all(dir.join("art")).unwrap();
        let png = ludion_assets::encode_png(8, 8, &[255; 8 * 8 * 4]).unwrap();
        std::fs::write(dir.join("art/ship.png"), png).unwrap();
        let atlas = dir.join("art/ship.atlas.yaml");
        std::fs::write(
            &atlas,
            ludion_scene::emit_atlas(&ship_atlas(&["ship/idle"])),
        )
        .unwrap();

        let mut engine = engine_at(&dir).expect("the atlas resolves at build");
        engine.load_inline_scene(SCENE).expect("the scene loads");
        std::fs::write(&atlas, ludion_scene::emit_atlas(&ship_atlas(&["ship/hit"]))).unwrap();
        engine
            .execute(Command::AssetReload(AssetReloadParams {
                path: Some("art/ship.atlas.yaml".to_string()),
            }))
            .unwrap();

        std::fs::write(dir.join(key), doc.replace("speed: 4.0", "speed: 9.0")).unwrap();
        engine
            .execute(Command::AssetReload(AssetReloadParams {
                path: Some(key.to_string()),
            }))
            .expect("an unchanged broken reference does not refuse the reload");
        let store = engine.world().resource::<DocStore<PatternDoc>>().unwrap();
        assert_eq!(store.get(key).unwrap().timeline[0].speed, 9.0);

        play_patterns(engine.world_mut(), &ctx_at(0));
        let scene = engine
            .extract_render_scene_through(320, 180, false)
            .expect("a broken bullet atlas never fails the frame");
        assert_eq!(scene.sprites[0].texture, None);
        engine.take_events();
        engine
            .execute(Command::SimStep(SimStepParams {
                ticks: 1,
                hash: false,
            }))
            .unwrap();
        let logged = engine
            .take_events()
            .into_iter()
            .filter_map(|event| match event {
                Event::Log(log) => log.diagnostic,
                _ => None,
            })
            .filter(|d| d.code.as_str() == "L1152")
            .count();
        assert_eq!(logged, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- the walk ordinal ---

    /// The turret of [`SCENE`], the player every seed folds in.
    fn turret(engine: &Engine) -> EntityId {
        engine
            .world()
            .entities()
            .next()
            .expect("the scene's turret")
    }

    #[test]
    fn seeds_follow_the_walk_ordinal() {
        let doc = "\
format: 1
kind: pattern
bullets:
  needle:
    life: 600
timeline:
  - at: 0
    repeat: 3
    every: 5
    bullet: needle
    speed: 1.0
  - at: 7
    bullet: needle
    speed: 1.0
";
        let (dir, mut engine) = playing("ordinal", &[("probe", doc)], SCENE);
        for frame in 0..11u64 {
            play_patterns(engine.world_mut(), &ctx_at(frame));
        }
        let player = turret(&engine);
        let seed_at = |engine: &Engine, started: u64| {
            volleys(engine)
                .iter()
                .find(|v| v.started == FrameIndex::new(started))
                .unwrap_or_else(|| panic!("no volley fired on {started}"))
                .seed
        };
        assert_eq!(
            seed_at(&engine, 5),
            volley_seed(player, FrameIndex::new(5), 1)
        );
        assert_eq!(
            seed_at(&engine, 7),
            volley_seed(player, FrameIndex::new(7), 3),
            "entry A's three candidates count first"
        );
        assert_eq!(
            seed_at(&engine, 10),
            volley_seed(player, FrameIndex::new(10), 2)
        );
        assert_ne!(
            volley_seed(player, FrameIndex::new(7), 3),
            volley_seed(player, FrameIndex::new(7), 0),
            "the walk ordinal must fold into the seed"
        );
        let _ = std::fs::remove_dir_all(&dir);

        let probe = "\
format: 1
kind: pattern
bullets:
  needle:
    life: 600
timeline:
  - at: 0
    play: patterns/leaf.yaml
  - at: 4
    bullet: needle
    speed: 1.0
";
        let leaf = "\
format: 1
kind: pattern
bullets:
  spark:
    life: 600
timeline:
  - at: 50
    bullet: spark
  - at: 60
    bullet: spark
";
        let (dir, mut engine) = playing("ordinal-play", &[("probe", probe), ("leaf", leaf)], SCENE);
        play_patterns(engine.world_mut(), &ctx_at(4));
        let player = turret(&engine);
        let out = volleys(&engine);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].seed, volley_seed(player, FrameIndex::new(4), 2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_long_train_fires_its_last_repeat_exactly() {
        let doc = "\
format: 1
kind: pattern
bullets:
  needle:
    life: 600
timeline:
  - at: 3
    repeat: 100000
    every: 2
    shape: ring
    bullet: needle
    speed: 1.0
    angle_step_deg: 0.5
";
        let (dir, mut engine) = playing("long-train", &[("probe", doc)], SCENE);
        play_patterns(engine.world_mut(), &ctx_at(199_999));
        play_patterns(engine.world_mut(), &ctx_at(200_001));
        let out = volleys(&engine);
        assert_eq!(out.len(), 2, "{out:?}");
        assert_eq!(out[0].angle_deg, 0.5 * 99_998.0_f32);
        assert_eq!(out[1].angle_deg, 0.5 * 99_999.0_f32);
        for frame in [200_003u64, 4] {
            play_patterns(engine.world_mut(), &ctx_at(frame));
        }
        assert_eq!(volleys(&engine).len(), 2, "nothing fires off the train");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_zero_fires_once_on_its_frame() {
        let (dir, mut engine) = playing("every-zero", &[("probe", ONE_BULLET)], SCENE);
        let doc = PatternDoc {
            bullets: BTreeMap::from([("needle".to_string(), BulletKind::default())]),
            timeline: vec![TimelineEntry {
                at: 2,
                repeat: 3,
                every: 0,
                bullet: Some("needle".to_string()),
                angle_step_deg: 10.0,
                ..TimelineEntry::default()
            }],
            ..PatternDoc::default()
        };
        engine
            .world_mut()
            .resource_mut::<DocStore<PatternDoc>>()
            .unwrap()
            .upsert("patterns/probe.yaml".to_string(), doc);
        for frame in 0..5u64 {
            play_patterns(engine.world_mut(), &ctx_at(frame));
        }
        let out = volleys(&engine);
        let angles: Vec<f32> = out.iter().map(|v| v.angle_deg).collect();
        assert_eq!(angles, vec![0.0], "{out:?}");
        assert!(out.iter().all(|v| v.started == FrameIndex::new(2)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A document the validator rejects that got live through the ungated path (a replay's reapply):
    /// the walk and the reaper must survive it without an overflow, and `every: 0` fires one volley
    /// however large `repeat` is.
    #[test]
    fn a_doc_the_validator_rejects_steps_without_a_panic() {
        let entry = |at: u32, bullet: &str| TimelineEntry {
            at,
            bullet: Some(bullet.to_string()),
            ..TimelineEntry::default()
        };
        let kind = |life: u32| BulletKind {
            life,
            ..BulletKind::default()
        };
        let hostile = PatternDoc {
            bullets: BTreeMap::from([
                ("needle".to_string(), kind(60)),
                ("blink".to_string(), kind(0)),
            ]),
            timeline: vec![
                TimelineEntry {
                    repeat: 4,
                    every: 0,
                    ..entry(2, "needle")
                },
                TimelineEntry {
                    count: 300,
                    ..entry(3, "needle")
                },
                entry(4, "blink"),
                entry(u32::MAX, "needle"),
                TimelineEntry {
                    repeat: u32::MAX,
                    every: u32::MAX,
                    ..entry(5, "needle")
                },
                TimelineEntry {
                    repeat: 100_000_000,
                    every: 0,
                    ..entry(6, "needle")
                },
            ],
            ..PatternDoc::default()
        };
        let (dir, mut engine) = playing("hostile-doc", &[("probe", ONE_BULLET)], SCENE);
        engine
            .world_mut()
            .resource_mut::<DocStore<PatternDoc>>()
            .unwrap()
            .upsert("patterns/probe.yaml".to_string(), hostile);
        let on = |engine: &Engine, frame: u64, bullet: &str| {
            volleys(engine)
                .iter()
                .filter(|v| v.started == FrameIndex::new(frame) && v.bullet == bullet)
                .count()
        };
        for frame in (0..8u64).chain([u64::from(u32::MAX)]) {
            play_patterns(engine.world_mut(), &ctx_at(frame));
            if frame == 2 {
                assert_eq!(on(&engine, 2, "needle"), 1, "every: 0 fires once");
            }
            if frame == 4 {
                assert_eq!(on(&engine, 4, "blink"), 1, "fired this frame");
            }
            if frame == 6 {
                assert_eq!(
                    on(&engine, 6, "needle"),
                    1,
                    "a huge `every: 0` train fires once"
                );
            }
            advance_volleys(engine.world_mut(), &ctx_at(frame));
            if frame == 4 {
                assert_eq!(on(&engine, 4, "blink"), 0, "life 0 is reaped as it fires");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A pattern document of bullet kinds `(name, atlas, frame)`.
    fn doc_of_kinds(kinds: &[(&str, &str, &str)]) -> PatternDoc {
        let bullets: serde_json::Map<String, serde_json::Value> = kinds
            .iter()
            .map(|(name, atlas, frame)| {
                let kind = serde_json::json!({ "atlas": atlas, "frame": frame, "life": 60 });
                (name.to_string(), kind)
            })
            .collect();
        serde_json::from_value(serde_json::json!({ "bullets": bullets })).unwrap()
    }

    fn atlas_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludion-bullet-atlas-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("art")).unwrap();
        let png = ludion_assets::encode_png(2, 2, &[255u8; 16]).unwrap();
        std::fs::write(dir.join("art/ship.png"), png).unwrap();
        let mut regions = BTreeMap::new();
        regions.insert(
            "ship/idle".to_string(),
            ludion_scene::AtlasRegion {
                rect: [0, 0, 2, 2],
                pivot: [1.0, 1.0],
            },
        );
        let atlas = ludion_scene::AtlasDocument {
            generated_from: "ship.atlas-source.yaml".to_string(),
            pixels_per_unit: 16.0,
            color_map: "ship.png".to_string(),
            filter: ludion_scene::AtlasFilter::Nearest,
            regions,
            flipbooks: Vec::new(),
        };
        std::fs::write(
            dir.join("art/ship.atlas.yaml"),
            ludion_scene::emit_atlas(&atlas),
        )
        .unwrap();
        dir
    }

    #[test]
    fn kinds_sharing_an_atlas_decode_it_once_and_report_at_their_own_pointers() {
        let dir = atlas_root("shared");
        let doc = doc_of_kinds(&[
            ("a", "art/ship.atlas.yaml", "ship/nope"),
            ("b", "art/ship.atlas.yaml", "ship/also-nope"),
        ]);
        let mut loaded = LoadedAtlases::new();
        let out = bullet_atlas_diagnostics(&dir, "p.yaml", &doc, &mut loaded);
        assert_eq!(loaded.len(), 1);
        for name in ["a", "b"] {
            let path = format!("pattern/p.yaml/bullets/{name}/frame");
            assert!(
                out.iter()
                    .any(|d| d.code.as_str() == "L1152" && d.path.as_deref() == Some(path.as_str())),
                "{out:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn kinds_sharing_a_missing_atlas_each_report_l1151() {
        let dir = atlas_root("missing");
        let doc = doc_of_kinds(&[
            ("a", "art/gone.atlas.yaml", "x"),
            ("b", "art/gone.atlas.yaml", "x"),
        ]);
        let mut loaded = LoadedAtlases::new();
        let out = bullet_atlas_diagnostics(&dir, "p.yaml", &doc, &mut loaded);
        assert_eq!(loaded.len(), 1);
        for name in ["a", "b"] {
            let path = format!("pattern/p.yaml/bullets/{name}/atlas");
            assert!(
                out.iter()
                    .any(|d| d.code.as_str() == "L1151" && d.path.as_deref() == Some(path.as_str())),
                "{out:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
