//! Integration tests for `ludion serve`, driven against the built `ludion` binary.
//!
//! `serve` is long-running (it blocks until Ctrl-C), so these tests spawn it as a child process,
//! read the mandatory `listening on ws://...` line from its stdout to learn the bound address
//! (`--addr 127.0.0.1:0` asks the OS for a free port), drive it over the wire with `ludion-client`,
//! then terminate the child. Rust's std has no cross-platform way to deliver a graceful Ctrl-C to a
//! *child* process (the `ctrlc` crate installs a handler for the *current* process only, and
//! Windows has no SIGINT-to-child primitive at all), so these tests use `Child::kill` — a hard
//! kill, not a signal. That's fine for the "does it serve" test; the hard-kill recording test can
//! only assert the file exists and is non-empty (the header + initial keyframe, written synchronously
//! by `set_recording` before the first client can even connect).
//!
//! The `engine.shutdown` wire command closes that gap on every platform (milestone-1-proof friction
//! #3): `engine_shutdown_stops_a_recording_session_cleanly_over_the_wire` drives a real `serve`,
//! sends `engine.shutdown`, and asserts the reply arrives, the process exits 0, and the recording it
//! *flushed on the graceful way out* replays — the clean-shutdown replay-identity check that a hard
//! kill could never do.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Output, Stdio};

fn fixture(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(file)
}

fn out_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

/// Spawns `ludion serve <args...>` with piped stdout (stderr discarded — the happy-path tests
/// expect none, and not reading it risks a full-pipe stall if they're wrong; a diagnostic that
/// never shows up there just fails the "got a listening line" assertion instead).
fn spawn_serve(args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("serve")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn `ludion serve`")
}

/// Like [`spawn_serve`] but runs the child with its working directory set to `cwd` — the lever the
/// asset-root test needs to prove `serve` resolves a scene's assets relative to the *scene file*, not
/// the process's cwd.
fn spawn_serve_in(cwd: &std::path::Path, args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("serve")
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn `ludion serve`")
}

/// Like [`spawn_serve`] but sets one environment variable on the *child* only.
///
/// Per-machine knobs (`LUDION_THREADS`, `LUDION_QUALITY`, `LUDION_RENDER_SIZE`) are read from the
/// environment, and a test that set them on its own process would leak into every other test in the
/// binary — they share one process. Setting them on the child is also the honest arrangement: it is
/// exactly how `ludion dev` hands them to the game it supervises.
fn spawn_serve_with_env(key: &str, value: &str, args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("serve")
        .args(args)
        .env(key, value)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn `ludion serve`")
}

/// Reads the mandatory first stdout line and returns the `ws://...` URL it carries, plus the
/// still-open reader.
///
/// The caller must hold onto the returned reader (not drop it) until the child is killed: `serve`
/// keeps printing its human-readable block right after the listening line, and Rust's `println!`
/// panics on a write error — closing our end of the pipe early (by dropping the `ChildStdout`)
/// would hand the still-running child a broken pipe on its very next line and kill it out from
/// under the test.
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

/// Hard-kills the child and reaps it (see the module docs on why this isn't a graceful signal).
fn kill(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Waits for the child to exit on its own, polling up to `max_polls` times at 25 ms each, and returns
/// its [`ExitStatus`]. On timeout it hard-kills the child and panics — so a shutdown that never
/// happens fails the test loudly instead of hanging it forever. (Polls a fixed count rather than a
/// wall clock: this crate's determinism denylist bans `Instant::now`, even in tests.)
fn wait_for_exit(mut child: Child, max_polls: u32) -> std::process::ExitStatus {
    for _ in 0..max_polls {
        match child.try_wait().expect("try_wait on the serve child") {
            Some(status) => return status,
            None => std::thread::sleep(std::time::Duration::from_millis(25)),
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("serve did not exit after engine.shutdown");
}

fn run_ludion(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

/// **The running example.** `serve --paused` binds, prints the mandatory listening line, and a
/// `ludion-client` can connect, `sim.hash`, and `sim.step` it — a paused engine still answers
/// queries and can be stepped explicitly over the wire.
#[test]
fn paused_serve_answers_sim_hash_and_step_over_the_wire() {
    let scene = fixture("good.yaml");
    let mut child = spawn_serve(&[
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

    // A paused engine does not free-run, so an explicit `sim.step` is required to advance it — and
    // it still answers with the resulting hash.
    let stepped = client
        .sim_step(5, true)
        .expect("sim.step answers while paused");
    assert!(
        stepped.is_some(),
        "sim.step with hash:true must return a hash"
    );

    // With no game systems registered, world content is frame-invariant (the same property
    // `sim_hash.rs`'s CLI tests pin), so the hash is unchanged — this proves the step actually
    // rode the same engine, not a stub.
    assert_eq!(
        hash_before,
        stepped.unwrap(),
        "stepping a system-free world must not change its hash"
    );

    kill(child);
}

/// **Recording stays a host-side switch.** `serve --record` arms recording before the sim thread
/// starts, so even after a hard kill (no clean shutdown, no flush) the file already contains the
/// header and initial keyframe: it exists, is non-empty, and opens with the replay magic.
#[test]
fn serve_record_writes_a_nonempty_session_file() {
    let scene = fixture("good.yaml");
    let session = out_path("serve_record_test.lrp");
    let _ = std::fs::remove_file(&session);

    let mut child = spawn_serve(&[
        "--scene",
        scene.to_str().unwrap(),
        "--addr",
        "127.0.0.1:0",
        "--record",
        session.to_str().unwrap(),
        "--paused",
    ]);

    let (url, _stdout) = read_listening_url(&mut child);
    let mut client = ludion_client::Client::connect(&url).expect("client connects");
    // Drive a couple of commands over the wire so the session has more than just the header.
    client.sim_step(3, false).expect("sim.step succeeds");
    client.sim_hash().expect("sim.hash succeeds");

    kill(child);

    let bytes = std::fs::read(&session).expect("the session file exists after a hard kill");
    assert!(!bytes.is_empty(), "the session file must not be empty");
    assert!(
        bytes.starts_with(&ludion_protocol::replay::REPLAY_MAGIC),
        "the session file must open with the replay magic"
    );

    let _ = std::fs::remove_file(&session);
}

/// **The friction-#3 regression test (milestone-1-proof).** A wire `engine.shutdown` stops a hosted
/// session cleanly and scriptably on every platform — no signal, no hard kill. `serve --record` is
/// driven over the wire, sent `engine.shutdown`, and:
///   - the command's `Ack` reaches the client *before* the process exits (the reply flushes first);
///   - the process exits 0 (a graceful stop, the same path Ctrl-C runs);
///   - the recording it flushed on the way out is a valid, replayable `.lrp`.
///
/// Before `engine.shutdown` existed, a scripted stop on Windows had no clean option (only a hard
/// kill, which cannot flush the recording) — this proves the gap is closed.
#[test]
fn engine_shutdown_stops_a_recording_session_cleanly_over_the_wire() {
    let scene = fixture("good.yaml");
    let session = out_path("serve_shutdown_test.lrp");
    let _ = std::fs::remove_file(&session);

    let mut child = spawn_serve(&[
        "--scene",
        scene.to_str().unwrap(),
        "--addr",
        "127.0.0.1:0",
        "--record",
        session.to_str().unwrap(),
        "--paused",
    ]);

    let (url, _stdout) = read_listening_url(&mut child);
    let mut client = ludion_client::Client::connect(&url).expect("client connects");
    // Drive a command so the session has content beyond the header/keyframe.
    client.sim_step(3, false).expect("sim.step succeeds");

    // The shutdown reply must arrive — the engine acks before the sim loop leaves and the host tears
    // down. A dropped connection here (no reply) would be the reply-ordering bug this guards against.
    let reply = client
        .call(ludion_protocol::Command::EngineShutdown)
        .expect("engine.shutdown must reply before the process exits");
    assert_eq!(
        reply.body,
        ludion_protocol::ResponseBody::Ack,
        "engine.shutdown acknowledges",
    );
    drop(client);

    let status = wait_for_exit(child, 600); // up to ~15s at 25ms/poll
    assert!(
        status.success(),
        "serve must exit 0 after engine.shutdown, got {status:?}",
    );

    // The recording flushed on the graceful way out: valid magic and actually replayable.
    let bytes = std::fs::read(&session).expect("the recording exists after a clean shutdown");
    assert!(
        bytes.starts_with(&ludion_protocol::replay::REPLAY_MAGIC),
        "the flushed session must open with the replay magic",
    );
    let replay = run_ludion(&["replay", session.to_str().unwrap(), "--hashes"]);
    assert!(
        replay.status.success(),
        "the flushed session must replay, stderr: {}",
        String::from_utf8_lossy(&replay.stderr),
    );
    assert!(
        !String::from_utf8_lossy(&replay.stdout).trim().is_empty(),
        "replay of the flushed session must produce a hash trace",
    );

    let _ = std::fs::remove_file(&session);
}

/// A bad scene path fails before ever binding: non-zero exit, a diagnostic on stderr, and no
/// `listening on` line on stdout.
#[test]
fn bad_scene_fails_with_diagnostics_and_never_binds() {
    let scene = fixture("broken.yaml");
    let output = run_ludion(&[
        "serve",
        "--scene",
        scene.to_str().unwrap(),
        "--addr",
        "127.0.0.1:0",
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    assert!(
        output.stdout.is_empty(),
        "nothing may be printed on stdout when serve never binds"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1013]"), "stderr was: {stderr}");
    assert!(stderr.contains("error[L1014]"), "stderr was: {stderr}");
}

/// **The Phase 4 asset-root fix.** A texture-backed gallery scene served with the child's working
/// directory set *away* from the repo root still renders. `serve` roots the engine at the scene
/// file's own directory (the convention `ludion render` already uses), so the scene's
/// `assets/checker.png` resolves from there rather than from the cwd. Before the fix this streamed
/// zero frames — every render failed to load the texture with an `L1101` that surfaced nowhere.
///
/// A `render.screenshot` exercises the exact asset path the frame stream does, so its success proves
/// the fix without reading binary frames. GPU graceful-skip: on an adapterless host the screenshot is
/// an `L2003` (a `SKIPPED` line, still green); the pinned RTX 3090 actually renders and asserts. The
/// load-bearing point is that neither outcome is the old `L1101`.
#[test]
fn needs_gpu_serve_renders_a_texture_backed_scene_from_a_foreign_cwd() {
    let scene =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/sprites.yaml");
    // A working directory that is emphatically not the repo root, so a cwd-rooted asset lookup could
    // never find `assets/checker.png` — only rooting at the scene file's own directory works.
    let cwd = std::env::temp_dir();

    let mut child = spawn_serve_in(
        &cwd,
        &[
            "--scene",
            scene.to_str().unwrap(),
            "--addr",
            "127.0.0.1:0",
            "--paused",
        ],
    );

    let (url, _stdout) = read_listening_url(&mut child);
    let mut client = ludion_client::Client::connect(&url).expect("client connects");

    match client.screenshot(None, None) {
        Ok(shot) => assert!(
            shot.width > 0 && shot.height > 0,
            "a rendered screenshot must have non-zero dimensions"
        ),
        Err(diag) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&diag)) => {
            eprintln!(
                "SKIPPED needs_gpu_serve_renders_a_texture_backed_scene_from_a_foreign_cwd: no GPU adapter"
            );
        }
        Err(diag) => panic!(
            "screenshot failed — a missing-texture L1101 here means the scene's assets did not \
             resolve relative to the scene file (the asset-root fix regressed): {diag}"
        ),
    }

    kill(child);
}

/// **The render extent reaches the hosted engine, from either source, and the flag wins.**
///
/// `--render-size` / `LUDION_RENDER_SIZE` is the per-machine stream-resolution knob, and `serve` is
/// the one verb that applies it (`render` and `dump-graph` keep taking their extent from their own
/// `--width`/`--height`, so no machine's environment can move a golden or a committed sheet). The
/// hosted engine's extent is readable over the wire without a GPU: `render.bounds` echoes it as
/// `render_size`, which is also the number the editor maps CSS pixels through — so this asserts the
/// exact value a picking client would read.
#[test]
fn the_render_extent_reaches_the_hosted_engine_from_the_flag_and_the_environment() {
    let scene = fixture("singletons.yaml");
    let args = [
        "--scene",
        scene.to_str().unwrap(),
        "--addr",
        "127.0.0.1:0",
        "--paused",
    ];

    // The `render_size` a served engine reports over the wire.
    fn hosted_extent(child: &mut Child) -> [u32; 2] {
        let (url, _stdout) = read_listening_url(child);
        let mut client = ludion_client::Client::connect(&url).expect("client connects");
        match client
            .call(ludion_protocol::Command::RenderBounds(Default::default()))
            .expect("render.bounds answers without a GPU")
            .body
        {
            ludion_protocol::ResponseBody::RenderBounds { render_size, .. } => render_size,
            other => panic!("expected RenderBounds, got {other:?}"),
        }
    }

    // The default: no flag, no variable, so the engine's own extent stands. Whatever it is, the two
    // resized cases below have to differ from it or they prove nothing.
    let mut child = spawn_serve(&args);
    let default_extent = hosted_extent(&mut child);
    kill(child);
    assert_ne!(default_extent, [960, 540]);
    assert_ne!(default_extent, [854, 480]);

    let mut flagged = {
        let mut with_flag = vec!["--render-size", "960x540"];
        with_flag.extend_from_slice(&args);
        spawn_serve(&with_flag)
    };
    assert_eq!(
        hosted_extent(&mut flagged),
        [960, 540],
        "`--render-size` must reach the hosted engine",
    );
    kill(flagged);

    let mut from_env = spawn_serve_with_env("LUDION_RENDER_SIZE", "854x480", &args);
    assert_eq!(
        hosted_extent(&mut from_env),
        [854, 480],
        "and so must `LUDION_RENDER_SIZE` — the form `ludion dev` relies on, since it forwards no \
         flags to the game it supervises",
    );
    kill(from_env);

    // The flag beats the variable outright, and is not validated against it: a host's one-run
    // instruction works on a machine whose environment is malformed.
    let mut both = {
        let mut with_flag = vec!["--render-size", "960x540"];
        with_flag.extend_from_slice(&args);
        Command::new(env!("CARGO_BIN_EXE_ludion"))
            .arg("serve")
            .args(&with_flag)
            .env("LUDION_RENDER_SIZE", "not-an-extent")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn `ludion serve`")
    };
    assert_eq!(hosted_extent(&mut both), [960, 540]);
    kill(both);
}

/// A malformed extent refuses to start before `serve` ever binds: non-zero exit, an `L0017` on
/// stderr, and no `listening on` line on stdout. From either source — the environment half is the
/// one that matters, since it is the form a `ludion dev` child inherits.
#[test]
fn a_malformed_render_size_never_binds() {
    let scene = fixture("good.yaml");
    let output = run_ludion(&[
        "serve",
        "--render-size",
        "960×540",
        "--scene",
        scene.to_str().unwrap(),
        "--addr",
        "127.0.0.1:0",
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    assert!(
        output.stdout.is_empty(),
        "nothing may be printed on stdout when serve never binds"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L0017]"), "stderr was: {stderr}");

    let from_env = Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args([
            "serve",
            "--scene",
            scene.to_str().unwrap(),
            "--addr",
            "127.0.0.1:0",
        ])
        .env("LUDION_RENDER_SIZE", "0x0")
        .output()
        .expect("failed to run the ludion binary");
    assert!(!from_env.status.success(), "expected a non-zero exit");
    assert!(from_env.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&from_env.stderr);
    assert!(stderr.contains("error[L0017]"), "stderr was: {stderr}");
    assert!(
        stderr.contains("LUDION_RENDER_SIZE"),
        "stderr was: {stderr}"
    );
}

/// An extent and scale whose product no render can allocate refuse to start too (`L2012`): 8K at
/// scale 2 is 15360 px wide, past the pinned 8192 px texture limit.
#[test]
fn an_extent_the_scale_pushes_past_the_limit_never_binds() {
    let scene = fixture("good.yaml");
    let output = run_ludion(&[
        "serve",
        "--render-size",
        "7680x4320",
        "--scale",
        "2",
        "--scene",
        scene.to_str().unwrap(),
        "--addr",
        "127.0.0.1:0",
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    assert!(
        output.stdout.is_empty(),
        "nothing may be printed on stdout when serve never binds"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L2012]"), "stderr was: {stderr}");
}

/// A missing scene file is the same `scene.load` IO diagnostic `sim-hash` reports (they share the
/// same `check_scene::load_engine` construction path).
#[test]
fn missing_scene_fails_with_scene_io_diagnostic() {
    let output = run_ludion(&[
        "serve",
        "--scene",
        "does-not-exist.yaml",
        "--addr",
        "127.0.0.1:0",
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1015]"), "stderr was: {stderr}");
}
