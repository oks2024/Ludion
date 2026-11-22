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

A radar blip and a turning edge arrow:

```yaml
# the blip: placed in the box of the rect showing the `radar` view
UiRect: { anchor: top_left, size: [12, 12] }
UiWorldAnchor: { target: { index: 3, generation: 0 }, camera: { index: 9, generation: 0 }, margin: 6 }
# the arrow: a `UiShape` pointing up
UiWorldAnchor: { target: { index: 3, generation: 0 }, rotate_toward: true, distance_scale: [5.0, 60.0, 1.0, 0.6] }
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
| `rotate_toward` | `false` | an edge-clamped element turns so its local "up" points at the target |
| `distance_scale` | none | `[near, far, scale_near, scale_far]`: linear scale between the distances, `0 <= near < far`, scales `> 0` |
| `camera` | main camera | a `Camera` + `RenderView` entity: place the point through that view in the box of the `UiRect` showing it |

Bad numbers are `L1611`. Code that builds the component writes `UiWorldAnchor { rotate_toward: true, ..UiWorldAnchor::new(target) }`.

## Rules

- **Clamp:** off screen the element sits where the line from the viewport centre to the point meets the inset rectangle. Behind the camera (perspective only) it goes to the edge on the target's side, never mirrored; straight behind is the bottom edge. A margin of half the viewport or more pins everything to the centre.
- A target that is dead or has no `Transform` draws nothing, silently. A `fill: true` rect ignores anchors (`L2300`).
- **`rotate_toward`** applies only while the element is clamped (not `on_screen`) and adds to its rotation (authored and tween) about the same pivot a `rotation` uses; draw the art pointing up. A `UiList` takes no rotation (`L2300`). The angle is clockwise degrees from 12 o'clock, `WorldAnchorPlacement.angle_deg`.
- **`distance_scale`** multiplies the tween scale and pivots on the projected point, so a `bottom_center` meter stays on the head; `offset` scales with it. The distance is camera to point, in metres.
- **`camera`** projects through the named view into the first visible `UiRect` (entity order) whose `view` names it; `margin` insets that box. The box follows the rect's position and size, not its tween, rotation or a group's scale, and the element is not clipped to it. A camera that is dead, has no `RenderView`, is disabled or over budget, or is shown by no rect draws nothing: `L2308`, never a fallback to the main camera. `ui_hit` does not follow a `camera` anchor.
- Under `present_interpolation` a world-anchored element moves with the blended camera and target point, so it does not lead the 3D scene (`show`, `behind`, `fade`, `distance_scale` and the `rotate_toward` turn take the newer tick). Under `late_latch` it follows the blended, not the latched, camera.
- Sim code and tests call the same placement: `ludion::math::world_anchor` (camera arguments as `world_to_pixel`, plus `margin`) returns the pixel, `on_screen`, `behind`, `distance` and `angle_deg`; `ludion::math::place_world_anchor_in` takes a pixel box.

## Next

- `ludion docs ui-tweens` — animate the element.
- `ludion docs screen-ui` — canvas units and anchors.
