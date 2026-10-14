//! The game's systems — the `fn(&mut World, &SystemCtx)` behaviors that run each tick.
//!
//! Definitions live here; the *order* they run in is the `.system()` list in `game()` (`lib.rs`).

use ludion::prelude::*;

use crate::{PLAYER_SPEED, Player};

/// Slides every player right while the Right-arrow key is held, keeping its [`Transform`] in step.
///
/// A plain system: `fn(&mut World, &SystemCtx)`. It reads time only through `ctx.dt` and input only
/// through the [`InputState`] resource, so it is fully deterministic — no wall clock, no
/// `thread_rng`. Querying a game component and a built-in component together is the everyday shape of
/// gameplay code.
pub fn move_player(world: &mut World, ctx: &SystemCtx) {
    if !world.resource::<InputState>().unwrap().is_held(Key::Right) {
        return;
    }
    for (player, transform) in world.query::<(&mut Player, &mut Transform)>() {
        player.x += PLAYER_SPEED * ctx.dt;
        transform.translation[0] = player.x;
    }
}
