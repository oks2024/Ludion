//! Golden-image test: renders `scenes/main.yaml` at rest — the camera framing the whole course —
//! and compares it against `goldens/<os>/main.png`. The recipe (per-OS path, `LUDION_BLESS=1`,
//! SSIM compare, no-GPU-adapter (`L2003`) skip) is `ludion_testkit::GoldenTest::single`; the scene names this game's
//! components, so it validates against `character_course::registry`. A second golden, `behind.png`,
//! is the course after 60 ticks: the camera rig settled on its rail behind the hero at spawn; it goes
//! through `GoldenTest::in_set(..).check`.

use std::path::PathBuf;

use ludion_testkit::GoldenTest;

#[test]
fn needs_gpu_main_scene_matches_its_golden() {
    GoldenTest::single(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenes/main.yaml"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("goldens"),
        "main",
        character_course::registry,
        &PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("testkit"),
    );
}

/// The course after 60 ticks, the rig behind the hero, against `goldens/<os>/behind.png`.
#[test]
fn needs_gpu_behind_rig_matches_its_golden() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    GoldenTest::in_set(
        root.join("scenes/main.yaml"),
        root.join("goldens"),
        "behind",
    )
    .with_builder(character_course::game)
    .with_registry(character_course::registry)
    .with_frames(60)
    .check(&PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("testkit-behind"));
}
