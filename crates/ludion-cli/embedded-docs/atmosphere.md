---
description: Fog and light shafts as one participating medium: the nine fog fields, the traps, the sweep loop
---

# Fog and light as one medium

**Scope:** The nine `PostProcess` `fog_*`/`shafts` fields, the single-scattering model behind them,
and the contact-sheet loop for tuning them. The rest of the imaging chain is
`ludion docs post-processing`; the sky the medium can in-scatter is `ludion docs environment`.

Fog and the light shafts are not two effects that happen to look related. They are the two
in-scatter terms of one **single-scattering participating medium**, and every `fog_*` field
describes that medium rather than a haze. For an eye ray reaching a surface at distance `d`, with
transmittance `T(t) = exp(−∫₀ᵗ σ_t ds)`:

```text
L = T(d)·surface  +  ∫₀ᵈ σ_s·T(t)·[ L_ambient + p(θ)·V(t)·L_sun ] dt
    └ extinction ┘    └───────── in-scatter along the ray ────────┘
```

## Rules

- **`fog_density` is the extinction σ_t** — one number for the whole medium, and the gate for all of
  it. There is deliberately no second density for the beams: the reach of an exponential medium is a
  function of its extinction, so the march runs to `3/fog_density` world units (past that the
  transmittance is under 5% and the rest cannot reach an output step). Thickening the fog therefore
  hazes the frame *and* brightens the beams — and dims them, because they travel back through the
  same medium.
- **`fog_albedo` is the single-scattering albedo α** (default `1.0`, purely scattering): σ_s = α·σ_t
  scatters, (1 − α)·σ_t absorbs. Only the scattering half sends light toward the lens, so this one
  knob scales **both** in-scatter terms. A dark absorbing smoke and a bright scattering haze are one
  knob apart at the same thickness.
- **The ambient term is analytic.** With a constant ambient radiance the integral collapses to
  `T(d)·surface + α·(1 − T(d))·L_ambient` — one exponential per pixel, no march. That is the haze,
  and it is why fog costs no pass.
- **The directional term is the march.** `shafts` is its **pure intensity** and nothing else: the
  reach, the dimming, the brightness and the lobe all come from the medium. The `shafts` pass walks
  32 shadow-map samples along each eye ray for the visibility `V(t)`, folds in σ_s and the phase, and
  a 4×4 blur cancels the march's per-pixel step offset; the composite only adds the result, tinted by
  the sun's colour. A value above `1.0` is legal and simply pushes the beams past what the medium
  physically scatters.
- **`fog_anisotropy` is the Henyey–Greenstein `g`** (default `0.7`, exclusive `(-1, 1)` — the phase
  function is singular at ±1). It reaches the **marched term only**, because the ambient term is
  isotropic and has no angle to be asymmetric about. At `0` the sun's contribution reads as a flat
  lift over the whole frame; positive values throw it forward along the sun's travel, which is what
  turns it into a shaft.
- **`fog_start`, `fog_height` and `fog_height_falloff` shape the whole medium.** The medium exists
  only past `fog_start`, and within it the density is `fog_density · exp(−fog_height_falloff · (y −
  fog_height))`. Both terms integrate that same optical depth, so a height medium thickens the beams
  toward the ground exactly as it thickens the haze, and a `fog_start` past the geometry removes both
  together. At `fog_height_falloff: 0` the formula is plain distance fog at its own limit, so the
  height half is a skip rather than a special case, and `fog_height` is inert.
- **`fog_color` and `fog_sky` decide the ambient radiance:** `L_ambient = mix(fog_color,
  sky(view_ray), fog_sky)`. At `fog_sky: 0` the haze is one authored slab; at `1.0` it is the
  `Environment` sky along that ray, blue toward the zenith and warm in the horizon band.

**The haze does not apply where nothing was drawn.** A background pixel is at the far plane, which is
infinitely distant, so the extinction would replace it outright and flatten an authored sky into a
slab of fog colour. Those pixels are skipped by the ambient term (the beams still cross them, having
no surface to stop at), which is what gives the haze a horizon to meet — and `fog_sky` is how it meets
it. At `0` only a `fog_color` picked near `Environment.sky_horizon` reads continuous; at `1.0` the
haze *is* the sky along that ray and the two agree by construction.

| What you want | `fog_density` | `fog_albedo` | `shafts` | `fog_anisotropy` | `fog_sky` |
|---|---|---|---|---|---|
| Fog, no beams | any | any | `0` (or no sun) | — | taste |
| Dark absorbing smoke | high | low | `0` | — | `0` (a slab reads as smoke) |
| Beams in a thin haze | small | high | large | high | taste |
| Haze that becomes the sky | any | any | any | — | `1.0` (needs an `Environment`) |

## Gotchas

**Two traps, both of them silence rather than an error.**

- **`shafts` on its own renders nothing.** The march needs all three of `shafts > 0`, a
  `DirectionalLight`, and `fog_density > 0` — no sun is no shadow map to march, and no medium is
  nothing to scatter. Miss any one and the two passes are not scheduled at all, which is the
  component's standing "absence is meaningful" posture, not a diagnostic. `ludion dump-graph --scene
  <file>` is how you see it: a working shaft frame lists a `shafts` and a `shafts_blur` pass, and a
  scene missing the sun or the medium lists neither. So "beams without fog" is a **thin** medium and
  a strong `shafts`, never `fog_density: 0`.
- **Shafts only see what the sun's map covers.** With the default anchored box, air outside the box is
  lit: anchor the box over the shafts, or use `cascades`. With a cascaded sun the march samples the
  cascades, and the last one reaches the march's `3/fog_density`, so an occluder anywhere within the
  reach shadows the air behind it (`ludion docs sun-cascades`).
- **`fog_sky` needs an `Environment`.** With no sky to in-scatter, the renderer forces the blend to
  zero and the medium is the flat `fog_color` slab — a scene may author `fog_sky` with no
  `Environment` and render byte-identically to one that omits it. `Environment.ibl_intensity`
  deliberately does not scale the term either: that knob grades how much the sky lights *surfaces*.

## The tuning loop

Tune the medium off a contact sheet rather than by reasoning about it. `render --sweep` renders the
scene once per value and stitches the results into one labelled sheet, and it creates a field the
scene left unauthored, so nothing has to be added to the file first:

```
ludion render --scene examples/gallery/post-full.yaml --out density.png \
  --sweep /entities/0/components/PostProcess/fog_density=0.004,0.01,0.02,0.04

ludion render --scene examples/gallery/post-full.yaml --out beams.png \
  --set /entities/0/components/PostProcess/fog_sky=1.0 \
  --sweep /entities/0/components/PostProcess/shafts=0.0,0.02,0.06,0.15
```

Judge the sheet, then commit the chosen value into the scene. A sheet is a judging instrument, never
a golden — regenerate it against current code rather than keeping one around.
`examples/gallery/post-full.yaml` authors the whole roster and records, in its header, the sweep or
the measurement behind every number in it.

## Next

- `ludion docs post-processing` — the rest of the imaging chain.
- `ludion docs environment` — the sky the medium in-scatters.
- `ludion docs lighting` — the sun the beams are marched against.
- `ludion docs materials` — the three material scalars.
