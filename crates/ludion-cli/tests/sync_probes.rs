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

/// `sync --json` with the lane's GPU ban lifted: the load-only tests below fail before any device
/// is touched, and the ban would stop the warm before it loads a scene.
fn sync(dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .env_remove("LUDION_FORBID_GPU")
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
fn a_scene_without_a_probe_is_not_loaded() {
    let dir = project("mixed", &["gamey.yaml", "plain.yaml"]);
    let output = sync(&dir);
    assert!(output.status.success(), "{output:?}");
    let report = json(&output);
    assert_eq!(
        report["probe_scenes_skipped"],
        serde_json::json!(["scenes/gamey.yaml"])
    );
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

fn sync_at(dir: &Path, quality: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(["sync", "--json", "--quality", quality, "--project"])
        .arg(dir)
        .output()
        .expect("failed to run the ludion binary")
}

fn bake(dir: &Path, scene: &Path, quality: &str) -> serde_json::Value {
    json(
        &Command::new(env!("CARGO_BIN_EXE_ludion"))
            .current_dir(dir)
            .args(["bake-probes", "--json", "--quality", quality])
            .arg(scene)
            .output()
            .expect("failed to run the ludion binary"),
    )
}

fn store(dir: &Path) -> PathBuf {
    dir.join(".ludion/cooked/probes")
}

fn count_entries(dir: &Path) -> usize {
    std::fs::read_dir(store(dir))
        .map(|rd| rd.count())
        .unwrap_or(0)
}

/// Writes `name` into `dir`, last used `days` ago.
#[allow(clippy::disallowed_methods)] // an mtime is wall-clock by nature
fn aged(dir: &Path, name: &str, days: u64) {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, b"x").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(days * 86_400))
        .unwrap();
}

#[test]
fn prune_removes_aged_entries_without_a_gpu() {
    let dir = project("prune", &[]);
    let probes = store(&dir);
    aged(&probes, "00000000000000aa.ktx2", 30);
    aged(&probes, "00000000000000bb.1.tmp", 1);
    aged(&probes, "00000000000000cc.ktx2", 1);
    aged(&probes, "00000000000000dd.2.tmp", 0);
    let output = sync(&dir);
    assert!(output.status.success(), "{output:?}");
    let report = json(&output);
    assert_eq!(report["probes_pruned"], 2);
    assert_eq!(report["probe_store_over_cap"], false);
    assert!(report["probes_pruned_bytes"].as_u64().unwrap() >= 2);
    assert!(report["probe_store_bytes"].as_u64().unwrap() >= 2);
    let mut left: Vec<_> = std::fs::read_dir(&probes)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, ["00000000000000cc.ktx2", "00000000000000dd.2.tmp"]);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn prune_never_reaches_another_project() {
    let dir = project("prune-own", &[]);
    let sibling = project("prune-sibling", &[]);
    aged(&store(&dir), "00000000000000aa.ktx2", 30);
    aged(&store(&sibling), "00000000000000aa.ktx2", 30);
    assert!(sync(&dir).status.success());
    assert_eq!(count_entries(&dir), 0);
    assert_eq!(count_entries(&sibling), 1);
    std::fs::remove_dir_all(dir).ok();
    std::fs::remove_dir_all(sibling).ok();
}

#[test]
fn needs_gpu_a_sync_at_one_quality_keeps_another_qualitys_entries() {
    let dir = project("qualities", &["scene.yaml"]);
    let low = json(&sync_at(&dir, "low"));
    if low["probes_captured"] == 0 {
        return;
    }
    assert_eq!(count_entries(&dir), 2);
    let medium = json(&sync_at(&dir, "medium"));
    assert_eq!(medium["probes_captured"], 2, "{medium}");
    assert_eq!(count_entries(&dir), 4, "the low entries were pruned");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn needs_gpu_sync_honours_quality() {
    let dir = project("honours", &["scene.yaml"]);
    let synced = json(&sync_at(&dir, "low"));
    if synced["probes_captured"] == 0 {
        return;
    }
    let scene = dir.join("scenes/scene.yaml");
    let low = bake(&dir, &scene, "low");
    assert!(
        low["probes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["outcome"] == "loaded"),
        "{low}"
    );
    let medium = bake(&dir, &scene, "medium");
    assert!(
        medium["probes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["outcome"] == "captured"),
        "{medium}"
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn needs_gpu_a_scene_outside_the_warm_keeps_its_entry() {
    let dir = project("outside", &["scene.yaml"]);
    let text = std::fs::read_to_string(fixture("scene.yaml"))
        .unwrap()
        .replace("- 0.8\n", "- 0.7\n");
    std::fs::create_dir_all(dir.join("other")).unwrap();
    let other = dir.join("other/elsewhere.yaml");
    std::fs::write(&other, text).unwrap();
    let baked = bake(&dir, &other, "medium");
    if baked["ok"] != true || baked["probes"].as_array().is_none_or(|p| p.is_empty()) {
        return;
    }
    let key = baked["probes"][0]["key"].as_str().unwrap().to_string();
    let entry = store(&dir).join(format!("{key}.ktx2"));
    assert!(entry.is_file());
    // The warm resolves other keys; this entry is protected by being recently used, not by `keep`.
    let output = sync(&dir);
    assert!(output.status.success(), "{output:?}");
    assert!(
        entry.is_file(),
        "a recent entry outside the warm was pruned"
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_scene_that_fails_to_load_is_reported_not_skipped() {
    let dir = project("loaderr", &[]);
    std::fs::write(
        dir.join("scenes/broken.yaml"),
        "format: 1\nentities:\n  - name: p\n    components:\n      Transform:\n        translation: nope\n      ReflectionProbe: {}\n",
    )
    .unwrap();
    let output = sync(&dir);
    assert!(output.status.success(), "{output:?}");
    let report = json(&output);
    assert_eq!(report["ok"], true);
    assert_eq!(report["probe_scenes_skipped"], serde_json::json!([]));
    let diagnostics = report["diagnostics"].as_array().unwrap();
    assert!(!diagnostics.is_empty(), "{report}");
    assert_ne!(diagnostics[0]["code"], "L1013");
    std::fs::remove_dir_all(dir).ok();
}
