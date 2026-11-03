//! The egress seam: the messages the sim thread hands *out* to a transport, and how they are addressed.
//!
//! This is the outbound half of the threading model (engine-architecture.md §3, §6), the mirror of
//! [`live`](crate::live)'s inbound intake channel. Where intake carries [`StampedCommand`](crate::live::StampedCommand)s *into*
//! the sim thread, egress carries rendered frames and subscription events *out* of it — over one more
//! [`crossbeam_channel`], so `ludion-engine` stays tokio-free and the server plugs in as a peer. The
//! server owns the receiving half, encodes each message for the wire (frames → the subscriber-selected
//! `FrameFormat` binary payload, events → JSON), and fans it out to the WebSocket connections.
//!
//! # Addressing
//!
//! The two message kinds are addressed differently, matching the two channel styles in §6:
//!
//! - [`EgressFrame`] is *fan-out addressed*: one render, carrying the id list of every open `frames`
//!   subscriber it should reach. Frames are delivered as latest-wins binary messages keyed by
//!   `frame` index (no per-subscriber sequence), and the encoding is the server's job — so the
//!   engine ships raw RGBA plus the subscriber list, nothing more.
//! - [`EgressEvent`] is *fully addressed*: exactly one per (event, subscriber) pair, each carrying a
//!   complete [`EventMessage`] with the subscription's own monotonic [`Seq`](ludion_protocol::envelope::Seq).
//!   The engine assigns the seq (from the subscription table). Events ride their **own** ordered,
//!   unbounded lane (see [`EgressChannels`]), so a burst of events in one tick is never dropped at this
//!   seam — the seq stream a subscriber sees is gapless. A drop can still show as a visible seq gap, but
//!   only far downstream at a wedged connection's bounded outbound queue (architecture §6), never here.
//!
//! # Two lanes: latest-wins frames, ordered events
//!
//! A transport hands [`run_live`](crate::live::run_live) an [`EgressChannels`] — two crossbeam
//! channels, addressed by kind:
//!
//! - **frames** — expected to be `crossbeam_channel::bounded(1)`, a single-slot latest-wins-at-source
//!   buffer. Before rendering, the loop checks the slot has room
//!   ([`Sender::is_full`](crossbeam_channel::Sender::is_full)); if the previous frame has not been
//!   drained it skips rendering entirely rather than produce a frame nobody can consume. An unbounded
//!   channel is accepted too (it simply never reports full), but bounded(1) is the intended shape.
//! - **events** — expected to be `crossbeam_channel::unbounded`, an ordered lane the loop never coalesces
//!   or drops from. Keeping events off the frame slot is what makes event delivery lossless: a frame
//!   sitting in the bounded(1) slot can no longer displace an event (audit finding 1.3). The lane is
//!   unbounded rather than capped-and-dropping because a dropped event is exactly the bug being fixed; a
//!   downstream pump drains it eagerly with no blocking work, so unboundedness only ever grows memory if
//!   that pump is itself wedged (a teardown-only window), mirroring the already-unbounded pump→router
//!   bridge.

use crossbeam_channel::Sender;
use ludion_core::time::FrameIndex;
use ludion_protocol::EventMessage;
use ludion_protocol::event::Event;
use ludion_protocol::{Channel, SubId};

/// One message the sim thread emits to a transport over the egress channel.
///
/// The transport receives these on the far half of the channel it handed to
/// [`run_live`](crate::live::run_live) and turns each into its wire form.
#[derive(Clone, Debug, PartialEq)]
pub enum Egress {
    /// A rendered frame, fan-out addressed to every open `frames` subscriber (see [`EgressFrame`]).
    Frame(EgressFrame),
    /// One subscription event, fully addressed to a single subscriber (see [`EgressEvent`]).
    Event(EgressEvent),
}

/// The two outbound lanes a transport hands to [`run_live`](crate::live::run_live).
///
/// Splitting frames from events at the source is the fix for audit finding 1.3: a single shared
/// `bounded(1)` slot let a pending frame displace an event, so an agent subscribing to `sim.events`/`log`
/// to verify gameplay had no lossless option. With separate lanes, events on [`events`](EgressChannels::events)
/// never contend with the latest-wins frame slot and their seq stream stays gapless.
///
/// The item type is [`Egress`] on both lanes for a uniform receiver; by convention the frame lane only
/// ever carries [`Egress::Frame`] and the event lane only [`Egress::Event`] (the loop enforces this — it
/// never mixes them).
pub struct EgressChannels {
    /// The frame lane — expected `bounded(1)`, latest-wins-at-source (see the module docs). Carries only
    /// [`Egress::Frame`].
    pub frames: Sender<Egress>,
    /// The event lane — expected `unbounded`, ordered and never coalesced. Carries only
    /// [`Egress::Event`]; this is the lossless path for subscription events.
    pub events: Sender<Egress>,
}

/// A rendered frame plus the subscribers it should reach.
///
/// The pixels are raw, packed RGBA8 (`rgba.len() == width * height * 4`, row-major, top row first);
/// the server encodes them into each subscriber's chosen `FrameFormat` (QOI, LZ4_RGBA, or RAW_RGBA)
/// and writes a binary [`frame`](ludion_protocol::frame) message per subscriber in `subs`. Frame
/// encoding is deliberately *not* the engine's job (architecture §2: "Frame encoding happens in the
/// server").
#[derive(Clone, Debug, PartialEq)]
pub struct EgressFrame {
    /// The sim frame index this image was rendered from.
    pub frame: FrameIndex,
    /// The frame-index run `frame` belongs to ([`Engine::frame_timeline`](crate::Engine::frame_timeline)
    /// at emission); a load or restore starts a new one.
    pub timeline: u64,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Packed RGBA8 pixels, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
    /// Whether this host's global illumination is being *held* rather than rebuilt for the current
    /// world ([`Engine::gi_stale`](crate::Engine::gi_stale)), which the server stamps into the frame
    /// header's flags byte. Staleness is a property of the session's lever, not of one image, so
    /// reading it at emission is honest even though the readback ring hands back an earlier frame's
    /// pixels.
    pub gi_stale: bool,
    /// The open `frames` subscriptions this frame should be delivered to.
    pub subs: Vec<SubId>,
}

/// One subscription event, fully addressed and sequenced.
///
/// The engine builds exactly one of these per (event, subscriber) pair, with the `sub` and monotonic
/// `seq` already filled in from the subscription table; the server serializes the [`EventMessage`] to
/// JSON and sends it to that one subscriber's connection.
#[derive(Clone, Debug, PartialEq)]
pub struct EgressEvent {
    /// The complete, addressed event envelope (`event` + `data`, `sub`, `seq`, `frame`).
    pub message: EventMessage,
    /// The frame-index run the event's `frame` belongs to ([`Engine::frame_timeline`](crate::Engine::frame_timeline)).
    pub timeline: u64,
}

/// The subscription [`Channel`] an [`Event`] is delivered on.
///
/// The event variant fixes its channel. [`Event`] is `#[non_exhaustive]`, so this cross-crate match
/// carries a wildcard — but the engine constructs every event it emits, so a variant arriving without
/// a channel mapping is an engine bug (a new variant added without extending this table), not a
/// runtime input; the wildcard panics rather than inventing a channel.
pub(crate) fn event_channel(event: &Event) -> Channel {
    match event {
        Event::Log(_) => Channel::Log,
        Event::SimEvents(_) => Channel::SimEvents,
        Event::SceneChanged(_) => Channel::SceneChanged,
        Event::AssetReloaded(_) => Channel::AssetReloaded,
        Event::ProfileWindow(_) => Channel::Profile,
        _ => unreachable!("a new Event variant needs its delivery Channel mapped in event_channel"),
    }
}
