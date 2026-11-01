//! Integration tests for `ludion sim-hash --record` and `ludion replay`, driven against the built
//! `ludion` binary.
//!
//! Mirrors `sim_hash.rs`'s house style: run the real binary as a subprocess and assert on its exit
//! code and output. The headline test is the CLI form of the Phase 3 exit criterion
//! (`engine-architecture.md` §6 / implementation-guide.md Phase 3: "a recorded session replays to
//! an identical `WorldHash` sequence"): record a session with `sim-hash --record`, replay it in a
//! *separate process*, and assert the final hash matches.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(file)
}

/// A fresh path in the OS temp dir for this test's recorded session, unique per test process.
fn session_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "ludion-cli-replay-test-{name}-{}.lrp",
        std::process::id()
    ))
}

fn run_ludion(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

/// Runs `sim-hash` expecting success and returns its trimmed stdout hash.
fn hash_of(args: &[&str]) -> String {
    let mut full_args = vec!["sim-hash"];
    full_args.extend_from_slice(args);
    let output = run_ludion(&full_args);
    assert!(
        output.status.success(),
        "sim-hash {args:?} failed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("hash output is UTF-8");
    let hash = stdout.trim().to_string();
    assert_eq!(hash.len(), 16, "not a 16-hex-digit hash: {stdout:?}");
    hash
}

/// **The CLI form of Phase 3 exit criterion 3.** Record a session in one process, replay it in a
/// second, separate process, and check the final `WorldHash` matches the one the recording run
/// printed.
#[test]
fn recorded_session_replays_to_the_same_hash() {
    let scene = fixture("good.yaml");
    let script = fixture("script.json");
    let session = session_path("basic");
    let _cleanup = Cleanup(session.clone());

    let recorded_hash = hash_of(&[
        "--scene",
        scene.to_str().unwrap(),
        "--script",
        script.to_str().unwrap(),
        "--frames",
        "600",
        "--record",
        session.to_str().unwrap(),
    ]);

    assert!(session.exists(), "sim-hash --record did not write a file");

    let replay_output = run_ludion(&["replay", session.to_str().unwrap(), "--hashes"]);
    assert!(
        replay_output.status.success(),
        "ludion replay failed, stderr: {}",
        String::from_utf8_lossy(&replay_output.stderr)
    );
    let stdout = String::from_utf8(replay_output.stdout).expect("replay output is UTF-8");
    let last_line = stdout
        .lines()
        .next_back()
        .expect("--hashes prints at least one line");
    let replayed_hash = last_line
        .split_whitespace()
        .nth(1)
        .expect("each line is `<frame> <hash>`");

    assert_eq!(
        replayed_hash, recorded_hash,
        "replay diverged from the recorded run\nfull replay trace:\n{stdout}"
    );

    // The no-`--hashes` default prints just the final `<frame> <hash>` line, matching the last
    // line of the full trace above.
    let default_output = run_ludion(&["replay", session.to_str().unwrap()]);
    assert!(default_output.status.success());
    let default_stdout = String::from_utf8(default_output.stdout).unwrap();
    assert_eq!(default_stdout.trim_end(), last_line);
}

/// **Portability across a moved tree (the Phase 5 committed-example use case).** Record a session
/// with the scene and the recording living together in one directory, then *move that whole tree to a
/// different directory and delete the original*, and replay from the new location. Because the engine
/// embeds the scene inline, the replay is self-contained: it reproduces the identical per-frame
/// `WorldHash` sequence with no dependency on the recording machine's paths — exactly what makes a
/// replay committed to the repo replay on any checkout.
#[test]
fn recording_replays_identically_after_its_tree_is_moved() {
    let dir_a = temp_dir("portability-a");
    let dir_b = temp_dir("portability-b");
    let _cleanup_a = DirCleanup(dir_a.clone());
    let _cleanup_b = DirCleanup(dir_b.clone());
    std::fs::create_dir_all(&dir_a).unwrap();

    // Scene and recording live together in dir A (as a repo-committed example would).
    let scene_a = dir_a.join("level.yaml");
    std::fs::copy(fixture("good.yaml"), &scene_a).unwrap();
    let session_a = dir_a.join("session.lrp");

    // Record, driving the scene by its path inside dir A.
    let recorded_hash = hash_of(&[
        "--scene",
        scene_a.to_str().unwrap(),
        "--script",
        fixture("script.json").to_str().unwrap(),
        "--frames",
        "600",
        "--record",
        session_a.to_str().unwrap(),
    ]);

    // Baseline: the full per-frame trace as replayed from dir A.
    let trace_a = replay_trace(&session_a);
    assert!(
        trace_a.lines().count() > 1,
        "trace should span several frames:\n{trace_a}"
    );

    // Move the whole tree to dir B, then delete dir A so nothing can resolve back to it.
    std::fs::create_dir_all(&dir_b).unwrap();
    let scene_b = dir_b.join("level.yaml");
    let session_b = dir_b.join("session.lrp");
    std::fs::copy(&scene_a, &scene_b).unwrap();
    std::fs::copy(&session_a, &session_b).unwrap();
    std::fs::remove_dir_all(&dir_a).unwrap();

    // Replay from dir B: the trace must be byte-for-byte identical to the one recorded in dir A.
    let trace_b = replay_trace(&session_b);
    assert_eq!(
        trace_b, trace_a,
        "replay from the moved tree diverged from the original location"
    );
    let last_hash = trace_b
        .lines()
        .next_back()
        .and_then(|line| line.split_whitespace().nth(1))
        .expect("each line is `<frame> <hash>`");
    assert_eq!(
        last_hash, recorded_hash,
        "final hash changed after the move"
    );
}

/// A malformed replay session fails with an `L3010` diagnostic, not a panic.
#[test]
fn malformed_session_is_l3010() {
    let session = session_path("malformed");
    let _cleanup = Cleanup(session.clone());
    std::fs::write(&session, b"not a replay session").unwrap();

    let output = run_ludion(&["replay", session.to_str().unwrap()]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L3010]"), "stderr was: {stderr}");
}

/// A missing replay file is an `L1015` input-read failure, not a malformed session.
#[test]
fn missing_session_is_l1015() {
    let output = run_ludion(&["replay", "does-not-exist.lrp"]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1015]"), "stderr was: {stderr}");
}

/// A fresh directory path in the OS temp dir, unique per test process.
fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "ludion-cli-replay-test-{name}-{}",
        std::process::id()
    ))
}

/// Replays `session` with `--hashes`, asserting success, and returns its full trimmed stdout trace.
fn replay_trace(session: &Path) -> String {
    let output = run_ludion(&["replay", session.to_str().unwrap(), "--hashes"]);
    assert!(
        output.status.success(),
        "ludion replay {} failed, stderr: {}",
        session.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("replay output is UTF-8")
        .trim_end()
        .to_string()
}

/// Removes the test's temp session file on drop, success or failure.
struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Removes the test's temp directory (and its contents) on drop, success or failure.
struct DirCleanup(PathBuf);

impl Drop for DirCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `--record` stages the session in `<FILE>.recording` and renames it on success: an existing
/// target is replaced by a readable session, and no staging sibling is left behind.
#[test]
fn record_replaces_the_target_and_leaves_no_staging_file() {
    let session = session_path("staged");
    let staged = PathBuf::from(format!("{}.recording", session.display()));
    let _cleanup = Cleanup(session.clone());
    let _cleanup_staged = Cleanup(staged.clone());
    std::fs::write(&session, b"sentinel").unwrap();

    hash_of(&[
        "--scene",
        fixture("good.yaml").to_str().unwrap(),
        "--frames",
        "3",
        "--record",
        session.to_str().unwrap(),
    ]);

    assert!(!staged.exists(), "the staging file is renamed away");
    let replay = run_ludion(&["replay", session.to_str().unwrap()]);
    assert!(
        replay.status.success(),
        "the replaced target is a readable session: {}",
        String::from_utf8_lossy(&replay.stderr)
    );
}
