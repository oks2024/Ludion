---
description: The facade's embedding core: surface-before-bind order, safe rect, argv modes, touch, iOS staticlib
---

# ludion — embedding core

**Scope:** the one host lifecycle behind `launch`, `run`, `host_scene` and the C ABI: how a surface,
listener and input intake are wired, host geometry, touch, and the iOS staticlib build. The facade's
layout, construction and disk seams are `ludion devdocs ludion`; the window crate is `ludion devdocs ludion-window`.

## Rules

- **`--window` attaches the surface *before* `Server::bind`, and feeds input through the core's
  intake — never a private path.** The main thread hands the surface to `Embedded::start`, whose sim
  thread attaches it before it binds; the order is forced: the renderer is built on
  the device the surface chose, and `bind` moves the engine onto the sim thread. The parity pillar:
  a human's window shows the very frame an agent's `render.screenshot` returns, and every key, click
  and pad sample is an ordinary `input.inject` in the recording. `ludion-window`'s viewer keeps one
  role: `ludion dev --window`, whose window outlives a restarting child.
- **The embedding core is the one lifecycle for every host — `Server::bind` is called only in
  `embed.rs`.** `launch` is it with no surface; `run` and `host_scene` (so `ludion serve`, a
  built engine as `Source::Built`) host through it too, windowed or headless, over the same engine
  prep (`prepare_game_engine`) — never a second path.
  `Embedded::start(builder, LaunchRoots, Option<Surface>, StartOptions)` does only cheap work
  (env settings, the default scene → `L0019`) and returns; the `ludion-sim` thread loads the scene,
  attaches the surface, then runs `run_live` itself or, with `listen` under `wire`, binds a `Server`
  and forwards the core's intake into it (`CommandSink::forward`, reply kept). A surface with no
  listener presents with `run_live`'s egress `None`. `ffi.rs` is `start` plus
  pad/touch/resize/suspend/resume/stop forwarders (`L0020`) and `ludion_ended` (the engine ended by
  itself — an argv mode, `engine.shutdown`, a failed boot, a fatal render fault — so a host can
  exit); a game exports the one
  symbol its native host calls. The FFI start binds loopback `9411` under `wire`, as `launch` does
  (`start_with(.., None)` is the unit tests', which must not contend for it). `Plan/ios-target.md` D5.
- **The safe rect is host geometry, never world state.** `StartOptions::safe_rect` /
  `Embedded::resize(w, h, safe)` fit the present (`letterbox_in`, one packed atomic on `PresentSize`,
  also read by the cursor map) into a rect of the surface; unless an explicit render size names one,
  `safe_render_extent` derives the extent (720 high, at the rect's aspect, ≥16:9) and a resize that
  changes it sends `render.size` through the intake.
- **Argv modes run on the boot thread.** `StartOptions::argv` (`ffi` passes the process argv under
  `wire`) is parsed as `run`'s `RunArgs`; a named mode (`--replay-verify`, …) runs through
  `run_short_circuit` against the `LaunchRoots` in place of hosting and prints its verdict. Argv that
  is not `run`'s warns and hosts — never fails; argv selects modes only, not the host's shape.
- **Touch is a game-owned layout mapped onto existing pad events (`touch.rs`).** `touch.yaml`
  (`kind: touch-layout`, a `ludion_scene::KINDS` row) sits beside the hosted scene; absent is no
  controls, malformed is `L0021` and refuses `Embedded::start`. Sizes and offsets are fractions of
  the safe rect's height; a stick emits the clamped deflection (+Y up, the game owns its deadzone),
  optionally the side's trigger past `deadzone` and a one-tap D-pad flick — never a held D-pad, which
  would outrank the stick. A finger that slides off a button releases it for good. `suspend` rests
  every control (suspend/resume wait at most 2 s, then report `L3023` with the transition still
  queued); the next `Began` resumes the core and is swallowed (`TouchOutcome::Resumed`, even with no
  layout). The host draws at `touch_controls()` rects (`ludion_touch_control`) and never
  hit-tests. Touch adds no `InputEvent`, so no wire or replay moves.
- **The iOS staticlib is built by the command, never by the manifest** (`ludion export` runs it). Run from `games/vigil/` (the private, Lore-only reference game): `cargo rustc -p vigil --lib
  --crate-type staticlib --target aarch64-apple-ios[-sim] --release --no-default-features [--features
  wire]` — vigil's `Cargo.toml` never names `staticlib` (it would build one on every desktop build;
  `Plan/ios-target.md` decision 5). The link needs `-framework AudioToolbox CoreAudio CoreFoundation
  QuartzCore CoreGraphics Metal Foundation` plus `-lobjc -liconv -lSystem -lc -lm` (cpal, wgpu-hal).

## Verify

```
cargo test -p ludion                              # embed/ffi/touch unit tests
node scripts/mobile-check.mjs                     # wire/window off: clippy, tree; + aarch64-apple-ios if installed
```
