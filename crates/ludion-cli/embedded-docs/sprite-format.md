---
description: The sprite and palette documents: keys, hex colours, mirror, frame patches, palette includes
---

# Sprite format

**Scope:** the `kind: sprite` document — glyph grid, palette, named frames, pivots — and the
`kind: palette` document it may include. It is the 2D sibling of a voxel model
(`ludion docs voxel-format`): single-character glyphs indexing a palette, `.` reserved for a
transparent texel, one `size` every frame shares, and a differently sized pose is a different
document. Checking, baking and tuning one is `ludion docs sprite-authoring`; the ordered `effects:`
list is `ludion docs sprite-effects`; packing frames is `ludion docs sprite-sheets`; what makes a
sprite *read* is `ludion docs sprite-craft`; the hub is `ludion docs sprites`.

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
    color: "#a0aabc"
  o:
    color: "#95edff"
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
`{}` may stay in flow. `examples/gallery/sprites/` holds worked documents in the accepted shape.

| Key | Rule |
|---|---|
| `format`/`kind` | `1` and `sprite`, both required |
| `name` | optional — the region prefix an atlas mints (`<name>/<frame>`), else the file stem |
| `size` | required `[width, height]`, each `1..=1024` texels — past that it is a texture, not an atlas region |
| `seed` | optional non-negative integer, `0` when absent — an unseeded document still bakes to one fixed image |
| `palette` | required, non-empty: an inline glyph map, or a `{ from, overrides }` include (below). Key = **exactly one** printable-ASCII character (`!` to `~`), never the reserved `.` |
| `palette/<glyph>/color` | required quoted `"#rrggbb"` or `"#rrggbbaa"` (below); the legacy `[r, g, b, a]` linear floats still load |
| `palette/<glyph>/emissive` | optional, finite, `>= 0` |
| `frames` | required, non-empty, **authored order** — the order an atlas mints region names in. A name is unique, non-empty and free of `/` (a region is `<doc>/<frame>`, so a `/` would make it ambiguous) |
| `frames/<name>/pivot` | optional `[px, py]` in texels from the frame's top-left, each inside `0..=size`; absent is the frame's centre |
| `frames/<name>/rows` | required unless `base`: exactly `size[1]` quoted strings of exactly `size[0]` characters, top-down; `.` transparent. Under `mirror`, only the kept half |
| `frames/<name>/mirror` | optional `x`, `y` or `xy` — author one half (below) |
| `frames/<name>/base` | optional name of another frame, instead of `rows` — the frame is that one plus `patches` |
| `frames/<name>/patches` | with `base`: each `{ at: [x, y], rows }` (below) |
| `effects` | optional; ops in application order |

**Emissive is palette data, not colour-map data.** The bake paints from `color` alone and an atlas
carries one `color` entry under `maps:`; `emissive` records the intensity a glyph is authored at, so it travels
with the document owning it.

### Hex colours

A colour is a quoted `"#rrggbb"` or `"#rrggbbaa"` string, any case: RGB is sRGB decoded to linear,
alpha is linear `byte / 255`. `#rgb` shorthand is refused. **An unquoted `#` starts a YAML comment**,
so `color: #a0aabc` parses as an empty value — always quote. The same string is the colour of
every effect parameter (`from`, `to`, `color`; `ludion docs sprite-effects`). The canonical writer
and `ludion import` write only hex; an exact-hex colour round-trips as hex. A voxel palette is a
different format (`ludion docs voxel-format`).

### Mirror

A symmetric frame authors half. `mirror: x` takes `ceil(w/2)` columns and reflects them left to
right, `y` takes `ceil(h/2)` rows, `xy` both; an odd size keeps the shared centre line in the half.
Expansion runs first and effects run on the full frame, so lighting stays asymmetric. A row of the
wrong width is `L1153` naming the expected half-width.

```yaml
format: 1
kind: sprite
name: gem
size:
  - 5
  - 2
palette:
  "#":
    color: "#ccccee"
  o:
    color: "#ff8019"
frames:
  idle:
    mirror: x
    rows:
      - "#o."
      - ".#o"
```

`ludion check-scene gem.sprite.yaml --rows` prints the **resolved** grid:

```
-- idle --
   01234
0  #o.o#
1  .#o#.
```

### Patches

A frame that differs from another by a few texels is `base` plus `patches` in place of `rows`.
Each patch is `at` (`[x, y]` in the base's **full** coordinates) and `rows`: a space keeps the base
texel, `.` writes transparent, any glyph writes that glyph. Add to the `gem` frames:

```yaml
  spark:
    base: idle
    patches:
      - at:
          - 1
          - 0
        rows:
          - "# #"
```

`--rows` prints it resolved (`idle` unchanged above):

```
-- spark --
   01234
0  ##.##
1  .#o#.
```

A `base` must itself author `rows` — patches never chain — and a patched frame cannot set `mirror`.
A patch outside the frame, an unknown `base`, or `mirror` on a patched frame is `L1153`. `at` and
`rows` are block sequences like everything else (`L1005`).

A recolour of a whole sprite is an atlas-source variant, not a copy — *Recolour variants* in
`ludion docs sprite-sheets`.

## One palette, many sprites

Thirty documents repeating the same sixteen colours drift on the first typo. Put the palette in its
own `kind: palette` document — `format`, `kind`, an optional `name`, and the identical glyph map —
and have each sprite reference it in place of its own:

```yaml
palette:
  from: emblem.palette.yaml
  overrides:
    O:
      color: "#f9edc4"
```

- `from` resolves **relative to the sprite document's own directory**; `overrides` is an ordinary
  palette map, optional — a glyph the shared palette defines is replaced, one it does not is added.
- The include resolves **at bake time**, so `check-scene`, `generate-atlas` and `render --sprite` all
  work against the merged palette: `--set`/`--sweep` address `/palette/<glyph>/…` whether the glyph is
  inherited or overridden (the shared file is never written), and a sprite that includes a palette
  bakes byte-identically to the same sprite with that palette inlined.
- `ludion check-scene palettes/orrery.palette.yaml` checks a palette alone.
- `examples/gallery/sprites/emblem.*` is the worked pair: palette, sprite, and the committed atlas.

## Rules

- **`L1153`** — the document does not hold together: a missing, wrong-typed or unknown key (with a
  nearest match), a `rows` grid that does not match `size`, a glyph naming no palette entry, a
  duplicate frame name, a pivot outside its frame, a bad hex colour, a bad `mirror` or patch. A JSON
  Pointer names the position (`/frames/idle/rows/3`, `/palette/~1`).
- **`L1147`** — a malformed palette document, or a `from` naming no readable `kind: palette`
  document, reported at `/palette/from` with the palette's own path and pointer in the message. A
  glyph a frame uses that the merged palette still lacks stays `L1153`.
- **`L1154`** — one `effects:` entry is invalid; the op set and every range it enforces are
  `ludion docs sprite-effects`.
- **`L1005`** — a non-empty flow collection anywhere in the document.

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

## Next

- `ludion docs sprite-authoring` — checking, baking, tuning and importing these documents.
- `ludion docs sprite-craft` — what makes a sprite read at game scale.
- `ludion docs sprite-effects` — the eight ops and their parameters.
- `ludion docs sprite-sheets` — packing frames into a `kind: atlas`.
- `ludion docs sprites` — the hub.
