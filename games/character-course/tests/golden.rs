//! Golden-image test: renders `scenes/main.yaml` at rest — the camera framing the whole course —
//! and compares it against `goldens/<os>/main.png`. The recipe (per-OS path, `LUDION_BLESS=1`,
//! SSIM compare, no-GPU-adapter (`L2003`) skip) is `ludion_testkit::GoldenTest::single`; the scene names this game's
//! components, so it validates against `character_course::registry`.

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
