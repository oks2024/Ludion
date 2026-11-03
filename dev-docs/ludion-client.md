---
description: Sync WebSocket SDK — schema-drift check at connect, plus buffered event/frame streams
---

# ludion-client

**Scope:** The `ludion-client` crate: the typed, synchronous WebSocket client for the Ludion
protocol — `Client::connect` (bounded handshake + drift check), `Client::call` (the blocking
primitive), the stream-consumption API (`subscribe`/`unsubscribe`/`next_event`/`next_frame`), and the typed
conveniences (`sim_hash`, `sim_step`, `screenshot`). This is a *peer* client: it speaks exactly the
protocol the editor and CLI must speak, and the parity gate means it gets no private endpoints. The
wire shapes themselves (envelope, `Command`, `ResponseBody`, `schema_hash`, the binary `frame` LFR1
format) live in `ludion-protocol`; the host side (and the delivery path that actually pushes streams)
is `ludion-server` — see `crate-graph` for the boundary picture, `diagnostics` for the `L3###`
protocol codes, and `frame-streaming` for the full tick-to-pixels path this crate is the Rust-side
consumer of.

## Layout

- `crates/ludion-client/src/lib.rs` — the whole crate, one file: `Client` (`connect`/`call`/
  `subscribe`/`unsubscribe`/`next_event`/`next_frame`/`sim_hash`/`sim_step`/`screenshot`/
  `schema_hash`/`server_hello`), `CallOk`, `Screenshot`, `ReceivedFrame`, and the private transport
  and buffering helpers (`send_text`, `read_message`, `classify_text`, `decode_frame`,
  `wait_for_event`/`wait_for_frame`, `set_read_timeout`/`restore_read_timeout`).
- `crates/ludion-client/tests/streams.rs` — the crate's own stream integration tests: event
  ordering/`seq` across an interleaved `call`, `next_event`'s prompt no-op timeout and read-timeout
  restore, and frame delivery decoding a QOI payload matching its LFR1 header (GPU graceful-skip).
- `crates/ludion-client/Cargo.toml` — dependencies: `ludion-core` (diagnostics), `ludion-protocol`
  (the contract), `tungstenite`, `serde_json`. Nothing else.
- `crates/ludion-client/clippy.toml` — deliberately empty: the *permissive* transport-side denylist
  (see Rules).
- `crates/ludion-server/tests/wire.rs`, `tests/exit_criteria.rs`, `tests/delivery.rs`,
  `tests/phase4.rs` — cross-crate surface: `ludion-server` lists this crate as a dev-dependency and
  drives it for real over a live server (`delivery.rs` also drives the same delivery path with a raw
  socket, for tests that need to peek at wire bytes the typed client would decode away).

## Rules

- **The client never depends on `ludion-engine` or `ludion-server`** (`engine-architecture.md`
  §2). It knows the wire shapes and the handshake; it does not know how the world is computed.
  The Cargo.toml dependency list is the gate.
- **Schema drift is a hard connect-time error, no lenient mode.** `connect` fails with `L3003`
  if the server's `schema_hash` differs from this build's `ludion_protocol::schema::schema_hash()`
  or its protocol version differs from `PROTOCOL_VERSION` — a stale client finds out immediately,
  not through a confusing failure three commands later.
- **Everything blocks.** Every method blocks the calling thread until its response (or, for
  `next_event`/`next_frame`, a timeout) arrives. There is no async API, no callback, no reconnect
  logic — a lost connection is an `L3009` and the caller reconnects with a fresh `Client`.
  `check_open()` is a non-blocking probe (buffering whatever is already waiting) that lets a caller
  decide a reconnect is safe *before* it sends; restores blocking mode on every path.
- **`call` is the only primitive, and it never discards subscription traffic.** The typed
  conveniences are thin wrappers that pattern-match the `ResponseBody`; anything they don't cover
  goes through `call(Command::…)` directly. A pushed `EventMessage` or binary `frame` message that
  arrives while `call` is reading for its own response is queued (`events`/`frames` internal
  `VecDeque`s), never dropped — so subscription traffic racing a call is picked up later by
  `next_event`/`next_frame`, not lost.
- **A timed-out subscribe unsubscribes itself** when its reply lands (`abandoned_subscribes`,
  `stray_reply`): the sub it minted is closed locally and `stream.unsubscribe` is sent without
  waiting. A closed sub's traffic is never handed back (`Inbox::closed`) — frames ride a lower-priority
  lane than replies, so one can land after the unsubscribe `Ack`.
- **`next_event`/`next_frame` drain their queue first**, and only then set a socket read timeout and
  block on the wire. The previous read timeout is always restored on the way out — success *or*
  error (`restore_read_timeout`) — so a `call` issued right after a timed-out `next_event`/
  `next_frame` behaves exactly as if neither had run.
- **A received frame's payload stays encoded exactly as the wire carried it** — `header.format`
  names which of the three codecs (QOI, LZ4_RGBA, RAW_RGBA) it is in — this crate does not decode
  pixels, the same choice `Screenshot` makes for `render.screenshot`'s base64 PNG. A caller that
  wants pixels decodes the payload itself, matching `header.format` to a decoder.
- **Errors are `Diagnostic`s, engine errors verbatim.** A `WireResult::Error` is returned as the
  engine's own diagnostic, untouched. The client adds only transport-shaped codes: `L3009`
  (`PROTOCOL_ENGINE_UNAVAILABLE`) for connect failure / handshake stall / lost connection / server
  close, `L3054` (`PROTOCOL_CALL_TIMED_OUT`) when a per-call timeout elapses (the command may still
  run; never resend blindly), `L3051`
  (`PROTOCOL_MALFORMED_REPLY`) for unparseable server messages (`L3001` stays our own request), `L3044` for a reply body of the wrong
  kind, `L3045` for a message over `ludion_protocol::MAX_MESSAGE_BYTES` (256 MiB, the one ceiling
  both ends set on the socket — sent: refused unsent; received: mapped from tungstenite's capacity
  error, never `L3009`), `L3004` for a malformed binary
  frame message, `L3003` (`PROTOCOL_UNSUPPORTED_VERSION`) for version/schema drift.
- **This crate is NOT on the deterministic sim path.** Its `clippy.toml` is intentionally empty
  of `disallowed-types`/`disallowed-methods` — determinism is a property of the engine on the far
  side of the wire, never of this crate. Don't copy the strict sim denylist here.
- `#![forbid(unsafe_code)]`, `#![warn(missing_docs)]` — every public item is documented.

## How it works

`Client::connect(url)` is `connect_within(url, HANDSHAKE_TIMEOUT)` (10 s, the server's own handshake
deadline): it dials each address the host resolves to in turn with `TcpStream::connect_timeout`
(`localhost` may list `::1` first), runs the upgrade with `client_with_config` under read/write
timeouts, sends a `Hello`, and reads one reply. A server that accepts and goes quiet, at the upgrade
or the `ServerHello`, is one `L3009` ("did not complete the handshake"); on Unix tungstenite reports
the upgrade timeout as `HandshakeError::Interrupted`, on Windows as an `Io` `TimedOut`. The socket
timeouts are cleared once the handshake succeeds, so later calls block as the call timeout says.

A reply carrying an `error` field is surfaced as that diagnostic verbatim; otherwise the reply
must parse as a `ServerHello`, whose protocol version and `schema_hash` are checked as above.

`call(command)` allocates the next request id (a per-client counter starting at 1) and serializes
`Request::new(id, command)`. It then reads messages in a loop until *its own* response arrives: a text
message is classified (`classify_text`, by whether the JSON object carries an `event` key) as either a
`Response` or a pushed `EventMessage`; a `Response` whose id doesn't match is skipped (defensive —
this SDK never issues two concurrent calls on one connection), an `EventMessage` is buffered onto
`self.events`, and a binary message is decoded as an LFR1 frame (`decode_frame`) and buffered onto
`self.frames`. `WireResult::Ok { ok, frame, game_frame }` becomes `CallOk { body, frame, game_frame }` once the
matching id shows up.

`subscribe(channel, mode)` / `unsubscribe(sub)` are thin `call` wrappers around `stream.subscribe`/
`stream.unsubscribe`, returning the minted `SubId` or the engine's diagnostic (`L3046` for an
out-of-range `max_fps`).

`next_event(timeout)`/`next_frame(timeout)` first pop their respective queue (filled by a prior `call`
or a prior wait); if empty, `wait_for_event`/`wait_for_frame` set the socket's read timeout (saving the
previous value via `set_read_timeout`), loop reading messages until the wanted kind arrives or the
deadline passes (buffering whatever *isn't* wanted — the frame-side wait buffers a stray event onto
`self.events` and vice versa), then always restore the previous timeout before returning. `Ok(None)`
on timeout is not an error — a caller polls a subscription in a loop the same way either way.

The conveniences map one-to-one onto envelope methods: `sim_hash()` → `sim.hash`; `sim_step(ticks,
hash)` → `sim.step`; `screenshot(width, height)` → `render.screenshot` returning base64 PNG bytes the
crate does not decode. Each convenience returns `L3044` (`PROTOCOL_UNEXPECTED_RESPONSE`) via the public
`unexpected_body` if the response body
kind doesn't match — which cannot happen against a matching-schema server, so it's a protocol-shape
error, not a user error.

Transport plumbing: `read_message` loops over socket messages, skipping ping/pong, mapping a close
frame or transport error to `L3009`. `set_read_timeout` requires the underlying stream to be
`MaybeTlsStream::Plain` — M1 only ever speaks plain `ws://` (no TLS backend is compiled in), so the
non-`Plain` arm is unreachable in practice but returns a `Diagnostic` rather than panicking, since
`MaybeTlsStream` is `#[non_exhaustive]`. Dropping the `Client` closes the socket.

## Verify

```
cargo test -p ludion-client                 # this crate's own tests: tests/streams.rs + the doc example
cargo test -p ludion-client --test streams  # event buffering/ordering, next_event timeout+restore, frame decode
cargo test -p ludion-server                 # wire.rs + exit_criteria.rs + delivery.rs + phase4.rs: driven for real
cargo run -p ludion-cli -- explain L3003   # also L3001 L3004 L3009 L3046 L3051
```

## Gotchas

- Production consumers are `ludion-cli` (dev supervisor), `ludion-mcp` and `ludion-window`; `ludion-server`'s
  tests drive it for real. The CLI's `replay`/`sim-hash`/`serve` still run in-process or host the engine
  directly, so don't assume every verb has a wire path.
- `SubscribeMode::LatestWins` is the only subscribe mode; there is no lossless variant.
- `Screenshot.data` is base64 text, not bytes — the exit-criteria tests carry their own tiny
  `base64_decode` helper precisely because the client (deliberately) doesn't decode.
- Tests that need a deterministic applied frame pause the engine (`Command::SimPause`) *before*
  serving, so an explicit `sim.step` is the only thing advancing the frame — otherwise the live
  scheduler ticks an unknowable number of times between connect and the first command.
- The handshake reply on rejection carries `id: 0`; `next_id` starts at 1, so a request can never
  be confused with a handshake error.
- Call sites serialize wire types with `serde_json::to_string` directly, so the crate needs no
  direct `serde` dependency — keep it that way.
