---
description: Fog and light shafts as one single-scattering medium, split across composite.wgsl and shafts.wgsl
---
# The participating medium (fog + light shafts)

**Scope:** The single-scattering medium behind the `fog_*` and `shafts` knobs — the one look feature whose math is split across two shaders and must not drift between them.

**Fog and the light shafts are two terms of one single-scattering medium, not two effects.** For an
eye ray reaching a surface at distance `d`:

```
L = T(d)·surface + ∫₀ᵈ σ_s·T(t)·[ L_ambient + p(θ)·V(t)·L_sun ] dt
```

- **One extinction, one albedo.** σ_t is `PostProcess.fog_density`, α is `PostProcess.fog_albedo`, and
  σ_s = α·σ_t. There is no second density — a change to `fog_density` moves the haze *and* the beams'
  reach, brightness and dimming together.
- **The ambient term is analytic** — `composite.wgsl`'s `fog_factor` gives `1 − T(d)`, and the closed
  form is `T(d)·surface + α·(1 − T(d))·L_ambient`.
- **The directional term is `shafts.wgsl`'s march** — 32 steps, Bayer-offset, `SHAFT_OPTICAL_CUTOFF /
  σ_t` (= `3/σ_t`) or the recorded surface as its end, whichever is nearer. It folds in σ_s, the
  Henyey–Greenstein phase at `g = fog_anisotropy`, and `PostProcess.shafts`, so the blurred target *is*
  the term `composite` adds — tinted by the sun's premultiplied colour and nothing more. With a cascaded
  sun the `Shafts` uniform carries the same cascade lanes: each step picks its cascade hard, with no
  band, and a step past the last far bound is lit (`sun_visibility`).
- **The geometry is shared through `snippets/medium.wgsl`.** `medium_optical_depth` (Wenzel's closed
  form for an exponential height medium past `fog_start`) is the *same function* the haze evaluates at
  the surface and the march evaluates per step, and `medium_density_ratio` weights each marched sample
  by the medium's local density. So `fog_start` / `fog_height` / `fog_height_falloff` shape the beams
  exactly as they shape the haze. At `fog_height_falloff == 0` the ratio is an exact `1.0` and the
  optical depth exactly `σ_t·t`: the collapse is the formula's own limit, with no branch and no second
  code path — which is what makes a height-free medium byte-identical, since `x * 1.0` is
  bit-preserving.
- **`L_ambient` is `mix(fog_color, sky_gradient(view_ray), fog_sky)`**, evaluated in the composite from
  the same `snippets/sky_gradient.wgsl` `sky.wgsl` draws the background with — three authored colours
  and a direction, exact, with no cube to bind and no new binding. The view ray makes it the
  aerial-perspective approximation rather than the isotropic integral. `fog_albedo` scales the term;
  `Environment.ibl_intensity` does not.
- **The recorder forces `fog_sky` to zero when the scene carries no `Environment`** (the sunless-uniform
  precedent), rather than adding a flag or a pipeline permutation — so a scene authoring `fog_sky` with
  no sky renders byte-identically to one that omits it.
- **`PostChain::shafts` is a three-way gate** — `PostProcess.shafts > 0` **and** a sun **and**
  `fog_density > 0`. The third condition is not defensive: with no medium the two passes would march 32
  steps and run a 4×4 blur to produce an exact zero, and an authored-but-inert `shafts` would still read
  as live to any client inspecting the chain. A scene missing any of the three gets neither pass.

## Gotchas
- **The shipped fog's bytes live in the intrinsic, not in the algebra.** At `fog_albedo == 1.0` the
  shader keeps the `mix` intrinsic rather than the algebraically equal explicit sum: `mix` lowers to
  each backend's own blend (HLSL `lerp` is `x + s*(y − x)`, not `x*(1 − s) + y*s`). Editing that
  branch moves goldens.
- `L_ambient` is reconstructed at the far plane **outside** the depth branch. The radiance does not
  depend on depth, and computing it inside the branch perturbs how the backend schedules the rest of
  the fragment enough to move a pixel of `post-full` with the branch never taken.

The frame graph and this crate's layout are `ludion devdocs ludion-renderer`; the per-pass walk is `ludion devdocs renderer-passes`.
