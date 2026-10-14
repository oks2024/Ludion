# Plan: Frame Codec — LZ4

> Status: **EXECUTED AND CLOSED (2026-08-08, Lore revs 571–577)** — with the design's central premise
> **falsified by its own verify step** and the arc pivoting mid-flight. Read the Results section
> first; the design below is the plan as approved, kept for the record.

## Results (what actually happened)

Waves 1–4 landed exactly as designed: the additive `format` rev (hash `fd20d9041d9e43d5` →
`e1b5eeb3312780ec`), per-format encode-once fan-out, the window's LZ4 arm, the editor's hand-rolled
`lz4.ts` + worker threading, the cross-language fixture, the mixed-format and per-format fps gates.

**Then the verify A/B falsified the premise.** On the real plaza night scene (release, 1280×720,
`ludion profile --attach` + the windowed viewer as the frames subscriber; the harness validated
itself by reproducing the recorded QOI baseline to 0.02 ms):

| format | server encode | send | payload | editor TS decode |
|---|---|---|---|---|
| QOI | **5.37 ms** | 0.87 ms | 1.20 MB (3.1:1) | ~16.1 ms |
| LZ4_RGBA | **7.55 ms** | 1.33 ms | 2.42 MB (1.5:1) | ~12.8 ms |
| RAW_RGBA | **0.68 ms** (the copy) | 1.83 ms | 3.69 MB | ~0 (one `set` call) |

The night city (dark + film grain) is dense **short** matches — LZ4's slow path at both compress
and JS decompress; `lz4_flex` has no acceleration knob (its unsafe impls only reach 6.63 ms), and
`copyWithin` variants of the TS decoder measured no better. LZ4 lost to QOI on *both* size and
encode, keeping only a ~4 ms decode edge. The "pays at both ends" trigger was half-false.

**The pivot (user decision, 2026-08-08): `RAW_RGBA` became the third format** — uncompressed RGBA8,
wire byte 2, a second additive rev (`e1b5eeb3312780ec` → `3a1565a598c9da6d`). On loopback the wire
is free and codec time is everything: total transport 2.5 ms vs QOI's 6.2 at 720p, and the client
decode disappears. First-party clients (window `default_stream_format`, editor `chooseFrameFormat`)
default to RAW on loopback hosts and **QOI — not LZ4 — otherwise**; LZ4 is nobody's default,
reachable only by explicit opt-in (the editor's `ludion.editor.frameFormat` override, a Rust
caller's argument), kept as a shipped, tested, additive wire capability whose numbers can be
re-measured when content changes (a smoother, brighter scene moves every row of that table).

**Open question, deliberately left:** at 1080p the phase4 uncapped gates read Qoi 60.0 / Lz4Rgba
59.6 / **RawRgba 51.6 fps** — at 8.3 MB/frame the socket write path (forwarder header+payload
assembly, tungstenite framing, kernel) becomes RAW's ceiling. RAW wins decisively at the game
host's native 720p; at 1080p **no current lane fits the 8.3 ms 120-fps budget** (QOI 13.4 ms total,
RAW socket-bound, LZ4 worst). The eventual 1080p@120 answer is fewer copies on the raw send path —
a future wave with its own trigger, not this one.

Deviations from the design: `lz4_flex` kept the safe features (the 0.9 ms unsafe delta only matters
on a default path, and LZ4 no longer is one); the editor escape grew from a QOI-forcing switch to
naming any of the three formats; `frames.ts`'s unknown-byte test moved to byte 3. Everything else
shipped as written.

## Context

The G0 input-lag investigation found that frame delivery, not the sim tick, caps fps on noisy voxel
content. The trigger has only strengthened since the first draft of this plan:

- **Server encode:** the profiler (shipped 2026-08-02) measures QOI `encode_to_vec` on served vigil
  plaza at **11.867 ms median @1080p** / 5.384 ms @720p (`ludion profile --attach`, recorded in
  `dev-docs/frame-streaming.md`). The old "~2.3 ms plaza" figure is retired — that scene no longer
  exists (the district grew 3× linear and gained the night sky + post stack; today's plaza sits in
  the *noisy* band).
- **Client decode:** the editor input-lag arc (2026-08-08) measured the editor's hand-rolled QOI
  decode at **10–20 ms/frame** — large enough that it had to be moved off the main thread into a
  worker. LZ4 is the one change that pays at **both ends** of the wire, and on localhost its
  payload-size cost is irrelevant.

The user's requirement is that frame transmission must never be the fps ceiling — 60 fps solid
today, 120 conceivable later. ~12 ms of encode against the 16.6 ms 60-fps budget leaves almost no
margin, and it makes the 8.3 ms 120-fps budget impossible outright. This doc is the codec fix that
buys transmission real headroom.

## Trigger criterion

Already met — no further profiling gate to clear. In implementation now.

## Current-code facts that shape the work (audited 2026-08-08)

- `FrameFormat` already declares both tags: `Qoi` (byte 0) and `Lz4Rgba` (byte 1), JSON `"QOI"` /
  `"LZ4_RGBA"`, with the `u8` round-trip (`crates/ludion-protocol/src/frame.rs:41-68`). The LFR1
  header is format-agnostic — it writes whichever tag is set (`frame.rs:112`) and reads it back
  (`:140-141`); the header tests already use `Lz4Rgba` as their sample format. Nothing changes here;
  `Lz4Rgba` just needs a producer and consumers.
- The generated TS union already carries the tag (`editor/protocol/src/protocol.d.ts:1476`), and the
  editor's hand-written LFR1 parser already decodes the format byte into `'QOI' | 'LZ4_RGBA'`
  (`editor/app/src/protocol/frames.ts:14`, `:73`) — the header path needs no change on either side.
- `StreamSubscribeParams` has two fields, `channel` and `mode`
  (`crates/ludion-protocol/src/command.rs:475-482`); there is no format selector. Adding one is the
  wire change this wave makes. `FrameFormat` is `Copy`, which the server's peek-without-consume
  observation contract requires (`crates/ludion-server/src/connection.rs:522-523`).
- The server only ever produces QOI today (`crates/ludion-server/src/delivery.rs:38-39` says so).
  `SharedFrame` (`delivery.rs:79-91`) has no `format` field — implicitly QOI, encoded exactly once
  per frame on the blocking pool inside `route_frame` (`:220-254`) and fanned out by `Arc` to every
  subscriber. **A second server site the first draft missed:** `frame_forwarder` hardcodes
  `FrameHeader { format: FrameFormat::Qoi, .. }` at `delivery.rs:295`.
- **The profiler brackets the encode** (`delivery.rs:229-233` → `perf.rs::record_encode`), with a
  written contract that the span covers "the codec call and nothing else" (`:216-219`).
  `TransportProfile` (`crates/ludion-protocol/src/perf.rs:297-313`) has no format dimension, and the
  sample ring is last-write-wins per frame — two naive `record_encode` calls per frame would make
  `encode_ms` a nondeterministic mixture. This wave must decide what the span means under two
  formats (resolved below).
- The format travels the same path `max_fps` already does: `observe()`
  (`connection.rs:524-534`) peeks the subscribe params, `Observed::Subscribe { channel, mode }`
  (`:329-332`) carries them, `apply_observation()` (`:356-386`) hands them to `delivery::register`
  (`delivery.rs:316-339`), which reads `max_fps` out of `SubscribeMode` and holds
  `SubState::Frames(watch::Sender<…>)` (`:99`).
- `ludion-window`'s decoder rejects anything but QOI by design (`crates/ludion-window/src/frame.rs:33-43`,
  L3004), with a test `rejects_non_qoi_format` (`:112-116`) that feeds `Lz4Rgba` expecting rejection —
  that test's premise inverts this wave. The window subscribes at
  `crates/ludion-window/src/reconnect.rs:164` via `Client::subscribe(Channel::Frames, …)`.
- **The editor decode path was rewritten by the input-lag arc (2026-08-08)** and is no longer the
  single early-return the first draft described. Decode happens at three sites: the worker's two
  paths (`editor/app/src/viewer/frameWorker.ts:38` offscreen-canvas, `:53` buffer-recycling
  worker-blit — the default) and the inline no-worker fallback
  (`editor/app/src/viewer/useFrameStream.ts:164`). The "ignore non-QOI" guard now lives at
  `useFrameStream.ts:417-420`, on the main thread upstream of the latest-wins pump. `FrameJob`
  (`editor/app/src/viewer/frameWorkerProtocol.ts:14-20`) carries `{frameIndex, width, height,
  payload}` and must grow a `format` field to cross the worker boundary. The subscribe call is
  `useFrameStream.ts:434-448` (raw `client.call`); a typed wrapper also exists at
  `editor/app/src/protocol/client.ts:228-236`.
- `editor/app/src/protocol/qoi.ts` reads dimensions from QOI's own payload header; an LZ4 block is
  **not** self-describing, so the LZ4 decoder's dimensions come from the LFR1 header — its natural
  shape is `decompressInto(src, out)` rather than `decodeQoi`'s return shape. The cross-language
  decode fixture precedent is `crates/ludion-server/examples/gen_qoi_fixture.rs` →
  `editor/app/src/protocol/__fixtures__/` → `qoi-fixture.test.ts`.
- `ludion-client` is format-agnostic pass-through (`crates/ludion-client/src/lib.rs:755-759`) and
  needs no decode change; its `Client::subscribe(channel, mode)` (`:357-372`) builds the params and
  is the API every Rust subscriber uses (~16 call sites across server/client/window tests).
- The engine is untouched by this wave: its `Subscriptions` table stores only channel + seq (it does
  not even store `mode`), and it emits raw RGBA `EgressFrame`s — encoding is purely a
  server-delivery concern. Old recorded subscribes deserialize via the serde default.
- `lz4_flex` is not in the dependency tree (confirmed; `zstd` exists but is scoped to `.lrp` replay
  framing only). The `qoi` entry's comment (`Cargo.toml:40-48`) documents the placement discipline
  and ends with "`LZ4_RGBA` stays a declared wire tag with no encoder yet" — a sentence this wave
  falsifies. `Cargo.toml:151` and `:162` also carry broken `docs/frame-streaming.md` references
  (the doc moved to `dev-docs/`); fix in passing.
- The additive-rev checklist lives in `dev-docs/protocol-evolution.md:64-88` (wire types →
  `BLESS_SCHEMA=1` snapshot bless → hand-update `EXPECTED_SCHEMA_HASH`, currently
  `fd20d9041d9e43d5` at `crates/ludion-protocol/tests/schema_snapshot.rs:22` → `npm run generate` in
  `editor/protocol/` → re-record replays **last**; one atomic commit across all five surfaces).
  Comment edits on wire types move the hash too (`:90-93`), so any doc-comment prose rides the rev
  commit. Committed replays: `games/breakout/run.lrp`, `games/vigil/run.lrp`,
  `games/vigil/stress.lrp`. **Re-record scoped to the replay tests so no PNG golden re-blesses**
  (precedent: the profiler and threads revs): `LUDION_BLESS=1 cargo test -p vigil --test replay
  <named test>` per replay (`stress` by its own test name,
  `recorded_stress_session_replays_to_identical_worldhash_trace`), breakout equivalent; cross-check
  with `--replay-verify`.

## Design

### 1. Protocol rev (additive, per `dev-docs/protocol-evolution.md`)

- Add `#[serde(default)] pub format: FrameFormat` to `StreamSubscribeParams`, and give `FrameFormat`
  a `Default` of `Qoi` — every existing caller (including recorded replays) keeps its exact current
  meaning. Update the params doc example.
- Run the five-step checklist in order, replay re-record last (scoped, see above). One atomic commit
  across `command.rs`, `frame.rs`, `protocol_schema.snapshot.json`, `schema_snapshot.rs`,
  `protocol.d.ts`, the mechanically-churned Rust call sites (§4), and the three `.lrp` files.

### 2. Server: per-format encode-once delivery

- Add `lz4_flex` (pure Rust, safe API) to `[workspace.dependencies]`; dependents: `ludion-server`
  (encode), `ludion-window` (decode), the fixture generator. Check the feature set at
  implementation — drop the unused frame format if the crate's features allow; comment whichever is
  chosen and why, mirroring the `qoi` entry's discipline. Update the `qoi` comment's now-false last
  sentence.
- `SharedFrame` gains `format: FrameFormat`. `SubState::Frames` becomes a struct variant
  `{ slot, format }`; `register()` gains a `format` parameter threaded exactly like `max_fps`
  (`Observed::Subscribe` gains the field; `observe()` copies it; `apply_observation()` passes it).
- `route_frame` rewrite: **snapshot the wanted-format set first** (walk the frame's registered subs
  under the registry lock; two bools — the set is at most `{Qoi, Lz4Rgba}`), and if empty, return
  without encoding. One `spawn_blocking` closure performs 1–2 encodes sequentially:
  `qoi::encode_to_vec` as today, and `lz4_flex::block::compress(&frame.rgba)` — raw block, **no
  prepended size**: the LFR1 header already carries width/height, so the decoder's preallocation is
  `width * height * 4` and a prepended size would be a second, contradictable source of truth. Build
  one `Arc<SharedFrame>` per produced format; fan out the Arc matching each sub's stored format. Two
  same-format subscribers still share one `Arc` and one encode — the invariant this wave must not
  regress. A sub registering between the snapshot and the fan-out misses at most one frame — the
  same benign registration race the module doc already documents.
- `frame_forwarder` stamps `format: shared.format` instead of the `:295` hardcode.
- **Profiler semantics: one span, sum.** The encode `Instant` starts before the first codec call and
  `record_encode` fires once after the last, so `encode_ms` means "total codec cost this frame put
  on the blocking pool" — deterministic, no `TransportProfile` schema change (a per-format dimension
  waits for a demonstrated consumer). Update the contract comment ("brackets the codec call**s** and
  nothing else").
- Module doc: delete "Only QOI", state the contract — each raw frame is encoded once per distinct
  format among its registered subscribers (at most twice), never per subscriber.

### 3. Decoders

- **`ludion-window`** (`frame.rs`): `decode_frame` becomes a `match` on the header format — `Qoi`
  unchanged; `Lz4Rgba` → `lz4_flex::block::decompress(&payload, w*h*4)`, with decompress error *or*
  output-length mismatch → L3004 (message names LZ4). The unknown-format arm disappears (closed
  two-variant enum; unknown bytes are rejected upstream at `FrameHeader::decode`). Module doc
  rewritten; note the two arms' different dimension sources (QOI's own header vs the LFR1 header).
- **Editor**: new `editor/app/src/protocol/lz4.ts` — a hand-rolled scalar LZ4 **block**
  decompressor, decompression only, ~60 lines, no npm/WASM dependency (same argument as `qoi.ts`'s
  header comment; WASM inside a module worker plus vitest's node environment is real friction for a
  mechanical byte loop): `decompressLz4Block(src: Uint8Array, out: Uint8ClampedArray): void`,
  throwing `Lz4DecodeError` on malformed input, offset 0 / out-of-range offsets, truncation, or a
  block that under- or over-fills `out`. Overlapping matches (offset < length — the RLE case) copy
  byte-at-a-time. Thread `format` through `FrameJob`; a `decodeInto(job, out)` switch feeds both
  worker paths and the inline sink; widen the `useFrameStream.ts:417-420` guard to "not a format
  this viewer decodes" (kept as the defensive future-format guard).
- **`ludion-client`** needs no decode change (verified pass-through); `ReceivedFrame`'s "QOI in this
  milestone" prose is updated to name both formats and point at `header.format`.

### 4. Choosing the format

- No auto-negotiation, per the one-version/no-capability-flags stance in
  `dev-docs/protocol-evolution.md`. The subscriber states `format` explicitly; the wire default
  stays `Qoi` so nothing existing changes behavior.
- **`Client::subscribe` changes signature** to `(channel, mode, format)` — one method mirroring the
  wire, per the one-way rule; ~16 call sites churn mechanically, all passing `FrameFormat::Qoi`
  except `ludion-window`, which opts into `Lz4Rgba` unconditionally at `reconnect.rs:164` (server
  and window are same-repo lockstep; a QOI frame still renders through the two-arm decoder anyway).
- **The editor defaults to `LZ4_RGBA`** — the 10–20 ms client decode is half this wave's trigger —
  with a localStorage escape `ludion.editor.frameFormat` (`'QOI'` forces the old codec), resolved
  beside the existing `ludion.editor.frameOffscreen` pattern and announced in the same
  `console.info`. The escape doubles as the A/B knob: flip the key, reload, read `decodeMs` off the
  lagProbe overlay. The typed `client.ts` wrapper gains an optional `format?` parameter.

### 5. Cross-language fixture + tests

- Extend `crates/ludion-server/examples/gen_qoi_fixture.rs` → `gen_frame_fixtures.rs` (same
  synthetic image, one generator): emit `fixture.lz4` (`lz4_flex::block::compress`) beside the
  existing `fixture.qoi`/`fixture.rgba`/`fixture.json`. New `lz4-fixture.test.ts` decompresses it
  and byte-compares against `fixture.rgba` — the one gate pinning that the TS decoder agrees with
  the exact `lz4_flex` the server ships.
- Synthetic `lz4.test.ts`: literals-only block; short match; overlapping match (offset 1);
  literal/match length-extension bytes (15 + 255 + n); malformed: offset 0, offset beyond written,
  truncated mid-literal and mid-offset, overrun, underrun.
- `crates/ludion-server/tests/delivery.rs`: new `frames_deliver_as_lz4_when_requested` (subscribe
  with `format: LZ4_RGBA`, decompress, assert `w*h*4` + header tag) and
  `mixed_format_subs_each_receive_their_own_format` (modeled on
  `two_clients_each_receive_only_their_own_sub` — client A QOI, client B LZ4, each frame's header
  matches its sub's request).
- `crates/ludion-window` tests: `rejects_non_qoi_format` deleted (premise inverted); replaced by an
  LZ4 round-trip, a garbage-payload rejection, and a length-mismatch rejection (all → L3004 where
  applicable).
- `phase4.rs`: factor the 1080p ≥30 fps gate body into `run_fps_gate(format)` and add the **LZ4
  twin** (user-approved 2026-08-08) — both first-party viewers default to LZ4, so the shipped path
  is the gated path; it cannot be flakier than the QOI gate (LZ4 is strictly cheaper).
- Editor `integration.test.ts`: the existing `format === 'QOI'` assertion is retitled as the
  default-is-QOI regression test; an LZ4 sibling subscribes with the format and asserts decode.

## What NOT to do

- No hardware encode (NVENC/QSV/etc.) — out of scope, a platform-conditional dependency for a
  problem plain CPU LZ4 solves within budget.
- No shared-memory transport. The wire protocol is the one command core's one surface (`CLAUDE.md`'s
  hard parity gate); a side-channel for frames would be an editor-private/perf-private path.
- No direct-swapchain presentation for `ludion-window` — it would break the "the human viewport and
  the agent screenshot query are the same endpoint" invariant. Shipping-era question only.
- **No inter-frame/delta codecs** (delta-vs-previous, temporal prediction, video codecs in P-frame
  mode). Frames are dropped at four independent latest-wins seams — the engine's `bounded(1)` frame
  lane, the router's watch slot, each per-sub forwarder slot, and the editor's frame pump — so any
  codec where frame N's decode depends on frame N−1 is structurally unsafe on this transport. QOI
  and LZ4_RGBA are both intra-frame by construction.
- Do not conflate this with a sim tick-rate change. 120 fps *display* additionally requires deciding
  whether the sim stays fixed at 60 Hz — a separate, larger decision (determinism, replay format,
  `WorldHash` cadence) explicitly out of scope. This wave only removes transmission from the
  ceiling.

## Risks

- **The share-generics trap does not recur, by construction:** `lz4_flex`'s block entry points are
  non-generic `&[u8]` functions, so their codegen lands inside the `lz4_flex` crate, which the
  dependencies-optimized dev profile already covers — unlike `qoi::encode_to_vec`, whose generic
  instantiation migrated into the caller's crate (the 2026-07-15 lesson). The dev-profile plaza fps
  run stays in the verify list as the empirical canary anyway — this exact class of bug shipped
  once.
- **LZ4 on noisy voxel RGBA may underwhelm on size, not time:** worst case it degrades to literals
  (fast to encode, ≤ ~0.4% expansion) and payloads approach `w*h*4` — cost moves from encode to
  send. Loopback absorbs this (send_median baseline 1.522 ms @1080p is memcpy-bound), but the verify
  step records **both** medians and makes no victory claim on encode alone. If send grows
  pathologically, the two escape hatches (window arg, editor localStorage key) hold the line.

## Sequencing (five commits)

1. **Atomic protocol rev** — §1 plus the §4 signature churn at `FrameFormat::Qoi`. Server behavior
   unchanged and honest at this commit: everyone still receives QOI, and the forwarder hardcode is
   still *true*.
2. **Server** — dep + Cargo.toml comment fixes, per-format fan-out, connection threading, profiler
   span, module docs, delivery tests, fixture generator + committed `fixture.lz4`, phase4 factoring
   + LZ4 fps gate.
3. **`ludion-window`** — decoder arm + tests, `Lz4Rgba` default at the subscribe site, module doc.
4. **Editor** — `lz4.ts` + both test files, `FrameJob`/worker/inline threading, guard widening,
   subscribe default + localStorage escape, `integration.test.ts`, stale header comments.
5. **Docs + measured numbers** — `dev-docs/frame-streaming.md` sweep (the "once per distinct
   registered format" rule, the retired only-QOI gotcha, the owed stage-7 Decode rewrite covering
   the worker paths, layout additions for `frameWorker.ts`/`framePump.ts`/`ludion-window`, the new
   LZ4 baseline numbers), `dev-docs/ludion-protocol.md`, `dev-docs/ludion-server.md`,
   `ludion-engine/src/egress.rs` comments, `ReceivedFrame` doc, this plan's Status flip, the
   frame-transport notebook entry. Verify runs between 4 and 5 so this commit carries
   the measured numbers.

## Verify

```
node scripts/check.mjs                        # full local gate: schema-drift, vitest, clippy, tests
BLESS_SCHEMA=1 cargo test -p ludion-protocol --test schema_snapshot -- --nocapture   # (rev step)
pwsh ci/scripts/schema-drift.ps1              # full drift gate
```

- **Server A/B:** `ludion profile --attach` on served vigil plaza @1080p with an LZ4 subscriber vs
  the recorded QOI baseline — expect `encode_median_ms` **11.867 → ~2–4 ms**; record
  `send_median_ms` both ways (see Risks).
- **Editor:** lagProbe overlay `decodeMs` with `ludion.editor.frameFormat` unset (LZ4) vs `'QOI'` —
  expect the 10–20 ms decode to fall to low single digits.
- **Window:** dev-profile plaza run confirming 60 fps (the share-generics canary).
