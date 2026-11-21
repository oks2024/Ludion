---
description: UiShape: analytic line, circle, ring, arc, wedge, rounded_rect; fill, stroke, origin rule, hit
---

# Screen shapes: `UiShape`

**Scope:** `UiShape`, one SDF-drawn primitive in screen space. It shares `UiRect`'s canvas, anchors, `order`,
`group`, `UiTween` and `UiWorldAnchor` (`ludion docs screen-ui`, `ludion docs ui-groups`,
`ludion docs ui-tweens`). Polygons, polylines and dashes are not drawn.

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

## Next

`ludion docs ui-groups` (clip, scale), `ludion docs ui-tweens`, `ludion docs components-ui`.
