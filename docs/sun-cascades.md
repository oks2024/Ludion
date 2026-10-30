---
description: Camera-fitted cascades for the sun's shadow: opt-in, the fit, seams, quality cap, profile and view
---

# Sun shadow cascades

**Scope:** `cascades` on `DirectionalLight` — the opt-in that fits up to four shadow maps to the
active camera, so shadows stay crisp near the lens and still reach the horizon. The default anchored
box, the sun's other tunables and the shafts' use of its map are `ludion docs lighting` and
`ludion docs atmosphere`; the quality axis is `ludion docs quality`.

## The loop

1. Opt the sun in: `cascades: 4` on its `DirectionalLight`.
2. `ludion render --scene examples/gallery/sun-cascades.yaml --out cascades.png`.
3. `ludion profile --scene examples/gallery/sun-cascades.yaml --frames 30 --json target/profile.json`
   and read `sun_cascades`: `authored`, `count`, `tile_edge`, and per cascade `near`, `far`, `texel`
   and `casters`. `count < authored` means quality capped it; `count` 0 with `authored > 0` means
   shadows are masked or the camera is degenerate. The panel is a planning shape, not a timing, so
   two runs agree exactly.
4. `ludion render --scene examples/gallery/sun-cascades.yaml --out tint.png --debug-view sun_cascades`
   tints each lit mesh fragment by its cascade, nearest first: red, green, blue, yellow. Tints blend
   across the seam bands; a fragment past the last cascade stays untinted. The view is also the wire
   `render.debug` `view: "sun_cascades"` (`ludion docs wire-render`).

## Format

```yaml
DirectionalLight:
  direction:
    - 0.5
    - -0.85
    - -0.3
  cascades: 4
  shadow_distance: 100.0
  cascade_splits:   # optional; cascades - 1 values
    - 0.1
    - 0.3
    - 0.6
```

| Field | Default | Range | Code |
|---|---|---|---|
| `cascades` | absent (the anchored box) | integer `1..=4` | `L1014` |
| `shadow_distance` | `100.0` | finite, `> 0`, world units | `L1014` |
| `cascade_splits` | absent (practical scheme) | exactly `cascades - 1` values, strictly ascending in `(0, 1)` | `L1123` |

Each field is saved only when set, so a sun that leaves them out saves, hashes and renders exactly as
before. `cascade_splits` are the inner far bounds as fractions of `shadow_distance`.

`shadow_near`/`shadow_far` keep their meaning, a depth range along `direction`, but are centred on
each cascade rather than on the anchor and widened to at least the cascade's radius either way.
`shadow_extent` and the entity's `Transform` anchor are unused under `cascades`; authoring a
non-default `shadow_extent` beside `cascades` is the warning `L1313`.

## Rules

- **The fit follows the camera, with no history.** Each cascade covers a slice of the view frustum.
  The splits are the practical blend of logarithmic and uniform spacing (weight `0.75`), or the
  authored `cascade_splits`. The last far bound is the smaller of `shadow_distance` and the camera's
  far plane.
- **Edges do not crawl.** A cascade's half-size is the bounding sphere of its slice, so turning the
  camera never resizes it, and its centre snaps to a world-locked texel grid, with one texel of
  margin each side. Moving the camera shifts which texels cover the ground, never the texel grid.
- **The cascade is picked by view depth:** the first whose far bound lies past the fragment.
- **Meshes blend at the seams.** Across a band of 10% of each cascade's depth the two neighbours
  mix, and the last cascade fades to lit across its own band, so no hard edge marks the shadow's end.
- **Light shafts pick hard.** The march (`ludion docs atmosphere`) takes the cascade of each step,
  with no band. On a frame with shafts the last cascade extends to the march's reach, `3/fog_density`,
  so an occluder anywhere within the reach shadows the air behind it; a step past the last far bound
  is lit. The inner splits do not move.
- **Bias is held per cascade.** `shadow_bias` means the same number of texels in every cascade, so a
  fine near cascade does not lift contact shadows off their casters.
- **One atlas.** The cascades are tiles of `shadow_map_size` in the same `shadow` pass and
  `shadow_map` transient, laid out 1×1, 2×1 or 2×2 (a count of 3 uses the 2×2 with one tile empty).
  Each tile draws only the casters its own cull keeps. The sun keeps its own atlas, apart from the
  local lights'.
- **Quality caps the count.** The `sun_cascades` axis is 4 at `full`, 3 at `high`, 2 at `medium` and
  `low` (where `shadows: false` masks it). The effective count is the smaller of `cascades` and the
  cap; a TOML `[params] sun_cascades` in `1..=4` replaces the preset's cap in either direction. A
  slower machine gets fewer cascades, but the shadow still ends at the same distance: the last far
  bound does not depend on the count.
- **Probes use the anchored box.** A reflection-probe capture face never sees the cascades.

## Gotchas

- **A golden is a function of the camera pose.** The fit follows the active camera, so the same scene
  from two poses shadows differently.
- **Cost.** A cascaded sun on a dense scene can cost several times the single box's `shadow` pass,
  because every cascade redraws the casters it sees. Check the `shadow` budget over the poses you
  ship; the measured numbers are in `Plan/shadow-mapping.md`.
- **Authored splits are ignored when quality lowers the count.** The fit re-splits with the practical
  scheme, since the authored list no longer matches.
- **`shadow_extent` and the anchor do nothing here** (`L1313`). Set `shadow_distance` instead.
- **Past the last cascade is lit.** Raise `shadow_distance`, or accept the fade.
- **The view on a sun without cascades draws the normal frame,** and so does a frame with no fitted
  cascades (`count` 0 in the panel).

## Next

- `ludion docs lighting` — the sun, its default box and the other shadow tunables.
- `ludion docs quality` — the presets and the `sun_cascades` axis.
- `ludion docs atmosphere` — the shafts that march the cascades.
- `ludion docs debug-draw` — the `sun_cascades` view beside the other debug views.
- `ludion docs wire-render` — the `render.debug` views.
- `ludion docs local-shadows` — opt-in shadows for point and spot lights.
