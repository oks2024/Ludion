# Plan: Secondary views

> Status: **IN PROGRESS** (Neon Shroud request 0017; W1 mechanized). Usage is `ludion docs render-views`; the gallery scene is
> `examples/gallery/render-view-radar.yaml`.

## Why

Neon Shroud wants a radar, a portrait and a camera feed: a second camera that renders the same world into a texture a
`UiRect` shows. The engine drew one camera per frame, so `L2101` rejected a second `Camera` and there was no
render-to-texture. Neon Shroud request 0017 asks for secondary views. W1 is the smallest slice the game can consume.

## Decisions

1. **A view is a derived frame.** It mirrors probe capture: `views::view_scene` clones the main `RenderScene` and swaps
   in the view's camera. It drops `ui`, `views`, the viewmodel, `silhouettes`, `debug_view` and `cull_camera`, and zeroes
   `post.ssao/ssr/dof/shafts` and the lights' `scatter`. Everything else stays. It renders through the ordinary
   `build_frame_graph` at its own native size with `aa: Off`, then is copied into a per-frame texture.
2. **Where.** At the top of `Renderer::prepare_frame_with`, only when `scene.views` is non-empty, after the main
   scene's probe resolve. Every render entry point gets views without a per-path change. Views prepare with
   `CacheEviction::Keep`.
3. **Budget.** At most `MAX_RENDER_VIEWS = 4` enabled views per frame, in `(order, EntityId)` order. A fifth is
   warning `L2374` and is not rendered. A `UiRect` naming a disabled or over-budget view draws nothing.
4. **Fields in W1.** `RenderView { name, size = [256, 256], enabled = true, order = 0 }`. Projection, near/far and LOD
   bias come from the entity's `Camera`. The request's `layers`, `lights`, `background`, `shading`, `post`, `sun` and
   `fog` are later waves, each additive.
5. **Determinism.** A view is an extract-time read. A presentation `RenderView` is unhashed; the sim never sees pixels.
6. **Main camera.** A `Camera` without `RenderView` is the main camera. `L2101` means more than one of those.

## Waves

- **W1** (this one): `RenderView`, `UiRect.view`, `views::render_views`, the budget, the gallery scene
  `render-view-radar`.
- **W2:** `render_layers:`, `RenderLayers` with `Parent` inheritance, `Camera.layers`/`lights`, `RenderView.layers`,
  `lights` and `sun`, light masks, layer-filtered shadow casters, and the portrait gallery.
- **W3:** `background`, `shading: unlit`, `fog`, `PostProfile`/`post`, and `L2125`.
- **W4:** `ludion render --view`/`--views-sheet`, `render.view {view}`, `render.bounds {camera}`, the `golden(view=)`
  test hook, `dump-graph` sub-graphs, a profile `views` section with a budget file, and presenter interpolation of
  view cameras.

## Out of scope / deferred

- **World-surface views (`Sprite.view`).** Trigger: a game puts a camera feed on a wall.
- **Split screen.** Trigger: a local-multiplayer game.
- **Portals.** Trigger: a game needs a view through a doorway.
- **Per-view shadow maps.** Trigger: a view looks at a region the main cascades do not cover.

## Proposed — owner ruling owed

- Narrowing `L2101` to cameras without `RenderView`.
- The 4-view budget and the 16..=2048 size range.
- A view at native size, which ignores `render_scale` (the request wanted it scaled).
- Views are not interpolated by the windowed presenter (`blend.rs` takes `next`'s views), so a fast-moving view camera
  may judder live.
- The main camera's sun cascades only, with no per-view shadow fit.
- Request differences: a disabled or over-budget view draws nothing where the request draws the clear colour; W1 has no
  `background` field.
- The `Plan/engine-gap-map.md` cameras line.

## Status — request 0017

- **W1:** mechanized. `RenderView` (`ludion-components`), `UiRect.view`, `L2370`–`L2374`, `Renderer::render_view` and
  `view_scene`, the 4-view budget, `ludion docs render-views`, and the gallery scene `render-view-radar` (blessed on the
  RTX 3090). A scene without a view keeps the same frame graph and bytes (`dump-graph` and the full gallery unchanged).
  View GPU time on the RTX 3090, a lit room, summed passes: 0.71 ms at 512², 0.65 ms at 256²
  (`needs_gpu_bench_view_gpu_ms`). Owed: the macOS twin of `render-view-radar` (M4).
