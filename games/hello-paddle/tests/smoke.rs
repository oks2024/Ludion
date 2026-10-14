//! Smoke test for the `hello-paddle` **binary**: it starts, prints the mandatory machine-parseable
//! `listening on ws://…` line, answers a `sim.hash` over the wire, and shuts down.
//!
//! This proves `ludion::run` hosts a game exactly as `ludion serve` hosts a scene. `run` blocks until
//! Ctrl-C, so — like `ludion-cli`'s `serve` tests — this spawns the bin as a child, reads its bound
//! address off stdout (`--addr 127.0.0.1:0` asks the OS for a free port), drives it with
//! `ludion-client`, then hard-kills the child (std has no cross-platform graceful-Ctrl-C-to-child).

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};

fn scene() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scenes")
        .join("paddle.yaml")
}

/// Spawns the `hello-paddle` binary with piped stdout (stderr discarded — the happy path prints
/// none, and not reading it avoids a full-pipe stall).
fn spawn(args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_hello-paddle"))
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn the hello-paddle binary")
}

/// Reads the mandatory first stdout line and returns the `ws://…` URL it carries, plus the still-open
/// reader (hold it until the child is killed, or the child hits a broken pipe on its next line).
fn read_listening_url(child: &mut Child) -> (String, BufReader<ChildStdout>) {
    let stdout = child.stdout.take().expect("stdout was piped");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .expect("read the `listening on ...` line");
    let url = line
        .trim()
        .strip_prefix("listening on ")
        .unwrap_or_else(|| panic!("first stdout line was not machine-parseable: {line:?}"))
        .to_string();
    (url, reader)
}

fn kill(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// The game binary hosts its scene: `--paused` binds, prints the listening line, and a client can
/// connect and `sim.hash` it (a paused engine still answers). Proves the ~3-line game `main` is a
/// full wire host.
#[test]
fn hello_paddle_binary_serves_and_answers_sim_hash() {
    let scene = scene();
    let mut child = spawn(&[
        "--scene",
        scene.to_str().unwrap(),
        "--addr",
        "127.0.0.1:0",
        "--paused",
    ]);

    let (url, _stdout) = read_listening_url(&mut child);
    assert!(url.starts_with("ws://"), "not a ws:// URL: {url}");

    let mut client = ludion_client::Client::connect(&url).expect("client connects");
    let hash_before = client.sim_hash().expect("sim.hash answers while paused");

    // Step it over the wire; with no key held the paddle does not move, so a system-bearing but
    // input-free world is frame-invariant — the hash is unchanged, proving the step rode the real
    // engine (schedule included), not a stub.
    let stepped = client
        .sim_step(5, true)
        .expect("sim.step answers while paused");
    assert_eq!(
        Some(hash_before),
        stepped,
        "stepping an input-free paddle world must not change its hash"
    );

    kill(child);
}
