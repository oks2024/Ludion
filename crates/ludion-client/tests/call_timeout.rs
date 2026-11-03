//! Tests for the opt-in per-call timeout ([`Client::with_call_timeout`]).
//!
//! The single-threaded sim dispatches synchronously, so an infinite-looping game system would hang
//! every connected client forever with no diagnostic. The opt-in timeout bounds a call's wait and
//! surfaces `L3054` on expiry; the default (no timeout) still blocks forever, so no existing caller
//! changes. Covered:
//!
//! - **expiry** — a stub server that completes the handshake but never answers a request; a client with
//!   a short timeout gets `L3054` instead of hanging.
//! - **late replies** — a reply landing after its call timed out is never read as the next call's,
//!   and the sub a timed-out subscribe minted is unsubscribed instead of leaked.
//! - **within budget / default unchanged** — a real server answers well inside a generous timeout, and a
//!   default client (no timeout set) reports `None` for its timeout and still works normally.

use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use ludion_client::Client;
use ludion_core::diagnostics::Diagnostic;
use ludion_engine::Engine;
use ludion_protocol::handshake::ServerHello;
use ludion_protocol::{Command, PROTOCOL_VERSION, Response, ResponseBody, SubId, schema};
use tungstenite::{Message, WebSocket};

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

/// Spawns a one-shot stub that completes the handshake, then runs `script` against the socket.
fn spawn_scripted_server(
    script: impl FnOnce(&mut WebSocket<TcpStream>) + Send + 'static,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub listener");
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (stream, _peer) = listener.accept().expect("accept one connection");
        let mut socket = tungstenite::accept(stream).expect("ws handshake");
        let _hello = socket.read();
        let server_hello = ServerHello {
            engine_version: "0.0.0".into(),
            protocol_version: PROTOCOL_VERSION,
            schema_hash: schema::schema_hash(),
        };
        socket
            .send(Message::text(serde_json::to_string(&server_hello).unwrap()))
            .expect("send ServerHello");
        script(&mut socket);
    });
    format!("ws://{addr}")
}

/// Reads the next request the client sent, as JSON.
fn read_request(socket: &mut WebSocket<TcpStream>) -> serde_json::Value {
    match socket.read().expect("a request arrives") {
        Message::Text(text) => serde_json::from_str(&text).expect("request is JSON"),
        other => panic!("expected a text request, got {other:?}"),
    }
}

fn reply(socket: &mut WebSocket<TcpStream>, response: Response) {
    socket
        .send(Message::text(serde_json::to_string(&response).unwrap()))
        .expect("send reply");
}

/// A reply that lands after its call timed out must not be read as the next call's reply.
#[test]
fn a_late_reply_is_not_read_as_the_next_call() {
    let url = spawn_scripted_server(|socket| {
        let first = read_request(socket);
        thread::sleep(Duration::from_millis(600)); // past the client's timeout
        reply(
            socket,
            Response::ok(first["id"].as_u64().unwrap(), ResponseBody::Ack),
        );
        let second = read_request(socket);
        let body = ResponseBody::Subscribed { sub: SubId::new(5) };
        reply(socket, Response::ok(second["id"].as_u64().unwrap(), body));
        thread::sleep(Duration::from_millis(500));
    });
    let mut client = Client::connect(&url)
        .unwrap()
        .with_call_timeout(Duration::from_millis(300));

    let first = client.call(Command::SimHash).expect_err("call 1 times out");
    assert_eq!(first.code.as_str(), "L3054", "{first}");

    client.set_call_timeout(Some(Duration::from_secs(5)));
    let second = client.call(Command::SimHash).expect("call 2 is answered");
    assert_eq!(second.body, ResponseBody::Subscribed { sub: SubId::new(5) });
}

/// A stub that answers a subscribe late with `answer` (a body, or a refusal), then reports every
/// request the client sends afterwards (until 800 ms of quiet) over the channel. With `ack_next` it
/// first acks the one request that follows.
fn late_subscribe_stub(
    answer: Result<ResponseBody, Diagnostic>,
    requests: mpsc::Sender<serde_json::Value>,
    ack_next: bool,
) -> String {
    spawn_scripted_server(move |socket| {
        let subscribe = read_request(socket);
        assert_eq!(subscribe["method"], "stream.subscribe");
        thread::sleep(Duration::from_millis(500));
        let id = subscribe["id"].as_u64().unwrap();
        let response = match answer {
            Ok(body) => Response::ok(id, body),
            Err(diagnostic) => Response::error(id, diagnostic),
        };
        reply(socket, response);
        if ack_next {
            let next = read_request(socket);
            reply(
                socket,
                Response::ok(next["id"].as_u64().unwrap(), ResponseBody::Ack),
            );
            requests.send(next).unwrap();
        }
        socket
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(800)))
            .unwrap();
        while let Ok(Message::Text(text)) = socket.read() {
            requests.send(serde_json::from_str(&text).unwrap()).unwrap();
        }
    })
}

fn subscribe_that_times_out(client: &mut Client) {
    let error = client
        .subscribe(
            ludion_protocol::Channel::SceneChanged,
            ludion_protocol::SubscribeMode::default(),
            ludion_protocol::frame::FrameFormat::Qoi,
        )
        .expect_err("the subscribe times out");
    assert_eq!(error.code.as_str(), "L3054", "{error}");
}

/// The sub a timed-out subscribe minted is unsubscribed when its reply lands during a later call.
#[test]
fn a_timed_out_subscribe_is_unsubscribed_when_its_reply_lands_in_a_later_call() {
    let (tx, rx) = mpsc::channel();
    let body = ResponseBody::Subscribed { sub: SubId::new(9) };
    let url = late_subscribe_stub(Ok(body), tx, true);
    let mut client = Client::connect(&url)
        .unwrap()
        .with_call_timeout(Duration::from_millis(300));
    subscribe_that_times_out(&mut client);

    client.set_call_timeout(Some(Duration::from_secs(5)));
    client
        .call(Command::SimHash)
        .expect("the later call is answered");

    let _next = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let unsubscribe = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("an unsubscribe is sent");
    assert_eq!(unsubscribe["method"], "stream.unsubscribe");
    assert_eq!(unsubscribe["params"]["sub"], 9);
}

/// Same, when the late reply is read by `next_event` rather than a call.
#[test]
fn a_timed_out_subscribe_is_unsubscribed_when_its_reply_lands_in_next_event() {
    let (tx, rx) = mpsc::channel();
    let body = ResponseBody::Subscribed { sub: SubId::new(9) };
    let url = late_subscribe_stub(Ok(body), tx, false);
    let mut client = Client::connect(&url)
        .unwrap()
        .with_call_timeout(Duration::from_millis(300));
    subscribe_that_times_out(&mut client);

    let event = client.next_event(Duration::from_millis(600)).unwrap();
    assert!(event.is_none());
    let unsubscribe = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("an unsubscribe is sent");
    assert_eq!(unsubscribe["method"], "stream.unsubscribe");
    assert_eq!(unsubscribe["params"]["sub"], 9);
}

/// A timed-out subscribe the host then refused minted nothing, so nothing is sent.
#[test]
fn a_timed_out_subscribe_answered_with_an_error_sends_nothing() {
    let (tx, rx) = mpsc::channel();
    let url = late_subscribe_stub(
        Err(Diagnostic::new(
            ludion_core::codes::code::PROTOCOL_SIM_STALLED,
            "refused",
        )),
        tx,
        false,
    );
    let mut client = Client::connect(&url)
        .unwrap()
        .with_call_timeout(Duration::from_millis(300));
    subscribe_that_times_out(&mut client);

    assert!(
        client
            .next_event(Duration::from_millis(600))
            .unwrap()
            .is_none()
    );
    assert!(
        rx.recv_timeout(Duration::from_millis(1500)).is_err(),
        "no request should follow an error reply"
    );
}

/// A call against a handshaken-but-silent server must fail with `L3054` once the per-call timeout
/// elapses — never hang.
#[test]
fn call_times_out_with_l3054_against_a_silent_server() {
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
        "L3054",
        "a timed-out call surfaces the call-timeout diagnostic: {diag}"
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

    server.shutdown_clean();
}
