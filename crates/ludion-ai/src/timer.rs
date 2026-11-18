//! [`Timer`] and [`Cooldown`]: deadlines in gameplay ticks.
//!
//! Both store an absolute gameplay-tick deadline and read `ctx.game_time`, so they freeze while the
//! game holds its clock and survive a snapshot restore (the `GameClock` rides snapshots).

use ludion_ecs::SystemCtx;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

fn now(ctx: &SystemCtx) -> u64 {
    ctx.game_time.ticks()
}

/// Seconds to whole ticks at `hz`, rounding up.
///
/// An `f32` such as `0.3` times 60 is `18.0000007`; the tiny relative slack absorbs that
/// representation error before the round-up, so a timer authored in tenths lands on the tick a human
/// expects.
fn seconds_to_ticks(seconds: f32, hz: u32) -> u64 {
    let x = f64::from(seconds.max(0.0)) * f64::from(hz);
    (x - x * 1e-6).ceil() as u64
}

fn ticks_of(ctx: &SystemCtx, seconds: f32) -> u64 {
    seconds_to_ticks(seconds, ctx.game_time.rate().hz())
}

/// A one-shot countdown in gameplay time. Absent deadline = stopped.
///
/// # Example
///
/// ```
/// use ludion_ai::Timer;
/// use ludion_core::time::{FrameIndex, TickRate};
/// use ludion_ecs::SystemCtx;
///
/// let at = |frame| SystemCtx::new(FrameIndex::new(frame), TickRate::DEFAULT);
/// let mut timer = Timer::default();
/// timer.start(&at(10), 0.5); // 30 ticks at 60 Hz
/// assert!(timer.running() && !timer.expired(&at(39)));
/// assert!(timer.just_expired(&at(40)) && timer.expired(&at(41)));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Timer {
    /// The gameplay tick the timer expires on; `None` while stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<u64>,
}

impl Timer {
    /// Starts (or restarts) the timer to expire `seconds` from now, rounded up to a whole tick.
    pub fn start(&mut self, ctx: &SystemCtx, seconds: f32) {
        self.start_ticks(ctx, ticks_of(ctx, seconds));
    }

    /// Starts the timer to expire `ticks` gameplay ticks from now.
    pub fn start_ticks(&mut self, ctx: &SystemCtx, ticks: u64) {
        self.deadline = Some(now(ctx).saturating_add(ticks));
    }

    /// Stops the timer: it neither runs nor reads as expired.
    pub fn stop(&mut self) {
        self.deadline = None;
    }

    /// Whether the timer was started and has not been stopped (an expired timer still runs).
    pub fn running(&self) -> bool {
        self.deadline.is_some()
    }

    /// Whether the deadline has been reached. `false` for a stopped timer.
    pub fn expired(&self, ctx: &SystemCtx) -> bool {
        self.deadline.is_some_and(|d| now(ctx) >= d)
    }

    /// Whether the deadline is exactly this gameplay tick, on the tick that crossed into it: true
    /// once, never on a held tick and never twice under a gameplay-time scale.
    pub fn just_expired(&self, ctx: &SystemCtx) -> bool {
        ctx.game_frames_crossed() > 0 && self.deadline == Some(now(ctx))
    }

    /// Gameplay ticks left; `0` once expired or when stopped.
    pub fn remaining_ticks(&self, ctx: &SystemCtx) -> u64 {
        self.deadline.map_or(0, |d| d.saturating_sub(now(ctx)))
    }

    /// Seconds left; `0.0` once expired or when stopped.
    pub fn remaining(&self, ctx: &SystemCtx) -> f32 {
        self.remaining_ticks(ctx) as f32 / ctx.game_time.rate().hz() as f32
    }
}

/// A reusable gate in gameplay time: `ready` once the last [`trigger`](Cooldown::trigger) has
/// run its course. A default cooldown is ready.
///
/// # Example
///
/// ```
/// use ludion_ai::Cooldown;
/// use ludion_core::time::{FrameIndex, TickRate};
/// use ludion_ecs::SystemCtx;
///
/// let at = |frame| SystemCtx::new(FrameIndex::new(frame), TickRate::DEFAULT);
/// let mut shot = Cooldown::default();
/// assert!(shot.ready(&at(0)));
/// shot.trigger(&at(0), 2.0);
/// assert!(!shot.ready(&at(119)) && shot.ready(&at(120)));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Cooldown {
    /// The gameplay tick the cooldown is ready from.
    #[serde(default)]
    pub ready_at: u64,
}

impl Cooldown {
    /// Whether the gate is open.
    pub fn ready(&self, ctx: &SystemCtx) -> bool {
        now(ctx) >= self.ready_at
    }

    /// Closes the gate for `seconds`, rounded up to a whole tick.
    pub fn trigger(&mut self, ctx: &SystemCtx, seconds: f32) {
        self.trigger_ticks(ctx, ticks_of(ctx, seconds));
    }

    /// Closes the gate for `ticks` gameplay ticks.
    pub fn trigger_ticks(&mut self, ctx: &SystemCtx, ticks: u64) {
        self.ready_at = now(ctx).saturating_add(ticks);
    }
}
