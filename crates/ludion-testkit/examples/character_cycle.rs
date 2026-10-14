//! **Character cycle** — the `character-cycle` gallery scene with its driver: a mannequin that
//! idles, runs, jumps, runs, spins, runs and returns to idle, crossfading each switch and leaving
//! the one-shots on their `end` events (`ludion docs character-animation`).
//!
//! The driver is the same code the `character_cycle` and `gallery` tests run
//! (`tests/common/mod.rs`); this file only hands it to the facade's argv shell, so every host flag
//! works. `--scene` is required beside the replay modes: they take their content root from its
//! parent directory.
//!
//! Open it in a window (from the repo root):
//!
//! ```text
//! cargo run -p ludion-testkit --example character_cycle -- --scene examples/gallery/character-cycle.yaml --window
//! ```
//!
//! Replay-verify the committed recording (exits 0 when every hash matches):
//!
//! ```text
//! cargo run -p ludion-testkit --example character_cycle -- --replay-verify examples/gallery/character-cycle.lrp --scene examples/gallery/character-cycle.yaml
//! ```

#[path = "../tests/common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    ludion::run(common::cycle_builder())
}
