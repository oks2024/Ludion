//! [`assert_no_boundary_crossings`] — the opt-in test gate for the sim/presentation boundary.
//!
//! The engine logs every `L0028` once per `(system, op, component)` even when a system throws the
//! `Result` away, but a log line never fails a test. Call this after a run to turn any crossing
//! into a failure.

use ludion_core::codes::code;
use ludion_ecs::World;
use ludion_engine::Diagnostics;

/// Panics, listing every key, if `world` saw an `L0028` phase-boundary refusal this run.
///
/// Pass `engine.world()`, or a `SimDriver`'s world after the run. It reads both places a crossing
/// lives: the world's undrained outbox (a bare driver never drains) and, for an engine run, the
/// `Diagnostics` latch the frame boundary moved it into.
///
/// # Panics
/// With the offending `"{system}/{op}/{Component}"` keys when any crossing was recorded.
///
/// # Example
/// ```
/// use ludion_ecs::World;
///
/// let mut world = World::new();
/// let e = world.spawn();
/// assert!(world.get::<u8>(e).is_ok());
/// ludion_testkit::assert_no_boundary_crossings(&world);
/// ```
pub fn assert_no_boundary_crossings(world: &World) {
    let mut keys = world.pending_boundary_crossings();
    if let Some(diagnostics) = world.resource::<Diagnostics>() {
        keys.extend(
            diagnostics
                .reported(code::PRESENTATION_BOUNDARY)
                .map(str::to_owned),
        );
    }
    keys.sort();
    keys.dedup();
    assert!(
        keys.is_empty(),
        "{} phase-boundary crossing(s) ({}); each key is `system/op/Component`:\n  {}",
        keys.len(),
        code::PRESENTATION_BOUNDARY,
        keys.join("\n  ")
    );
}
