//! **Camera chase** — the `camera-chase` gallery scene with its driver: a chase camera follows a hero
//! through a yard (the boom sweeps past a pillar), under a low ceiling through two overlapping
//! camera zones, behind a prop that fades, and into a peek corner (`ludion docs camera`).
//!
//! The driver is the same code the `camera_chase` and `gallery` tests run (`tests/common/mod.rs`);
//! this file only hands it to the facade's argv shell, so every host flag works.
//!
//! Open it in a window (from the repo root):
//!
//! ```text
//! cargo run -p ludion-testkit --example camera_chase -- --scene examples/gallery/camera-chase.yaml --window
//! ```
//!
//! Draw the pivot, the boom, the red pull-in cross and the active zone:
//!
//! ```text
//! cargo run -p ludion-testkit --example camera_chase -- --scene examples/gallery/camera-chase.yaml --window --debug-draw camera
//! ```

#[path = "../tests/common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    ludion::run(common::chase_builder())
}
