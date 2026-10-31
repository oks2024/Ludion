//! Golden-image test: renders `scenes/main.yaml` and compares it against a committed golden PNG.
//!
//! The full recipe — per-OS golden path, `LUDION_BLESS=1` bless flow, no-golden-yet skip, SSIM
//! compare, no-GPU-adapter (`L2003`) skip — is [`ludion_testkit::GoldenTest::single`]; see its doc
//! comment for the details every game's golden test shares. This game's own seam is
//! [`breakout::registry`]: the scene names this game's components, so it must validate against the
//! game's registry (built-ins + `breakout`'s own), not just the built-ins, or `check` would reject
//! them with `L1013`.

use std::path::PathBuf;

use ludion_core::codes::code;
use ludion_testkit::{GameplayDriver, GoldenOutcome, GoldenTest};

/// The scene, embedded as `tests/menu.rs` does.
const MAIN_SCENE: &str = include_str!("../scenes/main.yaml");

fn scene() -> PathBuf {
    scenes_dir().join("main.yaml")
}

fn scenes_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenes")
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
        breakout::registry,
        &out_dir(),
    );
}

/// The pause menu open over the board, drawn with its baked Inter face. The driver is rooted at
/// `scenes/`, where `fonts.yaml` lives; it presses the key, and `check_engine_golden` runs the
/// recipe on the engine it drove.
#[test]
fn needs_gpu_open_menu_matches_its_golden() {
    let mut driver = GameplayDriver::new(breakout::game().project_root(scenes_dir()), MAIN_SCENE);
    driver.tap("Escape");
    ludion_testkit::check_engine_golden(driver.engine_mut(), goldens_root(), "menu", &out_dir());
}

/// Frame 0 of the committed `run.lrp` (the post-load world, no tick run) is the main scene at rest.
/// `run`, not `check`: a bless would overwrite `main.png` and race the main-scene test.
#[test]
fn needs_gpu_replay_start_matches_the_main_golden() {
    let bytes = std::fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("run.lrp"))
        .expect("run.lrp reads");
    let outcome = GoldenTest::from_replay(&bytes, 0, scenes_dir(), goldens_root(), "main")
        .with_builder(breakout::game)
        .run(&out_dir());
    match outcome {
        Ok(GoldenOutcome::Passed { report }) => eprintln!("golden passed: {}", report.summary),
        Ok(GoldenOutcome::Failed { report, .. }) => panic!(
            "replay frame 0 differs from main.png: {}\n{:?}",
            report.summary, report.provenance
        ),
        Ok(GoldenOutcome::Blessed { .. }) => unreachable!("run() never blesses"),
        Err(diags) if ludion_core::require_gpu::is_no_adapter(&diags) => {
            eprintln!("SKIPPED replay golden test: no GPU adapter (L2003)");
        }
        Err(diags) => match ludion_testkit::unverifiable(&diags) {
            Some(skip) => eprintln!("SKIPPED replay golden test:\n{skip}"),
            None => panic!("rendering the replay frame failed: {diags:?}"),
        },
    }
}

/// A frame past the replay's end is `L3021`; the render is never reached, so no GPU is needed.
#[test]
fn a_replay_frame_past_its_end_is_l3021() {
    let bytes = std::fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("run.lrp"))
        .expect("run.lrp reads");
    let outcome = GoldenTest::from_replay(&bytes, 1_000_000, scenes_dir(), goldens_root(), "main")
        .with_builder(breakout::game)
        .run(&out_dir());
    let Err(diags) = outcome else {
        panic!("a frame past the end must be refused");
    };
    assert!(
        diags.iter().any(|d| d.code == code::REPLAY_FRAME_WINDOW),
        "expected L3021, got {diags:?}"
    );
}
