//! What a sequence tells the game each tick.

use super::SequenceHandle;

/// Which track a key belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SequenceTrack {
    /// A camera key.
    Camera,
    /// A transform key.
    Transform,
    /// An animation key.
    Animation,
    /// A ui key.
    Ui,
    /// An audio key.
    Audio,
    /// A dialogue key.
    Dialogue,
    /// An event key.
    Event,
}

/// How a sequence ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SequenceEnd {
    /// The playhead reached `length_ticks`.
    Completed,
    /// [`skip`](super::skip) cut it to its end state.
    Skipped,
    /// [`cancel`](super::cancel) (or a vanished document) stopped it where it was.
    Cancelled,
}

/// One thing that happened to a sequence this tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SequenceEvent {
    /// A key applied (a skip applies its end forms the same way).
    KeyApplied {
        /// The sequence.
        handle: SequenceHandle,
        /// The key's track.
        track: SequenceTrack,
        /// The key's `at`.
        at: u32,
    },
    /// An `event` key fired.
    Fired {
        /// The sequence.
        handle: SequenceHandle,
        /// The event name.
        name: String,
    },
    /// The sequence ended; its handle is dead from here on.
    Ended {
        /// The sequence.
        handle: SequenceHandle,
        /// How.
        end: SequenceEnd,
    },
}

/// Every [`SequenceEvent`] the last [`sequence_update`](super::sequence_update) produced, in the
/// order they happened. Rewritten on every run, so a consumer reads it the same tick, after that
/// system.
///
/// ```
/// use ludion_engine::SequenceEvents;
///
/// let events = SequenceEvents::default();
/// assert_eq!(events.iter().count(), 0);
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SequenceEvents {
    pub(crate) events: Vec<SequenceEvent>,
}

impl SequenceEvents {
    /// The events of this tick, in order.
    pub fn iter(&self) -> impl Iterator<Item = &SequenceEvent> {
        self.events.iter()
    }

    /// Whether `handle`'s sequence fired the event `name` this tick.
    pub fn fired(&self, handle: SequenceHandle, name: &str) -> bool {
        self.events.iter().any(|event| {
            matches!(event, SequenceEvent::Fired { handle: h, name: n } if *h == handle && n == name)
        })
    }

    /// How `handle`'s sequence ended this tick, if it did.
    pub fn ended(&self, handle: SequenceHandle) -> Option<SequenceEnd> {
        self.events.iter().find_map(|event| match event {
            SequenceEvent::Ended { handle: h, end } if *h == handle => Some(*end),
            _ => None,
        })
    }
}
