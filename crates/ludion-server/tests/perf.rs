//! The live profiler over the wire: the `profile` channel, and `profile.capture`.
//!
//! These drive a real [`Server`](ludion_server::Server) with a real [`Client`], because the whole
//! point of the wave is that the profiler is a *wire* capability, not an in-process one. The engine
//! they serve registers one named system so the per-system section has a game system to report beside
//! the engine's always-installed built-ins.
//!
//! Timing discipline: the sim is paced to a high speed so a 60-frame window closes in well under a
//! second, and every wait is a generous deadline rather than a sleep — flaky tests are P0. The two
//! transport tests are the exception and run at the engine's own pace; they say why.

use std::time::Duration;

use ludion_client::{CallOk, Client};
use ludion_core::diagnostics::Diagnostic;
use ludion_engine::Engine;
use ludion_protocol::command::SimSetSpeedParams;
use ludion_protocol::frame::FrameFormat;
use ludion_protocol::perf::{ProfileCaptureParams, TransportProfile};
use ludion_protocol::{Channel, Command, Event, ResponseBody, SubscribeMode};

/// A scene with nothing in it: the profiler measures the schedule and the host spans, not content.
const SCENE: &str = "format: 1\nseed: 7\nentities: []\n";

/// The name the served engine's one system is registered under.
const SYSTEM: &str = "profiled_noop";

/// The served schedule, in run order: the game's system between the engine's two always-installed
/// built-ins.
const SCHEDULE: [&str; 3] = ["path_follow", SYSTEM, "camera_rig"];

/// How long a window may take to arrive before the test calls it a failure. Far above the ~0.2 s a
/// 60-frame window takes at the speed these tests set — it bounds a stall, it is never the metric.
const WINDOW_TIMEOUT: Duration = Duration::from_secs(20);

/// Serves an engine that runs one named system, small enough to render cheaply (arming the profiler
/// makes the loop render even with no frame subscriber, so the size matters).
fn serve() -> ludion_server::Server {
    let mut engine: Engine = Engine::builder()
        .render_size(64, 64)
        .system(SYSTEM, |_world, _ctx| {})
        .build()
        .expect("the engine builds");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    ludion_server::Server::bind(engine, "127.0.0.1:0").expect("server binds")
}

/// Connects and paces the sim fast enough that windows close promptly.
fn client(server: &ludion_server::Server) -> Client {
    let mut client = Client::connect(&server.url()).expect("handshake succeeds");
    client
        .call(Command::SimSetSpeed(SimSetSpeedParams { speed: 20.0 }))
        .expect("speed is accepted");
    client
}

/// Whether the host has a GPU adapter, probed the way the delivery tests do (`L2003` is the
/// graceful-skip signal). Without one nothing is ever rendered, so nothing is encoded and there is no
/// transport layer to measure — the adapterless-CI half of the suite skips those tests.
fn gpu_available(client: &mut Client) -> bool {
    match client.screenshot(Some(32), Some(32)) {
        Ok(_) => true,
        Err(diag) => !ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&diag)),
    }
}

/// Checks the invariants every filled transport section holds, whatever produced it.
fn assert_transport_is_consistent(transport: &TransportProfile, window_frames: u64) {
    assert!(
        transport.sampled_frames >= 1,
        "a filled section describes at least one delivered frame"
    );
    assert!(
        transport.sampled_frames <= window_frames,
        "a window cannot deliver more frames than it measured"
    );
    assert!(
        transport.encode_median_ms > 0.0,
        "encoding a frame takes measurable time"
    );
    assert!(transport.send_median_ms >= 0.0);
}

/// Waits for the next `profile` window event, panicking with a readable reason if none arrives.
fn next_window(client: &mut Client) -> ludion_protocol::perf::ProfileWindowData {
    match client.next_event(WINDOW_TIMEOUT) {
        Ok(Some(message)) => match message.event {
            Event::ProfileWindow(window) => *window,
            other => panic!("expected a profile window on the profile channel, got {other:?}"),
        },
        Ok(None) => panic!("no profile window arrived within {WINDOW_TIMEOUT:?}"),
        Err(diag) => panic!("the profile subscription failed: {diag}"),
    }
}

/// **The channel is the switch.** Subscribing to `profile` starts the windows; consecutive windows
/// tile the frame axis with no gap, and each carries both medians and the raw columns a timeline
/// needs.
#[test]
fn needs_gpu_subscribing_streams_gapless_windows_with_medians_and_samples() {
    let server = serve();
    let mut client = client(&server);
    let sub = client
        .subscribe(Channel::Profile, SubscribeMode::default(), FrameFormat::Qoi)
        .expect("the profile channel accepts a subscription");

    let first = next_window(&mut client);
    let second = next_window(&mut client);

    for window in [&first, &second] {
        assert!(!window.capture, "a streamed window is not a capture");
        assert_eq!(
            window.frames,
            window.last_frame - window.first_frame + 1,
            "a window's frame count spans exactly its own frames"
        );
        // The system the served engine registered is measured by name.
        assert_eq!(
            window
                .systems
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            SCHEDULE,
        );
        // The live tick's two sequential spans, in their fixed order.
        assert_eq!(
            window
                .sequential
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["extract", "submit"],
        );
        assert!(window.frame_total_median_ms >= 0.0);
        // Medians cannot make a timeline; the columns can, and they are one entry per frame.
        let samples = &window.samples;
        assert_eq!(samples.frames.len() as u64, window.frames);
        assert_eq!(samples.frame_total_ms.len() as u64, window.frames);
        assert_eq!(samples.system_names, SCHEDULE);
        assert_eq!(samples.system_ms[0].len() as u64, window.frames);
        assert_eq!(samples.frames[0], window.first_frame);
        // No `frames` subscription, so no frame was ever encoded or sent: the server's transport
        // section is *absent*, not a section full of zeroes.
        assert!(window.transport.is_none());
    }

    assert_eq!(
        second.first_frame,
        first.last_frame + 1,
        "consecutive windows must tile the frame axis with no gap and no overlap"
    );

    client.unsubscribe(sub).expect("unsubscribe succeeds");
}

/// **A capture answers with a report.** The reply is deferred until the requested frames have run,
/// arrives against the request's own id, and carries the same shape the offline driver writes —
/// while the rolling stream keeps flowing underneath it.
#[test]
fn needs_gpu_a_capture_answers_with_a_report_while_windows_keep_streaming() {
    let server = serve();
    let mut client = client(&server);
    let sub = client
        .subscribe(Channel::Profile, SubscribeMode::default(), FrameFormat::Qoi)
        .expect("subscribes");
    // One window first, so the stream is demonstrably running before the capture is issued.
    let before = next_window(&mut client);

    let ok = client
        .call(Command::ProfileCapture(ProfileCaptureParams {
            frames: 30,
            include_samples: true,
        }))
        .expect("the capture is accepted and answered");
    let ResponseBody::ProfileReport { report } = ok.body else {
        panic!(
            "profile.capture must answer with a profile_report body, got {:?}",
            ok.body
        );
    };

    assert_eq!(report.measured_frames, 30);
    assert_eq!(report.warmup_frames, 0, "a live capture warms nothing up");
    assert_eq!(
        report
            .sim
            .systems
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        SCHEDULE,
        "a capture reports the running game's own systems — the gap the offline driver cannot close"
    );
    let samples = report.samples.expect("include_samples was set");
    assert_eq!(samples.frames.len(), 30);
    assert!(
        samples.frames[0] > before.first_frame,
        "the capture measured later frames"
    );
    assert!(
        report.transport.is_none(),
        "nothing was delivered to a viewer, so there is no transport to report"
    );
    if ludion_core::require_gpu::skip_no_adapter(!report.gpu.available) {
        eprintln!("SKIPPED: no GPU samples reached the capture (adapterless host)");
    } else {
        assert!(
            !report.gpu.passes.is_empty(),
            "a capture that sampled the GPU names the passes it sampled"
        );
    }

    // The rolling stream was never interrupted by the capture.
    let after = next_window(&mut client);
    assert!(after.first_frame > before.last_frame);
    client.unsubscribe(sub).expect("unsubscribe succeeds");
}

/// **One capture at a time.** A second `profile.capture` issued while one is armed is refused with
/// `L4013` rather than quietly measuring the same frames twice.
///
/// Two connections, because a connection answers its requests in order: the first capture's reply is
/// deferred, so its own connection is busy until it lands. That serialization is the reason the
/// method's docs tell a client that wants both to use a second connection.
#[test]
fn needs_gpu_a_second_capture_is_refused_while_one_is_in_flight() {
    let server = serve();
    let mut holder = Client::connect(&server.url()).expect("handshake succeeds");
    // At 1/64 speed 30 frames take about 32 s: the window is accepted but stays open for the whole
    // test (a paused sim would refuse the capture instead — see the paused-sim test below).
    holder
        .call(Command::SimSetSpeed(SimSetSpeedParams {
            speed: 1.0 / 64.0,
        }))
        .expect("slows");

    // The client gives up waiting; the engine keeps the capture armed, which is the state under test.
    holder.set_call_timeout(Some(Duration::from_secs(1)));
    let timed_out = holder.call(Command::ProfileCapture(ProfileCaptureParams {
        frames: 30,
        include_samples: false,
    }));
    assert!(
        timed_out.is_err(),
        "a 32 s window outlasts the client's own 1 s timeout"
    );

    let mut second = Client::connect(&server.url()).expect("a second connection");
    second.set_call_timeout(Some(Duration::from_secs(10)));
    let refused = second
        .call(Command::ProfileCapture(ProfileCaptureParams {
            frames: 30,
            include_samples: false,
        }))
        .expect_err("a second capture is refused");
    assert_eq!(refused.code.as_str(), "L4013", "{refused}");
}

fn capture(client: &mut Client, frames: u64) -> Result<CallOk, Diagnostic> {
    client.call(Command::ProfileCapture(ProfileCaptureParams {
        frames,
        include_samples: false,
    }))
}

/// A connected client whose sim is paused and has just been refused a capture.
fn refused_paused_capture(server: &ludion_server::Server) -> Client {
    let mut client = Client::connect(&server.url()).expect("handshake succeeds");
    client.set_call_timeout(Some(Duration::from_secs(5)));
    client.call(Command::SimPause).expect("pauses");
    let refused = capture(&mut client, 60).expect_err("a paused capture is refused");
    assert_eq!(refused.code.as_str(), "L4023", "{refused}");
    client
}

/// **A capture on a paused sim is refused at once** with `L4023`, not answered 120 s later as a
/// stalled sim.
#[test]
fn a_capture_on_a_paused_sim_is_refused_at_once() {
    let server = serve();
    refused_paused_capture(&server);
}

/// **A refused paused capture leaves nothing in flight**: after a resume the next capture arms
/// and answers, rather than drawing `L4013`. The capture renders, so it needs a device.
#[test]
fn needs_gpu_a_refused_paused_capture_leaves_nothing_in_flight() {
    let server = serve();
    let mut client = refused_paused_capture(&server);
    client.call(Command::SimResume).expect("resumes");
    if !gpu_available(&mut client) {
        return;
    }
    client
        .call(Command::SimSetSpeed(SimSetSpeedParams { speed: 20.0 }))
        .expect("speeds up");
    let report = capture(&mut client, 2).expect("the next capture arms, not L4013");
    assert!(matches!(report.body, ResponseBody::ProfileReport { .. }));
}

/// **An unmeasurable window is refused up front**, with a `L4012` naming the parameter, rather than
/// hanging until the transport calls the sim thread wedged.
#[test]
fn a_zero_frame_capture_is_rejected() {
    let server = serve();
    let mut client = Client::connect(&server.url()).expect("handshake succeeds");
    let diag = client
        .call(Command::ProfileCapture(ProfileCaptureParams {
            frames: 0,
            include_samples: false,
        }))
        .expect_err("zero frames is refused");
    assert_eq!(diag.code.as_str(), "L4012", "{diag}");
    assert_eq!(diag.path.as_deref(), Some("/params/frames"));

    let diag = client
        .call(Command::ProfileCapture(ProfileCaptureParams {
            frames: ludion_protocol::perf::MAX_CAPTURE_FRAMES + 1,
            include_samples: false,
        }))
        .expect_err("an over-long capture is refused");
    assert_eq!(diag.code.as_str(), "L4012", "{diag}");
}

/// **The layer the engine cannot measure.** With a viewer attached, a `profile` window arrives with
/// its `transport` section already filled — encode and send happen in the server, downstream of the
/// frame the sim produced, and are injected before the event is serialized. The per-frame columns
/// span the whole window and read `null` for a frame no viewer was delivered, the same convention the
/// GPU columns use.
///
/// Real time (no `sim.set_speed`): the frame a readback completes for is one to three ticks behind
/// the tick that submitted it, so at the engine's own pace a window closes *after* its frames have
/// been encoded. A sped-up sim would file those samples against windows that had already gone out.
#[test]
fn needs_gpu_a_delivered_frame_fills_the_windows_transport_section() {
    let server = serve();
    let mut client = Client::connect(&server.url()).expect("handshake succeeds");
    if !gpu_available(&mut client) {
        eprintln!("skipped: no GPU adapter, so no frame is delivered and there is no transport");
        return;
    }
    let frames = client
        .subscribe(Channel::Frames, SubscribeMode::default(), FrameFormat::Qoi)
        .expect("the frames channel accepts a subscription");
    let profile = client
        .subscribe(Channel::Profile, SubscribeMode::default(), FrameFormat::Qoi)
        .expect("the profile channel accepts a subscription");

    // The first window can close before the first readback completes; take the first that covers
    // delivered frames.
    let mut filled = None;
    for _ in 0..3 {
        let window = next_window(&mut client);
        assert_eq!(
            window.samples.frames.len() as u64,
            window.frames,
            "the frame column is what the transport columns align to"
        );
        if let Some(transport) = &window.transport {
            assert_eq!(
                transport.encode_ms.len() as u64,
                window.frames,
                "the encode column spans the window, one entry per measured frame"
            );
            assert_eq!(transport.send_ms.len() as u64, window.frames);
            assert_eq!(
                transport.encode_ms.iter().flatten().count() as u64,
                transport.sampled_frames,
                "`sampled_frames` counts exactly the non-null column entries"
            );
            filled = Some(window);
            break;
        }
    }
    let window = filled.expect("a window covering delivered frames arrives");
    assert_transport_is_consistent(
        window.transport.as_ref().expect("checked above"),
        window.frames,
    );

    client.unsubscribe(profile).expect("unsubscribe succeeds");
    client.unsubscribe(frames).expect("unsubscribe succeeds");
}

/// **A capture reports transport too.** The deferred `profile.capture` reply is completed on its way
/// back through the connection, so one call answers all four layers — the question the 2026-07-15
/// hand-instrumentation had to answer by hand.
#[test]
fn needs_gpu_a_capture_reply_carries_the_transport_section() {
    let server = serve();
    let mut client = Client::connect(&server.url()).expect("handshake succeeds");
    if !gpu_available(&mut client) {
        eprintln!("skipped: no GPU adapter, so no frame is delivered and there is no transport");
        return;
    }
    client.set_call_timeout(Some(Duration::from_secs(30)));
    let frames = client
        .subscribe(Channel::Frames, SubscribeMode::default(), FrameFormat::Qoi)
        .expect("the frames channel accepts a subscription");

    let ok = client
        .call(Command::ProfileCapture(ProfileCaptureParams {
            frames: 60,
            include_samples: true,
        }))
        .expect("the capture is accepted and answered");
    let ResponseBody::ProfileReport { report } = ok.body else {
        panic!("profile.capture must answer with a profile_report body");
    };
    let transport = report
        .transport
        .as_ref()
        .expect("a capture with a viewer attached measures transport");
    assert_transport_is_consistent(transport, report.measured_frames);
    let samples = report.samples.as_ref().expect("include_samples was set");
    assert_eq!(
        transport.encode_ms.len(),
        samples.frames.len(),
        "the transport columns align to the report's own frame column"
    );

    client.unsubscribe(frames).expect("unsubscribe succeeds");
}
