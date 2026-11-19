# Plan: Rain and wet weather

> Status: **W1a, W1b and W2a mechanized** (Neon Shroud request 0018): the `Rain` component, the GPU-evaluated streak field,
> roof occlusion, mesh wetness and the `rain-open`, `rain-awning` and `rain-wet` goldens. Usage is `ludion docs weather`; the gallery scenes are
> `examples/gallery/rain-open.yaml`, `rain-awning.yaml` and `rain-wet.yaml`.

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

## Waves

- **W1a** (shipped): the component, `L2380`–`L2382`, the GPU-evaluated field in the transparents pass and the
  transparent layer, `docs/weather.md`, the `rain-open` golden. A rain-free scene keeps the identical graph and bytes.
- **W1b** (shipped): the overhead occlusion pass, per-fragment roof clipping and the `rain-awning` golden.
- **W2a, mesh wetness:** per-pixel `w = wetness × exposure × facing` in `mesh.wgsl` from the occlusion map, with a
  fixed porosity of 0.5, so every voxel palette (16 and 64 cells/m, assemblies), sculpt, primitive and asset mesh gets
  wet without authoring. Characters are wet per pixel. `Rain.wetness` defaults to 1.
- **W2b:** the `porosity` field on palettes, sculpt, `Mesh` and mesh documents; `RainOccluder`,
  `--debug-view rain_occlusion` and a `rain_density` quality lever.
- **W3:** splashes, plus ripples on wet ground and on `coat` decals.
- **W4:** sun and local shadow taps, `backlight`, `ParticleEmitter.shadowed` and `.stretch`.
- **W5:** `rain-camera-move`, budget files, and a tile cache if the measured cost asks for one.

## Out of scope / deferred

- **Lens drops.** Trigger: a game asks for rain on the camera.
- **Snow mode.** Trigger: a game asks for snow; the lattice already carries the shape of it.
- **Gameplay rain zones.** Separate sim data, never read from the picture's rain.

## Proposed — owner ruling owed

- Density in streaks/m³ (the request's unit was per-tile) and its defaults.
- The 65,536 streak ceiling.
- Unsorted streaks within the draw.
- Time kept in `RainData` with a 4096 s wrap.
- Unshadowed light until W4.
- Codes `L2380`–`L2382` in place of the request's `L2130`–`L2134`, which are taken; the request's `L2134` dropped.
- The presenter takes `next`'s rain.
- Porosity fixed at 0.5 until W2b.
- The wet constants 0.6 (albedo) and 0.08 (roughness).
- A vertical-surface facing of 0.5.
- The 20 cm exposure fade.
- Characters wet per pixel: they dry instantly under cover.
- Coat decals and blended meshes are not wetted.

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
