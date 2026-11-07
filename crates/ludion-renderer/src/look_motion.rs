//! The raw mouse total a window host shares with the presenter, for the late latch.

use std::sync::{Arc, Mutex};
use std::time::Instant;

/// What the host has seen of the mouse.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LookCounts {
    /// Every raw count since the host began, summed in `f64` exactly as the sim's cut tags are.
    pub total: [f64; 2],
    /// Whether the window's pointer is locked: the latch applies only then.
    pub locked: bool,
    /// Arrival of the first count stored since the last [`LookMotion::take_oldest`].
    pub oldest: Option<Instant>,
}

/// Shared between the window host (writer) and the presenter (reader); host-side only, never
/// serialized.
#[derive(Debug, Default)]
pub struct LookMotion(Mutex<LookCounts>);

impl LookMotion {
    /// A fresh handle with no motion and an unlocked pointer.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, LookCounts> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Stores the host's new cumulative `total`; `at` is the arrival of the count that moved it.
    pub fn store(&self, total: [f64; 2], at: Option<Instant>) {
        let mut counts = self.lock();
        counts.total = total;
        if counts.oldest.is_none() {
            counts.oldest = at;
        }
    }

    /// Records whether the pointer is locked.
    pub fn set_locked(&self, locked: bool) {
        self.lock().locked = locked;
    }

    /// The current counts.
    pub fn load(&self) -> LookCounts {
        *self.lock()
    }

    /// The oldest arrival not yet taken, clearing it.
    pub fn take_oldest(&self) -> Option<Instant> {
        self.lock().oldest.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn store_keeps_the_oldest_arrival_until_taken() {
        let look = LookMotion::new();
        let (t0, t1) = (Instant::now(), Instant::now());
        look.store([1.0, 0.0], Some(t0));
        look.store([3.0, 0.0], Some(t1));
        assert_eq!(look.load().total, [3.0, 0.0]);
        assert_eq!(look.take_oldest(), Some(t0));
        assert_eq!(look.take_oldest(), None);
        look.store([4.0, 0.0], Some(t1));
        assert_eq!(look.take_oldest(), Some(t1));
    }
}
