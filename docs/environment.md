---
description: The Environment sky and image-based lighting, plus the per-instance Fade occlusion dither
---

# Environment & sky

**Scope:** The opt-in `Environment` singleton — a procedural gradient sky, an optional sun/moon disc,
the flat ambient term and image-based lighting — and the independent per-instance `Fade` occlusion
dither. The sun that lights the scene is `ludion docs lighting`; imaging is
`ludion docs post-processing`; the surfaces being lit are `ludion docs materials`.

`Environment` is the `DirectionalLight` sun precedent applied to the sky and its lighting. It is
*lighting* only — what the post stack does to the finished image is `PostProcess`'s job. As with
`DirectionalLight`, at most one is ever active: extract picks the lowest `EntityId` and ignores the
rest, no error. **Absent, the renderer is byte-identical to a pipeline with no `Environment` support
at all** — the hardcoded flat ambient, the plain clear-color background, no image-based lighting.
Adding one is a content decision that re-blesses its own scene's golden and touches no other scene's
bytes.

```yaml
  - name: environment
    components:
      Environment:
        sky_zenith:
          - 0.015
          - 0.028
          - 0.075
        sky_horizon:
          - 0.07
          - 0.10
          - 0.17
        sky_ground:
          - 0.008
          - 0.010
          - 0.018
        sun_disc_size: 5.0
        sun_disc_intensity: 8.0
        ibl_intensity: 1.9
  - name: moon
    components:
      Transform:
        translation:
          - -7.0
          - 12.0
          - 9.0
      DirectionalLight:
        direction:
          - 0.36
          - -0.28
          - 0.89
        color:
          - 0.55
          - 0.65
          - 0.9
        intensity: 1.4
```

(from `examples/gallery/night-sky.yaml` — the same roughness×metallic sweep as `material-park.yaml`,
under this sky: the metals reflect the gradient instead of going flat-black, see
`ludion docs materials --section "Metals under ambient-only light"`.)

## Fields

Every field is optional with a behavior-preserving default, so authoring only a sky changes nothing
else:

- **`sky_zenith` / `sky_horizon` / `sky_ground`** — linear RGB, HDR-capable (may exceed `1.0`); the
  gradient's color straight up, at the horizon, and straight down (the "ground" hemisphere under the
  horizon). Defaults are already a plausible night sky: a dim night blue (`[0.02, 0.03, 0.06]`), a
  slightly brighter dusk horizon (`[0.05, 0.06, 0.10]`), near-black underneath (`[0.01, 0.01, 0.02]`).
  Negative or non-finite components are `L1128`.
- **`sun_disc_size`** — angular diameter of a sun/moon disc drawn in the sky, in **degrees** (default
  `0.0` = no disc). The disc's *direction* is not authored here — it is read from the scene's
  `DirectionalLight` at render time, so it sits exactly where the sun/moon light comes from. A size
  authored with no `DirectionalLight` in the scene simply draws no disc. Set it to
  `2 × DirectionalLight.angular_radius` so the disc and the sun's specular highlight match.
- **`sun_disc_intensity`** — the disc's brightness, linear HDR, `>= 0` (default `1.0`); only relevant
  once `sun_disc_size > 0`.
- **`ambient`** — a flat ambient floor added to every lit surface (`ambient × diffuse` on meshes,
  `ambient` on lit sprites and tilemaps), linear RGB (default `[0, 0, 0]`). Under an `Environment`
  the sky's IBL *is* the ambient, so the default adds nothing; author this only for an extra floor
  beneath it. It is not scaled by `ibl_intensity`. It supersedes the engine's flat ambient
  (`[0.05, 0.05, 0.06]`, used when a scene has no `Environment`) the moment an `Environment` is
  present, so a lit sprite under an `Environment` with no sun or lights reads black unless you author it.
- **`ibl_intensity`** — an overall multiplier on the prefiltered sky's contribution to every lit
  surface, `>= 0` (default `1.0`).

## Sky authoring: a gradient, not a physical model

There is no time-of-day, turbidity, or physically-based atmosphere here — you author three colors,
the same discipline as palette authoring elsewhere in the engine. This is deliberate: turbidity-style
physical parameters are opaque to an agent choosing a look, and the sky is never an image (an HDRI
was considered and rejected as a binary asset smuggled in through the back door). The gradient is
night-capable by construction — a torch-and-moon-lit night district is the intended case, and the
defaults are already a plausible night sky — so a daytime scene is the same three fields with
brighter, warmer colors and a bigger, brighter disc; there is no separate "physical" mode alongside
the authored one.

Mechanically, the gradient eases from the horizon color toward the zenith above the horizon and
toward the ground color under it, and it is drawn as a fullscreen triangle at the far depth plane
**inside the existing scene pass** — after opaque meshes, before sprites, so alpha-blended sprites
composite over the sky rather than a flat clear color — filling only the pixels no opaque mesh wrote.
An `Environment`-less scene keeps the plain clear-color background, and the frame graph's pass list is
unaffected either way (`ludion devdocs ludion-renderer` has the graph detail).

## Image-based lighting: what the sky feeds

The same sky gradient is GPU-prefiltered — at load, and again on every live edit that changes a sky
color — into the inputs a lit mesh samples for its ambient term: a diffuse irradiance map and a
roughness-mipped specular map, both combined through the split-sum approximation, plus an
environment-independent BRDF lookup table. This is what fixes the flat-black metal case
(`ludion docs materials --section "Metals under ambient-only light"`): a mirror-metal reflects the sky
instead of nothing. `ibl_intensity` scales the whole result, and **no `Environment` means no IBL** —
the mesh shader takes the flat-ambient branch instead, byte for byte. `ludion devdocs renderer-ibl`
pins the exact math (texture sizes, sample counts, the mean-radiance convention, the cache key) for
anyone touching the shader side.

The sun/moon disc is drawn in the sky background only — it is **excluded from the prefilter**. Baking
it into the IBL inputs too would double-count it: once through the analytic `DirectionalLight`'s own
specular highlight (with its shadow), and again through the reflected sky. Keeping the disc
background-only avoids that entirely, at the cost of the disc itself never appearing in a mirror
reflection — accepted scope, not a bug.

An opt-in `GiVolume` attenuates this IBL term (and the flat ambient fallback) further, per world
position: a baked sky-visibility volume that dims an interior's fill without dimming an open-sky
surface. Its own fields, calibration, and dump instruments are `ludion docs global-illumination`.

## Occlusion fade: dither, top-fade, cutaway

`Fade` is a second, independent opt-in component — render-only, per-instance, and orthogonal to
`Environment` (a scene can carry either, both, or neither). It drives a deterministic screen-door
discard (a 4×4 Bayer dither) on the opaque mesh pipeline: no blend state change, no depth-write
change — some fragments of the instance simply aren't written.

```yaml
      Fade:
        fade: 0.35        # [0, 1]; 1.0 (the default) can never discard anything
        height_min: 3.0   # only fragments strictly ABOVE this world Y fade; omit to fade everywhere
```

- **`fade`** (default `1.0`) — the visibility knob. `1.0` can never discard a fragment (the Bayer
  matrix's highest threshold is `15/16`, and the rule is "discard when `fade <= threshold`"); `0.0`
  discards nearly every fragment.
- **`height_min`** (default `-f32::MAX`, i.e. every fragment is eligible) — a world-space Y cutoff:
  only fragments strictly *above* it fade, so a building's ground floor can stay solid while its
  upper stories dither away.

A mesh with no `Fade` of its own takes its nearest ancestor's through `Parent`, so fading a prefab's
root fades its parts.

`Fade` is deliberately **not** the `Mesh.color` alpha channel — that field stays a tint (unblended on
the opaque path) and `Fade` rides its own instance lane, so tinting and occlusion fade never fight
over one number. The engine never decides *who* fades or *when* — that is entirely game policy. The
three shapes a game reaches for are all just different `(fade, height_min)` pairs a system writes:

| Pattern | `fade` | `height_min` |
|---|---|---|
| Dither the whole building | `< 1` (e.g. `0.35`) | unset (`-f32::MAX`) |
| Top-fade the upper stories | small (e.g. `0.15`) | a story height (e.g. `3.0`) |
| Cutaway (near-total discard) | `≈ 0` (e.g. `0.04`) | unset (`-f32::MAX`) |

The copyable pattern — an `Occluder` marker naming its mode and one system that judges whether a wall
sits between the camera and the thing the player must see, then writes the matching `Fade` — is the
complete, runnable `crates/ludion/examples/occlusion_fade.rs` (`cargo run -p ludion --example
occlusion_fade`). A real game's occlusion *test* (which walls, with what hysteresis) is expected to be
a `ludion_spatial` raycast from the camera to the player; the `Fade` values it writes are exactly the
table above.

## What's not here

- **No texture maps or HDRI.** The sky is always these three authored colors; an image-based sky
  stays behind the textured-PBR trigger (`Plan/phase-11-pbr.md`).
- **No physical daylight model.** No time-of-day, no turbidity, no sun-angle-from-latitude — author
  the colors you want, like a palette.
- **No occlusion policy.** `Fade` is plumbing; deciding which entities fade, when, and with what
  hysteresis is the game's job — the engine only guarantees the dither is deterministic once you write
  the numbers.
- **Sprites, billboards, and world-space text stay flat-ambient**, untouched by IBL — mixing stylized
  flats with lit, reflective meshes is the intended HD-2D look, not a gap.

## Next

- `ludion docs lighting` — the sun and its shadow map.
- `ludion docs materials` — the three material scalars.
- `ludion docs atmosphere` — fog and light shafts as one participating medium.
- `ludion docs post-processing` — the imaging chain.
- `ludion docs reflection-probes` — captured reflections that replace the sky inside a volume.
