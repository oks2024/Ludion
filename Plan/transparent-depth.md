# Plan: Transparent depth — sprites, particles and text at their own distance

> Status: **mechanized; human look owed**. The goldens for `fog-sprites`,
> `fog-sprites-ortho`, `dof-sprites` and `shafts-sprites` are blessed on both pinned machines.

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
   `coverage · min(1, max(rgb))`, where `rgb` is the colour the colour pass adds, lit and with its
   flash. Readers take the mean distance `R/G` and the coverage `G`:
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

## Mesh path (Neon Shroud request 0016, Serious Isaac draft 0030)

An external game needs glass, holograms and ghost figures: meshes seen through. The trigger in
`Plan/engine-gap-map.md` (a committed asset that must be seen through) fired. Decisions continue the
numbering above.

5. **`Mesh.blend: alpha | additive`** on every mesh kind (primitive, asset, voxel, sculpt, assembly,
   character). A blended mesh is lit like an opaque one and drawn in `transparents` after the ground
   markers and before the sprites: back-to-front per entity, depth test on, depth write off. It joins
   `transparent_depth`, so fog, shafts and DoF see it at its own distance. It is not inherited down
   `Parent`; an assembly is one entity, so all its parts blend.
6. **Excluded by construction.** Extract puts a blended instance in `RenderScene::blended`, never in
   `RenderScene::meshes`, so the depth pre-pass, shadows, GI voxelization, the silhouette and
   probe captures never see it. A scene with no `blend` has the same frame graph and pixels, and its
   canonical text and `WorldHash` do not move.
7. **Sort.** One key per entity (all parts of an assembly share a group): the view distance to the
   centre of the group's world AABB, far first, ties by ascending group. Per-entity, not per-triangle:
   two interpenetrating panes do not sort exactly.
8. **Lighting.** Direct light, sun shadow and local shadows read as for an opaque mesh. Ambient
   occlusion is off for a blended mesh (AO is a screen-space read of opaque depth).
9. **Static, viewmodel.** A `Static` blended entity takes the dynamic path and reports `L1341`. A blend
   on a `Viewmodel` mesh is ignored with `L2360`; it draws opaque.
10. `alpha_mode: blend` on a mesh document draws the mesh on the blended path with mode `alpha`;
    `Mesh.blend` overrides it; there is no value that forces a blend document opaque.
11. Per instance: an assembly's parts each take their own document's mode, and share the entity's sort
    group.
12. glTF `BLEND` imports as `alpha_mode: blend`; it is no longer an `L1202`.
13. **Indirect specular on a lit blended mesh** is the sky's specular IBL plus the reflection probes (relit,
    emissive part, sky visibility), never SSR (ruling e). `fs_blend`/`fs_blend_layer` compute it inline with a
    binding-free twin of `reflections.wgsl` `reflection_fill`, because a blended mesh has no opaque surface
    targets for the `reflections` pass to read; the two fills must change together. The mesh pipeline already
    sits at the WebGPU floor of 16 sampled textures per fragment stage, so the reflection resources join group 3
    and `engine_limits` raises `max_sampled_textures_per_shader_stage` to 20 (Metal's floor is 31). Only the
    blended draw binds the frame's real probe resources; every other mesh draw binds fallbacks.
14. **`Mesh.lit: false`** (default `true`) draws a blended mesh unlit: albedo (instance, vertex and base map) +
    emissive + rim + flash, with no direct light, ambient, wrap or indirect specular. On a mesh that draws
    opaque it is ignored with `L2361`.
15. **`Mesh.blend_depth: none | nearest`** (default `none`, absent from the file). `nearest` makes only the
    entity's front surface blend, so a translucent figure shows no far limb through a near one. Per `nearest` entity
    (an assembly is one figure: every blended part, by the entity's own `blend_depth`), in draw order: end the
    `transparents` pass, copy `scene_depth` into a renderer-owned scratch depth, draw the group's instances
    depth-only into the scratch (`Less`, write on), then draw its colour against the scratch (`LessEqual`, read-only),
    then reopen the pass for what follows. The group never writes `scene_depth`, so sprites, particles and the
    composite's fog see the opaque depth and the ghost's own layer distance, not a changed depth buffer. One copy per
    group, so a nearer group is not clipped by an earlier one's front surface. `transparent_depth` breaks the same
    way into its layer, so fog, shafts and DoF see the front surface only. The graph declares `CopySrc` on
    `scene_depth` only when some blended mesh is `nearest`, so any other frame's graph is unchanged. `L2362` on a
    `nearest` mesh that draws opaque. Cost: a render-extent depth copy and two small passes per `nearest` entity,
    twice per frame with the layer; for a handful of figures. Limit: two `nearest` entities that interpenetrate do not
    sort exactly (each is placed whole, by its centre).

Waves:

- **W1:** `Mesh.blend`, the side list, the sort, the pipelines, `transparent_depth` integration, the
  `glass-and-holograms` gallery scene, docs.
- **W2a:** asset `alpha_mode: blend` (`L1110`/`L1111` extended; `Mesh.blend` overrides the document);
  glTF `BLEND` imports as blend (`L1202` narrowed, the lossy fallback goes). Data path only: no shader,
  pipeline or graph change.
- **W2b:** indirect specular (sky IBL and probes) on lit blended meshes; `Mesh.lit: false` with `L2361`.
  Design: the reflection resources join `mesh.wgsl`'s group 3 at new bindings, read only by `fs_blend`/
  `fs_blend_layer`, a binding-free twin of `reflections.wgsl` `reflection_fill` that leaves
  `reflections.wgsl` untouched (§A fragility); the mesh layout sits at the WebGPU floor of 16 sampled
  textures per stage, so `engine_limits` raises `max_sampled_textures_per_shader_stage` to 20 with its
  comment and test; `transparents`/`transparent_depth` read `probe_live` when blended and relit; an unlit
  `BLEND_UNLIT` twin; a parity test of an alpha-1 blended pane against its opaque twin.
- **W3:** `blend_depth: nearest` (decision 15), a per-entity depth pre-pass in which an assembly is one figure, and
  the `ghost-figure` gallery scene. The pre-pass must take the resolved per-instance mode (`Mesh.blend` or the document's
  `alpha_mode: blend`), not `Mesh.blend` alone.
- **Open, with trigger:** a merged distance sort with sprites and particles, if smoke behind office
  glass reads wrong.

**Owner rulings (2026-11-19):**

- Accepted. (a) The trigger firing (dock office glass after the water), and the mesh path in decision 1.
- Accepted. (b) Meshes draw before sprites and particles; decision 4's "unchanged by design" gains this ordering
  limit: a sprite behind glass is drawn over the glass's depth-off layer in sprite order, not by
  distance.
- Accepted. (c) `Fade` multiplies opacity on a blended mesh instead of dithering.
- Accepted. (d) A blended mesh never casts a shadow, whatever `Visibility.cast_shadows` says.
- Accepted. (e) A blended mesh is never an SSR origin; it gets probe and sky specular in W2.

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

Byte identity for opaque frames depends on how the shaders are spelled, and on every GPU compiler
tried, not only Metal. On Metal, a `select` around `length` in `fog_factor`, and reading the layer
above the march setup in `shafts.wgsl`, each moved goldens by 1 LSB, so the code uses early returns
and a fixed statement order. `layered_march` duplicates the plain loop, and the `fs_depth` entries
duplicate their `fs_main`, on purpose, for the same reason. Removing any of them must rerun a byte
comparison (`LUDION_GOLDEN_ACTUALS`, `ludion devdocs testkit-golden`).

The care does not reach the RTX 3090. r2072 moved three pixels of `night-street` by 1 level there,
under both Vulkan and Dx12, on a scene that never enters `layered_march`: that branch's own
`world_from_depth(source_uv, 1.0)` is what moves them. The same call with depth 0.5 leaves the frame
identical, and making the branch return a constant does too, so the move comes from compilation and
not from a computed value. The driver's exact transformation was not observed. Spelling cannot pin
this: any edit to `fs_main` can move any scene with shafts, so a byte move here is judged and
re-blessed. Passing `ray` into `layered_march` restored the old bytes on this box, but it was not
tried on Metal, in the orthographic shaft tests, or on the reference game, and was not taken.

## Status

- r2069: the target and the pass; nothing reads it.
- r2070: fog reads it, and fog under an orthographic camera measures along the view.
- r2071: the circle of confusion reads it.
- r2072: shafts read it.
- After r2073: a lit or flashing additive claims from the colour it adds, not its unlit brightness.
- Docs: `ludion docs atmosphere`, `post-processing`, `particles`, `components-ui`; `ludion devdocs
  renderer-passes`.
- macOS goldens blessed for `fog-sprites`, `fog-sprites-ortho`, `dof-sprites`, `shafts-sprites`.
- Windows: the same four first-blessed on the RTX 3090, and `night-street` re-blessed for the three
  pixels above. Mechanized; the human look and the Windows cost are in the owed ledger.
- Mesh path (0016) W1 mechanized (r2386, 2026-11-18): `Mesh.blend` (`alpha` or `additive`) draws in the
  `transparents` pass and joins `transparent_depth`; the `glass-and-holograms` gallery golden is blessed on the
  RTX 3090. Opaque scenes keep the same pixels, and no replay content hash, `WorldHash`, existing golden or
  protocol schema moved. W2a shipped (r2440): asset and glTF `blend`. W2b shipped (r2442): indirect specular on lit blends, `lit: false`. W3 in progress: `blend_depth: nearest` with `L2362`, the pass break and scratch depth copy, the `ghost-figure` gallery scene.
