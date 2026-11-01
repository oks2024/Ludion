---
description: The sprite loop: check-scene, the contact sheet, --steps, --set/--sweep tuning, importing a PNG
---

# Sprite authoring

**Scope:** the loop around a `kind: sprite` document — check it alone, read its resolved grid, bake
the contact sheet, see each effect's step, tune a value without editing the file, import a PNG. The
document itself (keys, hex colours, `mirror`, patches, palette includes) is `ludion docs sprite-format`;
the ordered `effects:` list is `ludion docs sprite-effects`; packing the frames is
`ludion docs sprite-sheets`; the grid component that spends the atlas per cell is `ludion docs tilemaps`.
What makes a sprite *read* at game scale is `ludion docs sprite-craft`. The hub is `ludion docs sprites`.

## The loop

1. **Author** `sprites/ship.sprite.yaml`: the grid by hand, the shading by `effects:`.
2. **Check it alone.** No GPU, and it points at the exact bad row:
   ```
   ludion check-scene examples/gallery/sprites/ship.sprite.yaml
   ```
   prints `OK (ship, 16x12, 4 palette glyphs, 3 frames, 3 effects)` and one `idle: pivot (8, 6)`
   line per frame — the *resolved* pivot, so a defaulted one reads without arithmetic.
   `--rows` adds every frame's glyph grid, with a column ruler and row numbers, before the `OK` line.
   It prints the **resolved** grid — `mirror` and `patches` already applied:
   ```
   ludion check-scene examples/gallery/sprites/ship.sprite.yaml --rows
   ```
   It is cheaper than an image for an exact texel fix, and it is the grid the bake reads. The
   document's keys: `ludion docs sprite-format`.
3. **Bake it and look:**
   ```
   ludion render --sprite examples/gallery/sprites/ship.sprite.yaml --scale 4 --out target/ship.sheet.png
   ```
   The sheet is composed on the CPU — no adapter is requested, so it answers on a machine with no
   GPU. A header (document, texel size, frame count, seed); **block A**, every frame
   nearest-upscaled and labelled, pivot crossed in magenta, the first tile ruled along its top and
   left edges so texels can be *counted*, a checkerboard behind every transparent texel and, from 6
   sheet pixels per texel up, a 1-px texel grid (every 8th line darker); **block B**, every frame again at `--scale N` whole sheet
   pixels per texel — the "readable at the size it ships at" half no upscaling can answer; and a
   **footer echoing the effect list that was applied**, in order. The same echo goes to stdout, so a
   session that never opens the image still reads what produced it:
   ```
   wrote target/ship.sheet.png (1696x553)
     effect 0. gradient  glyphs: #  frames: all  from=#ffffff to=#b3b8c8 direction=vertical blend=multiply
     effect 1. shade  glyphs: all  frames: all  light=[-1, -1] strength=0.35
     effect 2. outline  glyphs: all  frames: all  color=#303045 side=outside
   ```
   `--out` defaults to `<stem>.sheet.png` in the current directory; `--scale` is a whole number in
   `1..=16` (default `1`) — a fraction of a texel is not a pixel anyone can judge, so it is refused.
4. **Tune without touching the file** — `--set` and `--sweep`, below.
5. **Pack it** into an atlas and reference it: `ludion docs sprite-sheets`.

## Seeing what each effect did: `--steps`

```
ludion render --sprite examples/gallery/sprites/ship.sprite.yaml --steps [--frame idle]
```

A **steps sheet** instead of the contact sheet: one row per frame, the authored paint first and then
the image after each effect that applies to that frame, cumulatively, each cell captioned with that
effect's echo line (the glow, outline or shade that made the change). Same checker and grid as block
A, no ruler or pivot. `--out` defaults to `<stem>.steps.png` (`<stem>-<frame>.steps.png` with
`--frame`); stdout adds one `steps` line per row, listing the effect indices that produced its cells:

```
  steps idle: paint, 0, 1, 2
```

`--set` applies; `--sweep` is refused by clap. At most 16 cells in all: past that `L4008` names
`--frame <name>` (or says to split the effect list). `--frame` without `--steps` is `L4008`.

## Tuning: `--set` and `--sweep`

Both edit the document **in memory only**: the file is never written, so a look costs no replay.

```
ludion render --sprite examples/gallery/sprites/ship.sprite.yaml \
  --set /effects/1/strength=0.5 --set /palette/o/emissive=4.0 --out target/ship-set.png
ludion render --sprite examples/gallery/sprites/ship.sprite.yaml \
  --sweep /effects/1/strength=0.1,0.35,0.6 --out target/ship-shade.png
```

The first echoes `set /effects/1/strength: 0.35 -> 0.5` per override (a field left unset reads
`unset -> …`); the second bakes once per value into a labelled grid, at most 16 cells.

The domain is the tunable values alone: `/seed` (a non-negative integer, `null` to unseed),
`/palette/<glyph>/color` (a hex string), `/palette/<glyph>/emissive`, `/frames/<name>/pivot/<0|1>`,
`/effects/<i>/<param>` (a scalar, an enum keyword or a hex colour — `strength`, `density`, `amount`,
`radius`, `radius_px`, `direction`, `blend`, `side`, `from`, `to`, `color`), and
`/effects/<i>/<param>/<j>` (one component of `light` or `offset`). A glyph that is itself `/` or `~`
is escaped RFC 6901 style (`/palette/~1/color`).

Anything else is `L4008`, listing those shapes: `size`, a frame's `rows` and an effect's
`op`/`glyphs`/`frames` are the document's **shape**, not a knob, and a pointer that could rewrite
them would let a typo bake a sprite nobody authored. Range is not checked here — the tuned document
is re-validated before it bakes, so an out-of-range value is the ordinary `L1153`/`L1154`.

## Importing a PNG

Art authored elsewhere becomes an editable document through the verb that compiles a glTF:

```
ludion import crates/ludion-cli/tests/fixtures/atlas/spark.png --out target/spark.sprite.yaml
```

prints `wrote target/spark.sprite.yaml (4x4, 2 colours)`. `--out` names the document (an existing
*directory* receives `<stem>.sprite.yaml`), `--name` sets its `name:` (default the file stem). A
fully transparent texel becomes `.`; every distinct remaining RGBA8 value becomes one palette entry,
the busiest colour taking the first glyph, so the grid read as text is a rough greyscale of the
sprite. One frame is written, `main`, no pivot and no effects. The document is yours to edit, so an
import never overwrites one: an existing `--out` target is `L0030`, remove it first to re-import.

The import is **lossless while the image's colours fit the 93-glyph budget**; past it it refuses
rather than quietly dropping colours, and `--quantize N` (N up to 93) merges by deterministic median
cut, reporting the cost:

```
error[L4014]: the image holds 167 distinct RGBA colours, more than the 93 glyphs a sprite document has to name them with
warning[L4015]: --quantize merged 167 colours down to 24; the largest per-channel change is 71 of 255
```

The per-channel error is in 8-bit units — single digits are invisible, tens are a different sprite.
Then read the document and fix the palette where the merge shows.

## Rules

- **`L4008`** — a `--set`/`--sweep` pointer outside that domain, or `--frame` without `--steps`.
- **`L4014`**/**`L4015`** — the PNG import's refusal and its quantization warning.
- **`L1153`**/**`L1154`**/**`L1147`**/**`L1005`** — the document, an effect entry, a palette include,
  a flow collection: `ludion docs sprite-format`.

## Gotchas

- **The bake refuses to run on an invalid document**, so every loop step above stops at the first
  error: fix the document first (`ludion docs sprite-format`).
- **The sheet's burned-in labels are uppercase** (the 8×8 font has no lowercase), so the glyph `o`
  is drawn as `O` while the stdout echo keeps the authored spelling.

## Next

- `ludion docs sprite-format` — the document: keys, hex colours, `mirror`, patches, palette includes.
- `ludion docs sprites` — the hub: the sprite layer's documents, verbs and components at a glance.
- `ludion docs sprite-craft` — what makes a sprite read at game scale: silhouette, marks, shades, glyphs.
- `ludion docs sprite-effects` — the eight ops, their parameters, and their gotchas.
- `ludion docs sprite-sheets` — packing frames into a `kind: atlas` and drawing one.
- `ludion docs sprite-animation` — cycling packed frames with `Animation`'s atlas flipbooks.
- `ludion docs tilemaps` — a whole stage of cells off one atlas.
- `ludion docs components` — the `Sprite` component's fields and defaults.
