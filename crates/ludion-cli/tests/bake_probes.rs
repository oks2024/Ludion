//! Integration tests for `ludion bake-probes`, driven against the built `ludion` binary.
//!
//! Each test copies the fixture into its own temp directory so the probe store lands there.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/probes")
        .join(name)
}

/// A scratch directory holding a copy of `names` from the fixture directory.
fn scratch(tag: &str, names: &[&str]) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("ludion-bake-probes-{tag}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    for name in names {
        std::fs::copy(fixture(name), dir.join(name)).unwrap();
    }
    dir
}

fn ludion(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
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

fn no_adapter(output: &Output) -> bool {
    ludion_core::require_gpu::skip_no_adapter(
        String::from_utf8_lossy(&output.stderr).contains("error[L2003]"),
    )
}

fn outcomes(report: &serde_json::Value) -> Vec<(String, String)> {
    report["probes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["name"].as_str().unwrap().to_string(),
                p["outcome"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn needs_gpu_a_second_bake_loads_what_the_first_captured() {
    let dir = scratch("twice", &["scene.yaml"]);
    let scene = dir.join("scene.yaml");
    let scene = scene.to_str().unwrap();
    let first = ludion(&["bake-probes", scene, "--json"]);
    if no_adapter(&first) {
        return;
    }
    assert!(first.status.success(), "{first:?}");
    let report = json(&first);
    assert_eq!(report["ok"], true);
    assert!(outcomes(&report).iter().all(|(_, o)| o == "captured"));
    assert_eq!(outcomes(&report).len(), 2);

    let second = json(&ludion(&["bake-probes", scene, "--json"]));
    assert!(outcomes(&second).iter().all(|(_, o)| o == "loaded"));
    let keys = |report: &serde_json::Value| -> Vec<serde_json::Value> {
        report["probes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["key"].clone())
            .collect()
    };
    assert_eq!(
        keys(&second),
        keys(&report),
        "a loaded entry has the key it was captured under"
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn needs_gpu_probe_recaptures_only_the_named_one() {
    let dir = scratch("named", &["scene.yaml"]);
    let scene = dir.join("scene.yaml");
    let scene = scene.to_str().unwrap();
    let first = ludion(&["bake-probes", scene, "--json"]);
    if no_adapter(&first) {
        return;
    }
    let forced = json(&ludion(&["bake-probes", scene, "--probe", "orb", "--json"]));
    let outcomes = outcomes(&forced);
    assert!(
        outcomes.contains(&("orb".into(), "captured".into())),
        "{outcomes:?}"
    );
    assert!(
        outcomes.contains(&("room".into(), "loaded".into())),
        "{outcomes:?}"
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn an_unknown_probe_name_is_l1280() {
    let dir = scratch("unknown", &["scene.yaml"]);
    let scene = dir.join("scene.yaml");
    let output = ludion(&["bake-probes", scene.to_str().unwrap(), "--probe", "nope"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1280"), "{stderr}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_scene_without_probes_exits_clean_with_an_empty_list() {
    let dir = scratch("plain", &["plain.yaml"]);
    let scene = dir.join("plain.yaml");
    let scene = scene.to_str().unwrap();
    let output = ludion(&["bake-probes", scene, "--json"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(json(&output)["probes"], serde_json::json!([]));
    let text = ludion(&["bake-probes", scene]);
    assert!(text.status.success());
    assert!(String::from_utf8_lossy(&text.stdout).contains("no reflection probes"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_game_scene_points_at_the_games_own_mode() {
    let dir = scratch("gamey", &["gamey.yaml"]);
    let output = ludion(&["bake-probes", dir.join("gamey.yaml").to_str().unwrap()]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("L1013") && stderr.contains("--bake-probes"),
        "{stderr}"
    );
    std::fs::remove_dir_all(dir).ok();
}
