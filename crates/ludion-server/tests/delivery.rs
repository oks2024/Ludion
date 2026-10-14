//! Integration tests for Phase 4 stream *delivery*: events and rendered frames over the wire.
//!
//! Like `wire.rs`, every test binds port 0 and lets the [`Server`] drop (or `shutdown()`s it) at scope
//! end — a clean teardown, no leaked threads, no hangs, is itself part of what is under test. These use
//! a raw `tungstenite` socket rather than [`ludion_client`], because the typed client skips binary
//! messages (rendered frames), and because a couple of tests need to read on their own schedule.
//!
//! Event delivery (`scene.changed`) needs no GPU. Frame delivery does; those tests follow the
//! established graceful-skip convention — they probe with a `render.screenshot` first and, on `L2003`
//! (no adapter), print a skip line and return, so an adapterless CI stays green while the pinned
//! RTX 3090 runs them for real.

use std::net::TcpStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ludion_engine::Engine;
use ludion_protocol::frame::{FrameFormat, FrameHeader};
use serde_json::{Value, json};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// A minimal valid scene, written to disk so `scene.load` (which reads a file) can load it.
const BASE_SCENE: &str = "format: 1\nseed: 7\nentities: []\n";

/// A lit scene with a camera and a light — something the frame tests can actually render.
const LIT_SCENE: &str = "\
format: 1
seed: 7
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 5.0
      Camera:
        projection:
          type: perspective
          fov_y_deg: 50.0
  - name: light
    components:
      Transform:
        translation:
          - 2.0
          - 3.0
          - 4.0
      PointLight:
        color:
          - 1.0
          - 1.0
          - 1.0
        intensity: 6.0
        range: 22.0
";

type RawSocket = WebSocket<MaybeTlsStream<TcpStream>>;

/// Binds a server serving `engine` on a free localhost port.
fn serve(engine: Engine) -> ludion_server::Server {
    ludion_server::Server::bind(engine, "127.0.0.1:0").expect("server binds")
}

/// A server on the lit scene (for the frame tests).
fn serve_lit() -> ludion_server::Server {
    let mut engine = Engine::new();
    engine.load_inline_scene(LIT_SCENE).unwrap();
    serve(engine)
}

/// Opens a raw WebSocket, performs the hello handshake, sets a read timeout, and returns the socket.
fn raw_connect(url: &str) -> RawSocket {
    let (mut socket, _resp) = tungstenite::connect(url).unwrap();
    let hello = json!({ "protocol_version": 1, "client": "delivery-test", "features": [] });
    socket.send(Message::text(hello.to_string())).unwrap();
    let reply = read_json(&mut socket, Duration::from_secs(5)).expect("a ServerHello");
    assert!(
        reply.get("schema_hash").is_some(),
        "expected a ServerHello, got {reply}"
    );
    // A read timeout keeps the "expect nothing more" assertions from blocking forever; messages on
    // localhost arrive whole, so a timeout only ever fires with an empty buffer (never mid-frame).
    set_read_timeout(&mut socket, Duration::from_millis(250));
    socket
}

/// Sets the underlying TCP read timeout (plain `ws://` only).
fn set_read_timeout(socket: &mut RawSocket, timeout: Duration) {
    if let MaybeTlsStream::Plain(tcp) = socket.get_ref() {
        tcp.set_read_timeout(Some(timeout)).unwrap();
    }
}

/// Sends a request and returns the response's numeric id echo — the caller then reads the reply.
fn send_request(socket: &mut RawSocket, id: u64, method: &str, params: Value) {
    let request = json!({ "v": 1, "id": id, "method": method, "params": params });
    socket.send(Message::text(request.to_string())).unwrap();
}

/// Sends a request with **no** `params` member — the shape a parameterless method requires. Naming
/// `params` on one is refused `L3001` before dispatch, so [`send_request`] is not interchangeable
/// with this.
fn send_bare_request(socket: &mut RawSocket, id: u64, method: &str) {
    let request = json!({ "v": 1, "id": id, "method": method });
    socket.send(Message::text(request.to_string())).unwrap();
}

/// Reads the next text message as JSON within `deadline`, or `None` on timeout / close.
fn read_json(socket: &mut RawSocket, deadline: Duration) -> Option<Value> {
    set_read_timeout(socket, deadline);
    loop {
        match socket.read() {
            Ok(Message::Text(text)) => {
                return Some(serde_json::from_str(text.as_str()).expect("server text is JSON"));
            }
            Ok(Message::Binary(_)) | Ok(_) => continue,
            Err(_) => return None, // timeout, close, or io error
        }
    }
}

/// Reads the next binary (frame) message within `deadline`, or `None` on timeout / close.
fn read_binary(socket: &mut RawSocket, deadline: Duration) -> Option<Vec<u8>> {
    set_read_timeout(socket, deadline);
    loop {
        match socket.read() {
            Ok(Message::Binary(bytes)) => return Some(bytes.to_vec()),
            Ok(Message::Text(_)) | Ok(_) => continue,
            Err(_) => return None,
        }
    }
}

/// Sends `stream.subscribe` and returns the minted sub id from the reply.
///
/// No `format`: these tests speak raw JSON, so omitting the field also exercises the wire default
/// (QOI). [`subscribe_format`] is the explicit-codec variant.
fn subscribe(socket: &mut RawSocket, id: u64, channel: &str, mode: Value) -> u64 {
    subscribe_params(socket, id, json!({ "channel": channel, "mode": mode }))
}

/// Sends `stream.subscribe` naming an explicit frame `format` (`"QOI"` / `"LZ4_RGBA"`).
fn subscribe_format(
    socket: &mut RawSocket,
    id: u64,
    channel: &str,
    mode: Value,
    format: &str,
) -> u64 {
    subscribe_params(
        socket,
        id,
        json!({ "channel": channel, "mode": mode, "format": format }),
    )
}

/// Sends `stream.subscribe` with `params` verbatim and returns the minted sub id from the reply.
fn subscribe_params(socket: &mut RawSocket, id: u64, params: Value) -> u64 {
    send_request(socket, id, "stream.subscribe", params);
    // The subscribe reply is the first (and only) message pending; frames/events come after.
    loop {
        let reply = read_json(socket, Duration::from_secs(5)).expect("a subscribe reply");
        if reply.get("id").and_then(Value::as_u64) == Some(id) {
            return reply["ok"]["sub"]
                .as_u64()
                .expect("subscribed body carries a sub id");
        }
    }
}

/// Whether a JSON response is an `L2003` (no GPU adapter) error — the graceful-skip signal.
fn is_no_adapter(reply: &Value) -> bool {
    ludion_core::require_gpu::skip_no_adapter(reply["error"]["code"].as_str() == Some("L2003"))
}

/// Probes for a GPU adapter over the wire with a `render.screenshot`; returns `true` if one is present.
/// A no-adapter host returns `false`, and the frame tests skip.
fn gpu_available(url: &str) -> bool {
    let mut socket = raw_connect(url);
    send_request(
        &mut socket,
        1,
        "render.screenshot",
        json!({ "width": 32, "height": 32 }),
    );
    let reply = read_json(&mut socket, Duration::from_secs(30)).expect("a screenshot reply");
    let _ = socket.close(None);
    !is_no_adapter(&reply)
}

// --- Event delivery (no GPU) ---------------------------------------------------------------------

/// **Event delivery.** A client subscribes to `scene.changed`; a `scene.load` then fires the event,
/// which arrives as a JSON `EventMessage` carrying the right sub, seq 0, a frame, and the scene path. A
/// second load bumps the seq to 1. After `stream.unsubscribe`, a third load delivers nothing more.
#[test]
fn scene_changed_events_deliver_then_stop_after_unsubscribe() {
    // A project root with a scene file `scene.load` can read.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("delivery_events");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("evt.yaml"), BASE_SCENE).unwrap();

    let mut engine = Engine::with_project_root(&dir);
    engine.load_inline_scene(BASE_SCENE).unwrap();
    let server = serve(engine);
    let mut socket = raw_connect(&server.url());

    let sub = subscribe(
        &mut socket,
        10,
        "scene.changed",
        json!({ "mode": "latest_wins" }),
    );

    // First load → the first scene.changed event (seq 0).
    send_request(&mut socket, 11, "scene.load", json!({ "path": "evt.yaml" }));
    let event = read_event(&mut socket, sub).expect("the first scene.changed event");
    assert_eq!(event["event"], json!("scene.changed"));
    assert_eq!(event["sub"], json!(sub));
    assert_eq!(
        event["seq"],
        json!(0),
        "the first event on a fresh sub is seq 0"
    );
    assert!(
        event.get("frame").is_some(),
        "an event carries the sim frame it describes"
    );
    assert_eq!(event["data"]["path"], json!("evt.yaml"));

    // Second load → the same sub, seq bumped to 1.
    send_request(&mut socket, 12, "scene.load", json!({ "path": "evt.yaml" }));
    let event = read_event(&mut socket, sub).expect("the second scene.changed event");
    assert_eq!(
        event["seq"],
        json!(1),
        "the second event bumps the sequence"
    );

    // Unsubscribe, then a third load must deliver nothing.
    send_request(&mut socket, 13, "stream.unsubscribe", json!({ "sub": sub }));
    let ack = await_reply(&mut socket, 13);
    assert_eq!(ack["ok"]["kind"], json!("ack"));

    send_request(&mut socket, 14, "scene.load", json!({ "path": "evt.yaml" }));
    let _ = await_reply(&mut socket, 14); // the load still succeeds...
    assert!(
        read_event(&mut socket, sub).is_none(),
        "no event may arrive on a closed subscription"
    );

    server.shutdown();
}

/// **Subscription ownership.** A connection can close only the subscriptions it opened: another
/// socket's `stream.unsubscribe` of the same id is refused `L3007`, and the owner keeps receiving.
#[test]
fn another_connection_cannot_unsubscribe_a_sub_it_does_not_own() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("delivery_foreign_unsubscribe");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("evt.yaml"), BASE_SCENE).unwrap();

    let mut engine = Engine::with_project_root(&dir);
    engine.load_inline_scene(BASE_SCENE).unwrap();
    let server = serve(engine);
    let mut owner = raw_connect(&server.url());
    let mut stranger = raw_connect(&server.url());

    let sub = subscribe(
        &mut owner,
        10,
        "scene.changed",
        json!({ "mode": "latest_wins" }),
    );

    send_request(
        &mut stranger,
        20,
        "stream.unsubscribe",
        json!({ "sub": sub }),
    );
    let refusal = await_reply(&mut stranger, 20);
    assert_eq!(refusal["error"]["code"], json!("L3007"));

    send_request(&mut owner, 11, "scene.load", json!({ "path": "evt.yaml" }));
    assert!(
        read_event(&mut owner, sub).is_some(),
        "the owner's subscription survived the foreign unsubscribe"
    );

    server.shutdown();
}

/// Reads text messages until an `EventMessage` for `sub` arrives (skipping request replies that race
/// it through the one outbound queue), or `None` if none arrives before a short quiet window.
fn read_event(socket: &mut RawSocket, sub: u64) -> Option<Value> {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        match read_json(socket, Duration::from_millis(250)) {
            Some(msg) if msg.get("event").is_some() && msg["sub"].as_u64() == Some(sub) => {
                return Some(msg);
            }
            Some(_) => continue, // a request reply racing the event; keep looking
            None => break,       // quiet window elapsed
        }
    }
    None
}

/// Reads text messages until the reply for request `id` arrives (skipping any events), and returns it.
fn await_reply(socket: &mut RawSocket, id: u64) -> Value {
    loop {
        let msg = read_json(socket, Duration::from_secs(5)).expect("a reply");
        if msg.get("id").and_then(Value::as_u64) == Some(id) {
            return msg;
        }
    }
}

/// **Losslessness under a burst, with frames flowing (audit finding 1.3).** The end-to-end proof that
/// events no longer drop at the engine→pump seam when frames share the egress path. A client subscribes
/// to *both* `frames` and `scene.changed`, resumes the sim (so rendered frames stream continuously and
/// keep the frame lane busy), then fires a burst of `scene.load`s. Every `scene.changed` event must
/// arrive with a gapless seq stream `0..LOADS` — under the old shared `bounded(1)` egress slot a pending
/// frame displaced these events and the seqs showed gaps. GPU-gated: with frames flowing is the whole
/// point, so this skips (like the other frame tests) on an adapterless host, where the no-GPU
/// engine-seam counterpart in `ludion-engine/tests/egress.rs` pins the fix instead.
/// The burst also runs with the client briefly not reading, raw frames filling the socket, so the
/// per-connection text lane is exercised as well as the engine seam.
#[test]
fn needs_gpu_events_stay_gapless_while_frames_flow() {
    const LOADS: u64 = 24;

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("delivery_burst_gapless");
    std::fs::create_dir_all(&dir).unwrap();
    // A renderable scene on disk, so each reload keeps frames flowing while the burst runs.
    std::fs::write(dir.join("lit.yaml"), LIT_SCENE).unwrap();

    let mut engine = Engine::with_project_root(&dir);
    engine.load_inline_scene(LIT_SCENE).unwrap();
    let server = serve(engine);
    if !gpu_available(&server.url()) {
        eprintln!("SKIPPED events_stay_gapless_while_frames_flow: no GPU adapter");
        return;
    }
    let mut socket = raw_connect(&server.url());

    // Subscribe frames first (so the frame lane is busy), then the event channel under test, then run.
    let _frames_sub = subscribe_format(
        &mut socket,
        40,
        "frames",
        json!({ "mode": "latest_wins" }),
        "RAW_RGBA",
    );
    let evt_sub = subscribe(
        &mut socket,
        41,
        "scene.changed",
        json!({ "mode": "latest_wins" }),
    );
    send_request(&mut socket, 42, "sim.resume", json!({}));

    // Fire the whole burst back-to-back; each load emits one scene.changed event for our sub.
    for i in 0..LOADS {
        send_request(
            &mut socket,
            100 + i,
            "scene.load",
            json!({ "path": "lit.yaml" }),
        );
    }

    // A briefly busy client: the server's writer stalls behind raw frames while the whole burst is
    // answered, as full-suite CPU contention did by accident. The pause only makes the defect reproducible.
    std::thread::sleep(Duration::from_millis(500));

    // Collect our sub's event seqs while frames interleave on the same connection. Slow is not lost:
    // the only bound is a stall of no text message (reply or event); binary frames are not progress.
    let mut seqs: Vec<u64> = Vec::new();
    let mut load_replies = 0_u64;
    let mut last_progress = Instant::now();
    while (seqs.len() as u64) < LOADS && last_progress.elapsed() < Duration::from_secs(30) {
        set_read_timeout(&mut socket, Duration::from_millis(500));
        match socket.read() {
            Ok(Message::Text(text)) => {
                last_progress = Instant::now();
                let msg: Value = serde_json::from_str(text.as_str()).expect("server text is JSON");
                if msg.get("event").and_then(Value::as_str) == Some("scene.changed")
                    && msg["sub"].as_u64() == Some(evt_sub)
                {
                    seqs.push(msg["seq"].as_u64().expect("an event carries a seq"));
                } else if msg["id"]
                    .as_u64()
                    .is_some_and(|id| (100..100 + LOADS).contains(&id))
                {
                    load_replies += 1;
                }
            }
            Ok(Message::Binary(_)) | Ok(_) => continue, // a rendered frame or ping; ignore
            Err(_) => continue,                         // read timeout; keep waiting for the tail
        }
    }

    assert_eq!(
        seqs,
        (0..LOADS).collect::<Vec<_>>(),
        "every scene.changed event must arrive, in order, with no gaps — even while frames flow \
         (audit finding 1.3); {load_replies}/{LOADS} loads were answered, got {seqs:?}",
    );

    server.shutdown();
}

// --- Frame delivery (GPU) ------------------------------------------------------------------------

/// **Frame delivery, default format.** Subscribe to `frames` (latest_wins) *without* naming a format,
/// resume the sim, and receive a binary message: its LFR1 header names the sub, reports the wire
/// default codec (QOI), and carries plausible dimensions, and the QOI payload decodes to an image of
/// exactly those dimensions.
#[test]
fn needs_gpu_frames_deliver_as_qoi_encoded_binary_messages() {
    let server = serve_lit();
    if !gpu_available(&server.url()) {
        eprintln!("SKIPPED frames_deliver_as_qoi_encoded_binary_messages: no GPU adapter");
        return;
    }
    let mut socket = raw_connect(&server.url());

    let sub = subscribe(&mut socket, 20, "frames", json!({ "mode": "latest_wins" }));
    send_request(&mut socket, 21, "sim.resume", json!({}));

    let frame = read_binary(&mut socket, Duration::from_secs(10)).expect("a rendered frame");
    let (header, payload) = FrameHeader::decode(&frame).expect("a well-formed LFR1 frame");
    assert_eq!(
        header.sub.as_u64(),
        sub,
        "the frame is tagged with our sub id"
    );
    assert_eq!(
        header.format,
        FrameFormat::Qoi,
        "a subscription that names no format gets the wire default, QOI"
    );
    assert!(
        header.width > 0 && header.height > 0,
        "plausible dimensions"
    );

    // The QOI payload decodes, and its dimensions match the header (proving payload/header agree).
    let (qoi_header, pixels) = qoi::decode_to_vec(payload).expect("the payload is valid QOI");
    assert_eq!(qoi_header.width, header.width);
    assert_eq!(qoi_header.height, header.height);
    assert_eq!(
        pixels.len() as u32,
        header.width * header.height * u32::from(qoi_header.channels.as_u8()),
        "decoded pixels match the header dimensions"
    );

    server.shutdown();
}

/// **Frame delivery, LZ4.** A subscription naming `LZ4_RGBA` receives frames tagged with that format,
/// whose payload is a raw LZ4 *block* (no prepended size — the LFR1 header is the only place the
/// dimensions live) decompressing to exactly `width * height * 4` bytes of RGBA.
#[test]
fn needs_gpu_frames_deliver_as_lz4_when_requested() {
    let server = serve_lit();
    if !gpu_available(&server.url()) {
        eprintln!("SKIPPED frames_deliver_as_lz4_when_requested: no GPU adapter");
        return;
    }
    let mut socket = raw_connect(&server.url());

    let sub = subscribe_format(
        &mut socket,
        80,
        "frames",
        json!({ "mode": "latest_wins" }),
        "LZ4_RGBA",
    );
    send_request(&mut socket, 81, "sim.resume", json!({}));

    let frame = read_binary(&mut socket, Duration::from_secs(10)).expect("a rendered frame");
    let (header, payload) = FrameHeader::decode(&frame).expect("a well-formed LFR1 frame");
    assert_eq!(
        header.sub.as_u64(),
        sub,
        "the frame is tagged with our sub id"
    );
    assert_eq!(
        header.format,
        FrameFormat::Lz4Rgba,
        "a subscription that asked for LZ4_RGBA is served LZ4_RGBA"
    );
    assert!(
        header.width > 0 && header.height > 0,
        "plausible dimensions"
    );

    // The decoder's output size comes from the header, not the payload — that is the whole reason the
    // server compresses a raw block rather than a size-prepended one.
    let expected = (header.width * header.height * 4) as usize;
    let pixels =
        lz4_flex::block::decompress(payload, expected).expect("the payload is a valid LZ4 block");
    assert_eq!(
        pixels.len(),
        expected,
        "the block decompresses to exactly one RGBA image of the header's dimensions"
    );

    server.shutdown();
}

/// **Frame delivery, raw.** A subscription naming `RAW_RGBA` receives frames tagged with that format
/// whose payload is the pixels themselves: exactly `width * height * 4` bytes, no codec, no
/// embedded dimensions. The length assertion is the whole contract — a raw payload that disagrees
/// with its header is unreadable, since there is nothing else to recover the shape from.
#[test]
fn needs_gpu_frames_deliver_as_raw_rgba_when_requested() {
    let server = serve_lit();
    if !gpu_available(&server.url()) {
        eprintln!("SKIPPED frames_deliver_as_raw_rgba_when_requested: no GPU adapter");
        return;
    }
    let mut socket = raw_connect(&server.url());

    let sub = subscribe_format(
        &mut socket,
        100,
        "frames",
        json!({ "mode": "latest_wins" }),
        "RAW_RGBA",
    );
    send_request(&mut socket, 101, "sim.resume", json!({}));

    let frame = read_binary(&mut socket, Duration::from_secs(10)).expect("a rendered frame");
    let (header, payload) = FrameHeader::decode(&frame).expect("a well-formed LFR1 frame");
    assert_eq!(
        header.sub.as_u64(),
        sub,
        "the frame is tagged with our sub id"
    );
    assert_eq!(
        header.format,
        FrameFormat::RawRgba,
        "a subscription that asked for RAW_RGBA is served RAW_RGBA"
    );
    assert!(
        header.width > 0 && header.height > 0,
        "plausible dimensions"
    );
    assert_eq!(
        payload.len(),
        (header.width * header.height * 4) as usize,
        "a raw payload is exactly one RGBA image of the header's dimensions — no more, no less"
    );
    // The lit scene is not a black void: a payload of all zeroes would pass the length check while
    // proving the pixels were never actually the frame's.
    assert!(
        payload.iter().any(|&b| b != 0),
        "the raw payload carries real pixels, not a zeroed buffer"
    );

    server.shutdown();
}

/// **Per-format routing.** Three clients subscribe to `frames` with *different* codecs — one per
/// format that exists; each receives frames in its own format, and every payload decodes. The router
/// encodes each raw frame once per distinct requested format, so this is the test that a subscriber
/// is never handed another one's bytes. (Modeled on [`two_clients_each_receive_only_their_own_sub`],
/// which pins the sub-id half.)
#[test]
fn needs_gpu_mixed_format_subs_each_receive_their_own_format() {
    let server = serve_lit();
    if !gpu_available(&server.url()) {
        eprintln!("SKIPPED mixed_format_subs_each_receive_their_own_format: no GPU adapter");
        return;
    }
    let mut a = raw_connect(&server.url());
    let mut b = raw_connect(&server.url());
    let mut c = raw_connect(&server.url());

    let sub_a = subscribe_format(
        &mut a,
        90,
        "frames",
        json!({ "mode": "latest_wins" }),
        "QOI",
    );
    let sub_b = subscribe_format(
        &mut b,
        91,
        "frames",
        json!({ "mode": "latest_wins" }),
        "LZ4_RGBA",
    );
    let sub_c = subscribe_format(
        &mut c,
        93,
        "frames",
        json!({ "mode": "latest_wins" }),
        "RAW_RGBA",
    );
    send_request(&mut a, 92, "sim.resume", json!({}));

    for (socket, own, want) in [
        (&mut a, sub_a, FrameFormat::Qoi),
        (&mut b, sub_b, FrameFormat::Lz4Rgba),
        (&mut c, sub_c, FrameFormat::RawRgba),
    ] {
        let mut seen = 0;
        while seen < 3 {
            let Some(frame) = read_binary(socket, Duration::from_secs(10)) else {
                break;
            };
            let (header, payload) = FrameHeader::decode(&frame).unwrap();
            assert_eq!(
                header.sub.as_u64(),
                own,
                "each client sees only its own sub"
            );
            assert_eq!(
                header.format, want,
                "each client sees only the codec it subscribed with"
            );
            let expected = (header.width * header.height * 4) as usize;
            match want {
                FrameFormat::Qoi => {
                    let (_, pixels) = qoi::decode_to_vec(payload).expect("valid QOI");
                    assert_eq!(pixels.len(), expected, "QOI decodes to a full RGBA image");
                }
                FrameFormat::Lz4Rgba => {
                    let pixels =
                        lz4_flex::block::decompress(payload, expected).expect("valid LZ4 block");
                    assert_eq!(pixels.len(), expected, "LZ4 decodes to a full RGBA image");
                }
                FrameFormat::RawRgba => {
                    assert_eq!(payload.len(), expected, "raw IS a full RGBA image");
                }
            }
            seen += 1;
        }
        assert!(
            seen >= 1,
            "each client must receive at least one frame in its own format"
        );
    }

    server.shutdown();
}

/// **Stamped frame indices are monotonic (through the async streaming ring).** A resumed engine
/// streams through the decoupled submit/collect path (`live.rs` → the render-graph `StreamReadback`),
/// which stamps each pending readback with the `FrameIndex` captured at submission and delivers it
/// under that stamp — an *older* frame than the one currently ticking. Across a run of delivered
/// frames the header's frame index must be strictly increasing: latest-wins may skip frames (gaps are
/// expected), but a collected readback is emitted exactly once and never out of order, so the viewer
/// never sees a stale or repeated frame index.
#[test]
fn needs_gpu_streamed_frame_indices_are_strictly_increasing() {
    let server = serve_lit();
    if !gpu_available(&server.url()) {
        eprintln!("SKIPPED streamed_frame_indices_are_strictly_increasing: no GPU adapter");
        return;
    }
    let mut socket = raw_connect(&server.url());

    subscribe(&mut socket, 70, "frames", json!({ "mode": "latest_wins" }));
    send_request(&mut socket, 71, "sim.resume", json!({}));

    // Collect a run of frames and check every one advances the stamped index over its predecessor.
    let mut last: Option<u64> = None;
    let mut seen = 0;
    while seen < 8 {
        let Some(frame) = read_binary(&mut socket, Duration::from_secs(10)) else {
            break;
        };
        let (header, _) = FrameHeader::decode(&frame).expect("a well-formed LFR1 frame");
        let index = header.frame.as_u64();
        if let Some(prev) = last {
            assert!(
                index > prev,
                "stamped frame index must strictly increase, got {index} after {prev}"
            );
        }
        last = Some(index);
        seen += 1;
    }
    assert!(
        seen >= 2,
        "the stream must deliver at least two frames to compare indices, got {seen}"
    );

    server.shutdown();
}

/// **Per-connection routing.** Two clients each open their own `frames` subscription; each receives
/// frames tagged with its own sub id and never the other's.
#[test]
fn needs_gpu_two_clients_each_receive_only_their_own_sub() {
    let server = serve_lit();
    if !gpu_available(&server.url()) {
        eprintln!("SKIPPED two_clients_each_receive_only_their_own_sub: no GPU adapter");
        return;
    }
    let mut a = raw_connect(&server.url());
    let mut b = raw_connect(&server.url());

    let sub_a = subscribe(&mut a, 30, "frames", json!({ "mode": "latest_wins" }));
    let sub_b = subscribe(&mut b, 31, "frames", json!({ "mode": "latest_wins" }));
    assert_ne!(sub_a, sub_b, "each subscription mints a distinct id");
    send_request(&mut a, 32, "sim.resume", json!({}));

    // Each client sees several frames; every one carries only its own sub id.
    for socket_sub in [(&mut a, sub_a), (&mut b, sub_b)] {
        let (socket, own) = socket_sub;
        let mut seen = 0;
        while seen < 3 {
            let Some(frame) = read_binary(socket, Duration::from_secs(10)) else {
                break;
            };
            let (header, _) = FrameHeader::decode(&frame).unwrap();
            assert_eq!(
                header.sub.as_u64(),
                own,
                "a client only ever sees its own sub's frames"
            );
            seen += 1;
        }
        assert!(
            seen >= 1,
            "each client must receive at least one of its own frames"
        );
    }

    server.shutdown();
}

/// **max_fps throttling.** A `frames` subscription capped at 5 fps, read for ~1s, delivers far fewer
/// frames than an uncapped stream would — a generous smoke bound, not a precise-rate assertion.
#[test]
fn needs_gpu_max_fps_throttles_frame_delivery() {
    let server = serve_lit();
    if !gpu_available(&server.url()) {
        eprintln!("SKIPPED max_fps_throttles_frame_delivery: no GPU adapter");
        return;
    }
    let mut socket = raw_connect(&server.url());

    subscribe(
        &mut socket,
        40,
        "frames",
        json!({ "mode": "latest_wins", "max_fps": 5.0 }),
    );
    send_request(&mut socket, 41, "sim.resume", json!({}));

    // Count frames over a ~1s window. At 5 fps we expect ~5; an unthrottled stream would deliver many
    // times that. The bound is deliberately loose (<= 15) so it can never flake on timing.
    let window = Duration::from_millis(1100);
    let start = Instant::now();
    let mut count = 0;
    while start.elapsed() < window {
        if read_binary(&mut socket, Duration::from_millis(300)).is_some() {
            count += 1;
        }
    }
    assert!(
        count >= 1,
        "the throttled stream must still deliver frames, got {count}"
    );
    assert!(
        count <= 15,
        "max_fps=5 over ~1s must deliver far fewer than the unthrottled rate, got {count}"
    );

    server.shutdown();
}

/// Reads until the reply for `id` arrives, counting how many binary frame messages arrived ahead of
/// it. That count is the measurement: it is exactly how many megabyte payloads the reply had to wait
/// behind on the wire.
fn await_reply_counting_frames(
    socket: &mut RawSocket,
    id: u64,
    patience: Duration,
) -> (Value, usize) {
    let deadline = Instant::now() + patience;
    let mut frames = 0;
    loop {
        assert!(
            Instant::now() < deadline,
            "the reply for {id} never arrived (saw {frames} frames)"
        );
        set_read_timeout(socket, Duration::from_millis(500));
        match socket.read() {
            Ok(Message::Binary(_)) => frames += 1,
            Ok(Message::Text(text)) => {
                let msg: Value = serde_json::from_str(text.as_str()).expect("server text is JSON");
                if msg.get("id").and_then(Value::as_u64) == Some(id) {
                    return (msg, frames);
                }
            }
            Ok(_) => continue,  // ping/pong
            Err(_) => continue, // read timeout; keep waiting until the deadline
        }
    }
}

/// **A reply does not queue behind frames (the two outbound lanes).** The connection carries replies
/// and rendered frames on separate queues, drained text-first, so a request answered while a heavy
/// stream is running is not stuck behind the frames already owed to the socket.
///
/// The measurement is timing-free on purpose. A 1080p `RAW_RGBA` subscription runs uncapped (8.3 MB a
/// frame — the heaviest shipped stream), the client then stops reading for a window so the
/// connection's frame lane and the socket fill up, and only then is a request sent. What is counted
/// is how many binary frames arrive *ahead of that reply*, which is a property of the queue
/// structure, not of how fast this machine happens to be: with one shared queue the reply sits behind
/// everything already in it (measured 36), with two lanes it sits behind at most the frame the writer
/// is mid-send on (measured 3).
///
/// The request is a deliberately unknown method, refused `L3002` before dispatch: the sim thread
/// never sees it, so nothing but the outbound path is under test.
#[test]
fn needs_gpu_replies_do_not_queue_behind_frames() {
    // The stream the editor's own viewport uses on loopback, at the size `phase4.rs` gates.
    const WIDTH: u32 = 1920;
    const HEIGHT: u32 = 1080;
    /// Frames the reply may legitimately trail: the one the writer was mid-send on when the request
    /// landed, plus slack for the socket's own buffering. Well under a shared queue's depth.
    const MAX_FRAMES_AHEAD: usize = 8;
    /// Long enough for the frame lane and the socket buffers to fill with the client not reading.
    const BACKLOG_WINDOW: Duration = Duration::from_millis(800);

    let mut engine = Engine::new().with_render_size(WIDTH, HEIGHT);
    engine.load_inline_scene(LIT_SCENE).unwrap();
    let server = serve(engine);
    if !gpu_available(&server.url()) {
        eprintln!("SKIPPED replies_do_not_queue_behind_frames: no GPU adapter");
        return;
    }
    let mut socket = raw_connect(&server.url());

    subscribe_format(
        &mut socket,
        200,
        "frames",
        json!({ "mode": "latest_wins" }),
        "RAW_RGBA",
    );
    send_bare_request(&mut socket, 201, "sim.resume");

    // Wait for the stream to be genuinely running, then stop reading: frames pile up in the
    // connection's frame lane and the socket, which is the state a reply must not queue behind.
    let first = read_binary(&mut socket, Duration::from_secs(30)).expect("the stream starts");
    let (header, _) = FrameHeader::decode(&first).expect("a well-formed LFR1 frame");
    assert_eq!(
        (header.width, header.height),
        (WIDTH, HEIGHT),
        "the backlog has to be built out of full-size frames"
    );
    std::thread::sleep(BACKLOG_WINDOW);

    send_bare_request(&mut socket, 202, "not.a.method");
    let (reply, ahead) = await_reply_counting_frames(&mut socket, 202, Duration::from_secs(60));
    assert_eq!(
        reply["error"]["code"],
        json!("L3002"),
        "the probe request must be refused before dispatch: {reply}"
    );

    // The ordinary case rides behind it: with the stream flowing and the client reading again, real
    // sim-thread round trips still complete. The bound is deliberately loose and only on the median —
    // a single round trip legitimately spikes behind sim-thread work, and this test is not a latency
    // gate (that is `phase4.rs`), only a proof that the lane split did not wedge anything.
    let mut round_trips: Vec<Duration> = Vec::new();
    for i in 0..5u64 {
        let started = Instant::now();
        send_bare_request(&mut socket, 300 + i, "sim.hash");
        let reply = await_reply(&mut socket, 300 + i);
        assert!(reply.get("ok").is_some(), "sim.hash is answered: {reply}");
        round_trips.push(started.elapsed());
    }
    round_trips.sort();
    let median = round_trips[round_trips.len() / 2];
    eprintln!(
        "replies_do_not_queue_behind_frames: {ahead} frames ahead of the reply, sim.hash round-trip \
         median {:.1} ms",
        median.as_secs_f64() * 1000.0
    );

    assert!(
        ahead <= MAX_FRAMES_AHEAD,
        "a reply must overtake the queued frames, not wait behind them; it trailed {ahead}"
    );
    assert!(
        median < Duration::from_secs(8),
        "sim.hash round trips must still complete while frames stream; median {median:?}"
    );

    server.shutdown();
}

/// **Disconnect cleanup.** A client subscribes to `frames`, then drops its connection. The server stays
/// healthy: a fresh client's new `frames` subscription still delivers. (That the engine *stopped*
/// rendering for the dead viewer in between rides the same command queue — a `stream.unsubscribe`
/// submitted on disconnect — but is not directly observable without an engine-side probe we decline to
/// add; the surviving-delivery assertion is the externally visible proof the host is unharmed.)
#[test]
fn needs_gpu_dropping_a_frames_client_leaves_the_server_healthy() {
    let server = serve_lit();
    if !gpu_available(&server.url()) {
        eprintln!("SKIPPED dropping_a_frames_client_leaves_the_server_healthy: no GPU adapter");
        return;
    }

    // First client subscribes to frames, receives at least one, then vanishes (drop).
    {
        let mut doomed = raw_connect(&server.url());
        subscribe(&mut doomed, 50, "frames", json!({ "mode": "latest_wins" }));
        send_request(&mut doomed, 51, "sim.resume", json!({}));
        let _ = read_binary(&mut doomed, Duration::from_secs(10));
        // `doomed` drops here: the connection closes, and the server queues a stream.unsubscribe for
        // its sub through the one command queue, so the engine stops rendering for it.
    }

    // A brand-new client subscribing to frames still gets delivery — the host is unharmed.
    let mut fresh = raw_connect(&server.url());
    let sub = subscribe(&mut fresh, 60, "frames", json!({ "mode": "latest_wins" }));
    send_request(&mut fresh, 61, "sim.resume", json!({}));
    let frame = read_binary(&mut fresh, Duration::from_secs(10))
        .expect("a fresh client still receives frames after another disconnected");
    let (header, _) = FrameHeader::decode(&frame).unwrap();
    assert_eq!(
        header.sub.as_u64(),
        sub,
        "the fresh client sees its own new sub"
    );

    server.shutdown();
}
