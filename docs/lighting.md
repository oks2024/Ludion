---
description: The DirectionalLight sun and its shadow frustum, plus placing point lights by the hundred
---

# Lighting

**Scope:** The `DirectionalLight` sun and its shadow map, how many `PointLight`s a scene may place
(any number) with what that costs, and calibrating one. The sky and image-based lighting are
`ludion docs environment`; a `PointLight`'s field defaults live in `ludion docs components`; the
surfaces being lit are `ludion docs materials`; imaging is `ludion docs post-processing`.

## Sun and shadows

`DirectionalLight` is the "sun": one global, parallel-direction light that casts a single
orthographic shadow map, received by the mesh path only. It coexists with `PointLight`s (a scene can
have both — see `examples/gallery/sun-shadows.yaml`), but it is a different component with different
rules, not a `PointLight` variant.

```yaml
  - name: sun
    components:
      DirectionalLight:
        direction:
          - 0.5
          - -0.85
          - -0.3
        color:
          - 1.0
          - 0.96
          - 0.84
        intensity: 2.2
```

Drop that into a scene holding a `plane` ground and a `cube` above it and the cube throws a shadow
across the ground — no other setup needed. A scene with **no** `DirectionalLight` never schedules the
shadow pass, and its frame graph is byte-identical to one with no shadow support at all (the same
conditional-pass pattern as `ludion docs screen-ui`'s overlay).

**Direction, not rotation.** `direction` is an explicit `[x, y, z]` field, not the entity's
`Transform` rotation — a hand-authored vector reads far more legibly in YAML than a quaternion, and
stays agent-legible as plain numbers. It need not be unit length (normalized at extract). A
zero-length or non-finite `direction` is rejected by the semantic validator as `L1123` — give it a
real vector, e.g. the default `[-0.3, -1.0, -0.2]` (a slightly angled overhead sun; a straight-down
sun hides its own shadows directly beneath objects). `L1123` also covers a `shadow_near` that is not
below `shadow_far` (an absent bound counts as its default). The other typos are caught too: a
negative `intensity`, a `PointLight.range` or `shadow_extent` of 0 or less, and a negative
`shadow_bias` are `L1014` at the field, and a negative light `color` component is `L1263` — a
negative colour subtracts light.

**`Transform.translation` anchors the shadow frustum, not the light's origin.** A directional light
has no position physically — but the entity's `Transform.translation` sets the *center* of the
orthographic shadow frustum (a box `shadow_extent` units to each side, from `shadow_near` to
`shadow_far` along `direction`). A static scene can leave it at the origin; a game that needs the
shadow window to follow the action (a large level, a moving player) parents the sun entity to the
player so the frustum center tracks it. There is no automatic scene-fit — the frustum is exactly
what these fields say, deterministically, every frame.

**Shadow tunables**, all explicit fields with defaults tuned against the gallery scene:

- **`shadow_extent`** (default `40.0`) — the orthographic half-size, in world units. Too small clips
  shadow-casting geometry outside the frustum (it simply casts no shadow); too large spreads the
  fixed 2048² map's texels over more world space, softening shadow edges and inviting acne.
- **`shadow_near`/`shadow_far`** (default `1.0`/`200.0`) — the frustum's near/far planes along
  `direction`, centered on `Transform.translation`. A negative `shadow_near` is valid — a symmetric
  `-100`..`100` range brackets the anchor's own plane — as long as it stays below `shadow_far`.
- **`shadow_bias`** (default `0.05`) — the depth bias applied when sampling the shadow map, to fight
  acne (self-shadowing artifacts) at the cost of some peter-panning (a shadow detached slightly from
  its caster) if pushed too high. It is in **world units** at a 2048² map, independent of
  `shadow_near`/`shadow_far`, so widening the depth range never changes how far a shadow lifts off
  its caster. The shadow pass adds a slope-scaled bias on top (it follows the map's texel and the
  surface's grazing angle), which is why the constant can stay small. `0.05` reads clean — no acne, no
  peter-panning — on the gallery scene and on a lit night street; only override it if your content shows
  one of those artifacts. A quality that shrinks the map (`ludion docs quality`) scales the bias up
  with the texel, so `medium`'s 1024² map samples at twice it. A caster thinner than the bias and
  held that close to a surface casts no shadow on it, and a wide `shadow_extent` coarsens the texel,
  and with it the slope bias: shrink the extent for fine props. A vertical caster under a high sun
  lifts its shadow off its base by about `(shadow_bias + 1.5·tan(θ)·texel)·cos(e)` minus its own
  thickness, with `texel` = `2·shadow_extent / 2048`, `θ` the angle between its face's normal and
  the light and `e` the sun's elevation: a thin wall under the default 70° sun at
  `shadow_extent: 96` lifts about 0.12 world units (measured), and `shadow_extent: 40` brings that
  to about 0.05.

**Multiple suns: lowest `EntityId` wins.** Only one `DirectionalLight` is ever active — if a scene
has more than one, extract deterministically picks the lowest-`EntityId` entity and ignores the
rest (no error).

**What does *not* cast or receive a shadow**, by settled scope: sprites, billboards, and text neither
cast into the shadow map nor sample it (a lit sprite takes the sun's light unshadowed, two-sided, with
no IBL, beside its ambient and point lights);
`PointLight`s remain unshadowed — no cube-map/array-texture shadows, a recorded trigger revisited
when the sun alone can't sell a scene. A mesh always casts and receives; there is no per-mesh opt-out.

## Point lights: place as many as the look needs

A `PointLight` is a positioned, omnidirectional light — `color`, `intensity`, `range`, its position
taken from the entity's `Transform`. **There is no cap and no selection.** Every `PointLight` a scene
places is lit by, whether the scene places three or three hundred: nothing is silently dropped, and
nothing is swapped in and out as the camera moves. A torch on every post, a lantern in every
interior, and a light on every projectile are all ordinary content.

**Calibrating one.** The shading is `color · intensity · n·l · atten²`, where
`atten = clamp(1 − distance/range, 0, 1)` and, with no `Environment`, the flat ambient is only
`[0.05, 0.05, 0.06]` (under an `Environment` it is the sky's IBL plus `Environment.ambient`) — so a lit
surface with no light within `range` reads near-black. Two consequences worth knowing before you bless a golden:

- **`range` is a hard cutoff, and falloff is quadratic.** Brightness scales as `(1 − distance/range)²`
  and reaches exactly zero at `range`. Set `range` comfortably past the farthest surface you want lit
  (roughly 2–3× the light-to-surface distance) or the edges of the scene go dark.
- **`intensity` around `2`–`4` fully lights a small room.** The gallery scenes are the reference: a
  single white light ~3.5 units from the surfaces uses `intensity: 3.0`, `range: 14.0`
  (`examples/gallery/sprites.yaml`, `examples/gallery/tilemap.yaml`); the mesh scenes push
  `intensity: 6.0`, `range: 22.0` for a larger set. Values above ~1 drive colors past `1.0`, which is
  intended — that overflow is what feeds the bloom pass. Don't start at `intensity: 0.5` for a room
  the size of a Sokoban level; you'll under-light it and spend bless rounds discovering that.

The cost is proportional to the lights **near each pixel**, not to how many the scene holds. The
renderer divides the frame into cells and a fragment iterates only the lights whose `range` sphere
reaches its own cell, so a light on the far side of the district is free to the pixels it does not
touch. Two things follow for authoring:

- **Density is a look and budget question, never a validity one.** No diagnostic rejects a light
  count, and none will — measure the scene instead of rationing lights against a number nobody
  measured.
- **`range` matters more than count.** Range is what decides how much of the screen a light covers,
  so twenty torches at `range: 10` are far cheaper than twenty at `range: 60` that all overlap the
  same rooftops. A light whose range reaches much further than it visibly lights pays the full cost
  of that reach for nothing — trim `range` before you trim lights.

**Check it, don't guess.** One profile run answers both halves:

```
ludion profile --scene scenes/district-plaza.yaml --frames 120 --json target/profile.json
```

`passes.scene` is what shading those lights cost. The report's `cluster` section is the light path's
own panel: `total_refs` (how many light-in-cell entries the frame produced), `max_list_len` (the
longest list any fragment walks), `mean_list_len`, and `occupied_cells` against `cells` (the rest is
frame that pays nothing). Read them as a pair — a `scene` pass that grew while occupancy stood still
means the shading got more expensive, and one that grew alongside `total_refs` means the lights
themselves spread further across the screen.

For scale, on the pinned RTX 3090 at 1080p: `examples/gallery/lights-stress.yaml` — 500 lights, 200
static torches at ranges 8–15 plus 300 projectile-like movers — medians a **0.69 ms** `scene` pass
with 17,121 refs over 794 of 2,304 cells and a longest list of 74, against **0.061 ms** for the
three-light `examples/gallery/lights.yaml`. Eleven times the pass for 167× the lights. Your own
hardware will differ; the ratio is the point.

## Gotchas

- **A point light shines through walls.** Point lights are unshadowed (see above), so an interior
  lantern spills into the street and a street torch reaches inside. Placement and `range` are the
  tools: pull the range in to the room it is meant to fill, and prefer several short-range lights to
  one long-range one when a wall sits between them.
- **The shadow pass is genuinely conditional, and cheap when it runs.** A sunless scene skips it
  entirely (the mesh shader still binds a zeroed sun uniform and a 1×1 fallback map, whose cost is
  negligible and whose shadow lookup is never reached). On the pinned RTX 3090's budget-horde profile
  scene, adding one sun cost the GPU **`shadow` pass ≈0.084 ms** plus **≈0.070 ms** on the `scene`
  pass (shadow sampling + the sun's BRDF term) — about 0.15 ms total, against a ~0.85 ms sim-frame
  budget.
- **`shadow` is a recognized `ludion profile` pass name.** A `[passes] shadow` budget line is legal
  even on a scene that never triggers the pass (producible-but-unmeasured, not a violation).

## Next

- `ludion docs environment` — the `Environment` sky, IBL, and the `Fade` occlusion dither.
- `ludion docs materials` — the three material scalars.
- `ludion docs post-processing` — the imaging chain.
- `ludion docs components` — declaring components, and the built-in visual set.
