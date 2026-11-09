//! **Ragdoll pose blend** — the `ragdoll-pose-blend` gallery scene with its driver: a skinned and two
//! voxel ragdolls (one pinned to a wall stake) switch at tick 0, then game code ramps each group's
//! `PoseBlend.weight` for the `curl` pose 0 -> 1 -> 0 over 60 ticks (`ludion docs physics-joints`).
//!
//! The driver is the same code the `ragdoll_pose_blend` test runs (`tests/common/mod.rs`); this file
//! only hands it to the facade's argv shell, so every host flag works.
//!
//! Open it in a window (from the repo root):
//!
//! ```text
//! cargo run -p ludion-testkit --example ragdoll_pose_blend -- --scene examples/gallery/ragdoll-pose-blend.yaml --window
//! ```

#[path = "../tests/common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    ludion::run(common::pose_blend_builder(common::CurlDrive::Ramp))
}
