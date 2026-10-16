# Plan: SpotLight — a cone of light on the clustered-forward path

> Status: **W1–W3 mechanized (2026-10-15); Exit owed** — the look sitting (`Plan/owed.md`); the
> M4 twin passed 2026-10-15 at rev 1441. Current-code facts read from the tree on
> 2026-10-15 (file:line pointers are of that date; re-grep before editing). Parent:
> `Plan/clustered-forward.md` (its deferred `SpotLight` entry is this plan's trigger, now fired).

## Trigger

The reference game's request `0001-spot-light` (2026-10-15, priority *painful*): its art bible gives
every landmark a light no other place has, and the Wharves' landmark is a lighthouse whose **beam
sweeps across the quays**. A point light lights a sphere around the lamp, not a pool far from it. The
user ruled it is built in the engine now, not faked game-side. Later consumers the bible already
implies: a lantern over a door, a rose window throwing coloured light into a nave, light through a
breach in a curtain wall.

## What exists (2026-10-15)

- `PointLight { color, intensity, range }` (`crates/ludion-engine/src/components.rs:449`), position
  from the render pose; `DirectionalLight.direction` is an explicit world-space `[f32; 3]`, normalized
  at extract (`extract.rs` `build_sun`), a zero or non-finite one rejected as L1123.
- Extract walks entities in ascending `EntityId`; that order is the per-fragment summation order
  (`extract.rs` `extract_lights`), packed verbatim into `RenderScene.lights`, uncapped.
- The GPU `Light` is two `vec4`s (32 B; `color.w` unused), declared in `snippets/lights.wgsl`,
  `gi_bake.wgsl` and `gi_inject.wgsl` — bindgen unifies the three and pins the size
  (`crates/ludion-shader-bindgen/tests/real_shaders.rs`).
- Binning is CPU-side (`crates/ludion-renderer/src/cluster.rs`); `footprint_of` is the single
  per-light decision point, a range sphere fed to the shell and tile tests.
- Consumers of the light list: `mesh.wgsl` `shade` (meshes, voxels, assemblies), `sprite.wgsl`
  `shade_two_sided`, and the GI bake/inject (point lights only).

## Decisions

1. **The component.** `SpotLight { color, intensity, range, direction, inner_angle, outer_angle }`,
   position from the render pose. `direction` is an explicit **world-space** `[f32; 3]` like the sun's
   (agent-legible; a parent's rotation does not turn it), normalized at extract. Angles are
   **half-angles in degrees** from the axis. Defaults: white, intensity 1, range 10, direction
   `[0, -1, 0]`, inner 20°, outer 30°. Registered **last** (append-only registry), so every existing
   world hash holds.
2. **Validation.** Colour through `value_color_diagnostics` (L1263); `intensity`/`range` bounds as
   `PointLight`'s (schema, L1014). A new **L1272 `SPOT_LIGHT_INVALID_CONE`** at the offending pointer:
   a zero or non-finite `direction`, an angle outside `[0, 90)`, or `inner_angle > outer_angle`.
   `inner == outer` is legal (a hard edge).
3. **Falloff.** A spot is a point light times a cone factor: the same `atten²` range term, times
   `smoothstep(cos_outer, cos_inner, dot(-l, axis))`. The factor is an exact `0.0` outside the outer
   cone, so binning stays conservatism-only (an over-filed spot adds `+0.0`).
4. **One array, points then spots.** `RenderScene.spot_lights: Vec<SpotLightData>` beside `lights`;
   the packed list is every point light in extract order, then every spot in extract order. A scene
   with no spot packs and sums exactly as before. The GPU `Light` grows a third `vec4`
   (`axis.xyz`, `w` = `cos_outer`) and carries `cos_inner` in `color.w`; a point light writes
   `cos_outer = -2`, and the shader branches on it, so **point-light arithmetic is untouched** and
   every existing golden stays byte-identical. The three WGSL declarations grow together.
5. **Culling.** `footprint_of` bounds a spot by the cone's minimal bounding sphere (outer half-angle
   θ ≤ 45°: the sphere through the apex and the rim, centre `apex + axis·range/(2cosθ)`, radius
   `range/(2cosθ)`, widened 0.1%; wider: the range sphere)
   and feeds the same shell and tile tests. Tighter cone-vs-cell tests wait for occupancy stats that
   show bloat (`Plan/clustered-forward.md`'s refinement entry).
6. **No GI.** Spots feed neither the GI bake nor dynamic injection — `RenderScene.lights` stays the GI
   input and spots never enter it. `GiDynamic` stays PointLight-only. Trigger: a sitting judges a
   spot-lit interior wrong for lack of bounce.
7. **No shadows.** The lighthouse accepts a beam that crosses masts unshadowed. Spot shadows ride
   `Plan/clustered-forward.md`'s point/spot shadow entry.
8. **The visible shaft: the lit pool alone.** From the 58° game camera the beam reads by its pool on
   quays and walls; no additive cone mesh, no fog in-scatter now. Trigger: the lighthouse sitting
   judges the sweep unreadable — then the height-fog in-scatter (the medium already marches the sun)
   is the first candidate, an additive mesh the second.
9. **Import.** glTF `KHR_lights_punctual` spots map to `SpotLight` (radians → degrees, the node's
   `-Z` turned into a world `direction`) instead of the L1202 lossy refusal.

## Waves

- **W1 — the component (engine).** `SpotLight`, defaults, validator, L1272 (const, `CODES` row,
  `DECLARED_CODES`, the L1263 coverage text), registry append and its pinned-order test, prelude
  re-export, extract into `RenderScene.spot_lights`, unit tests mirroring `PointLight`'s.
- **W2 — the renderer.** `Light` to 48 B in all three declarations and the bindgen pins, packing
  (decision 4), the cone footprint (decision 5) with the never-misses conservatism test extended to
  spots, the cone factor in `mesh.wgsl` and `sprite.wgsl`, GPU tests: a spot lights its pool and
  nothing outside its cone; the culled build is byte-identical to the degenerate oracle with spots
  present; a point-only scene is unchanged.
- **W3 — content and clients.** `examples/gallery/spot-lights.yaml` + goldens on the pinned boxes,
  `ludion docs lighting` / `components` sections, the README gallery row, the editor palette
  archetype and Lighter listing, `model_sheet` rig components, glTF spot import (decision 9).
- **Exit.** The lighthouse kit lights its beam in the game; a human looks at the gallery scene and the
  sweep.

## Status

- W1–W2 (2026-10-15): `SpotLight` + L1272, extract into `RenderScene.spot_lights`, the 48 B
  `Light`, points-then-spots packing, the cone bounding sphere, the cone factor in `mesh.wgsl` and
  `sprite.wgsl`. Every existing golden byte-identical on the RTX 3090; the M4 confirmation is owed.
  Finding: branching around the accumulate moved the lit-sprite `tilemap` golden (the driver
  contracts the arms differently), so the factor multiplies into the colour first and is an exact
  1.0 for a point light (`dev-docs/renderer-clustering.md`).
- W3 (2026-10-15): `examples/gallery/spot-lights.yaml` + its Windows golden, `ludion docs lighting`
  §Spot lights and the `components` row, the editor's `spot` palette archetype and Lighter listing
  (`GiDynamic` stays PointLight-only), `model_sheet` rig lights, glTF `KHR_lights_punctual` spots →
  `SpotLight` (world direction composed up the node chain; a 90° outer cone clamps to 89.9°).
  Owed: the macOS golden and identity on the M4; the look sitting.
