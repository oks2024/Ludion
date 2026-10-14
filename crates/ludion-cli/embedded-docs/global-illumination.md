---
description: The opt-in GiVolume sky-visibility and one-bounce volume — fields, calibration, dump tools
---

# Global illumination

**Scope:** the opt-in `GiVolume` singleton — a baked world-space volume that attenuates indirect
sky light and adds one bounce of surface-reflected light — its five fields and how to choose them,
the `GiDynamic` marker that exempts a moving light or mesh from the bake, the voxel-authoring safety
condition `GiVolume` inherits, and the `--dump-voxels`/`--dump-gi` instruments for seeing what it
built. The sky it multiplies against is `ludion docs environment`; the surfaces it lights are
`ludion docs materials`; voxel models are `ludion docs voxel-format`.

`GiVolume` voxelizes the scene's own placed geometry (load-time work, cached) and bakes a coarse
probe lattice over it: how much of the sky each probe can see, roughly which direction the
remaining light comes from, and — when `bounce` is authored above `0` — how much light the scene's
own emissive surfaces, static point lights and sun send back into the volume after one bounce.
`mesh.wgsl` multiplies its ambient/IBL term by the reconstructed sky-visibility value, then **adds**
the reconstructed bounce term on top. **Indirect only, at every setting** — direct light (the sun,
point lights) and emissive surfaces read exactly as they would with no `GiVolume` in the scene;
bounce only ever adds what already-visible light does after it hits a surface once. A torch that is
merely `emissive` still lights only itself until `bounce` turns its glow into light on the wall
beside it.

## The loop

```yaml
  - name: environment
    components:
      Environment: { ... }             # something bright to occlude — see `ludion docs environment`
      GiVolume:
        cell_size: 0.25
        probe_spacing: 0.5
        normal_bias: 0.25
        sky_visibility: 1.0
        bounce: 1.0
```

1. Author the component (every field optional; an empty `GiVolume: {}` takes all five defaults —
   which, because `bounce` defaults to `0`, is sky visibility only, matching every scene authored
   before `bounce` existed).
2. `ludion render --scene your.yaml --dump-voxels grid.png` — a pure-CPU slice sheet of what got
   voxelized (albedo + emissive blocks), so a wrong `cell_size` or a missing wall shows up before you
   spend a bake on it. Needs no `--out`, and touches no GPU adapter.
3. `ludion render --scene your.yaml --dump-gi probes.png` — the probe volumes as VISIBILITY + BENT
   NORMAL + BOUNCE + DYNAMIC slice sheets. Interior probes should read dark, doorway probes bright;
   BOUNCE should glow near an emitter and stay dark past a partition, DYNAMIC near a marked light.
   An absent term SAYS why ("not baked", "not injected (no GiDynamic lights)") rather than drawing
   black; the header's `lights: N baked + M injected` census and bake-cache line show a thrashing
   bake at a glance.
4. `ludion render --scene your.yaml --out frame.png` — the real image; bless it as a golden once it
   reads right (`ludion docs verify-loop`).

`examples/gallery/gi-interior.yaml` is the calibration reference: a sealed room, one doorway, a
bright sky, and the field values below.

## Format

| Field | Default | Bound | Meaning |
|---|---|---|---|
| `cell_size` | `0.5` | `> 0` (`L1014`) | world units per voxel-grid cell — the voxelizer's resolution |
| `probe_spacing` | `1.0` | `> 0` (`L1014`) | world units between irradiance probes |
| `normal_bias` | `0.25` | `>= 0` (`L1014`) | world-space offset along the surface normal, applied at sample time, before the probe lookup |
| `sky_visibility` | `1.0` | `[0, 1]` (`L1014`) | strength: `mix(1.0, visibility, sky_visibility)` |
| `bounce` | `0.0` | `>= 0` (`L1014`), UI travel `[0, 2]` | strength: baked one-bounce irradiance is **added** to the ambient term, scaled by this value |

The volume's **origin and extent are never authored** — they derive from the scene's own world
bounds at `cell_size`, the same rule `render.bounds` uses for its own AABBs. `cell_size` is the one
thing the scene cannot be asked to infer, because the right resolution is an art-direction call (wall
thickness, prop scale), not a property of the geometry.

**`GiDynamic`: the marker for what moves.** A separate, field-less component — authored
`GiDynamic: {}` on the entity carrying the moving light or mesh, not a `GiVolume` field. A marked
`PointLight` is injected fresh every frame instead of baked (still gated by `bounce`, see Rules); a
marked `Mesh` neither occludes the probe rays nor deposits albedo/emissive into the voxelization.
Absent is today's behaviour exactly. **Mark every entity a system moves, recolours, or
reintensifies** — an unmarked one still renders correctly, but every such change invalidates the
load-time bake or voxelization, which rebuilds in full on the next frame. The failure is not a bad
pixel, it is a frame-time cliff with nothing visibly broken on screen: watch for it in `ludion
profile` (`ludion docs running`), not in the rendered image.

**Calibration** (`gi-interior`'s values, chosen off sweep sheets, not by taste):

- **`probe_spacing` must not exceed the thinnest occluder.** A lattice coarser than a wall can
  straddle it — probes on both sides interpolate to the same sample near the wall, leaking sky through it.
- **`normal_bias` is half of `probe_spacing`.** It stops a surface sampling a probe on the far side of
  the wall it's flush against; too small and it still can, too large and it over-corrects into the wrong room.
- `gi-interior` ships `cell_size: 0.25`, `probe_spacing: 0.5`, `normal_bias: 0.25` for room-scale
  geometry (wall thickness ~0.5). A larger district with thicker walls can afford a coarser
  `probe_spacing`; do not tighten it past what the geometry needs — probe count grows as the **cube**
  of the reciprocal.
- **`bounce`'s units are physical at `1.0`.** The volume stores the same cosine-weighted mean
  incoming radiance the IBL irradiance cube stores, so a fully-enclosing emitter of radiance `L`
  reconstructs to `L` at `bounce: 1.0` (no gain, no loss). `0` is off — the bake skips it entirely,
  see Rules. Above `1.0` is a deliberate exaggeration, not physical: the field's bound is a floor
  (`>= 0`), not `[0, 1]`, because the engine cannot know how much bounce an art direction wants —
  a night-lit game's emissive palette can run up to `5.0` on saturated colours.

## Rules

- **Absence is byte-identical.** No `GiVolume` voxelizes nothing and takes the pipeline exactly as it
  was before the component existed — no pass added, no binding changed in effect.
- **`sky_visibility: 0.0` is byte-identical to no component at all.** The shader's multiply is
  `acc * mix(1.0, visibility, sky_visibility)`; at `sky_visibility = 0` that's `acc * 1.0`, an
  IEEE-754 bit-preserving no-op. Use it as a sweep endpoint with no risk of moving a golden by
  accident, and as the "turn it off" value in an override.
- **Indirect only, and diffuse only, at every setting of either field.** `sky_visibility`'s multiply
  and `bounce`'s add both land on the ambient/IBL term alone, after the existing ambient-occlusion
  factor and before nothing; direct lighting, emissive surfaces, and indirect specular read exactly as
  they would with no `GiVolume` in the scene. A torch's own post is untouched by `bounce` — only the
  wall it lights changes. A material's baked occlusion map (`occlusion_texture`) darkens the bounce
  along with the rest of the indirect term; SSAO and sky visibility do not.
- **`bounce: 0.0` is the default, deliberately the opposite of `sky_visibility`'s `1.0` default in the
  same component.** `sky_visibility` only ever *removes* light a scene without `GiVolume` never had,
  so defaulting it on is safe; `bounce` *adds* light, and every scene that already authors a
  `GiVolume` — including content shipped before `bounce` existed — must stay byte-identical until it
  opts in. `bounce: 0.0` costs nothing: the bounce bake is skipped entirely (no volumes allocated, no
  extra ray work, no shader branch taken), not merely zeroed out after the fact.
- **An unmarked light re-bakes the volume every frame it moves; a `GiDynamic`-marked one never does.**
  A system that moves, recolours, or re-intensifies an unmarked point light invalidates the cached
  bake on the very next frame. Mark it `GiDynamic` instead and its motion reaches neither the bake's
  cache key nor its light loop — it is injected per frame in exchange, scaled by the same `bounce`
  strength as the baked term (see Format).
- **The host's `gi_dynamic` quality setting (`ludion docs quality`) masks only the injected term,
  never the marker's bake exclusion.** At `low`, a marked light's motion still invalidates nothing,
  but it stops being injected and reads direct-only. `full`/`high`/`medium` inject; the setting is
  per-machine, never scene content.
- **The voxelizer point-samples voxel models — one world cell per authored voxel centre.** A voxel
  model deposits densely only while `Transform.scale <= cell_size` (the convention: one authored voxel
  per object-space unit). Scale it past that and the deposit thins into a hollow lattice — the model
  reads as a shell, not a solid — and **refining `cell_size` makes it worse, not better**, because a
  finer grid only spreads the same point samples further apart. If a voxel prop is voxelizing hollow,
  shrink its `Transform.scale` or accept a coarser `cell_size`; there is no resolution fix.
- **Treat an unmarked `GiVolume` scene's geometry as static content; `GiDynamic` is the escape
  hatch.** The voxelizer caches its build behind a signature of the scene's placed instances (model
  matrix, colour, material); an unmarked instance a system animates every frame — moving,
  recolouring, changing emissive — invalidates that cache every frame and pays full re-rasterization
  each time. A `GiDynamic`-marked instance is excluded from the signature entirely, so its motion is
  free — in exchange, it stops occluding probe rays and stops depositing into the grid (see Format).

## Gotchas

- **`L2103`** — `cell_size` splits the scene's world bounds into more voxel-grid cells than the
  voxelizer will allocate (32M total, or too many along one axis for a 3D texture). The message names
  the count and suggests a `cell_size` that fits; almost always either a mistyped `cell_size` or one
  stray entity placed far from the rest, stretching the bounds around empty space.
- **`L2104`** — `--dump-voxels` or `--dump-gi` was pointed at a scene that authors no `GiVolume`, so
  extraction built nothing to dump. Add a `GiVolume` entity (minimally just `cell_size:`); `--set`
  can tune a field on a `GiVolume` the scene already has, but it never creates the component.
- **`L2105`** — `probe_spacing` lays more probes over the grid's world box than the bake will
  allocate (4M total, or too many along one axis). Same shape as `L2103`; raise `probe_spacing`.
- **`--dump-gi` requires a GPU adapter; `--dump-voxels` does not.** The voxel grid is CPU data, so
  `--dump-voxels` alone (no `--out`) never touches a GPU. The probe volume exists only as a baked GPU
  texture, so `--dump-gi` bakes it (or hits the renderer's cache) and reads it back — `L2003`/`L2004`
  apply if no adapter is available.
- **The bake is deterministic per machine, not across machines.** It's GPU float accumulation; two
  renders on the same adapter are byte-identical, but do not expect the Windows and macOS goldens for
  a `GiVolume` scene to match pixel-for-pixel the way CPU-side state does.
- **`GiDynamic` on the sun's entity marks only its `Mesh`/`PointLight`, if any — the
  `DirectionalLight` always bakes.** A moving sun re-bakes the volume every frame regardless of the
  marker.

## Next

- `ludion docs environment` — the sky and IBL the volume's ambient term multiplies.
- `ludion docs materials` — the surfaces the ambient term lands on.
- `ludion docs voxel-format` — authoring the voxel models the voxelizer deposits.
- `ludion docs lighting` — direct light, untouched by this feature.
- `ludion docs quality` — the per-machine `gi_dynamic` setting that masks the injected term.
