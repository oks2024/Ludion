//! Egress-seam tests: the frame/event stream the sim thread emits over the [`egress`] channel.
//!
//! These drive a real [`run_live`] loop on its own thread through the intake channel and observe the
//! [`Egress`] messages coming back — the same arrangement `ludion-server` will use. The event tests
//! need no GPU; the frame test graceful-skips on an adapterless machine (the `L2003` skip pattern from
//! `gpu_dispatch.rs`).
//!
//! [`egress`]: ludion_engine::egress
//! [`run_live`]: ludion_engine::live::run_live

use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded, unbounded};
use ludion_engine::live::{ShutdownSignal, StampedCommand, run_live, shutdown_channel};
use ludion_engine::{Egress, EgressChannels, Engine, dispatch};
use ludion_protocol::command::{
    EngineRestoreParams, EntitySpawnParams, SceneLoadParams, SimStepParams, StreamSubscribeParams,
    SubscribeMode,
};
use ludion_protocol::event::{Event, LogLevel};
use ludion_protocol::frame::FrameFormat;
use ludion_protocol::{Channel, Command, ResponseBody, SubId, WireHash};
use serde_json::json;

// --- Harness -------------------------------------------------------------------------------------

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh temp directory to hold scene files a `scene.load` reads.
fn temp_project() -> std::path::PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ludion-egress-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A live [`run_live`] loop on its own thread, with handles to feed it commands and read its egress.
///
/// The two egress lanes are separate channels (audit finding 1.3): frames on a bounded(1) latest-wins
/// slot, events on an unbounded ordered lane. [`recv_any`](Session::recv_any) selects across both so a
/// test can observe whichever [`Egress`] message arrives first.
struct Session {
    intake: Sender<StampedCommand>,
    shutdown: ShutdownSignal,
    frames: Receiver<Egress>,
    events: Receiver<Egress>,
    handle: Option<JoinHandle<Result<(), ludion_core::diagnostics::Diagnostic>>>,
}

impl Session {
    /// Starts a session over `engine`. `with_egress` decides whether `run_live` gets the egress
    /// channels (a bounded(1) latest-wins frame slot + an unbounded event lane); the receivers are
    /// returned either way (they just stay silent when the loop was handed `None`).
    fn start(engine: Engine, with_egress: bool) -> Session {
        let (intake_tx, intake_rx) = unbounded();
        let (shutdown_tx, shutdown_rx) = shutdown_channel();
        let (frame_tx, frame_rx) = bounded::<Egress>(1);
        let (event_tx, event_rx) = unbounded::<Egress>();
        let egress_arg = with_egress.then(|| EgressChannels {
            frames: frame_tx,
            events: event_tx,
        });
        let handle = thread::spawn(move || run_live(engine, intake_rx, shutdown_rx, egress_arg));
        Session {
            intake: intake_tx,
            shutdown: shutdown_tx,
            frames: frame_rx,
            events: event_rx,
            handle: Some(handle),
        }
    }

    /// Receives the next [`Egress`] message from either lane, waiting up to `timeout`.
    fn recv_any(&self, timeout: Duration) -> Result<Egress, RecvTimeoutError> {
        crossbeam_channel::select! {
            recv(self.frames) -> msg => msg.map_err(|_| RecvTimeoutError::Disconnected),
            recv(self.events) -> msg => msg.map_err(|_| RecvTimeoutError::Disconnected),
            default(timeout) => Err(RecvTimeoutError::Timeout),
        }
    }

    /// Sends a command and blocks for its reply, expecting success.
    fn send(&self, command: Command) -> ResponseBody {
        let (stamped, reply) = StampedCommand::new(command);
        self.intake.send(stamped).unwrap();
        reply
            .recv_timeout(Duration::from_secs(5))
            .expect("sim thread replied in time")
            .expect("command succeeded")
            .body
    }

    /// Subscribes to `channel` in `latest_wins` mode, returning the new subscription id.
    fn subscribe(&self, channel: Channel) -> SubId {
        match self.send(Command::StreamSubscribe(StreamSubscribeParams {
            channel,
            mode: SubscribeMode::LatestWins { max_fps: None },
            format: FrameFormat::Qoi,
        })) {
            ResponseBody::Subscribed { sub } => sub,
            other => panic!("expected Subscribed, got {other:?}"),
        }
    }

    /// The current `WorldHash` over the wire.
    fn hash(&self) -> WireHash {
        match self.send(Command::SimHash) {
            ResponseBody::SimHash { hash } => hash,
            other => panic!("expected SimHash, got {other:?}"),
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.shutdown.signal();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Whether a real GPU adapter is available (else the frame test skips, like `gpu_dispatch.rs`).
fn gpu_available() -> bool {
    match ludion_renderer::Renderer::new() {
        Ok(_) => true,
        Err(diag) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&diag)) => false,
        Err(diag) => panic!("unexpected renderer error: {diag}"),
    }
}

/// Writes a minimal scene file into `root` and returns its project-relative name.
fn write_scene(root: &std::path::Path, name: &str) -> String {
    std::fs::write(root.join(name), "format: 1\nentities: []\n").unwrap();
    name.to_string()
}

// --- Event egress (no GPU) -----------------------------------------------------------------------

#[test]
fn scene_load_delivers_scene_changed_events_with_per_sub_seq() {
    let root = temp_project();
    let path = write_scene(&root, "level.yaml");
    let session = Session::start(Engine::with_project_root(&root), true);

    let sub = session.subscribe(Channel::SceneChanged);

    // First load → one EgressEvent, seq 0, addressed to our sub, carrying the scene path.
    session.send(Command::SceneLoad(SceneLoadParams { path: path.clone() }));
    let first = session
        .recv_any(Duration::from_secs(5))
        .expect("a scene.changed event arrives");
    match first {
        Egress::Event(event) => {
            assert_eq!(event.message.sub, sub);
            assert_eq!(event.message.seq.as_u64(), 0);
            assert!(event.message.frame.is_some());
            match event.message.event {
                Event::SceneChanged(data) => assert_eq!(data.path.as_deref(), Some("level.yaml")),
                other => panic!("expected SceneChanged, got {other:?}"),
            }
        }
        Egress::Frame(_) => panic!("expected an event, got a frame"),
    }

    // Second load → the same sub's seq increments to 1 (per-subscription monotonic).
    session.send(Command::SceneLoad(SceneLoadParams { path }));
    let second = session
        .recv_any(Duration::from_secs(5))
        .expect("a second scene.changed event arrives");
    match second {
        Egress::Event(event) => {
            assert_eq!(event.message.sub, sub);
            assert_eq!(event.message.seq.as_u64(), 1);
        }
        Egress::Frame(_) => panic!("expected an event, got a frame"),
    }
}

#[test]
fn restore_delivers_a_pathless_scene_changed_and_a_failed_restore_none() {
    let root = temp_project();
    let mut engine = Engine::with_project_root(&root);
    engine
        .load_inline_scene(
            "format: 1
entities: []
",
        )
        .unwrap();
    let snapshot = match dispatch(&mut engine, Command::EngineSnapshot)
        .expect("snapshot")
        .body
    {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected Snapshot, got {other:?}"),
    };
    let session = Session::start(engine, true);
    let _sub = session.subscribe(Channel::SceneChanged);

    // A malformed document is refused before the engine is touched: no event.
    let (stamped, reply) = StampedCommand::new(Command::EngineRestore(EngineRestoreParams {
        snapshot: serde_json::json!({ "format": "nonsense" }),
    }));
    session.intake.send(stamped).unwrap();
    reply
        .recv_timeout(Duration::from_secs(5))
        .expect("sim thread replied in time")
        .expect_err("a malformed snapshot is refused");
    assert!(
        session.recv_any(Duration::from_millis(500)).is_err(),
        "a failed restore pushes no scene.changed"
    );

    session.send(Command::EngineRestore(EngineRestoreParams { snapshot }));
    match session
        .recv_any(Duration::from_secs(5))
        .expect("a scene.changed event arrives")
    {
        Egress::Event(event) => match event.message.event {
            Event::SceneChanged(data) => assert_eq!(data.path, None),
            other => panic!("expected SceneChanged, got {other:?}"),
        },
        Egress::Frame(_) => panic!("expected an event, got a frame"),
    }
}

#[test]
fn two_scene_changed_subs_get_independent_seqs() {
    let root = temp_project();
    let path = write_scene(&root, "level.yaml");
    // One event with two subscribers fans out to two back-to-back sends. On the old shared bounded(1)
    // egress slot the second raced the pump and could be dropped; events now ride their own unbounded
    // lane (audit finding 1.3), so both reliably arrive — which is exactly what this asserts.
    let session = Session::start(Engine::with_project_root(&root), true);

    let sub_a = session.subscribe(Channel::SceneChanged);
    let sub_b = session.subscribe(Channel::SceneChanged);
    assert_ne!(sub_a, sub_b);

    session.send(Command::SceneLoad(SceneLoadParams { path }));

    // Both subscribers receive their own event, each with its own seq starting at 0.
    let mut seen = std::collections::BTreeMap::new();
    for _ in 0..2 {
        match session
            .recv_any(Duration::from_secs(5))
            .expect("both subscribers' events arrive")
        {
            Egress::Event(event) => {
                seen.insert(event.message.sub.as_u64(), event.message.seq.as_u64());
            }
            Egress::Frame(_) => panic!("expected an event, got a frame"),
        }
    }
    assert_eq!(seen.get(&sub_a.as_u64()), Some(&0));
    assert_eq!(seen.get(&sub_b.as_u64()), Some(&0));
}

#[test]
fn event_with_no_subscriber_on_its_channel_is_dropped() {
    let root = temp_project();
    let path = write_scene(&root, "level.yaml");
    let session = Session::start(Engine::with_project_root(&root), true);

    // Subscribe a *different* channel; the scene.changed event has no subscriber and is dropped.
    session.subscribe(Channel::Log);
    session.send(Command::SceneLoad(SceneLoadParams { path }));

    assert!(
        session.recv_any(Duration::from_millis(400)).is_err(),
        "an event with no subscriber on its channel must not be delivered",
    );
}

/// A burst of events across several subscribers must arrive with a gapless per-subscriber seq stream.
///
/// This is the engine-seam half of the audit finding 1.3 fix (no GPU needed): each `scene.load` emits one
/// `scene.changed` event that fans out to *every* subscriber — so with N subscribers, one load is N
/// back-to-back sends into the event lane. On the old shared `bounded(1)` egress slot those sends raced
/// the pump and dropped (a visible seq gap); on the dedicated unbounded event lane every one arrives, so
/// each subscriber sees a contiguous 0..LOADS sequence. Gaplessness is the observable proof of
/// losslessness at this seam. The server-level counterpart (with frames flowing) is in
/// `ludion-server/tests/delivery.rs`.
#[test]
fn a_burst_of_events_arrives_with_gapless_per_subscriber_seqs() {
    const SUBS: usize = 3;
    const LOADS: u64 = 8;

    let root = temp_project();
    let path = write_scene(&root, "level.yaml");
    let session = Session::start(Engine::with_project_root(&root), true);

    let subs: Vec<SubId> = (0..SUBS)
        .map(|_| session.subscribe(Channel::SceneChanged))
        .collect();

    // Fire the whole burst; each load fans out one event per subscriber onto the unbounded event lane.
    for _ in 0..LOADS {
        session.send(Command::SceneLoad(SceneLoadParams { path: path.clone() }));
    }

    // Drain exactly SUBS * LOADS events and record each subscriber's seq sequence.
    let total = SUBS as u64 * LOADS;
    let mut seqs: std::collections::BTreeMap<u64, Vec<u64>> = std::collections::BTreeMap::new();
    for _ in 0..total {
        match session
            .recv_any(Duration::from_secs(5))
            .expect("every event in the burst arrives — none dropped at the engine seam")
        {
            Egress::Event(event) => seqs
                .entry(event.message.sub.as_u64())
                .or_default()
                .push(event.message.seq.as_u64()),
            Egress::Frame(_) => panic!("no frame subscriber, so no frame should be produced"),
        }
    }

    // No stragglers beyond the burst.
    assert!(
        session.recv_any(Duration::from_millis(300)).is_err(),
        "exactly SUBS * LOADS events, no more",
    );

    // Each subscriber's seq stream is a gapless 0..LOADS — the losslessness proof.
    let expected: Vec<u64> = (0..LOADS).collect();
    for sub in &subs {
        assert_eq!(
            seqs.get(&sub.as_u64()),
            Some(&expected),
            "subscriber {sub:?} must see a gapless seq stream with no drops",
        );
    }
}

// --- Frame egress (needs a GPU) ------------------------------------------------------------------

#[test]
fn needs_gpu_frames_subscription_streams_rendered_frames_and_stops_when_idle() {
    if !gpu_available() {
        eprintln!("[frames] SKIPPED: no GPU adapter available");
        return;
    }

    // A small render size keeps the readback cheap; content is irrelevant (even empty renders).
    let mut engine = Engine::new().with_render_size(160, 90);
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .unwrap();
    let session = Session::start(engine, true);

    let sub = session.subscribe(Channel::Frames);

    // Subscribing marks the world dirty, so a frame is rendered and delivered.
    let frame = loop {
        match session
            .recv_any(Duration::from_secs(10))
            .expect("a rendered frame arrives")
        {
            Egress::Frame(frame) => break frame,
            Egress::Event(_) => continue,
        }
    };
    assert_eq!((frame.width, frame.height), (160, 90));
    assert_eq!(frame.rgba.len(), 160 * 90 * 4);
    assert!(
        frame.subs.contains(&sub),
        "the frame is addressed to our sub"
    );

    // Pause, then drain everything still in flight (including the one refresh the pause command's
    // drain triggers). Once the loop goes quiet, an idle paused engine must emit nothing more.
    session.send(Command::SimPause);
    while session.recv_any(Duration::from_millis(300)).is_ok() {}
    assert!(
        session.recv_any(Duration::from_millis(500)).is_err(),
        "an idle, paused engine must stop emitting frames",
    );
}

/// A load discards the stream ring yet still delivers a frame to a frames subscriber, stamped with
/// the new timeline; the load's fresh driver runs, so the index is not asserted.
#[test]
fn needs_gpu_a_scene_load_still_delivers_a_frame_on_the_new_timeline() {
    if !gpu_available() {
        eprintln!("[frames] SKIPPED: no GPU adapter available");
        return;
    }
    let root = temp_project();
    let scene = write_scene(&root, "again.yaml");
    let mut engine = Engine::with_project_root(&root).with_render_size(160, 90);
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .unwrap();
    let session = Session::start(engine, true);
    session.send(Command::SimPause);
    session.subscribe(Channel::Frames);

    let next_frame = || loop {
        match session
            .recv_any(Duration::from_secs(10))
            .expect("a frame arrives in time")
        {
            Egress::Frame(frame) => break frame,
            Egress::Event(_) => continue,
        }
    };
    let first = next_frame();

    session.send(Command::SceneLoad(
        ludion_protocol::command::SceneLoadParams { path: scene },
    ));
    // Frames of the old run may still be in flight; the first one off the old timeline is the check.
    let after = (0..1_000)
        .map(|_| next_frame())
        .find(|frame| frame.timeline != first.timeline)
        .expect("a frame of a new timeline arrives");
    assert_eq!(
        after.timeline,
        first.timeline + 1,
        "the load's image is a new run"
    );
}

// --- A render failure during streaming is loud (no GPU: extract fails before the adapter) --------

/// A render failure that is *not* a missing GPU must surface on the `log` channel, exactly once per
/// distinct diagnostic, no matter how many passes repeat it.
///
/// The scene's only sprite references a texture that does not exist under the project root, so every
/// render pass fails at *extract* with an `L1101` — before the GPU is ever requested, which is why this
/// test needs no adapter. The `frames` subscription keeps the world dirty so the loop retries the
/// (failing) render every pass; a subscribed `log` client must receive one error-level log carrying the
/// `L1101` text, and then nothing more despite the failure recurring.
///
/// Recovery — a render *success* resetting the dedup latch so a later failure logs again — is only
/// observable once a render succeeds, which needs a GPU adapter; it is left to the GPU-gated frame
/// tests rather than asserted here, to keep this test adapter-independent.
#[test]
fn a_repeated_render_failure_logs_exactly_once() {
    let root = temp_project();
    // No texture files are written under `root`, so `does-not-exist.png` cannot resolve.
    let scene = "\
format: 1
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
  - name: sprite
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      Sprite:
        texture: does-not-exist.png
        size:
          - 1.0
          - 1.0
";
    let mut engine = Engine::with_project_root(&root).with_render_size(160, 90);
    engine.load_inline_scene(scene).unwrap();
    let session = Session::start(engine, true);

    // A viewer watching both the frame stream and the log channel.
    session.subscribe(Channel::Log);
    session.subscribe(Channel::Frames);

    // The frames subscription marks the world dirty, so the free-running loop attempts a render every
    // pass — each failing identically. Exactly one log event must reach the log subscriber (a scene
    // with a missing texture can never render a frame, so the first egress message is the log).
    let first = match session
        .recv_any(Duration::from_secs(5))
        .expect("a log event arrives")
    {
        Egress::Event(event) => event,
        Egress::Frame(_) => panic!("a scene with a missing texture cannot render a frame"),
    };
    let message = match first.message.event {
        Event::Log(log) => {
            assert_eq!(
                log.level,
                LogLevel::Error,
                "a render failure logs at error level"
            );
            log.message
        }
        other => panic!("expected a Log event, got {other:?}"),
    };
    assert!(
        message.contains("L1101"),
        "the log must carry the render failure's diagnostic code: {message}"
    );

    // Despite many more failing passes, the repeated identical failure is deduplicated: no second log
    // event arrives.
    assert!(
        session.recv_any(Duration::from_millis(600)).is_err(),
        "the same render failure repeating every pass must emit exactly one log event",
    );
}

/// The frame graph's physical textures are allocated once and reused for every later streamed frame
/// (`ludion_render_graph`'s physical cache), so the **first** frame of a stream is the only one that
/// draws into freshly-zeroed textures and every later one inherits its predecessor's contents. Any
/// pass that read its target before writing it, or leaned on a load op finding cleared memory, would
/// therefore make frame 0 differ from frames 1..N.
///
/// Two scenes, chosen for the two shapes that could break: `post-full` schedules the deepest post
/// chain (shadow, prepass, ssao pair, shafts pair, dof pair, the bloom pyramid, composite — so the
/// most aliased physicals), and `ui-hud` adds the `overlay` pass, the one pass in the renderer whose
/// load op is `Load` rather than `Clear`.
#[test]
fn needs_gpu_streamed_frames_survive_physical_texture_reuse() {
    if !gpu_available() {
        eprintln!("[physical-reuse] SKIPPED: no GPU adapter available");
        return;
    }
    // The gallery *is* the project root for these scenes — their `Sprite` textures are named
    // relative to it.
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery");

    for scene in ["post-full.yaml", "ui-hud.yaml"] {
        let session = Session::start(
            Engine::with_project_root(&root).with_render_size(320, 180),
            true,
        );
        session.send(Command::SceneLoad(SceneLoadParams {
            path: scene.to_string(),
        }));
        // A render failure is a log event, which is dropped unless something is subscribed — so
        // subscribe, and fail on it rather than waiting out the timeout with no reason.
        session.subscribe(Channel::Log);
        session.subscribe(Channel::Frames);

        // Readbacks come back in submission order, so the first frame collected is the one rendered
        // into the freshly-created textures — the reference every reusing frame must reproduce.
        let mut frames: Vec<Vec<u8>> = Vec::new();
        while frames.len() < 4 {
            match session
                .recv_any(Duration::from_secs(30))
                .unwrap_or_else(|e| panic!("{scene}: a rendered frame arrives ({e})"))
            {
                Egress::Frame(frame) => frames.push(frame.rgba),
                Egress::Event(event) => match event.message.event {
                    Event::Log(log) if log.level == LogLevel::Error => {
                        panic!(
                            "{scene}: the stream reported a render failure: {}",
                            log.message
                        )
                    }
                    _ => continue,
                },
            }
        }
        for (index, rgba) in frames.iter().enumerate().skip(1) {
            assert_eq!(
                rgba, &frames[0],
                "{scene}: streamed frame {index} reuses frame 0's physical textures and must render \
                 the same pixels",
            );
        }
    }
}

/// **The stream extent is what [`Engine::set_render_size`] says it is, and it costs the sim nothing.**
///
/// The non-consuming setter exists so a host can name a smaller stream extent (`--render-size`,
/// `LUDION_RENDER_SIZE`) on a machine that cannot afford 720p worth of readback, encode and blit
/// every frame. Two properties have to hold for that to be usable, and this pins both at the egress
/// seam — the only place the delivered extent is observable:
///
/// 1. **Every delivered frame carries the extent that was set.** Both spellings are driven, the
///    builder's `with_render_size` and the setter, so the two cannot drift into writing different
///    fields.
/// 2. **The `WorldHash` sequence is identical either way.** The extent is host state that never
///    enters the world, so a session streamed at 160×90 and one streamed at 320×180 simulate
///    bit-identically — which is what lets a resized viewport share a machine with a recording.
///
/// Both sessions are paused before they reach the sim thread and driven by an explicit script, so the
/// comparison is deterministic rather than a race against the wall clock.
#[test]
fn needs_gpu_the_stream_extent_follows_the_render_size_setter_without_moving_the_hash() {
    if !gpu_available() {
        eprintln!("[render-size] SKIPPED: no GPU adapter available");
        return;
    }

    // How long the session's *first* frame may take. A session's first render compiles the whole
    // pipeline set, which on a cold GPU runs to tens of seconds — far past `Session::send`'s 5 s
    // reply bound, and the next command's reply is what would wait behind it. Absorbing that pass on
    // the frame lane (which carries no such bound) is what keeps the scripted half honest instead of
    // racing a shader compiler.
    const WARMUP: Duration = Duration::from_secs(120);
    // The quiet bound once the pipelines are warm: long enough that a delivered frame is never
    // missed, short enough that draining to quiet costs a scripted step almost nothing.
    const DRAIN: Duration = Duration::from_secs(2);

    // The dimensions of the next delivered frame, or `None` once `timeout` passes with both lanes
    // quiet. A render failure fails loudly here rather than being waited out as silence.
    fn next_frame(session: &Session, timeout: Duration) -> Option<(u32, u32)> {
        while let Ok(message) = session.recv_any(timeout) {
            match message {
                Egress::Frame(frame) => {
                    assert_eq!(
                        frame.rgba.len() as u32,
                        frame.width * frame.height * 4,
                        "a delivered frame's payload matches its own header",
                    );
                    return Some((frame.width, frame.height));
                }
                Egress::Event(event) => match event.message.event {
                    Event::Log(log) if log.level == LogLevel::Error => {
                        panic!("the stream reported a render failure: {}", log.message)
                    }
                    _ => continue,
                },
            }
        }
        None
    }

    // Drives one paused session at `extent`, returning every delivered frame's dimensions and the
    // `WorldHash` sequence the script produced. `via_setter` picks which spelling applies the extent.
    fn drive(extent: (u32, u32), via_setter: bool) -> (Vec<(u32, u32)>, Vec<WireHash>) {
        let mut engine = Engine::new();
        if via_setter {
            engine.set_render_size(extent.0, extent.1);
        } else {
            engine = engine.with_render_size(extent.0, extent.1);
        }
        assert_eq!(
            engine.render_size(),
            extent,
            "the reader agrees with both setters"
        );
        engine
            .load_inline_scene("format: 1\nseed: 7\nentities: []\n")
            .unwrap();
        // Paused before the engine moves onto the sim thread: the loop never free-runs by wall
        // clock, so the script below applies identically in both sessions.
        dispatch(&mut engine, Command::SimPause).unwrap();
        let session = Session::start(engine, true);
        session.subscribe(Channel::Frames);

        // Absorb the subscription's first frame before sending anything else: that pass pays the
        // one-off pipeline compilation, and nothing that follows may queue a reply behind it.
        let mut sizes =
            vec![next_frame(&session, WARMUP).expect("the subscription's first frame arrives")];

        let mut hashes = vec![session.hash()];
        for i in 0..6u32 {
            let mut components = std::collections::BTreeMap::new();
            components.insert(
                "Transform".to_string(),
                json!({ "translation": [i as f32, 0.0, 0.0] }),
            );
            session.send(Command::EntitySpawn(EntitySpawnParams {
                name: None,
                components,
            }));
            hashes.push(session.hash());
            // Every drained command marks the world dirty, so each one owes a frames subscriber a
            // render; drain to quiet before driving the next step. Draining is not optional here —
            // the egress frame slot is bounded(1), and a full slot makes the engine skip submits.
            // The readback ring is pipelined, so a pass collects an *older* frame or none at all,
            // which is why the script runs several passes rather than expecting one frame each.
            while let Some(size) = next_frame(&session, DRAIN) {
                sizes.push(size);
            }
        }
        // Non-empty by construction (the warm-up frame above), and restated because the caller's
        // assertion is an `all(..)` — which an empty list would satisfy vacuously.
        assert!(
            !sizes.is_empty(),
            "the session must deliver at least one frame to prove anything",
        );
        (sizes, hashes)
    }

    let (small_sizes, small_hashes) = drive((160, 90), true);
    assert!(
        small_sizes.iter().all(|size| *size == (160, 90)),
        "every frame a session delivers carries the extent that was set: {small_sizes:?}",
    );

    let (large_sizes, large_hashes) = drive((320, 180), false);
    assert!(
        large_sizes.iter().all(|size| *size == (320, 180)),
        "and the builder spelling writes the same field: {large_sizes:?}",
    );

    assert_eq!(
        small_hashes, large_hashes,
        "the render extent is host state: two sessions streaming at different sizes must produce \
         the same WorldHash sequence",
    );
}

// --- Determinism guard (no GPU: the event-channel variant) ---------------------------------------

#[test]
fn egress_attachment_does_not_perturb_the_world_hash_sequence() {
    // Drive an identical, fully paused command script through two engines — one with the egress
    // channel and a subscriber attached, one without — and assert the WorldHash sequences match.
    // Frame rendering and event emission are pure reads of the world, so attaching egress cannot
    // change the sim; this proves it.
    fn drive(with_egress: bool) -> Vec<WireHash> {
        let mut engine = Engine::new();
        engine
            .load_inline_scene("format: 1\nseed: 7\nentities: []\n")
            .unwrap();
        // Pause *before* moving onto the sim thread, so the loop never free-runs by wall clock and
        // the script applies deterministically.
        dispatch(&mut engine, Command::SimPause).unwrap();
        let session = Session::start(engine, with_egress);
        if with_egress {
            session.subscribe(Channel::SceneChanged);
        }

        let mut hashes = vec![session.hash()];
        for i in 0..5u32 {
            let mut components = std::collections::BTreeMap::new();
            components.insert(
                "Transform".to_string(),
                json!({ "translation": [i as f32, 0.0, 0.0] }),
            );
            session.send(Command::EntitySpawn(EntitySpawnParams {
                name: None,
                components,
            }));
            hashes.push(session.hash());
            session.send(Command::SimStep(SimStepParams {
                ticks: 1,
                hash: false,
            }));
            hashes.push(session.hash());
        }
        hashes
    }

    assert_eq!(
        drive(true),
        drive(false),
        "attaching the egress channel and a subscriber must not change the WorldHash sequence",
    );
}
