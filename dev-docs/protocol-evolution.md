---
description: How the wire protocol evolves: the additive-rev checklist, what forces a v2, the one-version stance
---

# Protocol evolution

**Scope:** The policy for changing the wire protocol without stranding clients. It covers the two
kinds of change (an *additive rev* that keeps the version at 1, and a *breaking rev* that would be a
v2), the exact re-bless checklist an additive rev runs, and the enforcement that makes a stale client
fail loudly rather than silently misbehave. The mechanism this policy drives — the `Command` surface,
`schema_hash`, the snapshot test, the replay header — is documented in `ludion-protocol`; L-codes in
`diagnostics`. This is the *how you change it* companion to that *what it is*.

## The one-version stance

`PROTOCOL_VERSION` is `1` and the engine speaks **exactly one version at a time**. There is no
negotiation, no down-level fallback, no capability flags that turn features off. A request whose
envelope `v` does not match is rejected with `L3003` ("regenerate the client"), and the handshake
announces the current `schema_hash` in its `ServerHello` so a client generated from a stale schema
fails **at connect time**, not deep inside a later call.

The consequence is deliberate: **a version bump is breaking by design.** Because only one version is
ever live, there is no compatibility window to preserve — the enforcement is `schema_hash` rejection
(`L3003`), and the fix is always the same one sentence: regenerate the client against the new schema.
This keeps one obvious path through the protocol instead of a matrix of supported versions.

*Rejected alternative — multi-version negotiation.* Handshaking down to an older schema (or gating
features behind advertised capabilities) was considered and rejected: it doubles the wire surface (every
method must keep working under every still-supported shape), spreads version-conditional branches
through the dispatcher, and multiplies the test matrix — a direct violation of the "regularity over
cleverness / one obvious way per task" pillar. One live version, regenerated on change, is the whole
policy.

## Additive rev vs. breaking rev

An **additive rev** adds capability while leaving every existing method and field meaning **exactly as
it was**: a new `Command` variant, a new response `kind`, a new event, a new optional field that
serde-defaults. Old clients that never call the new thing keep working byte-for-byte — but they still
must regenerate, because the `schema_hash` moved (see below). The version stays `1`; the change is
recorded and re-blessed.

A **breaking rev — a v2** — is anything that changes the meaning of what already ships:

- **renames** — a method, variant, field, or response `kind` gets a new name;
- **removals** — a method, field, or variant goes away;
- **semantic changes** — an existing method's behavior, error mapping, units, or field shape changes
  under the same name.

The engine has never taken a v2; the protocol revs to date (Phase 6's `component.remove` +
inverse-carrying mutation responses, `render.bounds`, and the `entities` id filter on `query.entities`) were
all additive. The last also removed `SubscribeMode::Acked`, which stayed at version 1 because every
request using it was already refused (`L3012`), so no working client changes behaviour. When a v2 does
become necessary, it is a project-level event, not a routine commit: `PROTOCOL_VERSION` bumps to `2`,
every client regenerates, and the old shape is gone in one step (there is only one live version to be
gone from).

## Deprecation path

With exactly one supported version, **removal is a v2** — there is no "deprecated but still accepted"
grace state on the wire, because keeping the old shape alongside the new one *is* multi-version
support, which the stance above rejects. Deprecation therefore lives in **documentation, not in the
protocol**: a method slated for removal is announced as such in the docs (and in its `ludion explain`
text where a diagnostic is involved) ahead of the v2 that removes it, so a reader has advance notice
even though the wire itself flips atomically at the bump.

## The additive-rev checklist

Adding a `Command` variant is the parity gate: a variant with no handler in `ludion-engine`'s
exhaustive dispatch match is a **compile error**, and a variant missing its `Command::METHODS` entry
(in enum-declaration order) makes the envelope parser reject the otherwise-valid method as `L3002`.
Get the code compiling and the parity test green first, then run the re-bless in this order — **the
replay re-record is last for a reason**:

1. **Change the wire types** — add the variant plus its param struct, the `Command::METHODS` entry in
   enum-declaration order, the dispatch handler, and any new `ResponseBody`/`Event` shape.
2. **Bless the schema snapshot.** `BLESS_SCHEMA=1 cargo test -p ludion-protocol --test schema_snapshot
   -- --nocapture` rewrites `tests/protocol_schema.snapshot.json`, **prints the new hash** and fails until step 3.
3. **Hand-update `EXPECTED_SCHEMA_HASH`.** The bless rewrites the snapshot but does **not** touch the
   constant in `tests/schema_snapshot.rs` — copy the printed hash into it by hand, then re-run without `BLESS_SCHEMA` (it must pass). (A second test
   independently asserts the announced hash is the digest of the committed bytes, so a wrong paste is
   caught.)
4. **Regenerate the TS protocol types.** `npm run generate` in `editor/protocol/` turns the blessed
   snapshot into `protocol.d.ts` and the schema hash the editor compares at connect
   (`editor/app/src/protocol/schemaHash.ts`); CI byte-compares each regenerated file against the
   committed one.
5. **Re-record the `run.lrp` replays last.** A replay's `ReplayHeader` pins two hashes **at record
   time** (see "Two hashes" below), so any committed session recording must be re-recorded *after*
   they are final — do it first and it bakes in the old hashes and the replay gate refuses it.

This is one reviewed, atomic commit touching all five surfaces together (`command.rs`,
`protocol_schema.snapshot.json`, `schema_snapshot.rs`, `protocol.d.ts`, `games/*/run.lrp`) — verified
against the two Phase 6 additive revs, which each changed exactly that set.

**Two hashes.** `schema::schema_hash()` (handshake, `engine.describe`, the editor's `schemaHash.ts`,
the snapshot test) covers everything a generated client sees, prose included: a `///` edit on a
`Command` variant or param still fails the snapshot test until you re-bless. A replay header records
different hashes: `schema::replay_schema_hash()` (the same schema with `description`/`title` stripped)
and `replay_content_hash` (`ludion_engine::replay_content_hash()`: the record schema, the input events
and the snapshot document, prose stripped). So a doc-only edit re-pins the wire snapshot but never
the `.lrp` corpus; a new param, command, response field or snapshot field moves the replay hashes and
re-records. A stale hash, an absent content hash or another `protocol_version` refuses with `L3010`.

## Verify

```
cargo test -p ludion-protocol                 # code <-> committed snapshot + hash gate
BLESS_SCHEMA=1 cargo test -p ludion-protocol --test schema_snapshot -- --nocapture   # step 2
node ci/scripts/schema-drift.mjs              # full drift gate: snapshot -> TS -> byte-compare
```

The `schema-drift` CI job is the whole checklist mechanized in three layers — `cargo test -p
ludion-protocol` (code ↔ snapshot ↔ hash), `npm run generate` in `editor/protocol` (snapshot → TS),
and a byte-compare of the regenerated `protocol.d.ts` and `schemaHash.ts` — so an incomplete
re-bless (any one of the five steps
skipped) fails CI rather than shipping a lie.

## Gotchas

- **Additive still means regenerate.** A purely additive rev is source-compatible for callers that
  ignore the new capability, but the `schema_hash` still changed, so an un-regenerated client is
  refused at the handshake anyway (the editor shows a persistent L3003 drift banner instead).
  "Additive" is about the *wire shape's* backward safety, not about
  skipping the client regen.
- **The re-bless is a two-file commit** (`protocol_schema.snapshot.json` + the hand-edited
  `EXPECTED_SCHEMA_HASH`); the bless writes only the first. Forgetting the hand-edit leaves the snapshot
  and the constant disagreeing — caught by the snapshot test, never shipped.
- **Replay recordings are hash-stamped, so they re-record last** whenever a schema (not only prose)
  moved. A doc-comment edit does not touch them. A session `.lrp` that predates the current hashes is
  refused; a header with no content hash is reported as old-format, not as a
  determinism failure.
- **A v2 is not a bigger additive rev.** Renames/removals/semantic changes do not go through this
  checklist — they bump `PROTOCOL_VERSION`, and that is a deliberate project decision, not a routine
  re-bless.
