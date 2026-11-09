---
description: Authoring a district as a glyph grid: the map and map-kit documents, and the generate/check loop
---

# Map authoring

**Scope:** Building a district from a compact text map instead of hundreds of hand-placed entities — the `kind: map` grid, the `kind: map-kit` legend, and the compile-and-verify loop. The scene the compiler emits is scene-format; the props a kit points at are voxel-format and components.

A district authored by hand is hundreds of `Transform`/`Mesh`/`Collider` blocks, and moving a wall
touches all of them. A **map** is the same district as a glyph grid plus a marker list — a game's plaza
can be a ~130-line map against the ~900 hand-authored lines it replaced — compiled by `ludion generate-map`
into an ordinary canonical scene. The compiler is pure and deterministic: the same map always produces
byte-identical output, which is what lets the generated scene be committed and gated.

## The loop

```
ludion map-check scenes/district-plaza.map.yaml            # occupancy + reachability, before you compile
ludion generate-map scenes/district-plaza.map.yaml         # → scenes/district-plaza.yaml
ludion generate-map scenes/*.map.yaml                      # both verbs batch over several sources
ludion generate-map scenes/district-plaza.map.yaml --check # byte-compare against the committed scene, write nothing
ludion generate-map a.map.yaml --kit k.kit.yaml --seed 3 --mirror x --out rooms/a.yaml   # overrides need --out
ludion generate-map a.map.yaml --out rooms/a.prefab.yaml --prefab   # a room prefab for spawn_prefab
```

Iterate **edit map → `map-check` → `generate-map`**. Never edit the generated scene: it is
write-only output, and the next `generate-map` overwrites it.

Reading the check half — the ASCII report, the `--at` probe, the `--traverse` sizing pass and the
placement lints — is `ludion docs map-check`.

## Format

### The map (`kind: map`, `*.map.yaml`)

```yaml
format: 1
kind: map
name: district-plaza
seed: 0
kit: city.kit.yaml       # resolved relative to THIS file
cell_size: 4.0
origin:                  # world (x, z) of the (col 0, row 0) corner
  - -66.0
  - -128.0
base: "."                # optional: a glyph underlaid beneath every non-void cell
fragments:               # optional: whole scene files concatenated verbatim, first
  - env-night.yaml
  - hud.yaml
rows:
  - "#################"
  - "#...............#"
markers: []              # optional
```

A map needs a frame: `rows`, or `bounds` when it is only markers. Markers with neither (an empty
`rows`) are `L1023` — they would otherwise vanish. A `base` needs `rows`.

| key | meaning |
|---|---|
| `kit` | path to the `kind: map-kit` legend, relative to the map file |
| `cell_size` | world units per cell |
| `origin` | world `[x, z]` of the grid's `(col 0, row 0)` corner |
| `base` | a glyph whose primitives underlay **every** non-void cell, so wall glyphs need not redeclare the floor. The base glyph then contributes only that underlay. |
| `rows` | the grid, one string per row, one glyph per cell |
| `bounds` | `[width, height]` in cells, for a **grid-less, marker-only** map: it frames the markers (at `origin` / `cell_size`) and emits no cells. Exclusive with `rows` and `base`. |
| `physics` | the generated scene's `physics:` block, verbatim (`ludion docs physics`) |
| `fragments` | scene files emitted verbatim ahead of the grid — the shared environment, HUD, or hand-authored core of the district |
| `markers` | exact-position placements (below) |

**Grid orientation:** `col → +X`, `row → +Z`, so the first row is the lowest Z. Cell `(col, row)`'s
centre is `origin + (col + 0.5, row + 0.5) * cell_size`. A **space** (`' '`) is the reserved void
glyph: it emits nothing and is exempt from the kit-glyph check — that is how you leave ground outside
the district's footprint. A kit `tiles` key of a space is `L1020` (its cells would never emit).

**Markers** place one thing at an exact world position rather than on the grid lattice:

```yaml
markers:
  - name: gate-to-market       # optional (else `<kind>-<index>`); globally unique
    kind: gate                 # optional: a kit `kinds:` entry to stamp
    x: 0.0
    z: 12.0
    rotation: 90.0             # optional, degrees about Y, applied to every entity the marker emits
    variant: 1                 # optional: force a variant index instead of the seeded pick
    components:                # optional: merged verbatim onto the first primitive's entity
      Gate:
        to: "district-market.yaml"
        anchor: from-market
```

A marker with a `kind` stamps that kit entry's primitives at its `(x, z)`. A marker with **no** `kind`
is a single entity: a `Transform` plus its `components:` passthrough — which is how a bare inbound
anchor (`- name: from-market`, `x`, `z`) is authored.

### The kit (`kind: map-kit`)

The kit is the shared legend, and it is deliberately game-agnostic: it names only built-in components,
so the same kit serves every district.

```yaml
format: 1
kind: map-kit
name: city
tiles:                    # glyph → primitives
  ".":
    - name: floor
      shape: cube
      color: [0.30, 0.29, 0.27, 1.0]   # (block style in a real file — see Rules)
      y: -0.5
      scale: [4.0, 1.0, 4.0]
      merge: true
kinds:                    # marker kind → primitives
  fountain:
    - name: fountain
      voxel: "voxels/fountain.voxel.yaml"
      y: 0.0
      collider:
        aabb: [3.5, 3.5]
```

A **primitive** names exactly one mesh source and may carry a static collider:

| key | meaning |
|---|---|
| `shape:` / `voxel:` / `mesh:` | the one mesh source — a built-in `cube`/`plane`/`sphere`, a `.voxel.yaml`, or a `.mesh.yaml` |
| `variants` | further mesh sources; the primitive's own is candidate 0 |
| `color` | `[r, g, b, a]` |
| `collider` | `aabb: [half_x, half_z]` or `circle: r` — static |
| `y` | the vertical placement, and **where the game bakes its floor rule** — the engine never hardcodes one |
| `scale` | `[x, y, z]` |
| `name` | the generated entity's name stem |

**Expansion mode**, chosen per primitive so one glyph can merge a floor slab *and* footprint-tile a
wall piece:

- **`merge: true`** — same-glyph cells greedily rectangle-merge (row-major, deterministic) into one
  entity per rect, and the mesh's X/Z scale is multiplied by the rect's cell dimensions. Set
  `scale.x`/`scale.z` so one cell is covered (a unit cube wants `cell_size`). This is what floors and
  long wall runs want: one entity and one clean collider per run, no seams. A merged primitive —
  `merge: true`, or any primitive of the map's `base` glyph — takes `collider: aabb: [cell_size/2,
  cell_size/2]` (the collider grows to the rect) and never a `circle`; anything else is `L1022`. The `aabb` ties
  a kit to one `cell_size`: a kit shared across cell sizes needs per-size merged tiles.
- **`footprint: [w, h]`** — the glyph's cells tile with one **unscaled** prop per `w × h`-cell block,
  greedily row-major. Modular walls and fences.
- **neither** — one unscaled prop per cell.

**Variants** pick per cell via a splitmix64 mix of `(seed, row, col)` — **independent per cell**, so
editing one corner never reshuffles variants elsewhere.

An optional **`probe:`** block names the walker `map-check` measures with; `generate-map` ignores it,
so adding it never moves a generated scene (`ludion docs map-check`).

## Rules

- **Every array is block style.** The strict-YAML subset rejects non-empty flow collections
  (`L1005`), so `origin`, `color`, `scale`, `collider.aabb`, `footprint` and `rows` are all spelled
  one element per line, exactly as voxel colours are. (The `[a, b]` forms above are shorthand for
  reading, not legal text.)
- **The generated scene is write-only.** It carries `generated_from: <map file name>`, the provenance
  marker the drift gate pairs back against — there is no manifest. The gate
  enumerates every committed `*.map.yaml`, requires its scene at the default path to regenerate
  byte-identical, and flags a scene naming a map from anywhere else
  (`node scripts/map-drift.mjs`, a step in `node scripts/check.mjs` and in CI). An edit to the
  generated file fails the gate on the next run, and the wire's `scene.save` refuses to write it
  (`L1267`).
- **Generated entity names must be globally unique** across fragments, grid and markers (`L1025`),
  because a cross-district gate's `anchor` resolves by name. Grid names are coordinate-derived and a
  merged rect encodes its extent (`floor-r000-c000-w03-h03`), so collisions come from fragments or
  hand-named markers. The diagnostic names both copies with their JSON Pointers.
- **A fragment may not carry `physics:` or `seed:`** — only its entities merge, so the scene block
  would be dropped silently (`L1021` at `/fragments/<i>`); declare both on the map.
- **Entity references inside a fragment are local to it** (`Parent` and the like): composition rebases
  them past the entities emitted before the fragment; one naming no entity of its own fragment is
  `L1017` at `/fragments/<i>`. This covers prefab-instance overrides too, but only engine-registered
  components' references are rebased: a game component carrying an entity index is not.
- **Emission order is fixed** for diff stability: fragments verbatim (file order, entity order) →
  grid rects and props sorted row-major by anchor cell → markers in source order.
- Diagnostics: ragged rows `L1019`; a glyph with no kit entry `L1020`; a structurally malformed map
  or kit `L1021`; an invalid kit entry or marker `variant` `L1022`; a marker outside the grid
  `L1023`; an unknown marker `kind` `L1024`; a name collision `L1025`; a footprint run that does not
  divide evenly `L1026` (an 8-cell run tiles with `[4, 1]`, a 6-cell run does not); an IO failure
  `L1015`; `generate-map --check` finding a scene that differs from its source `L1039`; `--mirror` on a map with fragments `L1399`. `ludion explain L####` prints the full form of each. The placement lints `L1027`–`L1029`,
  `L1031` and `L1032` are warnings raised by `map-check`, never by `generate-map`
  (`ludion docs map-check`). Both verbs also run the scene check on the generated entities' built-in
  components, so a kit value `check-scene` rejects (a collider extent `<= 0`, `L1116`) fails them too.

## Gotchas

- **`generate-map` does not `check-scene` its output**, on purpose: a real district carries game
  components the built-in registry has never heard of, and validating would fail them with `L1013`.
  The self-check is structural instead — the emitted text must survive a parse→emit round-trip
  byte-for-byte or nothing is written. Validate the components through your game's own tests.
- **`--out` defaults to the map with its `.map` suffix dropped** (`district-plaza.map.yaml` →
  `district-plaza.yaml`); a plain `.yaml` source gets `.scene.yaml`, so a map can never overwrite
  itself. `--out` is valid only with a single input, and is for scratch output: a committed scene
  must sit at the default path, or the drift gate flags it as a moved copy.
- **`kit:` and every `fragments:` entry resolve relative to the map file**, never to your current
  directory — so the loop runs identically from anywhere in the tree.
- A marker's `components:` merge onto the **first** primitive's entity only. That is what makes a gate
  one entity carrying both its `Mesh` and its `Gate`; a kind whose first primitive is decorative will
  put your component on the decoration.

## Next

- `ludion docs prefabs` — rooms spawned at runtime.
- `ludion docs map-check` — reading the report, `--traverse` sizing, and the placement lints.
- `ludion docs scene-format` — the strict-YAML scene document the compiler emits.
- `ludion docs scene-transitions` — the gates a map's `Gate` markers drive.
- `ludion docs components` — the built-in components a kit may name.
- `ludion docs voxel-format` — the models a kit's `voxel:` primitives point at.
