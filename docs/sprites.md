---
description: Index of the sprite layer: its document kinds, CLI verbs, components, and seven spokes
---

# Sprites

**Scope:** the index for 2D art: pixel-grid documents, the committed atlas they pack into, and the
components that draw one. A `Sprite` is a textured quad **in the 3D world** — ordinary geometry that
takes a `Transform`, lights when `lit: true`, and is usually framed by an orthographic camera; a HUD
icon is a screen-space `UiRect` sampling the same atlas. Pull the spoke covering what you are about
to change.

## The documents

| `kind:` | Written by | What it holds |
|---|---|---|
| `sprite` | you | a glyph grid, a palette, named frames with pivots, and an `effects:` list |
| `palette` | you | one glyph map several sprites include through `palette:`'s `from`/`overrides` |
| `atlas-source` | you | the inputs to pack, `pixels_per_unit`, `filter`, and `flipbooks:` |
| `atlas` | `generate-atlas` | the committed region rects + `<name>.png`; never hand-edited |

## The verbs

- `ludion check-scene <doc>` — checks a sprite, palette or atlas document alone, no GPU; `--rows` on a
  sprite prints every frame's glyph grid with a ruler.
- `ludion render --sprite <doc> --scale 8` — the contact sheet: every frame, ruled and pivot-marked.
- `ludion render --atlas <doc>` — the packing overview; `--clip <flipbook>` with `--time`/`--strip
  N`/`--onion` draws the animation instead (add `--scale 8`, or 16 px frames are unreadable).
- `ludion generate-atlas <source>` — bakes the committed pair; `--check` is the `atlas-drift` gate.
- `ludion import <png>` — turns art authored elsewhere into an editable `kind: sprite` document.

## The components, at a glance

Full field table and defaults: `ludion docs components`.

- **`Sprite`** — `atlas` + `frame` (set together, never beside `texture`); `size:` omitted derives the
  quad from the region's pixels ÷ `pixels_per_unit`; `lit` (default `false` — texel × tint);
  `layer: i16` the primary sort key (higher draws in front, a pure ordering, never a Z displacement);
  `blend: alpha|additive`; `flip_x`/`flip_y` mirror the sampled art through the UV rect; `flash` +
  `flash_color` lerp the final colour for a hit flash; `billboard` faces the camera. The region's
  **pivot** is the point the entity's translation holds and rotation turns about; a flip mirrors the
  pivot with the art, so the translation holds the pivot texel either way. A sprite whose placement
  is not finite (a NaN or infinite `Transform`, or one up its `Parent` chain) is skipped and
  reported once as `L2109`.
- **`Animation`** — `clip` names one of the atlas's `flipbooks:` instead of the authored `frame`.
- **`Tilemap`** — one `layer` for the whole grid; each legend entry takes `Sprite`'s own
  `texture`/`atlas` + `frame`/`color`/`lit`/`flip_x`/`flip_y` shapes.
- **`UiRect`** — the same `atlas` + `frame` pair, in screen space.
- **`Camera`**, orthographic, with `pixel_snap: true` — snaps the view height to a whole
  texel-to-pixel scale and each sprite centre to the texel grid, so crisp art stops
  shimmering under motion. Extract-only: the world hash never moves.

## Scrolling and tiling

`uv_repeat` repeats the frame across the quad; `uv_scroll` slides it, in frame widths per second. An
endless parallax band is then one entity — no wrap point, no second copy. Either field off its
default wraps the quad's region instead of clamping it, and the offset is derived from sim time at
extract, so nothing enters the world hash. A linear filter blends the column where a tile restarts
toward the atlas neighbour: pad the frame, or pack it alone.

```yaml
Sprite:
  atlas: sprites/sky.atlas.yaml
  frame: band/zodiac
  uv_repeat:
    - 8.0
    - 1.0
  uv_scroll:
    - 0.05
    - 0.0
```

## The spokes

- `ludion docs sprite-authoring` — the `kind: sprite` grid, palette includes, pivots, the contact sheet.
- `ludion docs sprite-craft` — what makes a sprite read at game scale, as checkable rules.
- `ludion docs sprite-effects` — the eight baked ops, their parameters, and their traps.
- `ludion docs sprite-sheets` — `generate-atlas`, the committed pair, the drift gate, drawing a region.
- `ludion docs sprite-animation` — flipbooks and `Animation` over a `Sprite`.
- `ludion docs tilemaps` — a whole stage of cells off one atlas, and reading the grid from a system.
- `ludion docs screen-ui` — `UiRect`/`UiText`: the reference canvas, anchors, and atlas-backed icons.
