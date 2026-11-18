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
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GameClock {
    /// Frames already folded out of gameplay time.
    #[serde(default)]
    held: u64,
    /// Whether the frame now running was marked frozen.
    #[serde(default)]
    holding: bool,
}

impl GameClock {
    /// Marks the frame now running as frozen: gameplay time does not enter the next tick.
    ///
    /// Idempotent within a frame — a game may call it from several systems — so the cost of an
    /// extra call is nothing.
    pub fn hold(&mut self) {
        self.holding = true;
    }

    /// How many frames are already excluded from gameplay time.
    pub const fn held(self) -> u64 {
        self.held
    }

    /// Whether the frame now running was marked frozen.
    pub const fn is_holding(self) -> bool {
        self.holding
    }

    /// Whether this clock has excluded nothing at all — the default, and what lets a snapshot omit
    /// the field entirely.
    pub const fn is_zero(&self) -> bool {
        self.held == 0 && !self.holding
    }

    /// `frame` in gameplay frames: the raw count less everything held, saturating at zero.
    pub const fn game_frame(self, frame: FrameIndex) -> FrameIndex {
        FrameIndex(
            frame
                .as_u64()
                .saturating_sub(self.held)
                .saturating_sub(self.holding as u64),
        )
    }

    /// `now` in gameplay time: the same subtraction as [`game_frame`](GameClock::game_frame), at
    /// the same rate (a hold slows nothing down — it removes whole ticks).
    pub const fn game_time(self, now: SimTime) -> SimTime {
        SimTime {
            ticks: now
                .ticks
                .saturating_sub(self.held)
                .saturating_sub(self.holding as u64),
            rate: now.rate,
        }
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
            holding: self.holding,
        }
    }

    /// The driver's per-tick fold, run once at the start of every tick: a frame marked frozen
    /// becomes one more held frame and the mark clears.
    ///
    /// Returns whether gameplay time advanced into this tick — `false` exactly when the previous
    /// frame was held, which is what `SystemCtx::game_advanced` reports to systems.
    pub fn advance(&mut self) -> bool {
        if self.holding {
            self.held += 1;
            self.holding = false;
            false
        } else {
            true
        }
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
            holding: false,
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
        };
        let json = serde_json::to_string(&clock).unwrap();
        assert_eq!(serde_json::from_str::<GameClock>(&json).unwrap(), clock);
        // An old document that predates either field still deserializes.
        assert_eq!(
            serde_json::from_str::<GameClock>("{}").unwrap(),
            GameClock::default()
        );
    }
}
