---
description: The PostProcess imaging singleton: the pinned composite chain and every field with its range
---

# Post-processing

**Scope:** The opt-in `PostProcess` component — the pinned composite order and every field with its
range. The nine `fog_*`/`shafts` fields have their own topic, `ludion docs atmosphere`; sky, sun and
IBL are lighting; surface material is materials.

`PostProcess` is the *imaging* singleton, the same opt-in shape as `Environment` and
`DirectionalLight`: at most one is active (extract takes the lowest `EntityId` and ignores the rest,
no error), and **absent, the renderer is byte-identical to a pipeline with no `PostProcess` support
at all**. The split with `Environment` is by subject — `Environment` describes how the world is
*lit*, `PostProcess` describes what the post stack does to the *image* the scene pass produced. One
entity can carry both; they are separate components, not a nested block:

```yaml
  - name: environment
    components:
      Environment:
        sky_zenith:
          - 0.015
          - 0.028
          - 0.075
      PostProcess:
        bloom_strength: 0.12
```

(from `examples/gallery/night-sky.yaml`.)

## The chain, in order

Most of the stack is branches in one `composite` pass, run in one pinned order:

```text
chromatic aberration → depth of field → + bloom + halation → the medium (haze, then beams)
  → exposure → tonemap → grade → vignette → grain → dither
```

With SMAA on, the tail is `… vignette → SMAA → grain → dither`: grain and dither move into the AA's
last pass. See `ludion docs anti-aliasing`.

The split worth knowing is scene-referred against display-referred. Everything through the medium
happens to the *light*, in linear HDR, before any camera transform — a camera photographs fogged
light, it does not fog a photograph. The grade and the vignette are the opposite: a look and a lens
darkening applied to the *finished image*, so they land after the tonemap operator. Grain is the
emulsion recording that image, and the dither is strictly last because it exists to hide the output
quantization.

What appears in that list for bloom, depth of field and the beams is only the *blend*: each is
computed by its own graph passes, because an effect needing its own sampling footprint or resolution
cannot fold into a per-pixel branch. Ambient occlusion is further out still — it is not imaging at
all, but a modulation of *lighting* inside the scene pass, which is why it is absent from the order
entirely.

## Fields

Every field is optional with a behavior-preserving default, so an empty `PostProcess: {}` renders
exactly as no component at all. A step left at its neutral value is **skipped, not computed**: a
scene pays only for what it authors. Two shipped defaults are deliberately *not* neutral — `exposure`
is `1.3` and the dither is on — because both are the engine's imaging chain rather than an authored
effect.

This is the orientation map; `PostProcess`'s rustdoc in `crates/ludion-components/src/lib.rs` is
the reference, and records why each default is the number it is.

**Bloom, exposure, output**

- **`bloom_threshold`** — the bright-pass luminance cutoff: pixels brighter than this bloom, `>= 0`
  (default `1.0`). Lower it to bloom more of the frame, raise it to restrict bloom to the genuinely
  overdriven highlights (see `ludion docs materials --section "Emissive + bloom (copyable)"`).
- **`bloom_strength`** — how strongly the blurred bloom is mixed back in, `>= 0` (default `0.1`).
  The default is small because the chain is a six-level pyramid that composites the sum of all six
  levels — about 5.9× the energy of a single narrow blur at the same strength. A value authored
  against a single-blur bloom will blow the frame out. The mesh pass clamps its shaded radiance
  (sprites, particles and text do not) at 60 000 (below half-float overflow), and the bloom chain
  reads an infinite or NaN texel as 0 or that ceiling, so one bad texel cannot darken or poison the
  glow around it.
- **`exposure`** — a **linear multiplier** (not EV stops) on scene + bloom, applied immediately
  before the tonemap, `>= 0` (default `1.3`). It is the one scalar here that is not neutral at its
  natural value: `1.3` is what puts mid-grey where AgX expects it. Because it lands after the
  bright-pass, `bloom_threshold` keeps meaning *scene* luminance whatever the exposure is.
- **`tonemap`** — `agx` (default) | `aces` | `neutral` | `none`. AgX ships deliberately muted and
  expects a grade on top; `none` is the diagnostic setting, showing what the scene pass actually
  produced.
- **`dither`** — ordered sub-LSB dither on the 8-bit output (default `true`). On by default because
  banding is a defect rather than a look; it is deterministic and carries no frame index, so a
  single-frame render reproduces exactly.

**The grade** — display-referred, applied after the tonemap. Grading has no canonical order of
operations, so this one is part of the contract: white balance, then lift/gamma/gain, then contrast,
then saturation. Each step is skipped by its own comparison, so an all-identity grade is
byte-identical to no grade.

- **`temperature`** / **`tint`** — white balance, blue↔amber and green↔magenta, both `[-1, 1]`
  (default `0.0`), applied as one luminance-preserving channel scale.
- **`lift`** / **`gamma`** / **`gain`** — the ASC CDL triples, `pow(max(x * gain + lift, 0), 1 /
  gamma)` (defaults `[0,0,0]` / `[1,1,1]` / `[1,1,1]`). `gamma` is applied as its **reciprocal**, so
  a higher value *brightens* the midtones — the convention every grading UI uses.
- **`contrast`** — about a `0.5` pivot, `>= 0` (default `1.0`). On a mostly-dark frame it cannot
  deepen the shadows without pushing them through zero; darken with `gamma` instead.
- **`saturation`** — a mix toward Rec.709 luma, `>= 0` (default `1.0`); past `1.0` extrapolates.

**The medium** — `fog_density`, `fog_color`, `fog_start`, `fog_height`, `fog_height_falloff`,
`fog_albedo`, `fog_sky`, `fog_anisotropy` and `shafts` are nine knobs on one participating medium,
not on two effects. `ludion docs atmosphere` is the model; defaults are `0.0` extinction (which gates
the whole thing off), a mid grey ambient radiance, `1.0` albedo, `0.0` sky coupling and `0.7`
anisotropy.

**Ambient occlusion**

- **`ssao`** — how much of the estimate to apply, `[0, 1]` (default `0.0` — off, and the gate). Above
  zero, three passes are prepended to the frame; at zero none is built.
- **`ssao_radius`** — the sampling hemisphere in **world units**, `>= 0` (default `0.5`), which is
  what makes the effect identical at any resolution and any render scale.
- **`ssao_bias`** — the world-space depth a sample must clear before it counts as occluded, `>= 0`
  (default `0.02`). Raise it if flat walls look dirty; lower it if contact shadows pull away.
- **`ssao_direct`** — how much of the finished factor *also* attenuates direct light, `[0, 1]`
  (default `0.0` — the ambient + IBL term only). The default split is why AO does not read as dirt on
  torch highlights; this field is for the case it does not cover, a scene lit almost entirely by
  direct light whose ambient term has nothing for AO to remove. Emissive is untouched at any setting.

**The lens**

- **`dof`** — the far field's circle of confusion as a fraction of the frame's height, `[0, 0.25]`
  (default `0.0` — off, and the gate). It is the lens's *aperture*: there is deliberately no separate
  in-focus range, because the sharp band is a consequence of the aperture. Widen it by lowering `dof`.
- **`focus_distance`** — the focal plane in world units, `>= 0` (default `10.0`; inert while `dof` is
  zero). A game that wants autofocus writes this field from a system each tick: a sim system (hashed, replays), or a presentation
  system on a presentation entity's `PostProcess` (unhashed, `ludion docs presentation`; the scene must
  have no sim `PostProcess`, which would win). Either way it is a function of world state, never of the
  camera's history, which an engine-side autofocus would make the image.
  `ludion render --look-at` sets it to the framing distance.
- **`vignette`** / **`vignette_radius`** — corner darkening strength `[0, 1]` (default `0.0` — off,
  and the gate) and the normalized screen radius it starts at `[0, 1]` (default `0.5`). The geometry
  is a function of normalized screen position alone, so it is elliptical and resolution-independent.
- **`chromatic_aberration`** — radial per-channel displacement at an edge midpoint, in normalized
  screen units, `[0, 0.1]` (default `0.0`). Applied at the fetch: it is how the image was formed, not
  something done to it.
- **`halation`** — a warm bleed around bright sources, `>= 0` (default `0.0`). It reuses the bloom
  chain's blurred bright field rather than blurring again, so it inherits `bloom_threshold`, the
  chain's radius and its gain — and a useful value therefore sits near `bloom_strength`'s own `0.1`.
  The colour is not authorable: halation's hue is a property of the film stock.
- **`grain`** / **`grain_size`** — monochrome additive grain in the **sRGB-encoded** domain, `[0, 1]`
  (default `0.0` — off, and the gate), with a cell side as a fraction of the frame's height `[0, 1]`
  (default `0.002`, about two pixels at 1080p). The pattern is a deterministic integer hash and
  static — it carries no frame index, so a moving camera moves through a fixed field.
- **`ssr`** — screen-space reflections, `[0, 1]` (default `0.0` — off, and the gate): the strength
  that scales the trace's confidence, so `0.0` is exactly the sky reflection. It is opt-in per
  scene. **`ssr_fade`** is the roughness band `[start, end]` (default `[0.45, 0.65]`) over which the
  trace gives way to the sky; **`ssr_max_distance`** (default `30`) and **`ssr_thickness`**
  (default `0.5`) are world units, both `> 0`. A miss falls back to the sky, and a machine's
  quality can switch it off (`ludion docs quality`).
  `examples/gallery/mirror-floor.yaml` is the worked scene; `ludion render --debug-view ssr_confidence`
  (or `ssr_footprint`) shows what the trace found.

## Rules

Every scalar rides its schema range, so an out-of-range value is an `L1014` naming the field with a
JSON Pointer at it, and a misspelled `tonemap` is the same code listing the four allowed values. A
non-finite value cannot be authored at all — the scene format has no spelling for `.nan`/`.inf`, and
neither does JSON on the wire. The three **triples** are what a schema range cannot reach (it does not
apply to array elements), so `gain >= 0`, `gamma > 0` and `fog_color >= 0` are checked semantically
and report `L1130` at the offending element. `lift` is deliberately unbounded there: it is an offset,
and a negative one legitimately crushes blacks. The SSR look is `L1273` when an `ssr_fade` end is
outside `[0, 1]`, its start is not below its end, or `ssr_max_distance`/`ssr_thickness` is `0`.

Tune any of these off a contact sheet rather than by reasoning about them — `render --sweep` creates a
field the scene left unauthored, so nothing has to be added to the file first. The worked commands are
in `ludion docs atmosphere --section "The tuning loop"`; `examples/gallery/post-full.yaml` authors the
whole roster and records, in its header, the sweep or the measurement behind every number in it.

## Next

- `ludion docs atmosphere` — fog and light shafts as one participating medium.
- `ludion docs lighting` — the sun and its shadow map.
- `ludion docs environment` — the sky and image-based lighting.
- `ludion docs reflection-probes` — the captured reflections `ssr` blends over.
- `ludion docs materials` — the three material scalars.
- `ludion docs verify-loop` — the golden-image gate any change here moves.
