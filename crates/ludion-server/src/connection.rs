//! The TCP accept loop and the per-connection tasks: handshake, request→queue→reply shuttling, and
//! subscription delivery.
//!
//! Every connection shares the one intake channel (the whole point — no other path to the world). A
//! connection is three cooperating tasks over its split WebSocket:
//!
//! - the **reader** ([`handle_connection`]) decodes text requests *continuously*: each becomes a
//!   `StampedCommand` pushed into the intake channel and a [`Pending`] entry queued for the pump, and
//!   then it reads again. It never waits for a sim reply, so a client may pipeline — reply age is
//!   bounded by the live loop's pass rate rather than growing with every request already sent (the
//!   sim drains every pending command at each pass boundary). At most [`PIPELINE_DEPTH`] requests are
//!   in flight; past that the push waits, the reader stops reading, and the client feels TCP
//!   backpressure — bounded by construction, never a buffer that grows without end;
//! - the **reply pump** ([`reply_pump`]) pops those entries strictly FIFO and answers them in that
//!   order: it awaits each one-shot reply on the tokio blocking pool (so a runtime worker is never
//!   parked on the sim thread) under a bound that keeps a wedged sim thread from parking that pool
//!   thread for good, and queues the [`Response`] on the outbound sink. It awaits one entry at a time,
//!   so the pump itself holds at most one blocking-pool thread. A `stream.subscribe` is the exception
//!   to "the pump awaits the reply": the sim answers it at its command drain and starts emitting for
//!   the new sub in the same pass, so the reader hands it to its own task ([`spawn_resolution`]),
//!   which registers the sub in the shared [`Registry`] the moment the reply lands (the registry's
//!   in-flight buffer covers the rest of the race — see [`delivery`]) and passes the finished
//!   [`Response`] to the pump through a one-shot in the request's slot, so replies still leave in
//!   request order. Each in-flight subscribe holds one blocking-pool thread of its own, bounded by
//!   [`PIPELINE_DEPTH`]. The pump removes a sub when its `stream.unsubscribe` reply passes; and
//! - the **writer** ([`writer_task`]) solely owns the WebSocket sink, draining the connection's *two*
//!   outbound lanes — text (request replies and events) and binary (rendered frames) — with the text
//!   lane taking priority, so a reply overtakes queued frames instead of waiting out the megabytes
//!   ahead of it. Order *within* a lane is untouched: replies keep per-connection **request** order
//!   among themselves (the pump answers FIFO — a request refused before dispatch keeps its slot — and
//!   the writer drains in order), and events keep their `seq` order.
//!
//! The same reply-inspection seam is where transport profiling is armed: a `profile` subscription
//! holds a [`ProfileArm`] for its life and a `profile.capture` holds one from the moment the reader
//! accepts it — before dispatch, because the frames it measures start running the moment the sim
//! thread takes the command — across its deferred reply, which the writer's send span and the
//! router's encode span record into (see [`perf`](crate::perf)). The pump fills a finished capture's
//! `transport` section before the reply is serialized.
//!
//! On disconnect the reader first releases every sub the connection holds ([`release_subs`]): it
//! unregisters each *and* submits a `stream.unsubscribe` through the one command queue, so the engine
//! stops rendering for a viewer that is gone at once, not after the replies still held ahead of it —
//! cleanup rides the same queue as everything else (no server-private path). It then closes the
//! pending queue and the pump drains what is left; a subscribe still resolving by then cancels itself.

use std::future::Future;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{RecvTimeoutError, Sender};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_engine::Dispatched;
use ludion_engine::live::{CommandResult, ReplyReceiver, StampedCommand};
use ludion_protocol::command::{Channel, StreamUnsubscribeParams, SubscribeMode};
use ludion_protocol::frame::FrameFormat;
use ludion_protocol::handshake::{Hello, ServerHello};
use ludion_protocol::{
    Command, MAX_MESSAGE_BYTES, PROTOCOL_VERSION, Request, Response, ResponseBody, SubId,
    WireResult,
};
use serde_json::Value;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::accept_hdr_async_with_config;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::error::{CapacityError, Error as WsError};
use tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as HandshakeRequest, Response as HandshakeResponse,
};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::delivery::{self, BinaryFrame, OutboundSink, Registry, SubscribeInFlight};
use crate::perf::{DeliveredFrame, ProfileArm, ProfileHub};

/// A live-connection counter guard: increments on construction, decrements on drop. Held by each
/// connection task so [`Server`](crate::Server) teardown can wait for in-flight connection work to
/// drain — flushing a just-sent reply (e.g. `engine.shutdown`'s `Ack`) to the socket — before it drops
/// the runtime, which would otherwise cancel the writer task mid-write. Drop-based so a panicking
/// connection task still decrements.
pub(crate) struct ConnectionGuard(Arc<AtomicUsize>);

impl ConnectionGuard {
    fn new(active: Arc<AtomicUsize>) -> Self {
        active.fetch_add(1, Ordering::AcqRel);
        ConnectionGuard(active)
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The accept half of a listener, so the loop's failure handling can be driven by a listener that
/// fails on demand. Production always passes the real [`TcpListener`]; the peer address is dropped
/// here because nothing downstream reads it.
pub(crate) trait Accept: Send + Sync + 'static {
    /// Yields the next incoming connection, or the operating system's reason it could not.
    fn accept(&self) -> impl Future<Output = io::Result<TcpStream>> + Send;
}

impl Accept for TcpListener {
    // `TcpListener::accept` here is the *inherent* method — a path resolves inherent impls before
    // trait ones, so this delegates rather than recursing.
    async fn accept(&self) -> io::Result<TcpStream> {
        TcpListener::accept(self)
            .await
            .map(|(stream, _peer)| stream)
    }
}

/// How the accept loop answers a listener that returns errors: how long to wait before retrying, and
/// how many failures in a row mean the listener is not coming back.
#[derive(Clone, Copy)]
pub(crate) struct AcceptPolicy {
    backoff: Duration,
    failure_limit: u32,
}

impl AcceptPolicy {
    /// The shipped policy.
    ///
    /// `backoff` is what separates the two failure shapes: a peer that vanished mid-handshake costs
    /// the next client 100 ms of connect latency at worst, while a listener that fails instantly and
    /// forever (descriptor exhaustion, a socket gone bad) is held to ten wake-ups a second instead of
    /// spinning a core. `failure_limit` is therefore only reached after roughly six seconds of *every*
    /// accept failing — far past any burst of dropped clients, and past the descriptor pressure a
    /// closing connection relieves on its own.
    pub(crate) const DEFAULT: AcceptPolicy = AcceptPolicy {
        backoff: Duration::from_millis(100),
        failure_limit: 64,
    };
}

/// Accepts connections until the shutdown watch flips, spawning one task per connection.
///
/// A listener that errors is retried under [`AcceptPolicy::DEFAULT`] and reported as `L3022`; one that
/// never recovers ends the loop and stops the host, so an unserviceable listener is loud and idle
/// rather than silent and spinning.
pub(crate) async fn accept_loop(
    listener: TcpListener,
    intake: Sender<StampedCommand>,
    hello: ServerHello,
    registry: Registry,
    hub: Arc<ProfileHub>,
    active: Arc<AtomicUsize>,
    shutdown: watch::Receiver<bool>,
) {
    accept_loop_with(
        listener,
        intake,
        hello,
        registry,
        hub,
        active,
        shutdown,
        AcceptPolicy::DEFAULT,
    )
    .await;
}

/// [`accept_loop`] over any [`Accept`] and any [`AcceptPolicy`].
#[allow(clippy::too_many_arguments)] // one owned seam per collaborator; a bag struct would hide them
async fn accept_loop_with<L: Accept>(
    listener: L,
    intake: Sender<StampedCommand>,
    hello: ServerHello,
    registry: Registry,
    hub: Arc<ProfileHub>,
    active: Arc<AtomicUsize>,
    mut shutdown: watch::Receiver<bool>,
    policy: AcceptPolicy,
) {
    // Consecutive failures, and the error class already reported: a listener failing for one reason
    // says so once, not once per retry.
    let mut failures: u32 = 0;
    let mut reported: Option<io::ErrorKind> = None;
    loop {
        tokio::select! {
            // Prefer observing shutdown over accepting a new connection during teardown.
            biased;
            changed = shutdown.changed() => {
                // A value change (to true) or a dropped sender both mean "stop accepting".
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            accepted = listener.accept() => match accepted {
                Ok(stream) => {
                    failures = 0;
                    reported = None;
                    let guard = ConnectionGuard::new(active.clone());
                    let (intake, hello, registry, hub, shutdown) = (
                        intake.clone(),
                        hello.clone(),
                        registry.clone(),
                        hub.clone(),
                        shutdown.clone(),
                    );
                    tokio::spawn(async move {
                        handle_connection(stream, intake, hello, registry, hub, shutdown).await;
                        drop(guard); // decrement only once the connection has fully drained + closed
                    });
                }
                Err(error) => {
                    failures += 1;
                    if reported != Some(error.kind()) {
                        reported = Some(error.kind());
                        eprintln!("{}", accept_failed(&error));
                    }
                    if failures >= policy.failure_limit {
                        eprintln!("{}", accept_abandoned(&error, failures));
                        // The one stop seam: the same `engine.shutdown` a wire client sends, through
                        // the same queue, so whoever waits on `Server::wire_shutdown` runs the
                        // ordinary graceful teardown. The reply is discarded — nobody asked.
                        let (stamped, _reply) = StampedCommand::new(Command::EngineShutdown);
                        let _ = intake.send(stamped);
                        break;
                    }
                    // Back off before retrying, but let teardown cut the wait short.
                    tokio::select! {
                        biased;
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() {
                                break;
                            }
                        }
                        () = tokio::time::sleep(policy.backoff) => {}
                    }
                }
            },
        }
    }
}

/// One connection: spawn its writer, perform the handshake, then read requests continuously into the
/// bounded pipeline its [`reply_pump`] answers from, until close or shutdown.
async fn handle_connection(
    stream: TcpStream,
    intake: Sender<StampedCommand>,
    hello: ServerHello,
    registry: Registry,
    hub: Arc<ProfileHub>,
    mut shutdown: watch::Receiver<bool>,
) {
    // One deadline for the upgrade and the `Hello` read: a socket that never gets that far is dropped.
    let deadline = tokio::time::Instant::now() + HANDSHAKE_TIMEOUT;
    let upgrade = accept_hdr_async_with_config(stream, check_origin, Some(websocket_config()));
    let ws = match until_shutdown_or(upgrade, deadline, &mut shutdown).await {
        Some(Ok(ws)) => ws,
        // Not a WebSocket client, a refused origin, a stalled upgrade or a teardown; nothing to answer.
        Some(Err(_)) | None => return,
    };
    let (sink, mut stream) = ws.split();

    // Two outbound lanes feed the sole writer of the sink: text (replies, events) and binary frames.
    let (outbound, text_rx, frames_rx) = delivery::outbound_channel();
    let writer = tokio::spawn(writer_task(sink, text_rx, frames_rx, hub.clone()));

    // --- Handshake: the first message must be a Hello. -----------------------------------------
    // No sub can be owned yet, so the three exits below have nothing to release.
    let first = match tokio::time::timeout_at(deadline, read_text(&mut stream, &mut shutdown)).await
    {
        Ok(Some(text)) => text,
        // The client closed, teardown began, or no `Hello` came within the handshake deadline.
        Ok(None) | Err(_) => {
            finish(writer, outbound).await;
            return;
        }
    };
    match parse_hello(&first) {
        Ok(_client) => {
            // `serde_json::to_string` keeps its `Serialize` bound internal, so this crate needs no
            // direct `serde` dependency; the `"{}"` fallback is a defensive floor, never reached for
            // these always-serializable wire types.
            let text = serde_json::to_string(&hello).unwrap_or_else(|_| "{}".to_string());
            if outbound.text.send(text).await.is_err() {
                finish(writer, outbound).await;
                return;
            }
        }
        Err(diag) => {
            // A wrong first message or bad version: answer with a structured error, then close.
            // The id is 0 by the null-id convention (a handshake message carries no request id).
            let text = serde_json::to_string(&Response::error(0, diag))
                .unwrap_or_else(|_| "{}".to_string());
            let _ = outbound.text.send(text).await;
            finish(writer, outbound).await;
            return;
        }
    }

    // --- Request loop. --------------------------------------------------------------------------
    // The pump takes the connection's last outbound sender with it, so the writer sees the queue
    // close exactly when the pump has finished answering — not when the reader stops reading.
    let (pending_tx, pending_rx) = mpsc::channel::<Pending>(PIPELINE_DEPTH);
    let shared = ConnShared {
        intake: intake.clone(),
        registry: registry.clone(),
        hub: hub.clone(),
        outbound: outbound.clone(),
        subs: Arc::new(Mutex::new(ConnSubs::default())),
    };
    let pump = tokio::spawn(reply_pump(
        pending_rx,
        registry.clone(),
        shared.subs.clone(),
        outbound,
        writer,
        shutdown.clone(),
    ));
    while let Some(text) = read_text(&mut stream, &mut shutdown).await {
        let accepted = match accept_request(&text, &shared) {
            // A subscribe resolves on its own task so its sub registers the moment the sim answers,
            // not when the pump gets to its slot; the slot still takes the reply in order.
            Pending::Reply(mut entry) => match entry.subscribe.take() {
                Some(in_flight) => {
                    spawn_resolution(entry, in_flight, &shared, &shutdown, SIM_REPLY_TIMEOUT)
                }
                None => Pending::Reply(entry),
            },
            other => other,
        };
        // The only place the reader waits: with `PIPELINE_DEPTH` requests already in flight it stops
        // reading, and the client is held by TCP instead of the server buffering without end.
        if pending_tx.send(accepted).await.is_err() {
            break; // the pump has ended; nothing would answer what we read next
        }
    }
    // The socket is gone: release its subs now, not after the pump has waited out the replies still
    // held ahead of them, so the engine stops rendering for a viewer that no longer exists.
    release_subs(&shared);
    drop(shared);
    // Closing the queue is the pump's cue to drain what is left and close the lanes.
    drop(pending_tx);
    let _ = pump.await;
}

/// What a connection's request tasks share: the one command queue, the delivery registry, the
/// profile hub, the outbound lanes and the subs the connection owns.
#[derive(Clone)]
struct ConnShared {
    intake: Sender<StampedCommand>,
    registry: Registry,
    hub: Arc<ProfileHub>,
    outbound: OutboundSink,
    subs: Arc<Mutex<ConnSubs>>,
}

/// The subs a connection owns, for teardown. `closed` is set when the reader has exited: a subscribe
/// that resolves after that must not register, only be cancelled.
#[derive(Default)]
struct ConnSubs {
    owned: Vec<OwnedSub>,
    closed: bool,
}

/// How many requests one connection may have accepted but not yet answered.
///
/// The reader pushes each accepted request onto this queue and goes straight back to the socket, so a
/// client sending faster than the live loop's pass rate has its excess bounded *here* instead of
/// accumulating in the kernel's receive buffer with nothing to cap it. At the bound the push waits,
/// the reader stops reading, and TCP holds the client. Deep enough that a client pipelining a display
/// frame's worth of input never stalls, shallow enough that what one connection can pin is a handful
/// of small pending values.
pub(crate) const PIPELINE_DEPTH: usize = 64;

/// One accepted request, queued in socket order for the reply pump to answer.
enum Pending {
    /// It reached the sim thread; its one-shot reply is still to come.
    Reply(PendingReply),
    /// A `stream.subscribe` resolved by its own task (see [`spawn_resolution`]); the slot takes the
    /// finished reply when the pump reaches it.
    Resolving {
        id: u64,
        rx: oneshot::Receiver<Response>,
    },
    /// It was answered before dispatch (it did not parse, or the queue is gone). It still takes its
    /// place in the queue, so an error keeps its slot in the connection's reply order.
    Ready(Response),
}

/// A dispatched request waiting on the sim thread, with everything its reply needs.
struct PendingReply {
    /// The client's correlation id, echoed on the response.
    id: u64,
    /// The method name, for the `L3023` diagnostic.
    method: &'static str,
    /// The one-shot the sim thread replies on.
    reply_rx: ReplyReceiver,
    /// What the request was, for the delivery registry once the reply is known.
    observed: Observed,
    /// A `profile.capture`'s arming guard, held from before dispatch until its reply is filled.
    capture: Option<ProfileArm>,
    /// A `stream.subscribe`'s in-flight count, held from before dispatch; the reader hands it to the
    /// task that resolves the subscribe.
    subscribe: Option<SubscribeInFlight>,
    /// When the reader accepted it: the [`SIM_REPLY_TIMEOUT`] deadline runs from here, not from when
    /// the pump reached the entry.
    enqueued_at: Instant,
}

/// The reply pump: answer one connection's accepted requests in the order it read them.
///
/// Popping and awaiting strictly FIFO is what makes the module's ordering promise hold while the
/// reader runs ahead — including for a request refused before dispatch, which occupies its slot as a
/// ready-made [`Response`]. It is also what keeps the connection to one blocking-pool thread at a
/// time, exactly as the old read-one-answer-one loop did.
///
/// An unsubscribe reply is observed here, as it passes; a subscribe is not — it registers when the sim
/// answers (see [`spawn_resolution`]), and this only takes its finished reply in its slot. When the
/// reader drops the queue's sender this drains what is left, then runs [`finish`]; the subs were
/// already released by the reader.
async fn reply_pump(
    mut pending: mpsc::Receiver<Pending>,
    registry: Registry,
    subs: Arc<Mutex<ConnSubs>>,
    outbound: OutboundSink,
    writer: tokio::task::JoinHandle<()>,
    mut shutdown: watch::Receiver<bool>,
) {
    while let Some(entry) = pending.recv().await {
        let (response, observed) = match entry {
            Pending::Ready(response) => (response, Observed::Other),
            Pending::Resolving { id, rx } => (
                rx.await
                    .unwrap_or_else(|_| Response::error(id, engine_unavailable())),
                Observed::Other,
            ),
            Pending::Reply(entry) => {
                let mut response = await_reply(
                    entry.reply_rx,
                    entry.id,
                    entry.method,
                    &mut shutdown,
                    entry.enqueued_at,
                    SIM_REPLY_TIMEOUT,
                )
                .await
                .response;
                if let Some(capture) = &entry.capture {
                    capture.fill_capture_reply(&mut response);
                }
                (response, entry.observed)
            }
        };
        // Unregister delivery as the unsubscribe reply passes through (see `Observed`).
        apply_unsubscribe(&registry, &subs, &observed, &response);
        let text = reply_text(&response, MAX_MESSAGE_BYTES);
        if outbound.text.send(text).await.is_err() {
            break; // the writer is gone; the rest of the queue has nowhere to go
        }
    }

    finish(writer, outbound).await;
}

/// How many text messages the writer may send back-to-back before the frame lane gets a turn.
///
/// The text bias is what makes a reply overtake queued frames, and this bounds it so the reverse
/// cannot happen: a request flood can delay frames by at most this many small writes. It rarely
/// engages — accepted requests are capped at [`PIPELINE_DEPTH`] and answered at the sim's pass rate —
/// but without it a client that never stops asking could starve its own video.
const TEXT_BURST: usize = 8;

/// The writer task: sole owner of the WebSocket sink, draining the connection's two outbound lanes.
///
/// Text (replies and events) is drained in preference to binary (encoded frames), so a reply is never
/// stuck behind the megabytes of frames already queued — the whole point of the split. The bias is
/// bounded by [`TEXT_BURST`], and order *within* each lane is exactly the order it was queued in.
/// Ends when both lanes are closed (every sender dropped: the reader gone, the registry entry
/// removed, every forwarder finished) or a send fails; it closes the sink on the way out.
///
/// # The `send` span
///
/// While the profile hub is armed, each *binary* message's write is timed and filed under the frame
/// it depicts. The span covers exactly one thing: handing the frame's bytes to the WebSocket sink —
/// framing plus whatever of the write the socket accepts before the future completes, including the
/// backpressure of a client that is not reading. It is **not** kernel-to-client delivery time: the
/// wire is not observable from here, and nothing in this process learns when the peer received the
/// bytes. Text messages (replies, events) are not timed — `send_ms` is a per-frame transport cost.
async fn writer_task(
    mut sink: SplitSink<WebSocketStream<TcpStream>, Message>,
    mut text_rx: mpsc::Receiver<String>,
    mut frames_rx: mpsc::Receiver<BinaryFrame>,
    hub: Arc<ProfileHub>,
) {
    // A closed lane's `recv` returns `None` immediately and forever, so a closed lane must be taken
    // out of the select by hand — a `biased` loop over one would otherwise spin a core.
    let mut text_done = false;
    let mut frames_done = false;
    let mut text_run = 0usize;
    loop {
        if text_run >= TEXT_BURST && !frames_done {
            // Cash in the burst bound with a *non-blocking* turn for the frame lane. Deliberately not
            // a `select!` guard on the text arm: with no frame pending that would leave the loop
            // waiting on a lane that has nothing to say while replies queue up behind it.
            text_run = 0;
            match frames_rx.try_recv() {
                Ok(frame) => {
                    if !write_frame(&mut sink, &hub, frame).await {
                        break;
                    }
                    continue;
                }
                Err(mpsc::error::TryRecvError::Empty) => {}
                Err(mpsc::error::TryRecvError::Disconnected) => frames_done = true,
            }
        }
        if text_done && frames_done {
            break;
        }
        tokio::select! {
            // Replies and events first: a reply must not wait out the frames already queued.
            biased;
            text = text_rx.recv(), if !text_done => match text {
                Some(text) => {
                    text_run += 1;
                    if sink.send(Message::text(text)).await.is_err() {
                        break;
                    }
                }
                None => text_done = true,
            },
            frame = frames_rx.recv(), if !frames_done => match frame {
                Some(frame) => {
                    text_run = 0;
                    if !write_frame(&mut sink, &hub, frame).await {
                        break;
                    }
                }
                None => frames_done = true,
            },
        }
    }
    let _ = sink.close().await;
}

/// Writes one encoded frame to the sink, timing the send under the frame it depicts while the profile
/// hub is armed (see [`writer_task`]'s `send` span). Returns `false` once the sink has failed and the
/// writer must stop.
async fn write_frame(
    sink: &mut SplitSink<WebSocketStream<TcpStream>, Message>,
    hub: &ProfileHub,
    frame: BinaryFrame,
) -> bool {
    let stamp = DeliveredFrame {
        timeline: frame.timeline,
        frame: frame.frame,
    };
    let timed = hub.armed().then(|| (stamp, Instant::now()));
    let sent = sink.send(Message::binary(frame.bytes)).await;
    if let Some((stamp, started)) = timed {
        hub.record_send(stamp, started.elapsed());
    }
    sent.is_ok()
}

/// What a processed request was, for the delivery registry and the profile hub: a subscribe (carrying
/// the channel + mode + frame format to register), an unsubscribe (carrying the sub to remove), or a
/// `profile.capture` (which arms transport timing for its whole deferred flight). Everything else is
/// [`Observed::Other`].
enum Observed {
    /// A `stream.subscribe`: register the reply's minted sub with this channel + mode + format.
    Subscribe {
        channel: Channel,
        mode: SubscribeMode,
        format: FrameFormat,
    },
    /// A `stream.unsubscribe`: unregister this sub once the reply confirms it.
    Unsubscribe { sub: SubId },
    /// A `profile.capture`: time transport spans until its (deferred) reply comes back.
    Capture,
    /// Any other command: nothing to (un)register.
    Other,
}

/// One subscription this connection owns, and whatever it keeps alive.
struct OwnedSub {
    /// The engine-minted id, for registry removal and the teardown unsubscribe.
    sub: SubId,
    /// Held for a `profile` subscription only: subscribing to that channel is the arming switch on
    /// the engine side, and this is the same switch for the server's transport spans. Dropped by an
    /// unsubscribe or by teardown, which is what makes disconnect disarm for free.
    arm: Option<ProfileArm>,
}

/// Resolves a dispatched `stream.subscribe` on its own task, off the reply FIFO.
///
/// The sim answers a subscribe at its command drain and starts emitting for the new sub in the same
/// pass, so waiting for the pump to reach the reply would lose that traffic to any request ahead of it.
/// The task registers the sub the moment the reply lands, then hands the finished [`Response`] to the
/// pump through the returned slot — replies still leave strictly in request order. Each in-flight
/// subscribe holds one blocking-pool thread of its own while it waits, bounded by [`PIPELINE_DEPTH`].
fn spawn_resolution(
    entry: PendingReply,
    in_flight: SubscribeInFlight,
    shared: &ConnShared,
    shutdown: &watch::Receiver<bool>,
    budget: Duration,
) -> Pending {
    let (tx, rx) = oneshot::channel();
    let id = entry.id;
    let (shared, mut shutdown) = (shared.clone(), shutdown.clone());
    tokio::spawn(async move {
        let Awaited { response, late } = await_reply(
            entry.reply_rx,
            entry.id,
            entry.method,
            &mut shutdown,
            entry.enqueued_at,
            budget,
        )
        .await;
        open_subscription(&shared, in_flight, &entry.observed, &response);
        if let Some(late) = late {
            let (intake, shutdown) = (shared.intake.clone(), shutdown.clone());
            tokio::task::spawn_blocking(move || reap_late_subscribe(&late, &intake, &shutdown));
        }
        let _ = tx.send(response);
    });
    Pending::Resolving { id, rx }
}

/// Waits for the reply of a subscribe already answered `L3023` and unsubscribes the sub it mints.
///
/// The sim thread still runs the subscribe when it gets there, so a sub appears that the client was
/// told never opened and will never close; left alone it would render for nobody for the engine's
/// lifetime. Ends on any other reply, a gone sim thread, or teardown.
fn reap_late_subscribe(
    late: &ReplyReceiver,
    intake: &Sender<StampedCommand>,
    shutdown: &watch::Receiver<bool>,
) {
    loop {
        match late.recv_timeout(SIM_REPLY_TIMEOUT) {
            Ok(Ok(Dispatched {
                body: ResponseBody::Subscribed { sub },
                ..
            })) => return queue_unsubscribe(intake, sub),
            Ok(_) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) if *shutdown.borrow() => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

/// Registers the sub a successful subscribe reply minted with the connection's outbound sink, and
/// records it as owned. Any other reply leaves the registry untouched (`in_flight` drops). A subscribe
/// that resolves after the connection's subs were released is cancelled instead: nothing would ever
/// unsubscribe it, so a `stream.unsubscribe` is queued at once.
fn open_subscription(
    shared: &ConnShared,
    in_flight: SubscribeInFlight,
    observed: &Observed,
    response: &Response,
) {
    let (
        Observed::Subscribe {
            channel,
            mode,
            format,
        },
        WireResult::Ok {
            ok: ResponseBody::Subscribed { sub },
            ..
        },
    ) = (observed, &response.result)
    else {
        return;
    };
    let mut conn = shared.subs.lock().expect("connection subs mutex poisoned");
    if conn.closed {
        drop(conn);
        queue_unsubscribe(&shared.intake, *sub);
        return;
    }
    delivery::register(
        &shared.registry,
        &shared.hub,
        *sub,
        *channel,
        *mode,
        *format,
        &shared.outbound,
        in_flight,
    );
    conn.owned.push(OwnedSub {
        sub: *sub,
        arm: matches!(channel, Channel::Profile).then(|| shared.hub.arm()),
    });
}

/// Applies a successful `stream.unsubscribe` reply as it passes the pump: the sub leaves the registry
/// and the connection's owned set. A failed one (e.g. `L3007`) changes nothing.
fn apply_unsubscribe(
    registry: &Registry,
    subs: &Mutex<ConnSubs>,
    observed: &Observed,
    response: &Response,
) {
    if let Observed::Unsubscribe { sub } = observed
        && matches!(response.result, WireResult::Ok { .. })
    {
        delivery::unregister(registry, *sub);
        subs.lock()
            .expect("connection subs mutex poisoned")
            .owned
            .retain(|owned| owned.sub != *sub);
    }
}

/// Queues a `stream.unsubscribe` through the one command queue — no server-private path into the
/// engine. Fire-and-forget: the reply receiver drops at once and the sim thread's reply send ignores it.
fn queue_unsubscribe(intake: &Sender<StampedCommand>, sub: SubId) {
    let (stamped, _reply) =
        StampedCommand::new(Command::StreamUnsubscribe(StreamUnsubscribeParams { sub }));
    let _ = intake.send(stamped);
}

/// Releases every sub the connection owns: unregister it and queue a `stream.unsubscribe`, so the
/// engine stops rendering for a viewer that is gone. Marks the connection closed so a subscribe still
/// resolving cancels itself ([`open_subscription`]).
fn release_subs(shared: &ConnShared) {
    let owned = {
        let mut conn = shared.subs.lock().expect("connection subs mutex poisoned");
        conn.closed = true;
        std::mem::take(&mut conn.owned)
    };
    for owned in owned {
        delivery::unregister(&shared.registry, owned.sub);
        queue_unsubscribe(&shared.intake, owned.sub);
        // A `profile` subscription's arming guard goes with it: the last one out disarms the hub.
        drop(owned.arm);
    }
}

/// Closes the outbound lanes and joins the writer.
async fn finish(writer: tokio::task::JoinHandle<()>, outbound: OutboundSink) {
    // Drop the sink — both lanes — so the writer sees them close once all clones are gone.
    drop(outbound);
    let _ = writer.await;
}

/// How long a request waits for the sim thread's reply before it is answered `L3023` instead.
///
/// The bound caps a wedged sim thread; it is not there to police slow work, so it sits far above any
/// command this engine performs in one go — the heaviest district renders a frame in under 2 ms, so
/// even a several-thousand-tick `sim.step` on a debug build and a loaded machine finishes in a small
/// fraction of it. A `sim.step` whose `ticks` is large enough to exceed two minutes is, from outside
/// the sim thread, indistinguishable from a wedge, and is reported as one.
const SIM_REPLY_TIMEOUT: Duration = Duration::from_secs(120);

/// Once teardown has been signalled, how much longer an in-flight reply may take before the request
/// is answered `L3009` instead.
///
/// The reply that matters here is `engine.shutdown`'s own `Ack`: the dispatch that trips the shutdown
/// is the dispatch that produces it, so it is already on its way when the watch flips and this only
/// has to outlast the scheduling gap. It stays well under the teardown grace in `lib.rs`, so the
/// connection has answered and drained before teardown stops waiting for it.
const SHUTDOWN_REPLY_GRACE: Duration = Duration::from_secs(2);

/// Turns one request text into the [`Pending`] entry that holds its place in the reply order,
/// routing it through the single command queue on the way.
///
/// A parse failure becomes an error response immediately (never reaching the queue), as does a
/// request the queue refuses because the sim thread has stopped (`L3009`) — both ride the queue as
/// [`Pending::Ready`] so they are answered in their slot, not ahead of it. Everything else becomes a
/// `StampedCommand` whose one-shot reply the pump awaits.
///
/// A `stream.unsubscribe` is answered only for a sub this connection owns; any other is refused
/// `L3007` here, so one client can never close another's subscription.
///
/// A `profile.capture` is armed here, *before* the command reaches the sim thread: the frames it
/// measures start running the moment it does. The guard travels with the entry, so it covers the
/// answered capture, a refused one, a stalled sim thread, and a connection dropped mid-flight alike.
///
/// A `stream.subscribe` is counted in flight in the registry for the same reason, and the count rides
/// the entry to the task that resolves it.
fn accept_request(text: &str, shared: &ConnShared) -> Pending {
    let request = match parse_request(text) {
        Ok(request) => request,
        Err((fallback_id, diag)) => return Pending::Ready(Response::error(fallback_id, diag)),
    };
    let id = request.id.as_u64();
    // Read what the command is for the registry *before* moving it into the stamped command.
    let observed = observe(&request.command);
    if let Observed::Unsubscribe { sub } = observed {
        let owns = {
            let conn = shared.subs.lock().expect("connection subs mutex poisoned");
            conn.owned.iter().any(|owned| owned.sub == sub)
        };
        if !owns {
            return Pending::Ready(Response::error(id, not_owned(sub)));
        }
    }
    let capture = matches!(observed, Observed::Capture).then(|| shared.hub.arm());
    let subscribe = matches!(observed, Observed::Subscribe { .. })
        .then(|| delivery::subscribe_in_flight(&shared.registry));
    let method = request.command.method();

    let (stamped, reply_rx) = StampedCommand::new(request.command);
    if shared.intake.send(stamped).is_err() {
        return Pending::Ready(Response::error(id, engine_unavailable()));
    }
    Pending::Reply(PendingReply {
        id,
        method,
        reply_rx,
        observed,
        capture,
        subscribe,
        enqueued_at: Instant::now(),
    })
}

/// Waits for the sim thread's one-shot reply and turns it into a [`Response`].
///
/// The wait runs on the blocking pool so a runtime worker is never parked on the sim thread, and it is
/// bounded there — an unbounded `recv` would park that pool thread for the process's whole life if the
/// sim thread never came back. `budget` is the whole bound, measured from `enqueued_at` (when the
/// reader accepted the request) rather than from now: a request the pump reaches late, behind a
/// marathon `sim.step`, must not be handed a fresh full budget of its own, and `L3023` then means what
/// it says — no reply within `budget` of the request being taken.
///
/// Teardown is watched alongside the wait so a shutdown is not held hostage by a request in flight,
/// but an already-produced reply still wins: the wait is shortened to [`SHUTDOWN_REPLY_GRACE`], not
/// cancelled. A request the pump only reaches *after* teardown began takes the same grace up front —
/// with no reply coming, that is `L3009` (the engine is going away), never a stall report.
async fn await_reply(
    reply_rx: ReplyReceiver,
    id: u64,
    method: &str,
    shutdown: &mut watch::Receiver<bool>,
    enqueued_at: Instant,
    budget: Duration,
) -> Awaited {
    let wait = budget.saturating_sub(enqueued_at.elapsed());
    if *shutdown.borrow() {
        let handle = tokio::task::spawn_blocking(move || {
            reply_rx.recv_timeout(wait.min(SHUTDOWN_REPLY_GRACE))
        });
        let response = match handle.await {
            Ok(Ok(Ok(dispatched))) => {
                Response::ok_at(id, dispatched.body, dispatched.frame, dispatched.game_frame)
            }
            Ok(Ok(Err(diag))) => Response::error(id, diag),
            // No reply within the grace, a gone sim thread, or a cancelled wait: all the same answer.
            _ => Response::error(id, engine_unavailable()),
        };
        return Awaited {
            response,
            late: None,
        };
    }
    let mut handle = tokio::task::spawn_blocking(move || {
        let got = reply_rx.recv_timeout(wait);
        let late = matches!(got, Err(RecvTimeoutError::Timeout)).then_some(reply_rx);
        (got, late)
    });
    loop {
        tokio::select! {
            joined = &mut handle => return awaited(joined, id, method, budget),
            changed = shutdown.changed() => {
                if changed.is_ok() && !*shutdown.borrow() {
                    continue;
                }
                return match tokio::time::timeout(SHUTDOWN_REPLY_GRACE, handle).await {
                    Ok(joined) => awaited(joined, id, method, budget),
                    Err(_) => Awaited {
                        response: Response::error(id, engine_unavailable()),
                        late: None,
                    },
                };
            }
        }
    }
}

/// What [`await_reply`] resolved to.
struct Awaited {
    response: Response,
    /// The receiver a stalled reply will still arrive on — `Some` only when `response` is `L3023`.
    late: Option<ReplyReceiver>,
}

/// The blocking wait's outcome and the receiver it hands back on a timeout.
type Waited = (
    Result<CommandResult, RecvTimeoutError>,
    Option<ReplyReceiver>,
);

/// Splits the blocking wait's outcome into the response and the receiver a stalled reply may still use.
fn awaited(
    joined: Result<Waited, tokio::task::JoinError>,
    id: u64,
    method: &str,
    budget: Duration,
) -> Awaited {
    match joined {
        Ok((got, late)) => Awaited {
            response: reply_response(Ok(got), id, method, budget),
            late,
        },
        Err(e) => Awaited {
            response: reply_response(Err(e), id, method, budget),
            late: None,
        },
    }
}

/// Maps the awaited reply — or the reason there was none — onto the response envelope. `budget` is
/// only what the stall diagnostic names: the whole bound the request was given, not the remainder it
/// actually waited out.
fn reply_response(
    joined: Result<Result<CommandResult, RecvTimeoutError>, tokio::task::JoinError>,
    id: u64,
    method: &str,
    budget: Duration,
) -> Response {
    match joined {
        Ok(Ok(Ok(dispatched))) => {
            Response::ok_at(id, dispatched.body, dispatched.frame, dispatched.game_frame)
        }
        Ok(Ok(Err(diag))) => Response::error(id, diag),
        Ok(Err(RecvTimeoutError::Timeout)) => Response::error(id, sim_stalled(method, budget)),
        // The reply channel disconnected (sim thread gone), or the blocking task was cancelled.
        Ok(Err(RecvTimeoutError::Disconnected)) | Err(_) => {
            Response::error(id, engine_unavailable())
        }
    }
}

/// Classifies a command for the delivery registry (see [`Observed`]). The subscribe/unsubscribe params
/// are all `Copy`, so this peeks without consuming the command.
fn observe(command: &Command) -> Observed {
    match command {
        Command::StreamSubscribe(params) => Observed::Subscribe {
            channel: params.channel,
            mode: params.mode,
            format: params.format,
        },
        Command::StreamUnsubscribe(params) => Observed::Unsubscribe { sub: params.sub },
        Command::ProfileCapture(_) => Observed::Capture,
        _ => Observed::Other,
    }
}

/// Parses a request, recovering its `id` for the error path when possible.
///
/// A failure carries the id to stamp on the *error* response. When the text is valid JSON with a
/// numeric `id`, that id is recovered even if the rest of the envelope is invalid (so an unknown
/// method or bad version still echoes the client's id). When the text is not JSON, or has no numeric
/// `id`, the fallback is `0` — the null-id convention for an unattributable request.
fn parse_request(text: &str) -> Result<Request, (u64, Diagnostic)> {
    Request::from_json(text).map_err(|diag| {
        let fallback_id = serde_json::from_str::<Value>(text)
            .ok()
            .as_ref()
            .and_then(|value| value.get("id"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        (fallback_id, diag)
    })
}

/// Parses the opening [`Hello`], validating the protocol version.
///
/// # Errors
/// - `L3001` if the message is not a well-formed hello (a wrong first message).
/// - `L3003` if the hello declares a protocol version this engine does not speak.
fn parse_hello(text: &str) -> Result<Hello, Diagnostic> {
    let hello: Hello = serde_json::from_str(text).map_err(|e| {
        Diagnostic::new(
            code::PROTOCOL_MALFORMED_ENVELOPE,
            format!(
                "expected a hello handshake as the first message, but could not parse one: {e}"
            ),
        )
        .with_help(
            "the first message must be a hello object \
             {\"protocol_version\": 1, \"client\": \"<name>\", \"features\": []}",
        )
    })?;
    if hello.protocol_version != PROTOCOL_VERSION {
        return Err(Diagnostic::new(
            code::PROTOCOL_UNSUPPORTED_VERSION,
            format!(
                "this engine speaks protocol version {PROTOCOL_VERSION}, but the client's hello \
                 declared version {}",
                hello.protocol_version
            ),
        )
        .with_path("/protocol_version")
        .with_help(
            "regenerate the client from this engine's schema; protocol versions are breaking by \
             design",
        ));
    }
    Ok(hello)
}

/// Reads the next text message, or `None` on close / error / shutdown. Non-text frames (ping/pong,
/// binary) are skipped; a close frame ends the read.
async fn read_text(
    stream: &mut SplitStream<WebSocketStream<TcpStream>>,
    shutdown: &mut watch::Receiver<bool>,
) -> Option<String> {
    loop {
        let message = tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return None;
                }
                continue;
            }
            message = stream.next() => message,
        };
        match message {
            Some(Ok(Message::Text(text))) => return Some(text.as_str().to_string()),
            Some(Ok(Message::Close(_))) | None => return None,
            Some(Ok(_)) => continue, // ping / pong / binary: not a request
            Some(Err(WsError::Capacity(CapacityError::MessageTooLong { size, max_size }))) => {
                let diagnostic = message_too_large(format!(
                    "a client sent a message of {size} bytes, over the {max_size}-byte wire limit"
                ));
                eprintln!("{diagnostic}");
                return None;
            }
            Some(Err(_)) => return None,
        }
    }
}

/// How long a new socket has to finish the WebSocket upgrade and send its `Hello`.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Awaits `future` until `deadline`, giving up early on shutdown; `None` on either.
async fn until_shutdown_or<T>(
    future: impl Future<Output = T>,
    deadline: tokio::time::Instant,
    shutdown: &mut watch::Receiver<bool>,
) -> Option<T> {
    tokio::pin!(future);
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return None;
                }
            }
            result = tokio::time::timeout_at(deadline, &mut future) => return result.ok(),
        }
    }
}

/// The one WebSocket config every connection is accepted with: the wire's message ceiling in place
/// of tungstenite's 16 MiB default, which a 4K raw frame already exceeds.
fn websocket_config() -> WebSocketConfig {
    WebSocketConfig {
        max_message_size: Some(MAX_MESSAGE_BYTES),
        max_frame_size: Some(MAX_MESSAGE_BYTES),
        ..WebSocketConfig::default()
    }
}

/// The `L3045` diagnostic for a message over the wire ceiling; `what` names the size and the limit.
pub(crate) fn message_too_large(what: String) -> Diagnostic {
    Diagnostic::new(code::PROTOCOL_MESSAGE_TOO_LARGE, what)
        .with_help("narrow the query, or snapshot a smaller world; `ludion explain L3045` has more")
}

/// Serializes `response`, or — when the text is over `limit` bytes — an `L3045` error in the same
/// slot (same id), which keeps the connection and the reply order.
fn reply_text(response: &Response, limit: usize) -> String {
    let text = serde_json::to_string(response).unwrap_or_else(|_| "{}".to_string());
    if text.len() <= limit {
        return text;
    }
    let diagnostic = message_too_large(format!(
        "this reply is {} bytes, over the {limit}-byte wire limit; it was not sent",
        text.len()
    ));
    serde_json::to_string(&Response::error(response.id.as_u64(), diagnostic))
        .unwrap_or_else(|_| "{}".to_string())
}

/// The `L3009` diagnostic for a request the sim thread could not answer (it has stopped).
fn engine_unavailable() -> Diagnostic {
    Diagnostic::new(
        code::PROTOCOL_ENGINE_UNAVAILABLE,
        "the engine is not answering; its sim thread has stopped",
    )
    .with_help("the server is shutting down or has stopped — reconnect once it is running again")
}

/// The `L3007` diagnostic for an unsubscribe of a sub this connection did not open.
fn not_owned(sub: SubId) -> Diagnostic {
    Diagnostic::new(
        code::PROTOCOL_UNKNOWN_SUBSCRIPTION,
        format!(
            "subscription {} is not open on this connection",
            sub.as_u64()
        ),
    )
    .with_path("/params/sub")
    .with_help(
        "unsubscribe only a `sub` this connection's stream.subscribe returned; another client's \
         subscription is not yours to close",
    )
}

/// The `L3023` diagnostic for a request the sim thread took but never answered.
fn sim_stalled(method: &str, timeout: Duration) -> Diagnostic {
    Diagnostic::new(
        code::PROTOCOL_SIM_STALLED,
        format!(
            "`{method}` reached the sim thread but produced no reply within {} s; the sim thread is \
             alive and is not returning to its command loop",
            timeout.as_secs_f32()
        ),
    )
    .with_help(
        "a game system is looping without terminating, or a `sim.step` was given more ticks than can \
         run in that window — attach a debugger to the `ludion-sim` thread, or split the step; run \
         `ludion explain L3023` for the full walkthrough",
    )
}

/// The `L3022` diagnostic for one accept the listening socket refused, which the host retries.
fn accept_failed(error: &io::Error) -> Diagnostic {
    Diagnostic::new(
        code::SERVER_ACCEPT_FAILED,
        format!("could not accept an incoming connection: {error}"),
    )
    .with_help(
        "the host keeps listening and retries; if this repeats, check the process's open-file limit \
         and whether the listening socket is still valid",
    )
}

/// The handshake callback: a browser page from another site is answered HTTP 403 and never upgraded.
// The signature is the one tungstenite's `Callback` impl for closures fixes.
#[allow(clippy::result_large_err)]
fn check_origin(
    request: &HandshakeRequest,
    response: HandshakeResponse,
) -> Result<HandshakeResponse, ErrorResponse> {
    let mut origins = request.headers().get_all("origin").iter();
    let origin = origins.next().map(|value| value.to_str().ok());
    // A second header, or one that is not text, is refused like any foreign origin.
    let allowed = match (origin, origins.next()) {
        (None, _) => true,
        (Some(Some(text)), None) => origin_allowed(Some(text)),
        _ => false,
    };
    if allowed {
        return Ok(response);
    }
    let diagnostic = origin_refused(origin.flatten());
    eprintln!("{diagnostic}");
    let mut refusal = ErrorResponse::new(Some(diagnostic.to_string()));
    *refusal.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN;
    Err(refusal)
}

/// Whether a handshake's `Origin` header may connect. A native client sends none; a browser always
/// does, so only the loopback hosts and the editor's own origins pass. The host must match exactly,
/// so `http://localhost.example.com` does not.
fn origin_allowed(origin: Option<&str>) -> bool {
    let Some(origin) = origin else { return true };
    let origin = origin.to_ascii_lowercase();
    let Some((scheme, authority)) = origin.split_once("://") else {
        return false;
    };
    let (host, port) = match authority.strip_prefix('[') {
        Some(rest) => match rest.split_once(']') {
            Some((inner, "")) => (format!("[{inner}]"), None),
            Some((inner, tail)) => match tail.strip_prefix(':') {
                Some(port) => (format!("[{inner}]"), Some(port)),
                None => return false,
            },
            None => return false,
        },
        None => match authority.split_once(':') {
            Some((host, port)) => (host.to_string(), Some(port)),
            None => (authority.to_string(), None),
        },
    };
    if port.is_some_and(|port| port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit())) {
        return false;
    }
    match scheme {
        "http" | "https" => matches!(
            host.as_str(),
            "localhost" | "127.0.0.1" | "[::1]" | "tauri.localhost"
        ),
        "tauri" => host == "localhost",
        _ => false,
    }
}

/// The `L3039` diagnostic for a handshake whose `Origin` is not loopback or the editor's own.
fn origin_refused(origin: Option<&str>) -> Diagnostic {
    let shown = origin.unwrap_or("<multiple or non-text Origin headers>");
    Diagnostic::new(
        code::WIRE_ORIGIN_REFUSED,
        format!("refused a WebSocket handshake from origin {shown:?}: it is not a loopback origin"),
    )
    .with_help(
        "connect from a native client or a page served from localhost; run `ludion explain L3039` \
         for the full walkthrough",
    )
}

/// The `L3022` diagnostic for a listening socket that never recovered, so the host stopped accepting.
fn accept_abandoned(error: &io::Error, failures: u32) -> Diagnostic {
    Diagnostic::new(
        code::SERVER_ACCEPT_FAILED,
        format!(
            "the listening socket failed {failures} accepts in a row (last: {error}); the host has \
             stopped accepting connections and is shutting down"
        ),
    )
    .with_help(
        "raise the process's open-file limit, or find what is holding descriptors open, then restart \
         the host; run `ludion explain L3022` for the full walkthrough",
    )
}

#[cfg(test)]
mod tests {
    use ludion_core::time::FrameIndex;
    use ludion_protocol::schema;

    use super::*;

    /// A listener that only ever fails, counting how many times it was asked — the seam that lets the
    /// accept loop's failure handling be observed without an operating system in fd exhaustion.
    struct FailingListener(Arc<AtomicUsize>);

    impl Accept for FailingListener {
        fn accept(&self) -> impl Future<Output = io::Result<TcpStream>> + Send {
            self.0.fetch_add(1, Ordering::AcqRel);
            std::future::ready(Err(io::Error::other("too many open files")))
        }
    }

    /// A capture the engine accepts must answer well inside the reply budget, or the transport
    /// would call the sim stalled first.
    #[test]
    fn the_reply_budget_is_twice_the_longest_capture_window() {
        assert!(
            SIM_REPLY_TIMEOUT.as_secs_f64()
                >= 2.0 * ludion_protocol::perf::MAX_CAPTURE_WALL_SECONDS
        );
    }

    #[tokio::test]
    async fn until_shutdown_or_gives_up_on_the_deadline() {
        let (_tx, mut shutdown) = watch::channel(false);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(50);
        let outcome =
            until_shutdown_or(std::future::pending::<()>(), deadline, &mut shutdown).await;
        assert_eq!(outcome, None);
        let ready = until_shutdown_or(std::future::ready(7), deadline, &mut shutdown).await;
        assert_eq!(ready, Some(7));
    }

    #[tokio::test]
    async fn until_shutdown_or_gives_up_on_shutdown() {
        let (tx, mut shutdown) = watch::channel(false);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        tx.send(true).unwrap();
        let started = Instant::now();
        let outcome =
            until_shutdown_or(std::future::pending::<()>(), deadline, &mut shutdown).await;
        assert_eq!(outcome, None);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    fn hello() -> ServerHello {
        ServerHello {
            engine_version: "test".to_string(),
            protocol_version: PROTOCOL_VERSION,
            schema_hash: schema::schema_hash(),
        }
    }

    fn error_code(response: &Response) -> String {
        match &response.result {
            WireResult::Error { error } => error.code.to_string(),
            WireResult::Ok { .. } => panic!("expected an error response, got a success"),
        }
    }

    #[test]
    fn an_over_limit_reply_becomes_an_l3045_in_its_slot() {
        let big = Response::ok(7, ResponseBody::Ack);
        let text = reply_text(&big, 16);
        let back: Response = serde_json::from_str(&text).unwrap();
        assert_eq!(back.id.as_u64(), 7);
        assert_eq!(error_code(&back), "L3045");
        let small = reply_text(&big, usize::MAX);
        assert_eq!(small, serde_json::to_string(&big).unwrap());
    }

    /// The ceiling holds an 8K frame in its worst encoding and an 8K stored-PNG screenshot as base64,
    /// both computed from the engine's own limits.
    #[test]
    fn the_message_ceiling_covers_the_8k_frame_and_screenshot() {
        let (w, h) = (
            ludion_engine::MAX_RENDER_WIDTH as usize,
            ludion_engine::MAX_RENDER_HEIGHT as usize,
        );
        let qoi_worst = w * h * 5 + ludion_protocol::frame::FRAME_HEADER_LEN;
        let png_stored = w * h * 4 + h + (w * h * 4 + h).div_ceil(65_535) * 5 + 1024;
        let base64 = png_stored.div_ceil(3) * 4;
        assert!(MAX_MESSAGE_BYTES >= qoi_worst, "{qoi_worst}");
        assert!(MAX_MESSAGE_BYTES >= base64 + 4096, "{base64}");
    }

    #[test]
    fn origin_policy_admits_native_clients_loopback_and_the_editor() {
        for origin in [
            None,
            Some("http://localhost"),
            Some("http://localhost:5173"),
            Some("https://localhost:8443"),
            Some("http://127.0.0.1:3000"),
            Some("http://[::1]:3000"),
            Some("http://[::1]"),
            Some("http://tauri.localhost"),
            Some("https://tauri.localhost"),
            Some("tauri://localhost"),
            Some("HTTP://LocalHost:5173"),
        ] {
            assert!(origin_allowed(origin), "{origin:?} should connect");
        }
    }

    #[test]
    fn origin_policy_refuses_every_other_origin() {
        for origin in [
            "null",
            "",
            "https://example.com",
            "http://localhost.evil.com",
            "http://127.0.0.1.evil.com",
            "http://evil.com:80",
            "http://localhost:",
            "http://localhost:80x",
            "http://localhost/path",
            "http://user@localhost",
            "http://[::1",
            "http://[::1]evil",
            "http://[::1]evil.com",
            "http://[::1]:",
            "tauri://tauri.localhost",
            "tauri://evil.com",
            "file://localhost",
            "ftp://localhost",
            "localhost",
        ] {
            assert!(
                !origin_allowed(Some(origin)),
                "{origin:?} should be refused"
            );
        }
    }

    /// A listener that fails forever must not be retried forever: the loop backs off, gives up after
    /// its limit, and stops the host through the ordinary `engine.shutdown` queue rather than spinning
    /// a core in silence.
    #[tokio::test]
    async fn a_listener_that_always_fails_is_bounded_and_stops_the_host() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (intake, intake_rx) = crossbeam_channel::unbounded();
        // Held (not `_`) so `shutdown.changed()` never resolves: only the failure limit can end this.
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let policy = AcceptPolicy {
            backoff: Duration::from_millis(1),
            failure_limit: 5,
        };

        accept_loop_with(
            FailingListener(calls.clone()),
            intake,
            hello(),
            delivery::new_registry(),
            crate::perf::ProfileHub::new(),
            Arc::new(AtomicUsize::new(0)),
            shutdown_rx,
            policy,
        )
        .await;

        assert_eq!(calls.load(Ordering::Acquire), 5, "retries must be bounded");
        let stamped = intake_rx
            .try_recv()
            .expect("giving up must queue a shutdown");
        assert!(matches!(stamped.command, Command::EngineShutdown));
    }

    /// The backoff must not hold teardown: a shutdown signalled while the loop is sleeping between
    /// retries ends it, rather than costing a whole backoff interval.
    #[tokio::test]
    async fn shutdown_during_the_accept_backoff_ends_the_loop() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (intake, intake_rx) = crossbeam_channel::unbounded();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        // A backoff far longer than the test may take, so passing proves the sleep was interrupted.
        let policy = AcceptPolicy {
            backoff: Duration::from_secs(600),
            failure_limit: u32::MAX,
        };

        let loop_task = tokio::spawn(accept_loop_with(
            FailingListener(calls.clone()),
            intake,
            hello(),
            delivery::new_registry(),
            crate::perf::ProfileHub::new(),
            Arc::new(AtomicUsize::new(0)),
            shutdown_rx,
            policy,
        ));
        while calls.load(Ordering::Acquire) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        shutdown_tx
            .send(true)
            .expect("the loop still holds a receiver");

        tokio::time::timeout(Duration::from_secs(5), loop_task)
            .await
            .expect("the loop must observe shutdown during its backoff")
            .expect("the loop must not panic");
        assert!(
            intake_rx.try_recv().is_err(),
            "an ordinary shutdown must not queue an `engine.shutdown` of its own"
        );
    }

    /// The A2 regression: a sim thread that takes a command and never answers must produce the stall
    /// diagnostic instead of parking the blocking-pool thread and the request loop forever.
    #[tokio::test]
    async fn a_reply_that_never_arrives_becomes_the_stall_diagnostic() {
        // The stamped command is held, so the reply channel is connected — the sim thread is "alive".
        let (stamped, reply_rx) = StampedCommand::new(Command::SimHash);
        let (_shutdown_tx, mut shutdown_rx) = watch::channel(false);

        let response = await_reply(
            reply_rx,
            7,
            "sim.hash",
            &mut shutdown_rx,
            Instant::now(),
            Duration::from_millis(50),
        )
        .await
        .response;

        drop(stamped);
        assert_eq!(response.id.as_u64(), 7);
        assert_eq!(error_code(&response), "L3023");
    }

    /// A stall is not a disconnect: a sim thread that is *gone* still answers `L3009`, so the two
    /// conditions stay distinguishable from the client side.
    #[tokio::test]
    async fn a_dropped_reply_channel_is_still_engine_unavailable() {
        let (stamped, reply_rx) = StampedCommand::new(Command::SimHash);
        drop(stamped);
        let (_shutdown_tx, mut shutdown_rx) = watch::channel(false);

        let response = await_reply(
            reply_rx,
            8,
            "sim.hash",
            &mut shutdown_rx,
            Instant::now(),
            Duration::from_secs(60),
        )
        .await
        .response;

        assert_eq!(error_code(&response), "L3009");
    }

    /// Watching shutdown must not cost a reply that has already been produced — `engine.shutdown`'s own
    /// `Ack` is handed back by the very dispatch that trips the shutdown, and must still reach the
    /// client.
    #[tokio::test]
    async fn a_reply_produced_during_teardown_still_reaches_the_client() {
        let (stamped, reply_rx) = StampedCommand::new(Command::EngineShutdown);
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        shutdown_tx.send(true).expect("receiver is alive");
        stamped
            .reply
            .send(Ok(Dispatched {
                body: ResponseBody::Ack,
                frame: FrameIndex::new(4),
                game_frame: FrameIndex::new(4),
            }))
            .expect("the one-shot reply always fits");

        let response = await_reply(
            reply_rx,
            9,
            "engine.shutdown",
            &mut shutdown_rx,
            Instant::now(),
            Duration::from_secs(60),
        )
        .await
        .response;

        assert!(matches!(response.result, WireResult::Ok { .. }));
    }

    /// The other half of teardown: with no reply coming, the wait ends on its shutdown grace rather
    /// than on the full sim bound, so a request in flight cannot hold the host open.
    #[tokio::test]
    async fn teardown_stops_waiting_for_a_reply_that_never_comes() {
        let (stamped, reply_rx) = StampedCommand::new(Command::SimHash);
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        shutdown_tx.send(true).expect("receiver is alive");

        let started = std::time::Instant::now();
        let response = await_reply(
            reply_rx,
            10,
            "sim.hash",
            &mut shutdown_rx,
            Instant::now(),
            Duration::from_secs(600),
        )
        .await
        .response;

        drop(stamped);
        assert_eq!(error_code(&response), "L3009");
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "teardown must not wait out the sim bound"
        );
    }

    /// The reply-inspection seam is also the arming switch for transport timing: a `profile`
    /// subscription arms the hub, another channel does not, and the unsubscribe reply disarms it.
    #[tokio::test]
    async fn a_profile_subscription_is_the_arming_switch() {
        let (shared, _text_rx, _frames_rx, _intake_rx) = conn_shared();

        let subscribe = |channel| Observed::Subscribe {
            channel,
            mode: SubscribeMode::default(),
            format: FrameFormat::default(),
        };
        let subscribed = |sub| Response::ok(1, ResponseBody::Subscribed { sub });
        let open = |channel, sub| {
            open_subscription(
                &shared,
                delivery::subscribe_in_flight(&shared.registry),
                &subscribe(channel),
                &subscribed(SubId::new(sub)),
            );
        };

        open(Channel::Log, 1);
        assert!(
            !shared.hub.armed(),
            "another channel does not arm transport timing"
        );

        open(Channel::Profile, 2);
        assert!(shared.hub.armed());

        apply_unsubscribe(
            &shared.registry,
            &shared.subs,
            &Observed::Unsubscribe { sub: SubId::new(2) },
            &Response::ok(2, ResponseBody::Ack),
        );
        assert!(
            !shared.hub.armed(),
            "unsubscribing releases the last interest"
        );
    }

    /// Disconnect cleanup disarms for free: the guard lives in the connection's owned-sub state, and
    /// release consumes it — no separate disarm path to forget.
    #[tokio::test]
    async fn a_disconnect_disarms_the_hub() {
        let (shared, _text_rx, _frames_rx, _intake_rx) = conn_shared();
        open_subscription(
            &shared,
            delivery::subscribe_in_flight(&shared.registry),
            &Observed::Subscribe {
                channel: Channel::Profile,
                mode: SubscribeMode::default(),
                format: FrameFormat::default(),
            },
            &Response::ok(1, ResponseBody::Subscribed { sub: SubId::new(9) }),
        );
        assert!(shared.hub.armed());

        release_subs(&shared);
        assert!(!shared.hub.armed(), "a gone viewer stops costing timing");
    }

    /// A subscribe whose reply lands after the connection was released is cancelled, not registered:
    /// nothing would ever unsubscribe it.
    #[tokio::test]
    async fn a_subscribe_resolving_after_disconnect_is_cancelled() {
        let (shared, _text_rx, _frames_rx, intake_rx) = conn_shared();
        release_subs(&shared);
        open_subscription(
            &shared,
            delivery::subscribe_in_flight(&shared.registry),
            &Observed::Subscribe {
                channel: Channel::Log,
                mode: SubscribeMode::default(),
                format: FrameFormat::default(),
            },
            &Response::ok(1, ResponseBody::Subscribed { sub: SubId::new(4) }),
        );

        assert!(!registered(&shared.registry, 4));
        let queued = intake_rx.try_recv().expect("an unsubscribe is queued");
        assert!(matches!(
            queued.command,
            Command::StreamUnsubscribe(ref params) if params.sub == SubId::new(4)
        ));
    }

    /// A connection may close only what it opened: an unsubscribe naming another connection's sub is
    /// answered `L3007` in its slot and never reaches the engine.
    #[tokio::test]
    async fn an_unsubscribe_of_a_sub_this_connection_does_not_own_is_refused() {
        let (shared, _text_rx, _frames_rx, intake_rx) = conn_shared();
        let text = r#"{"v":1,"id":3,"method":"stream.unsubscribe","params":{"sub":5}}"#;

        let Pending::Ready(response) = accept_request(text, &shared) else {
            panic!("an unowned unsubscribe is refused before dispatch");
        };
        assert_eq!(response.id.as_u64(), 3);
        assert_eq!(error_code(&response), "L3007");
        assert!(intake_rx.try_recv().is_err(), "nothing reaches the engine");

        open_subscription(
            &shared,
            delivery::subscribe_in_flight(&shared.registry),
            &Observed::Subscribe {
                channel: Channel::Log,
                mode: SubscribeMode::default(),
                format: FrameFormat::default(),
            },
            &Response::ok(1, ResponseBody::Subscribed { sub: SubId::new(5) }),
        );
        assert!(matches!(accept_request(text, &shared), Pending::Reply(_)));
        let queued = intake_rx
            .try_recv()
            .expect("an owned unsubscribe is forwarded");
        assert!(matches!(
            queued.command,
            Command::StreamUnsubscribe(ref params) if params.sub == SubId::new(5)
        ));
    }

    /// A subscribe the sim answers after its `L3023` still minted a sub; the client was told it
    /// failed and will never close it, so the server does, and never registers it.
    #[tokio::test]
    async fn a_subscribe_answered_after_its_timeout_is_unsubscribed() {
        let (shared, _text_rx, _frames_rx, intake_rx) = conn_shared();
        let (_shutdown_tx, shutdown) = watch::channel(false);
        let (stamped, reply_rx) = StampedCommand::new(Command::StreamSubscribe(
            ludion_protocol::command::StreamSubscribeParams {
                channel: Channel::Log,
                mode: SubscribeMode::default(),
                format: FrameFormat::default(),
            },
        ));
        let entry = PendingReply {
            id: 11,
            method: "stream.subscribe",
            reply_rx,
            observed: Observed::Subscribe {
                channel: Channel::Log,
                mode: SubscribeMode::default(),
                format: FrameFormat::default(),
            },
            capture: None,
            subscribe: None,
            enqueued_at: Instant::now(),
        };
        let in_flight = delivery::subscribe_in_flight(&shared.registry);
        let Pending::Resolving { rx, .. } = spawn_resolution(
            entry,
            in_flight,
            &shared,
            &shutdown,
            Duration::from_millis(50),
        ) else {
            panic!("a subscribe resolves on its own task");
        };
        let response = rx.await.expect("the resolution answers");
        assert_eq!(error_code(&response), "L3023");

        answer_subscribed(&stamped, 5);
        let deadline = Instant::now() + Duration::from_secs(5);
        let queued = loop {
            if let Ok(queued) = intake_rx.try_recv() {
                break queued;
            }
            assert!(
                Instant::now() < deadline,
                "the late sub was never unsubscribed"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        };
        assert!(matches!(
            queued.command,
            Command::StreamUnsubscribe(ref params) if params.sub == SubId::new(5)
        ));
        assert!(!registered(&shared.registry, 5));
    }

    #[test]
    fn parse_request_recovers_the_id_only_on_failure() {
        let ok = parse_request(r#"{"v":1,"id":7,"method":"sim.hash"}"#);
        assert_eq!(ok.expect("a valid request parses").id.as_u64(), 7);
        let (id, _) = parse_request(r#"{"v":1,"id":42,"method":"nope"}"#)
            .expect_err("an unknown method fails");
        assert_eq!(id, 42);
        let (id, _) = parse_request("not json").expect_err("garbage fails");
        assert_eq!(id, 0);
    }

    /// A connection's shared state over free-standing lanes and a held intake.
    fn conn_shared() -> (
        ConnShared,
        mpsc::Receiver<String>,
        mpsc::Receiver<BinaryFrame>,
        crossbeam_channel::Receiver<StampedCommand>,
    ) {
        let (intake, intake_rx) = crossbeam_channel::unbounded();
        let (outbound, text_rx, frames_rx) = delivery::outbound_channel();
        let shared = ConnShared {
            intake,
            registry: delivery::new_registry(),
            hub: ProfileHub::new(),
            outbound,
            subs: Arc::new(Mutex::new(ConnSubs::default())),
        };
        (shared, text_rx, frames_rx, intake_rx)
    }

    /// A `profile.capture` is the second arming source: it is classified before dispatch, because the
    /// frames it measures start running the moment the sim thread takes the command.
    #[test]
    fn a_capture_is_observed_as_an_arming_source() {
        let capture =
            Command::ProfileCapture(ludion_protocol::perf::ProfileCaptureParams::default());
        assert!(matches!(observe(&capture), Observed::Capture));
        assert!(matches!(observe(&Command::SimHash), Observed::Other));
    }

    // --- The request pipeline over a real socket ------------------------------------------------
    //
    // These drive a live `accept_loop_with` on loopback and *hold* the sim side: the test owns the
    // intake receiver, so a reply exists only when the test produces one. That is the only way to
    // observe the reader running ahead of the replies.
    //
    // Every one of them drops its held `StampedCommand`s before returning: that disconnects the reply
    // channels, so the pump's blocking waits return at once instead of the runtime's drop waiting
    // them out (a blocking task cannot be cancelled).

    /// The client half of a test connection.
    type ClientSocket = WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

    /// A handshaken connection to a live accept loop whose sim side is held by the test.
    struct Wire {
        client: ClientSocket,
        intake: crossbeam_channel::Receiver<StampedCommand>,
        /// The registry the loop delivers through, for tests that play the egress side.
        registry: Registry,
        /// Held so the loop's `shutdown.changed()` never resolves: only the test ends these.
        _shutdown: watch::Sender<bool>,
    }

    /// Binds a loop on a free port, connects a client, and completes the handshake.
    async fn wire() -> Wire {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a free loopback port");
        let addr = listener.local_addr().expect("a bound address");
        let (intake, intake_rx) = crossbeam_channel::unbounded();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let registry = delivery::new_registry();
        tokio::spawn(accept_loop_with(
            listener,
            intake,
            hello(),
            registry.clone(),
            ProfileHub::new(),
            Arc::new(AtomicUsize::new(0)),
            shutdown_rx,
            AcceptPolicy::DEFAULT,
        ));

        let (mut client, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .expect("the WebSocket upgrade succeeds");
        client
            .send(Message::text(
                r#"{"protocol_version":1,"client":"pipeline-test","features":[]}"#.to_string(),
            ))
            .await
            .expect("the hello sends");
        let reply = text_within(&mut client, Duration::from_secs(5))
            .await
            .expect("a ServerHello");
        assert!(
            reply.contains("schema_hash"),
            "expected a ServerHello, got {reply}"
        );
        Wire {
            client,
            intake: intake_rx,
            registry,
            _shutdown: shutdown_tx,
        }
    }

    /// One `sim.hash` request as wire text.
    fn request(id: u64) -> Message {
        Message::text(format!(r#"{{"v":1,"id":{id},"method":"sim.hash"}}"#))
    }

    /// The next text message on the client socket, or `None` if none arrives within `patience`.
    async fn text_within(client: &mut ClientSocket, patience: Duration) -> Option<String> {
        loop {
            return match tokio::time::timeout(patience, client.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => Some(text.as_str().to_string()),
                Ok(Some(Ok(_))) => continue, // ping/pong; keep waiting for a reply
                _ => None,
            };
        }
    }

    /// Answers one held command, as the sim thread would.
    fn answer(stamped: &StampedCommand) {
        stamped
            .reply
            .send(Ok(Dispatched {
                body: ResponseBody::Ack,
                frame: FrameIndex::new(0),
                game_frame: FrameIndex::new(0),
            }))
            .expect("the one-shot reply always fits");
    }

    /// Takes commands off the held intake until there are `want` of them or `patience` runs out.
    ///
    /// Polls rather than blocking on `recv`: these tests share a thread with the connection tasks
    /// they are measuring, so a blocking wait here would stop the very reader under test.
    async fn take_commands(
        intake: &crossbeam_channel::Receiver<StampedCommand>,
        want: usize,
        patience: Duration,
    ) -> Vec<StampedCommand> {
        let deadline = Instant::now() + patience;
        let mut taken = Vec::new();
        while taken.len() < want && Instant::now() < deadline {
            match intake.try_recv() {
                Ok(stamped) => taken.push(stamped),
                Err(_) => tokio::time::sleep(Duration::from_millis(1)).await,
            }
        }
        taken
    }

    /// Everything the held intake receives, up to the point it has been quiet for `quiet`.
    async fn drain_settled(
        intake: &crossbeam_channel::Receiver<StampedCommand>,
        quiet: Duration,
    ) -> Vec<StampedCommand> {
        let mut taken = Vec::new();
        let mut idle = Instant::now();
        while idle.elapsed() < quiet {
            match intake.try_recv() {
                Ok(stamped) => {
                    taken.push(stamped);
                    idle = Instant::now();
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(1)).await,
            }
        }
        taken
    }

    /// The pipelining guarantee: reads are not gated on replies. With nothing answering the sim side,
    /// a whole burst still reaches the one queue — under a read-one-answer-one loop only the first
    /// request would have, and the rest would sit in the receive buffer growing stale.
    #[tokio::test]
    async fn reads_are_not_gated_on_replies() {
        let mut wire = wire().await;
        for id in 1..=8 {
            wire.client
                .send(request(id))
                .await
                .expect("the burst sends");
        }

        let held = take_commands(&wire.intake, 8, Duration::from_secs(5)).await;
        assert_eq!(held.len(), 8, "every pipelined request reached the queue");
        assert!(
            text_within(&mut wire.client, Duration::from_millis(100))
                .await
                .is_none(),
            "no reply can exist yet: nothing has answered the sim side"
        );

        drop(held);
    }

    /// Replies keep per-connection *request* order however the sim answers, and a request refused
    /// before dispatch (an unknown method never reaches the queue) keeps its slot in that order
    /// rather than overtaking the requests around it.
    #[tokio::test]
    async fn replies_keep_request_order_with_an_error_in_its_slot() {
        let mut wire = wire().await;
        wire.client.send(request(1)).await.expect("id 1 sends");
        wire.client
            .send(Message::text(
                r#"{"v":1,"id":2,"method":"nope"}"#.to_string(),
            ))
            .await
            .expect("the malformed request sends");
        wire.client.send(request(3)).await.expect("id 3 sends");

        // Only the two well-formed requests reach the queue — and the later one is answered first.
        let held = take_commands(&wire.intake, 2, Duration::from_secs(5)).await;
        assert_eq!(
            held.len(),
            2,
            "an unknown method never reaches the sim thread"
        );
        answer(&held[1]);
        answer(&held[0]);

        let mut ids = Vec::new();
        for _ in 0..3 {
            let text = text_within(&mut wire.client, Duration::from_secs(5))
                .await
                .expect("every request is answered");
            let value: Value = serde_json::from_str(&text).expect("a JSON response");
            let id = value["id"].as_u64().expect("an echoed id");
            if id == 2 {
                assert_eq!(
                    value["error"]["code"], "L3002",
                    "the parse error is the one in the middle slot"
                );
            }
            ids.push(id);
        }
        assert_eq!(
            ids,
            vec![1, 2, 3],
            "replies follow request order, not completion order"
        );

        drop(held);
    }

    /// The pipeline is bounded: a client bursting past `PIPELINE_DEPTH` stalls the reader at the
    /// bound (TCP holds the rest) instead of the server accepting without end — and the reader
    /// resumes the moment answered replies free slots.
    #[tokio::test]
    async fn the_in_flight_pipeline_is_bounded() {
        let burst = PIPELINE_DEPTH + 16;
        let mut wire = wire().await;
        for id in 1..=burst as u64 {
            wire.client
                .send(request(id))
                .await
                .expect("the burst sends");
        }

        // The bound is the queue plus the one entry the pump is holding, and the reader has already
        // queued the command it is blocked on pushing — a couple over `PIPELINE_DEPTH`, never the
        // whole burst.
        let mut held = drain_settled(&wire.intake, Duration::from_millis(200)).await;
        assert!(
            held.len() >= PIPELINE_DEPTH && held.len() <= PIPELINE_DEPTH + 4,
            "the reader must stall at the bound; it took {} of {burst}",
            held.len()
        );

        // Free ten slots, oldest first (the pump answers FIFO), and the reader reads on.
        for stamped in held.iter().take(10) {
            answer(stamped);
        }
        let resumed = drain_settled(&wire.intake, Duration::from_millis(200)).await;
        assert!(
            resumed.len() >= 10,
            "freeing slots must let the reader resume; it took {} more",
            resumed.len()
        );

        held.extend(resumed);
        drop(held);
    }

    /// The subscribe request for `channel` as wire text.
    fn subscribe_request(id: u64, channel: &str) -> Message {
        Message::text(format!(
            r#"{{"v":1,"id":{id},"method":"stream.subscribe","params":{{"channel":"{channel}"}}}}"#
        ))
    }

    /// A log event for `sub`.
    fn log_event(sub: u64, seq: u64) -> ludion_protocol::EventMessage {
        ludion_protocol::EventMessage {
            event: ludion_protocol::event::Event::Log(ludion_protocol::event::LogEvent {
                level: ludion_protocol::event::LogLevel::Info,
                message: format!("e{seq}"),
                diagnostic: None,
            }),
            sub: SubId::new(sub),
            seq: ludion_protocol::Seq::new(seq),
            frame: None,
        }
    }

    /// Answers a held subscribe with the sub id the engine would mint.
    fn answer_subscribed(stamped: &StampedCommand, sub: u64) {
        stamped
            .reply
            .send(Ok(Dispatched {
                body: ResponseBody::Subscribed {
                    sub: SubId::new(sub),
                },
                frame: FrameIndex::new(0),
                game_frame: FrameIndex::new(0),
            }))
            .expect("the one-shot reply always fits");
    }

    fn registered(registry: &Registry, sub: u64) -> bool {
        registry
            .lock()
            .expect("registry mutex poisoned")
            .subs
            .contains_key(&sub)
    }

    /// The sim answers a subscribe at its drain and emits events right after, but replies leave
    /// in request order — so a subscribe behind a held request must still receive its first events.
    #[tokio::test]
    async fn a_subscribe_behind_a_held_request_gets_its_first_events() {
        let mut wire = wire().await;
        wire.client.send(request(1)).await.expect("id 1 sends");
        wire.client
            .send(subscribe_request(2, "log"))
            .await
            .expect("the subscribe sends");
        let held = take_commands(&wire.intake, 2, Duration::from_secs(5)).await;
        assert_eq!(held.len(), 2, "both requests reach the queue");
        answer_subscribed(&held[1], 7);
        let hub = ProfileHub::new();
        for seq in 0..3 {
            delivery::route_event(&wire.registry, &hub, log_event(7, seq), 0);
        }

        for seq in 0..3 {
            let text = text_within(&mut wire.client, Duration::from_secs(5))
                .await
                .expect("an event is delivered before any reply");
            let value: Value = serde_json::from_str(&text).expect("a JSON message");
            assert_eq!(
                value["event"], "log",
                "the first messages are events: {text}"
            );
            assert_eq!(value["seq"], seq, "events arrive in seq order");
        }
        answer(&held[0]);
        for id in [1, 2] {
            let text = text_within(&mut wire.client, Duration::from_secs(5))
                .await
                .expect("a reply");
            let value: Value = serde_json::from_str(&text).expect("a JSON response");
            assert_eq!(value["id"], id, "replies keep request order");
        }

        drop(held);
    }

    /// W11: a closed socket releases its subs at once; it does not wait for the pump to get through
    /// the replies still held ahead of the subscribe.
    #[tokio::test]
    async fn a_disconnect_unsubscribes_without_waiting_for_held_replies() {
        let mut wire = wire().await;
        wire.client.send(request(1)).await.expect("id 1 sends");
        wire.client
            .send(subscribe_request(2, "log"))
            .await
            .expect("the subscribe sends");
        let held = take_commands(&wire.intake, 2, Duration::from_secs(5)).await;
        assert_eq!(held.len(), 2, "both requests reach the queue");
        answer_subscribed(&held[1], 7);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !registered(&wire.registry, 7) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert!(registered(&wire.registry, 7), "the sub registers at once");

        wire.client.close(None).await.expect("the client closes");

        let queued = take_commands(&wire.intake, 1, Duration::from_secs(2)).await;
        assert!(
            matches!(
                queued.first().map(|stamped| &stamped.command),
                Some(Command::StreamUnsubscribe(params)) if params.sub == SubId::new(7)
            ),
            "the disconnect queues an unsubscribe while id 1 is still unanswered"
        );
        assert!(!registered(&wire.registry, 7), "and unregisters the sub");

        drop(held);
    }
}
