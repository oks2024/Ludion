//! Tests for the opt-in per-call timeout ([`Client::with_call_timeout`]).
//!
//! The single-threaded sim dispatches synchronously, so an infinite-looping game system would hang
//! every connected client forever with no diagnostic. The opt-in timeout bounds a call's wait and
//! surfaces `L3009` on expiry; the default (no timeout) still blocks forever, so no existing caller
//! changes. Two cases are covered:
//!
//! - **expiry** — a stub server that completes the handshake but never answers a request; a client with
//!   a short timeout gets `L3009` instead of hanging.
//! - **within budget / default unchanged** — a real server answers well inside a generous timeout, and a
//!   default client (no timeout set) reports `None` for its timeout and still works normally.

use std::net::TcpListener;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use ludion_client::Client;
use ludion_engine::Engine;
use ludion_protocol::handshake::ServerHello;
use ludion_protocol::{PROTOCOL_VERSION, schema};
use tungstenite::Message;

const BASE_SCENE: &str = "format: 1\nseed: 7\nentities: []\n";

/// Spawns a one-shot stub WebSocket server that completes the Ludion handshake (so `Client::connect`
/// succeeds) and then **never replies** to anything, holding the connection open for `hold`. Returns the
/// `ws://` URL to connect to. This is the wedged-engine stand-in: the socket is alive, but no reply ever
/// comes — exactly what a client with a call timeout must escape.
fn spawn_silent_server(hold: Duration) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub listener");
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (stream, _peer) = listener.accept().expect("accept one connection");
        let mut socket = tungstenite::accept(stream).expect("ws handshake");
        // Read the client's Hello (the first text message), then answer with a matching ServerHello so
        // the connect-time schema/version check passes.
        let _hello = socket.read();
        let server_hello = ServerHello {
            engine_version: "0.0.0".into(),
            protocol_version: PROTOCOL_VERSION,
            schema_hash: schema::schema_hash(),
        };
        socket
            .send(Message::text(serde_json::to_string(&server_hello).unwrap()))
            .expect("send ServerHello");
        // Now go silent: hold the connection open (never reply to the pending request) so the client's
        // wait must be ended by its own timeout, not by a close.
        thread::sleep(hold);
        let _ = socket.close(None);
    });
    format!("ws://{addr}")
}

/// A call against a handshaken-but-silent server must fail with `L3009` once the per-call timeout
/// elapses — never hang.
#[test]
fn call_times_out_with_l3009_against_a_silent_server() {
    let url = spawn_silent_server(Duration::from_secs(5));
    let mut client = Client::connect(&url)
        .expect("connect succeeds (the stub handshakes)")
        .with_call_timeout(Duration::from_millis(300));

    let start = Instant::now();
    let result = client.sim_hash();
    let elapsed = start.elapsed();

    let diag = result.expect_err("a silent server must make the call time out, not succeed");
    assert_eq!(
        diag.code.as_str(),
        "L3009",
        "a timed-out call surfaces the engine-unavailable/timeout diagnostic: {diag}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "the call must return near its 300ms budget, not hang: took {elapsed:?}"
    );
}

/// The default client has no timeout, and a client *with* a generous timeout still succeeds against a
/// responsive server — the timeout only fires when a reply genuinely fails to arrive.
#[test]
fn default_has_no_timeout_and_a_generous_timeout_still_succeeds() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("client_call_timeout_ok");
    std::fs::create_dir_all(&dir).unwrap();
    let mut engine = Engine::with_project_root(&dir);
    engine.load_inline_scene(BASE_SCENE).unwrap();
    let server = ludion_server::Server::bind(engine, "127.0.0.1:0").expect("server binds");

    // Default: no timeout configured, and a normal call works exactly as before.
    let mut default_client = Client::connect(&server.url()).unwrap();
    assert_eq!(
        default_client.call_timeout(),
        None,
        "the default is block-forever (no timeout)"
    );
    default_client.sim_hash().expect("a default call succeeds");

    // With a generous timeout, a responsive server answers well within budget.
    let mut timed_client = Client::connect(&server.url())
        .unwrap()
        .with_call_timeout(Duration::from_secs(10));
    assert_eq!(timed_client.call_timeout(), Some(Duration::from_secs(10)));
    timed_client
        .sim_hash()
        .expect("a call within the timeout budget succeeds");

    server.shutdown();
}
