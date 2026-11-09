//! **Ragdoll skin switch** — the `ragdoll-skin-switch` gallery scene with its driver: a mannequin
//! running its `run` clip is switched to its `SkinBone` ragdoll by game code at tick 40; the parts
//! land at the rest pose and keep the run's forward velocity (`ludion docs physics-joints`).
//!
//! The driver is the same code the `ragdoll_skin_switch` and `gallery` tests run
//! (`tests/common/mod.rs`); this file only hands it to the facade's argv shell, so every host flag
//! works.
//!
//! Open it in a window (from the repo root):
//!
//! ```text
//! cargo run -p ludion-testkit --example ragdoll_skin_switch -- --scene examples/gallery/ragdoll-skin-switch.yaml --window
//! ```

#[path = "../tests/common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    ludion::run(common::skin_switch_builder())
}
