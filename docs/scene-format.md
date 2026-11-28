---
description: The strict YAML 1.2 subset: allowed constructs, document skeleton, canonical serialization
---

# Scene format

**Scope:** The scene *file* format — what you need to write, read, or hand-edit a `.yaml` scene: the allowed YAML subset, the document skeleton, scalar rules, and the canonical form the engine emits. The parser/validator/emitter implementation is documented in ludion-scene; the L-code system behind the validation errors is in diagnostics.

## Layout

- `examples/scenes/empty.yaml` — the minimal scene; `examples/gallery/*.yaml` — real scenes (goldens in `examples/gallery/goldens/`).
- `crates/ludion-scene/` — the implementing crate (single source of truth for parse/emit behavior).

## Rules

**Strict subset of YAML 1.2.** Everything outside it is rejected with an L-coded diagnostic (never silently tolerated):

- No anchors/aliases (`&name`, `*name`) — `L1002`. Repeat the value; repetition keeps diffs local.
- No tags (`!!int`, `!custom`) — `L1003`. Types come from the component's JSON Schema, not per-node annotations. To force a string, quote it (`"42"`).
- One document per file — `L1004`. A single leading `---` is tolerated on parse, never emitted. A leading UTF-8 BOM is ignored on parse, never written.
- No non-empty flow collections (`[1, 2]`, `{a: 1}`) — `L1005`. Only the empty `[]` and `{}` are legal flow forms. Block style everywhere else, so a one-value change is a one-line diff.
- No literal/folded block scalars (`|`, `>`) — `L1006`. Strings are plain or quoted scalars on one line; use `\n` escapes in double quotes for line breaks.
- No duplicate mapping keys — `L1007` (points at the second occurrence).
- Keys are plain strings only — `L1008` (no `? complex key`).
- No directives but `%YAML 1.2` — `L1037` (`%YAML 1.1`, `%TAG` and unknown directives; the file is always read as YAML 1.2 core). Delete the line.
- Diagnostic positions are a 1-based line and column.
- No values JSON cannot represent — `L1009`: non-finite floats (`.inf`, `.nan`) and integers outside the 64-bit range are rejected.
- Unknown keys in the skeleton are errors, not ignored — `L1010` (with a nearest-match suggestion). `entites:` will not silently produce an empty scene.

**Parsers never panic on arbitrary input**: any malformed, truncated or hostile byte sequence yields a structured L-coded diagnostic, never a panic or unbounded allocation (fuzzing in `fuzz/` plus committed regression tests).

**Scalar resolution** follows the YAML 1.2 core schema: plain `true`/`false`/`null`/`~`/numbers resolve to their types; quoted scalars are *always* strings. `no`, `yes`, `on`, `off` are plain strings (no Norway problem); the canonical writer quotes anything ambiguous.

**How paths resolve.** Every relative path — a scene's asset references (`Sprite.texture`,
`Tilemap` legend textures) and a wire `scene.load`/`scene.save` path — resolves against the **content
root**: the directory of the scene the engine was started with (its `--scene`, or the builder's
`default_scene`), **not** the process's current directory. So a scene at `scenes/main.yaml` with
`texture: assets/brick.png` reads `assets/brick.png` relative to `scenes/main.yaml`'s folder. Pass
`scene.load` a path relative to that same root — never re-prefix `scenes/`, or you get `scenes/scenes/…`.

## How it works

### Document skeleton (format version 1)

```yaml
format: 1              # required; the only supported version (else L1012)
name: "level-1"        # optional string
seed: 42               # optional non-negative integer; seeds the sim's DeterministicRng (0 if absent)
physics:               # optional; declares rigid-body physics — `ludion docs physics`
  space: free
nav:                   # optional; a NavConfig for the built-in hosts, needs `physics` — `ludion docs nav`
  cell_size: 0.25
render_layers:         # optional; the render layer names the scene uses, declared once — `ludion docs render-views`
  - name: radar
preload:               # optional; prefab paths a system will `spawn_prefab`, readied at load (a bad path fails the load, `L1230`)
  - "prefabs/enemy.prefab.yaml"
entities:              # required shape: a sequence of mappings (use `entities: []` for none)
  - name: "paddle"     # optional; becomes the reserved `Name` component, so it is in the World and the WorldHash
    # presentation: true  # optional, after `name`: drawn, never hashed — `ludion docs presentation`
    components:        # mapping from component name to component value (`components: {}` for none)
      Transform:
        translation:
          - 1.5
          - 2.0
          - 0.0
  - components: {}
```

- Top-level keys: exactly `format`, `name`, `generated_from`, `seed`, `physics`, `nav`, `render_layers`, `preload`, `entities`. Entity keys: exactly `name`, `presentation`, `components`. Anything else is `L1010`; a missing `format` is `L1011`. An entry with a `prefab:` key is an instance of a prefab file, not a plain entity (`ludion docs prefab-instances`).
- Component *names* must exist in the component registry (`L1013`, with nearest-match suggestion). Component *values* are validated against that component's JSON Schema (`L1014`): wrong types, missing required fields, undeclared fields (typos are errors), out-of-range values — each with a JSON Pointer like `/entities/0/components/Transform/yy` and a fix suggestion.
- Entity order is data: entities instantiate (and re-emit) in document order.

### Canonical serialization

One scene has exactly one canonical text (per registry). Any client's edit re-emits to a minimal git diff. The engine's writer produces:

- LF line endings, 2-space indent, block style; `[]`/`{}` only for empty collections.
- Stable key order — top-level `format`, `name`, `generated_from`, `seed`, `physics`, `nav` (omitted when absent; keys in `NavConfig` field order), `render_layers` (omitted when empty), `preload`, `entities`; entity `name`, `presentation` (only when true), `components`; components in registry registration order; component fields in struct declaration order; an enum variant, an `Option` or an untagged value orders by the alternative it matches (an internally tagged enum's `type` leads, as serde writes it), and map keys sort in byte order — the order `scene.save` writes a `BTreeMap<String, _>` in.
- Floats in ryu shortest-round-trip form (`1.5`, not `1.50000`); never rounded, never fixed precision. A field the schema types `f32` is written as its shortest f32 decimal (`0.3`, not `0.30000001192092896`) — the same `f32` on read-back. An integer field stays an integer (`1`, and a schema with `integer` rejects `1.0`); a float field is written with a fraction digit (`1.0`), whatever spelling the source used, and committed hand-authored scenes and prefabs use the writer's spelling, gated by `ludion_testkit::assert_saves_respell_nothing`.
- Strings quoted with double quotes whenever they are not simple identifier-ish text; anything that could resolve as a number/bool/null is always quoted.
- Emission is idempotent: parsing canonical text and re-emitting reproduces the bytes exactly.

Any strict-subset document parses, but the next engine write normalizes the file; write canonically to keep diffs clean.

### `kind:`-tagged documents (meshes, voxels, game data-docs)

A `.yaml` file with a top-level `kind:` key is **not** a scene — the `kind:` scalar dispatches it to a different document family, all sharing this same strict-YAML subset and canonical writer. A scene has **no** `kind:`; `kind: mesh`/`kind: voxel`/`kind: sculpt` are engine-owned asset/model documents (`ludion docs voxel-format`, `ludion docs sculpt-format`), `kind: mesh-edit` is a write-only recipe deriving an edited copy of a `kind: mesh` document's texture maps (`ludion docs mesh-editing`), and `kind: generation`/`kind: origin` are the generation recipe and the provenance sidecar an accepted asset carries (`ludion docs generation`), `kind: map`/`kind: map-kit` are the map-authoring source documents (`ludion docs map-authoring`); any other `kind:` is a **game data-doc** — declarative game data (weapons, enemies, items) whose type the game registers with `doc_kind::<T>`. The engine peeks `kind:` on every `*.yaml` under a content root and routes: no `kind` → scene, an engine-owned kind → asset (skipped by the data-doc walk), a registered kind → its typed store, an unregistered kind → `L1119` with a nearest-match.

A data-doc opens with a two-key envelope, then the registered type's own fields flattened at the top level (never nested under a `data:` key):

```yaml
format: 1
kind: weapon        # a game-registered kind
damage: 12.0        # the registered type's own fields, validated against its JSON Schema
cooldown: 0.8
```

A malformed envelope (missing/wrong `format`/`kind`, a stray envelope key) is `L1119`; a field that violates the type's schema (wrong type, out of range, a typo) is `L1120` with a JSON Pointer + suggestion — the field-level twin of `L1014`. Authoring, `check_doc`/`emit_doc` and hot-reload: `ludion docs data-docs`.

`kind: profile` is a fourth, **engine-owned** family: a game's persistent player save state (`save/profile.yaml`), loaded into a `Profile<P>` resource through the same envelope and canonical writer. The content-root doc walk and `--check-docs` **skip** it, alongside `mesh`/`voxel`. Its load is **lenient**: a drifted save migrates (fill/drop/back-up-and-default with `L1121`) rather than hard-rejecting, because it is a player's accumulated bytes, not authored content. Migration rules: the engine guide's "Player profiles" chapter.

## Placement lint

`ludion check-scene scene.yaml --placement` warns about dressing that is placed wrong. It is opt-in: it
reads every mesh document, and its rules are heuristics that would otherwise print on every scene.
Warnings only (stderr, JSON Pointer into the document); the exit code never changes, and a
`placement: N groups checked, U unmeasured, W warnings (…)` line closes the output (under `--json` it is the summary's `placement` field).

- **Groups.** The unit is a *placement group*: a depth-0 prefab instance (all its parts), or else a
  hierarchy root with every `Parent` descendant (presentation children included, at their world pose).
  Parts of one group never support or overlap each other.
- **`L1241` sunk:** the group's mesh box bottom is more than `--sink` (0.1 m) below `--ground-y` (0).
  Ground tiles embed slightly by design.
- **`L1240` floating:** the bottom is more than `--tolerance` (0.05 m) above the ground and no other
  group's box, grown by the tolerance, touches it. A torch on a wall or a barrel on a terrace is supported.
- **`L1242` overlap:** a group's static in-plane `Collider` (`circle`/`aabb`), shrunk by the tolerance,
  overlaps another group's. This is the footprint the game collides with, queried through the same
  `spatial::overlaps`. Mesh boxes are used **vertically only**: an imported mesh's bounds include
  protrusions (an awning), which can only add support there but would be noise as a horizontal
  footprint. Props without a collider may intersect freely.

Voxel, sculpt and assembly meshes are not measured (counted as unmeasured). Put the
`Collider` on the prefab root, as `ludion docs presentation` does for set dressing.

## Verify

```
cargo run -p ludion-cli -- check-scene path/to/scene.yaml     # parse + validate; L-codes + JSON Pointers on failure
cargo run -p ludion-cli -- explain L1014                      # long-form explanation of any scene diagnostic
cargo run -p ludion-cli -- sim-hash --scene path/to/scene.yaml --frames 600   # proves it also instantiates
cargo test -p ludion-scene                                    # round-trip + idempotence property tests
```

These `cargo run -p ludion-cli -- …` lines work in the engine checkout; from a game project, get the CLI as `ludion docs project-anatomy` § "Getting the ludion CLI" describes.

## Gotchas

- `seed` must be a non-negative integer (`L1009` otherwise); it configures the sim RNG and therefore changes `sim-hash` output.
- `format: 1` plus `entities: []` is the minimal valid document; an empty file is `L1011`.
- Indentation is spaces; a tab is an `L1001` syntax error, often reported at a confusing position.
- Component names are case-sensitive registry keys; `transform` is not `Transform` (you will get `L1013` with the suggestion).
- Entity `name` is carried by the reserved `Name` component (author it through `name:`, never a `components:` block): protocols still reference entities by id, but renaming an entity changes the `WorldHash`.
- `check-scene` opens every mesh, voxel, sculpt, assembly, collider-mesh, mesh-texture and decal-map file a scene (or a prefab it instances) names, and reports a missing or bad one as `L1101` at the entity naming it; a `MeshGrid` glyph no row uses is not opened.
- **`ludion check-scene` is built-in-only** — the CLI links no game code, so every scene naming one of
  your components fails it with `L1013`. Check those from your own binary, which has the registry:
  `cargo run -- --scene scenes/main.yaml --check-scene` validates and exits, and
  `cargo run -- --scene scenes/main.yaml --render-out frame.png` writes the frame. That pair is the
  authoring loop for a real game scene; details in running.
