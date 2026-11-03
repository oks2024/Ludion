//! Run-state and speed: explicit driver state, never tick semantics.
//!
//! [`RunState`] says whether the driver may tick right now (run / pause / step-N); [`Speed`] is a
//! wall-clock pacing factor for a *live* scheduler. Neither ever changes what a tick computes:
//! `dt` is a constant of the simulation, a step of N runs exactly N ticks, and headless runs
//! ignore speed entirely and tick as fast as possible. This split is what keeps run control and
//! determinism from ever touching.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Whether the driver may perform ticks.
///
/// This is plain, serializable state on [`SimDriver`](crate::SimDriver) — the protocol's
/// `sim.pause` / `sim.resume` / `sim.step` commands are setters for it. A paused driver still
/// exists and still answers queries (the command queue keeps draining upstream); it just performs
/// no ticks.
///
/// # Example
/// ```
/// use ludion_sim::RunState;
/// let s = RunState::Stepping { remaining: 3 };
/// let json = serde_json::to_value(&s).unwrap();
/// assert_eq!(json, serde_json::json!({"state": "stepping", "remaining": 3}));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RunState {
    /// Tick freely (a live scheduler paces ticks by wall clock × [`Speed`]; headless callers tick
    /// as fast as possible).
    Running,
    /// Perform no ticks until resumed or stepped.
    Paused,
    /// Perform exactly `remaining` more ticks, then become [`Paused`](RunState::Paused).
    Stepping {
        /// Ticks left before the driver pauses.
        remaining: u64,
    },
}

/// A wall-clock pacing factor for the live scheduler: `1.0` is real time, `2.0` double speed.
///
/// Speed affects **when** a live scheduler performs ticks and nothing else — `dt` never changes,
/// tick counts never change, and headless runs ignore it. It is validated at construction: the
/// factor must be a finite number in `[1/64, 64]` (`L4001` otherwise), including when it arrives
/// via deserialization.
///
/// # Example
/// ```
/// use ludion_sim::Speed;
///
/// let s = Speed::new(2.0).unwrap();
/// assert_eq!(s.factor(), 2.0);
/// assert_eq!(Speed::default(), Speed::NORMAL);
///
/// // Invalid factors are structured errors, not scheduler surprises.
/// let err = Speed::new(0.0).unwrap_err();
/// assert_eq!(err.code.as_str(), "L4001");
/// ```
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub struct Speed(f64);

impl Speed {
    /// Real time: factor `1.0`.
    pub const NORMAL: Speed = Speed(1.0);

    /// The slowest accepted factor. The live loop sleeps on `tick period / factor`, so an unbounded
    /// small factor overflows the sleep duration.
    pub const MIN: f64 = 1.0 / 64.0;

    /// The fastest accepted factor; a larger one only spins the loop, which cannot tick faster than
    /// the host can.
    pub const MAX: f64 = 64.0;

    /// Validates and wraps a pacing factor.
    ///
    /// # Errors
    /// Returns an `L4001` diagnostic if `factor` is not a finite number in
    /// [`MIN`](Speed::MIN)`..=`[`MAX`](Speed::MAX) (rejects `0`, negatives, `NaN`, infinities, and
    /// anything outside `[1/64, 64]`).
    pub fn new(factor: f64) -> Result<Self, Diagnostic> {
        if factor.is_finite() && (Self::MIN..=Self::MAX).contains(&factor) {
            Ok(Speed(factor))
        } else {
            Err(Diagnostic::new(
                code::INVALID_SIM_SPEED,
                format!("simulation speed must be between 1/64 and 64, got {factor}"),
            )
            .with_help(
                "use 1.0 for real time, 2.0 for double speed, 0.5 for half speed; to stop ticking \
                 use sim.pause instead of a zero speed",
            ))
        }
    }

    /// The pacing factor.
    pub const fn factor(self) -> f64 {
        self.0
    }
}

impl Default for Speed {
    fn default() -> Self {
        Speed::NORMAL
    }
}

impl TryFrom<f64> for Speed {
    type Error = Diagnostic;

    fn try_from(factor: f64) -> Result<Self, Self::Error> {
        Speed::new(factor)
    }
}

impl From<Speed> for f64 {
    fn from(speed: Speed) -> Self {
        speed.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_speeds_construct() {
        assert_eq!(Speed::new(0.25).unwrap().factor(), 0.25);
        assert_eq!(Speed::new(10.0).unwrap().factor(), 10.0);
        assert_eq!(Speed::NORMAL.factor(), 1.0);
        assert_eq!(Speed::new(Speed::MIN).unwrap().factor(), Speed::MIN);
        assert_eq!(Speed::new(Speed::MAX).unwrap().factor(), Speed::MAX);
        assert_eq!(Speed::new(1.0).unwrap(), Speed::NORMAL);
    }

    #[test]
    fn invalid_speeds_are_l4001() {
        for bad in [
            0.0,
            -1.0,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            1e-30,
            1.0 / 65.0,
            65.0,
            1e300,
        ] {
            let err = Speed::new(bad).unwrap_err();
            assert_eq!(err.code, code::INVALID_SIM_SPEED, "factor {bad}");
            assert!(err.help.is_some());
        }
    }

    #[test]
    fn serde_validates_on_deserialize() {
        let ok: Speed = serde_json::from_str("1.5").unwrap();
        assert_eq!(ok.factor(), 1.5);
        assert_eq!(serde_json::to_string(&ok).unwrap(), "1.5");

        let err = serde_json::from_str::<Speed>("0.0").unwrap_err();
        assert!(err.to_string().contains("L4001"), "{err}");

        let err = serde_json::from_str::<Speed>("100.0").unwrap_err();
        assert!(err.to_string().contains("L4001"), "{err}");
    }

    #[test]
    fn run_state_serde_round_trips() {
        for state in [
            RunState::Running,
            RunState::Paused,
            RunState::Stepping { remaining: 7 },
        ] {
            let json = serde_json::to_string(&state).unwrap();
            let back: RunState = serde_json::from_str(&json).unwrap();
            assert_eq!(state, back);
        }
    }
}
