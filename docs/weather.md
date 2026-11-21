---
description: Rain: world-anchored streaks around the camera, and the wet surfaces under it
---

# Weather: `Rain`

**Scope:** `Rain` draws falling rain as streaks in a lattice anchored to the world, evaluated on the GPU from one
record. It is render-only: on a presentation entity it is never hashed, and a scene without it, or with
`intensity: 0`, keeps the identical frame graph and the identical pixels. Registering it is `ludion docs components`;
CPU-evaluated, emitter-relative effects are `ludion docs particles`.

## Format
```yaml
entities:
  - name: weather
    presentation: true
    components:
      Rain:
        intensity: 1.0       # [0, 1]: scales the drop count
        density: 1.5         # streaks per m^3 at intensity 1, in (0, 16]
        radius: 16.0         # reach around the camera in metres, [2, 64]
        height: [-6.0, 14.0] # the band relative to the camera, low < high, span <= 64
        cell: 4.0            # lattice cube size in metres, [1, 8]
        fall_speed: [7.0, 9.0]   # m/s, 0 < min <= max <= 40
        wind: [1.2, 0.0, 0.4]    # m/s; y must be 0, |x| and |z| <= 20
        gust: 0.15           # per-drop wind variation, [0, 1]
        drop_width: 0.006    # metres, (0, 0.05]; widened to one output pixel, alpha scaled to match
        streak: 1.0          # length = speed * streak / 60 metres, [0, 8]
        color: [0.55, 0.6, 0.65, 0.25]   # display colour and base opacity, each [0, 1]
        lit: true            # shade each streak from the scene's lights (unshadowed)
        medium: 1.0          # how much fog reaches the streaks, [0, 1]
        occlusion: true      # hide streaks under roofs; false keeps the pass but draws no casters
        wetness: 1.0         # how wet exposed surfaces look, [0, 1]; 0 leaves every surface dry
        seed: 0              # varies the lattice
```
Every field is optional. `Rain` is a singleton: the lowest entity id wins and each further one is `L2381`.

## Why nothing swims
World space is tiled in cubes of `cell` metres. Each drop hashes `(seed, tile, k)` into a start phase, a speed and a
gust factor, and its position is a function of the tile and the time alone: it falls and drifts, then wraps inside its
own tile column. The camera only picks which tiles are drawn, so moving the camera never moves a drop. The streak lies
along the drop's own velocity, never the camera's. `ludion_renderer::drop_position` is the CPU twin of the shader's
arithmetic, with a bit-identical hash.

## Time
The rain reads game time (`GameClock`): a game that holds the clock holds the rain, and a replay or a screenshot at a
tick shows the same drops. The time wraps at 4096 s to keep `f32` precision, so a drop jumps once every 68 minutes.

## Budget
Per tile the draw holds `round(density * intensity * cell^3)` streaks, over the tiles meeting the radius and the height
band: `(ceil(2 * radius / cell) + 1)^2 * (ceil(span / cell) + 1)`. The default is 96 per tile over 486 tiles, 46,656
streaks. One draw carries at most **65,536**; past that the per-tile count is clamped and extract warns `L2382` once
per load. Streaks fade over the last 15 % of the radius and of the height band. The `transparents` pass time with and
without rain is measured with `cargo test -p ludion-renderer --release -- --ignored --nocapture needs_gpu_bench_rain_gpu_ms`.

## Roofs
A `rain_occlusion` pass draws every shadow caster over the rain volume straight down into a 512x512 depth map around
the camera (`radius` either side, texel-snapped so a camera move under a texel leaves the map still, and reaching 64 m
above the band). Each streak fragment compares its height with the nearest caster over its column and fades out over
5 cm below it: no rain under an awning, a roof or a doorway lintel, with a crisp edge. What occludes is what casts a
shadow: a `Mesh` with `cast_shadows: false` lets rain through (`RainOccluder`, a component that occludes without
drawing, is in `Plan/rain-and-wet.md`). The map is straight down, so wind-slanted rain still reaches the first metre
inside a wall's edge, and a roof more than 64 m over the band does not occlude. The pass runs only with a `Rain`
present: a rain-free scene keeps the identical frame graph and pixels. `occlusion: false` keeps the pass and draws no
casters, so nothing occludes. The pass costs 0.003 ms on `rain-awning` at 1080p on the RTX 3090 (a handful of
casters); it grows with the casters in the volume, like the sun's shadow pass.

## Wetness
`wetness` (default 1) darkens and glosses every opaque mesh surface the rain reaches. Per fragment the scene pass
computes `w = wetness x exposure x facing` and applies it: the albedo is multiplied by `mix(1, 0.6, w x porosity)` and
the roughness goes to `mix(r, 0.08, w x (1 - metallic))`, so metal stays dry-looking and the screen-space and probe
reflections then show the wet ground with no extra wiring. Porosity is `[0, 1]` and 0.5 when nothing authors it, so a voxel palette
(world models, assemblies), a sculpt, a primitive and an asset mesh all get wet with nothing authored. Author it where
the material lives, with the same selection rule as `roughness` (`ludion docs materials`): geometry that authors any
material field wins, and `Mesh.porosity` is inert on it.

| Surface | Where |
| --- | --- |
| Voxel, assembly or sculpt model | `porosity` on a palette entry, beside `metallic` |
| `.mesh.yaml` asset | a per-vertex `porosity:` list, one float per vertex; authoring it alone selects the vertex material |
| Primitive or plain asset mesh | `Mesh.porosity`, not inherited down `Parent` |

`porosity: 0` keeps the albedo dry-looking while the roughness still drops; `1` darkens the most.

**Exposure** is read from the `rain_occlusion` map (`## Roofs`): a surface at its column's top is wet, and one more
than 20 cm below the nearest caster is dry, so the ground under an awning, the inside of a doorway and a figure
standing under a roof read dry. Past the map's edge (the outer 10 % of `radius` blends toward it) exposure is the
`GiVolume` sky visibility when the scene has one, else 1. **Facing** is `0.5 + 0.5 * n.y` on the geometric normal:
1 facing up, 0.5 on a wall, 0 facing down. Drying is a game-written `wetness`: write it from a presentation system
over time, the way a game writes `focus_distance`; `wetness: 0` renders the frame as a dry one.

## Gotchas
| Situation | What happens |
|---|---|
| A roof over the camera | No streaks under it, per pixel; `occlusion: false` draws through. A mesh with `cast_shadows: false` does not occlude |
| Light | A lit streak takes the lit-sprite rule at its centre, so the sun and local lights are unshadowed |
| Fog | Streaks fog through the transparent layer; `medium: 0` keeps them out of the fog |
| Streaks overlapping | Blended premultiplied, unsorted within the draw |
| Probe captures | Drop the rain; secondary views keep it |
| Gameplay rain zones | Separate sim data, not the picture's rain |
| A character under a roof | Wet per pixel from the map, so it dries the instant it steps under cover |
| A `coat` decal | The authored puddle: wetness leaves decals and blended meshes (glass, holograms) untouched |
| Viewmodel | Never wetted |
| Time past 4096 s | Wraps; every drop jumps once |

## Diagnostics
- `L2380` (error): a field is non-finite or out of range, at its JSON Pointer.
- `L2381` (warning): more than one `Rain`; the lowest id is used.
- `L2382` (warning): the streak budget clamped the per-tile count.

`ludion explain L2380` gives the fix. The gallery scenes are `examples/gallery/rain-open.yaml`, `rain-awning.yaml` and
`rain-wet.yaml` (wet voxel ground and figures, a dry awning).
