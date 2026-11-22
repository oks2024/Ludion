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
        background: { color: [0.0, 0.03, 0.01] }  # linear HDR clear; default sky
        shading: unlit         # lit (default) | unlit: albedo + emissive only
        fog: false             # default true; false drops the medium in this view
        post: radar-feed       # absent = the scene's PostProcess; none = no look; else a PostProfile name
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

## Look
A view can look different from the world it draws:
- `background`: `sky` (default) or `{ color: [r, g, b] }`, linear HDR, each component finite and `>= 0`. A colour replaces
  the clear and drops the sky draw in that view; the lighting still uses the `Environment`. The texture is opaque: a
  translucent panel is `UiRect.color`'s alpha.
- `shading: unlit`: albedo plus emissive (and rim); no lights, sun, shadows from lights, GI, probes or environment light.
- `fog: false`: the medium (`fog_*`) is off in this view only. A view's fog always comes from the scene's `PostProcess`.
- `post`: absent renders with the scene's imaging, `none` with no look at all (no tonemap, bloom, meter or LUT; a
  neutral grade; dither stays), a name with that `PostProfile`'s look. A `PostProfile { name }` beside a `PostProcess`
  makes that `PostProcess` a named profile: it carries the grade, bloom, tonemap, exposure and LUT a view takes, and it
  is **not** the scene's `PostProcess`. A view's `ScreenEffect` still runs last.

```yaml
  - name: radar-look
    presentation: true
    components:
      PostProfile: { name: radar-feed }
      PostProcess: { tonemap: none, exposure: 1.0, bloom_strength: 0.0, gain: [0.4, 1.0, 0.5] }
```

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
- `L2377` (warning): a `PostProfile` a view names authors `ssao`, `ssr`, `dof`, `shafts` or `fog_density > 0`: a view never runs those and takes its fog from the scene (check-scene only).
- `L2378` (error): a `PostProfile` with an empty name, the name `none`, a repeated name or no `PostProcess` beside it, or a `RenderView.post` naming no profile (extract raises it too, with the nearest name).

`ludion check-scene` reports `L2370`–`L2373`, the list shape of `L2375`, `L2377` and `L2378` statically, with pointers. `ludion explain L2370` gives the fix.

## Checking a view
- `ludion render --scene s.yaml --view radar --out radar.png` writes the view's texture alone, at its `size` (default
  `--out` `<scene>.view-<name>.png`); `--width`/`--height` size only the main frame. It prints `view radar: 256x256 on <adapter>`.
- `ludion render --scene s.yaml --views-sheet --out sheet.png` writes the frame, then each rendered view in `views` order, in one
  unlabelled row at native sizes, 8 px apart on a dark fill (default `<scene>.views.png`). A judging instrument, not a golden.
- `GoldenTest::in_set(..).with_view("radar")` goldens one view instead of the frame, on the built-in path or through
  `with_builder`; a replay or a model document refuses it. In a directory suite, a `goldens/manifest.json` entry
  `"render-view-radar": { "views": ["radar"] }` adds a test `<stem>.view-<name>` with the golden `<stem>.view-<name>.png` beside the
  frame's; `ludion test --filter .view-` runs those alone.
- `L2379` (error): the name matches no enabled, in-budget view (unknown, disabled or a fifth); the help lists the rendered names.

## Gotchas
- A view is a presentation read: it sees the world as the last extract left it, never writes sim state, and a game
  system cannot read its pixels.
- The windowed presenter does not interpolate a view's camera between ticks, so a fast-moving view camera may judder
  live; headless renders are exact.
- A rect whose view is disabled shows nothing, not the clear colour.
- `unlit` is a derived scene, not a shader variant: it drops the lights, sun, GI, probes and environment lighting and
  lights with a flat ambient of 1, so a surface reads its albedo. The sky still draws unless `background` is a colour.
- A profile's fog is ignored (the world's medium, `fog: false` to drop it), and `ssao`, `ssr`, `dof` and `shafts` never run
  in a view. A view with a `post` takes the profile's LUT, not the scene's; `none` takes none.
- A view whose `lights` or `sun` differ from the main frame's draws without global illumination, so it never rebakes
  the main frame's volume and does not pick up its bounce light. Probes keep the main frame's capture: a lit view near
  a probe reflects the main frame's lights.

The gallery scenes are `examples/gallery/render-view-radar.yaml` (the room in perspective, and from above in the
corner) `render-view-layers.yaml` (a blue marker only on the radar, a green one only in the main frame) 
`render-view-portrait.yaml` (a figure lit only by its own key light in a bottom-left panel, absent from the main frame) and
`render-view-unlit.yaml` (a flat green unlit radar with its own background, no fog and a `PostProfile`, beside a fogged lit room).
