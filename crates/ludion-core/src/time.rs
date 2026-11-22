//! Fixed-timestep time types.
//!
//! The engine ticks at a fixed rate ([`TickRate`], 60 Hz by default). Sim time is therefore an
//! **integer tick count**, never an accumulated floating-point `dt`. Accumulating `dt += 1.0/60.0`
//! each frame would drift (f64 cannot represent 1/60 exactly), so simulated seconds are *derived*
//! from the exact tick count on demand ([`SimTime::seconds`]) and never fed back into state. This
//! is the determinism rationale: the source of truth is a `u64`, which is exact and reproducible.
//!
//! These are *types only*. There is no loop driver here — the tick loop lives in `ludion-sim`
//! (architecture doc §2). Wall-clock time never appears in the sim; time reaches systems as
//! [`FrameIndex`] / [`SimTime`] parameters.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The fixed simulation tick rate, in hertz.
///
/// `dt` is `1 / hz` and is a compile-time-fixed constant of the simulation: `sim.set_speed`
/// scales *when* the live scheduler ticks, but never changes `dt` (that would break determinism).
///
/// # Example
///
/// ```
/// use ludion_core::time::TickRate;
/// assert_eq!(TickRate::DEFAULT.hz(), 60);
/// assert!((TickRate::DEFAULT.seconds_per_tick() - 1.0 / 60.0).abs() < 1e-12);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(try_from = "u32")]
pub struct TickRate(u32);

impl TryFrom<u32> for TickRate {
    type Error = &'static str;

    /// Refuses `0`, the same invariant [`TickRate::hz_of`] asserts, so a deserialized rate cannot
    /// bypass it.
    fn try_from(hz: u32) -> Result<Self, Self::Error> {
        if hz == 0 {
            Err("a tick rate must be at least 1 Hz")
        } else {
            Ok(TickRate(hz))
        }
    }
}

impl TickRate {
    /// The default 60 Hz tick rate.
    pub const DEFAULT: TickRate = TickRate(60);

    /// Constructs a tick rate from a frequency in hertz.
    ///
    /// # Panics
    /// Panics if `hz` is zero (a zero-hertz clock has an undefined timestep).
    pub const fn hz_of(hz: u32) -> Self {
        assert!(hz != 0, "TickRate must be non-zero");
        TickRate(hz)
    }

    /// The tick frequency in hertz.
    pub const fn hz(self) -> u32 {
        self.0
    }

    /// The fixed timestep `dt` in seconds (`1 / hz`).
    pub fn seconds_per_tick(self) -> f64 {
        1.0 / f64::from(self.0)
    }
}

impl Default for TickRate {
    fn default() -> Self {
        TickRate::DEFAULT
    }
}

/// A monotonically increasing frame counter.
///
/// Every mutating protocol response carries the `FrameIndex` at which it applied, which is what
/// makes agent reasoning about causality — and replay stitching — possible (architecture doc §6).
///
/// # Example
///
/// ```
/// use ludion_core::time::FrameIndex;
/// let f = FrameIndex::ZERO;
/// assert_eq!(f.as_u64(), 0);
/// assert_eq!(f.next().as_u64(), 1);
/// ```
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize, JsonSchema,
)]
pub struct FrameIndex(pub u64);

impl FrameIndex {
    /// The first frame.
    pub const ZERO: FrameIndex = FrameIndex(0);

    /// Constructs a frame index from a raw count.
    pub const fn new(index: u64) -> Self {
        FrameIndex(index)
    }

    /// The raw frame count.
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// The next frame.
    ///
    /// # Panics
    /// Panics on overflow of the underlying `u64` (unreachable in practice: at 60 Hz this is
    /// billions of years).
    pub const fn next(self) -> Self {
        FrameIndex(self.0 + 1)
    }
}

/// A point in simulated time, stored as an exact tick count against a [`TickRate`].
///
/// Seconds are derived on demand and never accumulated (see the module docs for why). Equality is
/// by `(ticks, rate)`.
///
/// # Example
///
/// ```
/// use ludion_core::time::{SimTime, TickRate};
/// let t = SimTime::from_ticks(90, TickRate::DEFAULT);
/// assert_eq!(t.ticks(), 90);
/// assert!((t.seconds() - 1.5).abs() < 1e-12); // 90 ticks / 60 Hz
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SimTime {
    ticks: u64,
    rate: TickRate,
}

impl SimTime {
    /// Constructs a `SimTime` from an exact tick count and the rate those ticks are measured at.
    pub const fn from_ticks(ticks: u64, rate: TickRate) -> Self {
        SimTime { ticks, rate }
    }

    /// The exact elapsed tick count — the deterministic source of truth.
    pub const fn ticks(self) -> u64 {
        self.ticks
    }

    /// The rate these ticks are measured against.
    pub const fn rate(self) -> TickRate {
        self.rate
    }

    /// Elapsed simulated seconds, derived exactly from the tick count (`ticks / hz`).
    ///
    /// This is a pure function of `ticks` and `rate`; it is never fed back into simulation state,
    /// so no floating-point drift accumulates.
    pub fn seconds(self) -> f64 {
        self.ticks as f64 * self.rate.seconds_per_tick()
    }
}

/// Q16 fixed-point ticks: how many fractional units make one tick (1/65536 of a tick each).
pub const Q16_PER_TICK: u64 = 65536;

/// A point in gameplay time with a fractional tick, stored as an exact Q16 count (1/65536 tick).
///
/// [`SimTime`] counts whole ticks; once the gameplay clock runs slower than the sim
/// ([`GameClock::scale`]) gameplay time sits between ticks, and this is its exact form. Seconds are
/// derived on demand, never accumulated. A system reads it as `SystemCtx::game_instant`.
///
/// # Example
///
/// ```
/// use ludion_core::time::{GameTime, TickRate};
/// let t = GameTime::from_ticks(90, TickRate::DEFAULT);
/// assert_eq!(t.ticks_floor(), 90);
/// assert!((t.seconds() - 1.5).abs() < 1e-12);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct GameTime {
    q16: u64,
    rate: TickRate,
}

impl GameTime {
    /// A whole-tick instant: `ticks` ticks at `rate`.
    ///
    /// ```
    /// use ludion_core::time::{GameTime, TickRate};
    /// assert_eq!(GameTime::from_ticks(2, TickRate::DEFAULT).as_q16(), 2 * 65536);
    /// ```
    pub const fn from_ticks(ticks: u64, rate: TickRate) -> Self {
        GameTime {
            q16: ticks.saturating_mul(Q16_PER_TICK),
            rate,
        }
    }

    /// An instant from a raw Q16 count (1/65536 tick) at `rate`.
    ///
    /// ```
    /// use ludion_core::time::{GameTime, TickRate};
    /// let half = GameTime::from_q16(32768, TickRate::DEFAULT);
    /// assert_eq!(half.ticks_floor(), 0);
    /// ```
    pub const fn from_q16(q16: u64, rate: TickRate) -> Self {
        GameTime { q16, rate }
    }

    /// The raw Q16 count (1/65536 tick) — the deterministic source of truth.
    ///
    /// ```
    /// use ludion_core::time::{GameTime, TickRate};
    /// assert_eq!(GameTime::from_q16(98304, TickRate::DEFAULT).as_q16(), 98304);
    /// ```
    pub const fn as_q16(self) -> u64 {
        self.q16
    }

    /// Whole ticks elapsed, the fraction dropped.
    ///
    /// ```
    /// use ludion_core::time::{GameTime, TickRate};
    /// assert_eq!(GameTime::from_q16(3 * 65536 + 65535, TickRate::DEFAULT).ticks_floor(), 3);
    /// ```
    pub const fn ticks_floor(self) -> u64 {
        self.q16 / Q16_PER_TICK
    }

    /// How many whole ticks this instant crossed when it advanced by `step_q16`: `0` or `1` for a
    /// step of at most a tick.
    ///
    /// ```
    /// use ludion_core::time::{GameTime, TickRate};
    /// let t = GameTime::from_q16(65536 + 16384, TickRate::DEFAULT);
    /// assert_eq!(t.ticks_crossed(16384), 0);
    /// assert_eq!(t.ticks_crossed(32768), 1);
    /// // The first tick enters frame 0; a held tick crosses nothing.
    /// let zero = GameTime::from_q16(0, TickRate::DEFAULT);
    /// assert_eq!(zero.ticks_crossed(65536), 1);
    /// assert_eq!(zero.ticks_crossed(0), 0);
    /// ```
    pub const fn ticks_crossed(self, step_q16: u32) -> u64 {
        let now = self.q16 as i128;
        let q = Q16_PER_TICK as i128;
        // A signed floor: the tick that enters frame 0 from before it crosses 1.
        (now.div_euclid(q) - (now - step_q16 as i128).div_euclid(q)) as u64
    }

    /// The whole-tick [`SimTime`] at or before this instant.
    ///
    /// ```
    /// use ludion_core::time::{GameTime, SimTime, TickRate};
    /// let t = GameTime::from_q16(2 * 65536 + 1, TickRate::DEFAULT);
    /// assert_eq!(t.floor(), SimTime::from_ticks(2, TickRate::DEFAULT));
    /// ```
    pub const fn floor(self) -> SimTime {
        SimTime::from_ticks(self.ticks_floor(), self.rate)
    }

    /// The rate these ticks are measured against.
    ///
    /// ```
    /// use ludion_core::time::{GameTime, TickRate};
    /// assert_eq!(GameTime::from_ticks(1, TickRate::hz_of(30)).rate().hz(), 30);
    /// ```
    pub const fn rate(self) -> TickRate {
        self.rate
    }

    /// Elapsed gameplay seconds, derived from the Q16 count and never fed back into state.
    ///
    /// ```
    /// use ludion_core::time::{GameTime, TickRate};
    /// let t = GameTime::from_q16(65536 / 2, TickRate::hz_of(60));
    /// assert!((t.seconds() - 0.5 / 60.0).abs() < 1e-12);
    /// ```
    pub fn seconds(self) -> f64 {
        self.q16 as f64 / 65536.0 * self.rate.seconds_per_tick()
    }

    /// Fractional ticks since the frame `started`: negative when `started` is ahead.
    ///
    /// ```
    /// use ludion_core::time::{FrameIndex, GameTime, TickRate};
    /// let t = GameTime::from_q16(10 * 65536 + 32768, TickRate::DEFAULT);
    /// assert_eq!(t.ticks_since(FrameIndex::new(4)), 6.5);
    /// assert_eq!(t.ticks_since(FrameIndex::new(12)), -1.5);
    /// ```
    pub fn ticks_since(self, started: FrameIndex) -> f64 {
        let delta = i128::from(self.q16) - i128::from(started.as_u64()) * i128::from(Q16_PER_TICK);
        delta as f64 / 65536.0
    }
}

/// A whole-tick [`SimTime`] is the same instant as a [`GameTime`] with no fraction, so an evaluator
/// taking `impl Into<GameTime>` accepts either.
///
/// ```
/// use ludion_core::time::{GameTime, SimTime, TickRate};
/// let t = SimTime::from_ticks(3, TickRate::DEFAULT);
/// assert_eq!(GameTime::from(t), GameTime::from_ticks(3, TickRate::DEFAULT));
/// ```
impl From<SimTime> for GameTime {
    fn from(t: SimTime) -> Self {
        GameTime::from_ticks(t.ticks(), t.rate())
    }
}

/// A [`GameTime`] equals a [`SimTime`] when it is that whole tick at the same rate.
///
/// ```
/// use ludion_core::time::{GameTime, SimTime, TickRate};
/// let t = SimTime::from_ticks(3, TickRate::DEFAULT);
/// assert!(GameTime::from(t) == t);
/// assert!(GameTime::from_q16(3 * 65536 + 1, TickRate::DEFAULT) != t);
/// ```
impl PartialEq<SimTime> for GameTime {
    fn eq(&self, other: &SimTime) -> bool {
        *self == GameTime::from(*other)
    }
}

/// The gameplay clock: how much of the tick count the game has excluded from its own time.
///
/// The sim keeps ticking while a game is frozen — a pause overlay's input has to run, and so does
/// the host — but everything the engine evaluates in closed form derives its state from
/// `now − started`: a volley's bullets, a pattern timeline, a particle emitter's live window, a
/// flipbook's frame. Without a clock they cannot be frozen: bullets fly through a pause and a
/// timeline fires entries whose frame passed while the game was not running.
///
/// So a game that freezes calls [`hold`](GameClock::hold) on this resource **every frame it is
/// frozen**, and every closed-form evaluator reads gameplay time — [`game_frame`](GameClock::game_frame)
/// and [`game_time`](GameClock::game_time) — instead of the raw tick count. A default
/// clock holds nothing, so a game that never calls `hold` is bit-identical to one built before this
/// existed: `held == 0` makes `game_frame` the identity.
///
/// `Command::SimPause` is the other mechanism and stays distinct: it stops the sim ticking at all.
/// This one keeps it ticking and stops gameplay time.
///
/// # Example
///
/// ```
/// use ludion_core::time::{FrameIndex, GameClock};
/// let mut clock = GameClock::default();
/// assert_eq!(clock.game_frame(FrameIndex::new(10)), FrameIndex::new(10));
///
/// // The game freezes during frame 10: gameplay time stops at 10 …
/// clock.hold();
/// assert_eq!(clock.game_frame(FrameIndex::new(10)), FrameIndex::new(9));
/// // … and the driver folds the held frame in at the start of frame 11.
/// assert!(!clock.advance());
/// assert_eq!(clock.game_frame(FrameIndex::new(11)), FrameIndex::new(10));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GameClock {
    /// Frames already folded out of gameplay time.
    #[serde(default)]
    held: u64,
    /// Whether the frame now running was marked frozen.
    #[serde(default)]
    holding: bool,
    /// Q16 ticks already folded out by scaled frames. Never normalized into `held`: a scaled frame
    /// is not a held one.
    // Outside the typed snapshot schema (so `replay_content_hash` is unchanged); the snapshot still carries it.
    #[schemars(skip)]
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    slowed_q16: u64,
    /// The scale requested by the frame now running, in Q16.
    // Outside the typed snapshot schema (so `replay_content_hash` is unchanged); the snapshot still carries it.
    #[schemars(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_q16: Option<u32>,
    /// The scale step folded into the tick now running, in Q16 (a full tick is `Q16_PER_TICK`).
    // Outside the typed snapshot schema (so `replay_content_hash` is unchanged); the snapshot still carries it.
    #[schemars(skip)]
    #[serde(default = "full_step", skip_serializing_if = "is_full_step")]
    step_q16: u32,
    /// The f32 bits of the frame's first invalid scale request, for the engine to report.
    #[serde(skip)]
    #[schemars(skip)]
    rejected: Option<u32>,
}

const FULL_STEP: u32 = Q16_PER_TICK as u32;

fn full_step() -> u32 {
    FULL_STEP
}

fn is_full_step(step: &u32) -> bool {
    *step == FULL_STEP
}

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

impl Default for GameClock {
    fn default() -> Self {
        GameClock {
            held: 0,
            holding: false,
            slowed_q16: 0,
            pending_q16: None,
            step_q16: FULL_STEP,
            rejected: None,
        }
    }
}

impl GameClock {
    /// The smallest accepted scale, 1/1024: a positive request below it clamps up to it.
    pub const MIN_SCALE: f32 = 1.0 / 1024.0;

    /// Marks the frame now running as frozen: gameplay time does not enter the next tick.
    ///
    /// Idempotent within a frame — a game may call it from several systems — so the cost of an
    /// extra call is nothing. A hold wins over [`scale`](GameClock::scale).
    pub fn hold(&mut self) {
        self.holding = true;
    }

    /// Requests that gameplay time enter the next tick at fraction `s` of a full tick, `s` in
    /// `(0, 1]`: call it every frame the game is slowed, as with [`hold`](GameClock::hold).
    ///
    /// Several calls in one frame compose by `min`. The request is quantized to Q16 and clamped up
    /// to [`MIN_SCALE`](GameClock::MIN_SCALE). A non-finite value, `s <= 0` or `s > 1` is ignored
    /// and latched for the engine to report as `L4260`.
    ///
    /// ```
    /// use ludion_core::time::{FrameIndex, GameClock};
    /// let mut clock = GameClock::default();
    /// clock.scale(0.5);
    /// assert!(clock.advance());
    /// assert_eq!(clock.applied_scale(), 0.5);
    /// // Half a tick is excluded: gameplay time lags the sim by half a tick.
    /// assert_eq!(clock.game_frame(FrameIndex::new(1)), FrameIndex::new(0));
    /// ```
    pub fn scale(&mut self, s: f32) {
        if !(s.is_finite() && s > 0.0 && s <= 1.0) {
            if self.rejected.is_none() {
                self.rejected = Some(s.to_bits());
            }
            return;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped into u32 range
        let q = ((f64::from(s) * 65536.0).round() as u32).clamp(64, FULL_STEP);
        if q < FULL_STEP {
            self.pending_q16 = Some(self.pending_q16.map_or(q, |p| p.min(q)));
        }
    }

    /// How many frames are already excluded from gameplay time.
    pub const fn held(self) -> u64 {
        self.held
    }

    /// Whether the frame now running was marked frozen.
    pub const fn is_holding(self) -> bool {
        self.holding
    }

    /// Whether the tick now running was folded at a scale below 1.
    pub const fn is_scaling(self) -> bool {
        self.step_q16 < FULL_STEP
    }

    /// The Q16 step the tick now running was folded at; `Q16_PER_TICK` unless scaled. A held tick
    /// is reported by [`advance`](GameClock::advance) returning `false`, not here.
    pub const fn step_q16(self) -> u32 {
        self.step_q16
    }

    /// [`step_q16`](GameClock::step_q16) as a fraction of a tick, exact in f32.
    pub fn applied_scale(self) -> f32 {
        self.step_q16 as f32 / 65536.0
    }

    /// Q16 ticks already excluded by scaled frames.
    pub const fn slowed_q16(self) -> u64 {
        self.slowed_q16
    }

    /// Takes the first invalid [`scale`](GameClock::scale) value of this frame, if any.
    pub fn take_rejected_scale(&mut self) -> Option<f32> {
        self.rejected.take().map(f32::from_bits)
    }

    /// Whether this clock has excluded nothing at all — the default, and what lets a snapshot omit
    /// the field entirely.
    pub const fn is_zero(&self) -> bool {
        self.held == 0
            && !self.holding
            && self.slowed_q16 == 0
            && self.pending_q16.is_none()
            && self.step_q16 == FULL_STEP
            && self.rejected.is_none()
    }

    /// The Q16 amount the frame now running has already been marked to exclude.
    const fn pending_deficit(&self) -> u64 {
        if self.holding {
            Q16_PER_TICK
        } else if let Some(p) = self.pending_q16 {
            Q16_PER_TICK - p as u64
        } else {
            0
        }
    }

    /// Gameplay position at `ticks` sim ticks, in Q16, saturating at zero.
    const fn game_q16(&self, ticks: u64) -> u64 {
        let now = ticks as u128 * Q16_PER_TICK as u128;
        let excluded = self.held as u128 * Q16_PER_TICK as u128
            + self.slowed_q16 as u128
            + self.pending_deficit() as u128;
        let q = now.saturating_sub(excluded);
        if q > u64::MAX as u128 {
            u64::MAX
        } else {
            q as u64
        }
    }

    /// `frame` in gameplay frames: the raw count less everything excluded, floored.
    pub const fn game_frame(self, frame: FrameIndex) -> FrameIndex {
        FrameIndex(self.game_q16(frame.as_u64()) / Q16_PER_TICK)
    }

    /// `now` in gameplay time, floored to a whole tick, at the same rate (a hold slows nothing
    /// down — it removes whole ticks).
    pub const fn game_time(self, now: SimTime) -> SimTime {
        SimTime {
            ticks: self.game_q16(now.ticks) / Q16_PER_TICK,
            rate: now.rate,
        }
    }

    /// `now` in fractional gameplay time: [`game_time`](GameClock::game_time) before the floor.
    ///
    /// ```
    /// use ludion_core::time::{GameClock, SimTime, TickRate};
    /// let mut clock = GameClock::default();
    /// clock.scale(0.25);
    /// clock.advance();
    /// let now = SimTime::from_ticks(4, TickRate::DEFAULT);
    /// assert_eq!(clock.game_instant(now).as_q16(), 4 * 65536 - 3 * 65536 / 4);
    /// ```
    pub const fn game_instant(self, now: SimTime) -> GameTime {
        GameTime::from_q16(self.game_q16(now.ticks), now.rate)
    }

    /// Everything excluded from gameplay time as of the frame now running, in Q16: held frames,
    /// scaled frames and the deficit already marked this frame. `game_instant(now).as_q16() +
    /// excluded_q16()` is the sim instant (unless the instant saturated at zero).
    ///
    /// ```
    /// use ludion_core::time::GameClock;
    /// let mut clock = GameClock::default();
    /// clock.hold();
    /// clock.advance();
    /// clock.hold();
    /// assert_eq!(clock.excluded_q16(), 2 * 65536);
    /// ```
    pub const fn excluded_q16(self) -> u64 {
        self.held * Q16_PER_TICK + self.slowed_q16 + self.pending_deficit()
    }

    /// This clock with `frames` more frames excluded (saturating): the rewind a checkpoint restore
    /// applies, so gameplay time after the restore resumes where the slot's clock stood.
    ///
    /// ```
    /// use ludion_core::time::{FrameIndex, GameClock};
    /// let slot = GameClock::default();
    /// // Captured at frame 10, restored at frame 40: 30 frames are excluded.
    /// let live = slot.rebased(30);
    /// assert_eq!(live.game_frame(FrameIndex::new(40)), slot.game_frame(FrameIndex::new(10)));
    /// ```
    pub const fn rebased(self, frames: u64) -> GameClock {
        GameClock {
            held: self.held.saturating_add(frames),
            ..self
        }
    }

    /// The driver's per-tick fold, run once at the start of every tick: a frame marked frozen
    /// becomes one more held frame, a scaled one excludes the rest of its tick, and the marks clear.
    ///
    /// Returns whether gameplay time advanced into this tick — `false` exactly when the previous
    /// frame was held, which is what `SystemCtx::game_advanced` reports to systems.
    pub fn advance(&mut self) -> bool {
        let advanced = if self.holding {
            self.held += 1;
            self.step_q16 = FULL_STEP;
            false
        } else if let Some(p) = self.pending_q16 {
            self.slowed_q16 = self.slowed_q16.saturating_add(Q16_PER_TICK - u64::from(p));
            self.step_q16 = p;
            true
        } else {
            self.step_q16 = FULL_STEP;
            true
        };
        self.holding = false;
        self.pending_q16 = None;
        self.rejected = None;
        advanced
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_rate_deserialize_refuses_zero_and_round_trips() {
        let err = serde_json::from_str::<TickRate>("0")
            .unwrap_err()
            .to_string();
        assert!(err.contains("at least 1 Hz"), "{err}");
        let rate = serde_json::from_str::<TickRate>("60").unwrap();
        assert_eq!(rate, TickRate::DEFAULT);
        assert_eq!(serde_json::to_string(&rate).unwrap(), "60");
    }

    #[test]
    fn default_rate_is_60hz() {
        assert_eq!(TickRate::default().hz(), 60);
    }

    #[test]
    fn frame_index_advances() {
        assert_eq!(FrameIndex::ZERO.next().next().as_u64(), 2);
    }

    #[test]
    fn sim_time_seconds_derived_from_ticks() {
        let t = SimTime::from_ticks(120, TickRate::DEFAULT);
        assert_eq!(t.ticks(), 120);
        assert!((t.seconds() - 2.0).abs() < 1e-12);
    }

    #[test]
    fn serde_round_trips() {
        let t = SimTime::from_ticks(7, TickRate::hz_of(30));
        let json = serde_json::to_string(&t).unwrap();
        let back: SimTime = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn a_default_clock_is_the_identity() {
        let clock = GameClock::default();
        assert!(clock.is_zero());
        assert_eq!(clock.held(), 0);
        assert!(!clock.is_holding());
        assert_eq!(clock.game_frame(FrameIndex::new(900)), FrameIndex::new(900));
        let now = SimTime::from_ticks(900, TickRate::DEFAULT);
        assert_eq!(clock.game_time(now), now);
    }

    #[test]
    fn holding_is_idempotent_within_a_frame() {
        let mut clock = GameClock::default();
        clock.hold();
        clock.hold();
        clock.hold();
        assert_eq!(clock.held(), 0, "a hold excludes nothing until the fold");
        assert!(clock.is_holding());
        assert!(!clock.advance());
        assert_eq!(
            clock.held(),
            1,
            "three holds in one frame exclude one frame"
        );
    }

    #[test]
    fn the_fold_reports_whether_gameplay_time_advanced() {
        let mut clock = GameClock::default();
        assert!(clock.advance(), "an unheld frame advances");
        clock.hold();
        assert!(!clock.advance(), "the frame after a held one does not");
        assert!(clock.advance(), "and the one after that does again");
        assert_eq!(clock.held(), 1);
    }

    /// The normative sequence: a game holding frames 10..=14 sees gameplay frame 10 for the whole
    /// window, advances into it exactly once, and resumes at 11 two frames after the last hold.
    #[test]
    fn a_hold_window_freezes_gameplay_frames_without_skipping_one() {
        let mut clock = GameClock::default();
        let mut seen = Vec::new();
        for frame in 8..=17u64 {
            let advanced = clock.advance();
            seen.push((clock.game_frame(FrameIndex::new(frame)).as_u64(), advanced));
            if (10..=14).contains(&frame) {
                clock.hold();
            }
        }
        assert_eq!(
            seen,
            vec![
                (8, true),
                (9, true),
                (10, true),
                (10, false),
                (10, false),
                (10, false),
                (10, false),
                (10, false),
                (11, true),
                (12, true),
            ]
        );
    }

    #[test]
    fn gameplay_time_runs_at_the_same_rate_behind_the_sim() {
        let clock = GameClock {
            held: 30,
            ..GameClock::default()
        };
        let now = SimTime::from_ticks(150, TickRate::DEFAULT);
        let game = clock.game_time(now);
        assert_eq!(game.ticks(), 120);
        assert_eq!(game.rate(), now.rate());
        assert!((game.seconds() - 2.0).abs() < 1e-12);
    }

    #[test]
    fn gameplay_time_saturates_rather_than_wrapping() {
        let clock = GameClock {
            held: 9,
            holding: true,
            ..GameClock::default()
        };
        assert_eq!(clock.game_frame(FrameIndex::new(3)), FrameIndex::ZERO);
        assert_eq!(
            clock.game_time(SimTime::from_ticks(3, TickRate::DEFAULT)),
            SimTime::from_ticks(0, TickRate::DEFAULT)
        );
    }

    #[test]
    fn game_clock_serde_round_trips_and_defaults_missing_fields() {
        let clock = GameClock {
            held: 7,
            holding: true,
            ..GameClock::default()
        };
        let json = serde_json::to_string(&clock).unwrap();
        assert_eq!(serde_json::from_str::<GameClock>(&json).unwrap(), clock);
        // An old document that predates either field still deserializes.
        assert_eq!(
            serde_json::from_str::<GameClock>("{}").unwrap(),
            GameClock::default()
        );
    }

    const Q: u64 = Q16_PER_TICK;

    #[test]
    fn a_long_slow_run_excludes_exactly_the_quantized_step() {
        let mut clock = GameClock::default();
        for _ in 0..100_000 {
            clock.scale(0.15);
            assert!(clock.advance());
        }
        assert_eq!(clock.step_q16(), 9830);
        assert_eq!(clock.slowed_q16(), 100_000 * (Q - 9830));
        assert_eq!(clock.held(), 0, "a scaled frame is not a held one");
    }

    #[test]
    fn scales_compose_by_min_within_a_frame() {
        let mut clock = GameClock::default();
        clock.scale(0.5);
        clock.scale(0.25);
        clock.scale(0.75);
        assert!(clock.advance());
        assert_eq!(clock.step_q16(), 16384);
        assert!(clock.advance(), "the request does not outlive its frame");
        assert_eq!(clock.step_q16(), Q as u32);
    }

    #[test]
    fn a_hold_wins_over_a_scale() {
        let mut clock = GameClock::default();
        clock.scale(0.5);
        clock.hold();
        assert!(!clock.advance());
        assert_eq!(clock.held(), 1);
        assert_eq!(clock.slowed_q16(), 0);
        assert!(!clock.is_scaling());
    }

    #[test]
    fn an_unscaled_clock_matches_the_held_maths() {
        let mut clock = GameClock::default();
        for frame in 0..40u64 {
            clock.advance();
            let held = clock.held();
            let expect = frame.saturating_sub(held);
            assert_eq!(clock.game_frame(FrameIndex::new(frame)).as_u64(), expect);
            let now = SimTime::from_ticks(frame, TickRate::DEFAULT);
            assert_eq!(clock.game_time(now).ticks(), expect);
            assert_eq!(clock.game_instant(now).as_q16(), expect * Q);
            if (10..14).contains(&frame) {
                clock.hold();
            }
        }
    }

    #[test]
    fn invalid_scales_latch_and_change_nothing() {
        for bad in [f32::NAN, f32::INFINITY, 0.0, -1.0, 1.5] {
            let mut clock = GameClock::default();
            clock.scale(bad);
            assert_eq!(
                clock.take_rejected_scale().map(f32::to_bits),
                Some(bad.to_bits())
            );
            assert!(clock.is_zero(), "{bad} left a mark");
        }
        let mut clock = GameClock::default();
        clock.scale(-1.0);
        clock.scale(f32::NAN);
        assert_eq!(
            clock.take_rejected_scale(),
            Some(-1.0),
            "the first one is kept"
        );
        assert_eq!(clock.take_rejected_scale(), None);
    }

    #[test]
    fn a_tiny_scale_clamps_to_the_minimum() {
        let mut clock = GameClock::default();
        clock.scale(1.0e-9);
        clock.advance();
        assert_eq!(clock.applied_scale(), GameClock::MIN_SCALE);
    }

    #[test]
    fn a_full_scale_is_the_identity() {
        let mut clock = GameClock::default();
        clock.scale(1.0);
        assert!(clock.is_zero());
        assert!(clock.advance());
        assert!(clock.is_zero());
    }

    #[test]
    fn unscaled_clocks_serialize_as_before_and_a_scaled_one_round_trips() {
        assert_eq!(
            serde_json::to_string(&GameClock::default()).unwrap(),
            r#"{"held":0,"holding":false}"#
        );
        let mut held = GameClock::default();
        held.scale(0.5);
        held.advance();
        held.hold();
        held.advance();
        assert_eq!(
            serde_json::to_string(&held).unwrap(),
            r#"{"held":1,"holding":false,"slowed_q16":32768}"#
        );
        let mut scaled = GameClock::default();
        scaled.scale(0.25);
        scaled.advance();
        scaled.scale(0.5);
        let json = serde_json::to_string(&scaled).unwrap();
        assert!(
            json.contains("step_q16") && json.contains("pending_q16"),
            "{json}"
        );
        assert_eq!(serde_json::from_str::<GameClock>(&json).unwrap(), scaled);
    }

    #[test]
    fn rebase_preserves_the_slowed_total() {
        let mut clock = GameClock::default();
        clock.scale(0.5);
        clock.advance();
        let live = clock.rebased(5);
        assert_eq!(live.slowed_q16(), clock.slowed_q16());
        assert_eq!(live.held(), 5);
        assert_eq!(live.step_q16(), clock.step_q16());
    }

    #[test]
    fn game_time_measures_fractional_ticks_since_a_stamp() {
        let t = GameTime::from_q16(5 * Q + Q / 4, TickRate::DEFAULT);
        assert_eq!(t.ticks_since(FrameIndex::new(2)), 3.25);
        assert_eq!(t.ticks_since(FrameIndex::new(6)), -0.75);
        assert_eq!(t.floor().ticks(), 5);
    }

    #[test]
    fn whole_tick_ticks_since_is_bit_identical_to_the_integer_delta() {
        for now in [0u64, 1, 7, 59, 60, 1000, 123_456_789] {
            for started in [0u64, 1, 5, 60, 999, 123_456_790] {
                let t = GameTime::from(SimTime::from_ticks(now, TickRate::DEFAULT));
                let old = (now as i64 - started as i64) as f64;
                assert_eq!(
                    t.ticks_since(FrameIndex::new(started)).to_bits(),
                    old.to_bits(),
                    "{now} - {started}"
                );
            }
        }
    }

    #[test]
    fn the_first_tick_crosses_into_frame_zero() {
        let zero = GameTime::from_q16(0, TickRate::DEFAULT);
        assert_eq!(zero.ticks_crossed(Q as u32), 1, "unscaled");
        assert_eq!(zero.ticks_crossed(Q as u32 / 4), 1, "at 0.25");
        assert_eq!(zero.ticks_crossed(0), 0, "held");
    }

    #[test]
    fn the_clock_schema_lists_only_held_and_holding() {
        let schema = serde_json::to_value(schemars::schema_for!(GameClock)).unwrap();
        let mut keys: Vec<&str> = schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["held", "holding"]);
    }
}
