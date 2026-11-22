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
        lit: true            # shade each streak from the scene's lights
        medium: 1.0          # how much fog reaches the streaks, [0, 1]
        occlusion: true      # hide streaks under roofs; false keeps the pass but draws no casters
        wetness: 1.0         # how wet exposed surfaces look, [0, 1]; 0 leaves every surface dry
        ripples: 1.0         # rings on wet ground and coat decals, [0, 2]; 0 keeps the water still
        splashes: 1.0        # rings and droplets where rain lands on exposed tops, [0, 2]; 0 draws none
        shadowed: true       # a lit rain takes one shadow tap per light, so roofs and walls darken it
        backlight: 0.6       # forward-scatter glow when light travels toward the camera, [0, 1]; 0 is off
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

## Cost and budget
Per tile the draw holds `round(density * intensity * cell^3)` streaks, over the tiles meeting the radius and the height
band: `(ceil(2 * radius / cell) + 1)^2 * (ceil(span / cell) + 1)`. The default is 96 per tile over 486 tiles, 46,656
streaks. One draw carries at most **65,536**; past that the per-tile count is clamped and extract warns `L2382` once
per load. Streaks fade over the last 15 % of the radius and of the height band.

Where the time goes, on `rain-heavy` (every default, 24 casters, a shadowed spot, SSR, fog) at 1080p on the pinned RTX 3090:
the occlusion map is its own pass, `rain_occlusion`, 0.002 ms (0.01 ms over 500 voxel meshes); streaks and splashes draw
inside `transparents` (0.049 ms against 0.009 without rain); wetness and ripples are inside `scene` (0.253 ms against
0.179). The whole rain is about 0.25 ms of a 1.32 ms frame. There is no `rain` post group. The budget file is
`budgets/pinned-rtx3090-rain-heavy.toml` (`ludion profile --scene examples/gallery/rain-heavy.yaml --frames 120 --check
budgets/pinned-rtx3090-rain-heavy.toml`), gating `rain_occlusion`, `scene`, `transparents` and `gpu_frame_ms`. A static
cache of the occlusion map is not built: it waits for `rain_occlusion` above 0.15 ms (`Plan/rain-and-wet.md` D37). The
`transparents` time with and without rain is also measured with `cargo test -p ludion-renderer --release -- --ignored
--nocapture needs_gpu_bench_rain_gpu_ms`.

Drops stay world-anchored: `rain-camera-move.yaml` is rendered from two cameras 2 m apart at one tick, and a test checks
each drop lands where its fixed world position projects.

## Roofs
A `rain_occlusion` pass draws every shadow caster over the rain volume straight down into a 512x512 depth map around
the camera (`radius` either side, texel-snapped so a camera move under a texel leaves the map still, and reaching 64 m
above the band). Each streak fragment compares its height with the nearest caster over its column and fades out over
5 cm below it: no rain under an awning, a roof or a doorway lintel, with a crisp edge. What occludes is what casts a
shadow: a `Mesh` with `cast_shadows: false` lets rain through unless a `RainOccluder` says otherwise. The map is straight down, so wind-slanted rain still reaches the first metre
inside a wall's edge, and a roof more than 64 m over the band does not occlude. The pass runs only with a `Rain`
present: a rain-free scene keeps the identical frame graph and pixels. `occlusion: false` keeps the pass and draws no
casters, so nothing occludes. The pass costs 0.003 ms on `rain-awning` at 1080p on the RTX 3090 (a handful of
casters); it grows with the casters in the volume, like the sun's shadow pass.

## RainOccluder
`RainOccluder { occludes: bool }` (default `true`) decides whether an entity's opaque meshes write the map, whatever
they do for shadows. `RainOccluder: {}` makes a non-caster (`cast_shadows: false`) occlude; `occludes: false` exempts a
caster, such as a chain-link fence or a net canopy. The nearest `RainOccluder` on the entity or a `Parent` ancestor
wins, so a prefab is tagged once at its root; with none, a mesh occludes when it casts a shadow. Blended and viewmodel
meshes never occlude (`occludes: true` on a blended mesh is `L2383`), and a hidden entity draws nothing, so it occludes
nothing.

`ludion render --scene <scene> --debug-view rain_occlusion` tints every opaque mesh fragment inside the map: blue where
rain reaches it, orange where it is covered, untinted outside the map. It needs a `Rain` (the normal frame is drawn
without one) and is CLI-only.

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

## Ripples
**Ripples are on by default** (`ripples: 1`); `ripples: 0` turns them off and renders the frame as one without
them, byte for byte. Rain leaves expanding rings on standing water: the world's xz is tiled in 0.35 m cells, each
sending one ring per 0.7 s from a hashed origin, summed over the 3x3 neighbouring cells. The rings are a function of the
world position, the rain's game time and `seed` alone, so they never swim with the camera, a held clock holds them
and two renders at one tick are byte-identical. A ripple leans the shading normal, so it shows in the lit highlight
and the reflection, not as geometry.
- **Wet ground.** Every up-facing opaque mesh fragment ripples at weight `ripples x 0.3 x w`, where `w` is its
  wetness (`## Wetness`): ground under a roof is dry and never ripples.
- **`coat` decals.** A coat decal ripples at weight `Rain.ripples x Decal.ripples x exposure`. `Decal.ripples`
  (`[0, 2]`, default 1) scales the rain's ripples on that decal and `0` is still water; it is read for `coat`
  only, never for `alpha`. The exposure is read once per decal at the column of the box's top-face centre in the
  `rain_occlusion` map (1 outside the map or with no caster), so a puddle under an awning is still while its
  neighbour in the open rings. Keep `size.y` small, so the box top stays below the roof over it.


| Situation | What happens |
|---|---|
| A roof over the camera | No streaks under it, per pixel; `occlusion: false` draws through. A mesh with `cast_shadows: false` does not occlude unless it carries `RainOccluder: {}`; a net that should let rain through carries `occludes: false` |
| Light | A lit streak takes the lit-sprite rule at its centre; with `shadowed` the sun and each light that holds a shadow grant are tapped once there (`## Shadows and backlight`) |
| Fog | Streaks fog through the transparent layer; `medium: 0` keeps them out of the fog |
| Streaks overlapping | Blended premultiplied, unsorted within the draw |
| Probe captures | Drop the rain; secondary views keep it |
| Gameplay rain zones | Separate sim data, not the picture's rain |
| A character under a roof | Wet per pixel from the map, so it dries the instant it steps under cover |
| A `coat` decal | The authored puddle: wetness leaves decals and blended meshes (glass, holograms) untouched |
| A coat decal over wet ground | Both ripple: the ground at 0.3 and the decal at its own weight |
| A floor that is not a shadow caster | No splashes: it writes no map, so give it `RainOccluder: {}` |
| Ripples on `alpha` decals, blended meshes, walls | None: up-facing opaque meshes and `coat` decals only |
| Viewmodel | Never wetted |
| Time past 4096 s | Wraps; every drop jumps once |

## Splashes
**Splashes are on by default** (`splashes: 1`); `splashes: 0` draws none and renders the frame as one without them,
byte for byte. Where rain lands on an exposed top it leaves a ring that spreads and four droplets that arc off it. The
world's xz is tiled in 0.5 m cells over the camera's xz out to `min(radius, 12)` metres; each cell holds one hashed
splash per 0.16 s slot, present when a hash falls under `intensity x splashes x 0.5` (`1` is half the cells), so a
lower rate draws a subset of the same splashes. They are a function of the world position, the rain's game time and
`seed` alone: nothing swims with the camera, a held clock holds them and two renders at one tick are byte-identical.
- **They land on the `rain_occlusion` map's top** (`## Roofs`): the topmost caster over the splash's column, so a roof's
  top splashes and the floor under it never does. A surface that writes no map gets no splashes, and with
  `occlusion: false` the map is empty and none are drawn.
- **Flat tops only.** A top whose slope passes 45 degrees (one map texel of height over one texel of run) takes none.
- **Lit like the streaks** (shadow taps and backlight included), and faded out over the last 15 % of the reach.
- `--quality low` thins them with `rain_density`, a subset of the same splashes.

## Shadows and backlight
**Both are on by default** (`shadowed: true`, `backlight: 0.6`); `shadowed: false` with `backlight: 0` renders the frame
without them, byte for byte. A lit rain shades each streak (and each splash) once at its centre.
- **`shadowed`** multiplies the sun's term and each light's term by **one shadow tap** at that point: one compare tap in the
  sun's map (the cascade that covers the point, or the anchored map) and one in the local atlas for a light that holds a
  shadow grant (`cast_shadows: true`). No PCF or PCSS. A light without a shadow stays unshadowed, as does the sun
  without a shadow map. A floodlight's cone of rain stops at the hut wall that shadows it.
- **`backlight`** scales each term by `mix(1, hg, backlight)`, a Henyey-Greenstein lobe of the angle between the
  light's travel direction and the direction from the drop to the eye, with `g` the scene's `fog_anisotropy`
  (`PostProcess`, default 0.7): rain between a light and the camera glows, rain lit from the camera's side dims, and with
  `fog_anisotropy: 0` the lobe is 1 and `backlight` changes nothing.
- **Cost.** A rain with `shadowed` and `lit` makes `transparents` and `transparent_depth` read the shadow map and the
  local atlas, so a scene with shadow maps keeps them alive through those passes; a lit rain with `shadowed: false`
  keeps the old graph. `lit: false` ignores both fields.

## Diagnostics
- `L2380` (error): a field is non-finite or out of range, at its JSON Pointer.
- `L2381` (warning): more than one `Rain`; the lowest id is used.
- `L2382` (warning): the streak budget clamped the per-tile count.
- `L2383` (warning): a `RainOccluder` with `occludes: true` sits on a blended mesh, which never writes the map.

`ludion explain L2380` gives the fix. The gallery scenes are `examples/gallery/rain-open.yaml`, `rain-awning.yaml`,
`rain-wet.yaml` (wet voxel ground and figures, a dry awning), `rain-porosity.yaml`, `rain-occluder.yaml` (a net that lets
rain through beside a shadowless canopy that stops it), `rain-ripples.yaml` (rings on an open puddle and the ground,
a still one under a roof) and `rain-splashes.yaml` (splashes on open ground, a crate and a roof, none on a steep ramp or
under the roof), `rain-shadows.yaml` (a backlit floodlight cone whose rain stops at a hut wall's shadow),
`rain-camera-move.yaml` (sparse white drops, the world-anchoring proof) and `rain-heavy.yaml` (every default over casters, a
shadowed spot and SSR; the budget's calibration scene). The first five
author `ripples: 0.0`, all but `rain-splashes`, `rain-shadows` and `rain-heavy` author `splashes: 0.0`, and the eight earlier scenes (with `rain-camera-move`) author
`shadowed: false` and `backlight: 0.0`, so their goldens show the scene without them.

`--quality low` draws 0.4 of the streaks (`rain_density`, a prefix of the same drops); wetness and the map are unchanged.

