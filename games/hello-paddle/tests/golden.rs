//! Golden-image test for `hello-paddle`, exercising `ludion-testkit`'s registry seam: the scene
//! names the game's `Paddle` component, so it must be validated against the game's registry
//! (built-ins + `Paddle`) rather than the built-in registry alone, or it would fail `L1013`.
//!
//! The full recipe — per-OS golden path, `LUDION_BLESS=1` bless flow, no-golden-yet skip, SSIM
//! compare, no-GPU-adapter (`L2003`) skip — is [`ludion_testkit::GoldenTest::single`]; see its doc
//! comment for the details every game's golden test shares.

use std::path::PathBuf;

use ludion_testkit::GoldenTest;

fn scene() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scenes")
        .join("paddle.yaml")
}

fn goldens_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("goldens")
}

fn out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(concat!("testkit-", env!("CARGO_PKG_NAME")))
}

#[test]
fn needs_gpu_paddle_scene_matches_its_golden() {
    GoldenTest::single(
        scene(),
        goldens_root(),
        "paddle",
        hello_paddle::registry,
        &out_dir(),
    );
}
