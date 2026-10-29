# GI Wave 0 — the voxelizer, zero pixels (detailed implementation plan)

> Status: **DELIVERED (2026-08-07, Lore revs 508–509).** All six work packages landed per §13's
> sequencing (WP-A ∥ WP-B → WP-C → WP-D ∥ WP-E ∥ WP-F); every §14 criterion met — full `check.mjs`
> green (the one red lane is the pre-existing untracked statue-gemma vitest set, unrelated), every
> golden byte-identical (§12 test 4 landed as two byte-exact fixtures with non-vacuity guards, in
> `ludion-engine::gpu_render` and `vigil::golden`), `ludion explain L2103`/`L2104` work, the D15
> measurement recorded (plaza: 9 distinct models, ~375k cells, ~7.5–9 MB ⇒ option A). §15 answers:
> option A; occupancy `Vec<bool>` (2.3 MB of a 48 MB default district grid, a bitset saves ~4% and
> taxes every reader); no committed sheet golden; `GiVolume` carries exactly `cell_size` (default
> 0.5, `exclusiveMinimum` ⇒ `L1014`, no new component code). Deviations from the text, all recorded
> in the two commit messages: the acceptance sheet ran on a `post-full` copy rather than a district
> (vigil components need the game registry, which the CLI cannot host — the districts' fill is
> instead proven by the `vigil::golden` neutrality fixture's `voxel.is_some()` guard); `--dump-voxels`
> gained `L2104` for the no-`GiVolume` case (the `L2102` no-camera shape); `VoxelBake` packs two 3D
> textures (`Rgba32Float` albedo + `Rg32Float` emissive/occupancy) rather than folding emissive into
> alpha, which would have dropped the grid's real alpha. **Two cost shapes recorded for W1's bake
> design:** the voxelizer runs per-extract and the bake hash is proportional to grid size per frame
> (§10's accepted W0 shape — free while no committed scene authors `GiVolume`; W1's load-time bake is
> where "scene as loaded" becomes structural). File:line references below were the planning-time
> grounding; the code as landed is authoritative.

---

## 1. Goal and the "zero pixels" contract

W0 builds the **foundation both later waves stand on**: a deterministic, world-space voxelization of
a loaded scene's static geometry into an occupancy / albedo / emissive grid. W1 traces it (the static
bake); §9's W3′ injects dynamic light into it. W0 itself renders **nothing** — it produces the grid,
uploads it to an inert GPU 3D texture, and makes it *lookable* by an agent. No shader changes, no
render pass consumes the grid, **every existing golden stays byte-identical**.

The neutrality guarantee is by construction (C6, the `Environment`/`PostProcess`/`ssao_direct`
precedent): a scene with **no `GiVolume` component** skips the voxelizer entirely and takes the exact
historical pixel path. A scene *with* one builds the grid — but since no pass samples it yet, even an
authored `GiVolume` moves no pixel in W0.

## 2. Locked decisions that bind W0

From `Plan/global-illumination.md` §0, plus the two settled in the 2026-08-06 grill-me follow-up:

- **D3 GPU bake** — refined for W0: *voxelization* (rasterizing instances into the world grid) runs
  **CPU-side** in `ludion-engine`; the GPU bake proper is **the trace in W1**. Rationale in §4 below.
  This is consistent with D3, not a reversal — "the bake" is the trace, not the voxelization.
- **D2 (a) scene as loaded** — the bake input is every placed mesh instance present in the extracted
  scene; runtime-spawned entities never enter occupancy. No `GiStatic` marker in W0.
- **D4 spacing authored** — the trace/grid **cell size is an authored `GiVolume` field**, not an
  engine constant. Grid extent derives from the scene AABB; cell size sets resolution.
- **Storage seam (D4)** — W0 stores the grid as plain occupancy/albedo/emissive. SH-L1 is W1's
  concern; W0's grid type must not bake in a probe-storage assumption.
- **C3 no committed derived data** — the grid lives in memory, hash-cached; nothing is written to
  disk or committed.
- **GiVolume is a new dedicated component** (grill-me follow-up 2026-08-06) — opt-in, absent ⇒
  byte-identical, the growing home for the arc's params. Costs one freeze-list entry, accepted.

## 3. Architecture — crate split (forced by the boundary rules)

| Concern | Crate | Why |
|---|---|---|
| `GiVolume` component (schema, validation, registration) | `ludion-scene` / `ludion-ecs` (wherever components are registered) | Same home as `Environment`/`PostProcess` |
| Reading `.voxel.yaml` grids + palettes, rasterizing instances → world grid (CPU) | **`ludion-engine`** (new `voxelizer.rs`) | Only crate that sees both scene data and the renderer; sibling of `voxel_mesh.rs` |
| The plain-data grid handed to the renderer | `ludion-renderer::RenderScene` (new field) | Carries no wgpu types — `Vec`/`Mat4`/`f32`, exactly like `MeshData` |
| GPU 3D-texture upload + hash-cache (inert holder) | **`ludion-renderer`** (new `voxel_bake.rs`, `ibl.rs` shape) | Needs `wgpu`/`Gpu`; consumes the grid as plain data |
| Debug slice-sheet dump | `ludion-engine` + CLI flag | CPU-only, reuses `model_sheet` machinery |

Boundary compliance to preserve: `ludion-renderer` has no `ludion-scene` dep (so it cannot see
`VoxelDocument`/`PaletteEntry` — voxel reading must be engine-side); `ludion-assets` never sees wgpu
(untouched); `ludion-engine` never depends on `ludion-server`; the new `RenderScene` field carries no
wgpu type.

## 4. Why voxelization is CPU in W0 (the one D3 refinement)

- **Occupancy** is an order-independent OR — safe on CPU or GPU.
- **Albedo/emissive** are *not*: when several model cells or several instances fall in one coarser
  world cell, the combined value is order-dependent, and GPU float scatter is non-associative (C5
  forbids the resulting cross-process byte drift). W0 sidesteps this entirely: CPU rasterization with
  a **stated deterministic combine rule — last writer wins, iterating instances in ascending
  entity-slot order** (the same order `extract`'s `place()` walks, which is the WorldHash order).
- CPU voxelization sits in the **bit-exact determinism tier**, giving the strongest no-golden-move
  proof (a pure function, testable for double-run bit-identity like the mesher already is).
- "Zero pixels" wants the least new GPU surface; the only new GPU work in W0 is a deterministic
  *upload* of the finished grid (order-independent), which is where the `ibl.rs`-shaped holder earns
  its place as W1's home.

State this split explicitly in the wave commit so W1 is not surprised that the GPU bake it adds is the
first *trace*, not the first GPU touch.

## 5. The `ibl.rs` shape W0 mirrors (renderer holder)

`crates/ludion-renderer/src/ibl.rs` is the precedent for a renderer-owned, load-time GPU step that
runs outside the frame graph and is hash-cached:

- Struct `Ibl` holds pipelines + a single-entry `cache: RefCell<Option<Cached>>`; created once in
  `Renderer::new` (`render.rs` ~`let ibl = Ibl::new(&gpu);`), stored as a `Renderer` field.
- `sky_hash(env)` serializes only the output-affecting inputs and runs
  `ludion_assets::content_hash`; `resolve()` checks the cached hash before recomputing — a no-op
  change is a cache hit.
- Invoked per-frame inside `build` (`self.ibl.resolve(&gpu, scene.environment.as_ref())`); the cache
  makes idle frames free.
- Output `ResolvedEnv` carries `wgpu::TextureView`s + a `params[0]` on/off flag; storage textures are
  `STORAGE_BINDING | TEXTURE_BINDING`.

**W0 `VoxelBake` mirrors this exactly, minus the binding**: struct field on `Renderer`, built in
`new`, `resolve(&gpu, scene.voxel.as_ref())` called in `build`, hash-cache-keyed on the grid bytes,
produces a 3D `wgpu::Texture` (`STORAGE_BINDING | TEXTURE_BINDING`). **It binds into nothing and no
shader samples it.** This is the inert home for W1's trace.

## 6. D15 — keep the authored grid reachable

**Confirmed:** `voxel_mesh.rs::mesh()` builds `let grid = Grid::from_doc(doc)` as a local, uses it
for greedy quads, and returns only `MeshData`; the asset cache retains `Arc<MeshData>` only. The
occupancy grid is dropped.

**Minimal change (recommended option A):**
- In `voxel_mesh.rs`: add a public plain-data type `VoxelStaticGrid { extent: [u32;3],
  cells: Vec<Option<VoxelCell>> }` where `VoxelCell { albedo: [f32;4], emissive: f32 }`, and a
  `pub(crate) fn static_grid(doc: &VoxelDocument) -> VoxelStaticGrid` that reuses `Grid::from_doc` +
  the existing palette color/emissive tables the mesher already builds.
- In `asset_cache.rs`: add a `voxel_grids: BTreeMap<String, Arc<VoxelStaticGrid>>` map and a
  `load_voxel_grid(root, rel)` method mirroring `load_voxel` (parse once, `Arc`-share on hit).
- In `extract.rs`: add a `SceneAssets::voxel_grid` method beside `voxel`, implemented on both
  `DiskAssets` and `CachedAssets`.

**Decision to measure, not guess (risk 4):** option A retains a dense grid per distinct voxel model
beside `MeshData`. If a district instances few distinct models this is cheap; a large streamed model
runs to MBs. Measure distinct-model count on a district during W0; if it's a concern, fall back to
option B (cache the parsed `VoxelDocument`, re-derive the grid on demand — voxelization is
load-time/cache-keyed, not per-frame, so recompute is acceptable). Record the measured number and the
choice in the wave commit.

## 7. Enumerating placed instances + transform convention

`extract.rs::place()` is the single deterministic walk: it iterates entity ids in ascending slot
order and, per `Mesh` component, builds `model = Mat4::from_scale_rotation_translation(scale,
euler_quat(rotation_degrees), translation)` and a `PlacedMesh { id, index, model, normal_matrix,
color, material, source }` where `source` is `Primitive(MeshShape)` | `Asset(&str)` | `Voxel(&str)`.
This is exactly the per-instance model + world transform the voxelizer needs, in the WorldHash order
that makes the last-writer combine rule deterministic.

**Wiring:** run the voxelizer inside `extract_inner` after `place()`, gated on the `GiVolume`
component (absent ⇒ skip, produce `None`). Store the result as `RenderScene.voxel:
Option<WorldVoxelGrid>`, and **include the field in `RenderScene`'s `PartialEq` derive** so the
extract-determinism test still proves two extractions are byte-equal.

## 8. Rasterizing each source into the world grid

Grid model: `WorldVoxelGrid { origin: Vec3, cell_size: f32, extent: [u32;3], occupancy: Vec<bool>
(or bitset), albedo: Vec<[f32;4]>, emissive: Vec<f32> }`. A cell `(x,y,z)` covers
`[origin + (x,y,z)·cell_size, origin + (x+1,y+1,z+1)·cell_size]`.

- **Cube** (`MeshShape::Cube`, object AABB `[-0.5,0.5]³`): transform the AABB by `model`; fill cells
  whose centers fall inside the transformed box. Exact for the axis-aligned cubes every committed
  scene uses (walls, the kit's merged boxes).
- **Plane** (object AABB `[-0.5,0.5]×{0}×[-0.5,0.5]`, normal +Y): the ground archetype — fill the
  single cell row the plane's world Y lands in, so the floor registers as solid occupancy.
- **Sphere** (unit diameter, centered): W0 conservatively fills the transformed AABB (spheres are
  rare in vigil); per-cell ellipsoid test is a later refinement, noted not built.
- **Voxel model** (bottom-center origin, 1 voxel = 1 unit; a 16-vox/unit prop scales ~0.0625, so a
  model voxel is far finer than a 0.25–0.5-unit world cell): for each filled model cell (via the D15
  `VoxelStaticGrid`), transform its object-space center by `model` and deposit into the world cell it
  lands in. Multiple model cells fall in one world cell ⇒ occupancy OR, albedo/emissive by the
  last-writer-ascending-instance rule.

- **Asset mesh** (`PlacedMeshSource::Asset`, unskinned, not `GiDynamic`): a conservative surface shell —
  every world-space triangle fills the cells it overlaps (triangle/box SAT, box inflated by
  `cell_size * 1e-4`), the interior stays empty, the mesh stretches the grid bounds, and it enters the
  signature by `MeshData::hash`. Albedo per triangle is `mean(vertex colours) × instance colour ×
  MeshData::base_color_mean`; emissive is the vertex mean when the document authors a material, the
  instance's otherwise. Skinned meshes, characters, sculpts and assemblies are skipped and counted in
  the skip report.

  *Amended 2026-10-29 (owner approval, Clockout 0013): asset meshes contribute their surface shell.*

Albedo/emissive for a **primitive** come from the instance `color` and `material[2]` (the
`[roughness, metallic, emissive]` lane) — the same channels the mesh path already carries.

## 9. Grid extent / resolution sizing (the least-specified part — risk 1)

- **Origin + extent** derive from the scene's world AABB — reuse the `render.bounds` union that
  already computes scene bounds — snapped outward to whole cells.
- **Cell size** is the authored `GiVolume` field (D4).
- **Hard cell-count cap**: compute `extent.x·y·z` and **error loudly** (a new `L####`) above a
  generous ceiling rather than allocating a multi-GB grid. A district at 0.5 units is ~3M cells
  (~tens of MB) — fine; the cap catches an authoring mistake (cell size far too small, or an
  unbounded scene) before it OOMs. The cap is not the streaming trigger (that is §10 of the GI plan);
  it is a guardrail.

Not built: voxelizing only inside an authored `GiVolume` box (a `GiVolume.bounds` field), so a
district's far geometry stops stretching the grid. Asset meshes now stretch the bounds, and the probe
cap (`L2105`) derives from the device's storage-binding limit rather than a fixed count.

## 10. Hash-cache key vs "static" (risk 3)

The `VoxelBake` cache key must be a pure function of what affects the grid and **exclude runtime
churn**, or it misses every frame:
- per-instance: model content hash (voxel model or primitive shape) + world matrix + color + emissive
- global: authored cell size + grid origin/extent
- iterate instances in ascending slot order so the key is order-stable.

Since there is no `GiStatic` marker (D2 = scene-as-loaded), W0's definition of the keyed set is
"every placed instance in the extracted scene." The start-pose caveat (an authored-then-animated
entity bakes at its current transform, and moving it invalidates the cache every frame) is accepted
and documented; vigil's moving cast is runtime-spawned, so it doesn't arise there. If a future scene
animates an authored entity and thrashes the cache, that is the trigger to add `GiStatic` (D2 option
b), a trivial additive component.

## 11. Debug visualization — the slice-sheet dump

A silent voxelizer is undebuggable. W0 ships a CPU-only inspection output following the `model_sheet`
precedent (`ludion render --model` → one stitched labeled contact sheet):

- New CLI flag `ludion render --scene <X> --dump-voxels <PNG>` (sibling of `--dump-timings`).
- It runs `extract` (which builds the grid), then renders the **CPU grid** to a PNG: a stack of
  horizontal (XZ) slices, one per Y layer, tiled near-square into one sheet, each pixel colored by
  occupancy/albedo, with a parallel emissive sheet (or a second channel/tile block).
- Reuse `model_sheet`'s stitch/`blit_cell` and burned-glyph label machinery and `RenderedImage`.
- Deterministic (pure CPU row-copy, no GPU, no clock) ⇒ golden-testable if we choose, but not blessed
  by default in W0.
- Answers the two agent questions in one image: "did the walls fill?" (occupancy) and "did the
  torches register?" (emissive).

A GPU debug raymarch of the 3D texture is strictly more code and is **deferred**; the slice sheet is
the W0 instrument.

## 12. Tests and the gate

The wave-7/8 fixture style. Five tests:

1. **Extract determinism** — extract the same world twice; assert `RenderScene` equality *including*
   the new `voxel` field (the existing `PartialEq` precedent).
2. **Voxelizer bit-identity** — build the world grid twice from the same scene; assert byte-equal
   buffers (analog of the mesher's `double_run_is_bit_identical`).
3. **Conservative occupancy** — one axis-aligned cube at a known transform fills exactly the cells
   its world AABB covers and no fewer (occupancy ⊇ surface); assert the filled-cell set equals the
   analytic box.
4. **Golden neutrality (the arc gate)** — `district-plaza` (and one gallery golden) render
   byte-identical with and without the voxelizer running. The no-golden-move proof.
5. **Grid-vs-mesh agreement (D15)** — for one model, the occupancy grid's filled cells match the
   mesher's authored-grid footprint (a world/model cell is occupied iff `Grid::at` is `Some`),
   pinning that the D15 accessor and the mesher read the same authored data.

Plus: full `node scripts/check.mjs` green. **No golden bless, no `run.lrp` re-record** — nothing
authors `GiVolume` into a committed scene in W0 (that is W4). A new `L####` for the cell-count cap
(§9), registered in the code table with an `explain` entry.

## 13. Work-package decomposition (for when we implement)

Dependency-ordered; not a fan-out — the types flow across packages, so waves are mostly sequential.

- **WP-A — `GiVolume` component** (foundational; others gate on it): schema + validation + registry
  registration + freeze-list entry, carrying at minimum the authored `cell_size`. Absent ⇒
  byte-identical. Model: opus (freeze-list + registry judgment). *Blocks C.*
- **WP-B — D15 accessor** (foundational, independent of A): `VoxelStaticGrid` + `static_grid` in
  `voxel_mesh.rs`, `AssetCache::load_voxel_grid` + map, `SceneAssets::voxel_grid`. Model: sonnet
  (well-specified against `load_voxel`). *Parallel with A; both block C.*
- **WP-C — the voxelizer + extract wiring** (the core): `voxelizer.rs` (`WorldVoxelGrid`, `voxelize`,
  the §8 rasterization, §9 sizing + cap L-code, §10 the combine rule), plus the `RenderScene.voxel`
  field and its `PartialEq`, plus the extract-side gate on `GiVolume`. Model: opus (the algorithmic
  and determinism heart). *Depends on A + B.*
- **WP-D — renderer `VoxelBake` holder**: `voxel_bake.rs` mirroring `ibl.rs`, hash-cache + 3D-texture
  upload, bound into nothing. Model: sonnet (verbatim pattern). *Depends on C (the field).*
- **WP-E — debug `--dump-voxels`**: CLI flag + CPU slice-sheet render reusing `model_sheet`. Model:
  sonnet. *Depends on C (the field).*
- **WP-F — tests**: the five in §12. Model: sonnet, briefed with the exact assertions. *Depends on
  C/D/E; some can be written against C alone.*

Suggested sequencing: **Wave 1** = WP-A ∥ WP-B (parallel groundwork). **Wave 2** = WP-C. **Wave 3** =
WP-D ∥ WP-E ∥ (WP-F for tests that only need C). **Integration + full `check.mjs` + the neutrality
golden test** on the orchestrator side, then one commit for the whole of W0 (or two: groundwork, then
the voxelizer+holder+debug+tests).

## 14. Definition of done

- `node scripts/check.mjs` green (fmt, clippy, tests, smoke, drift gates, frontend, schema).
- Every existing golden byte-identical (test 4 proves it; no bless, no re-record).
- The five tests pass.
- `ludion render --scene <a vigil district> --dump-voxels out.png` produces a legible occupancy +
  emissive slice sheet that visibly shows the district's walls filled and its torches lit.
- `ludion explain L####` works for the new cap code.
- The D15 memory measurement recorded and the A/B choice justified in the commit.
- `Plan/global-illumination.md` §4 W0 marked done; the graphics-engine notebook gains a W0 Done entry
  (torch/wall visibility confirmed via the dump, the CPU-voxelization decision, the D15 measurement,
  the last-writer combine rule).

## 15. Open micro-decisions deferred into implementation (record the answers in the commit)

- D15 option A vs B — decide on the measured distinct-model count (§6).
- Occupancy storage as `Vec<bool>` vs a bitset — pick on the measured grid size; bitset if a district
  grid is large.
- Whether the debug slice sheet gets a committed golden — lean no for W0 (it's an instrument, not a
  product; test 4 already gates pixels), revisit if it regresses silently.
- Exact `GiVolume` field set for W0 — minimum is `cell_size`; do not add W1+ fields (intensity,
  bounce count, dynamic strength) speculatively (no dead schema fields, roadmap decision 3).
