//! An inbound message over the wire ceiling is `L3045`, and the connection is not reused: the
//! rejection happens on the frame header, mid-frame, so the stream cannot be resynchronised.
//!
//! The stub writes only a header that announces a body one byte past the ceiling — no allocation,
//! because the client rejects on the announced length alone.

use std::io::Write;
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use ludion_client::Client;
use ludion_core::codes::code;
use ludion_protocol::handshake::ServerHello;
use ludion_protocol::{Command, MAX_MESSAGE_BYTES, PROTOCOL_VERSION, schema};
use tungstenite::Message;

/// A stub that handshakes, writes the over-limit header and holds the socket open.
fn spawn_over_limit_server() -> String {
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
        // FIN + binary, a 64-bit length one byte over the ceiling, then a few payload bytes.
        let mut raw = vec![0x82, 127];
        raw.extend_from_slice(&(MAX_MESSAGE_BYTES as u64 + 1).to_be_bytes());
        raw.extend_from_slice(&[0; 8]);
        let tcp = socket.get_mut();
        tcp.write_all(&raw).expect("write the over-limit header");
        tcp.flush().unwrap();
        thread::sleep(Duration::from_secs(10));
    });
    format!("ws://{addr}")
}

#[test]
fn an_over_limit_message_ends_the_connection() {
    let url = spawn_over_limit_server();
    let mut client = Client::connect(&url).expect("connect");
    let error = client
        .next_frame(Duration::from_secs(5))
        .expect_err("the message is over the ceiling");
    assert_eq!(error.code, code::PROTOCOL_MESSAGE_TOO_LARGE);

    let after = client
        .check_open()
        .expect_err("a connection left mid-frame must not read as open");
    assert_eq!(after.code, code::PROTOCOL_ENGINE_UNAVAILABLE);

    let call = client
        .call(Command::SimPause)
        .expect_err("a call on an ended connection is refused before writing");
    assert_eq!(call.code, code::PROTOCOL_ENGINE_UNAVAILABLE);
}
