//! A server's unparseable message is `L3051`, not the request-envelope `L3001`: a stub that answers
//! the handshake with garbage fails the connect, and one that answers a request with garbage fails
//! the call.

use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use ludion_client::Client;
use ludion_protocol::handshake::ServerHello;
use ludion_protocol::{PROTOCOL_VERSION, schema};
use tungstenite::Message;

#[test]
fn a_garbage_handshake_reply_is_l3051() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let (stream, _peer) = listener.accept().unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        let _hello = socket.read();
        socket.send(Message::text("not json")).unwrap();
        let _ = socket.close(None);
    });

    let diag = Client::connect_within(&url, Duration::from_secs(5))
        .err()
        .expect("a garbage handshake reply fails the connect");
    assert_eq!(diag.code.as_str(), "L3051", "{diag}");
    server.join().unwrap();
}

#[test]
fn a_garbage_response_is_l3051() {
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
        socket.send(Message::text("not json")).unwrap();
        let _ = socket.close(None);
    });

    let mut client = Client::connect_within(&url, Duration::from_secs(5))
        .expect("the stub completes the handshake");
    let diag = client.sim_hash().expect_err("the reply is not JSON");
    assert_eq!(diag.code.as_str(), "L3051", "{diag}");
    server.join().unwrap();
}
