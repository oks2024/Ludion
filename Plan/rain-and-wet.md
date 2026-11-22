# Plan: Rain and wet weather

> Status: **W1a, W1b, W2a, W2b and W2c mechanized** (Neon Shroud request 0018): the `Rain` component, the GPU-evaluated streak field,
> roof occlusion, mesh wetness, authored porosity, `RainOccluder`, the `rain_occlusion` debug view, the `rain_density` quality lever and the `rain-open`, `rain-awning`, `rain-wet`, `rain-porosity` and `rain-occluder` goldens. Usage is `ludion docs weather`; the gallery scenes are
> `examples/gallery/rain-open.yaml`, `rain-awning.yaml`, `rain-wet.yaml`, `rain-porosity.yaml` and `rain-occluder.yaml`.

## Why

Neon Shroud's night city is rained on. The engine had no weather: `ParticleEmitter` is CPU-evaluated and
emitter-relative, so a rain field around a moving camera would swim and cost the sim thread thousands of particles.
Neon Shroud request 0018 asks for rain, then wet surfaces, as render-only systems. This plan is the design of record
for the whole arc; W1a ships the streak field.

## Decisions

1. **A separate primitive.** `Rain` is not a `ParticleEmitter`, which stays CPU-evaluated and emitter-relative. Rain is
   GPU-evaluated because the sim never reads it, so the reason behind `Plan/particles-engine-and-editor.md`
   decision 1 does not apply.
2. **Lattice.** World space is tiled in cubes of `cell` m. Drop `k` of tile `(i, j, l)` hashes `(seed, tile, k)` with
   the PCG round of `shaders/ground_marker.wgsl` (never `fract(sin())`) into a start phase `p`, a speed `s` in
   `fall_speed` and a gust factor `g` in `[1 - gust, 1 + gust]`. Then `y = (l + 1 - fract(p + t·s/cell))·cell` and
   `x = (i + fract(h.x + t·wind.x·g/cell))·cell`, `z` likewise. Each tile column wraps inside itself, so a drop's
   position never depends on the camera.
3. **Camera wrap.** The draw covers the tiles meeting `camera ± radius` (xz) and `camera.y + height` (y):
   `n_xz = ceil(2·radius/cell) + 1`, `n_y = ceil(span/cell) + 1`. One instanced draw of `n_xz²·n_y·per_tile` quads,
   decoded from `instance_index`. Drops fade over the last 15 % of the radius and of the height band.
4. **Budget.** `per_tile = round(density·intensity·cell³)`. Past `MAX_RAIN_STREAKS = 65,536` it is clamped to
   `floor(MAX / tiles)` and extract warns `L2382` once per load.
5. **Streak.** A quad along the world velocity `(wind.x·g, -s, wind.z·g)`; camera motion is never added. Length is
   `|v|·streak/60`. Width is `drop_width`, widened to at least one output pixel with alpha scaled by
   `drop_width / widened`. Premultiplied `(One, OneMinusSrcAlpha)`, depth-tested `LessEqual`, no depth write. Streaks
   are unsorted within the draw.
6. **Occlusion** (W1b): a 512² `Depth32Float` straight-down map over `camera.xz ± radius`, texel-snapped, with a 64 m
   reach above the band, drawn every frame with the shadow casters. Clipping is per fragment with a 5 cm fade.
7. **Time.** `RainData.time_s = (game_seconds mod 4096) as f32`, from the `GameClock`-read `now` that extract already
   has. It lives in `RainData`, not on `RenderScene`.
8. **Singleton.** Presentation or sim entity; the lowest `EntityId` wins and a second is `L2381`. On a presentation
   entity it is unhashed. Probe captures drop it; secondary views keep it; a blended frame takes `next`'s.
9. **Fields.** `intensity`, `density`, `radius`, `height`, `cell`, `fall_speed`, `wind`, `gust`, `drop_width`,
   `streak`, `color`, `lit`, `medium`, `occlusion`, `seed`, all serde-defaulted. A non-finite or out-of-range value is
   `L2380` at its JSON Pointer. `lit` takes the lit-particle rule (unshadowed light); `medium` scales the fog the
   transparent layer applies.
10. **The map moves ahead of `scene`** (W2a). `rain_occlusion` runs before the scene pass; its map is the last read of
    `scene` and stays the last read of `transparents` and `transparent_depth`. The map is built by direct caster draws,
    so it does not depend on the cull pass.
11. **Env group binding 11, mesh pipelines only.** `mesh.wgsl` declares `wet_map: texture_depth_2d` at group 3
    binding 11 (read with `textureLoad`) beside `surface_wetness`, and the shared `EnvParams` gains `wet_view` (the
    occlusion view) and `wet` (x wetness, y map flag, z the map's half extent, w reserved, 0). The lit decal pass
    shares `mesh_shading.wgsl` and already fills the fragment stage's 16 sampled textures, so it keeps the group-3
    layout without the map (`LayoutId::Env`); the mesh pipelines take `LayoutId::EnvWet`. A frame without rain and the
    blended draws bind a 1x1 depth fallback and write `wet = 0`, so the wet branch is never taken.
12. **One occluder-height function.** The map arithmetic of `rain_open_sky` moves into `snippets/rain_occlusion.wgsl`
    (`occluder_top` returns `(top, inside)`); the streaks and `mesh_shading.wgsl` share it, and the streak output is
    unchanged.
13. **Exposure** is `smoothstep(top - 0.2, top, world.y)` inside the map: a surface at its column's top is wet and a
    floor under a roof is dry. It blends to the fallback over the outer 10 % of the map's half extent, where the
    fallback is the `GiVolume` sky visibility when present, else 1. `VIEWMODEL` and `BLENDED` draws skip wetness.
    Facing is `clamp(0.5 + 0.5·n.y, 0, 1)` on the geometric normal (flipped for a back face).
14. **`wetness` defaults to 1.0**, the game's default, so `rain-open` and `rain-awning` are re-blessed once at the new
    default. Wet look: albedo `× mix(1, 0.6, w × porosity)`, roughness `mix(r, 0.08, w × (1 - metallic))`.
15. **Porosity fields** (W2b). `porosity` in `[0, 1]`, absent ⇒ `0.5`, on a voxel palette entry (which also covers sculpt
    and assembly palettes, the same struct), as a per-vertex float list `porosity` on a mesh document (a sibling of
    `roughness`), and as `Mesh.porosity: Option<f32>` for primitives and plain asset meshes. One selection rule, the one
    roughness follows: geometry that authors a material wins over the `Mesh` component, and porosity is part of
    `authored_material`, so a palette that authors only `porosity` takes the vertex material. A non-finite or
    out-of-range value is the format's existing material-scalar or material-list code at its JSON Pointer. No new code.
16. **GPU lanes, no new attribute.** All 22 vertex attributes are in use. Per vertex, `MeshVertex.material` widens
    `[f32; 3]` to `[f32; 4]`, w = porosity. Per instance, porosity rides `MeshInstanceRaw.medium[2]` (`.y` is the cull pass's
    scatter slot), a lane already padded to a `vec4`, so the cull pass's stride is unchanged. `VsOut.material` widens to `vec4` and interpolates like
    roughness; `vs_main` mixes porosity through the same `mix` as the rest of the material.
17. **Probe key.** Probe captures carry no rain, so porosity cannot change a capture: `probe_store` does not key it.
18. **`RainOccluder { occludes: bool }`** (default true), beside `Rain`. It decides whether an entity's opaque meshes write
    the `rain_occlusion` map: `{}` makes a non-caster occlude, `occludes: false` exempts a caster (a chain-link fence, a net
    canopy). Absent, the rule stays D6: what casts a shadow occludes. It is resolved at extract per placed mesh, the nearest
    `RainOccluder` on the entity or a `Parent` ancestor winning (the shape `Visibility.cast_shadows` has), into the CPU-only
    `MeshInstance.rain_occluder`. Blended and viewmodel instances never occlude; a hidden entity never reaches the map.
19. **`L2383` (warning).** `occludes: true` on an entity whose mesh is routed as blended is inert, because blended meshes are
    drawn outside `scene.meshes`; extract reports it once per entity through `degraded`.
20. **The `rain_occlusion` debug view** (CLI-only, no wire enum). `PostChain.rain_view` is set only with a `Rain` in the scene.
    It adds no pass and no pipeline: `EnvParams.wet.w` (reserved until now) is 1, and `mesh.wgsl` tints each opaque fragment
    inside the map by its column's exposure (the `surface_wetness` `under` term): blue exposed, orange covered, untinted outside.
    `surface_wetness` is untouched, so no existing golden moves.
21. **`rain_density`** is a `QualityParams` multiplier (full/high/medium 1.0, low 0.4) spent in the renderer: `build_rain_draw`
    thins `per_tile` before the layout, so drops `0..thinned` are the same drops as at full. Wetness, occlusion and the `L2382`
    budget are untouched. It is an override-file key, not in the wire-hashed `QualityProfile` (`particle_density`'s precedent).

## Waves

- **W1a** (shipped): the component, `L2380`–`L2382`, the GPU-evaluated field in the transparents pass and the
  transparent layer, `docs/weather.md`, the `rain-open` golden. A rain-free scene keeps the identical graph and bytes.
- **W1b** (shipped): the overhead occlusion pass, per-fragment roof clipping and the `rain-awning` golden.
- **W2a, mesh wetness:** per-pixel `w = wetness × exposure × facing` in `mesh.wgsl` from the occlusion map, with a
  fixed porosity of 0.5, so every voxel palette (16 and 64 cells/m, assemblies), sculpt, primitive and asset mesh gets
  wet without authoring. Characters are wet per pixel. `Rain.wetness` defaults to 1.
- **W2b, porosity:** the `porosity` field on palettes, sculpt, `Mesh` and mesh documents (D15–D17).
- **W2c:** `RainOccluder`, `--debug-view rain_occlusion` and a `rain_density` quality lever.
- **W3:** splashes, plus ripples on wet ground and on `coat` decals.
- **W4:** sun and local shadow taps, `backlight`, `ParticleEmitter.shadowed` and `.stretch`.
- **W5:** `rain-camera-move`, budget files, and a tile cache if the measured cost asks for one.

## Out of scope / deferred

- **Lens drops.** Trigger: a game asks for rain on the camera.
- **Snow mode.** Trigger: a game asks for snow; the lattice already carries the shape of it.
- **Gameplay rain zones.** Separate sim data, never read from the picture's rain.
- **Invisible rain-occluder volumes.** Trigger: a game asks.

## Owner rulings (2026-11-19)

- Accepted. Density in streaks/m³ (the request's unit was per-tile) and its defaults.
- Accepted. The 65,536 streak ceiling.
- Accepted. Unsorted streaks within the draw.
- Accepted. Time kept in `RainData` with a 4096 s wrap.
- Accepted. Unshadowed light until W4.
- Accepted. Codes `L2380`–`L2382` in place of the request's `L2130`–`L2134`, which are taken; the request's `L2134` dropped.
- Accepted. The presenter takes `next`'s rain.
- Accepted, superseded by D15. Porosity was fixed at 0.5 until W2b.
- Accepted. The wet constants 0.6 (albedo) and 0.08 (roughness).
- Accepted. A vertical-surface facing of 0.5.
- Accepted. The 20 cm exposure fade.
- Accepted. Characters wet per pixel: they dry instantly under cover.
- Accepted. Coat decals and blended meshes are not wetted.

## Status — request 0018

- **W1a mechanized (2026-11-18):** `Rain`, `RainData`, the `rain` shader and the `rain-open` gallery golden, blessed on
  the RTX 3090. Scenes without a `Rain` keep the same frame graph and pixels. No replay content hash, `WorldHash`,
  existing golden or protocol schema moved.
- **W1b mechanized (2026-11-18):** the conditional `rain_occlusion` pass (`PostChain.rain`; a 512² `Depth32Float` map
  over `camera.xz ± radius`, texel-snapped, 64 m reach), read as the last input of `transparents` and
  `transparent_depth`; the streak fragment fades out over 5 cm below the nearest shadow caster at its column;
  `occlusion: false` keeps the pass and issues no draws. The `rain-awning` golden is blessed on the RTX 3090; the pass
  costs 0.003 ms there at 1080p. Rain-free frames keep the same graph and pixels, and `rain-open` did not move. No replay
  content hash, `WorldHash`, existing golden or protocol schema moved.
- **W2a mechanized (2026-11-19):** `Rain.wetness` (default 1) wets every opaque mesh fragment in the scene pass:
  `w = wetness × exposure × facing`, porosity 0.5, from the `rain_occlusion` map, which now runs ahead of `scene` and is
  its last read. The mesh pipelines take `LayoutId::EnvWet` (group-3 binding 11); the lit decal pass keeps `Env`. A wet
  rain with no streaks still fills the map. `rain-wet` is new; `rain-open` and `rain-awning` were re-blessed at the
  new default after a `wetness: 0` render of both proved byte-equal to the W1b output. The scene pass costs 0.124 ms at
  wetness 0 and 0.135 ms at 1 on `rain-wet` at 1080p on the RTX 3090. Rain-free frames keep the same graph and pixels.
  No replay content hash, `WorldHash` or protocol schema moved.
- **W2b mechanized (2026-11-21):** `porosity` (`[0, 1]`, absent 0.5) on a voxel/sculpt/assembly palette entry, as a per-vertex
  `.mesh.yaml` list and as `Mesh.porosity`; it is part of `authored_material`, so a palette that authors only porosity takes
  the vertex material. `MeshVertex.material` is `[f32; 4]` (w = porosity) and the instance porosity rides
  `MeshInstanceRaw.medium[2]` (`.y` is the cull pass's scatter slot); no new attribute. `WET_POROSITY_DEFAULT` is gone.
  `rain-porosity` is new (stripes at 0, 0.5 and 1 darken in order, all gloss; a `Mesh.porosity: 0.1` cube); the other 110
  gallery goldens passed unblessed. No replay content hash, `WorldHash` or protocol schema moved. Owed: the macOS twin of
  `rain-porosity`. W2c (`RainOccluder`, `rain_occlusion` debug view, `rain_density`) is next.
- M4 (2026-11-22, Lore 2449 verified on the pinned Apple M4): the macOS twin of `rain-porosity` is blessed (r2455).
- **W2c mechanized (2026-11-22):** `RainOccluder { occludes }` (D18; resolved per placed mesh at extract, nearest on the entity or a `Parent` ancestor, into the CPU-only `MeshInstance.rain_occluder`), `L2383` (D19), `ludion render --debug-view rain_occlusion` (D20; `PostChain.rain_view`, `EnvParams.wet.w`, no pass, CLI-only) and `QualityParams.rain_density` (D21; `low` 0.4, a prefix of the same drops). `rain-occluder` and `rain-occluder-view` are new Windows goldens blessed on the RTX 3090; the other 124 gallery goldens passed unblessed. No replay content hash, `WorldHash` or protocol schema moved; the component schema snapshot gained `RainOccluder`. Owed: the macOS twins of both goldens.
