---
description: The native windowed viewer — an in-process peer protocol client over ludion-client, not a GPU blit
---
# ludion-window

**Scope:** `ludion-window`'s [`run`] — the native OS window (winit + wgpu) that shows a host it
*attaches to*. The single fact that governs everything else in this crate: **it is an ordinary
peer protocol client**, wired exactly like the Tauri editor or an MCP-driven agent, just started
in-process for convenience. A game hosting its own window does not use this path at all — it
presents in process (`ludion devdocs ludion`); the viewer's one remaining host is `ludion dev
--window`, whose window must outlive a restarting child. This doc covers the four-thread
architecture, the connect/reconnect contract, and how input becomes wire commands. It defers the
frame-streaming wire format itself to `frame-streaming`, the protocol/`Client` SDK to
`ludion-client`, and `dev --window`'s process-level behavior (headless-child rules) to
`ludion-cli`.

## Layout
- `crates/ludion-window/src/lib.rs` — `run`/`WindowOptions`, the winit `ApplicationHandler` (`App`),
  and the four `spawn_*_thread` functions that start the stream/input/gamepad/waiter threads.
- `crates/ludion-window/src/reconnect.rs` — `run_frame_stream` + `StreamSink`: the reconnecting
  frame-stream loop, factored out of the stream thread so it's testable without opening a window.
  Owns the backoff schedule and the fatal-vs-retryable classification.
- `crates/ludion-window/src/frame.rs` — `DecodedFrame` + `decode_frame`, decoding a streamed frame
  payload in whichever of the three wire formats (QOI, LZ4_RGBA, RAW_RGBA) it arrived in.
- `crates/ludion-window/src/gpu.rs` — `Presenter`: the wgpu surface (device via
  `Gpu::request_for_surface`), the shared `ludion_render_graph::blit` of the latest decoded frame,
  resize/reconfigure handling; a device fault after a submit is fatal and ends the viewer.
- `crates/ludion-window/src/host_input.rs` — `HostInput`, the one router both native window hosts
  (this viewer and the facade's in-process window) call for keyboard, mouse and focus events, so a
  session recorded through either window carries the same `input.inject` events. `keymap`, `mouse`
  and `gamepad` stay public as its building blocks.
- `crates/ludion-window/src/keymap.rs` — physical keycode → the wire's `input.inject` key-event
  shape (`KeyMapper::map`). Stateful only to fold left/right Shift/Ctrl/Alt into one key;
  `KeyMapper::release_held` frees every held key on window focus loss.
- `crates/ludion-window/src/mouse.rs` — cursor-position → render-target-space mapping
  (`map_cursor_to_frame`; `window_to_surface` first scales a window beyond the surface cap) plus mouse button/wheel → wire event `Value`s.
- `crates/ludion-window/src/gamepad.rs` — `gilrs` axis/button → wire `gamepad_*` event `Value`s
  (pure mapping; capture itself happens in `lib.rs`'s gamepad thread).
- `ludion_core::letterbox` — the letterbox geometry math shared by the presenter and the cursor
  mapping (so "where the frame is drawn" and "where a click lands" never disagree), moved to
  `ludion-core` so `ludion-render-graph`'s in-process present target can share it too.
- `crates/ludion-cli/src/dev.rs` — this crate's one CLI host: `ludion dev --window` keeps a single
  persistent window across every child restart (its reconnect rides the same-port blink, so it never
  blinks itself). `crates/ludion-cli/src/serve.rs` opens no viewer — `serve --window` hosts
  in-process through the facade's `host_scene`, like every other windowed game.
- `crates/ludion-cli/tests/window_reconnect.rs` — drives `run_frame_stream` against a real `ludion
  serve` the test kills and restarts, headlessly (no window opened).
- `crates/ludion/src/present.rs` — the facade's in-process windowed shell, which routes input through this
  crate's `HostInput` (and reuses `gamepad`) but none of its transport: `run --window` and `serve --window`
  present the engine's own frames rather than attaching a viewer. The facade's dependency on this
  crate sits behind its default `window` feature; a build without it refuses `--window` up front
  (`L0018`) instead of hosting headless unasked.

## Rules
- **No privileged access to the engine.** This crate connects over [`ludion_client::Client`] like
  any other client and sends only `input.inject` commands — there is no back-door to renderer GPU
  state or a private endpoint. This is what keeps the parity pillar intact: a human's window shows
  exactly what a headless agent's `render.screenshot` query would show.
- **`run` must be called on the process's main thread.** Winit requires the event loop to own it;
  `ludion dev --window` structures its process around this, supervising the child from a worker
  thread (see its module docs for exactly which thread does what).
- **The stream thread owns the one fatal path.** Only a fatal schema/version mismatch ends the
  loop with an `Err` (a lost connection is retried while the window is open); the
  input and gamepad threads are always best-effort and never end the loop themselves — a dropped
  input event during a blink is simply lost, correctly, since the sim is down anyway.
- **A schema/version mismatch is the one connect error treated as fatal on sight**
  (`reconnect::is_fatal`, keyed on `PROTOCOL_UNSUPPORTED_VERSION`) — it means a different engine
  build is now on the port, so retrying would only mask it. Every other connect failure (typically
  `PROTOCOL_ENGINE_UNAVAILABLE`, the ordinary "nothing's listening yet" blink) is retried.
- `#![forbid(unsafe_code)]`, `#![warn(missing_docs)]`. Cargo.toml deliberately excludes
  `ludion-engine`, `ludion-server`, `ludion-renderer`, and tokio — a client-side dependency creeping
  in here is a boundary violation worth catching in review.
- The `keymap`/`mouse`/`gamepad` modules and `spawn_gamepad_thread` are public reuse points for the
  `ludion` facade's in-process present host (`crates/ludion/src/present.rs`) — a *host*, not a
  client, so it wires these device-event builders directly instead of going through a `Client`. This
  crate itself stays the peer-client viewer; nothing here becomes privileged.

## How it works
`run(options, stop)` builds the winit event loop, then spawns four worker threads before handing
control to `event_loop.run_app`:

- **Stream thread** — runs `reconnect::run_frame_stream` with a `WindowStreamSink`: connects its own
  `Client`, subscribes to `Channel::Frames` with `SubscribeMode::LatestWins`, and on each decoded
  frame stores it in a shared `Arc<Mutex<Option<Arc<DecodedFrame>>>>` slot and wakes the event loop
  via `EventLoopProxy::send_event(FrameReady)`. On a dropped connection it flags `Reconnecting`
  (title changes to "… — reconnecting…"), retries with exponential backoff (100ms → 2s ceiling,
  endless — a same-port `ludion dev` blink, typically 1–3s, is invisible, and a dead server is waited
  out), and flags `Reconnected` once frames resume.
- **Input thread** — owns a second `Client`; drains one shared channel that already carries
  pre-built `input.inject` event `Value`s from *all* input sources (keyboard, mouse, gamepad — see
  Gotchas) and forwards each as `Command::InputInject`. Reconnects lazily: a dropped connection just
  clears its `Client`, and the next event triggers a fresh `Client::connect` before sending.
- **Gamepad thread** — polls `gilrs` for the first connected pad only (multi-pad is out of scope)
  and pushes mapped `gamepad_*` `Value`s onto the same channel the input thread drains. Ends
  immediately, harmlessly, if no gamepad backend exists on the machine.
- **Waiter thread** — blocks on the caller's `stop` receiver (a host's Ctrl-C hook) or an internal
  signal fired when the event loop tears down for any other reason, and asks the loop to exit.

On `resumed`, the main thread creates the OS window and a `Presenter` (the wgpu surface). Every
`RedrawRequested` clones the latest frame out of the slot (an `Arc` clone, not a pixel copy) and
letterboxes it into the surface, clearing to black around it. Keyboard/mouse events from winit are
mapped (`keymap`/`mouse`) and pushed onto the input channel; cursor positions are mapped from
window-surface pixels into the *decoded frame's* pixel space using the same letterbox geometry the
presenter draws with, so a click lands where the pixel under it actually is regardless of window
size or aspect.

Teardown (window closed, `stop` fired, or a fatal `ConnectionLost`) flips a shared `running` flag,
drops the app (which drops `input_tx`, ending the input thread's `recv`), signals the waiter, and
joins all four threads with bounded waits before `run` returns `Ok(())` (clean) or the fatal
diagnostic (typically `L3009` connection-lost, or `L2007`/`L2003`/`L2004` for a window/GPU-adapter/
GPU-device that could never be created in the first place).

## Verify
```
cargo test -p ludion-window                          # title-flag unit test, pure geometry/mapping tests
cargo test -p ludion-cli --test window_reconnect      # reconnect over a real killed-and-restarted `ludion serve`
cargo run -p ludion-cli -- dev --window              # manual: move/click, then the window rides a live-reload restart
cargo run -p ludion-cli -- explain L2007
```
There is no windowed golden test — the streamed pixels are the same frames `render.screenshot`
goldens already cover; this crate's own tests are headless (`run_frame_stream` driven against a
`RecordingSink`, no window opened).

## Gotchas
- **This is a peer client, not a GPU blit of the engine's own framebuffer.** It shares the blit
  code and the adapter policy, never the engine's wgpu device — it decodes frames off the wire
  (whichever of the three codecs `default_stream_format` asked for) exactly like the Tauri viewer
  does. If the picture looks wrong, suspect the frame-streaming/encode path (`frame-streaming`,
  `ludion-renderer`) before suspecting this crate's presenter.
- **All input — keyboard, mouse, *and* gamepad — is recorded as ordinary `input.inject`,
  indistinguishably from any other client's injected input.** The input thread only ever sees
  pre-built `Value`s and doesn't know or care which device produced them. A
  session recorded through the window replays identically to one recorded through any other client
  that calls `input.inject` — there is nothing window-specific in the recorded `.lrp`.
- **`ludion dev --window`'s child is always headless.** Passing `--window` through to the
  supervised child would make the window die and reopen on every reload; the persistent window
  lives in the `dev` process itself and reconnects across the child's same-port restart instead.
  Don't expect `--window` on the child side to do anything in dev mode.
- **The window rides any outage while `dev` supervises.** A lost connection, including a child
  that cannot boot, leaves the window open and flagged "— reconnecting…" until the next good save
  relaunches the child. Only a fatal schema mismatch or the supervisor ending (dev stops, Ctrl-C,
  or a supervisor panic) closes it; a window failure is not a silent fallback to headless
  supervision.
- **Reconnect time is never build time.** During a long rebuild the *old* child is still serving, so
  the stream never drops; only the gap between the old child's shutdown and the new child's listen
  is an outage.
- **Multi-pad setups only ever drive the first-seen connected gamepad.** A second pad's events are
  silently ignored (`gamepad.rs`); this is a recorded scope limit (`Plan/phase-13-input.md`), not a
  bug to chase.
- **AltGr on Windows.** The OS reports AltGr as AltRight plus a ControlLeft press, so the fold sends
  `Ctrl` and `Alt` down together. A game should not bind a Ctrl chord that a non-US layout types as
  AltGr.
