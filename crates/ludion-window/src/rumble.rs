//! The pad thread's rumble: the pure mixer and the output side that plays its result.
//!
//! `RumbleMix` collapses overlapping requests to one pair of motor strengths; it is pure bookkeeping,
//! so the mixing rules are unit-tested without a pad. `PadRumble` plays that pair on the pad through
//! gilrs force feedback, or through the Apple backend first when the `apple-rumble` feature is on.

use gilrs::{GamepadId, Gilrs};
use ludion_core::rumble::RumbleCommand;
use std::time::Duration;

/// What the pad thread has playing on the pad. Dropping a gilrs effect stops it.
#[derive(Default)]
pub(crate) struct PadRumble {
    effect: Option<gilrs::ff::Effect>,
    #[cfg(all(feature = "apple-rumble", target_vendor = "apple"))]
    apple: crate::apple_rumble::AppleRumble,
}

impl PadRumble {
    /// Replaces the current output with `motors` on `pad`; silence when there is no pad or no motor.
    pub(crate) fn set(
        &mut self,
        gilrs: &mut Gilrs,
        pad: Option<GamepadId>,
        motors: (f32, f32),
        now: Duration,
    ) {
        self.effect = None;
        #[cfg(all(feature = "apple-rumble", target_vendor = "apple"))]
        self.apple.stop();
        if let (Some(id), true) = (pad, motors != (0.0, 0.0)) {
            #[cfg(all(feature = "apple-rumble", target_vendor = "apple"))]
            if self.apple.play(motors, now) {
                return;
            }
            self.effect = play_rumble(gilrs, id, motors);
        }
        #[cfg(not(all(feature = "apple-rumble", target_vendor = "apple")))]
        let _ = now;
    }

    /// Keeps long-running output alive between mix changes.
    pub(crate) fn refresh(&mut self, now: Duration) {
        #[cfg(all(feature = "apple-rumble", target_vendor = "apple"))]
        self.apple.refresh(now);
        #[cfg(not(all(feature = "apple-rumble", target_vendor = "apple")))]
        let _ = now;
    }

    /// Drops all output (the primary pad disconnected).
    pub(crate) fn release(&mut self) {
        self.effect = None;
        #[cfg(all(feature = "apple-rumble", target_vendor = "apple"))]
        self.apple.release();
    }
}

/// Starts a force-feedback effect on `pad` with the mixed motor strengths (`low` drives the strong
/// motor, `high` the weak one). The mix owns every stop (it drops the effect on a change, a clear or a
/// disconnect), so the effect is built to play far longer than any mix lifetime: a constant mix that
/// outlasts one request must not die with the backend. A pad without force feedback or a backend error
/// is silent: this is host feel, not sim.
fn play_rumble(
    gilrs: &mut Gilrs,
    pad: GamepadId,
    (low, high): (f32, f32),
) -> Option<gilrs::ff::Effect> {
    use gilrs::ff::{BaseEffect, BaseEffectType, EffectBuilder, Replay, Ticks};
    if !gilrs.gamepad(pad).is_ff_supported() {
        return None;
    }
    let magnitude = |v: f32| (v * f32::from(u16::MAX)) as u16;
    let replay = Replay {
        play_for: Ticks::from_ms(u32::MAX),
        ..Default::default()
    };
    let effect = EffectBuilder::new()
        .add_effect(BaseEffect {
            kind: BaseEffectType::Strong {
                magnitude: magnitude(low),
            },
            scheduling: replay,
            ..Default::default()
        })
        .add_effect(BaseEffect {
            kind: BaseEffectType::Weak {
                magnitude: magnitude(high),
            },
            scheduling: replay,
            ..Default::default()
        })
        .gamepads(&[pad])
        .finish(gilrs)
        .ok()?;
    effect.play().ok()?;
    Some(effect)
}

/// The longest any one request plays, whatever the sender asked for.
const MAX_PLAY: Duration = Duration::from_secs(2);

struct Playing {
    end: Duration,
    low: f32,
    high: f32,
}

/// The effects currently playing, keyed by the pad thread's own clock.
#[derive(Default)]
pub(crate) struct RumbleMix {
    playing: Vec<Playing>,
}

impl RumbleMix {
    /// Folds a command in at time `now`: a `Play` joins the mix, `StopAll` empties it.
    pub(crate) fn apply(&mut self, now: Duration, cmd: RumbleCommand) {
        match cmd {
            RumbleCommand::Play { low, high, seconds } => {
                let length = Duration::try_from_secs_f32(seconds)
                    .unwrap_or(Duration::ZERO)
                    .min(MAX_PLAY);
                self.playing.push(Playing {
                    end: now + length,
                    low: low.clamp(0.0, 1.0),
                    high: high.clamp(0.0, 1.0),
                });
            }
            RumbleCommand::StopAll => self.clear(),
        }
    }

    /// Prunes the effects that ended by `now` and returns the per-motor maximum `(low, high)`.
    pub(crate) fn motors(&mut self, now: Duration) -> (f32, f32) {
        self.playing.retain(|effect| effect.end > now);
        self.playing.iter().fold((0.0, 0.0), |(low, high), effect| {
            (low.max(effect.low), high.max(effect.high))
        })
    }

    /// Drops every effect (a stop, a focus loss, a disconnect).
    pub(crate) fn clear(&mut self) {
        self.playing.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(low: f32, high: f32, seconds: f32) -> RumbleCommand {
        RumbleCommand::Play { low, high, seconds }
    }

    fn at(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn overlap_takes_the_maximum_per_motor() {
        let mut mix = RumbleMix::default();
        mix.apply(at(0), play(0.8, 0.1, 1.0));
        mix.apply(at(0), play(0.2, 0.6, 1.0));
        assert_eq!(mix.motors(at(10)), (0.8, 0.6));
    }

    #[test]
    fn an_effect_expires_at_its_end() {
        let mut mix = RumbleMix::default();
        mix.apply(at(0), play(1.0, 0.0, 0.25));
        mix.apply(at(0), play(0.0, 0.5, 0.5));
        assert_eq!(mix.motors(at(249)), (1.0, 0.5));
        assert_eq!(mix.motors(at(250)), (0.0, 0.5));
        assert_eq!(mix.motors(at(500)), (0.0, 0.0));
    }

    #[test]
    fn stop_all_clears_and_a_fresh_play_still_plays() {
        let mut mix = RumbleMix::default();
        mix.apply(at(0), play(1.0, 1.0, 1.0));
        mix.apply(at(5), RumbleCommand::StopAll);
        assert_eq!(mix.motors(at(6)), (0.0, 0.0));
        mix.apply(at(7), play(0.4, 0.0, 1.0));
        assert_eq!(mix.motors(at(8)), (0.4, 0.0));
    }

    #[test]
    fn a_request_plays_for_at_most_two_seconds() {
        let mut mix = RumbleMix::default();
        mix.apply(at(0), play(1.0, 1.0, 60.0));
        assert_eq!(mix.motors(at(1_999)), (1.0, 1.0));
        assert_eq!(mix.motors(at(2_000)), (0.0, 0.0));
    }
}
