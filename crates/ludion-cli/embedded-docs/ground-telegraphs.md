---
description: GroundMarker — analytic disc, segment and wedge (vision cone) ground telegraph, color, order
---

# Ground telegraphs: `GroundMarker`

**Scope:** `GroundMarker` — a flat disc, capsule or wedge lying in the entity's local XZ plane, for an
area-of-effect landing zone, a charge-line indicator or a wireframe vision cone. Registering and defaulting it, and the rest
of the built-in visual set, are `ludion docs components`; the surfaces it never carries a material
for are `ludion docs materials`.

## Format

```yaml
GroundMarker:
  shape:
    type: disc        # or: type: segment, length: 8.0, width: 2.0
    radius: 3.0
  # A vision cone instead:
  # shape: { type: wedge, half_angle_deg: 30.0, outer_radius: 7.0, inner_radius: 3.0, fill: 0.5 }
  color:   # linear RGBA, HDR-capable, both fields have defaults
    - 1.6
    - 0.2
    - 0.1
    - 0.55
```

`shape` is `type: disc { radius }`, `type: segment { length, width }` (local +X is the segment's
length axis) or `type: wedge { half_angle_deg, outer_radius, inner_radius, fill, grid_spacing }`;
`color` is linear RGBA. Default shape is a unit-radius disc, default color opaque
white — an empty `GroundMarker: {}` draws something rather than nothing, the same posture as
`Sprite`/`Mesh`. Every shape dimension must be strictly positive and finite: `L2106`.

## Rules

- **Every dimension is the marker's maximum extent**, further multiplied by the entity's uniform
  `Transform.scale.x` — a telegraph filling in as its warmup counts down is a per-frame scale write
  (the countdown-`f32` idiom), never a re-authored shape.
- **A wedge is a wireframe sector**: apex at the entity origin, facing local +X, outline plus a polar
  grid (arcs every `grid_spacing` metres, default 1; spokes spaced so cells are about square at the
  rim) over a mostly transparent interior. `inner_radius` (default 0 = none) adds a range arc;
  `fill` (default 0) is the fraction of `outer_radius` filled from the apex — write it per frame as
  suspicion rises. `half_angle_deg` and `fill` are never scaled by `Transform.scale.x`; the radii and
  `grid_spacing` are. Line widths and opacities are fixed (the colour is the look). For a narrower
  inner zone, stack a second wedge. The wedge is not clipped by walls.
- **The marker's lift off the ground is a fixed engine constant, not an authored field** — no scene
  picks an offset, so a marker lying flush on a floor never z-fights the geometry it marks.
- **Draw order is fixed: after opaque meshes, before sprites and text.** A marker sits on the ground
  and never occludes a sprite standing on it, but an opaque mesh standing on the marker occludes the
  marker where it stands.
- **Color is linear RGBA; a value above `1.0` rides the bloom chain** exactly like an emissive
  mesh — how urgency reads at a glance. Alpha multiplies the marker's analytic coverage, so it is the
  marker's overall opacity.
- **A `GroundMarker` is always the analytic SDF primitive**, never voxel- or mesh-doc-backed, so
  `Mesh.emissive`'s inertness on palette/mesh-doc materials (`ludion docs materials`) does not apply
  to it.

## Gotchas

- A non-positive or non-finite `radius`/`length`/`width` is `L2106` at the field — the same posture
  as other strictly-positive shape dimensions in the engine.
- A wedge needs `half_angle_deg` in `(0, 180]`, `inner_radius` in `[0, outer_radius)` and `fill` in
  `[0, 1]`: `L2113`. A `DropShadow` authoring `type: wedge` is `L2113` too.
- Keep `width <= length` on a segment: past that point the rounded-box SDF stops growing with
  `width` and narrows into a lens instead (a true disc only exactly at `width == length`).
- `cargo run -p ludion --example ground_marker` animates a charge telegraph's fill by writing
  `length`, never `Transform.scale` — the shape field is the animated value, the scale is a whole-
  footprint multiplier that would shrink `width` right along with it.

## Next

- `ludion docs components` — declaring components, drift safety, and the built-in visual set.
- `ludion docs materials` — the three material scalars, and why `GroundMarker` doesn't carry them.
- `ludion docs post-processing` — the bloom chain a bright marker color feeds.
- `ludion docs drop-shadow` — a marker is a telegraph; `DropShadow` is the depth cue, and a marker draws on top of one.
- `ludion docs decals` — textured marks projected onto any surface; a marker draws on top of a decal.
