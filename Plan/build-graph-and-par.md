# Plan: Build graph, file size, and the `par` runtime

> Status: **planned, not started.** Source: the 2026-09-25 code audit, finding J9. Current-code facts
> were read from the tree on 2026-09-26; file:line pointers are of that date, so re-grep before editing.
> Four independent waves. Each ships alone with its own green verify, in any order.

## Trigger

These are structural costs, not bugs. Each wave has its own trigger:

| Wave | Build when |
|---|---|
| W1 `codes.rs` | an explain-text edit is measured as the dominant rebuild in a fix loop, or `codes.rs` passes 8k lines |
| W2 `render.rs` | the twelfth pipeline is added, or a cache-key bug of the D2 class recurs |
| W3 `par` pool | a sim profile shows `par` fan-out at 5% or more of a tick, or a nested `par` needs a real thread count (the 1/2/8 gate's meaning) |
| W4 parallel `WorldHash` | hashing is the largest term of a verification loop (replay, bisect, probe campaign) on the reference box |

## What exists (2026-09-26)

**`crates/ludion-core/src/codes.rs` is 7,198 lines** (the crate is 10.7k).

- `pub mod code` (`:44`) has 229 `DiagnosticCode` constants.
- `CODES: &[CodeEntry { code, severity, title, explanation }]` (`:1398-6630`) holds the long explain
  text.
- 24 of 25 crates reference `codes::`, and `codes.rs` changed in 39 of the last 171 commits. Every
  touch rebuilds the workspace.
- **Severity is read at runtime** by `Diagnostic::new` (`diagnostics.rs:221-226`) and by
  `engine.rs:1439`.
- **Explanation text** is read only by `ludion explain` (`crates/ludion-cli/src/explain.rs`) and by a few
  tests that assert a code is registered.
- The module doc and `CLAUDE.md` both state the rule "registered in `ludion-core`'s code table".

**`ludion-core` holds `letterbox.rs` (298 lines) and `audio.rs` (196)**, each justified as a type
shared by two crates that must not depend on each other.

**`crates/ludion-renderer/src/render.rs` is 10,410 lines**; tests start at `:4625`, about 56% of the
file.

- It has 11 `build_*_pipeline` functions (`:4165-4590`) behind one `build_pipeline` dispatcher
  (`:3975`).
- `ShaderDesc { id, file_name, body, snippets, layouts }` (`:3478`) has no pipeline-state column.
  Its doc says a new shader needs "a SHADERS row — plus a `build_pipeline` arm".
- Three hand-written `RefCell<HashMap>` caches with upload-on-miss and retain-referenced eviction:
  `texture_cache` `:278`, `mesh_asset_cache` `:282`, `mesh_texture_cache` `:288`.

**`ludion-ecs`** is about 3.87k non-test lines against the ≈3.5k budget (`engine-architecture.md:195`,
`dev-docs/ludion-ecs.md:26`).

- The risk table at `engine-architecture.md:389` still says 2–3k, which is stale.
- `CommandBuffer` (`command_buffer.rs`, 228 lines) has no user outside its own crate's doc example and
  conformance test. It was deleted 2026-09-26 (Lore rev 935).

**`ludion_core::par`** (`par.rs`, 809 lines) spawns scoped OS threads on every call
(`par_chunks` `:322-374`).

- `with_threads` is a thread-local (`:117,280`) that spawned workers do not inherit, so a nested `par`
  on a worker uses the process default.
- A chunk's panic is caught and re-raised with `resume_unwind`, and loses its location (F8).
- `Plan/deterministic-job-system.md` measured 0.3–1.5 ms of per-call fan-out overhead and recorded that
  the internals are swappable "if profiles demand".

**`WorldHash::compute`** (`crates/ludion-sim/src/hash.rs:106-143`) is sequential: one `Xxh3`, one
reused `Canonical` encoder, and per-component cursors. The audit cites about 13 ms of a 20.65 ms game
loop; re-measure before acting.

## Proposed design

### W1 — `codes.rs`: the text leaves core, the constants stay

The audit proposed moving each domain's constants into the raising crate. **Prefer the smaller cut.**

- The constants and their severities stay in `ludion-core`: `DiagnosticCode::new("L1234")` gains a
  severity, or `code::` keeps a compact `(code, severity, title)` table. That keeps the runtime severity
  lookup (`Diagnostic::new`) working with no cross-crate registry.
- The explanation prose moves into `ludion-cli` (`src/explain/` split by domain), and `ludion-cli`
  assembles the `explain` table. That is the bulk of the 7k lines and the part that churns.
- A test in `ludion-cli` pins that every core constant has exactly one explanation and vice versa.
- Registration-only tests elsewhere switch to the core table.

Result: editing an explanation rebuilds `ludion-cli` only, and adding a code is a one-line change to
core. The full per-crate move is kept as a follow-up only if adding codes, not editing text, turns out
to dominate.

Update `CLAUDE.md`'s working-conventions line and the `codes.rs` module doc in the same change: the
rule stays "registered in `ludion-core`", and the text lives in `ludion-cli`.

Leave `letterbox` and `audio` where they are unless a third crate needs them. Their placement is
documented and costs nothing measurable.

### W2 — `render.rs` split and one pipeline builder

- **Move without editing** to `render/{mod, reload, registry, pipelines, assets, draws, prepare,
  recorder}.rs`, and move the tests to `render/tests/` by feature.
- **Add a `PipelineSpec` column to the `SHADERS` row:** vertex layout, blend, depth, cull and target
  format. One `build_pipeline` reads it, and the eleven builders are deleted.
- **One `FrameCache<K, V>`** (upload-on-miss, retain-referenced) replaces the three caches. Its key is a
  documented, deliberate choice per use: every input file of the mesh and texture caches.
- Every pinned golden is byte-identical on the reference box: the pipelines must be bit-for-bit the
  same state. `--dump-graph` snapshots must not move.

### W3 — a persistent `par` pool that carries its context

- A lazily started pool of `threads() - 1` workers, owned by `ludion_core::par`.
- Each job carries the caller's effective thread count, so a nested `par` on a worker honours
  `with_threads`. Nested calls run inline on the worker (no re-entrant fan-out) unless measured
  otherwise.
- A panicking chunk reports its location: capture `std::panic::Location` via a scoped hook, or have
  the job return a structured payload. Then re-raise on the caller, lowest chunk first, as today.
- The chunking and the lowest-index-first reduction are unchanged, so results are thread-count-invariant
  by construction. The existing `par` identity tests and `check.mjs --full`'s 1/2/8-thread gate are the
  judge.
- Keep std-only. The deterministic-job-system plan's rayon-free ruling stands.

### W4 — parallel `WorldHash`, identical digest

- Split the entity range into chunks. Each chunk encodes its entities' canonical bytes into its own
  buffer through `par_map_chunks`.
- Feed the buffers into one streaming `Xxh3` in chunk order. XXH3 over concatenated bytes is the same
  digest, so there is **no re-bless**.
- Trap: the `Canonical` encoder and the component cursors are reused across entities today. Check that
  neither carries state that depends on earlier entities before giving each chunk its own.
- Gate: a test hashing each committed scene both ways and asserting equal digests, plus the whole replay
  corpus passing unchanged.

### Housekeeping (any wave)

- Done 2026-09-26 (rev 935): `CommandBuffer` deleted by user ruling; `L0003` retired.
- Fix the stale 2–3k line in the architecture risk table.

## Risks

- **W1 splits a table agents grep.** `ludion explain` stays the lookup, and the constants stay in one
  file, so `rg L1234` still lands on both the constant and its text.
- **W2 on a hot file.** Most rendering arcs edit `render.rs`. Sequence the move commit when no other arc
  has renderer work open.
- **W3 lifetime.** A pool outliving a test, or a process that forks, must not deadlock. Join on drop
  and keep the scoped fallback for `threads() == 1`.
- **W4 memory.** Buffering a large world's encoding costs its size. Chunk by bytes, not only by entity
  count, if the 512-body or bullet-stress scenes show it.

## Exit gate

- `node scripts/check.mjs` green per wave; `--full` after W3 and W4.
- W1: `codes.rs` under 1.5k lines. The `ludion explain` output for every code is byte-identical: diff
  `ludion explain` over all codes before and after.
- W2: no golden, `--dump-graph` or budget moves; `rg 'fn build_.*_pipeline'` finds one.
- W3: nested `with_threads(2)` really runs two workers (a test counts distinct thread ids); a chunk
  panic's report names its file and line.
- W4: the digest is equal on every committed scene; the measured hash time is recorded in this plan's
  Status, per thread count.

## Status

Planned, not started (2026-09-26). Nothing from J9 shipped in the 2026-09 fix stage.
