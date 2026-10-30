---
description: Shadows from PointLight and SpotLight: opt-in, the caster budget, tuning and observing them
---

# Local-light shadows

**Scope:** `cast_shadows` on `PointLight` and `SpotLight` — which lights get a shadow map each frame,
the budget that decides it, the bias and near tuning, and how to see the plan. The sun's shadow is
`ludion docs lighting`; the contact blob under a character is `ludion docs drop-shadow`; the lights'
other fields are `ludion docs components`.

## The loop

1. Opt a light in: `cast_shadows: true` on the `PointLight` or `SpotLight`.
2. `ludion render --scene examples/gallery/local-shadows.yaml --out local-shadows.png`.
3. `ludion profile --scene examples/gallery/local-shadows.yaml --frames 30 --json target/profile.json`
   and read `local_shadows`: `requested`, `granted`, `skipped`, `views`, `atlas_edge`. `skipped > 0`
   means opted-in lights the camera sees went unshadowed. At a quality that masks shadows (`low`) the
   panel reports every requested light as skipped, so a `skipped == 0` check must run at a quality
   with shadows on.
4. `ludion render --scene examples/gallery/local-shadows.yaml --out atlas.png --debug-view local_shadow_atlas`
   shows the atlas, and `--debug-draw local_shadows` marks each opted-in light in the world: green
   granted, red skipped. `ludion docs debug-draw` has the pen; the view is also the wire `render.debug`
   `view: "local_shadow_atlas"` (`ludion docs wire-render`).

## Format

```yaml
PointLight:
  intensity: 4.0
  range: 12.0
  cast_shadows: true     # omitted from the saved file when false
  shadow_bias: 0.02
  shadow_near: 0.05
```

| Field | Default | Range | Code |
|---|---|---|---|
| `cast_shadows` | `false` | — | — |
| `shadow_bias` | `0.02` | `>= 0`, world units | `L1014` |
| `shadow_near` | `0.05` | finite, `> 0` and `< range` | `L1312` |

Both components carry the same three fields. The view's far plane is `range`. A light that leaves them
at their defaults saves and hashes exactly as before.

## Rules

- **Meshes only.** Meshes cast and receive (a `mask` mesh casts its holes, a double-sided one from both faces); sprites, billboards, text and particles do neither, and
  fog and global illumination are not shadowed.
- **A fixed budget per frame.** Candidates are the opted-in lights whose cluster footprint touches
  the camera frustum: the range sphere for a point light, the cone's bounding sphere for a spot. They rank by distance from the camera to the sphere's surface, nearest first, ties
  going to light-list order (point lights, then spots, each by ascending `EntityId`).
- **An 8×8-cell atlas, granted whole.** A spot takes a 2×2 block of cells; a point light takes six
  one-cell cube faces, and so does a spot with `outer_angle` over 60°. A light is granted only if all
  its views fit; one that doesn't is skipped and later candidates are still tried. That is 16 spot
  tiles at most, at every quality.
- **No fade, no hysteresis.** A light that falls out of the budget loses its shadow at once, and a
  light that enters it gains one at once. Nothing is cached between frames.
- **Quality scales only the atlas edge.** `local_shadow_atlas_size` is 4096 at `full` and `high`,
  2048 at `medium` (`ludion docs quality`). Which lamps cast never depends on the machine. `low` turns
  the sun's and the local shadows off together.
- **Each granted view draws only what it can see,** culled on the CPU; skinned meshes are always drawn.
  A frame that grants nothing keeps its previous frame graph exactly.
- **Keeping every framing in budget** is a profile check: a game that wants all its lamps shadowed
  asserts `local_shadows.skipped == 0` over the camera poses it ships.

## Gotchas

- **Acne or peter-panning.** Speckle on a lit surface wants a larger `shadow_bias`; a shadow detached
  from its caster wants a smaller one. The bias is in world units and applied before projection.
- **`shadow_near` too large clips casters close to the lamp.** Keep it well below the nearest caster.
- **The granted set follows the camera.** The same scene from two poses can shadow different lights, so
  a golden's casters depend on its pose.
- **Nothing granted draws the normal frame.** `--debug-view local_shadow_atlas` on such a frame shows
  the scene, not an empty atlas.
- **`local_shadow` is a legal `[passes]` budget name,** even on a scene that never runs it.

## Next

- `ludion docs lighting` — the sun's shadow map, point-light calibration and the spot cone.
- `ludion docs quality` — the presets and the `local_shadow_atlas_size` axis.
- `ludion docs debug-draw` — the `local_shadows` category.
- `ludion docs wire-render` — the `render.debug` views.
