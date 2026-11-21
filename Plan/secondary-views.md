# Plan: Secondary views

> Status: **IN PROGRESS** (Neon Shroud request 0017; W1 and W2a mechanized). Usage is `ludion docs render-views`; the gallery scene is
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
7. **Split at the end of extract.** A frame is *layered* when any `RenderLayers` exists or any `Camera`/`RenderView` authors
   `layers`. Extract builds the full scene, then splits it: the main `RenderScene` keeps the items whose mask meets the main
   camera's, and carries the full scene and every item's mask as `layered: Option<Box<LayeredSource>>`. `view_scene` starts
   from that source and keeps the view's mask. An unlayered frame has `layered: None` and takes no new code path.
8. **Names become bits per frame.** `default` is bit 0; every other name in `RenderLayers`, `Camera.layers` and
   `RenderView.layers` gets bits 1.. in sorted order. More than 31 such names is error `L2375` at extract. W2b adds the
   scene-level `render_layers:` declaration; W2a needs none.
9. **Inheritance.** The nearest `Parent` ancestor's `RenderLayers` wins, through `hierarchy::InheritMemo` (`own.or(parent)`).
10. **Ownerless items are `default`:** projectile meshes and particles, volley bullets. Debug lines are never filtered. The
    viewmodel ignores layers (its meshes and particles carry the full mask; a view drops them anyway, D1). Lights are
    unfiltered until W2c. A drop shadow's `exclude` list, a decal's receivers, the silhouette table and the viewmodel's
    locals are mesh indices: `RenderScene::retain` rewrites each when a filter drops meshes (this also fixed a W1 shift of
    decal receivers when a viewmodel mesh sat before them).
11. **The `render.view` editor override draws every layer** (main mask `u32::MAX`), so a free camera sees radar overlays
    and portrait rigs.
12. **Statics.** Dropping a static mesh recomputes the static prefix (`StaticRange::of`); only then. A view whose kept
    static set differs from the main frame's (`statics.key`) draws its statics as dynamics, so it never swaps the
    renderer's one persistent static slot. GI voxelization ignores layers. Probes capture the main frame's world, and
    views reuse it: `split_layers` freezes the filtered main scene as the `probe_world` when probes exist, and
    `view_scene` hands every view that same world, so a view resolves to the main frame's store key (a cache hit)
    instead of recapturing every probe each frame.
13. **API break:** `Camera` loses `Copy` (`layers: Option<Vec<String>>` is heap data). Absent `layers` is omitted from the
    document and the hash.

14. **The declaration is validation only.** `Scene.render_layers: Vec<SceneRenderLayer { name }>`. Parse errors are `L2375` at
    `/render_layers/<i>/name`: an empty name, the reserved `default`, a duplicate, or more than 31 entries. Extract still
    interns names per frame (D8): the world does not carry the block (restore is text-free), so a runtime-inserted
    undeclared name still renders. `L2376` is static, as `L2372` is. A scene that authors no layer name needs no block.
15. **The block lives in a scene file only.** It is not overridable, not allowed in a prefab, and `generate-map` writes none.
    Canonical key order: `format`, `name`, `generated_from`, `seed`, `physics`, `render_layers` (omitted when empty),
    `preload`, `entities`. `Engine::save_live` carries the authored block.
16. **Light masks.** A light's mask is its entity's `RenderLayers`, else its nearest ancestor's, else `default`. It lights an
    image when it meets that image's light mask. `Camera.lights` defaults to the camera's own `layers` mask, and so does
    `RenderView.lights`. A light removed from an image casts no shadow there. `editor_view` gives every layer.
17. **`RenderView.sun: bool = true`.** `false` drops the sun from the view; it needs no layer. There is no main-camera sun
    switch.
18. **GI follows the main frame.** The GI bake and inject are keyed on the light list and the sun, so a view whose `lights`
    or `sun` differ from the main frame's draws without GI and never rebakes the main volume. Probes keep the main frame's
    capture (D12): a lit view near a probe reflects the main frame's lights.
19. **The main frame filters by `Camera.lights` too,** so a portrait key light on `portrait` stays out of a main camera on
    `[default]`.

## Waves

- **W1** (this one): `RenderView`, `UiRect.view`, `views::render_views`, the budget, the gallery scene
  `render-view-radar`.
- **W2a** (shipped): `RenderLayers` with `Parent` inheritance, `Camera.layers`, `RenderView.layers`, layer-filtered
  meshes, sprites, markers, shadows, decals, particles and text, `L2375`, the gallery scene `render-view-layers`.
- **W2b** (shipped): the scene-level `render_layers:` declaration (declare-once, like `physics.layers`) and `L2376` for an
  undeclared name from check-scene and `scene.check`.
- **W2c** (shipped): lights join layers through `RenderLayers`, `Camera.lights`, `RenderView.lights` and `sun`, and the
  portrait gallery.
- **W3:** `background`, `shading: unlit`, `fog`, `PostProfile`/`post`, and `L2125`.
- **W4:** `ludion render --view`/`--views-sheet`, `render.view {view}`, `render.bounds {camera}`, the `golden(view=)`
  test hook, `dump-graph` sub-graphs, a profile `views` section with a budget file, and presenter interpolation of
  view cameras.

## Out of scope / deferred

- **World-surface views (`Sprite.view`).** Trigger: a game puts a camera feed on a wall.
- **Split screen.** Trigger: a local-multiplayer game.
- **Portals.** Trigger: a game needs a view through a doorway.
- **Per-view shadow maps.** Trigger: a view looks at a region the main cascades do not cover.

## Owner rulings (2026-11-19)

- Accepted. Narrowing `L2101` to cameras without `RenderView`.
- Accepted. The 4-view budget and the 16..=2048 size range.
- Accepted. A view at native size, which ignores `render_scale` (the request wanted it scaled).
- Accepted. Views are not interpolated by the windowed presenter (`blend.rs` takes `next`'s views), so a fast-moving view camera
  may judder live.
- Accepted. The main camera's sun cascades only, with no per-view shadow fit.
- Accepted. Request differences: a disabled or over-budget view draws nothing where the request draws the clear colour; W1 has no
  `background` field.
- Accepted. The `Plan/engine-gap-map.md` cameras line.

## Status — request 0017

- **W1:** mechanized. `RenderView` (`ludion-components`), `UiRect.view`, `L2370`–`L2374`, `Renderer::render_view` and
  `view_scene`, the 4-view budget, `ludion docs render-views`, and the gallery scene `render-view-radar` (blessed on the
  RTX 3090). A scene without a view keeps the same frame graph and bytes (`dump-graph` and the full gallery unchanged).
  View GPU time on the RTX 3090, a lit room, summed passes: 0.71 ms at 512², 0.65 ms at 256²
  (`needs_gpu_bench_view_gpu_ms`). Owed: the macOS twin of `render-view-radar` (M4).
- **W2a:** mechanized. `RenderLayers` (`ludion-components`), `Camera.layers` and `RenderView.layers`, `L2375`,
  `RenderScene::retain`/`split_layers` and `RenderScene.layered` (`ludion-renderer`), the extract split with the blend keys
  following the filtered lists, `ludion docs render-views`, and the gallery scene `render-view-layers` (blessed on the
  RTX 3090). A scene that authors none keeps the same frame graph and bytes (`dump-graph` of `post-full` and
  `render-view-radar` identical, the full gallery unchanged). API break: `Camera` is not `Copy`. Owed: the macOS twin
  of `render-view-layers` (M4).
