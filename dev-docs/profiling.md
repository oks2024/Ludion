---
description: Profiling hub: the three timing layers, the one report, the arming rule, layout, verify
---

# Profiling

**Scope:** How Ludion answers *what does a frame cost, and where did the time go* — the three
measured layers (sim per-system, GPU per-pass, transport encode/send), the two surfaces that
produce them (`ludion profile` offline, `profile.capture` / the `profile` channel live), the one
report shape everything speaks — including its offline-only `cluster` section — and the arming rule that makes measurement free when nobody is
watching. This doc ties `ludion-engine`, `ludion-render-graph`, `ludion-server`, `ludion-cli` and
the editor's Performance view together; each crate's internals stay in its own topic. Read it
before adding a timing site, before changing what a report carries, or when a number looks wrong.
Budget *files* and the CI perf gate are `ludion-cli` territory; the delivery path the transport
layer measures is `frame-streaming`; the determinism guarantees the profiler must not break are
`determinism`.

## Layout

- `crates/ludion-protocol/src/perf.rs` — the report types both producers fill: `ProfileReport`
  (`sim` / `gpu` / `transport` / optional `samples`), `ProfileWindowData` (+ `into_report`),
  `ProfileSamples`, `ProfileCaptureParams`, `DEFAULT_CAPTURE_FRAMES` (60), `MAX_CAPTURE_FRAMES`
  (3600, the absolute cap) and `MAX_CAPTURE_WALL_SECONDS` (60, the wall-clock bound).
- `crates/ludion-engine/src/profile.rs` — the **offline** driver: `profile_scene` loads a scene,
  drives warmup + N frames, medians them. `SEQUENTIAL_NAMES` (`world_hash`, `extract`,
  `command_drain`), `POST_GROUPS` (the post-stack pass grouping `--check` compares),
  `profile_cluster` (the clustered-forward light binning, offline only), `profile_replay` (a game's
  committed `.lrp` timed tick by tick through a `DriveHook`, sim only) and `reduce_sim` (the shared
  median + `TimingTail` reduction both sim drivers call).
- `crates/ludion-engine/src/load_observer.rs` — `LoadObserver`, the clock-free seam scene load and
  the asset cache report through (`LoadPhase`, `LoadKind`); its one clock-reading implementor,
  `LoadClock`, lives in `profile.rs` and fills `ProfileReport::load` (`measure_load`). Its
  `FrameLoadObserver` adapts the renderer's own `FrameObserver` (`ludion-renderer/src/frame_observer.rs`:
  `FramePhase::{Upload, GiBake}`, passed to `Renderer::render_observed`) onto the load phases. An
  observed frame fences the device after each phase so GPU time lands in the phase that submitted
  it; an unobserved frame submits exactly what it always did. The first cached extract after a
  world rebuild prefetches the world's assets in parallel (`AssetCache::prefetch`), and a map
  shared by several meshes is one `texture` decode.
- `crates/ludion-engine/src/perf_live.rs` — the **live** profiler: `LivePerf`, `FrameSample`,
  `PROFILE_WINDOW_FRAMES` (60), `LIVE_SEQUENTIAL_NAMES` (`extract`, `submit`), the crate's only
  live-path wall-clock site (`Span`).
- `crates/ludion-engine/src/engine.rs` — `Engine::perf: Option<Box<LivePerf>>` plus
  `perf_arm`/`perf_disarm`/`perf_armed`/`perf_take_windows`/`perf_take_capture`.
- `crates/ludion-engine/src/live.rs` — `sync_perf_arming` (subscription state → armed state),
  `publish_perf_windows`, `PendingCapture` + `flush_pending_capture` (the deferred reply).
- `crates/ludion-render-graph/src/pass_timing.rs` — `PassTimingRing`, the non-blocking 3-deep
  timestamp ring the live loop uses. `timings.rs` is its blocking counterpart, for `dump-timings`.
- `crates/ludion-server/src/perf.rs` — `ProfileHub`, `ProfileArm`, the encode/send rings, and the
  server-side injection that fills `transport` before serialization.
- `crates/ludion-cli/src/profile.rs` — `run` (offline) and `run_attach` (live), converging on one
  `finish` facade for `--json` / `--check` / `--perfetto`.
- `crates/ludion-cli/src/perfetto.rs` — `chrome_trace`: a pure `ProfileReport` → Chrome trace-event
  JSON serializer. Client-side by design; it is not a wire method.
- `editor/app/src/perf/` — `model.ts` (window ring, series extraction, chart layout), `colors.ts`
  (stable series colours), `trace.ts` (the TS twin of `chrome_trace`), `PerformanceView.tsx` +
  `TimelineCanvas.tsx`.
- `editor/personas/performance.md` — the Performance view's terminal persona.
- `crates/ludion-server/tests/perf.rs`, `crates/ludion-cli/tests/profile_attach.rs` — the wire and
  attach integration suites.

## Rules

- **Measurement is observation, never mutation.** Nothing a profiler measures may reach the world.
  An armed engine's `WorldHash` sequence and its recorded `.lrp` are bit-identical to a disarmed
  one's — asserted by `live_perf_does_not_change_world_hash` and
  `profiled_recording_is_byte_identical` in `perf_live.rs`, and by
  `observer_does_not_change_world_hash` for the offline driver. Break this and the profiler stops
  being usable on the artifacts (`.lrp`, goldens) the rest of the repo gates on.
- **Subscribing is the arming switch. There are no start/stop verbs.** A `profile` subscription (or
  a capture in flight) arms; the last unsubscribe or a disconnect disarms. This is why a client
  that dies mid-session cannot leave an engine armed, and why there is exactly one way to turn
  measurement on. Do not add a verb.
- **Disarmed costs nothing.** Engine-side that is one `Option` test per tick — no clock read, no
  allocation, no observer attached. Server-side it is one relaxed atomic load. A change that makes
  the disarmed path pay for the armed path's convenience is a defect.
- **A profiler must never stall what it measures.** `PassTimingRing` *skips* a frame's measurement
  when the ring is full rather than blocking on a readback: a dropped measurement is invisible, a
  stalled render loop corrupts every number in the report. Same reason the transport rings are
  capped and evict rather than grow. They hold one entry per frame (last write wins) and cover a
  full capture, so the transport medians describe the same span as the sim and GPU sections.
- **One report shape.** Offline and live both produce `ProfileReport`; a window converts into one
  via `ProfileWindowData::into_report`. A budget file, a chart, and a trace exporter never learn
  which side produced their input. Adding a number means adding it to `ludion_protocol::perf`, not
  to one producer.
- **The report types live in `ludion-protocol`** even though three of the four layers are measured
  elsewhere, because all three reach a client only through the wire. Every number is a name plus an
  `f64`, so the crate's no-concrete-component-types boundary rule holds.
- **Transport is filled server-side, before serialization.** Every client — editor, CLI attach,
  MCP agent — reads the identical completed message; no client assembles layers itself. There is no
  client-private perf path, which is the parity gate applied to measurement.
- **`ProfileHub`'s mutex is never held across an `.await`.** Nothing in `ludion-server/src/perf.rs`
  is `async`, which is what makes the rule checkable by reading it. Callers time an operation and
  record *after* it completes.


## Dataflow

**Three layers.** *Sim* is per-system wall time through `ludion-ecs`'s clock-free `ScheduleObserver`
seam, plus the tick total and the sequential host spans beside the schedule. *GPU* is per-pass
timestamps. *Transport* is `encode_ms` (the QOI encode inside the frame router's blocking closure)
and `send_ms` (the write of one frame's bytes into a connection's WebSocket sink). Read `send_ms`
as **sink-write time, not delivery** — a client that stops reading shows up here as backpressure.
There is no `readback_ms`: the pipelined `StreamReadback` ring left no blocking readback to
bracket, and what submission costs shows up in the engine-side `submit` span. The editor's
Performance view can subscribe to `frames` (its own preview) to populate this section; the
encode/send it measures is that viewer's own delivery cost, not one the game pays when unwatched.

**The `load` section** is offline only: `Engine::observe_load` (or `EngineBuilder::observe_load`) arms
a `LoadClock` before the scene load, and a disarmed host reads no clock for loading. Its phases are
exclusive self-times, and its decoded/reused/bytes counters are deterministic.

The offline driver, live channel, `profile.capture`, tails, the `cluster` section and the Perfetto export are `ludion devdocs profiling-capture`.

## Verify

```
cargo test -p ludion-engine perf_live      # arming, windows, and both determinism proofs
cargo test -p ludion-server --test perf    # ProfileHub: disarmed records nothing, refcounting, columns
cargo test -p ludion-server --test wire    # the channel and the capture over a real socket
cargo test -p ludion-cli --test profile_attach   # --attach, --check, --perfetto against an in-process server
cargo test -p ludion-render-graph          # PassTimingRing (GPU-skipping where unavailable)
node scripts/check.mjs                     # the whole local gate set
```

A live end-to-end check, which no test performs: serve a scene, subscribe, capture.

```
ludion serve --scene examples/gallery/lights-stress.yaml
ludion profile --attach ws://127.0.0.1:9411 --frames 180 --perfetto trace.json
```

The report should carry the game's own system names; `trace.json` opens at ui.perfetto.dev with
four labelled lanes and no overlap warnings.


## Gotchas

- **Debug-build numbers are not comparable to a release budget.** QOI encode in particular is an
  order of magnitude slower unoptimized (see `frame-streaming`). Profile in `--release`, on the
  hardware the budget was calibrated on, at the scale it was calibrated at.
- **Nor are numbers taken at different worker counts.** `sim.threads` records what the measured sim
  ran on, and it varies per machine by design (`LUDION_THREADS`, a host's `--threads`, else the
  machine's capped available parallelism). Two reports are comparable only at equal `threads` — pass
  `--threads N` explicitly whenever a measurement is going to be compared with another, and read the
  field before believing a regression. It is `null` only in a report predating the field. Results
  themselves never depend on it: the sim is bit-identical at any count (see `determinism`).
- **`send_ms` is sink-write time.** It includes a non-reading client's backpressure and excludes
  kernel-to-client delivery. Text messages are not timed at all.
