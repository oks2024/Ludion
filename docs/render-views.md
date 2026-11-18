---
description: RenderView — a second camera rendered into a texture that a UiRect shows
---

# Secondary views: `RenderView`

**Scope:** `RenderView` makes the `Camera` on its entity a secondary view: the same world rendered into its own texture,
every frame it is enabled, in the same frame. A `UiRect` shows it by name. Render-only: a presentation `RenderView` is
never hashed, and a scene without one renders the identical graph and bytes. The main camera is `ludion docs camera`;
`UiRect` is `ludion docs screen-ui`.

## Format
```yaml
entities:
  - name: radar-cam            # a presentation entity: a camera beside a RenderView is never the main camera
    components:
      Transform:
        translation: [0.0, 15.0, 0.0]
        rotation: [-90.0, 0.0, 0.0]
      Camera:
        projection: { type: orthographic, height: 13.0 }
      RenderView:
        name: radar            # required, unique per scene; what UiRect.view names
        size: [256, 256]       # texels, each axis 16..=2048 (default [256, 256])
        enabled: true          # default true
        order: 0               # lowest first; ties by entity
  - name: radar-window
    components:
      UiRect:
        anchor: top_right
        offset: [-30.0, 30.0]
        size: [200.0, 200.0]
        view: radar            # the rect's whole image; no texture, atlas, frame, slice or uv_rect beside it
```
Projection, `near`/`far` and `lod_bias` come from the entity's `Camera`; the aspect comes from `size`. A `Camera`
without a `RenderView` is the main camera: `L2101` counts only those. Groups, clip, tween, opacity and `color` apply to
a view rect like any other.

## What a view draws
The ordinary frame graph through the view's camera, at `size` texels (not scaled by `render_scale`), without
anti-aliasing. It keeps meshes, voxels, sprites, ground markers, particles, text, decals, blended meshes, lights with
shadows, the environment, GI and probes. It drops the UI, other views, the viewmodel, silhouettes, debug draw, and the
screen-space effects SSAO, SSR, depth of field and light shafts, and light scattering. The sun's shadow cascades are
fitted to the main camera, so a view that looks outside them sees unshadowed ground.

## Budget and cost
At most **4** enabled views render per frame, in `(order, entity)` order. Each further one is warning `L2374` and is
not rendered; a `UiRect` naming a disabled or skipped view draws nothing. A view costs about one extra frame at its
size, and its passes run before the main frame's. `needs_gpu_bench_view_gpu_ms` in `ludion-renderer` (ignored; run it in
release) prints the summed pass GPU time at 512² and 256².

## Diagnostics
- `L2370` (error): empty `name`, a `size` axis outside `16..=2048`, or a `RenderView` with no `Camera` beside it.
- `L2371` (error): two views share a `name`.
- `L2372` (error): `UiRect.view` names no `RenderView`.
- `L2373` (error): `UiRect.view` beside `texture`, `atlas`, `frame`, `slice` or `uv_rect`.
- `L2374` (warning): more than 4 enabled views.

`ludion check-scene` reports the first four statically, with pointers. `ludion explain L2370` gives the fix.

## Gotchas
- A view is a presentation read: it sees the world as the last extract left it, never writes sim state, and a game
  system cannot read its pixels.
- The windowed presenter does not interpolate a view's camera between ticks, so a fast-moving view camera may judder
  live; headless renders are exact.
- A rect whose view is disabled shows nothing, not the clear colour.

The gallery scene is `examples/gallery/render-view-radar.yaml`: the room in perspective, and from above in the corner.
