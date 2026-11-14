# Audit: the render graph, frame extraction, and frame streaming (2026-10-03)

**Scope.** Report only, at rev `24613ff7`. Covered: `crates/ludion-render-graph` (`graph.rs`,
`resource.rs`, `schedule.rs`, `execute.rs`, `readback.rs`, `present.rs`, `timings.rs`,
`pass_timing.rs`, `dump.rs`); `crates/ludion-renderer` (`graph.rs` frame-graph construction and
`FrameSize`, and in `render.rs` the `Renderer` entry points, `prepare_frame`, the `FrameRecorder`
pass recorders and the sprite/particle sorts); `crates/ludion-engine` (`extract.rs`'s asset path,
`engine.rs`'s `render_frame` / `render_png_through` / `attach_surface` / ring lifecycle, `live.rs`'s
`run_live` / `plan_egress` / `emit_egress`, `render_size.rs`, `commands/render.rs`); the in-process
windowed host `crates/ludion/src/present.rs`; the out-of-process viewer `crates/ludion-window`
(`gpu.rs`, `frame.rs`); and the golden gate in `crates/ludion-testkit` (`golden.rs`, `ssim.rs`,
`report.rs`, `image.rs`), measured against every committed per-OS golden set. Docs read:
`engine-architecture.md` §1/§3, `dev-docs/ludion-render-graph.md`, `frame-streaming.md`,
`engine-present.md`, `ludion-testkit.md`. The wire-side WebSocket size limits are
`2026-10-03-wire-server.md` W1 and are not repeated here.

## Overall assessment

The declarative core is solid. Scheduling, lifetimes and transient aliasing are correct and
deterministic. The seam rule holds: no wgpu type appears in the public declaration API. Row-padded
readback, the sRGB chain and the GPU fault sink are sound, and two items from earlier audits are now
fixed (C1's extract deep-clone; the depth target that used to live outside the graph). The real
weakness is the **golden gate**. It scores luma-only SSIM over 64 single-window tiles and does not
gate on MAE. A fully **grayscale** render passes all 54 Windows goldens, and erasing the most detailed
16×16 region passes 31 of 54. That is a false-green tier sitting under every renderer change. The
live path has three robustness gaps:
- a NaN sprite depth can panic the sim thread;
- a lost device or dead surface freezes a game window with no signal to the host;
- the readback ring locks at full after one GPU hitch, which adds two frames of latency for good.

## High

### G1 — The golden gate is colour-blind: SSIM runs on luma only and MAE is never gated · High · [verified]

**Evidence.** `crates/ludion-testkit/src/ssim.rs:23-28` and `:184-190`: the comparison reduces each
pixel to Rec. 709 luma before any statistic. `crates/ludion-testkit/src/report.rs:138-139`:
`pass = global_ok && floor_breach.is_none()`. `mae` is computed (`ssim.rs:147`) and written to the
report (`report.rs:211`) but plays no part in `pass`. The module doc says outright that chroma and
alpha are left to MAE (`ssim.rs:25-28`), and MAE is not gated.

**Measured.** A faithful port of `ssim::compare` (same tile bounds, flat-window formula, constants
and luma weights) was run against every committed Windows golden (`examples/gallery`,
`games/vigil`, `games/sokoban`; 54 images):
- **A full grayscale conversion** of each golden (RGB replaced by its own luma) **passes all 54**.
  Global SSIM is ≥ 0.9985 and the worst tile ≥ 0.997, against a gate of 0.99 / 0.95. MAE is 2–11.
- **An R↔B channel swap** (the classic RGBA/BGRA mix-up) **passes 39 of 54**, among them `lights`,
  `night-sky`, `material-park`, `sun-shadows`, `ui-hud`, `text` and all four `district-plaza*`.
  MAE reaches 10–18 on these and nothing reads it.

**Why it matters.** The goldens are the perceptual tier of tiered determinism. Most render
regressions that matter visually change colour, not structure: a broken saturation or
temperature/tint grade, a palette or material-colour bug, team colours swapped, a sky or fog colour
regression, a wrong channel order. None of them fails a golden. An agent iterating on the post
stack gets a green suite for a desaturated game.

**Fix.** Gate on colour as well as structure. Two simple options: compute SSIM per channel (or on
luma plus two chroma planes) and take the minimum; or gate MAE / per-tile max-abs-diff with a
calibrated ceiling, since same-GPU goldens are near bit-exact. Add a regression test that a
grayscaled golden fails.

## Medium

### G2 — One flat SSIM window per 1/64 of the image hides small vanished elements · Medium · [verified]

**Evidence.** `crates/ludion-testkit/src/ssim.rs:9-21`: each tile of the 8×8 grid is a single
unweighted SSIM window, about 120×68 px at the 960×540 default (240×135 px for the `*-1080`
goldens). `golden.rs:42-47`: the gate is global ≥ 0.99, worst tile ≥ 0.95.

**Measured** with the same port: in each golden, the 16×16 block of highest luma variance was
flattened to its mean colour (the element is gone).

| Block size | Goldens that still pass |
|---|---|
| 8×8 | 50/54 |
| 16×16 | 31/54 |
| 24×24 | 15/54 |
| 32×32 | 2/54 |

**Why it matters.** A 16×16 element at 960×540 is a HUD icon, a bullet, a pickup, a glyph run or a
small character. With one window per tile, the tile's mean and variance barely move when a 256-px
patch disappears among about 8,000 px. The tile floor added for exactly this case (`golden.rs:43-46`)
only catches elements of about 24 px and up.

**Fix.** Use the textbook sliding-window SSIM (an 8×8 or 11×11 Gaussian window), take the tile score
as the minimum or a low percentile of its windows, and keep the 8×8 grid only to localize damage in
the report. Alternatively, gate a per-tile max-abs-diff alongside SSIM.

### G3 — The sprite back-to-front sort is not a total order; a NaN sprite depth can panic the sim thread · Medium · [verified]

**Evidence.** `crates/ludion-renderer/src/render.rs:1443-1453` sorts with
`da.partial_cmp(&db).unwrap_or(Ordering::Equal)`. The particle sort beside it was hardened for this
exact reason, and its comment explains the hazard (`render.rs:2116-2127`: "intransitive, and
`sort_by` is entitled to panic on that"). The sprite sort was not. No sprite or `Transform` path
rejects non-finite positions: `extract.rs` has no `is_finite` check on placement, and
`components.rs` only validates `DirectionalLight.direction`. Stream renders run in `emit_egress`
(`crates/ludion-engine/src/live.rs:627`), which is not under the `catch_unwind` that protects command
execution (`live.rs:334`).

**Verified.** A throwaway program with the identical comparator (layer, then partial-cmp depth with
an `Equal` fallback, then index) on rustc 1.94 panicked in **314 of 600** sorts that mixed NaN and
finite depths.

**Failure scenario.** A game system normalizes a zero vector, or divides by a zero distance, and one
sprite's position becomes NaN. The next streamed or windowed frame panics on the sim thread, killing
the session. The agent gets a dead server instead of an `L`-coded diagnostic.

**Fix.** Use `da.total_cmp(&db)` as `particle_order` does, and add the same NaN test. Optionally
report a non-finite placement as a diagnostic from extract.

### G4 — A lost device or a dead window surface freezes a windowed game with no signal to the host · Medium · [verified]

**Evidence.**
- `Gpu::take_fault` makes `L2011` sticky (`crates/ludion-render-graph/src/execute.rs:266-278`).
  `PresentTarget::present` returns `L2007` on a surface validation failure (`present.rs:271-275`).
- Both surface in `emit_egress` and are only pushed as a one-time `Event::Log` on the `log` channel.
  The frame stays dirty and the sim keeps ticking (`live.rs:664-681`, which notes "ending it is the
  window host's call").
- The windowed host's `App::fail` is reached only from `start()` failures
  (`crates/ludion/src/present.rs:244-250`, `:272-275`). Nothing in the host observes a runtime
  render failure.
- The render-graph doc says the opposite: "a dead surface must end the host, not paint black
  forever" (`dev-docs/ludion-render-graph.md:59`, echoed at `present.rs:250-251`).

**Failure scenario.** The reference box's known TDRs, a driver reset or a removed eGPU during a
`ludion run --window` session (no wire subscriber) leave a frozen window. The game still simulates
and still consumes input, and nothing reaches stderr. Each later pass also re-extracts the whole
scene before `guarded` refuses it.

**Fix.** Give the sim thread a way to report a fatal render fault to the host, for example a fatal
slot that `Embedded::ended` already watches. End a windowed host on `L2011`/`L2007` with the
diagnostic printed. Latch the no-render state (as `gpu_unavailable` already does for `L2003`) so a
dead device stops paying for extract. Align the two docs.

### G5 — The readback ring latches full after one GPU hitch, adding two frames of stream latency until a pass skips submitting · Medium · [verified]

**Evidence.** `crates/ludion-render-graph/src/readback.rs:673-680` and `:746-750`. When the ring is
full, `render_frame_unguarded` block-collects only the oldest slot (`collect_oldest_blocking`, whose
`poll(wait_indefinitely)` waits for all in-flight work), submits the new frame, and returns the freed
frame without the usual `try_collect`. Pending is back to 3 on every such pass. `run_live` runs one
pass per tick when no commands arrive (`live.rs:278-283`), and every tick is a submitting pass, so
nothing ever drains the ring below full.

**Trigger** (traced, not measured on a GPU): two consecutive passes where the previous frame has not
finished at the non-blocking poll. Typical causes are a first-frame GI bake, pipeline creation, a
shader reload, or any GPU frame longer than a tick.

**Impact.**
- From then on each delivered frame is 3 ticks old instead of 1 (about +33 ms at 60 Hz).
- Each pass waits on the GPU, although the two newer frames it waited for are already complete.
- The ring recovers only when a pass does not submit, such as a mouse-move between ticks or a full
  egress slot. An agent or editor watching a running world without input stays latched.
- `dev-docs/frame-streaming.md:157-158`'s "~2 frames" cap is the latched steady state, not a
  transient worst case. `LUDION_SUBMIT_TRACE`'s `stalls` count would show it.

**Fix.** After the blocking collect, drain every other completed slot and deliver only the newest
(latest-wins inside the ring, as at every other stage). Alternatively, on the full path, wait only
for the oldest slot's map and then run the normal `try_collect`. Add a pure test that drives the
ring's bookkeeping through a stall and asserts pending returns below `DEPTH`.

## Low

### G6 — The wire `render.dump_graph` reports the unmasked chain, so at lowered quality it shows passes and extents that do not run · Low · [verified]

**Evidence.** `crates/ludion-engine/src/commands/render.rs:245-254` calls
`PostChain::for_scene(&scene)` with no `.masked(...)`. The CLI path masks
(`crates/ludion-engine/src/render.rs:288-291`), as do all three renderer entry points
(`ludion-renderer/src/render.rs:726`, `:830`, `:878`). The handler's doc claims "the reported shape
is the one a screenshot of this world would actually schedule".

**Impact.** On a machine with `LUDION_QUALITY` lowered (shadows, SSAO, shafts, DoF or `gi_dynamic`
off; a smaller `shadow_map_size`; a coarser GI lattice), an agent introspecting over the wire sees
passes and extents that never execute.

**Fix.** Mask with `engine.quality()` exactly as `dump_scene_graph_json` does.

### G7 — Extract copies every referenced sprite, atlas and UI texture's full RGBA every frame, though the renderer only reads it on a cache miss · Low · [verified]

**Evidence.** `crates/ludion-engine/src/extract.rs:171-173` (`self.cache.load(..).cloned()` clones an
`Image` with a `Vec<u8>`) and `:2749` (`asset.image.rgba.clone()` for atlases). The renderer keys its
texture cache on `(hash, premultiplied)` and reads `t.rgba` only inside `or_insert_with`
(`ludion-renderer/src/render.rs:1338-1355`). Meshes already moved to `Arc<MeshData>` for this reason
(`extract.rs:161-163`).

**Impact.** A memcpy and allocation per unique texture per frame: 4 MB for one 1024² atlas, 16 MB
for one 2048² atlas. This is not C7 (GPU buffers) and not C1 (component clones).

**Fix.** Make `TextureImage.rgba` an `Arc<[u8]>` (or `Arc<Image>`) shared with the asset cache, as
`MeshData` is.

### G8 — Two drifted copies of the window blit: the viewer panics where the in-process presenter returns `L2007`, and a comment claims parity that is not there · Low · [verified]

**Evidence.** `crates/ludion-window/src/gpu.rs` and `crates/ludion-render-graph/src/present.rs` each
carry their own `BLIT_WGSL`, sampler, bind-group layout, format choice and adapter request.

**Drift.**
- The viewer indexes `caps.formats[0]` and `caps.alpha_modes[0]` (`gpu.rs:132`, `:149`), which
  panics on empty capabilities; `present.rs:188-195` returns `L2007` instead.
- The viewer requests `adapter.limits()` with no software-adapter retry (`gpu.rs:99-118`), unlike the
  engine's one adapter policy (`execute.rs:151-212`).
- `present.rs:286-287` says the bind group is "built per frame, as the out-of-process viewer's blit
  does". The viewer builds it once per frame size (`gpu.rs:264-270`, `:294-323`).

**Fix.** Share the blit pipeline and format choice in one place that both crates can depend on, and
correct the comment.

### G9 — Stale docs and comments on the frame path · Low · [verified]

- `engine-architecture.md:131-140` still describes a separate render thread and a "latest-wins slot
  (triple buffer)" sim→render handoff. Rendering runs on the sim thread inside `emit_egress`, and
  `present.rs:203-204` says so.
- `crates/ludion-engine/src/engine.rs:760` and `dev-docs/frame-streaming.md:82` name
  `Engine::render_streamed_rgba`, which no longer exists. It is now `Engine::render_frame`.
- `crates/ludion-renderer/src/render.rs:2399` contrasts the shadow pass with "the scene pass's
  discard-on-store main depth". The scene pass stores its depth (`render.rs:2672-2678`) because
  `composite` samples it.

## Checked and healthy

- **Scheduling and aliasing.** Kahn order with a declaration-order tie-break, deduplicated
  producer edges, unwritten-read and self-read rejection, inclusive lifetimes with a strict `<` reuse
  test, outputs and untouched resources never aliased, and format/extent/slice-count compatibility.
  Depth never aliases colour and volumes never alias 2D. Checked against every resource
  `build_frame_graph` declares (bloom's long-lived `bloom_bright`, `scene_depth` living to
  `composite`/`overlay`, conditional shadow, AO, shafts, DoF and GI chains).
- **Seam rule.** `graph`, `resource`, `schedule` and `dump` expose no wgpu type; the `to_wgpu`
  mappings are `pub(crate)`.
- **Prior-audit items now fixed.** The depth target is a graph resource (`scene_depth`,
  `shadow_map`, `prepass_depth`), and `MAX_LIGHTS` is gone with clustered lighting. C1 is fixed
  (`place()` borrows). C7 (per-frame GPU buffers) remains as the prior audit deferred it.
- **`PhysicalCache` reuse.** Keyed on the whole `Aliasing`. Every recorder clears or covers its
  target whole (`fullscreen_effect` clears, shadow, prepass and scene clear, `gi_inject` writes the
  whole volume, the overlay's `Load` sits under a full blit).
- **Readback.** 256-byte row padding and unpadding, the blocking path for screenshots and goldens,
  ring slot round-robin invariants, `discard_pending` replacing rather than unmapping in-flight
  buffers, and stamps carried through so a frame reports the index it depicts.
- **Colour and alpha.** `Rgba8UnormSrgb` output, PNG bytes equal to readback bytes, the present
  path decoding and re-encoding through an sRGB view, and every committed golden fully opaque (alpha
  checked on all per-OS sets).
- **Extents.** `FrameSize::validate` bounds every axis to 1..=8192 (`L2012`). `render.size` and
  `render.screenshot` refuse zero or 8K+ (`L3025`). A minimized window skips presenting, and
  `resample()`'s mixed-axis assert is unreachable under `max(1)` rounding. Extent changes apply at
  frame boundaries on the sim thread and rebuild the ring.
- **Fault policy.** Every submitting `Gpu` method is `guarded`, `L2011` is sticky, the validation
  sink is capped, and `L2003` latches until a new subscription.
- **Dumps and timings.** Byte-stable plain-struct JSON. Timestamp indices are per graph pass, every
  recorder attaches its pair exactly once, and adapters without timestamps get the fallback shape.
- **Mesh cache key** folds in the material texture hashes (`mesh_asset.rs:72`), so an edited map
  invalidates the cached bind group.
- **No temporal accumulation** exists in the renderer, so the test-mode rule has nothing to disable.
- **Per-OS golden-set gaps** (Windows lacks `ui-fonts*.png` and breakout `menu.png`; macOS lacks
  `mesh-grid.png`) are already tracked in the owed ledger and are not re-reported.

## Suggested order of work

1. **G1**, then **G2**: make the gate see colour and small elements, add regression tests
   (grayscale fails; a vanished 16×16 block fails), and re-check that the current goldens still pass
   on their pinned GPUs.
2. **G3**: a one-line `total_cmp` fix plus a NaN test; it removes a sim-thread panic.
3. **G4**: a fatal render-fault path to the windowed host, and latch the no-render state.
4. **G5**: latest-wins drain inside the ring, with a bookkeeping test.
5. **G6**, **G7**, **G8**, **G9**: small and independent; batch them.
