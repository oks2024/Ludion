//! The [`Rumble`] resource: the gamepad force-feedback outbox a game system fills, the
//! [`CursorLock`](crate::CursorLock) mold with the audio contract.
//!
//! Host state: never hashed, snapshotted, recorded or on the wire. The boundary drain validates and
//! scales each request and sends it to an armed host channel (the window host's pad thread); headless
//! and in the out-of-process viewer the request is a silent no-op. A replay re-issues the same
//! requests by re-running the systems.

use ludion_core::rumble::RumbleCommand;

/// The longest a single rumble request plays, in seconds.
const MAX_SECONDS: f32 = 2.0;

/// One rumble request: two motor strengths and how long they run.
///
/// Out-of-range values are clamped (motors to `0..=1`, `duration` to `0..=2` seconds); a non-finite
/// value counts as 0 and raises `L4279`. A request with a zero duration or both motors at 0 is dropped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RumbleEffect {
    /// The low-frequency (strong, left) motor, `0..=1`.
    pub low: f32,
    /// The high-frequency (weak, right) motor, `0..=1`.
    pub high: f32,
    /// How long it plays, in seconds (at most 2).
    pub duration: f32,
}

/// An ECS resource: the gamepad rumble requests a game system queues, sent at the next frame
/// boundary.
///
/// The engine installs one (empty) into every world it builds. Overlapping effects take the maximum
/// per motor on the pad thread, which also stops them when the window loses focus or the pad
/// disconnects. A plain resource: excluded from the `WorldHash` and from snapshots.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, Rumble, RumbleEffect};
///
/// fn thump(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
///     if let Some(rumble) = world.resource_mut::<Rumble>() {
///         rumble.play(RumbleEffect { low: 0.8, high: 0.3, duration: 0.18 });
///     }
/// }
///
/// let (tx, rx) = crossbeam_channel::unbounded();
/// let mut engine = Engine::builder().system("thump", thump).build().unwrap();
/// engine.set_rumble_sender(tx);
/// engine.advance_live();
/// assert_eq!(rx.try_iter().count(), 1);
/// ```
#[derive(Default, Debug)]
pub struct Rumble {
    commands: Vec<RumbleCommand>,
    non_finite: bool,
}

impl Rumble {
    /// Queues an effect. Later calls in a tick stack; the pad takes the per-motor maximum.
    pub fn play(&mut self, effect: RumbleEffect) {
        let mut bad = false;
        let mut clean = |v: f32| {
            if v.is_finite() {
                v
            } else {
                bad = true;
                0.0
            }
        };
        let low = clean(effect.low).clamp(0.0, 1.0);
        let high = clean(effect.high).clamp(0.0, 1.0);
        let seconds = clean(effect.duration).clamp(0.0, MAX_SECONDS);
        self.non_finite |= bad;
        if seconds > 0.0 && (low > 0.0 || high > 0.0) {
            self.commands
                .push(RumbleCommand::Play { low, high, seconds });
        }
    }

    /// Stops every effect: earlier plays this tick are discarded, later ones still go.
    pub fn stop_all(&mut self) {
        self.commands.clear();
        self.commands.push(RumbleCommand::StopAll);
    }

    /// Whether a request is waiting for the next frame boundary.
    pub fn is_pending(&self) -> bool {
        !self.commands.is_empty() || self.non_finite
    }

    /// Takes the queued commands and whether any value was non-finite (the engine's drain path).
    pub(crate) fn take(&mut self) -> (Vec<RumbleCommand>, bool) {
        (
            std::mem::take(&mut self.commands),
            std::mem::take(&mut self.non_finite),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effect(low: f32, high: f32, duration: f32) -> RumbleEffect {
        RumbleEffect {
            low,
            high,
            duration,
        }
    }

    #[test]
    fn play_clamps_and_flags_non_finite() {
        let mut rumble = Rumble::default();
        rumble.play(effect(1.5, f32::NAN, 5.0));
        let (commands, bad) = rumble.take();
        assert_eq!(
            commands,
            [RumbleCommand::Play {
                low: 1.0,
                high: 0.0,
                seconds: 2.0
            }]
        );
        assert!(bad);
        assert!(!rumble.is_pending());
    }

    #[test]
    fn silent_or_instant_requests_are_dropped() {
        let mut rumble = Rumble::default();
        rumble.play(effect(0.0, 0.0, 1.0));
        rumble.play(effect(1.0, 1.0, 0.0));
        rumble.play(effect(1.0, 1.0, -1.0));
        assert!(!rumble.is_pending());
    }

    #[test]
    fn stop_all_discards_earlier_plays_only() {
        let mut rumble = Rumble::default();
        rumble.play(effect(1.0, 0.0, 1.0));
        rumble.stop_all();
        rumble.play(effect(0.0, 0.5, 1.0));
        let (commands, _) = rumble.take();
        assert_eq!(
            commands,
            [
                RumbleCommand::StopAll,
                RumbleCommand::Play {
                    low: 0.0,
                    high: 0.5,
                    seconds: 1.0
                }
            ]
        );
    }
}
