---
description: Decal — projected textured marks on opaque surfaces, blend modes, receivers, draw order
---

# Decals: `Decal`

**Scope:** `Decal` — an oriented box that projects a textured footprint onto the opaque surfaces
inside it: paint, stripes, scorch, wet patches, glowing marks. The field table with defaults is
`ludion docs components`; the contact shadow under a character is `ludion docs drop-shadow`; the flat
analytic marker is `ludion docs ground-telegraphs`; the map block is `ludion docs materials`.

## Format

```yaml
- name: gap-lip-stripe
  components:
    Transform:
      translation: [0.0, 1.0, -14.6]   # box centre
      rotation: [-45.0, 0.0, 0.0]      # tilted: one box paints a top and the lip face below it
    Decal:                             # every field optional; `Decal: {}` is a 1 m box of white lit paint
      size: [2.4, 0.6, 0.6]            # x by z footprint, y projection depth (metres, times world scale)
      base_color_texture:
        path: textures/hazard-stripes.png
      blend: alpha                     # alpha | multiply | add | coat
      angle_fade: [60.0, 80.0]
```

| Field | Default | Code |
|---|---|---|
| `size` | `[1, 1, 1]` × world scale | `L2106` |
| `color` | `[1, 1, 1, 1]` (HDR for `add`) | `L1263` |
| `opacity`, `metallic` | `1`, `0` | `L1014` |
| `roughness` | the receiver's own | `L1014` |
| `uv_rect` | `[0, 0, 1, 1]` | `L1014` |
| `angle_fade` | `[60, 80]` degrees, `0 <= start <= end <= 90` | `L2112` |
| `blend`, `layer`, `receivers` | `alpha`, `0`, `all` | — |

Maps: `base_color_texture` (sRGB; RGB × `color`, A is coverage), `normal_texture` and
`metallic_roughness_texture` (`alpha` and `coat` only), each the map block of `ludion docs materials`.
A missing or unreadable file is `L1101`/`L1102` at the map path, which `check-scene` reports. Normal-map convention: red = box +X,
green = image up = box −Z.

## Rules

- **The box.** Placed by the entity's world transform through the `Parent` hierarchy, projecting
  along local −Y: unrotated, it paints floors. Map u runs along +X, v along +Z. The box clips hard.
- **Angle fade.** Let θ be the angle between the receiving normal and the box's +Y: weight 1 up to
  `start`, 0 from `end`, smooth between; a surface facing away gets nothing.
- **Blend modes.**
  - `alpha`: lit paint, shaded with the receiver's terms: the sun and its shadow, clustered lights
    with their shadows, IBL diffuse, GI, AO.
  - `multiply`: an unlit darkening (scorch, grime).
  - `add`: an HDR add that blooms.
  - `coat`: the `multiply` darkening plus a dielectric specular lobe (F0 0.04 at the coat's
    roughness); it replaces the receiver's environment specular. Wet patches, varnish.
  - Lit decals (`alpha`, `coat`) show probe, sky and SSR reflections at their own roughness and
    normal. At quality `low` they lose the sun shadow.
- **Order.** Lit decals first (the `decals_lit` pass, before `reflections`), then `multiply` and `add`
  (the `projector` pass), then every `DropShadow`. Within a group, ascending `layer`; within a layer
  the lowest EntityId draws last and wins. A `multiply` scorch therefore darkens paint even at a
  lower layer; the drop shadow darkens paint under the hero.
- **Receivers.** Every opaque mesh kind. Never the sky, sprites, particles, text, ground markers or
  skinned meshes (the default). An `all` decal carries no receiver list. Whenever an `all` decal
  shares a scene with skinned meshes, the `decal_receivers` id pass redraws each skinned mesh once per
  frame, so its cost grows with the skinned count, not with decals × skinned. `receivers: hierarchy`
  limits a decal to its own root and the root's descendants and adds its own meshes to that list. A
  1–2 px contact rim can show.
- **Hash-neutral and graph-neutral.** Render-only. A scene with no `Decal` and no `DropShadow` keeps
  its exact graph; lit decals add `decals_lit` and the `lit_normal` transient; unlit ones ride
  `projector`.
- **No cap, no silent drop.** `ProfileReport.decals` counts alpha, multiply, add and coat decals, the
  batched `draws` and `receiver_meshes`; it is absent without decals. The gallery `decals` scene
  (RTX 3090, release): 11 alpha, 1 multiply, 1 add, 1 coat, 26 draws, 2 receiver meshes; the decal
  passes cost about 0.06 ms of a 0.56 ms pass sum.

## Cost and lifetime

- **No frustum cull.** Every decal is packed and drawn each frame, on screen or not.
- **Indicative cost** (unpinned M5): 1,000 multiply decals beside 500 skinned enemies add about
  0.2 µs of GPU frame at the crowd view (0.03 µs facing a wall) and 0.35 µs of extract per decal.
  Figures and method: `Plan/gpu-driven-culling.md` Status, "The decal room bench".
- **Lifetime recipe.** Kill decals are presentation entities, spawned and despawned by a
  *presentation* system (a sim system doing either is `L0028`; `ludion docs presentation`): the sim
  records the kill, the presentation system reads it and spawns the decal. Despawn them on room exit.
  The game keeps its own oldest-first cap and picks the number from the cost line. The engine never
  caps and never drops.
- **The check.** `ProfileReport.decals` (`multiply`, `receiver_meshes`).

## Gotchas

- **Enclose the receiver.** A box face exactly coplanar with the receiving surface vanishes at some
  views: centre or straddle the box on the surface. The clip is hard by design.
- **Probe captures contain no decals.**
- **Scale is the world scale.** `size` multiplies the entity's world scale per axis.
- **A black surface shows no `alpha` paint.** It is lit like the receiver.

## Try it

`examples/gallery/decals.yaml`: a stripe across a step, a wall decal, paint half in a sun shadow, a
coat puddle, a glossy swatch, a scorch over paint and a crate, an add line, an equal-layer overlap, a
clean skinned character, a `hierarchy` decal, a drop shadow over paint and a ground marker over a decal.

```
ludion render --scene examples/gallery/decals.yaml --out decals.png
ludion profile --scene examples/gallery/decals.yaml
```

## Next

- `ludion docs components` — the built-in visual set.
- `ludion docs drop-shadow` — the projected contact shadow that draws after every decal.
- `ludion docs materials` — the map block and sRGB/linear roles.
- `ludion docs ground-telegraphs` — the flat marker that draws on top of a decal.
