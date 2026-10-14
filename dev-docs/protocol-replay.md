---
description: The LRP1 replay container: records, additive kinds, seek, and the decompression-bomb ceiling
---

# protocol-replay

**Scope:** `crates/ludion-protocol/src/replay.rs` — the one replay format (`LRP1`), its parsers' hostile-input
guarantees and the `ReplayReader::seek` index. The rest of the contract layer (envelopes, `Command`, schema
hash) is `ludion devdocs ludion-protocol`; recording at queue-drain time is `ludion-engine` (`ludion devdocs engine-replay`); the
determinism contract a replay proves is `ludion devdocs determinism`.

## Layout

- `crates/ludion-protocol/src/replay.rs` — `ReplayHeader` (scene ref, seed, versions, and the additive `profile`-at-start text), `ReplayRecord` (the record-kind enum — `Command`/`Input`/`Keyframe`, plus the additive `DocReload` kind 3 and `SceneSwitch` kind 4, both carrying inline text within `LRP1`), `ReplayWriter`, `ReplayReader` (+`seek`).

## Rules

- Exactly one replay format, and it lives in `ludion-protocol` (not `ludion-sim`); recording happens in
  `ludion-engine` at queue-drain time.
- **Parsers never panic on arbitrary input.** Every reader that faces untrusted bytes — the envelope
  parser, the binary frame decoder, and the `ReplayReader` — turns malformed, truncated, or hostile
  input into a structured L-coded diagnostic, never a panic or an unbounded allocation. A replay
  frame that decompresses past its ceiling is rejected with `L3017` rather than inflated (a
  decompression bomb cannot exhaust memory). The ceiling is a *ratio* bound, because a bomb is a
  ratio attack: `frame_ceiling(compressed) = clamp(compressed * 256, 1 MiB, 256 MiB)`, shared by the
  reader and the writer (which checks after compressing, so it refuses exactly what the reader
  would). The fuzz reproducer is 274 bytes inflating to 8 MiB (30,615:1); no committed replay
  compresses better than ~26:1, so 256:1 sits an order of magnitude over real data and far under
  any bomb, and a keyframe of any size up to 256 MiB records and replays. A unit test holds every
  committed `.lrp` 4x under its ceiling. Rejected: a header-declared maximum (attacker-controlled),
  a ceiling scaled from the frame-0 keyframe (a session can grow 10x after it), delta keyframes (a
  format change). Coverage-guided fuzzing (`fuzz/`) plus committed regression tests back this.

## Dataflow

A replay is everything needed to re-drive the engine to the same `WorldHash`
sequence: `LRP1` magic, u16 format version, a zstd-compressed JSON `ReplayHeader` (scene ref, seed,
engine/protocol versions, `schema_hash` at record time), then records to EOF — each a clear-text
`{kind, frame, body_len}` framing around a zstd-compressed JSON body (command / input / keyframe
snapshot). The `SceneRef` is one of two shapes: `Inline { document }` (the full scene text) or
`Path { path }` (a scene file resolved against the replay's project root). The engine always records
**inline** — the scene text travels in the replay (zstd-framed, so cheap), making a recording
self-contained and portable: it reproduces the same sequence from any directory or machine. `Path`
is accepted on replay for hand-authored sessions; a path that cannot be loaded is `L3013`. Because framing sits in the clear, `ReplayReader::seek`
indexes keyframes without decompressing bodies, then returns the latest keyframe ≤ target plus the
deltas to reach it. Malformed containers are `L3010`; failed zstd inflation is `L3011`.

The record kind is a 1-byte discriminant scanned against `KIND_MAX`; new kinds are **additive within
`LRP1`** (no framing change, every older `.lrp` still decodes, an unknown kind is still rejected). Two
kinds beyond the `Command`/`Input`/`Keyframe` base carry *external* text a replay cannot regenerate:
`DocReload` (kind 3) embeds a reloaded data-doc's canonical text, and `SceneSwitch` (kind 4,
`{ frame, scene_path, document }`) embeds an in-run district switch's target scene text. Both are
consumed as an **off-disk text source**, not as a drive action — `ludion-engine` regenerates the reload
/ switch from the sim and reads the embedded text so the session reproduces cross-machine (see
`ludion devdocs ludion-engine` and `ludion devdocs determinism`). Neither moves `schema_hash` (`ReplayRecord` is outside the schema
composition) and neither is a `Command` variant, so the wire surface is unmoved.

## Gotchas

- Replay bodies are one zstd frame *each* ("zstd-framed" deltas) — that's what makes seek cheap;
  batching records into shared frames for a better ratio is a deliberate optimization, not a
  bug to fix in passing.

## Verify

```
cargo test -p ludion-protocol                 # replay round-trips, seek, ceiling, committed-.lrp headroom
```
