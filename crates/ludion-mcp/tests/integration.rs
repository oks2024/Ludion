//! Cross-process integration test: real `ludion serve` + real `ludion-mcp`, driven over stdio.
//!
//! This is the Phase 5 wiring proof. It spawns `ludion serve --scene <gallery> --paused` (reusing
//! the readiness pattern from `crates/ludion-cli/tests/serve.rs`: `--addr 127.0.0.1:0` for an
//! OS-chosen port, parse the mandatory `listening on ws://…` first stdout line), then spawns the
//! `ludion-mcp` binary pointed at that URL and speaks MCP over its stdin/stdout:
//! `initialize` → `tools/list` → `tools/call sim_step` → `tools/call sim_hash`, asserting a
//! well-formed WorldHash comes back through the adapter. No GPU is needed (sim.step/sim.hash never
//! render).
//!
//! `ludion` lives in a different package (`ludion-cli`), so `CARGO_BIN_EXE_ludion` is not set for
//! this test (cargo sets it only for the package that defines the bin; reaching another package's
//! bin needs artifact dependencies, which are unstable). Instead the test locates the compiled
//! binary next to its own test executable (same profile dir) and spawns it directly, so
//! `Child::kill` reaps the actual server (killing a `cargo run` wrapper would orphan it on Windows).

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

/// The gallery scene served for the test (loads without a GPU).
fn gallery_scene() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/sprites.yaml")
}

/// Path to the `ludion` binary, which sits in this test executable's own profile directory.
///
/// An existing binary is used as-is, never rebuilt: under the workspace test lane the
/// `crates/ludion-cli/tests/*.rs` smoke tests are running that same file concurrently, and a
/// rebuild that decides it is stale then fails to replace a running executable on Windows
/// (`os error 5`). A stale binary is impossible under that lane — the workspace test run builds
/// every bin before any test starts. The build below is the standalone `cargo test -p ludion-mcp`
/// path only, where the binary is missing and therefore cannot be running anywhere.
fn ludion_binary() -> PathBuf {
    // .../target/<profile>/deps/<test>-<hash>(.exe)  ->  profile dir is two levels up. Derived from
    // the test executable, so a `CARGO_TARGET_DIR` override is followed for free.
    let test_exe = std::env::current_exe().expect("current exe path");
    let profile_dir = test_exe
        .parent()
        .and_then(|deps| deps.parent())
        .expect("profile dir above deps/")
        .to_path_buf();
    let bin = profile_dir.join(format!("ludion{}", std::env::consts::EXE_SUFFIX));
    if bin.exists() {
        return bin;
    }

    let mut build = Command::new(env!("CARGO"));
    build.args(["build", "-p", "ludion-cli", "--bin", "ludion"]);
    if profile_dir.file_name().and_then(|s| s.to_str()) == Some("release") {
        build.arg("--release");
    }
    let status = build.status().expect("run `cargo build -p ludion-cli`");
    assert!(status.success(), "building the `ludion` binary failed");
    assert!(
        bin.exists(),
        "`ludion` binary not found at {}",
        bin.display()
    );
    bin
}

/// Spawns `ludion serve` and returns the child plus the parsed `ws://…` URL (holding the stdout
/// reader open so the child never hits a broken pipe on its human-readable block).
fn spawn_serve(scene: &PathBuf) -> (Child, String, BufReader<ChildStdout>) {
    let mut child = Command::new(ludion_binary())
        .args(["serve", "--scene"])
        .arg(scene)
        .args(["--addr", "127.0.0.1:0", "--paused"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn `ludion serve`");

    let stdout = child.stdout.take().expect("serve stdout piped");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .expect("read serve's `listening on` line");
    let url = line
        .trim()
        .strip_prefix("listening on ")
        .unwrap_or_else(|| panic!("serve's first line was not machine-parseable: {line:?}"))
        .to_string();
    assert!(url.starts_with("ws://"), "not a ws:// URL: {url}");
    (child, url, reader)
}

/// A live MCP conversation over the `ludion-mcp` child's stdio.
struct Mcp {
    child: Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Mcp {
    /// Spawns `ludion-mcp --url <url>` with piped stdio (stderr inherited so its logs are visible
    /// on a failure).
    fn spawn(url: &str) -> Mcp {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ludion-mcp"))
            .args(["--url", url])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn `ludion-mcp`");
        let stdin = child.stdin.take().expect("mcp stdin piped");
        let stdout = BufReader::new(child.stdout.take().expect("mcp stdout piped"));
        Mcp {
            child,
            stdin,
            stdout,
        }
    }

    /// Sends one JSON-RPC request and reads the single response line it produces.
    fn request(&mut self, message: Value) -> Value {
        let line = serde_json::to_string(&message).expect("serialize request");
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|()| self.stdin.write_all(b"\n"))
            .and_then(|()| self.stdin.flush())
            .expect("write request to mcp stdin");

        let mut response = String::new();
        self.stdout
            .read_line(&mut response)
            .expect("read mcp response line");
        assert!(
            !response.trim().is_empty(),
            "mcp closed stdout unexpectedly"
        );
        serde_json::from_str(&response).expect("mcp response is JSON")
    }

    /// Calls a tool and returns its result value (`{content, isError}`).
    fn call_tool(&mut self, id: i64, name: &str, arguments: Value) -> Value {
        let resp = self.request(json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": name, "arguments": arguments },
        }));
        assert!(resp.get("error").is_none(), "tools/call errored: {resp}");
        resp["result"].clone()
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A WorldHash on the wire is a 16-character lowercase-hex string.
fn assert_world_hash(hash: &str) {
    assert_eq!(hash.len(), 16, "WorldHash must be 16 hex chars: {hash:?}");
    assert!(
        hash.chars().all(|c| c.is_ascii_hexdigit()),
        "WorldHash must be hex: {hash:?}",
    );
}

/// Extracts the first text content block of a tool result and parses it as JSON.
fn text_json(result: &Value) -> Value {
    let text = result["content"]
        .as_array()
        .and_then(|blocks| blocks.iter().find(|b| b["type"] == json!("text")))
        .and_then(|b| b["text"].as_str())
        .unwrap_or_else(|| panic!("no text content block in {result}"));
    serde_json::from_str(text).unwrap_or_else(|e| panic!("text block was not JSON ({e}): {text}"))
}

#[test]
fn mcp_drives_a_served_engine_end_to_end() {
    let scene = gallery_scene();
    let (mut serve, url, _serve_stdout) = spawn_serve(&scene);
    let mut mcp = Mcp::spawn(&url);

    // initialize — the adapter answers without contacting the engine yet.
    let init = mcp.request(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } },
    }));
    assert_eq!(init["result"]["serverInfo"]["name"], json!("ludion-mcp"));

    // tools/list — one tool per protocol method, plus ludion_status, minus the two stream methods
    // (subscription events cannot be delivered over request/response MCP, so they get no tool).
    let list = mcp.request(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
    let tools = list["result"]["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), ludion_protocol::Command::METHODS.len() + 1 - 2);
    assert!(
        tools.iter().all(|t| t["name"] != json!("stream_subscribe")),
        "stream_subscribe must not be exposed over MCP",
    );
    assert!(
        tools.iter().any(|t| t["name"] == json!("sim_hash")),
        "sim_hash tool must be present",
    );
    assert!(
        tools
            .iter()
            .all(|t| t["inputSchema"]["type"] == json!("object")),
        "every tool has an object inputSchema",
    );

    // tools/call sim_step {ticks: 5, hash: true} — the first call connects lazily. A system-free
    // world is frame-invariant, so the returned hash is valid regardless of tick count.
    let stepped = mcp.call_tool(3, "sim_step", json!({ "ticks": 5, "hash": true }));
    assert_eq!(stepped["isError"], json!(false), "sim_step: {stepped}");
    let stepped_body = text_json(&stepped);
    assert_eq!(stepped_body["kind"], json!("stepped"));
    assert_world_hash(
        stepped_body["hash"]
            .as_str()
            .expect("sim_step with hash:true returns a hash"),
    );

    // tools/call sim_hash — the determinism primitive, straight through the adapter.
    let hashed = mcp.call_tool(4, "sim_hash", json!({}));
    assert_eq!(hashed["isError"], json!(false), "sim_hash: {hashed}");
    let hashed_body = text_json(&hashed);
    assert_eq!(hashed_body["kind"], json!("sim_hash"));
    assert_world_hash(
        hashed_body["hash"]
            .as_str()
            .expect("sim_hash returns a hash"),
    );

    // Both hashes describe the same system-free world, so they agree — proof the step and the hash
    // rode the same live engine, not a stub.
    assert_eq!(
        stepped_body["hash"], hashed_body["hash"],
        "stepping a system-free world must not change its hash",
    );

    // Tear down: drop the MCP child (its `Drop` kills it), then hard-kill serve. `_serve_stdout` is
    // held open until here so serve never hit a broken pipe on its human-readable startup block.
    drop(mcp);
    let _ = serve.kill();
    let _ = serve.wait();
    drop(_serve_stdout);
}
