---
description: The .voxel.yaml schema — glyph slices, orientation, palette, origin and sizing, scene reference
---

# Voxel format

**Scope:** what a `voxels/*.voxel.yaml` document is, and how a scene references it. Checking and
rendering one is `ludion docs voxel-authoring`; which values and shapes read at game scale is
`ludion docs voxel-craft`; models past hand-editing size are `ludion docs voxel-ops`.

A **voxel model** is a small 3D grid of colored cubes in one strict-YAML file, greedily meshed into
ordinary geometry at load. It is not a second render mode: the model is a `Mesh.shape`, so it renders
through the same lit mesh pipeline as a `cube` — a `Transform` places and sizes it, `PointLight`s
light it. A palette of glyphs, then a stack of ASCII grids you read as text and diff line by line;
one file describes one model.

## Format

Eleven keys, in this order:

| key | meaning |
| --- | --- |
| `format: 1` | document format version |
| `kind: voxel` | what `ludion check-scene` dispatches on: a scene has no `kind`, a mesh asset is `kind: mesh` |
| `name:` | optional, human-readable |
| `size: [x, y, z]` | grid extent in cells — width (x), height (y), depth (z), each a positive integer |
| `scale:` | optional, positive — the world scale the model was *drawn for*; metadata only |
| `generated_from:` | optional — the generator script that owns this model; its presence makes the file **write-only** and enrols it in the `voxel-drift` gate (`ludion docs voxel-generation`) |
| `budget_exempt:` | optional, only ever `true` — this model is deliberately over the `L1115` cell budget (`ludion docs voxel-ops`) |
| `rim:` | optional `{ color, strength, power }` — one view-angle edge for the whole model; wins over `Mesh.rim` (`ludion docs materials`) |
| `lods:` | optional, up to 3 `{ screen_size }` entries (finite, above 0, strictly falling; `L1395`) — each coarser level is a 2× majority downsample generated at load and drawn by the mesh LOD selection (`ludion docs culling`) |
| `palette:` | map from a **single glyph** to `{ color: [r, g, b, a] }` |
| `slices:` | the grid — `y` layers, each a list of `z` row-strings of exactly `x` characters |

`name`, `scale`, `generated_from` and `budget_exempt` are metadata the mesher never reads; a model
that declares none of them is the ordinary case.

A complete model — a 3×3×3 mushroom, copyable verbatim:

```yaml
format: 1
kind: voxel
name: mushroom
size:
  - 3
  - 3
  - 3
palette:
  C:
    color:
      - 0.85
      - 0.15
      - 0.12
      - 1.0
  S:
    color:
      - 0.85
      - 0.75
      - 0.55
      - 1.0
slices:
  -
    - "..."
    - ".S."
    - "..."
  -
    - "..."
    - ".S."
    - "..."
  -
    - "CCC"
    - "CCC"
    - "CCC"
```

`ludion check-scene voxels/mushroom.voxel.yaml` prints
`OK (3x3x3, 11 filled voxels, 1 component, meshed 14 quads / 28 triangles / 56 vertices)`: a tan stem
(`S`) rising two cells, capped by a red (`C`) 3×3 lid. The meshed counts are what the model costs the
renderer after greedy merging — the number `budget_exempt:` should be decided on, since `L1115`
counts cells and a flat wall of them collapses into one quad.

### Orientation — the one thing to get right

The grid axes are fixed, and getting them right is the whole game:

- **`slices` are bottom-up: `slices[0]` is `y = 0`, the ground layer**; the last slice is the top. The
  mushroom's cap is on top *because* it is written last.
- **Within a layer each row-string is one `z`, and each character in it is one `x`.** The first
  row-string is `z = 0`, the leftmost character `x = 0`. A 2×1×2 layer written `"A."` then `".A"`
  fills `(x=0, y=0, z=0)` and `(x=1, y=0, z=1)`, a diagonal from front-left to back-right; to flip it
  you swap glyphs *within* a row (that is `x`), not the order of the rows (that is `z`).
- **Row `z = 0` is the model's front — and it meshes at world −Z, so −Z is a model's forward
  direction.** Put the face (eyes, muzzle, sigil) in the low-`z` rows and the `front` view of
  `render --model` shows it. The mesher lays cell `z` at world `z − sz/2`, with no flip, so the low-`z`
  end is the *most negative* world Z. This has bitten twice, both times as a 180° error: **code that
  turns a model to face a direction wants `yaw = atan2(-dx, -dz)`**, never the `+Z` mirror
  `atan2(dx, dz)`.
- **`.` is an empty cell**, and it is reserved — it can never name a palette entry.
- **A glyph is one printable-ASCII character** (`!` through `~`), so columns always line up; a
  multi-byte glyph is rejected.

### Palette

Each entry requires `color` — a four-number linear RGBA (`[r, g, b, a]`, the same convention as
`Mesh.color`; finite, normally `0.0..=1.0`) — and may add any of three optional material scalars,
the same fields and ranges as `Mesh`'s (`ludion docs materials`):
`roughness` (`[0, 1]`, absent → `1.0`), `metallic` (`[0, 1]`, absent → `0.0`), and `emissive` (an HDR
intensity multiplier on the entry's own `color`, `>= 0`, absent → `0.0`).

```yaml
palette:
  E:
    color:
      - 0.4
      - 0.8
      - 1.0
      - 1.0
    emissive: 4.5        # glass, bright enough to bloom
  T:
    color:
      - 0.85
      - 0.6
      - 0.25
      - 1.0
    metallic: 1.0         # metal trim
    roughness: 0.25
```

An entry that authors *any* of the three owns its material entirely for every quad drawn in that
glyph's color — it does not blend with the `Mesh` component's fields. Glyphs must be unique.
`check-scene` reports each violation with a JSON Pointer, all `L1114`.

The canonical writer sorts entries by glyph and, within an entry, writes `color` first then the
authored optional fields alphabetically; an absent field stays absent, so re-emitting a model (or
letting the editor round-trip it) yields a stable, minimal diff whatever order you typed the keys in.

Which values to aim at is `ludion docs voxel-craft`, and you judge them under the light the model
will be seen in — `--rig` (`ludion docs voxel-sheets`). Against the plain contact sheet's neutral
key + fill a mid-value linear color (`~0.4–0.6`) renders near-white, so authoring on that sheet means
aiming roughly 2–3× darker than the target read.

### `scale:` — declared intent, not applied size

A model may record the world scale it was drawn for as an optional positive `scale:`, written between
`size` and `palette` (`scale: 0.24`). **The document is authoritative as *intent*; a scene's
`Transform` is authoritative as *fact*.** The mesher and the render path never read `scale`, so
`Transform.scale` keeps exactly one meaning everywhere and a model with no `Transform` still renders
at 1 voxel = 1 unit. What it is for: whoever *places* the model (an editor's placement tool, a kit
shelf) stamps the declared number into the `Transform` it authors, and a scene that wants a different
size just authors a different `Transform.scale` — no migration, no double-scaling.
`ludion check-scene` echoes it in the `OK (…)` summary (and says nothing when the model declares
none), which is how you check that a model's intent still matches how the game actually places it.

### Referencing it from a scene

A voxel model is a `Mesh` shape. The path is relative to the **content root** (the scene file's
directory), and the convention is `voxels/*.voxel.yaml`. In a scaffolded project the scene lives at
`scenes/main.yaml`, so the model file belongs at `scenes/voxels/mushroom.voxel.yaml` — **not** at the
project root next to `goldens/` (a root-level `voxels/` fails to resolve with `L1101`):

```yaml
  - name: mushroom
    components:
      Transform:
        scale:
          - 0.6
          - 0.6
          - 0.6
      Mesh:
        shape:
          voxel: "voxels/mushroom.voxel.yaml"
```

Referencing the same path from several entities meshes it **once** (deduplicated by path), so a forest
of identical trees is a single mesh upload.

## Rules

**Filled cells should form one connected piece.** The `component` count in the `OK (…)` summary is a
6-connectivity check (two cells connect only across a shared *face*); more than one, and `check-scene`
also prints a non-fatal `L1129` — the disconnected-limb bug (a floating arm, a cell stranded by an
edit) caught on the CPU instead of only in a GPU render. It warns rather than fails because a
deliberately detached accent is legal. One line per stray piece, smallest first, names its size, a
sample cell, and — the part that locates the *fix* — the nearest pair of cells between it and the
main body:

```
warning[L1129]: this voxel model's filled cells split into 2 disconnected components (main body 1058 cells); each stray piece, smallest first:
  141 cells, sample (2, 0, 1); nearest cells to the main body: stray (5, 18, 6) vs main (7, 18, 6), gap 2 (1 bridge cell to connect)
```

The sample cell and the gap are often far apart — a stray shoulder's first cell in scan order can sit
a dozen layers below the single break that severed it. Bridge with face-adjacent `set` steps: a
`line` op with a diagonal step draws edge-touching cells, which look continuous in a slice print and
are **not** connected.

### Origin, sizing, and tint

- **Origin is bottom-center.** The model sits *on* the ground plane (`y = 0` is the bottom of the
  lowest cell), centered in x and z, so placed at a `Transform.translation` it stands on that point.
- **1 voxel = 1 unit** before scaling — a `size: [3, 3, 3]` model is 3 units cubed at `scale: 1`.
  There is no per-file voxel size; size the model in the world with `Transform.scale`.
- **Density convention: 16 voxels per world unit** (`Transform.scale: 0.0625`) for standard props —
  a 1-unit crate spans 16 cells, a 3-unit lantern post 48. Large structures (houses, walls) halve to
  8 per unit (`scale: 0.125`) to stay inside the 16³–48³ single-file sweet spot; past that, assemble
  the thing from several models — or take the ladder (`ludion docs voxel-ops`) instead of
  hand-writing giant slices.
- **Keep interiors solid.** `.` and a glyph cost the same byte, and the mesher emits faces only where
  filled meets empty — a solid interior adds no geometry, while a hollowed-out one adds invisible
  inward-facing walls.
- **`Mesh.color` still tints.** Each face is shaded `palette_color × Mesh.color × light`, so a white
  `Mesh.color` (`[1, 1, 1, 1]`) shows the palette unchanged and a colored one washes the whole model.
  Material is different: `Mesh.roughness`/`metallic`/`emissive` are inert on a voxel model the moment
  any one palette entry authors any material field — color always tints, material never blends across
  the two surfaces.
- **Rain wets every palette.** Under a `Rain` (`ludion docs weather`) a voxel model's exposed faces darken
  and gloss by the palette entry's `porosity` (`[0, 1]`, default 0.5). Authoring `porosity` alone makes the entry
  author a material, so `Mesh.roughness`/`metallic`/`emissive`/`porosity` go inert like they do for any material field.

## Gotchas

- **`L1114` — the grid doesn't match `size`:** a ragged row (not `size.x` characters), the wrong
  number of rows in a layer (not `size.z`), or the wrong number of layers (not `size.y`). Pad short
  rows with `.`; the pointer names the offending `/slices/y/z`.
- **`L1114` — a glyph names no palette entry.** Every non-`.` character must be a palette key. Add it
  to the palette, or make the cell empty with `.`.
- **`L1114` — a bad palette key, color, or material scalar:** a multi-character, non-ASCII, reserved
  (`.`), or duplicate glyph; a non-finite color component; or an authored `emissive`/`metallic`/
  `roughness` that is non-finite or outside its range (`emissive < 0`, or `metallic`/`roughness`
  outside `[0, 1]`).
- **`L1395` — a malformed `lods:`:** more than three levels, a `screen_size` that is not finite, above
  zero and below the previous level's, or a level key other than `screen_size`. A level is generated,
  so it names no file. An odd extent rounds up when halved, so a coarse level overhangs the base by
  under one coarse cell toward +X/+Y/+Z. A sculpt `shape: voxel` part and an assembly part ignore the
  chain.
- **`L1398` — an empty `lods:` level (warning):** a level's halved grid holds no solid cell (a coarse cell
  needs four of its eight), so it and every coarser level draw nothing. `check-scene` names the first.
  Drop it, or keep a thin model unchained.
- **`L1113` — structure:** a missing key, a wrong `kind`, a non-integer `size`, or an unknown key
  (reported with a nearest-match suggestion).
- **Block scalars are rejected (`L1006`).** The strict-YAML subset forbids `|` and `>`, so a layer
  cannot be one block of text. Use a sequence of quoted row-strings (as every example here does; the
  canonical writer always double-quotes rows) — it reads just as spatially and diffs one row per line.

## Next

- `ludion docs voxel-authoring` — the author → check → render loop and the text checks.
- `ludion docs voxel-assemblies` — several parts in one model, when one grid can no longer move.
- `ludion docs voxel-sheets` — the `render --model` contact-sheet harness.
- `ludion docs voxel-craft` — which palette values and shapes actually read at game scale.
- `ludion docs voxel-ops` — `ludion voxel new/apply/upscale/diff` past hand-editing size.
- `ludion docs voxel-generation` — building a model from a committed script, and the drift gate.
