//! Integration tests for `ludion export`, driving the real built binary. The refusals all run before
//! the toolchain probe, so none of these needs Xcode; no test builds a staticlib or calls
//! `xcodebuild`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::common;

/// The `ludion` binary under test.
fn ludion() -> &'static str {
    env!("CARGO_BIN_EXE_ludion")
}

thread_local! {
    // A test's scratch locks live until its thread ends, so helpers can hand back a plain path.
    static SCRATCH_LOCKS: std::cell::RefCell<Vec<common::ScratchLock>> = const { std::cell::RefCell::new(Vec::new()) };
}
/// A fresh scratch directory (see `common::stable_scratch`), locked until the calling test ends.
fn scratch(name: &str) -> PathBuf {
    let (dir, lock) = common::stable_scratch(name);
    SCRATCH_LOCKS.with(|locks| locks.borrow_mut().push(lock));
    dir
}

/// A project directory holding just a manifest.
fn project(name: &str, manifest: &str) -> PathBuf {
    let dir = scratch(name).join("game");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("ludion.toml"), manifest).unwrap();
    dir
}

const GOOD: &str = "name = \"demo\"\ndefault_scene = \"scenes/main.yaml\"\n\
                    [ios]\nbundle_id = \"dev.example.demo\"\n";

fn export(project: &Path, out: &Path) -> Output {
    Command::new(ludion())
        .args(["export", "--target", "ios-sim", "--project"])
        .arg(project)
        .arg("--out")
        .arg(out)
        .output()
        .expect("run `ludion export`")
}

/// `export` with extra arguments (flags, then perhaps `-- app args`).
fn export_with(project: &Path, out: &Path, extra: &[&str]) -> Output {
    Command::new(ludion())
        .args(["export", "--target", "ios-sim", "--project"])
        .arg(project)
        .arg("--out")
        .arg(out)
        .args(extra)
        .output()
        .expect("run `ludion export`")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_project_without_a_manifest_is_l0023() {
    let dir = scratch("export_no_manifest");
    let output = export(&dir, &dir.join("out"));
    assert!(!output.status.success());
    assert!(stderr(&output).contains("L0023"), "{}", stderr(&output));
}

#[test]
fn a_manifest_without_ios_names_bundle_id() {
    let game = project(
        "export_no_ios",
        "name = \"demo\"\ndefault_scene = \"scenes/main.yaml\"\n",
    );
    let output = export(&game, &game.parent().unwrap().join("out"));
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("L0023") && err.contains("bundle_id"), "{err}");
}

#[test]
fn an_out_inside_the_project_is_refused() {
    let game = project("export_out_inside", GOOD);
    let output = export(&game, &game.join("build/out"));
    assert!(!output.status.success());
    assert!(stderr(&output).contains("overlaps"), "{}", stderr(&output));
}

#[test]
fn a_non_empty_folder_that_is_not_an_export_is_refused() {
    let game = project("export_out_occupied", GOOD);
    let out = game.parent().unwrap().join("occupied");
    fs::create_dir_all(&out).unwrap();
    fs::write(out.join("notes.txt"), "keep me").unwrap();
    let output = export(&game, &out);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("not a previous export"),
        "{}",
        stderr(&output)
    );
    assert!(out.join("notes.txt").exists());
}

/// Every flag combination refusal names the flag and runs before the manifest is read, so the
/// project here has none and the output folder is never made.
#[test]
fn flag_refusals_run_before_anything_else() {
    let dir = scratch("export_flags");
    let out = dir.join("out");
    let replay = dir.join("run.lrp");
    fs::write(&replay, "x").unwrap();
    let replay = replay.to_str().unwrap();
    let (first, second) = (dir.join("a/run.lrp"), dir.join("b/RUN.lrp"));
    for file in [&first, &second] {
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, "x").unwrap();
    }
    let (first, second) = (first.to_str().unwrap(), second.to_str().unwrap());
    let cases: &[(&[&str], &str)] = &[
        (&["--replay", replay], "--replay"),
        (
            &["--debug", "--replay", first, "--replay", second],
            "two `--replay`",
        ),
        (&["--debug", "--replay", "missing.lrp"], "is not a file"),
        (&["--debug", "--", "--replay-verify", "run.lrp"], "--run"),
        (&["--run", "--", "--replay-verify", "run.lrp"], "--debug"),
    ];
    for (extra, expected) in cases {
        let output = export_with(&dir, &out, extra);
        assert!(!output.status.success(), "{extra:?}");
        let err = stderr(&output);
        assert!(err.contains(expected), "{extra:?}: {err}");
        assert!(err.contains("L0024"), "{extra:?} lacks the code: {err}");
        assert!(!err.contains("L0023"), "{extra:?} read the manifest: {err}");
    }
    assert!(!out.exists());

    let device = Command::new(ludion())
        .args(["export", "--target", "ios", "--run", "--project"])
        .arg(&dir)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(!device.status.success());
    assert!(
        stderr(&device).contains("L0024") && stderr(&device).contains("`ios-sim` only"),
        "{}",
        stderr(&device)
    );
}
