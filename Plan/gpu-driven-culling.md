# Plan: Culling, the visibility flag, and how far toward GPU-driven rendering

> Status: **DESIGN — not settled, open for grilling.** Written 2026-09-30 as the input to a grill-me
> session. Nothing here is decided until the user rules on "Questions for the grilling". Every
> current-code fact below was read from the tree on 2026-09-30. Every measurement was taken the same
> day on the pinned RTX 3090 box (the Vulkan backend: that is the adapter wgpu picks there), release
> build, 1920×1080.
>
> **Source.** `Plan/engine-gap-map.md` "Recommended order" item 3, second bullet: frustum culling and
> a visibility flag. `Plan/world-streaming.md` names these as W0 (measure) and W1 (culling in
> extract), and says neither needs a trigger. The user's direction widens the question: explore a
> GPU-driven pipeline in the spirit of Nanite, "maybe not that far", where culling and draw decisions
> run on the GPU, not the CPU. This plan lays the whole ladder out, from a CPU frustum test up to
> Nanite-style cluster LOD, and recommends where to stop.
>
> **Relationships:**
> - `Plan/world-streaming.md`: this plan would **absorb its W1** (culling) and share its W0 (the
>   composed-city measurement). Its W5 voxel LOD stays there; this plan only makes sure the culling
>   design has a slot for per-instance LOD selection.
> - `Plan/clustered-forward.md`: its decision 1 (one lighting architecture, forward, deferred
>   rejected) rules out a visibility-buffer material pass, and that is what the Nanite rung needs.
>   Its decisions 3 and 7 (a CPU build first, GPU only behind a measurement, and a degenerate build
>   kept as the byte-identity oracle) are the pattern this plan copies.
> - `Plan/shadow-mapping.md`: the shadow window is an explicit orthographic box (no auto-fit), which
>   makes caster culling exact and cheap.
> - `Plan/character-controller.md` W1 shipped world-space debug draw: host-enabled categories,
>   `--debug-draw`. This plan uses it to make culling visible to an agent.
> - The transform-hierarchy grilling (gap map item 3, first bullet) owns whether a hidden parent
>   hides its children.
> - `Plan/scalability.md`: a distance cutoff would be one more render-only quality axis.

## The problem, measured

### What the code does today

- **Extract walks every entity every frame.** `extract_inner` (`crates/ludion-engine/src/extract.rs`)
  collects every `EntityId`, runs `place` over all of them (world matrix, normal matrix, sprite
  sizing, the GI `dynamic` flag), then resolves each placed mesh into one or more `MeshInstance`s.
  Resolving means asset lookup, assembly part posing and skinned palette sampling. It does this
  with no camera test of any kind. Sprites are sorted back to front, particles evaluated in closed
  form, and text laid out, all regardless of visibility.
- **The renderer rebuilds every instance buffer every frame.** `build_mesh_batch`
  (`crates/ludion-renderer/src/render/mesh_batch.rs`) buckets instances by primitive
  shape or by `mesh_assets` index. Each bucket gets a **new** `VERTEX` buffer via `init_buffer`
  every frame, and each bucket is one `draw`/`draw_indexed` with `0..count` instances. Sprites are
  batched by `(layer, texture)` after a CPU sort.
- **Three geometry passes draw the same full list.** `record_shadow` (when a sun is present),
  `prepass` (when AO is on) and `record_scene` all loop over the same `mesh_draws` and
  `mesh_asset_draws`. The shadow pass therefore draws every mesh in the world, including those
  outside its own 2048² orthographic window.
- **Bounds exist, but they are not conservative.** `render.bounds` (`crates/ludion-engine/src/bounds.rs`)
  reuses `place` and reports a world AABB plus `on_screen` for each entity. For asset, voxel,
  sculpt and assembly meshes it uses a **unit box** under the model matrix ("the honest best-effort
  bound", chosen so the answer needs no asset load). A voxel prop bigger than one unit would be
  wrongly culled against that box.
- **The GPU surface is minimal.** `Gpu::request_from` (`crates/ludion-render-graph/src/execute.rs`)
  asks for `TIMESTAMP_QUERY` only, with `engine_limits()` = WebGPU defaults plus two raises. The
  frame graph has texture resources and a compute pass kind (used by `gi_inject`, which writes a
  storage texture), but **no buffer resource kind**. The engine's only storage buffers are the
  cluster tables, which are bound outside the graph.
- **The ECS has no change detection, and the freeze list rejects adding it** (`engine-architecture.md`
  §4). Any "upload only what changed" design has to find another way to know what changed.

### Numbers

| Scene | Renderables | `extract` median | GPU `scene` | GPU `shadow` | GPU `prepass` |
|---|---|---|---|---|---|
| vigil `district-market` (224×224 u ward, 103 authored meshes, 53 lights) | ~200 | 0.153 ms | 0.190 ms | 0.020 ms | 0.036 ms |
| vigil `budget-horde` (1 046 meshes, 500 animated enemies, 342 lights) | ~1 100 | 0.889 ms | 1.032 ms | 0.088 ms | 0.170 ms |
| synthetic, 10 000 cubes over 850×850 u (the W0 city footprint), vigil camera, sun | 10 000 | 5.82 ms | 0.281 ms | 0.131 ms | — |
| synthetic, 10 000 mixed cube/sphere (spheres are 24×16, non-indexed) | 10 000 | 6.06 ms | 1.738 ms | 0.683 ms | — |
| synthetic, 40 000 mixed | 40 000 | 22.46 ms | 7.133 ms | 2.720 ms | — |

The vigil rows come from the game binary's own `--profile 60`; the synthetic rows from `ludion
profile --frames 60` on throwaway scenes (not committed). `budget-horde`'s `scene` time is dominated
by its 342 lights (clustered shading), not by geometry.

**What the numbers say:**

1. **At vigil's current scale, culling is not a perf emergency.** One district costs 0.15 ms of
   extract and about 0.25 ms of geometry passes. The frame budget is 16.6 ms.
2. **Most of what vigil draws is off-screen.** The follow camera sits 30 u out at a 58° pitch with a
   50° vertical FOV (`games/vigil/src/tunables.rs`). Its ground footprint is a trapezoid about 36 u
   deep and 42–77 u wide, roughly 2 150 u². A 224×224 u ward is 50 000 u², so about **4 %** of the
   floor is in view. Wall height widens that a little. A frustum cull would drop on the order of
   90 % of static instances from the scene pass and prepass. The shadow window's 80×80 u box would
   drop a similar share from the shadow pass.
3. **At world-streaming scale, extract is the wall, not the GPU.** Extract costs a flat ~0.6 µs per
   renderable, whatever its shape. That is 6 ms at 10 k and 22 ms at 40 k, which blows the frame
   budget alone. GPU cost depends on vertex count: 10 k cubes cost 0.41 ms across both geometry
   passes, while the same count with half spheres costs 2.4 ms. **A frustum cull fixes the GPU
   side completely at this scale.** With about 0.3 % of an 850 u world in view, both passes fall to
   near-constant. **It does not fix extract,** because extract still walks and places every entity
   before any test can run.
4. **GPU culling pays only once the CPU stops touching invisible instances.** A CPU AABB-vs-frustum
   test costs about 10–20 ns per instance, roughly 0.1–0.2 ms at 10 k. That is small next to the
   6 ms extract walk. Moving the *test* to the GPU saves that 0.1–0.2 ms and nothing more, as long
   as extract still rebuilds and uploads every instance every frame. The real prize is a
   **persistent scene**: static instances uploaded once, never walked by extract again, and culled
   where they live, on the GPU. That prize needs a notion of "static", which the ECS does not have.

The out-of-scope costs are recorded so no one mistakes them for this plan's problem. `world_hash`
measured 4.97 ms on `budget-horde` and 23.9 ms on the 10 k synthetic scene. It is sim-side, linear
in entities, and `Plan/world-streaming.md` already lists it. Culling cannot touch it.

## What wgpu 30 actually offers on the two pinned platforms

These facts come from the pinned crates' own docs and backend code (`wgpu-types` 30.0.1
`features.rs` and `instance.rs`, `wgpu-hal` 30.0.1 `metal/command.rs`), not from memory.

| Capability | wgpu surface | RTX 3090 (Vulkan, today's backend; DX12 also available) | Apple M4 (Metal) |
|---|---|---|---|
| Compute shaders, storage buffers, `draw_indirect` / `draw_indexed_indirect` | core WebGPU | yes | yes |
| `multi_draw_indexed_indirect` (fixed count) | core wgpu | native | **emulated**: `wgpu-hal`'s Metal backend loops `draw_count` separate indirect draws on the CPU |
| `multi_draw_indexed_indirect_count` (GPU-written draw count) | `Features::MULTI_DRAW_INDIRECT_COUNT` | yes (Vulkan 1.2 / DX12) | **no**: the Metal functions are `//TODO` no-ops |
| Non-zero `first_instance` in indirect args | `Features::INDIRECT_FIRST_INSTANCE` | yes ("Vulkan (mostly)") | yes (Mac1+) |
| Subgroup ops | `Features::SUBGROUP` | yes | yes |
| 64-bit atomic min/max (buffer) | `Features::SHADER_INT64_ATOMIC_MIN_MAX` | yes (`VK_KHR_shader_atomic_int64`) | yes (MSL 2.4+) |
| 64-bit image atomics (visibility buffer) | `Features::TEXTURE_INT64_ATOMIC` | yes (`VK_EXT_shader_image_atomic_int64`) | yes (MSL 3.1+) |
| Mesh / task shaders | `Features::EXPERIMENTAL_MESH_SHADER` | yes | yes, but **naga compiles mesh shaders only for Vulkan**; Metal and DX12 need passthrough shaders (MSL / HLSL) |
| Indirect-call validation | `InstanceFlags::VALIDATION_INDIRECT_CALL` | **on by default even in release**: a validation compute dispatch clamps bad indirect args | same |

Notes that shape the design:

- **Metal is the binding constraint.** A GPU-written draw count does not exist there, and
  multi-draw is a CPU loop. A design that issues one indirect draw per mesh bucket (tens of
  buckets) and lets the GPU write only each bucket's `instance_count` works identically on both
  boxes, with **no optional feature at all**. A design built on `multi_draw_*_count` (Bevy's
  default) does not work on the M4. wgpu PR #9640 (open as of September 2026) lowers fixed-count
  multi-draw to Metal indirect command buffers. It does not cover the count variants.
- **Bevy's experience matches.** Bevy 0.16's release notes list Metal as "GPU transform ✅,
  multi-draw & GPU cull ❌".
- **iOS** (`Plan/ios-target.md`) is Metal too. The core-only design carries there for free.
- **Mesh shaders stay deferred** (`CLAUDE.md` "Deferred"). The naga restriction adds a concrete
  reason: on the M4 they would mean a second shader language beside WGSL, which breaks the bindgen
  single-source rule (`dev-docs/renderer-shaders.md`).

## The constraints every rung is scored against

- **Introspectable frame graph.** Every GPU step must be a declared pass with declared resources and
  edges visible in `--dump-graph`, with declarations free of wgpu types. A compute cull writing an
  indirect buffer means **the graph must learn buffer resources**. `Plan/clustered-forward.md`
  deliberately avoided that step.
- **Bit-exact goldens on pinned GPUs.** Culling that drops only instances with no visible fragment
  cannot move a pixel, **if it is conservative and preserves draw order**. Both conditions matter:
  - A non-conservative bound makes geometry pop, a visible bug.
  - Order matters for exact depth ties (coplanar faces under `LessEqual`) and for anything blended
    (sprites, particles, markers).
  - **An atomic-append compaction, the standard GPU-culling idiom, makes instance order depend on
    thread scheduling.** A tie-breaking pixel would then flip between runs: a flaky golden, which
    `CLAUDE.md` classes as a P0 bug. Any GPU compaction must be a **stable** scan. It must also not
    use single-pass decoupled look-back, whose forward-progress assumption Apple GPUs do not
    guarantee. A reduce-then-scan over two or three dispatches is portable and deterministic.
- **No temporal accumulation.** Classic two-phase HZB occlusion draws last frame's visible set first
  and tests everything else against that depth pyramid. Given conservative tests the final picture
  is correct. But **draw order, and so tie-breaking, depends on the previous frame's camera**, and a
  single-frame `ludion render` has no previous frame. Frame N's bytes would depend on frame N-1.
  That is temporal state entering the image, and the tiered-determinism pillar forbids it in test
  mode.
- **Headless and offscreen.** Nothing may assume a swapchain or a present cadence. GPU-side cull
  counters must reach reports through the existing readback ring, never a stall on the frame path.
- **The sim/render split.** Culling is render-only: never hashed, saved or recorded, and never able
  to change sim state. An *authored* hide flag is the opposite: it is sim state, hashed like any
  component, and replays. The two must never be confused in the API.
- **Agent verifiability.** An agent must be able to answer "what did the cull drop, and why",
  headless, without a human looking: counters in the profile report, a debug-draw view, a
  cull-from-another-camera mode, and a `render.bounds` that agrees with the cull.
- **Profile budgets** (`dev-docs/budget-files.md`). A new CPU cost gets a ceiling that is skipped
  when absent (the `cluster_build_ms` precedent). A new GPU pass joins `KNOWN_PASS_NAMES` and gets a
  `[passes]` line. The first budget that names an instance count is world-streaming W5's
  `city-stress`.
- **One way per task.** A GPU cull that ships replaces the CPU mesh cull. It does not sit beside it
  as a mode. The replaced path survives only as a test oracle, as clustered-forward's degenerate
  build did.

## The ladder

Each rung lists what it buys at Ludion's content scale, what it costs, and how it fits the
constraints. Content scale here means vigil's 3× city, voxel props and kit pieces, sprites and
billboards, CPU particles, one sun shadow map, and HD-2D low-poly.

### Rung 0 — the authored visibility flag (needed by every other rung)

A way for game code and scenes to say "do not draw this", independent of culling.

- **Option 0a: a `Visibility` component** `{ visible: bool = true, cast_shadows: bool = true }`.
  Absent means visible and casting. It applies uniformly to `Mesh`, `Sprite`, `Text`, particle
  emitters and ground markers on the same entity. It matches the `visible: bool` convention
  `UiRect` and the UI components already use. `cast_shadows: false` gives decals-like props that do
  not shadow. `visible: false` always wins over `cast_shadows` (decision 6), so there is no shadow-only proxy.
- **Option 0b: a `Hidden` marker** (zero-size, presence means hidden). Smallest possible surface,
  and insert/remove is the toggle. It cannot express shadow-only or no-shadow, which would need a
  second marker later.
- **Option 0c: a field on each renderable** (`Mesh.visible`, `Sprite.visible`, …). Duplicates one
  concept across five components, violating Pillar 6.
- **Option 0d: render layers / a camera mask.** Premature. Ludion has exactly one camera (`L2101`).

**Semantics to state explicitly:**
- The flag is **sim state**: hashed, saved, replayed, editable over the wire like any component.
- Culling is **render state**: never hashed.
- Extract honours the flag before any cull test, so a hidden entity costs no placement work beyond
  the flag read.
- Propagation: an AND down `Parent` (and so `ParentJoint`); a hidden ancestor always wins (decision 3).
- `render.bounds` still reports a hidden entity, with a new `hidden` marker. Selection and picking
  in the editor must still find it.
- The GI voxelizer ignores the flag. A hidden wall still occludes sky light, and the bake signature
  must not move when a prop is toggled. Settled as recommended (decision 5).

### Rung 1 — CPU frustum culling in extract

A conservative AABB-vs-frustum test per instance, against two volumes:

- **The camera frustum** (six planes from `view_proj`).
- **The shadow window** (the sun's explicit ortho box; a caster outside it cannot write the shadow
  map).

An instance is kept for the passes whose volume it touches, carried as a per-instance view mask
(`CAMERA | SHADOW`). The prepass shares the camera mask. The shadow pass loops only over casters
whose mask has `SHADOW` and whose `cast_shadows` is set. Lights are never culled by this test: the
cluster build already bins them per view, and an off-screen light lights on-screen pixels.

**Bounds sources.** These are the real work of the rung, since a wrong bound is a pop:

| Renderable | Bound |
|---|---|
| Primitives | exact local box (as `bounds.rs` has it) |
| Asset, voxel, sculpt meshes | the true local AABB of the `MeshData`, computed once at mesh load and cached beside it, replacing `bounds.rs`'s unit box. This fixes `render.bounds` too. |
| Assemblies | per part: the part mesh's AABB under its already-posed part matrix. Exact and animation-safe, because extract poses parts before building instances. |
| Skinned meshes | the posed joints are known in extract (the palette is CPU-built). Bound = union of posed joint origins inflated by a per-mesh radius, where the radius is the max bind-pose vertex distance from its dominant joint, computed at load. A safe fallback is to never cull skinned meshes (single digits per scene today). |
| Sprites | exact: `center ± right ± up`. |
| Particles | per-emitter AABB from its closed-form envelope (max lifetime × max speed + max size), culling whole emitters. Per-particle tests cost more than they save at 24 k particles. |
| Ground markers | model-transformed disc/capsule box. |
| Text, UI | never culled (screen-space or tiny). |
| Debug lines | never culled. |

- **Conservatism.** Plane tests use a small outward epsilon, so float disagreement at the frustum
  edge can only keep an instance, never drop it.
- **Order.** Order is preserved by construction, since the filter keeps survivors in ascending
  extract order.
- **Goldens.** Goldens stay **byte-identical by construction**, and that is the gate.
- **`render.view`** (the editor's host camera): the cull uses **the camera the frame is rendered
  from**. Otherwise a free camera would see holes.
- **Where it lives.** A pure GPU-free function in `ludion-renderer`'s scene types (`cull::classify(
  bounds, frusta) -> ViewMask`), called from extract. Extract is where the saving is: a culled
  instance skips asset resolution, assembly posing, **skinned palette sampling** (the horde's
  expensive part) and sprite texture resolution. The same function later becomes the GPU cull's
  oracle.

**What it buys:** the GPU side of every measured scene, fully. It also recovers part of extract:
resolve and pose work for culled instances, though not the `place` walk. For vigil today it is
small and positive: a 90 % cut of the scene pass and prepass instance count, and the shadow pass
cut to its window.

**Cost:** small, about a wave:
- bounds caching on `MeshData`;
- the skinned bound;
- the `Visibility` component (schema hash moves once);
- the view mask on `MeshInstance`;
- per-pass loops in the renderer;
- counters;
- the debug category;
- tests.

No new GPU feature, no graph change, no golden churn.

**Verification:**
- **Byte identity.** Goldens byte-identical on both boxes.
- **`culling` test.** Counts instances for a camera placed to exclude known entities (the
  world-streaming W1 gate).
- **Randomized oracle.** A randomized fixture renders N random instances under random cameras with
  culling on and with the cull forced off, and byte-compares. The clustered-forward oracle pattern:
  any conservatism hole shows as moved bytes.
- **Profile report.** Gains a `culling` section: `instances`, `scene_drawn`, `shadow_drawn`,
  `hidden`, `cull_ms`, gated as `cull_ms` with skip-when-absent.
- **Debug draw.** A `cull` category draws kept bounds green and culled bounds red. It is appended
  by extract into `RenderScene.debug_lines` when the host enables it, so `ludion render
  --debug-draw cull` makes the decision a screenshot.
- **Cull-from-game-camera.** With `render.view` in force, cull against the game camera while viewing
  from the free camera. The agent sees exactly the culled set from outside, the classic "freeze
  culling" debug view, and it is headless.

### Rung 1.5 — a CPU spatial index over static instances (CPU-driven, persistent buffers)

Rung 1 still walks every entity. At the composed-city scale that walk, not the test, is the cost
(finding 3). The CPU answer:

- Mark static renderables (question G6).
- Place them once per scene or region load into a persistent per-region instance buffer, sorted
  by `(bucket, cell)`.
- Keep a coarse cell grid (say 32 u) over them.
- Per frame, extract walks **only dynamic entities**. The renderer picks the visible cells'
  instance ranges from the static buffer and draws each range as a sub-slice of the persistent
  buffer, with CPU-known offsets and counts.

- **Buys:** extract becomes O(dynamic + visible cells), which is the fix for finding 3.
- **Costs:** a static notion and its enforcement, persistent buffers keyed by region (the natural
  unit, since world-streaming W2 loads and unloads regions whole), and a more complex renderer loop.
- **Draw count:** it grows with (visible cells × buckets), which could reach a few hundred draws.
  That is fine on both backends.
- **Determinism:** trivially deterministic, and no new GPU machinery.

This rung is the **CPU-driven alternative to rung 2b**. They solve the same problem.

### Rung 2 — GPU frustum culling with indirect draws

Split in two, because the halves pay very differently.

**2a — a GPU cull over the per-frame instance list.** Extract still produces every instance, and
the renderer still uploads them. What changes:

- A `cull` compute pass per view (camera, shadow) tests each instance's world AABB (uploaded
  beside it) and writes a visibility bit.
- A stable reduce-then-scan compacts survivors per bucket into an output buffer created with
  `STORAGE | VERTEX` usage.
- It writes each bucket's `instance_count` into an indirect-args buffer.
- The geometry passes issue **one `draw_indexed_indirect` per bucket**. The CPU binds each bucket's
  output slice at a CPU-known offset: bucket capacity is known on the CPU, only the count is
  GPU-written.
- This needs no `INDIRECT_FIRST_INSTANCE`, no `MULTI_DRAW_*`, and no feature beyond core WebGPU.

**The mesh vertex path is unchanged.** The compaction copies the same `MeshInstanceRaw` bytes the
pipeline already reads as per-instance vertex attributes. So `mesh.wgsl` and `shadow.wgsl` codegen
do not move, and **the GPU cull can be proven byte-identical against rung 1's CPU cull on every
golden, on both boxes**, the clustered-forward wave-2 proof shape.

- **Buys:** almost nothing on its own at Ludion's scale (finding 4). It saves the CPU test and moves
  the compaction off-CPU.
- **Costs:**
  - **a buffer resource kind in the frame graph**: declared size, usage (`storage`, `indirect`,
    `vertex`), producer and consumer edges, visible in `--dump-graph`, graph snapshot moved
    deliberately once;
  - a compute pass with multiple buffer writes;
  - a WGSL scan;
  - bindgen learning read-write storage and atomic-free compaction buffers;
  - GPU-side counters for the report, read back through the ring (one to two frames late, never
    stalling) and reported as medians;
  - indirect-call validation's own dispatch per frame, which has to be measured.
- **Portable:** identical on Vulkan, DX12, Metal and iOS.

**2b — the persistent GPU scene.** The payoff rung, the GPU twin of rung 1.5:

- Static instances go into a persistent per-region GPU buffer at region or scene load, and are
  freed on unload.
- Dynamic instances keep the per-frame upload.
- The cull pass runs over both. Extract never touches a static entity again.
- **LOD selection** slots in naturally: the cull shader picks a bucket per instance from camera
  distance, so each LOD level is its own bucket. world-streaming W5's voxel downsample chain rides
  this. Mesh LOD shipped in the rung-2a pass instead (decisions 21–29).

- **Buys:** the same as 1.5, plus culling cost that scales on the GPU, where 40 k tests are about
  0.02 ms. It also scales further than 1.5's cell ranges without growing the draw count, and it is
  the foundation for rung 3.
- **Costs:** everything in 2a, plus the static notion (question G6), persistent buffer lifetime
  tied to region load and unload, and static-edit invalidation. An editor gizmo moving a "static"
  wall has to re-upload it; that goes through the one command queue as an explicit invalidation.

### Rung 3 — HZB occlusion culling

Build a hierarchical-Z depth pyramid, then test each instance's screen-space bound against the
pyramid level that covers it in about four texels.

- **3a — classic two-phase** (Haar & Aaltonen, SIGGRAPH 2015; Bevy's experimental implementation,
  PR #17413). Draw last frame's visible set, build the HZB, test the rest, then draw the newly
  visible. **Rejected for Ludion as a shipping mode:** the image depends on history (see
  constraints), and `ludion render`'s single frame has none.
- **3b — same-frame occluder pre-pass.** Draw a depth-only pass of designated occluders: large
  static geometry, chosen by bound size or authored. Build the HZB from it, cull every instance
  against it, then draw survivors in canonical order in one pass. It has no history and is
  deterministic by construction. Its cost is one extra occluder depth pass. The existing AO
  prepass cannot serve, because it draws everything. It is still conservative, so goldens stay
  byte-identical.
- **Buys:** little for vigil. A 58° top-down camera looks *down* into streets, and the occlusion
  `Fade` system already dissolves the buildings that would occlude. It would matter for an
  eye-level camera in dense interiors: the platformer, or a street-level city.
- **Costs:**
  - mip-chain storage writes in the graph (a new resource shape);
  - a min-reduction compute chain;
  - the occluder heuristic;
  - screen-space bound projection in WGSL;
  - one more pass on the budget.
- **Status:** belongs behind a trigger.

### Rung 4 — meshlet / cluster culling

Split meshes offline into ~64–128-triangle clusters with bounding spheres and normal cones. Cull
clusters (frustum, backface cone, HZB) in compute, and draw visible clusters with a vertex-pulling
shader: a non-indexed draw where the vertex shader fetches cluster, index and vertex from storage.
This is Bevy's meshlet hardware-raster path, and Wihlidal's GDC 2016 compute-pipeline talk.

- **Buys:** per-cluster rejection inside large meshes. Ludion's meshes are small (voxel props, kit
  pieces, low-poly glTF, sculpts). Instance-level culling already takes almost everything a
  cluster cull would.
- **Costs:**
  - the mesh vertex path moves from 20 vertex attributes to storage fetch, so **every lit mesh
    golden is re-blessed once** on both boxes;
  - an offline clusterizer (meshoptimizer is C++; a pure-Rust port is an evaluation of its own);
  - cluster data joins the derived-content drift gates;
  - skinning moves into the fetch path.
- **Status:** rejected now, with a falsifier: content that ships meshes above ~50 k triangles each,
  measured as vertex-bound.

### Rung 5 — Nanite-style cluster LOD

A cluster DAG built by simplify-and-group, runtime selection of a cut where parent error exceeds a
screen-space threshold and child error does not, software rasterization of small triangles with
64-bit atomic min into a visibility buffer, and a material resolve pass. Sources: Karis, Stubbe
and Wihlidal, "Nanite: A Deep Dive", SIGGRAPH 2021 Advances; Bevy 0.14+ "virtual geometry", whose
`MeshletPlugin` requires 64-bit atomics among other native features.

- **The hardware can do it:** 64-bit buffer and image atomics exist on both boxes (table above).
- **It fails on four other counts:**
  1. **The visibility buffer plus material resolve is deferred-class shading.** Clustered-forward
     decision 1 rules out a second lighting architecture. Nanite's forward-compatible variant
     rasterizes a visibility buffer and then shades it anyway.
  2. **Mesh shaders are deferred,** and naga's are Vulkan-only. Nanite's hardware path wants them.
  3. **The content is the opposite of what it serves.** Nanite exists for film-density micro-polygon
     assets. Ludion's style is stylized low-poly and voxels, where the LOD that matters is a voxel
     downsample (world-streaming W5).
  4. **Its engineering is Ludion's largest renderer investment by an order of magnitude,**
     competing with every item the gap map ranks above it.
- **Status:** rejected, with a falsifier: the roadmap adopts dense scanned or sculpted assets as a
  content style, and the forward/deferred decision is reopened on its own terms.

## Recommendation

**Build rungs 0 and 1 now. Build rung 2 (2a then 2b) as the GPU-driven arc, scheduled with or right
after world-streaming W2, because regions are 2b's persistence unit. Hold rung 3b behind a
trigger. Reject rungs 4 and 5 with recorded falsifiers.** Rung 1.5 is 2b's CPU twin and is not
built if 2b is.

**Why this order.**

- **Rungs 0 and 1 first.** Every later rung needs the flag, the bounds, the view mask, the
  counters, the debug views and the byte-identity oracle. Together they take the GPU side of every
  measured scene. Costing about a wave with zero golden churn, they are the right first move
  regardless of how far the ladder goes.
- **Rung 2 as the exploration.** This is where the user's GPU-driven direction pays off honestly.
  It has to be framed truthfully: 2a is mostly foundation (the graph's buffer resources, the
  compute cull, the stable scan, indirect draws, GPU counters) and saves little by itself. 2b
  delivers the win at world-streaming scale, extract no longer walking static geometry, which no
  amount of culling achieves while extract walks everything. Choosing 2b over 1.5 is choosing the
  GPU-driven foundation (it scales further, LOD and occlusion build on it) over the cheaper CPU
  answer. The user's direction settles that choice, and its costs are stated above.
- **The core-only design (one `draw_indexed_indirect` per bucket, GPU writes counts only)** is
  chosen over `multi_draw_*_count`. It is the only shape that runs identically on the M4 and on
  iOS. It keeps the vertex path, so the proof is byte identity rather than a re-bless.
- **Rung 3b** waits for an eye-level camera slice with dense interiors, and a measurement.
- **Rungs 4 and 5** are the "maybe not that far" line. The reasons are architectural (forward-only,
  mesh-shader deferral, one shader language) and content-driven, not effort alone.

## Proposed waves

Each wave's gate is mechanical. "Both boxes" means the RTX 3090 now and the M4 through `Plan/owed.md`,
as usual.

### W0 — the measurement (shared with world-streaming W0)

Compose vigil's fourteen districts at real offsets (world-streaming's throwaway script) and record in
Status:
- extract median;
- instance count per pass;
- the share of instances inside the camera frustum and inside the shadow window across a scripted
  walk;
- a per-phase breakdown of extract's ~0.6 µs per renderable (place, resolve, pose, sprite resolve,
  allocation) from a temporary timing probe.

**Gate:** numbers recorded. **Decision it feeds:** whether a constant-factor extract optimization
comes before 1.5/2b (question G7), and the persistent-buffer sizes.

### W1 — the flag, the bounds, the CPU cull (rungs 0 + 1)

- `Visibility` (or the ruled variant), registered with schema and docs.
- True local AABBs on `MeshData` at load; `render.bounds` switched to them, with a `hidden` flag.
- Skinned, assembly, sprite and particle bounds as tabled.
- The `cull::classify` function; the camera and shadow view mask; per-pass loops.
- The `culling` profile section and the `cull_ms` budget line.
- The `cull` debug category; the cull-from-game-camera debug mode.
- A `culling` agent-docs topic.

**Gates:**
- every golden byte-identical on the 3090, M4 queued;
- the randomized cull-on/cull-off byte oracle;
- the `culling` count test;
- the schema snapshot moved once (new component) with the new hash recorded;
- `WorldHash` unchanged for scenes that do not author `Visibility`;
- `budget-horde` and the district budgets green, with `cull_ms` calibrated into the vigil budget
  file.

### W2 — the 0008 wave set: the graph learns buffers, the GPU cull lands (rung 2a)

Re-cut by decisions 7–20 below for the first real consumer, request 0008 (the M2 torture scene).
There is no W1 CPU cull to compare against: the GPU cull ships first and the CPU `cull.rs` becomes
the oracle. The steps, in order:

- **Bench first.** An ignored release bench with the cull off records the "before" numbers, the
  extract per-phase breakdown and the quad and decal costs (decision 20). The host off switch lands
  here, unused until the cull does (decision 19).
- **Graph and bindgen.** `ludion-render-graph`: a buffer resource kind (size, usage set, lifetime),
  producer and consumer edges scheduled like textures, a `--dump-graph` schema extension (snapshot
  updated deliberately), still free of wgpu types. Bindgen: read-write storage buffers. No aliasing
  or sub-allocation.
- **Bounds.** A local box table per mesh asset on the GPU, and the skinned world box (decisions 13,
  14). `cull.rs` uses the same skinned bound, so it stays an exact oracle.
- **5a, camera views.** `cull.wgsl`, multi-view by design: one `cull` node over all views, the stable
  reduce-then-scan per view and bucket, per-bucket `draw_indexed_indirect` in the camera scene pass
  and the AO prepass, against the exact camera of that render (decision 11). The viewmodel bypasses
  it.
- **5b, shadow views.** Spot shadows, the six point faces, the sun cascades and the legacy
  single-sun `record_shadow` window become views of the same pass (decision 12). The CPU prologue
  leaves the shipping path and stays as the oracle.
- **6a, profile.** The `culling` section (per-view instances in and drawn, read back through the
  ring) and `cull` in `KNOWN_PASS_NAMES`, with the indirect-validation cost measured.
- **6b, debug surfaces.** `--debug-draw cull` and `render.view` `cull_from` (decision 18).

Sprites, particles, billboard shots, decals and volleys keep their CPU path and are not culled
(decision 16).

**Gates:**
- every golden byte-identical on the 3090, M4 queued;
- a GPU-vs-CPU-oracle visibility-mask test over randomized fixtures, comparing bit masks read back;
- a repeat-render identity test (same scene rendered 20 times, identical bytes), which catches any
  ordering nondeterminism;
- `world_hash` unchanged;
- the graph snapshot diff reviewed, and the wire schema snapshot moved once with 6b;
- the bench "after" figures recorded in Status.

The pinned `[passes] cull` budget lines are owed in `Plan/owed.md`, because the M5 box is unpinned.

### W3 — the persistent static scene (rung 2b)

Opened for Serious Isaac 0039; G6 is resolved (decisions 36–48).

- **W3.0 Bench and `static-kit` gallery.** About 3k unscaled kit-style statics interleaved with dynamics, an ignored release bench in code (the `culling_bench` precedent) with `extract_ms` statics off and on. The "before" figures go in Status.
- **W3.1 ECS write journal.** `World::watch_with::<M>()` / `drain_watched::<M>()` (decision 41), the `ludion-ecs` budget raise.
- **W3.2 `Static` component, eligibility codes, map-kit `static: true`.** The component schema snapshot moves deliberately.
- **W3.3 `StaticScene` on the CPU.** Pure placement function, journal-fed cache, extract skip, static-first order, `set_static_scene`, every index consumer taught the static range.
- **W3.4 Persistent GPU buffer** as the cull's second input; statics draw uncull above the capacity clamp.
- **W3.5 Gates, budget, docs, close.**

**Gates:**
- goldens byte-identical;
- gallery `static-kit` on/off byte-identical, and a 20-repeat render identical;
- the static-invalidation test (edit a static wall over the wire, the next frame shows it moved);
- the GPU-vs-oracle mask test covers statics; `world_hash` unchanged for content without `Static`;
- the first **instance-count-naming budget** (world-streaming W5's `city-stress`, brought forward) plus an `extract_ms` ceiling from the bench (about 3× the median unpinned; pinned lines owed);
- the game's `perf_m2` extract delta at or below +0.5 ms composite p95 on a dressed horde-large-a (horde-500, gibfest, full-run-bot), as the game's own acceptance.

G7 stays a trigger, out of W3 (decision 38).

### Later, on triggers

- **Rung 3b** (same-frame occluder HZB). Trigger: a slice with an eye-level camera where the scene
  pass measures dominated by hidden geometry.
- **Distance cutoff** as a scalability axis, off at `full` so goldens never see it. It rides
  `Plan/scalability.md` when a low preset needs it.
- **Voxel LOD selection in the cull shader** shipped as decisions 30–35: the same selection, voxel models chained at load.

## Risks

- **Non-conservative bounds pop geometry.** This is the most likely real bug, and skinned meshes and
  animated assemblies are the exposure. It is mitigated by the byte oracle (any hole moves bytes),
  the skinned fallback (never cull) and the `cull` debug view. A pop found by a human is a missing
  oracle case, fixed by adding the case.
- **`render.bounds` answers move.** Swapping the unit box for true AABBs changes every asset
  entity's reported bound. Clients that snapshot it (tests, the editor) move once, deliberately, in
  W1.
- **Ordering nondeterminism sneaking in.** One `atomicAdd` compaction added later "for speed" would
  make goldens flaky. The repeat-render identity test is the tripwire, and the `culling` doc states
  the rule.
- **Metal emulation cost.** With one indirect draw per bucket this is tens of encoder calls, which
  is negligible. If a later design wants per-instance draws, Metal pays a CPU loop per draw. That is
  a reason to keep the bucket design, not a reason to switch.
- **Indirect-call validation.** It is on by default and adds a dispatch. Turning it off voids
  wgpu's bounds guarantee and on DX12 breaks `instance_index`. Keep it on and budget it.
- **Graph scope creep.** Buffer resources are a real extension of the declarative interface. They
  are held to what the cull needs: sized buffers, a usage set, edges. No sub-allocation and no
  aliasing policy until a second consumer (the GI injection is a candidate) asks for it.
- **The static notion vs the ECS freeze list.** A `Static` marker whose Transform is written by a
  system silently desyncs the persistent copy. The validation mode catches it in tests. Whether to
  also catch it at runtime is question G6.
- **Counter latency.** GPU counters arrive one to two frames late through the ring. Reports take
  medians, so this is harmless. A per-frame assertion on GPU counts must read back with a stall,
  which is allowed only in lockstep and test mode.
- **The macOS twin lags.** Every golden-identity claim is proven on the 3090 first. The M4 re-run
  is an `owed.md` line per wave, as for every renderer arc.

## Questions for the grilling

Each carries the recommended answer; the user's rulings become the Decisions section.

- **G1. How far up the ladder?** Recommended: rungs 0 and 1 now; rung 2 (2a, 2b) as the
  GPU-driven arc; 3b on a trigger; 4 and 5 rejected with falsifiers. The main alternative is to stop
  at rung 1 plus 1.5 (all CPU, no graph buffers). That is cheaper, but gives up the foundation the
  user asked to explore.
- **G2. When does rung 2 start?** Recommended: right after W1 for 2a (the foundation, its gate
  being byte identity), and 2b with or after world-streaming W2 (regions are its persistence unit).
  The alternative, 2a only when W0 shows extract over budget on the composed city, is the
  measure-first stance clustered-forward took. The user's stated direction argues for building it.
- **G3. The visibility flag's shape.** Recommended: `Visibility { visible, cast_shadows }`, absent
  meaning visible and casting (0a). One component covers hide, no-shadow and shadow-only, and
  matches the UI's `visible` convention. `Hidden` (0b) is smaller but needs a second marker the day
  shadow-only is wanted.
- **G4. Does a hidden entity still affect the GI bake and dynamic injection?** Recommended: yes for
  the bake (a hidden wall is still a wall, and the bake signature must not flicker with a toggle),
  and no for dynamic injection (a hidden `GiDynamic` body emits nothing). Alternative: hidden means
  gone from lighting entirely, which moves the bake on every toggle.
- **G5. Which camera culls under `render.view`?** Recommended: the camera the frame is rendered
  from, plus a debug switch to cull from the game camera while viewing from the free camera.
  Question: does that switch belong on `render.view` (a protocol field, schema hash moves), or only
  in the `--debug-draw cull` render path? Recommended: a `render.view` field, so the editor gets
  the freeze-culling view through the parity gate.
- **G6. What makes an instance "static" for the persistent scene (rung 1.5 or 2b)?** Options:
  1. An authored `Static` marker, emitted by `generate-map` for kit pieces, with its Transform
     writes a documented rule checked by a test/`--full` validation mode.
  2. Implicit: no `Velocity`, not `GiDynamic`, never written, inferred. The ECS freeze list rejects
     the change detection this needs.
  3. Everything a region's scene file spawns at load, until a system writes it, which also needs
     change detection.

  Recommended: option 1. It is explicit, text-visible and generator-emitted, with the validation
  mode as the backstop. Open sub-question: is writing a `Static` entity's Transform a runtime
  diagnostic (`L`-code, which costs a per-frame check) or test-only?
- **G7. Optimize extract's constant factor first?** Extract costs ~0.6 µs per renderable, even for
  a plain cube. There is likely a 2–3× constant-factor win (allocation, per-entity string-keyed
  lookups) independent of any culling. Recommended: W0 measures the breakdown, and any obvious
  constant-factor fix lands inside W1. It is not a separate arc, and it does not replace 1.5/2b,
  because a 3× cut to a linear walk is still linear in the whole world.
- **G8. Where does the cull live in W1: extract or the renderer's prologue?** *Superseded by
  decision 11: the cull runs on the GPU at render time, and extract does no camera-based skipping.*
  Recommended: extract.
  That is where the skipped work is (posing, skinning, resolution). It calls a pure function owned
  by `ludion-renderer` so the W2 GPU shader has a single oracle. The renderer-prologue alternative
  (the cluster-build precedent) keeps extract "what exists" pure, but saves nothing on the CPU.
- **G9. Culling sprites, particles and markers.** Recommended: CPU-culled in W1 (exact quad and
  emitter bounds), and never moved to the GPU. They are sorted or blended, a GPU compaction would
  have to preserve a CPU sort, and their counts are bounded by the stress budgets already.
- **G10. Is `Visibility` added to vigil in W1?** Recommended: no content change in W1. The
  component ships, and vigil adopts it only where a game system wants hide or reveal: spawners, safe
  zones, set dressing. A game that authors nothing is byte- and hash-identical.
- **G11. Should `render.bounds` report the cull decision** (`culled_camera`, `culled_shadow`) as
  well as `on_screen`? Recommended: yes, in W1. It is the headless, wire-level answer to "why is X
  not drawn", and costs one schema-hash move shared with the `hidden` field.
- **G12. Rung 3a (two-phase HZB with last-frame reuse): confirm rejected as a shipping mode?**
  Recommended: rejected. It makes a frame's bytes depend on the previous frame's camera, which is
  temporal state in the image, against the tiered-determinism pillar. If occlusion is ever
  triggered, the same-frame occluder variant (3b) is the shape.

## Sources

- Karis, Stubbe, Wihlidal, "Nanite: A Deep Dive into Nanite Virtualized Geometry", SIGGRAPH 2021
  Advances in Real-Time Rendering.
- Haar, Aaltonen, "GPU-Driven Rendering Pipelines", SIGGRAPH 2015 Advances (two-phase occlusion,
  cluster culling in Assassin's Creed Unity).
- Wihlidal, "Optimizing the Graphics Pipeline with Compute", GDC 2016.
- Bevy 0.16 release notes, "GPU-Driven Rendering" (https://bevy.org/news/bevy-0-16/): the
  per-platform support table, Metal "multi-draw & GPU cull ❌".
- Bevy PR #17413, experimental two-phase GPU occlusion culling
  (https://github.com/bevyengine/bevy/pull/17413).
- Bevy meshlet / virtual geometry (0.14+), `MeshletPlugin`'s required native features.
- wgpu 30: `wgpu-types` `Features` docs (`MULTI_DRAW_INDIRECT_COUNT`, `INDIRECT_FIRST_INSTANCE`,
  `SUBGROUP`, `SHADER_INT64_ATOMIC_MIN_MAX`, `TEXTURE_INT64_ATOMIC`, `EXPERIMENTAL_MESH_SHADER`),
  `InstanceFlags::VALIDATION_INDIRECT_CALL`, and `wgpu-hal`'s Metal `draw_indexed_indirect` loop and
  `draw_indexed_indirect_count` stub.
- wgpu PR #9640, Metal indirect command buffers for fixed-count multi-draw (open)
  (https://github.com/gfx-rs/wgpu/pull/9640).
- Tellusim, "MultiDrawIndirect and Metal" (https://tellusim.com/metal-mdi/).

## Decisions

Decisions 1–6 are rung 0, amended by the owner (Clockout 0030). Decisions 7–20 were ruled on
2026-11-07 for request 0008, GPU frustum culling for the M2 torture scene (500 enemies, ragdoll parts
and 5,000 projectiles in a room of about 40 m, 5.0 ms render budget). They cover only what 0008
needs: G6 (the static marker), rung 2b (the persistent scene), the world-streaming coupling, G7 and
G11 stay open with their recommendations as defaults. Decisions 36–48 were ruled on 2026-11-13 (owner
grilling, Serious Isaac 0039) and resolve G6 and open rung 2b.

1. Rung 0 ships ahead of the grilling.
2. G3 is settled as option 0a, the `Visibility` component.
3. Propagation is an AND through `Parent` and `ParentJoint`: a hidden ancestor always wins, and a part's own `visible: true` never shows it. `cast_shadows` folds by AND the same way, down the `Parent` chain. This replaces the "hides only itself" rule.
4. The flag covers point and spot lights, decals, `DropShadow`, ground markers, particles, volleys, prepasses, every shadow pass and probe captures.
5. G4 as recommended: the static GI bake ignores the flag, dynamic injection honours it.
6. Q3 reversal: `visible: false, cast_shadows: true` is **not** a shadow-only proxy, because `visible: false` always wins. Revisit when a consumer asks for a shadow-only proxy: add an explicit mode, never a reinterpretation.
7. **Scope.** Only what 0008 needs. The rest of the ladder stays open, with its recommendations as defaults.
8. **Rung.** 2a straight away. There is no shipping CPU camera cull, because GPU culling is the long-term path. The CPU `cull.rs` becomes the **test oracle only**.
9. **Rung 3a rejected** as a shipping mode (G12): it puts temporal state in the image. **Rung 3b** (same-frame occluder) stays behind a trigger: an eye-level slice whose scene pass is measured as dominated by hidden geometry.
10. **Rungs 4 and 5 rejected**, with the falsifiers already recorded above: meshes over about 50k triangles measured as vertex-bound (rung 4); dense scanned assets plus a reopened forward/deferred decision (rung 5).
11. **Where and when.** At render time, per rendered frame, against the exact camera of that render: `late_latch`, `present_interpolation` re-renders and `render.view` included. Extract does **no camera-based skipping**, because the presenter re-renders an extracted scene with a newer camera and an extract cull would pop on flicks. This supersedes G8.
12. **One way.** The GPU cull serves every per-frame mesh view: the camera scene pass, the AO prepass, spot shadows, the six point faces, the sun cascades and the legacy single-sun `record_shadow` ortho window (uncull today). Probe-capture faces and the `decal_receivers` redraw are not included; they join only on a measurement.
13. **Skinned bounds.** The union of the palette's posed joint origins, inflated by a per-mesh radius (the max bind-pose vertex distance from its joint, computed once at load). This covers `SkinBone` ragdolls.
14. **Bounds upload.** A local box per mesh asset, transformed on the GPU by the instance model (Arvo, as in `cull.rs`). Skinned instances carry a CPU-computed world-box override. This is the shape rung 2b will need.
15. **Dispatch.** One `cull` pass over all views writes a visibility bit per view, then a **stable** reduce-then-scan compaction runs per view and bucket: no atomic append, no decoupled look-back. Per-bucket `draw_indexed_indirect` calls follow, with CPU-known offsets and GPU-written counts. Core WebGPU only, identical on Metal and iOS. One node in `--dump-graph` and one `[passes] cull` line; the pinned budget lines are owed (the M5 box is unpinned).
16. **Coverage.** Meshes only, projectile mesh looks included. Sprites, particles, billboard shots, decals and volleys are not culled. The bench reports their cost, and they are culled (CPU, per G9) only if they show up. The viewmodel (`MeshInstance.viewmodel`) never enters the world cull.
17. **Extract.** Measured, not changed. The bench records the per-phase breakdown. A constant-factor fix or an animation-visibility (palette) skip becomes a trigger, acted on only when the budget is breached.
18. **Agent surfaces.** A profile `culling` section (per-view instances in and drawn, read back through the ring, medians); a `--debug-draw cull` category (kept green, culled red, from the CPU oracle) that shows the **rendered camera view only**; and on `render.view`, `cull_from: view | game | frozen`, default `view`. `game` culls from the live game camera; `frozen` captures the cull camera on the command that sets it. These are debug and editor surfaces only: a game never issues `render.view`. The wire schema hash moves once.
19. **Off switch.** A host render setting (the `RenderSettings` pattern), not on the wire and not a quality axis. Shipping is always on. The oracle tests and the bench's "before" numbers use it.
20. **Measurement.** An ignored release bench in `ludion-engine`, built in code (the nav-bench precedent): 500 skinned enemies, voxel-ragdoll parts, a pool of 5,000 shots, decals, AO on, a room of about 40 m. It reports extract and GPU scene, prepass and cull with the camera facing a wall and facing the crowd, cull off and on. Its budget is about 3× the median on the unpinned box; pinned figures are owed.
21. **LOD asset (request 0013).** A base `*.mesh.yaml` carries an optional `lods:` list of up to three coarser sibling mesh documents, each with a finite, positive, strictly decreasing `screen_size`. A level must match the base's draw key (alpha mode and cutoff, `double_sided`, `rim`, texture slots) and, when skinned, its exact joint list, so it draws through the base's pipeline, bucket and bone palette. `L1395` is a malformed chain, `L1396` a mismatched level.
22. **Screen size.** The fraction of viewport height covered by the instance's bounding sphere (centre and half-diagonal of the world box the cull already computes). Perspective `size = r·k / max(d, r)`, orthographic `r·k`, `k = proj[1][1]·2^-lod_bias`. For a skinned instance `r` is the half-diagonal of its asset's rest box times the model's largest axis scale (the Settled block below), not the posed box's. The level is the count of thresholds above `size`, plus the view's level offset, clamped. Stateless, and from distance, not depth, so turning never changes a level. `cull::lod_level` is the CPU oracle the GPU result is tested against.
23. **Draw layout.** Survivors stay in their bucket's region of each view's slice, ordered by level, one `draw_indexed_indirect` per level whose GPU-written `first_instance` starts that level's run, so the instance stride (224 B) and the view cap do not move. The device requests `INDIRECT_FIRST_INSTANCE` when the adapter offers it (both pinned boxes do); without it every instance draws level 0. The cull dispatches one lane per (view, level), and with no chain in the frame every buffer, dispatch and draw is today's. Rejected: a per-level worst-case region (breaks the view cap) and degenerate-vertex draws (wastes coarse vertex work on fine instances).
24. **Shadow views** take the camera's eye and `k`, never their own frustum, with `Camera.shadow_bias_levels` (`0..=3`) as the level offset, so a shadow is never finer than its mesh. `render.view` takes the game camera's values.
25. **Knobs.** `Camera.lod_bias` (`-4..=4`) and `Camera.shadow_bias_levels`, both optional and omitted when absent so scenes and `WorldHash` are unchanged (the wire schema hash moved once); `QualityParams.lod_bias` (0 at `full`/`high`, 0.5 `medium`, 1 `low`) only coarsens. The camera field is what `render --sweep` reaches.
26. **Off switch with a chain.** `set_frustum_culling(false)` keeps the pass when the frame draws a chain, with the frustum test skipped, so culling on and off stay byte-identical. Over-cap views, probe captures and the viewmodel draw level 0; the decal-receiver redraw follows the camera view's levels (review fix: the cull stamps each compacted copy's bucket-local source index + 1 into the medium lane's spare `y`, and the receiver shader maps back through it; no new buffer or stride). A device without `INDIRECT_FIRST_INSTANCE` draws level 0 everywhere, and `--debug-draw cull` tints as level 0. Generated mesh levels keep the base's normals (flat only when it has none). A voxel level that halves to no solid cell is `L1398` from `check-scene`.
27. **Generation.** `ludion import --lod <screen>[:<ratio>]` (repeatable; an `[[import]]` `lod` list): a ratio runs meshoptimizer's simplify on the base with vertices kept by reference, so skin weights carry over; without one the glTF's `<name>_LOD<n>` sibling nodes supply the level. Files are `meshes/<base>.lod<n>.mesh.yaml`, derived and write-only. The pool's Mesh looks select levels the same way. Animation LOD is out (the level index is exposed in the profile and the debug tint); voxel LOD is out and plugs in as another bucket kind (`Plan/world-streaming.md` W5) (superseded for voxel models by decision 30).
28. **Dither or crossfade between levels** is deferred until popping is visible in the game's horde golden or the `lod-hall` sweep.
29. **Exposure.** `ludion_renderer::lod_level` is the one pure selection function: the CPU oracle uses it, and a later animation LOD keys on the same level index (animation LOD is out of scope). Observability is the profile's `levels_drawn_median` and the `--debug-draw cull` level tint.
30. **Voxel LOD generated at load, not by a verb (request 0013).** A voxel mesh is already derived at load and never committed, so a `kind: voxel` document's `lods:` levels are built beside the greedy mesh, ride the same `Arc<MeshData>`, and are cached and hot-reloaded with the model. A committed level file would be a second source of truth for what the downsample rule already determines.
31. **Authoring.** `lods:` is a list of up to three maps with exactly `screen_size` (same threshold rules as a mesh chain: finite, above 0, strictly falling); level n is the 2× downsample of level n−1. A malformed chain is `L1395`, broadened to "LOD chain malformed": one concept, one code.
32. **Pinned downsample rule.** Coarse cell `(X,Y,Z)` covers fine cells `2X..2X+1` per axis, extent `ceil(n/2)`, out-of-bounds cells empty. It is solid when at least 4 of its 8 cells are (ties keep thin walls) and takes the most frequent palette index among its solid cells, ties to the lowest. Integer-only, so there is no cross-arch item.
33. **Base frame, odd-extent overhang.** Level n meshes with step `2^n` against the base size's bottom-centre offset, so silhouettes stay aligned; an odd extent overhangs the base by under one coarse cell toward +X/+Y/+Z.
34. **Chain-union bounds.** `AssetBounds::of_mesh` takes the union of the base box and every level's box, so an overhang is never frustum-culled early and culling on/off stays byte-identical. A meshoptimizer chain's levels lie inside the base, so its bounds, and every existing golden, are unchanged.
35. **Not chained:** sculpt `shape: voxel` parts (they read the document and ignore `lods`), assembly parts, the GI voxelizer and the static grid (level 0 only). The pool's Voxel looks select levels like its Mesh looks.

**Rung 2b, the persistent static scene (Serious Isaac 0039, owner grilling 2026-11-13).** Resolves G6.

36. **Marker (G6).** An authored `Static: {}` component, a marker like `Viewmodel`. A map-kit visual primitive's `static: true` makes `generate-map`, and through it `spawn_map` (same emitter), put `Static` on each emitted entity, merged rects and `MeshGrid` entities included. Colliders and gameplay entities are never marked.
37. **Scope.** The contract is defined engine-wide; only render is wired in W3. `GiDynamic` is kept as is. GI, probes and nav adopt `Static` in their own requests (0041/0042 or later).
38. **G7 is out of scope.** Decision 17's trigger stands.
39. **Rung 2b.** One engine-wide `StaticScene`: a CPU cache of placed instances and cull items keyed by entity, plus a persistent GPU buffer sorted by (bucket, static-first, EntityId). It is the cull's second input. Core WebGPU only, unchanged from 2a.
40. **Draw order.** Inside each bucket, statics draw before dynamics, each by ascending EntityId. The host off switch `set_static_scene(false)` (modelled on `set_frustum_culling`: host-only, never sim or wire) makes extract walk statics again in the same static-first order, so on and off are byte-identical. No existing golden moves.
41. **ECS write journal** (amends `engine-architecture.md` §4). A public, generic `World::watch_with::<M>()` / `drain_watched::<M>()`, with docs and a runnable example. It journals any mutable access to any component of an entity carrying M (`get_mut`, `component_mut`, `insert`, `remove`, `&mut` query and `par_for_each` yields), the insertion or removal of M, and despawn. A bitset with a sorted drain, so deterministic at 1, 2 and 8 threads; never hashed or snapshotted (a restore or `ludion dev` reload rebuilds `StaticScene`). A drained watched ancestor expands to its `Parent` descendants. Non-watched entities pay one bit test per mutable access. `ludion-ecs` budget target 3250, a named one-off raise; quality wins over the number. General change ticks stay out.
42. **Writes to statics are legal.** A journaled static is re-placed the next frame. A static journaled on 30 consecutive frames gets a warning `L`-code, once per entity ("static rewritten every frame; remove `Static`").
43. **GPU updates.** A whole rebuild (`create_buffer_init`, the `cull_bounds_table` pattern) when membership or a bucket changes; otherwise `queue.write_buffer` patches the 224 B slot and its cull item. If the `CullShape` capacity clamp is exceeded, statics draw uncull from their persistent ranges.
44. **Eligible:** primitive `Mesh` shapes; unskinned mesh assets and voxel models, LOD chains included; `MeshGrid` (every cell); `generate-map` merged rects; mirrored instances (the existing mirrored bucket and pipeline twin). **Ineligible:** skinned, `SkinBone`, assemblies, the viewmodel, projectile and pool looks, sprites, particles, text, decals, lights, markers. An ineligible `Static`, or a `Static` whose `Parent` ancestor is not `Static`, gets a warning `L`-code (`check-scene`, `scene.check`, runtime) and takes the dynamic path.
45. **Participation is identical everywhere.** All camera views, the prepass, sun cascades, spot and point shadows (LOD, `cast_shadows`); decal receivers and drop-shadow occlusion (index schemes gain a static range); the probe content key; the GI voxelizer, with a static-set generation folded into its signature; cascade fit, the CPU oracle and `--debug-draw cull`. Interpolation treats statics as fixed. `render.bounds` is unchanged (it walks the world). Cached static shadow maps are deferred to a measured trigger.
46. **`asset.reload`** re-places the statics whose mesh asset generation moved. Wire and editor edits are covered by the journal.
47. **Hashing.** `Static` is hashed like any component on sim entities (map rooms are sim). The component schema snapshot and `GameIdentity.schema` move, so the game's run recordings fail `L1404` once and are re-recorded when the game regenerates its rooms. `replay_content_hash` does not move.
48. **The 0042 hook.** Static placement is a pure function of a part's component values plus its placement (no EntityId, no World borrow), so 0042 can pre-place unspawned prefab statics. No prewarm command in W3.
49. **Static runs reach the cull by GPU copy.** The persistent static instance and cull-item buffers are copied (`copy_buffer_to_buffer`) into the bucket-leading slots of the frame's packed buffers, and only the dynamics are written from the CPU, so the frame layout and every direct draw (culling off, views past the capacity clamp, probe faces, decal receivers, drop-shadow `sources`) are unchanged and a scene with statics packs the bytes the same scene without them does. This refines decisions 39 and 43, which had the cull read the statics as a second input: `cull.wgsl` already binds eight storage buffers, the core WebGPU per-stage limit `engine_limits()` keeps, and the direct paths read one bucket-contiguous range. The cost is S×272 B of GPU copy per frame. **Trigger:** a segment-aware in-place cull, the statics read where they live with two ranges per bucket, when the copy shows in the GPU frame.
50. **The 0042 prewarm (consumes decision 48).** `PrefabPrewarm` is a render-only resource a sim system sets with `prewarm_prefab(world, path, placement)`, like `ProbeRefreeze`: not hashed, not recorded, not on the wire; a replay reproduces it by re-running the system. The engine instantiates the prefab into a scratch world (no sim id moves), prefetches its assets and keeps its meshes and maps GPU-resident through `RenderScene.prewarm`, never drawn, until superseded, cleared or the world rebuilds. One request at a time: the latest wins. **Trigger** for a `prefab.prewarm` wire verb: a client needs it. It would move the wire schema and the replay hash. Relay-approved 2026-11-14.

**Proposed (Serious Isaac 0021, owner approval owed). Decals join culling.** Ready to build once approved; it amends decisions 12 and 16. The trigger evidence is the decal room bench figure in Status.

- (i) A CPU box-vs-frustum test per decal at render time, against the rendered camera. Extract stays camera-free per decision 11, so the test is not in extract. It is the G9 path, and `cull.rs`'s Arvo box transform does the test (the oracle already exists).
- (ii) A culled decal is neither packed nor drawn. Layer order among survivors is unchanged.
- (iii) `ProfileReport.decals.culled` counts them; it moves the wire schema hash once.
- (iv) The `decal_receivers` redraw draws from the camera view's cull survivors (the decision 26 source-index path), instead of the full union.
- (v) Byte-identity: culling on against off is byte-identical, a gallery `decals` golden test pins it, and `--dump-graph` is unchanged.
- (vi) The bench gains a `CULLING_BENCH_CULL=on` room row.

**Proposed (Serious Isaac 0019, owner approval owed). Animation LOD: a distance cadence keyed on the mesh level.** Ready to build once approved; it amends decisions 11 and 29 here and decision 19 in `Plan/animation-clips.md`.

- (i) **Key.** `lod_hold: [f0, f1, …]` on `kind: character`: one positive integer per mesh level, base included, at most chain length + 1 entries. The effective hold is `Animation.hold × lod_hold[level]` on decision 19's global grid (`ticks − ticks % h`). A cadence alone only changes the picture; its cost saving comes through reusing the held palette (piece 1, rejected at this rig size, `Plan/animation-clips.md`) or through sharing (piece 4: coarser grids collapse more keys), so it is built with or after piece 4 and passes the same gate in `skin_pose_bench`. For the owner: without palette reuse, `lod_hold` saves cost only through sharing, and exact sharing rarely collapses keys in a staggered horde, so piece 3 likely needs `pose_buckets` to pass its gate. Absent means factor 1 everywhere and byte-identical output. A malformed list (empty, a 0, a non-integer, too long) is a new `L1xxx` (next free after `L1405`; confirm at build); `check-scene` warns with the code after it when a character has `lod_hold` and its mesh has no `lods:`.
- (ii) **Level source, amends decision 11.** A cadence is not a skip (every instance still gets a palette), but it is camera-dependent extract work. The level is picked once per extract from the extracted game camera: the eye, and `k` with `Camera.lod_bias` and `QualityParams.lod_bias`, offset 0 (decisions 22 and 25). It is rotation-invariant, so a flick never changes it. A presenter re-render keeps the extract's palettes; a level mismatch shows at most one stale held pose, never a geometry pop.
- (iii) **Unposed bound, resolving the circularity.** The GPU level uses the posed world box, which needs the palette. The cadence level uses the instance's unposed bound instead: the mesh asset's bounding sphere radius × the `Transform`'s largest scale, centred on the skin root's world position. Near a threshold the CPU level may differ from the drawn mesh level; that is accepted and documented.
- (iv) **One selection function.** Extract calls `ludion_renderer::lod_level` with `lod_size` (decision 29; `ludion-engine` already depends on `ludion-renderer`), so decision 29's "Animation LOD is out" is amended to "in, as a cadence only".
- (v) **Exactness.** `AnimationEvents` and the ragdoll pose keep the exact tick (decision 19's rule). `posed_joint_world`, which has no camera, returns the entity-hold pose, not the cadenced one. The drawn socket child (`ParentJoint`) rides the drawn, cadenced pose: the socket pre-pass `posed_joint_model` takes the same effective hold.
- (vi) **Quality bias.** An optional per-preset level offset for the cadence level, as `lod_bias` does for the mesh level; decide at build.
- (vii) **Determinism and schema.** Nothing enters `WorldHash`. The wire schema does not move: a document key, not a component.
- (viii) **Acceptance.** A `character-socket`-style golden with a socket child on a cadenced far figure; `world_hash` and the `AnimationEvents` stream unchanged with `lod_hold` on; sample counts fall by the expected factor in the `skin_pose_bench`; a scene without the key renders byte-identically.

**Settled (Serious Isaac 0034 amendment, owner-approved 2026-11-10, shipped). Skinned LOD sizes from the rest mesh, not the posed cull box.** It amends decision 22 for skinned instances only. Evidence: decision 13's posed box is a union of joint spheres each as wide as the mesh's farthest vertex from its joint, so its half-diagonal is about 2.3× the mesh's (the shmup's gnasher: rest half-diagonal 0.62 m, measured effective radius 1.3–1.5 m), and authored `screen_size` thresholds switch about 2.3× too far.

- (i) **Radius.** A skinned instance's LOD radius is the half-diagonal of its asset's rest box (`AssetBounds::local`, the chain union of decision 34) times the model's largest axis scale (the longest of the upper 3×3's columns). It is pose- and rotation-invariant, so an animating or ragdolled figure never flips a level, and it is 0019 (iii)'s unposed bound, so a future cadence level and the drawn level share one radius. Unskinned instances keep decision 22 unchanged.
- (ii) **Centre.** The centre of the posed world box (decision 13), so a figure whose palette carries it away from its `Transform` (a ragdoll) is sized from where it is drawn.
- (iii) **Culling unchanged.** Visibility keeps the conservative posed box; only the level selection reads the new radius.
- (iv) **GPU.** `CullItem.pad0` becomes `lod_radius: f32` (struct size unchanged; 0 means the box half-diagonal), filled for `SLOT_OVERRIDE` skinned items; `cull.wgsl` `select_level` and the CPU oracle (`cull::instance_level`, `cull_marks`) take it. No new binding.
- (v) **Observing.** The shipped `culling.lod_radii` (per chained mesh asset: path, skinned, min/median/max radius) reports the new radius for skinned assets once adopted; no schema move. `docs culling` "Mesh LOD" states the rule for both kinds.
- (vi) **Determinism.** Render-only: `WorldHash`, replay content hashes and `.lrp` records do not move. Goldens: `lod-hall` (500 skinned chained mannequins) and `crowd-animation` (chained mannequin-lod characters) move, Windows re-blessed on the pinned 3090, macOS twins owed to the M4; every unchained or unskinned golden (`voxel-horde` included) stays byte-identical, and culling on/off stays byte-identical.
- (vii) **Acceptance.** A renderer test: a skinned chained asset with a known rest half-diagonal at known distances and scales selects `lod_level(r·k/d)` with `r` from (i), on the CPU oracle and the GPU cull, under a posed palette that inflates the cull box (so the box provably no longer sizes it), plus a scaled skinned instance on both paths (guards a path that bakes scale into the bone palette rather than the model). The shmup then re-scales its thresholds by about 0.62/1.4 ≈ 0.44, or keeps 0.25/0.1 and draws coarser sooner: at its camera, level 1 from about 3.7 m and level 2 from about 9 m, so horde-500's 4–8 m band moves from level 0 to level 1.

## Status

0042 upload prewarm shipped (decision 50).

Serious Isaac 0034 shipped (r2294): `culling.lod_radii` and `docs culling` "Mesh LOD"; the tighter skinned radius (decision 22 amended, below) shipped for the 0034 amendment: `cull::skinned_lod_radius`, `CullItem.lod_radius`, `docs culling` "Mesh LOD"; Windows `lod-hall` and `crowd-animation` re-blessed, macOS twins owed to the M4.

Design written 2026-09-30. Rung 0 shipped (Clockout 0030; `Visibility`, `examples/gallery/visibility.yaml`). Rung 2a shipped for request 0008 (decisions 7–20; the W2 wave set above); the bench figures are below.

**A real consumer exists (2026-10-02).** vigil's environment moves to Blender-authored mesh kits
(a private plan). The first dressed set, a 40 × 42 m walled square, draws 605 mesh
instances and 614k triangles with no culling: 1.67 ms GPU at 1080p on the 3090, so not a breach yet.
Its static dressing is now presentation entities and its ground can be one `MeshGrid`, so the sim
side is nearly free and the cost left is extract plus draws. A city of such maps, or a bigger one
after the map-size sitting, is the workload this plan's rungs should be measured against.

**The torture bench, cull off (2026-11-07).** `crates/ludion-engine/tests/culling_bench.rs`, ignored, release:
`cargo test -p ludion-engine --release --test it -- culling_bench --ignored --nocapture`. The 40 × 40 m room
holds 500 animated enemies, 100 SkinBone corpses (1,100 parts, the voxel ragdoll on each bone but the
pelvis, drawn at the assembly's own scale), 60 decals, 200 sprites, 16 emitters and 5,000 frozen shots, with
SSAO on; the bench turns `Engine::set_frustum_culling` off. The per-phase extract breakdown rides
`ExtractObserver` (`extract_observer.rs`) and is bench output only. Figures are on an unpinned M5
laptop, 1080p, not authoritative; pinned M4 / RTX 3090 figures are owed.

| View | extract | top extract phases | GPU scene | prepass | GPU frame |
| --- | --- | --- | --- | --- | --- |
| Wall | 0.92 ms | place 0.29, meshes 0.20, shots 0.14 | 0.9 to 2.7 ms | 0.2 to 0.5 ms | 2.8 to 5.9 ms |
| Crowd | 0.94 ms | the same, to the hundredth | 1.0 to 2.7 ms | 0.2 to 0.5 ms | 3.1 to 6.3 ms |

Extract does the same work from either camera, as decision 17 says. The GPU figures are ranges because the
laptop GPU flips power state between runs (the same arm reads 3.4 or 6.3 ms); each arm keeps its fastest of
five rounds and the inline budgets are about 3× those. Costs by ablation at the crowd view (full minus
ablated): extract decals 0.01 to 0.04 ms, sprites 0.02 to 0.04, particles 0.03 to 0.06, shots 0.15 to 0.16.
GPU frame: decals 0.6 to 1.1 ms, sprites -0.2 to 0.8, particles 0.1 to 0.6, shots -0.1 to 0.2. The GPU
deltas move by half a millisecond between runs, so only the decals cost and the shots' extract cost are
clear of the noise.

**The decal room bench (Serious Isaac 0021, unpinned M5, 2026-11-09).** `cargo test -p ludion-engine --release --test it -- culling_bench::needs_gpu_decal_room_bench --ignored --nocapture`: 1,000 textured `multiply` scorches on the floor and two walls beside the torture room minus sprites, particles and shots (500 skinned enemies, 100 corpses), decals on then ablated. One projector draw, 600 receiver meshes (the skinned ones, once per frame). GPU frame on/off, three runs: crowd 2.42/2.22 ms (delta 0.19 to 0.24), wall 2.17/2.14 ms (delta 0.03). Passes on: `projector` 0.71 (crowd) and 0.44 to 0.45 ms (wall), `decal_receivers` 0.76 and 0.27 ms. Extract 0.99/0.65 ms (delta 0.35 ms, the `Decals` phase 0.17 ms). Per decal: about 0.2 µs GPU frame at the crowd view (0.03 µs at the wall), 0.35 µs extract. Run-to-run spread under 0.05 ms. The frame delta is far below the summed pass medians (tiled-GPU overlap). Pinned figures are owed.

**The decal room bench, pinned RTX 3090 (Serious Isaac 0032, release, three runs).** The `all` decal's receiver redraw is gone (the scene pass flags skinned pixels, `Plan/telegraphs-and-vfx.md` general decal system). Crowd arm, decals on: before `receiver_meshes` 600, `decal_receivers` 0.060 ms, `projector` 0.047 ms, GPU frame 1.10 to 1.16 ms; after `receiver_meshes` 0, no `decal_receivers`, `projector` 0.043 to 0.049 ms, frame 0.98 to 1.10 ms. Wall arm: frame 0.96 to 1.19 before, 0.90 to 0.94 after (`decal_receivers` 0.05 to 0.07 ms gone, `projector` 0.17 ms unchanged). Ablated arms unchanged. Game perf harness (`perf_m2`, seed 11, 8 threads): horde-500 GPU frame 9.25 to 7.6 ms (`decal_receivers` 1.376 ms to absent), gibfest 5.50 to 5.05 ms (0.483 ms to absent); extract unchanged.

**Animation palette work (Serious Isaac 0019).** Decision 17's constant-factor fix shipped for request 0019: extract's `Skin` phase and parallel palette sampling. The palette skip was measured and rejected. Figures: `Plan/animation-clips.md` Status "Crowd posing".

**Indirect-validation cost (6a, unpinned M5, 2026-11-08).** wgpu validates every indirect draw with a
compute dispatch outside the timed passes, so the GPU frame span is the figure that shows it. Method:
`CULLING_BENCH_CULL=on cargo test -p ludion-engine --release --test it -- culling_bench::needs_gpu_culling_torture_bench --ignored --nocapture`,
run three times with `WGPU_VALIDATION_INDIRECT_CALL=0` and three times without, interleaved. GPU frame,
fastest of five rounds per run, validation off then on: Wall 2.84, 3.04, 2.77 ms against 2.83, 2.91, 3.00 ms;
Crowd 3.71, 3.43, 3.24 ms against 3.32, 3.67, 3.42 ms. `cull` is 0.06 to 0.12 ms, `scene` 0.5 to 1.2 ms,
`prepass` 0.07 to 0.23 ms in every run. The validation delta is below the M5's noise floor of ~0.3 ms
(run-to-run spread of the same arm). Pinned figures are owed.

**The torture bench, cull off then on (rung 2a, unpinned M5, 2026-11-08).** `CULLING_BENCH_CULL=off|on`, same
bench and caveats as above; the M5's power state swings these by about 2×, so they are indicative only.
Wall, full: GPU scene 2.81 → 1.60 ms, prepass 0.52 → 0.17, frame 5.71 → 5.22, `cull` pass 0.23. Crowd, full:
scene 0.97 → 1.79, frame 2.96 → 5.11 (power-state noise dominates), `cull` pass 0.12. Extract stays about
0.9–1.2 ms both ways, unchanged by design (decision 17), no breach.
- The CPU caster cull left the shipping path, so "cull off" draws every caster in every shadow view. The
  off/on delta therefore overstates the gain over the pre-0008 engine for shadow passes; the "before" figures
  above predate that change.
- Owed: pinned before/after figures, the `[passes] cull` budget lines, the pinned indirect-validation figure
  and M4/3090 golden byte-identity (`Plan/owed.md`).
- **Shipped:** the W2 wave set (rung 2a) for request 0008; the agent doc is `docs/culling.md`.

**The static-kit room, before W3 (Serious Isaac 0039, unpinned M5, 2026-11-14).** `cargo test -p ludion-engine
--release --test it -- static_kit::needs_gpu_static_kit_bench --ignored --nocapture`, three runs, GPU cull on
(the shipping default). The room is `examples/gallery/staged/static-kit.yaml` (generated by
`crates/ludion-engine/tests/static_kit.rs`, outside the gallery root, no golden yet): 2,189 entities, 1,024
`MeshGrid` floor cells plus 1,943 unscaled kit pieces (walls, trims, pillars of three pieces, crate piles, voxel
trees and lanterns) on 2 m cells, interleaved with 219 dynamics (8 skinned) and one decal; 3,186 mesh
instances per extract. Indicative only (an unpinned laptop); pinned figures are owed.
- **Extract**, both views (it does not depend on the camera): total 0.528–0.534 ms, of which `Place` 0.141–0.148,
  `Meshes` 0.080, `Lights` 0.050, `Poses` 0.037. Kit ablated (219 instances): 0.052 ms. The kit costs 0.48 ms.
- **GPU**, overview, full → kit ablated: `cull` 0.10–0.11 → 0.13, `shadow` 0.12–0.13 → 0.10, `scene` 0.83–0.92 →
  0.42–0.45, `transparents` 0.65–0.73 → 0.29–0.33, `decals_lit` 0.63–0.69 → 0.25–0.28. Floor view (eye height in
  a corridor): `scene` 0.69–0.85 → 0.43, `shadow` 0.15, `cull` 0.12 → 0.17.
- **GPU frame** (fastest of five): 2.66–2.92 ms full against 2.71–2.78 ablated. The kit's frame delta reads
  −0.05 to +0.18 ms, inside the run-to-run noise, so the frame is not bound by the kit at this size.
- **Owed:** the pinned M4 and RTX 3090 figures and the golden (`Plan/owed.md`).

**The static-kit room, after W3 (Serious Isaac 0039, unpinned M5, 2026-11-14).** Same command, same room.
Three runs, 3,186 mesh instances, indicative only.
- **Extract**, both views: statics on 0.306–0.311 ms (`Place` 0.086, the rest `Lights` 0.052, `Poses` 0.038);
  against the pre-W3 0.53 ms extract the saving is about 0.22 ms (0.53 → 0.31). Statics off (1.69–1.70 ms,
  `Place` 1.41–1.43) rebuilds `StaticScene` every frame: an identity switch, not the old path. Kit ablated: 0.054 ms, so
  the kit now costs 0.25 ms (was 0.48 ms; the residue is the held-prefix copy and the dynamics' share).
- **GPU frame**, on against off: −0.08 to +0.11 ms across runs, inside noise (the image is identical; the saving
  is CPU). `cull` 0.10–0.12 ms, `scene` 0.60–0.91 ms, the same as before W3.
- **Inline budget:** `MESH_INSTANCES = 3_186` (asserted every push in `static_kit_scene_matches_its_counts` and
  in the bench) and `STATIC_KIT_EXTRACT_BUDGET_MS = 1.0` (about 3x the 0.31 ms median); no `budgets/*.toml`
  entry, no GPU ceiling. Pinned lines are owed.
- **Remaining per-frame O(all meshes) walks** (renderer, medians at 3,186 instances, statics on): `cull_shape`
  2.8 µs, `casts` 1.0 µs, the static `sources` copy 84 ns per run (about 10 runs); all far under 0.05 ms, so
  none is fixed. Re-measure when a scene passes ~50k mesh instances. `viewmodel_bounds` returns early without a
  viewmodel (this room has none). `sun_cascade_stats` is profile-only (its one non-test caller is
  `profile.rs`), not on the frame path.

**W3 shipped (rung 2b).** Mechanized: the ECS write journal, `Static`, `StaticScene`, the persistent static GPU
buffers (decision 49) and the inline budget. Owed: pinned M4 / RTX 3090 figures, the pinned budget lines and the
static-kit golden (`Plan/owed.md`).

**LOD (request 0013).** Shipped as decisions 21–29; the acceptance scene is `examples/gallery/lod-hall.yaml` (500
skinned figures down a 130 m hall on `meshes/lod/`, a three-level mannequin imported with `--lod 0.3:0.5 --lod
0.12:0.2`, held by a drift gate in `ludion-cli/tests/import.rs`). Gallery tests: the hall's golden, and culling on or
off byte-identical; no-chain identity is every other gallery golden staying untouched plus `CullShape.levels == 1` for
a chainless frame. Bench, `CULLING_BENCH_LOD=off|on`, the crowd view, fastest of five rounds, GPU pass medians
(unpinned M5, 2026-11-08, indicative): chain off `shadow` 0.117, `prepass` 0.195, `scene` 1.000 ms, no `cull` pass;
chain on `cull` 0.071, `shadow` 0.135, `prepass` 0.197, `scene` 1.049 ms. The mannequin is only 512, 256 and 200 triangles across its three levels, so 500 of them are a few hundred
thousand triangles at most and the chain is a wash here: the saving is for the game's horde of heavier meshes, which the game re-measures (`Plan/owed.md`).

**Voxel LOD (request 0013).** Shipped as decisions 30–35; the acceptance scene is `examples/gallery/voxel-horde.yaml` (500 figures down the hall on `voxels/horde-figure-lod.voxel.yaml`, a 24×40×10 upscale of `rim-figure` with a three-threshold chain, so all four levels draw at `lod_bias 0`: 33/65/150/253 in the camera view). Gallery tests: the golden, culling on/off byte-identical, and a chain whose thresholds can never fire byte-identical to the chainless `horde-figure`; no committed document but the new one gains `lods`, so every other voxel golden is untouched. Bench, `CULLING_BENCH_LOD=voxel-off|voxel-on`, crowd view, GPU pass medians (unpinned M5, 2026-11-08, indicative): chain off `shadow` 0.111, `prepass` 0.189, `scene` 0.850 ms, no `cull` pass; chain on `cull` 0.058, `shadow` 0.082, `prepass` 0.129, `scene` 0.527 ms. The 60-triangle base is cheap, so most of the saving is draw and vertex work on the far ranks.
