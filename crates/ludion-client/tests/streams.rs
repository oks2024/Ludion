//! Integration tests for the stream-consumption API added to `ludion-client`: buffering event/frame
//! traffic during [`Client::call`], and draining it through [`Client::next_event`] /
//! [`Client::next_frame`].
//!
//! Mirrors `ludion-server`'s own `tests/delivery.rs` (which exercises delivery with a raw socket,
//! since the typed client didn't consume streams yet) — these tests exercise the same server-side
//! delivery, but entirely through the typed [`Client`], which is exactly the surface under test here.
//! Event delivery needs no GPU; frame delivery does, and follows the established graceful-skip
//! convention: probe with a `render.screenshot` first and skip on `L2003` (no adapter).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ludion_client::Client;
use ludion_engine::Engine;
use ludion_protocol::command::SceneLoadParams;
use ludion_protocol::frame::FrameFormat;
use ludion_protocol::{Channel, Command, Event, Seq, SubscribeMode};

/// A minimal valid scene, written to disk so `scene.load` (which reads a file) can load it.
const BASE_SCENE: &str = "format: 1\nseed: 7\nentities: []\n";

/// A lit scene with a camera and a light — something the frame test can actually render.
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

/// Binds a server serving `engine` on a free localhost port.
fn serve(engine: Engine) -> ludion_server::Server {
    ludion_server::Server::bind(engine, "127.0.0.1:0").expect("server binds")
}

/// Whether a diagnostic is the "no GPU adapter" skip signal (`L2003`) — the convention every
/// GPU-dependent wire test in this workspace follows.
fn is_no_adapter(diag: &ludion_core::diagnostics::Diagnostic) -> bool {
    ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(diag))
}

// --- Event flow (no GPU) --------------------------------------------------------------------------

/// A client subscribes to `scene.changed`, then drives two `scene.load`s through `call`. Each load's
/// event is picked up by `next_event` with the right `sub` and a strictly increasing `seq` (0, then
/// 1 — a fresh subscription's first event is `seq` 0, per `ludion-engine`'s `Subscriptions`).
/// Interleaved in the middle: a further `call` (`sim.hash`) is issued *before* the first event is
/// drained, proving a buffered event neither blocks nor corrupts an unrelated in-flight call, and
/// that the event is still there, intact, once the caller gets around to asking for it.
#[test]
fn scene_changed_events_arrive_in_order_and_survive_an_interleaved_call() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("client_streams_events");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("evt.yaml"), BASE_SCENE).unwrap();

    let mut engine = Engine::with_project_root(&dir);
    engine.load_inline_scene(BASE_SCENE).unwrap();
    let server = serve(engine);
    let mut client = Client::connect(&server.url()).unwrap();

    let sub = client
        .subscribe(
            Channel::SceneChanged,
            SubscribeMode::default(),
            FrameFormat::Qoi,
        )
        .unwrap();

    // First load: fires the first scene.changed event. Whether it lands on the socket before or
    // after this call's own response, `call` buffers it internally rather than dropping it.
    client
        .call(Command::SceneLoad(SceneLoadParams {
            path: "evt.yaml".into(),
        }))
        .unwrap();

    // Interleave: a second, unrelated call before the buffered event is drained. `call` must still
    // answer correctly — a queued event must never be mistaken for, or block, a response.
    let hash_before = client.sim_hash().unwrap();

    let event = client
        .next_event(Duration::from_secs(5))
        .unwrap()
        .expect("the first scene.changed event, buffered during one of the two calls above");
    assert_eq!(event.sub, sub, "the event is tagged with our subscription");
    assert_eq!(
        event.seq,
        Seq::new(0),
        "the first event on a fresh subscription is seq 0"
    );
    match &event.event {
        Event::SceneChanged(data) => {
            assert_eq!(data.path.as_deref(), Some("evt.yaml"));
        }
        other => panic!("expected a SceneChanged event, got {other:?}"),
    }

    // The interleaved call's own response arrived intact (unwrapped above); a second load bumps seq.
    let hash_after = client.sim_hash().unwrap();
    assert_eq!(
        hash_before, hash_after,
        "no ticks ran between the two hash calls"
    );

    client
        .call(Command::SceneLoad(SceneLoadParams {
            path: "evt.yaml".into(),
        }))
        .unwrap();
    let second = client
        .next_event(Duration::from_secs(5))
        .unwrap()
        .expect("the second scene.changed event");
    assert_eq!(second.sub, sub);
    assert_eq!(
        second.seq,
        Seq::new(1),
        "the second event bumps the sequence"
    );

    server.shutdown();
}

/// `next_event` returns `Ok(None)` promptly when nothing is subscribed and nothing arrives — a
/// bounded, generous timeout so the assertion can never flake, but tight enough that a regression
/// turning this into a hang gets caught fast. Also proves the socket read timeout used internally is
/// restored afterward: a normal `call` still works on the same client right after.
#[test]
fn next_event_times_out_promptly_with_no_subscription() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("client_streams_timeout");
    std::fs::create_dir_all(&dir).unwrap();
    let mut engine = Engine::with_project_root(&dir);
    engine.load_inline_scene(BASE_SCENE).unwrap();
    let server = serve(engine);
    let mut client = Client::connect(&server.url()).unwrap();

    let start = Instant::now();
    let result = client.next_event(Duration::from_millis(300)).unwrap();
    assert!(
        result.is_none(),
        "nothing is subscribed, so nothing should ever arrive"
    );
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "the timeout must return promptly, not hang: took {:?}",
        start.elapsed()
    );

    // The read timeout used internally must be restored: `call` still works afterward.
    let hash = client.sim_hash();
    assert!(
        hash.is_ok(),
        "call must still work after next_event times out: {hash:?}"
    );

    server.shutdown();
}

// --- Frame flow (GPU, graceful skip) --------------------------------------------------------------

/// A client subscribes to `frames` (latest_wins), resumes the sim, and `next_frame` returns a binary
/// message whose LFR1 header names the right subscription; the still-QOI-encoded payload decodes to
/// an image of exactly the header's dimensions.
#[test]
fn needs_gpu_frames_deliver_as_qoi_frames_matching_their_header_dimensions() {
    let mut engine = Engine::new();
    engine.load_inline_scene(LIT_SCENE).unwrap();
    let server = serve(engine);
    let mut client = Client::connect(&server.url()).unwrap();

    // Graceful skip: probe for a GPU adapter first, following the established convention (see
    // `ludion-server/tests/delivery.rs` and `exit_criteria.rs`).
    match client.screenshot(Some(32), Some(32)) {
        Ok(_) => {}
        Err(diag) if is_no_adapter(&diag) => {
            eprintln!(
                "SKIPPED frames_deliver_as_qoi_frames_matching_their_header_dimensions: no GPU adapter"
            );
            return;
        }
        Err(diag) => panic!("screenshot probe failed: {diag}"),
    }

    let sub = client
        .subscribe(
            Channel::Frames,
            SubscribeMode::LatestWins { max_fps: None },
            FrameFormat::Qoi,
        )
        .unwrap();
    client.call(Command::SimResume).unwrap();

    let frame = client
        .next_frame(Duration::from_secs(10))
        .unwrap()
        .expect("a rendered frame arrives once the sim is resumed");
    assert_eq!(frame.header.sub, sub, "the frame is tagged with our sub");
    assert!(
        frame.header.width > 0 && frame.header.height > 0,
        "plausible dimensions"
    );

    let (qoi_header, pixels) = qoi::decode_to_vec(&frame.payload).expect("a valid QOI payload");
    assert_eq!(qoi_header.width, frame.header.width);
    assert_eq!(qoi_header.height, frame.header.height);
    assert_eq!(
        pixels.len() as u32,
        frame.header.width * frame.header.height * u32::from(qoi_header.channels.as_u8()),
        "decoded pixels match the header dimensions"
    );

    server.shutdown();
}
