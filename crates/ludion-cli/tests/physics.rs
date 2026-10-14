//! `Plan/physics.md` W2, step 8: `ludion sim-hash` steps a scene's `physics:` block through the
//! real CLI binary — the host that has no game systems at all, so this is the cleanest proof that
//! a scene's own block is the opt-in and needs no game. The same proof runs at the 512-body stress
//! scene's scale, and at the W3 ragdoll gallery scene's, too.
//!
//! Mirrors `replay.rs`'s house style: run the real `ludion` binary as a subprocess.

use std::path::PathBuf;
use std::process::{Command, Output};

fn scene_path(file_name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gallery")
        .join(file_name)
}

fn trace_path(file_name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gallery")
        .join(file_name)
}

/// The committed trace's `<tick>` -> `<hash>` lines, comment/blank-filtered.
fn committed_trace(trace_file_name: &str) -> Vec<(String, String)> {
    std::fs::read_to_string(trace_path(trace_file_name))
        .expect("the committed reference trace")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(|line| {
            let mut parts = line.split_whitespace();
            let tick = parts.next().expect("a tick column").to_string();
            let hash = parts.next().expect("a hash column").to_string();
            (tick, hash)
        })
        .collect()
}

fn run_ludion(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

/// Runs `sim-hash --frames <frames>` against the committed gallery scene, returning the trimmed
/// stdout hash.
fn sim_hash_at(scene_file_name: &str, frames: u64) -> String {
    let output = run_ludion(&[
        "sim-hash",
        "--scene",
        scene_path(scene_file_name).to_str().unwrap(),
        "--frames",
        &frames.to_string(),
    ]);
    assert!(
        output.status.success(),
        "sim-hash --frames {frames} failed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("hash output is UTF-8");
    let hash = stdout.trim().to_string();
    assert_eq!(hash.len(), 16, "not a 16-hex-digit hash: {stdout:?}");
    hash
}

/// The CLI has no game systems at all (`sim_hash.rs`'s own module docs): the scene's own
/// `physics: space: free` block is the only thing that could install the triple here, so a hash
/// matching the committed trace is proof the bare CLI host steps it.
///
/// `sim.step ticks: N` runs `N` total `advance_live` calls in one command
/// (`ludion-cli/src/sim_hash.rs`'s `Command::SimStep { ticks: frames, .. }`), the same as
/// `physics_gallery.rs`'s `run(n)` calling `advance_live()` `n` times — so `--frames N` lands on
/// the trace's 0-based tick `N - 1`: `--frames 120` matches the committed line `0119`.
#[test]
fn sim_hash_matches_the_committed_trace_at_two_frame_counts() {
    let trace = committed_trace("physics-pile.trace.txt");
    for frames in [120u64, 240] {
        let hash = sim_hash_at("physics-pile.yaml", frames);
        let (tick, expected) = &trace[(frames - 1) as usize];
        assert_eq!(
            &hash, expected,
            "sim-hash --frames {frames} should match committed trace tick {tick}"
        );
    }
}

/// `--frames 0`'s hash must differ from both stepped hashes — the pile is authored mid-air, so an
/// unstepped world is trivially distinguishable from one that has fallen at all.
#[test]
fn sim_hash_at_frames_zero_differs_from_a_stepped_hash() {
    let zero = sim_hash_at("physics-pile.yaml", 0);
    let stepped = sim_hash_at("physics-pile.yaml", 120);
    assert_ne!(zero, stepped);
}

/// The same proof at the 512-body stress scene's scale, at one frame count (`--frames 240` lands
/// on committed line `0239`, the same convention as the pile's own test).
#[test]
fn sim_hash_matches_the_stress_trace() {
    let trace = committed_trace("physics-stress.trace.txt");
    let frames = 240u64;
    let hash = sim_hash_at("physics-stress.yaml", frames);
    let (tick, expected) = &trace[(frames - 1) as usize];
    assert_eq!(
        &hash, expected,
        "sim-hash --frames {frames} should match the stress scene's committed trace tick {tick}"
    );
}

/// The same proof at the W3 ragdoll gallery scene's scale, at one frame count (`--frames 240` lands
/// on committed line `0239`, the same convention as the pile's own test).
#[test]
fn sim_hash_matches_the_ragdoll_trace() {
    let trace = committed_trace("ragdoll.trace.txt");
    let frames = 240u64;
    let hash = sim_hash_at("ragdoll.yaml", frames);
    let (tick, expected) = &trace[(frames - 1) as usize];
    assert_eq!(
        &hash, expected,
        "sim-hash --frames {frames} should match the ragdoll scene's committed trace tick {tick}"
    );
}

/// The same proof at the `Plan/skeletal-animation.md` W3 `ragdoll-skinned` gallery scene's scale, at
/// one frame count inside its 360-tick trace (`--frames 200` lands on committed line `0199`).
#[test]
fn sim_hash_matches_the_ragdoll_skinned_trace() {
    let trace = committed_trace("ragdoll-skinned.trace.txt");
    let frames = 200u64;
    let hash = sim_hash_at("ragdoll-skinned.yaml", frames);
    let (tick, expected) = &trace[(frames - 1) as usize];
    assert_eq!(
        &hash, expected,
        "sim-hash --frames {frames} should match the ragdoll-skinned scene's committed trace tick \
         {tick}"
    );
}
