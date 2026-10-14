---
description: ludion-server delivery: egress lanes, routers, writer lanes, subscribe races, disconnect
---

# server-delivery

**Scope:** the receiving half of the engine's egress seam inside `ludion-server` — pump, routers,
forwarders, the per-connection outbound lanes and the subscription `Registry`. The host, handshake and
request pipeline are `ludion devdocs ludion-server`; the end-to-end tick-to-pixels map is
`ludion devdocs frame-streaming`; the engine's egress half is `ludion devdocs ludion-engine`.

## Rules

- **The frame egress lane is bounded(1), latest-wins-at-source; events ride a separate unbounded,
  ordered lane.** `Server::bind` wires both up on purpose. The frame lane's exact bound is what the
  engine's own render-skip logic (in `run_live`) depends on (see `ludion-engine`'s `egress` module
  doc): it never renders a frame while the previous is undrained — an unbounded frame lane would
  silently defeat that. Events get their own lane so a burst is never dropped behind a pending frame
  (audit finding 1.3): they leave the sim thread strictly ordered and unbounded. The pump's own handoff
  into tokio keeps the same discipline: latest-wins for frames (a watch slot the pump *replaces* — a
  frame the router hasn't picked up is superseded, never queued), strictly ordered and never coalesced
  for events (an unbounded mpsc). Buffering raw frames at the frame seam would rebuild the very backlog
  bounded(1) exists to prevent.
- **Frame encoding happens once per distinct subscribed format** (at most three; two same-format
  subscribers share one encode and one `Arc`), in `frame_router`'s `route_frame`, on the blocking
  pool — never per subscriber, never on a runtime worker. Awaiting the encodes on a worker would
  starve every forwarder and connection task behind a back-to-back encode loop.
- **The `Registry` mutex is held only for a map lookup/insert (or a buffer flush of non-blocking
  sends), never across an `.await`.** A lock held across an await would let one slow subscriber stall
  every other connection's registration.
- **A subscribe is resolved by its own task, and the registry buffers what races it.** The sim answers
  a subscribe at its command drain and emits for the new sub later in the same pass, while the reply
  (blocking pool) and the egress (pump thread → router) take different paths to the server — so a
  sub registered when the FIFO pump reached its reply would lose its first events, and a paused
  session's one first frame. `spawn_resolution` awaits the reply on its own task and `register`s at
  once, never by a server-private write ahead of the queue. Even so nothing orders reply against
  egress, so the registry counts subscribes **in flight** (`delivery::subscribe_in_flight`, taken by
  the reader *before* `intake.send`, consumed by `register`, released by drop on every other exit);
  while the count is above zero, an event for an unknown sub is buffered (per sub, capped at
  `TEXT_CAPACITY`) and the latest frame is stashed, and `register` delivers both in the same lock
  that inserts the sub. At zero both are cleared. A client can therefore receive a sub's first events
  (or first frame) *before* its `stream.subscribe` reply, when the subscribe is pipelined behind a
  held request; clients must buffer by sub id (`ludion-client` already queues events seen while
  waiting for a reply). Each in-flight subscribe holds one blocking-pool
  thread of its own besides the pump's, bounded by `PIPELINE_DEPTH`.

## Dataflow

**Message ceiling.** Connections are accepted with `websocket_config()` — `max_message_size` and
`max_frame_size` both `ludion_protocol::MAX_MESSAGE_BYTES` (256 MiB), not tungstenite's 16 MiB
default. A reply whose serialized text is over it becomes an `L3045` error with the same id
(`reply_text`), keeping the connection and the reply order; an over-limit *request* is logged as
`L3045` by `read_text` and ends the connection. An *event* over the ceiling is replaced, in its
slot and with its `sub`/`seq`, by a `log` event carrying `L3045` (`event_text`), so the drop is
visible. A test pins the ceiling above the 8K worst-case frame
and screenshot sizes computed from `MAX_RENDER_WIDTH`/`HEIGHT`.

**Two outbound lanes, one writer.** A connection owes the socket on *two* independent queues, both
drained by its single `writer_task`: text (replies and JSON events, `TEXT_CAPACITY` 4 ×
`PIPELINE_DEPTH`, 256) and binary (LFR1 frame messages, `FRAME_CAPACITY` 4). The writer is `biased`
on text, so a reply overtakes the frames already queued instead of waiting out their megabytes —
under one shared FIFO an `input.inject` reply could sit behind a full queue of multi-MB frames. The
frame lane is shallow because each forwarder already coalesces upstream in its latest-wins slot, so
depth here would buy staleness, not throughput; a full frame lane backpressures the forwarder, which
is where coalescing is free. Order *within* a lane is untouched: replies stay strictly FIFO, events
keep `seq` order.

**Delivery, end to end** is `frame-streaming`'s topic; this crate owns the receiving half.
`event_router` serializes each event to JSON and sends it to its one owning connection
(`route_event`); `frame_router` takes the freshest pending frame, encodes it once per distinct
subscribed `FrameFormat`, and pushes each subscriber its own format's shared payload through a
per-subscriber `watch` slot (`route_frame`); one `frame_forwarder` per open subscription applies
`max_fps`, prepends its own `FrameHeader`, and queues the binary message. A router slower than the
render rate degrades to lower delivery fps with ~one frame of staleness at the pump seam, never a
growing backlog of raw frames. **Events have no throttle or coalescing**: they are ordered, and a
full text lane (or a vanished receiver) drops the message (the lane is sized so that only a client
that stopped reading fills it), but the engine already advanced that subscription's `seq` before
handing it off, so the drop is a visible gap on the wire, never a silent loss. `max_fps` is
validated by the engine (`L3046`, `[0.1, 1000]`) and `frame_forwarder` still never panics on one
that gets through: a value no `Duration` can hold means no cap.

**Disconnect cleanup** rides the same queue as everything else: the moment the reader exits —
before the pump has drained the replies still held ahead of the subs — `connection::release_subs`
unregisters every sub the connection owns from the `Registry` *and* submits a fire-and-forget
`stream.unsubscribe` `StampedCommand` for each through the shared intake channel (the reply receiver is
dropped immediately — the sim thread's non-blocking reply send tolerates that), so the engine's own
subscription table is freed too. It also marks the connection closed: a subscribe still resolving then
cancels itself with its own unsubscribe instead of registering. A subscribe answered *after* its
`L3023` is unsubscribed by a detached reaper (`reap_late_subscribe`): the client was told it failed
and will never close the sub the sim still minted. The pump only drains and closes the
lanes (`finish`). There is no server-private teardown path into the engine.

## Gotchas

- **Registration races the engine, and the in-flight buffer closes it.** Egress addressed to a `sub`
  the engine has opened but the server has not registered would find no `Registry` entry; that was
  once called benign, but it loses a subscription's first events and a paused session's only frame.
  A new path that reads the registry for egress must treat "unknown sub while a subscribe is in
  flight" as "wait", not "drop" (see `delivery.rs`'s module doc).
- **The writer's text bias is two `done` flags plus a non-blocking frame turn — never a `select!`
  guard.** A closed `mpsc` returns `None` immediately and forever, so a `biased` loop over a closed
  lane spins a core; the flags take that arm out instead, and both flags set is the loop's only exit.
  Bounding the bias by *guarding* the text arm is the other trap: with the frame lane idle it leaves
  the writer waiting on a lane with nothing to say (or with every branch disabled, which panics)
  while replies queue behind it. `TEXT_BURST` is cashed in as a `try_recv` turn for the frame lane.
- **The egress pump must stay cheap.** It exists purely so the crossbeam recv/tokio handoff never
  stalls; any work added there (encoding, locking, anything `.await`-shaped) costs real frames because
  the engine's bounded(1) frame lane silently drops a frame while it's full. The allowed work per message is
  exactly: a `watch` `send_replace` for a frame, an unbounded mpsc send for an
  event — both non-blocking.
- All three `FrameFormat`s have encoders behind them server-side: QOI (the wire default), LZ4_RGBA,
  and RAW_RGBA (uncompressed — the "encode" is one payload copy). A frame is encoded once per
  distinct format among its subscribers, so a mixed-format audience costs one encode per format,
  never one per subscriber. The TS viewer is the lagging half — check `editor/app/src/protocol/`
  before assuming a format reaches the browser.
