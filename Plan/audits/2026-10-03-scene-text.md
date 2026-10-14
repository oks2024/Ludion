# Audit: the scene text pipeline (strict YAML, canonical writer, schema validation)

**Date:** 2026-10-03 · **Rev:** `191246d3` · **Report only — no code changed.**

**Scope.** `crates/ludion-scene` (`yaml.rs` strict-subset reader over saphyr-parser, `scalar.rs`
plain-scalar resolution and quoting, `emit.rs` canonical writer, `schema.rs` cursor, `validate.rs`
checker, `parse.rs`, `doc.rs` data-docs, `kinds.rs` kind sniffing, `fields.rs`; the per-kind emitters
were scanned for non-canonical number formatting), the write-back paths (`scene.save` in
`crates/ludion-engine/src/commands/scene.rs` → `Engine::scene_text` / `world_to_scene` /
`flatten_ids` in `engine.rs`; `component.set` / `entity.spawn` in `commands/entity.rs`), the data-doc
content walk (`crates/ludion-engine/src/docs.rs`), the profile reader (`player_profile.rs`),
`check-scene` (`crates/ludion-cli/src/check_scene.rs`, `crates/ludion-engine/src/scene_check.rs`),
the editor's second YAML writer (`editor/app/src/editor/effects/yamlDoc.ts`), the `generated_from:`
drift gate (`scripts/map-drift.mjs`), and the fuzz targets `scene_parse`, `canonical_idempotence`,
`kind_roundtrip` (`fuzz/src/lib.rs`). Findings already in `Plan/engine-audit-2026-07-17.md` (oneOf
validated as anyOf, the five envelope checks, the duplicated `scalar_string`, now fixed) are not
repeated.

## Overall assessment

The reader and writer are careful work. The strict-subset layer rejects anchors, tags, multiple
documents, non-empty flow collections, block scalars, duplicate and complex keys, and over-deep
nesting, each with an L-code and a JSON Pointer. Plain-scalar resolution follows YAML 1.2 core and
quotes YAML 1.1 lookalikes. Floats are ryu everywhere, and property tests pin bit-exact float and
arbitrary-string round trips. The serious problems are at the edges where the engine meets the text.
`scene.save` writes every `f32` field as its widened `f64` digits, so any save of a hand-authored
scene rewrites every short decimal: the minimal-diff promise fails in the pillar's main use case.
The wire's `component.set`/`entity.spawn` skip the JSON Schema checker that guards files, so a
typo'd field name is accepted silently and resets the real field to its default. The data-doc
envelope also collides with any type that has a top-level `kind` or `format` field, which for a
player profile means a save that cannot be read back.

## High

### S1 — `scene.save` widens every `f32` to 17 digits, so a load → save of a hand-authored scene rewrites every short decimal · High · [verified]

**Evidence.** Engine components are `f32` (`crates/ludion-engine/src/components.rs:59-69`, e.g.
`Transform.translation: [f32; 3]`; 79 `f32` fields against 0 `f64` component fields). `flatten_ids`
reads each component back through `info.serialize` (`engine.rs:3790`), which is
`serde_json::to_value` (`crates/ludion-ecs/src/registry.rs:450`). serde_json 1.0.150 stores an `f32`
as `N::Float(f as f64)` (`number.rs:323-328`, without `arbitrary_precision`). `leaf_text` then formats
that `f64` with ryu (`crates/ludion-scene/src/emit.rs:329-331`). The writer's own unit test pins the
behaviour: `format_float(f64::from(0.3_f32)) == "0.30000001192092896"` (`scalar.rs`, test
`float_format_is_shortest_round_trip`).

Reproduced over the wire against `ludion serve` with a scratch scene holding
`translation: [0.3, 0.9, 1.0e-7]`. `scene.save` wrote:

```yaml
        translation:
          - 0.30000001192092896
          - 0.8999999761581421
          - 1.0000000116860974e-7
```

By the same path an integer authored in an `f32` field (`scale: [1, 1, 1]`) comes back as `1.0`.

**Why it matters.** Pillar 4 and `engine-architecture.md` §7 promise that any client's edit produces
a minimal diff and that the canonical form is "minimal-diff, *and* lossless". Here the writer is
lossless only for the `f32`. The text the human or agent wrote is replaced by digits nobody typed.
A rough scan found that 71 of the 79 hand-authored scenes under `games/` and `examples/` contain
short decimals that are not `f32`-exact. This is an upper bound, since some may sit in non-`f32`
fields. The repo also holds two conflicting float conventions. The editor's TS writer deliberately
writes the shortest `f32` decimal (`yamlDoc.ts`, `shortestFloat`), on the stated belief that "a
canonical Rust writer would emit … ryu's shortest form". The importer deliberately writes the
widened form so that load → save is a fixed point (`dev-docs/ludion-import.md:69-73`). The existing
save tests only compare save → save (`prefab_save.rs:206-209`, `prefab_presentation.rs:303-306`),
never source → save, so the gate stays green.

**Failure scenario.** A designer opens `camp-core.yaml` in the editor, nudges one prop with the
gizmo, and saves. The diff touches every non-dyadic float in the file (`radius: 0.9` →
`0.8999999761581421`, and so on), burying the one-line change. An agent hand-writes `speed: 0.3`.
The first round trip through the engine rewrites it, and later hand edits now compete with
17-digit values.

**Suggested fix.** Format at the schema position, which the writer already knows. Where the
component schema says `format: "float"` (schemars emits it for `f32`) and `v == (v as f32) as f64`,
emit `ryu::Buffer::format(v as f32)` (the shortest `f32` decimal, which reads back to the same
`f32`). Keep the `f64` path elsewhere. Then align `ludion-import`'s rule and re-run the import and
mesh drift gates. Also add one gate that committed hand-authored scenes are already the engine's
load → save fixed point.

### S2 — Wire `component.set` / `entity.spawn` skip the JSON Schema checker: a typo'd field is accepted and silently resets the real field · High · [verified]

**Evidence.** `crates/ludion-engine/src/commands/entity.rs:162-174` runs only the semantic validator
and then `registry.deserialize_insert`. Its comment says "a schema-invalid value is caught by
`deserialize_insert` below (L0006)", but serde only catches what serde rejects. No component uses
`deny_unknown_fields` (0 hits in `components.rs`), and most fields are `#[serde(default)]`.
`entity.rs:57-62` (spawn) takes the same route. The file path does run the checker and does reject
undeclared fields with a suggestion (`validate.rs`, `check_object`, `L1014` "nearest match is …").

Reproduced against `ludion serve`:
`component.set {component: "Transform", value: {translaton: [5,0,0], scale: [1,1,1]}}` returned
`ok`. The following `component.get` showed `translation: [0,0,0]` and `rotation: [0,0,0]`, and
`scene.save` wrote the zeroed transform.

**Why it matters.** `engine-architecture.md` §7 says the registry's JSON Schema "serves YAML files
and JSON wire payloads alike". `CLAUDE.md` (Settled architecture) says it validates "file and wire
payloads". The same typo is an `L1014` with a suggestion in a file and a silent data reset on the
wire. That is a direct hit on the North Star: an agent's mistake costs a screen check to find
instead of one error message.

**Failure scenario.** An agent sends `component.set` with `{"colour": [...]}` or omits a field it
meant to keep. The engine answers `ok`, the entity renders wrong, and a later `scene.save` commits
the reset value.

**Suggested fix.** Expose the checker for one component value (for example
`ludion_scene::check_component(name, value, registry) -> Vec<Diagnostic>`, reusing `Checker` with
`Subject::Component`). Call it in `set` and in spawn before `deserialize_insert`, returning the first
`L1014` with its pointer and suggestion. Add a dispatch test for the typo case.

## Medium

### S3 — A data-doc or profile type with a top-level `kind` or `format` field cannot round-trip: `emit_doc` writes a duplicate key, and `check_doc` strips the field · Medium · [verified]

**Evidence.** `emit_doc` writes `format: 1` and `kind: <kind>`, then every field of the value
(`emit.rs:183-194`). `check_doc` strips both envelope keys before validating
(`doc.rs:99-101`). Nothing at registration rejects such a field (`builder.rs:362-385`,
`docs.rs:228-239`). A scratch program calling `ludion_scene::emit_doc("weapon", {kind: "melee",
damage: 12.0}, schema)` produced:

```yaml
format: 1
kind: weapon
kind: melee
damage: 12.0
```

`check_doc` on that text returns `L1007 the key \`kind\` appears twice`. For profiles, the engine
writes with `emit_doc(kind: profile)`, and on any `open_profile` error the reader backs the file up
and starts a default profile (`player_profile.rs:254-255`).

**Why it matters.** `kind` is the most natural field name a game author reaches for, and today's
vigil types only escape because theirs are nested (`games/vigil/src/docs.rs:464,791`). A top-level
one fails in one of three ways. A required field fails every hand-written doc with "missing
required field `kind`". A `#[serde(default)]` field silently reads its default. On a profile, every
save produces a file the next boot resets.

**Suggested fix.** Reject `format`/`kind` among a doc or profile schema's top-level properties when
the kind is registered, with a readable message that names the field and suggests a rename. Add a
`debug_assert`/test in `emit_doc` that the value has no envelope keys.

## Low

### S4 — Diagnostic columns are 0-based while lines are 1-based · Low · [verified]

`yaml.rs` documents nodes as "1-indexed line/column" (`yaml.rs:45`) and prints
`span.start.col()` / `err.marker().col()` directly (`yaml.rs:142,336,443,486`). saphyr's column is
0-based. `check-scene` on `entities: [1]` reports `L1005 … (line 2, column 11)`, but the `1` is at
1-based column 12. `  - name: &x a` reports column 13 for the scalar at 1-based column 14. Editors
and rustc use 1-based columns, so every "column" an agent jumps to is one left. Fix: add 1 at the
three print sites (and to `Node.col`).

### S5 — Duplicate-key detection is quadratic per mapping · Low · [verified]

`push_node` runs `entries.iter().find(|e| e.key == key)` for every key (`yaml.rs:331`). Timed with a
release build: 5 000 keys took 31 ms, 20 000 took 0.41 s, and 40 000 took 1.63 s, for one flat
mapping. Committed documents are far below this today, but generated tables (atlas regions, glyph
maps, `additionalProperties` maps) are the growth axis and every boot walk parses them. Fix: keep a
`BTreeSet<&str>`/sorted index per mapping frame alongside `entries`.

### S6 — `scene.save` is a truncate-and-write, unlike every other engine/CLI writer · Low · [verified]

`commands/scene.rs:74` uses `std::fs::write` straight onto the target. The profile writer
(`player_profile.rs:478`), `ludion dev` (`dev.rs:386`), `sculpt` (`sculpt.rs:118`), `voxel`
(`voxel.rs:706`) and `sync` all write a temp file and rename it. A crash, a full disk (a documented
hazard on the reference box) or a concurrent reader (a watcher, the editor's scene picker) can see a
truncated scene. Fix: the same temp-plus-rename.

### S7 — Stale docs about the pipeline · Low · [verified]

- `docs/scene-format.md:64` says top-level keys are "exactly `format`, `name`, `seed`, `physics`,
  `entities`", and `:73` gives the canonical order without `generated_from` (the writer emits it
  between `name` and `seed`, `emit.rs:40-44`).
- `dev-docs/ludion-engine.md:137-138`: "`scene.save` round-trips components but not entity names …
  they come back as `null`". False: the save in S1 wrote `name: a`.
- `docs/patterns.md:34-35` (and its `embedded-docs/` mirror), plus the comments in
  `editor/app/src/editor/effects/yamlDoc.ts:10-13` and `patternFiles.ts`, say the content walk
  "silently skips a flow-style file". `load_doc_stores` now reports it (`docs.rs:466-470`,
  `peek_kind_checked` → `L1005`). The editor's prune-empty-collections logic is still right, but
  the stated reason is wrong.

### S8 — The `canonical_idempotence` fuzz target never exercises schema-driven ordering or value preservation · Low · [verified]

`fuzz/src/lib.rs:69-80` emits with `Registry::new()`, so every component is unregistered. The
`SchemaCursor` paths (declared order, tag-first, `oneOf` selection, sorted open maps) are never
fuzzed. The target only asserts `emit ∘ parse ∘ emit == emit`, so an emitter that consistently
changes a value (an S1-class defect) is a fixed point and passes. Fix: fuzz with a small registry
of test components carrying enums, options and maps, and add `parse(emit(s)) == s`, comparing
floats by bits.

### S9 — A `%YAML 1.1` directive is accepted and the document is silently resolved as 1.2 · Low · [verified]

`%YAML 1.1\n---\nformat: 1\nname: yes\n…` passes `check-scene`, and `name` reads as the string
`"yes"` even though the file declares a schema in which it is `true`. The subset's promise is
"unambiguous to every YAML tool". Fix: reject any `%YAML` directive other than `1.2` (and `%TAG`) in
`yaml.rs` with an `L1001`-family code.

### S10 — The checker reports `allOf` siblings' fields as unknown · Low · [suspected]

`Checker::check` validates each `allOf` part separately against the whole object (`validate.rs:467-471`).
Each part's `check_object` then flags the keys declared by the *other* parts as undeclared, because
a part has `properties` and no `additionalProperties`. `SchemaCursor` already merges `allOf`
properties for ordering (`schema.rs:79-96`), so the writer handles the shape and the validator
does not. It is latent: no registered type uses `#[serde(flatten)]` today, the usual source of
`allOf`. Fix: check `allOf` parts with unknown-field reporting off, then do one unknown-field pass
against the merged `property_order()`.

## Checked and healthy

- **Strict subset:** anchors, aliases, tags, multiple documents, non-empty flow collections, block
  scalars, complex and non-string keys, over-long canonical keys (`L1030`, measured on the emitted
  spelling) and depth over 128 (`L1034`, checked before push, so no recursive reader can overflow)
  are all rejected with pointers. Merge keys (`<<`) are inert ordinary keys, and the writer quotes
  them.
- **Scalars:** YAML 1.2 core resolution. `.inf`/`.nan`/overflowing integers and floats are `L1009`,
  never a non-JSON value. `-0.0` survives load → save. Integers stay integers in the pure scene path.
  `plain_safe` quotes 1.1 booleans, the null/bool/number lookalikes, and anything non-ASCII or
  indicator-bearing. `quote_double` escapes C0/C1/DEL/BOM.
- **Writer:** every per-kind emitter routes floats through `format_float`. The remaining
  `to_string()` sites are all integer fields (atlas `rect`, assembly `place`, sprite `offset`/
  `radius`, font int sequences). Entity order is the document order. Component order is registry
  order. `preserve_order` is on workspace-wide, so declared-field order is struct order.
- **Property tests** (`crates/ludion-scene/tests/roundtrip.rs`): value → emit → parse equality,
  three-way byte idempotence, LF-only, bit-exact `f64` round trip over arbitrary bits, and
  arbitrary-`String` round trip through the name field.
- **Kind sniffing:** `may_declare_kind` is conservative in the right direction (quoted, escaped,
  `?`-keyed and propertied keys answer `true`). The content walk reports strict-YAML failures
  instead of dropping them.
- **No second writer for committed scene text:** the CLI pipeline verbs (`import`, `generate-map`,
  `generate-atlas`, `voxel`, `sculpt`, `gen`) all call `ludion_scene::emit_*`. The editor's
  `yamlDoc.ts` writer is a deliberate, documented exception for comment-preserving pattern-doc
  edits (`Plan/projectile-patterns.md` decision 10). It is block-only and minimal-diff, apart from
  the float convention in S1. `model_sheet.rs` builds in-memory scenes only.
- **`generated_from:` contract:** `scene.save` carries the authored marker forward
  (`engine.rs:3336-3341`), so an edited generated scene trips `scripts/map-drift.mjs` instead of
  silently losing provenance.
- **JSON-text float parsing:** serde_json without `float_roundtrip` lands 1 ULP off on about 6% of
  17-digit inputs (re-measured here). This is already documented (`dev-docs/ludion-engine.md:125-129`)
  and harmless for scene text while every engine component field is `f32`. It becomes live if S1's
  fix keeps any `f64` component field, or if a game registers one.

## Suggested order of work

1. **S2** — one exported checker call in `set`/spawn. Small, closes the silent-typo hole, and needs
   no file migration.
2. **S3** — a registration-time refusal of `kind`/`format` fields before a game ships a profile that
   hits it.
3. **S1** — the `format: float` shortest-`f32` emission. It moves committed derived trees, so
   sequence it with the import/mesh drift gates and the replay/schema re-record order in `CLAUDE.md`.
   Then add the load → save fixed-point gate over committed scenes.
4. **S4, S6, S7** — small, mechanical fixes.
5. **S8** alongside S1 (the fuzz property that would have caught it), then **S5, S9, S10** as
   capacity allows.
