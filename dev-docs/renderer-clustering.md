---
description: Clustered forward: the light payload, the cell grid, the CPU binning, and its conservatism proof
---
# Clustered forward: the light payload

**Scope:** How every light a scene places reaches every lit fragment with no cap and no selection —
the `@group(1)` payload, the screen-tile × radial-shell grid, the CPU binning and the proof
obligations anyone touching it inherits, and the occupancy panel a light-cost regression is
diagnosed from. The BRDF that consumes a light is `ludion devdocs renderer-shading`; snippet
composition and the generated bindings are `ludion devdocs renderer-shaders` and
`ludion devdocs ludion-shader-bindgen`; the report section is `ludion devdocs profiling`; the
authoring side is `ludion docs lighting`.

## Layout

- `crates/ludion-renderer/src/shaders/snippets/lights.wgsl` — the GPU half, and the source of truth
  for the grid's shape: the `CLUSTER_TILES_X`/`CLUSTER_TILES_Y`/`CLUSTER_SLICES` consts,
  `Light`/`LightHeader`, all four `@group(1)` bindings, and `cluster_slice(frag_pos, view_dist_sq)`.
  Composed ahead of both `mesh.wgsl` and `sprite.wgsl`, so one declaration serves both consumers.
- `crates/ludion-renderer/src/cluster.rs` — the CPU half, and **the only place the which-lights-in-
  which-cell decision is made**: `build_lights` (the frame's whole payload), `cull` (the shipping
  build), `degenerate` (`#[cfg(test)]` — the oracle), `cluster_stats` (the profiler's adapter-free
  entry point), `ClusterStats`.
- `crates/ludion-renderer/src/render.rs` — `bind_lights` uploads the four entries and builds the
  bind group; `Renderer::cluster_build` is a `#[cfg(test)]`-only `Cell` the oracle flips.
- `crates/ludion-engine/src/extract.rs` — `extract_lights`, which fixes the array's order. Its doc
  comment is the contract; `ludion devdocs engine-image-paths` states it on the extract side.

## Dataflow

Once per frame, in the renderer's GPU-free prologue: `build_lights(scene, render_extent, Culled)`
produces a header uniform (`@binding(0)`) plus three read-only storage buffers — `light_list` every
light in the scene, `cluster_table` one `(offset, count)` per cell, `cluster_lights` the
concatenated per-cell index lists. A fragment maps `@builtin(position)` → tile and its squared
camera distance → shell, fetches its cell's `(offset, count)`, and iterates only that slice. Cost
tracks the lights actually near the pixel.

The grid is a fixed **count** — 16 × 9 tiles × 16 radial shells = 2304 cells — never a fixed pixel
size, so a supersampled frame clusters exactly like its native twin and `--scale` stays a pure
quality knob. The third axis is **radial distance from the camera**, not view-space Z: `Camera`
carries `view_proj` and `cam_pos` and no view matrix, and a sphere-vs-shell test is easier to make
conservative than a sphere-vs-z-plane one. The ladder is geometric over `[1, 512]` world units —
both powers of two, so the derived `(scale, bias)` are exact — and the shader is handed the
*squared* distance with `log2(d²) = 2·log2(d)`'s halving folded into `scale` CPU-side, so no lit
fragment pays a square root.

`cull` is three sequential passes — count, prefix-sum, scatter — a counting sort keyed by the
*light* index, so each cell's slice comes out ascending with no sort and no per-cell `Vec`. Per
light it derives a `Footprint`: an inclusive tile rectangle from the screen AABB of the range
sphere's eight bounding-box corners, and an inclusive shell interval from `[d − range, d + range]`
(the triangle inequality). Both are widened outward (`TILE_PIXEL_MARGIN` one pixel, `SHELL_MARGIN`
1%), and a light with any corner at or behind the camera takes the full-tile fallback.

A spot is the same `Light` with a third `vec4`: `axis.xyz` its unit direction, `axis.w` its
`cos_outer`, `color.w` its `cos_inner`; a point light writes `axis = (0, 0, 0, -2)`. `footprint_of`
feeds the same shell and tile tests a spot's **cone bounding sphere** (`bounding_sphere`): for an
outer half-angle θ ≤ 45° the sphere through the apex and the rim — centre `apex + axis·R/(2cosθ)`,
radius `R/(2cosθ)`, widened 0.1% — and the range sphere at the apex for anything wider.

## Rules

- **Conservatism is mandatory; tightness is negotiable.** Every light whose range sphere can reach a
  fragment in a cell must appear in that cell's list. A light admitted *spuriously* costs a loop
  iteration and contributes exactly `+0.0` — both consumers' attenuation is a hard zero at
  `dist >= range`, a spot's cone factor (`spot_cone`) is a hard zero at or outside its outer cone,
  and `acc + 0.0` is bit-preserving — so over-inclusion cannot move a pixel; a light
  *missed* is a visible hole. Every margin rounds outward, and a new or tightened test owes a
  written conservatism argument of the kind `tile_range`'s convex-hull one makes, not just a passing
  case.
- **The order is load-bearing on both sides.** `scene.lights` arrives in ascending-`EntityId`
  extract order and is packed verbatim, then `scene.spot_lights` in theirs — a spot-free scene
  packs exactly as a point-only one; within a cell the indices ascend. Together those make the
  clustered loop's float sum *exactly* a flat loop's — the identity argument. Never sort, filter or
  re-associate: it churns every lit golden and voids the proof.
- **Binning is deliberately off the determinism-critical path.** Because the rendered result is
  independent of over-inclusion, two platforms whose `log2` disagrees in the last bit may bin
  differently and still render identically. The requirement is conservatism, not bit-reproducibility
  — do not add a cross-platform bit-identity gate on `cluster_table`, and do not read a differing
  `total_refs` across OSes as a defect.
- **The degenerate build is an oracle, never a mode.** `ClusterBuild::Degenerate` and the
  `Renderer::cluster_build` switch are `#[cfg(test)]`, so a shipping build cannot be put into it. It
  files every light into every cell — the conservative extreme, correct by definition — which is
  what makes it the reference: a frame the culled build renders differently is a hole in the culled
  build, not a difference of opinion.
- **No implicit derivatives inside a light loop.** The loop bound is non-uniform, which makes
  `textureSample`/`dpdx`/`dpdy` illegal there. Both consumers are ALU-only (the sun's PCF sits
  outside the loop); an effect wanting a per-light texture read moves the read out or pays
  `textureSampleLevel`.
- **The build stays CPU-side and sequential by measurement, not by preference.**
  `ludion_core::par` and a GPU compute build are both unbuilt behind one trigger: a measured 0.3 ms
  at the stress population. `cluster_build_cost_at_the_stress_population` is that measurement (500
  lights, `#[ignore]`d, run `--release --ignored --nocapture`); re-run it before assuming otherwise.
- **CPU and GPU cell arithmetic mirror each other exactly.** `shell_of` and `tile_of` reproduce the
  snippet's clamp-then-truncate-then-`min` shape down to the `max(_, 1e-12)` guard, and
  `(shell, tile_y, tile_x)` row-major is the one cell ordering both sides index. A divergence here
  is a hole no unit test of either side alone can see — the oracle is what catches it.

## Gotchas

- **The GI arc's light bin (`gi_bin.rs`) inherits this module's counting-sort *shape*, one seam
  inward, over a different domain and a different determinism stance.** Same count/prefix-sum/scatter
  passes keyed by light index, so a cell's list ascends the same way — but it bins the voxel grid's
  **world** box instead of the view frustum (both its consumers ask a world-space question with no
  camera in it), and unlike this module it is **deliberately deterministic and fixed-order**, not
  "not bit-reproducible": today both bins' consumers re-test range exactly, so over-inclusion is
  invisible either way, but the GI bin's gather is one step closer to a future aggregation that would
  make bin *assignment* value-bearing, and staying deterministic now is what keeps that door open
  cheaply. `ludion devdocs renderer-gi-dynamic` owns it.
- **A light-cost regression is diagnosed from one `ludion profile` run, not a GPU capture.** The
  report's `cluster` section pairs `build_median_ms` with the occupancy of the tables that build
  produced. `scene` up with occupancy *unmoved* means shading got more expensive; `scene` up with
  `total_refs`/`max_list_len` *climbing* means the binning got looser, and the fix is a tighter
  footprint rather than a shader change. `total_refs` is the honest measure: it grows when
  conservatism loosens even if no single list gets longer.
- **A point light's arithmetic must stay the point-only shader's.** Both loops multiply
  `spot_factor` (an exact `1.0` for `axis.w < -1.5`) into the colour *first*, so every later
  product, rounding and FMA contraction is unchanged. Branching around the whole statement instead
  moved the `tilemap` golden on the RTX 3090 — the driver contracts the two arms differently.
- **Spots never reach GI.** `RenderScene.lights` alone feeds the bake and injection; their `Light`
  declarations carry the point encoding.
- **An empty light array still uploads one element.** wgpu rejects a zero-sized binding and the
  generated `min_binding_size` is one element (naga sizes a runtime array as exactly one), so
  `build_lights` pushes a zeroed `Light` and a zero index. Every cell's count is 0, so the shader
  never reads the padding — shrinking the buffer "correctly" is a validation error at bind time.
- **`ClusterStats` describes what the tables *say*, not how they are stored.** The oracle encodes
  every-light-in-every-cell as one shared `[0..N)` list that every cell's `(0, N)` entry points at,
  and still reports `cells × lights` refs. The panel is the shape a fragment sees.
- **The fixed indirection is not free on a cheap scene.** Header fetch + `log2` shell + table fetch
  are ~20% of a `scene` pass whose light loop runs three iterations: `examples/gallery/lights.yaml`
  went 0.051 → 0.0614 ms and its ceiling moved to 0.075 (`budgets/pinned-rtx3090.toml`). The win is
  at the other end — 500 lights on `lights-stress.yaml` cost 11× that pass at 167× the lights, with
  66% of the grid unoccupied.

## Verify

```
cargo test -p ludion-renderer cluster            # footprints, the CPU/GPU ladder mirror, stats re-derived the slow way
cargo test -p ludion-renderer needs_gpu_the_culled_build_renders_byte_identically_to_the_degenerate_oracle
cargo test -p ludion-renderer                    # the 18-light, sprite-parity and population byte-stability fixtures
cargo run -p ludion-shader-bindgen -- --check    # the snippet's structs/bindings against shader_gen.rs
cargo run -p ludion-cli --release -- profile --scene examples/gallery/lights-stress.yaml --frames 120 \
  --check budgets/pinned-rtx3090-lights-stress.toml
```

The oracle and the fixtures graceful-skip without an adapter (`L2003`), so a green run on a headless
box proves less than it looks — run them where a GPU exists before believing a binning change.
