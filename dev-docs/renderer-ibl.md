---
description: Image-based lighting: the sky prefilter, the lobe-matched specular chain, and the content cache
---
# Image-based lighting (IBL)

**Scope:** How an `Environment` becomes light on a mesh: the prefilter in `ibl.rs`/`ibl.wgsl`, diffuse sampling in `mesh.wgsl`, the cache, and the flag-off fallback. The BRDF this feeds is `ludion devdocs renderer-shading`.

`RenderScene.environment` present drives `Ibl::resolve` (`ibl.rs`) to prefilter the sky into three
resources. `mesh.wgsl` samples only the irradiance cube through `@group(3)`
(`irradiance_tex`/`env_samp`/`env: EnvParams`); the `reflections` pass binds `specular_tex` and `brdf_lut` (`ludion devdocs renderer-reflections`). Absent, 1×1 fallbacks are
bound and `EnvParams.params.x` (the flag) is `0`. Pinned math, not to be casually retuned — a change
here re-blesses every `Environment`-carrying golden.

**What's prefiltered, from what.** The three sky colors alone (`sky_zenith`/`sky_horizon`/
`sky_ground` — the disc and the ambient knob are consumed elsewhere), into `IBL_FORMAT` targets whose
extents and mip counts are the `*_SIZE`/`*_MIPS` consts in `ibl.rs`, sample counts the
`IRR_SAMPLES`/`SPEC_SAMPLES`/`BRDF_SAMPLES` consts in `shaders/ibl.wgsl`:

- **Irradiance cube** — one compute dispatch per face, cosine-weighted hemisphere samples over a
  Hammersley sequence and a per-normal tangent basis, **stored as the mean radiance (no `1/π`
  division)** so `irradiance × albedo` in `mesh.wgsl` matches the same premultiplied-diffuse
  convention a `PointLight` already uses — one light-unit convention across both source kinds.
- **Specular cube** — `QualityParams::ibl_specular_size` (256², 128² at `low`) down a chain to the
  8×8 face (`lobe::chain_max_lod`), one roughness mip per level, GGX-importance-sampled at
  `lobe::mip_roughness` under the `N = V = R` split-sum assumption; mip 0 is a mirror sample of the
  gradient. The lookup lod is `lobe::cube_lod` / `specular_cube_lod` in `snippets/lobe.wgsl`, the one
  function every reflection path shares: `ludion devdocs renderer-reflections`.
- **BRDF LUT** — environment-independent (`NdotV` × roughness → a `(scale, bias)` pair for `f0`),
  computed **once** in `Ibl::new`. Every environment reuses it; only the two cubes re-bake per sky.
  It integrates the direct lobe's own height-correlated Smith visibility (`snippets/dfg.wgsl`), so
  R + G is the single-scatter albedo at `F0 = 1` and `dfg_specular` can compensate multi-scatter.

All three passes are deterministic by construction: fixed sample counts, a pure Hammersley
low-discrepancy sequence (`radical_inverse_vdc`/`hammersley`), no RNG and no atomics — same input,
same bytes, on a pinned GPU.

**One chain, two sources.** The specular mip loop is `Ibl::dispatch_chain`, shared with the probe path
(`Ibl::prefilter_probe`): each mip runs at `lobe::mip_roughness` into six layers of a target, and a
probe writes slot `s` of a cube array at `base_layer = 6·s`. The sky's `prefilter` entry stays analytic
(it samples `sky_gradient`), so its bytes are pinned; the probe's `prefilter_cube` entry integrates the
same lobe, the same Hammersley sequence and `SPEC_SAMPLES` over a captured source cube, reading the
source mip whose texel solid angle matches each sample's (filtered importance sampling), because a
captured scene is high-frequency and 64 samples off mip 0 would alias. The `downsample` entry builds
the source chain. Alpha is filtered with rgb by the same weights.

**Sampling.** `env.params.x > 0.5` (IBL on) makes `mesh.wgsl` sample `irradiance_tex` at the surface
normal for the diffuse term, times `ibl_intensity` (`env.params.y`). **`mesh.wgsl` does not sample the
specular cube**: the `reflections` pass reads it through `sky_specular` (`ludion devdocs
renderer-reflections`), with `brdf_lut` at `(NdotV, roughness)` for the split-sum `(scale, bias)`. The
flag off takes the flat-ambient line instead — the same branch-on-a-uniform pattern as the sunless
shadow fallback; see `ludion devdocs renderer-shading`.

**No sun/IBL double-counting.** The sun/moon disc renders in the sky background only (`sky.wgsl`) and
is deliberately **excluded from the prefilter** — `ibl.wgsl`'s own `sky_gradient` (duplicated
byte-for-byte from `sky.wgsl`'s, since a render shader and a compute shader can't share a module) bakes
the smooth authored gradient alone, never the disc. The direct sun stays the analytic
`DirectionalLight` term with its own shadow; baking the disc into the cubes too would double-count it
once through that analytic specular highlight and once through the reflected sky.

**Content-hash cache.** `Ibl` caches one prefiltered `(irradiance_cube, specular_cube)` pair, keyed by
`sky_hash` and the specular size — a content hash (`ludion_assets::content_hash`) of just the three sky colors (36 bytes:
3 colors × 3 floats × 4 bytes), the mesh/texture-cache precedent. A live edit that changes a sky color or the quality's specular size
re-keys and misses, triggering a re-prefilter; a live edit that doesn't (a disc tweak, an
`ibl_intensity`/`ambient` change, a `PostProcess` bloom tweak) hits the cache and reuses the existing
cubes. The BRDF LUT and the 1×1 fallback cube are computed once at `Ibl::new` and never re-keyed.

**The 1×1 fallback, and why it costs nothing.** An `Environment`-less scene binds a 1×1×6 fallback
cube to both cube slots and the flag off — the same "always-bound, never-sampled-when-off" shape as
the sunless shadow-map fallback (`ludion devdocs renderer-shading`). `mesh.wgsl` never reaches the
`textureSampleLevel` calls on that branch, so the fallback's contents are irrelevant; its only job is
keeping the `@group(3)` bind group well-formed. The `reflections` pass skips the sample the same way, on its own flag.

The frame graph and this crate's layout are `ludion devdocs ludion-renderer`; the per-pass walk is `ludion devdocs renderer-passes`. An opt-in `GiVolume` multiplies this same ambient/IBL term by a further, per-position baked visibility factor — `ludion devdocs renderer-gi`.
