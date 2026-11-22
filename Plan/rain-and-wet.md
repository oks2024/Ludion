# Plan: Rain and wet weather

> Status: **W1a-W5 mechanized (W4a shadowed, backlit rain; W4b shadowed, stretched particles; W5 the `rain-camera-move` proof, the `rain-heavy` budget and the deferred tile cache)** (Neon Shroud request 0018): the `Rain` component, the GPU-evaluated streak field,
> roof occlusion, mesh wetness, authored porosity, `RainOccluder`, the `rain_occlusion` debug view, the `rain_density` quality lever, ripples on wet ground and coat decals, splashes on exposed tops (`rain-splashes`), shadowed and backlit rain (`rain-shadows`), and the `rain-open`, `rain-awning`, `rain-wet`, `rain-porosity` and `rain-occluder` goldens. Usage is `ludion docs weather`; the gallery scenes are
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
22. **The ripple field** (W3a; `snippets/rain_ripple.wgsl`, CPU twin `ludion_renderer::ripple_slope`, tests only). `ripple_slope(xz, time, seed)`
    returns a height gradient. World xz is tiled in cells of 0.35 m; each cell carries one ring per 0.7 s period, with a phase and
    an origin inside the cell from a PCG hash of `(seed ^ 0x9e3779b9, i, j)`. Age `a = fract(time / 0.7 + phase)`, radius
    `a x 0.16 m`, profile `sin((d - r) x 2pi / 0.05)` inside `|d - r| < 0.03`, amplitude `(1 - a)^2`; the 3x3 neighbouring cells sum.
    Stateless: a function of `(xz, RainData.time_s, seed)`, so a held `GameClock` holds it. Never `fract(sin())`. The shading normal
    is `normalize(n + weight x up x (-s.x, 0, -s.y))`, `up = smoothstep(0.7, 0.95, n.y)` on the geometric normal.
23. **Uniform lanes.** `EnvParams.ripple`: x = `time_s`, y = the seed bitcast, z = ground weight (`ripples x 0.3`), w = decal base
    weight (`ripples`). A frame without rain writes zero; both shaders skip the ripple at weight <= 0. `Rain.ripples` in `[0, 2]`,
    default 1. Ground weight is `ripples x 0.3 x w` (`w` = `surface_wetness`, so covered ground never ripples). The decal env
    bind group now carries the frame's `wet_view` and `ripple`.
24. **Decal exposure.** `Decal.ripples` in `[0, 2]` (default 1; coat decals only; 0 is still water). `vs_lit` reads the
    `rain_occlusion` map (`@group(4) @binding(12)`, vertex stage only) at the column of the box's top-face centre; exposure is
    `smoothstep(column - 0.2, column, top.y)`, 1 outside the map. The weight `env.ripple.w x Decal.ripples x exposure` rides
    `LitVsOut` flat; `ProjectorDecal.surface.w` carries `Decal.ripples` for coat decals (0 otherwise). `decals_lit` reads
    `rain_occlusion` as its last input only with rain. The five rain scenes of W1-W2c author `ripples: 0.0` so their goldens
    stay still.
25. **The splash field** (W3b; `rain.wgsl`, CPU twin `ludion_renderer::splash_at`, tests only). A world-anchored lattice of
    `SPLASH_CELL` = 0.5 m cells over `camera.xz +- min(radius, SPLASH_REACH = 12)`. Cell `(i, j)`: `h = pcg(pcg(pcg(seed ^ 0x85ebca6b) ^ i) ^ j)`,
    `phase = unit_hash(pcg(h + 1))`, `u = time / SPLASH_SLOT (0.16 s) + phase`, `slot = floor(u)`, `age = fract(u)`,
    `hs = pcg(h ^ slot)`. The cell holds a splash iff `unit_hash(pcg(hs + 1)) < p`, `p = clamp(intensity x splashes x SPLASH_RATE (0.5) x
    rain_density, 0, 1)`, so a lower `p` draws a subset of the same splashes. Origin inside the cell from `hs + 2`/`hs + 3`, droplet
    spin from `hs + 4`. Height comes from the `rain_occlusion` map in the vertex stage (the topmost caster, roofs included, never
    under them); none when the column or its two one-texel neighbours are off the map or `NO_OCCLUDER`, or a neighbour differs by
    more than one texel (45 degrees). A splash is 5 quads: a horizontal ring (half-size 0.03 to 0.12 m, alpha `a x 1.5 x (1 - age)^2`)
    and 4 ballistic droplets (`v0` = 1.2 m/s, horizontal speed 0.45 to 0.75, built like a streak, alpha `a x 2 x (1 - age)`). The
    horizontal fade is `1 - smoothstep(0.85 R, R, |o - cam.xz|)`; a lit rain shades the splash once at its top as the streaks are.
    Stateless in game time. The request's ring-only splash on puddle decals is Proposed, owner ruling owed.
26. **Draw and lanes.** One pipeline: the same `vs_main`/`vs_depth`/`fs_main`/`fs_depth`, bind group and graph reads. The vertex stages
    dispatch on the instance index (`ii >= splash_grid.w` decodes splash part `ii - splash_grid.w`); `record_rain` issues a second
    draw after the streaks only when splash instances > 0. `RainParams` gains `splash_grid` (base cell, cells per side, first splash
    instance) and `splash` (x = `p`) after `occ_view`. `RainVert`/`VsOut`/`DepthOut` gain a flat `ring` (0 for streaks and droplets).
    The map is read in the vertex stage, so `RAIN_OCCLUSION_BINDING` reflects as `VERTEX_FRAGMENT`. No new `ShaderId` or L-code.
27. **Data and quality.** `RainData.splash_rate` = `intensity x splashes`. `splash_layout(rain, cam, density)` gives `n = ceil(2R / CELL) + 1`,
    `base = floor((cam.xz - R) / CELL)`, `instances = n^2 x 5` (0 when `p` is 0). `rain_density` multiplies `p`; wetness, ripples and
    streaks are untouched. `build_rain_draw` returns `None` only when streaks, splashes and wetness are all zero. With
    `occlusion: false` the map is empty, so no splashes draw. The six earlier rain scenes author `splashes: 0.0`.
28. **Shadow bindings snippet** (W4a). The `Sun` struct, the `@group(2)` bindings 0-5, `LocalView`, `UNSHADOWED`,
    `NORMAL_OFFSET_TEXELS` and `MAX_PENUMBRA_TEXELS` move verbatim from `mesh_shading.wgsl` into
    `snippets/shadow_bindings.wgsl`; the PCF, PCSS and cascade functions stay. Every composition listing `mesh_shading.wgsl` gains
    `shadow_bindings.wgsl` immediately before it, in both registries (`render.rs` and the bindgen `COMPOSITIONS`); the composed
    text moves only by the relocation.
29. **Rain at group 3.** `rain.wgsl` moves `rain`/`rain_occlusion` from `@group(2)` to `@group(3)` and binds the sun group at 2.
    The `Rain` shader composes `shadow_bindings.wgsl` and `billboard_shadowed.wgsl`; its layouts are `[Camera, Lights, Sun, Rain]`.
    The sun group's bindings reflect as `VERTEX_FRAGMENT`, since rain samples them at the vertex stage.
30. **One tap** (`snippets/billboard_shadowed.wgsl`). `sun_tap(p)` takes one compare tap in the anchored map or the first
    covering cascade (no seam band; 1 past the last cascade or outside the box); `local_tap(index, p, l, dist)` is
    `local_shadow`'s view pick with `n = l`, one tap clamped in its tile, `UNSHADOWED` giving an exact 1; `hg4pi(c, g) =
    (1 - g^2) / (1 + g^2 - 2gc)^1.5`; `shade_billboard_shadowed` copies `shade_two_sided`'s loop and multiplies each light's term by
    `select(1, tap, shadowed) x mix(1, hg4pi(dot(travel, v), g), backlight)`, the sun's likewise with `sun.direction`. Unshadowed
    lights stay unshadowed. `shade_two_sided` is untouched.
31. **Lanes, graph and binding.** `Rain.shadowed` (default true) and `Rain.backlight` (`[0, 1]`, default 0.6, `L2380`). `RainParams`
    gains `shade` after `splash` (x shadowed, y backlight, z `PostProcess.fog_anisotropy`); `rain.wgsl` shades through
    `shade_billboard_shadowed` only when `shade.x > 0.5 || shade.y > 0`, else the old `shade_two_sided`, so `shadowed: false` with
    `backlight: 0` keeps the W3b bytes. `PostChain.rain_shadowed = rain.lit && rain.shadowed` opens the lighting-tail reads in
    `transparents`/`transparent_depth`; `record_rain` binds the sun group through the same `sun_group` factor `shading_groups` uses.
    The seven earlier rain scenes author `shadowed: false` and `backlight: 0.0`.
32. **Particle velocity** (W4b). `Segment::velocity_at` and `Motion::velocity_at` in `particles.rs` walk the phases as
    `offset_at` does and return the closed-form derivative: ballistic `velocity x (1 - k x drift) + acceleration x drift` (with
    drag; `velocity + acceleration x t` without), ramp `heading x lerp(from, to, t / duration)`, turn
    `speed x (heading x cos(rate t) + normal.cross(heading) x sin(rate t))` through `ludion_core::math`. A motion past its last
    finite phase is still (zero); `t <= 0` gives the first phase's start velocity. `EmitterContext::particle` calls it only
    when `stretch > 0` and never reorders the RNG draws. Render-only, deterministic.
33. **Particle lanes.** `ParticleEmitter.shadowed` (default false) and `.stretch` (`>= 0`, default 0) skip serialization at
    their defaults, so text, snapshots and `WorldHash` do not move. `ParticleInstance` gains `shadowed` (`lit && shadowed`, false
    for viewmodel particles) and `streak` (`velocity x stretch / 60`, zero for a disc); `ParticleInstanceRaw` gains
    `motion: [f32; 4]` (xyz streak, w shadowed) at `@location(6)`. A non-finite `stretch` is the existing `L2107`.
34. **Particle shader and binding.** `particle.wgsl` composes `shadow_bindings.wgsl` and `billboard_shadowed.wgsl`; its layouts
    are `[Camera, Lights, Sun]`. A shadowed lit particle shades through `shade_billboard_shadowed` with `backlight = 0`; a
    streaked one is a capsule along `axis = normalize(streak)` of core length `|streak|` and the disc's width, with the
    softness falloff around both ends (a flat `core` varying; 0 is the old disc path). `PostChain.particles_shadowed` joins
    `blended` and `rain_shadowed` in opening the lighting-tail reads; one `transparent_sun_group` helper builds the sun group
    for rain and particles, binding fallbacks when the chain declared no tail.
35. **Camera-move proof** (W5). `needs_gpu_drops_stay_world_anchored_when_the_camera_moves` (`ludion-engine/tests/rain.rs`)
    extracts `rain-camera-move` at one tick from two cameras 2 m apart, checks the two `RainData` are equal, renders both, and
    checks the CPU twin's (`rain_layout`, `drop_position`) projection of each fixed world drop against the GPU pixels,
    with a swim control: a pixel lit in A and not predicted in B stays dark in B.
36. **The calibration scene** (W5). `rain-heavy` keeps every `Rain` default over a yard with casters, a shadowed spot, a
    puddle decal, SSR and fog. The gated keys are `[passes] rain_occlusion`, `scene` and `transparents`, plus
    `gpu_frame_ms` (`budgets/pinned-rtx3090-rain-heavy.toml`). There is no `rain` post group: streaks and splashes share
    `transparents`.
37. **The static occlusion-tile cache is deferred** (W5). Measured 2026-11-22 on the RTX 3090 at 1080p, 120-frame release runs:
    `rain_occlusion` 0.002 ms on `rain-heavy` (four runs, ~35 casters) and 0.0102 / 0.0092 ms on `voxel-horde` with a default
    `Rain` (~500 voxel meshes), against the 0.15 ms gate (about a fifth of the request's 0.8 ms rain target). The whole rain
    costs ~0.25 ms of a 1.32 ms frame. Trigger: a `rain_occlusion` median above 0.15 ms at 1080p on the pinned RTX 3090,
    on a calibration scene or a game's scene.

## Waves

- **W1a** (shipped): the component, `L2380`–`L2382`, the GPU-evaluated field in the transparents pass and the
  transparent layer, `docs/weather.md`, the `rain-open` golden. A rain-free scene keeps the identical graph and bytes.
- **W1b** (shipped): the overhead occlusion pass, per-fragment roof clipping and the `rain-awning` golden.
- **W2a, mesh wetness** (shipped): per-pixel `w = wetness × exposure × facing` in `mesh.wgsl` from the occlusion map, with a
  fixed porosity of 0.5, so every voxel palette (16 and 64 cells/m, assemblies), sculpt, primitive and asset mesh gets
  wet without authoring. Characters are wet per pixel. `Rain.wetness` defaults to 1.
- **W2b, porosity** (shipped): the `porosity` field on palettes, sculpt, `Mesh` and mesh documents (D15–D17).
- **W2c** (shipped): `RainOccluder`, `--debug-view rain_occlusion` and a `rain_density` quality lever.
- **W3a, ripples** (shipped): `Rain.ripples` and `Decal.ripples`; a shading-normal perturbation on wet ground and on `coat` decals (D22-D24).
- **W3b, splashes** (shipped): the instanced splash draw on exposed tops (D25-D27).
- **W4a, rain lighting** (shipped): sun and local shadow taps and `backlight` on the rain (D28-D31).
- **W4b, particles** (shipped, D32-D34): `ParticleEmitter.shadowed` through `shade_billboard_shadowed`, and `.stretch`, a quad along the
  screen-projected velocity of length `stretch x |v| / 60` (default 0 is byte-identical).
- **W5** (shipped, D35-D37): `rain-camera-move`, the `rain-heavy` budget, and the tile-cache measurement (deferred).

## Out of scope / deferred

- **Lens drops.** Trigger: a game asks for rain on the camera.
- **Snow mode.** Trigger: a game asks for snow; the lattice already carries the shape of it.
- **Gameplay rain zones.** Separate sim data, never read from the picture's rain.
- **Invisible rain-occluder volumes.** Trigger: a game asks.
- **The static occlusion-tile cache** (D37). Trigger: `rain_occlusion` median above 0.15 ms at 1080p on the pinned RTX 3090, on a calibration scene or a game's scene.

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
- **W3a mechanized (2026-11-22):** `Rain.ripples` and `Decal.ripples` (both `[0, 2]`, default 1; the decal's is read for `coat` only), the stateless ripple field `rain_ripple.wgsl` with its CPU twin `ripple_slope` (D22), `EnvParams.ripple` (D23) and the decal exposure read at the box top from the `rain_occlusion` map through `decal_lit.wgsl`'s `@group(4) @binding(12)`, which `decals_lit` reads last only with rain (D24). A frame without rain, and `ripples: 0`, render the W2c bytes: the five earlier rain scenes now author `ripples: 0.0`, and every one of them and `decals` re-rendered byte-equal to its pre-edit PNG. `rain-ripples` is the new Windows golden blessed on the RTX 3090; nothing else was re-blessed. No replay content hash, `WorldHash` or protocol schema moved; the component schema snapshot gained the two fields. Owed: the macOS twin of `rain-ripples`, the editor TS types. W3b (splashes, D25) is next.
- **W3b mechanized (2026-11-22):** `Rain.splashes` (`[0, 2]`, default 1; no new L-code), the stateless splash field in `rain.wgsl` with its CPU twin `splash_at` and `splash_layout` (D25), a second instanced draw after the streaks through the same pipeline and bind group with `RainParams.splash_grid`/`splash` and a flat `ring` varying (D26), and `RainData.splash_rate` with `rain_density` thinning (D27). Splashes land on the `rain_occlusion` map's top: roofs splash, floor under them and tops steeper than 45 degrees do not, and `occlusion: false` draws none. The six earlier rain scenes author `splashes: 0.0` and passed unblessed; the new `rain-splashes` golden is blessed on the RTX 3090. Nothing existing was re-blessed; no replay, `WorldHash` or protocol schema moved; the component schema snapshot moved. Owed: the macOS twin of `rain-splashes` (M4) and the `Rain.splashes` TS type. The ring-only splash on puddle decals stays Proposed, owner ruling owed.
- **W4a mechanized (2026-11-22):** `Rain.shadowed` (default true) and `Rain.backlight` (`[0, 1]`, default 0.6; no new L-code). The shadow bindings moved verbatim out of `mesh_shading.wgsl` into `shadow_bindings.wgsl` (D28; the whole gallery re-rendered byte-identical before anything else changed). The rain moved to group 3 and binds the sun group at 2 (D29); `billboard_shadowed.wgsl` holds `sun_tap`, `local_tap`, `hg4pi` and `shade_billboard_shadowed` (D30); `RainParams.shade`, `RainData.shadowed`/`backlight`, `PostChain.rain_shadowed` and the `sun_group` factor `record_rain` shares with `shading_groups` (D31). `shadowed: false` with `backlight: 0` takes the old `shade_two_sided` branch and graph: `rain-open` and `rain-splashes` renders were byte-equal before and after. The seven earlier rain scenes author both off and passed unblessed; the new `rain-shadows` golden is blessed on the RTX 3090. Nothing existing was re-blessed; no replay, `WorldHash` or protocol schema moved; the component schema snapshot moved. Owed: the macOS twin of `rain-shadows` (M4), the `Rain.shadowed`/`backlight` TS types, and the probe-store code key, which moves with every mesh shader's composition. W4b (particles) is next.
- **W4b mechanized (2026-11-22):** `ParticleEmitter.shadowed` and `.stretch` ship (D32-D34): `velocity_at` in `particles.rs`, the `streak`/`shadowed` instance lanes, `particle.wgsl` on the sun group, `PostChain.particles_shadowed`, one `transparent_sun_group` helper shared with the rain. An unauthored emitter keeps its bytes (the 18 guard renders of the particle scenes at ticks 0 and 12 are byte-equal; no replay, `WorldHash` or protocol schema moved); the disc distance is a `select`, since an `if` there moved three pixels of `particles-stress` by one level. New golden `particles-shadowed` (Windows blessed on the RTX 3090; macOS twin owed). Docs: `docs/particles.md`.
- **W5 mechanized (2026-11-22):** the `rain-camera-move` golden and its GPU test `needs_gpu_drops_stay_world_anchored_when_the_camera_moves` (D35; density 0.15 where the plan guessed 0.05, since 0.05 kept only 3 drops in frame; with 10 per tile it keeps enough in both frames and the swim control checks 6 pixels), the `rain-heavy` golden (D36), both blessed on the RTX 3090, `rain_occlusion` in `KNOWN_PASS_NAMES` (66 to 67), and `budgets/pinned-rtx3090-rain-heavy.toml` (`rain_occlusion` 0.002 flat 0.05, `scene` 0.2529 to 0.51, `transparents` 0.0492 to 0.10, `gpu_frame_ms` 1.3222 to 2.65; `extract` ~0.027 ms left ungated). The rain's share from `intensity: 0` runs: `scene` +0.074, `transparents` +0.040, frame +0.25 ms against the request's 0.8 ms. D37: the tile cache is deferred (`rain_occlusion` 0.002 ms on `rain-heavy`, 0.0097 ms over 500 voxel meshes, against the 0.15 ms gate). The private TeamCity Windows budget step is wired (config paused). Nothing existing was re-blessed; no replay, `WorldHash`, component or protocol schema moved. Owed: the macOS twins of both goldens and of the budget file (M4).
