# Plan: Engine Profiler — CPU/GPU/transport, agent-queryable

> Status: **SHIPPED 2026-08-02** (Lore revs 448–456, executed as waves 1–6.5; see the Results section
> at the end). P1 and P2 landed in full — `profile.capture` is the **27th wire method**, the live
> three-layer breakdown answers the trigger question from a running process, and `ludion profile
> --attach` reads it. P3 is the standing remainder (encoder-level GPU timestamps, a Tracy/puffin
> sink); the Perfetto export shipped the human deep-dive earlier and by other means. Read the Results
> section before the design below — the design is preserved as authored, and the Results record where
> the build deviated from it. Decision by user (directive 2026-07-15: "we should plan a robust cpu/gpu
> profiler, it will be more and more needed to access performance data").

## Trigger criterion

**Met.** The gate is: *a performance investigation had to hand-roll throwaway timing because the engine
could not answer "where does this frame's N ms go?" from a running process.* That has now happened
twice in three days:

- The **G0 input-lag** investigation and the **debug-plaza ~19 fps cap** each needed temporary
  `std::time::Instant` probes and crate-level opt-level bisection to name a hot path
  (`docs/frame-streaming.md:126-163`, "debug-profile QOI encode"). The
  bisection burned hours and *still* required manual instrumentation to isolate the QOI encode
  (`~5–6 ms @720p / ~12 ms @1080p`) and to prove the ~19 fps cap was broad debug-codegen in the
  render/readback path, not the encode.

A `profile.capture` over the wire would have answered both in one command. The North-Star cost of the
gap is direct: every un-instrumented perf question spends agent iterations that a query would spend
none of.

## Context — what the engine can and cannot measure today

The engine already has the two *hard* pieces of a profiler and is missing the *cheap* connective
tissue and the wire surface.

**Present and working:**

- **`ludion profile` (CLI, offline)** — `crates/ludion-cli/src/profile.rs` +
  `crates/ludion-engine/src/profile.rs`. `profile_scene(path, frames, warmup)`
  (`profile.rs:116`) loads a *scene file*, drives a warmup then N frames, and emits one JSON report
  with three sections: sim per-system **medians** via the clock-free `ScheduleObserver` seam
  (`TimingObserver`, `profile.rs:152-170`), GPU per-pass medians (`profile_gpu`, `profile.rs:224`), and
  a sim frame-total median. `--check budget.toml` compares against per-line ms ceilings
  (`Budget`, `cli/src/profile.rs:100-110`; `check_budget`, `:124-153`) — over-budget is `L4005`, a bad
  budget file `L4004`. Median is the chosen statistic (`median_ms`/`median_f64`,
  `profile.rs:302-328`).
- **Per-pass GPU timing** — `crates/ludion-render-graph/src/timings.rs`. `dump_timings_json`
  (`timings.rs:52`) brackets each pass with wgpu-30 **descriptor-level** timestamp writes
  (`RenderPassTimestampWrites`, `execute.rs:486`), resolves + reads back + scales by the queue
  timestamp period (`collect_pass_timings`, `timings.rs:100-197`). One machinery serves both the
  built-in proving graph and the external HD-2D recorder (`timings.rs:11-13,137-148`). Capability is
  detected once (`execute.rs:114`) and requested only when present (`:115-119`); a missing feature
  degrades to `{"timestamps_supported": false, passes:[…null]}` (`timings.rs:78-90`), never an error.
- **Two wire methods already expose this** in the 26-method surface: `render.dump_timings`
  (`command.rs:179` → `dispatch.rs:68` → `commands/render.rs:61-70`) and `render.dump_graph`
  (`command.rs:178` → `render.rs:40-45`).

**Missing — the gaps this plan closes:**

1. **No transport-segment timing anywhere.** `ludion-server`'s delivery path uses `Instant` *only* for
   `max_fps` throttling (`crates/ludion-server/src/delivery.rs:232-263`). The QOI encode on the
   blocking pool (`qoi::encode_to_vec`, `delivery.rs:195`) and the wire send are **never timed** — the
   exact numbers both investigations had to hand-measure. Readback, encode, and send are the three
   segments that turned out to dominate, and none is observable.
2. **`render.dump_timings` over the wire does not profile the live frame.** The handler re-renders a
   *fresh synthetic frame once* (`extract_render_scene` + `dump_timings_json`, `render.rs:63-67`) — a
   single sample of a frame nobody is watching, not the in-flight streamed frame, and with no median.
   It answers "how long would this pass take in isolation," not "where did *this* delivered frame's
   time go."
3. **Sim per-system timing is unreachable from a running game.** The `ScheduleObserver` timing path is
   wired only into the offline `profile_scene`. Worse, the CLI **cannot link a game's own systems**, so
   for any CLI-loaded scene the `systems` list is empty by construction (`SimProfile.systems` doc,
   `profile.rs:55-58`). The one place a real game's systems *do* run — the live server loop — has no
   way to report their per-system cost.
4. **No `profile.*` wire method, and no perf in `engine.describe`.** `engine.describe` returns versions
   + schema only (`engine_cmds.rs:11-15`); `render.bounds` is pure CPU geometry, no timing
   (`render.rs:51-57`). A running `ludion serve` / editor / `ludion dev` process cannot be asked for a
   frame breakdown at all.

Net: the engine can profile a *toy re-render of a scene file offline*, but cannot answer "where does
*this running frame* go?" — which is the only question the performance work actually had.

## Decision: direction

**Recommended: hand-rolled spans assembled into a `profile.capture` / `profile.stream` wire method
(design-space option 1), with Tracy/puffin as an OPTIONAL human sink layered over that same JSON
(option 3). Reject the `tracing` ecosystem as the core (option 2); keep it in reserve only as a future
export bridge.**

Why: the pillars decide this cleanly. A profiler is a *capability*, so under **One Command Core** it
must exist as a typed wire method first — the agent queries perf exactly the way it queries a
screenshot, and the editor/`ludion-window` human clients ride the same surface for free (no
editor-private perf path, the same hard parity gate that rules out a shared-memory frame channel in
`Plan/frame-codec-lz4.md`). The engine already owns the two seams a hand-rolled core needs — the
WorldHash-neutral `ScheduleObserver` (proven not to perturb sim, test `observer_does_not_change_world_hash`,
`profile.rs:367`) and the wgpu timestamp path — so option 1 is *mostly wiring already-built pieces to a
new method*, not new infrastructure. `tracing` as the core fails two pillars at once: it drags a
cross-cutting dependency into the determinism-locked sim crates (against the per-crate clippy
denylists and the "one obvious way" rule), and its native output is host-console/flamegraph oriented,
not protocol-native JSON — the agent would be the second-class consumer of its own engine. Tracy/puffin
are superb *human deep-dive GUIs* and belong as an opt-in sink a human client renders **from the
`profile.stream` JSON**, never as a capability that exists only in a GUI.

## Determinism — how instrumentation stays outside the envelope

The profiler **observes**; it must never feed time back into sim state. The discipline is already
established by `profile.rs` and is reused verbatim:

- **One sanctioned wall-clock site per crate, output-only.** `profile.rs:11-19` documents the pattern:
  the sim-side clock reads live only inside the observer's `before_system`/`after_system`
  (`profile.rs:160-169`), scoped `#[allow(clippy::disallowed_methods)]` with the rationale, and the
  clippy denylist keeps `Instant::now` banned everywhere else. New sim-side timing reuses that exact
  seam — it cannot mutate `World`, so the WorldHash is provably unaffected (the existing test is the
  template).
- **Transport timing lives in `ludion-server`, already outside the determinism envelope.** Encode/send
  happen post-sim, downstream of the frame the sim produced; timing them changes nothing the sim hashes.
  `ludion-server` already reads the wall clock for throttling (`delivery.rs:232-263`).
- **Stripped from replay hashing.** Timing numbers are never part of `WorldHash`, the snapshot format,
  or the `.lrp` replay stream — a replayed session reproduces the same *state* regardless of how long
  any frame took, exactly as the LZ4 codec plan notes for encoded frames.
- **Zero-cost when disabled.** Capture is off by default. The sim-loop observer is attached only while
  a `profile.capture`/`profile.stream` is active; when no profile is in flight, `run_live` takes the
  existing un-observed path and pays nothing (mirror `advance_live` vs `advance_observed`,
  `profile.rs:373-382`). This is a hard requirement, not a nicety — an always-on profiler that costs
  frame time would corrupt the very budgets it feeds.

## Design — three timing layers, one report

The report assembles **three distinct layers** for a window of *live* frames, each a median (the
noise-resistant statistic the existing profiler already commits to):

1. **Sim layer** — per-system + sim frame-total wall time, from the `ScheduleObserver` seam attached to
   the live loop. This is the layer the running game uniquely has and the offline CLI cannot (it links
   no game systems).
2. **GPU layer** — per-pass GPU ms from the existing timestamp machinery, measured on the *live* frame
   rather than a synthetic re-render.
3. **Transport layer** — readback ms, encode ms, and send/wire ms per frame, newly recorded in
   `ludion-server`'s delivery path. This is the layer that was missing entirely and that both
   investigations needed.

### P1 — the live frame breakdown in one command (smallest thing that answers "where do the 52 ms go?")

The minimum that would have closed both 2026-07-15 episodes with a single query.

- **Transport segments.** In `crates/ludion-server/src/delivery.rs`, wrap the readback boundary, the
  `qoi::encode_to_vec` call (`:195`), and the send with span timing, pushing `(readback_ms, encode_ms,
  send_ms)` into a small fixed-capacity ring per frame channel (a rolling window, not unbounded
  growth). This is additive to the existing `Instant`-based throttle bookkeeping and touches no sim
  crate.
- **Live sim + GPU timings.** Attach the `TimingObserver` seam to the live sim loop (`live.rs`) *only
  while a capture is active*, and take the GPU per-pass numbers from the frame the loop already
  renders rather than a fresh `extract_render_scene`. Both reuse code that exists; the work is the
  on/off gating and threading the samples to the server.
- **`profile.capture` wire method** (additive protocol rev per `docs/protocol-evolution.md` — wire
  types → `BLESS_SCHEMA=1` snapshot → `EXPECTED_SCHEMA_HASH` → `npm run generate` → re-record
  `games/*/run.lrp`, one atomic commit; same checklist `Plan/frame-codec-lz4.md` runs). Params: a
  frame count / window. Returns one JSON `ProfileReport` extended with a `transport` section
  (`readback_ms`, `encode_ms`, `send_ms` medians) alongside the existing `sim` + `gpu` sections. The
  27th method; dispatch is a ≤10-line arm delegating to the owning domain (`dispatch.rs` stays
  routing-only). The response reuses the `ProfileReport` shape so the CLI and wire share one type.
- **Report shape stays the CLI's.** `ProfileReport`/`SimProfile`/`GpuProfile` (`profile.rs:37-91`) gain
  a `transport` sibling; the offline `ludion profile` fills it with `null`/absent (no server, no
  transport), the wire capture fills it in. One struct, two producers.

Exit check for P1: on the vigil-plaza scene under `ludion serve`, `profile.capture` names the encode as
the transport-dominant segment with a median in ms — the answer the hand-instrumentation produced,
now in one call.

### P2 — continuous streaming + budgets over the wire + attach

- **`profile.stream`** — a subscription channel modeled on frame streaming (`stream.subscribe`
  machinery, `command.rs:181`), emitting a per-window perf event so the editor/agent can watch cost
  live instead of polling `profile.capture`. Humans and agents read the same event; a Tracy/puffin GUI
  (P3) is just one more consumer of it.
- **`ludion profile --attach <server>`** — point the existing CLI at a *running* process and read
  `profile.capture` instead of driving a scene file offline. This is the real fix for gap #3: the
  running game *has* linked its own systems, so per-system numbers finally include game code. The
  offline scene-file mode stays for CI (it needs no server), and the same `--check budget.toml`
  (`L4004`/`L4005`) runs against either source — one budget format, two capture sources.
- New diagnostics land in the **L4xxx profiling domain** (L4001–L4005 used, `codes.rs:408-426`; next
  free `L4006`) — e.g. "capture requested but no adapter," "stream backpressure dropped a window."

### P3 — deeper GPU timing + optional human deep-dive sink

- **Encoder-level GPU timestamps** for inter-pass gaps and the readback copy, via wgpu-30
  `TIMESTAMP_QUERY_INSIDE_ENCODERS` (`CommandEncoder::write_timestamp`;
  `wgpu-types-30.0.0/src/features.rs:702-721`). **Capability caveat, verified against the vendored
  source:** this feature is Vulkan/DX12/Metal-AMD-&-Intel/OpenGL — **not available on Apple GPUs**, and
  `TIMESTAMP_QUERY_INSIDE_PASSES` (`:722-747`) is "generally not available on tile-based rasterization
  GPUs." So encoder-level timing must **degrade exactly the way descriptor-level timing already does**
  (`timings.rs:78-90`): report the extra segments where supported, null them where not — and the
  portable per-pass path (needing only base `TIMESTAMP_QUERY`, `features.rs:1663`) remains the floor on
  every adapter including the macOS pinned target. (Note: the in-code comment at `execute.rs:485`
  misspells the feature as `TIMESTAMPS_INSIDE_ENCODERS`; the real name is
  `TIMESTAMP_QUERY_INSIDE_ENCODERS` — worth fixing when this lands.)
- **Optional Tracy/puffin sink** in `ludion-window` and/or the editor, rendering the `profile.stream`
  JSON as a flame view. Strictly a client-side view over the canonical wire data — it adds no
  capability the protocol lacks, honoring the "no GUI-exclusive capabilities" anti-goal.

## What NOT to do

- **No `tracing`/`tracing-subscriber` in the sim crates.** It violates the per-crate determinism
  denylists and the one-obvious-way rule, and its output is host-oriented. If a `tracing` *export
  bridge* is ever wanted for an external tool, it layers over `profile.stream` outside the sim crates —
  never inside them.
- **No always-on profiling.** Capture is opt-in and zero-cost when off (see Determinism). An
  unconditional per-frame timing tax would poison the budgets it feeds and dent the fps the transport
  work fought for.
- **No GUI-only profiler and no side-channel.** Tracy/puffin are sinks over the wire JSON, not the
  source of truth; no shared-memory or editor-private perf path (same parity gate that rules out a
  shared-texture frame channel in `Plan/frame-codec-lz4.md`).
- **Do not delete the offline `ludion profile`.** It is the CI budget gate and needs no running server;
  P1/P2 *feed* it a richer report and an `--attach` source, they do not replace it.
- **No sampling/statistical profiler or external `perf`/VTune dependency as the core.** Instrumented
  spans on the schedule + frame-graph seams are deterministic-friendly, protocol-native, and already
  half-built; a sampling core would be host-oriented and OS-specific.
- **Do not let timing touch determinism.** No sim-side wall-clock read outside the sanctioned observer
  seam; nothing timing-related enters `WorldHash`, snapshots, or `.lrp`.
- **Do not fold sim tick-rate or 120-fps decisions in here.** Those are the separate scope
  `Plan/frame-codec-lz4.md` already fenced off; the profiler only *measures*.

## Verify

Per phase, the standard loop plus targeted checks:

```
node scripts/check.mjs --rust-only          # fmt + clippy (incl. determinism denylists) + test + smoke
cargo test -p ludion-engine profile         # observer WorldHash-neutrality + report-shape stability
cargo test -p ludion-render-graph           # per-pass timing machinery, supported + fallback shapes
```

- **Protocol rev (P1+):** the five-step additive-rev checklist end to end —
  `cargo test -p ludion-protocol` (code ↔ snapshot + hash gate) → `BLESS_SCHEMA=1 … schema_snapshot`
  → hand-copy `EXPECTED_SCHEMA_HASH` → `npm run generate` (`editor/protocol`) →
  `pwsh ci/scripts/schema-drift.ps1` → re-record `games/*/run.lrp` last, one atomic commit.
- **Determinism proof:** extend the `observer_does_not_change_world_hash` template to the live-loop
  attachment — a profiled `ludion serve` run must produce a byte-identical `WorldHash` sequence and
  `.lrp` to an unprofiled one.
- **The trigger, closed:** on the vigil-plaza scene, `profile.capture` (or `--attach`) reports the
  transport-segment medians (readback / encode / send) plus sim-frame-total and per-pass GPU in one
  call — the numbers the 2026-07-15 hand-instrumentation produced, now queryable. Confirm the encode
  median matches the isolation-benched `~12 ms @1080p` on noisy voxel content
  (`docs/frame-streaming.md:126-163`) within measurement noise.

## Results (2026-08-02): shipped, P1+P2 in full — where the build deviated from the design

Lore revs 448–456, executed as waves 1–6.5. The three-layer shape, the arming discipline, the
report-as-one-type rule, and every "What NOT to do" line held as written. Six deviations are worth
recording so the design above reads true against the code:

- **The transport triple lost `readback_ms`.** The design named readback/encode/send, written against
  a delivery path that still blocked on readback. The pipelined 3-deep `StreamReadback` ring landed
  first (`docs/frame-streaming.md`), so by the time the profiler was built there was no blocking
  readback left in the server to bracket — the cost is submission-side and shows up in the
  **engine-side `submit` span**. Transport is `encode_ms` + `send_ms`, and `send_ms` is sink-write
  time, not network delivery.
- **Subscription IS arming; there are no start/stop verbs.** `profile.stream` as designed became
  `Channel::Profile` + `Event::ProfileWindow` (~1 Hz, one window per 60 sim frames) — subscribing arms
  the profiler, unsubscribing disarms it, so a client cannot leave the engine armed by dying mid-session
  and there is no second way to do the same thing. One `Option` test engine-side and one relaxed load
  server-side is the whole off-cost.
- **Windows carry columnar per-frame samples, not medians alone.** The design committed to medians as
  *the* statistic. Medians cannot draw a timeline, and the timeline is what the editor view and the
  Perfetto export both need, so a window carries both: the medians the budget gate compares against,
  plus per-frame sample columns. Columnar, not per-frame structs, to keep the JSON small at 60 rows.
- **The diagnostics are `L4012`/`L4013`, not `L4006`.** The plan's "next free `L4006`" was stale by the
  time it was built — the post-process and hardening arcs took L4006–L4011. `L4012` is an
  out-of-range frame count, `L4013` a capture arriving while one is already in flight.
- **A deferred capture reply blocks its own connection.** `profile.capture`'s reply is held until the
  requested window closes — the simplest shape that keeps one request/one response, but it means a
  capture freezes every other call on *that* connection for its duration (the reason the editor's
  capture card opens a short-lived connection of its own, and the reason an MCP-attached agent should
  prefer `ludion profile --attach`). Documented on the wire variant itself, since the shape is
  invisible from the method name.
- **P3 stands open, but its human half arrived early by another route.** Encoder-level GPU timestamps
  (`TIMESTAMP_QUERY_INSIDE_ENCODERS`, with the Apple-GPU degradation caveat) and a Tracy/puffin sink
  remain unbuilt future work. The **Perfetto export** — a pure client-side trace-event serializer over
  the same report, shipped in both the CLI (`--perfetto`) and the editor — delivered the human
  deep-dive P3 was holding, with no new capability and no new wire method, exactly the
  "sinks over the wire JSON" posture P3 argued for.

**The game preview (Performance view), shipped the same day, is not designed above — recorded here
since it rides this arc's wire surface.** It is a streaming 16:9 box that adds **no GPU work
structurally**: no wire verb changes render resolution, and an armed profiler already renders every
frame, so the preview only stops discarding a frame that was already produced. Transport (encode +
socket) is real added cost and is this viewer's alone — the point, not a caveat. The preview's chip
shows the *true* pixel dimensions of the stream, not the CSS box size, so "still rendering full
resolution" reads directly off the screen. `Command::may_change_frame()` (`command.rs:237`) is
exhaustive with no wildcard; its two oldest settled traps are `AssetReload` ⇒ **true** (a reloaded
mesh/texture/shader changes every pixel though no entity moved) and `StreamSubscribe` ⇒ **true** (a
viewer attaching to a paused session is owed a first frame nothing else would trigger). Frame decode
runs off the editor's main thread (`framePump.ts`'s latest-wins gate + `frameWorker.ts`); the default
path is `worker-blit`, and `offscreen` waits behind the `ludion.editor.frameOffscreen` `localStorage`
flag because the canvas transfer it needs is permanent and `Viewport` reads `canvas.width` for
gizmo/picking geometry.

**The trigger, closed.** `profile.capture` on the vigil plaza names the encode as the transport-dominant
segment: **11.87 ms median at 1080p, dev profile** — in the isolation-benched `~12 ms @1080p` band, now
in one call instead of an afternoon of hand-instrumentation. **Determinism held by proof**, both halves
of the exit criterion: a profiled run's `WorldHash` sequence and its recorded `.lrp` are byte-identical
to an unprofiled one's. **Docs:** the `profiling` topic (three layers, two capture surfaces, the arming
rule, the two determinism proofs, the gotchas), the editor's `performance` persona, and the
frame-streaming encode figures re-measured against today's 3×-scale plaza.
