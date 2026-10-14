# Scene load speed — measured, parallel, retained

> **Status:** planned 2026-10-14, not started.

## Why

Mesh-dressed districts made scene opening noticeably slow. Measured on the reference box (RTX 3090,
5950X, warm OS file cache, release `--render-out` at 320×180 — boot + load + one frame), on the
reference game's mesh plaza district and its voxel twin:

| Variant | Wall time |
|---|---|
| voxel twin (same layout, no asset meshes) | 2.0 s |
| mesh district, every asset mesh swapped for a cube (same ~3,200 entities) | 2.1 s |
| mesh district, texture maps stripped from the mesh documents | 4.4 s |
| mesh district as authored | 7.1 s |

Entity count is free; the ~5 s is asset work. That district loads 79 unique `.mesh.yaml` documents
(23 MB of YAML, 83k vertices) and 235 PNG maps (144 at 1024², 84 at 512²; 173 Mpx, 694 MB of RGBA8
decoded). A standalone probe over exactly those files, single-threaded: **mesh parse + validate
0.83 s, PNG decode 1.05 s**, file reads 0.08 s, xxh3 hashing negligible. The remaining ~3 s (GPU
upload of 694 MB, per-texture mip generation, first-frame work) is unattributed — no load-phase
timing exists anywhere in the engine.

Three structural causes, all in `ludion-engine`:

1. **Loading is lazy and serial.** Assets decode one at a time, on first use inside the first
   frame's `extract`, through `AssetCache::load_*` (live host) or `DiskAssets` (one-shot paths).
2. **Nothing measures a load.** `ProfileReport` covers frames only; a load regression is noticed by
   a human, not a gate.
3. **The cache is dropped on every `Rebuild::Load`** (`engine.rs`, "A new scene brings new
   assets"), so opening a second district re-decodes every kit it shares with the first. The
   renderer's GPU caches already keep content-hash-keyed entries a frame still references
   (`retain`), so a retained CPU cache would also spare the re-upload. `Rebuild::Switch` keeps the
   cache but never evicts, so a long run through many sectors only grows.

## Decisions

1. **Measure first.** Wave 1 ships the instrument, so waves 2–3 are gated on numbers, not on this
   file's estimates.
2. **One optional `load` section on `ProfileReport`** (`ludion_protocol::perf`, the one report
   shape): wall-clock milliseconds per phase (`scene` parse + instantiate, `mesh` decode, `texture`
   decode, other asset kinds, `gpu_upload` (the renderer's upload + mip work for the first frame),
   `first_frame`, `total`) plus **deterministic counters**: files decoded, files reused from the
   cache, bytes read, per asset kind. The counters let tests assert behaviour with no timing flake.
   Filled by the offline driver (`profile_scene` / a game's `--profile`) and by `--render-out`
   (one line on stderr). Live capture leaves it `None` for now: the arming rule says a disarmed
   host reads no clock, and the live variant is not needed to gate this work. The field is additive
   (`skip_serializing_if`), but the schema snapshot moves — update the hash in `CLAUDE.md`.
3. **The wall clock lives only in the profile/host layer**, the existing pattern (`profiling`
   devdoc). Load code reports phase boundaries to an optional observer; it never reads a clock and
   never writes into the world. `observer_does_not_change_world_hash` extends to cover a profiled
   load.
4. **Prefetch, then extract.** At every world (re)build (`Rebuild::Load`, `Switch`, `Restore`),
   collect every asset path the new world references — sim *and* presentation entities,
   prefab-expanded parts, mesh texture maps, `MeshGrid` cells, sprites/atlases — dedup, sort by
   normalized key, and decode the missing ones in parallel. The first `extract` then finds every
   entry as a cache hit. Decode is a pure function of bytes, so parallelism goes through
   `ludion_core::par` (`par_map_chunks`), never rayon or ad-hoc threads. Insertion order is the
   sorted key order, which makes the cache contents byte-identical at 1/2/8 threads (a test).
5. **One loader path.** The one-shot paths (`extract_at`, `ludion render`, goldens, `--render-out`)
   use a freshly prefetched `AssetCache` too, so the measured path is the shipped path. `DiskAssets`
   is removed if nothing else needs it ("before adding a second way, remove the first"). Its
   voxelization no-cache semantics (always rebake) must survive as a property of the fresh cache.
6. **Errors stay where they are.** A reference that fails to decode during prefetch is not reported
   at prefetch time. It is left uncached, so the first `extract` reaches it as a miss and raises
   today's diagnostic with today's JSON Pointer (`L1101`/`L1102`/`L1110`/`L1111`, re-pointed at the
   `Mesh` component). No diagnostic text or pointer moves; existing error tests are the guard.
7. **Retain across loads, revalidated by content hash.** `Rebuild::Load` stops clearing the cache.
   After the new world's prefetch, each retained entry's recorded source files (`record_sources`)
   are re-read and re-hashed (xxh3; the full plaza set reads + hashes in under 0.1 s). A changed
   source drops the entry and its dependents, the existing reload path. Then **entries the new world
   does not reference are evicted.** The CPU cache becomes "the current world's working set",
   mirroring the renderer's retain-referenced policy. The same policy applies to `Switch` and
   `Restore`, which fixes the unbounded growth across sectors.
8. **No async or streamed loading.** A load still blocks the frame boundary it happens on, as now;
   only its duration changes. Placeholder-while-streaming is a separate decision (see Deferred).

## Waves

### Wave 1 — the instrument

- `LoadProfile` (name TBD by the planner) in `ludion_protocol::perf`, `ProfileReport.load`.
- Phase observer threaded through scene build, asset decode, and the renderer's first-frame upload
  (texture `write_texture`, `MipGen::generate`, mesh buffer creation) — split so the ~3 s
  unattributed share gets a name.
- Counters (decoded / reused / bytes per kind) alongside the timings.
- `ludion profile --json` and a game's `--profile` emit it; `--render-out` prints a one-line
  summary to stderr; `ludion docs`/`devdocs` topics (`profiling`, `ludion-cli`) updated.
- Budget: an optional `load_total_ms` ceiling in the budget TOML schema (`Budget`, `check_budget`,
  `L4005` on breach). Calibrate the reference game's mesh districts in its pinned budget files on
  the reference box. The M4 twin is owed (`Plan/owed.md`).
- **Exit:** the plaza report attributes ≥ 90 % of `total` to named phases; a schema snapshot and
  CLAUDE.md hash refresh; the counters asserted in a test over a synthetic fixture (generated meshes
  + PNGs in a scratch dir).
- **Checkpoint:** if Wave 1 shows the GPU share dominates (e.g. one submit per texture's mip chain),
  the cheap fix goes into Wave 2's scope (one encoder for all of a load's mip chains), recorded as a
  decision here first. Anything larger (texture compression) goes to Deferred.

### Wave 2 — prefetch in parallel

- The reference walk (decision 4) and the parallel decode into `AssetCache`, called from the world
  rebuild sites before the first extract.
- One-shot paths moved onto a prefetched cache (decision 5).
- Tests: cache contents and every asset's `hash` identical at 1/2/8 threads; a missing / corrupt
  texture or mesh still produces today's diagnostic and pointer through `extract` (decision 6);
  goldens and `WorldHash` traces unchanged (no re-bless, no re-record expected — a moved golden is a
  defect, not a re-bless).
- **Exit:** on the reference box at default threads, the plaza `load` report's mesh + texture
  decode phases total ≤ 250 ms (from ~1.9 s), and `total` drops accordingly; the budget files are
  re-calibrated to the new numbers.

### Wave 3 — retain across loads

- Remove the `Rebuild::Load` clear; revalidate + evict per decision 7 at every rebuild.
- Tests (deterministic, counter-based): load A then B sharing assets → B reports the shared files as
  reused, not decoded; edit a shared PNG between the loads → that file (and the meshes naming it) is
  decoded again, nothing else; load A, B, A → the second A decodes only what B evicted; a
  `Switch` chain across N scenes holds only the current world's entries; a stale-bytes regression
  test for the case the old clear existed for (same path, new content).
- **Exit:** in the reference game, opening the breach district after the plaza district shows the
  shared kits as reused in the `load` counters, and the GPU texture cache is not re-uploaded for
  them (a renderer counter or test asserting no `write_texture` for retained hashes).

## Traps

- `ludion-engine` is a sim-side crate under the determinism denylist: no std `HashMap`, no rayon,
  no wall clock. The clock is the observer's, outside the crate's sim code (decision 3).
- Prefab instances expand to presentation children. A walk over sim entities alone misses most of a
  mesh district's assets — the plaza has 619 prefab instances.
- Mesh documents load their texture maps inside `mesh_asset::load_textures`. Prefetching textures
  and meshes as independent jobs must not decode a map twice. Either meshes own their maps (decode
  the mesh job's maps inside it), or maps go through the texture cache key. Pick one; the
  `a_parent_spelled_mesh_map_and_a_loose_texture_share_one_key` test pins the key normalization.
- Shared-checkout test lanes: GPU tests and `check.mjs` are serialized across instances.

## Deferred

- GPU-compressed textures, precomputed mips, binary mesh files, a per-import maximum texture size,
  and the `ludion sync` / `ludion export` cook stages: `Plan/cooked-assets.md`.
- Streamed/async loading with placeholders: no plan; take it when a blocking load is measured as
  too long even after both plans.
