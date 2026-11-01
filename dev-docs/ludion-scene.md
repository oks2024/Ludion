---
description: Strict-subset YAML scene parsing, schema validation, and the canonical writer
---

# ludion-scene

**Scope:** The crate that owns all YAML knowledge in the engine: parsing the strict YAML 1.2 subset,
validating component values against the `Registry`'s JSON Schemas, emitting canonical text, and
instantiating a `Scene` into a `World`. This page is for changing the *scene* parser/validator/
emitter and the rules every document family shares. The **formats themselves** belong to the
authoring topics: `ludion docs scene-format` (the scene skeleton and the YAML subset),
the family topics `scene-kind-documents` lists. Component schemas and erased serde: `ludion devdocs ludion-ecs`; L-codes: `ludion devdocs diagnostics`.

## Spokes

- `ludion devdocs scene-kind-documents` — hub for the `kind:`-dispatched sibling families `peek_kind` routes to (`mesh` and the data-doc/profile machinery here); spokes `scene-kind-voxel`, `scene-kind-sculpt-map`, `scene-kind-sprite-font`, `scene-kind-generation` (with `mesh-edit`) hang off it.
- `ludion devdocs assembly-animation` — the three further families `peek_kind` routes to
  (`assembly`/`part`/`animation`), `clip_eval.rs`, and the engine path that consumes them.

## Layout

- `crates/ludion-scene/src/lib.rs` — the public surface: `Scene`/`SceneEntity`, `FORMAT_VERSION`, the scene functions (`parse`, `validate`, `check_component` (one wire value against its schema), `check`, `instantiate_into` and its fresh-world wrapper `instantiate`, `emit`), the override layer, and `peek_kind` (the `kind:`-dispatch sniffer).
- `crates/ludion-scene/src/yaml.rs` — the strict-subset layer over `saphyr_parser`: events → `Node`/`NodeValue`/`MapEntry`. **The only module in the engine that knows YAML syntax exists.** Rejects anchors/aliases (`L1002`), tags (`L1003`), multi-doc (`L1004`), non-empty flow (`L1005`), block scalars (`L1006`), duplicate keys (`L1007`), non-string keys (`L1008`), keys whose *canonical* spelling exceeds the scanner's 1024-char simple-key limit (`L1030` — quoting and `\u00XX` escapes expand a key, so the writer must never emit one the parser would reject), collections nested deeper than 128 levels (`L1034`), and any directive but `%YAML 1.2` (`L1037` — saphyr drops directives silently, so a pre-scan of the source text before the first `---` does it). A per-mapping `BTreeMap` key index keeps the duplicate check non-quadratic.
- `crates/ludion-scene/src/scalar.rs` — YAML 1.2 core-schema plain-scalar resolution and the writer's quoting rule (`plain_safe`, `quote_double`, `format_float`). One module on purpose: reader and writer must agree.
- `crates/ludion-scene/src/parse.rs` — scene-structure extraction and the fixed key sets; collects every structure error in one pass (`L1009`–`L1012`).
- `crates/ludion-scene/src/validate.rs` — component names against the registry (`L1013`) and values against JSON Schemas (`L1014`), for scene components and for wire values (`check_component`, run by `component.set`/`entity.spawn`); the checker implements exactly the keyword subset `schemars` emits. A key declared by an `allOf` sibling is not unknown; a `#[serde(flatten)]`ed *enum* (`oneOf`) is not yet modelled.
- `crates/ludion-scene/src/schema.rs` — `SchemaCursor`: the schema walk shared by validator and emitter; resolves local `$ref`s (16 hops) and exposes `property_order()`.
- `crates/ludion-scene/src/emit.rs` — the canonical writer.
- `crates/ludion-scene/src/overrides.rs` — render-time overrides (`SceneOverride`, `apply_overrides`, `parse_sweep`) — see Dataflow.
- `crates/ludion-scene/src/mesh.rs`, `crates/ludion-scene/src/voxel.rs`, `crates/ludion-scene/src/voxel_edit.rs`, `crates/ludion-scene/src/voxel_gen.rs`, `crates/ludion-scene/src/map.rs`, `crates/ludion-scene/src/doc.rs` (and `sculpt.rs`, `font.rs`, `sprite.rs`, `atlas.rs`, `generation.rs`, `mesh_edit.rs`) — the sibling `kind:` families and generic data-doc entry points; `ludion devdocs scene-kind-documents` routes to each.
- `crates/ludion-scene/src/prefab.rs` — `kind: prefab`: the body is a scene's `entities:` list, so it reuses `parse::parse_entity`, `validate::validate` (through `PrefabDocument::to_scene`, which ranges entity references over the prefab's own parts) and `emit::emit_entity`. Prefab-only rules: the envelope and scene-only keys (`L1226`), part names (`L1227`/`L1228`). The singleton refusal (`L1229`) names concrete components, so it is `ludion_engine::prefab_singleton_diagnostics`; `check-scene`'s prefab arm also runs the hierarchy body rules. The atlas, skin-bone and collider-layer passes need the content root or the spawning scene's `physics:`, so they run at spawn time.
- `crates/ludion-scene/src/instantiate.rs` — `instantiate_into` (scene entities → an existing `World`, all-or-nothing, ids remapped) and its fresh-world wrapper `instantiate`, via the registry's erased deserialization.
- `crates/ludion-scene/tests/` — `roundtrip.rs` (value→emit→parse→equal; emit→parse→emit byte-equal), `validation.rs`, `instantiate.rs`, and a `*_roundtrip.rs` per family for byte-idempotence.

## Rules

- **`ludion-ecs` never learns YAML exists.** This crate consumes the registry's schemas and erased
  serde fns and never defines a component. Component values are carried as `serde_json::Value` +
  registry name.
- **Canonical emission never loses data.** Floats are ryu shortest-round-trip (`format_float`), never
  fixed precision — fixed precision is a data-loss bug. At a schema position marked
  `"format": "float"` (schemars' mark for `f32`) an f32-exact value — an engine component widened by
  `serde_json::to_value` — is written as ryu's shortest *f32* decimal (`format_f32_float`: `0.3`, not
  `0.30000001192092896`); it reads back as the same `f32`, so `WorldHash` does not move. So is the
  f64 reading of such a decimal, which keeps `emit` a fixed point. `f64` fields and every
  hand-written kind emitter keep the f64 path. Integers stay integers (`1` and `1.0` are different
  scalars). Quantization happens at the source (e.g. a gizmo), never in the writer. Committed
  hand-authored scenes and prefabs are spelled as the writer spells them, gated by
  `ludion_testkit::assert_saves_respell_nothing`.
- **Emission is idempotent**: `emit(parse(emit(s)))` is byte-equal to `emit(s)`, property-tested in
  `tests/roundtrip.rs`. Every sibling family holds the same property.
- **Reader/writer quoting must agree**: the writer emits a string unquoted only if `resolve_plain`
  would give the same string back. `plain_safe` is deliberately conservative (identifier-ish strings
  only). Change one half only in lockstep with the other.
- **Every document type holds `f64`, never `f32`**, so text round-trips losslessly and the crate never
  names a GPU type. Narrowing happens engine-side when buffers are built.
- **Structure and semantics are separate passes** in every family — `parse_*` checks shape only,
  `validate_*`/`generate` checks meaning. Syntax and strict-subset violations fail fast (one
  diagnostic — a broken token stream is unrecoverable); structure and validation errors are
  *collected*, so an agent fixes a bad document in one iteration.
- **Document position is not the entity index** once a `presentation: true` entity interleaves with sim ones (separate allocator ranges). `instantiated_ids` is the one mapping; never index `world.entities()` by position.
- Every diagnostic carries an `L1###` code, a JSON Pointer path where applicable, and a suggested fix.
  Suggestions must be deterministic — pass candidates to `ludion_core::suggest::nearest` in a
  deterministic order (registry order, schema property order); it compares case-insensitively, returns the original casing, and ties keep the earliest candidate.
- `#![forbid(unsafe_code)]`; sim-side clippy denylist.

## Dataflow

`Scene { name, seed, entities }`; `SceneEntity { name, components }`. `format` is
not stored — the crate reads and writes exactly `FORMAT_VERSION` (1), and the emitter always writes
`format: 1` first. The pipeline is `parse` → `validate` → (`emit` | `instantiate`), each stage running
once:

- **`parse`** drives `yaml::parse_document` (fail-fast) then `parse::parse` (collecting). Plain
  scalars resolve per the YAML 1.2 core schema; quoted scalars are always strings; values JSON cannot
  represent (`.inf`, `.nan`, 65-bit ints) are rejected with `L1009`, so nothing unrepresentable ever
  reaches a `serde_json::Value`.
- **`validate`** returns `Vec<Diagnostic>` (empty = valid). The checker understands `type`, `enum`,
  `const`, `properties`/`required`/`additionalProperties`, `items`/`prefixItems`/`minItems`/
  `maxItems`, numeric bounds, `oneOf`/`anyOf`/`allOf` and local `$ref`; unknown keywords are ignored.
  Two strictnesses: **undeclared fields are errors**; `integer` rejects `1.0`.
- **`check`** = parse + validate — what `ludion check-scene` and `scene.check` run.
- **`instantiate_into`** is the one instantiation primitive: it spawns one entity per scene entity in
  document order into a world and inserts via `ComponentInfo::deserialize_insert`, remapping every
  marked reference to the new id (index and generation). `InstanceHooks` is the engine-typed half
  (naming, placing roots) — a trait because this crate never names `Transform`/`Name`. `instantiate`
  is the same on `World::new()`. Neither re-validates: an unchecked scene fails with registry-level `L0004`/`L0006`, not `check`'s richer diagnostics. Entity `name`s reach the world only through the hooks.
- **`emit`** writes the canonical form. Key order: top-level `format`, `name`, `seed`, `entities`;
  entity `name`, `components`; components in **registry registration order** (unregistered names
  follow in document order); fields in schema property order = struct declaration order, read through the alternative a value takes (`oneOf`/`anyOf`/`allOf`; an internally tagged enum's tag leads), map (`additionalProperties`) keys in byte order. Entity order
  is data and is preserved exactly. Emission never fails.

**Overrides** (`overrides.rs`) are applied **between `parse` and `validate`**, so an ill-typed override
is rejected by the schema exactly as the same edit to the file would be — there is no second rejection
path. They replace values only: never create a field, a component, or an entity (`L4008`).
`parse_sweep` is the `<json-pointer>=<v,v,…>` spelling behind `--sweep`; both spellings share one
private `parse_value` (JSON first, plain string on failure) so the same text cannot mean two things.
The sweep split on `,` has **no escape** (no comma in a value) and trims no whitespace.

## Verify

```
cargo test -p ludion-scene                                        # round-trip, validation, instantiate, doctests
cargo run -p ludion-cli -- check-scene examples/scenes/empty.yaml # prints OK
cargo run -p ludion-cli -- check-scene <broken.yaml>              # L-code + JSON Pointer + suggestion
cargo nextest run -p ludion-cli --test it -E 'test(/^check_scene::/)'                       # CLI-level; family verifies: `ludion devdocs scene-kind-documents`
```

## Gotchas

- `SchemaCursor` is best-effort by design: an unresolvable `$ref` ends navigation — the emitter falls back to document order and the validator skips what it cannot see. A wrong-looking field order in emitted output usually means the schema walk lost track — no `oneOf`/`anyOf` alternative matched the value (`SchemaCursor::select`) — not an emitter bug. `crates/ludion-scene/tests/emit_shapes.rs` pins one golden per schema shape, so a `schemars` bump fails there by name.
- The emitter needs the `Registry` for ordering; emitting with a different registry (or registration order) than the one a file was written under produces a different — still valid — canonical text. Registry order is part of the canonical form.
- A single leading `---` is tolerated on parse but never emitted; content after it is `L1004`.
- `components: {}` and `entities: []` are the only legal flow forms — the emitter uses them for the empty cases, and the parser rejects every non-empty flow collection.
- Arrays are block-style (`L1005` rejects non-empty flow): `origin`, `color`, `scale`, `collider.aabb`, `footprint`, `rows` are one element per line.
