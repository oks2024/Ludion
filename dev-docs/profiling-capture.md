---
description: Profiling capture surfaces: offline driver, live channel, profile.capture, tails, Perfetto
---

# profiling-capture

**Scope:** the capture half of `ludion devdocs profiling` (the hub: layers, report shape, arming rule, layout). Offline and live surfaces, the deferred capture reply, replay tails, the `cluster`, `load` and `skin` sections and the Perfetto export.

## Dataflow

**The `cluster` section, which is not a fourth timing layer.** A report may also carry
`ClusterProfile`: the clustered-forward binning's median build time beside the *shape* of the tables
it produced. It is the mirror image of `transport` — filled by the **offline** driver only, and for
the mirror-image reason: a live host builds its cluster tables inside a frame a profiler must
neither stall nor make pay twice, while the offline driver owns its loop and calls
`ludion_renderer::cluster_stats` itself. That call is pure CPU arithmetic over an already-extracted
scene, so this is the one section of a report that never degrades on an adapter-less box. Read its
two halves **as a pair**: a `scene` pass that grew with occupancy unmoved means shading got more
expensive, one that grew alongside `total_refs` means the binning got looser — the diagnosis that
otherwise needs a GPU capture (`ludion devdocs renderer-clustering`). Occupancy is the tables
themselves, never a sample, so two runs of one frame agree exactly.

**The `local_shadows` and `sun_cascades` sections.** Two more offline-only planning shapes, not timings,
so two runs of one frame agree exactly: which local lights the shadow budget granted
(`ludion docs local-shadows`) and the sun's fitted cascades with their casters (`ludion docs sun-cascades`).

**The `load` section.** A report may also carry `LoadProfile`: wall ms for the scene's load, split into
`scene`, `docs`, `mesh`, `texture` and `other` phases, the render phases `voxelize`, `renderer_init`,
`gpu_upload`, `gi_bake` and `first_frame`, plus a `total`, and per asset kind a decoded / reused /
bytes-read counter row (all eight kinds, fixed order). Offline only, like `cluster`: the offline driver
attaches a `LoadObserver` before the scene loads and closes the measurement with one rendered first
frame. The phases are **exclusive** self-times (a mesh's map decode counts in `texture`, not `mesh`;
`first_frame` is the frame's own remainder once the renderer's creation, uploads and bake are taken
out); `total` runs to the last phase's end and `total` minus the phases' sum is the unattributed share.
A render phase is absent when it never began (no adapter; `voxelize` and `gi_bake` need a `GiVolume`).
Ticks a game's `--frames N` runs between the scene load and the first frame sit in no phase, so they
count as unattributed. The counters are deterministic. A game's `--render-out` attaches the same
observer and prints the load as one stderr line.

**The `skin` section.** A report may also carry `SkinProfile`: the median wall time of extract's `Skin`
phase beside the medians of its five per-extract counters (instances; palettes sampled, reused and
shared; joints sampled). Offline only, like `cluster`, and absent for a scene with no skinned
instance. `profile_sim` attaches a `SkinClock` over the measured frames alone and restores the caller's
extract observer. Posing is parallel, so read `pose_median_ms` beside `sim.threads`.

**Offline: `ludion profile --scene <FILE> --frames N`.** Loads a scene, drives a warmup then N
measured frames, medians them. It links no game code, so `sim.systems` holds only engine-registered
systems (the physics triple for a scene with a `physics:` block, otherwise empty) — it measures the
*engine's* frame, which is what a budget gate on a fixed scene wants. Add `--scale` to
supersample (a different measurement, not a noisier one), `--json` to write the report, `--check
<budget.toml>` to gate it.

**Tails and the replay variant.** Every offline `SystemTiming` (systems, sequential costs) and
`SimProfile::frame_total_tail` carries a `TimingTail` — `max_ms`, `max_frame` (the produced-frame
index, `engine.frame()` after the tick) and nearest-rank `p99_ms` (`ceil(0.99·n)`-th smallest) — beside
the median; a live capture leaves the sim ones `None`. `gpu.frame_tail` is the exception: the live
GPU column exists at window close, so a live window fills it (`max_frame` is the produced-frame index),
and `gpu_frame_max_ms` works on `--attach`. A game's `--profile-replay <FILE> [--from N] [--to M]`
(`ludion_engine::profile_replay`) drives a committed `.lrp` through the shared replay loop with a
`DriveHook` that overrides `tick`: ticks before `from` run plain (`warmup_frames`), ticks in the window
run through `Engine::step_ticks_observed` with the timing observer — hash-neutral, asserted by
`replay_profile_is_hash_neutral` — and the first tick past `to` ends the drive. Sim only (`gpu`
unavailable, no `cluster`); the per-frame series is the report's `samples`. Budgets gate the tails with
`frame_total_max_ms` and `[systems_max]` (`L4005`, statistic `max`, frame named), skipped when the
report has no tail. `gpu_frame_ms` / `gpu_frame_max_ms` gate `gpu.frame_median_ms` / `frame_tail`; a
report with no span is `L4019`, not a skip. `load_total_ms` gates the one-sample `load.total_ms`, which only a fresh offline or
game `--profile` run carries; a report without it is `L4025`, not a skip.

**Live: the `profile` channel and `profile.capture`.** Subscribing to `Channel::Profile` arms the
engine; every `PROFILE_WINDOW_FRAMES` (60) sim frames a window closes and is published as
`Event::ProfileWindow` — ~1 Hz at the default tick rate, on the ordinary ordered event lane. A
window carries **both** the medians (`*_median_ms`) and columnar per-frame `ProfileSamples`:
medians answer *what does a frame cost*, samples answer *when did it spike*, and a timeline can
only be drawn from the latter. `profile.capture {frames, include_samples}` is the same machinery
armed for one window of a caller-chosen length, answered as a `ResponseBody::ProfileReport`.

**The deferred reply.** A capture measures frames that have not run yet, so `run_live`'s `reply`
hands the request's one-shot channel back as a `PendingCapture` the loop holds, and
`flush_pending_capture` answers it when the window closes. The client sees one ordinary response to
its request id, just later. Exactly one capture may be outstanding (`L4013` otherwise), and a frame
count outside `1..=MAX_CAPTURE_FRAMES` is `L4012`. A capture that cannot close is `L4023`: refused
at arming when the sim is paused or the window does not fit `MAX_CAPTURE_WALL_SECONDS` at the
current speed and tick rate, and — via `abandon_unclosable_capture`, after the flush — answered and
cancelled when a `sim.pause`, a system panic or a `sim.set_speed` makes it unclosable mid-flight
(re-checked only on a run-state or speed transition, so a host that is merely late is not
cancelled), so the next capture arms. The wall bound is half the server's reply budget, so an
accepted capture answers before the transport would call the sim stalled (`L3023`) on a host that
keeps its tick rate.

**Two capture surfaces over the wire.** `ludion profile --attach <ws-url>` issues one
`profile.capture` against a running host — this is the report that carries the *game's own*
systems, which the offline driver structurally cannot see. The editor's Performance view subscribes
to the channel for its scrolling timeline and issues captures for a deep dive. Both feed the same
`--check` budget path and the same Perfetto export.

**Perfetto export.** `chrome_trace` (Rust, `--perfetto <out>`) and `editor/app/src/perf/trace.ts`
build a Chrome trace-event JSON from a report's samples, by one shared timeline rule: a frame's
origin is the cumulative sum of earlier frames' `frame_total + extract + submit`; sim spans
butt-join within their frame (the schedule is strictly sequential); GPU and transport lanes are
monotonic, so a lane that cannot keep up reads as a queue instead of as overlapping spans. It is a
picture of the *work*, not of the wall clock — inter-frame sleep is not represented. The GPU lane is
schematic: it butt-joins per-pass durations, never stamps, so it shows no overlap or gaps; the frame
cost is `gpu.frame_median_ms`.

## Gotchas

- **A capture blocks its own connection for the window's duration.** The reply is deferred, so
  every other call on *that* connection waits behind it. Issue a capture on a connection of its own
  — the editor's Performance view opens a short-lived `ProtocolClient` for exactly this reason, and
  an MCP-attached agent should prefer `ludion profile --attach` over an in-session capture.
- **A paused or too-slow sim cannot complete a capture window.** Windows close on sim frames, not
  on a timer, so such a capture is refused or cancelled with `L4023` (help: resume, capture, pause
  again; raise the speed; fewer frames). The rolling `profile` stream just emits nothing while
  paused — that is the expected shape. A host that renders slower than its tick rate is not
  cancelled: its capture answers late, and only a host slower than half its tick rate overruns the
  reply budget (`L3023`).
- **Offline reports have no game `sim.systems`.** The driver links no game code, so only
  engine-registered systems appear (the physics triple, for a `physics:` scene). If you want a
  game's systems, `--attach`.
- **Offline reports also have no GPU or `extract`/`submit` sample columns.** `profile_gpu` reduces
  each pass to one median by design, and the offline driver's `extract` is already reported as a
  different measurement inside `sim.sequential`. `--scene --perfetto` works; those two lanes are
  simply absent from it.
- **GPU samples arrive 1–3 frames late** and are filed against the frame they measured, never the
  frame that collected them. Without `TIMESTAMP_QUERY` the ring allocates nothing and every pass
  reports `None` — degradation, never an error (`timestamps_supported()` says which).
- **Medians hide spikes by construction.** A window's median is the honest answer to "what does a
  frame cost" and the wrong tool for "why did it hitch" — that is what `include_samples`, the
  trace export and, offline, the `tail` (`max_ms`/`max_frame`/`p99_ms`) of a `--profile-replay` are for.
