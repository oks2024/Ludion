//! The game's components — the plain data structs the systems read and write.
//!
//! At starter scale the tunables (`PLAYER_SPEED` and friends) ride here with the component they
//! configure; once they outgrow one file, give them a dedicated `tunables.rs` (the engine repo's `games/shmup-stress/src/tunables.rs`
//! is the worked large-scale example).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How fast the player slides, in world units per second. Applied as `speed * dt` each tick, so it
/// is frame-rate independent and deterministic. Tunables like this are the kind of value a designer
/// asks you to change.
pub const PLAYER_SPEED: f32 = 6.0;

/// The starter game component: a player's horizontal position.
///
/// It rides on the same entity as the built-in `Transform` and `Sprite`, and `move_player`
/// keeps the `Transform` in step so the sprite actually slides. Change these fields — or add your
/// own components (Ball, Brick, …) — to build your game.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct Player {
    /// The player's horizontal position, in world units.
    #[serde(default)]
    pub x: f32,
}

impl Default for Player {
    fn default() -> Self {
        Player { x: 0.0 }
    }
}
