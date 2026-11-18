---
description: UiRect uv_rect and slice: sample a sub-rect of a texture, or draw a resizable nine-slice frame
---

# UI nine-slice

**Scope:** a textured `UiRect` that samples part of its image (`uv_rect`) or draws as a resizable frame (`slice`). Placement and draw order are `ludion docs screen-ui`; the atlas a frame may come from is `ludion docs sprites`.

## The loop

Add `slice` to a `UiRect` that has a `texture`, or an `atlas` and `frame`; `ludion check-scene`; `ludion render`. `extract` draws nine ordinary rects in place of one: the four corners keep their size, the four edges stretch along one axis, the centre stretches or is hidden. A rect without `slice` or `uv_rect` draws exactly as before.

```yaml
components:
  UiRect:
    anchor: bottom_center
    size: [640, 160]
    texture: ui/panel.png
    slice: { border: [8, 8, 8, 8], scale: 2 }
```

## Format

| Field | Default | Notes |
|---|---|---|
| `uv_rect` | none | `[u0, v0, u1, v1]` in 0..1, top-left origin, inside the texture (or atlas region); `u0 < u1`, `v0 < v1` (`L1612`) |
| `slice.border` | required | `[left, top, right, bottom]` in source texels of the texture or region, each `>= 0` (`L1612`) |
| `slice.scale` | `1` | canvas units per source texel for the corners, `> 0` (`L1612`) |
| `slice.center` | `stretch` | `hidden` skips the centre piece, leaving a frame |

## Rules

- **`slice` wins over `uv_rect`;** both together, or either without a `texture` or `atlas`, is the warning `L1613` and the field is ignored. `check-scene` reports it.
- **Corners are `border x scale` canvas units** (times the viewport's pixel scale). A border wider than the image region, or corners wider than the rect, shrink proportionally and draw; this is reported once per entity as the degraded warning `L2301`, not an error.
- **Edges stretch, they do not tile.** Frame art that visibly breaks when stretched is the trigger to add tiling.
- **An atlas region is the image:** the border counts texels of the region and the split stays inside the region's uv.
- **Tweens and anchors act on all nine pieces:** a `UiTween` scale, size or opacity and a `UiWorldAnchor` move the frame as one element (`ludion docs ui-tweens`, `ludion docs ui-world-anchor`).
- Pure sim logic and tests use the same split: `ludion::math::nine_slice` and `ludion::math::sub_uv`. Run `cargo run -p ludion --example ui_nine_slice` for the pieces at three sizes.

## Next

- `ludion docs screen-ui` — the canvas, anchors and draw order.
- `ludion docs ui-tweens` — animate a frame's size or opacity.
