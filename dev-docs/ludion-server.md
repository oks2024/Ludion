---
description: WebSocket host — one command queue, one sim thread, plus the frame/event delivery path
---

# ludion-server

**Scope:** The `ludion-server` crate: the `Server` WebSocket host, the handshake, the per-connection
request→queue→reply shuttle over the single command queue, and the *delivery* path that turns the
engine's egress messages into wire traffic for `frames` and event subscribers. The queue's engine half (`StampedCommand`, `run_live`, the sim-thread loop) and
the *outbound* half (`Egress`, `EgressFrame`, `EgressEvent`) live in `ludion-engine`'s `live`/`egress`
modules and are covered in `ludion-engine`; the envelope, handshake, `Channel`/`SubscribeMode`, and
the binary `frame` (LFR1) wire format are `ludion-protocol`; the typed sync SDK that consumes streams
from this host is `ludion-client`; the full tick-to-pixels story that ties this crate to the TS
viewer is `frame-streaming`. Siblings: `crate-graph` (why the dependency points server→engine and
never back), `diagnostics` (the `L3xxx` codes this crate speaks).

## Layout

- `crates/ludion-server/src/lib.rs` — `Server`: `bind`, `local_addr`, `url`, `shutdown` (+ `Drop`
  running the same teardown); spawns the sim thread, a 2-worker tokio runtime, the egress pump
  thread, and the router task; the `L3009` startup-error helper.
- `crates/ludion-server/src/connection.rs` — `accept_loop` (one task per connection),
  `handle_connection` (handshake, then a read loop that never waits on a reply), `reply_pump` (the
  per-connection FIFO that answers accepted requests in request order and unregisters as an
  unsubscribe reply passes), `spawn_resolution` (a subscribe's own task: registers the sub the
  moment the sim answers), `release_subs` (disconnect cleanup), `writer_task` (sole
  owner of the WebSocket sink, drains the two outbound lanes text-first),
  `accept_request` + `Pending`/`PIPELINE_DEPTH`, `parse_request`/`parse_hello`, `read_text`.
- `crates/ludion-server/src/perf.rs` — the transport timing layer: `ProfileHub` (capped encode/send
  rings behind an interest count — one relaxed atomic load when disarmed), `ProfileArm` (RAII
  arming through the connection's own state, so disconnect disarms for free), and the server-side
  injection that fills a profile window's or capture reply's `transport` section before
  serialization. Nothing here is `async`, which is what makes "the mutex is never held across an
  `.await`" checkable by reading. See the `profiling` topic.
- `crates/ludion-server/src/delivery.rs` — the receiving half of the engine's egress seam:
  `run_egress_pump` (a std thread bridging the crossbeam egress slot into tokio: events onto an
  ordered mpsc, frames into a latest-wins watch slot), `event_router` (delivers each JSON event in
  order), `frame_router` (encodes the freshest pending frame once per distinct subscribed
  `FrameFormat`, fans each format's payload out to the subscribers that asked for it),
  `frame_forwarder` (one task per open `frames` subscription: latest-wins + `max_fps`
  throttle), the `Registry` (the open subs plus the in-flight buffer) and `register`/`unregister`.
  Read its module doc first.
- `crates/ludion-server/tests/` — `wire.rs` (handshake, wire-vs-in-process `WorldHash` parity, shared queue,
  structured errors, null-id convention), `crates/ludion-server/tests/pipelining.rs` (reply-`frame`-stamp spread stays within about a pass), `crates/ludion-server/tests/delivery.rs` (event order/`seq`, LFR1 header, lane
  priority, disconnect cleanup over a raw socket), `phase4.rs` (≥30 fps 1080p delivery, profile-gated — read
  its module doc before touching the assertion; recorded viewer session replays to identical `WorldHash`es),
  `crates/ludion-server/tests/exit_criteria.rs` (byte-identical wire screenshot, SSIM over the wire, wire-recorded replay).
- `crates/ludion-server/clippy.toml` — intentionally permissive: this is a transport, not on the
  deterministic sim path; determinism lives on the other side of the queue.

## Rules

- **The sim thread solely owns `World` — never `Arc<Mutex<World>>`.** `Server::bind` moves the
  `Engine` onto a dedicated thread (named `ludion-sim`) running `ludion_engine::live::run_live`;
  every read and write — queries, `sim.pause`/`sim.step`, mutations — flows through the one
  crossbeam intake channel and applies at a frame boundary. There is no other path.
- **No server-private way to touch the world.** Every request becomes a `StampedCommand` and goes
  through `ludion_engine::dispatch`, exactly as an in-process caller's would. `stream.subscribe`/
  `unsubscribe` are no exception: this crate never writes to the engine's subscription table
  directly, only through the queue like everything else.
- **The dependency points server→engine, never back.** `ludion-engine` is tokio-free; the seams it
  exposes (`live`'s two `crossbeam_channel`s for commands, `egress`'s one more for frames/events) are
  what let this crate plug in. Nothing in the engine tree may name this crate.
- **`watch_assets` is `ludion_assets::watch`**, so this crate enables that crate's opt-in `watch`
  feature (`notify`) — with `ludion-cli`'s `dev`, the only two that do. A facade built without
  `wire` has no server and therefore no OS file watcher in its tree.
- **tokio is confined to this crate**: a 2-worker runtime running the accept loop, one task per
  connection, the event and frame routers, and one `frame_forwarder` task per open `frames`
  subscription.
- **The sim thread's reply send never blocks**: each `StampedCommand` carries its own capacity-1
  reply channel, and a dropped receiver is ignored — a slow or vanished client can never stall the
  sim loop. One command, one reply, one receiver: race-free by construction.
- Egress lane discipline, per-format encode, the registry lock and the subscribe in-flight buffer are
  `ludion devdocs server-delivery`.
- **One reader per connection, one FIFO pump behind it.** The reader is the sole consumer of the
  socket's read half, so command order into the one intake queue *is* socket order; the pump is the
  sole answerer, so reply order *is* request order — a request refused before dispatch rides the
  queue as a ready-made `Response` rather than overtaking its neighbours. A second reader, or
  answering entries concurrently, breaks a guarantee every client has. The pump awaits one entry at a
  time; a `stream.subscribe` is the one request resolved elsewhere (next bullet), and its slot just
  takes the finished `Response`, so replies still *leave* in request order.
- **Hostile-peer bounds, all in `connection.rs`.** `AcceptPolicy::max_connections` (64) refuses the
  next client's handshake with `L3055`; each pump holds at most one blocking-pool thread, so the
  pool cannot be exhausted. `request_budget` is a per-connection byte semaphore taken *before*
  `accept_request` and held by the queued `Accepted` until its reply is queued; it defaults to
  `MAX_MESSAGE_BYTES` so one maximum request (a restorable snapshot) always fits — it is not a lower
  message ceiling. `arm_keepalive` sets kernel TCP keepalive (15 s idle, 5 s interval) on every
  socket: an idle half-open peer ends in about a minute. **Not WebSocket pings** — `ludion-client` is
  synchronous and answers a ping only while reading, so an idle adapter would look dead. A peer with
  unacknowledged frames still ends at the OS retransmit timeout, and there is no writer send timeout
  (a client paused while reading is legal under latest-wins).
- **A subscribe's wait takes no pool thread.** `await_reply` takes a `ReplyWait`: the pump's
  `Blocking` parks one pool thread; a subscribe's resolution uses `Polled` (5 ms async polling), and
  a late subscribe is reaped by an async task (`reap_late_subscribe`, 250 ms poll).
- **`ludion-client` is the peer, not a dependency.** It appears only in dev-dependencies, so the
  integration tests drive the server exactly the way the editor or CLI will — over the wire.
- Startup failures (bad address, runtime, bind) and a stopped sim thread are all `L3009`
  (`PROTOCOL_ENGINE_UNAVAILABLE`), structured with a help line; a connection over the cap is `L3055`.

## How it works

`Server::bind(engine, addr)` spawns the sim thread, the tokio runtime and the egress pump (lane wiring:
`ludion devdocs server-delivery`). It binds *synchronously* — port `0` (the pattern every test uses) is read back via
`local_addr()`/`url()`. Ceiling `L3045`, outbound lanes: `server-delivery`.

Per connection: the first message must be a `Hello`; the server answers with a `ServerHello` (engine
version, protocol version, schema hash — drift dies at the handshake). After that the connection runs
as a **reader** and a **reply pump** joined by a bounded FIFO. The reader reads a text frame →
`accept_request` (`parse_request` → push a `StampedCommand` into the intake channel) → queue the
resulting `Pending` entry → read again; it never waits for a reply. The pump pops those entries
strictly FIFO, blocks for each reply on the blocking pool, and queues the `Response`. So **a client may pipeline**: the sim drains every pending command at each pass boundary (`run_live`), so reply age
is a function of *passes*, not of how many requests the client has already sent — the backlog a fast
injector builds cannot grow the way an unread receive buffer would. Every request is also
peeked (`observe`) for whether it is a `stream.subscribe`/`unsubscribe`; once a subscribe's *reply*
comes back, `open_subscription` (in its task) registers the sub in the shared `Registry`, reading the
minted `sub` id straight out of the reply body, never guessing it ahead of time; an unsubscribe's
reply is applied by the pump (`apply_unsubscribe`). An unsubscribe naming a sub the connection does
not own is refused `L3007` before dispatch (`accept_request`), so one client cannot close another's.

Teardown (`shutdown()` or `Drop`, same code): flip the connections watch, signal the sim thread, wind
the runtime down with a bounded grace period (`SHUTDOWN_GRACE`, 5 s), join the sim thread with
`live::join_sim` (at most `SIM_STOP_GRACE`, 5 s), then join the egress pump — by then the sim thread
has already dropped its egress sender, so the pump's blocking `recv` has already returned and this
join cannot hang. `shutdown()` returns `Some(L4017)` when the sim thread's `join()` was a panic (a
panic outside any command or system boundary), and `Some(L4022)` when the sim thread was still busy
after the grace (a `sim.step` that never ends, a system that loops): the message names what it was
doing — `run_live` keeps that in an atomic slot carried by the shutdown pair — the thread and the pump are left detached. The hosts exit
non-zero on either, `Drop` ignores them. A second Ctrl-C exits the process at once. `Embedded::stop` (`ludion devdocs ludion-embedding`) bounds its boot-thread join the same way: `join_sim` over `SIM_STOP_GRACE`, a stuck sim reported `L4022` with a failure exit; the wire host's listener join is unbounded because the server teardown above already is.

A new socket has one `HANDSHAKE_TIMEOUT` (10 s) for the WebSocket upgrade and the `Hello`; both are
selected against the shutdown watch, so a silent socket neither outlives the deadline nor holds
teardown for the drain grace.

## Verify

```
cargo test -p ludion-server                                          # every test file in this crate
cargo test -p ludion-server --test phase4 -- --nocapture             # debug-floor fps; add --release for the real ≥30fps criterion
```

GPU-dependent cases print `SKIPPED … no GPU adapter` (`L2003`) and pass without an adapter. Every test binds port 0 and relies on drop being a
clean teardown — a hang or leaked thread here is itself a test failure.

## Gotchas

- The live loop free-runs by wall clock when the engine is `Running` — a wire session that needs a
  deterministic applied frame must `sim.pause` **before** serving (or as its first command), then
  drive with explicit `sim.step`; see the pause-before-serve pattern in `exit_criteria.rs`.
  `profile.capture` is the exception: a capture needs a running sim (`sim.resume`, capture,
  `sim.pause`); a paused or stepped sim is refused `L4023`.
- To record a wire session, arm recording (`engine.set_recording`) *before* `Server::bind` — the
  engine has moved onto the sim thread by the time `bind` returns.
- The intake channel (commands, not egress) is unbounded; backpressure is per-client, from the
  connection's bounded request pipeline (`PIPELINE_DEPTH`, 64 accepted-but-unanswered requests). At
  the bound the reader stops reading and TCP holds the client, so no receive buffer grows without end. A request answered late still gets honest `L3023`
  semantics: `SIM_REPLY_TIMEOUT` is measured from when the reader *accepted* it, so sitting behind a
  marathon `sim.step` eats that budget.
- This crate has no direct `serde` dependency by design: call sites serialize concrete wire types
  with `serde_json::to_string` (the `Serialize` bound stays inside `serde_json`). Don't add one.
- An accept error is reported as `L3022` (once per error class), retried after a 100 ms backoff,
  and after 64 consecutive failures the loop queues `engine.shutdown` (a vanishing client
  mid-handshake is a connection error, not an accept error).
- The handshake error path answers with id `0` (a hello carries no request id), then closes — a
  raw client that sends a request first gets `L3001`, not a hang.
