---
description: GPU frustum culling of mesh views — what is culled, against which camera, how to observe it
---

# GPU frustum culling

**Scope:** what the engine drops before drawing a mesh, so a game never culls by hand. Always on; nothing to author.
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

## Off switch
`Engine::set_frustum_culling(false)` — a host setting, not on the wire and not a quality axis. Shipping is
always on; tests and the culling bench use it to compare. With it off the `cull` pass is absent from
`--dump-graph`.

## Observing it
- `ludion profile` JSON `culling`: per view (`camera`, `sun`, `cascade_N`, `local_N`) the median instances
  tested and drawn, read back through a non-blocking ring that skips a frame rather than stall. Offline
  profiles only; absent with culling off, a live capture or a scene with no world mesh.
- `[passes] cull` in `--dump-timings` and the profile pass list.
- `--debug-draw cull`: each mesh's box in the rendered camera view, green kept, red culled (the CPU oracle).
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
