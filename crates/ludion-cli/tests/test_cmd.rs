//! Integration tests for `ludion test`, driven against the built `ludion` binary.
//!
//! Mirrors the house style in `render.rs`/`sim_hash.rs`: run the real binary as a subprocess and
//! assert on its exit code and output. Tests that bless run against a **scratch copy** of the
//! example gallery (never the committed `examples/gallery/goldens/` — a `cargo test` run should
//! not rewrite tracked files as a side effect); the real `ludion test --dir examples/gallery
//! --bless` / `ludion test --dir examples/gallery` flow is exercised manually (per the task's own
//! verification checklist) and by `ludion-testkit`'s `tests/gallery.rs`, which asserts against the
//! actual committed goldens without ever re-blessing them. Skips gracefully when the machine has no
//! GPU adapter, matching `render.rs`'s established pattern (`ludion test` itself never skips; these
//! `cargo test`s do).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn gallery_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

/// Every bless here writes into a scratch `goldens/<os>/`, so the reference-adapter guard
/// (`L2206`) is opted out — these tests must pass on any GPU.
fn run_ludion(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .env("LUDION_BLESS_ANY_ADAPTER", "1")
        .output()
        .expect("failed to run the ludion binary")
}

fn is_no_adapter(output: &Output) -> bool {
    ludion_core::require_gpu::skip_no_adapter(
        String::from_utf8_lossy(&output.stderr).contains("error[L2003]"),
    )
}

/// An OS with no pinned golden tier refuses discovery with `L2205` before any verdict: the run is
/// unverifiable there, a skip rather than a failure.
fn skip_unpinned(output: &Output, test: &str) -> bool {
    let unpinned = String::from_utf8_lossy(&output.stderr).contains("error[L2205]");
    if unpinned {
        eprintln!("SKIPPED {test}: no pinned golden tier on this OS (L2205)");
    }
    unpinned
}

/// Copies `src` into `dst` recursively — `voxels/` nests an assembly's `parts/` and `anims/`.
fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let target = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Copies every `*.yaml`/`*.yml` scene and the `assets/` + `characters/` + `clips/` + `fonts/` + `meshes/` +
/// `paths/` + `sculpts/` + `sprites/` + `voxels/` directories from the real gallery into `dir` — a
/// throwaway project directory a test can freely bless without touching tracked files.
fn copy_gallery_into(dir: &Path) -> usize {
    std::fs::create_dir_all(dir).unwrap();
    let gallery = gallery_dir();
    for sub in [
        "assets",
        "characters",
        "clips",
        "fonts",
        "meshes",
        "paths",
        "sculpts",
        "sprites",
        "voxels",
    ] {
        copy_tree(&gallery.join(sub), &dir.join(sub));
    }
    let mut scene_count = 0;
    for entry in std::fs::read_dir(&gallery).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("yaml") | Some("yml")
        ) {
            std::fs::copy(&path, dir.join(entry.file_name())).unwrap();
            // `fonts.yaml` is the content root's font table, not a scene.
            if path.file_name().and_then(|n| n.to_str()) != Some("fonts.yaml") {
                scene_count += 1;
            }
        }
    }
    scene_count
}

/// **Phase 2 CLI exit criterion**, against a scratch copy of the gallery: `ludion test --bless`
/// then `ludion test` both succeed, and the second run reports every scene at SSIM 1.0.
#[test]
fn needs_gpu_bless_then_test_passes_the_whole_gallery_at_exactly_one() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("ludion_test_cmd_bless_gallery");
    let scene_count = copy_gallery_into(&dir);
    let dir_str = dir.to_str().unwrap();

    let bless = run_ludion(&["test", "--dir", dir_str, "--bless"]);
    if !bless.status.success() {
        if is_no_adapter(&bless) {
            eprintln!(
                "SKIPPED bless_then_test_passes_the_whole_gallery_at_exactly_one: no GPU adapter"
            );
            return;
        }
        panic!("bless failed: {}", String::from_utf8_lossy(&bless.stderr));
    }
    let bless_stdout = String::from_utf8_lossy(&bless.stdout);
    assert!(
        bless_stdout.contains("BLESSED"),
        "stdout was: {bless_stdout}"
    );

    let test = run_ludion(&["test", "--dir", dir_str]);
    assert!(
        test.status.success(),
        "test run failed after blessing: {}",
        String::from_utf8_lossy(&test.stderr)
    );
    let stdout = String::from_utf8_lossy(&test.stdout);
    assert!(stdout.contains("PASS"), "stdout was: {stdout}");
    assert!(
        !stdout.contains("ssim=0"),
        "expected every scene at ssim=1.0000: {stdout}"
    );
    let pass_lines = stdout.lines().filter(|l| l.contains(": PASS")).count();
    assert_eq!(pass_lines, scene_count, "stdout was: {stdout}");
}

/// `--filter` narrows the suite to scenes whose stem contains the substring.
#[test]
fn needs_gpu_filter_narrows_the_suite() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("ludion_test_cmd_filter");
    copy_gallery_into(&dir);
    let dir_str = dir.to_str().unwrap();

    let bless = run_ludion(&["test", "--dir", dir_str, "--bless"]);
    if !bless.status.success() && is_no_adapter(&bless) {
        eprintln!("SKIPPED filter_narrows_the_suite: no GPU adapter");
        return;
    }

    let output = run_ludion(&["test", "--dir", dir_str, "--filter", "sprites"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("sprites:"), "stdout was: {stdout}");
    assert!(!stdout.contains("lights:"), "stdout was: {stdout}");
    assert!(!stdout.contains("meshes:"), "stdout was: {stdout}");
}

/// A directory with no scene files fails with `L2209`: a suite that checked nothing is not a pass.
#[test]
fn empty_directory_is_refused_with_l2209() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("ludion_test_cmd_empty_dir");
    std::fs::create_dir_all(&dir).unwrap();
    let output = run_ludion(&["test", "--dir", dir.to_str().unwrap()]);
    if skip_unpinned(&output, "empty_directory_is_refused_with_l2209") {
        return;
    }
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "stdout carries no verdict line");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L2209"), "stderr was: {stderr}");
    assert!(stderr.contains("`--dir`"), "stderr was: {stderr}");
}

/// A game project (`Cargo.toml` + `ludion.toml`) with no top-level scene is refused with help that
/// names `cargo test`, where its golden actually runs.
#[test]
fn a_game_project_directory_points_at_cargo_test() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("ludion_test_cmd_game_project");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    std::fs::write(dir.join("ludion.toml"), "name = \"x\"\n").unwrap();
    let output = run_ludion(&["test", "--dir", dir.to_str().unwrap()]);
    if skip_unpinned(&output, "a_game_project_directory_points_at_cargo_test") {
        return;
    }
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L2209"), "stderr was: {stderr}");
    assert!(
        stderr.contains("(`tests/golden.rs`); `ludion test"),
        "stderr was: {stderr}"
    );
}

/// A `--filter` that selects no scene fails with `L2209`: a run that checked nothing is not a pass.
#[test]
fn a_filter_matching_nothing_fails_with_l2209() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("ludion_test_cmd_filter_nothing");
    std::fs::create_dir_all(&dir).unwrap();
    let output = run_ludion(&["test", "--dir", dir.to_str().unwrap(), "--filter", "zzz"]);
    if skip_unpinned(&output, "a_filter_matching_nothing_fails_with_l2209") {
        return;
    }
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L2209"), "stderr was: {stderr}");
}

/// A directory whose golden is missing fails loudly with `L2201` and the `--bless` hint — `ludion
/// test` never silently skips a missing golden.
#[test]
fn needs_gpu_missing_golden_is_l2201_with_bless_help() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("ludion_test_cmd_missing_golden");
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    let gallery = gallery_dir();
    std::fs::copy(gallery.join("sprites.yaml"), dir.join("sprites.yaml")).unwrap();
    for entry in std::fs::read_dir(gallery.join("assets")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.join("assets").join(entry.file_name())).unwrap();
    }

    let output = run_ludion(&["test", "--dir", dir.to_str().unwrap()]);
    if is_no_adapter(&output) {
        eprintln!("SKIPPED missing_golden_is_l2201_with_bless_help: no GPU adapter");
        return;
    }
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L2201]"), "stderr was: {stderr}");
    assert!(stderr.contains("--bless"), "stderr was: {stderr}");
}

/// A top-level `*.voxel.yaml` model document participates in `ludion test` discovery exactly like a
/// scene: it is paired with `goldens/<os>/<stem>.png` (the stem is the full name minus the final
/// `.yaml`, so `tiny.voxel.yaml` → `goldens/<os>/tiny.voxel.png`), blessed by `--bless`, and passes
/// on the next run. The testkit's `kind:` dispatch renders it as a contact sheet; discovery needs no
/// change.
#[test]
fn needs_gpu_model_document_participates_in_discovery_bless_and_pass() {
    let platform = match ludion_testkit::platform_dir() {
        Ok(platform) => platform,
        Err(diag) => {
            eprintln!(
                "SKIPPED model_document_participates_in_discovery_bless_and_pass:
{diag}"
            );
            return;
        }
    };
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("ludion_test_cmd_model_doc");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("tiny.voxel.yaml"),
        "format: 1\nkind: voxel\nsize:\n  - 1\n  - 1\n  - 1\npalette:\n  A:\n    color:\n      \
         - 0.8\n      - 0.3\n      - 0.2\n      - 1.0\nslices:\n  -\n    - \"A\"\n",
    )
    .unwrap();
    let dir_str = dir.to_str().unwrap();

    let bless = run_ludion(&["test", "--dir", dir_str, "--bless"]);
    if !bless.status.success() {
        if is_no_adapter(&bless) {
            eprintln!(
                "SKIPPED model_document_participates_in_discovery_bless_and_pass: no adapter"
            );
            return;
        }
        panic!("bless failed: {}", String::from_utf8_lossy(&bless.stderr));
    }
    let bless_stdout = String::from_utf8_lossy(&bless.stdout);
    assert!(
        bless_stdout.contains("tiny.voxel: BLESSED"),
        "the model doc must be discovered and blessed under its `<name>.voxel` stem: {bless_stdout}"
    );
    assert!(
        dir.join("goldens")
            .join(platform)
            .join("tiny.voxel.png")
            .exists(),
        "the golden must be written to goldens/<os>/<stem>.png"
    );

    let test = run_ludion(&["test", "--dir", dir_str]);
    assert!(
        test.status.success(),
        "test run failed after blessing: {}",
        String::from_utf8_lossy(&test.stderr)
    );
    let stdout = String::from_utf8_lossy(&test.stdout);
    assert!(
        stdout.contains("tiny.voxel: PASS"),
        "the model doc must pass its freshly-blessed golden: {stdout}"
    );

    // `--filter` keys on the same stem.
    let filtered = run_ludion(&["test", "--dir", dir_str, "--filter", "tiny"]);
    let filtered_stdout = String::from_utf8_lossy(&filtered.stdout);
    assert!(
        filtered_stdout.contains("tiny.voxel:"),
        "filter must match the model doc stem: {filtered_stdout}"
    );
}

/// Every L-code `ludion test` can print has a working `ludion explain` entry.
#[test]
fn testkit_codes_are_explainable() {
    for code in [
        "L2201", "L2202", "L2203", "L2204", "L2205", "L2206", "L2207", "L2210",
    ] {
        let output = run_ludion(&["explain", code]);
        assert!(
            output.status.success(),
            "`ludion explain {code}` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
