---
description: The .sculpt.yaml schema — primitives, folding order, palette blending, bones, override pointers
---

# Sculpt format

**Scope:** what a `sculpts/*.sculpt.yaml` document is, and how a scene references it. The author →
check → render loop is `ludion docs sculpt-authoring`; the mesher that turns this document into
geometry is `ludion devdocs ludion-sculpt`.

A **sculpt model** is an implicit-surface model: named primitives with real dimensions, folded left
to right by `union`/`subtract`/`intersect` with a smoothing radius, in one strict-YAML file. It is a
`Mesh.shape` sibling of a voxel model (`ludion docs voxel-format`), for smooth organic or
hard-surface forms a faceted grid would need many cells to approximate. There is no vertex editing:
the mesher turns the field into a triangle mesh at load, deterministically, on every machine.

## Format

Nine keys, in this order: `format`, `kind`, `name`, `resolution`, `triangles`, `budget_exempt`,
`palette`, `bones`, `parts`.

| key | meaning |
| --- | --- |
| `format: 1` | document format version |
| `kind: sculpt` | what `ludion check-scene` dispatches on |
| `name:` | optional, human-readable |
| `resolution:` | grid cells per world unit, integer `4..=128` (default `48`) |
| `triangles:` | decimation target triangle count, integer `>= 12` (default `5000`; a hero earns `10000`) |
| `budget_exempt:` | optional, only ever `true` — opts the meshed result out of the `L1112` vertex-budget warning |
| `palette:` | the voxel palette shape (`ludion docs voxel-format`), one-glyph keys |
| `bones:` | the bone tree; `bones[0]` is the root and has no `parent` |
| `parts:` | named primitives, evaluated in file order |

Shaped after the test idol (`crates/ludion-sculpt/tests/fixtures/idol.sculpt.yaml`; vectors inlined
here, the canonical writer uses block style):

```yaml
format: 1
kind: sculpt
name: idol
resolution: 32
triangles: 4000
palette:
  E:
    color:
      - 0.2
      - 1.0
      - 0.6
      - 1.0
    emissive: 4.0
  S:
    color:
      - 0.35
      - 0.33
      - 0.3
      - 1.0
    roughness: 0.85
bones:
  - name: root
  - name: head
    parent: root
parts:
  - name: plinth
    shape: box
    size:
      - 1.2
      - 0.3
      - 1.2
    round: 0.02
    at:
      - 0
      - 0.15
      - 0
    material: S
    sharp: true
  - name: body
    shape: capsule
    radius: 0.25
    from:
      - 0
      - 0.3
      - 0
    to:
      - 0
      - 1.1
      - 0
    blend: 0.1
    material: S
```

`ludion check-scene crates/ludion-sculpt/tests/fixtures/idol.sculpt.yaml` prints
`OK (sculpt, 6 parts, 2 bones, grid 50x57x50 @ 32/unit, 4000 triangles after decimation from 13436,
2102 vertices, meshed in 70 ms)`.

### Parts

Every part has `name` (required, unique — `L1157` on a duplicate) and `shape` (required, decides the
key set below — a key that belongs to another shape, like `radii` on a `box`, is `L1157` naming that
shape's keys; a key no shape has at all is `L1156` with a nearest match).

| shape | own keys |
| --- | --- |
| `sphere` | `radius` |
| `capsule` | `radius`, `from`, `to` — no `at` |
| `ellipsoid` | `radii` |
| `box` | `size`, optional `round` (edge radius, `0..=` half the smallest `size`) |
| `torus` | `radius` (centre to tube), `tube` (tube radius) |
| `cone` | `radius`, `height` — base disc centred on `at`, apex at `at + (0, height, 0)` |
| `voxel` | `path`, `size` |
| `mesh` | `path`, `size` (optional) |

A `cone`'s apex is a mathematical point: an exposed tip always sheds a four-vertex `L1159` island,
at every `resolution`. Bury it, or use the spike/horn/tooth idiom — a tapered `capsule`.

A `voxel` or `mesh` part reads another document's own shape as its field instead of describing a
primitive — `voxel` an occupancy, `mesh` a triangle soup — so existing or generated content blends,
bevels and carves like any other part. Their keys, placement, override pointers and diagnostics are
`ludion docs sculpt-format-sources`.

Shared keys: `at` (centre, default `[0,0,0]`; refused on a `capsule`, which is placed by `from`/
`to`), `rotation` (Euler XYZ degrees, default none), `op` (`union` default | `subtract` |
`intersect`), `blend` (smoothing radius, `>= 0`, default `0` = hard edge), `mirror` (`x`|`y`|`z` —
duplicates the part across that origin plane before its own op runs), `repeat` (`count >= 2`, `step:
[x,y,z]` — an arithmetic march, all copies one op), `material` (a palette glyph; required on a
`union` part, refused on `subtract`/`intersect`), `bone` (a `bones:` name, default the root), `sharp`
(bool, default `false` — the crease-angle normal split, `ludion devdocs ludion-sculpt`), `deform`
and `noise` (below). A part's block ends with `deform` then `noise`, after every other key.

### Deformers and noise

`deform: { bend: {...}, twist: {...}, taper: {...} }` bends, twists and tapers the part's local
space before its primitive is measured; any non-empty subset is legal, an empty `deform` `L1157`.
The sample point always passes through `bend`, then `twist`, then `taper`, whatever order the
document writes them in. **Every deformer acts about the part's own centre**, the midpoint of its
undeformed box, so a part turns about itself wherever it stands — a `from`/`to` `capsule` like an
`at`-placed shape.

- `bend` and `twist` each take `axis` (`x`|`y`|`z`) and `degrees` (finite, either sign — rotation
  per world unit travelled along `axis`); either missing is `L1156`. `bend` rotates in the plane of
  `axis` and the next axis (`x`→`y`, `y`→`z`, `z`→`x`); `twist` rotates in the plane perpendicular to
  `axis`.
- `taper` takes `axis`, `from`, `to` (any missing is `L1156`): the cross-section scale at the part's
  **minimum** and **maximum** coordinate along `axis`, over its own undeformed extent; `1` keeps that
  end's size. A non-positive `from`/`to` is `L1157` (it would collapse or invert the cross-section).

`noise: { amplitude, scale, seed }` displaces the part's distance with seeded value noise; all three
keys are required (any missing is `L1156`). `amplitude` (world units, `>= 0`; negative is
`L1157`) is the peak displacement — the surface moves within `±amplitude`. `scale` (world units,
`> 0`; non-positive is `L1157`) is the lattice spacing, the feature size, not a frequency.
`seed` (a non-negative integer) draws a different surface per value, the same one on every machine.

```yaml
  - name: horn
    shape: capsule
    radius: 0.05
    from:
      - 0
      - 1.2
      - 0
    to:
      - 0
      - 1.5
      - 0
    material: S
    deform:
      taper:
        axis: y
        from: 1
        to: 0.4
    noise:
      amplitude: 0.01
      scale: 0.06
      seed: 3
```

### Semantics

Model space is world units at `Transform.scale: 1`, origin bottom-center like a voxel model: the
meshed surface is translated so its box's minimum `y` is `0` and its `x`/`z` extents are centered,
**after** meshing — so a part authored below `y = 0` is not an error.

Parts fold left to right: the running field starts at `+inf`; `union` is a polynomial smooth `min`,
`subtract` a smooth `max` of the negated part, `intersect` a smooth `max`, each exact at `blend: 0`.
Each `union` part contributes its palette color and `(roughness, metallic, emissive)` weighted by the
smooth-min share it won at each point; a `subtract`/`intersect` contributes no material but
re-weights the parts it passed through. A smoothed seam between two `union` parts therefore blends
their colors — a glowing vein fades into flesh instead of cutting hard.

### Bones

`bones:` is a tree, `bones[0]` the root with no `parent`; every other entry names a `parent`. A
part's `bone` must name an entry, or is left unset to mean the root (`L1160`: an unknown bone or
parent, a second root, a self- or cycle-parented chain). `check-scene --bounds` prints each bone's
world-unit bounds, the union of its parts'. Emit order: `name`, `parent`, `head`, `tail`.

A bone optionally carries `head`/`tail` — `[x, y, z]` positions in the document's coordinates, the
segment between them the bone's axis and length. Authored together or not at all: one without the
other is `L1160`, as is a non-finite component of either; a bone with neither is a pure grouping
node. `check-scene --bounds` prints an authored pair on its own line under the bone's bounds.

```yaml
bones:
  - name: root
  - name: upper_arm
    parent: root
    head:
      - 0.26
      - 1.35
      - 0
    tail:
      - 0.26
      - 1.05
      - 0
```

### Override pointers

Every numeric field is a `--set`/`--sweep` pointer (`ludion render --model`, `ludion docs
sculpt-authoring`): `/resolution`, `/triangles`, `/palette/<glyph>/color/<0..3>`,
`/palette/<glyph>/emissive|metallic|roughness`, `/parts/<i>/blend`, `/parts/<i>/at/<0..2>`,
`/parts/<i>/rotation/<0..2>`, `/parts/<i>/repeat/count`, `/parts/<i>/repeat/step/<0..2>`, and a
part's own shape fields (`radius`, `tube`, `height`, `round`, `radii/<0..2>`, `size/<0..2>`,
`from/<0..2>`, `to/<0..2>`, a `voxel`/`mesh` part's `size` — `ludion docs sculpt-format-sources` has
the mesh part's own creates-it-on-write nuance). Once authored, a bone's `head`/`tail` and a part's
`deform`/`noise` are pointers too: `/bones/<i>/head/<0..2>`, `/bones/<i>/tail/<0..2>`,
`/parts/<i>/deform/bend/degrees`, `/parts/<i>/deform/twist/degrees`,
`/parts/<i>/deform/taper/from|to`, `/parts/<i>/noise/amplitude`, `/parts/<i>/noise/scale`.

A non-numeric field (`op`, `material`, `bone`, `mirror`, `name`, `shape`, `path`) is `L4008`, naming
this domain — so is `noise`'s `seed` (a document identity, not a value to sweep) and a pointer whose
parent key is not authored (a missing `deform`, `deform/twist`, `noise`, or bone `head`/`tail`): an
override never creates one.

### Referencing it from a scene

A sculpt model is a `Mesh` shape, resolved relative to the content root, conventionally
`sculpts/*.sculpt.yaml`:

```yaml
  - name: idol
    components:
      Mesh:
        shape:
          sculpt: "sculpts/idol.sculpt.yaml"
```

Several entities on one path mesh it once (deduplicated by path), like a voxel model.

## Rules

- **`resolution` is `4..=128`; a lattice above 16,000,000 corners is refused at `/resolution`
  (`L1157`)** rather than allocated — the mesher's memory cap (`ludion devdocs ludion-sculpt`).
- **A `union` part needs a `material`; a `subtract`/`intersect` part must not have one** — it never
  contributes color, so authoring one is `L1157`.
- **A capsule takes no `at`** — its `from`/`to` place it; an `at` on a capsule is `L1157`.
- **`triangles` floors at 12** — below that a closed surface cannot survive decimation.

## Gotchas

- **`L1158` — a part the mesh cannot see:** thinner than two grid cells (the message names the
  `resolution` or thickness that would resolve it), or contributing no surface vertex — then it
  names the part that owns the field there, `hidden by part <name>`, often a *later* one. A warning,
  printed before the `OK (…)` summary. (A mesh part's own thinness rule is `ludion docs
  sculpt-format-sources`.)
- **`L1159` — the meshed surface splits into more than one piece:** the `L1129`-shaped warning,
  naming each stray piece and the nearest vertex pair to the main body.
- **Surface nets bevels a sharp edge over about one grid cell** before decimation reduces it to flat
  faces; `sharp: true` fixes the edge's *shading* (a crease-angle normal split), not its exact
  geometry — a box's edges read slightly rounded at ordinary counts. Crisp hard-surface
  edges are not this format's job today (`ludion devdocs ludion-sculpt`).


## Next

- `ludion docs sculpt-authoring` — the new → check → render → look loop and the text views.
- `ludion docs sculpt-craft` — what makes a valid model actually read.
- `ludion docs sculpt-format-sources` — the `voxel`/`mesh` parts that read another document.
- `ludion docs voxel-format` — the palette shape and the sibling faceted grid format.
- `ludion devdocs ludion-sculpt` — the mesher: pipeline, determinism, the conformance hash, cost model.
