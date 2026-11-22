# Plan: Secondary views

> Status: **IN PROGRESS** (Neon Shroud request 0017; W1–W2c mechanized). Usage is `ludion docs render-views`; the gallery scene is
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
20. **Background.** `ViewBackground { Sky, Color([f32; 3]) }`, the `ScreenEffectTarget` serde shape (`sky` | `{color: [..]}`),
    skipped when `Sky`. Components finite and `>= 0` (`L2370` at `/background/color/<i>`). Opaque: the view texture has no
    alpha; a translucent panel is `UiRect.color`'s alpha. A colour replaces the clear and drops the sky draw in that view.
21. **Unlit is a derived scene, not a shader variant.** `view_scene` clears lights, spot lights, sun, GI, voxel, probes and
    `probe_world`, sets `ambient = [1; 3]` and `RenderScene.unlit = true`; the renderer then resolves IBL as absent
    (flat-ambient branch) while still drawing the sky. No new pipeline, no new pass.
22. **Fog is the world's.** A view's `fog_*` fields are the scene `PostProcess`'s whatever its `post`; `fog: false` zeroes
    `fog_density` only in that view.
23. **`PostProfile { name }`.** A marker; it needs a `PostProcess` beside it. The main frame's singleton (and its LUT) is the
    lowest-id `PostProcess` without `PostProfile`. A profile's LUT is honoured in its views. `L2378`: name not empty, not
    `none`, unique; a view's `post` must name one.
24. **`post: none`** is passthrough: `PostData::passthrough()` (bloom 0, exposure 1, no tonemap, no meter, no LUT, neutral
    grade, every effect off; dither kept).
25. **`L2377` (warning).** A profile a view names authors `ssao`, `ssr`, `dof`, `shafts` or `fog_density > 0`: a view never
    runs those (D1) and takes the medium from the scene (D22). Static only (check-scene, `scene.check`).
    Request differences: `L2377`/`L2378` stand for the request's `L2125`/`L2127`; `background.color` is RGB; a profile's fog is
    ignored.
19. **The main frame filters by `Camera.lights` too,** so a portrait key light on `portrait` stays out of a main camera on
    `[default]`.
26. **A view render is the frame's view.** The same extract and derived scene the frame's `UiRect` samples
    (`Renderer::render_view`), at the view's own `size`. `--width`/`--height` size only the main frame; the view's camera
    takes its aspect from `size` (D4).
27. **`L2379` (error).** A render names a view that no enabled, in-budget `RenderView` renders this frame: an unknown
    name, a disabled view or a fifth. The help lists the rendered names (nearest first).
28. **The views sheet** is one row, top-aligned, at native sizes: the frame, then each rendered view in `views` order,
    8 px gutters on a dark fill (`[20, 20, 28, 255]`). Unlabelled (a view has no UI pass to draw a label with). A judging
    instrument, not a golden.
29. **View goldens** are named `<stem>.view-<name>.png` beside the frame's golden. The manifest's `views` adds tests named
    `<stem>.view-<name>`; `--filter` matches the test name, so `--filter .view-` selects view tests only.
    Request differences: the wire fields are W4c; the sheet has no drawn labels; `golden(view=)` is `GoldenTest.view`.
30. **A view's sub-graph is the graph it renders.** `view_scene(scene, i)` at `FrameSize::native(view.size)` with
    `PostChain { aa: Off, ..chain_of(&derived) }`, where `chain_of` is the same quality mask (and, live, the same
    frustum-culling switch) the main dump uses. Emitted as `"views": [{ "name", "graph" }]` after `aliasing`, omitted when
    empty, so a view-free scene dumps byte-identical output.
31. **View timings are rows, not a section.** `view:<name>` is the view's GPU frame-span median (`null` when the backend
    proves no span); `view:<name>/<pass>` are its per-pass medians, appended to `gpu.passes`. Offline driver only: a live
    capture (`perf_live`, the `PassTimingRing`) and `render.dump_timings` carry none. Each view is timed in its own one-shot
    render beside the measured frame, so a profile pays each view twice (once inside the frame, untimed).
32. **`[views]` gates the span row.** Over the ceiling is `L4005`; a `view:<name>` row with a `null` or `0.0` median is
    `L4019`; a `[views]` key with no row while the GPU was measured is `L4006` listing the measured view names; a `[views]`
    line against a run with no GPU measurement is `L4019`. `[passes]` keeps its static producible check: a `view:` row is
    not a `[passes]` key.
33. **Calibration.** `budgets/pinned-rtx3090-render-views.toml` on `examples/gallery/render-view-radar.yaml`, about 2x the
    release median over 120 frames, the measured value in a comment.
    Request differences: the request's `views` profile section is `view:`-named `gpu.passes` rows (a section would move the
    protocol schema, an R14 item); live captures carry no view rows.
34. **View cameras blend like the main camera.** `BlendData.views: Vec<Option<CameraBlend>>`, parallel to
    `RenderScene::views` (empty = unkeyed), keyed by the view's entity. A view camera pairs with the previous tick's entry of
    the same key (a search, not an index: a toggled view shifts the list). A snapped (`NoInterpolate`, `PresentCuts`) or
    unpaired view takes the newer tick. The view camera ignores `CameraOffset`, and the late latch stays main-only.
35. **A layered frame blends its source too.** `BlendData.layered: Option<Box<BlendData>>` carries item keys parallel to
    `layered.scene`'s lists (its `poses`, `snaps`, `camera`, `views` and `epoch` are unused: the outer ones serve). The pose
    table covers the union of both key sets, so an item on a view-only layer is keyed.
36. **What stays the newer tick:** an orthographic view's height, a view's projection when its `size` aspect changes between
    ticks, and the viewmodel (views drop it).

## Waves

- **W1** (shipped): `RenderView`, `UiRect.view`, `views::render_views`, the budget, the gallery scene
  `render-view-radar`.
- **W2a** (shipped): `RenderLayers` with `Parent` inheritance, `Camera.layers`, `RenderView.layers`, layer-filtered
  meshes, sprites, markers, shadows, decals, particles and text, `L2375`, the gallery scene `render-view-layers`.
- **W2b** (shipped): the scene-level `render_layers:` declaration (declare-once, like `physics.layers`) and `L2376` for an
  undeclared name from check-scene and `scene.check`.
- **W2c** (shipped): lights join layers through `RenderLayers`, `Camera.lights`, `RenderView.lights` and `sun`, and the
  portrait gallery.
- **W3:** `background`, `shading: unlit`, `fog`, `PostProfile`/`post`, and `L2377`/`L2378`.
- **W4a:** `ludion render --view`/`--views-sheet`, `Engine::render_view_png`, `render_scene_views`, the `GoldenTest.view`
  hook, the manifest's `views`, and the gallery view goldens (D26-D29).
- **W4b-1:** `dump-graph` sub-graphs, `view:` rows in the offline profile, `[views]` in a budget file and the render-views
  budget file (D30-D33).
- **W4b-2:** presenter interpolation of view cameras and of a layered frame's view content (today `blend` blends only the
  main lists, so `scene.layered`'s source and `scene.views[i].camera` come from the newer tick).
- **W4c:** the wire fields `render.screenshot`/`render.view {view}` and `render.bounds {camera}`. They move the protocol
  schema hash and the replay schema hash (an R14 item), so they are held for the owner.

## Out of scope / deferred

- **World-surface views (`Sprite.view`).** Trigger: a game puts a camera feed on a wall.
- **Split screen.** Trigger: a local-multiplayer game.
- **Portals.** Trigger: a game needs a view through a doorway.
- **Per-view shadow maps.** Trigger: a view looks at a region the main cascades do not cover.
- **The late latch for a view camera.** Trigger: a game shows a view driven by the main camera's look (a helmet cam): the
  view then trails the main view by up to one tick. Owner ruling 2026-11-21: accepted.
- **Interpolating an orthographic view's height.** Trigger: a game animates a radar zoom that must be smooth live.

## Owner rulings (2026-11-19)

- Accepted. Narrowing `L2101` to cameras without `RenderView`.
- Accepted. The 4-view budget and the 16..=2048 size range.
- Accepted. A view at native size, which ignores `render_scale` (the request wanted it scaled).
- Accepted. Views are not interpolated by the windowed presenter (`blend.rs` takes `next`'s views), so a fast-moving view camera
  may judder live (fixed in W4b-2).
- Accepted. The main camera's sun cascades only, with no per-view shadow fit.
- Accepted. Request differences: a disabled or over-budget view draws nothing where the request draws the clear colour; W1 has no
  `background` field.
- Accepted. The `Plan/engine-gap-map.md` cameras line.
- Accepted (2026-11-21). The late latch stays main-camera only (D34–D36); a view driven by the main camera's look trails it
  by up to one tick until the trigger in Out of scope.

## Status — request 0017

- **W1:** mechanized. `RenderView` (`ludion-components`), `UiRect.view`, `L2370`–`L2374`, `Renderer::render_view` and
  `view_scene`, the 4-view budget, `ludion docs render-views`, and the gallery scene `render-view-radar` (blessed on the
  RTX 3090). A scene without a view keeps the same frame graph and bytes (`dump-graph` and the full gallery unchanged).
  View GPU time on the RTX 3090, a lit room, summed passes: 0.71 ms at 512², 0.65 ms at 256²
  (`needs_gpu_bench_view_gpu_ms`). Owed: the macOS twin of `render-view-radar` (M4).
- **W3:** mechanized. `RenderView.background`/`shading`/`fog`/`post` and `PostProfile` (`ludion-components`), `L2377`/`L2378`,
  `ViewPost`, `RenderViewData`'s look fields, `RenderScene.background`/`unlit` and `PostData::passthrough` (`ludion-renderer`),
  `resolve_view_posts` and the profile-aware `extract_post`/`extract_lut` (`ludion-engine`), docs `render-views` and
  `post-processing`, and the gallery scene `render-view-unlit` (blessed on the RTX 3090). A scene that authors none keeps the
  same frame graph and bytes (`dump-graph` of `post-full`, `render-view-radar`, `render-view-portrait` and
  `screen-effect-view` identical, the full gallery unchanged). API break: `RenderView` is not `Eq`; no external game names
  it. Owed: the macOS twin of `render-view-unlit` (M4).
- **W2b + W2c:** mechanized (r2410): the `render_layers:` block and `L2376`, `Camera.lights`, `RenderView.lights`/`sun`,
  light masks, GI dropped for a view that lights differently, golden `render-view-portrait`. Owed: its macOS twin (M4).
- **W2a:** mechanized. `RenderLayers` (`ludion-components`), `Camera.layers` and `RenderView.layers`, `L2375`,
  `RenderScene::retain`/`split_layers` and `RenderScene.layered` (`ludion-renderer`), the extract split with the blend keys
  following the filtered lists, `ludion docs render-views`, and the gallery scene `render-view-layers` (blessed on the
  RTX 3090). A scene that authors none keeps the same frame graph and bytes (`dump-graph` of `post-full` and
  `render-view-radar` identical, the full gallery unchanged). API break: `Camera` is not `Copy`. Owed: the macOS twin
  of `render-view-layers` (M4).
- **W4a:** mechanized. `ludion render --view <name>` / `--views-sheet`, `render_scene_views`, `find_view`, `ViewsOutcome`,
  `Engine::render_view_png` and `views_sheet` (`ludion-engine`), `L2379`, `GoldenTest.view` and the manifest's `views`
  (`ludion-testkit`), the view goldens `render-view-radar.view-radar` and `render-view-portrait.view-portrait` (blessed on the
  RTX 3090), docs `render-views` and `verify-loop`. No wire field, golden, replay or graph moved (`dump-graph` of
  `render-view-radar` and `render-view-unlit` identical). Owed: the macOS twins of the two view goldens (M4). W4b and W4c are open.
- **W4b-1:** mechanized. A trailing `views` array in `dump-graph`/`render.dump_graph` (`dump_graph_json_with_views`,
  `dump_scene_frame_graph_json`), `view:<name>` and `view:<name>/<pass>` rows in the offline profile
  (`Renderer::dump_view_timings_json`, a separate loop after the measured loop so the cull ring never sees a view),
  `Budget.views` / `[views]` (`L4005`/`L4019`/`L4006`, no new code), and `budgets/pinned-rtx3090-render-views.toml` (`radar = 0.42`,
  measured 0.2139 / 0.2092 / 0.2064 ms on the RTX 3090, 2026-11-22). View-free scenes dump byte-identical output (`meshes`,
  `post-full`, `ui-hud`); the four view scenes differ only by the appended `views` key. No schema, replay, golden or hash
  moved. API break: `Budget` gains a pub field. Owed: the macOS twin of the budget (M4) and the TeamCity step's
  re-provisioning. W4b-2 and W4c are open.
- **W4b-2:** mechanized. `BlendData.views` and `BlendData.layered` (`ludion-renderer`), `blend_camera`/`blend_items` factored out of
  `blend` and used for the main frame, each view camera and a layered frame's unfiltered source, `ViewTable.entities` and
  the extract fill of both (`ludion-engine`), docs `render-views` and `camera`. `NoInterpolate`/`PresentCuts` cut a view camera;
  `alpha >= 1` is still `next.clone()`. No `WorldHash`, replay, schema, `SnapshotDoc` or golden moved. API break: `BlendData` gains two
  pub fields (a struct literal needs `..Default::default()`; none found outside `crates/`). W4c is open.
- M4 (2026-11-22, Lore 2449 verified on the pinned Apple M4): the macOS twins of `render-view-unlit`, `render-view-portrait` and `render-view-radar` are blessed (r2451); the full macOS gallery re-renders byte-identically.
