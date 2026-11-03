# Plan: Transparent depth — sprites, particles and text at their own distance

> Status: **mechanized; human look owed** (`Plan/owed.md`). The macOS goldens for `fog-sprites`,
> `fog-sprites-ortho`, `dof-sprites` and `shafts-sprites` are blessed on the M4; their Windows twins
> and the Windows byte proof are owed.

## Trigger

Only opaque meshes write `scene_depth`, so fog, light shafts and the depth-of-field circle of
confusion treated a sprite, particle or world-text pixel as sitting at the opaque surface behind it,
or at the sky. A character was fogged by a distant hill, popped between fogged and clear at a
building's edge, had a seam across its body at the horizon, and blurred out when DoF focused on it.
The reference game's fogged sprite look drove it. Separately, fog and shafts fanned under an
orthographic camera.

## What exists

- `crates/ludion-renderer/src/graph.rs` `PostChain::transparent_depth_scheduled` is the one
  predicate for the pass; the graph and the recorder both call it.
- The `transparent_depth` pass and its `Rg16Float` target; readers in `composite.wgsl`,
  `dof_coc.wgsl` and `shafts.wgsl`. Pass notes: `ludion devdocs renderer-passes`.
- Gallery scenes `fog-sprites`, `fog-sprites-ortho`, `dof-sprites`, `shafts-sprites`.

## Decisions

1. **The design.** The `transparent_depth` pass redraws sprites, particles and world text after
   `transparents`, in the same order and with the same depth compare, into an `Rg16Float` target of
   `(t·w, w)`. `t` is the distance along the view ray (the depth along the view under an orthographic
   camera) and `w` is the weight the colour pass blended with. An additive texel claims
   `coverage · min(1, max(rgb))`. Readers take the mean distance `R/G` and the coverage `G`:
   - fog: `mix(F_background, F(t̄), G)`;
   - circle of confusion: `mix(coc(d_b), coc(t̄), G)`;
   - shafts: samples past `t̄` count for `1-G`, and the march stops at `t̄` at full coverage.

   Sky and ground markers do not draw into it: a marker lies on the ground, whose depth is already
   right.
2. **What it costs when absent.** The predicate is `transparent_cover && (fog || dof)` with no debug
   view, using the masked `dof`. Absent, there is no pass and no target, and composite binds a 1×1
   fallback. An uncovered pixel takes the same code as before.
3. **Alternatives that lost.**
   - *Per-fragment fog, composite skipping covered pixels:* composite cannot fog the `(1-a)`
     background share alone, so it needs the coverage target anyway; in-scatter written in `scene`
     reaches bloom; DoF would blur fog that was baked in.
   - *Precompensated fog `S' = (S·T_s + a·L·(T_b-T_s)) / T_b`:* exact, composite untouched, but
     `1/T_b` blows up in dense fog, and bloom and DoF read the amplified value.
   - *A separate transparent colour layer:* exact, but bloom and DoF need a merged image, so more
     paths and an 8-byte target.
   - *Writing the target from `transparents` as a second attachment:* no redraw, but twin pipelines
     for everything in that pass, so byte identity would hold by care rather than by construction.
   - *A coverage latch keeping the pass scheduled after the first transparent:* a flip measured about
     0.17 ms of CPU over steady, not worth hidden state that makes the live graph differ from the
     scene's.
4. **Unchanged by design.** Particles never interleave with sprites; world text draws with no depth
   test, in scene order. The target follows the colour pass in both, so fog and CoC agree with what
   was painted.

## Known errors

- **Soft texels.** In-scatter is exact; the transmitted colour is off by `a(1-a)(S-B)(F_s-F_b)`,
  zero at `a = 0` and `a = 1`, at most a quarter of `|S-B||F_s-F_b|`.
- **Additive fog hole.** The background under a bright additive texel takes the layer's fog, so over
  a fogged background a bright ember can read as a small fog hole with its tint shifted toward the
  unfogged background. In the gallery at 1080p it reads as a faint disc.
- **Stacked layers** share one mean distance.
- **Half-resolution rim.** A rim pixel shares a DoF prefilter block with partly covered texels
  (7 LSB measured at a rim).
- **Perspective seams.** Fog's seam test (`a_sprite_straddling_the_horizon_has_no_fog_seam`) runs
  under a perspective camera and pins a step of at most 1 LSB. The CoC and shaft seam tests use an
  orthographic camera and pin nothing for perspective: a flat sprite's off-axis points really are
  farther than focus (6–18 LSB of real CoC), and the shaft phase slopes about 1 LSB per row.
- **Fog after DoF.** Fog runs after the DoF blend using the sharp pixel's value, so a blurred
  sprite's spill past its coverage takes the background's fog. Opaque silhouettes behave the same.

## Cost

One pass and one 4-byte-per-sample target (8.3 MB at 1080p; it must store, not be memoryless).
`budget-horde` frame time on the M4: 8.44 ms without the pass, 8.58 ms with it, three runs each. A
flip costs about 0.17 ms of CPU median over steady. The first-use GPU cost of a flip's fresh textures
is unmeasured, and the RTX 3090 is unmeasured. There is no per-pass budget line: Metal's per-pass
figure is smeared, and `gpu_frame_ms` is the gate.

## A fragility

Byte identity for opaque frames depends on how the shaders are spelled, on Metal: a `select` around
`length` in `fog_factor`, and reading the layer above the march setup in `shafts.wgsl`, each moved
goldens by 1 LSB, so the code uses early returns and a fixed statement order. `layered_march`
duplicates the plain loop, and the `fs_depth` entries duplicate their `fs_main`, on purpose, for the
same reason. Removing any of them must rerun a byte comparison (`LUDION_GOLDEN_ACTUALS`,
`ludion devdocs testkit-golden`).

## Status

- r2069: the target and the pass; nothing reads it.
- r2070: fog reads it, and fog under an orthographic camera measures along the view.
- r2071: the circle of confusion reads it.
- r2072: shafts read it.
- Docs: `ludion docs atmosphere`, `post-processing`, `particles`, `components-ui`; `ludion devdocs
  renderer-passes`.
- macOS goldens blessed for `fog-sprites`, `fog-sprites-ortho`, `dof-sprites`, `shafts-sprites`.
  Mechanized; the human look and the Windows half are in `Plan/owed.md`.
