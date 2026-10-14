---
description: Packing sprite frames into a committed kind: atlas, and drawing one from Sprite or Tilemap
---

# Sprite sheets

**Scope:** turning sprite documents and loose PNGs into one packed, **committed** atlas, and spending
its regions from a `Sprite` or a `Tilemap` legend. Authoring the frames is
`ludion docs sprite-authoring`; the grid component's own conventions are `ludion docs tilemaps`;
the hub over the whole sprite layer is `ludion docs sprites`.

Three documents, two of them yours: you write a `kind: atlas-source` listing what goes in;
`ludion generate-atlas` writes the pair `<name>.png` + `<name>.atlas.yaml`, both committed; scenes
name the `kind: atlas` document and one region of it. Nothing packs at runtime, and the derived pair
is **write-only for authors** — edit the source and re-bake.

## The loop

1. **Author** `sprites/gallery.atlas-source.yaml` beside the sprite documents it lists.
2. **Bake the pair:**
   ```
   ludion generate-atlas examples/gallery/sprites/gallery.atlas-source.yaml
   ```
   prints `wrote …/gallery.atlas.yaml + …/gallery.png (52x76, 14 regions)`. Pass several sources in
   one run; `--out-dir <dir>` writes elsewhere (both files together — the document names the PNG
   beside it). An atlas over 8192 px a side is refused here with `L1153` at `/inputs` — split the
   source in two.
3. **Check what landed**, document and packed image together:
   ```
   ludion check-scene examples/gallery/sprites/gallery.atlas.yaml
   ```
   prints `OK (atlas, 52x76 px, 14 regions, 2 flipbooks, 16 px/unit, nearest filter, generated from
   gallery.atlas-source.yaml)`. This path reads the PNG too, so a region rect outside it is `L1153`.
4. **Look at the packing:**
   ```
   ludion render --atlas examples/gallery/sprites/gallery.atlas.yaml --out target/gallery.atlas-sheet.png
   ```
   The overview upscales the committed image with every region outlined in one of six cycling
   colours, named, and its pivot crossed. CPU-only: no adapter is requested. `--out` defaults to
   `<stem>.atlas-sheet.png` (a sprite document and its atlas share a stem, so the two sheets must not
   share a name).
5. **Reference it** from a scene and check that too:
   ```
   ludion check-scene examples/gallery/sprite-atlas.yaml
   ludion render --scene examples/gallery/sprite-atlas.yaml --out target/sprite-atlas.png
   ```
6. **Re-bake after every edit** to a source or one of its inputs. `generate-atlas --check` is the
   gate, below.

## `kind: atlas-source` — what you write

```yaml
format: 1
kind: atlas-source
name: gallery                  # the output stem; absent, the source file's own stem
pixels_per_unit: 16.0
filter: nearest                # or `linear`; nearest is the default
inputs:
  - ship.sprite.yaml           # relative to THIS file's directory
  - bullet.sprite.yaml
flipbooks:
  - name: ship/bank
    frames:
      - ship/idle
      - ship/bank-left
      - ship/idle
      - ship/bank-right
    fps: 8.0
  - name: explosion/burst
    frames:
      - explosion/f0
      - explosion/f1
    fps: 12.0
    loop: false
```

| Key | Rule |
|---|---|
| `pixels_per_unit` | required, finite and `> 0` — how many texels of this art span one world unit, the way a voxel model declares `scale:` |
| `filter` | `nearest` (default) or `linear`. Point sampling is the default because a linear filter on pixel art is the commonest way crisp source art arrives on screen blurred |
| `inputs` | required, non-empty, each path relative to this file. A `.yaml`/`.yml` **sprite document** contributes one region per frame; a `.png` contributes its whole image as one region, pivoted at its centre. Any other extension is refused |
| `flipbooks` | optional. `name` (unique), `frames` (non-empty, region names), `fps` (`> 0`), `loop` (default `true`), optional `hold` — one integer `>= 1` per frame, so tick quantization is exact |

**A region's name is minted, not authored:** `<sprite document name>/<frame name>` for a sprite
document (the document's `name:`, else its file stem), and the file stem alone for a loose PNG. Two
inputs minting the same name is `L1153` at the offending `/inputs/<i>`.

**Flipbooks are data the atlas carries.** They are parsed, checked against the regions the inputs
actually produce (a name that resolves to nothing is `L1153` with a nearest match), and written onto
the derived atlas so a clip name resolves against the same document a frame name does. An `Animation`
component names one to play — `ludion docs sprite-animation`.

## `kind: atlas` — what the bake writes

```yaml
format: 1
kind: atlas
generated_from: gallery.atlas-source.yaml
pixels_per_unit: 16.0
maps:
  color: gallery.png
regions:
  bullet/enemy:
    rect:                       # x, y, w, h in texels, top-left origin
      - 21
      - 65
      - 4
      - 8
    pivot:                      # texels from the REGION's own top-left
      - 2.0
      - 4.0
```

Explicit rects only — no grid shorthand, so a region's position is readable straight out of the file
and out of a diff. `regions:` is sorted by name, and that name order *is* the canonical order, so a
packer change moves rects without reshuffling the file. `generated_from:` is the provenance marker
and the drift gate's pair-back check. `maps:` is a mapping so a further map is an added key
rather than a second document. `maps.color` is relative to the atlas file: `..` is fine, an
absolute, drive or device path is refused (`L1153`).

The packer is deterministic and deliberately explainable: shelves, filled left to right, over one
fixed sort (**height descending, then width descending, then name ascending**); a **1-texel extrude**
gutter around every region, so a `linear` sampler reaching past an edge finds that region's own
colour and never a neighbour's; both axes rounded up to a multiple of four, with **no power-of-two
requirement**. Same inputs, byte-identical pair, on either platform.

## The `atlas-drift` gate

A committed atlas that no longer matches what its source bakes is gated:

```
ludion generate-atlas --check examples/gallery/sprites/gallery.atlas-source.yaml
```

prints `up to date -> …gallery.atlas.yaml, …gallery.png`, or one `DRIFT <path>` per differing file
(`L1153`) and exits non-zero. It byte-compares **both** files and writes nothing. `scripts/check.mjs`
and CI run exactly this over every committed `*.atlas-source.yaml` and `*.font-source.yaml`, as the
`atlas-drift` step — a source with no baked pair fails too. The gate then pairs both ways: a baked
document naming a source from anywhere but that source's own output path is a moved or orphaned copy,
and a verified document that lost its `generated_from:` is rejected. The fix is always to re-run the bake.

## Drawing a region

A `Sprite` takes its pixels from exactly one place. `atlas` and `frame` are set **together** and
never beside `texture` — any other combination is `L1151`:

```yaml
  - name: ship
    components:
      Transform:
        translation:
          - 0.0
          - 2.7
          - 0.0
      Sprite:
        atlas: sprites/gallery.atlas.yaml     # relative to the content root
        frame: ship/idle
        layer: 10                             # sort key: higher draws in front
```

- **`size:` omitted derives the quad from the art**: the region's pixel `rect` width and height
  divided by the atlas's `pixels_per_unit`. A 16×12-texel region at 16 px/unit is `1.0 × 0.75` world
  units. An explicit `size:` *overrides* that — for untextured quads and deliberate stretches — and
  `Transform.scale` x/y multiplies whichever you get.
- **`lit: false` (the default) is texel × tint with no light lookup**, which is what a bullet swarm
  needs; `lit: true` takes clustered-forward shading: ambient, every point light, and the
  sun's light unshadowed (no shadow map, no IBL), all two-sided.
- **`color`** tints, multiplying the sampled texel; **`flash`** (`0`–`1`) lerps the result toward
  **`flash_color`** (HDR-capable, default white) for a hit flash the game times itself.
  `flash: 0.0` is render-identical by construction.
- **`layer` (`i16`, default `0`) is the primary sort key**: sprites draw in
  `(layer, view depth back-to-front, scene order)`, higher in front. A pure ordering, never a Z
  displacement — nothing leaks into picking, shadows or debug draws.
- **`blend: alpha` (default) or `additive`.** Both run through the one premultiplied
  `One, OneMinusSrcAlpha` blend, so the mode is a per-instance flag that never splits a batch.
- **`flip_x`/`flip_y` mirror the sampled art** left↔right and top↔bottom, through the UV rect at
  extract — no second copy of the region, no shader branch.
- **A region's `pivot` is the point the entity sits at and turns about** — the quad centre is offset
  by the pivot's fraction of the region's extent, so a `Transform.rotation` swings the art around the
  authored pivot, not the quad's middle. An untextured quad has no region, so no pivot.
- Every quad naming one atlas resolves to **one** texture entry: a thousand tiles batch into one
  instanced draw.
- **Crisp art shimmers under motion** until an orthographic `Camera` sets `pixel_snap: true`
  (`ludion docs components`).
- **A sprite is an XY quad; a `Collider` is on (x, z)** —
  `ludion docs visuals --section "Which plane is 2D?"`.

A `Tilemap` legend entry takes the same fields per glyph (`texture` **or** `atlas` + `frame`, plus
`color`, `lit` and `flip_x`/`flip_y`), and the grid carries one `layer` for every cell, so a whole
stage is one packed image — see
`examples/gallery/sprite-tilemap-atlas.yaml`, where `cell_size: 0.5` matches 8-texel regions at
16 px/unit so the room tiles with no stretch.

A screen-space `UiRect` takes the same `atlas` + `frame` pair under the same `L1151`/`L1152` rules,
so HUD icons live in the committed atlas instead of loose PNGs — `ludion docs screen-ui`.

## Rules

- **`L1151`** — the atlas reference does not resolve: a `frame` with no `atlas`, an `atlas` with no
  `frame`, `texture` and `atlas` together, or an atlas that could not be read or parsed. Raised by
  `check-scene` and at extract alike, on a `Sprite` or on one legend entry.
- **`L1152`** — the atlas resolved but has no region of that name, with the nearest matching name.
  The usual cause is a frame or document renamed without re-baking.
- **`L1153`** — an `atlas-source` or `atlas` document that does not hold together: a missing or
  wrong-typed key, an empty `inputs`, a duplicate region or flipbook name, a flipbook frame naming no
  region, a `hold` list a different length from its `frames`, a region rect outside the packed image,
  a missing colour map — and `--check` drift.

## Gotchas

- **Never hand-edit a `kind: atlas` document or its PNG.** An edit is undone by the next bake and
  caught by `atlas-drift`. Fix the `kind: atlas-source` and re-run `generate-atlas`.
- **You never meet the premultiplied-alpha convention.** The bake writes the atlas premultiplied and
  the loader premultiplies any straight-alpha PNG, so one pipeline blends both and authored colours
  are ordinary straight-alpha values everywhere you type them.
- **The sprite pass samples with a nearest sampler.** `filter:` records the sampling an atlas's art
  is authored for and travels with the document.
- **A region's `pivot` travels with it** through the bake unchanged (the frame's own, or its centre).
- **`--out-dir` moves both files or neither**: the pair must land in one directory.
- **On a densely packed atlas the overview's region labels overlap** — the six outline colours tell
  neighbours apart.

## Next

- `ludion docs sprites` — the hub: the sprite layer's documents, verbs and components at a glance.
- `ludion docs sprite-authoring` — writing the `kind: sprite` documents an atlas packs.
- `ludion docs sprite-animation` — playing a `flipbooks:` entry with `Animation`.
- `ludion docs tilemaps` — the legend, the placement convention, reading the grid from a system.
- `ludion docs components` — `Sprite` and `TileAppearance` fields and defaults.
