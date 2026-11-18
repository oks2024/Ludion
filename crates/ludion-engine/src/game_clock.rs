//! The engine half of [`GameClock::scale`]: `ludion-core` cannot report, so the clock latches the
//! frame's first invalid request and the boundary drain turns it into `L4260`.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::GameClock;
use ludion_ecs::World;

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
