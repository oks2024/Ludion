# Plan: Editor Viewport Latency on Low-Power Hardware

> Status: **PHASES 1a, 1b, 2, 2b MECHANIZED-COMPLETE (2026-08-16).** First implemented 2026-08-15,
> lost uncommitted to a drive failure, and redone from the surviving agent reports. Re-scoped by
> the user 2026-10-10: the original low-power laptop is retired, so Phase 0 (needs a human at the
> screen) re-runs on an unpinned dev machine; if the latency is fine there the plan closes, and
> Phase 3 (the dev opt-level decision) is built only on a measured problem. The Phase 2 fps-twin
> gate got its authoritative run on the RTX 3090 2026-09-07: Qoi 60.0 / Lz4Rgba 60.0 / RawRgba
> 46.6, 48.8, 50.2 over three runs against the 60.0 / 59.6 / 51.6 baseline — green, both codecs at
> the render ceiling, RAW inside its own spread. Outcomes recorded in the private frame-transport notebook.
> Written from a codebase investigation; every claim carries `file:line` evidence so a fresh instance
> can re-verify rather than trust.

## Trigger

User testing on a **low-power laptop**, running **`ludion dev` (debug profile)** at `--quality low`:
framerate is acceptable, but input-to-photon lag "keeps climbing" until the game is unplayable in the
editor viewport. Proposed remedy was switching the editor to a **direct framebuffer**.

Two user decisions taken at planning time (2026-08-13), which scope this plan:
- **The game must stay playable inside the editor viewport.** A native-window play affordance
  (`ludion window --attach` / `--window`) was offered as the cheap escape hatch and **rejected**.
- **Cheap, high-leverage work only.** The readback collect split and its Windows timer-resolution
  prerequisite are out. See Non-goals.

## Findings (the investigation, so it need not be repeated)

### 1. `--quality low` cannot reduce transport cost — by design

This finding alone explains the reported symptom.

- `Engine::set_render_scale` (`crates/ludion-engine/src/engine.rs:518`) renders at
  `render_size × scale` and **resamples back**, documented on `:516` as "so the images a client
  receives keep exactly the dimensions it asked for".
- `render_streamed_rgba` (`engine.rs:1531`, reading `self.render_size` at `:1535`) sizes the readback
  from that unchanged extent. The in-code comment at `:1543` states it outright: *"The ring is sized
  from the output extent — the render scale never changes how many bytes come back."*
- `with_render_size` (`engine.rs:505`) is the only setter, and it is called **only by tests and
  `ludion render`** (`grep -rn "with_render_size" crates/` — no hit in `ludion-cli/src/serve.rs` or
  `dev.rs`). There is **no protocol verb** to change it either.
- So every live session streams `DEFAULT_RENDER_SIZE = (1280, 720)` (`engine.rs:67`) at every quality
  preset. Quality lowers **GPU work only**.

Framerate up, latency unchanged is exactly what this predicts.

### 2. The per-frame memcpy bill is linear in pixels, and larger than the docs suggest

For each delivered `RAW_RGBA` frame the server copies the whole image at least **four** times beyond
the GPU readback:

1. `RowLayout::unpad` in `read_slot` (`crates/ludion-render-graph/src/readback.rs`)
2. `frame.rgba.to_vec()` inside `route_frame`'s `spawn_blocking` (`crates/ludion-server/src/delivery.rs`)
3. `FrameHeader::encode(&payload)` (`crates/ludion-protocol/src/frame.rs:134-151`) — allocates
   `header + payload` and copies again, **per subscriber**
4. tungstenite's own framing copy in `writer_task`

At 1280×720 that is ~3.69 MB × ≥4 ≈ **15 MB of memcpy per frame**; at 60 fps ≈ **0.9 GB/s** of pure
memory traffic on a machine with slow single-channel LPDDR. Every one of those terms scales with the
pixel count, and `--quality low` reduces none of them. This is the strongest quantitative argument
for Phase 1.

### 3. Where the rest of the latency is on a debug `ludion dev` run

Dominated by terms that scale with the **pass period**, not by transport:

- **Frame production.** The private frame-transport notebook records dev-mode vigil at **~22 fps**
  (`vigil`/`ludion-engine`/`ludion-renderer` at `opt-level = 1`; only `ludion` and `ludion-server`
  carry opt-3 overrides). One pass ≈ **45 ms**.
- **The readback ring's +1 frame is a *frame*, not 16.7 ms.** `Gpu::render_streamed`
  (`readback.rs:543-648`) calls `try_collect` microseconds after `queue.submit`, so it always pops
  the *previous* submission. At 60 fps that is 16.7 ms; **at 22 fps it is ~45 ms.** This is why a slow
  machine feels disproportionately worse than its fps number suggests.
- **Tick lateness.** `LUDION_TICK_TRACE` (`crates/ludion-engine/src/live.rs`, summary every 600 ticks)
  measured mean period 17.1 ms vs an intended 16.667; lateness p50 8.25 / p95 15.5 / max 39.2 ms —
  the ~15.6 ms Windows quantum signature. A laptop's C-states and EPP make this worse.
- **The shared outbound queue.** `OUTBOUND_CAPACITY = 32` (`delivery.rs:80`) carries replies, events
  *and* multi-MB binary frames in one FIFO drained by a single `writer_task`
  (`crates/ludion-server/src/connection.rs:410`, a plain `while let Some(..) = rx.recv().await`
  loop). An `input.inject` reply can sit behind up to 32 frames.
- **Transport itself is NOT the problem.** On loopback with `RAW_RGBA`, total server transport is
  ~2.5 ms and editor decode ~0 (the 2026-08-08 measured table).

### 4. "Keeps climbing" is a *residual*, not the known 2026-08-09 backlog

Both fixes from that arc are **already present in this tree** — verified:
- `PIPELINE_DEPTH = 64` with the split reader / reply-pump — `connection.rs:285, 316`.
- `createInjectCoalescer` wired into the viewport — `editor/app/src/viewer/Viewport.tsx:197`.

So the report is either a residual accumulator those did not cover, **or** lag plateauing so high
(~100–140 ms at 22 fps) that it reads as climbing. **These need different fixes — hence Phase 0.**
The frame-transport notebook's first open TODO ("Manual confirmation of the lag fix on a live dev
session") is precisely this measurement, still owed.

### 5. Direct framebuffer: already rejected twice, on record

- The private editor-rendering notebook — "Single-app/shared-framebuffer viewer rejected (violates parity
  gate + peer-client pillar)".
- The private frame-transport notebook — "Direct-swapchain presentation for `ludion-window`: evaluated,
  NOT adopted … stays a possible shipping-era latency feature."
- `crates/ludion-window` is explicitly "a peer client, **not** a GPU blit"
  (`dev-docs/ludion-window.md`); `CLAUDE.md`'s parity gate forbids editor-private endpoints.

Combined with finding 3 (transport ≈ 2.5 ms), it is a large architectural break for a non-dominant
term. **Not pursued.**

### 6. Three constraints that shape Phase 1 (verified, and each one killed an obvious design)

- **`ludion dev` forwards nothing to its child.** `try_launch_once`
  (`crates/ludion-cli/src/dev.rs:1291-1310`) builds argv of exactly `--addr`, `--scene`, `--paused`,
  `--dev-shaders`. `main.rs:823-828` documents this deliberately. **A new CLI flag alone therefore
  cannot reach a `ludion dev` session — only an environment variable can.**
- **`--width` / `--height` are already taken.** `RunArgs` (`crates/ludion/src/lib.rs:152-160`) binds
  both with `requires = "replay_frames"`, and `serve.rs` and `RunArgs` are documented mirrors. The
  new flag must be **`--render-size WxH`**.
- **A `stream.subscribe`-side downscale breaks picking.** `render.bounds` echoes
  `engine.render_size()` in its reply (`crates/ludion-engine/src/commands/render.rs:88`;
  `response.rs:154`), while the editor maps CSS→engine pixels through `canvas.width`
  (`editor/app/src/viewer/picking.ts`, `clientToEnginePoint` / `engineRectToCssRect`). Those agree
  **only because the streamed extent and `render_size` are the same number.** Per-subscriber
  downscaling silently mis-maps every gizmo hit-test and pick by the scale ratio. It also adds a
  full-frame CPU resample on the very machine that is short of CPU, while cutting none of the
  upstream cost (GPU render, `copy_texture_to_buffer`, `unpad` all still at 720p). **Rejected.**

### 7. A live extent change is nearly free at the engine layer

`render_streamed_rgba` already rebuilds the ring when the extent moves (`engine.rs:1543-1552`):
`ring.dimensions() != (width, height)` → `new_stream_readback(size)`. In-flight `map_async` readbacks
go with the dropped buffers (wgpu unmaps cleanly on drop), invisible under latest-wins. The ring owns
the frame graph's `PhysicalCache`, so a resize reallocates render targets once. **GI bakes live on the
`Renderer`, not the ring — a resize does not force a GI rebake.**

The client already handles mid-stream resize on all three paths: `useFrameStream.ts:185-190` (inline),
`:273-275` (offscreen), `:313`/`:323-325` (worker-blit), plus `mirrorPlaceholderSize` (`:150-165`)
which exists specifically for "a mid-stream resolution change". `ludion-window` letterboxes
(`crates/ludion-window/src/letterbox.rs`). **Nothing downstream needs changing to accept a new extent.**

---

## Phase 0 — Measure (hard gate, no code, ~half a sitting)

Everything needed exists. Run on **an unpinned dev machine**, in order; stop at the first that answers.

### 0.1 The overlay, 30 seconds of mouse-waving

`ludion dev` + editor attached, play mode, viewport focused. Read the overlay
`{frameMs} ms · {fps} fps · lag L · q Q · dec D · duty U` (`Viewport.tsx:1010-1016`, formatted by
`formatLag` in `editor/app/src/viewer/lagProbe.ts:354`). Four-way discriminator:

| Reading | Diagnosis | Go to |
|---|---|---|
| `lag` **climbs monotonically** toward the 2000 ms censor | a backlog is still forming; the 2026-08-09 fix did not hold here | **STOP** — find the queue first (§0.3); Phase 2 is the prime suspect |
| `lag` **plateaus** at 3–6× the frame period, `fps` low, `duty` ≪ 0.3, `dec` small | frame production/transport too slow; the client is idle, staleness is upstream | **Phase 1**, then Phase 3 |
| `duty` ≥ 0.5 (`q` large too) | main thread saturated; input can't reach `ws.send` | **Phase 1** (smaller frames = smaller copies/blits) + set `ludion.editor.frameOffscreen = '1'` |
| `q` large but `duty` small | something other than the frame path hogs the main thread (React, inspector poll, terminal drawer) | outside this plan — devtools profile |
| `dec` > 8 ms on a loopback URL | the RAW default did not take (stale override) | clear `ludion.editor.frameFormat`; §0.2 |

### 0.2 Codec A/B (30 seconds)

`localStorage.setItem('ludion.editor.frameFormat','QOI')`, reload, compare `dec` and `fps`. Loopback
already defaults to `RAW_RGBA` (`viewer/frameFormat.ts:63-65`). **If QOI is *faster* here, the memory
bus — not the CPU — is the constraint, which reinforces Phase 1 twice over.** Clear the key after;
the console line `[ludion] frame format: …` confirms what ran.

### 0.3 Server-side truth

```
cargo run --release -p ludion-cli -- profile --attach ws://127.0.0.1:9411 --frames 300 --json lap.json
```
(`crates/ludion-cli/src/main.rs:988` → `profile::run_attach`, over `profile.capture`.) Read:

- `sim.frame_total_median_ms`, and the two live spans **`extract` and `submit`**
  (`crates/ludion-protocol/src/perf.rs:331`). **`submit` folds the readback in** (`perf.rs:295`:
  *"There is deliberately no readback segment"*). Large `submit` + small `extract` + small GPU passes
  ⇒ the readback/copy path ⇒ Phase 1.
- `gpu.passes[]` medians — if these dominate, `--quality low` is working and the extent is not the
  problem; the scene cost is.
- `transport.encode_median_ms` / `send_median_ms` (`perf.rs:298-311`). For RAW, `encode` **is** the
  `to_vec()` copy. Both scale linearly with the extent, so Phase 1's payoff is predictable:
  **0.56× at 960×540, 0.44× at 854×480.**

The editor's Performance view (`editor/app/src/perf/PerformanceView.tsx`) reads the same data live.

### 0.4 Tick cadence

```
LUDION_TICK_TRACE=1 cargo run --release -p ludion-cli -- serve --scene <scene>
```
If mean period ≫ intended and the histogram shows the ~15.6 ms quantum, the sim thread is not woken
on time — that **caps what any other phase can achieve**. Record next to the 2026-08-08 desktop
baseline (mean 17.1, p50 8.25, p95 15.5, max 39.2).

### 0.5 Which profile is running

Compare §0.3's `sim.frame_total_median_ms` against a `--release ludion serve` of the same scene.
A ≥3× gap ⇒ **Phase 3**, not Phase 1.

**Record §0.1–0.5 in the private frame-transport notebook under Done before writing code** (notebook
convention: outcomes only), and strike its first open TODO, which §0.1 discharges.

---

## Phase 1 — A stream-resolution knob

Fewer pixels is the only change that pays at *every* stage at once: readback bytes, the four memcpys
of finding 2, encode, socket write, the editor's payload copy, decode, and blit.
960×540 = 44% of 720p's pixels; 854×480 = 35%.

Two independently shippable sub-phases. **1a first — it is usable the day it lands.**

### Phase 1a — startup extent (no protocol change, ships alone)

The cheap 80%. Changing it costs an engine restart, which in `ludion dev` is already the normal loop.

**Engine seam** — `crates/ludion-engine/src/engine.rs`, beside `with_render_size` (`:505`):
```rust
/// Sets this session's render extent (also the live stream extent).
pub fn set_render_size(&mut self, width: u32, height: u32)
```
Non-consuming sibling; the builder form stays. Both write the one field; `render_size()` (`:1327`) is
already the reader. **Nothing else in the engine changes** — the ring-staleness check (finding 7)
does the rest.

**Resolution helper** — new `crates/ludion-engine/src/render_size.rs`, mirroring
`resolve_startup_quality` (`quality.rs:210`) exactly:
```rust
pub fn resolve_startup_render_size(flag: Option<&str>) -> Result<Option<(u32,u32)>, Diagnostic>
```
Precedence `--render-size` → `LUDION_RENDER_SIZE` → `None` (keep the default). Accepts `WxH`
(`960x540`). Reject 0 and anything over 7680×4320. Failure is a new **`L0017`
`RENDER_SIZE_INVALID`** in `crates/ludion-core/src/codes.rs`, sibling to `L0014` (`THREADS_ENV_INVALID`)
and `L0016` (`QUALITY_SETTING_INVALID`), with explain text. Refuse to start, like both of those.

**Wiring:**
- `crates/ludion-cli/src/main.rs` — `#[arg(long, value_name = "WxH")] render_size: Option<String>` on
  the **global** `Cli` struct (beside `quality` at `:80`, not per-subcommand), resolved once in the
  `:805-834` precedence block. One chain then serves `serve`, `render`'s defaults, `profile` and
  `dump-graph`.
- `crates/ludion-cli/src/serve.rs` — take `render_size: Option<(u32,u32)>` and call
  `engine.set_render_size(w,h)` in the existing host-settings block beside `set_render_scale` /
  `set_quality` (`serve.rs:66-73`), with the same "applied before the engine moves onto the sim
  thread" comment.
- `crates/ludion/src/lib.rs` — add the same `--render-size` to `RunArgs`. **Must not be
  `--width`/`--height`** (finding 6). Resolve beside `--quality` (`:261`), apply beside
  `engine.set_quality` (`:340`).
- `crates/ludion-cli/src/dev.rs` — **no argv change.** The child inherits `LUDION_RENDER_SIZE` from
  the environment exactly as it inherits `LUDION_QUALITY` and `LUDION_THREADS`. Document this in
  `dev.rs`'s module doc and extend `main.rs:823-828`'s comment ("like `--threads`" → "like
  `--threads` and `--quality`").

**Immediate payoff:** `LUDION_RENDER_SIZE=960x540 ludion dev` works the day this lands — no schema
re-bless, no editor build, no protocol rev.

**Tests:** parser unit tests + doctest (well-formed, `0x0`, unicode `×`, missing `x`, overflow), in
`quality.rs:225`'s style. Clone the `L0016`-refuses-to-start pattern at
`crates/ludion-cli/tests/render.rs:484-500` for `L0017`. Extend
`crates/ludion-engine/tests/egress.rs` (already builds engines at 160×90 / 320×180 —
`:299`, `:379`, `:443`) with "resize mid-stream → the next `EgressFrame` carries the new dimensions
and the `WorldHash` sequence is unchanged".

### Phase 1b — `render.size`, the live verb (protocol rev; the editor knob)

Modeled **exactly** on `render.gi_rebake`, the settled precedent for a session lever on the wire.

**Why a wire verb is legitimate, and why it does not contradict the `--scale`/`--quality` doctrine.**
`set_render_scale` and `set_quality` are documented as host GPU budget that "a connected viewer never
negotiates" (`engine.rs:509-520`, `:536-540`). **The extent is not that** — it is already a
client-negotiable concept on this protocol: `render.screenshot` takes `width`/`height` on the wire
(`RenderScreenshotParams`, `command.rs:461-467`), and `render.bounds` *reports* `render_size` in its
reply (`response.rs:154`). The stream is the one render path with no spelling for an extent every
other render path already has. This closes an asymmetry rather than opening a negotiation surface —
and it satisfies the hard parity gate (`CLAUDE.md`: nothing ships in the editor unless it exists as a
protocol command), which a startup flag alone cannot.

**Files, in `dev-docs/protocol-evolution.md`'s checklist order:**

1. `crates/ludion-protocol/src/command.rs`
   - `RenderSize(RenderSizeParams)` variant after `RenderGiRebake` (`:129`)
   - `"render.size"` in `method()` (`:196`) **and** in `Command::METHODS` in enum-declaration order
     (`:291`) — a missing entry makes the envelope parser return `L3002` for a valid method
   - `may_change_frame()` ⇒ **`true`** (`:249`): a resize invalidates the ring and owes the next pass
     a render
   - `pub struct RenderSizeParams { pub width: Option<u32>, pub height: Option<u32> }` — both optional
     so `{}` is a pure read (the gi_rebake "one verb sets and queries" shape). Doctest mirroring `:504-520`.
2. `crates/ludion-protocol/src/response.rs` — `ResponseBody::RenderSize { width: u32, height: u32 }`
   after `GiRebake` (`:162`). Always reports the *resulting* extent, so a client never guesses whether
   it was clamped.
3. `crates/ludion-engine/src/dispatch.rs` — one arm near `:70`. The exhaustive match is the parity
   gate; omitting it is a compile error.
4. `crates/ludion-engine/src/commands/render.rs` — `pub(crate) fn size(engine, params)`: reject
   exactly one of width/height (both or neither), reject 0 or > 7680/4320, else
   `engine.set_render_size(w,h)`; return the current extent either way. New **`L3025`
   `PROTOCOL_RENDER_SIZE_INVALID`** in `crates/ludion-core/src/codes.rs` (highest today is `L3024`).
5. `crates/ludion-engine/src/recording.rs` — add `Command::RenderSize(_)` to the **`None`** arm at
   `:88` beside `RenderGiRebake`, extending its comment: the extent is host state that never enters a
   `WorldHash`, so a session resized while recording replays on a default-extent engine bit-for-bit.
6. **Re-bless, in this order:** `BLESS_SCHEMA=1 cargo test -p ludion-protocol --test schema_snapshot
   -- --nocapture` → paste the printed hash into `EXPECTED_SCHEMA_HASH`
   (`crates/ludion-protocol/tests/schema_snapshot.rs:22`) → `npm run generate` in `editor/protocol/`
   → **re-record `games/*/run.lrp` LAST** (their `ReplayHeader` pins the hash at record time).
7. `editor/app/src/protocol/parity.test.ts` — bump `expect(generatedMethods.size).toBe(28)` → `29`
   (`:127`) and add a named guard beside the `render.gi_rebake` one (`:114-119`).

**Editor UI** — `editor/app/src/viewer/Viewport.tsx`:
- Beside `FPS_CAPS` (`:98`): `STREAM_SIZES = [null /* host default */, [1280,720], [960,540],
  [854,480], [640,360]]`.
- A second `<select>` in `.editorbar`, immediately after the max-fps one (`:1027-1041`), same markup.
- Persist under **`ludion.editor.streamSize`**, following the `frameFormat` / `frameOffscreen`
  pattern. Put the pure part (parse/serialize/clamp, the choice list) in a new
  `editor/app/src/viewer/streamSize.ts` with `streamSize.test.ts`, mirroring `frameFormat.ts` +
  `frameFormat.test.ts` — `vite.config.ts` only collects `src/**/*.test.ts` under `node`, so nothing
  testable may live in the `.tsx`.
- On mount and on change: `void client.call({ method: 'render.size', params: { width, height } })`.
  **This is a plain command, NOT a subscription parameter** — it must *not* join `useFrameStream`'s
  effect deps (`useFrameStream.ts:525`), because re-subscribing is exactly what we do not want.
  Frames simply start arriving at the new size and the existing resize paths handle it (finding 7).
- The `null` choice sends nothing, so the flag and the knob compose.
- `render.bounds` follows `render_size` automatically, and the overlay re-polls at
  `OVERLAY_POLL_MS = 300`, so selection boxes self-correct within a poll. Optionally force one
  immediate re-poll after a successful `render.size`.

`ludion-client` (Rust) needs no change; `Client::call` takes a `Command`.

### Determinism, goldens and exact-frame semantics — the honest statement

**Safe:**
- `WorldHash` untouched. `render_streamed_rgba` is a pure read of the world (`engine.rs:1522-1523`),
  `render_size` is never extracted into any component, and the verb is classified `None` in
  `recording.rs` so it leaves no replay record.
- **Goldens cannot move.** `crates/ludion-testkit/src/golden.rs` builds its own engine with explicit
  width/height (`:47-50`, `:66-67`), renders through `FrameSize::native(...)` (`:250`) at pinned FULL
  quality (`:251`), and never reads `Engine::render_size`.
- **`ludion render` cannot move** — `main.rs` passes `--width`/`--height` (default 1920/1080) straight
  into `render::run`; the session extent is not consulted.
- **`render.screenshot` keeps exact-frame semantics.** A request naming width/height is byte-identical
  before and after (`commands/render.rs:24-25` uses `render_size` only as the *default*). Only an
  extent-omitting screenshot in a session that asked to be resized changes — the requested behaviour.
- **`crates/ludion-server/tests/phase4.rs` is immune**: `run_fps_gate` builds its engine with an
  explicit `.with_render_size(1920, 1080)` (`:170`), so the three fps twins measure 1080p regardless
  of any new default, flag or verb — provided those tests never issue the verb.

**Two real couplings that must be documented, not waved away:**

1. **Mouse coordinates are render-target pixels.** `ludion_sim::InputEvent::MouseMove` is documented
   as "a new render-target-pixel position" (`crates/ludion-sim/src/input.rs:252-256`), and the editor
   derives them from `canvas.width` (`picking.ts:83`, `mousemap.ts`). Changing the extent changes the
   *numbers a game's aim code sees*. A recorded `.lrp` stores injected values verbatim, so **any
   replay still re-drives bit-identically** — but sessions recorded at 960×540 and at 1280×720 are
   different input traces producing different hashes. **Rule to add to `dev-docs/engine-replay.md`
   and the frame-transport notebook: committed `run.lrp` recordings are made at the default extent;
   never record a golden session from a resized viewport.**
2. **Any client that hardcoded 1280×720 breaks.** The correct read is the `render_size` field
   `render.bounds` already returns (`response.rs:154`). Add a line to `docs/wire-protocol.md` and grep
   `crates/ludion-mcp` and `games/*` for literal `1280`/`720`.

**Other risks:**
- **Ring recreate churn** — each distinct extent reallocates the `PhysicalCache` and three staging
  buffers. A `<select>` of discrete values is fine; **do not build a slider**, it would thrash GPU
  allocation.
- **A resize during an open `profile.capture` window** mixes two extents into one median. Harmless
  but misleading; note it in the verb's doc comment.
- **Multiple viewers share one extent** (last writer wins). Document it on the verb — same shape as
  `render.gi_rebake`'s session lever, and it is what keeps `render.bounds` coherent.

### Verify (Phase 1)

```
cargo test -p ludion-engine
cargo test -p ludion-protocol                       # snapshot + hash gate
BLESS_SCHEMA=1 cargo test -p ludion-protocol --test schema_snapshot -- --nocapture   # 1b only
cargo test -p ludion-cli --test render              # L0017 refusal
cargo test -p ludion-server --release               # phase4 twins + delivery + pipelining
pwsh ci/scripts/schema-drift.ps1                    # 1b only: snapshot -> TS -> byte-compare
cd editor/app && npx vitest run && npx tsc -b && npx oxlint
node scripts/check.mjs                              # the whole gate
```
Manual: `LUDION_RENDER_SIZE=960x540 cargo run --release -p ludion-cli -- serve --scene <scene>` +
editor. Confirm the overlay reports 960×540, **picking still selects the entity under the cursor**,
and `lag` drops.

---

## Phase 2 — The outbound reply lane (server-only, fully independent)

Executes the sketch recorded in the frame-transport notebook's TODO. **No wire change, no schema rev,
no editor change** — ships before, after or alongside Phase 1.

**Changes:**

1. `crates/ludion-server/src/delivery.rs`
   - Replace `pub(crate) type OutboundSender = mpsc::Sender<Outbound>` (`:83`) with a cloneable
     struct holding two senders: `text: mpsc::Sender<String>` (capacity `TEXT_CAPACITY = 32`) and
     `frames: mpsc::Sender<BinaryFrame>` (capacity `FRAME_CAPACITY = 4`). Keep `BinaryFrame`'s
     `frame: FrameIndex` field — it is what lets the writer file its `send` span under the right frame.
   - Split `OUTBOUND_CAPACITY` (`:80`) into the two constants; rewrite its doc comment: the frame lane
     is shallow because forwarders coalesce upstream through their `watch` slot, so depth only buys
     latency; the text lane stays 32.
   - `route_event` sends on `.text`; `frame_forwarder` (`:342`) on `.frames`. **Preserve current
     semantics exactly**: events still *drop* on a full queue (a visible `seq` gap — architecture §6),
     frames still *await* (backpressure into the latest-wins slot).
   - `register` takes `&OutboundSink` and stores/clones it unchanged.
2. `crates/ludion-server/src/connection.rs`
   - `handle_connection:246-248` creates both channels, passes both receivers to `writer_task`.
   - `writer_task:410` becomes a `tokio::select!` with `biased;`, text arm first. **Two `done` flags
     are required** — without them a closed `mpsc` returns `None` immediately and `biased` turns the
     loop into a busy spin. Keep the `hub.armed()` span on the binary arm only, and the
     `sink.close()` on the way out.
   - **Bound the bias**: drain at most `TEXT_BURST = 8` consecutive text messages before giving the
     frame arm a turn, so a request flood cannot starve frames. Two lines; rarely engages (replies are
     bounded by `PIPELINE_DEPTH = 64` and the sim pass rate) but removes the pathological case.
   - `finish` (`:506-527`) drops both senders — unchanged semantics, two drops instead of one.
   - Update the module doc at `:22` ("draining one outbound queue") and the in-crate tests at `:1014`,
     `:1065` that construct the single channel.
3. `crates/ludion-server/src/lib.rs` and `dev-docs/ludion-server.md` — rewrite the one-queue
   description to two lanes. **Note:** that doc is already 17.9 KB against a 12 KB spoke budget
   (recorded in the frame-transport notebook); do not grow it — take the deferred split, or trade
   prose out.

**Gate (recorded acceptance criterion):** the three `crates/ludion-server/tests/phase4.rs` 1080p fps
twins — Qoi 60.0 / Lz4Rgba 59.6 / RawRgba 51.6 — must not regress. Analysis says queue depth changes
latency, not throughput, but **RawRgba's 51.6 fps is the thinnest margin** against the 30 fps floor,
so run on the pinned hardware in `--release`.

**Other risks:** dropping the frame lane to 4 makes `frame_forwarder`'s `send(...).await` block sooner
— that is the intent (backpressure lands in the `watch` slot where coalescing is free), but confirm
`max_fps_throttles_frame_delivery` still holds its rate. Text-lane priority must never reorder
*within* a lane: replies stay strictly FIFO (`reply_pump`'s contract), event `seq` streams gapless.

**Gating tests (all existing):**
```
cargo test -p ludion-server --test phase4 --release   # the 3 fps twins — the named gate
cargo test -p ludion-server --test delivery           # events_stay_gapless_while_frames_flow,
                                                      # max_fps_throttles_frame_delivery,
                                                      # mixed_format_subs_each_receive_their_own_format,
                                                      # dropping_a_frames_client_leaves_the_server_healthy
cargo test -p ludion-server --test pipelining
cargo test -p ludion-server --test wire --test exit_criteria
```
**One new test to add** — this is the whole point of the phase and does not exist yet. In
`crates/ludion-server/tests/delivery.rs`, `replies_do_not_queue_behind_frames`: subscribe `frames`
uncapped at 1080p on one socket, issue N `sim.hash` requests on the *same* socket, assert the reply
round-trip median stays under a generous bound and is dramatically better than a single-lane control.
Use the existing raw-socket helpers (`raw_connect`, `send_request`, `await_reply`, `:73-131`).

### Phase 2b (optional) — kill the RAW double-copy

The recorded "fewer copies on the raw send path" TODO. Make `SharedFrame::payload` an `Arc<[u8]>`
(or an enum with a `Raw(Arc<EgressFrame>)` arm) so the `RawRgba` lane aliases the egress buffer
instead of `frame.rgba.to_vec()`; and add `FrameHeader::encode_into(&self, out: &mut Vec<u8>, payload:
&[u8])` so the per-subscriber header build reuses a buffer. Saves ~2 full-frame memcpys per delivered
frame — 7.4 MB/frame at 720p. Independent; measure `transport.encode_median_ms` before and after.

---

## Phase 3 — The dev-profile opt-level decision

The frame-transport notebook records this as an open **user decision**, deferred 2026-08-09 with an
explicit precondition: "measure with `profile.capture` first before raising any crate".
**Phase 0.5 supplies that measurement**, so the precondition is met.

At ~22 fps a pass is ~45 ms **and** the readback ring's structural +1 frame costs a full frame — so
raising the pass rate collapses two of the largest latency terms at once, from a `Cargo.toml` edit.

- Raise `ludion-engine`, `ludion-renderer` and the game crate (`vigil`) from `opt-level = 1` toward 3
  in `[profile.dev.package.*]` in the workspace root `Cargo.toml`, joining the existing `ludion` and
  `ludion-server` overrides.
- **`opt-level` only — never `debug-assertions = false` on a crate hosting game sim generics**; debug
  sim runs must keep their overflow panics (the recorded precedent for the facade override).
- **Measure incremental rebuild time, don't assume.** Precedent: raising workspace crates 0 → 1 was
  taken only once rebuild time measured unchanged (`dev-docs/frame-streaming.md`).
- If compile cost is unacceptable, the recorded alternative is a **dedicated fast-debug profile**, so
  the trade is opt-in per session.
- Mirror any override into the `ludion new` template — generated projects inherit no workspace profile.

Verify: `cargo test -p ludion-server --test phase4 -- --nocapture` (debug-profile fps).

---

## Non-goals (and why)

- **Direct / shared framebuffer** — rejected twice on record; transport is ~2.5 ms, not the dominant
  term. Finding 5.
- **A `stream.subscribe` per-subscriber downscale** — breaks picking, adds CPU on the constrained
  machine, cuts nothing upstream. Finding 6.
- **Native-window play affordance** (`ludion window --attach` / `--window`) — **user decision**: the
  game must stay in the editor viewport. (For the record, the cheap shape would be a `ludion window`
  CLI verb over `ludion_window::run`, ~50 lines and no protocol change, spawned from
  `editor/app/src-tauri/src/stage.rs`'s existing `cli_argv` machinery.)
- **Readback collect split** (recovering the +1 frame) and its **Windows timer-resolution**
  prerequisite — `timeBeginPeriod(1)` / `CREATE_WAITABLE_TIMER_HIGH_RESOLUTION` are `unsafe` FFI
  against `#![forbid(unsafe_code)]` on all seventeen workspace crates, so it needs an explicit pillar
  decision, not an implementation wave. Phase 3 attacks the same term far more cheaply, and Phase 1
  shrinks it further. Revisit only after re-measuring.

## Not designed here: a dedicated input socket

Recorded from the input-lag notebook, not built or scoped into a phase above. A second `Client`
carrying only `input.inject`, mirroring `ludion-window/src/lib.rs`'s input thread (which already
"owns a second `Client` and forwards mapped input events"), so mouse/keyboard traffic never queues
behind frame or reply traffic on the shared connection — its value is exactly what the overlay's
`queuedMs` reports. Distinct from Phase 2's outbound reply lane above (that splits *outbound* text
from frames; this would be a second *connection* for inbound input). No trigger recorded; revisit
only if Phase 1/2 leave `q` still large on the reference measurement.

## Independence and recommended order

| Phase | Depends on | Wire change | Ships alone |
|---|---|---|---|
| 0 measure | — | no | yes — **do first** |
| 1a startup extent | 0 | **no** | **yes — build first** |
| 1b `render.size` verb | 1a's `set_render_size` seam | **yes** (additive rev + replay re-record) | yes |
| 2 reply lane | — | no | yes, in parallel with 1 |
| 2b raw copy elision | — | no | yes |
| 3 dev opt-level | 0.5's numbers | no | yes (a `Cargo.toml` edit) |

**Recommended:** 0 → 1a (usable immediately via `LUDION_RENDER_SIZE`, zero ceremony) → 3 (if §0.5
shows the gap) → 1b (the editor knob) → 2 (+2b) → re-measure.

## Files a fresh instance will touch

| Phase | Files |
|---|---|
| 1a | `crates/ludion-engine/src/engine.rs` (`set_render_size`), new `render_size.rs`, `crates/ludion-core/src/codes.rs` (`L0017`), `crates/ludion-cli/src/{main.rs,serve.rs,dev.rs}`, `crates/ludion/src/lib.rs` |
| 1b | `crates/ludion-protocol/src/{command.rs,response.rs}` + schema snapshot, `crates/ludion-engine/src/{dispatch.rs,commands/render.rs,recording.rs}`, `crates/ludion-core/src/codes.rs` (`L3025`), `editor/protocol/` regen, `editor/app/src/viewer/{Viewport.tsx,streamSize.ts}`, `editor/app/src/protocol/parity.test.ts` |
| 2 | `crates/ludion-server/src/{connection.rs,delivery.rs,lib.rs}`, `crates/ludion-server/tests/delivery.rs` |
| 3 | workspace root `Cargo.toml`; the `ludion new` template |

## Background reading (in this order)

1. `dev-docs/frame-streaming.md` — the end-to-end path; **read before touching any leg of it**.
2. The private frame-transport notebook — the measured history and the open TODOs this plan executes.
3. The private editor-rendering notebook — the shared-framebuffer rejection.
4. `dev-docs/protocol-evolution.md` — the 5-step additive-rev checklist Phase 1b follows.
5. `dev-docs/renderer-quality.md` + `docs/quality.md` — why quality/scale are host switches.
6. `dev-docs/ludion-server.md` — pump / router / forwarder / connection internals.

Consult the `ludion-docs` skill before writing any doc changes.
