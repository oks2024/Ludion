//! A server whose handshake reply disagrees with this build is `L3003` at connect: a drifted schema
//! hash or a newer protocol version fails the connect instead of being accepted.

use std::net::TcpListener;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use ludion_client::Client;
use ludion_protocol::handshake::ServerHello;
use ludion_protocol::{PROTOCOL_VERSION, WireHash, schema};
use tungstenite::Message;

/// A one-shot server that reads the client's hello, answers with `hello`, and closes.
fn stub(hello: ServerHello) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let (stream, _peer) = listener.accept().unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        let _hello = socket.read();
        socket
            .send(Message::text(serde_json::to_string(&hello).unwrap()))
            .unwrap();
        let _ = socket.close(None);
    });
    (url, server)
}

#[test]
fn a_drifted_schema_hash_is_l3003() {
    let (url, server) = stub(ServerHello {
        engine_version: "0.0.0".into(),
        protocol_version: PROTOCOL_VERSION,
        schema_hash: WireHash::from_u64(1),
    });
    let diag = Client::connect_within(&url, Duration::from_secs(5))
        .err()
        .expect("a drifted schema hash fails the connect");
    assert_eq!(diag.code.as_str(), "L3003", "{diag}");
    assert!(diag.message.contains("schema drift"), "{diag}");
    server.join().unwrap();
}

#[test]
fn a_newer_protocol_version_is_l3003() {
    let (url, server) = stub(ServerHello {
        engine_version: "0.0.0".into(),
        protocol_version: PROTOCOL_VERSION + 1,
        schema_hash: schema::schema_hash(),
    });
    let diag = Client::connect_within(&url, Duration::from_secs(5))
        .err()
        .expect("a newer protocol version fails the connect");
    assert_eq!(diag.code.as_str(), "L3003", "{diag}");
    server.join().unwrap();
}
