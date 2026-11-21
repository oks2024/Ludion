//! Rumble on Apple pads: the pad thread's motor mix drives GameController haptics.
//!
//! `low` plays on the left grip (`GCHapticsLocalityLeftHandle`), `high` on the right: Xbox, DualShock
//! and DualSense put the heavy low-frequency motor left and the light high-frequency one right, the
//! same convention as XInput and gilrs's Strong/Weak. A pad that lacks either handle gets one
//! `Default`-locality engine playing the louder of the two.
//!
//! Each play builds one continuous haptic event of [`PATTERN_SECONDS`] with the motor value as its
//! intensity. Core Haptics caps a continuous event at 30 s, so a mix that stays constant is replayed
//! once its pattern is [`REFRESH_AFTER`] old. Every `Retained` here is `!Send`: the struct is built and
//! used on the pad thread only, and each call runs in its own autorelease pool because a background
//! thread has none. Any failure releases the pad and declines, so the caller falls back to gilrs and
//! the next mix change retries.
// The objc2 FFI is unsafe; the rest of the crate stays `deny(unsafe_code)`.
#![allow(unsafe_code)]

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, msg_send};
use objc2_core_haptics::{
    CHHapticEngine, CHHapticEvent, CHHapticEventParameter, CHHapticEventParameterIDHapticIntensity,
    CHHapticEventTypeHapticContinuous, CHHapticPattern, CHHapticPatternPlayer,
};
use objc2_foundation::NSArray;
use objc2_game_controller::{
    GCController, GCDeviceHaptics, GCHapticsLocalityDefault, GCHapticsLocalityLeftHandle,
    GCHapticsLocalityRightHandle,
};
use std::time::Duration;

/// Core Haptics' longest continuous event.
const PATTERN_SECONDS: f64 = 30.0;
/// A running pattern is replayed at this age, comfortably before it ends.
const REFRESH_AFTER: Duration = Duration::from_secs(25);

/// What to play on each engine the pad offers.
#[derive(Debug, PartialEq)]
pub(crate) enum Targets {
    /// One engine per grip.
    Handles { left: f32, right: f32 },
    /// One `Default`-locality engine.
    Single(f32),
}

/// Maps the mixed `(low, high)` onto the pad's engines.
pub(crate) fn targets(has_handles: bool, (low, high): (f32, f32)) -> Targets {
    if has_handles {
        Targets::Handles {
            left: low,
            right: high,
        }
    } else {
        Targets::Single(low.max(high))
    }
}

/// Whether a pattern started at `started` is old enough to replay at `now`.
pub(crate) fn refresh_due(started: Duration, now: Duration) -> bool {
    now.saturating_sub(started) >= REFRESH_AFTER
}

type Player = Retained<ProtocolObject<dyn CHHapticPatternPlayer>>;

#[derive(Clone, Copy)]
enum Side {
    Left,
    Right,
    Default,
}

struct Lane {
    engine: Retained<CHHapticEngine>,
    player: Option<Player>,
}

struct Pad {
    // Keeps the controller (and so its haptics) alive for the engines.
    _controller: Retained<GCController>,
    lanes: Vec<Lane>,
}

/// The Apple haptics backend for the one primary pad.
#[derive(Default)]
pub(crate) struct AppleRumble {
    pad: Option<Pad>,
    started: Duration,
    playing: (f32, f32),
}

impl AppleRumble {
    /// Plays `motors`; `false` when no controller or haptics engine can, so the caller falls back.
    pub(crate) fn play(&mut self, motors: (f32, f32), now: Duration) -> bool {
        autoreleasepool(|_| {
            if self.pad.is_none() {
                self.pad = Pad::acquire();
            }
            let Some(pad) = self.pad.as_mut() else {
                return false;
            };
            if pad.play(motors) {
                self.started = now;
                self.playing = motors;
                true
            } else {
                self.release_inner();
                false
            }
        })
    }

    /// Replays the running mix once its pattern nears the 30 s cap.
    pub(crate) fn refresh(&mut self, now: Duration) {
        if self.pad.is_some() && self.playing != (0.0, 0.0) && refresh_due(self.started, now) {
            self.play(self.playing, now);
        }
    }

    /// Silences the pad but keeps its engines for the next mix.
    pub(crate) fn stop(&mut self) {
        autoreleasepool(|_| {
            if let Some(pad) = self.pad.as_mut() {
                pad.stop();
            }
            self.playing = (0.0, 0.0);
        });
    }

    /// Silences the pad and drops the controller and engines.
    pub(crate) fn release(&mut self) {
        autoreleasepool(|_| self.release_inner());
    }

    fn release_inner(&mut self) {
        if let Some(mut pad) = self.pad.take() {
            pad.stop();
            for lane in &pad.lanes {
                // SAFETY: a null completion handler is allowed.
                unsafe { lane.engine.stopWithCompletionHandler(std::ptr::null_mut()) };
            }
        }
        self.playing = (0.0, 0.0);
    }
}

impl Pad {
    /// The first controller with haptics, with a started engine per grip (or one default engine).
    fn acquire() -> Option<Pad> {
        // SAFETY: plain class/instance queries; `controllers` is only a snapshot of the connected set.
        let controllers = unsafe { GCController::controllers() };
        controllers.iter().find_map(|controller| {
            // SAFETY: as above.
            let haptics = unsafe { controller.haptics() }?;
            let sides: &[Side] = if has_handles(&haptics) {
                &[Side::Left, Side::Right]
            } else {
                &[Side::Default]
            };
            let lanes = sides
                .iter()
                .map(|&side| Lane::new(&haptics, side))
                .collect::<Option<Vec<_>>>()?;
            Some(Pad {
                _controller: controller,
                lanes,
            })
        })
    }

    fn play(&mut self, motors: (f32, f32)) -> bool {
        self.stop();
        let values = match targets(self.lanes.len() == 2, motors) {
            Targets::Handles { left, right } => [left, right],
            Targets::Single(value) => [value, 0.0],
        };
        self.lanes
            .iter_mut()
            .zip(values)
            .all(|(lane, value)| value <= 0.0 || lane.play(value))
    }

    fn stop(&mut self) {
        for lane in &mut self.lanes {
            if let Some(player) = lane.player.take() {
                // SAFETY: the player is live; a failed stop leaves nothing playing that we track.
                let _ = unsafe { player.stopAtTime_error(0.0) };
            }
        }
    }
}

fn has_handles(haptics: &GCDeviceHaptics) -> bool {
    // SAFETY: a plain getter; the locality constants are process-lifetime statics.
    unsafe {
        let supported = haptics.supportedLocalities();
        supported.containsObject(GCHapticsLocalityLeftHandle)
            && supported.containsObject(GCHapticsLocalityRightHandle)
    }
}

impl Lane {
    fn new(haptics: &GCDeviceHaptics, side: Side) -> Option<Lane> {
        // SAFETY: the locality constants are process-lifetime statics.
        let locality = unsafe {
            match side {
                Side::Left => GCHapticsLocalityLeftHandle,
                Side::Right => GCHapticsLocalityRightHandle,
                Side::Default => GCHapticsLocalityDefault,
            }
        };
        // objc2-game-controller 0.3.2 gates `createEngineWithLocality:` to iOS/tvOS/visionOS although
        // Apple ships it on macOS 11+, so it is sent by hand.
        // SAFETY: the selector takes a locality `NSString` and returns a nullable `CHHapticEngine`.
        let engine: Option<Retained<CHHapticEngine>> =
            unsafe { msg_send![haptics, createEngineWithLocality: locality] };
        let engine = engine?;
        // SAFETY: live engine; the motors only need haptics.
        unsafe {
            engine.setPlaysHapticsOnly(true);
            engine.startAndReturnError().ok()?;
        }
        Some(Lane {
            engine,
            player: None,
        })
    }

    /// Starts a continuous pattern at `intensity` (0..=1) and keeps its player.
    fn play(&mut self, intensity: f32) -> bool {
        // SAFETY: freshly allocated objects initialised by their designated initialisers; the engine
        // and pattern are live for the call.
        let player = unsafe {
            let parameter = CHHapticEventParameter::initWithParameterID_value(
                CHHapticEventParameter::alloc(),
                CHHapticEventParameterIDHapticIntensity,
                intensity.clamp(0.0, 1.0),
            );
            let event = CHHapticEvent::initWithEventType_parameters_relativeTime_duration(
                CHHapticEvent::alloc(),
                CHHapticEventTypeHapticContinuous,
                &NSArray::from_retained_slice(&[parameter]),
                0.0,
                PATTERN_SECONDS,
            );
            let pattern = CHHapticPattern::initWithEvents_parameters_error(
                CHHapticPattern::alloc(),
                &NSArray::from_retained_slice(&[event]),
                &NSArray::new(),
            );
            let Ok(pattern) = pattern else { return false };
            let Ok(player) = self.engine.createPlayerWithPattern_error(&pattern) else {
                return false;
            };
            if player.startAtTime_error(0.0).is_err() {
                return false;
            }
            player
        };
        self.player = Some(player);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_pass_the_motors_through() {
        assert_eq!(
            targets(true, (0.8, 0.2)),
            Targets::Handles {
                left: 0.8,
                right: 0.2
            }
        );
        assert_eq!(
            targets(true, (0.0, 0.0)),
            Targets::Handles {
                left: 0.0,
                right: 0.0
            }
        );
    }

    #[test]
    fn a_single_engine_takes_the_louder_motor() {
        assert_eq!(targets(false, (0.3, 0.7)), Targets::Single(0.7));
        assert_eq!(targets(false, (0.9, 0.1)), Targets::Single(0.9));
        assert_eq!(targets(false, (0.0, 0.0)), Targets::Single(0.0));
    }

    #[test]
    fn a_pattern_is_refreshed_at_twenty_five_seconds() {
        let start = Duration::from_secs(3);
        assert!(!refresh_due(start, start + Duration::from_millis(24_999)));
        assert!(refresh_due(start, start + Duration::from_secs(25)));
    }

    #[test]
    fn with_no_controller_play_declines_and_release_is_a_no_op() {
        let mut backend = AppleRumble::default();
        assert!(!backend.play((1.0, 1.0), Duration::ZERO));
        backend.stop();
        backend.release();
        assert!(!backend.play((0.5, 0.0), Duration::from_secs(1)));
    }
}
