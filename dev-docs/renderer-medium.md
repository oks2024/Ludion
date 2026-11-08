---
description: Fog, sun shafts, local-light scatter and GI haze as one medium, split across three shaders
---
# The participating medium (fog + light shafts)

**Scope:** The single-scattering medium behind the `fog_*` and `shafts` knobs — the one look feature whose math is split across three shaders (`composite.wgsl`, `shafts.wgsl`, `medium_scatter.wgsl`) and must not drift between them.

**Fog and the light shafts are two terms of one single-scattering medium, not two effects.** For an
eye ray reaching a surface at distance `d`:

```
L = T(d)·surface + ∫₀ᵈ σ_s·T(t)·[ L_ambient + p(θ)·V(t)·L_sun + Σ p(θ_l)·V_l(t)·scatter_l·E_l(t) ] dt
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
- **The local-light term is `medium_scatter.wgsl`**, a half-resolution pass (plus `medium_scatter_blur`,
  a 4×4 box blur over its four channels). Per pixel it clips the eye ray to each selected light's
  sphere and takes Bayer-jittered steps over that segment only, evaluating the same `medium.wgsl`
  functions under the light's falloff, spot cone and phase angle. Steps per segment are
  `max(shaft_steps / 4, 2)`. The result is the composite's binding 6 (`medium_scatter_tex`), added
  right after the shafts; with the pass off the composite binds a stand-in and the `medium_scatter`
  flag is clear.
- **Selection is CPU-side in `scatter.rs`** (`select_scatter`): at most 64 lights that opted in
  (`scatter > 0`), ranked by `scatter·intensity·max(color)·range·e^(−σd)`, ties to the lower
  light-list index, returned in ascending index so the GPU's sum order is list order. A light with a
  local-shadow grant tests each step against the local-shadow atlas with one exact texel load (no PCF —
  the step is in free space), using `mesh_shading`'s face table; one without a grant, or any light in
  a frame with no atlas, scatters unshadowed. Scatter never changes who is granted.
- **GI rides the same pass** (group 1 binds the baked GI volumes; the injected dynamic volume is in
  group 0). It reads DC terms only. The pass's **alpha is V̄**, the sky visibility averaged along the
  ray weighted by in-scatter; the composite multiplies it into `L_ambient` (the `medium_gi` flag), and
  the rgb gains the bounce, scaled by `GiVolume.bounce` as mesh shading scales it.
- **`PostChain::medium_scatter` is the pass gate**, true when either reason holds: `local_scatter`
  (the medium and a selected scattering light) or `medium_gi` (the medium and a `GiVolume` with a
  nonzero `sky_visibility` or `bounce`). It needs no sun and no shadow map, so it sits beside the
  three-way `shafts` gate, not under it. `Quality.shafts` masks it. `PostChain::without_medium`
  drops it for probe captures, which have no composite.

## Gotchas
- **The shipped fog's bytes live in the intrinsic, not in the algebra.** At `fog_albedo == 1.0` the
  shader keeps the `mix` intrinsic rather than the algebraically equal explicit sum: `mix` lowers to
  each backend's own blend (HLSL `lerp` is `x + s*(y − x)`, not `x*(1 − s) + y*s`). Editing that
  branch moves goldens.
- **The composite's GI multiply sits outside the depth branch on purpose,** beside the ambient it
  scales: ambient is depth-independent, and moving the multiply inside the branch perturbs scheduling
  as the next bullet describes.
- `L_ambient` is reconstructed at the far plane **outside** the depth branch. The radiance does not
  depend on depth, and computing it inside the branch perturbs how the backend schedules the rest of
  the fragment enough to move a pixel of `post-full` with the branch never taken.

The frame graph and this crate's layout are `ludion devdocs ludion-renderer`; the per-pass walk is `ludion devdocs renderer-passes`.
