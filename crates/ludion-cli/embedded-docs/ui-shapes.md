---
description: UiShape: line, circle, ring, arc, wedge, rounded_rect, polygon, polyline; fill, stroke, joins, hit
---

# Screen shapes: `UiShape`

**Scope:** `UiShape`, one SDF-drawn primitive in screen space. It shares `UiRect`'s canvas, anchors, `order`,
`group`, `UiTween` and `UiWorldAnchor` (`ludion docs screen-ui`, `ludion docs ui-groups`,
`ludion docs ui-tweens`). Dashes are not drawn.

## The loop

```yaml
  - name: countdown
    components:
      UiShape:
        anchor: center
        shape: { type: ring, radius: 80, thickness: 14, start_deg: 0, sweep_deg: 270 }
        fill: [1.0, 0.8, 0.2, 1.0]
        stroke: { width: 3, color: [0, 0, 0, 1] }
```

`ludion check-scene` validates the shape (`L1621`, `L1620` for `rotation`). Write `sweep_deg` each frame
for a gauge: a system sets the field, extract draws it. Runnable: `cargo run -p ludion --example ui_shapes`.

## Format

`shape` is required in a scene (a `UiShape` default is a circle of radius 16); `type:` selects the kind.
Lengths are canvas units; angles are clock degrees (0 is 12 o'clock, clockwise).

| `type` | fields | notes |
|---|---|---|
| `line` | `from`, `to`, `cap: butt\|round` (`butt`) | stroke only |
| `circle` | `radius` | |
| `ring` | `radius`, `thickness`, `start_deg` (`0`), `sweep_deg` (`360`) | the band `[radius - thickness, radius]` |
| `arc` | `radius`, `start_deg`, `sweep_deg` | stroke only: the band `radius ± width/2` |
| `wedge` | `radius`, `inner_radius` (`0`), `start_deg`, `sweep_deg` | |
| `rounded_rect` | `size`, `corner_radius` (`0`) | centred on the origin; the radius clamps to half the short side |
| `polygon` | `points: [[x, y], ..]`, `join: miter|round|bevel` (`miter`) | a simple polygon, filled and/or stroked; see Polygons |
| `polyline` | `points`, `closed` (`false`), `join` (`miter`), `cap: butt|round` (`butt`) | stroke only; see Polylines |

Other fields: `anchor` (`top_left`), `offset` (`[0,0]`, +Y down), `group`, `fill: [r,g,b,a]`, `stroke: {width, color}`,
`rotation` (degrees clockwise, `0`), `order` (`0`), `visible` (`true`). A shape with neither fill nor stroke
draws nothing; so does `sweep_deg: 0` and a zero-length butt line.

## Rules

- **Origin.** The shape's `(0, 0)` is the anchor point plus `offset`, moved into the group box when grouped.
  A group's or tween's scale and the `rotation` pivot on the origin, not a bounds centre (unlike `UiRect`).
- **Stroke** is centred on the edge and drawn over the fill. `line` and `arc` draw only their stroke: a
  `fill` on them, or no `stroke`, is `L1621`.
- `L1621` also covers non-finite numbers, `radius <= 0`, a ring `thickness` outside `(0, radius]`, an
  `inner_radius` outside `[0, radius)`, `sweep_deg` outside `[0, 360]`, a negative `size` or `corner_radius`,
  and a stroke `width <= 0`.
- **Tweens:** `opacity`, `offset`, `scale`, `rotation` and `color` (it replaces `fill`, or the stroke colour
  when there is no fill, and always on `line`/`arc`); `size` is `L2300`. There is no `sweep_deg` track.
- **Hit:** `ludion::math::ui_shape_hit(&shape, viewport, cursor)` tests the fill (`d < 0`) or the stroke band
  for the ungrouped, untweened shape with its own `rotation` turned back. `ui_hit` adds group, tween, anchor
  and clip. Both agree with the draw to the 1 px anti-aliasing band.

## Polygons

`polygon` fills a simple polygon (concave is fine, either winding) of 3 to 256 `points` in canvas units about
the origin, +Y down. It takes `rotation`, `group`, tweens and `UiWorldAnchor` like every shape. A `stroke`
outlines it over the fill (a stroke alone draws only the outline), turning corners by `join`; more than 256
points or a non-finite one is `L1621`.

- A polygon with fewer than 3 distinct points, zero area, or edges that cross or touch draws nothing and warns
  once per entity (`L2305`). Fewer than 3 points is not a scene error, so a game can write `points` live.
- Write `points` only on change: extract triangulates (ear clipping) every frame it draws.
- The edge is anti-aliased by a one-pixel fringe built at the final size, so a group's scale never widens it.
- Hit: an even-odd point-in-polygon test for the fill, so a concave notch is not a hit, plus the stroke band.

## Polylines

`polyline` draws a path of 2 to 256 `points` as its stroke only: a `fill`, or no `stroke`, is `L1621`. It may
cross itself. `closed: true` joins the last point back to the first, with no caps.

- **Joins** (polygon and polyline): `miter` keeps a sharp corner until it would be longer than 4 half-widths
  (SVG's limit; no field), then bevels; `round` is an arc about the corner; `bevel` is a flat cut.
- **Caps** (an open polyline): `butt` ends flush at the end point, `round` is a half-disc of the stroke's radius.
- Fewer than 2 distinct points draws nothing and warns once per entity (`L2305`), as a degenerate polygon does.
- A stroke under 1 unit wide draws 1 px wide with its alpha scaled by the width, so it fades rather than aliases.
- A translucent polyline darkens where it crosses itself: each part is drawn over the last.
- Tweens: `scale` multiplies the points and the stroke width, `opacity` the fill and stroke alpha, and
  `color` replaces the fill (the stroke on a polyline). Hit: the stroke band, joins and round caps.

## Next

`ludion docs ui-groups` (clip, scale), `ludion docs ui-tweens`, `ludion docs components-ui`.
