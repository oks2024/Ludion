---
description: Authoring a kind: sprite document — the glyph grid, its palette, named frames, and the contact sheet
---

# Sprite authoring

**Scope:** writing a `kind: sprite` document — its glyph grid, palette, named frames and pivots — and
the CPU contact sheet you judge it by. It is the 2D sibling of a voxel model
(`ludion docs voxel-format`): single-character glyphs indexing a palette, `.` reserved for a
transparent texel, one `size` every frame shares, and a differently sized pose is a different
document. The ordered `effects:` list baked on top is `ludion docs sprite-effects`; packing the
frames is `ludion docs sprite-sheets`; the grid component that spends the same atlas per cell is
`ludion docs tilemaps`. The hub over all of them is `ludion docs sprites`.

## The loop

1. **Author** `sprites/ship.sprite.yaml`: the grid by hand, the shading by `effects:`.
2. **Check it alone.** No GPU, and it points at the exact bad row:
   ```
   ludion check-scene examples/gallery/sprites/ship.sprite.yaml
   ```
   prints `OK (ship, 16x12, 4 palette glyphs, 3 frames, 3 effects)` and one `idle: pivot (8, 6)`
   line per frame — the *resolved* pivot, so a defaulted one reads without arithmetic.
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
   wrote target/ship.sheet.png (2038x553)
     effect 0. gradient  glyphs: #  frames: all  from=[1, 1, 1, 1] to=[0.45, 0.48, 0.58, 1] direction=vertical blend=multiply
     effect 1. shade  glyphs: all  frames: all  light=[-1, -1] strength=0.35
   ```
   `--out` defaults to `<stem>.sheet.png` in the current directory; `--scale` is a whole number in
   `1..=16` (default `1`) — a fraction of a texel is not a pixel anyone can judge, so it is refused.
4. **Tune without touching the file** — `--set` and `--sweep`, below.
5. **Pack it** into an atlas and reference it: `ludion docs sprite-sheets`.

## Format

```yaml
format: 1
kind: sprite
name: ship
size:
  - 16
  - 12
seed: 4211
palette:
  "#":
    color:
      - 0.35
      - 0.40
      - 0.50
      - 1.0
  o:
    color:
      - 0.30
      - 0.85
      - 1.0
      - 1.0
    emissive: 2.5
frames:
  idle:
    pivot:
      - 8.0
      - 9.0
    rows:                       # twelve of these, each exactly sixteen characters
      - "......##........"
      - ".....####......."
effects:
  - op: shade
    light:
      - -1.0
      - -1.0
    strength: 0.35
```

**Block style is the parser rule, not a house preference.** The strict-YAML subset rejects every
non-empty flow collection — `size: [16, 12]`, `- { op: shade, … }` — as `L1005`, pointing at the
offending node (`--> /size/0`) and telling you to write one element per line; only an empty `[]` or
`{}` may stay in flow. `examples/gallery/sprites/` holds four worked documents in the accepted shape.

| Key | Rule |
|---|---|
| `format`/`kind` | `1` and `sprite`, both required |
| `name` | optional — the region prefix an atlas mints (`<name>/<frame>`), else the file stem |
| `size` | required `[width, height]`, each `1..=1024` texels — past that it is a texture, not an atlas region |
| `seed` | optional non-negative integer, `0` when absent — an unseeded document still bakes to one fixed image |
| `palette` | required, non-empty: an inline glyph map, or a `{ from, overrides }` include (below). Key = **exactly one** printable-ASCII character (`!` to `~`), never the reserved `.` |
| `palette/<glyph>/color` | required linear RGBA, each component in `0..=1` |
| `palette/<glyph>/emissive` | optional, finite, `>= 0` |
| `frames` | required, non-empty, **authored order** — the order an atlas mints region names in. A name is unique, non-empty and free of `/` (a region is `<doc>/<frame>`, so a `/` would make it ambiguous) |
| `frames/<name>/pivot` | optional `[px, py]` in texels from the frame's top-left, each inside `0..=size`; absent is the frame's centre |
| `frames/<name>/rows` | required: exactly `size[1]` quoted strings of exactly `size[0]` characters, top-down; `.` transparent |
| `effects` | optional; ops in application order |

**Emissive is palette data, not colour-map data.** The bake paints from `color` alone and an atlas
carries one `color` entry under `maps:`; `emissive` records the intensity a glyph is authored at, so it travels
with the document owning it.

## One palette, many sprites

Thirty documents repeating the same sixteen colours drift on the first typo. Put the palette in its
own `kind: palette` document — `format`, `kind`, an optional `name`, and the identical glyph map —
and have each sprite reference it in place of its own:

```yaml
palette:
  from: palettes/orrery.palette.yaml
  overrides:
    o:
      color:
        - 0.3
        - 0.85
        - 1.0
        - 1.0
```

- `from` resolves **relative to the sprite document's own directory**; `overrides` is an ordinary
  palette map, optional — a glyph the shared palette defines is replaced, one it does not is added.
- The include resolves **at bake time**, so `check-scene`, `generate-atlas` and `render --sprite` all
  work against the merged palette: `--set`/`--sweep` address `/palette/<glyph>/…` whether the glyph is
  inherited or overridden (the shared file is never written), and a sprite that includes a palette
  bakes byte-identically to the same sprite with that palette inlined.
- `ludion check-scene palettes/orrery.palette.yaml` checks a palette alone. A malformed palette
  document — or a `from` naming no readable `kind: palette` document — is `L1147`, reported at
  `/palette/from` with the palette's own path and pointer in the message; a glyph a frame uses that
  the merged palette still lacks stays the ordinary `L1153`.
- `examples/gallery/sprites/emblem.*` is the worked pair: palette, sprite, and the committed atlas.

## The effect ops

`effects:` applies ops from a closed set of eight — `gradient`, `outline`, `shade`, `shadow`,
`dither`, `noise`, `blur`, `glow` — in list order, each scoped by an optional `glyphs:`/`frames:`
filter. Parameters, ranges and traps: `ludion docs sprite-effects`.

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
  steps idle: paint, 0, 1
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
`/palette/<glyph>/color/<0..3>`, `/palette/<glyph>/emissive`, `/frames/<name>/pivot/<0|1>`,
`/effects/<i>/<param>` (a scalar or an enum keyword — `strength`, `density`, `amount`, `radius`,
`radius_px`, `direction`, `blend`, `side`), and `/effects/<i>/<param>/<j>` (one component of `from`,
`to`, `light`, `offset` or `color` — a `glow`'s `color` only when it authored one to write into). A glyph that is itself `/` or `~` is escaped RFC 6901 style
(`/palette/~1/color/0`).

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

- **`L1153`** — the document does not hold together: a missing, wrong-typed or unknown key (with a
  nearest match), a `rows` grid that does not match `size`, a glyph naming no palette entry, a
  duplicate frame name, a pivot outside its frame. A JSON Pointer names the position
  (`/frames/idle/rows/3`, `/palette/~1`).
- **`L1154`** — one `effects:` entry is invalid; the op set and every range it enforces are
  `ludion docs sprite-effects`.
- **`L1005`** — a non-empty flow collection anywhere in the document.
- **`L4008`** — a `--set`/`--sweep` pointer outside that domain.
- **`L4014`**/**`L4015`** — the PNG import's refusal and its quantization warning.

`ludion explain L1153` prints the full rule.

## Gotchas

- **A gradient, a glow, a shadow or shading painted into the glyphs is a defect.** Author the
  silhouette and its flat colour regions in the grid, everything on top of them in `effects:` — that
  is what keeps the grid diffable and the shading re-tunable by one `--set`. A missing op is a note
  for the notebook, not a licence to hand-paint the result: a painted highlight cannot be swept and
  does not follow a palette change.
- **The bake refuses to run on an invalid document**: an out-of-range colour has no defined image,
  and a guess would end up in a committed PNG.
- **Every op has a trap worth reading before you reach for it** — `blur` and `glow` need a
  transparent margin, `shade` seams a full-bleed tile, `noise` speckles dark glyphs:
  `ludion docs sprite-effects --section "Gotchas"`.
- **The sheet's burned-in labels are uppercase** (the 8×8 font has no lowercase), so the glyph `o`
  is drawn as `O` while the stdout echo keeps the authored spelling.

## Next

- `ludion docs sprites` — the hub: the sprite layer's documents, verbs and components at a glance.
- `ludion docs sprite-effects` — the eight ops, their parameters, and their gotchas.
- `ludion docs sprite-sheets` — packing frames into a `kind: atlas` and drawing one.
- `ludion docs sprite-animation` — cycling packed frames with `Animation`'s atlas flipbooks.
- `ludion docs tilemaps` — a whole stage of cells off one atlas.
- `ludion docs components` — the `Sprite` component's fields and defaults.
