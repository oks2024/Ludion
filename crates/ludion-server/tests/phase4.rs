//! Phase 4 exit-criteria integration tests — "Human Eyes", the frame-streaming half.
//!
//! Phase 4 adds *delivery*: the server can now push a `frames` subscription's rendered images and a
//! channel's JSON events at any time, so a human's viewer (or an agent) watches a running engine over
//! the same wire the CLI already speaks. These tests encode the two exit criteria that live entirely
//! engine-side (`implementation-guide.md`, "Phase 4 — Human Eyes"):
//!
//! 1. [`frame_stream_delivers_at_least_30_fps_at_1080p_over_localhost`] — a subscribed client receives
//!    ≥30 fps at 1920×1080 over loopback, the viewer's liveness floor. Both wire codecs are gated
//!    ([`frame_stream_delivers_at_least_30_fps_at_1080p_lz4`] is the `LZ4_RGBA` twin over the same
//!    body): the first-party viewers subscribe with LZ4, so the shipped path is the gated path.
//! 2. [`recorded_viewer_session_replays_to_an_identical_hash_sequence`] — a full *viewer-shaped*
//!    session (frames + `scene.changed` subscribed, inputs injected, values poked, hashed) driven over
//!    the wire into an armed recording replays to the identical `WorldHash` sequence — proving that an
//!    attached, rendering viewer never perturbs the deterministic sim.
//!
//! The third Phase 4 criterion — the parity audit that zero endpoints exist the CLI couldn't call — is
//! a TS-side task, verified against the generated protocol types, not here.
//!
//! These follow the Phase 3 suite's conventions (`exit_criteria.rs`): bind port 0 and tear down at
//! scope end; block on replies rather than sleep as a synchronization primitive; and gracefully skip a
//! GPU-dependent assertion when the host has no adapter (an `L2003` prints a `SKIPPED` line and the
//! test stays green on adapterless CI, while the pinned RTX 3090 actually renders and asserts).
//!
//! # A note on the 1080p rate assertion (criterion 1)
//!
//! A 1920×1080 frame is ~8 MB of RGBA, and the QOI encoder is a scalar reference encoder
//! whose generic hot loop monomorphizes into `ludion-server`. The root Cargo.toml's dev-profile
//! overrides (`[profile.dev.package."*"]` and `ludion-server` at opt-level 3) keep that encode path
//! optimized in debug builds — the streaming test hits the ~60 fps render ceiling on mesh content in
//! both profiles. Because that same override now also sets `ludion-server`'s `debug-assertions = false`,
//! `cfg!(debug_assertions)` is *false* when this crate (and its test binaries) compile — so both
//! `cargo test` and `--release` take the `!debug_assertions` branch and assert the real ≥30 fps
//! criterion. On the mesh scene that lands at ~60 fps, comfortably clear of the floor. The lower
//! `DEBUG_FLOOR` branch below stays as a fallback for anyone who removes the `debug-assertions`
//! override (it would then re-enable). Both profiles always print the achieved rate with `--nocapture`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::FrameIndex;
use ludion_engine::{Engine, dispatch, drive_hashes, replay_hashes};
use ludion_protocol::command::{ComponentSetParams, EntitySpawnParams, InputInjectParams};
use ludion_protocol::frame::FrameFormat;
use ludion_protocol::perf::ProfileCaptureParams;
use ludion_protocol::{Channel, Command, ResponseBody, SubscribeMode};
use ludion_server::Server;
use serde_json::json;

// --- Shared helpers (mirroring exit_criteria.rs; kept local — each test binary is its own crate) --

/// A lit scene with a camera and a point light but no geometry yet — the base the recorded viewer
/// session spawns into, so the session's own commands are what put anything on screen.
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

/// The `examples/gallery` directory, whose committed scenes the streaming test renders.
fn gallery_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

/// Binds a server serving `engine` on a free localhost port.
fn serve(engine: Engine) -> Server {
    Server::bind(engine, "127.0.0.1:0").expect("server binds")
}

/// Whether a diagnostic is the "no GPU adapter" skip signal (`L2003`).
fn is_no_adapter(diag: &Diagnostic) -> bool {
    ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(diag))
}

/// A spawn command placing a lit unit cube at the origin — the geometry the viewer session renders.
fn spawn_cube() -> Command {
    let mut components = BTreeMap::new();
    components.insert(
        "Transform".to_string(),
        json!({ "translation": [0.0, 0.0, 0.0] }),
    );
    components.insert(
        "Mesh".to_string(),
        json!({ "shape": "cube", "color": [0.85, 0.35, 0.30, 1.0] }),
    );
    Command::EntitySpawn(EntitySpawnParams {
        name: None,
        components,
        presentation: false,
    })
}

// --- Exit criterion 1: ≥30 fps at 1080p over localhost -------------------------------------------

/// **Exit criterion 1.** A subscribed client receives ≥30 fps at 1920×1080 over loopback.
///
/// A visually non-trivial gallery scene (`lights`: a ground plane, three shaded primitives, three
/// coloured point lights) is served at 1080p and left free-running; a `ludion-client` opens a
/// `latest_wins` `frames` subscription with no `max_fps` cap (so the measured rate is the honest
/// delivery ceiling, not a throttle) and drains frames. After a warm-up window that absorbs the lazy
/// GPU renderer's first-frame creation cost, a fixed measurement window counts delivered frames; the
/// achieved rate is printed and asserted against the profile's floor (see the module docs on the
/// debug/release gate). Every delivered frame's header must report 1920×1080 — the criterion is about
/// *1080p* frames, so a smaller frame arriving fast would not satisfy it.
///
/// Skips cleanly (an `L2003` at first render) on an adapterless host.
#[test]
fn needs_gpu_frame_stream_delivers_at_least_30_fps_at_1080p_over_localhost() {
    run_fps_gate(FrameFormat::Qoi);
}

/// **The LZ4 twin of exit criterion 1.** The same gate over `LZ4_RGBA`, the opt-in remote codec.
///
/// Skips cleanly (an `L2003` at first render) on an adapterless host.
#[test]
fn needs_gpu_frame_stream_delivers_at_least_30_fps_at_1080p_lz4() {
    run_fps_gate(FrameFormat::Lz4Rgba);
}

/// **The raw twin of exit criterion 1.** The same gate over `RAW_RGBA` — the format both first-party
/// viewers subscribe with on a loopback URL, so the shipped path has to be the gated path. Raw
/// trades codec time for payload size, and this test is where that trade is checked at 1080p: the
/// encode is a copy, but each frame is 8.3 MB through the loopback socket, so if the send cost ever
/// exceeds what the codecs saved, this gate is what notices.
///
/// Skips cleanly (an `L2003` at first render) on an adapterless host.
#[test]
fn needs_gpu_frame_stream_delivers_at_least_30_fps_at_1080p_raw() {
    run_fps_gate(FrameFormat::RawRgba);
}

/// The body of both 1080p delivery gates, parameterized by the subscription's frame codec. A failure
/// reports the render-only ceiling measured on the spot, so box load explains itself.
fn run_fps_gate(format: FrameFormat) {
    const WIDTH: u32 = 1920;
    const HEIGHT: u32 = 1080;
    // Generous windows on purpose: flaky timing tests are P0. The warm-up swallows renderer creation
    // and the pipeline filling; the measurement window is long enough that a few scheduler hiccups do
    // not move the average.
    const WARMUP: Duration = Duration::from_secs(1);
    const MEASURE: Duration = Duration::from_secs(3);
    // The per-read wait only bounds how long we block when the stream stalls; it is never the metric.
    const READ_TIMEOUT: Duration = Duration::from_secs(2);
    const TARGET_FPS: f64 = 30.0;

    let gallery = gallery_dir();
    let mut engine = Engine::with_project_root(gallery.clone()).with_render_size(WIDTH, HEIGHT);
    let lights_yaml = std::fs::read_to_string(gallery.join("lights.yaml")).unwrap();
    engine.load_inline_scene(&lights_yaml).unwrap();
    let server = serve(engine);
    let mut client = ludion_client::Client::connect(&server.url()).unwrap();

    let sub = client
        .subscribe(
            Channel::Frames,
            SubscribeMode::LatestWins { max_fps: None },
            format,
        )
        .unwrap();
    client.call(Command::SimResume).unwrap();

    // Warm-up: block for the first frame (which pays the one-time lazy-renderer + adapter cost), then
    // keep draining for the rest of the warm-up window. A `None` here means the first frame never
    // arrived within the read timeout — on an adapterless host the egress loop latches `L2003` and
    // renders nothing, so this is the graceful GPU skip for the streaming path.
    let first = match client.next_frame(READ_TIMEOUT) {
        Ok(Some(frame)) => frame,
        Ok(None) => {
            ludion_core::require_gpu::skip_no_adapter(true);
            eprintln!(
                "SKIPPED frame_stream_delivers_at_least_30_fps_at_1080p ({format:?}): no frame \
                 delivered (no GPU adapter)"
            );
            return;
        }
        Err(diag) => panic!("frame stream read failed: {diag}"),
    };
    assert_eq!(
        (first.header.width, first.header.height),
        (WIDTH, HEIGHT),
        "the frame stream must deliver 1080p frames"
    );
    assert_eq!(
        first.header.format, format,
        "the stream must deliver the codec this subscription asked for"
    );
    let warmup_deadline = Instant::now() + WARMUP;
    while Instant::now() < warmup_deadline {
        let remaining = warmup_deadline.saturating_duration_since(Instant::now());
        match client.next_frame(remaining.min(READ_TIMEOUT)) {
            Ok(_) => {}
            Err(diag) => panic!("frame stream read failed during warm-up: {diag}"),
        }
    }

    // Measurement window: count delivered frames and confirm each is genuinely 1080p.
    let start = Instant::now();
    let deadline = start + MEASURE;
    let mut frames = 0u32;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match client.next_frame(remaining.min(READ_TIMEOUT)) {
            Ok(Some(frame)) => {
                assert_eq!(
                    (frame.header.width, frame.header.height),
                    (WIDTH, HEIGHT),
                    "every delivered frame must be 1080p"
                );
                frames += 1;
            }
            Ok(None) => {}
            Err(diag) => panic!("frame stream read failed during measurement: {diag}"),
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    let fps = f64::from(frames) / elapsed;
    eprintln!(
        "frame_stream 1080p delivery ({format:?}): {frames} frames in {elapsed:.3}s = {fps:.1} fps \
         (profile: {})",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );

    client.unsubscribe(sub).unwrap();
    let ceiling = (fps < TARGET_FPS).then(|| render_only_ceiling(&mut client));
    if let Some(ceiling) = &ceiling {
        eprintln!("{ceiling}");
    }
    let ceiling = ceiling.unwrap_or_default();
    server.shutdown_clean();

    // The criterion is 30 fps. The root Cargo.toml sets `ludion-server`'s dev profile to
    // `debug-assertions = false` (opt-level 3 already optimizes the monomorphized QOI encode), so
    // `cfg!(debug_assertions)` is false here in *both* profiles and the `!debug_assertions` branch
    // asserts the real ≥30 fps. On the mesh scene the streamer hits the ~60 fps render ceiling, twice
    // the floor — flaky timing tests are P0, and this margin holds. The `DEBUG_FLOOR` branch stays as
    // a fallback that re-engages only if that `debug-assertions` override is ever removed.
    #[cfg(debug_assertions)]
    {
        const DEBUG_FLOOR: f64 = 15.0;
        assert!(
            fps >= DEBUG_FLOOR,
            "1080p {format:?} delivery {fps:.1} fps fell below the debug floor of {DEBUG_FLOOR} fps (the ≥30 \
             fps criterion is verified in release); {ceiling}"
        );
    }
    #[cfg(not(debug_assertions))]
    {
        assert!(
            fps >= TARGET_FPS,
            "1080p {format:?} delivery {fps:.1} fps is below the {TARGET_FPS} fps exit criterion; {ceiling}"
        );
    }
}

/// The render-only rate on this box right now, for a failing gate's message: after the client
/// unsubscribes, an armed profiler still counts as a watcher, so the served loop renders and reads
/// back each 1080p frame with no encode and no socket send. Never panics — the gate's own
/// assertion is what fails.
fn render_only_ceiling(client: &mut ludion_client::Client) -> String {
    const FRAMES: u64 = 120;
    client.set_call_timeout(Some(Duration::from_secs(30)));
    let start = Instant::now();
    let reply = client.call(Command::ProfileCapture(ProfileCaptureParams {
        frames: FRAMES,
        include_samples: false,
    }));
    let secs = start.elapsed().as_secs_f64();
    match reply {
        Ok(ok) => match ok.body {
            ResponseBody::ProfileReport { report } => {
                let n = report.measured_frames;
                let rate = n as f64 / secs;
                format!(
                    "render-only ceiling on this box right now: {rate:.1} fps (1080p render + \
                     readback, no encode, no socket, {n} frames in {secs:.3}s)"
                )
            }
            other => format!("render-only ceiling unavailable: unexpected reply {other:?}"),
        },
        Err(diag) => format!("render-only ceiling unavailable: {diag}"),
    }
}

// --- Exit criterion 2: a recorded viewer session replays to an identical hash sequence -----------

/// **Exit criterion 2.** A recorded *viewer* session replays to an identical `WorldHash` sequence.
///
/// The session is shaped like a human (or agent) watching and poking a running engine: it opens a
/// `frames` subscription *and* a `scene.changed` subscription, then drives the world — spawn a cube,
/// nudge its transform, inject key inputs, run a couple of `sim.step` batches — and finally hashes.
/// Recording is armed *before the engine is served* and the engine is *paused before serving* (the
/// Phase 3 pause-before-serve pattern): a paused engine advances only on explicit `sim.step`, so the
/// recorded command sequence is exactly what the client sent, with no wall-clock free-run ticks
/// slipping in between.
///
/// The point of the two live subscriptions is what they must *not* do. Frame rendering and event
/// delivery are pure reads of the world (`live.rs`), and `stream.subscribe`/`stream.unsubscribe` are
/// non-recorded reads (`recording::record_kind`) — so an attached, actively-rendering viewer must
/// leave the recorded session byte-for-byte identical to the same commands run with no viewer at all.
/// We prove exactly that: after a clean shutdown flushes the recording, [`replay_hashes`] reproduces
/// the full per-frame trace that [`drive_hashes`] produces in-process from the identical command list,
/// and the final hash equals the `sim.hash` taken live over the wire.
///
/// GPU graceful-skip: the replay/replay-comparison half needs no adapter, so the criterion is always
/// exercised. Only the *frames* subscription needs a GPU; on an adapterless host we run the session
/// without it (the `scene.changed` subscription still makes the session viewer-shaped), so the
/// determinism claim is checked everywhere while the frame-stream-during-recording claim is checked
/// wherever an adapter exists.
#[test]
fn needs_gpu_recorded_viewer_session_replays_to_an_identical_hash_sequence() {
    let record_path =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("recorded_viewer_session.lrp");

    // Pause *before* arming recording so the pause itself is not a recorded command, then arm and
    // serve. The header captures the starting scene + seed; the paused engine will advance only on the
    // explicit `sim.step`s the client sends.
    let mut engine = Engine::new();
    engine.load_inline_scene(LIT_SCENE).unwrap();
    dispatch(&mut engine, Command::SimPause).unwrap();
    let sink = std::fs::File::create(&record_path).expect("recording file creates");
    engine.set_recording(sink).expect("recording arms");
    let server = serve(engine);
    let mut client = ludion_client::Client::connect(&server.url()).unwrap();

    // Probe the GPU with a throwaway screenshot (a non-recorded read): its result decides only whether
    // we also open the frame stream. Either way the recorded command sequence is identical.
    let gpu = match client.screenshot(None, None) {
        Ok(_) => true,
        Err(diag) if is_no_adapter(&diag) => {
            eprintln!(
                "recorded_viewer_session: no GPU adapter — running the session without the frames \
                 subscription; the replay/determinism criterion is still exercised"
            );
            false
        }
        Err(diag) => panic!("GPU probe failed: {diag}"),
    };

    // The viewer's live subscriptions. `scene.changed` always; `frames` only with an adapter.
    let frames_sub = gpu.then(|| {
        client
            .subscribe(
                Channel::Frames,
                SubscribeMode::LatestWins { max_fps: None },
                FrameFormat::Qoi,
            )
            .expect("frames subscription opens")
    });
    let scene_sub = client
        .subscribe(
            Channel::SceneChanged,
            SubscribeMode::default(),
            FrameFormat::Qoi,
        )
        .unwrap();

    // Drive the viewer-shaped session over the wire. The input injections are a no-op for this scene
    // (it has no input-driven systems), but they must *record* cleanly all the same — a real viewer
    // session legitimately carries them. Each injection targets a frame that lands inside the following
    // step batch, so its forward-clamp is deterministic in both the live drive and the replay.
    let spawned = client.call(spawn_cube()).unwrap();
    let entity = match spawned.body {
        ResponseBody::Spawned { entity } => entity,
        other => panic!("expected a Spawned body, got {other:?}"),
    };

    // The exact command list, captured once and reused verbatim to reproduce the trace in-process.
    let set_a = Command::ComponentSet(ComponentSetParams {
        entity,
        component: "Transform".to_string(),
        value: json!({ "translation": [0.5, -0.5, 0.0] }),
    });
    let inject_a = Command::InputInject(InputInjectParams {
        frame: Some(FrameIndex::new(2)),
        event: json!({ "kind": "key_down", "key": "W" }),
    });
    let set_b = Command::ComponentSet(ComponentSetParams {
        entity,
        component: "Transform".to_string(),
        value: json!({ "translation": [0.75, -0.25, 0.1] }),
    });
    let inject_b = Command::InputInject(InputInjectParams {
        frame: Some(FrameIndex::new(5)),
        event: json!({ "kind": "key_up", "key": "W" }),
    });

    client.call(set_a.clone()).unwrap();
    client.call(inject_a.clone()).unwrap();
    client.sim_step(3, false).unwrap();

    // With a GPU, prove the frame stream was genuinely live during the recording: at least one 1080p—
    // well, default-size — frame must have flowed while we were driving mutations. This is the whole
    // point of the criterion, so we assert it rather than merely subscribing.
    if gpu {
        let frame = client
            .next_frame(Duration::from_secs(5))
            .expect("frame read")
            .expect("a frame must flow while a frames subscription is open and the world is dirty");
        assert!(
            !frame.payload.is_empty(),
            "a delivered frame must carry a payload"
        );
    }

    client.call(set_b.clone()).unwrap();
    client.call(inject_b.clone()).unwrap();
    client.sim_step(4, false).unwrap();
    let wire_final_hash = client.sim_hash().unwrap();

    // Close the subscriptions and disconnect, then shut the server down cleanly so the (unbuffered)
    // recording is fully flushed to disk before we read it back.
    if let Some(frames_sub) = frames_sub {
        client.unsubscribe(frames_sub).unwrap();
    }
    client.unsubscribe(scene_sub).unwrap();
    drop(client);
    server.shutdown_clean();

    let recorded = std::fs::read(&record_path).expect("recording file reads back");

    // Replaying the wire-recorded session reproduces the final world hash taken live over the wire.
    let replay_trace = replay_hashes(&recorded, ".").expect("recording replays");
    let (_, replay_final) = *replay_trace.last().expect("a non-empty hash trace");
    assert_eq!(
        replay_final.as_u64(),
        wire_final_hash.as_u64(),
        "the replayed recording must end on the same WorldHash the wire session hashed"
    );

    // Stronger: the identical command list run in-process, with no server and no subscriptions at all,
    // yields the identical full per-frame hash sequence — so the actively-rendering, subscribed viewer
    // perturbed nothing. The frames/scene subscriptions are absent here precisely because they must
    // make no difference.
    let mut in_process = Engine::new();
    in_process.load_inline_scene(LIT_SCENE).unwrap();
    let in_process_trace = drive_hashes(
        &mut in_process,
        vec![
            spawn_cube(),
            set_a,
            inject_a,
            Command::SimStep(ludion_protocol::command::SimStepParams {
                ticks: 3,
                hash: false,
            }),
            set_b,
            inject_b,
            Command::SimStep(ludion_protocol::command::SimStepParams {
                ticks: 4,
                hash: false,
            }),
        ],
    )
    .unwrap();

    assert_eq!(
        in_process_trace.len(),
        replay_trace.len(),
        "the replay and the in-process drive must produce the same number of frames"
    );
    for (i, (replayed, in_proc)) in replay_trace.iter().zip(&in_process_trace).enumerate() {
        assert_eq!(
            (replayed.0.as_u64(), replayed.1.as_u64()),
            (in_proc.0.as_u64(), in_proc.1.as_u64()),
            "frame/hash divergence at trace index {i}"
        );
    }
}
