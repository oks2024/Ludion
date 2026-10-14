//! The bounded-reply-age gate: a client that sends faster than the host's pass rate must not
//! accumulate lag.
//!
//! The symptom this exists to catch is the editor's: it injects input at display rate while the host
//! runs slower, so under a read-one-answer-one request loop every reply is older than the last and
//! key-to-pixel lag climbs without bound. The server answers it by reading continuously into a
//! bounded pipeline — the sim drains every pending command at each pass boundary, so a whole burst is
//! answered within about one pass however many requests it holds.
//!
//! The measurement is the reply's own `frame` stamp, never a wall clock: with the engine paused, the
//! frame index only moves inside the load client's `sim.step`, so the spread of a burst's stamps *is*
//! the number of passes it took to answer. No GPU, no sleeps, nothing to be flaky about.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use ludion_client::Client;
use ludion_engine::Engine;
use ludion_protocol::Command;
use ludion_protocol::command::SimStepParams;
use serde_json::json;
use tungstenite::Message;

/// The empty, seeded base scene the host serves.
const BASE_SCENE: &str = "format: 1\nseed: 7\nentities: []\n";

/// Starts a server on a free localhost port and returns it.
fn serve() -> ludion_server::Server {
    let mut engine = Engine::new();
    engine.load_inline_scene(BASE_SCENE).unwrap();
    ludion_server::Server::bind(engine, "127.0.0.1:0").expect("server binds")
}

type RawSocket = tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>;

/// Opens a raw WebSocket and performs the hello handshake — raw because the typed [`Client`] is
/// one-call-at-a-time by construction, and bursting before reading is the whole point here.
fn raw_connect(url: &str) -> RawSocket {
    let (mut socket, _response) = tungstenite::connect(url).unwrap();
    let hello = json!({ "protocol_version": 1, "client": "pipelining-test", "features": [] });
    socket.send(Message::text(hello.to_string())).unwrap();
    let reply = read_text(&mut socket);
    assert!(
        reply.contains("schema_hash"),
        "expected a ServerHello, got {reply}"
    );
    socket
}

/// Reads the next text message from a raw socket.
fn read_text(socket: &mut RawSocket) -> String {
    loop {
        match socket.read().unwrap() {
            Message::Text(text) => return text.as_str().to_string(),
            Message::Close(_) => return String::new(),
            _ => continue,
        }
    }
}

/// **A pipelined burst is answered within about one pass, not one pass per request.**
///
/// Connection B keeps the sim busy with back-to-back `sim.step`s; connection A writes its whole burst
/// before reading a single reply — the shape a fast injector produces. Every reply lands in the same
/// drain (or the one after it), so the spread of their frame stamps is a pass or two. The serial loop
/// this replaced would have cost a step per request or two, spreading the stamps by tens of passes.
#[test]
fn a_pipelined_burst_is_answered_within_two_passes() {
    // One pass's worth of frames. Sized so a step's wall time dominates the burst's: the assertion
    // holds as long as the burst lands inside one or two steps, and a step of this many ticks on an
    // empty scene is milliseconds against the burst's microseconds of loopback writes.
    const TICKS: u64 = 100_000;
    // Comfortably inside the connection's in-flight bound, so this measures reply age and not the
    // backpressure stall (which `connection.rs`'s own tests cover).
    const BURST: u64 = 40;

    let server = serve();
    let url = server.url();

    // B, the load. Pause first: from here the frame index moves only inside B's own steps, so every
    // stamp below is a deterministic multiple of TICKS rather than a wall-clock reading.
    let mut busy = Client::connect(&url).expect("the load client connects");
    busy.call(Command::SimPause).expect("the engine pauses");

    let stop = Arc::new(AtomicBool::new(false));
    let steps = Arc::new(AtomicU64::new(0));
    let load = {
        let (stop, steps) = (stop.clone(), steps.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                busy.call(Command::SimStep(SimStepParams {
                    ticks: TICKS,
                    hash: false,
                }))
                .expect("the step runs");
                steps.fetch_add(1, Ordering::AcqRel);
            }
        })
    };
    // Let the load get going, so the burst lands inside a step rather than ahead of the first one.
    while steps.load(Ordering::Acquire) == 0 {
        std::thread::sleep(Duration::from_millis(1));
    }

    // A, the fast client: the whole burst goes out before any reply is read.
    let mut fast = raw_connect(&url);
    for id in 1..=BURST {
        let request = json!({
            "v": 1,
            "id": id,
            "method": "input.inject",
            "params": { "event": { "kind": "key_down", "key": "W" } },
        });
        fast.send(Message::text(request.to_string()))
            .expect("the burst sends");
    }
    let mut frames = Vec::new();
    for _ in 0..BURST {
        let reply: serde_json::Value = serde_json::from_str(&read_text(&mut fast)).unwrap();
        assert!(reply.get("ok").is_some(), "every inject succeeds: {reply}");
        frames.push(reply["frame"].as_u64().expect("a stamped frame"));
    }

    stop.store(true, Ordering::Release);
    load.join().expect("the load thread ends");

    let spread = frames.iter().max().unwrap() - frames.iter().min().unwrap();
    assert!(
        spread <= 2 * TICKS,
        "a pipelined burst must be answered within about a pass: {BURST} requests spread over \
         {spread} frames ({} passes)",
        spread / TICKS
    );
    assert!(
        steps.load(Ordering::Acquire) >= 2,
        "the load must actually have kept the sim busy across the burst"
    );
}
