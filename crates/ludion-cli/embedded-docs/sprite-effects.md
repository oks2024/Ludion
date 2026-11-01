---
description: The eight baked sprite effect ops, their parameters, the glyphs/frames filters, and their traps
---

# Sprite effects

**Scope:** the ordered `effects:` list a `kind: sprite` document bakes on top of its glyph grid — the
closed op set, each op's parameters and ranges, the two filters that scope them, and the traps each
has sprung. The document those effects sit in (its grid, palette, frames, pivots) and the contact
sheet you judge a bake by are `ludion docs sprite-format` and `ludion docs sprite-authoring`; the hub
over both is `ludion docs sprites`.

## The op set

Eight ops, a closed set: no expressions and no node graph (`engine-pillars.md` — Rust is the only
Turing-complete layer), so an op the set does not name is `L1154` with a nearest match rather than a
silently skipped entry. Every op is CPU-baked and deterministic, and every channel is clamped to
`0..=1` after each runs.

| `op` | Parameters (default) | What it does |
|---|---|---|
| `gradient` | `from`, `to` (required hex), `direction: vertical\|horizontal\|radial` (`vertical`), `blend: replace\|multiply` (`replace`) | Ramps between two colours across the **target set's own bounding box**, so a ramp over one glyph fills that glyph's extent; radial runs from the box centre to half its diagonal. `replace` takes the ramp's RGB and scales the texel's alpha by it, `multiply` scales all four |
| `outline` | `color` (required hex), `side: outside\|inside` (`outside`) | A one-texel border on the silhouette. `outside` paints transparent texels with a 4-neighbour in the set — the frame edge clips it, so leave a margin; `inside` paints the set's own edge texels |
| `shade` | `light` (`[-1, -1]`, never both zero), `strength` (`0.25`, in `0..=1`) | A one-texel bevel: the edge facing the light moves toward white by `strength`, the edge away scales by `1 - strength`, the interior is untouched. Texel space, `+x` right and **`+y` down** |
| `shadow` | `offset` (`[1, 1]`, never `[0, 0]`), `color` (black at half alpha, `"#00000080"`) | An offset copy of the silhouette landing **only on transparent texels**, so it never overwrites the sprite it belongs to |
| `dither` | `glyphs` **required, exactly two**; `density` (`0.5`, in `0..=1`) | An ordered 4×4 Bayer checker: a texel authored as the first glyph takes the second's colour where the threshold falls under `density`. Ordered, not random, so the pattern survives an edit that does not move the texel |
| `noise` | `amount` (required, `0..=1`) | Per-texel RGB jitter in `[-amount, amount]`, a pure function of `(seed, frame index, x, y, channel)`. Alpha is never jittered — a noisy silhouette is a different sprite |
| `blur` | `radius` (required, `1..=8` texels) | A box blur — the glow primitive. A wider glow is a second sprite, not a bigger radius |
| `glow` | `radius_px` (required, `0 < radius_px <= 32`, **fractional**), `strength` (required, `>= 0`), `color` (optional hex) | A soft halo grown **outside** the silhouette, leaving the opaque core crisp. Every transparent texel within `radius_px` of an opaque source texel gains alpha from a parabolic `1 - d²/r²` falloff; the halo's colour is `color` when given, else the contribution-weighted average of the sources' own colours (a red bolt glows red). `strength` scales the accumulated alpha, then it clamps to `0..=1`. Unlike `blur`, `radius_px` is fractional, so a bright 4–8 px core keeps its shape with a soft ring instead of dissolving |
| `light` | `radius_px` (required, `0 < radius_px <= 32`, fractional), `strength` (required in colour mode, `>= 0`), `color` (optional hex), `sources` (optional glyph list), `bands` (optional, `>= 1`), `mode`, `steps` | Bakes a sprite's **own** glow onto itself — see *Self-light* |

`light` and `offset` are block sequences like every other collection in the strict-YAML subset — a
flow sequence anywhere in a document is `L1005` (`ludion docs sprite-format --section "Format"`).
Colours are quoted hex strings, `"#rrggbb"` or `"#rrggbbaa"` (`ludion docs sprite-format --section "Hex colours"`).

## Ramp mode

`shade` and `gradient` take `mode: ramp` (default `color`) and a signed whole-number `steps`, and
repaint each texel along the palette's `ramps:` (`ludion docs sprite-format --section "Ramps"`) — the
output is always a palette colour, never a blend. A ramp op takes none of `from`/`to`/`blend`/`strength`.

- **`shade`** moves lit edge texels `+steps` and shadowed ones `-steps` along their ramp; a negative
  `steps` swaps them.
- **`gradient`** moves each texel from `0` steps at the start of its direction to `steps` at the end.
  Where that position falls between two steps, the 4×4 Bayer matrix picks per texel, so a `steps: -1`
  fade is a dithered band, not a seam. Stepping clamps at a ramp's ends.
- **The current-glyph plane.** The bake carries a glyph per texel beside the authored grid the filters
  test. Ramp ops read and write that plane, so a second ramp op steps what the first left; `dither`
  writes it too. A texel an `outline` or `shadow` created has no glyph and is skipped.
- **Ordering:** every ramp-mode op comes before every colour-mode op (`dither` may sit anywhere), since
  a colour op leaves the plane stale. Otherwise `L1154`, as are: no `ramps`, a target glyph in no
  ramp (narrow the op with `glyphs:`), and a colour key on a ramp op.

```yaml
effects:
  - op: shade
    mode: ramp
    steps: 1
  - op: gradient
    mode: ramp
    steps: -1
    direction: vertical
  - op: outline
    color: "#1b1d2b"
```

`hull-ramp.sprite.yaml` and its colour-mode twin `hull-color.sprite.yaml` are the pair
(`sprite-ramps` gallery scene). `--set`/`--sweep` address `/effects/<i>/steps`.

## Self-light

`op: light` lights a sprite's own opaque texels from its light-source glyphs: every glyph with
`emissive` above zero in the merged palette, or the glyphs named in `sources:` (none, or an unknown
glyph, is `L1154`). Receivers are the opaque texels within `radius_px` of a source texel, narrowed by
`glyphs:`; sources are never changed. The falloff is `glow`'s parabola, and the light follows its source
glyphs in every frame, so mirrored and patched frames light themselves.

- **Colour mode** adds `strength` × falloff of the light colour: `color` when given, else each source
  lights in its own colour. Overlapping sources combine per channel by **max**, then add to the texel.
  `bands: N` quantizes the falloff into N crisp rings.
- **Ramp mode** (`mode: ramp`, `steps >= 0`) steps receivers up their palette ramp by at most `steps`
  and repaints from the palette; `strength` and `color` there are `L1154`, and the ramp ordering rule above applies.
- `emissive` only *selects* sources — it is a render intensity and never scales the bake. No occlusion.
- Order: after the colour ops it must survive, before `outline`.

```yaml
effects:
  - op: light
    radius_px: 3.5
    strength: 0.45
    bands: 3
```

`ship-lit.sprite.yaml` is the worked example, beside `ship.sprite.yaml` (`sprite-self-light` gallery
scene). `--set`/`--sweep` address `radius_px`, `strength`, `color`, `bands`, `steps`.

## The two filters

- **`glyphs:`** — the texels whose **authored** glyph is one of these; that identity never changes,
  so a texel an earlier op repainted keeps its authored glyph and one an op created out of
  transparency belongs to no glyph at all. Without `glyphs:` an op targets whatever is **currently
  opaque**, so an outline follows a shadow's silhouette rather than the original grid's — except
  `blur`, which unfiltered covers the whole frame including transparent texels, because that spread
  into transparency *is* the glow.
- **`frames:`** — the frames the op applies to; absent is all.

List order is application order. `ship.sprite.yaml` reads gradient → shade → outline — plating tone,
then bevel, then the rim over both:

```yaml
effects:
  - op: gradient
    glyphs:
      - "#"
    from: "#ffffff"
    to: "#b3b8c8"
    direction: vertical
    blend: multiply
  - op: shade
    light:
      - -1.0
      - -1.0
    strength: 0.35
  - op: outline
    color: "#303045"
    side: outside
```

## Rules

- **`L1154`** — one `effects:` entry is invalid, ramp-mode cases included (above): an unknown `op` or key (with a nearest match), a
  missing required parameter, a value out of range, or a `glyphs:`/`frames:` name this document does
  not define. The JSON Pointer names the entry and the field (`/effects/2/glyphs/0`,
  `/effects/0/from`). `ludion explain L1154` prints the full rule.
- **An effect that has no op is a gap to record, not something to paint into the grid.** A gradient,
  glow, shadow or shading drawn as glyphs cannot be swept by `--set` and does not follow a palette
  change (`ludion docs sprite-format --section "Gotchas"`).
- **Tuning an op's parameters never edits the file** — `--set`/`--sweep` address
  `/effects/<i>/<param>` (and `/effects/<i>/<param>/<j>` for a `light` or `offset` component) in memory; anything outside that domain is
  `L4008`. `ludion docs sprite-authoring` owns the whole pointer domain.

## Gotchas

- **`shade` bevels the silhouette, and on a tile that fills its frame the silhouette *is* the frame
  border.** Out-of-frame texels read as transparent, so every edge texel of a full-bleed frame is an
  edge: the result is a bright column on the light side and a dark one opposite, a visible seam at
  every boundary once the tiles are laid. It also cannot touch interior features — a groove wall or a
  rivet head is not a silhouette edge. Filter it with `glyphs:` so the target set has an edge *inside*
  the frame, or drop the op and author the chamfers as flat colour regions.
- **`noise` jitters each channel by an absolute amount, not a proportional one.** Every channel draws
  independently in `[-amount, amount]`, so on near-black glyphs even `amount: 0.012` swings the
  channels apart relative to their own value and reads as coloured speckle rather than grain. Filter
  it to the mid-tone glyphs that can absorb it.
- **`blur` with no `glyphs:` filter needs room** — it spreads into transparency, so the frame needs a
  margin; `bullet.sprite.yaml` keeps a one-texel transparent border for that reason.
- **On a small bright sprite, reach for `glow`, not `blur`.** `blur`'s smallest `radius: 1` is a 3×3
  box that averages a 4–8 px core into a featureless smudge with no bright centre. `glow` instead
  adds a fractional-radius halo *outside* the silhouette and never writes an opaque texel, so the
  core stays crisp — a bullet or jewel keeps its bright centre and gains a soft ring.
  `glow-bullet.sprite.yaml` is the worked example. `glow` grows a halo into transparency, so leave a
  margin the same way; and its `glyphs:` filter scopes which texels *emit* the halo, while the halo
  still lands only on transparent texels around them.

## Next

- `ludion docs sprite-format` — the document these ops bake onto.
- `ludion docs sprite-authoring` — the contact sheet, `--steps`, and tuning.
- `ludion docs sprite-craft` — what makes a sprite read: marks with a cause, few shades, ops over paint.
- `ludion docs sprites` — the sprite layer end to end: documents, verbs, components.
- `ludion docs sprite-sheets` — packing baked frames into a committed `kind: atlas`.
