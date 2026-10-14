//! The [`Shutdown`] resource: the quit a game system asks the host for, the
//! [`SceneSwitch`](crate::SceneSwitch) mold.
//!
//! A system is handed `&mut World` and nothing else, so the only way it can end the process is to
//! leave a request the engine honors at the frame boundary — the same shape a district change takes.
//! The boundary drain routes it into the engine's `request_shutdown`, which is exactly what the wire
//! `engine.shutdown` handler calls: the live loop leaves its loop, the host runs the graceful
//! teardown Ctrl-C triggers, and a `--window` session closes its window. On a bare in-process engine
//! there is no host, so the request only makes
//! [`Engine::shutdown_requested`](crate::Engine::shutdown_requested) true.

/// An ECS resource: a quit a game system requests, honored at the next frame boundary.
///
/// The engine installs one (empty) into every world it builds, so a game system can always
/// `world.resource_mut::<Shutdown>()` it and call [`request`](Shutdown::request). Like
/// [`SceneSwitch`](crate::SceneSwitch) it is a plain resource, excluded from the `WorldHash` and from
/// snapshots — a capture taken mid-quit describes a session that is ending, so there is nothing for a
/// restore to reconstruct — and it registers no component, so it trips no drift gate.
///
/// A quit requested in the same frame as a [`SceneSwitch`](crate::SceneSwitch) **wins**: the world
/// the switch would rebuild is the one the host is about to tear down, and rebuilding it first would
/// only wipe the pending quit with every other resource.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, Shutdown};
///
/// /// A title screen's back binding: the one line that ends the process.
/// fn quit(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
///     if let Some(quit) = world.resource_mut::<Shutdown>() {
///         quit.request();
///     }
/// }
///
/// let mut engine = Engine::builder().system("quit", quit).build().unwrap();
/// assert!(!engine.shutdown_requested());
/// engine.advance_live();
/// assert!(engine.shutdown_requested());
/// ```
#[derive(Default, Debug)]
pub struct Shutdown {
    requested: bool,
}

impl Shutdown {
    /// Requests a graceful stop of the hosting process, to be honored at the next frame boundary.
    ///
    /// Idempotent: a system that calls this every frame it sits on the quit item requests one
    /// shutdown, exactly as a single call does.
    pub fn request(&mut self) {
        self.requested = true;
    }

    /// Whether a quit is currently pending — the [`SceneSwitch::is_pending`](crate::SceneSwitch::is_pending)
    /// read. False again once the engine has drained it (ask
    /// [`Engine::shutdown_requested`](crate::Engine::shutdown_requested) for the state after that).
    pub fn is_pending(&self) -> bool {
        self.requested
    }

    /// Takes the pending request, leaving none (the engine's drain path).
    pub(crate) fn take(&mut self) -> bool {
        std::mem::take(&mut self.requested)
    }
}
