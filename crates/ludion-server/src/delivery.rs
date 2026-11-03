//! Frame and event *delivery*: the egress pump, the frame encoders + routers, and the per-subscriber
//! forwarding that turns [`Egress`] messages into wire messages on each connection.
//!
//! This is the receiving half of the engine's egress seam ([`ludion_engine::egress`]). The sim thread
//! emits [`Egress`] on a [`crossbeam_channel::bounded(1)`] latest-wins-at-source slot; this module
//! carries each message the rest of the way to the WebSocket connections:
//!
//! 1. **The pump** ([`run_egress_pump`]) is a dedicated std thread that owns the two crossbeam
//!    receivers (the frame lane and the event lane). Its only job is to `recv()` from whichever is
//!    ready and immediately hand the message into tokio-land, then `recv()` again — nothing expensive
//!    (no encoding, no locking, no awaiting) sits between the recvs, because the engine's bounded(1)
//!    frame slot *drops* frames while full: any delay here costs frames. The handoff splits by lane: an
//!    event goes into an unbounded mpsc (ordered, never coalesced); a frame *replaces* a single
//!    [`watch`] slot — latest-wins at this seam, so a frame the router has not picked up yet is
//!    superseded, never queued.
//! 2. **The event router** ([`event_router`]) is one tokio task that drains the event mpsc,
//!    serializing each [`EventMessage`] to JSON and handing it to the owning connection.
//! 3. **The frame router** ([`frame_router`]) is one tokio task that wakes on the frame slot, takes
//!    the freshest pending frame, encodes its raw RGBA **once per distinct format** among its
//!    registered subscribers (never per subscriber), and pushes the shared, Arc-wrapped payload of
//!    each subscriber's own format into that subscriber's latest-wins slot. A router cycle
//!    slower than the render rate (e.g. a debug-profile encode) degrades to lower delivery fps with
//!    at most ~one frame of staleness at this seam — never a growing backlog.
//! 4. **The forwarders** ([`frame_forwarder`]) are one tokio task per open `frames` subscription. Each
//!    drains its latest-wins slot, applies the subscription's `max_fps` throttle, wraps the shared
//!    payload in its own [`FrameHeader`] (only for a frame it actually sends), and queues it as a
//!    binary message on the connection's outbound sink.
//!
//! # Two outbound lanes
//!
//! A connection's [`OutboundSink`] is two independent queues, not one: text (replies and JSON events)
//! and binary frames. They are separate because a multi-megabyte frame and a request reply have
//! nothing in common but the socket — sharing one FIFO made a reply wait out every frame queued ahead
//! of it. The writer drains both, text first (see `connection.rs`), so a reply overtakes queued
//! frames rather than queueing behind them. Ordering *within* a lane is untouched: replies stay
//! strictly FIFO, events stay in `seq` order.
//!
//! # The registry
//!
//! [`Registry`] maps each open `sub` id to its [`SubState`]: an event subscription holds the owning
//! connection's outbound sender directly; a frame subscription holds a [`watch`] sender the router
//! replaces the pending frame through. A connection [`register`]s a sub the moment the
//! `stream.subscribe` reply arrives — out of reply order — and [`unregister`]s it on a
//! `stream.unsubscribe` reply or on disconnect.
//!
//! Registration races the engine: the sim opens a sub and emits for it in the same pass, while the
//! reply and the egress take different paths to the server (the blocking pool, and the pump thread →
//! router). So the registry counts subscribes *in flight* — incremented by the reader before the
//! command reaches the sim ([`subscribe_in_flight`]) — and while any exists it buffers what arrives
//! for an unknown sub: events per sub (up to [`TEXT_CAPACITY`]; the overflow is a visible `seq`
//! gap), and the latest frame per sub. [`register`] delivers that buffer in the lock that inserts the
//! sub, so nothing falls between; when no subscribe is in flight the buffers are cleared, which is
//! what keeps events for closed subs from accumulating.
//!
//! # Formats
//!
//! A subscriber names its [`FrameFormat`] when it subscribes ([`FrameFormat::Qoi`] by wire default,
//! or [`FrameFormat::Lz4Rgba`] / [`FrameFormat::RawRgba`]). Each raw frame is encoded **once per
//! distinct format among its registered subscribers** (three formats exist), never once per
//! subscriber: two subscribers on the same format share one encode and one [`Arc`], and a frame
//! nobody is registered for is not encoded at all.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver as CrossbeamReceiver;
use ludion_core::time::FrameIndex;
use ludion_engine::egress::{Egress, EgressEvent, EgressFrame};
use ludion_protocol::command::{Channel, SubscribeMode};
use ludion_protocol::event::{Event, LogEvent, LogLevel};
use ludion_protocol::frame::{FrameFormat, FrameHeader};
use ludion_protocol::{EventMessage, MAX_MESSAGE_BYTES, SubId};
use tokio::sync::{mpsc, watch};

use crate::connection::{PIPELINE_DEPTH, message_too_large};
use crate::perf::{self, DeliveredFrame, ProfileHub};

/// One encoded rendered frame queued on a connection's binary lane by its writer task.
pub(crate) struct BinaryFrame {
    /// The sim frame these bytes depict. Not on the wire — the [`FrameHeader`] inside `bytes`
    /// already carries it; this is what lets the writer file its send span under the right
    /// frame ([`ProfileHub`](crate::perf::ProfileHub)).
    pub(crate) frame: FrameIndex,
    /// The frame-index run `frame` belongs to; likewise not on the wire.
    pub(crate) timeline: u64,
    /// The encoded message (LFR1 header + payload).
    pub(crate) bytes: Vec<u8>,
}

/// Bounded depth of a connection's text lane (replies and JSON events). Small messages, so depth is
/// cheap. Sized to hold a full pipeline of replies plus a burst of events while the writer is stalled
/// behind frames, so a full lane means a client that stopped reading; only then does an event drop
/// (a visible `seq` gap, architecture §6) rather than buffering unbounded latency.
pub(crate) const TEXT_CAPACITY: usize = 4 * PIPELINE_DEPTH;

/// Bounded depth of a connection's binary frame lane. Shallow on purpose: a frame forwarder already
/// coalesces upstream through its latest-wins [`watch`] slot, so anything queued here is a frame the
/// socket has not accepted yet and depth would buy nothing but staleness. Four is enough to keep the
/// sink fed across a scheduling gap, short enough that a stalled client's backlog is measured in a
/// frame or two rather than a second of video.
pub(crate) const FRAME_CAPACITY: usize = 4;

/// The sending half of a connection's two outbound lanes, cloned into the registry and each
/// forwarder. Text (replies, events) and binary frames queue independently so a reply never waits out
/// the frames ahead of it; see the module docs.
#[derive(Clone)]
pub(crate) struct OutboundSink {
    /// Replies and JSON events, already serialized.
    pub(crate) text: mpsc::Sender<String>,
    /// Encoded rendered frames (LFR1 header + payload).
    pub(crate) frames: mpsc::Sender<BinaryFrame>,
}

/// Creates one connection's outbound lanes: the cloneable sink, and the two receivers its writer task
/// drains onto the socket.
pub(crate) fn outbound_channel() -> (
    OutboundSink,
    mpsc::Receiver<String>,
    mpsc::Receiver<BinaryFrame>,
) {
    let (text, text_rx) = mpsc::channel(TEXT_CAPACITY);
    let (frames, frames_rx) = mpsc::channel(FRAME_CAPACITY);
    (OutboundSink { text, frames }, text_rx, frames_rx)
}

/// One rendered frame encoded in one [`FrameFormat`], shared by [`Arc`] across every subscriber that
/// asked for *that* format — one of these per distinct format among a frame's subscribers (three
/// formats exist). Only the small per-sub [`FrameHeader`] differs between them, and it is prepended
/// at send time — so the megabyte payload is never cloned per subscriber, only the `Arc`.
pub(crate) struct SharedFrame {
    /// The sim frame index this image was rendered from.
    frame: FrameIndex,
    /// The frame-index run `frame` belongs to.
    timeline: u64,
    /// Image width in pixels.
    width: u32,
    /// Image height in pixels.
    height: u32,
    /// Which codec produced [`payload`](Self::payload) — stamped into every header built from it.
    format: FrameFormat,
    /// The LFR1 payload, in `format`.
    payload: FramePayload,
    /// The engine's GI staleness when this frame was emitted — stamped into every header built from
    /// it, like [`format`](Self::format).
    gi_stale: bool,
}

/// The pixel bytes behind a [`SharedFrame`]: either bytes a codec produced (owned, unique to this
/// `SharedFrame`) or, for [`FrameFormat::RawRgba`], the egress frame's own pixel buffer aliased
/// straight through — `RAW_RGBA`'s whole "encode" is choosing this variant, which costs one atomic
/// refcount bump rather than a whole-frame copy.
enum FramePayload {
    /// QOI or LZ4_RGBA bytes, built once per frame by the router's blocking-pool encode.
    Encoded(Vec<u8>),
    /// `RAW_RGBA`: the pixels are `frame.rgba`, read straight from the egress frame this
    /// [`SharedFrame`] was built from.
    Raw(Arc<EgressFrame>),
}

impl FramePayload {
    /// The pixel bytes, whichever variant holds them.
    fn as_bytes(&self) -> &[u8] {
        match self {
            FramePayload::Encoded(bytes) => bytes,
            FramePayload::Raw(frame) => &frame.rgba,
        }
    }
}

/// Per-subscriber delivery state held in the [`Registry`].
pub(crate) enum SubState {
    /// A non-frame subscription: the router sends each JSON event straight to this connection sink.
    Events(OutboundSink),
    /// A `frames` subscription: the router replaces the latest pending frame here; a [`frame_forwarder`]
    /// task drains it, throttles, and writes to the connection sink.
    Frames {
        /// The latest-wins slot the router pushes this subscription's encoded frame through.
        slot: watch::Sender<Option<Arc<SharedFrame>>>,
        /// The codec this subscriber asked for: the router only ever hands it a [`SharedFrame`]
        /// encoded in this format.
        format: FrameFormat,
    },
}

/// What the [`Registry`] guards: the open subs, and the in-flight buffer that closes the gap between
/// the sim opening a sub and the server registering it (see the module docs).
#[derive(Default)]
pub(crate) struct RegistryState {
    /// `sub` id → its delivery state.
    pub(crate) subs: HashMap<u64, SubState>,
    /// Subscribes the reader has dispatched whose sub is not registered yet.
    in_flight: usize,
    /// Events that arrived for a sub no one has registered, while a subscribe was in flight; per sub,
    /// at most [`TEXT_CAPACITY`] (the overflow is dropped, a visible `seq` gap).
    unclaimed_events: HashMap<u64, VecDeque<String>>,
    /// The latest frame addressed to a sub no one has registered, while a subscribe was in flight.
    unclaimed_frames: HashMap<u64, Arc<EgressFrame>>,
}

impl RegistryState {
    /// One in-flight subscribe is over; the last one out drops whatever nobody claimed.
    fn release_in_flight(&mut self) {
        self.in_flight = self.in_flight.saturating_sub(1);
        if self.in_flight == 0 {
            self.unclaimed_events.clear();
            self.unclaimed_frames.clear();
        }
    }
}

/// The shared subscription registry. Guarded by a plain [`Mutex`] — it is only ever held for the length
/// of a map lookup or insert, never across an `.await`.
pub(crate) type Registry = Arc<Mutex<RegistryState>>;

/// A fresh, empty subscription registry.
pub(crate) fn new_registry() -> Registry {
    Arc::new(Mutex::new(RegistryState::default()))
}

/// One `stream.subscribe` between its dispatch and its sub's registration. While any exists, the
/// registry buffers egress addressed to subs it does not know yet. Consumed by [`register`]; every
/// other way out (an error reply, a timeout, a closed connection) ends it by drop.
pub(crate) struct SubscribeInFlight {
    registry: Registry,
    spent: bool,
}

/// Counts a subscribe in flight. The reader calls this *before* it pushes the command to the sim, so
/// the count is up before the sim can open the sub and emit for it.
pub(crate) fn subscribe_in_flight(registry: &Registry) -> SubscribeInFlight {
    registry.lock().expect("registry mutex poisoned").in_flight += 1;
    SubscribeInFlight {
        registry: registry.clone(),
        spent: false,
    }
}

impl Drop for SubscribeInFlight {
    fn drop(&mut self) {
        if !self.spent
            && let Ok(mut state) = self.registry.lock()
        {
            state.release_in_flight();
        }
    }
}

/// The pump: drain the engine's two egress lanes eagerly, handing each message into tokio-land.
///
/// This runs on its own std thread (crossbeam `recv` is blocking and cannot be awaited). The loop is
/// deliberately trivial — [`select`](crossbeam_channel::select) over the two lanes, then a cheap
/// non-blocking handoff — so nothing stalls between recvs: the engine's bounded(1) frame slot drops
/// frames while it is full, so any work done here costs frames. Events go into the unbounded `events`
/// mpsc (ordered, never coalesced); a frame *replaces* the `frames` watch slot, superseding one the
/// [`frame_router`] has not picked up yet — latest-wins at this seam, so a router slower than the render
/// rate never accumulates a backlog of raw frames. The event lane is unbounded end to end (this mpsc
/// and the engine-side lane both), so an event is never dropped between the sim thread and the router.
///
/// Returns when the sim thread drops either egress sender (both drop together — they live in one
/// `EgressChannels`) or the event router's receiver is gone (the runtime is shutting down). Dropping
/// `frames` on return ends the frame router.
pub(crate) fn run_egress_pump(
    frame_rx: CrossbeamReceiver<Egress>,
    event_rx: CrossbeamReceiver<Egress>,
    events: mpsc::UnboundedSender<EgressEvent>,
    frames: watch::Sender<Option<Arc<EgressFrame>>>,
) {
    loop {
        crossbeam_channel::select! {
            recv(event_rx) -> msg => match msg {
                Ok(Egress::Event(event)) => {
                    if events.send(event).is_err() {
                        break; // the event router is gone; nothing left to hand off to
                    }
                }
                Ok(_) => {} // the event lane only ever carries events; ignore defensively
                Err(_) => break, // the sim thread stopped (both lanes disconnect together)
            },
            recv(frame_rx) -> msg => match msg {
                Ok(Egress::Frame(frame)) => {
                    let _ = frames.send_replace(Some(Arc::new(frame)));
                }
                Ok(_) => {} // the frame lane only ever carries frames; ignore defensively
                Err(_) => break, // the sim thread stopped (both lanes disconnect together)
            },
        }
    }
}

/// The event router: one tokio task that drains the pump's event bridge in order.
///
/// Every event is delivered to [`route_event`] exactly as it arrived — never coalesced, never
/// reordered (a drop only ever happens downstream, at a full outbound queue, where it stays a visible
/// `seq` gap per architecture §6). Ends when the pump drops its sender.
pub(crate) async fn event_router(
    mut events: mpsc::UnboundedReceiver<EgressEvent>,
    registry: Registry,
    hub: Arc<ProfileHub>,
) {
    while let Some(event) = events.recv().await {
        route_event(&registry, &hub, event.message, event.timeline);
    }
}

/// The frame router: one tokio task that encodes and fans out the freshest pending frame.
///
/// Wakes on the pump's watch slot and always takes what is *now* pending — frames rendered while an
/// encode was in flight replaced each other in the slot, so a slow cycle (a debug-profile encode)
/// degrades to lower delivery fps with bounded staleness, never a backlog. Ends when the pump drops
/// its sender.
pub(crate) async fn frame_router(
    mut frames: watch::Receiver<Option<Arc<EgressFrame>>>,
    registry: Registry,
    hub: Arc<ProfileHub>,
) {
    while frames.changed().await.is_ok() {
        // Clone the Arc out and release the borrow guard: a watch borrow must never cross an await.
        let frame = frames.borrow_and_update().clone();
        if let Some(frame) = frame {
            let subs = frame.subs.clone();
            route_frame(&registry, &hub, frame, subs, false).await;
        }
    }
}

/// Sends one addressed event to its subscription's connection as JSON text.
///
/// A `profile` window is completed on the way past: the engine measures its own sim and GPU layers
/// but cannot fill `transport`, so the hub's spans are injected here — **before** serialization, so
/// every subscriber receives the identical finished message rather than assembling it client-side.
///
/// A full text lane drops the message: the engine already advanced the subscription's `seq`, so
/// the client sees a gap — an intentional, visible drop, never a silent loss (architecture §6). An
/// event for a sub that is not registered *yet* — a subscribe is in flight — waits in the registry's
/// buffer and is flushed, in order, by [`register`]; with no subscribe in flight it is for a closed sub
/// and is dropped. A message over the wire ceiling is replaced by an `L3045` log in its slot.
pub(crate) fn route_event(
    registry: &Registry,
    hub: &ProfileHub,
    mut message: EventMessage,
    timeline: u64,
) {
    let sub = message.sub.as_u64();
    perf::fill_window_event(hub, &mut message, timeline);
    let text = event_text(&message, MAX_MESSAGE_BYTES);
    let mut state = registry.lock().expect("registry mutex poisoned");
    if let Some(SubState::Events(outbound)) = state.subs.get(&sub) {
        let _ = outbound.text.try_send(text);
    } else if !state.subs.contains_key(&sub) && state.in_flight > 0 {
        let queue = state.unclaimed_events.entry(sub).or_default();
        if queue.len() < TEXT_CAPACITY {
            queue.push_back(text);
        }
    }
}

/// Serializes `message`, or — when the text is over `limit` bytes — a `log` error carrying `L3045` with
/// the same `sub`/`seq`/`frame`, so the seq stays gapless and the drop is visible to the client.
fn event_text(message: &EventMessage, limit: usize) -> String {
    // `serde_json::to_string` keeps its `Serialize` bound internal, so this crate needs no direct
    // `serde` dependency; the `"{}"` floor is defensive and never reached for these wire types.
    let text = serde_json::to_string(message).unwrap_or_else(|_| "{}".to_string());
    if text.len() <= limit {
        return text;
    }
    let diagnostic = message_too_large(format!(
        "this event is {} bytes, over the {limit}-byte wire limit; it was not sent",
        text.len()
    ));
    let replacement = EventMessage {
        event: Event::Log(LogEvent {
            level: LogLevel::Error,
            message: diagnostic.to_string(),
            diagnostic: Some(diagnostic),
        }),
        sub: message.sub,
        seq: message.seq,
        frame: message.frame,
    };
    serde_json::to_string(&replacement).unwrap_or_else(|_| "{}".to_string())
}

/// Encodes one rendered frame once per format its subscribers asked for, and pushes each subscriber
/// the payload in *its* format.
///
/// A subscriber named in `subs` that is not registered is skipped — unless a subscribe is in flight, in
/// which case the frame is stashed for [`register`] to replay (see the module docs): a paused session
/// emits its first frame once, and nothing would replace it.
///
/// The encodes are CPU-bound (a whole frame each), so they run on the blocking pool rather than a
/// runtime worker: awaiting them yields the worker, keeping the per-subscriber forwarders and
/// connection I/O responsive instead of being starved behind a back-to-back encode loop.
///
/// While the profile hub is armed, the encoding is timed *inside* the blocking closure: the span
/// brackets the codec calls and nothing else — not the handoff to the pool, not the fan-out. It is
/// one value per frame however many formats are subscribed, so `encode_median_ms` names this frame's
/// total codec cost on the blocking pool. [`FrameFormat::RawRgba`] is inside the span like the
/// others even though its "encode" is now just an `Arc` clone of the egress frame (see
/// [`FramePayload::Raw`]) rather than a payload copy — a raw-only frame therefore reports a near-zero
/// `encode_median_ms`. `RAW_RGBA` still pays one payload copy per delivered frame, but it has moved
/// downstream, into each subscriber's own [`FrameHeader::encode`] call in [`frame_forwarder`].
///
/// `subs` is who the frame is delivered to: the frame's own subscriber list, or a single sub when
/// [`register`] replays a stashed frame to a late registration (`replay`, see [`deliver`]).
async fn route_frame(
    registry: &Registry,
    hub: &Arc<ProfileHub>,
    frame: Arc<EgressFrame>,
    subs: Vec<SubId>,
    replay: bool,
) {
    let index = DeliveredFrame {
        timeline: frame.timeline,
        frame: frame.frame,
    };
    let width = frame.width;
    let height = frame.height;
    let gi_stale = frame.gi_stale;
    // Snapshot which formats are actually wanted *before* encoding: work nobody subscribed to is
    // work not done. One bool per variant rather than a set — `FrameFormat` is a closed enum, so the
    // wanted set is at most `{Qoi, Lz4Rgba, RawRgba}` and a set would only add an allocation and a
    // hash. The snapshot also stashes the frame for a sub that is unknown while a subscribe is in
    // flight; a sub registering after this snapshot is therefore never missed: either the stash
    // happened first and [`register`] replays it, or the sub is already known here.
    let (want_qoi, want_lz4, want_raw) = {
        let mut state = registry.lock().expect("registry mutex poisoned");
        let mut want_qoi = false;
        let mut want_lz4 = false;
        let mut want_raw = false;
        for sub in &subs {
            match state.subs.get(&sub.as_u64()) {
                Some(SubState::Frames { format, .. }) => match format {
                    FrameFormat::Qoi => want_qoi = true,
                    FrameFormat::Lz4Rgba => want_lz4 = true,
                    FrameFormat::RawRgba => want_raw = true,
                },
                Some(SubState::Events(_)) => {}
                None if state.in_flight > 0 => {
                    state.unclaimed_frames.insert(sub.as_u64(), frame.clone());
                }
                None => {}
            }
        }
        (want_qoi, want_lz4, want_raw)
    };
    if !want_qoi && !want_lz4 && !want_raw {
        // Nobody is registered for this frame; encoding it would be pure waste.
        return;
    }
    // Encode once per wanted format, off the worker threads — never per subscriber (architecture
    // §2, §6). A malformed image (never expected for engine-produced RGBA) or a panicked encode
    // just skips that format for this frame.
    let encode_hub = Arc::clone(hub);
    let encoded = tokio::task::spawn_blocking(move || {
        let started = encode_hub.armed().then(Instant::now);
        let qoi = want_qoi.then(|| qoi::encode_to_vec(&frame.rgba, width, height));
        // The raw *block* format, deliberately not `compress_prepend_size`: the LFR1 header already
        // carries width and height, so the decoder allocates `width * height * 4` and a prepended
        // length would be a second, contradictable source of truth about the same frame.
        let lz4 = want_lz4.then(|| lz4_flex::block::compress(&frame.rgba));
        // The raw lane's whole "encode" is choosing to keep this frame's `Arc` around — an atomic
        // refcount bump, not a copy of the pixels. `FramePayload::Raw` reads straight from it at
        // send time, so `frame.rgba` is never duplicated on this path at all.
        let raw = want_raw.then(|| Arc::clone(&frame));
        if let Some(started) = started {
            encode_hub.record_encode(index, started.elapsed());
        }
        (qoi, lz4, raw)
    })
    .await;
    let (qoi, lz4, raw) = match encoded {
        Ok(encoded) => encoded,
        Err(_) => return, // the blocking task panicked or was cancelled; skip this frame
    };
    let build = |format: FrameFormat, payload: FramePayload| {
        Arc::new(SharedFrame {
            frame: index.frame,
            timeline: index.timeline,
            width,
            height,
            format,
            payload,
            gi_stale,
        })
    };
    // A QOI encode error skips that format's fan-out; LZ4 block compression and the raw alias are
    // infallible.
    let qoi = qoi
        .and_then(Result::ok)
        .map(|payload| build(FrameFormat::Qoi, FramePayload::Encoded(payload)));
    let lz4 = lz4.map(|payload| build(FrameFormat::Lz4Rgba, FramePayload::Encoded(payload)));
    let raw = raw.map(|frame| build(FrameFormat::RawRgba, FramePayload::Raw(frame)));
    let state = registry.lock().expect("registry mutex poisoned");
    for sub in &subs {
        if let Some(SubState::Frames { slot, format }) = state.subs.get(&sub.as_u64()) {
            let shared = match format {
                FrameFormat::Qoi => &qoi,
                FrameFormat::Lz4Rgba => &lz4,
                FrameFormat::RawRgba => &raw,
            };
            if let Some(shared) = shared {
                deliver(slot, shared.clone(), replay);
            }
        }
    }
}

/// Puts `shared` in a subscription's slot. A watch send never blocks: a live frame replaces the
/// pending one (latest-wins at the slot).
///
/// A *replay* — a frame stashed while the subscribe was in flight — writes only into an empty slot.
/// Anything already in a freshly registered sub's slot was routed after registration, so it is at
/// least as new; this also holds across a frame-index reset (scene load, `engine.restore`), where
/// comparing indices would not.
fn deliver(slot: &watch::Sender<Option<Arc<SharedFrame>>>, shared: Arc<SharedFrame>, replay: bool) {
    if replay {
        slot.send_if_modified(|current| {
            let empty = current.is_none();
            if empty {
                *current = Some(shared);
            }
            empty
        });
    } else {
        let _ = slot.send(Some(shared));
    }
}

/// One frame subscription's forwarder task: drain the latest-wins slot, throttle, write to the sink.
///
/// Wakes on each new frame in `slot`, waits out the `max_fps` interval since its last send (coalescing
/// any frames that arrive during the wait — it always sends the freshest), prepends this subscription's
/// own [`FrameHeader`], and queues the binary message on the connection's frame lane. The header is
/// built only for a frame actually sent, so throttled/superseded frames cost no header work.
///
/// The queue send *awaits*: a full frame lane backpressures this task, and the pressure lands in the
/// latest-wins slot above, where coalescing is free. That is the opposite of an event's behaviour on
/// the text lane (which drops), and it is why the frame lane can be shallow.
///
/// Ends when the registry drops the slot's sender (the sub was unregistered — unsubscribe or
/// disconnect) or the connection's frame lane closes (the writer task is gone).
async fn frame_forwarder(
    sub: SubId,
    mut slot: watch::Receiver<Option<Arc<SharedFrame>>>,
    outbound: OutboundSink,
    max_fps: Option<f32>,
) {
    // The engine validates `max_fps`; this still never panics on one it let through — a rate too small
    // for a `Duration`, or not finite, simply means no cap.
    let min_interval = max_fps.and_then(|fps| {
        (fps.is_finite() && fps > 0.0)
            .then(|| Duration::try_from_secs_f32(1.0 / fps).ok())
            .flatten()
    });
    let mut last_sent: Option<Instant> = None;
    loop {
        // Wait for a newer frame; an error means the sender was dropped (unregistered).
        if slot.changed().await.is_err() {
            break;
        }
        // Throttle: hold off until the max_fps interval since the last send has elapsed. Frames that
        // arrive during the wait replace the slot; we pick up the freshest below.
        if let (Some(interval), Some(sent)) = (min_interval, last_sent) {
            let since = sent.elapsed();
            if since < interval {
                tokio::time::sleep(interval - since).await;
            }
        }
        // Grab the freshest pending frame (coalescing anything that landed during the throttle wait).
        let shared = match slot.borrow_and_update().clone() {
            Some(shared) => shared,
            None => continue, // only the initial empty slot; wait for a real frame
        };
        let header = FrameHeader {
            sub,
            frame: shared.frame,
            format: shared.format,
            width: shared.width,
            height: shared.height,
            gi_stale: shared.gi_stale,
        };
        let message = BinaryFrame {
            frame: shared.frame,
            timeline: shared.timeline,
            bytes: header.encode(shared.payload.as_bytes()),
        };
        if outbound.frames.send(message).await.is_err() {
            break; // the connection's writer is gone
        }
        last_sent = Some(Instant::now());
    }
}

/// Registers a freshly opened subscription so egress starts flowing to `outbound`.
///
/// For `frames` this spawns the subscription's [`frame_forwarder`] and stores the latest-wins slot the
/// router pushes through, alongside the `format` this subscriber asked for (which decides which of the
/// router's encodes it is handed); for every other channel it stores the connection sink the router
/// sends events to directly and `format` is meaningless. Called from the connection's subscribe task the
/// instant the reply arrives — not when the pump reaches it — so it runs inside the tokio runtime
/// (required for [`tokio::spawn`]).
///
/// In the one lock that inserts the sub, anything the in-flight buffer holds for it is delivered: the
/// buffered events in order, or the stashed frame replayed to this sub alone. `in_flight` is consumed
/// in that same lock, so no event or frame can fall between the buffer and the registered sub.
#[allow(clippy::too_many_arguments)] // one owned seam per collaborator; a bag struct would hide them
pub(crate) fn register(
    registry: &Registry,
    hub: &Arc<ProfileHub>,
    sub: SubId,
    channel: Channel,
    mode: SubscribeMode,
    format: FrameFormat,
    outbound: &OutboundSink,
    mut in_flight: SubscribeInFlight,
) {
    let key = sub.as_u64();
    let mut guard = registry.lock().expect("registry mutex poisoned");
    let state = &mut *guard;
    let mut replay = None;
    let sub_state = match channel {
        Channel::Frames => {
            let SubscribeMode::LatestWins { max_fps } = mode;
            let (slot_tx, slot_rx) = watch::channel(None);
            tokio::spawn(frame_forwarder(sub, slot_rx, outbound.clone(), max_fps));
            replay = state.unclaimed_frames.remove(&key);
            SubState::Frames {
                slot: slot_tx,
                format,
            }
        }
        _ => {
            for text in state.unclaimed_events.remove(&key).unwrap_or_default() {
                let _ = outbound.text.try_send(text);
            }
            SubState::Events(outbound.clone())
        }
    };
    state.subs.insert(key, sub_state);
    in_flight.spent = true;
    state.release_in_flight();
    drop(guard);
    if let Some(frame) = replay {
        let (registry, hub) = (registry.clone(), hub.clone());
        tokio::spawn(async move { route_frame(&registry, &hub, frame, vec![sub], true).await });
    }
}

/// Unregisters a subscription: the router stops delivering to it and (for `frames`) dropping the slot
/// sender ends its forwarder task. Idempotent — unregistering an unknown id is a no-op.
pub(crate) fn unregister(registry: &Registry, sub: SubId) {
    registry
        .lock()
        .expect("registry mutex poisoned")
        .subs
        .remove(&sub.as_u64());
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_engine::egress::EgressEvent;
    use ludion_protocol::Seq;
    use ludion_protocol::event::{Event, LogEvent, LogLevel};

    fn shared(index: u64) -> Arc<SharedFrame> {
        Arc::new(SharedFrame {
            frame: FrameIndex::new(index),
            timeline: 0,
            width: 2,
            height: 1,
            format: FrameFormat::Qoi,
            payload: FramePayload::Encoded(Vec::new()),
            gi_stale: false,
        })
    }

    fn held(slot: &watch::Sender<Option<Arc<SharedFrame>>>) -> Option<u64> {
        slot.borrow().as_ref().map(|f| f.frame.as_u64())
    }

    #[test]
    fn a_replay_never_overwrites_a_newer_frame_but_a_live_one_does() {
        let (slot, _rx) = watch::channel(None);
        deliver(&slot, shared(5), true);
        assert_eq!(held(&slot), Some(5), "a replay fills an empty slot");

        let (slot, _rx) = watch::channel(Some(shared(6)));
        deliver(&slot, shared(5), true);
        assert_eq!(held(&slot), Some(6), "a replay never goes backwards");

        deliver(&slot, shared(5), false);
        assert_eq!(held(&slot), Some(5), "a live frame is latest-wins");
    }

    fn frame(index: u64) -> EgressFrame {
        EgressFrame {
            frame: FrameIndex::new(index),
            timeline: 0,
            width: 2,
            height: 1,
            rgba: vec![0; 8],
            subs: vec![SubId::new(1)],
            gi_stale: false,
        }
    }

    fn event(seq: u64) -> EgressEvent {
        EgressEvent {
            timeline: 0,
            message: EventMessage {
                event: Event::Log(LogEvent {
                    level: LogLevel::Info,
                    message: format!("e{seq}"),
                    diagnostic: None,
                }),
                sub: SubId::new(2),
                seq: Seq::new(seq),
                frame: None,
            },
        }
    }

    /// The header's flags byte says what the sim thread said. `EgressFrame::gi_stale` is filled from
    /// [`Engine::gi_stale`](ludion_engine::Engine::gi_stale) at emission; from there to the wire it
    /// crosses the encode fan-out and one subscriber's forwarder, which is the stretch that could
    /// silently drop it — the header is built from the [`SharedFrame`], not from the `EgressFrame`.
    #[test]
    fn the_frames_stale_flag_survives_the_router_and_the_forwarder() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a test runtime");
        runtime.block_on(async {
            for gi_stale in [false, true] {
                let registry = new_registry();
                let (outbound, _text_rx, mut inbox) = outbound_channel();
                let sub = SubId::new(1);
                let hub = ProfileHub::new();
                register(
                    &registry,
                    &hub,
                    sub,
                    Channel::Frames,
                    SubscribeMode::default(),
                    // Raw: the payload is the pixels, so nothing about this test depends on a codec.
                    FrameFormat::RawRgba,
                    &outbound,
                    subscribe_in_flight(&registry),
                );
                let routed = Arc::new(EgressFrame {
                    gi_stale,
                    ..frame(3)
                });
                let subs = routed.subs.clone();
                route_frame(&registry, &hub, routed, subs, false).await;

                let message = tokio::time::timeout(Duration::from_secs(5), inbox.recv())
                    .await
                    .expect("the forwarder sends within the timeout")
                    .expect("a frame message");
                let (header, _) =
                    FrameHeader::decode(&message.bytes).expect("a well-formed LFR1 frame");
                assert_eq!(
                    header.gi_stale, gi_stale,
                    "the wire reports the engine's own staleness"
                );
            }
        });
    }

    /// W4: a paused session emits its one first frame when the sub opens; if that lands before the
    /// server registers the sub, no later frame ever replaces it.
    #[test]
    fn a_paused_sessions_first_frame_reaches_a_late_registration() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a test runtime");
        runtime.block_on(async {
            let registry = new_registry();
            let hub = ProfileHub::new();
            let (outbound, _text_rx, mut inbox) = outbound_channel();
            let sub = SubId::new(1);
            let in_flight = subscribe_in_flight(&registry);
            let early = Arc::new(frame(5));
            let subs = early.subs.clone();
            route_frame(&registry, &hub, early, subs, false).await;
            register(
                &registry,
                &hub,
                sub,
                Channel::Frames,
                SubscribeMode::default(),
                FrameFormat::RawRgba,
                &outbound,
                in_flight,
            );

            let message = tokio::time::timeout(Duration::from_secs(5), inbox.recv())
                .await
                .expect("the stashed frame is delivered on registration")
                .expect("a frame message");
            let (header, _) = FrameHeader::decode(&message.bytes).expect("a well-formed frame");
            assert_eq!(header.frame, FrameIndex::new(5));
        });
    }

    /// With no subscribe in flight an event for an unknown sub is for a closed one: it is dropped,
    /// and nothing it left behind outlives the last in-flight subscribe.
    #[test]
    fn unclaimed_egress_does_not_accumulate() {
        let registry = new_registry();
        let hub = ProfileHub::new();
        route_event(&registry, &hub, event(0).message, 0);
        assert!(registry.lock().unwrap().unclaimed_events.is_empty());

        let in_flight = subscribe_in_flight(&registry);
        route_event(&registry, &hub, event(1).message, 0);
        assert_eq!(registry.lock().unwrap().unclaimed_events.len(), 1);
        drop(in_flight);
        assert!(registry.lock().unwrap().unclaimed_events.is_empty());
    }

    /// An event past the wire ceiling is replaced by an `L3045` log in the same slot: the seq stays
    /// gapless and the drop is visible.
    #[test]
    fn an_over_limit_event_becomes_an_l3045_log_in_its_slot() {
        let message = event(9).message;
        let text = event_text(&message, 16);
        let back: EventMessage = serde_json::from_str(&text).unwrap();
        assert_eq!(back.seq, message.seq);
        assert_eq!(back.sub, message.sub);
        let Event::Log(log) = back.event else {
            panic!("expected a log event, got {text}");
        };
        assert_eq!(log.level, LogLevel::Error);
        assert_eq!(
            log.diagnostic.expect("a diagnostic").code.to_string(),
            "L3045"
        );
        assert_eq!(
            event_text(&message, usize::MAX),
            serde_json::to_string(&message).unwrap()
        );
    }

    /// A rate no `Duration` can hold must not panic the forwarder: it forwards, uncapped.
    #[test]
    fn a_max_fps_no_duration_can_hold_does_not_panic_the_forwarder() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a test runtime");
        runtime.block_on(async {
            for fps in [1e-30_f32, f32::NAN, -1.0, f32::INFINITY] {
                let registry = new_registry();
                let hub = ProfileHub::new();
                let (outbound, _text_rx, mut inbox) = outbound_channel();
                register(
                    &registry,
                    &hub,
                    SubId::new(1),
                    Channel::Frames,
                    SubscribeMode::LatestWins { max_fps: Some(fps) },
                    FrameFormat::RawRgba,
                    &outbound,
                    subscribe_in_flight(&registry),
                );
                for index in [1, 2] {
                    let routed = Arc::new(frame(index));
                    let subs = routed.subs.clone();
                    route_frame(&registry, &hub, routed, subs, false).await;
                    tokio::time::timeout(Duration::from_secs(5), inbox.recv())
                        .await
                        .expect("the forwarder still forwards")
                        .expect("a frame message");
                }
            }
        });
    }

    /// The regression test for the unbounded-backlog bug: with no consumer draining the frame slot,
    /// the pump's seam must coalesce frames (latest-wins — only the last one is pending) while every
    /// event still arrives, in order, on its own bridge. The two egress lanes are separate channels now
    /// (audit finding 1.3), so this feeds frames and events independently and asserts the same
    /// guarantees — frame latest-wins unchanged, events lossless and ordered.
    #[test]
    fn pump_coalesces_frames_but_never_events() {
        let (frame_egress_tx, frame_egress_rx) = crossbeam_channel::unbounded::<Egress>();
        let (event_egress_tx, event_egress_rx) = crossbeam_channel::unbounded::<Egress>();
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<EgressEvent>();
        let (frame_tx, frame_rx) = watch::channel::<Option<Arc<EgressFrame>>>(None);
        let pump = std::thread::spawn(move || {
            run_egress_pump(frame_egress_rx, event_egress_rx, event_tx, frame_tx)
        });

        for i in 0..3 {
            frame_egress_tx.send(Egress::Frame(frame(i))).unwrap();
            event_egress_tx.send(Egress::Event(event(i))).unwrap();
        }
        drop(frame_egress_tx);
        drop(event_egress_tx);
        pump.join().unwrap();

        let pending = frame_rx.borrow().clone().expect("a frame is pending");
        assert_eq!(
            pending.frame,
            FrameIndex::new(2),
            "the slot holds exactly the last frame sent"
        );

        for i in 0..3 {
            let message = event_rx.try_recv().expect("every event arrives");
            assert_eq!(
                message.message.seq,
                Seq::new(i),
                "events arrive in send order"
            );
        }
        assert!(
            event_rx.try_recv().is_err(),
            "no duplicated or invented events"
        );
    }
}
