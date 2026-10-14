//! Integration tests for `ludion sim-hash`, driven against the built `ludion` binary.
//!
//! The headline test is the Phase 1 cross-process exit criterion (`engine-architecture.md` §5,
//! determinism test layer 2): two *separate OS processes* run the same scene + script and must
//! print the identical final hash. Running under `cargo test` puts that check in CI on every
//! platform in the matrix.

use std::path::PathBuf;
use std::process::{Command, Output};

fn fixture(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(file)
}

fn run_sim_hash(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("sim-hash")
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

/// Runs sim-hash expecting success and returns the trimmed stdout hash.
fn hash_of(args: &[&str]) -> String {
    let output = run_sim_hash(args);
    assert!(
        output.status.success(),
        "sim-hash {args:?} failed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("hash output is UTF-8");
    let hash = stdout.trim().to_string();
    assert_eq!(hash.len(), 16, "not a 16-hex-digit hash: {stdout:?}");
    assert!(
        hash.bytes().all(|b| b.is_ascii_hexdigit()),
        "not a 16-hex-digit hash: {stdout:?}"
    );
    hash
}

/// **The Phase 1 cross-process exit criterion.** Two invocations of the built binary — two OS
/// processes, two address-space layouts, two allocator lifetimes — on the same scene, script,
/// and frame count print the identical final hash.
#[test]
fn two_processes_same_inputs_same_hash() {
    let scene = fixture("good.yaml");
    let script = fixture("script.json");
    let args = [
        "--scene",
        scene.to_str().unwrap(),
        "--script",
        script.to_str().unwrap(),
        "--frames",
        "600",
    ];
    let first = hash_of(&args);
    let second = hash_of(&args);
    assert_eq!(first, second, "cross-process determinism is broken");
}

/// A different scene yields a different hash — the hash actually reflects world content.
///
/// (A different *seed* alone cannot change the hash at CLI level: no game
/// systems are registered here, so nothing draws from the RNG, and `WorldHash` deliberately
/// covers world content, not resources. Seed sensitivity through gameplay systems is pinned by
/// `ludion-sim`'s in-process determinism suite.)
#[test]
fn different_scene_different_hash() {
    let good = fixture("good.yaml");
    let other = fixture("other.yaml");
    let a = hash_of(&["--scene", good.to_str().unwrap()]);
    let b = hash_of(&["--scene", other.to_str().unwrap()]);
    assert_ne!(a, b, "distinct scenes must not collide");
}

/// The frame count is part of the run, not the hash inputs: with no systems registered, world
/// content never changes, so 0 and 600 frames hash identically. This pins that ticking itself
/// (input application, frame advance) leaves untouched content untouched.
#[test]
fn empty_schedule_hash_is_frame_invariant() {
    let scene = fixture("good.yaml");
    let a = hash_of(&["--scene", scene.to_str().unwrap(), "--frames", "0"]);
    let b = hash_of(&["--scene", scene.to_str().unwrap(), "--frames", "600"]);
    assert_eq!(a, b);
}

/// A broken scene exits non-zero, prints check-scene's diagnostics on stderr, and no hash.
#[test]
fn broken_scene_fails_with_diagnostics() {
    let scene = fixture("broken.yaml");
    let output = run_sim_hash(&["--scene", scene.to_str().unwrap()]);
    assert!(!output.status.success(), "expected a non-zero exit");
    assert!(
        output.stdout.is_empty(),
        "no hash may be printed on failure"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1013]"), "stderr was: {stderr}");
    assert!(stderr.contains("error[L1014]"), "stderr was: {stderr}");
}

/// A missing script file is an L4002 diagnostic; a malformed one is an L4003 with the record
/// shape in the help. Both exit non-zero without printing a hash.
#[test]
fn bad_script_fails_with_script_diagnostics() {
    let scene = fixture("good.yaml");

    let output = run_sim_hash(&[
        "--scene",
        scene.to_str().unwrap(),
        "--script",
        "does-not-exist.json",
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    assert!(
        output.stdout.is_empty(),
        "no hash may be printed on failure"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L4002]"), "stderr was: {stderr}");

    // The scene fixture is valid YAML but not an input script: parse failure, not IO failure.
    let not_a_script = fixture("good.yaml");
    let output = run_sim_hash(&[
        "--scene",
        scene.to_str().unwrap(),
        "--script",
        not_a_script.to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L4003]"), "stderr was: {stderr}");
    assert!(stderr.contains("key_down"), "stderr was: {stderr}");
}

/// Every L-code sim-hash can print has a working `ludion explain` entry.
#[test]
fn sim_hash_codes_are_explainable() {
    for code in ["L4002", "L4003"] {
        let output = Command::new(env!("CARGO_BIN_EXE_ludion"))
            .args(["explain", code])
            .output()
            .expect("failed to run the ludion binary");
        assert!(
            output.status.success(),
            "`ludion explain {code}` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
