# Plan: Protocol evolution — a shape-only hash, checked by both ends

> Status: **planned, not started.** Source: the 2026-09-25 code audit, finding J8 (with F12).
> Current-code facts were read from the tree on 2026-09-26; file:line pointers are of that date, so
> re-grep before editing. The policy this refines is `ludion devdocs protocol-evolution`.

## Trigger

The policy is one live protocol version plus a strict `schema_hash`. That stays. What does not hold
up is what the hash covers and who checks it:

- **The hash covers prose.** A doc-comment edit on a wire type moves it, and breaks every pairing across
  revisions. Contributors already dodge this by writing `//` instead of `///` on wire types
  (`envelope.rs:213`, `event.rs:33`, `response.rs:40`), which strips that documentation from the
  generated TypeScript.
- **Only the Rust client checks the hash.** The server never learns the client's hash. The editor
  checks only `protocol_version`: it compares a reconnect's hash with the first server's, never with the
  hash its own `protocol.d.ts` was generated from.
- **`Hello.features` is accepted and silently ignored**, and `ServerHello` has no capability field. A
  field that reads as a feature but does nothing is a trap.

Build it when **any one** of these happens:

- the external shmup switches to a git dependency pinning Ludion revisions, the first real
  cross-revision client;
- a `PROTOCOL_VERSION` bump is proposed (the capability decision must precede it);
- an editor/engine skew incident, where the editor runs against a server whose shapes differ;
- the next schema-moving change. W1's one-time hash move then rides that change's re-bless and replay
  re-record instead of costing its own.

## What exists (2026-09-26)

- **`PROTOCOL_VERSION = 1`** (`crates/ludion-protocol/src/lib.rs:74`).
  - The hash is XXH3 over the canonical, key-sorted schemars document (`schema.rs:36-99`). The snapshot
    holds 662 `description` keys.
  - Pinned as `EXPECTED_SCHEMA_HASH` (`tests/schema_snapshot.rs:22`, currently `be7ebe1877875ed1`) and
    gated by `ci/scripts/schema-drift.ps1`.
  - `dev-docs/protocol-evolution.md:90-93` documents that prose moves the hash, as intended.
- **`Hello { protocol_version, client, features }`** and
  **`ServerHello { engine_version, protocol_version, schema_hash }`** (`handshake.rs:28-60`).
  - The server discards the parsed `Hello` (`crates/ludion-server/src/connection.rs:264`) and checks only
    the version (`parse_hello`, `:754-776`).
- **`ludion-client`** sends `features: []` and rejects a hash mismatch with `L3003`
  (`crates/ludion-client/src/lib.rs:133-197`).
- **The editor.**
  - `editor/app/src/protocol/client.ts:155,178` checks the version.
  - `App.tsx:746-800` pins the first server's hash across reconnects.
  - `protocol.d.ts` carries no hash constant.
  - The Tauri host (`editor/app/src-tauri/src/wire.rs:11,25`) hardcodes the version and deliberately
    skips the drift check.
- **Replays pin the hash** in `ReplayHeader` (`dev-docs/engine-replay.md`). A hash move means the `.lrp`
  corpus is re-recorded last.
- Multi-version negotiation and capability flags are recorded as rejected
  (`dev-docs/protocol-evolution.md:27-32`).

## Proposed design

### W1 — hash wire shapes only

- `schema_hash` strips `description` (and `title`, `examples`, if present) from a copy of the document
  before hashing.
- The committed snapshot keeps the prose, so the TypeScript generation still gets it.
- A test pins that editing a doc comment leaves the hash unchanged, and that renaming a field moves it.
- The `//` workaround comments go back to `///`.
- **One-time move of the hash value:** re-bless the snapshot and `EXPECTED_SCHEMA_HASH`, regenerate
  `protocol.d.ts`, and re-record every `.lrp` last. This follows the five-surface checklist in
  `protocol-evolution.md`, and the doc's "comment edits move the hash" paragraph is rewritten.

### W2 — both ends check

- **The handshake.**
  - `Hello` gains `schema_hash: String`.
  - The server compares it with its own and refuses a mismatch with `L3003`, naming both hashes and the
    fix ("regenerate the client").
  - Every in-repo client sends it: `ludion-client`, the editor, the Tauri host and the MCP adapter, if
    it handshakes.
  - This is itself a wire change, so ride it with W1's re-bless.
- **The editor.**
  - The schema-drift gate emits `export const SCHEMA_HASH = "…"` into the generated `protocol.d.ts`.
  - `client.ts` sends it and also compares it with `ServerHello.schema_hash`. The per-session pinning
    in `App.tsx` becomes redundant: delete it rather than keep two checks.
- **The Tauri host.** It gets the constant through the same generated file, or a build-time include,
  and its version-only exemption is removed.

### W3 — decide `features` before the first bump

The written policy rejects capability flags. Keep that. Then `Hello.features` has no meaning, and a
field with no meaning is removed, not ignored.

Recommendation: delete `Hello.features` in W2's wire change. Record in `protocol-evolution.md` that
host optionality (no GPU, no window, headless) is reported through existing describe/stat methods, not
a handshake channel.

If a real consumer needs negotiation before a bump (the shmup pinning an older revision against a newer
editor, say), reopen the rejected alternative there with that consumer's case. Do not reopen it here.

## Risks

- **Stripping too much.** A schemars keyword that carries shape but looks like prose (a `format`, an
  `enum` of descriptive strings) must stay in the hash. Strip by key name from a closed list, never by
  heuristic. The rename test and an enum-variant test are the guard.
- **The editor and engine ship together in-repo, so a strict check is cheap.** An external client
  built against a pinned revision gets a hard refusal at connect time. That is the policy, and the
  error must say which side to regenerate.
- **Replay corpus churn.** W1 and W2 each move every replay header. Land them as one change.

## Exit gate

- `node scripts/check.mjs` green, and `schema-drift.ps1` green.
- A doc-comment edit on any `Command` variant leaves `EXPECTED_SCHEMA_HASH` unchanged; a field rename
  fails the snapshot.
- A client sending a stale hash is refused at handshake, from each client (Rust and TypeScript tests).
- `protocol-evolution.md` is rewritten for the new rule; `rg '"features"' crates/ludion-protocol` finds
  nothing, or a documented meaning.

## Status

Planned, not started (2026-09-26). The editor's reconnect-time hash pin already covers part of what
F12 asked for; W2 replaces it with a build-time check.
