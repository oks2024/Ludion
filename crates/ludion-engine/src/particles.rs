//! The particle motion kernel and the analytic emitter evaluator
//! (`Plan/particles-engine-and-editor.md` decisions 1, 2 and 4).
//!
//! Two layers, deliberately separable:
//!
//! - **[`Motion`]** — a short list of [`Segment`]s with a closed-form [`Motion::position_at`]. It
//!   knows nothing about emitters, components or rendering, and it is the one implementation of
//!   "where is this thing after `t` seconds" in the engine. That matters because a second consumer
//!   is coming that runs the same function *sim-side* (a projectile pattern's hit test): two
//!   implementations of the same trajectory would drift, and the visual one would be the lie.
//! - **[`evaluate`]** — a [`ParticleEmitter`] plus a [`Transform`] plus a [`SimTime`] in, a stream
//!   of [`ParticleInstance`]s out. It reads *nothing else*: no world, no resource, no accumulated
//!   state, no clock. The same three arguments therefore give the same particles in any process, on
//!   any thread count, at any point of a replay, and after any `engine.restore`.
//!
//! **A particle is an index, not an object.** Continuous particle `i` is released at
//! `started + i / rate` and lives `life_i` seconds; the burst's `j` is released at `started`. Each
//! one draws its own life, speed, direction and spawn offset from
//! `DeterministicRng::derived_salted(seed, FrameIndex(i), salt)` — a stream named by the particle's
//! own index, so nothing depends on iteration order and only the *live* window is ever walked
//! ([`live_window`]), never the history. An endless emitter's window admits **negative** indices,
//! which is what puts an authored ambient plume in steady state at frame 0 with no warm-up.

use std::ops::Range;

use ludion_core::ids::EntityId;
use ludion_core::math::glam::{Mat4, Quat, Vec3};
use ludion_core::math::{cos, exp, powf, sin, sqrt};
use ludion_core::rng::DeterministicRng;
use ludion_core::time::{FrameIndex, SimTime};
use ludion_renderer::ParticleInstance;

use crate::components::{
    EmitShape, PARTICLE_MAX_LIVE_PER_EMITTER, ParticleBlend, ParticleEmitter, Transform,
};
use crate::extract::rotation_quat;

/// How many phases one [`Motion`] may hold.
///
/// A fixed budget rather than a `Vec`: a `Motion` is built per particle per frame on the extract
/// path, so it stays `Copy` and allocation-free. Five is the pattern vocabulary's four authored
/// phases (a launch, a change of heading, a settle, a rest) plus the open-ended straight tail a
/// bullet appends.
pub const MOTION_MAX_SEGMENTS: usize = 5;

/// Names the random stream of a continuous emitter's particle `i`.
///
/// A distinct constant per stream so index `0` of a burst and index `0` of the continuous emission
/// never draw the same life and speed on the same emitter.
pub const PARTICLE_SALT_CONTINUOUS: u64 = 0x7061_7274_5f63_6f6e;

/// Names the random stream of a burst's particle `j`. See [`PARTICLE_SALT_CONTINUOUS`].
pub const PARTICLE_SALT_BURST: u64 = 0x7061_7274_5f62_7273;

/// One phase of a trajectory, in the frame its [`Motion`] was built in.
///
/// A segment reports the offset travelled *since the phase began*; [`Motion::position_at`] chains
/// them, so a phase never needs to know where it starts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Segment {
    /// Constant acceleration `(0, −gravity, 0)` with linear drag `k = drag` (per second), the
    /// motion of everything thrown, dropped or blown.
    ///
    /// With `a = (0, −gravity, 0)`, the closed form is exact rather than integrated:
    ///
    /// ```text
    /// k = 0:  p(t) = v·t + ½·a·t²
    /// k > 0:  p(t) = (a/k)·t + (v − a/k)·(1 − e^(−k·t))/k
    /// ```
    ///
    /// `a/k` is the terminal velocity, which is why the drag form reads as "drift at terminal speed
    /// plus a decaying memory of the launch". The exponential goes through
    /// [`ludion_core::math::exp`] — a platform `libm` differs across OSes, and this value reaches an
    /// image.
    Ballistic {
        /// Velocity at the start of the phase, in units per second.
        velocity: Vec3,
        /// Downward acceleration in units per second squared; negative lifts.
        gravity: f32,
        /// Linear drag in inverse seconds. Zero (or non-finite) takes the drag-free form.
        drag: f32,
        /// How long the phase lasts, in seconds. [`f32::INFINITY`] runs to the end of the motion.
        duration: f32,
    },
    /// Straight travel along `heading` at a speed sweeping linearly from `from` to `to` — a launch,
    /// a settle, or (with `from == to`) a constant-speed leg.
    ///
    /// With `a = (to − from)/duration` the closed form is the textbook one:
    ///
    /// ```text
    /// p(t) = heading · (from·t + ½·a·t²)
    /// ```
    ///
    /// An open-ended phase (or one that does not change speed) has no acceleration and degrades to
    /// `heading · from · t`.
    Ramp {
        /// Unit direction of travel; unchanged for the whole phase.
        heading: Vec3,
        /// Speed at the start of the phase, in units per second.
        from: f32,
        /// Speed at the end of the phase, in units per second.
        to: f32,
        /// How long the phase lasts, in seconds. [`f32::INFINITY`] runs to the end of the motion.
        duration: f32,
    },
    /// A circular arc: constant `speed` with the heading rotating at `rate` radians per second in
    /// the plane whose unit normal is `normal`, positive turning `heading` toward `normal × heading`.
    ///
    /// With `r = speed / rate` the radius and `b = normal × heading` the in-plane binormal:
    ///
    /// ```text
    /// p(t) = heading · r·sin(rate·t) + b · r·(1 − cos(rate·t))
    /// ```
    ///
    /// A rate near zero has no radius to speak of and degrades to the straight line `heading ·
    /// speed · t` rather than dividing by it. The trigonometry goes through
    /// [`ludion_core::math`] for the same reason the ballistic exponential does.
    Turn {
        /// Unit direction of travel at the start of the phase.
        heading: Vec3,
        /// Unit normal of the plane of motion.
        normal: Vec3,
        /// Speed along the arc, in units per second.
        speed: f32,
        /// Turn rate in **radians** per second; negative turns the other way.
        rate: f32,
        /// How long the phase lasts, in seconds. [`f32::INFINITY`] runs to the end of the motion.
        duration: f32,
    },
}

impl Segment {
    /// An open-ended straight leg at constant `speed` along `heading` — the tail a trajectory ends
    /// with when it should keep going rather than hold still.
    pub fn straight(heading: Vec3, speed: f32) -> Segment {
        Segment::Ramp {
            heading,
            from: speed,
            to: speed,
            duration: f32::INFINITY,
        }
    }

    /// How long this phase lasts, in seconds ([`f32::INFINITY`] for an open-ended one).
    pub fn duration(self) -> f32 {
        match self {
            Segment::Ballistic { duration, .. }
            | Segment::Ramp { duration, .. }
            | Segment::Turn { duration, .. } => duration,
        }
    }

    /// The heading a following phase starts from: unchanged by a ramp, rotated by `rate · duration`
    /// by a turn, and the launch direction of a ballistic phase (which is FX-only and never chained).
    ///
    /// An open-ended phase has no end, so it reports the heading it began with — as does a turn
    /// whose rate is negligible, since its offset is already a straight line.
    pub fn end_heading(&self) -> Vec3 {
        match *self {
            Segment::Ballistic { velocity, .. } => velocity.normalize_or_zero(),
            Segment::Ramp { heading, .. } => heading,
            Segment::Turn {
                heading,
                normal,
                rate,
                duration,
                ..
            } => {
                if !duration.is_finite() || rate.abs() < TURN_MIN_RATE {
                    return heading;
                }
                let angle = rate * duration;
                heading * cos(angle) + normal.cross(heading) * sin(angle)
            }
        }
    }

    /// The speed a following phase starts at, in units per second.
    pub fn end_speed(&self) -> f32 {
        match *self {
            Segment::Ballistic { velocity, .. } => velocity.length(),
            Segment::Ramp { to, .. } => to,
            Segment::Turn { speed, .. } => speed,
        }
    }

    /// The offset travelled `t` seconds into this phase. `t` at or below zero is the origin, and a
    /// `t` past [`duration`](Segment::duration) is *not* clamped here — [`Motion::position_at`]
    /// owns that, because only it knows the phase boundaries.
    pub fn offset_at(self, t: f32) -> Vec3 {
        match self {
            Segment::Ballistic {
                velocity,
                gravity,
                drag,
                ..
            } => {
                let t = if t > 0.0 { t } else { return Vec3::ZERO };
                let acceleration = Vec3::new(0.0, -gravity, 0.0);
                if drag > 0.0 && drag.is_finite() {
                    let terminal = acceleration / drag;
                    terminal * t + (velocity - terminal) * ((1.0 - exp(-drag * t)) / drag)
                } else {
                    velocity * t + acceleration * (0.5 * t * t)
                }
            }
            Segment::Ramp {
                heading,
                from,
                to,
                duration,
            } => {
                let t = if t > 0.0 { t } else { return Vec3::ZERO };
                if to != from && duration > 0.0 && duration.is_finite() {
                    heading * (from * t + (to - from) * (t * t) / (2.0 * duration))
                } else {
                    heading * (from * t)
                }
            }
            Segment::Turn {
                heading,
                normal,
                speed,
                rate,
                ..
            } => {
                let t = if t > 0.0 { t } else { return Vec3::ZERO };
                if rate.abs() < TURN_MIN_RATE {
                    return heading * (speed * t);
                }
                let radius = speed / rate;
                let angle = rate * t;
                heading * (radius * sin(angle))
                    + normal.cross(heading) * (radius * (1.0 - cos(angle)))
            }
        }
    }
}

/// Below this turn rate (radians per second) the arc's radius stops being representable and the
/// phase is a straight line — the guard that keeps `speed / rate` finite.
const TURN_MIN_RATE: f32 = 1e-6;

/// A trajectory: an origin plus up to [`MOTION_MAX_SEGMENTS`] phases walked in order.
///
/// Past the end of the last finite phase the motion **holds still** at the point it reached; a
/// trajectory that should keep going says so with an infinite last phase.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    origin: Vec3,
    segments: [Option<Segment>; MOTION_MAX_SEGMENTS],
}

impl Motion {
    /// A motion that starts at `origin` and, with no phases pushed, never leaves it.
    pub fn new(origin: Vec3) -> Self {
        Motion {
            origin,
            segments: [None; MOTION_MAX_SEGMENTS],
        }
    }

    /// The one-phase ballistic motion a [`ParticleEmitter`] builds: launched at `velocity`, falling
    /// under `gravity`, slowed by `drag`, for as long as the particle lives.
    pub fn ballistic(origin: Vec3, velocity: Vec3, gravity: f32, drag: f32) -> Self {
        let mut motion = Motion::new(origin);
        motion.push(Segment::Ballistic {
            velocity,
            gravity,
            drag,
            duration: f32::INFINITY,
        });
        motion
    }

    /// Appends a phase, returning `false` and appending nothing once every slot is full — a
    /// refusal rather than a silent truncation, because a dropped phase is an invisible wrong
    /// trajectory.
    pub fn push(&mut self, segment: Segment) -> bool {
        for slot in &mut self.segments {
            if slot.is_none() {
                *slot = Some(segment);
                return true;
            }
        }
        false
    }

    /// How many phases this motion holds.
    pub fn len(&self) -> usize {
        self.segments.iter().flatten().count()
    }

    /// Whether this motion has no phases (and therefore never leaves its origin).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Where the moving thing is `t` seconds after the motion began.
    ///
    /// Walks the phases by their durations, handing each the share of `t` that falls inside it and
    /// accumulating the offsets. A negative `t` is the origin.
    pub fn position_at(&self, t: f32) -> Vec3 {
        let mut position = self.origin;
        let mut remaining = if t > 0.0 { t } else { return position };
        for segment in self.segments.iter().flatten() {
            let span = remaining.min(segment.duration());
            position += segment.offset_at(span);
            remaining -= span;
            if !remaining.is_finite() || remaining <= 0.0 {
                break;
            }
        }
        position
    }
}

/// The camera's world-space right and up axes — the basis every camera-facing quad in one frame
/// billboards against.
///
/// Passed *in* rather than derived per particle so the sprite path and the particle path can never
/// disagree about which way "up" is in a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraBasis {
    /// Unit world-space right.
    pub right: Vec3,
    /// Unit world-space up.
    pub up: Vec3,
}

impl CameraBasis {
    /// The basis of the camera whose view matrix is `view`: the first two columns of its inverse.
    #[allow(clippy::disallowed_methods)] // render-side: GPU matrix, never reaches a hash
    pub fn from_view(view: Mat4) -> Self {
        let model = view.inverse();
        CameraBasis {
            right: model.x_axis.truncate(),
            up: model.y_axis.truncate(),
        }
    }
}

/// Which of an emitter's two streams a particle belongs to, and its index within that stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stream {
    /// Continuous particle `i`, released at `started + i / rate`. Negative for an endless emitter's
    /// steady state.
    Continuous(i64),
    /// Burst particle `j`, released at `started`.
    Burst(i64),
}

/// The particle indices of `emitter` that could be alive at `now`: the continuous half-open index
/// range, and how many of the burst's `0..burst` particles are still within reach.
///
/// Continuous index `i` is released `i / rate` seconds after `started`, so it is live when
/// `now − life[1] < t_i ≤ now`. Three things narrow that:
///
/// - `rate == 0` (or non-finite) means no continuous emission at all — a burst-only emitter.
/// - `duration > 0` makes the emitter finite: `i ≥ 0` and `t_i ≤ started + duration`.
/// - `duration == 0` makes it **endless, and therefore in steady state at every instant**. The
///   range is left unbounded below, so negative indices — particles released before `started` —
///   are real, and an authored ambient plume is already full on the frame the scene loads.
///
/// Release and retirement are decided from the exact tick count with the division last (the rule
/// of `flipbook_frame`), so a particle on its exact release or death tick lands on the right side.
///
/// The burst count is `emitter.burst` while `0 ≤ now − started < life[1]` and zero outside it; the
/// per-particle life draw then decides exactly which of them are still alive.
///
/// The continuous range and the burst count together are clamped to
/// [`PARTICLE_MAX_LIVE_PER_EMITTER`]: the continuous range keeps its **newest** particles first,
/// and the burst gets only what room is left. The semantic validator already rejects an over-cap
/// emitter at check time (`L2107`), but a component built in Rust never passes through a
/// validator, and a render loop must not be freezable from game code.
pub fn live_window(emitter: &ParticleEmitter, now: SimTime) -> (Range<i64>, u32) {
    let (continuous, burst) = unclamped_burst_window(emitter, now);
    let room = i64::from(PARTICLE_MAX_LIVE_PER_EMITTER) - (continuous.end - continuous.start);
    (continuous, burst.min(u32::try_from(room).unwrap_or(0)))
}

/// [`live_window`] with the continuous range capped but the burst count as authored.
fn unclamped_burst_window(emitter: &ParticleEmitter, now: SimTime) -> (Range<i64>, u32) {
    let ticks = ticks_since(now, emitter.started);
    let hz = f64::from(now.rate().hz());
    let life_max = f64::from(emitter.life[1].max(0.0));

    let burst = if ticks >= 0.0 && ticks < life_max * hz {
        emitter.burst
    } else {
        0
    };

    if !emitter.rate.is_finite() || emitter.rate <= 0.0 {
        return (0..0, burst);
    }
    let rate = f64::from(emitter.rate);

    // `t_i ≤ now` is `i ≤ ticks · rate / hz`; `t_i > now − life_max` is
    // `i > (ticks − life_max · hz) · rate / hz`. Each product is exact and the division comes last.
    let mut newest = (ticks * rate / hz).floor();
    let mut oldest = ((ticks - life_max * hz) * rate / hz).floor() + 1.0;
    if emitter.duration > 0.0 && emitter.duration.is_finite() {
        oldest = oldest.max(0.0);
        newest = newest.min((f64::from(emitter.duration) * rate).floor());
    }

    let (oldest, newest) = (index_of(oldest), index_of(newest));
    if newest < oldest {
        return (0..0, burst);
    }
    let end = newest.saturating_add(1);
    let cap = i64::from(PARTICLE_MAX_LIVE_PER_EMITTER);
    let start = if end.saturating_sub(oldest) > cap {
        end - cap
    } else {
        oldest
    };
    (start..end, burst)
}

/// Every particle `emitter` shows at `now`, in `(continuous ascending, then burst ascending)`
/// order — the order [`extract`](crate::extract) emits and therefore the order the renderer's
/// tie-break sees.
///
/// `density` in `0..=1` is the quality knob's seam: it scales `rate` and `burst` **before** the
/// window is computed, so a thinned emitter emits fewer particles rather than hiding some of them.
/// The thinned set is a coarser plume, not a subset — lowering the rate moves every release time.
///
/// The result is a pure function of the four arguments plus `basis`: nothing here reads the world,
/// a resource, a clock, or any state left by a previous frame.
pub fn evaluate(
    emitter: &ParticleEmitter,
    transform: &Transform,
    now: SimTime,
    density: f32,
    basis: CameraBasis,
) -> impl Iterator<Item = ParticleInstance> {
    evaluate_keyed(emitter, transform, now, density, basis).map(|(_, particle)| particle)
}

/// [`evaluate`] with each particle's stream, for the presenter's cross-tick correspondence.
pub(crate) fn evaluate_keyed(
    emitter: &ParticleEmitter,
    transform: &Transform,
    now: SimTime,
    density: f32,
    basis: CameraBasis,
) -> impl Iterator<Item = (Stream, ParticleInstance)> {
    let emitter = thinned(emitter, density);
    let (continuous, burst) = live_window(&emitter, now);
    let context = EmitterContext::new(emitter, transform, now, basis);

    continuous
        .map(Stream::Continuous)
        .chain((0..i64::from(burst)).map(Stream::Burst))
        .filter_map(move |stream| context.particle(stream).map(|p| (stream, p)))
}

/// The presenter's identity for one particle of `owner`'s emitter that started on `started`.
pub(crate) fn instance_key(
    owner: EntityId,
    started: FrameIndex,
    stream: Stream,
) -> ludion_renderer::InstanceKey {
    let (burst, index) = match stream {
        Stream::Continuous(i) => (false, i),
        Stream::Burst(j) => (true, j),
    };
    ludion_renderer::InstanceKey {
        owner,
        started: started.0,
        burst,
        index,
    }
}

/// An upper bound on how many particles [`evaluate`] yields for the same arguments: the live
/// window's continuous length plus its burst count. `evaluate` drops dead and below-floor particles
/// after the window, so the bound is exact only for an emitter that drops none.
pub(crate) fn live_bound(emitter: &ParticleEmitter, now: SimTime, density: f32) -> usize {
    let emitter = thinned(emitter, density);
    let (continuous, burst) = live_window(&emitter, now);
    usize::try_from(continuous.end - continuous.start).unwrap_or(0) + burst as usize
}

/// Everything one emitter's particles share, resolved once per emitter per frame.
struct EmitterContext {
    emitter: ParticleEmitter,
    /// The emitter's position *now*: particles live in the emitter's frame, so a moving emitter
    /// carries its plume rather than leaving it behind.
    translation: Vec3,
    /// Orients the emission cone and the spawn shape; gravity stays world −Y.
    rotation: Quat,
    /// The uniform `Transform.scale.x`, multiplying extents, speeds and sizes.
    scale: f32,
    basis: CameraBasis,
    /// Signed seconds since `started`: drives motion and phase, never a live/dead decision.
    elapsed: f64,
    /// Signed whole ticks since `started`, and the tick rate: the exact side of the live/dead decision.
    ticks: f64,
    hz: f64,
    /// `1 − cos(spread_deg)`, the cone's cap fraction — precomputed because it is the same for
    /// every particle of the emitter.
    cone: f32,
}

impl EmitterContext {
    fn new(
        emitter: ParticleEmitter,
        transform: &Transform,
        now: SimTime,
        basis: CameraBasis,
    ) -> Self {
        let cone = 1.0 - cos(emitter.spread_deg.clamp(0.0, 180.0).to_radians());
        EmitterContext {
            translation: Vec3::from(transform.translation),
            rotation: rotation_quat(transform.rotation),
            scale: transform.scale[0],
            basis,
            elapsed: seconds_since(now, emitter.started),
            ticks: ticks_since(now, emitter.started),
            hz: f64::from(now.rate().hz()),
            cone,
            emitter,
        }
    }

    /// One particle, or `None` if it is not alive at this instant or lies below the emitter's
    /// `floor`.
    ///
    /// **The draw order is fixed and load-bearing**: life, speed, direction, spawn offset. Anything
    /// that reorders it re-rolls every particle in the repo, so treat it the way a chunk size is
    /// treated. The offset draws come last precisely so that changing `shape` (which changes how
    /// many draws the offset costs) leaves life, speed and direction untouched.
    fn particle(&self, stream: Stream) -> Option<ParticleInstance> {
        let rate = f64::from(self.emitter.rate);
        let (mut rng, age, released_for) = match stream {
            Stream::Continuous(i) => (
                DeterministicRng::derived_salted(
                    self.emitter.seed,
                    FrameIndex::new(i as u64),
                    PARTICLE_SALT_CONTINUOUS,
                ),
                self.elapsed - i as f64 / rate,
                // Emission units since release, `age · rate`, with the division last.
                (self.ticks * rate - i as f64 * self.hz) / self.hz,
            ),
            Stream::Burst(j) => (
                DeterministicRng::derived_salted(
                    self.emitter.seed,
                    FrameIndex::new(j as u64),
                    PARTICLE_SALT_BURST,
                ),
                self.elapsed,
                // The burst's "emission units" are ticks, so its life is `life · hz`.
                self.ticks,
            ),
        };

        let life = lerp(self.emitter.life[0], self.emitter.life[1], rng.next_f32());
        // A dead or not-yet-released particle drops out here, and so does any NaN the emitter's
        // own numbers could have produced: an unorderable age is not "alive".
        let life_limit = match stream {
            Stream::Continuous(_) => f64::from(life) * rate,
            Stream::Burst(_) => f64::from(life) * self.hz,
        };
        if !life.is_finite()
            || life <= 0.0
            || !age.is_finite()
            || !released_for.is_finite()
            || released_for < 0.0
            || released_for >= life_limit
        {
            return None;
        }
        // On the release tick the seconds age can read a hair below zero.
        let age = age.max(0.0);
        let speed = lerp(self.emitter.speed[0], self.emitter.speed[1], rng.next_f32());
        let direction = cone_direction(&mut rng, self.cone);
        let offset = shape_offset(&mut rng, &self.emitter);

        let age = age as f32;
        let motion = Motion::ballistic(
            self.rotation * (offset * self.scale),
            self.rotation * (direction * (speed * self.scale)),
            self.emitter.gravity,
            self.emitter.drag,
        );
        let center = self.translation + motion.position_at(age);
        if center.y < self.emitter.floor {
            return None;
        }

        let phase = (age / life).clamp(0.0, 1.0);
        let mut color = [0.0f32; 4];
        for (channel, slot) in color.iter_mut().enumerate() {
            *slot = lerp(
                self.emitter.start_color[channel],
                self.emitter.end_color[channel],
                phase,
            );
        }
        Some(ParticleInstance {
            center,
            right: self.basis.right,
            up: self.basis.up,
            size: lerp(self.emitter.start_size, self.emitter.end_size, phase) * self.scale,
            color,
            softness: self.emitter.softness,
            additive: self.emitter.blend == ParticleBlend::Additive,
            lit: self.emitter.lit,
        })
    }
}

/// A direction uniformly distributed over the spherical cap of half-angle `acos(1 − cone)` about
/// local **+Y**: `cos θ = 1 − u·cone` is the inverse-CDF that keeps the density uniform per unit
/// area rather than per unit angle (which would crowd the axis).
fn cone_direction(rng: &mut DeterministicRng, cone: f32) -> Vec3 {
    let cos_theta = 1.0 - rng.next_f32() * cone;
    let sin_theta = sqrt((1.0 - cos_theta * cos_theta).max(0.0));
    let phi = rng.next_f32() * std::f32::consts::TAU;
    Vec3::new(sin_theta * cos(phi), cos_theta, sin_theta * sin(phi))
}

/// A spawn offset uniformly distributed through the emitter's [`EmitShape`], in the emitter's local
/// frame and before `Transform.scale.x`.
fn shape_offset(rng: &mut DeterministicRng, emitter: &ParticleEmitter) -> Vec3 {
    match emitter.shape {
        EmitShape::Point => Vec3::ZERO,
        // Uniform through a ball: a direction on the sphere times `r·u^(1/3)` — the cube root is
        // what stops the samples piling up at the centre.
        EmitShape::Sphere => {
            let cos_theta = 1.0 - 2.0 * rng.next_f32();
            let sin_theta = sqrt((1.0 - cos_theta * cos_theta).max(0.0));
            let phi = rng.next_f32() * std::f32::consts::TAU;
            let radius = emitter.extent[0] * powf(rng.next_f32(), 1.0 / 3.0);
            Vec3::new(sin_theta * cos(phi), cos_theta, sin_theta * sin(phi)) * radius
        }
        EmitShape::Box => {
            let x = (2.0 * rng.next_f32() - 1.0) * emitter.extent[0];
            let y = (2.0 * rng.next_f32() - 1.0) * emitter.extent[1];
            let z = (2.0 * rng.next_f32() - 1.0) * emitter.extent[2];
            Vec3::new(x, y, z)
        }
        // Uniform across a disc in the local XZ plane: `r = R·√u` for the same reason.
        EmitShape::Disc => {
            let radius = emitter.extent[0] * sqrt(rng.next_f32());
            let phi = rng.next_f32() * std::f32::consts::TAU;
            Vec3::new(radius * cos(phi), 0.0, radius * sin(phi))
        }
    }
}

/// `emitter` with its `rate` and `burst` scaled by `density` (clamped to `0..=1`; a non-finite
/// density is read as full).
fn thinned(emitter: &ParticleEmitter, density: f32) -> ParticleEmitter {
    let density = if density.is_finite() {
        density.clamp(0.0, 1.0)
    } else {
        1.0
    };
    if density >= 1.0 {
        return emitter.clone();
    }
    ParticleEmitter {
        rate: emitter.rate * density,
        burst: (f64::from(emitter.burst) * f64::from(density)).round() as u32,
        ..emitter.clone()
    }
}

/// Seconds from `started` to `now`, **signed** — a `started` in the future is a negative elapsed,
/// which is what lets an endless emitter's window run below index zero.
///
/// Derived from the exact tick counts on every call and never accumulated (`clock_ticks`'s
/// discipline in `extract`), and computed in unsigned arithmetic on both sides so no tick count can
/// wrap on its way through a signed integer.
fn seconds_since(now: SimTime, started: FrameIndex) -> f64 {
    let seconds_per_tick = now.rate().seconds_per_tick();
    let (now_ticks, started_ticks) = (now.ticks(), started.as_u64());
    if now_ticks >= started_ticks {
        (now_ticks - started_ticks) as f64 * seconds_per_tick
    } else {
        -((started_ticks - now_ticks) as f64 * seconds_per_tick)
    }
}

/// Whole ticks from `started` to `now`, **signed**, in unsigned arithmetic like [`seconds_since`].
/// The live/dead decisions multiply by this exact count and divide last (see `flipbook_frame`),
/// so the tick count itself never carries a rounded second. A lifetime that is not exact in `f32`
/// (0.1 s at 60 Hz) can still keep a particle one tick longer than its ideal boundary.
fn ticks_since(now: SimTime, started: FrameIndex) -> f64 {
    let (now_ticks, started_ticks) = (now.ticks(), started.as_u64());
    if now_ticks >= started_ticks {
        (now_ticks - started_ticks) as f64
    } else {
        -((started_ticks - now_ticks) as f64)
    }
}

/// A particle index from a floating-point bound, saturating well inside the range `f64` represents
/// integers exactly in — a NaN reads as zero.
fn index_of(value: f64) -> i64 {
    if value.is_nan() {
        0
    } else {
        value.clamp(-9.0e15, 9.0e15) as i64
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use super::*;
    use ludion_core::math::f64 as math64;
    use ludion_core::time::TickRate;

    const BASIS: CameraBasis = CameraBasis {
        right: Vec3::X,
        up: Vec3::Y,
    };

    fn at(tick: u64) -> SimTime {
        SimTime::from_ticks(tick, TickRate::DEFAULT)
    }

    /// How many indices a window holds (`Range<i64>` is not an `ExactSizeIterator`).
    fn span(window: &Range<i64>) -> i64 {
        window.end - window.start
    }

    /// `e^-1` at `f32` precision — the hand expectation the drag tests below are checked against,
    /// so they pin the closed form rather than re-deriving it.
    const E_INV: f32 = 0.367_879_45;

    /// Drag-free ballistics is exactly `v·t + ½·a·t²`, hand-computed: launched along +X at 1 u/s
    /// from `(1, 2, 3)` under gravity 10, two seconds in, the offset is `(2, −20, 0)`.
    #[test]
    fn drag_free_ballistics_is_the_textbook_parabola() {
        let motion = Motion::ballistic(Vec3::new(1.0, 2.0, 3.0), Vec3::X, 10.0, 0.0);
        let p = motion.position_at(2.0);
        assert!((p - Vec3::new(3.0, -18.0, 3.0)).length() < 1e-5, "got {p}");
        assert_eq!(motion.position_at(0.0), Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(
            motion.position_at(-1.0),
            Vec3::new(1.0, 2.0, 3.0),
            "a negative time is the origin"
        );
    }

    /// With drag and no gravity the closed form collapses to `v·(1 − e^(−k·t))/k`: at `k = 1`,
    /// `t = 1`, `v = (2, 0, 0)`, that is `2·(1 − e⁻¹)`.
    #[test]
    fn drag_only_ballistics_matches_the_hand_computed_exponential() {
        let motion = Motion::ballistic(Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0), 0.0, 1.0);
        let expected = 2.0 * (1.0 - E_INV);
        let p = motion.position_at(1.0);
        assert!((p.x - expected).abs() < 1e-5, "got {} want {expected}", p.x);
        assert!(p.y.abs() < 1e-6 && p.z.abs() < 1e-6);
    }

    /// Gravity *and* drag: dropped from rest under gravity 4 with `k = 2`, terminal velocity is
    /// −2 u/s, and half a second in the fall is `−2·0.5 + 2·(1 − e⁻¹)/2 = −(1 − (1 − e⁻¹))`.
    #[test]
    fn ballistics_under_gravity_and_drag_falls_toward_terminal_velocity() {
        let motion = Motion::ballistic(Vec3::ZERO, Vec3::ZERO, 4.0, 2.0);
        let expected = -1.0 + (1.0 - E_INV);
        let p = motion.position_at(0.5);
        assert!((p.y - expected).abs() < 1e-5, "got {} want {expected}", p.y);

        // Far out, the fall is terminal-velocity drift: `y ≈ −2·t + 1`.
        let far = motion.position_at(10.0);
        assert!((far.y - (-19.0)).abs() < 1e-3, "got {}", far.y);
    }

    /// Phases are walked by their durations, and a motion past its last finite phase holds still.
    #[test]
    fn motion_walks_its_segments_by_duration_and_then_holds() {
        let mut motion = Motion::new(Vec3::ZERO);
        assert!(motion.is_empty());
        assert!(motion.push(Segment::Ballistic {
            velocity: Vec3::X,
            gravity: 0.0,
            drag: 0.0,
            duration: 1.0,
        }));
        assert!(motion.push(Segment::Ballistic {
            velocity: Vec3::Y * 2.0,
            gravity: 0.0,
            drag: 0.0,
            duration: 2.0,
        }));
        assert_eq!(motion.len(), 2);

        assert_eq!(motion.position_at(0.5), Vec3::new(0.5, 0.0, 0.0));
        assert_eq!(motion.position_at(1.0), Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(motion.position_at(2.0), Vec3::new(1.0, 2.0, 0.0));
        assert_eq!(motion.position_at(3.0), Vec3::new(1.0, 4.0, 0.0));
        assert_eq!(
            motion.position_at(50.0),
            Vec3::new(1.0, 4.0, 0.0),
            "past the last finite phase the motion holds still"
        );
    }

    /// The fifth phase is refused rather than silently dropped.
    #[test]
    fn a_motion_refuses_a_fifth_segment() {
        let segment = Segment::Ballistic {
            velocity: Vec3::X,
            gravity: 0.0,
            drag: 0.0,
            duration: 1.0,
        };
        let mut motion = Motion::new(Vec3::ZERO);
        for _ in 0..MOTION_MAX_SEGMENTS {
            assert!(motion.push(segment));
        }
        assert!(!motion.push(segment));
        assert_eq!(motion.len(), MOTION_MAX_SEGMENTS);
    }

    /// A ramp is `from·t + ½·a·t²` hand-computed: 6 → 2 u/s over one second travels
    /// `6 − 4/2 = 4` units, and its speed and heading at the end are what a following phase starts
    /// from.
    #[test]
    fn a_ramp_matches_the_hand_computed_quadratic() {
        let ramp = Segment::Ramp {
            heading: Vec3::X,
            from: 6.0,
            to: 2.0,
            duration: 1.0,
        };
        assert_eq!(ramp.offset_at(1.0), Vec3::new(4.0, 0.0, 0.0));
        assert_eq!(ramp.offset_at(0.0), Vec3::ZERO);
        assert_eq!(ramp.offset_at(-1.0), Vec3::ZERO);
        assert_eq!(ramp.end_heading(), Vec3::X);
        assert_eq!(ramp.end_speed(), 2.0);
    }

    /// An open-ended ramp — the tail every trajectory that keeps going ends with — is a straight
    /// line at its one speed, with no acceleration term to divide an infinite duration into.
    #[test]
    fn an_open_ended_ramp_is_a_straight_line() {
        let tail = Segment::straight(Vec3::Y, 3.0);
        assert_eq!(tail.duration(), f32::INFINITY);
        assert_eq!(tail.offset_at(2.0), Vec3::new(0.0, 6.0, 0.0));
        assert_eq!(tail.offset_at(100.0), Vec3::new(0.0, 300.0, 0.0));
        assert_eq!(tail.end_speed(), 3.0);

        // A ramp that *does* change speed but never ends holds its start speed rather than
        // accelerating toward a target it will never reach.
        let endless_ramp = Segment::Ramp {
            heading: Vec3::X,
            from: 2.0,
            to: 9.0,
            duration: f32::INFINITY,
        };
        assert_eq!(endless_ramp.offset_at(3.0), Vec3::new(6.0, 0.0, 0.0));
    }

    /// A quarter turn of radius 1 (`speed = rate = π/2`, one second) puts the arc's end one radius
    /// forward and one radius sideways, and leaves the heading rotated a quarter turn.
    #[test]
    fn a_quarter_turn_lands_one_radius_forward_and_sideways() {
        let turn = Segment::Turn {
            heading: Vec3::X,
            normal: Vec3::Y,
            speed: FRAC_PI_2,
            rate: FRAC_PI_2,
            duration: 1.0,
        };
        // `Y × X` is `−Z`: the in-plane sideways direction a positive rate turns toward.
        let want = Vec3::new(1.0, 0.0, -1.0);
        let got = turn.offset_at(1.0);
        assert!((got - want).length() < 1e-5, "got {got}, want {want}");
        assert!(
            (turn.end_heading() - Vec3::NEG_Z).length() < 1e-5,
            "got {}",
            turn.end_heading()
        );
        assert_eq!(turn.end_speed(), FRAC_PI_2);
        assert_eq!(turn.offset_at(0.0), Vec3::ZERO);
    }

    /// A rate below the guard has no radius to divide by, and the phase is the straight line it
    /// visually is.
    #[test]
    fn a_negligible_turn_rate_is_a_straight_line() {
        let turn = Segment::Turn {
            heading: Vec3::X,
            normal: Vec3::Y,
            speed: 5.0,
            rate: TURN_MIN_RATE * 0.5,
            duration: 2.0,
        };
        assert_eq!(turn.offset_at(2.0), Vec3::new(10.0, 0.0, 0.0));
        assert_eq!(turn.end_heading(), Vec3::X);
    }

    /// The four-segment shape the vocabulary was sized for — ramp, turn, ramp, open-ended tail —
    /// with heading and speed carried across every boundary by the caller. At each boundary the
    /// motion is exactly the sum of the completed offsets: nothing is lost or double-counted.
    #[test]
    fn a_four_segment_motion_carries_position_across_every_boundary() {
        let launch = Segment::Ramp {
            heading: Vec3::X,
            from: 6.0,
            to: 2.0,
            duration: 1.0,
        };
        let curve = Segment::Turn {
            heading: launch.end_heading(),
            normal: Vec3::Y,
            speed: launch.end_speed(),
            rate: FRAC_PI_2,
            duration: 1.0,
        };
        let settle = Segment::Ramp {
            heading: curve.end_heading(),
            from: curve.end_speed(),
            to: 4.0,
            duration: 2.0,
        };
        let tail = Segment::straight(settle.end_heading(), settle.end_speed());

        let origin = Vec3::new(1.0, 2.0, 3.0);
        let mut motion = Motion::new(origin);
        for segment in [launch, curve, settle, tail] {
            assert!(motion.push(segment));
        }
        assert_eq!(motion.len(), 4);

        let mut walked = origin;
        let mut elapsed = 0.0;
        for segment in [launch, curve, settle] {
            walked += segment.offset_at(segment.duration());
            elapsed += segment.duration();
            let got = motion.position_at(elapsed);
            assert!(
                (got - walked).length() < 1e-5,
                "at t = {elapsed}: got {got}, want {walked}"
            );
        }

        // The tail keeps going instead of holding still: three more seconds at 4 u/s along −Z.
        let far = motion.position_at(elapsed + 3.0);
        let want = walked + Vec3::NEG_Z * 12.0;
        assert!((far - want).length() < 1e-4, "got {far}, want {want}");
    }

    /// Decision 7's precision pin: the longest authored life (600 frames at 60 Hz, ten seconds)
    /// evaluated in `f32` stays within a thousandth of the same closed forms in `f64`.
    #[test]
    fn the_closed_forms_hold_f64_accuracy_over_the_longest_life() {
        let launch = Segment::Ramp {
            heading: Vec3::X,
            from: 6.0,
            to: 2.0,
            duration: 1.0,
        };
        let curve = Segment::Turn {
            heading: Vec3::X,
            normal: Vec3::Y,
            speed: 2.0,
            rate: 0.5,
            duration: f32::INFINITY,
        };
        let mut motion = Motion::new(Vec3::ZERO);
        assert!(motion.push(launch));
        assert!(motion.push(curve));

        let t = 600.0 / 60.0;
        let got = motion.position_at(t);

        // The same two forms in `f64`, hand-written so the reference cannot inherit an `f32` bug.
        let ramp_x = 6.0_f64 * 1.0 + (2.0_f64 - 6.0) * 1.0 / 2.0;
        let arc = t as f64 - 1.0;
        let radius = 2.0_f64 / 0.5;
        let want_x = ramp_x + radius * math64::sin(0.5 * arc);
        let want_z = -radius * (1.0 - math64::cos(0.5 * arc));

        assert!(
            (got.x as f64 - want_x).abs() < 1e-3,
            "x: got {} want {want_x}",
            got.x
        );
        assert!(
            (got.z as f64 - want_z).abs() < 1e-3,
            "z: got {} want {want_z}",
            got.z
        );
        assert!(got.y.abs() < 1e-6, "the motion never leaves its plane");
    }

    fn endless() -> ParticleEmitter {
        ParticleEmitter {
            rate: 10.0,
            life: [1.0, 1.0],
            burst: 0,
            duration: 0.0,
            ..ParticleEmitter::default()
        }
    }

    /// An endless emitter's window admits negative indices at `started`, and holds `rate × life`
    /// particles at every instant — decision 4's steady state, stated as arithmetic.
    #[test]
    fn an_endless_emitter_window_is_steady_from_frame_zero() {
        let emitter = endless();
        let (early, _) = live_window(&emitter, at(0));
        assert!(early.start < 0, "the plume predates `started`: {early:?}");
        assert_eq!(span(&early), 10, "rate × life particles at tick 0");

        let (late, _) = live_window(&emitter, at(10_000));
        assert_eq!(span(&late), span(&early));
        assert!(late.start > 0);
    }

    /// A finite emitter never emits before `started` and stops `duration` seconds after it, with
    /// the tail of the plume outliving the emission by one lifetime.
    #[test]
    fn a_finite_emitter_window_opens_at_started_and_closes_after_duration() {
        let emitter = ParticleEmitter {
            duration: 1.0,
            started: FrameIndex::new(60),
            ..endless()
        };
        assert_eq!(live_window(&emitter, at(0)).0, 0..0, "before `started`");
        assert_eq!(live_window(&emitter, at(60)).0, 0..1, "the first particle");

        // One second of emission at 10/s is indices 0..=10; a second later they have all expired.
        assert_eq!(live_window(&emitter, at(120)).0.end, 11);
        assert_eq!(live_window(&emitter, at(180)).0, 0..0, "the plume is spent");
    }

    /// `rate: 0` is a burst-only emitter: no continuous particles ever, and the burst count is live
    /// only inside its own lifetime.
    #[test]
    fn a_burst_only_emitter_has_no_continuous_particles() {
        let emitter = ParticleEmitter {
            rate: 0.0,
            burst: 25,
            life: [1.0, 1.0],
            started: FrameIndex::new(30),
            ..ParticleEmitter::default()
        };
        assert_eq!(live_window(&emitter, at(0)), (0..0, 0), "before `started`");
        assert_eq!(live_window(&emitter, at(30)), (0..0, 25));
        assert_eq!(live_window(&emitter, at(60)), (0..0, 25));
        assert_eq!(live_window(&emitter, at(120)), (0..0, 0), "all expired");
    }

    /// A `rate` or `burst` past the cap cannot freeze a render loop even when the component skipped
    /// validation (built in Rust, never checked): the continuous window and the burst together
    /// never exceed `PARTICLE_MAX_LIVE_PER_EMITTER`, and `evaluate` walks no more than that.
    #[test]
    fn the_live_window_is_clamped_to_the_cap() {
        let cap = i64::from(PARTICLE_MAX_LIVE_PER_EMITTER);
        let emitter = ParticleEmitter {
            rate: 1.0e6,
            life: [10.0, 10.0],
            ..ParticleEmitter::default()
        };
        let (window, _) = live_window(&emitter, at(600));
        assert_eq!(span(&window), cap);

        let cases = [
            ("burst only", 0.0, u32::MAX),
            ("rate and a large burst", 1.0e6, 1_000_000),
            ("a small rate and a huge burst", 10.0, u32::MAX),
        ];
        for (label, rate, burst) in cases {
            let emitter = ParticleEmitter {
                rate,
                burst,
                life: [10.0, 10.0],
                ..ParticleEmitter::default()
            };
            let (window, burst) = live_window(&emitter, at(60));
            assert!(
                span(&window) + i64::from(burst) <= cap,
                "{label}: {} continuous + {burst} burst exceeds the cap",
                span(&window)
            );
            let walked = evaluate(&emitter, &Transform::default(), at(60), 1.0, BASIS).count();
            assert!(walked as i64 <= cap, "{label}: evaluate yielded {walked}");
        }
    }

    /// Release and death land on the right tick even when the boundary is exact. The oracle is pure
    /// integer arithmetic: particle `i` of rate `num/den` is released on tick `i·hz·den / num` and
    /// retires `h/2` seconds later, so at tick `t` it is alive iff `i·hz·den ≤ t·num` and
    /// `2·(t·num − i·hz·den) < h·num·hz`.
    #[test]
    fn particles_are_released_and_retired_on_their_exact_ticks() {
        for hz in [24u64, 30, 60, 144] {
            let rate_hz = TickRate::hz_of(hz as u32);
            let rates: [(u64, u64); 6] = [(3, 1), (5, 1), (15, 1), (30, 1), (15, 2), (25, 2)];
            for (num, den) in rates {
                for h in 1u64..=3 {
                    let half_life = h as f32 / 2.0;
                    let emitter = ParticleEmitter {
                        rate: num as f32 / den as f32,
                        life: [half_life, half_life],
                        duration: 1000.0,
                        burst: 0,
                        ..ParticleEmitter::default()
                    };
                    for t in 0..=4 * hz {
                        let alive = (0u64..)
                            .take_while(|i| i * hz * den <= t * num)
                            .filter(|i| 2 * (t * num - i * hz * den) < h * num * hz)
                            .count();
                        let now = SimTime::from_ticks(t, rate_hz);
                        let walked =
                            evaluate(&emitter, &Transform::default(), now, 1.0, BASIS).count();
                        assert_eq!(
                            walked, alive,
                            "{hz} Hz, rate {num}/{den}, life {h}/2 s, tick {t}"
                        );
                    }
                }
            }
            for h in 1u64..=3 {
                let half_life = h as f32 / 2.0;
                let burst = ParticleEmitter {
                    rate: 0.0,
                    life: [half_life, half_life],
                    duration: 1000.0,
                    burst: 5,
                    ..ParticleEmitter::default()
                };
                for t in 0..=4 * hz {
                    let now = SimTime::from_ticks(t, rate_hz);
                    let walked = evaluate(&burst, &Transform::default(), now, 1.0, BASIS).count();
                    let expected = if 2 * t < h * hz { 5 } else { 0 };
                    assert_eq!(walked, expected, "{hz} Hz burst, life {h}/2 s, tick {t}");
                }
            }
        }
    }

    /// The steady state again, this time as pictures rather than indices: the spread of ages the
    /// evaluator produces at tick 0 and at tick 10,000 is the same ladder of `1 / rate` rungs, with
    /// the same number of them.
    ///
    /// The two spreads are equal **up to where `now` falls between two releases** — tick 10,000 is
    /// not on this emitter's emission grid, and never has to be, so every age is shifted by the same
    /// sub-interval remainder. That constant offset *is* the steady state seen from a different
    /// instant; what would break decision 4 is a different count or an uneven ladder.
    ///
    /// Ages are read back from the emitter's own colour ramp, which with `life: [1, 1]` and the
    /// default white→transparent fade is exactly `alpha = 1 − age`.
    #[test]
    fn a_steady_emitter_shows_the_same_age_spread_at_every_instant() {
        let emitter = ParticleEmitter {
            spread_deg: 0.0,
            ..endless()
        };
        let transform = Transform::default();
        // Micro-units of alpha, so the comparison is an integer one.
        let spread = |tick: u64| {
            let mut alphas: Vec<i64> = evaluate(&emitter, &transform, at(tick), 1.0, BASIS)
                .map(|p| (f64::from(p.color[3]) * 1.0e6).round() as i64)
                .collect();
            alphas.sort_unstable();
            alphas
        };
        let (early, late) = (spread(0), spread(10_000));
        assert_eq!(early.len(), 10, "the plume is full at frame 0");
        assert_eq!(late.len(), early.len(), "and still full 166 seconds in");

        let offset = late[0] - early[0];
        assert!(
            early
                .iter()
                .zip(&late)
                .all(|(a, b)| (b - a - offset).abs() <= 1),
            "the same ladder, offset by the sampling phase: {early:?} vs {late:?}"
        );
        // One rung per emission interval: `1 / rate` seconds of a one-second life is 0.1 alpha.
        assert!(
            early.windows(2).all(|w| (w[1] - w[0] - 100_000).abs() <= 1),
            "the rungs are one emission interval apart: {early:?}"
        );
    }

    /// The evaluator is a pure function: the same `(emitter, transform, now)` twice is element-wise
    /// equal, with no state carried between the calls.
    #[test]
    fn evaluating_the_same_instant_twice_is_element_wise_equal() {
        let emitter = ParticleEmitter {
            shape: EmitShape::Sphere,
            burst: 7,
            gravity: 9.0,
            drag: 0.5,
            ..endless()
        };
        let transform = Transform {
            translation: [3.0, 1.0, -2.0],
            rotation: [10.0, 45.0, 0.0],
            scale: [2.0, 2.0, 2.0],
        };
        let once: Vec<_> = evaluate(&emitter, &transform, at(90), 1.0, BASIS).collect();
        let again: Vec<_> = evaluate(&emitter, &transform, at(90), 1.0, BASIS).collect();
        assert!(!once.is_empty());
        assert_eq!(once, again);
    }

    /// `floor` clips a particle out of the frame without shortening its life: the same emitter with
    /// the floor raised above the fall shows strictly fewer particles.
    #[test]
    fn the_floor_cuts_particles_below_it() {
        let emitter = ParticleEmitter {
            spread_deg: 0.0,
            speed: [0.0, 0.0],
            gravity: 20.0,
            rate: 40.0,
            life: [2.0, 2.0],
            ..ParticleEmitter::default()
        };
        let transform = Transform::default();
        let uncut = evaluate(&emitter, &transform, at(600), 1.0, BASIS).count();

        let floored = ParticleEmitter {
            floor: -1.0,
            ..emitter
        };
        let cut = evaluate(&floored, &transform, at(600), 1.0, BASIS).count();
        assert!(uncut > 0);
        assert!(
            cut < uncut,
            "a floor at −1 must cut the fall: {cut} of {uncut}"
        );
    }

    /// `live_bound` caps the count `evaluate` yields and meets it when nothing is floored away.
    #[test]
    fn the_live_bound_caps_the_evaluated_count() {
        let emitter = ParticleEmitter {
            spread_deg: 0.0,
            speed: [0.0, 0.0],
            gravity: 20.0,
            rate: 40.0,
            life: [2.0, 2.0],
            burst: 7,
            ..ParticleEmitter::default()
        };
        let transform = Transform::default();
        for tick in [0, 30, 600] {
            let walked = evaluate(&emitter, &transform, at(tick), 1.0, BASIS).count();
            assert_eq!(live_bound(&emitter, at(tick), 1.0), walked, "tick {tick}");
        }
        let floored = ParticleEmitter {
            floor: -1.0,
            ..emitter
        };
        let cut = evaluate(&floored, &transform, at(600), 1.0, BASIS).count();
        assert!(live_bound(&floored, at(600), 1.0) > cut);
    }

    /// `density` is the quality seam: half the density is half the particles, within one.
    #[test]
    fn density_thins_the_plume_proportionally() {
        let emitter = ParticleEmitter {
            rate: 100.0,
            life: [1.0, 1.0],
            burst: 40,
            ..ParticleEmitter::default()
        };
        let transform = Transform::default();
        let full = evaluate(&emitter, &transform, at(300), 1.0, BASIS).count() as i64;
        let half = evaluate(&emitter, &transform, at(300), 0.5, BASIS).count() as i64;
        assert!((half * 2 - full).abs() <= 2, "{half} is not half of {full}");
    }

    /// The emitter's frame: a particle at age zero sits at the emitter's own translation for a
    /// point emitter, and the cone's axis follows `Transform.rotation`.
    #[test]
    fn particles_live_in_the_emitter_frame() {
        // A single burst particle, no spread, no gravity: its offset is pure `speed × age` along
        // the rotated +Y.
        let emitter = ParticleEmitter {
            rate: 0.0,
            burst: 1,
            spread_deg: 0.0,
            speed: [4.0, 4.0],
            life: [10.0, 10.0],
            ..ParticleEmitter::default()
        };
        let transform = Transform {
            translation: [5.0, 0.0, 0.0],
            // A quarter turn about +Z takes local +Y onto world −X.
            rotation: [0.0, 0.0, 90.0],
            scale: [1.0, 1.0, 1.0],
        };
        let one = evaluate(&emitter, &transform, at(60), 1.0, BASIS)
            .next()
            .expect("the burst particle is alive");
        assert!(
            (one.center - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-4,
            "got {}",
            one.center
        );
    }

    /// `Transform.scale.x` multiplies speed and size, the `Sprite`/`GroundMarker` convention.
    #[test]
    fn uniform_scale_multiplies_speed_and_size() {
        let emitter = ParticleEmitter {
            rate: 0.0,
            burst: 1,
            spread_deg: 0.0,
            speed: [4.0, 4.0],
            life: [10.0, 10.0],
            start_size: 0.5,
            end_size: 0.5,
            ..ParticleEmitter::default()
        };
        let unit = evaluate(&emitter, &Transform::default(), at(60), 1.0, BASIS)
            .next()
            .unwrap();
        let big = evaluate(
            &emitter,
            &Transform {
                scale: [3.0, 3.0, 3.0],
                ..Transform::default()
            },
            at(60),
            1.0,
            BASIS,
        )
        .next()
        .unwrap();
        assert!((big.size - unit.size * 3.0).abs() < 1e-6);
        assert!((big.center.y - unit.center.y * 3.0).abs() < 1e-4);
    }

    /// The blend and lit flags and the camera basis travel verbatim onto every instance, at full
    /// and at a thinned density.
    #[test]
    fn the_blend_flag_and_camera_basis_reach_every_instance() {
        let emitter = ParticleEmitter {
            blend: ParticleBlend::Additive,
            softness: 0.25,
            lit: true,
            ..endless()
        };
        let basis = CameraBasis {
            right: Vec3::new(0.0, 0.0, 1.0),
            up: Vec3::Y,
        };
        for density in [1.0, 0.5] {
            let instances: Vec<_> =
                evaluate(&emitter, &Transform::default(), at(0), density, basis).collect();
            assert!(!instances.is_empty());
            assert!(
                instances.iter().all(|p| {
                    p.additive && p.lit && p.softness == 0.25 && p.right == basis.right
                })
            );
        }
        let unlit = ParticleEmitter {
            lit: false,
            ..emitter
        };
        assert!(evaluate(&unlit, &Transform::default(), at(0), 1.0, basis).all(|p| !p.lit));
    }

    /// The camera basis is the inverse view matrix's first two columns — the same derivation the
    /// sprite billboarding path uses, which is why both come through this one function.
    #[test]
    fn the_camera_basis_is_the_inverse_view_columns() {
        let view = Mat4::look_at_rh(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y);
        let basis = CameraBasis::from_view(view);
        assert!((basis.right - Vec3::X).length() < 1e-6, "{}", basis.right);
        assert!((basis.up - Vec3::Y).length() < 1e-6, "{}", basis.up);
    }

    /// A camera on the +X side looks down -X, so its right is -Z. The on-axis camera above has an
    /// identity rotation, where the view matrix and its inverse share their basis columns; a
    /// rotated camera tells them apart.
    #[test]
    fn a_side_cameras_basis_is_rotated() {
        let view = Mat4::look_at_rh(Vec3::X * 5.0, Vec3::ZERO, Vec3::Y);
        let basis = CameraBasis::from_view(view);
        assert!(
            (basis.right - (-Vec3::Z)).length() < 1e-6,
            "{}",
            basis.right
        );
        assert!((basis.up - Vec3::Y).length() < 1e-6, "{}", basis.up);
    }
}
