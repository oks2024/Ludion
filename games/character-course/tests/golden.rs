//! Golden-image test: renders `scenes/main.yaml` at rest — the camera framing the whole course —
//! and compares it against `goldens/<os>/main.png`. The recipe (per-OS path, `LUDION_BLESS=1`,
//! SSIM compare, no-GPU-adapter (`L2003`) skip) is `ludion_testkit::GoldenTest::single`; the scene names this game's
//! components, so it validates against `character_course::registry`. A second golden, `behind.png`,
//! is the course after 60 ticks: the camera rig settled on its rail behind the hero at spawn.

use std::path::PathBuf;

use ludion_testkit::{GoldenOutcome, GoldenTest, bless_requested, platform_dir};

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
    let platform = match platform_dir() {
        Ok(platform) => platform,
        Err(diag) => {
            eprintln!(
                "SKIPPED golden test:
{diag}"
            );
            return;
        }
    };
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let golden = root.join("goldens").join(platform).join("behind.png");
    let test = GoldenTest::new(root.join("scenes/main.yaml"), &golden)
        .with_builder(character_course::game)
        .with_registry(character_course::registry)
        .with_frames(60);
    if bless_requested() {
        test.bless().expect("blessing behind.png");
        eprintln!("blessed {} — review it, then commit it", golden.display());
        return;
    }
    assert!(golden.exists(), "missing {}", golden.display());
    let out_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("testkit-behind");
    match test.run(&out_dir) {
        Ok(GoldenOutcome::Passed { report }) => eprintln!("golden passed: {}", report.summary),
        Ok(GoldenOutcome::Failed { report, .. }) => {
            panic!("golden comparison failed: {}", report.summary)
        }
        Ok(GoldenOutcome::Blessed { .. }) => unreachable!("run() never blesses"),
        Err(diags) if ludion_core::require_gpu::is_no_adapter(&diags) => {
            eprintln!("SKIPPED golden test: no GPU adapter (L2003)");
        }
        Err(diags) => panic!("rendering behind.png failed: {diags:?}"),
    }
}
