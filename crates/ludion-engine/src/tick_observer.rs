//! A read-only per-tick watch over an [`Engine`](crate::Engine) — the hook behind the testkit's
//! `assert_deterministic`, open to any test or tool that wants to see every tick a body causes.
//!
//! The observer sees `&World`, never `&mut`: it lives on the engine (not in the world), is never
//! hashed or snapshotted, and costs one `Option` test per tick when unset. It watches
//! [`advance_live`](crate::Engine::advance_live) — the primitive `sim.step` and the live loop both
//! use — so every tick a test body can cause is seen. A tick it probes (by returning `true` from
//! [`before_tick`](TickObserver::before_tick)) skips the live profiler for that tick only; the world
//! after it is bit-identical to an unprobed tick's.
//!
//! # Example
//! ```
//! use std::sync::Arc;
//! use std::sync::atomic::{AtomicU64, Ordering};
//! use ludion_engine::{Engine, TickObserver};
//! use ludion_core::time::FrameIndex;
//! use ludion_ecs::{Registry, World};
//! use ludion_protocol::{Command, command};
//!
//! struct CountTicks(Arc<AtomicU64>);
//! impl TickObserver for CountTicks {
//!     fn after_tick(&mut self, _: FrameIndex, _: &World, _: &Registry) {
//!         self.0.fetch_add(1, Ordering::Relaxed);
//!     }
//! }
//!
//! let ticks = Arc::new(AtomicU64::new(0));
//! let mut engine = Engine::builder().build().unwrap();
//! engine.set_tick_observer(CountTicks(ticks.clone()));
//! engine
//!     .execute(Command::SimStep(command::SimStepParams { ticks: 3, hash: false }))
//!     .unwrap();
//! assert_eq!(ticks.load(Ordering::Relaxed), 3);
//! ```

use ludion_core::time::FrameIndex;
use ludion_ecs::{Registry, World};

/// Read-only per-tick watch over an Engine's `advance_live` ticks (`sim.step` and the live loop) —
/// a test/tooling hook. Install with [`Engine::set_tick_observer`](crate::Engine::set_tick_observer).
pub trait TickObserver: Send {
    /// A tick is about to run from `frame`; `world` is as the tick finds it. Return `true` to probe
    /// this tick per system ([`after_system`](TickObserver::after_system)).
    fn before_tick(&mut self, frame: FrameIndex, world: &World, registry: &Registry) -> bool {
        let _ = (frame, world, registry);
        false
    }

    /// After each system of a probed tick, the world as that system left it.
    fn after_system(&mut self, system: &'static str, world: &World, registry: &Registry) {
        let _ = (system, world, registry);
    }

    /// After the tick and its frame-boundary drains; `frame` is the frame it produced.
    fn after_tick(&mut self, frame: FrameIndex, world: &World, registry: &Registry);
}
