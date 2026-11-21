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
        layers: [default, radar]   # what the view draws; absent draws [default] (Render layers below)
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
fitted to the main camera, so a view that looks outside them sees unshadowed ground. A `ScreenEffect` with
`target: { view: <name> }` filters the view's texture (`ludion docs screen-effects`).

## Render layers
`RenderLayers: { layers: [radar] }` puts an entity, and every `Parent` descendant without its own `RenderLayers` (the
nearest ancestor wins), in the named layers. An entity without one is in `default`. `Camera.layers` (the main camera;
absent draws every layer) and `RenderView.layers` (absent draws `[default]`) choose what each image draws: meshes,
blended meshes, sprites, ground markers, drop shadows, decals, particles and world text. Shadows follow, because each
image casts from the meshes it draws.

```yaml
  - name: marker-for-radar
    components:
      GroundMarker: { shape: { type: disc, radius: 1.0 } }
      RenderLayers: { layers: [radar] }   # drawn by views that list radar, not by the main camera
```
Names are interned per frame: `default` is a bit of its own and a scene may use 31 other names (more is `L2375`). A
list must name at least one layer, none empty and none twice (`L2375`). Debug lines are never filtered; projectiles
and volley bullets are in `default`; the viewmodel ignores layers. The editor's `render.view` flight draws every layer,
so a designer sees radar overlays. A scene that authors none of these is unchanged, byte for byte. Probe captures and
the GI voxelization ignore layers.

### Declaring layers
A scene lists the layer names it uses once, at the top, as `physics.layers` does:

```yaml
format: 1
render_layers:
  - name: radar
  - name: portrait
entities: []
```
`default` is implicit and reserved; a name may not be empty or repeated, and the block holds at most 31 entries (`L2375`
at `/render_layers/<i>/name`). The block is validation only: `ludion check-scene` and `scene.check` report `L2376`, with
the nearest declared name, for an undeclared name in `RenderLayers`, `Camera.layers`/`lights` or
`RenderView.layers`/`lights`. Load and extract do not, so a name inserted at runtime still renders. A scene file only: a
prefab, an override and `generate-map` carry none.

## Light layers
A light joins layers through `RenderLayers` like any entity (a child light inherits its parent's). A point or spot light
lights an image when its layers meet that image's **light mask**:
- `Camera.lights` (main camera) and `RenderView.lights` list the light layers; absent, they default to the camera's or
  view's own `layers`. An unauthored camera is lit by every light.
- A light an image drops casts no shadow in it.
- `RenderView.sun: false` drops the `DirectionalLight` from that view. It needs no layer. There is no main-camera switch.

```yaml
  - name: portrait-key
    components:
      PointLight: { color: [0.3, 0.5, 1.0], intensity: 6.0, range: 10.0 }
      RenderLayers: { layers: [portrait] }   # lights only images whose light mask has portrait
```
The main camera filters by `Camera.lights` too, so a `portrait` light stays out of a main camera on `[default]`.

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
- `L2375` (error): an empty `layers`/`lights` list, an empty or repeated layer name, more than 31 names besides `default`, or a bad `render_layers:` entry.
- `L2376` (error): a layer name the scene's `render_layers:` does not declare (check-scene only).

`ludion check-scene` reports `L2370`–`L2373` and the list shape of `L2375` statically, with pointers. `ludion explain L2370` gives the fix.

## Gotchas
- A view is a presentation read: it sees the world as the last extract left it, never writes sim state, and a game
  system cannot read its pixels.
- The windowed presenter does not interpolate a view's camera between ticks, so a fast-moving view camera may judder
  live; headless renders are exact.
- A rect whose view is disabled shows nothing, not the clear colour.
- A view whose `lights` or `sun` differ from the main frame's draws without global illumination, so it never rebakes
  the main frame's volume and does not pick up its bounce light. Probes keep the main frame's capture: a lit view near
  a probe reflects the main frame's lights.

The gallery scenes are `examples/gallery/render-view-radar.yaml` (the room in perspective, and from above in the
corner) `render-view-layers.yaml` (a blue marker only on the radar, a green one only in the main frame) and
`render-view-portrait.yaml` (a figure lit only by its own key light in a bottom-left panel, absent from the main frame).
