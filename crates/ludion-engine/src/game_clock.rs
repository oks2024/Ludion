//! The engine half of [`GameClock::scale`]: `ludion-core` cannot report, so the clock latches the
//! frame's first invalid request and the boundary drain turns it into `L4260`.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::GameClock;
use ludion_ecs::World;
use xxhash_rust::xxh3::Xxh3;

/// The clock's `WorldHash` term (rule 6): `None` while it has excluded nothing, so a game that
/// never holds or scales hashes as it did before the term existed. It covers the clock's mode
/// only (holding, step, pending), never the accumulated `held`/`slowed` counters: those are a pure
/// function of the mode history, so hashing them would make a frozen world's hash drift every frame.
pub(crate) fn digest(clock: &GameClock) -> Option<u64> {
    let pending = clock.pending_q16();
    if !clock.is_holding() && !clock.is_scaling() && pending.is_none() {
        return None;
    }
    let mut hasher = Xxh3::new();
    hasher.update(&[u8::from(clock.is_holding())]);
    match pending {
        Some(q) => {
            hasher.update(&[1]);
            hasher.update(&q.to_le_bytes());
        }
        None => hasher.update(&[0]),
    }
    hasher.update(&clock.step_q16().to_le_bytes());
    Some(hasher.digest())
}

/// Reports `L4260` for the invalid `scale` value the frame's systems latched, once per session.
pub(crate) fn report_rejected_scale(world: &mut World) {
    let Some(value) = world
        .resource_mut::<GameClock>()
        .and_then(GameClock::take_rejected_scale)
    else {
        return;
    };
    crate::diagnostics::report_full(
        world,
        "game_clock",
        Diagnostic::new(
            code::GAME_SCALE_INVALID,
            format!("`GameClock::scale({value})` is outside (0, 1] and was ignored"),
        )
        .with_help("clamp to `(0, 1]`; use `hold()` for a freeze"),
    );
}
