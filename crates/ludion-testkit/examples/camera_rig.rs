//! **Camera rig** — the `camera-rig` gallery scene with its driver: a camera on a rail follows a hero
//! cube, blends to a side-on rig at tick 60, cuts back at 180 and shakes at 220
//! (`ludion docs camera`).
//!
//! The driver is the same code the `camera_rig` and `gallery` tests run (`tests/common/mod.rs`);
//! this file only hands it to the facade's argv shell, so every host flag works.
//!
//! Open it in a window (from the repo root):
//!
//! ```text
//! cargo run -p ludion-testkit --example camera_rig -- --scene examples/gallery/camera-rig.yaml --window
//! ```
//!
//! Draw the rail, the search window, the eyes, the focus and the anchor:
//!
//! ```text
//! cargo run -p ludion-testkit --example camera_rig -- --scene examples/gallery/camera-rig.yaml --window --debug-draw camera
//! ```

#[path = "../tests/common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    ludion::run(common::rig_builder())
}
