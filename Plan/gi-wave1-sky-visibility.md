# GI Wave 1 — sky visibility (detailed implementation plan)

> Status: **DELIVERED (2026-08-07).** Feature path Lore revs 511–514; Wave D revs 516–519 (a
> review-fix batch rode the same wave). Expands `Plan/global-illumination.md` §4's W1 outline against
> the code as W0 left it (`Plan/gi-wave0-voxelizer.md` Status is the predecessor's outcome record).
> The private graphics-engine notebook's Done entries carry the outcome record and the findings worth not
> re-deriving; the commit messages carry the full detail.
>
> **Feature path (revs 511–514), each with full `check.mjs` green and every existing golden
> byte-identical:** **WP-1** the load-time voxelization cache + the grid's sealed `hash` (rev 511),
> **WP-2** `GiVolume`'s three new fields (rev 511), **WP-3** the GPU bake `gi_bake.rs`/`gi_bake.wgsl`
> + `L2105` + `GiData` (rev 512), **WP-4** `mesh.wgsl` consumption + the group-3 binding + `EnvParams`
> 32 → 64 (rev 513), and a review-fix commit (rev 514: per-axis budget caps, the f32 extent overflow,
> the DDA tangent case).
>
> **Wave D (revs 516–519), run as one parallel four-package wave:** **WP-5** `gi-interior` + its
> first-bless Windows golden, with `cell_size 0.25` / `probe_spacing 0.5` / `normal_bias 0.25` chosen
> off committed sweep sheets (`Plan/artifacts/gi-wave1-*.png`, deleted before publication and in
> Lore history, + `-sweeps.md`; the criteria: spacing
> must not exceed the thinnest occluder, bias is half the spacing) and one authored `PostProcess`
> field, `lift 0.002`, because a fully occluded surface receives exactly zero ambient and 10% of the
> frame sat on the black rail where a golden resolves nothing. **WP-6** §11 tests 2–5 in
> `ludion-engine/tests/gi_sky_visibility.rs`; the leak gate was demonstrated able to fail — the
> literal "push-out as no-op" mutation is provably toothless (`trace` already returns 0 for a buried
> start), so the mutation with teeth is an unconditional push-out *success* that skips occupancy and
> bounds checks, which reddened both assertions (dim 158.0, bright 214.7). **Test 6 lives in
> `ludion-cli::render_gi`, not the fixture file**: `CARGO_BIN_EXE_ludion` hands a cargo-managed
> binary there, and the first draft's in-test `cargo build` raced the parallel suite for the
> `ludion.exe` link target (os error 5 mid-suite). **WP-7** `--dump-gi` (VISIBILITY + BENT NORMAL
> sheet, f16 decoded by hand, no new dependency, `L2104` reused, GPU requirement in the help text).
> Plus the review batch: voxel-model emissive now mirrors the render path's `authored_material`
> selection, the LWW combine rule is pinned by an order-flipping test, vigil's neutrality gate guards
> the probe lattice (not just `voxel.is_some()`), and the point-sampling safety condition
> (`Transform.scale <= cell_size`) is recorded at the site with a boundary test. The three new GPU
> test binaries joined both keep-in-step lists, plus `ludion-cli::render_voxels` which the review
> found missing.
>
> **Findings for later waves:** (1) WP-6's fixture exploration found the push-out can legitimately
> relocate a wall-embedded probe to the *bright* side of a wall that is thin relative to
> `probe_spacing` with open exterior immediately adjacent — the shipped protection is not airtight;
> a future hardening pass owns it (the calibration criterion "spacing ≤ thinnest occluder" is the
> authoring-side guard). (2) §14's `2/3`-vs-`1.0` L1 reconstruction remains unmeasured; `gi-interior`
> and its camera are the right fixture. (3) The bake never appears in `--dump-timings` — it runs in
> `prepare_frame`, outside the frame graph.

---

## 1. Goal and the product

W1 is the first wave whose output a human can see. It bakes **sky visibility** — how much of the sky
each point of the world can actually reach — into a world-space SH-L1 probe volume, and `mesh.wgsl`
multiplies its IBL/ambient term by it. Alleys, interiors, and the underside of an arch stop receiving
open-sky fill.

**It carries no bounce and no light sources.** A ray that escapes the volume without hitting occupancy
contributes visibility 1; a ray that hits contributes 0. Emissive and static point lights are W2.

## 2. The neutrality contract — sharper than the plan pre-authorized

`Plan/global-illumination.md` §4 pre-authorizes W1 as the arc's one pixel-moving wave. **We do not
spend it.** W1 adds a *new* gallery scene (`gi-interior`, a first bless, not a move) and authors
`GiVolume` into **no existing committed scene**, so every existing golden stays byte-identical and the
re-bless budget carries to W4's content adoption. This follows what the post-process arc actually did
(seven waves of "zero goldens moved; the single re-bless belongs at the flip") rather than what its
plan text allowed.

Two gates, both mechanized:

- **Absent-component neutrality, by construction.** A scene with no `GiVolume` binds a 1×1×1 fallback
  probe texture, carries a zero flag, and takes the historical path. Proven by the whole golden suite
  staying byte-identical.
- **Identity-value neutrality, by arithmetic.** A scene that *authors* a `GiVolume` with
  `sky_visibility: 0.0` renders **byte-identical** to the same scene without one. The multiply is
  `acc * mix(1.0, vis, strength)`, and at `strength = 0` that is `acc * 1.0` — bit-preserving in
  IEEE-754, the `ssao_direct` precedent. This gate is what exercises the new branch and the new
  binding, so it is the one that matters.

The volumetric-medium W3 finding applies and must be respected: **a byte-identity claim is about
generated code, not just values** (an untaken branch moved a pixel there). Both gates are measured,
never assumed.

## 3. What the volume stores, and why

**SH-L1 of a scalar visibility function `V(ω) ∈ [0,1]`** — four floats per probe, packed into one
`Rgba16Float` 3D texture: `.x` = the DC term, `.yzw` = the L1 vector (the "bent normal", unnormalized).

Two rejected alternatives, so the choice is not re-litigated:

- **RGB sky radiance per probe** (project `V(ω)·L_sky(ω)`) would make the probe *replace* the diffuse
  IBL rather than multiply it. That kills the identity-value gate (there is no strength at which the
  arithmetic is bit-preserving), leaves the specular IBL unoccluded, and duplicates the sky colour the
  prefiltered cube already carries. The plan's own phrasing is "**multiplies** the IBL term by the
  volume's directional sky visibility."
- **A scalar-only (L0) term** is AO by another name: it has no direction, so a floor and a wall inside
  the same alley receive the same fill. The L1 band is what makes an interior's remaining light come
  *from the door*. D4's ruling — SH-L1 is the style-neutral format — is upheld.

`Rgba16Float` over `Rgba32Float` deliberately: it is **filterable** on wgpu without the
`float32-filterable` feature, and hardware trilinear between probes is not optional here (probes are
metres apart; nearest sampling would be visibly blocky). Precision is ample for `[0,1]` visibility and
an L1 vector bounded by 0.75 in magnitude. `ibl.rs` already ships `Rgba16Float` as a storage +
sampled texture, so the format is proven on both pinned GPUs.

**The storage seam D4 requires:** `mesh.wgsl` reads the volume through exactly one function,
`gi_sky_visibility(world_pos, normal) -> f32`. An ambient-cube experiment later replaces that function
and the bake's write, and nothing else.

W2 grows this by **adding** coefficient sets (RGB bounce), never by reinterpreting these four floats.

## 4. The structural debt W0 recorded, paid here

W0's outcome record names two per-frame cost shapes and says W1's bake "is where 'scene as loaded'
(D2a) must become structural rather than incidental":

1. **The voxelizer runs on every `extract`** — a live `GiVolume` scene rebuilds a multi-million-cell
   grid every frame.
2. **`VoxelBake`'s hash is proportional to grid size, per frame** — a ~48 MB district grid is re-hashed
   every frame even when the cache hits.

Both are fixed in WP-1, before any bake exists to inherit them:

- `WorldVoxelGrid` gains `hash: u64`, computed **once** where the grid is built (the same XXH3 over
  origin/cell_size/extent + the three parallel vectors that `voxel_bake.rs::grid_hash` computes today).
  `VoxelBake` compares `grid.hash` instead of re-hashing. The field is part of the grid's identity, so
  it participates in `PartialEq` like every other field.
- The voxelizer gains an **instance-signature cache**. The signature is `O(instances)`, not `O(cells)`:
  hash `cell_size` and, per placed mesh in ascending slot order, the model matrix, colour, material,
  and a source discriminant (primitive shape, or the asset/voxel path's own content hash). On a hit,
  `extract` returns the cached `Arc<WorldVoxelGrid>` and does no rasterization at all. The cache lives
  beside the other asset caches (the `AssetCache`/`SceneAssets` seam W0 used for `voxel_grid`), which
  is the only persistent state `extract` can reach.
- `RenderScene.voxel` becomes `Option<Arc<WorldVoxelGrid>>`. `Arc<T>: PartialEq` compares contents, so
  the extract-determinism test is unchanged in meaning.

This is D2(a) — "the scene as loaded is the bake input" — becoming a real cache rather than a
statement, and it is what makes a live `ludion serve` on a GI scene affordable. The documented
consequence stands: an *authored* entity that a system animates invalidates the signature every frame
and thrashes the cache. Vigil's moving cast is runtime-spawned, so it does not arise; the trigger for
adding `GiStatic` (D2 option b) is unchanged.

## 5. `GiVolume`'s W1 fields

The component exists (`crates/ludion-engine/src/components.rs`, registered last, index 17). W1 adds
three fields; **no new component L-code** — the schema's own ranges are `L1014` at the field pointer,
which is the discipline that has held for eleven waves.

| Field | Type | Default | Bound | Meaning |
|---|---|---|---|---|
| `cell_size` | `f32` | `0.5` | `exclusiveMinimum: 0` | *(existing)* voxel grid resolution |
| `probe_spacing` | `f32` | `1.0` | `exclusiveMinimum: 0` | world units between probes (D4: authored, never an engine constant) |
| `normal_bias` | `f32` | `0.25` | `minimum: 0` | world-space offset along the surface normal before sampling — the leak control that lives at sample time |
| `sky_visibility` | `f32` | `1.0` | `range 0..=1` | strength: `mix(1.0, V, sky_visibility)`. `0` is the identity gate; a `--set` sweep axis; W3's "intensity knob" arrives already built |

Ray count per probe is **not** authored: a fixed engine constant `GI_BAKE_RAYS = 64`. It is a
quality-tier axis (D0: "bake resolution is a second quality-tier axis"), and the tier system does not
exist yet — a field the tier system will want to own is a field authored in the wrong place. Record the
trigger, do not add it.

Adding fields to `GiVolume` changes the serialized value of an authored `GiVolume`, hence `WorldHash`
for a scene carrying one. **No committed scene authors `GiVolume`**, so no replay re-records and no
`sim-hash` reference moves. Verify this rather than assume it.

## 6. Probe volume geometry

Probes sit on a **lattice aligned to the grid's origin**, not at cell centres:

```
count[a] = ceil(span[a] / probe_spacing) + 1          // span = extent[a] * cell_size
probe(i,j,k) world position = grid.origin + (i,j,k) * probe_spacing
volume_span[a] = (count[a] - 1) * probe_spacing        // ≥ span, covers the grid box
```

Sampling in the shader is `uv = (world_pos + normal*normal_bias - gi_origin) * gi_inv_span`, one
`textureSampleLevel` with the existing `linear_sampler` (verified `ClampToEdge` on all three axes, so a
point outside the volume reads the boundary probe rather than wrapping). `count[a] ≥ 2` on every axis
so trilinear is always defined.

**Probe budget cap — one new code, `L2105`** (next free; `L2104` is W0's last). Checked on
`count.x·y·z` *before* allocation, mirroring `L2103`'s shape exactly: a generous ceiling
(`MAX_GI_PROBES = 4_000_000`, ~64 MB at 16 bytes/probe), help text that walks whole doublings of
`probe_spacing` to name one that fits, an `explain` entry. It catches an authoring mistake, it is not a
streaming trigger.

## 7. The bake — `crates/ludion-renderer/src/gi_bake.rs` + `shaders/gi_bake.wgsl`

`ibl.rs`'s shape verbatim: a `Renderer` field built once in `new`, `resolve(&gpu, ...)` called from
`prepare_frame`, a single-entry `RefCell` cache, a fallback texture for the absent case.

```rust
pub(crate) struct ResolvedGi {
    pub probes: wgpu::TextureView,   // 3D, Rgba16Float, or the 1×1×1 fallback
    pub origin: [f32; 4],            // xyz = gi_origin, w = flag (0 or 1)
    pub scale:  [f32; 4],            // xyz = 1 / volume_span, w = normal_bias
}
```

Cache key: `(grid.hash, probe_spacing.to_bits())` — a cheap `u64` pair, never a re-walk of the grid.
`resolve(None)` clears the cache and returns the fallback with a zero flag.

**`gi_bake.wgsl`, one compute entry point**, one workgroup per 4×4×4 probe block:

- Bindings: the occupancy/emissive 3D texture `VoxelBake` already uploads (read the `.g` channel —
  emissive is W2's business), a small `GiBakeParams` uniform (grid origin, cell size, grid extent,
  probe spacing, probe counts, ray count), and the probe volume as a write-only
  `texture_storage_3d<rgba16float, write>`.
- **Ray set**: `GI_BAKE_RAYS = 64` directions from the **spherical Fibonacci / golden-angle** sequence,
  computed in-shader from the ray index. Chosen for the reason wave 7 chose Roberts' R2 over
  Hammersley and wave 9 chose a Vogel spiral: it is well-distributed at *any* prefix length, so
  changing the ray count later cannot quietly unbalance the set. Deterministic from the index alone —
  no RNG, no per-probe jitter (a jittered set would make the bake a function of nothing the world
  records, and W1 has no temporal filter to average it back out).
- **March**: an **Amanatides–Woo DDA** over the occupancy texture from the probe's cell, stepping cell
  by cell to the volume boundary. Escaped ⇒ `V = 1`; first occupied cell ⇒ `V = 0` and stop. A fixed
  step cap (`grid extent L1 sum`) bounds the loop.
- **Accumulate** over the 64 rays: `sh0 += V`, `sh1 += V * ω`. Write `vec4(sh0 / N, 3.0 * sh1 / N)`.
  The `3.0` is the L1 band's normalization, so reconstruction is a plain `dot`.
- **Probe-in-solid push-out** (leak control, half of it): if the probe's own cell is occupied, scan
  the 3³ neighbourhood then the 5³ for the nearest free cell in a **fixed deterministic order** and
  trace from that cell's centre instead. If none is free, write `vec4(0.0)` — a fully buried probe
  contributes darkness, which is the correct failure direction (a buried probe that reports sky is the
  leak this whole section exists to prevent).

**Determinism note.** The bake is GPU float accumulation over a fixed ray order in one invocation per
probe — no cross-invocation reduction, no atomics — so it is *deterministic per machine* and sits
exactly where `ibl.rs` sits: off the bit-exact CPU tier, gated by per-OS goldens. Do not claim
cross-platform bit-identity for it.

## 8. Consumption — `mesh.wgsl` and `EnvParams`

`EnvParams` grows 32 → 64 bytes, and the existing `params.w` **pad becomes `gi_strength`** (a field
that costs no layout growth is the right home for it):

```wgsl
struct EnvParams {
    params:    vec4<f32>, // x=ibl flag, y=ibl_intensity, z=specular max LOD, w=gi_strength
    ao:        vec4<f32>, // x=ao flag, yz=1/render-extent px, w=ssao_direct
    gi_origin: vec4<f32>, // xyz=volume origin, w=gi flag
    gi_scale:  vec4<f32>, // xyz=1/volume span, w=normal_bias
};
```

Group 3 gains **binding 6, `gi_tex: texture_3d<f32>`**, sampled with the existing `env_samp`. One
helper, per D4's seam:

```wgsl
fn gi_sky_visibility(world_pos: vec3<f32>, n: vec3<f32>) -> f32 {
    let p = world_pos + n * env.gi_scale.w;
    let uv = (p - env.gi_origin.xyz) * env.gi_scale.xyz;
    let sh = textureSampleLevel(gi_tex, env_samp, uv, 0.0);
    // cosine-convolved L1 reconstruction: A0 = 1, A1 = 2/3
    return clamp(sh.x + (2.0 / 3.0) * dot(sh.yzw, n), 0.0, 1.0);
}
```

Applied to the ambient term only — **both** the IBL branch and the `lights.ambient` fallback branch,
because a scene with no `Environment` still has an ambient constant that an interior should not
receive:

```wgsl
    if (env.gi_origin.w > 0.5) {
        acc = acc * mix(1.0, gi_sky_visibility(world_pos, n), env.params.w);
    }
```

Placed **after** the existing `env.ao.x` multiply, so AO and sky visibility compose as two independent
attenuations of the same term. **Direct lighting and emissive are untouched at every setting** — D5 is
indirect-only, and the `ssao_direct` finding is the precedent for why emissive must stay outside
(`fs_main` adds it after `shade`).

`render.rs`'s `env_bind_group` fills the two new vec4s from `ResolvedGi` and binds `gi_tex`. Regenerate
`shader_gen.rs` with `node scripts/check.mjs --fix`; never hand-edit it.

## 9. `gi-interior` — the calibration scene (D6)

`examples/gallery/gi-interior.yaml`, **built-in registry** so `render --sweep` can drive it:

- A closed room (four walls, a roof, a floor) with **one open doorway**.
- One interior torch (emissive), inert in W1 — it is what W2 lights the room with, and the scene must
  already carry it so W2 changes one number, not the scene.
- One dynamic element (a `PointLight` on a small mover) for W3′'s injection testbed.
- An `Environment` with a bright sky, so sky visibility has something to occlude, and a `GiVolume`
  with `cell_size` and `probe_spacing` picked off a sweep.
- Camera framing the doorway from inside, so the golden shows the three things that are GI and nothing
  else: no sky leak through the walls, a light wedge on the floor at the door, and the room's back
  corner dark.

First bless on Windows; queue the macOS bless in the private macOS queue. Sweep sheets that settle
`probe_spacing` and `normal_bias` went to `Plan/artifacts/gi-wave1-*.png` (deleted before
publication; in Lore history) with a `-sweeps.md` note.

## 10. The debug instrument — `--dump-gi`

W0's stated lesson is that a silent bake is undebuggable, and a probe volume is *more* likely to be
subtly wrong than a voxelizer. `ludion render --scene X --dump-gi out.png` renders the baked probe
volume as a stitched slice sheet in `voxel_sheet.rs`'s shape: one XZ tile per probe layer, two blocks
side by side — **VISIBILITY** (the DC term as greyscale) and **BENT NORMAL** (the L1 vector remapped to
RGB) — near-square packing, integer upscale, burned labels.

Unlike `--dump-voxels` this **requires a GPU adapter** (it reads the baked texture back), which is the
one honest difference from its sibling and must be stated in the help text. `L2104` (no `GiVolume`)
covers the missing-component case unchanged — no new code.

## 11. Tests

The wave-7/8 fixture style. Every assertion names the quantity it measures.

1. **Identity-value byte-identity (the arc gate).** `examples/gallery/meshes.yaml` rendered without a
   `GiVolume` and with one authored at `sky_visibility: 0.0` are **byte-exact**, with a non-vacuity
   guard asserting the volume actually baked (the W0 `voxel.is_some()` pattern, extended to the probe
   volume being non-fallback).
2. **The interior darkens.** On a closed-room fixture, the interior floor's mean luminance is
   materially lower with `sky_visibility: 1.0` than at `0.0`, while an exterior wall facing open sky
   holds within a small tolerance. Assert both halves — a bake that darkens *everything* passes the
   first half alone.
3. **The doorway wedge.** The floor region in front of the door is measurably brighter than the back
   corner of the same room, with the difference asserted as a ratio so it survives an exposure retune.
4. **No leak through a wall.** A probe-sized sample point sealed inside solid geometry receives
   visibility at or near zero; the sealed room's interior does not brighten when the *exterior* sky
   colour is raised. This is the Risk-1 gate and it must be able to fail — mutate the push-out to a
   no-op and confirm it goes red.
5. **Emissive and direct light untouched.** Whole-image byte equality of an emissive-only and a
   point-lit fixture across `sky_visibility` 0 → 1 (the `ssao_direct` fixture shape).
6. **Bake stability.** Two separate `render` processes on the same GI scene produce byte-identical
   images (per-machine determinism; explicitly *not* a cross-platform claim).
7. **Voxelizer cache (WP-1).** A second `extract` of an unchanged world does no rasterization
   (a counter, the `VoxelBake::uploads` precedent), and a moved instance invalidates it.
8. **`L2105`** fires on a too-fine `probe_spacing` before allocation, with `ludion explain L2105`
   working.

Plus full `node scripts/check.mjs` green, every existing golden byte-identical, no `run.lrp`
re-record, no `sim-hash` reference moved.

## 12. Work packages

Dependency-ordered. Files listed are the ownership boundary — no two packages in one wave share one.

- **WP-1 — the load-time voxelization + grid hash** (§4). `scene.rs` (`WorldVoxelGrid.hash`,
  `RenderScene.voxel: Option<Arc<..>>`), `voxelizer.rs` (signature + cache), `extract.rs` (the Arc),
  `voxel_bake.rs` (key on `grid.hash`), `asset_cache.rs`. Tests 7. **opus** — determinism and cache-key
  judgment. *Wave A.*
- **WP-2 — `GiVolume`'s three new fields** (§5). `components.rs` only. Schema, defaults, round-trip and
  rejection tests, freeze-list untouched. **sonnet** — fully specified against the existing field.
  *Wave A, parallel with WP-1.*
- **WP-3 — the GPU bake** (§6, §7). New `gi_bake.rs` + `shaders/gi_bake.wgsl`, `voxel_bake.rs` exposing
  its occupancy texture, `render.rs` (`prepare_frame` only), `codes.rs` (`L2105`), `shader_gen.rs`
  regenerated. Test 8. **opus** — the algorithmic heart. *Wave B.*
- **WP-4 — consumption** (§8). `mesh.wgsl`, `render.rs` (`env_bind_group` + `EnvParams` fill),
  `shader_gen.rs` regenerated. Test 1. **opus** — the pixel-moving edge and the layout growth.
  *Wave C.*
- **WP-5 — `gi-interior` + golden** (§9). `examples/gallery/gi-interior.yaml`, the blessed golden, the
  sweep artifacts. **sonnet**. *Wave D.*
- **WP-6 — fixtures** (§11 tests 2–6). `crates/ludion-engine/tests/`. **sonnet**, briefed with exact
  assertions. *Wave D, parallel with WP-5.*
- **WP-7 — `--dump-gi`** (§10). `voxel_sheet.rs`'s sibling, CLI flag, renderer readback. **sonnet**.
  *Wave D, parallel.*

Integration, full `check.mjs`, the golden byte-identity sweep, and the commits are the orchestrator's.

## 13. Definition of done

- `node scripts/check.mjs` green.
- Every existing golden byte-identical; `gi-interior` blessed as a new golden with the macOS bless
  queued.
- All eight §11 tests pass, and test 4 is demonstrated able to fail.
- `ludion explain L2105` works.
- `ludion render --scene examples/gallery/gi-interior.yaml --dump-gi out.png` shows the room's interior
  probes dark and the doorway probes bright.
- Bake time measured and recorded on the pinned GPU against the plan's ~200 ms district target
  (Risk 3); the voxelizer cache hit rate confirmed on a repeated extract.
- `Plan/global-illumination.md` §4's W1 paragraph marked delivered; this file's Status filled in;
  the graphics-engine notebook gains a W1 Done entry.

## 14. Micro-decisions deferred into implementation (record the answers in the commit)

- Whether the probe texture needs its own sampler or the existing `linear_sampler` serves (verified
  `ClampToEdge`; confirm the mip/filter settings suit a 3D fetch).
- The shipped `probe_spacing` and `normal_bias` for `gi-interior`, off the sweep sheets.
- Whether the L1 reconstruction wants the `2/3` cosine convolution or the raw `1.0` — measure on the
  interior fixture; the cosine form is the default because the term it multiplies is a cosine-weighted
  irradiance.
- Whether `--dump-gi`'s readback plumbing stays contained; if it grows past the brief, report rather
  than half-build it.
