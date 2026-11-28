---
description: Material scalars, author surfaces, precedence, texture maps, cutout, blended meshes, emissive, rim
---

# Materials

**Scope:** How every mesh-bearing surface shades: `roughness`, `metallic`, `emissive`, where each may be authored, and which surface wins. The lights that hit them are lighting; what the post stack then does to the image is post-processing.

Every mesh-bearing surface — a primitive, an imported asset, a voxel model — shades through the same
Cook-Torrance point/spot-light + ambient BRDF, driven by three scalars, each optionally modulated by a
texture map (below). A material is always exactly these three numbers, flat or sampled per-pixel.

- **`roughness`** — perceptual, `[0, 1]`. `0.0` is mirror-smooth (a tight, bright highlight); `1.0`
  is fully matte (a broad, dim one). Default `1.0` — deliberately the closest the GGX lobe gets to
  the engine's pre-PBR Lambert look, so content that authors no material moves the least.
- **`metallic`** — `[0, 1]`. `0.0` is a dielectric (a colored diffuse lobe plus a faint, colorless
  highlight); `1.0` is a metal (no diffuse lobe, and the highlight tints to the surface's own color).
  Default `0.0`.
- **`emissive`** — `>= 0`, an HDR intensity multiplier on the surface's **own** color — never a
  separate RGB. `2.0` on a `[0.4, 0.8, 1.0]` surface glows that same cyan twice as bright, not some
  other color. Default `0.0` (no glow). `ludion import` bakes a glTF `emissiveFactor` that is not a
  multiple of the base colour into an emissive map, so a tinted glow still imports (the field itself
  stays a scalar).

## The three author surfaces

The same three fields exist in four places, always named and ranged identically. A further field, `porosity` (`[0, 1]`, default 0.5; how far rain darkens the surface, `ludion docs weather`), follows the same surfaces and the same selection rule, as `Mesh.porosity`, a palette key and a `porosity:` list:

1. **`Mesh` component fields** — `roughness`, `metallic`, `emissive` (the built-in component table is
   `ludion docs components`). The only surface for a primitive (`cube`/`plane`/`sphere`) or a plain
   imported asset mesh, since neither has per-vertex or per-palette-entry material of its own.
2. **Voxel palette entry keys** — optional `emissive`/`metallic`/`porosity`/`roughness` alongside `color`, one
   value per glyph (every quad of that color gets it). See
   `ludion docs voxel-format --section Palette`.
3. **`.mesh.yaml` per-vertex lists** — optional `emissive:`/`metallic:`/`porosity:`/`roughness:` flat lists, one
   float per vertex, alongside `positions`/`normals`/`uvs`, for a hand-authored mesh asset that wants
   per-vertex material (`ludion devdocs scene-kind-documents --section "Mesh documents (\`kind: mesh\`)"` has the exact format).
4. **Sculpt palette entry keys** — the same shape as a voxel palette entry, one per part's
   `material` glyph. Unlike the other three surfaces, a sculpt vertex's material is a **weighted
   blend** of the parts that reach it — the smooth-union share each part won at that point, never a
   selection — so a smoothed seam between two glowing and matte parts fades rather than snaps. See
   `ludion docs sculpt-format`.

One exception: `rim` (see `## Rim light`) is a single document-level block on a voxel, sculpt or
`.mesh.yaml` document, not a per-palette or per-vertex value.

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
authors, not a mix with `Mesh`'s fields.) `rim` is the exception: a document's `rim:` wins over
`Mesh.rim`, which wins over an ancestor's.

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

Textures arrive through `ludion import` (glTF), through `gen accept` on a generated mesh, through `ludion texture build` from a `kind: texture`
recipe (`ludion docs texture-recipes`), or through `ludion mesh apply` deriving one from a `kind: mesh-edit` document (`ludion docs mesh-editing`) — the
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

## Level-of-detail chain

A `.mesh.yaml` may list up to three coarser levels, written before `indices` and finest first:
`lods: [{mesh: crate.lod1.mesh.yaml, screen_size: 0.25}, ...]`. Paths are relative to this document;
`screen_size` is the fraction of viewport height under which the level takes over, finite, above zero
and strictly decreasing. A level is an ordinary mesh document with no `lods`, and must match the base's
`alpha_mode`, `alpha_cutoff`, `double_sided`, `rim`, texture slots and (if skinned) identical `joints`,
so it draws through the base's pipeline and bone palette. A bad chain is `L1395`, a mismatched level
`L1396`; the cull pass picks the level per instance and view (`ludion docs culling`).

## Cutout and double-sided

An asset mesh document carries three optional keys after its maps, none written at its default, so a
document that authors none keeps its bytes:

- **`alpha_mode`** — `opaque` (default), `mask` or `blend` (drawn see-through, see §Blended meshes); any
  other value is `L1110`. On `opaque` alpha is ignored.
- **`alpha_cutoff`** — `[0, 1]`, `mask` only, default `0.5`. Non-finite, outside `[0, 1]`, or set
  without `mask` (the mode is `opaque`, `blend` or absent) is `L1111`.
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
Voxel, sculpt and primitive meshes carry no UVs and cannot be cutout. glTF `BLEND` imports as
`alpha_mode: blend`; `MASK` and `doubleSided` map onto these keys too. `examples/gallery/cutout-double-sided.yaml` shows it all; the Blender set-up is
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
point lights, spot lights and sun. A `metallic: 1.0` surface has no diffuse response at all, so **a metal lit only
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

## Wrap diffuse

`Mesh.wrap` (`0..=1`, default `0`) lets direct light bend past the terminator, a cheap stand-in for
subsurface on skin, leaves, wax or cloth: the diffuse term becomes `max((n·l + wrap) / (1 + wrap), 0)`
instead of `max(n·l, 0)` (call it `x`), and `scatter_color` (linear RGB, default white, so
`[1.0, 0.3, 0.2]` gives a warm red edge) tints the **total** diffuse per channel as
`x * mix(scatter, 1, 1 - wrap * (1 - x))`, kept above plain Lambert, so the tint has no crease at the
terminator. This departs from "tint only the excess"; a channel at `0` still gets some light near the
terminator, so `scatter_color: [0, 0, 0]` is not plain Lambert. Only the sun and point/spot lights
feed it; specular, image-based light, ambient and GI are unchanged. `wrap: 0` renders the same
bytes as a scene without the field.

```yaml
- name: ear
  components:
    Transform:
      translation:
        - 0.0
        - 1.0
        - 0.0
    Mesh:
      shape: sphere
      wrap: 0.6
      scatter_color:
        - 1.0
        - 0.3
        - 0.2
```

- **Cost.** A mesh that wraps pays a second pass over its cluster's lights and a second set of shadow
  lookups per fragment; a mesh without wrap pays one branch. Meshes with large screen coverage and
  wrap on cost the most.
- **Shadows.** The band is shadow-tested from a point pushed toward the light by the mesh's own
  bounding thickness along the light ray, so a rounded part does not shade its own band. The
  consequence: **wrap is for small, rounded parts.** A large receiver (ground, wall, long plank) or a
  thing close under an occluder keeps the band inside shadows, because the push clears occluders
  nearer than the mesh's thickness. A distant occluder still shades it. An occluder within the mesh's thickness along the light leaves the band lit with a hard edge, even a broad, near-touching one (about 110 levels in the gallery's near-slab case); wrap suits parts that are not closely overhung, and a lower `wrap` narrows the band and the edge. The tint is gentle under the smooth formula, and a `scatter_color` above `1` is allowed.
- **Decals.** A lit decal on a wrapped mesh shades the decal as plain Lambert.
- **Import.** A glTF material's `extras` keys `ludion_wrap` (`0..=1`) and `ludion_scatter_color`
  (three numbers `>= 0`) land on the part's `Mesh`; a wrong type or range is an import error
  naming the material. `examples/gallery/wrap-diffuse.yaml` shows wrap 0 / 0.5 / 1, tints, a
  shaded band and the shadow limit.

## Rim light

`Mesh.rim` adds an unlit view-angle edge, for keeping a figure readable against a dark wall without
lighting the room: `color * strength * (1 - n·v)^power`, with `n·v` clamped to `0..=1`. Fields:
`color` (linear RGB, `>= 0`, default white), `strength` (`>= 0`, default `1.0`), `power` (`> 0`,
default `3.0`; higher hugs the silhouette). A colour channel below `0` is `L1263`. Absent, `strength: 0`
or a black `color` renders the same bytes as a scene without it.

```yaml
- name: figure
  components:
    Transform:
      translation:
        - 0.0
        - 1.0
        - 0.0
    Mesh:
      shape: cube
      rim:
        color:
          - 0.9
          - 0.8
          - 0.7
        strength: 0.6
        power: 3.0
```

- **Unlit and authored.** It is added to the colour where `emissive` is, not scaled by ambient or
  haze, so the game tunes it per kit. It rides the medium like emissive: fog and haze treat it the
  same way. Keep the colour a dim neutral; a saturated hue reads as a glow.
- **Inherited down `Parent`.** A mesh without its own `Mesh.rim` takes its nearest ancestor's. A
  `Mesh`-less root cannot carry one: put the rim on the root's `Mesh`, or on every part.
- **Documents.** A `kind: voxel`, `kind: sculpt` or `kind: mesh` document may carry one top-level
  `rim:` with the same keys (in a voxel or sculpt document after `budget_exempt` and before `palette`; in a `.mesh.yaml` after `double_sided`). Precedence: the document's
  rim, then `Mesh.rim`, then the inherited one. There is no per-palette-entry rim. A sculpt `voxel`/`mesh` part's source document `rim:` is ignored; only the sculpt's own applies. Assembly parts are
  `kind: part` documents with no `rim` key, so an assembly takes its entity's `Mesh.rim`. A bad value
  is `L1114` (voxel), `L1157` (sculpt) or `L1111` (mesh).
- **Where it applies.** Viewmodel meshes take it, with the drawn camera as the eye; skinned meshes use
  their skinned normals. Sprites, particles and text have none. Probe captures leave it out: a probe's
  eye is not the player's.
- **Cost.** One instance lane (`@location(20)`); a mesh without a rim pays one branch.
- `examples/gallery/rim-light.yaml` shows the four paths against a dark wall, with its sweeps.

`Mesh.medium` (`[0, 1]`, default `1`, inherited down `Parent`) exempts a mesh from the fog medium:
`ludion docs atmosphere --section "Exempting an object from the medium"`.

## Blended meshes

`Mesh.blend: alpha | additive` (absent = opaque) draws a mesh seen through, on every mesh kind: primitive,
asset, voxel, sculpt, assembly (voxel or skinned). `color`'s alpha is the opacity; `Fade` multiplies it instead of
dithering. It is not inherited down `Parent`, and an assembly is one entity, so all its parts blend.

An asset mesh document may say `alpha_mode: blend` instead: the mesh draws as `blend: alpha` with no
`Mesh.blend` on the entity. `Mesh.blend` overrides the document (`additive` wins over the document's alpha);
no value forces a blend document opaque. On an assembly each part takes its own document's mode and
shares the entity's sort group, so one glass part among opaque ones blends alone.

```yaml
Mesh:
  shape: cube
  color: [0.45, 0.7, 0.8, 0.35]
  roughness: 0.1
  blend: alpha        # `additive`: adds the lit colour scaled by the opacity; brightens, never darkens
```

A blended mesh is lit like an opaque one, then drawn after the ground markers and before the sprites,
sorted back to front **per entity** (an assembly's parts sort as one figure, by the view distance to the
centre of their joint bounds; under an orthographic camera by depth along the view). It tests depth and
writes none. Two interpenetrating panes do not sort exactly. Because it joins the transparent layer, fog,
light shafts and depth of field read it at its own distance.

| Consumer | Sees a blended mesh? |
|---|---|
| Depth pre-pass, ambient occlusion, SSR, decals | No: it writes no opaque depth, so they read what is behind it |
| Sun and local shadows | No: it never casts a shadow, whatever `Visibility.cast_shadows` says |
| GI voxelization, probe captures, `OccludedSilhouette` mask | No |
| Asset `alpha_mode: blend` | Blended exactly as `Mesh.blend: alpha`: every row here holds for it, including `Static` and `Viewmodel` |
| Sun and local-light shadows *on* it, sun, point and spot light | Yes: it is shaded like an opaque mesh |
| Indirect specular: the sky's IBL and `ReflectionProbe`s | Yes, a lit blended mesh reflects the sky and the probes (relit, with their emissive part and sky visibility) as an opaque mesh does |
| Screen-space reflections | Never: a blended mesh is not an SSR origin and shows no trace |
| Fog, shafts, depth of field, `Mesh.medium` | Yes, at its own distance |
| Sprites, particles, text | Draw after it, in their own order: a sprite behind glass is painted over it |
| `Static` | A `Static` blended mesh takes the dynamic path (`L1341`) |
| `Viewmodel` | The `blend` is ignored with `L2360`: the mesh draws opaque |
| Physics, perception | Unchanged: `blend` is render-only, so a pane blocks or passes sight and shots by its collider and layer, not by its look |

Primitive glass: a thin cube (`scale: [3, 3, 0.08]`) with a low alpha and low `roughness`, in a frame of
opaque cubes. Give the pane a collider on a layer that sight rays ignore if guards should see through it. `examples/gallery/glass-and-holograms.yaml` shows glass, an additive
hologram, a ghost assembly and a faded pane.

A glossy pane reflects the sky and a nearby probe: low `roughness` with an `Environment` (and a `ReflectionProbe`
around the scene) shows them in the glass, scaled by the pane's F0 and its opacity like the rest of its colour.
`examples/gallery/glass-reflections.yaml` shows glass over a sky and a probe.

`Mesh.lit: false` (default `true`) draws a blended mesh unlit: instance colour × vertex colour × base map, plus
`emissive`, `rim` and `flash`, with no direct light, ambient, `wrap` or indirect specular. It is the hologram
look: the colour is what it says whatever lights the scene. On a mesh that draws opaque (no `blend`, a document
without `alpha_mode: blend`, or a `Viewmodel` mesh) `lit: false` is ignored with `L2361` and the mesh draws lit.

```yaml
Mesh:
  shape: cube
  color: [0.1, 1.0, 0.4, 0.6]
  blend: additive
  lit: false          # `L2361` if the mesh draws opaque
```

`Mesh.blend_depth: nearest` (default `none`, absent from the file) makes only a blended entity's front surface
blend. Without it every surface blends in draw order, so a translucent figure shows its far arm through its near
one; with it the figure's far surfaces are dropped, and an assembly counts as one figure, every part included. The
same holds in the transparent layer, so fog, shafts and depth of field see the front surface's distance. It costs a
render-extent depth copy and two small passes per `nearest` entity per frame, twice with fog or depth of field on, so
it is for a handful of figures, not a crowd. Two `nearest` entities that interpenetrate do not sort exactly: each is
placed whole, by its centre. On a mesh that draws opaque it is ignored with `L2362`.

```yaml
Mesh:
  shape: { assembly: voxels/figure.assembly.yaml }
  color: [0.25, 1.0, 0.55, 0.45]
  blend: alpha
  blend_depth: nearest   # `L2362` if the mesh draws opaque
```

`examples/gallery/ghost-figure.yaml` shows the same figure with and without it, and behind glass.

## Next

- `ludion docs environment` — the sky and image-based lighting.
- `ludion docs lighting` — the sun and its shadow map.
- `ludion docs post-processing` — the imaging chain.
- `ludion docs voxel-format` — voxel model documents.
- `ludion docs sculpt-format` — sculpt model documents and their weighted material blend.
- `ludion docs components` — declaring components, and the built-in visual set.
- `ludion docs blender-craft` — exporting flat-factor and emissive materials from Blender cleanly.
