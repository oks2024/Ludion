//! Integration tests for the probe warming `ludion sync` ends with, driven against the built binary
//! on a scratch project whose default scene directory holds the probe fixtures.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/probes")
        .join(name)
}

/// A project with no `[[import]]` table, whose `scenes/` holds copies of `names`.
fn project(tag: &str, names: &[&str]) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("sync-probes-{tag}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(dir.join("scenes")).unwrap();
    std::fs::write(
        dir.join("ludion.toml"),
        "name = \"scratch\"\ndefault_scene = \"scenes/main.yaml\"\n",
    )
    .unwrap();
    for name in names {
        std::fs::copy(fixture(name), dir.join("scenes").join(name)).unwrap();
    }
    dir
}

fn sync(dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(["sync", "--json", "--project"])
        .arg(dir)
        .output()
        .expect("failed to run the ludion binary")
}

fn json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

#[test]
fn needs_gpu_sync_captures_then_finds_the_probes_stored() {
    let dir = project("warm", &["scene.yaml"]);
    let first = sync(&dir);
    assert!(first.status.success(), "{first:?}");
    let report = json(&first);
    if report["probes_captured"] == 0 {
        // No adapter: warming stopped quietly (the lane that requires a GPU panics in the CLI).
        assert_eq!(report["probes_stored"], 0);
        return;
    }
    assert_eq!(report["probes_captured"], 2);
    assert_eq!(report["probes_stored"], 0);
    let second = json(&sync(&dir));
    assert_eq!(second["probes_captured"], 0);
    assert_eq!(second["probes_stored"], 2);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_scene_the_cli_cannot_load_is_named_not_failed() {
    let dir = project("game", &["gamey.yaml"]);
    let output = sync(&dir);
    assert!(output.status.success(), "{output:?}");
    let report = json(&output);
    assert_eq!(report["ok"], true);
    assert_eq!(
        report["probe_scenes_skipped"],
        serde_json::json!(["scenes/gamey.yaml"])
    );
    assert_eq!(report["probes_captured"], 0);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_project_without_probes_does_no_probe_work() {
    let dir = project("none", &["plain.yaml"]);
    let output = sync(&dir);
    assert!(output.status.success(), "{output:?}");
    let report = json(&output);
    assert_eq!(report["probes_captured"], 0);
    assert_eq!(report["probes_stored"], 0);
    assert_eq!(report["probe_scenes_skipped"], serde_json::json!([]));
    assert!(!dir.join("scenes/.ludion").exists());
    std::fs::remove_dir_all(dir).ok();
}
