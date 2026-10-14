---
description: mesh-edit region details — polyline capsule, the key colour filter, the seeded surface fill
---

# Mesh-edit regions

**Scope:** the parts of a `kind: mesh-edit` op's `region` that need more than a line — the polyline
`capsule`, the `key` colour filter and the seeded `fill`. The document, the loop and the verbs are
`ludion docs mesh-editing`; a sphere and a box are as that topic describes them.

## Format

- **`capsule`** takes `points:`, a polyline of **two or more** `[x, y, z]` points — everything within
  `radius` of the line through them, in order; two points are a straight segment. It has no `from`/`to`
  (a sculpt capsule part does; this one does not): the old spelling is `L1173` and its message shows
  the rewrite.

```yaml
      capsule:
        points:
          - - 0.10
            - 1.34
            - 0.20
          - - 0.14
            - 1.34
            - 0.20
        radius: 0.012
```

### `key`: only where the colour matches

`key: { map, color, tolerance, color_feather? }` beside the shape keeps only the texels whose colour
is within `tolerance` of `color` — **Euclidean distance over sRGB components in `[0, 1]`**, `0`
matching only the exact colour — and gives every other texel weight `0`. `map` is any of the five
slots, read as the map stands after the earlier ops (a slot the source lacks and no earlier op wrote
is `L1176`). A key that rejects **every** texel its region reaches is `L1178`, naming the nearest
distance it found and that texel's colour — the number to raise `tolerance` to, or to re-sample
`color` from.

`color_feather` (optional, `>= 0`, the same sRGB-distance units) softens the colour cut: a texel at
distance `d` past `tolerance` keeps `(tolerance + color_feather - d) / color_feather` of its weight,
reaching `0` at `tolerance + color_feather`. The colour weight multiplies the shape's own weight
(and its `feather`). Absent or `0` is the hard edge.

```yaml
      key:
        map: base_color
        color:
          - 0.62
          - 0.08
          - 0.05
        tolerance: 0.12
        color_feather: 0.08         # optional, fades the match to 0 at a distance of 0.20
```

### `fill`: a patch of surface, by colour

```yaml
      fill:
        at:                         # a surface point — `mesh pick`'s `at:` block
          - 0.118
          - 1.342
          - 0.203
        tolerance: 0.15             # required, >= 0
        max_radius: 0.05            # required, > 0, straight-line world units from `at`
        map: base_color             # optional, the map whose colour is compared; default base_color
        feather: 0.01               # optional, at the max_radius edge
        color_feather: 0.05         # optional, a soft colour edge past tolerance
```

A fill starts at the covered texel nearest `at` and keeps every texel **reachable across the
surface** whose colour is within `tolerance` of the **seed's** colour (the same sRGB metric as `key`,
measured against the seed, never against a neighbour) and whose surface point is within `max_radius` of
`at` in a 3D straight line. Weight is `1` for every texel reached; `feather` fades it linearly to `0`
over the last `feather` world units **inside** `max_radius` (the fill never paints past the radius).
`color_feather` extends the patch to texels nearer than `tolerance + color_feather` to the seed's
colour — the walk goes on through that band — each weighing its colour fade, the same formula as a
key's, times its `feather` weight. A `key` beside a fill filters its result further.

- **Connectivity is the surface's, not the UV layout's.** Vertices weld by exact position, so the
  walk crosses a UV seam through the mesh's own edges; a patch that a texture atlas cut in two is one
  patch. Texels no triangle covers are never reached, and two UV islands that merely touch in the
  atlas do not leak into each other.
- **The seed must be on the surface**, within a thousandth of the mesh's bounding-box diagonal (a
  picked point, copied at its printed precision, always is), and inside `max_radius`. A seed off the
  surface is `L1178`, so a fill never silently starts somewhere other than where you pointed.
- **`max_radius` is the bound that keeps a fill from flooding the model** — pick it from the size of
  the thing you mean to light, not from the mesh's bounds.

## Next

- `ludion docs mesh-editing` — the document, the blind-authoring loop, the verbs and the codes.
