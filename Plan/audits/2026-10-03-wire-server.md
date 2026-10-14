# Audit — wire server, protocol and command dispatch (2026-10-03)

**Scope.** `crates/ludion-protocol` (envelope, `Command`/`METHODS`, schema document + hash, error
proxy, LFR1 frame header), `crates/ludion-server` (`lib.rs`, `connection.rs`, `delivery.rs`,
`perf.rs`), `crates/ludion-engine/src/dispatch.rs` plus the handlers it routes to under
`commands/` and the queue half in `live.rs`, `crates/ludion-client`, `crates/ludion-mcp`, the
editor's two wire clients (`editor/app/src-tauri/src/wire.rs`, `editor/app/src/protocol/client.ts`)
and `ludion-window`'s stream consumer, against `engine-architecture.md` §3/§6 and
`dev-docs/ludion-server.md`. Rev `c5d19986` (git mirror). Static reading only; no test was run. The
pinned dependency behaviour cited below (tungstenite 0.24 limits and single-frame writes) was read
from the crate source at the version `Cargo.lock` pins.

## Overall assessment

The core is in good shape. The parity gate holds (38 one-line arms, no wildcard, `METHODS` locked
to the schema). Nothing reaches `World` except through the one queue. Path-taking commands refuse
traversal. Both prior findings are fixed and pinned by tests: A1 (backoff, `L3022`, failure limit)
and A2 (`recv_timeout` measured from acceptance, a shutdown grace). What the July audits missed sits
at the edges. The WebSocket library's default size limits are smaller than payloads the engine
itself permits, and an oversized message shows up as "connection lost". Graceful teardown still
joins a wedged sim thread with no bound, and Ctrl-C is honoured only once. A `profile.capture` on a
paused sim ends as a misleading "stalled" diagnosis. The agent-facing claim that event delivery is
"lossless" is stronger than the server delivers. There are no High findings: every issue needs a
non-default extent, a wedged or paused sim, or an unusual pipelining pattern.

## High

None.

## Medium

### W1 — Payloads the engine allows exceed the WebSocket library's default 16 MiB frame limit, and the failure reads as "connection lost" · Medium · [verified]

**Evidence.** Both ends use tungstenite 0.24 defaults: the server through
`accept_hdr_async(stream, check_origin)` (`crates/ludion-server/src/connection.rs:244`), the client
through `tungstenite::connect(url)` (`crates/ludion-client/src/lib.rs:125`). Neither sets a
`WebSocketConfig`; no `max_frame_size`/`max_message_size` appears anywhere in `crates/` or `editor/`.
Those defaults are `max_frame_size = 16 MiB` and `max_message_size = 64 MiB`. tungstenite writes
every `Message` as **one** final frame (`Frame::message(.., true)`), and its reader rejects any
frame whose length exceeds the limit with `Capacity(MessageTooLong)`. The engine allows larger
messages:
- a `RAW_RGBA` frame at 3840×2160 is 33.2 MB, and at the documented 8K ceiling
  (`MAX_WIDTH`×`MAX_HEIGHT`, `crates/ludion-engine/src/render_size.rs`) 132.7 MB. That is over even
  the 64 MiB message cap;
- `ludion-window` subscribes `RawRgba` by default on loopback
  (`crates/ludion-window/src/reconnect.rs:188`), so `ludion dev --window` with
  `LUDION_RENDER_SIZE=3840x2160` fails on the first frame. Anything above about 4.19 Mpx
  (16 MiB / 4) fails;
- a QOI frame's worst case is 5 B/px, so noisy 4K content can also exceed the limit;
- the JSON replies to `render.screenshot` (base64 PNG, extent bounded only by the 8K ceiling) and
  `engine.snapshot`, and large `query.entities` results, are single text frames. `ludion dev`'s
  `engine.restore` sends a whole snapshot as one request frame, which the *server* rejects past
  16 MiB. `Plan/physics.md` measures about 5.5 MB of snapshot base64 per 2 000 bodies alone.

**Why it matters.** `transport_lost` (`crates/ludion-client/src/lib.rs:811`) maps the capacity
error to `L3009` "the connection to the server was lost". The window reconnect-loops on the same
oversized frame. A hot reload whose snapshot crosses 16 MiB fails as if the child had died. The MCP
adapter treats `L3009` as a dropped socket, reconnects, and asks again (see W10).

**Failure scenario.** `LUDION_RENDER_SIZE=3840x2160 ludion dev --window`: the viewer connects,
subscribes raw, reads a 33 MB frame, gets `L3009`, reconnects and repeats until its reconnect
budget is spent.

**Suggested fix.** Set an explicit `WebSocketConfig` on both ends, sized from the engine's own
ceilings (8K raw plus header for frames, a stated cap for text). Or have the writer fragment large
messages. Map `Capacity` to its own `L3xxx` code that names the size and the limit instead of
`L3009`. Add a test that streams one frame above 16 MiB.

### W2 — Teardown joins a wedged sim thread with no bound, and the process honours Ctrl-C only once · Medium · [verified]

**Evidence.** `Server::teardown` (`crates/ludion-server/src/lib.rs:352`) bounds the connection
drain and the runtime shutdown by `SHUTDOWN_GRACE`. It then calls
`self.sim_thread.take().and_then(|handle| handle.join().err())` (`:381`) with no bound. `run_live`
observes the shutdown signal only between commands (`crates/ludion-engine/src/live.rs`, step 2), so
a sim thread inside a long command never returns. `sim.step`'s `ticks` is an unbounded `u64`
(`crates/ludion-protocol/src/command.rs:710`), so one wire request can keep it busy indefinitely.
The headless host waits once on `recv(ctrl_c_stop())` (`crates/ludion/src/host.rs:138`) and then
runs `embedded.stop()`. The `ctrlc` handler (`:225`) `try_send`s into a capacity-1 channel whose
receiver has already been consumed, so a second Ctrl-C does nothing.

**Why it matters.** This is what remains of A2. The reply wait is now bounded, and a client gets
`L3023` after 120 s, but the *host* still cannot be stopped from its own terminal: the first Ctrl-C
starts a teardown that never finishes, and every later one is swallowed. The supervising hosts
(`ludion dev`, the editor's stage session) hard-kill after a timeout. A human running a game binary
or `ludion serve` does not have that fallback. The trigger is the engine's typical failure, an
agent-written system that loops forever, or a typo such as `ticks: 60000000`.

**Failure scenario.** An agent sends `sim.step {ticks: 100000000}`. `L3023` arrives two minutes
later. The user presses Ctrl-C, the host prints nothing and keeps running, and further Ctrl-C
presses are ignored until the process is killed from outside.

**Suggested fix.** Join with a deadline: park on a channel the sim thread signals when it exits, and
after the grace print an `L4xxx` that names the stuck command and exit non-zero, leaving the thread
detached. Make a second Ctrl-C exit immediately. Optionally cap `sim.step` ticks per request, with a
diagnostic that says to split the step.

### W3 — `profile.capture` on a paused or slowed sim is answered after 120 s as a "stalled sim", with the wrong remedy · Medium · [verified]

**Evidence.** The capture's frames are counted only by `advance_live`/`step_ticks`
(`crates/ludion-engine/src/engine.rs`). `run_live` does not tick while `Paused`, so the deferred
reply (`PendingCapture`, `live.rs`) is never produced unless someone else steps the sim.
`MAX_CAPTURE_FRAMES = 3_600` (`crates/ludion-protocol/src/perf.rs:55`) is justified as "a minute
of frames at the default tick rate", but `sim.set_speed` accepts down to 1/64, which turns that
window into 64 minutes. In both cases the server answers `L3023` at `SIM_REPLY_TIMEOUT`
(`connection.rs:600`, 120 s) with the help text "a game system is looping without terminating…
attach a debugger to the `ludion-sim` thread". The capture also stays in flight, so every further
`profile.capture` is refused with `L4013` until ticks resume.

**Why it matters.** `dev-docs/ludion-server.md` recommends running paused with explicit
`sim.step`. A synchronous client cannot pipeline the step that would close the window: that covers
`ludion-client`, the MCP adapter, and `ludion profile`. An agent on a paused session therefore
waits two minutes and is then told to debug a hang that does not exist.

**Failure scenario.** In MCP, `sim_pause` then `profile_capture {frames: 60}`. The call blocks for
120 s and returns `L3023` naming a runaway system. A retried capture returns `L4013` "already in
flight".

**Suggested fix.** Refuse a capture up front while paused (or when the window cannot close within
the reply budget at the current speed), with a diagnostic that says "resume, or pipeline a
`sim.step`". Derive the `MAX_CAPTURE_FRAMES` check from the current speed rather than from 60 Hz.

### W4 — Event delivery is advertised as lossless, but the server drops events in two places · Medium · [verified]

**Evidence.** The agent-facing `L3012` help (`crates/ludion-engine/src/commands/stream.rs:40`) and
that module's docs (`:11`, `:28`) say event delivery under `latest_wins` "is already lossless and
ordered (a gapless seq stream on log / sim.events / …), which is what verifying gameplay needs".
The server drops events in two places:
1. `route_event` does `outbound.text.try_send(text)` (`crates/ludion-server/src/delivery.rs:276`)
   and drops on a full text lane (256). `dev-docs/ludion-server.md` documents this as a visible
   gap, which contradicts the `L3012` text.
2. A subscription is registered only when its *reply* passes through `apply_observation`
   (`connection.rs:531`), and the pump answers strictly FIFO (`:369`). The sim thread opens the
   subscription and starts addressing events to it as soon as it dispatches the command, and
   `route_event` silently skips a sub missing from the `Registry`. If the subscribe sits behind an
   unanswered request on the same connection, every event in that window is lost. The obvious case
   is a `profile.capture`, whose reply is deferred for the whole window; a full text lane does the
   same. The "benign registration race" note in `delivery.rs`/dev-docs argues only about *frames*.
   It also stops being benign for frames on a paused session: the one owed first frame
   (`may_change_frame` for `StreamSubscribe`) is the only frame that will come.

**Why it matters.** An agent that subscribes to `sim.events` to verify gameplay is told it will see
every event. A gap at the start of a stream is invisible unless the client knows the first `seq`.

**Failure scenario.** One connection pipelines `profile.capture {frames: 600}` and then
`stream.subscribe {channel: "sim.events"}`. The sim opens the subscription immediately, but the
server registers it about 10 s later, so 10 s of events are dropped with no error.

**Suggested fix.** Register a subscription when its reply reaches the pump, *out of FIFO order*.
The pump can hold the `Observed::Subscribe` aside and register as soon as that entry's
`reply_rx` resolves, while still *sending* replies in order. Alternatively, buffer events for a
sub id the sim has opened but the server has not yet registered. Reword the `L3012` help to
"ordered, with visible `seq` gaps if the client stops reading".

## Low

### W5 — No timeout on the WebSocket upgrade or the `Hello`, so idle sockets pin tasks and slow every teardown · Low · [verified]

`accept_loop` takes the `ConnectionGuard` at accept time. `accept_hdr_async` (`connection.rs:244`)
has no timeout and does not watch shutdown, so a TCP client that never sends the HTTP upgrade holds
its task and guard for as long as it stays connected. The `Hello` read (`:257`) does watch
shutdown but has no deadline. Each such socket makes `Server::teardown` wait the full 5 s
`SHUTDOWN_GRACE` before the runtime is torn down (`lib.rs:352`). **Fix:** wrap the upgrade and the
first `read_text` in a short `tokio::time::timeout` and select the upgrade against the shutdown
watch.

### W6 — A tiny positive `max_fps` panics the frame forwarder, which silently kills the stream · Low · [verified]

`frame_forwarder` computes `Duration::from_secs_f32(1.0 / fps)` whenever `fps > 0.0`
(`delivery.rs:409`). That call panics on overflow, which `max_fps: 1e-30` (valid JSON, valid
`f32`) triggers. The panic ends the spawned tokio task. The sub stays registered, the reply was
`Ok`, and the engine keeps rendering and encoding frames that nobody forwards. Negative values are
also quietly treated as "no cap". **Fix:** validate `max_fps` in `stream::subscribe` (finite and
inside a stated range, else `L30xx` with path `/params/mode/max_fps`), and use `try_from_secs_f32`
on the server side.

### W7 — The published error schema marks `related` as required, but the wire omits it in almost every error · Low · [verified]

`WireErrorSchema.related: Vec<WireErrorSchema>` (`crates/ludion-protocol/src/lib.rs:119`) has no
`default`, so the snapshot lists `"required": ["code", "message", "related"]`. The real
`Diagnostic` serializes `related` with `skip_serializing_if = "Vec::is_empty"`
(`crates/ludion-core/src/diagnostics.rs:215`). A schema-validating client would reject ordinary
errors. The generated TS type declares `related` non-optional, so `ProtocolError.related`
(`editor/app/src/protocol/client.ts:56`) is `undefined` while typed as an array. The existing parity
test compares property *names* only. **Fix:** add `#[serde(default)]` to the proxy's `related`
(and any other skipped field); this moves the schema hash. Extend the proxy test to compare
`required` against an error with empty `related`.

### W8 — Envelope errors misattribute a bad `id` to `/params` and truncate `v` · Low · [verified]

`Request::from_value` checks `v` and `method` by hand and then deserializes the whole struct. A
missing or non-`u64` `id` therefore surfaces as "`params` do not match method `…`: missing field
`id`" with path `/params` (`crates/ludion-protocol/src/envelope.rs:171`). `unsupported_version(got
as u32)` (`:152`) reports `v: 4294967297` as "declared version 1" and a non-numeric `v` as
4294967295. **Fix:** validate `id` explicitly (path `/id`) before the typed parse, and report `v`
as the raw JSON value.

### W9 — The editor's TS client never checks `schema_hash` against its own generated types · Low · [verified]

`ProtocolClient.connect` validates only `protocol_version`. `App.tsx` stores the *first* server's
hash as the expected value (`editor/app/src/App.tsx:1034`) and only notices a change across
reconnects. `@ludion/protocol-types` embeds no hash to compare. `engine-architecture.md` §9 risk 4
says the handshake "catches stale clients at runtime", and `ludion-client` does (`L3003`), but the
editor, the main human client, does not. That matters now that the editor attaches to separately
built games (the editor-run game slot, an external game on a git dependency). **Fix:** have
`editor/protocol/scripts/generate.js` emit the schema hash next to the types, and refuse or warn at
connect on a mismatch.

### W10 — The MCP adapter re-sends any command that failed with `L3009`, which also covers "sent, reply lost" and "reply too large" · Low · [verified]

`Session::call` reconnects and retries on `code == PROTOCOL_ENGINE_UNAVAILABLE`
(`crates/ludion-mcp/src/session.rs:46`). The client raises the same code for "could not connect",
"socket dropped after the request was sent", a server-side `L3009` answer, and W1's capacity error.
A mutating command (`sim.step`, `entity.spawn`, `prefab.apply`) whose reply is lost after dispatch
is applied twice. An oversized reply is requested twice and then reported as a lost connection. The
window is narrow today, but nothing guards it. **Fix:** retry only failures that provably happened
before the request was written, such as a connect error or a failed `send_text`. Return everything
else to the model.

### W11 — Disconnect cleanup waits for every outstanding reply first, so a gone viewer keeps being rendered · Low · [verified]

When the reader exits, `reply_pump` keeps awaiting each pending entry (`connection.rs:369`) and runs
`finish`, which unregisters subs and queues their `stream.unsubscribe`, only after the first reply
fails to send. With a `profile.capture` pending that can take the whole capture window, or 120 s on
a paused sim (W3). Until then the engine keeps rendering and encoding frames for a subscriber whose
socket is closed. **Fix:** on reader exit, unregister and unsubscribe at once, and let the pump drain
the leftover replies without holding the subs.

### W12 — A capture longer than 600 frames reports transport timing for only its last 600 frames, without saying so · Low · [verified]

`RING_CAPACITY = 600` (`crates/ludion-server/src/perf.rs:53`), while a capture may measure up to
3 600 frames. `profile_after` builds the medians from whatever is still in the ring, so the
transport medians describe a different span from the sim/GPU sections of the same report. With
columns, `profile_for_frames` returns `None` for every evicted frame, which the wire type defines as
"not delivered". **Fix:** size the ring from `MAX_CAPTURE_FRAMES`, or record the covered span in
`TransportProfile` and keep evicted frames distinct from undelivered ones.

### W13 — Stale documentation of the server and the stream protocol · Low · [verified]

- `dev-docs/ludion-server.md:223` still says "A transient accept error is silently skipped (a
  log-free continue) … don't expect a log line". Since the A1 fix the server logs `L3022`, backs off,
  and stops after 64 failures.
- `dev-docs/ludion-server.md:113` describes one `bounded::<Egress>(1)` lane that the pump "splits".
  There are two lanes (`lib.rs:182`). The teardown paragraph says no join can hang and does not
  mention the unbounded sim join (W2).
- `engine-architecture.md` §6 lists frame formats `QOI | LZ4_RGBA` (no `RAW_RGBA`) and channels
  without `profile`, and presents `acked` as a working mode. §3 says the readback bytes go to "a
  tokio blocking task for QOI/LZ4 encode + WS send".
- `SubscribeMode::Acked` is documented in the schema as "Lossless, acknowledged delivery"
  (`crates/ludion-protocol/src/command.rs:1168-1182`). That text reaches the MCP tool schema and the
  TS types, but the mode is always refused with `L3012`. Either state the refusal in the doc or drop
  the variant at the next schema move.

## Checked and healthy

- **A1 holds:** `accept_loop_with` backs off 100 ms, logs `L3022` once per error kind, and ends the
  host after 64 consecutive failures through the ordinary `engine.shutdown` path. It is pinned by
  `a_listener_that_always_fails_is_bounded_and_stops_the_host` and
  `shutdown_during_the_accept_backoff_ends_the_loop`.
- **A2 holds for the reply wait:** `recv_timeout` runs from `enqueued_at`, a shutdown shortens the
  wait to `SHUTDOWN_REPLY_GRACE` rather than cancelling it, and one blocking-pool thread is used per
  connection at a time. Four `await_reply` tests cover it. (The host-side residue is W2.)
- **Parity gate:** `dispatch.rs` has 38 one-line arms and no wildcard. `METHODS` has 38 entries,
  matching the schema. The MCP catalog is derived from the schema (`tools.rs`) and excludes exactly
  `stream.subscribe`/`unsubscribe`, with a stated reason. The editor host's `wire.rs` sends only
  `asset.reload` and `engine.shutdown`, and its protocol version is pinned by a source-reading test.
  No editor-private endpoint was found.
- **One queue, no shared world:** there is no `Mutex<World>`/`Mutex<Engine>` anywhere in `crates/`.
  Disconnect cleanup and the asset watcher go through the intake queue (`CommandSink`).
- **Trust boundary:** `scene.load`/`save`/`check` and the prefab commands resolve through
  `ludion_core::paths::relative_refusal` (`L3040`), which refuses empty, absolute, prefixed (`:`),
  NUL and dot-climbing paths. `render.screenshot`/`render.size` extents are bounded. Handler panics
  are caught per command (`catch_unwind` → `L4016`). The `Origin` check refuses foreign browser pages
  (which also blocks DNS rebinding). The default bind is loopback.
- **Backpressure:** at most 64 accepted-but-unanswered requests per connection. Text lane 256, frame
  lane 4, writer bias bounded by `TEXT_BURST`, and the closed-lane spin is avoided. Frames are encoded
  once per format on the blocking pool. The registry and profile-hub mutexes are never held across an
  `.await`. The `ProfileHub` rings are capped.
- **Client:** the timed call restores the socket's previous read timeout, treats both
  `WouldBlock` and `TimedOut` as a timeout, and skips stale replies by id. The prior "no call
  timeout" item is addressed (opt-in). The unused `Request::check_version` from the July audit is
  gone.
- **Schema:** the document is canonicalized (keys sorted recursively). The snapshot test pins both
  the bytes and the hash `d5f769fa052ee453`. The `Diagnostic` proxy is name-parity-tested (but see
  W7 for `required`).

Aside, outside this audit's scope: an untracked 250 KB `crates/ludion-engine/src/engine.rs.orig`
sits in the working tree. It is not compiled, but it looks like a merge leftover.

## Suggested order of work

1. **W1:** an explicit `WebSocketConfig` on both ends plus a distinct capacity diagnostic. Small,
   and it unblocks 4K viewing and large snapshots.
2. **W3:** refuse a capture that cannot close (paused, or too slow for the budget). A few lines in
   `commands/perf.rs`.
3. **W2:** a deadline on the sim join plus a second-Ctrl-C hard exit. Small and contained in
   `lib.rs`/`host.rs`.
4. **W4:** register subscriptions outside FIFO order and correct the `L3012` wording. Needs a small
   design pass in `reply_pump`.
5. **W6, W5, W11:** small hardening in `delivery.rs`/`connection.rs`; batch them.
6. **W10:** narrow the MCP retry condition.
7. **W7 + W8:** one schema/envelope pass. W7 moves the schema hash, so re-record the replays after
   the snapshot, as the house rule requires.
8. **W9, W12, W13:** generated hash for the TS client, ring sizing, doc refresh.
