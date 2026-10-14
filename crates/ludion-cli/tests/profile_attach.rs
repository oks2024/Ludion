//! Integration tests for `ludion profile --attach <ws-url> ...` and `--perfetto`, driven against
//! the built `ludion` binary.
//!
//! Unlike `tests/serve.rs`'s subprocess-only pattern, the server side here is an **in-process**
//! [`ludion_server::Server`] built with a named game system — the CLI's own `ludion serve` links no
//! game code, so it could never prove the "an attached report carries the running game's own
//! systems" claim `profile.rs`'s docs make. `ludion-server`/`ludion-engine`/`ludion-client` are
//! already ordinary (non-dev) dependencies of this crate (`serve.rs` depends on the first, `dev.rs`
//! and `tests/serve.rs` on the others), so this test file can use them directly, exactly as
//! `crates/ludion-server/tests/perf.rs` does for the same wire capability from the other side.

use std::path::PathBuf;
use std::process::{Command as ProcCommand, Output};

use ludion_engine::Engine;
use ludion_protocol::Command as WireCommand;
use ludion_protocol::command::SimSetSpeedParams;

/// The name the served engine's one system is registered under — the marker every "did the report
/// carry the running game's systems" assertion below looks for.
const SYSTEM: &str = "attach_test_system";

/// A scene with nothing in it: these tests measure the schedule and host spans, not content.
const SCENE: &str = "format: 1\nseed: 7\nentities: []\n";

fn fixture(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(file)
}

fn out_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn run_ludion(args: &[&str]) -> Output {
    ProcCommand::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

/// Binds an in-process server running one named system, small enough to render cheaply (arming the
/// profiler makes the loop render even with no frame subscriber — see `ludion-server`'s perf docs),
/// then paces it fast so a short capture window closes promptly rather than at real-time 60 Hz.
fn serve() -> ludion_server::Server {
    let mut engine: Engine = Engine::builder()
        .render_size(64, 64)
        .system(SYSTEM, |_world, _ctx| {})
        .build()
        .expect("the engine builds");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    let server = ludion_server::Server::bind(engine, "127.0.0.1:0").expect("server binds");

    let mut pacer = ludion_client::Client::connect(&server.url()).expect("pacer connects");
    pacer
        .call(WireCommand::SimSetSpeed(SimSetSpeedParams { speed: 20.0 }))
        .expect("sim speed is accepted");
    drop(pacer);

    server
}

/// **The gap `--attach` closes.** An attached report carries the served engine's own system by
/// name — the offline CLI driver links no game code and can never report this.
#[test]
fn needs_gpu_attach_captures_the_running_hosts_own_systems_and_writes_json() {
    let server = serve();
    let json_out = out_path("attach_report.json");
    let _ = std::fs::remove_file(&json_out);

    let output = run_ludion(&[
        "profile",
        "--attach",
        &server.url(),
        "--frames",
        "10",
        "--json",
        json_out.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let text = std::fs::read_to_string(&json_out).expect("the JSON report was written");
    let report: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(report["measured_frames"], 10);
    assert_eq!(
        report["warmup_frames"], 0,
        "a live capture warms nothing up"
    );
    let systems = report["sim"]["systems"].as_array().expect("systems array");
    assert_eq!(
        systems
            .iter()
            .map(|s| s["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [SYSTEM],
    );

    let _ = std::fs::remove_file(&json_out);
}

/// **Stdout is the report.** With `--perfetto` and a passing `--check`, the status lines go to
/// stderr, so the whole of stdout parses as the one report object.
#[test]
fn needs_gpu_offline_profile_stdout_is_only_the_report() {
    let scene = fixture("good.yaml");
    let trace_out = out_path("stdout_only_trace.json");
    let budget = out_path("stdout_only_budget.toml");
    std::fs::write(&budget, "frame_total_ms = 1000000.0\n").unwrap();

    let output = run_ludion(&[
        "profile",
        "--scene",
        scene.to_str().unwrap(),
        "--frames",
        "5",
        "--perfetto",
        trace_out.to_str().unwrap(),
        "--check",
        budget.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .expect("the whole of stdout is the report JSON");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("wrote Perfetto trace"), "{stderr}");
    assert!(stderr.contains("within budget"), "{stderr}");

    let _ = std::fs::remove_file(&trace_out);
    let _ = std::fs::remove_file(&budget);
}

/// `--check` runs over an attached report exactly as it does over an offline one: a near-zero
/// `frame_total_ms` ceiling must be exceeded, `L4005` on stderr, non-zero exit.
#[test]
fn needs_gpu_attach_check_flags_an_over_budget_line() {
    let server = serve();
    let budget = out_path("attach_budget.toml");
    std::fs::write(&budget, "frame_total_ms = 0.0\n").unwrap();

    let output = run_ludion(&[
        "profile",
        "--attach",
        &server.url(),
        "--frames",
        "10",
        "--check",
        budget.to_str().unwrap(),
    ]);
    assert!(
        !output.status.success(),
        "a 0.0 ms frame_total_ms budget must be exceeded by any real frame"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L4005"), "{stderr}");

    let _ = std::fs::remove_file(&budget);
}

/// **`--perfetto` on an attach capture.** The written file parses as JSON and carries the `sim
/// systems`/`host` lanes' `thread_name` metadata (GPU/transport are host-dependent and not asserted
/// here — an adapterless CI box legitimately omits them, per `perfetto.rs`'s degrade rule).
#[test]
fn needs_gpu_attach_perfetto_writes_a_trace_with_the_expected_lanes() {
    let server = serve();
    let trace_out = out_path("attach_trace.json");
    let _ = std::fs::remove_file(&trace_out);

    let output = run_ludion(&[
        "profile",
        "--attach",
        &server.url(),
        "--frames",
        "10",
        "--perfetto",
        trace_out.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let text = std::fs::read_to_string(&trace_out).expect("the trace file was written");
    let trace: serde_json::Value =
        serde_json::from_str(&text).expect("the trace file is valid JSON");
    let events = trace["traceEvents"]
        .as_array()
        .expect("a traceEvents array");
    assert!(!events.is_empty());

    let lane_names: Vec<&str> = events
        .iter()
        .filter(|e| e["ph"] == "M")
        .filter_map(|e| e["args"]["name"].as_str())
        .collect();
    assert!(lane_names.contains(&"sim systems"), "{lane_names:?}");
    assert!(lane_names.contains(&"host"), "{lane_names:?}");

    let x_events: Vec<&serde_json::Value> = events.iter().filter(|e| e["ph"] == "X").collect();
    assert!(!x_events.is_empty(), "at least one span must be present");
    assert!(
        x_events.iter().any(|e| e["name"] == SYSTEM),
        "a sim span named after the served system must appear: {x_events:?}"
    );

    let _ = std::fs::remove_file(&trace_out);
}

/// **Offline `--perfetto` now succeeds.** The offline driver fills the report's own per-frame
/// samples (the integration-fix wave this test was rewritten for), so `--scene` + `--perfetto` needs
/// no `--attach` — the host lane's `frame_total` spans, one per measured frame, are the proof. The
/// built-in registry links no game systems, so the offline trace's "sim systems" lane is legitimately
/// absent (unlike the attach case above, which names its running system) — that absence is asserted
/// here too, so this test cannot pass by accident on a report that happens to carry a stray lane.
#[test]
fn needs_gpu_offline_perfetto_writes_a_trace_with_the_host_lane() {
    let scene = fixture("good.yaml");
    let trace_out = out_path("offline_trace.json");
    let _ = std::fs::remove_file(&trace_out);

    let output = run_ludion(&[
        "profile",
        "--scene",
        scene.to_str().unwrap(),
        "--frames",
        "5",
        "--perfetto",
        trace_out.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let text = std::fs::read_to_string(&trace_out).expect("the trace file was written");
    let trace: serde_json::Value =
        serde_json::from_str(&text).expect("the trace file is valid JSON");
    let events = trace["traceEvents"]
        .as_array()
        .expect("a traceEvents array");
    assert!(!events.is_empty());

    let lane_names: Vec<&str> = events
        .iter()
        .filter(|e| e["ph"] == "M")
        .filter_map(|e| e["args"]["name"].as_str())
        .collect();
    assert!(lane_names.contains(&"host"), "{lane_names:?}");
    assert!(
        !lane_names.contains(&"sim systems"),
        "the built-in registry links no game systems: {lane_names:?}"
    );

    let frame_totals: Vec<&serde_json::Value> = events
        .iter()
        .filter(|e| e["ph"] == "X" && e["name"] == "frame_total")
        .collect();
    assert_eq!(
        frame_totals.len(),
        5,
        "one frame_total span per measured frame"
    );

    let _ = std::fs::remove_file(&trace_out);
}

/// **Clap conflict matrix: `--attach` + `--scene`.** Mutually exclusive — there is no scene file to
/// load once a running host is being profiled.
#[test]
fn attach_and_scene_are_mutually_exclusive() {
    let scene = fixture("good.yaml");
    let output = run_ludion(&[
        "profile",
        "--scene",
        scene.to_str().unwrap(),
        "--attach",
        "ws://127.0.0.1:1",
        "--frames",
        "1",
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot be used with"),
        "expected a clap conflict message, got: {stderr}"
    );
}

/// **Clap conflict matrix: `--attach` + `--scale`.** Mutually exclusive — there is no render-scale
/// override to make against a host that is already rendering at whatever scale it runs.
#[test]
fn attach_and_scale_are_mutually_exclusive() {
    let output = run_ludion(&[
        "profile",
        "--attach",
        "ws://127.0.0.1:1",
        "--scale",
        "2.0",
        "--frames",
        "1",
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot be used with"),
        "expected a clap conflict message, got: {stderr}"
    );
}

/// **Clap conflict matrix: neither `--scene` nor `--attach`.** Rejected before any engine work
/// starts.
#[test]
fn missing_both_scene_and_attach_is_rejected() {
    let output = run_ludion(&["profile", "--frames", "1"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("required") && stderr.contains("scene"),
        "expected a clap required-argument message naming `scene`, got: {stderr}"
    );
}

/// **Clap conflict matrix: `--attach` alone is a valid combination.** An address nothing listens on
/// gets past clap parsing cleanly — the failure that follows is `Client::connect`'s own `L3009`
/// diagnostic, not a usage error, proving `--attach` alone (no `--scene`) is accepted.
#[test]
fn attach_alone_is_a_valid_combination_and_fails_at_connect_time_not_parse_time() {
    let output = run_ludion(&["profile", "--attach", "ws://127.0.0.1:1", "--frames", "1"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L3009"), "{stderr}");
    assert!(
        !stderr.contains("cannot be used with") && !stderr.contains("required"),
        "a connect-time failure must not read as a clap usage error: {stderr}"
    );
}
