//! Tests for the bounded connect handshake ([`Client::connect_within`]).
//!
//! A server that accepts a TCP connection and then goes quiet — a suspended or wedged engine — must
//! turn `connect` into `L3009`, not a forever hang, at either handshake stage; and the bound must not
//! leak into the connected client's later calls.
//!
//! Both stall stages produce the same `L3009` message, so the client cannot tell which one it hit; the
//! stub reports the stage it reached, and the hello test asserts it got past the upgrade. A stub that
//! must answer a stage in time gets [`SERVED_BOUND`], so a saturated box cannot turn the test into a
//! vacuous upgrade-stage timeout.

use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use ludion_client::Client;
use ludion_protocol::handshake::ServerHello;
use ludion_protocol::{PROTOCOL_VERSION, schema};
use tungstenite::Message;

/// The bound where nothing is supposed to succeed.
const BOUND: Duration = Duration::from_millis(300);

/// The bound where the stub must answer a stage in time: its thread's scheduling allowance on a
/// saturated box.
const SERVED_BOUND: Duration = Duration::from_secs(2);

/// How far a stub got before the client let go.
#[derive(Debug, PartialEq, Eq)]
enum Stage {
    Accepted,
    Upgraded,
    HelloRead,
}

fn assert_stalled(url: &str, bound: Duration) {
    let start = Instant::now();
    let diag = Client::connect_within(url, bound)
        .err()
        .expect("a silent server must fail the connect");
    assert_eq!(diag.code.as_str(), "L3009", "{diag}");
    assert!(
        diag.message.contains("did not complete the handshake"),
        "{diag}"
    );
    assert!(
        start.elapsed() < bound + Duration::from_secs(5),
        "the connect must return near its bound, took {:?}",
        start.elapsed()
    );
}

/// A listener that accepts and never writes: the WebSocket upgrade read is what the bound ends.
#[test]
fn a_listener_that_never_answers_the_upgrade_is_l3009() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (release, released) = mpsc::channel::<()>();
    let holder = thread::spawn(move || {
        let accepted = listener.accept();
        let _ = released.recv();
        drop(accepted);
    });
    assert_stalled(&url, BOUND);
    drop(release);
    holder.join().unwrap();
}

/// A server that completes the WebSocket upgrade but never sends a `ServerHello`.
#[test]
fn a_server_that_never_sends_its_hello_is_l3009() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (release, released) = mpsc::channel::<()>();
    let holder = thread::spawn(move || {
        let Ok((stream, _peer)) = listener.accept() else {
            return Stage::Accepted;
        };
        let Ok(mut socket) = tungstenite::accept(stream) else {
            return Stage::Accepted;
        };
        if socket.read().is_err() {
            return Stage::Upgraded;
        }
        let _ = released.recv();
        let _ = socket.close(None);
        Stage::HelloRead
    });
    assert_stalled(&url, SERVED_BOUND);
    drop(release);
    assert_eq!(
        holder.join().unwrap(),
        Stage::HelloRead,
        "the client gave up before the upgrade completed: this stalled at the upgrade stage, not the \
         hello stage"
    );
}

/// A handshaken client keeps no leftover socket timeout: a reply that arrives after the connect bound
/// has elapsed is still received, and the client reports no call timeout.
#[test]
fn a_connected_client_is_not_cut_short_by_the_handshake_bound() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let (stream, _peer) = listener.accept().unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        let _hello = socket.read();
        let hello = ServerHello {
            engine_version: "0.0.0".into(),
            protocol_version: PROTOCOL_VERSION,
            schema_hash: schema::schema_hash(),
        };
        socket
            .send(Message::text(serde_json::to_string(&hello).unwrap()))
            .unwrap();
        let _request = socket.read();
        // A sleep is a lower bound: the reply is always later than the connect bound.
        thread::sleep(SERVED_BOUND + Duration::from_millis(500));
        let _ = socket.send(Message::text(
            r#"{"id":1,"error":{"code":"L3001","message":"stub reply"}}"#,
        ));
        let _ = socket.close(None);
    });

    let mut client =
        Client::connect_within(&url, SERVED_BOUND).expect("the stub completes the handshake");
    assert_eq!(client.call_timeout(), None);
    let diag = client
        .sim_hash()
        .expect_err("the stub answers with an error");
    assert_eq!(
        diag.code.as_str(),
        "L3001",
        "the reply must arrive, not be cut short by a leftover socket timeout: {diag}"
    );
    server.join().unwrap();
}
