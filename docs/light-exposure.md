---
description: light_exposure — how much authored light reaches a point, from sim state, with physics-ray shadows
---

# Light exposure: the renderer's light model, read from the sim

**Scope:** `light_exposure`, `ExposureQuery`, `Facing`, `Combine`, `Exposure`, `LightContribution`,
`ExposureError`, the `LightExposure` component, `light_exposure_update`, `LightExposureResults` and
`L4250`–`L4252`. The lights are `ludion docs lighting`; the sky is `ludion docs environment`;
the shadow rays use `ludion docs collision-layers` filters.

`light_exposure(&World, &ExposureQuery) -> Result<Exposure, ExposureError>` answers "how lit is this
body" from your own system (stealth, plant growth, a lit-or-dark verdict). It reads no pixel: the
falloff, cone, clamps and sky irradiance are the renderer's own formulas
(`ludion_renderer::light_model`), pinned to the WGSL by a drift test.

## The formula

| Term | Value |
|---|---|
| light colour | `max(color, 0) · max(intensity, 0)` (the sun: premultiplied) |
| range falloff | `clamp(1 − d / max(range, 1e-3), 0, 1)`, squared; exactly `+0.0` at the range |
| cone (`SpotLight`) | 1 inside `inner_angle`, Hermite ramp, exactly 0 at or past `outer_angle` |
| facing | `Omni`: 1. `Normal(n)`: `max(n·l, 0)` |
| ambient | no `Environment`: the default ambient. With: `ambient + ibl_intensity · E` |
| occlusion | one ray per casting light per point, 1 or 0 |

`E` is the sky's cosine-weighted irradiance about `n` for `Normal`, the sphere mean for `Omni`.
Contributions sum ambient, points, spots, then the sun; `total` is the Rec.709 luminance of `rgb`.

## What counts

- Sim entities' `PointLight`, `SpotLight` and the lowest-`EntityId` `DirectionalLight`. Presentation
  lights never count.
- A hidden light, or one under a hidden `Parent`, is absent. The sun ignores `Visibility`, as in the
  renderer. A point or spot light needs a `Transform`.
- Position is the tick pose; the renderer draws the interpolated one, so they differ by up to one tick
  of motion.

## Shadows are physics rays, opt-in per light

Only the sun and lights with `cast_shadows: true` cast a ray; the renderer's camera budget and
`shadow_distance` are ignored. The ray starts at `p + l·shadow_bias` and stops at the light (minus
`shadow_near`; the sun at `shadow_far`). The **filter is yours, as given**: nothing is added. Use

```rust
QueryFilter::new().only_fixed().exclude(subject)   // static scenery, never the subject
// or, with the scene's layers declared:
QueryFilter::new().include_layer("light_occluder")
```

A hit on an entity whose inherited `Visibility` is hidden or has `cast_shadows: false` is skipped. No
`PhysicsWorld` means nothing occludes. **Authoring condition:** every mesh that casts a gameplay shadow
carries a collider on an occluding layer (compare `local_shadows.skipped == 0` in a profile).

## Several points

Up to 16 points per query (`MAX_EXPOSURE_POINTS`). `Combine::Max` returns the brightest point's reading
whole; `Combine::Mean` averages `rgb`, `ambient` and each light's `luminance`/`unoccluded`, and marks a
light `occluded` only if blocked at every point it reached. `lights` is ascending `EntityId`, and holds
the lights that reached a point (`unoccluded > 0`).

## Known differences from the rendered frame

Not applied: penumbra, `GiVolume` sky visibility and bounce, SSAO, specular and the energy split,
emissive, `Mesh.wrap`, fog. The sim pose differs from the render pose by up to one tick.

## The agreement gate

`ludion render --scene examples/gallery/light-exposure.yaml --debug-view irradiance` draws every opaque
`Mesh` fragment as the diffuse irradiance the renderer summed there (albedo 1: ambient/IBL, local lights
and the sun, each with its shadow; no specular, emissive or post), stored as `sRGB(E / 4)`, so it saturates
at `E = 4`. `render.debug {"view": "irradiance"}` sets it too (`ludion docs wire-render`).
The engine test `light_exposure_agreement` renders that scene at 512×512 and compares a 32×32 grid of
floor points with `light_exposure(Facing::Normal([0, 1, 0]))`:

- A sample is clean unless a neighbour 0.25 m away differs on any light's reached/occluded status, it
  lies in a spot's ramp band (inner − 1° to outer + 1°), or it is within 0.25 m of the wall. Clean samples
  agree within `max(5 %, 0.01)`.
- The lit verdict (`total ≥ 0.25`) agrees on ≥ 99 % of all samples and on every sample whose neighbours
  share its verdict.
- The tolerances are fixed, not a per-OS floor: both sides run one formula.

**Authoring condition:** keep the floor's irradiance below 3.5, and give each occluding mesh a collider.

## Errors

`L4250` (`ludion explain L4250`): no points, over 16, a non-finite point (`/points/<i>`), a zero or
non-finite `Normal` (`/facing`). A filter naming an undeclared layer is `ExposureError::Query`.

## Scene-authored path

```yaml
- name: guard_target
  components:
    Transform: { translation: [0, 0, 0] }
    LightExposure:
      points: [[0, 0.3, 0], [0, 1.6, 0]]   # entity-local; default [[0, 0, 0]]
      facing: { normal: [0, 1, 0] }        # world-space; default omni
      combine: mean                        # default max
      occluder_layers: [light_occluder]    # absent: static colliders only
```

Schedule `.system("light_exposure_update", light_exposure_update)` **after physics and after whatever
moves lights and subjects**, before the systems that read. Each tick it rebuilds `LightExposureResults`
(a derived resource: not hashed, not snapshotted); read it with `results.reading(entity)` (`None` for no
`Transform`, a faulted subject, or no component), `results.readings()` (ascending `EntityId`) and
`results.tick()`. The system's profile time shows under its own name.

- Points are entity-local offsets mapped through the world pose (rotation, scale, translation); posture
  is the game rewriting `points`.
- `occluder_layers` absent: static colliders only, the subject excluded. Present: exactly those layers
  (declared under `physics.layers`), the subject excluded.
- A fault is reported once per subject and gives it no reading: an undeclared layer is `L1186`
  (`/occluder_layers`), a malformed field `L4250`.
- `check-scene` warns `L4251` for a light on a `presentation: true` entity (drawn, never counted) and
  `L4252` for `occluder_layers` that no 3D collider carries (`Collider.layer`, `default`, or
  `QueryLayers`).

## Example

```rust
use ludion::prelude::*;

fn lit(world: &World, body: EntityId, at: [f32; 3]) -> bool {
    let filter = QueryFilter::new().only_fixed().exclude(body);
    let query = ExposureQuery {
        points: &[at],
        facing: Facing::Omni,
        occluders: &filter,
        combine: Combine::Max,
    };
    light_exposure(world, &query).map_or(false, |x| x.total > 0.3)
}
```

Runnable, with the authored component too: `crates/ludion/examples/light_exposure.rs`.
