---
description: Index of the look topics: materials, lighting, environment, GI, telegraphs, particles, post
---

# Visuals

**Scope:** the index for everything that decides how a scene *looks* — the surfaces, the lights and
sky that fall on them, the effects placed on top, and the imaging chain applied to the finished
frame. Each entry below is its own topic; pull the one covering what you are about to change. The
components you place and the systems that drive them are `ludion docs components` and
`ludion docs systems`.

- `ludion docs materials` — `roughness`/`metallic`/`emissive`, and which surface authors them
  (voxel and sculpt palettes included).
- `ludion docs mesh-editing` — painting a mesh's emissive map with a verified, replayable region op.
- `ludion docs texture-recipes` — procedural tileable PBR maps from a `kind: texture` recipe, the `std:` library, `ludion texture`.
- `ludion docs texture-ops` — every texture op with its fields, defaults and ranges.
- `ludion docs lighting` — the `DirectionalLight` sun and its shadow map, and point-light density.
- `ludion docs environment` — the `Environment` sky, image-based lighting, and `Fade` occlusion.
- `ludion docs global-illumination` — the baked `GiVolume` sky-visibility volume and its dump instruments.
- `ludion docs ground-telegraphs` — the `GroundMarker` disc/segment ground telegraph.
- `ludion docs debug-draw` — world-space debug lines a system draws, and `--debug-draw` on the offline renders.
- `ludion docs particles` — `ParticleEmitter`: the analytic particle effect, and firing a burst.
- `ludion docs sprites` — the sprite layer's hub: document kinds, CLI verbs, components, six spokes.
- `ludion docs sprite-authoring` — the `kind: sprite` glyph grid, palettes, frames, and the contact sheet.
- `ludion docs sprite-effects` — the eight baked effect ops, their parameters, and their traps.
- `ludion docs sprite-sheets` — packing frames into a committed `kind: atlas`, and drawing one region.
- `ludion docs sprite-animation` — playing an atlas flipbook on a `Sprite` with `Animation`.
- `ludion docs post-processing` — the pinned imaging chain and every `PostProcess` field.
- `ludion docs atmosphere` — fog and light shafts as one participating medium.

## Which plane is 2D?

**Drawing has one fixed plane; collision's is a builder choice.** A non-billboard `Sprite` is a quad
spanned by its `Transform`'s local +X and +Y, facing +Z — what a camera looking down its own −Z sees
square-on — and a `Tilemap` lays cells the same way, column **+X**, row **−Y**. Collision and every
`spatial` query instead run on whichever two axes `Engine::builder().plane(...)` names
(`ludion docs collision`): the default `Plane::Xz` is the ground plane a top-down or 3D-eyed game
moves on, and `Plane::Xy` is the plane sprites and tilemaps already draw on.

A sprite game — one whose whole play field is drawn, not walked on in 3D — sets
`Engine::builder().plane(Plane::Xy)` once, so `Collider`, `spatial`'s four calls, and a `Volley`'s
launch angle (`ludion docs patterns`) all live in the same plane the sprites do: no rotating sprites
−90° to face a top-down camera, no separate overlap arithmetic to keep in sync with what's drawn. A
ground-plane game (a pitched or free 3D camera looking down at x/z) keeps the default `Plane::Xz` and
needs no builder call at all. Authoring content in one plane while the builder names the other is
the failure mode: every collider collapses onto one line, or a volley fires perpendicular to the
field it was drawn on.
