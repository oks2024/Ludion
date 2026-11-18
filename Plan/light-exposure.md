# Plan: light exposure — a sim-side answer to "how lit is this point"

> Status: **W1 mechanized (2026-11-18); W2 and W3 not started.** Neon Shroud request 0013
> (`engine-requests/0013-light-exposure-query.md`, read-only), audited at engine r2369. The game's stealth
> perception (request 0001) needs the authored light at a body's position from its own system, without
> a rendered frame. Related: `Plan/spot-light.md`, `Plan/shadow-mapping.md`, `Plan/physics.md`.

## Trigger

A stealth game decides "seen / hidden" from light. Reading the rendered image is neither deterministic
nor available headless, and re-deriving the light model in the game forks it from the renderer. The
engine answers from sim state, with the renderer's own formulas.

## Decisions

1. **CPU twin location.** `crates/ludion-renderer/src/light_model.rs`: pure functions, no wgpu types.
   `ludion-engine` already depends on `ludion-renderer`, and the drift test sits beside the WGSL it
   pins. Extract's `point_light_data`/`spot_light_data` call it for the clamps, so each formula exists
   once on the CPU. **Proposed, owner ruling owed** (the request's refactor decision). Additive; moves
   no pixel.
2. **Drift pin.** A textual, statement-for-statement test (`light_model_matches_the_wgsl`) in the style
   of `kept_world_from_depth_copies_match_the_snippet`: it asserts the WGSL still contains the lines the
   Rust twin mirrors. Numeric agreement is W3's gate.
3. **Light contract** (mirrors extract and the shaders).
   - The lights come from `world.entities()`; presentation lights never count.
   - A hidden light, or one with a hidden ancestor (the `Visibility` AND-fold), is absent.
   - The sun is the lowest-`EntityId` `DirectionalLight` and **ignores `Visibility`**, as `extract_sun`
     does (`docs/components.md`: "sun ... unaffected").
   - A point or spot light's position is `hierarchy::world_transform` translation, the sim pose; no
     `Transform` means absent.
   - `c = max(color, 0) · max(intensity, 0)`; `atten = clamp(1 − d / max(range, 1e-3), 0, 1)`; the cone
     is `spot_cone`; the sun has no attenuation and its colour is premultiplied.
   - Per light: `c · cone · facing · atten² · vis`.
4. **Facing.** `Omni` is `n·l = 1`, the body default the game prefers. `Normal(n)` is `max(n·l, 0)` with
   `n` normalised. `TwoSided` is out of scope (no consumer, Pillar 6).
5. **Ambient.** With no `Environment`, `DEFAULT_AMBIENT`. With one, `env.ambient + ibl_intensity · E`:
   for `Normal(n)` the 64-sample Hammersley cosine-weighted `sky_irradiance(n)` (mirrors
   `ibl.wgsl:85-101`), for `Omni` the sphere mean `(zenith + horizon + ground) / 3`. Not applied:
   `GiVolume` sky visibility and bounce, SSAO, specular, the env-branch energy split, emissive,
   `Mesh.wrap`, fog — documented as known differences. **Proposed, owner ruling owed** (GI in v1).
6. **Occlusion is opt-in and mirrors the renderer.**
   - Rays test only the sun and point/spot lights with `cast_shadows: true`; the camera budget and
     `shadow_distance` are ignored.
   - One ray per light per point from `p + l·bias`. Point/spot: `bias` is `shadow_bias`, length
     `d − bias − shadow_near`. Sun: `bias` is its `shadow_bias`, length `shadow_far − bias`. A length
     `≤ 0` casts no ray and nothing occludes.
   - The filter is the caller's `occluders` **as given**; no implicit `only_fixed`. The docs recommend
     `QueryFilter::new().only_fixed().exclude(subject)` or an `include_layer("light_occluder")`. The
     request's implicit static-only default is **Proposed, owner ruling owed**; W2's component default
     re-raises it.
   - A hit on an entity whose inherited `Visibility` is hidden or has `cast_shadows: false` is skipped;
     the first remaining hit occludes. No `PhysicsWorld` means nothing occludes.
7. **Combine across points.** `Max` (default) returns the point with the largest `total`, whole (its
   `lights` are that point's); ties go to the first in authored order. `Mean` returns the componentwise
   mean of `rgb` and of each light's `luminance`/`unoccluded` (over all points), lists every light that
   reached any point, and is `occluded` only if blocked at every point it reached. `total` is the
   Rec.709 luminance of linear `rgb`; `lights` is ascending `EntityId`.
8. **Summation order.** Ambient, point lights ascending, spots ascending, the sun: the renderer's order.
   Math goes through `ludion_core::math`; only `sqrt` and normalize are IEEE.
9. **Code.** `L4250 LIGHT_EXPOSURE_INVALID` (error): points empty, over 16 or non-finite
   (`/points/<i>`), or a zero or non-finite `Normal` (`/facing`). `L4251` (a presentation light in a
   scene with `LightExposure`) and `L4252` (a casting light whose filter matches nothing) are reserved
   for W2.
10. **Sim pose vs render pose.** The query uses the tick pose and the renderer the interpolated one, so
    they differ by up to one tick of motion. The docs say so.

## Waves

- **W1 — the function (engine).** `light_model.rs`, extract's clamps through it, `light_exposure`,
  `L4250`, `docs/light-exposure.md`, the facade example, tests incl. determinism across thread counts.
- **W2 — the component.** `LightExposure`/`LightExposureReading` components, an engine system, a
  profile key, check-scene warnings (`L4251`, `L4252`), a WorldHash trace.
- **W3 — the agreement gate.** `--debug-view irradiance`, a gallery scene, goldens pinned on both OSes
  and the rendered-vs-CPU agreement test. Owed to the pinned machines.

## Out of scope

- `Facing::TwoSided` — trigger: a lit-billboard consumer.
- GI sky visibility and bounce — trigger: an Area C reading that comes out wrong.
- Penumbra rays (soft-shadow sampling) — trigger: a sitting judges the hard answer unreadable.
- Render layers (request 0017).
- A per-light opt-out — rejected: "looks lit, isn't".

## Status

- W1 (2026-11-18): `ludion_renderer::light_model`, `light_exposure` and its types, `L4250`,
  `ludion docs light-exposure`, example `light_exposure`. Extract's point/spot clamps call the twin
  (bit-identical; existing tests unmodified). `sky_irradiance` falls back to `up = Y` at exactly `±X`
  where the shader's basis degenerates (its cube texels never land there; a query can). The renderer's
  GPU goldens are the Tech Lead's check that extract moved no pixel.
