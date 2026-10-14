---
description: The path from a sim tick to viewer pixels — egress, per-format encode, delivery, TS decode
---

# frame-streaming

**Scope:** The cross-cutting path from an engine tick to pixels painted in a viewer's `<canvas>`:
`ludion-engine`'s egress seam → `ludion-server`'s pump/router/forwarder delivery → the binary LFR1
wire message → `ludion-client`'s or `editor/app`'s decode. This doc is the map that ties five crates
together and states the properties (latest-wins staging at every hop, visible-not-silent drops,
`max_fps` throttling, the debug-build QOI bottleneck) that only make sense read end to end; it
defers each crate's internals to its own topic. Read this before touching *any* leg of the
streaming path, or when debugging "the viewer looks laggy/stuck/behind." Consumers: anyone changing
`egress.rs`, `delivery.rs`, `connection.rs`, the client's stream API, or `editor/app`'s frame/event
consumption.

## Layout

No files of its own — this doc is pure cross-reference. The path, in order:

- `crates/ludion-engine/src/live.rs` — `run_live`'s `emit_egress` step (renders + drains events).
- `crates/ludion-engine/src/egress.rs` — `Egress`/`EgressFrame`/`EgressEvent`, `event_channel`; read
  its module doc for the fan-out-addressed-vs-fully-addressed split.
- `crates/ludion-server/src/lib.rs` — wires the two egress lanes (bounded(1) frames, unbounded
  events), spawns the pump thread and the two router tasks.
- `crates/ludion-server/src/delivery.rs` — the pump (`run_egress_pump`), the routers
  (`event_router`, `frame_router`, `route_event`, `route_frame`), the per-subscription
  `frame_forwarder`.
- `crates/ludion-server/src/connection.rs` — subscribe/unsubscribe registration, disconnect cleanup.
- `crates/ludion-protocol/src/frame.rs` — the LFR1 binary header both Rust and TS decode.
- `crates/ludion-client/src/lib.rs` — `next_frame`/`next_event`, the Rust consumption half.
- `crates/ludion-window/src/reconnect.rs` — `default_stream_format`, the windowed Rust viewer's
  loopback-vs-remote codec pick.
- `crates/ludion-window/src/frame.rs` — `decode_frame`, the windowed viewer's own synchronous
  decoder for whichever of the three formats it subscribed with.
- `editor/app/src/protocol/` and `editor/app/src/viewer/` — the TS decode half; file map in
  `ludion devdocs frame-streaming-viewer`.
- `crates/ludion-server/tests/phase4.rs` — the ≥30 fps / debug-floor profile-gated assertion.

## Rules

- **Frame encoding happens in the server, never the engine** (`engine-architecture.md` §2). The
  engine ships raw RGBA plus a subscriber list; encoding into whichever of the three wire formats a
  subscriber asked for (QOI, LZ4_RGBA, RAW_RGBA — measurements in `ludion devdocs frame-streaming-codecs`) is
  `ludion-server`'s job, done once per distinct subscribed format — at most three encodes, never one
  per subscriber and never one per format that nobody actually subscribed with.
- **Latest-wins at every frame stage; events ordered, with visible `seq` gaps if the client stops
  reading.** The engine's frame egress lane is
  `bounded(1)` (source); the pump's handoff into tokio is a `watch` slot the pump *replaces*
  (the pump seam — a frame the router hasn't picked up is superseded, never queued); the router's
  per-subscriber `watch` slot is latest-wins again (fan-out); the forwarder always sends the
  freshest pending frame, never a queue of stale ones. A slow consumer at any stage loses old
  frames, never falls behind on a growing backlog. Events take a separate, unbounded, ordered lane out
  of the engine and through the pump — never coalesced, never dropped at that seam (audit finding 1.3).
- **A drop is always visible, never silent.** Events carry a `Seq` assigned inside the engine
  *before* the message leaves the sim thread; a downstream drop (full outbound queue, vanished
  registry entry) still consumes that `seq`, so the next event a client actually sees has a gap.
  Frames carry a `FrameIndex` the same way — non-consecutive frame indices tell a consumer frames
  were skipped, which is expected for a latest-wins video-like stream, not a bug to chase.
- **Rendering for delivery never perturbs the sim.** It is a pure read of the world — never mutates
  state or the `WorldHash` — so an attached, rendering viewer cannot change what a headless run
  would have hashed. This is what `ludion-server`'s recorded-viewer-session replay criterion
  (`tests/phase4.rs`) actually proves, not just a design intention.

## How it works

1. **Tick.** `run_live` ticks per its wall-clock schedule (see `ludion-engine`'s doc) — untouched by
   whether anything is subscribed.
2. **Egress slot (pipelined, non-stalling).** After ticking, `emit_egress` (in `live.rs`) decides
   whether a frame is owed — the dirty flag, set whenever any command drained since the last
   *submit*, or the frame index having advanced — and whether a `frames` subscriber exists with room
   in the egress slot. Submission and readback are **decoupled** across a persistent 3-deep async
   ring (`ludion-render-graph`'s `StreamReadback`, driven via `Engine::render_frame`): the sim
   thread submits this frame's render + texture→buffer copy + `map_async` into the next ring slot,
   then **non-blocking-collects** every readback that has *already* finished on the GPU
   (`device.poll(Poll)` + `try_recv`s, never `poll(Wait)`) and delivers the newest (latest-wins
   inside the ring). The frame it delivers is that collected readback — an *older* frame — sent as
   an `EgressFrame` stamped with the `FrameIndex` captured when it was submitted (not the frame
   current now), naming every subscriber id that should receive it. The sim thread blocks **only**
   when all three slots are still in flight: it waits once, drains every finished slot and delivers
   the newest, so the ring is not full again. That caps stream latency at ~2 frames in the worst
   case (engine-architecture.md §readback) rather than stalling on a full render+readback every
   frame. **First latest-wins stage:** a submit that would overwrite an undrained egress slot is
   skipped, and a pass with nothing new to submit still drains the finished readbacks so the tail of
   a burst arrives; when the egress slot is full the in-flight readbacks are held for a later pass,
   never dropped. Losing the last `frames` subscriber discards the ring — nothing is ever collected
   for zero subscribers. (The blocking single-frame path stays exactly as it was for `ludion render`
   and `render.screenshot`, which need exact-frame semantics.)
3. **Pump.** `ludion-server`'s `run_egress_pump` (its own std thread) does nothing but `select!` over
   the two crossbeam lanes and hand each message into tokio — kept trivial on purpose, since any delay
   here costs frames (the frame lane upstream drops while full): an event is sent
   into an unbounded, ordered `mpsc`; a frame *replaces* a single `watch` slot. **Second latest-wins
   stage** (the pump seam): a frame the router has not yet picked up is superseded in the slot,
   never queued behind it.
4. **Routers.** `event_router` drains the event bridge in order; each event is serialized to JSON
   and sent straight to its one owning connection (`route_event`, no coalescing — events are not
   fungible, dropping one must stay visible). `frame_router` wakes on the frame slot and takes the
   freshest pending frame; `route_frame` scans the subscribers actually registered for it and encodes
   the raw RGBA **once per distinct format among them** — at most three encodes (QOI, LZ4_RGBA,
   RAW_RGBA), never one per subscriber, and a format nobody asked for is not encoded at all — on the
   blocking pool, then pushes each shared `Arc`-wrapped payload into every subscriber on that format's
   own `watch` slot. **Third latest-wins stage,** this time per subscriber.
5. **Forwarder.** One tokio task per open `frames` subscription wakes on its `watch` slot, applies
   the subscription's `max_fps` throttle (sleeping out the remaining interval since its last send,
   then always grabbing the *freshest* pending frame — coalescing anything that arrived mid-wait),
   prepends its own `FrameHeader` (sub id, frame index, format, flags, dimensions), and queues an LFR1
   binary message on the connection's outbound sink.
6. **Wire.** The connection's writer task drains its two outbound lanes onto the WebSocket, text
   (replies, JSON events) in preference to binary (frames) — a reply overtakes the frames already
   queued instead of waiting out their megabytes.
7. **Decode.** The viewer decodes off the message-event thread (worker, latest-wins pump, one codec switch);
   the Rust consumers leave payloads encoded or decode synchronously — all in
   `ludion devdocs frame-streaming-viewer`.

**Registration is a benign race.** A connection opens its delivery slot only once it observes the
`stream.subscribe` *reply* flow back through it (`ludion-server`'s `connection.rs`); a frame the
engine addresses to that `sub` before registration finishes is held in the registry's in-flight
buffer while any subscribe is unresolved, then delivered to the sub that claims it. See
`ludion-server`'s doc for the full detail.

**`max_fps` throttles delivery, not rendering.** The engine still renders at whatever rate the world
changes and the wall clock allows (uncapped); `max_fps` (an optional field on
`SubscribeMode::LatestWins`) is enforced only in the forwarder. `None` means uncapped delivery,
bounded only by render rate and the `watch` slot's own coalescing.

**Codec cost and the debug-build cap.** The debug-profile QOI encode that once capped a debug plaza
run at ~19 fps and its fix are in `ludion devdocs frame-streaming-codecs`; check both dev-profile overrides first when a debug viewer feels slow.

## Verify

```
cargo test -p ludion-server --test delivery                          # event/frame delivery over a raw socket
cargo test -p ludion-client --test streams                           # the same, through the typed Client
cargo test -p ludion-server --test phase4 -- --nocapture              # debug-floor fps + printed achieved rate
cargo test -p ludion-server --test phase4 --release -- --nocapture    # the real >=30fps criterion
```

TS-side tests: `ludion devdocs frame-streaming-viewer`.

## Gotchas

- Three formats reach the wire, chosen by the subscriber: QOI (the wire default), LZ4_RGBA, and
  RAW_RGBA (uncompressed `width * height * 4` — no codec at either end). First-party Rust clients
  pick by *locality*: `ludion-window`'s `default_stream_format` asks for RAW_RGBA on a loopback URL,
  where payload size is free and codec time is not, and QOI otherwise — measurement demoted LZ4 to
  an opt-in, since on engine content QOI produces both the smaller payload and the cheaper encode.
  The TS viewer decodes all three today (`editor/app/src/protocol/decode.ts`'s `canDecodeFormat`),
  but that parity is not guaranteed to hold — a format added to the wire lands in the server first;
  check `decode.ts` before assuming a new one reaches the browser.
- A `frames` subscriber attached to a *paused* engine still gets exactly one fresh render per
  mutating pass (the dirty flag fires on every drained command, not just mutations) — it does not
  free-run frames just because a viewer is attached; nothing renders again until the next command
  drains or a tick advances the frame index.
- The per-subscriber `FrameHeader` is only built for a frame actually sent — a throttled or
  superseded frame in the forwarder's `watch` slot costs no header work, only the one shared
  per-format encode already paid for by the router.
- Events and frames share nothing at the coalescing level: an event dropped under backpressure is
  gone (visible only as a `seq` gap); a frame "dropped" under latest-wins was never lost so much as
  superseded — there is no `seq` gap to look for on the frame side, only a `frame_index` gap.
