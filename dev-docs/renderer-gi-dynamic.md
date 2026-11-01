---
description: The world-space GI light bin, the strided hit-cache replay, and the gi_inject compute pass
---

# Renderer GI — dynamic injection

**Scope:** `gi_bin.rs`, `gi_inject.rs` + `shaders/gi_inject.wgsl` — the machinery behind the
`GiDynamic` marker's injected term: the world-space light bin (shared with the static bake), the
bake's hit cache and why it strides, the per-frame compute pass that replays it, and the quality knob
that masks the result. The bake's own machinery (march, probe lattice, sky visibility) is
`ludion devdocs renderer-gi`; the static bounce term the bin's other consumer feeds is
`ludion devdocs renderer-gi-bounce`; the marker's authoring-side meaning is `ludion docs
global-illumination`; the compute-pass primitives this pass rides on are `ludion devdocs
ludion-render-graph`.

## Layout

- `crates/ludion-renderer/src/gi_bin.rs` — `GiBinParams`, `LightBin`, `BinBuild` (`Culled`, and a
  `#[cfg(test)]`-only `Degenerate` oracle), `build_bin`: the counting-sort bin the static bake and
  the per-frame injection share, one build, two callers.
- `crates/ludion-renderer/src/gi_inject.rs` — `GiInject` (the `ibl.rs`-shaped renderer field:
  compute pipeline + 1×1×1 fallback volume), `scheduled_lattice`, `InjectInput`/`GiInjectFrame`,
  `dynamic_lights`.
- `crates/ludion-renderer/src/shaders/gi_inject.wgsl` — one `@compute` entry point, `inject`. Its
  PODs, binding consts and workgroup size are generated; the layout stays auto-derived, and its
  resources carry an `inject_` prefix.
- `crates/ludion-renderer/src/gi_bake.rs` — the hit cache's writer and packing constants (`HIT_RAYS`,
  `HIT_FACE_SHIFT`/`_BOUNDARY`/`_ESCAPED`), and `static_lights`/`premultiplied`, which
  `dynamic_lights` mirrors.
- `crates/ludion-components/src/lib.rs` — `GiDynamic`; `extract.rs` — `PointLightData.dynamic`
  and the mesh-side exclusion (`ludion devdocs engine-voxelizer`).
- `crates/ludion-renderer/src/quality.rs` — `Quality::gi_dynamic`; `graph.rs` —
  `PostChain::gi_inject`/`masked`, the scheduling seam.

## Dataflow

```
extract: PointLightData.dynamic = the GiDynamic marker's presence
    → RenderScene.lights: every light, marked or not, ascending-EntityId order, UNCHANGED —
      clustered forward still reads it whole; the split governs GI only.

prepare_frame, once chain.gi_inject is Some (a lattice, bounce > 0, ≥1 marked light, quality on):
    dynamic_lights(scene.lights)       → the marked subset, premultiplied — static_lights' exact
                                          complement, same pinned extract order
    gi_bin::build_bin(grid, …, Culled) → a LightBin: (offset, count) table + index list, over the
                                          voxel grid's world box
    GiInject::prepare                  → GiInjectFrame: params uniform, light/bin-table/bin-index
                                          storage buffers, the bake's hit-cache buffer (borrowed),
                                          the voxel albedo 3D view

gi_inject.wgsl `inject`, one invocation per probe:
    for k in 0..K:
        word = hit_cache[probe*K + k]     // the bake's own march, replayed — never re-traced
        escaped → skip
        else    → cell = unpack_cell(word), n = hit_normal(face, k)
                   a = textureLoad(albedo, cell)
                   loop bin_table[bin_cell_of(cell_center(cell))]'s slice (not light_list whole)
                       → acc += a * light.color * ndl * atten²
    dyn_out[probe] = vec4(acc / K, 0.0)   // one RGB DC texel, every probe, every frame

mesh.wgsl `shade`, nested inside the existing `gi_bounce.x > 0.0` branch:
    env.gi_bounce.y > 0.5 → acc += gi_dynamic(world_pos, n) * diffuse * env.gi_bounce.x
                             (group 3 binding 10, `gi_dyn_tex`)
```

## Rules

- **The bin's shape is `cluster.rs`'s; its domain is not.** Same three-pass counting sort (count,
  prefix-sum, scatter) keyed by light index, lights outer / cells inner, so every cell's list ascends
  by construction with no sort — but `cluster.rs` bins the view frustum in screen-tile × radial-shell
  space, and this bin covers a fixed `32×16×32` grid over the voxel grid's own **world** box, since
  both consumers (a cached hit, a bake march) ask a world-space question with no camera in it.
  `ludion devdocs renderer-clustering` carries the shape's own proof; this topic never re-derives it.
- **Conservatism is mandatory, tightness is negotiable — the `cluster.rs` rule, transplanted.** A
  light files into every cell its range sphere (expanded by one voxel cell, covering the CPU/shader
  rounding gap) can reach; over-filing costs a loop iteration and contributes an exact `+0.0` (a hard
  zero at `dist >= range`, added to a sum of non-negatives), so it cannot move a value. A light the
  bin *misses* is a wrong number — why the margin always rounds outward, and why the `#[cfg(test)]`
  degenerate (every-light-in-every-cell) oracle exists: any bake or injection it disagrees with has a
  filing hole, not a difference of opinion.
- **This bin does NOT inherit `cluster.rs`'s "deliberately not bit-reproducible" stance — the reason
  must survive.** `cluster.rs`'s consumer re-tests range exactly, so its *assignment* never reaches a
  rendered value, and two platforms may bin differently and still render identically. This bin's
  build is instead **fixed-order and deterministic as written** (sequential, no `par`, no atomics,
  additions/multiplications/`floor` only, no transcendentals) — nearly free today, since the gather's
  per-light term carries the same hard zero-at-range property this bin hides over-inclusion behind.
  It must **stay** that way: the moment a future change sums lights into a per-cell virtual source
  (the deferred stage-1 aggregation, `Plan/gi-wave3-dynamic-injection.md` §16), bin *assignment*
  becomes part of the radiance arithmetic and lands in pixels — such a change must carry a
  cross-process byte-identity fixture as its gate.
- **The hit cache holds a strided subset of the bake's rays, not a prefix — because a prefix is a
  polar cap.** The bake's ray set sweeps elevation monotonically (`y = 1 − 2(i+0.5)/N`); its first
  sixteen of sixty-four directions are a ~59°-half-angle cone about `+Y`. A cache built from that
  prefix replays a probe's ceiling and never its floor — measured as exactly zero injection under an
  open sky with a marked light overhead, the vigil ground-bounce case. Slot `k` holds ray
  `k * (rays / K)` instead, spanning elevation evenly at the same storage cost. `K = HIT_RAYS = 16` is
  a constant on both sides because it sizes the buffer; the **stride** is derived from the ray count,
  which is `QualityParams::gi_rays` and therefore a per-machine setting — hence the rule that a
  selectable count is a whole multiple of 16, and hence the count travelling from
  `ResolvedGi::rays` into `InjectInput::rays` rather than being read twice
  (`ludion devdocs renderer-quality`).
- **The hit word's packing: cell index in bits `0..HIT_FACE_SHIFT` (25), face code above.** Codes
  `0..5` are the six axis faces (`0 = -X, 1 = +X, …, 5 = +Z`, low bit = sign). Code `6`
  (`HIT_FACE_BOUNDARY`) means the DDA took no step (the probe started inside its own cell), so the
  normal is recomputed as `-ray_direction(k * (rays / K))` instead of read from the word — the one
  place either shader needs `k` for anything but indexing. Code `7` (`HIT_FACE_ESCAPED`) means the
  ray left the grid. `gi_bake.wgsl` writes the word, `gi_inject.wgsl` unpacks it, and both agree with
  `gi_bake.rs`'s Rust constants: a compile-time assertion plus
  `the_two_gi_shaders_agree_on_the_ray_set_and_the_hit_layout` hold all four declarations (two `.rs`,
  two `.wgsl`) to one set of numbers.
- **The scheduling predicate is one function, read by both the graph and the recorder.**
  `scheduled_lattice` is `Some(lattice)` iff a `GiVolume` resolved a real lattice **and**
  `bounce > 0.0` **and** at least one light carries `GiDynamic` — the machine's quality switch is
  deliberately not part of it (masking is `PostChain::masked`'s job, applied after). All three are
  load-bearing: without a lattice there is nowhere to write, `bounce: 0.0` means the injected term
  (which rides that same strength — no `dynamic_bounce` knob) has nothing to scale into, and zero
  marked lights would only ever write an all-zero volume for `+0.0`.
- **Energy: one term per light, in exactly one of the two GI paths, never both.** The gather shades
  only *replayed hits* (cached, static geometry) under the *dynamic* subset — never `light_list` in
  full, and never a hit cell's own emissive: that already lives in the baked bounce (a static term),
  so re-adding it would double-count every torch a bolt flies past. An escaped ray contributes
  nothing (sky is the visibility term's business), and a static light never reaches `light_list` —
  `dynamic_lights`/`static_lights` are exact complements over the same pinned extract order.
- **Determinism is per-machine, exactly where the bake sits.** `k` ascends, each bin cell's list
  ascends by construction, one invocation per probe, no atomics — but the accumulation is GPU float
  arithmetic: byte-identical across renders on one adapter, not claimed across adapters.
- **Quality masks the injected term only — never the marker's bake exclusion.** `Quality::gi_dynamic`
  (`false` at `LOW`) is read once, in `PostChain::masked`: `gi_inject: self.gi_inject.filter(|_|
  quality.gi_dynamic)`. Masked, the pass is not scheduled, the fallback 1×1×1 volume binds, and
  `EnvParams.gi_bounce.y` writes `0.0` — the *same* codegen path a scene with no marked light takes,
  never a cheaper variant of the marked one. What quality cannot reach: a marked light's exclusion
  from the static bake and a marked mesh's exclusion from the voxelizer, both world-state semantics
  fixed by `GiDynamic` itself, not by the machine rendering it.

## Gotchas

- **The bin is not the injection's alone.** `gi_bake.wgsl`'s `direct_at` also looks up
  `bin_table[bin_cell_of(hit)]`, over the *static* subset, built once per bake rather than once per
  frame (`ludion devdocs renderer-gi-bounce` owns the radiance it computes) — so a change to `gi_bin.rs`'s filing rule moves both consumers, and the byte-identity proof
  (the culled-vs-degenerate oracle) must be re-run for both, not just the one being changed.
- **No per-probe early-out on an empty bin *neighbourhood*.** A probe's `K` cached hits can land
  anywhere the bake's rays reached, so no bin-space region conservatively bounds them short of
  reading the words — the per-hit `ESCAPED`/empty-slice skip already in the inner loop. A coarser
  skip would not pay for itself.
- **A marked mesh's motion is free by construction, not convention.** `voxelizer.rs`'s `resolve`
  drops a `GiDynamic`-marked instance before it reaches the signature (`ludion devdocs
  engine-voxelizer`), so this module never sees it move — the energy rule above is the light-side
  mirror of the same marker.
- **The fragment stage is at 10 of the guaranteed 16 sampled textures after this pass** (nine from
  the sky-visibility + bounce trio, one more for `gi_dyn_tex` at binding 10). A directional upgrade
  (three more volumes, an SH-L1 set like the static bounce's) would take it to 13 — counted, not
  assumed.

## Verify

```
cargo test -p ludion-renderer gi_bin              # bin conservatism, ascent, determinism, the degenerate oracle
cargo test -p ludion-renderer gi_inject           # Gates A/B, the bin oracle (both consumers), the strided-cache pin
cargo run -p ludion-cli -- dump-graph --scene examples/gallery/gi-interior.yaml --quality low   # gi_inject absent
cargo run -p ludion-cli -- dump-graph --scene examples/gallery/gi-interior.yaml                 # gi_inject present
```

Every GPU fixture graceful-skips without an adapter (`L2003`); the bin's own conservatism proof
(`the_bin_never_drops_a_light_that_reaches_an_evaluation_point`) needs none — it runs on CPU-built
tables alone.
