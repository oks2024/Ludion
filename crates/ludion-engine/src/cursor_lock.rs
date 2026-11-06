//! The [`CursorLock`] resource: the pointer-lock switch a game system asks the window host for, the
//! [`RenderSettings`](crate::RenderSettings) mold.
//!
//! Host state: never hashed, snapshotted, recorded or on the wire. Write-only, like `RenderSettings`.
//! The boundary drain stores the request on the [`Engine`](crate::Engine) and forwards each change to
//! an armed host channel; headless, the request is a silent no-op.

/// An ECS resource: a pointer lock or release a game system requests, honored at the next frame
/// boundary.
///
/// The engine installs one (empty) into every world it builds. The last call in a tick wins. The
/// request lives on the [`Engine`](crate::Engine): it survives world rebuilds and an
/// `engine.restore` does not roll it back, but a `ludion dev` restart forgets it, so a game
/// re-requests from its own state. A plain resource: excluded from the `WorldHash` and from snapshots.
///
/// # Example
/// ```
/// use ludion_engine::{CursorLock, Engine};
///
/// fn capture(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
///     if let Some(lock) = world.resource_mut::<CursorLock>() {
///         lock.lock();
///     }
/// }
///
/// let mut engine = Engine::builder().system("capture", capture).build().unwrap();
/// assert_eq!(engine.cursor_lock_request(), None);
/// engine.advance_live();
/// assert_eq!(engine.cursor_lock_request(), Some(true));
/// ```
#[derive(Default, Debug)]
pub struct CursorLock {
    request: Option<bool>,
}

impl CursorLock {
    /// Asks the window host to lock the pointer. The last call in a tick wins.
    pub fn lock(&mut self) {
        self.request = Some(true);
    }

    /// Asks the window host to release the pointer. The last call in a tick wins.
    pub fn release(&mut self) {
        self.request = Some(false);
    }

    /// Whether a request is waiting for the next frame boundary.
    pub fn is_pending(&self) -> bool {
        self.request.is_some()
    }

    /// Takes the pending request, leaving none (the engine's drain path).
    pub(crate) fn take(&mut self) -> Option<bool> {
        self.request.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn the_last_request_wins_and_take_empties() {
        let mut lock = CursorLock::default();
        assert!(!lock.is_pending());
        lock.lock();
        lock.release();
        assert!(lock.is_pending());
        assert_eq!(lock.take(), Some(false));
        assert_eq!(lock.take(), None);
    }

    static RELEASING: AtomicBool = AtomicBool::new(false);

    fn drive(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
        if let Some(lock) = world.resource_mut::<CursorLock>() {
            if RELEASING.load(Ordering::SeqCst) {
                lock.release();
            } else {
                lock.lock();
            }
        }
    }

    #[test]
    fn only_a_change_is_sent_and_no_sender_is_a_no_op() {
        RELEASING.store(false, Ordering::SeqCst);
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut engine = Engine::builder().system("drive", drive).build().unwrap();
        engine.set_cursor_lock_sender(tx);
        for _ in 0..3 {
            engine.advance_live();
        }
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), [true]);

        RELEASING.store(true, Ordering::SeqCst);
        for _ in 0..3 {
            engine.advance_live();
        }
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), [false]);

        RELEASING.store(false, Ordering::SeqCst);
        let mut bare = Engine::builder().system("drive", drive).build().unwrap();
        for _ in 0..3 {
            bare.advance_live();
        }
        assert_eq!(bare.cursor_lock_request(), Some(true));
    }
}
