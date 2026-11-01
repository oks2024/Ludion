---
description: The one-bounce GI term — radiance at a hit, storage layout, and the cache key's light signature
---

# Renderer GI — bounce

**Scope:** the one-bounce radiance term the GI bake's compute dispatch produces alongside sky
visibility, when the authored `GiVolume.bounce > 0`. Same file, same march, same cache — this topic
covers only what bounce adds: the radiance-at-a-hit formula, the three-volume storage, the cache key's
light signature, and `mesh.wgsl`'s `gi_bounce` consumption. The shared bake machinery (the DDA march,
the probe lattice, the cache shape, sky-visibility's own consumption) is `ludion devdocs renderer-gi`;
read it first. The authored field is `ludion docs global-illumination`.

## Layout

Same files as `ludion devdocs renderer-gi`; bounce adds no new file.

- `gi_bake.rs` — `BakeInput.lights`/`.sun`, `lights_signature`, `premultiplied`, the bounce volumes in
  `Baked`/`ResolvedGi`, `voxel_bake.rs`'s albedo texture as a bake input.
- `gi_bake.wgsl` — `radiance`, `direct_at`, `sun_shadow`, the bounce accumulators in `bake`.
- `mesh.wgsl` — `gi_bounce(world_pos, n)` and its add site in `shade`.

## Dataflow

```
gi_bake.wgsl `bake`, per ray, after trace_hit:
    hit.hit == false          → contributes nothing (the sky is gi_sky_visibility's business)
    hit.hit == true, bounce_enabled:
        albedo   = textureLoad(albedo, hit.cell)          // voxel_bake.rs's per-cell texture
        emissive = textureLoad(occupancy, hit.cell).r      // free: same texel occupancy already read
        direct   = Σ_lights color·ndl·atten²  +  sun.color·ndl_sun·sun_shadow(hit.cell, hit.normal)
        L_out    = albedo * (emissive + direct)
        accumulate L_out into three SH-L1 sums, one per channel
    → gi_bounce_r/g/b: three Rgba16Float 3D textures, written once per probe, all-zero if bounce_enabled
      is false or the probe was buried past push-out's reach
    → mesh.wgsl group 3 bindings 7/8/9, sampled through gi_bounce()
        → ADDED to the ambient term, after gi_sky_visibility's multiply, scaled by `diffuse` and
          `env.gi_bounce.x`
```

## Rules

- **Radiance at a hit: `L_out = albedo(cell) × (emissive(cell) + direct(cell, n))`.** `albedo` is
  `voxel_bake.rs`'s per-cell `Rgba32Float` texture — which had no reader before bounce, and carried an
  `#[allow(dead_code)]` this feature deletes. `emissive` is the `.r` channel of the same occupancy texel
  the march already loads for occupancy (`.g`), so it costs a swizzle, not a fetch. **`albedo ×` covers
  the emissive term too** — the same product `fs_main` forms (`lit + albedo * material.z`) — which is
  what makes the bounce carry the emitter's *colour*: `gi-interior`'s torch is `color [1.0, 0.62, 0.28]`
  at `emissive: 4.0`, radiance `[4.0, 2.48, 1.12]`, and a scalar-emissive bounce would read white
  instead of the orange pool the feature exists to produce.
- **`direct(cell, n)` mirrors `mesh.wgsl`'s own direct terms, through one shared premultiplied `Light`
  helper both the mesh pass and the bake call** — the convention (`light.color` IS the diffuse response
  at `ndl = 1`, ranged by `atten²`) is never re-derived. The light set is the **static (unmarked)
  subset** of `RenderScene.lights` (`gi_bake::static_lights` — a `GiDynamic`-marked light is injected
  per frame instead, see `renderer-gi-dynamic`), summed in the pinned ascending-`EntityId` extract
  order, which is also the float summation order; `direct` walks the shaded cell's slice of the
  world-space light bin (`gi_bin.rs`), byte-identical to the flat loop by the bin's conservative
  filing rule.
- **No shadow rays for point lights.** The grid's own occupancy is the occlusion, coarsely, which is the
  right amount for an indirect term; the cosine test does most of the work for free — a torch on the far
  side of a wall faces the wall's *other* normal, so `dot(n, l) ≤ 0` and the contribution is exactly
  zero. What survives is light leaking around a **corner**, accepted for an indirect term (shadow rays
  per light per hit are recorded as a deferred escalation, triggered by a corner-leak artifact called by
  eye).
- **The sun is the one source that is occluded**, because it has no range — without occlusion every cell
  in a sealed interior would bounce sunlight and flood the room. `sun_shadow` is one more DDA from the
  hit cell, offset **one whole cell** along `n` (not a small `t` epsilon — the face normal is an exact
  unit axis, so a whole-cell offset cannot re-enter the origin cell through a rounding error) toward
  `-sun.direction`: escaped ⇒ `1.0`, occupied ⇒ `0.0`. Runs only when the scene has a sun and
  `dot(n, l_sun) > 0`, so a sunless scene pays nothing. **Deliberate divergence from the render pass:**
  this is the *grid's* shadow, not the camera-fitted shadow map's, and the two disagree at the margins —
  the shadow map is unavailable to a cached load-time bake, and for an indirect term the grid's answer
  is the right one.
- **An escaped ray contributes exactly `vec3(0.0)`.** The sky arrives through the prefiltered IBL cube,
  modulated by the visibility volume; adding sky radiance in the bounce term would double-count it.
- **One bounce only.** `L_out` is evaluated at the hit and the ray stops; a second bounce would re-march
  from the hit and gather `L_out` again. No bounce-count constant, authored field, or loop ships — that
  knob belongs to a future quality-tier system, not to `GiVolume`.
- **Storage: three `Rgba16Float` volumes, one per colour channel (R/G/B), each structurally identical to
  the visibility volume — never one volume with the coefficient sets packed along an axis.** Packing
  (e.g. `z = k*4 + c`) would blend across coefficient sets under hardware trilinear filtering, and manual
  trilinear in the shader (eight fetches per volume) was rejected outright. Per-probe cost is 8 B → 32 B;
  `bounce == 0.0` allocates none of the three (the fallbacks fill both the compute pass's write slots and
  the mesh pass's sampled bindings), so a scene wanting sky occlusion without bounce pays nothing extra.
  Group 3 gains bindings 7/8/9 (`gi_bounce_r`/`_g`/`_b`), bringing the fragment stage to nine sampled
  textures total — comfortably under WebGPU's guaranteed 16, with seven of headroom for the next
  coefficient set to count against.
- **Cache key's bounce-specific terms: `lights_signature` and `bounce_enabled`.** `lights_signature` is
  an XXH3 over the sun's presence/direction/colour, then every light's **premultiplied** `Light` bytes
  (`color * intensity`, in `PointLightData`, not the raw authored fields) in pinned extract order —
  premultiplied because that is exactly what the bake reads, so two lights whose authored fields differ
  but whose product agrees share a cache hit rather than re-baking for nothing. `bounce_enabled` is the
  zero/non-zero discriminant of the authored `bounce` value only; the non-zero *value itself* stays out
  of the key, because strength is applied at sample time (`env.gi_bounce.x` in `mesh.wgsl`) and changing
  it re-reads the volumes already written rather than requiring new ones —
  `needs_gpu_the_bake_key_tracks_the_lights_and_the_bounce_switch` proves flipping between two non-zero strengths
  costs zero re-bakes, while a moved, recoloured, or re-intensified light each cost one. The sun's
  `light_space`/`bias` are **excluded**: camera-fitted, read only by the render pass's own shadow-map
  stage, and hashing them would re-bake the world on every step the camera takes — proven by
  `needs_gpu_the_bake_key_reads_the_suns_direction_and_colour_but_not_its_shadow_fit`. **Documented consequence:**
  a light a system moves invalidates the bake every frame, exactly as a moving mesh invalidates the
  voxelization every frame — two ways to thrash a load-time cache instead of one.
- **Consumption: `gi_bounce(world_pos, n)` shares `gi_sky_visibility`'s sample point and reconstruction,
  differing only in the floor.** Same `normal_bias` push, same `sh_l1(sh, n) = sh.x + (2/3)·dot(sh.yzw,
  n)` per channel. `gi_sky_visibility` clamps to `[0, 1]` (a fraction has a ceiling); `gi_bounce` floors
  with `max(e, vec3(0.0))` only — this is HDR radiance with no upper bound, and an SH-L1 field can ring
  negative under a strongly directional bounce.
- **The add site: `acc = acc + gi_bounce(world_pos, n) * diffuse * env.gi_bounce.x`, after
  `gi_sky_visibility`'s multiply, inside the same `env.gi_origin.w > 0.5` block.** `× diffuse`, not raw
  and not `albedo`: `gi_bounce` returns *incident* irradiance, and the outgoing diffuse radiance is
  `E(n) · (1 - metallic) · albedo`, exactly `diffuse` — the same factor the IBL branch's
  `irradiance * diffuse` uses; using `albedo` alone would light metals diffusely. **Not attenuated by
  AO** — reordering the expression would move every AO golden, and darkening the only light source in a
  torch-lit interior is the `ssao_direct` failure mode repeated; recorded as a deferred micro-decision
  with a trigger (the sitting reports bounce reading flat at contact scale), not a closed one. Diffuse
  only: indirect specular is not expressible in SH-L1 and stays the prefiltered cube's job. Nested inside
  the existing GI branch behind its own `env.gi_bounce.x > 0.0` guard, so a non-GI frame cannot reach the
  branch and the `x + 0.0 == x` identity (never `-0.0` at that add site, since every term feeding `acc`
  there is a product or sum of non-negative quantities) is what keeps a `bounce: 0.0` scene's rendered
  bytes unchanged.

## Gotchas

- **The district-scale light term is dead; the sun march is what remains.** The old
  `probes × rays × lights` bound (billions of evaluations at district scale) was deleted by the
  world-space light bin: `direct` loops the hit cell's bin slice, so the per-hit cost is `O(bin
  occupancy ≈ 1–3)`, byte-identical to the flat loop by `gi_bin.rs`'s conservative-filing argument.
  What the bin cannot touch is `probes × gi_rays × sun_march_steps` — the per-hit sun shadow march on a
  sunlit district. The designed escalation (a per-(cell, face) sun-visibility mask, a byte-identical
  memoization since the face is the DDA's own entry axis) is recorded in
  `Plan/gi-wave3-dynamic-injection.md` §6.3, deferred until a measured district bake breaches the
  budget.
- **Vigil's emissive magnitudes are large.** Authored emissive runs up to `5.0` on saturated palette
  colours, and sixteen of twenty-one voxel models emit. A physically correct bounce off that content can
  blow past a comfortable grade range; `bounce`'s `x-ui-range [0, 2]` admits it will be authored below
  `1.0` as often as above.
- **A moving light or a moving mesh both re-bake the same dispatch** — see the cache-key rule above.
  There is no per-source distinction yet; `GiStatic` (deferred) is where one would land.

## Verify

```
cargo test -p ludion-renderer                   # includes gi_bake's bounce unit tests (below)
cargo nextest run -p ludion-engine --test it -E 'test(/^gi_bounce::/)'    # physics fixtures: torch lights room, albedo tint,
                                                 # distance falloff, sun occlusion, no leak through a wall
```

`gi_bounce.rs`'s `needs_gpu_an_enclosing_emissive_shell_reconstructs_its_own_radiance` is the unit gate: a probe
fully enclosed by an emissive shell of known radiance `L` with albedo `1.0` must reconstruct
`gi_bounce ≈ L` — the arithmetic proof behind `ludion devdocs renderer-gi`'s "same units as the
irradiance cube" claim, not left to prose.
