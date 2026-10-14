# Finding: strict-YAML canonical writer — key-escape expansion crosses the parser's simple-key limit

**Target:** `canonical_idempotence` (`ludion_scene::parse` / `ludion_scene::emit`)
**Severity:** parse/emit round-trip failure on crafted scene input (harness abort, not a
memory-safety issue)
**Status:** FIXED — `crates/ludion-scene/src/yaml.rs` now rejects, at parse time, any mapping key
whose **canonical spelling** exceeds saphyr-parser's 1024-character simple-key scan limit, as new
diagnostic `L1030`; `parse` now rejects such input up front, before it ever reaches the writer.
**Discovered by:** the nightly `canonical_idempotence` fuzz target, TeamCity, macOS agent
(2026-08-06).

## Reproducer

`key-escape-expansion.yaml` (310 bytes on disk). A minimal valid scene: `format: 1`, a name, one
entity with one component (`C`) whose value mapping has a single key — `k` followed by 200 DEL
(`0x7F`) bytes, 201 raw characters — mapped to `1`.

Repro:
```
cd fuzz && cargo fuzz run canonical_idempotence findings/canonical_idempotence/key-escape-expansion.yaml
```

## Precise behavior

Pre-fix, the strict-YAML parser accepted the ~200-DEL-byte key as an ordinary plain-scalar mapping
key (nothing in the parser rejected control characters in a key). `canonical_idempotence`'s first
`parse` therefore succeeded. The canonical writer then double-quoted the key (control characters
force quoting) and escaped every control byte as `\u00XX` — six characters each — expanding the
201-character raw key to 1203 canonical characters. saphyr-parser scans at most 1024 characters
from a key's start to its `:` before giving up on it as a simple key, so the *emitted* document
failed to re-parse, and the harness's `.expect("canonical emitter produced text that does not
re-parse")` at `fuzz/src/lib.rs:54` aborted.

## The fix

The check is on the key's canonical spelling, not its source spelling, because the writer can only
expand a key (quoting adds two characters, each control character escapes to six) — never shrink
it — so bounding the canonical length is what actually guarantees the emitted document re-parses.
`crates/ludion-scene/src/yaml.rs` computes each mapping key's canonical length while building the
document tree and rejects any key over 1024 canonical characters with `L1030`, naming the offending
key's prefix, its canonical length, and the limit. `canonical_idempotence`'s first `parse` now
returns `Err` on this input and the fuzz body returns early — a designed rejection, not a panic.
