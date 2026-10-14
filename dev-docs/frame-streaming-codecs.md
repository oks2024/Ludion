---
description: Frame delivery cost: debug-build QOI cap, share-generics fix, the QOI/LZ4/RAW measured baseline
---

# frame-streaming-codecs

**Scope:** the measured cost of the frame path's encode/send/decode legs and the dev-profile overrides
that keep a debug game streaming at 60 fps. The path itself, its latest-wins stages and the TS decode are
`ludion devdocs frame-streaming`; the server-side encode site is `ludion devdocs ludion-server`; profiler
layers that reproduce these figures are `ludion devdocs profiling`.

## Dataflow

### Measurements

**The debug-profile QOI encode, and the debug-plaza cap.** A debug **vigil-plaza** run once
delivered only **~19 fps (~52 ms/frame at 720p)** against a clean **60 fps** in release. An
instrumented per-segment breakdown of the delivery cycle (2026-07-15) settled where the time went:
extract 0.15 ms, record passes 0.16, submit+track 0.37, readback poll 0.57, unpad copy 0.57,
header+copy 0.03, WS write 0.22 — the render/submit/readback legs cost ~2–3 ms combined — and the
**QOI encode alone was 52.4 ms**. The encode was the entire cap.

The mechanism is rustc's **share-generics**: in dev builds a generic instantiation is emitted once,
into the deepest already-compiled dependency that can host it, not into the crate that owns the call
site. The QOI encoder (the `qoi` crate, a scalar reference implementation) is generic
(`encode_to_vec(impl AsRef<[u8]>, ..)`), so the instantiation a *game* binary runs is compiled into
the `ludion` facade crate (a game's one engine dep) — at opt-level 0 — while `ludion-server`'s own
test binaries host it in their already-covered root target. That is why
`crates/ludion-server/tests/phase4.rs` always streamed at ~60 fps while a live game crawled: the
test's encode was optimized, the game's was not. Crate-level opt bisection couldn't see this —
raising ludion-server or the whole render stack never moved a live game, because none of those is
where the game's encode actually lived; the isolated 5–6 ms encode benches ran in binaries whose
root target already covered the instantiation.

The fix is the facade override. The workspace root `Cargo.toml` carries two dev-profile encode
overrides on top of `[profile.dev.package."*"] opt-level = 3` (dependencies optimized; workspace
crates otherwise sit at `[profile.dev] opt-level = 1` — raised from 0 because vigil's committed
full-run replay tests made test *execution*, not compile time, the verify loop's cost, and
incremental rebuild time measured unchanged; still far below the 3 the encode needs):
`[profile.dev.package.ludion] opt-level = 3` (covers game-executed generic instantiations,
including the encode) and `[profile.dev.package.ludion-server] opt-level = 3` (plus
`debug-assertions`/`overflow-checks = false` — transport hygiene for the per-pixel loop, and it
covers ludion-server's own test targets). With the facade override the encode drops to **~2.3 ms**
and a debug plaza run streams at **60.0 fps, every frame delivered (max_skip=1), staleness 0** (from
19.3). `phase4.rs`'s three `frame_stream_delivers_at_least_30_fps_at_1080p_*` codec twins (QOI
`_over_localhost`, `_lz4`, `_raw`) hit the ~60 fps render ceiling in both profiles (and, because `ludion-server`'s `debug-assertions` is off, both
profiles compile the `!debug_assertions` branch and assert the real ≥30 fps criterion; the
`DEBUG_FLOOR` branch stays as a fallback if that override is removed). Anyone chasing "the viewer
feels slow" in a debug build should first check **both** the `ludion` and `ludion-server` overrides
are intact.

**The plaza sits in the noisy band.** The "~2.3 ms at 1280×720 on real content (the plaza)"
figure was measured 2026-07-15 on an earlier scene: the district has since grown 3× linear,
gained the night sky and the post stack, so its pixels are dense and noisy (dark, film
grain) rather than flat — near-worst case for both codecs below. Read the 2.3 ms as the measurement of that earlier scene.

**The measured three-way baseline** (release, vigil plaza night content, 1280×720, via
`ludion profile --attach` with a `frames` subscriber attached, e.g. `ludion dev --window` or the
editor):

| format   | server encode | send    | payload                | editor TS decode |
|----------|---------------|---------|-------------------------|-------------------|
| QOI      | 5.37 ms       | 0.87 ms | 1.20 MB (3.1:1)         | ~16.1 ms          |
| LZ4_RGBA | 7.55 ms (6.63 ms with `lz4_flex`'s unsafe impls) | 1.33 ms | 2.42 MB (1.5:1) | ~12.8 ms |
| RAW_RGBA | 0.675 ms (the copy) | 1.83 ms | 3.69 MB (uncompressed) | one `set()` call |

**Why LZ4 loses on this content.** The night city is dense in SHORT matches — LZ4's slow path at both
compress and JS decompress, and `lz4_flex` has no acceleration knob to trade ratio for speed. QOI
beats LZ4 at both size and encode cost here, which is why LZ4 lost the *remote* default too (see
`ludion devdocs frame-streaming` Gotchas) and is reachable only through explicit opt-in.

**The 1080p nuance.** `phase4.rs`'s uncapped fps gates measure **Qoi 60.0 / Lz4Rgba 59.6 / RawRgba
51.6 fps at 1920×1080** — at 8.3 MB/frame the socket write itself becomes RAW's ceiling. RAW wins
decisively at the game host's native 720p (total transport ~2.5 ms vs QOI's ~6.2 ms); at 1080p no
lane fits an 8.3 ms 120-fps budget (QOI ~13.4 ms total, RAW socket-bound, LZ4 worst of the three).

These figures need no hand-instrumentation to reproduce: `encode_ms`/`send_ms` are a live transport
layer of the profiler report, queryable on any running server via `profile.capture` or
`ludion profile --attach` with a `frames` subscriber attached (e.g. `ludion dev --window` or the
editor) — see `ludion devdocs profiling`.

Crucially, even when the encode capped delivery, it was a *throughput* limit, not the old backlog
bug. Before the pump seam was made latest-wins, the pump bridged frames into an unbounded mpsc, so a
router slower than the 60/s render rate accumulated raw ~8 MB RGBA frames without bound — measured as
display staleness growing ~0.7 s per wall second and the server's working set climbing 190 MB →
1.6 GB in 10 s (the "G0 input-lag" bug). With the latest-wins slot the same overload degrades
cleanly: at the former ~19 fps debug-plaza cap, a probe measured **bounded staleness (≤1 frame behind
real-time), flat memory (~165 MB)**, and every delivered frame *fresh*.

## Verify

```
cargo test -p ludion-server --test phase4   # the three codec fps gates
```
