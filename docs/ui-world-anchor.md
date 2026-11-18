---
description: UiWorldAnchor: pin a screen-space element to a 3D target, with edge clamp, hide and fade
---

# UI world anchor

**Scope:** a health bar over an enemy, an off-screen objective marker. Animation is `ludion docs ui-tweens`; the canvas is `ludion docs screen-ui`.

## The loop

Put `UiWorldAnchor` beside a `UiRect`, `UiText` or `UiList` (a presentation entity is the usual home; presentation to sim refs are allowed, the reverse is `L1036`), then `ludion render`. `extract` projects the target through the camera as drawn and moves the element there. The element's `anchor` becomes its pivot on the projected point and `offset` still adds, in canvas units.

```yaml
components:
  UiRect: { anchor: bottom_center, offset: [0, -12], size: [80, 8] }
  UiWorldAnchor:
    target: { index: 3, generation: 0 }
    world_offset: [0.0, 2.0, 0.0]
    fade: [20.0, 40.0]
```

## Format

| Field | Default | Meaning |
|---|---|---|
| `target` | required | entity whose `Transform` (world pose) is followed |
| `world_offset` | `[0,0,0]` | added in world axes |
| `show` | `always` | `on_screen` / `off_screen` draw only inside / outside the margin |
| `behind` | `clamp` | `hide` draws nothing behind the camera |
| `margin` | `48` | canvas units, `>= 0`; the edge inset used when clamping |
| `fade` | none | `[near, far]` metres, `0 <= near < far`: alpha 1 at `near`, 0 at `far` |

Bad numbers are `L1611`.

## Rules

- **Clamp:** off screen the element sits where the line from the viewport centre to the point meets the inset rectangle. Behind the camera (perspective only) it goes to the edge on the target's side, never mirrored; straight behind is the bottom edge. A margin of half the viewport or more pins everything to the centre.
- A target that is dead or has no `Transform` draws nothing, silently. A `fill: true` rect ignores anchors (`L2300`).
- Under `present_interpolation` the anchor follows the newest tick while the 3D scene is blended, so it can lead by up to one tick.
- Sim code and tests call the same placement: `ludion::math::world_anchor` (camera arguments as `world_to_pixel`, plus `margin`) returns the pixel, `on_screen`, `behind` and `distance`.

## Next

- `ludion docs ui-tweens` — animate the element.
- `ludion docs screen-ui` — canvas units and anchors.
