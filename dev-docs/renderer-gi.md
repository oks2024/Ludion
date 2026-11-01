---
description: The GI probe bake's shared machinery — the DDA march, the cache, sky-visibility consumption
---

# Renderer GI (bake infrastructure + sky visibility)

**Scope:** `gi_bake.rs` + `shaders/gi_bake.wgsl` — the GPU compute bake that turns the voxelizer's
occupancy grid into a baked SH-L1 probe volume — and `mesh.wgsl`'s sky-visibility consumption site.
This topic owns the machinery both GI terms share: the probe lattice, the DDA march (`trace_hit`), the
cache, and the storage format. The second term the same bake produces — one bounce of surface-reflected
radiance — is `ludion devdocs renderer-gi-bounce`. The CPU voxel grid the bake reads is
`ludion devdocs engine-voxelizer`; the component's authored fields are `ludion docs global-illumination`.

## Layout

- `crates/ludion-renderer/src/gi_bake.rs` — `GiBake` (the `ibl.rs`-shaped renderer field:
  `resolve(&gpu, Option<BakeInput>)`, a single-entry cache, 1×1×1 fallbacks), `ResolvedGi` (four probe
  texture views + origin/scale uniforms), `BakeInput`, the compute pipeline setup.
- `crates/ludion-renderer/src/shaders/gi_bake.wgsl` — one `@compute` entry point, `bake`. Its PODs,
  binding consts and workgroup size are generated; the layout stays auto-derived (`layout: None`),
  so its resources carry a `bake_` prefix (bindgen unifies resources by name).
- `crates/ludion-renderer/src/shaders/mesh.wgsl` — `sh_l1` (the shared SH-L1 reconstruction) and
  `gi_sky_visibility(world_pos, n)`, called from `fs_main`.
- `crates/ludion-renderer/src/scene.rs` — `GiData`, the plain-data extract carries alongside
  `RenderScene.voxel`; `MAX_GI_TEXTURE_AXIS`. `gi_bake.rs` owns `max_gi_probes()`, the probe cap
  derived from the device's storage-binding limit (the 64 B/probe hit cache is the largest buffer).

## Dataflow

```
voxelizer (ludion-engine) → WorldVoxelGrid
    → voxel_bake.rs: occupancy/emissive + albedo upload (keyed on grid.hash)
        → gi_bake.rs::resolve(gpu, BakeInput):
            cache key (grid.hash, probe_spacing.to_bits(), lights_signature, bounce_enabled, rays)
            hit  → cached ResolvedGi, no dispatch
            miss → gi_bake.wgsl `bake` compute pass, one workgroup per 4×4×4 probe block,
                   one march per ray (trace_hit — shared with renderer-gi-bounce)
                → visibility: Rgba16Float 3D texture (SH-L1: .x = DC term, .yzw = L1 vector)
                → bounce, when authored: three sibling volumes — renderer-gi-bounce owns this half
    → mesh.wgsl group 3 binding 6 `gi_tex`, sampled through `gi_sky_visibility()`
        → multiplies the ambient/IBL term, after the AO factor
        → (bindings 7/8/9 add the bounce term after that multiply — renderer-gi-bounce)
```

`resolve` runs in `prepare_frame`, **outside the frame graph** — it never appears in `--dump-timings`
and is not one of `build_frame_graph`'s passes; a `GiVolume` scene adds zero entries to the graph's
pass list, exactly like `Environment`'s sky and IBL.

## Rules

- **`Rgba16Float`, not `Rgba32Float`.** It is filterable on wgpu with no `float32-filterable` feature
  requested, and the consumer needs hardware trilinear between probes (they sit metres apart —
  nearest sampling would be visibly blocky). Precision is ample: visibility is `[0, 1]` and the L1
  vector's magnitude is bounded by `0.75`.
- **The sampler-pairing trap lives in the bake's own bind group, not the consumer's.** The compute
  pipeline is built with `layout: None`; wgpu derives a sampled texture's `filterable` from whether the
  entry point pairs it with a sampler, and `bake` only ever `textureLoad`s its voxel-grid inputs — so
  the non-filterable formats the voxel upload uses satisfy the auto-derived layout with no device
  feature and no hand-written BGL. **Pairing one of those textures with a sampler in the shader would
  break the derivation, not merely change it.** (`gi_tex`, the bake's *output*, is a separate, genuinely
  filterable `Rgba16Float` texture sampled with the ordinary `env_samp` on the consumption side.)
- **Ray set: spherical-Fibonacci directions, computed from the ray index and the count alone.**
  `y = 1 - 2(i + 0.5)/N`, azimuth advanced by the golden angle (`fract(i * 0.6180339887…)`). No RNG,
  no per-probe jitter. **The count is a quality axis, not an authored field**
  (`QualityParams::gi_rays`, 64 at `full` down to 16 at `low`), uploaded as `params.flags.y` rather
  than declared as a constant — so it is a host setting the scene never sees. Only the azimuth is
  count-free: the elevation divides by `N`, so a smaller count **re-derives** the whole set rather
  than truncating it. Each count is nevertheless a complete, well-formed sphere on its own terms
  (`the_fibonacci_ray_set_is_a_whole_sphere_at_every_quality_ray_count`), and `ludion devdocs
  renderer-quality` owns what follows from that.
- **March: Amanatides–Woo DDA, cell by cell, to the volume boundary — reports a `Hit`, not a bit.**
  `trace_hit(start, dir, extent) -> Hit { hit, cell, normal }`; visibility is `select(1.0, 0.0,
  hit.hit)`, the same value from the same walk. Exit test is `exit <= max(enter, 0.0)`, not `<`: a
  probe on a max face pointing outward (the lattice puts probes exactly on grid boundaries whenever
  `probe_spacing` divides the box span) or a ray that grazes an edge must count as an escape traversing
  zero cells — under `<` both fall through to the boundary clamp and read a cell they never actually
  entered, producing a dark rim around the volume. The hit **normal** rides along free: the DDA already
  branches on which axis it stepped, so the face the ray crossed to enter the occupied cell is exactly
  that voxel face; a ray with no step yet (started inside its own cell) takes the negated ray direction
  instead. `renderer-gi-bounce` is the normal's only reader.
- **Probe-in-solid push-out is the bake's half of leak control.** If a probe's own cell is occupied,
  search the 3³ neighbourhood then the 5³ in a fixed shell order (ascending z, y, x within each shell)
  for the nearest free cell and trace from there instead. If none is free, write `vec4(0.0)` — a
  fully buried probe contributes **darkness**, the correct failure direction, since a buried probe
  that reports sky is exactly the leak this exists to prevent. (Known incomplete: the push-out can
  legitimately relocate a probe embedded in a wall thin relative to `probe_spacing`, with open exterior
  immediately adjacent, to the *bright* side — the calibration rule "`probe_spacing` ≤ thinnest
  occluder" in `ludion docs global-illumination` is the authoring-side guard, not a bake-side fix.)
- **The storage seam is exactly one function per term.** `mesh.wgsl`'s `gi_sky_visibility(world_pos,
  n)` is the only site that reads the visibility volume; an ambient-cube experiment later replaces that
  function's body and the bake's write, nothing else. Reconstruction is cosine-convolved SH-L1
  (`A0 = 1`, `A1 = 2/3`) through the shared `sh_l1(sh, n)` helper, a plain `dot`, because the bake
  already folds the band's `3.0` normalization into what it writes (`vec4(sh0/N, 3.0*sh1/N)`).
- **Applied to the ambient term only, on both shader branches.** The multiply sits after the existing
  `env.ao.x` (SSAO) multiply so the two attenuations compose independently, and it covers *both* the
  IBL branch and the flat `lights.ambient` fallback branch — a scene with no `Environment` still has an
  ambient constant an interior should not receive. Direct lighting and emissive are untouched at every
  `sky_visibility` setting; `fs_main` adds emissive after `shade` runs, same as the `ssao_direct`
  precedent.
- **Neutrality is arithmetic, not a branch guard.** Absence binds the 1×1×1 fallback and a zero
  `gi_origin.w` flag, so the branch is never taken. `sky_visibility: 0.0` *does* take the branch —
  `acc * mix(1.0, visibility, 0.0)` is `acc * 1.0`, bit-preserving under IEEE-754 — which is the gate
  that actually exercises the new binding and branch rather than merely proving it unreachable.
- **The cache key covers every input the dispatch reads.** `(grid.hash, probe_spacing.to_bits(),
  lights_signature, bounce_enabled, rays)` — the grid hash and spacing are the visibility term's own
  inputs, and `rays` is there because a different count re-derives every direction, so a held-over
  volume would be stale rather than coarse;
  `lights_signature` and `bounce_enabled` exist because the same dispatch also produces the bounce term,
  and are `renderer-gi-bounce`'s to explain. `normal_bias` stays out (sample-time, read only in
  `mesh.wgsl`).
- **The rebake lever suspends the *response* to a key miss, never the key.** `bake_or_cached` still
  computes the whole key every frame; under hold, a miss with a cache present serves the retained
  volumes and raises `Renderer::gi_stale` instead of dispatching. Staleness is therefore recomputed
  per frame and never latched — a world that moves away from the held baseline and back reports stale,
  then clean — and a *first* build is never held (an unlit frame is a worse answer than a stale one).
  The lever is host/session state (`Engine::set_gi_rebake`, `render.gi_rebake` on the wire): it never
  enters a `WorldHash`, a snapshot or a replay, and no one-shot path (`ludion render`, goldens,
  `sim-hash`) has a spelling of it, so `auto` is byte-identical to no lever at all. The engine ORs
  this with the voxelizer's own hold flag into one `Engine::gi_stale`, which rides out on the frame
  header's flags byte (bit 0) and on `render.screenshot`'s `stale`.
- **Determinism is per-machine, never cross-platform.** One invocation per probe, no atomics, no
  cross-invocation reduction, a fixed ray order — but the accumulation is GPU float arithmetic, so it
  sits exactly where `ibl.rs`'s prefilter sits: bit-identical across repeated renders on one adapter,
  never claimed bit-identical across OSes or GPUs.

## Gotchas

- **The voxelizer dominates the cost, not the bake — at room scale.** Measured on `post-full` + a
  `GiVolume` at `cell_size 0.5` (grid 440×44×440 = 8.52M cells, RTX 3090): the GPU bake is ~60 ms at
  `probe_spacing 1.0` (1.12M probes), ~47 ms at `2.0`, ~8.1 ms at `4.0` — comfortably inside a ~200 ms
  district target — while the CPU voxelize that feeds it runs 310–340 ms, an order of magnitude more
  (`ludion devdocs engine-voxelizer`). The `2.0` row's timing is anomalous relative to `1.0`/`4.0`
  (7.6× fewer probes for only 1.3× less time, repeatable within a run) — recorded, not yet explained.
  This balance does not hold once bounce is on at district scale; see `renderer-gi-bounce`.
- **`EnvParams` has grown twice, and only the first growth reclaimed a pad slot.** 32 → 64 bytes when
  the former `params.w` pad became `gi_strength` (the authored `sky_visibility`); 64 → 80 bytes when
  `gi_bounce` was added, which took a wholly new `vec4` because no pad slot remained (`renderer-gi-bounce`
  has the layout). Check for unused pad slots before growing a uniform struct, but do not assume one
  exists.
- **`GiData`/`ResolvedGi` are `None`/fallback whenever `scene.gi` is absent**, including a `GiVolume`
  authored over a scene with no geometry to voxelize. `ludion-cli` checks `scene.gi` and reports
  `L2104` before this path is ever reached for a dump request, so the fallback here is defensive, not
  load-bearing for the CLI's own error UX.
- Regenerate `shader_gen.rs` with `node scripts/check.mjs --fix` after touching `mesh.wgsl`'s group-3
  layout; never hand-edit it (`ludion devdocs renderer-shaders`).

## Verify

```
cargo test -p ludion-renderer                                  # gi_bake unit tests, scene GiData shape
cargo run -p ludion-cli -- render --scene examples/gallery/gi-interior.yaml --dump-gi target/gi.probes.png
cargo nextest run -p ludion-cli --test it -E 'test(/^render_gi::/)'                      # --dump-gi CLI behavior, requires a GPU adapter
cargo run -p ludion-cli -- test --dir examples/gallery          # gi-interior against its committed golden
```

Rendering `gi-interior` (or any `GiVolume` scene) twice on the same adapter must produce byte-identical
PNGs — the bake's per-machine determinism claim, proved the same way as every other renderer golden.
