//! Cross-process reconnect test for `ludion-window`'s stream loop (Phase 7 Wave 4, the native-window
//! half of decision 6).
//!
//! It drives the **actual reconnect module** — `ludion_window::run_frame_stream` — against a real
//! `ludion serve` it kills and restarts on the **same port**, asserting that re-subscription resumes
//! after the restart, all **without opening a window** (the loop is factored to run headlessly behind
//! the `StreamSink` trait). This is exactly the blink `ludion dev --window` puts the window through on
//! every reload: the socket drops for the seconds between the old child's shutdown and the new child's
//! listen, and the window is supposed to ride it out rather than die.
//!
//! Frame *delivery* needs a GPU, so the load-bearing assertions are on the reconnect lifecycle
//! (`reconnecting` then a second `reconnected`), which a `frames` subscription reports with or without
//! an adapter. When a GPU is present, resumed frames are asserted as a bonus; otherwise that check is
//! skipped, never failed — the same graceful-skip contract the other serve GPU tests use.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use ludion_window::{DecodedFrame, StreamSink};

/// The scene the serve hosts — a trivial, texture-free scene so the test never depends on assets.
fn good_scene() -> String {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("good.yaml")
        .to_string_lossy()
        .into_owned()
}

/// A `StreamSink` that just counts what the reconnect loop reported. Shared across the test thread and
/// the stream thread, so every field is atomic.
#[derive(Default)]
struct RecordingSink {
    frames: AtomicU64,
    reconnecting: AtomicU64,
    reconnected: AtomicU64,
    running: AtomicBool,
}

impl RecordingSink {
    fn started() -> Arc<RecordingSink> {
        let sink = RecordingSink::default();
        sink.running.store(true, Ordering::Relaxed);
        Arc::new(sink)
    }
    fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

impl StreamSink for RecordingSink {
    fn frame(&self, _frame: DecodedFrame) {
        self.frames.fetch_add(1, Ordering::Relaxed);
    }
    fn reconnecting(&self) {
        self.reconnecting.fetch_add(1, Ordering::Relaxed);
    }
    fn reconnected(&self) {
        self.reconnected.fetch_add(1, Ordering::Relaxed);
    }
    fn keep_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }
}

/// Spawns `ludion serve --scene good --addr <addr> --paused` with piped stdout, returning the child
/// and the `ws://…` URL it announced. `None` if the child never printed a listening line (e.g. a
/// same-port bind race lost) — the caller retries.
fn spawn_serve(addr: &str) -> Option<(Child, String)> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("serve")
        .arg("--scene")
        .arg(good_scene())
        .arg("--addr")
        .arg(addr)
        .arg("--paused")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn `ludion serve`");

    let stdout = child.stdout.take().expect("serve stdout piped");
    match read_listening_url(stdout) {
        Some(url) => Some((child, url)),
        None => {
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}

/// Reads the mandatory `listening on ws://…` line off a just-spawned serve's stdout (draining the
/// rest of the pipe on a background thread so the child never stalls on a full pipe). Returns the URL,
/// or `None` if the stream closed before a listening line arrived (the child failed to bind).
fn read_listening_url(stdout: ChildStdout) -> Option<String> {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let url = loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return None, // EOF before listening: bind failed
            Ok(_) => {
                if let Some(rest) = line.trim().strip_prefix("listening on ")
                    && rest.starts_with("ws://")
                {
                    break rest.to_string();
                }
            }
            Err(_) => return None,
        }
    };
    // Keep draining so the still-running child's later stdout writes never block.
    std::thread::spawn(move || {
        let mut sink = String::new();
        while reader.read_line(&mut sink).map(|n| n != 0).unwrap_or(false) {
            sink.clear();
        }
    });
    Some(url)
}

/// Spawns a serve on `addr`, retrying a few times so a same-port `TIME_WAIT` race (right after a
/// graceful stop) clears rather than flaking the test — the same bind-retry idea `ludion dev` uses.
fn spawn_serve_retrying(addr: &str) -> (Child, String) {
    for _ in 0..400 {
        if let Some(pair) = spawn_serve(addr) {
            return pair;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("serve never bound {addr} within the retry window");
}

/// Gracefully stops a serve over the wire (`engine.shutdown`, the clean path that frees the port),
/// then waits for the process to exit.
fn shutdown_serve(mut child: Child, url: &str) {
    if let Ok(mut client) = ludion_client::Client::connect(url) {
        let _ = client.call(ludion_protocol::Command::EngineShutdown);
    }
    for _ in 0..200 {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Runs the reconnect stream loop on a worker thread against `url`, driving `sink`.
fn spawn_stream(
    url: String,
    sink: Arc<RecordingSink>,
) -> JoinHandle<Option<ludion_core::diagnostics::Diagnostic>> {
    std::thread::spawn(move || ludion_window::run_frame_stream(&url, sink.as_ref()))
}

/// Polls `value` until it reaches `target`, or panics after ~15 s — so a missing reconnect fails
/// loudly instead of hanging. (Poll count over `sleep`, not a wall clock: the crate's determinism
/// denylist bans `Instant::now` even in tests.)
fn wait_until_at_least(value: &AtomicU64, target: u64, label: &str) {
    for _ in 0..600 {
        if value.load(Ordering::Relaxed) >= target {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!(
        "timed out waiting for `{label}` to reach {target} (last {})",
        value.load(Ordering::Relaxed)
    );
}

#[test]
fn needs_gpu_stream_resubscribes_after_a_same_port_serve_restart() {
    // Serve #1 on an OS-chosen port; learn the concrete address so serve #2 can rebind it exactly.
    let (serve1, url) = spawn_serve_retrying("127.0.0.1:0");
    let addr = url.strip_prefix("ws://").expect("a ws:// url").to_string();

    let sink = RecordingSink::started();
    let stream = spawn_stream(url.clone(), sink.clone());

    // The initial connect + `frames` subscription succeeds: one `reconnected`.
    wait_until_at_least(&sink.reconnected, 1, "initial reconnected");
    let frames_before = sink.frames.load(Ordering::Relaxed);

    // The blink: stop serve #1 cleanly (frees the port). The stream loses its connection and starts
    // retrying — it must flag `reconnecting`, not give up.
    shutdown_serve(serve1, &url);
    wait_until_at_least(&sink.reconnecting, 1, "reconnecting after the server stops");

    // Bring serve #2 up on the SAME port. The stream must re-establish and re-subscribe on its own.
    let (serve2, url2) = spawn_serve_retrying(&addr);
    assert_eq!(url2, url, "serve #2 must rebind the exact same port");
    wait_until_at_least(&sink.reconnected, 2, "reconnected after the restart");

    // Bonus, only when a GPU actually renders frames: resumed frames prove the subscription is live
    // again, not merely reconnected. Adapterless hosts stream nothing, so skip rather than fail.
    for _ in 0..400 {
        if sink.frames.load(Ordering::Relaxed) > frames_before {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    if sink.frames.load(Ordering::Relaxed) <= frames_before {
        ludion_core::require_gpu::skip_no_adapter(true);
        eprintln!("SKIPPED resumed-frames assertion: no frames delivered (likely no GPU adapter)");
    }

    // Stop the stream loop and join it, then stop serve #2. The clean stop returns `None` (no fatal).
    sink.stop();
    let outcome = stream.join().expect("stream thread joins");
    assert!(
        outcome.is_none(),
        "a clean stop must not surface a fatal diagnostic, got {outcome:?}",
    );
    shutdown_serve(serve2, &url);
    let _ = std::io::stdout().flush();
}
