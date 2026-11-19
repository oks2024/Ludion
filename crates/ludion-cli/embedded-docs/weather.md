---
description: Rain — a world-anchored field of velocity-stretched streaks around the camera
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
        occlusion: true      # read by the roof occlusion pass, not yet drawn
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

## Gotchas
| Situation | What happens |
|---|---|
| A roof over the camera | Streaks fall through it: roof clipping is not drawn yet (`occlusion` is stored for it) |
| Light | A lit streak takes the lit-sprite rule at its centre, so the sun and local lights are unshadowed |
| Fog | Streaks fog through the transparent layer; `medium: 0` keeps them out of the fog |
| Streaks overlapping | Blended premultiplied, unsorted within the draw |
| Probe captures | Drop the rain; secondary views keep it |
| Gameplay rain zones, wetness | Separate sim data, not the picture's rain |
| Time past 4096 s | Wraps; every drop jumps once |

## Diagnostics
- `L2380` (error): a field is non-finite or out of range, at its JSON Pointer.
- `L2381` (warning): more than one `Rain`; the lowest id is used.
- `L2382` (warning): the streak budget clamped the per-tile count.

`ludion explain L2380` gives the fix. The gallery scene is `examples/gallery/rain-open.yaml`.
