# Finding: LRP1 replay reader — zstd decompression bomb (unbounded body inflation)

**Target:** `replay_reader` (`ludion_protocol::replay::ReplayReader`)
**Severity:** allocation bomb / memory-exhaustion DoS on untrusted replay input
**Status:** FIXED — `decompress()` now streams through a bounded reader with a per-frame ratio
ceiling (256x the compressed size, 1 MiB to 256 MiB) and rejects over-ceiling frames as `L3017`; the
regression test (`crates/ludion-protocol/tests/replay_decompression_bomb.rs`) runs this reproducer
on the pinned stable toolchain.
**Discovered by:** targeted analysis while authoring the target (not by a fuzzer crash — the
deterministic mutation driver will not synthesize a valid zstd frame by chance, and the
coverage-guided run that would find it is deferred to the CI agent, where libFuzzer's
`-rss_limit_mb` flags exactly this).

## Reproducer

`zstd-decompression-bomb.lrp` (404 bytes on disk). A well-formed LRP1 container:
magic + version + a valid zstd header, then one keyframe record whose zstd body decompresses to
**8 MiB of zeros** — a 30,615× expansion. Scaling the pre-compressed size scales the RAM used
without bound (the analysis run used a 256 MiB body from an 8,340-byte file, 32,696×).

Repro:
```rust
let bytes = std::fs::read("zstd-decompression-bomb.lrp").unwrap();
let reader = ludion_protocol::replay::ReplayReader::new(&bytes).unwrap(); // header parses fine
let _ = reader.records(); // decompresses the body -> 8 MiB allocated before the JSON parse fails
```

## Precise behavior

`ReplayReader::records()` (and `seek()`) call `decompress()` → `zstd::decode_all` on each record
body with **no cap on the decompressed size**. A tiny compressed frame inflates to an arbitrarily
large `Vec<u8>` in RAM *before* the subsequent JSON parse even runs. The observed failure is a
downstream `L3010` ("replay keyframe record is not valid JSON"), but the memory is already spent:
the diagnostic is raised *after* the full inflation, so it provides no protection.

## Important correction to the plan's suspicion

The plan flags "a corrupt `len`" (the in-the-clear length fields) as the suspected allocation bomb.
**Those are already safe.** `ReplayReader::new` (header_len) and `scan()` (per-record body_len) both
use `checked_add(...).filter(|end| *end <= bytes.len())` before slicing, so a corrupt in-the-clear
length is rejected cleanly with `L3010` and never drives an allocation (verified: a header claiming
`header_len = 0xFFFFFFFF` is rejected, not honored). The real uncapped path is the **zstd
decompressed output size**, not the clear-text length. The fix therefore belongs on the decompressed
size — e.g. cap `decode_all` output (stream-decompress with a byte ceiling, or reject when zstd's
advertised frame content-size exceeds a limit) — rather than on the `len` fields.

## Suggested regression (for the fixing wave, per decision 3)

Commit this `.lrp` as a plain `#[test]` in `ludion-protocol` asserting that `records()` on a body
whose decompressed size exceeds the cap returns a diagnostic (new L-code) *without* allocating the
full output — so the pinned stable CI runs the regression with no nightly.
