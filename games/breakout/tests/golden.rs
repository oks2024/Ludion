//! Golden-image test: renders `scenes/main.yaml` and compares it against a committed golden PNG.
//!
//! The full recipe — per-OS golden path, `LUDION_BLESS=1` bless flow, no-golden-yet skip, SSIM
//! compare, no-GPU-adapter (`L2003`) skip — is [`ludion_testkit::GoldenTest::single`]; see its doc
//! comment for the details every game's golden test shares. This game's own seam is
//! [`breakout::registry`]: the scene names this game's components, so it must validate against the
//! game's registry (built-ins + `breakout`'s own), not just the built-ins, or `check` would reject
//! them with `L1013`.

use std::path::PathBuf;

use ludion_testkit::golden::{DEFAULT_HEIGHT, DEFAULT_SSIM_THRESHOLD, DEFAULT_WIDTH};
use ludion_testkit::{GameplayDriver, GoldenTest};

/// The scene, embedded as `tests/menu.rs` does.
const MAIN_SCENE: &str = include_str!("../scenes/main.yaml");

fn scene() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scenes")
        .join("main.yaml")
}

fn goldens_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("goldens")
}

fn out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("testkit")
}

#[test]
fn needs_gpu_main_scene_matches_its_golden() {
    GoldenTest::single(
        scene(),
        goldens_root(),
        "main",
        breakout::registry,
        &out_dir(),
    );
}

/// The pause menu open over the board, drawn with its baked Inter face. The driver is rooted at
/// `scenes/`, where `fonts.yaml` lives; `GoldenTest::single` renders a scene at rest and cannot press
/// a key, so this test follows its recipe by hand.
#[test]
fn needs_gpu_open_menu_matches_its_golden() {
    let platform = match ludion_testkit::platform_dir() {
        Ok(platform) => platform,
        Err(diag) => {
            eprintln!("SKIPPED golden test:\n{diag}");
            return;
        }
    };
    let golden = goldens_root().join(platform).join("menu.png");

    let scenes = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenes");
    let mut driver = GameplayDriver::new(breakout::game().project_root(scenes), MAIN_SCENE);
    driver.tap("Escape");
    let actual = match driver
        .engine_mut()
        .render_png(DEFAULT_WIDTH, DEFAULT_HEIGHT)
    {
        Ok(png) => png,
        Err(diag) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&diag)) => {
            assert!(
                !ludion_testkit::bless_requested(),
                "a bless must never skip: no GPU adapter (L2003)"
            );
            eprintln!("SKIPPED golden test: no GPU adapter (L2003)");
            return;
        }
        Err(diag) => panic!("rendering the open menu failed: {diag}"),
    };

    if ludion_testkit::bless_requested() {
        std::fs::write(&golden, &actual).expect("the menu golden is writable");
        eprintln!("blessed {} — review it, then commit it", golden.display());
        return;
    }
    if !golden.exists() {
        ludion_testkit::missing_golden(
            &golden,
            "bless it on your pinned GPU with `LUDION_BLESS=1 cargo test -p breakout --test golden`, \
             review the written PNG, then commit it",
        );
        return;
    }
    let report = ludion_testkit::compare_png_bytes(
        &std::fs::read(&golden).expect("the menu golden reads back"),
        &actual,
        DEFAULT_SSIM_THRESHOLD,
    )
    .expect("both PNGs decode");
    assert!(
        report.pass,
        "menu golden comparison failed: {}",
        report.summary
    );
}
