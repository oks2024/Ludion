---
description: OccludedSilhouette — a tint and outline for an entity's meshes where cover hides them
---

# Occluded silhouette: `OccludedSilhouette`

**Scope:** `OccludedSilhouette` draws an entity's meshes as a flat tint, and an optional outline, wherever a nearer
surface hides them. Visible pixels stay untouched. It is render-only: never hashed on a presentation entity, and absent
means the identical graph and the identical pixels. Registering it is `ludion docs components`; fading the occluder
instead is `ludion docs environment`.

## Format
```yaml
entities:
  - name: player
    components:
      Mesh:
        shape:
          assembly: voxels/figure.assembly.yaml
      OccludedSilhouette:
        color:               # display colour [r, g, b, a], each in [0, 1]; alpha is the fill opacity
          - 1.0
          - 1.0
          - 1.0
          - 0.55
        outline:             # optional; absent means no outline
          px: 2.0            # width in 1080p pixels, in (0, 8]; scales with the render height
          alpha: 1.0
        when: hidden         # hidden (default) | always: also tint visible pixels, an ally highlight
        bias: 0.05           # how far behind the occluder a pixel must be, in view-space units
```
`{}` takes every default. A mesh without its own takes its nearest ancestor's through `Parent`, as `Fade` does, so
one component on a root covers its children and every part of a voxel assembly. `outline` is an *outer* outline:
only a pixel outside every silhouetted mesh takes it, so a visible body pixel is never outlined.

## The hidden test
A pixel is hidden when the silhouetted mesh drew there and the scene's own surface is nearer along the view axis
by more than `bias`. Where the mesh is the front surface the two depths are equal, so a figure in the open, or one
arm in front of its own torso, draws nothing. Works for perspective and orthographic cameras alike.

## Interplay
| With | Rule |
|---|---|
| `Fade` on the silhouetted entity | Ignored by the mask: the silhouette follows the body, not its fade |
| `Fade` on an occluder | The dithered wall still writes depth where it is kept, so the tint shows through those texels, interleaved with the real body through the holes |
| `Viewmodel` | Never in the mask; `L2351` warns once per entity |
| `Visibility.visible: false` | No silhouette: a hidden mesh is never extracted |
| `alpha_mode: mask` occluder | The cut holes write no depth: the body shows through the grating and is tinted behind the bars |
| Alpha-blended meshes, sprites, particles, ground markers, text | Never occlude: they write no scene depth |
| Fog, depth of field, bloom | The silhouette is not fogged, defocused or bloomed |
| Vignette, grain | Apply: the tint is drawn into the graded image before them |
| SSAO, SSR, GI, probes, shadows | Unaffected: the mask is not a scene surface |

## Position in the chain
`composite` blends the tint and outline right after the grade and before the vignette, in the display colour
the grade produced (the target's sRGB store is the only thing after it), so white is white at any exposure. The
`silhouette_mask` pass draws only the tagged meshes into an `R8Unorm` style-index target and its own depth, and exists
only on a frame with at least one. It appears under that name in `ludion profile` pass timings, so its cost is
measured, not guessed. A frame holds at most **8 distinct styles**: entities with an identical component share a slot.

## Diagnostics
- `L2350` (error): a field out of range — a colour channel outside `[0, 1]`, `outline.px` outside `(0, 8]`,
  `outline.alpha` outside `[0, 1]`, a negative or non-finite `bias`.
- `L2351` (warning): the component resolves onto a `Viewmodel` mesh.
- `L2352` (warning): a frame resolves a ninth distinct style; it is not drawn.

`ludion explain L2350` gives the fix. The gallery scene is `examples/gallery/occluded-silhouette.yaml`.
