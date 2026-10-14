---
description: The kind: assembly/part/animation path: scene families, per-part meshing, extract-time posing
---

# Assemblies and clip playback

**Scope:** the engine-internal path behind a multi-part animated voxel model — three `ludion-scene`
document families, per-part meshing, the multi-source asset-cache entry, and the extract-time
expansion that turns one entity into N `MeshInstance`s. The authoring formats are
`ludion docs voxel-assemblies` and `ludion docs voxel-animation`; the single-grid mesher this reuses
unchanged is `crate::voxel_mesh` (`ludion devdocs scene-kind-documents` for its document family);
`extract()` at large is `ludion devdocs engine-image-paths`.

## Layout

- `crates/ludion-scene/src/assembly.rs` — the `kind: assembly` family (`L1137`/`L1138`) **plus** the cross-file pure functions the caller feeds resolved documents to: `check_parts`, `part_voxel`, `parent_indices`, `assembled_grid` → `AssembledGrid { voxel, owners }`, `overlap_lint` (`L1143`). `AssemblyPartEntry::pivot_in_mesh_frame` is the mirrored-frame carrier.
- `crates/ludion-scene/src/part.rs` — `kind: part` (`L1139`/`L1140`): a voxel grid with no palette.
- `crates/ludion-scene/src/animation.rs` — `kind: animation` (`L1141`/`L1142`, event data `L1198`): tracks name a *target* (part or joint), a `scale` channel, `events`, `generated_from`.
- `crates/ludion-scene/src/clip_eval.rs` — the pure sampler: `clip_phase`, `part_pose` (quaternion rotation via `sample_rotation`), `sample_channel` (both find the segment by binary search), `ClipSampler` (a clip resolved against a skin's joints once, rotations as quaternions; the character palette samples through it). Knows nothing of assemblies, matrices, or the engine.
- `crates/ludion-engine/src/assembly_mesh.rs` — `load`/`load_documents`/`load_with_sources`/`mesh_assembly` → `AssemblyData`. **Read its module doc first**: it carries the two-frames offset/pivot formulas.
- `crates/ludion-engine/src/extract.rs` — `resolve_assembly`, `animation_pose`, `elapsed_seconds`, `part_placements`, `compose_chain`.
- `crates/ludion-engine/src/asset_cache.rs` — the `assemblies` map and the `dependents` map, which holds every kind's source-file edges.
- `crates/ludion-engine/src/model_sheet.rs` — `ModelSheetRequest::{clip, time, strip}`, `ClipOutcome`/`PhaseReport`, and the posed gap rasterization (module doc).
- `crates/ludion-engine/tests/assembly_animation.rs` — the visual-only proof; `crates/ludion-engine/examples/assembly_clip.rs` — the runtime shape end to end.

## Dataflow

**Load.** `load_with_sources(root, rel)` reads the assembly, then every part file and every bound clip
file *relative to the assembly's own directory*, and returns `AssemblyData` plus the flat list of
source paths it touched. Each part is meshed alone through the ordinary greedy mesher — nothing is
welded — into an `Arc<MeshData>`; a part's content hash folds its own bytes and its part index, so a
part edit moves that part's GPU dedup key and no other's.

**Cache.** `AssetCache::load_assembly` keys the `AssemblyData` by path and records one reverse edge
per source. `reload(key)` therefore fires for the assembly document, any part, or any clip: it drops
the stale edges first (a re-read assembly may name a different part set), re-reads, re-records, and
returns *the changed file's own* content hash, because one source can feed several assemblies and no
derived hash describes that.

**Extract.** `resolve_assembly` pushes an assembly's parts as one contiguous run of `mesh_assets`, so
part `i` is `base + i` and a second entity referencing the same path reuses the run — one index per
path is the entire dedup state. `animation_pose` reads the entity's `Animation`, resolves the short
name against `AssemblyData::clips` and converts elapsed ticks to a clip phase; `part_placements` then
returns one `(Mat4, Mat3)` per part, in authored order, and the walk pushes one `MeshInstance` per
part with `model = entity_model * local`. Instance order is (ascending entity, then authored part).

**Time.** `extract_at(world, viewport, root, now)` is the only entry point that takes sim time;
`extract()` delegates at tick 0 and keeps its three-argument shape. `Engine::extract_render_scene`
passes the driver's own `SimTime`. `elapsed_seconds` derives `(now − started) × seconds_per_tick ×
speed` from tick counts on every call and never accumulates.

**Sheets.** A strip stands the one model up `n` times, each entity carrying `started = now − tick_i`,
and takes **one** `extract_at` — so `n` phases come out of one evaluator run at one `now`.

## Rules

- **`part_placements` is the one pose implementation.** It is `pub(crate)` precisely so
  `model_sheet`'s gap analysis rasterizes the same matrices the renderer draws with; a second
  implementation is exactly what would let a strip disagree with a running game.
- **The rest branch must stay byte-identical.** `pose: None` returns plain translations and
  `Mat3::IDENTITY`, which is what every un-animated assembly in every scene takes. In the posed
  branch the pivot rotation is built as one `Mat4::from_rotation_translation(r, pivot − r * pivot)`
  so an identity rotation yields the *exact* identity rather than a nearly-cancelling round trip —
  otherwise a clip that animates one limb would move every other part's last bits and churn goldens.
- **Translation is applied in the part's rest frame** (`T · R`, folded into the same `T` as the
  offset). Reversing the order bends a translation key by that part's own rotation; an exact-matrix
  test pins it.
- **Clip bytes are deliberately outside the mesh content hash.** A clip is pose, not geometry: fold
  it in and every clip edit invalidates GPU dedup keys for buffers that did not change.
- **Mirroring is consumed at meshing time.** `assembly_part_voxel` reverses rows (ordinary geometry,
  correct winding, no negative scale) and `pivot_in_mesh_frame` carries the authored pivot across.
  Nothing downstream may re-mirror — a consumer that branches on `mirror_x` itself produces a limb
  rotating about the wrong end.
- **`ludion-scene` stays IO-free.** Part and clip paths are stored verbatim; every cross-file check is
  a pure function taking `parts` **index-aligned** with `AssemblyDocument::parts`. That alignment is
  the shared convention of `check_parts`, `assembled_grid` and `overlap_lint` (the last asserts it).
- **`render.bounds` loads the document.** An assembly (or a lone `AssemblyPart`) is boxed by its grid,
  through `voxel_mesh::grid_box` like a voxel model; only a document that will not load falls back to
  the bottom-centered unit box, pinned by a missing-file test.
- **No transform propagation exists.** `Parent` is a link only; the whole hierarchy walk lives in
  `compose_chain` at extract, and parts are not entities — nothing sim-side can reach them.
- **The GI voxelizer skips assemblies.** Many part grids are not one solid; animated characters carry
  `GiDynamic` instead.
- **`Animation` is a registered component**, so it rides the append-only registration order, the
  registry tripwire count, and the pinned world hash. Its `started` is a `FrameIndex` (`Eq`/`Hash`,
  canonical integer) — never a float in hashed sim state.

## Gotchas

- **Authored part order is not topological.** A child may be written before its parent, so
  `compose_chain` resolves ancestors on demand with memoization and a depth counter; the counter is a
  cycle guard that degrades a malformed assembly to identity-parented instead of looping forever.
- **The posed gap analysis dilates on purpose.** A point-sampled version reported a rotated thin limb
  as detached *from itself*; each rest cell is now carried into the posed frame as a cube and every
  cell its integer AABB touches is filled. It can never under-connect, and it cannot see a sub-cell
  hairline. At rest the transform is the identity, so the rest count is the trustworthy baseline.
- **`--set`/`--sweep` stage an assembly's full dependency tree.** The override path re-emits the
  mutated assembly plus its parts and clip files (`copy_assembly_dependencies` in `model_sheet.rs`),
  one cell's worth per swept value. `--sweep` combined with `--strip` is `L4008` — both multiply cells.
- **Lineup staging is per-model subdirectories** (`lineup/<i>/`) because part paths resolve relative
  to the assembly file's own directory; a flat `{index}-{file}` layout breaks part resolution.
- **`L1136` is not ours** — it belongs to `AudioEmitter.range` (`ludion devdocs ludion-audio`). The family runs `L1137`–`L1143`.

## Verify

```
cargo test -p ludion-scene                          # assembly/part/animation round-trips + emit idempotence
cargo test -p ludion-engine --test assembly_animation   # the visual-only proof
cargo test -p ludion-engine --test extract --test model_sheet
cargo test -p ludion-cli --test check_scene_voxel_animation --test render_model
cargo run -p ludion-engine --example assembly_clip   # prints posed vs rest part origins
```

`assembly_animation.rs` is the load-bearing one, and it asserts in both directions: an animated run's
per-frame `WorldHash` trace and its recorded `.lrp` are byte-identical whether or not anything
extracts, **and** the same scene extracted at different times produces different part matrices (an
evaluator that did nothing would satisfy the first claim alone).
