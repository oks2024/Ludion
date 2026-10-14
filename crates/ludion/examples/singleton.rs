//! **A score that exactly one entity carries** — the singleton lookup. `world.single::<T>()` is the
//! first entity carrying `T`, `world.single_get::<T>()` its value; both answer `None` when absent.
//!
//! `read_score` only reads, through `single_get`; `award` finds the id with `single` and writes with
//! `get_mut`. Neither scans the entities by hand.
//!
//! Everything above the `main` divider is copyable into a real game.
//!
//! Run it headless (prints the final score):
//!
//! ```text
//! cargo run -p ludion --example singleton
//! ```

use ludion::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// The run's score; one entity carries it.
#[derive(Serialize, Deserialize, JsonSchema, Debug)]
struct Score(u32);

/// The current score, or 0 when no entity carries one.
fn read_score(world: &World) -> u32 {
    world.single_get::<Score>().map_or(0, |s| s.0)
}

/// Adds `points` to the score singleton.
fn award(world: &mut World, points: u32) {
    if let Some(entity) = world.single::<Score>()
        && let Ok(Some(score)) = world.get_mut::<Score>(entity)
    {
        score.0 += points;
    }
}

// --- main (not copyable) -------------------------------------------------------------------------

fn main() -> ExitCode {
    let mut world = World::new();
    for _ in 0..3 {
        world.spawn();
    }
    let holder = world.spawn();
    world.insert(holder, Score(0)).expect("the holder is alive");

    award(&mut world, 5);
    award(&mut world, 10);
    let score = read_score(&world);
    println!("score: {score}");
    if score == 15 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
