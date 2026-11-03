---
description: The wire contract: envelopes, the Command surface, replay format, and the schema drift gate
---

# ludion-protocol

**Scope:** The contract layer of the wire protocol: the `Command` / `ResponseBody` / `Event` enums,
the request/response/event envelopes, the handshake, the binary frame header, the replay session
format, and the deterministic schema export with its stable `schema_hash`. This crate *defines* the
parity gate — a capability with no `Command` variant here does not exist. What *executes* commands
(the Engine, `dispatch.rs`, recording) lives in `ludion-engine`; the WS host in `ludion-server`;
the sync SDK in `ludion-client`. Boundary rules are in `crate-graph`; L-codes in `diagnostics`.

## Layout

- `crates/ludion-protocol/src/lib.rs` — re-exports, `PROTOCOL_VERSION` (= 1), and `schema_proxy`: private `JsonSchema` mirrors for `ludion-core` types (`EntityId`, `Diagnostic`) that don't derive it, with tests pinning the mirrors to the real serde shapes.
- `crates/ludion-protocol/src/envelope.rs` — `Request`, `Response`/`WireResult`, `EventMessage`, `RequestId`/`SubId`/`Seq`; `Request::from_json`/`from_value` with `L3001`–`L3003` diagnostics.
- `crates/ludion-protocol/src/command.rs` — the exhaustive `Command` enum (38 methods), per-method param structs, `Command::METHODS`, `Channel`, `SubscribeMode`.
- `crates/ludion-protocol/src/response.rs` — `ResponseBody` (tagged by `kind`), `EntityMatch`.
- `crates/ludion-protocol/src/event.rs` — `Event` payloads for the non-frame channels (`log`, `sim.events`, `scene.changed`, `asset.reloaded`, `profile`).
- `crates/ludion-protocol/src/perf.rs` — the performance-report types both producers fill: `ProfileReport` (`sim`/`gpu`/`transport`/optional `samples`), `ProfileWindowData` (+ `into_report`), `ProfileSamples`, `ProfileCaptureParams`. Measured in three other crates, shaped here because they only ever reach a client through the wire — see the `profiling` topic.
- `crates/ludion-protocol/src/handshake.rs` — `Hello` / `ServerHello` (announces `schema_hash`).
- `crates/ludion-protocol/src/frame.rs` — binary frame-stream message: `LFR1` magic, 36-byte little-endian `FrameHeader`, `FrameFormat` (QOI / LZ4_RGBA / RAW_RGBA), a flags byte (bit 0 = `gi_stale`), `encode`/`decode` (`L3004`).
- `crates/ludion-protocol/src/replay.rs` — the replay container (`LRP1`); see `ludion devdocs protocol-replay`.
- `crates/ludion-protocol/src/hash.rs` — `WireHash`: XXH3-64 serialized as exactly 16 lowercase hex digits (string, never a JSON number).
- `crates/ludion-protocol/src/schema.rs` — `protocol_schema()`, `schema_document()`, `schema_hash()`.
- `crates/ludion-protocol/tests/` — `schema_snapshot.rs` + the committed `protocol_schema.snapshot.json`.

## Rules

- **The protocol never names a concrete component type.** The crate depends on `ludion-core`
  **only** — never ecs/scene/sim/engine/renderer. Component values, spawn payloads, query results,
  injected input events, snapshots, and graph/timing dumps all travel as `serde_json::Value` plus a
  registry *name* (`String`); the dispatcher validates them against the component registry. The
  moment `Transform` appeared in a struct here, a protocol→ecs cycle would begin.
- **One derive, three artifacts.** Every wire type derives serde **and** `schemars::JsonSchema`;
  that single source powers the Rust dispatch match, the exported JSON Schema, and the generated TS
  types in `editor/protocol/`. They cannot drift independently.
- **`PROTOCOL_VERSION` is breaking by design.** v1 speaks exactly one version; a mismatched
  envelope `v` is rejected with `L3003` ("regenerate the client"), never negotiated down.
- **Adding a `Command` variant is the parity gate** — a variant without a dispatch handler is a
  compile error in `ludion-engine`'s exhaustive match. Every new variant also needs its
  `Command::METHODS` entry *in enum-declaration order* (a unit test compares METHODS against the
  generated schema's `oneOf`; a missing entry makes the envelope parser reject the valid method as
  `L3002`), plus a snapshot re-bless.
- Payload maps are `BTreeMap`, never `HashMap` — the crate carries the same strict clippy denylist
  as `ludion-core` (no std hash containers, no wall clock). `#![forbid(unsafe_code)]`,
  `#![warn(missing_docs)]`.
- Replay container, its one format and the parsers' never-panic guarantee (hostile input becomes an L-coded diagnostic; `L3017` decompression ceiling): `ludion devdocs protocol-replay`.

## How it works

Every JSON message on the wire is one of three envelopes:

```text
→ {"v":1, "id":42, "method":"entity.spawn", "params":{…}}                    Request
← {"id":42, "ok":{"kind":"spawned", …}, "frame":1203, "game_frame":1150}   Response (success)
← {"id":42, "error":{"code":"L…","message":"…","path":"…","help":"…"}}       Response (failure)
← {"event":"scene.changed", "sub":7, "seq":981, "frame":1204, "data":{…}}    EventMessage
```

`Command` is one `#[serde(tag = "method", content = "params")]` enum; the dotted method name
(`scene.load`, `entity.spawn`, `component.set`, `component.remove`, `query.entities`, `sim.step`,
`input.inject`, `render.screenshot`, `stream.subscribe`, `engine.snapshot`, …) is the serde tag, and
parameterless methods (`sim.pause`, `sim.hash`, `engine.describe`, …) are unit variants serializing
with **no** `params` key. `Request::from_json` maps failures precisely: not-an-object / missing `v` /
params that don't match the method → `L3001`; unknown method → `L3002` with a nearest-method
suggestion; wrong version → `L3003`. Success responses are a `kind`-tagged `ResponseBody` (`Ack` for
effect-only commands) with the applied sim `frame` and the `game_frame` (the gameplay frame at the
same boundary, the value a `started` stamp uses) on the *envelope*, so every success carries them
uniformly. Three mutations carry their undo inverse instead of a bare `Ack`, so a client can build
an undo stack without a racy read-before-write: `component.set` → `{previous: Value|null}` (`null` for a
fresh insert), `component.remove` → `{previous: Value}` (the removed value; `component.set` with it
re-inserts), `entity.destroy` → `{components: {name → value, …}}` (the entity's full component state
at the moment of destruction, in registry order, `Name` included like any other component —
`entity.spawn` with that map respawns it under a fresh id). `entity.spawn` and `component.set` check
every component value before touching the world — its JSON Schema first (`L1014`, a component-relative
pointer and nearest-field suggestion, the checker the scene path runs), then its semantic validator;
a self-joined `Joint` is `L1183` there too, not a wire-only escape hatch. They also check the entity references a value holds: a `Parent` takes the `entity.reparent` rules (`L0002`, `L1221`, `L1222`, `L1224`, `L1225`, `L1260`), any other marked field refuses naming its own carrier where the schema forbids it (`L1017`) and a sim carrier naming a presentation id (`L1036`; `entity.spawn` accepts a dead one); a dead non-`Parent` reference is allowed (`scene.save`'s `L1018` catches it). `L0006` remains only for what
serde rejects that the schema allows. `component.remove` shares
`component.get`'s exact error semantics: `L0004` unregistered name, `L0002` dead entity, `L3006` a
live entity that simply doesn't carry the component. Errors are `ludion-core` `Diagnostic`s serialized
whole. Subscriptions: `stream.subscribe` picks a
`Channel` and a `SubscribeMode` (`latest_wins` with optional `max_fps` — the lossy viewer
default, the only mode); events carry `sub` + a monotonic `seq` whose gaps are visible, intentional
drops. The `frames` channel alone is binary (`frame.rs`), keeping megabytes of pixels off the
base64 JSON path; `render.screenshot` is the one-shot JSON alternative.

`render.bounds` is the read-only introspection/pick primitive — "what did I draw, and where". It is
**pure CPU** (extract + projection math, no GPU, so it answers on an adapterless machine like
`render.dump_graph`) and returns, for every renderable entity under the current camera in
ascending-slot order, its `entity`, world-space `world_aabb` (`{min, max}`), and the pixel
`screen_rect` (`{x, y, w, h}` at the engine's `render_size`, echoed in the response). Its world
geometry comes from the *same* `extract` sizing path a real render uses, so a reported bound can
never disagree with what would be drawn. Two honesty signals distinguish "off screen" from "behind
the camera": `screen_rect` is `null` when no part of the AABB lies in front of the camera's near
plane (a box crossing it reports the rect of its drawn part), and `on_screen` is true only when a
non-null rect overlaps the viewport. Camera rule matches a render's — exactly one camera, more than
one is `L2101` — with one deliberate difference: a scene with *no* camera is `L2102` (a render
falls back to a default framing, but a projected rect needs a real viewpoint).

`schema.rs` assembles the JSON Schema of every wire type into one document, **canonicalized** by
recursively sorting object keys (array order is semantic and preserved), so the bytes don't depend
on schemars emission order. `schema_hash()` is the XXH3-64 of exactly those bytes, and
`ServerHello` announces it, so a client generated from a stale schema fails **at connect time**,
not just in CI. The committed snapshot (`tests/protocol_schema.snapshot.json` +
`EXPECTED_SCHEMA_HASH` in `tests/schema_snapshot.rs`) makes any wire-type change a deliberate,
reviewed re-bless. The policy for that re-bless — the additive-rev checklist, what forces a v2, and
why there is only one live version — is `protocol-evolution`.

## Verify

```
cargo test -p ludion-protocol                 # round-trips, replay seek, snapshot + hash gate
BLESS_SCHEMA=1 cargo test -p ludion-protocol --test schema_snapshot -- --nocapture   # re-bless
node ci/scripts/schema-drift.mjs              # full drift gate, from the repo root (node 22+)
```

The `schema-drift` CI job has three layers: `cargo test -p ludion-protocol` (code ↔ committed
snapshot), `npm run generate` in `editor/protocol` (snapshot → TS types), and a byte-compare of the
regenerated `protocol.d.ts` against the committed one. The regeneration goes to a scratch directory, so the gate never writes a tracked file. The VCS-agnostic script is
`ci/scripts/schema-drift.mjs` (run as-is by the private TeamCity configs and, with `--fast`, by `check.mjs`), with the
`schema-drift` job in `.github/workflows/ci.yml` as the public definition.

## Gotchas

- Re-blessing is a **two-file** commit: `BLESS_SCHEMA=1` rewrites the snapshot and then **fails**
  until you copy the printed hash into `EXPECTED_SCHEMA_HASH` by hand (re-run without it to pass); a second test independently asserts the
  announced hash is the digest of the committed bytes.
- `schema_document()` is pretty-printed **and newline-terminated**; the hash is over exactly those
  bytes. Don't "clean up" the trailing newline.
- The `schema_proxy` mirrors (`EntityIdSchema`, `WireErrorSchema`) are tied to the real
  `ludion-core` serde shapes only by unit tests in `lib.rs` — a field added to `EntityId` or
  `Diagnostic` without the proxy following is a silent schema lie until those tests catch it.
- `WireHash` parses uppercase hex leniently (matching `WorldHash`) but always displays/serializes
  lowercase; `WireHash::parse` rejects anything that isn't exactly 16 hex digits.
- `sim.set_speed` carries a plain `f64`; validation (finite, > 0 → `L4001`) happens at dispatch,
  not here. Same pattern for `input.inject`'s `event` (`L3005` at dispatch) and `engine.restore`'s
  `snapshot` (`L3008`): the protocol layer never interprets them.
