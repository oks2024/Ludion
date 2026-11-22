# Plan: light exposure — a sim-side answer to "how lit is this point"

> Status: **W1 and W2 mechanized (2026-11-18); W3 mechanized on Windows (2026-11-19); macOS twins owed.** Neon Shroud request 0013
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
   once on the CPU. **accepted by the owner 2026-11-19** (the request's refactor decision). Additive; moves
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
   `Mesh.wrap`, fog — documented as known differences. **accepted by the owner 2026-11-19** (GI in v1).
6. **Occlusion is opt-in and mirrors the renderer.**
   - Rays test only the sun and point/spot lights with `cast_shadows: true`; the camera budget and
     `shadow_distance` are ignored.
   - One ray per light per point from `p + l·bias`. Point/spot: `bias` is `shadow_bias`, length
     `d − bias − shadow_near`. Sun: `bias` is its `shadow_bias`, length `shadow_far − bias`. A length
     `≤ 0` casts no ray and nothing occludes.
   - The filter is the caller's `occluders` **as given**; no implicit `only_fixed`. The docs recommend
     `QueryFilter::new().only_fixed().exclude(subject)` or an `include_layer("light_occluder")`. The
     request's implicit static-only default is left out, **accepted by the owner 2026-11-19**; W2's component default
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
11. **Readings are a derived resource, not a hashed component.** Mirrors `PerceptionResults`:
    `light_exposure_update` rebuilds `LightExposureResults` every tick from hashed state, so it is not
    hashed and not snapshotted. One pattern for derived sensor state, no snapshot or schema field, and
    the replay is identical by construction. The request's hashed `LightExposureReading` component is
    left out, **accepted by the owner 2026-11-19**; trigger: a trace or HUD that must show the reading inside the hash.
12. **The component's default occluders.** `occluder_layers` absent means static colliders only, the
    subject excluded (`QueryFilter::new().only_fixed().exclude(subject)`); present means
    `include_set(layers).exclude(subject)`. The request's default, applied at the authored layer; the
    function keeps "the filter as given" (decision 6). **accepted by the owner 2026-11-19**, with decision 6.
13. **One type per concept.** `Facing` and `Combine` live in `ludion-components` with serde and
    `rename_all = "snake_case"` (`omni` | `{ normal: [x,y,z] }`, `max` | `mean`); `ludion_engine`
    re-exports them, so `ludion_engine::Facing` and the prelude are unchanged.
14. **Points are entity-local offsets**, mapped through the subject's exact world affine (rotation,
    scale, translation, shear kept). A subject with no `Transform` has no reading. `facing.normal` is
    world-space, like `SpotLight.direction`. Posture is the game rewriting `points`.
15. **Runtime faults** go through `report_full`, keyed by subject, latched: an undeclared
    `occluder_layers` name is `L1186` at `/occluder_layers`, and the subject has no reading that tick.
16. **Codes.** `L4251` (warning): a light on a `presentation: true` entity in a scene with any
    `LightExposure`, "drawn but never counts". `L4252` (warning): `occluder_layers` that no 3D-claimed
    collider carries (`Collider.layer`, `default` when absent, or `QueryLayers`). An undeclared name
    reuses `L1186`; field validation reuses `L4250`. This replaces decision 9's reading of `L4252` as a
    casting light whose filter matches nothing: the function takes a caller's filter, so only the
    authored layers can be checked.
17. **The `irradiance` view.** `DebugView::Irradiance` (`ludion render --debug-view irradiance`): every
    opaque `Mesh` fragment writes the diffuse irradiance the mesh path summed — lighting with albedo 1,
    ambient/IBL, local lights and the sun each with its shadow — with no specular, emissive, wrap, rim,
    flash or post, stored as `sRGB(E / 4)` (`IRRADIANCE_VIEW_SCALE`). It saturates at `E = 4`. It is a
    pipeline-constant twin of the mesh scene pass (`IRRADIANCE_VIEW`, as `sun_cascades` is) and
    `irradiance_debug` blits the `scene` target in place of `composite`; the normal path is untouched.
    Blended meshes, sprites, decals, particles and the sky draw as usual.
18. **Not on the wire.** A `RenderDebugView` variant feeds `replay_schema_hash`, which every committed
    `.lrp` carries, so adding one re-records the corpus. The view is renderer, Rust and CLI only:
    `render.debug` reports no view for it. **accepted 2026-11-19, scheduled (R14 re-record round)**, trigger: the next sanctioned
    corpus re-record.
19. **The gate's tolerances.** The request's, fixed constants, not a per-OS calibrated floor: both sides
    run one formula, so a floor would hide drift. A 32×32 grid of floor points of the gallery's
    `light-exposure` scene, rendered under the view at 512×512 (one sample per 16 px, half a metre),
    against `light_exposure` with `Facing::Normal([0, 1, 0])`. A sample is **clean** unless one of its
    eight neighbours at 0.25 m differs on any light's reached/occluded status, it lies in a spot's ramp
    band (inner − 1° to outer + 1°), or it is within 0.25 m of the wall: clean samples agree within
    `max(5 %, 0.01)`. The lit/unlit verdict (`total ≥ 0.25`) agrees on ≥ 99 % of all samples and on every
    sample whose neighbours share its verdict. The gate asserts ≥ 400 clean, ≥ 50 lit and ≥ 50 unlit
    samples, and no saturated sample. **Authoring condition:** the scene's floor irradiance stays below 3.5.

## Waves

- **W1 — the function (engine).** `light_model.rs`, extract's clamps through it, `light_exposure`,
  `L4250`, `docs/light-exposure.md`, the facade example, tests incl. determinism across thread counts.
- **W2 — the component (shipped).** `LightExposure`, `light_exposure_update`, `LightExposureResults`,
  check-scene warnings (`L4251`, `L4252`), a WorldHash trace test. The hashed `LightExposureReading`
  stays left out (decision 11).
- **W3 — the agreement gate (shipped on Windows).** `--debug-view irradiance`, a gallery scene, goldens
  pinned on both OSes and the rendered-vs-CPU agreement test. The macOS goldens and the test's M4 run are
  owed to the M4.

## Out of scope

- `Facing::TwoSided` — trigger: a lit-billboard consumer.
- GI sky visibility and bounce — trigger: an Area C reading that comes out wrong.
- Penumbra rays (soft-shadow sampling) — trigger: a sitting judges the hard answer unreadable.
- Render layers (request 0017; `Plan/secondary-views.md` W2).
- A per-light opt-out — rejected: "looks lit, isn't".

## Status

- W1 (2026-11-18): `ludion_renderer::light_model`, `light_exposure` and its types, `L4250`,
  `ludion docs light-exposure`, example `light_exposure`. Extract's point/spot clamps call the twin
  (bit-identical; existing tests unmodified). `sky_irradiance` falls back to `up = Y` at exactly `±X`
  where the shader's basis degenerates (its cube texels never land there; a query can). The renderer's
  GPU goldens are the Tech Lead's check that extract moved no pixel.
- W2 (2026-11-18): `LightExposure` (sim-only, appended after `SightTarget`), `Facing`/`Combine` moved to
  `ludion-components`, `light_exposure_update` and `LightExposureResults`, `scene_light_exposure_diagnostics`
  (`L4251`, `L4252`, `L1186` reuse), docs and example extended. Tests: validator, YAML scenes through
  the engine (pose mapping, default and authored occluders, a latched fault, byte-identity without the
  component), and a 60-tick trace identical at 1/2/8 threads and across snapshot/restore. No trace,
  replay, golden or wire-schema hash moved.
- W3 (2026-11-19): `DebugView::Irradiance` (renderer, engine and CLI; not on the wire, decision 18), the
  `light-exposure` gallery scene with goldens `light-exposure.png` and `light-exposure-irradiance.png`
  blessed on the RTX 3090, and `light_exposure_agreement.rs`. On this box: clean 696 of 1024, max relative
  error 0.0167, max absolute error 0.0099, binary agreement 100 % (864 of 864 steady samples). Owed: the
  M4 twins of both goldens and the agreement test green there; the wire variant (decision 18).
- M4 (2026-11-22, Lore 2449 verified on the pinned Apple M4): `light_exposure_agreement` is green on Metal, and both macOS goldens pass.
