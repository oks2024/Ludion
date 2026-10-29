---
description: DropShadow — the projected contact shadow under a character, first-surface rule, fade, footprint
---

# Drop shadow: `DropShadow`

**Scope:** `DropShadow` — a soft disc or capsule darkening the first surface straight below a
character, the depth cue for a body in the air. It is not a light: no sun is needed and it never
touches the light list. The flat telegraph a marker draws is `ludion docs ground-telegraphs`; the sun's
real shadow map is `ludion docs lighting`; parenting the component under a character is
`ludion docs hierarchy`.

## Format

```yaml
DropShadow:            # every field optional; `DropShadow: {}` is the default blob
  shape:
    type: disc         # or: type: segment, length: 1.2, width: 0.5
    radius: 0.5
  opacity: 0.6         # peak darkness
  softness: 0.5        # 0 hard anti-aliased edge .. 1 wide smudge
  max_distance: 20.0   # metres below the anchor a surface may be and still receive it
  fade_distance: 4.0   # length of the linear fade that ends at max_distance
```

| Field | Default | Range | Code |
|---|---|---|---|
| `shape` | disc, `radius: 0.5` | every dimension `> 0` and finite | `L2106` |
| `opacity` | `0.6` | `0..=1` | `L1014` |
| `softness` | `0.5` | `0..=1` | `L1014` |
| `max_distance` | `20.0` | positive, finite | `L2110` |
| `fade_distance` | `4.0` | `0..=max_distance` | `L2110` |

## Rules

- **Put it on a child at the soles.** The anchor is the entity's world position. A character is an
  unscaled root, a body-mesh child, and a child holding `DropShadow` at `translation: [0, 0, 0]`
  (the soles). The idiom is in `examples/gallery/drop-shadow.yaml`.
- **Projection is always world −Y.** Rotation only yaws a `segment` (its +X is the length axis); a
  tilted character still drops a straight-down shadow.
- **Size is the shape times the world `scale.x`**, constant with height: the blob does not grow or
  shrink as the body rises.
- **First surface only.** The shadow lands on the topmost opaque surface below the anchor, within
  `max_distance`. A surface is kept if its plane, extended to under the anchor, is at or below the
  anchor plus 5 cm: a ramp you stand on takes the whole blob, and a step top above you never does.
  Never the floor under the platform you stand on, never the caster's own meshes (its hierarchy root
  and every descendant).
- **Facing weight.** The darkening is scaled by the surface normal's `y`: a wall stays clean, a slope
  is slightly weaker (the cosine of its tilt).
- **Height fade.** Full strength until `fade_distance` before `max_distance`, then linear to zero.
- **It multiplies the lit colour**, emissive included, before bloom. Only opaque meshes receive:
  sky, sprites, billboards, text, particles and `GroundMarker` draw later and sit on top. Overlapping
  drop shadows and sun shadows multiply.
- **No cap on casters** (the occlusion tiles shrink past 256), at every quality including `low`; it
  ignores the `shadows` knob. A scene with no `DropShadow` renders and hashes exactly as without the
  feature.
- **Render-only and hash-neutral.** It rides the renderer's generic projector pass.

## Gotchas

- **Scale is the world scale.** A `DropShadow` under a scaled body mesh takes the parent's scale and
  shrinks (a 0.4-wide body makes a 0.2 disc, hidden behind it). Keep the root unscaled and scale a
  mesh child.
- **A black surface shows no blob.** The shadow multiplies what is lit; light the receiver.
- **A 5 cm self-receive band.** Caster geometry within 5 cm under the anchor can darken itself:
  anchor at the soles, not inside the body.
- **An anchor above the soles** lets the blob climb onto surfaces higher than the feet.

## Try it

`examples/gallery/drop-shadow.yaml`: a step, a floating platform, a ramp, a six-metre pit, a faded
case, a marker over a blob and a segment.

```
ludion render --scene examples/gallery/drop-shadow.yaml --out drop-shadow.png
```

## Next

- `ludion docs lighting` — the sun's shadow map, a different mechanism.
- `ludion docs ground-telegraphs` — the flat marker that draws on top of a drop shadow.
- `ludion docs hierarchy` — `Parent` and local transforms.
- `ludion docs components` — the built-in visual set.
