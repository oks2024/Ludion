---
description: GPU frustum culling of mesh views — what is culled, against which camera, how to observe it
---

# GPU frustum culling

**Scope:** what the engine drops, and which mesh level it draws, before drawing a mesh, so a game never culls or picks LODs by hand. Always on; a mesh chain is the only authoring.
The debug surfaces are `ludion docs debug-draw` (`cull`) and `ludion docs wire-render` (`cull_from`).

## What is culled
- **World meshes only**, projectile mesh looks included, in every per-frame mesh view: the camera scene pass,
  the AO prepass, spot shadows, the six point-light faces, sun cascades and the single-sun shadow window.
  Shadow views keep only meshes that cast shadows.
- **Not culled:** sprites, particles, billboard shots, decals, volleys, reflection-probe captures and the
  decal-receiver redraw. The viewmodel (`MeshInstance.viewmodel`) never enters the cull.

## Against which camera
The exact camera of each render: `late_latch` and `present_interpolation` re-renders cull against their own
camera, and `render.view` against the flown view (unless `cull_from` says otherwise). Extract never skips
anything by camera, so a newer camera cannot reveal a mesh that was dropped earlier.

## Bounds
Each mesh asset has a local box, transformed by the instance's model on the GPU. A skinned mesh uses the box
of its posed joint origins inflated by a per-mesh skin radius, so ragdolls on `SkinBone` parts stay visible.
Bounds are conservative: a mesh may be kept when off screen, never dropped when visible.

## Mesh LOD
A mesh asset with a `lods:` chain draws the coarser level its screen size selects, chosen in the same cull pass.
- **Asset.** Up to three levels, each a sibling mesh document that matches its base's draw key and joints
  (`ludion docs materials`, `L1395`/`L1396`). `ludion import --lod <screen>[:<ratio>]` writes one (`ludion docs
  blender-craft`); the gallery `meshes/lod/` is a three-level mannequin. Generated levels keep each corner's normal and uv from the first base vertex at its position (a base without normals stays flat).
- **Selection** is stateless and rotation-invariant: the fraction of viewport height the bounding sphere covers
  from the drawn camera's eye, against each `screen_size`, so turning never changes a level. Shadow views take the
  camera's eye, never their own frustum, so a shadow is never finer than its mesh.
  The sphere's radius is the half-diagonal of the instance's world box; for a skinned instance it is the half-diagonal of
  its asset's rest box times the model's largest axis scale, so it ignores the pose and an animating or ragdolled
  figure never flips a level (its centre is the posed box's, where it is drawn). Visibility still uses the posed box.
  Author a threshold as `screen_size = r·k/d` for the wanted switch distance `d` (`k` is the projection's `[1][1]`
  scale times `2^-lod_bias`, so a nonzero `Camera.lod_bias` or quality `lod_bias` scales it), reading `r` from
  `culling.lod_radii` `radius_median` in `ludion profile`.
- `Camera.lod_bias` (`-4..=4`, absent is `0`) adds octaves to the pick: each unit halves every switch distance, so `+1`
  goes coarser sooner. `Camera.shadow_bias_levels` (`0..=3`) makes shadow views draw that many levels coarser.
  Out of range is `L1397`. A free `render.view` camera takes the game camera's values.
- `lod_bias` in the machine quality (`ludion docs quality`: `0` at `full`/`high`, `0.5` `medium`, `1.0` `low`) adds to the
  camera's, so a quality only ever coarsens. Absent camera knobs leave `WorldHash` unchanged; authoring
  `Camera.lod_bias` or `shadow_bias_levels` moves it like any `Camera` field, and the quality's `lod_bias` is unhashed.
- **Level 0 always:** a mesh without a chain, views past the output cap, probe captures, the viewmodel, and every
  instance on an adapter without `INDIRECT_FIRST_INSTANCE` (the profile says so; `--debug-draw cull` then tints
  everything as level 0). The decal-receiver redraw follows the camera view's levels, so a decal's ids land on the
  level the scene pass drew.
- **`cull_from: game|frozen`** selects levels from the cull camera's eye, the intended debugging view: freeze the
  cull and fly away to see the levels the frozen camera picked.
- **Voxel models** chain too: `lods: [{screen_size}]` on the `kind: voxel` document (`ludion docs voxel-format`),
  up to three levels, each a 2× majority downsample generated at load. Selection, knobs and observing are the
  mesh chain's; sculpt and assembly parts are not chained. `examples/gallery/voxel-horde.yaml` draws four levels:
  `ludion render --scene examples/gallery/voxel-horde.yaml --sweep /entities/0/components/Camera/lod_bias=-1,0,1,2
  --out sheet.png`.
- **Compare:** `ludion render --scene examples/gallery/lod-hall.yaml --sweep /entities/0/components/Camera/lod_bias=-1,0,1,2
  --out sheet.png` (500 figures down a hall; a sweep does not apply `--quality`).

## Off switch
`Engine::set_frustum_culling(false)` — a host setting, not on the wire and not a quality axis. Shipping is
always on; tests and the culling bench use it to compare. With it off the `cull` pass is absent from
`--dump-graph`, unless the frame draws a LOD chain: the pass then stays with the frustum test skipped, so levels
and the image are unchanged.

## Static meshes
A mesh marked `Static` (`ludion docs components`) is placed once and kept in persistent GPU buffers. It is
re-placed only when it is written, despawned, loses `Static`, has an ancestor change or its asset reloads. It is
culled, LOD-selected, shadowed and received by decals and drop shadows exactly like a dynamic mesh, and draws
before the dynamics of its bucket, each in ascending `EntityId`. The image is byte-identical with the static scene
on or off: `Engine::set_static_scene(false)` is a host setting, not on the wire and not a quality axis; tests and
the bench use it. A static that is posed every frame (under a `ParentJoint` or `SkinBone` ancestor, or a part of
a prefab whose root carries `Animation`) is never held: it takes the dynamic path with `L1341`; the prefab case
shows only at runtime. Writing a static is legal, but one rewritten every frame wastes the cache (`L1343`). The
saving shows in the profile's `extract` timing.

## Observing it
- `ludion profile` JSON `culling`: per view (`camera`, `sun`, `cascade_N`, `local_N`) the median instances
  tested and drawn (`levels_drawn_median`: the drawn count per LOD level when the frame drew a chain), read back through a non-blocking ring that skips a frame rather than stall. Offline
  profiles only; absent with culling off, a live capture or a scene with no world mesh.
  `lod_radii`: per chained mesh asset the radius its instances size from (`radius_min`/`radius_median`/`radius_max`, metres, with `skinned`).
- `[passes] cull` in `--dump-timings` and the profile pass list.
- `--debug-draw cull`: each mesh's box in the rendered camera view, green kept, red culled (the CPU oracle); a kept mesh with a `lods:` chain is tinted by its level: green 0,
  yellow 1, orange 2, magenta 3.
- `render.view` `cull_from: view | game | frozen`: fly a free camera while the player's frustum culls;
  `frozen` holds the cull camera captured when the command is sent.

## Rules
- **Instance order is stable.** The compaction is a reduce-then-scan, never an atomic append; bindgen
  rejects atomics in the cull shader. Append order depends on thread timing, so the image would vary between runs.
- **Images are byte-identical with culling on or off**, and across repeat renders; a difference is a bug.
- **Output cap 128 MiB** (the WebGPU binding floor): views fill in order camera, sun, local, and a view past
  the cap draws uncull. `culling.over_cap_views` counts them; the image is the same, only the saving is lost.
  With every view over the cap (a scene too big for even the camera view), nothing is culled.
- **`--debug-draw cull` uses the tick's camera.** Under `late_latch`/`present_interpolation` an edge mesh can be
  marked differently from what the GPU, culling against the latched camera, keeps.
- **Cascade caster counts appear twice:** `sun_cascades.cascades[].casters` (CPU oracle) and
  `culling.views[cascade_N].drawn_median` (GPU). They agree except for rounding at a plane.

## Next
- `ludion docs debug-draw` — the `cull` category.
- `ludion docs wire-render` — `render.view` and `cull_from`.
- `ludion docs presentation` — late latch and interpolation re-renders.
