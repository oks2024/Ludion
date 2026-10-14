//! Thin helpers for headless gameplay assertions on a [`SimDriver`]-driven world.
//!
//! `ludion-sim`'s own determinism suite (`crates/ludion-sim/tests/determinism.rs`) already hand-
//! writes the two repeating patterns every scripted-gameplay test needs: "tick one frame at a time,
//! watching a component predicate, until it holds" and "run two drivers, compare their `WorldHash`
//! sequence." These functions are exactly those two patterns, extracted once — not a DSL. Callers
//! still build their own [`World`], [`Schedule`](ludion_ecs::Schedule), and [`SimDriver`]; this
//! module only removes the hand-rolled loop and the paired `.unwrap()`s around
//! [`WorldHash::compute`].
//!
//! # A note on this crate's dependency edge
//!
//! `engine-architecture.md`'s crate graph lists `ludion-testkit → engine, client`. These helpers
//! need `SimDriver`/`World`/`Registry`/`WorldHash` directly, which are not re-exported through
//! `ludion-engine`'s public surface (a minimal, render-focused crate by design — see its module
//! docs). Depending on `ludion-ecs`/`ludion-sim` directly, rather than routing through `engine`,
//! adds no cycle risk (neither depends on `testkit` or `engine`) and keeps this module a thin
//! wrapper instead of forcing new re-exports onto `ludion-engine` just to satisfy a diagram.

use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::FrameIndex;
use ludion_ecs::{Registry, World};
use ludion_sim::{SimDriver, WorldHash};

/// Ticks `driver` one tick at a time, up to `max_ticks`, stopping at the first tick where
/// `predicate(world)` holds. Returns the frame at which it first became true, or `None` if it never
/// did within `max_ticks`.
///
/// Mirrors the "tick one frame at a time, watching a component predicate" loop written by hand in
/// `ludion-sim`'s own scripted-gameplay test (`scripted_dash_crosses_x_3_by_frame_30_then_stops`).
///
/// # Panics
/// If a system panics in a tick, with the `L4007` diagnostic — `None` would read as "the predicate
/// never held" and hide a broken world.
pub fn run_until(
    driver: &mut SimDriver,
    max_ticks: u64,
    mut predicate: impl FnMut(&World) -> bool,
) -> Option<FrameIndex> {
    for _ in 0..max_ticks {
        driver.run_ticks(1);
        if let Some(d) = driver.take_system_panic() {
            panic!("{}: {}", d.code.as_str(), d.message);
        }
        if predicate(driver.world()) {
            return Some(driver.frame());
        }
    }
    None
}

/// Like [`run_until`], but panics with `description` and `max_ticks` in the message if the
/// predicate never becomes true — the common case in a `#[test]` fn, where the crossing is
/// expected to happen.
///
/// # Panics
/// If `predicate` does not hold for any tick in `0..max_ticks`, or if a system panics in a tick.
pub fn assert_reaches(
    driver: &mut SimDriver,
    max_ticks: u64,
    description: &str,
    predicate: impl FnMut(&World) -> bool,
) -> FrameIndex {
    run_until(driver, max_ticks, predicate).unwrap_or_else(|| {
        panic!("expected {description} within {max_ticks} ticks, but it never happened")
    })
}

/// Runs `driver` for `ticks` ticks, one at a time, recording the [`WorldHash`] after every tick.
///
/// Mirrors the "double run, compare hash sequences" pattern from `ludion-sim`'s determinism suite
/// as one call — the sequence (not just the final hash) is what catches a divergence at the exact
/// frame it happens.
///
/// # Errors
/// Returns the `L4007` diagnostic of a system that panicked in a tick, and propagates a component
/// serialization failure from [`WorldHash::compute`] (`L0007`) — a determinism bug in its own
/// right, not swallowed here.
pub fn hash_sequence(
    driver: &mut SimDriver,
    registry: &Registry,
    ticks: u64,
) -> Result<Vec<WorldHash>, Diagnostic> {
    let mut hashes = Vec::with_capacity(usize::try_from(ticks).unwrap_or(0));
    for _ in 0..ticks {
        driver.run_ticks(1);
        if let Some(d) = driver.take_system_panic() {
            return Err(d);
        }
        hashes.push(WorldHash::compute(driver.world(), registry)?);
    }
    Ok(hashes)
}

/// Asserts that `a` and `b` produce identical `WorldHash` sequences over `ticks` ticks — the
/// determinism check every scripted gameplay test and replay-equivalence check needs. Panics
/// naming the first diverging frame (the driver's frame after that tick) if they differ.
///
/// # Panics
/// If the two hash sequences diverge, if either driver's `WorldHash::compute` fails, or if a
/// system panics in either run.
pub fn assert_hash_sequences_match(
    a: &mut SimDriver,
    b: &mut SimDriver,
    registry: &Registry,
    ticks: u64,
) {
    // Hash `i` is the world after the tick that starts at `start + i`, i.e. at frame `start + i + 1`.
    let start = a.frame().as_u64();
    let hashes_a = hash_sequence(a, registry, ticks).expect("hash_sequence failed");
    let hashes_b = hash_sequence(b, registry, ticks).expect("hash_sequence failed");
    for (index, (ha, hb)) in hashes_a.iter().zip(hashes_b.iter()).enumerate() {
        assert_eq!(
            ha,
            hb,
            "WorldHash sequences diverge at frame {}",
            start + index as u64 + 1
        );
    }
    assert_eq!(
        hashes_a.len(),
        hashes_b.len(),
        "hash sequences must cover the same number of ticks"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_core::time::FrameIndex as FI;
    use ludion_ecs::{Schedule, SystemCtx};
    use ludion_sim::{InputEvent, InputScript, InputState, Key, SimConfig};
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
    struct Position {
        x: f32,
    }

    fn steer(world: &mut World, _ctx: &SystemCtx) {
        let held = world.resource::<InputState>().unwrap().is_held(Key::D);
        if held {
            for (pos,) in world.query::<(&mut Position,)>() {
                pos.x += 1.0;
            }
        }
    }

    fn registry() -> Registry {
        let mut registry = Registry::new();
        registry.register::<Position>("Position").unwrap();
        registry
    }

    fn driver_with_ball() -> (SimDriver, ludion_core::ids::EntityId) {
        let mut world = World::new();
        let ball = world.spawn();
        world.insert(ball, Position { x: 0.0 }).unwrap();
        let mut schedule = Schedule::new();
        schedule.add_system("steer", steer);

        let mut script = InputScript::new();
        script.push(FI::new(0), InputEvent::KeyDown { key: Key::D });

        let driver = SimDriver::new(
            world,
            schedule,
            SimConfig {
                seed: 1,
                input: script,
                ..SimConfig::default()
            },
        );
        (driver, ball)
    }

    #[test]
    fn run_until_stops_at_the_first_true_frame() {
        let (mut driver, ball) = driver_with_ball();
        let frame = run_until(&mut driver, 10, |world| {
            world.get::<Position>(ball).unwrap().unwrap().x >= 3.0
        });
        assert_eq!(frame, Some(FI::new(3)));
    }

    #[test]
    fn run_until_returns_none_when_never_true() {
        let (mut driver, ball) = driver_with_ball();
        let frame = run_until(&mut driver, 2, |world| {
            world.get::<Position>(ball).unwrap().unwrap().x >= 100.0
        });
        assert_eq!(frame, None);
    }

    #[test]
    #[should_panic(expected = "expected x to reach 100 within 2 ticks")]
    fn assert_reaches_panics_with_a_readable_message() {
        let (mut driver, ball) = driver_with_ball();
        assert_reaches(&mut driver, 2, "x to reach 100", |world| {
            world.get::<Position>(ball).unwrap().unwrap().x >= 100.0
        });
    }

    #[test]
    fn hash_sequence_matches_across_two_identically_configured_drivers() {
        let registry = registry();
        let (mut a, _) = driver_with_ball();
        let (mut b, _) = driver_with_ball();
        assert_hash_sequences_match(&mut a, &mut b, &registry, 20);
    }

    #[test]
    #[should_panic(expected = "diverge at frame 6")]
    fn assert_hash_sequences_match_names_the_real_diverging_frame() {
        let registry = registry();
        let (mut a, _) = driver_with_ball();
        let (mut b, ball_b) = driver_with_ball();
        // Perturb b after 5 ticks: both start at frame 5, and the first compared hash (the world at
        // frame 6) already differs.
        b.run_ticks(5);
        b.world_mut()
            .get_mut::<Position>(ball_b)
            .unwrap()
            .unwrap()
            .x += 1.0;
        a.run_ticks(5);
        assert_hash_sequences_match(&mut a, &mut b, &registry, 5);
    }

    fn panics_on_frame_one(_world: &mut World, ctx: &SystemCtx) {
        if ctx.frame.as_u64() == 1 {
            panic!("boom");
        }
    }

    fn driver_that_panics() -> SimDriver {
        let mut schedule = Schedule::new();
        schedule.add_system("panics_on_frame_one", panics_on_frame_one);
        SimDriver::new(World::new(), schedule, SimConfig::default())
    }

    #[test]
    #[should_panic(expected = "L4007")]
    fn assert_reaches_panics_on_a_system_panic() {
        let mut driver = driver_that_panics();
        assert_reaches(&mut driver, 10, "never", |_| false);
    }

    #[test]
    #[should_panic(expected = "L4007")]
    fn assert_hash_sequences_match_panics_on_a_system_panic() {
        let registry = registry();
        let (mut a, _) = driver_with_ball();
        let mut b = driver_that_panics();
        assert_hash_sequences_match(&mut a, &mut b, &registry, 5);
    }

    #[test]
    fn hash_sequence_returns_l4007_on_a_system_panic() {
        let mut driver = driver_that_panics();
        let error = hash_sequence(&mut driver, &registry(), 5).unwrap_err();
        assert_eq!(error.code.as_str(), "L4007");
    }
}
