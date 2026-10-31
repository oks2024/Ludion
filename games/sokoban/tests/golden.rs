//! Golden-image test: renders `scenes/main.yaml` and compares it against a committed golden PNG.
//!
//! The full recipe — per-OS golden path, `LUDION_BLESS=1` bless flow, no-golden-yet skip, SSIM
//! compare, no-GPU-adapter (`L2003`) skip — is [`ludion_testkit::GoldenTest::single`]; see its doc
//! comment for the details every game's golden test shares. This game's own seam is
//! [`sokoban::registry`]: the scene names this game's components, so it must validate against the
//! game's registry (built-ins + `sokoban`'s own), not just the built-ins, or `check` would reject
//! them with `L1013`.

use std::path::PathBuf;

use ludion_testkit::GoldenTest;

fn scene() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scenes")
        .join("main.yaml")
}

fn goldens_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("goldens")
}

fn out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(concat!("testkit-", env!("CARGO_PKG_NAME")))
}

#[test]
fn needs_gpu_main_scene_matches_its_golden() {
    GoldenTest::single(
        scene(),
        goldens_root(),
        "main",
        sokoban::registry,
        &out_dir(),
    );
}
