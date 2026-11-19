---
description: The render pipeline: passes, GPU resources, and deterministic PNG output on the frame graph
---
# ludion-renderer

**Scope:** The wgpu render pipeline — the frame graph, the `RenderScene` plain-data input, pipelines and shaders, per-frame upload, and pass recording. Scheduling, transient aliasing and readback are `ludion devdocs ludion-render-graph`; PNG decoding is `ludion devdocs ludion-assets`; golden verification of the output is `ludion devdocs testkit-golden`.

## Spokes
- `ludion devdocs renderer-passes` — the per-pass walk and the composite imaging chain.
- `ludion devdocs renderer-shading` — the Cook-Torrance mesh shading model in `mesh.wgsl`.
- `ludion devdocs renderer-ibl` — image-based lighting: the sky prefilter and the split-sum sampling.
- `ludion devdocs renderer-medium` — fog and the light shafts as one participating medium.
- `ludion devdocs renderer-shaders` — shader authoring: generated bindings, shared snippets, hot reload.
- `ludion devdocs renderer-clustering` — the uncapped light payload, the cell grid, and its binning.
- `ludion devdocs renderer-quality` — per-machine quality: `PostChain` masking and the FULL golden pin.
- `ludion devdocs renderer-gi` — the shared GI bake machinery (probe lattice, DDA march, cache) and sky visibility's consumption.
- `ludion devdocs renderer-gi-bounce` — the one-bounce radiance bake: L_out, the light signature, the three-volume storage, `gi_bounce`.
- `ludion devdocs renderer-frame-graph` — the frame graph's shape: pass chains, which extent sizes which target.

## Layout
- `crates/ludion-renderer/src/lib.rs` — re-exports.
- `crates/ludion-renderer/src/graph.rs` — `build_frame_graph(FrameSize, PostChain)`, `RenderScale` (a validated `[0.25, 4]` client factor, never scene content), `FrameSize::resample` (which resampling pass the two extents call for), `FrameSize::validate` (the extent gate — L2012 unless every output and render axis is 1..=8192 px; the five `Renderer` entry points and `Engine::frame_size` all reach it), and the `pass` label constants.
- `crates/ludion-renderer/src/scene.rs` — `RenderScene` and its parts: the plain data extract hands over.
- `crates/ludion-renderer/src/render.rs` — `Renderer`, the `SHADERS` registry, per-frame upload, and `FrameRecorder` (the `PassRecorder` impl). `from_gpu` builds one on a `Gpu` the caller already has (the surface-chosen context; `new()` is that over a headless `Gpu::request`), and `render_frame` is the general form of `render_streamed` — same frame, `FrameSinks` naming where its finished texture goes. Also owns the per-draw material `@group(4)` (five texture slots + sampler + `MeshMaterial` uniform) and its content-hash-keyed texture cache (`Renderer::uploads()` counts every cache-miss upload: textures, mesh maps, mesh assets) — `ludion devdocs renderer-shading` has the shading side.
- `crates/ludion-renderer/src/local_shadow.rs` — the per-frame local-light shadow plan: candidates, camera-distance rank, the 8×8-cell atlas budget (pure CPU).
- `crates/ludion-renderer/src/sun_cascade.rs` — the sun's cascades: the pure camera fit (`fit_sun_cascades`), the atlas grid and `sun_cascade_casters`.
- `crates/ludion-renderer/src/debug_view.rs`, `src/render/irradiance_view.rs` — the debug views; `irradiance` is a mesh scene-pass twin (`IRRADIANCE_VIEW` in `mesh_shading.wgsl`, built lazily beside the `SUN_CASCADE_VIEW` twins) whose `irradiance_debug` pass blits `scene` in place of `composite`. It is off the wire (`ludion-engine`'s `wire_view` maps it to `None`); `light_exposure_agreement.rs` gates it against `light_exposure`.
- `crates/ludion-renderer/src/cull.rs` — the CPU oracle for the GPU cull: bounds, frustum and sphere tests (tests, `--debug-draw cull`, cascade caster counts).
- `crates/ludion-renderer/src/gpu_cull.rs` — the GPU cull's buffers, bind groups and per-frame dispatch (`ludion docs culling`).
- `crates/ludion-renderer/src/cull_stats.rs` — the non-blocking readback ring behind the profile `culling` section.
- `crates/ludion-renderer/src/render/mesh_batch.rs` — world-mesh batching: instance and bucket layout the cull and indirect draws consume.
- `crates/ludion-renderer/src/render/silhouette.rs` — the occluded-silhouette mask's direct draws and the composite's silhouette uniform lanes.
- `crates/ludion-renderer/src/render/static_batch.rs` — the persistent static instance and cull-item buffers (quiet / patch / rebuild / remap), GPU-copied each frame into the leading slots of every bucket (`Plan/gpu-driven-culling.md` decision 49).
- `crates/ludion-renderer/src/shaders/cull.wgsl` — the reduce-then-scan cull compute shader.
- `crates/ludion-renderer/src/ibl.rs` — the sky prefilter and its cache (`ludion devdocs renderer-ibl`).
- `crates/ludion-renderer/src/mesh.rs` — procedural cube/plane/sphere geometry, non-indexed triangle lists.
- `crates/ludion-renderer/src/compressed.rs` — the cooked-map (BC7/ASTC 4×4) wgpu format mapping and `CompressedUploadError` validation; the upload itself is in `render.rs`.
- `crates/ludion-renderer/src/mipgen.rs` — GPU mip generation for uploaded textures, a blit chain reusing `blit.wgsl`.
- `crates/ludion-renderer/src/shaders/` — one WGSL body per pipeline, plus `ibl.wgsl`'s three compute entry points, which is deliberately not a `SHADERS` row.
- `crates/ludion-renderer/src/shaders/snippets/` — fragments composed ahead of a body (`ludion devdocs renderer-shaders`).
- `crates/ludion-renderer/src/shader_gen.rs` — **generated, never hand-edited** (`ludion devdocs renderer-shaders`).

## Rules
- **Input is exactly one `RenderScene`** — plain data: baked model matrices, decoded RGBA8 textures by index, resolved camera matrices, already-anchor-resolved pixel UI. No ECS reference, no asset path, no `Anchor` concept; the crate depends on neither `ludion-ecs` nor `ludion-scene` (`engine-architecture.md` §2). `RenderScene` **is** the extract boundary, which a `wgpu` type leaking into it would dissolve — keep those inside this crate and `ludion-render-graph`'s execute layer.
- **All passes are `PassKind::External`.** The graph owns scheduling, transient aliasing and the JSON dump; this crate records the GPU work through the `PassRecorder` seam. The `graph.rs::pass` label constants are shared between graph construction and the recorder's match arms, so the two cannot drift.
- **Content decides the graph, which is what makes an unaffected scene byte-identical by construction.** `PostChain::for_scene` answers every flag in one place (`ui`, `sun`, `ssao`, `shafts`, `dof`), and each false flag leaves the graph with none of that flag's passes; the resampling pass is not a flag at all — `FrameSize::resample()` is what adds `resolve` or `upscale`. A UI-free, sunless scene gets the fixed 13-pass graph and byte-identical pixels. Structural, not coincidence.
- **The sky and IBL add no pass.** An `Environment` drives a fullscreen sky fill *inside* the `scene` pass plus three textures `mesh.wgsl` samples, so `build_frame_graph`'s signature and pass count are unchanged either way. Without one, 1×1 fallbacks bind, the sky draw is skipped, and the clear-color background and flat-ambient math stay byte-identical.
- **No temporal accumulation — test mode is the only mode.** The same scene on the same adapter yields byte-identical pixels across processes.
- **Headless by default, and a window never becomes a render target**: `Renderer::new()` goes through `Gpu::request` (L2003/L2004) and its output is a `RenderedImage` and a blocking readback. A windowed host builds the same renderer with `from_gpu` over `Gpu::request_for_surface` and asks `render_frame` for the present sink, which blits the offscreen output after the frame's own submit — so the presented pixels are the readback's pixels and the recorded passes are unchanged.
- Every `RenderScene` field derives `PartialEq`, so two extractions of one world can be asserted equal.
- **Lights are uncapped — clustered forward.** Every placed light rides the group-1 storage buffers in ascending-`EntityId` extract order and a fragment iterates only its own cell's index list; the order is load-bearing, so never reorder `RenderScene.lights`. Render-only — no `WorldHash` or wire impact. The grid, the binning and its proof obligations are `ludion devdocs renderer-clustering`.

## Verify
```
cargo test -p ludion-renderer        # graph shape, scene types, doc examples (dump is CPU-only)
cargo run -p ludion-cli -- render --scene examples/gallery/meshes.yaml --out target/meshes.png
cargo run -p ludion-cli -- render --scene examples/gallery/lights.yaml --out target/lights.png --dump-timings target/timings.json
# Pass counts are derived from each scene's content, so shipping a new conditional pass moves the
# ones whose scenes author it. The command is the source of truth; these are what to expect today.
cargo run -p ludion-cli -- dump-graph --scene examples/gallery/meshes.yaml    # 13 passes (pyramid bloom), aliasing visible, no GPU
cargo run -p ludion-cli -- dump-graph --scene examples/gallery/ui-hud.yaml    # 14 passes: overlay after composite
cargo run -p ludion-cli -- dump-graph --scene examples/gallery/sun-shadows.yaml  # 14 passes: shadow prepended, scene samples shadow_map
cargo run -p ludion-cli -- dump-graph --scene examples/gallery/night-sky.yaml    # 14 passes: sky + IBL add none — the moon's DirectionalLight is what prepends `shadow`
cargo run -p ludion-cli -- dump-graph --scene examples/gallery/post-full.yaml    # 21 passes: shadow + the ssao trio + the shafts and dof pairs, all stacked
cargo run -p ludion-cli -- test --dir examples/gallery                        # full pipeline vs committed goldens
cargo run -p ludion-shader-bindgen -- --check   # shader-drift: fails if shader_gen.rs is stale
cargo test -p ludion-server --test shader_reload   # dev-flag reload: valid edit, L2008, L2009 keep-last-good
```
Rendering the same scene twice must produce byte-identical PNGs (compare hashes of two `render` runs). Every gallery golden whose scene does not carry the feature you changed must stay byte-identical: the conditional graph and the flag-off shader branches make that a structural guarantee, not a hope. Prefilter determinism: resolving the same `Environment` twice, in one process or across processes on the same pinned GPU, must produce identical cube/LUT bytes.

## Gotchas
- `MeshInstanceRaw.normal` carries the wrap-diffuse data in floats no shader reads: `n0.w` wrap, `n3.xyz` scatter colour, `(n1.w, n2.w, n3.w)` the instance's world half-extents. The packing exists in exactly two places, `instance_normal_lanes` (Rust) and `wrap_lanes` (`mesh.wgsl`); those floats are repurposed, so the next per-instance value gets a real lane and attribute, not more packing. The rim is that real lane: `MeshInstanceRaw.rim` at `@location(20)` (`color·strength rgb, power`, all-zero is no rim), instance stride 224 B, resolved per mesh in `extract.rs` (document rim, then `Mesh.rim`, then the nearest `Parent` ancestor's). Everything packed must be finite (`n3.xyz` is multiplied by `0.0`). The medium scale is the next real lane (`@location(21)`, flat, read only by `fs_medium`), resolved by the same `Parent` walk. When any mesh is below `1.0` on a fogged frame (`PostChain::mesh_medium_scheduled`), the scene pass gains a fourth target, `scene_medium` (`R8Unorm`, so a mesh's `m` is quantized to 1/255), written by the `fs_medium` twin of the mesh fragment; probe captures stay on `fs_capture`. When an exempt sprite or particle is drawn (`transparent_medium_scheduled`), the transparent layer is `Rgba16Float` with the coverage-weighted medium in B (otherwise `Rg16Float`). The composite reads them through `medium_gi.y` / `.z`; absent frames build the same graph and pipelines as before. The medium design: `Plan/volumetric-medium.md` decisions 16–20.
- A model matrix with a negative determinant (a reflection) reverses triangle winding, so mesh/capture/prepass/decal-receiver draws it through the shader's `mirrored` (`FrontFace::Cw`) pipeline twin (`ShaderPipelines`), each built in four variants (mirrored × double-sided, picked per draw; shadow casters stay culled-opaque by default, and the `mesh_material` snippet holds the material group those shaders share) and the mesh shader flips the tangent handedness via `fade.w`; `is_mirrored` is the one predicate for both, mirrored draws follow unmirrored ones in draw order, and a new culling mesh-geometry pipeline needs every variant. Every caster pass (sun map and cascades, local-light atlas, drop-shadow occlusion) draws through `draw_casters`, which picks the depth-only pipeline or the `shadow_cutout` one per draw (a `mask` mesh discards on the scene pass's coverage test; a double-sided one is cull-none).
- `FrameRecorder::record` panics on an unknown pass kind token — if you add a pass to `build_frame_graph`, add its label constant to `graph.rs::pass` and a match arm in `render.rs`, or every render dies at runtime.
- Sprite draw order is view-depth sorted, then batched by *consecutive* texture: interleaving textures at one depth splits batches but never reorders, and determinism rests on `sprite_order`, a total order (`total_cmp`, each view depth computed once) with an index tie-break. UI has no view depth at all — `extract()` delivers `RenderScene.ui` **pre-sorted** by `(order, EntityId)`, and `build_ui_batches` only ever batches *consecutive* same-kind/same-texture runs in that order.
- Both depth textures are graph resources, not renderer-private allocations: `scene_depth` (attachment + texture-binding, store-on-write — `composite` samples it for fog) and, on a sun frame, `shadow_map` (the same, sampled by the scene pass). Both appear in `dump-graph`'s resource list; `shadow_map` only when the scene carries a sun. The overlay pass has no depth buffer at all — UI layering is the sort, not a depth test.
- `shadow_map` is the first graph resource a *later pass actually samples* rather than merely reads back, so it is `TextureDesc::sampled_depth`, not `TextureDesc::depth`. Picking `depth` compiles, then leaves `scene`'s shadow-map binding pointing at a texture with no `TextureBinding` usage — a wgpu validation error at bind-group creation, not a graph-level one, because the render-graph crate does not know what a shader will bind.
- `TextureImage.hash` is the content hash of the *source file bytes*, not the decoded pixels — that is the intended texture-cache/dedup key. `UiRect`'s texture reuses the same cache and the sprite path's white-texture fallback for `texture: None`.
- Text: the renderer never lays text out — extract hands it `GlyphQuad`s from `ludion_font::layout` (`ludion devdocs ludion-font`), copied into vertices verbatim (never re-add or re-round coordinates). `TextRun.quads` are world XY (`YAxis::Up`, `pos[1]` above `pos[3]`), `UiInstance::Text.quads` pixel space. `font: None` draws the embedded atlas (`ludion_font::EMBEDDED_PNG`, one dedicated upload, nearest-sampled); `font: Some(FontDraw { texture, sdf })` draws a table face's atlas from `RenderScene.textures`. Text batches split on a face change, as UI rects do on a texture change (`build_text_draws`, `build_ui_batches`), each batch binding its own atlas group: nearest for bitmap, linear for SDF. The text shaders compute the sampled alpha *and* the thresholded SDF coverage (`smoothstep` over `fwidth`, edge 0.5), then `select` on the per-vertex `sdf` flag — both stay in uniform control flow, and the bitmap result is the sampled alpha untouched. `TextRun.extent` is the laid-out block, which `bounds.rs` reads instead of re-laying out.
