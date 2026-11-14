---
description: The CPU voxelizer behind GiVolume — the signature cache, the LWW combine rule, the safety condition
---

# Engine voxelizer

**Scope:** `crates/ludion-engine/src/voxelizer.rs` — turning a scene's placed geometry into the
world-space `WorldVoxelGrid` the GI bake reads. The bake and `mesh.wgsl`'s consumption of the result
are `ludion devdocs renderer-gi`; the component's authored fields are `ludion docs
global-illumination`.

## Layout

- `voxelize(meshes, cell_size, assets)` — the entry point `extract` calls: resolves instances, hashes
  the `O(instances)` signature, returns a cache hit or rasterizes.
- `allocate(bounds, cell_size)` — sizes and allocates the empty grid, snapped outward to the
  `cell_size` lattice; raises `L2103` in all three of its shapes (degenerate `cell_size`, non-finite
  bounds, over budget) before any allocation happens.
- `rasterize` / `Solid::{Cube, Box, Voxel, Mesh}` — per-instance fill: exact inverse-transform cube
  fill for a primitive mesh, conservative plane/sphere fill, `deposit_voxels` for a voxel model, and
  `deposit_triangles` for an asset mesh: every world-space triangle fills the cells it overlaps
  (`triangle_box_overlap`, a separating-axis test with a `cell_size * 1e-4` inflation), never the
  interior. An asset mesh's object box is its vertex extent, computed on a miss only and memoized per
  mesh hash (`MeshBoxes`); `resolve` and `signature` run every frame and never walk vertices.
- `probe_lattice` — sizes the irradiance lattice; its `L2105` ceiling is `ludion_renderer::max_gi_probes()`,
  derived, not chosen: the bake's hit cache (64 B/probe, one storage binding) is the largest per-probe
  buffer, so the engine's 128 MiB binding admits about 2.1M probes. The cell cap (`MAX_GI_CELLS`) is separate.
- `SkipReport` — `voxelize`'s second product: counts (dynamic, skinned, non-finite, unsupported) and
  the first entity index of each, built in `resolve` so a hit and a miss agree. The voxel-sheet dumps
  read it (`skip_report`) to explain an empty grid as `L2111`.
- `write_cell` — the one site that writes occupancy/albedo/emissive; every fill path funnels through
  it, which is what makes the combine rule (below) a property of one function instead of an invariant
  scattered across several.
- `signature(instances, cell_size)` — the cache key: `cell_size`, then per instance in ascending
  entity-slot order its model matrix, colour, emissive, and a source discriminant (primitive shape, or
  a voxel model's or asset mesh's own content hash).

## Dataflow

```
extract → voxelize(meshes, cell_size, assets)
            ├─ signature hit  → Arc<WorldVoxelGrid> from AssetCache, no rasterization
            └─ signature miss → allocate() → rasterize every instance → cache → Arc<WorldVoxelGrid>
          → RenderScene.voxel: Option<Arc<WorldVoxelGrid>>
```

`WorldVoxelGrid` seals its own `hash: u64` once, at build — computed where the grid is built, not
re-walked later. `voxel_bake.rs` (the renderer's GPU upload) keys its own cache off `grid.hash`
instead of re-hashing the grid's contents every frame.

## Rules

- **The combine rule is stated, not emergent.** Occupancy is an order-independent OR — any
  contribution marks a cell filled. Albedo and emissive are **last-writer-wins**, iterating instances
  in ascending order of their signature bytes (`instance_bytes`, a content order, so the grid is a
  function of the instance *set*, independent of entity slots, spawn history and which instances sit
  in the kept statics) and, within one voxel model, its cells in ascending flat-index order (an asset
  mesh's triangles in mesh order). The sort runs on a miss only; `signature` stays slot-ordered. Stated because GPU float scatter has
  no such order and the CPU path must pick one deterministically when two instances' fills overlap one
  world cell.
- **Voxel-model emissive mirrors the render path's material selection.** `deposit_voxels` picks
  `palette_material` (the document's own `MeshData::authored_material`) the same way the render path
  does: the palette's emissive when the document authors a material, the instance's `Mesh.emissive`
  when it does not. Getting this wrong makes a glowing lamp render emissive but voxelize as a
  non-emitter (or the reverse) — the two paths must agree, because the bake reads only the voxelized
  value.
- **The signature cache is `O(instances)`, not `O(cells)`.** A hit costs one hash comparison and
  returns the same `Arc`, touching no cell; a district-scale grid stays affordable in `extract` (which
  runs every frame) only because of this. The cache lives on `AssetCache`/`SceneAssets`, the same seam
  `voxel_grid` uses for meshed voxel models — the only persistent state `extract` can reach.
- **Point-sampling safety condition.** A voxel model deposits one world cell per authored voxel
  centre: each filled authored cell's object-space centre is transformed by the instance model and
  tested against the world cell it lands in. This is dense only while an authored voxel is no larger
  than a world cell — `Transform.scale <= cell_size`, one authored voxel per object-space unit. Past
  that the deposit thins into a lattice and the model reads as a hollow shell, and **refining
  `cell_size` makes it worse**, not better — a finer grid only spreads the same point samples further
  apart, it never fills the gaps between them. Recorded at `deposit_voxels`'s call site
  (`voxelizer.rs`); there is no diagnostic for it, because the same field (`Transform.scale`) is a
  perfectly ordinary authoring choice everywhere else a model is placed.
- **`L2103` is checked on the computed extent, before any allocation.** Two independent shapes under
  one code: too many cells in total (`MAX_GI_CELLS = 32_000_000`, roughly 0.7 GB of grid at ~21
  bytes/cell — an order-of-magnitude-off `cell_size` or a stray far-flung entity, not a resolution
  choice) and too many cells along one axis (`MAX_GI_TEXTURE_AXIS = 2048`, WebGPU's guaranteed-minimum
  `max_texture_dimension_3d`, checked against the spec floor rather than the local adapter's larger
  limit, so a grid that fits builds on every conformant GPU).

## Gotchas

- **Thin cards still fill, and `mask` meshes are skipped (reason `cutout`).** A single-sided leaf card or a plane
  fills the cells its surface overlaps even at a `cell_size` far coarser than the card is thick, and
  an `alpha_mode: mask` mesh is skipped whole, so light passes through a grating. An asset mesh's albedo
  uses `MeshData::base_color_mean` (the base-colour map's mean, white without one), not the texture.

- **The voxelizer, not the bake, is the cost centre.** On a `post-full`-scale scene at
  `cell_size 0.5` (grid 440×44×440 = 8.52M cells over a 220×22×220 box, measured on an RTX 3090), CPU
  voxelization in `extract` runs **310–340 ms** — an order of magnitude above the GPU bake's ~60 ms at
  the same scale (`ludion devdocs renderer-gi`). This is exactly why the signature cache above is
  load-bearing rather than a nicety: a live `ludion serve` on a `GiVolume` scene is only affordable
  because an unchanged world rasterizes nothing on the next `extract`.
- **An animated instance thrashes the cache — `GiDynamic` is the answer.** A system that moves,
  recolours, or changes the emissive of an entity the voxelizer sees invalidates that instance's
  contribution to the signature every frame, forcing a full re-rasterization on the next `extract`.
  Marking the entity `GiDynamic: {}` (`Plan/gi-wave3-dynamic-injection.md` §3) removes it from
  `resolve`'s input before the signature is ever hashed (see Dataflow), so its motion cannot reach
  the cache at all — the trade is that it stops occluding probe rays and stops depositing
  albedo/emissive into the grid (`ludion docs global-illumination`). Committed static `GiVolume`
  content needs no marker; a moving cast that is runtime-spawned and never voxelized doesn't trigger
  this either way.
- **`AssetCache::reload` on a `.voxel.yaml` can leave a stale grid behind the new cache key** if the
  reload path drops the meshed entry but not the voxelizer's own document-derived grid — check both
  are invalidated together when touching hot-reload of voxel documents.

## Verify

```
cargo test -p ludion-engine                              # voxelizer unit tests (voxelizer.rs), gi_sky_visibility.rs fixtures
cargo run -p ludion-cli -- render --scene examples/gallery/gi-interior.yaml --dump-voxels target/gi.voxels.png
cargo nextest run -p ludion-cli --test it -E 'test(/^render_voxels::/)'            # --dump-voxels CLI behavior, adapter-free
```

A second `extract` of an unchanged world must rasterize zero instances (the `RASTERIZATIONS` counter
in `voxelizer.rs`'s test module); moving one instance and moving it back must reproduce the original
signature hash.
