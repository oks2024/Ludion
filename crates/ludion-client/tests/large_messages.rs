//! A binary frame above tungstenite's default 16 MiB message limit still arrives intact.
//!
//! A 2400×1800 raw RGBA frame is 17.28 MB; the default limit would fail the read and surface a
//! lost connection (`L3009`). The oracle is the byte generator in this file, not the client.

use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use ludion_client::Client;
use ludion_core::time::FrameIndex;
use ludion_protocol::frame::{FrameFormat, FrameHeader};
use ludion_protocol::handshake::ServerHello;
use ludion_protocol::{PROTOCOL_VERSION, SubId, schema};
use tungstenite::Message;

const WIDTH: u32 = 2400;
const HEIGHT: u32 = 1800;

fn payload() -> Vec<u8> {
    (0..WIDTH as usize * HEIGHT as usize * 4)
        .map(|i| (i % 251) as u8)
        .collect()
}

/// A stub that handshakes, then pushes one large raw frame and holds the socket open.
fn spawn_frame_server() -> String {
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
        let header = FrameHeader {
            sub: SubId::new(1),
            frame: FrameIndex::new(3),
            format: FrameFormat::RawRgba,
            width: WIDTH,
            height: HEIGHT,
            gi_stale: false,
        };
        socket
            .send(Message::binary(header.encode(&payload())))
            .expect("send the large frame");
        thread::sleep(Duration::from_secs(10));
    });
    format!("ws://{addr}")
}

#[test]
fn a_frame_over_the_default_message_limit_arrives_intact() {
    let url = spawn_frame_server();
    let mut client = Client::connect(&url).expect("connect");
    let frame = client
        .next_frame(Duration::from_secs(20))
        .expect("the large frame is read, not a lost connection")
        .expect("a frame arrives");
    assert_eq!(frame.header.width, WIDTH);
    assert_eq!(frame.header.height, HEIGHT);
    assert_eq!(frame.payload, payload());
}
