# Plan: Display-paced present — a presenter thread, then interpolation between ticks

> Status: **mechanized (2026-11-01)** — every wave shipped, gates green; closes when its owed live checks pass (`Plan/owed.md`: windowed smoke on both pinned boxes, a 120 Hz Mac trace and look, the iOS M3 stall re-read). 01 presenter module + engine wiring r1898–r1899 (test de-flake r1901); 02 blend r1900, engine blend data r1902; 03 presenter interpolates + `present_interpolation` r1903; 04 particles/bullets by instance key r1904; 05 review fixes r1905. Trigger:
> Clockout request 0026 (`engine-requests/0026-sim-presentation-boundary-display-paced-present.md`,
> priority *painful*). Code facts read from the tree on 2026-11-01 (file:line pointers are of that
> date; re-grep before editing).

## Trigger

A windowed game draws one image per 60 Hz tick, on the sim thread, presented with `AutoNoVsync`.
On macOS that is `Immediate` (a tear line); on a 60 Hz panel the wall-clock deadline drifts against
vblank (a doubled or skipped tick every few seconds); on a 120 Hz panel each tick lasts one, two or
three refreshes, unevenly. Clockout's camera rig moves the whole frame every tick, so every one of
these shows. On iOS, `AutoNoVsync` resolves to `Fifo` and the sim thread itself waits in
`nextDrawable` (`Plan/ios-target.md` §Review 3, Risks "Present pacing under Fifo").

## What exists (2026-11-01)

- `live_loop` (`crates/ludion-engine/src/live.rs:296`) is the one wall-clock site: it ticks when
  `Instant::now() >= next_tick`, re-bases on a whole-period lag (`:367`), then `emit_egress` (`:829`)
  renders **and presents on the sim thread**. `plan_egress` (`:758`) presents a frame at most once
  (`presented_already`), independently of the stream's subscriber and slot.
- `Engine::render_frame` (`engine.rs:3437`) extracts once (`extract_render_scene_through(.., true)`,
  the view override included) and calls `Renderer::render_frame(&scene, size, ring, frame, timing,
  FrameSinks { readback, present })`; the present blit reads the graph's output texture out of the
  stream ring's physical textures (`ludion-render-graph/src/readback.rs:787-800`).
- `Engine::attach_surface` (`engine.rs:3821`) requests a `Gpu` for the surface, builds the one
  `Renderer` on it and a `PresentTarget`; it must run before the first frame. The renderer is
  `Send`, not `Sync` (`RefCell` caches, `ludion-renderer/src/render.rs:426`).
- `PresentTarget` (`ludion-render-graph/src/present.rs`) is already `Send` and already
  cross-thread on size: `PresentSize` atomics carry the window's size in and the presented frame's
  size out (cursor mapping). Resize of the *render extent* (0023) is a `render.size` command the
  host sends (`crates/ludion/src/embed.rs:559`), so it lands in `Engine::render_size` on the sim thread.
- Faults: `absorb_render_failure` (`live.rs:932`) latches no-render on `L2007`/`L2011` and reports
  once through `Engine::report_render_fault` to the host's armed channel.
- The renderer crates carry no wall-clock denylist (`ludion-renderer/clippy.toml`); `ludion-engine`
  bans `Instant::now` outside `run_live`.
- wgpu 30: `Device`, `Queue`, `Adapter` are `Clone`.

## Decisions

1. **A presenter thread owns the window and its own renderer.** `attach_surface` spawns a
   `Presenter` (new, `ludion-renderer/src/presenter.rs`) that owns a `Renderer`, a private
   `StreamReadback` (used only for its physical textures, never a readback) and the
   `PresentTarget`. The sim thread keeps its own lazily-built `Renderer` for every verification
   path — `render.screenshot`, the frame stream, the live profiler, probe bake, dumps — exactly as
   a headless host has it today. **Why:** the verification path stays the untouched code, so
   goldens, screenshot, `--render-out` and the stream are byte-identical by construction; there is
   no lock on any renderer, so a 120 Hz presenter never contends with the sim thread; and a shipped
   windowed game with no subscriber never builds the second renderer at all. **Cost, accepted:** a
   windowed *dev* session that also streams or screenshots holds two sets of GPU caches and runs GI
   twice; each renderer's GI history converges on its own (render-only state). Rejected:
   `Arc<Mutex<Renderer>>` shared by both threads (32 call sites move to a guard, and part 2's
   per-refresh renders would contend with every stream frame); a render-service thread owning the
   one renderer (screenshot and every lockstep path become cross-thread requests — a rewrite).
2. **One device, shared.** `Gpu::share(&self) -> Result<Gpu, Diagnostic>` builds a second `Gpu` over
   clones of the same device/queue/adapter and **the same fault sink** (wgpu keeps one
   uncaptured-error handler per device; a second install would steal the first's faults). The
   sim-side lazy renderer is built on the surface's device when one is attached (today it is the
   same device), and on `Gpu::request()` otherwise.
3. **The handover is immutable extracted data, latest-wins.** After a tick, when a window is
   attached and this frame has not been handed over (today's `presented_already` rule, unchanged),
   the sim thread extracts once and publishes a `PresentPacket { frame, scene: Arc<RenderScene>,
   size: FrameSize, settings }` into a one-slot mailbox (`Mutex<Option<PresentPacket>>`, held only
   for a pointer swap, plus a wake). The packet also carries a `TickStamp { at: Instant, period,
   rebased: bool }` from the live loop (decision 13). `settings` is the render-side state the engine pushes into its
   renderer per frame today (`ensure_renderer`: quality + params, anti-aliasing, GI hold, probe
   store); the presenter applies it when it differs. If the same pass also files a stream readback,
   that render reuses the same `Arc<RenderScene>` — one extract per pass, as today. The presenter
   never sees `World`, `Engine` or a command; the sim never waits on the presenter. Part 2's tick
   timestamp rides the same packet. **Why extracted data, not a finished frame:** part 2 must draw
   between ticks, which needs scene data on the presenter; a finished-frame handover would have to
   be redone.
4. **Present mode `Fifo`, everywhere.** The presenter's `PresentTarget` is configured `Fifo`
   (`present.rs:158`); `desired_maximum_frame_latency` stays 2. **Why not `Mailbox`:** Mailbox never
   blocks, so the presenter would have no pacing signal — it would spin, rendering frames the
   compositor discards, and part 2's one-render-per-refresh contract would need its own vsync
   estimate. Fifo is the one mode wgpu guarantees on every backend (and the only one on iOS), it
   never tears, and blocking in acquire *is* the display clock. Its cost is up to one extra refresh
   of latency; revisit (FifoRelaxed, latency 1) only on a measured complaint. `AutoNoVsync` and its
   rationale (`dev-docs/render-graph-gpu.md:27`) are retired: the presenter no longer runs on the
   sim thread.
5. **The presenter loop.** Each iteration: take the newest packet if one arrived; if it is new,
   render it (into the presenter's own graph textures) — otherwise reuse the last output; then
   acquire (blocks at display rate under Fifo), blit, present. So a 120 Hz panel gets 120 presents
   a second carrying 60 distinct images, each shown for exactly two refreshes, and the sim's
   deadline no longer decides which refresh a tick lands on. Re-presenting is a blit, not a
   re-render (GI and other per-frame render state advance once per tick, as today). Minimized
   (`PresentSize` 0×0) or before the first packet: park on the mailbox with a short timeout, never
   spin. Part 2 replaces "render if new" with "render every iteration at alpha".
6. **Resize.** Unchanged in shape: the window thread writes `PresentSize`, the presenter reads it
   before acquiring and reconfigures (it already lives where the surface is used). The render
   extent still moves by `render.size` on the sim thread and reaches the presenter as the packet's
   `FrameSize`; the presenter rebuilds its private ring when the size moves, as `Engine::render_frame`
   does today.
7. **Faults and notes flow back on one channel.** The presenter sends `PresenterNote`s — a fault
   (`L2007` dead surface, `L2011` lost device; a cleared `L2010` is a log note) and its renderer's probe resolve — on a channel
   the live loop drains in step 4 beside `take_queued_render_fault`. A fault goes through the
   existing `absorb_render_failure` (latch, report once to the armed host, log) and the presenter
   stops presenting; the probe resolve feeds `finish_probe_frame`, so a window-only host keeps its
   `L1279` warning. Shader hot reload (dev only) applies to the sim renderer as today and is
   forwarded to the presenter; a presenter-side reload failure comes back as a note and is logged.
8. **Lifetime.** The `Presenter` handle lives in `Engine.present` (replacing `PresentTarget` there);
   dropping it closes the mailbox and joins the thread, so the surface still drops before the host
   drops the window (the host joins the sim thread first, as now). No presenter exists without a
   surface: every headless host, test, golden and lockstep path is unchanged.
9. **Observability.** `LUDION_PRESENT_TRACE=1` (read once, the `LUDION_TICK_TRACE` discipline) prints
   a summary every 600 presents: presents/s, distinct frames shown, repeats, frame-index gaps
   (ticks never shown), acquire-wait percentiles. It is the instrument for the acceptance capture.
   **Profiling:** a windowed host's present cost leaves the sim thread; the live profiler,
   `profile` and `--dump-timings` keep measuring the sim-side renderer (an armed profiler renders
   there for measurement, as a headless host does). The presenter is unmeasured beyond its trace
   (`dev-docs/profiling.md` says so).
10. **Part 2 — interpolation, render-side only** (the owner's rulings, restated so part 1 is built
    for them): opt-in `EngineBuilder::present_interpolation(true)`, off = byte-identical to part 1;
    interpolation only, never extrapolation; `alpha = (now − t_tick) / period` clamped to `[0, 1]`,
    blending tick n−1 → n, so the window shows the world up to one tick late. Only poses and the
    camera (pose, FOV) blend — lerp translation/scale, slerp rotation (via `ludion_core::math`'s
    fixed-order rotation, though it never touches hashed state); everything else takes the newer
    tick; animation, clip and skinned poses stay stepped (decision 15). **Snap** (no blend) when an
    entity has no pose in the previous packet (spawned), was teleported (`teleport_character`), sits
    under a rig `snap_rig` cut, or after a scene load/switch/restore — the engine sets these itself;
    a one-tick render-only `NoInterpolate` marker, cleared by the engine, covers a hand-written
    `Transform` teleport. Every verification sink renders at `alpha = 1` (the completed tick):
    they stay on the sim-side renderer, which never blends. Interpolation adds no hashed, recorded
    or snapshotted state; `Plan/character-controller.md` decision 11 ("render may interpolate
    presentation only") holds.
11. **Part 2's blend contract (to be confirmed at its research).** Extract publishes, beside the
    `RenderScene`, a pose table — each drawn entity's world affine — and tags every pose-derived
    item (mesh, sprite, voxel, light, decal, the camera) with its entity key, plus the tick's snap
    set. The presenter blends without re-deriving anything: for a keyed item with world matrix
    `M_n`, `M' = blend(W_{n−1}, W_n) · W_n⁻¹ · M_n`; a snapped or newly spawned entity takes `M_n`.
    The blend is a pure function `blend(prev, next, alpha) -> RenderScene` in `ludion-renderer`,
    GPU-free and unit-testable (the 0026 "alpha 0.5 lands at the midpoint" test). The
    `NoInterpolate` marker should be a non-registered, never-hashed component so the wire schema and
    every `.lrp` header stay put; if it must be registered, the schema re-pin and replay re-record
    are scheduled last.

12. **The presenter is the renderer crate's one wall-clock site.** `ludion-renderer` gains a
    `render` denylist tier (the clock group only: it keeps glam trig and std hash containers), and
    `presenter.rs` scopes the one `allow`, mirroring `run_live` in `ludion-engine`. Nothing the
    presenter reads from the clock reaches a packet back to the sim.
13. **A re-based deadline is an explicit snap.** When the live loop re-bases its deadline (a whole
    period behind, a speed change, a resume) the tick it then runs is stamped `rebased: true`; part
    2's presenter never blends across a re-based tick (it shows the newer tick at alpha 1). 01
    carries the stamp so 03 does not touch the tick block again; 01's trace counts rebases. A
    packet the presenter never takes (replaced in the mailbox) hands its snap set, and its `rebased`
    flag, to the packet that replaces it, so the presenter blends from the last tick it drew across
    the gap rather than snapping; the blend then spans the larger gap, still `(now − t_next) / period`.
    A gap in published frame numbers, or a replaced packet with no blend data, still snaps.
14. **Threads named.** The engine process gains a fourth thread in `engine-architecture.md` §3 — the
    presenter, render-only, owning no sim state. The thread-identity gate (`--full`) runs headless,
    so no presenter exists there; its module doc says so, and a live test pins that a presenter
    leaves the per-frame `WorldHash` sequence unchanged.
15. **Particles and bullets blend by stable instance key (correspondence).** Clip and skinned poses keep
    the newer tick because games step them deliberately (Clockout 0027); revisit if a game asks. The pose
    table holds stored transforms, so a prefab part's clip offset (like an assembly part's) takes the newer tick.
16. **Opt-in frame latency 1 (Serious Isaac 0004).** `EngineBuilder::low_latency(true)` sets the window
    surface's `desired_maximum_frame_latency` to 1 (default 2). Still `Fifo`: no Mailbox, no `FifoRelaxed`.
    This amends decision 4 (owner-approved 2026-11-06). The trace gains an input→present figure (host
    arrival stamp → `TickStamp.input` → first present of the frame), host-side only: never hashed,
    serialized or recorded. It ends at submission, so it cannot see the swapchain queue latency 1
    shortens; the photon capture is owed on the M4.

## Phasing

- **01 — the presenter thread (part 1).** Decisions 1–9: `Gpu::share` (render-graph), `Presenter` +
  `PresentPacket` + `PresenterNote` (renderer), `attach_surface`/`emit_egress`/`plan_egress`/fault
  drain (engine), `Fifo`. Tests: the packet types are `Send`; `plan_egress`'s table updated (a
  window now means "hand over", never a sim-thread render); a host-level decoupling test with a fake
  display backend that blocks ~50 ms per present (a 20 Hz panel) — the sim still ticks at ~60/s
  while the presenter delivers ~20/s, where today's coupling would cap the sim near 20 (a slow fake
  rather than a 120 Hz one: Windows sleep granularity is ~15.6 ms, so an 8 ms fake would flake);
  and the existing present/embed/egress tests green. Docs: `engine-architecture.md` §3 (a fourth
  thread; the windowed-mode paragraph), `dev-docs/render-graph-gpu.md` (the reversal),
  `dev-docs/engine-present.md`, `dev-docs/profiling.md`, `docs/desktop-window.md:51`,
  `Plan/owed.md`; the renderer's `render` denylist tier (decision 12); the tick stamp (decision 13);
  the thread-identity doc line and the live hash-neutrality test (decision 14). Crates:
  ludion-render-graph, ludion-renderer, ludion-engine. No wire, schema, hash, replay or golden move.
- **02 — extract publishes the blend data.** Decision 11's pose table, entity keys and snap set; the
  engine's snap sites; the `NoInterpolate` marker and its clearing; the pure `blend` and its unit
  tests (midpoint, snap on spawn/teleport, `WorldHash` identical on/off). Crates: ludion-engine
  (extract, motor, camera_rig, load/restore), ludion-renderer (scene types, blend). Re-sized at
  its own research; it splits again if the keyed-item change spans every instance type at once.
- **03 — the presenter interpolates.** The builder flag, the tick timestamp in the packet, render
  every refresh at alpha, animation/particle fractional time, the `Frame pacing` section in
  `docs presentation`, and the host-level trace acceptance (120 Hz present, 60 ticks/s) — the
  trace on a real 120 Hz panel is owed (below).

## Risks

- **Two renderers in a dev session** double asset VRAM while a windowed host also streams. Accepted
  (decision 1); measured by `profile` if a dev box ever runs short.
- **Fifo latency** adds up to a refresh of input-to-photon latency over today's immediate present.
  The trace measures acquire waits; the remedy is `desired_maximum_frame_latency: 1`, not Mailbox.
- **iOS** gets the presenter through the same `attach_surface`; the sim thread no longer waits in
  `nextDrawable`. Whether the M3 stall is gone is measured there (`LUDION_TICK_TRACE` +
  `LUDION_PRESENT_TRACE`).
- **Part 2's alpha clock** is the presenter's wall clock against the sim loop's tick instant; a
  re-based deadline (`live.rs:367`) is handled by decision 13.

## Owed (when 01 ships)

- A live windowed smoke on both pinned boxes (Windows 3090, macOS M4): no tear line, a resize
  follows, a dead surface still ends the host with `L2007`.
- A `LUDION_PRESENT_TRACE` capture on a 120 Hz Mac (the user's MBP is unpinned but fine for a look
  check): ~120 presents/s, ~60 distinct frames/s, no gaps.
- The iOS M3 stall measurement re-read against this design (`Plan/ios-target.md` Risks).
- CLAUDE.md's Active plans gains this plan (the Tech Lead's edit; CLAUDE.md carries foreign
  uncommitted edits today).

## Measurements

`low_latency` off vs on, input→present from `LUDION_PRESENT_TRACE=1` (decision 16). *Unpinned, indicative only*
(the M5, 120 Hz). The figure ends at submission, so expect little movement between off and on. Recipe, one human
sitting after Serious Isaac 0004 subtask 03:

1. Copy the throwaway probe (a mouse-look system as in `first_person_kick`, `low_latency` read from
   `PROBE_LOW_LATENCY`, `ludion::run`) into `crates/ludion/examples/latency_probe.rs`.
2. `cargo build --release -p ludion --example latency_probe`.
3. For each of `PROBE_LOW_LATENCY=0` and `=1`, run `LUDION_PRESENT_TRACE=1 target/release/examples/latency_probe
   --window --scene examples/gallery/camera-rig.yaml`, moving the mouse continuously for at least three trace windows.
4. Record the `input→present` p50/p95 and `frame latency` here, then delete the example.

- 2026-11-06, M5: the trace prints `frame latency 2` (off) and `1` (on) at a steady 120 presents/s, acquire+present
  p50 about 7.7 ms either way. The input→present figure is still owed: synthetic `CGEventPost` moves from
  `osascript` never reached the window (no input samples in either run), so the figures need a human at the mouse.

