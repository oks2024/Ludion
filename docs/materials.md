---
description: Material scalars, author surfaces, precedence, texture maps, cutout, double-sided, emissive bloom
---

# Materials

**Scope:** How every mesh-bearing surface shades: `roughness`, `metallic`, `emissive`, where each may be authored, and which surface wins. The lights that hit them are lighting; what the post stack then does to the image is post-processing.

Every mesh-bearing surface — a primitive, an imported asset, a voxel model — shades through the same
Cook-Torrance point-light + ambient BRDF, driven by three scalars, each optionally modulated by a
texture map (below). A material is always exactly these three numbers, flat or sampled per-pixel.

- **`roughness`** — perceptual, `[0, 1]`. `0.0` is mirror-smooth (a tight, bright highlight); `1.0`
  is fully matte (a broad, dim one). Default `1.0` — deliberately the closest the GGX lobe gets to
  the engine's pre-PBR Lambert look, so content that authors no material moves the least.
- **`metallic`** — `[0, 1]`. `0.0` is a dielectric (a colored diffuse lobe plus a faint, colorless
  highlight); `1.0` is a metal (no diffuse lobe, and the highlight tints to the surface's own color).
  Default `0.0`.
- **`emissive`** — `>= 0`, an HDR intensity multiplier on the surface's **own** color — never a
  separate RGB. `2.0` on a `[0.4, 0.8, 1.0]` surface glows that same cyan twice as bright, not some
  other color. Default `0.0` (no glow).

## The three author surfaces

The same three fields exist in three places, always named and ranged identically:

1. **`Mesh` component fields** — `roughness`, `metallic`, `emissive` (the built-in component table is
   `ludion docs components`). The only surface for a primitive (`cube`/`plane`/`sphere`) or a plain
   imported asset mesh, since neither has per-vertex or per-palette-entry material of its own.
2. **Voxel palette entry keys** — optional `emissive`/`metallic`/`roughness` alongside `color`, one
   value per glyph (every quad of that color gets it). See
   `ludion docs voxel-format --section Palette`.
3. **`.mesh.yaml` per-vertex lists** — optional `emissive:`/`metallic:`/`roughness:` flat lists, one
   float per vertex, alongside `positions`/`normals`/`uvs`, for a hand-authored mesh asset that wants
   per-vertex material (`ludion devdocs scene-kind-documents --section "Mesh documents (\`kind: mesh\`)"` has the exact format).
4. **Sculpt palette entry keys** — the same shape as a voxel palette entry, one per part's
   `material` glyph. Unlike the other three surfaces, a sculpt vertex's material is a **weighted
   blend** of the parts that reach it — the smooth-union share each part won at that point, never a
   selection — so a smoothed seam between two glowing and matte parts fades rather than snaps. See
   `ludion docs sculpt-format`.

## Precedence: selection, not blending

**A mesh that authors any of its own material — any palette-entry field, any mesh-doc material
list — uses it entirely; the `Mesh` component's fields apply only to a mesh that authors none, and
are otherwise inert.** There is no partial mix *between surfaces*: a voxel model with one palette
entry carrying `metallic: 1.0` ignores `Mesh.roughness`/`metallic`/`emissive` for every quad,
including the ones whose palette entry authored nothing (those quads fall back to the default
material `1.0/0.0/0.0`, not the component's values). Unlike `color`, which always multiplies (vertex × instance),
metallic's default (`0.0`) is multiplication's absorbing element, so multiplying would make either a
component-driven sweep or a palette-driven metal inexpressible. Pick **one** surface per mesh and author the whole material there. (A sculpt's own palette surface blends *within*
itself, across its parts, as noted above — that blend is internal to the one surface a sculpt
authors, not a mix with `Mesh`'s fields.)

## Texture maps

A `.mesh.yaml` asset mesh (`Mesh.shape: asset`) may reference up to five PNG texture maps, each
modulating one of the terms above:

- **`base_color_texture`** — sRGB. Multiplies the vertex/instance albedo (`color`).
- **`metallic_roughness_texture`** — linear, glTF's packing: `G` = roughness, `B` = metallic. Each
  channel multiplies the corresponding scalar.
- **`normal_texture`** — linear. Replaces the geometric normal outright (via a tangent-space TBN
  frame), rather than nudging it; its `scale` knob scales the map's XY before that replacement.
  Maps follow the glTF/OpenGL convention (+Y = image-up, what Blender bakes) and a generated tangent
  frame follows it too; a DirectX (−Y) map reads with inverted relief.
- **`occlusion_texture`** — linear. Multiplies the ambient/IBL term at the same site as SSAO, never
  the direct or emissive terms; its `strength` knob is how far toward the sampled value the term
  darkens (`0` leaves it alone).
- **`emissive_texture`** — sRGB. Multiplies the per-vertex emissive intensity. It is independent of
  `base_color_texture` (glTF's rule): an emissive glow does not require, or inherit from, a base
  color map.

Textures arrive through `ludion import` (glTF), through `gen accept` on a generated mesh, or through
`ludion mesh apply` deriving one from a `kind: mesh-edit` document (`ludion docs mesh-editing`) — the
one deliberate way to paint a map without a DCC tool, `emissive` only. There is no wire command and
no editor UI that authors a texture directly. A hand-written reference in a `.mesh.yaml` is legal
but not a promoted workflow. Each map is a
`{ path, wrap?, scale?/strength? }` block, `path` relative to the mesh document's own directory (`..`
is fine; an absolute, drive or device path is refused, `L1111`):

```yaml
base_color_texture:
  path: textures/0123456789abcdef.png
normal_texture:
  path: textures/fedcba9876543210.png
  wrap: clamp
  scale: 0.75
```

`wrap` is `repeat` (default, may be omitted), `clamp`, or `mirror`. A map's scalar (`scale` on
`normal_texture`, `strength` on `occlusion_texture`) is omitted at its neutral value of `1`. A
`normal_texture` requires the document to also carry `tangents`; any texture requires `uvs` — both
enforced by `L1111`.

A mesh that references no texture is unaffected: the renderer's untextured arithmetic runs exactly
as it did before textures existed (`ludion devdocs renderer-shading`). Voxel models, sculpt models,
and primitives (`cube`/`plane`/`sphere`) carry no UVs and so cannot reference a texture at all —
voxel texturing is a recorded deferred possibility, not a current surface. To mark a UV-less mesh, project a texture onto it with a `Decal` (`ludion docs decals`).

## Cutout and double-sided

An asset mesh document carries three optional keys after its maps, none written at its default, so a
document that authors none keeps its bytes:

- **`alpha_mode`** — `opaque` (default) or `mask`; any other value is `L1110`. On `opaque` alpha is ignored.
- **`alpha_cutoff`** — `[0, 1]`, `mask` only, default `0.5`. Non-finite, outside `[0, 1]`, or set
  without `mask` is `L1111`.
- **`double_sided`** — default `false`; the mesh draws unculled.

```yaml
base_color_texture:
  path: textures/0123456789abcdef.png
alpha_mode: mask
alpha_cutoff: 0.35
double_sided: true
```

**Coverage** is `Mesh.color.a × base_color_texture.a` (glTF's rule); a `mask` fragment under the cutoff
is discarded. `mask` with no base map is legal — the coverage is then `Mesh.color.a`. The test runs in
the scene pass, probe capture, the SSAO prepass and every shadow caster (the sun's map and cascades,
the local-light atlas, the drop-shadow occlusion depth), so a grating's holes reach its shadow and the
light shafts. A double-sided mesh casts from both faces.

On a back face the whole shading frame flips: the final normal is negated after the normal map applies,
so both faces show the same relief, and `scene_normal`, reflections, SSR and lit decals see the flipped
normal. Decal receivers paint double-sided backs. A mirrored double-sided instance matches its
unmirrored twin.

A base-colour map bound by a `mask` mesh gets a coverage-preserving mip chain: each level's alpha is
scaled so the fraction of texels at or above the cutoff matches level 0, so thin bars survive at
distance. It holds under `textures: source` and `cooked` alike; every other map keeps the box chain.

**Render-only:** a collider built from a cutout mesh stays solid, so a grating stays walkable. The GI
voxelizer skips `mask` meshes, so light passes through a grating (`ludion docs global-illumination`).
Voxel, sculpt and primitive meshes carry no UVs and cannot be cutout. `BLEND` (sorted transparency) is
not supported: `ludion import` refuses it with `L1202`; glTF `MASK` and `doubleSided` map onto these
keys. `examples/gallery/cutout-double-sided.yaml` shows it all; the Blender set-up is
`ludion docs blender-craft`.

## A roughness × metallic sweep (copyable)

Component fields are the surface for this — no palette or mesh doc involved. Two spheres from a
larger grid (`examples/gallery/material-park.yaml` is the full 6×6 version):

```yaml
  - name: sphere-smooth-dielectric
    components:
      Transform:
        translation:
          - -2.0
          - 1.0
          - 0.0
      Mesh:
        shape: sphere
        color:
          - 0.95
          - 0.8
          - 0.42
          - 1.0
        roughness: 0.0
        metallic: 0.0
  - name: sphere-matte-metal
    components:
      Transform:
        translation:
          - 2.0
          - 1.0
          - 0.0
      Mesh:
        shape: sphere
        color:
          - 0.95
          - 0.8
          - 0.42
          - 1.0
        roughness: 1.0
        metallic: 1.0
```

The left sphere gets a tight bright highlight on a colored diffuse ball; the right a broad, dim
highlight tinted to the albedo with almost no diffuse response.

## Emissive + bloom (copyable)

`emissive` rides the existing HDR/bloom pipeline with no new pass: any fragment whose final color
exceeds `1.0` blooms. A voxel palette entry is a natural source — from `examples/gallery/voxels/beacon.voxel.yaml`:

```yaml
palette:
  E:
    color:
      - 0.4
      - 0.8
      - 1.0
      - 1.0
    emissive: 4.5    # cyan glass core, bright enough to overdrive and bloom
  T:
    color:
      - 0.85
      - 0.6
      - 0.25
      - 1.0
    metallic: 1.0    # metal trim, unrelated to the glow
    roughness: 0.25
```

A `Mesh.emissive` on a primitive works the same way: set it so `color × emissive` clears `1.0`; there
is no separate "make it bloom" switch.

## Metals under ambient-only light

**Without an `Environment`** (see `ludion docs environment`), ambient stays
`ambient.rgb × albedo` (ordinary, non-directional), and the specular lobe only comes from the scene's
point lights and sun. A `metallic: 1.0` surface has no diffuse response at all, so **a metal lit only
by flat ambient (no point light or sun reaching it) reads as nearly black** — that is the BRDF working
as specified, not a bug. If a metallic mesh looks dead in an `Environment`-less scene, the fix is
content, not the shader: add or move a point light so its specular highlight lands on the surface, as
`material-park.yaml`'s warm key + cool fill do. Raising `emissive` or `color` doesn't help — it changes
the surface's own light or tint, not its ability to reflect a light that isn't there.

**With an `Environment`**, this stops being a hard limit: its GPU-prefiltered sky feeds every lit
surface's ambient term, so a metal reflects the sky instead of nothing, with no point light required.
`examples/gallery/night-sky.yaml` is the same roughness×metallic sweep as `material-park.yaml`, alive
under a procedural night sky — see `ludion docs environment` for the component and
`ludion devdocs renderer-ibl` for the math.

## Next

- `ludion docs environment` — the sky and image-based lighting.
- `ludion docs lighting` — the sun and its shadow map.
- `ludion docs post-processing` — the imaging chain.
- `ludion docs voxel-format` — voxel model documents.
- `ludion docs sculpt-format` — sculpt model documents and their weighted material blend.
- `ludion docs components` — declaring components, and the built-in visual set.
- `ludion docs blender-craft` — exporting flat-factor and emissive materials from Blender cleanly.
