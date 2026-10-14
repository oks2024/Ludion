//! Phase 3 exit-criteria integration tests — the whole point of Wave 4.
//!
//! Each test binds port 0 and lets the [`Server`] drop (or calls `shutdown()`) at scope end, which
//! must tear down cleanly — no leaked threads, no hangs. The protocol is request/response, so every
//! test blocks on replies; nothing here sleeps as a synchronization primitive. GPU-dependent tests
//! follow the established graceful-skip convention: an `L2003` (no adapter) prints a skip line and
//! returns, so an adapterless CI stays green, while a real adapter (this repo's pinned RTX 3090)
//! actually runs the render and asserts.
//!
//! The three tests map one-to-one onto `implementation-guide.md`'s Phase 3 exit criteria:
//!
//! 1. [`connect_spawn_step_screenshot_is_byte_identical_to_in_process`] — connect → spawn →
//!    `sim.step 10` → `render.screenshot`, **byte-identical** to the same frame rendered in-process.
//! 2. [`testkit_ssim_comparison_runs_over_the_wire`] — a real testkit SSIM comparison whose candidate
//!    image arrives over the protocol (a `render.screenshot`), passing against a committed gallery
//!    golden, plus a negative case proving the wire path carries real signal.
//! 3. [`wire_recorded_session_replays_to_an_identical_hash_sequence`] — a session *driven over the
//!    wire* into an armed recording replays to an identical `WorldHash` sequence.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::FrameIndex;
use ludion_engine::{Engine, dispatch, drive_hashes, replay_hashes};
use ludion_protocol::command::{
    ComponentSetParams, EntitySpawnParams, InputInjectParams, RenderScreenshotParams, SimStepParams,
};
use ludion_protocol::{Command, ResponseBody};
use ludion_server::Server;
use ludion_testkit::golden::{DEFAULT_HEIGHT, DEFAULT_SSIM_THRESHOLD, DEFAULT_WIDTH};
use serde_json::json;

// --- Shared helpers ------------------------------------------------------------------------------

/// A lit scene with a camera and a point light but nothing to draw yet — the exit-criterion-1 base
/// both engines start from, so the *spawn* over the wire is what puts geometry on screen.
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

/// The `examples/gallery` directory, whose scenes and committed goldens the wire-driven testkit
/// suite reuses.
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

/// A spawn command placing a lit unit cube at the origin (the geometry exit criterion 1 renders).
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
    })
}

/// Decodes standard (RFC 4648) base64 — the inverse of `ludion-engine`'s own dependency-free encoder,
/// kept just as small and dependency-free here so a test can turn a `render.screenshot` payload back
/// into PNG bytes.
fn base64_decode(text: &str) -> Vec<u8> {
    fn sextet(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for &byte in text.as_bytes() {
        if byte == b'=' {
            break;
        }
        let Some(value) = sextet(byte) else { continue };
        acc = (acc << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

// --- Exit criterion 1: byte-identical wire vs in-process screenshot ------------------------------

/// **Exit criterion 1.** Over the wire: connect → spawn a lit cube → `sim.step 10` →
/// `render.screenshot`. In-process: a fresh engine loaded from the same scene, driven through the
/// same command sequence via [`dispatch`], screenshotted with the same parameters. The two PNG
/// payloads, base64-decoded to bytes, must be **byte-identical** — the same command core reached two
/// different ways (a WebSocket queue vs a direct call) produces the exact same pixels — and the two
/// responses must report the same applied frame.
#[test]
fn needs_gpu_connect_spawn_step_screenshot_is_byte_identical_to_in_process() {
    let (width, height) = (320, 180);
    let screenshot = || {
        Command::RenderScreenshot(RenderScreenshotParams {
            width: Some(width),
            height: Some(height),
            ..RenderScreenshotParams::default()
        })
    };

    // --- Over the wire ---
    // Pause before serving so the sim thread never free-runs by wall clock: with the loop paused,
    // an explicit `sim.step` is the only thing that advances the frame, making the applied frame
    // deterministic (the standard pause + step control pattern). Without this, the live scheduler
    // ticks an unknowable number of times between connect and the first command.
    let mut wire_engine = Engine::new();
    wire_engine.load_inline_scene(LIT_SCENE).unwrap();
    dispatch(&mut wire_engine, Command::SimPause).unwrap();
    let server = serve(wire_engine);
    let mut client = ludion_client::Client::connect(&server.url()).unwrap();

    client.call(spawn_cube()).unwrap();
    client.sim_step(10, false).unwrap();

    let wire_call = match client.call(screenshot()) {
        Ok(ok) => ok,
        Err(diag) if is_no_adapter(&diag) => {
            eprintln!(
                "SKIPPED connect_spawn_step_screenshot_is_byte_identical_to_in_process: no GPU adapter"
            );
            return;
        }
        Err(diag) => panic!("wire screenshot failed: {diag}"),
    };
    let (wire_png, wire_width, wire_height, wire_format, wire_frame) = match wire_call.body {
        ResponseBody::Screenshot {
            data,
            width,
            height,
            format,
            ..
        } => (base64_decode(&data), width, height, format, wire_call.frame),
        other => panic!("expected a Screenshot body over the wire, got {other:?}"),
    };

    // --- In-process, same scene + same command sequence through the parity gate ---
    let mut engine = Engine::new();
    engine.load_inline_scene(LIT_SCENE).unwrap();
    dispatch(&mut engine, spawn_cube()).unwrap();
    dispatch(
        &mut engine,
        Command::SimStep(SimStepParams {
            ticks: 10,
            hash: false,
        }),
    )
    .unwrap();
    let dispatched = dispatch(&mut engine, screenshot()).unwrap();
    let (in_process_png, in_process_frame) = match dispatched.body {
        ResponseBody::Screenshot { data, .. } => (base64_decode(&data), dispatched.frame),
        other => panic!("expected a Screenshot body in-process, got {other:?}"),
    };

    assert_eq!(wire_format, "png");
    assert_eq!((wire_width, wire_height), (width, height));
    assert!(
        !wire_png.is_empty(),
        "the wire screenshot must not be empty"
    );
    assert_eq!(
        wire_png, in_process_png,
        "the wire screenshot must be byte-identical to the in-process render of the same frame"
    );
    assert_eq!(
        wire_frame,
        Some(in_process_frame),
        "both paths must report the same applied frame"
    );
    assert_eq!(
        in_process_frame,
        FrameIndex::new(10),
        "the screenshot applies after the 10 stepped ticks"
    );

    server.shutdown();
}

// --- Exit criterion 2: a testkit suite runs over the wire ----------------------------------------

/// **Exit criterion 2.** A real testkit SSIM comparison where the candidate image arrives over the
/// protocol, not from an in-process render: a `render.screenshot` of the `sprites` gallery scene is
/// fetched via [`ludion_client`], decoded, and compared against the *committed* `sprites.png` golden
/// through testkit's [`compare_png_bytes`](ludion_testkit::compare_png_bytes) — the same tiled-SSIM
/// comparison `ludion test` runs in-process. It must pass at the suite threshold. Then the negative
/// half: the `meshes` scene, screenshotted over the same wire and compared against the *sprites*
/// golden, must **fail** with a localized diff report (a worst tile, damaged tiles) — proving the
/// wire path carries real image signal, so the pass above is not vacuous.
#[test]
fn needs_gpu_testkit_ssim_comparison_runs_over_the_wire() {
    let platform = match ludion_testkit::platform_dir() {
        Ok(platform) => platform,
        Err(diag) => {
            eprintln!(
                "SKIPPED testkit_ssim_comparison_runs_over_the_wire:
{diag}"
            );
            return;
        }
    };
    let gallery = gallery_dir();
    let sprites_golden = std::fs::read(gallery.join("goldens").join(platform).join("sprites.png"))
        .expect("committed sprites golden reads");

    // Serve the sprites scene with the gallery as project root so its textures resolve, exactly as
    // the in-process golden render does.
    let mut engine = Engine::with_project_root(gallery.clone());
    let sprites_yaml = std::fs::read_to_string(gallery.join("sprites.yaml")).unwrap();
    engine.load_inline_scene(&sprites_yaml).unwrap();
    let server = serve(engine);
    let mut client = ludion_client::Client::connect(&server.url()).unwrap();

    // --- Positive: the golden scene over the wire matches its committed golden ---
    let shot = match client.screenshot(Some(DEFAULT_WIDTH), Some(DEFAULT_HEIGHT)) {
        Ok(shot) => shot,
        Err(diag) if is_no_adapter(&diag) => {
            eprintln!("SKIPPED testkit_ssim_comparison_runs_over_the_wire: no GPU adapter");
            return;
        }
        Err(diag) => panic!("wire screenshot failed: {diag}"),
    };
    let sprites_png = base64_decode(&shot.data);
    let report =
        ludion_testkit::compare_png_bytes(&sprites_golden, &sprites_png, DEFAULT_SSIM_THRESHOLD)
            .expect("both PNGs decode");
    assert!(
        report.pass,
        "the wire screenshot of the golden scene must pass its committed golden: {}",
        report.summary
    );
    assert!(
        report.ssim.unwrap() >= DEFAULT_SSIM_THRESHOLD,
        "ssim {:?} below the suite threshold {DEFAULT_SSIM_THRESHOLD}",
        report.ssim
    );
    server.shutdown();

    // --- Negative: a different scene over the wire fails the sprites golden, and the failure is
    // localized (real signal, not vacuous success). ---
    let mut other = Engine::with_project_root(gallery.clone());
    let meshes_yaml = std::fs::read_to_string(gallery.join("meshes.yaml")).unwrap();
    other.load_inline_scene(&meshes_yaml).unwrap();
    let other_server = serve(other);
    let mut other_client = ludion_client::Client::connect(&other_server.url()).unwrap();

    let meshes_shot = other_client
        .screenshot(Some(DEFAULT_WIDTH), Some(DEFAULT_HEIGHT))
        .expect("adapter proven present by the positive half above");
    let meshes_png = base64_decode(&meshes_shot.data);
    let bad =
        ludion_testkit::compare_png_bytes(&sprites_golden, &meshes_png, DEFAULT_SSIM_THRESHOLD)
            .expect("both PNGs decode");
    assert!(
        !bad.pass,
        "a different scene over the wire must NOT pass the sprites golden (ssim {:?})",
        bad.ssim
    );
    assert!(
        bad.ssim.unwrap() < DEFAULT_SSIM_THRESHOLD,
        "the different scene should score well below threshold, got {:?}",
        bad.ssim
    );
    assert!(
        !bad.damaged_tiles.is_empty() && bad.worst_tile.is_some(),
        "the failure must localize damage to tiles: {}",
        bad.summary
    );

    other_server.shutdown();
}

// --- Exit criterion 3: a wire-driven recording replays to an identical hash sequence -------------

/// **Exit criterion 3 (wire-driven).** An engine has recording *armed before it is served*; a full
/// session — spawn, `component.set`, `input.inject`, several `sim.step`s — is then driven entirely
/// over the wire and hashed at the end. After a clean shutdown flushes the recording, the file is
/// replayed with [`replay_hashes`]: its final `WorldHash` must equal the `sim.hash` taken over the
/// wire. And the stronger claim: replaying the recording produces the *entire* per-frame hash
/// sequence that the same command list produces in-process via [`drive_hashes`] — the
/// recorded-over-the-wire session reproduces exactly, frame for frame.
#[test]
fn wire_recorded_session_replays_to_an_identical_hash_sequence() {
    let record_path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("wire_recorded_session.lrp");

    // Arm recording on a scene *before* serving, so the replay header captures the starting scene
    // and seed. Pause *before* arming (the exit-criterion-1 pattern above): a served Running engine
    // free-runs on the sim thread, and since the live-record fix (audit 1.2) those free-run ticks
    // are faithfully recorded as batched `SimStep` records — a wall-clock-dependent count that can't
    // be reproduced by a fixed command list. Pausing first makes the wire session lockstep, so the
    // recording is exactly the driven commands; the free-running recording path has its own
    // dedicated coverage in `ludion-engine/tests/live_record.rs`. The pause precedes `set_recording`
    // so it is not itself a record in the file.
    let mut engine = Engine::new();
    engine.load_inline_scene(LIT_SCENE).unwrap();
    dispatch(&mut engine, Command::SimPause).unwrap();
    let sink = std::fs::File::create(&record_path).expect("recording file creates");
    engine.set_recording(sink).expect("recording arms");
    let server = serve(engine);
    let mut client = ludion_client::Client::connect(&server.url()).unwrap();

    // Drive the session over the wire.
    let spawned = client.call(spawn_cube()).unwrap();
    let entity = match spawned.body {
        ResponseBody::Spawned { entity } => entity,
        other => panic!("expected a Spawned body, got {other:?}"),
    };
    // The exact commands the session runs; reused verbatim to reproduce the trace in-process. The
    // input event carries an explicit apply frame so the recorded `Input` record and the in-process
    // replay clamp to the identical frame (matching what `replay_hashes` reconstructs).
    let component_set = Command::ComponentSet(ComponentSetParams {
        entity,
        component: "Transform".to_string(),
        value: json!({ "translation": [0.5, -0.5, 0.0] }),
    });
    let input_inject = Command::InputInject(InputInjectParams {
        frame: Some(FrameIndex::new(2)),
        event: json!({ "kind": "key_down", "key": "W" }),
    });

    client.call(component_set.clone()).unwrap();
    client.call(input_inject.clone()).unwrap();
    client.sim_step(5, false).unwrap();
    let wire_final_hash = client.sim_hash().unwrap();

    // Shut down cleanly: joins the sim thread, drops the engine, and closes the (unbuffered) replay
    // file — every record is already on disk.
    drop(client);
    server.shutdown();

    let recorded = std::fs::read(&record_path).expect("recording file reads back");

    // Replaying the wire-recorded session reproduces the final world hash taken over the wire.
    let replay_trace = replay_hashes(&recorded, ".").expect("recording replays");
    let (_, replay_final) = *replay_trace.last().expect("a non-empty hash trace");
    assert_eq!(
        replay_final.as_u64(),
        wire_final_hash.as_u64(),
        "the replayed recording must end on the same WorldHash the wire session hashed"
    );

    // Stronger: the same command list, run in-process, yields the identical full hash sequence — so
    // the recording is a faithful, frame-for-frame reproduction of the wire-driven session.
    let mut in_process = Engine::new();
    in_process.load_inline_scene(LIT_SCENE).unwrap();
    let in_process_trace = drive_hashes(
        &mut in_process,
        vec![
            spawn_cube(),
            component_set,
            input_inject,
            Command::SimStep(SimStepParams {
                ticks: 5,
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
