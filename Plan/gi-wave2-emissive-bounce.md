# GI Wave 2 — emissive and static lights as bounce sources (detailed implementation plan)

> Status: **DELIVERED (2026-08-07, Lore revs 525–531).** Expands `Plan/global-illumination.md`
> §4's W2 outline against the code as W1 left it (`Plan/gi-wave1-sky-visibility.md`'s Status is
> the predecessor's outcome record, and its shape is this document's template). All four waves
> landed as specified with all six §0 rulings holding: A the inert field (rev 525), B the bake
> (rev 526 — one march, the free face normal, the premultiplied lights signature, magnitude gate
> 0.31%), C consumption (rev 527 — R6 resolved KEEP with hash-proven byte-identity both ways,
> `EnvParams` 80 B, bindings 7/8/9, Gates A+B), D the four parallel packages (revs 528–531:
> `gi-interior` at `bounce: 1.0` re-blessed as the wave's single golden move with SSIM 0.7675 and
> a committed diff report; the five physics fixtures with BOTH leak gates demonstrated able to
> fail — `sun_shadow` forced to 1 reddened the sealed box at ratio 21.4, a six-cell tunneling
> mutation reddened the shared-wall gate at 1.330; the `--dump-gi` BOUNCE block with fixed-exposure
> Reinhard and the "not baked" label; the docs, with `dev-docs/renderer-gi.md` split at the
> machinery/bounce seam under the size budget). The private graphics-engine notebook's Done entry
> carries the outcome record and the findings; the commit messages carry the detail. **Recorded
> finding, open for the sitting:** the bounce term reads visibly mottled on flat surfaces in the
> calibrator at 64 rays/probe — estimate variance, not a leak; ray count is the recorded
> quality-tier axis.
>
> One line, so the sections below can be read against it: **the same probe lattice, the same
> single compute dispatch, the same one march per ray — the march now reports *what it hit*, and
> the hit cell's outgoing radiance (albedo × (emissive + direct light)) is projected into three
> more SH-L1 coefficient sets that `mesh.wgsl` ADDS to the ambient term.** The scalar visibility
> volume's four floats are frozen; W2 grows the storage, never reinterprets it.

---

## 0. Rulings needed before the wave starts

> **All six ratified as recommended (orchestrator ruling, 2026-08-07), the wave built against
> them.** R1 (`bounce` default `0.0`) is the one with a visible authoring-surface consequence —
> two defaults with opposite senses inside one component — and is deliberately cheap to reverse
> later (one line + the calibrator re-bless); it is flagged for the user's next sitting rather
> than blocked on it.

Six questions this document takes a position on but cannot settle from the written record. Each is
marked **needs user/orchestrator ruling**; the recommendation is what the rest of the document is
written against, so a different ruling means an edit, not a redesign.

- **R1 — `bounce` defaults to `0.0`, not `1.0`.** Recommended, and it contradicts `sky_visibility`'s
  own `1.0` default inside the same component, which is why it needs a ruling. The argument for
  `0.0`: `GiVolume` is already authored in a committed scene and in every W1 fixture, so a `1.0`
  default silently turns bounce on in all of them, moves `gi-interior`'s golden with **no scene
  diff**, and could legitimately move W1's asserted ratios (`the_interior_darkens…` measures a
  sealed room that a bounce term would brighten back). `0.0` is also every `PostData` effect gate's
  default (`ssao`, `shafts`, `dof`, `halation`), makes the bake's skip the default path, and makes
  `gi-interior`'s re-bless provably caused by a one-line authored edit. The argument for `1.0`:
  bounce is physics, not an effect, and `sky_visibility: 1.0` is the in-component precedent.
  **Recommend `0.0`.**
- **R2 — is `gi-interior`'s re-bless the arc's pre-authorized pixel-moving budget?** W1 declined to
  spend it and `Plan/global-illumination.md` §4 records the budget carrying to W4's content
  adoption. W2 moves exactly one golden — the arc's own calibrator, which W1 created for this
  purpose. **Recommend: this is not a content re-bless and the W4 budget is untouched**; the
  calibrator exists to record the arc's product and moving it is what it is for.
- **R3 — which lights count as "static"?** The engine has no static/dynamic marker (D2 chose option
  (a), "the scene as loaded", and deferred `GiStatic`). W2 therefore bakes **every** light on
  `RenderScene.lights` plus `RenderScene.sun`, and folds an `O(lights)` signature into the bake's
  cache key — symmetric with the voxelizer's `O(instances)` signature, inventing no new concept. Two
  consequences must be accepted with it: a moving light re-bakes every frame exactly as a moving
  mesh re-rasterizes every frame, and **W3′'s dynamic injection would double-count** unless the
  split becomes explicit there. **Recommend: accept both, and record that `GiStatic` (D2b) is
  W3′/W4's to fire, not W2's.**
- **R4 — should the sky's own bounce enter the volume?** A courtyard's sunlit wall bouncing sky
  light onto the ground is a real, large outdoor effect, and this wave does not carry it (§4.6).
  Doing so would put the sky gradient back inside the bake, which W1 deliberately kept out.
  **Recommend: out of scope for W2**; vigil is a night city and the missing term is invisible
  there. Recorded as deferred with a trigger, not forgotten.
- **R5 — does `gi-interior` gain a `DirectionalLight` so the shipped golden exercises the sun
  bounce?** Its header comment says the absent sun is deliberate ("the image IS the `GiVolume`'s
  product rather than a shadow map's"). **Recommend: no** — the sun-bounce path is gated by a
  fixture (§8 T7), and the calibrator keeps its single-variable framing.
- **R6 — may `gi_sky_visibility`'s body be refactored to share an `sh_l1_eval` helper with
  `gi_bounce`?** Cleaner, and "before adding a second way to do something, remove the first" wants
  it — but it rewrites a shipped shader function whose output every W1 golden depends on, and the
  volumetric-medium W3 finding is that byte-identity is a property of generated code, not of values.
  **Recommend: attempt the shared helper, keep it only if the §8 T1 identity gate stays
  byte-identical, and inline the three lines otherwise.** Decided by measurement inside WP-3, with
  the answer recorded in the commit.

---

## 1. Goal and the product

W1 made the sky stop reaching where it cannot. W2 makes light **arrive from surfaces**: the
scene's own emissive cells and its static point lights and sun are gathered along the probe rays,
one bounce, and land in the volume as RGB irradiance the mesh shader adds to the ambient term.

The product, stated so it can be judged by eye: **`gi-interior`'s torch, authored in W1 and inert
ever since, lights the room.** Its own post warms, the wall behind it glows, the cobbles under it
pick up an orange pool, and the room's back corner — which the torch cannot see — stays dark. The
scene changes by exactly one line (`bounce: 1.0`); everything else was already there.

This is also the wave that answers `Plan/global-illumination.md` §1 fact 2 (*"emissive lights
nothing"* — sixteen of vigil's twenty-one voxel models author `emissive` and today it is a bloom
feed) and §1 fact 3's residue (**all** static lights contribute to GI, with no per-frame light
budget of any kind in the way — clustered forward already removed the cap for direct light; this
removes it for indirect).

**What it is not.** Still a load-time, single-shot, per-scene bake with **no history and no
accumulation across frames** — Pillar 3 and `Plan/global-illumination.md` §7/§8 are untouched, and
§9's dynamic injection grid remains W3′'s. Still indirect-only (D5): direct lighting is entirely
clustered forward's job and the volume adds only what bounced.

---

## 2. The neutrality contract, and this wave's golden policy

**Golden policy, stated plainly: exactly one golden moves — `examples/gallery/goldens/windows/gi-interior.png`
— and its move IS the wave's product.** Every other committed golden stays byte-identical. The
macOS bless for `gi-interior` re-queues in the private macOS queue, and an SSIM diff report is
committed beside the moved golden as `Plan/artifacts/gi-wave2-gi-interior.diff.json` (the medium
arc's discipline, unchanged).

Three gates carry that claim, and each is **measured, never assumed**:

- **Absent-component neutrality, by construction.** A scene with no `GiVolume` binds the 1×1×1
  fallbacks, carries a zero flag, and takes the historical path — proven by the whole golden suite
  staying byte-identical. Unchanged from W1.
- **Gate A — skip neutrality.** A scene authoring `GiVolume` with `bounce: 0.0` (which under R1 is
  the default, so *every* existing authored `GiVolume` qualifies) allocates no bounce volumes,
  binds the fallbacks, takes no new shader branch, and renders byte-identical to the same scene
  before the field existed. This is what keeps every W1 fixture green **by construction**.
- **Gate B — the additive identity, and the gate that matters.** A scene whose grid contains **no
  emissive cell, no point light and no sun** bakes an all-zero bounce volume. At `bounce: 1.0` the
  shader binds it, samples it, takes the branch, and adds exactly `vec3(0.0)` — and the image is
  byte-identical to the same scene at `bounce: 0.0`. Gate A proves the skip; **Gate B proves the
  arithmetic**, because it exercises the three bindings, the three fetches, the reconstruction, and
  the add.

**The bit-exactness reasoning behind Gate B, verified rather than asserted.** The multiplicative
precedent (`mix(1.0, v, 0)` → an exact `1.0` multiply) does not transfer; for an additive term the
identity is adding exactly `+0.0`. In IEEE-754 round-to-nearest, `x + (+0.0) == x` bit-for-bit for
every `x` **except `x = −0.0`**, where the sum is `+0.0`. Three facts close that hole:

1. `acc` at the add site cannot be `−0.0`. Both branches that produce it are products and sums of
   non-negative quantities (`irradiance × diffuse`, `prefiltered × (f0·a + b)`, `ambient × albedo`),
   and `(+0) × (+finite)` and `(+0) + (+0)` are both `+0.0`.
2. The bounce sample is `+0.0` because the bake **writes** every probe — the volumes are never
   left undefined, so there is no NaN or `−0.0` to propagate, and `max(vec3(0.0), sh)` floors the
   reconstruction at `+0.0` regardless.
3. Even if (1) failed, the value is stored to an `Rgba16Float` target and passes tonemap +
   8-bit encode, where `−0.0` and `+0.0` produce the same byte.

Two further requirements, both from the volumetric-medium W3 finding:

- The new branch must not perturb the untaken path's codegen. Gate A is exactly the measurement
  that catches it. If it fires, the escape is to nest the bounce inside the *existing*
  `env.gi_origin.w > 0.5` block with no second condition and rely on the `+0.0` argument above
  (§5 gives the shape).
- **W1's visibility output must be bit-unchanged by W2's refactor of the march.** The DDA now
  reports hit information the visibility term also derives from, so the two share one walk. The
  guards already exist and are strong: `gi_bake.rs`'s
  `a_probe_on_the_grids_max_face_sees_the_sky_behind_it` asserts an *exact* `33/64`, and
  `an_empty_grid_bakes_full_visibility_and_a_balanced_ray_set` asserts an exact `1.0`. Neither may
  move. If one does, the refactor is wrong — it is not a re-bless.

---

## 3. What the volume stores, and why

**Three additional `Rgba16Float` 3D textures on the existing lattice, one per colour channel**, each
holding that channel's SH-L1 exactly as W1's texture holds the scalar's:

```
gi_bounce_r  =  (DC_r,  L1x_r,  L1y_r,  L1z_r)
gi_bounce_g  =  (DC_g,  L1x_g,  L1y_g,  L1z_g)
gi_bounce_b  =  (DC_b,  L1x_b,  L1y_b,  L1z_b)
```

Twelve floats — 3 colour channels × 4 SH-L1 coefficients — packed into twelve texels' worth of
lanes with **zero waste**, and each texture is structurally identical to the one W1 already ships.
`W2 grows this by ADDING coefficient sets, never by reinterpreting these four floats` is satisfied
literally: the visibility texture is not read, not written, and not re-meant.

Per-probe cost goes 8 B → **32 B**. Consequences to handle:

- `MAX_GI_PROBES` (4 M) stays — it is a guardrail against an authoring typo, not a memory policy —
  but **its doc comment in `voxelizer.rs` is wrong today and must be corrected**: it claims "16
  bytes per probe … ~64 MB", where `Rgba16Float` is 8 B/probe (~32 MB at the cap). After W2 the
  true figure is 32 B/probe, ~128 MB at the cap. A doc-only fix, but the number is load-bearing for
  anyone sizing a district.
- Group 3 gains bindings **7, 8, 9**. That puts eight sampled textures in group 3 (irradiance cube,
  specular cube, BRDF LUT, AO, visibility, and the three bounce volumes) and nine in the mesh
  fragment stage counting `shadow_tex` — comfortably under WebGPU's guaranteed
  `max_sampled_textures_per_shader_stage` of 16, but the headroom is now **seven**, and a future
  fourth coefficient set (an ambient cube, a second bounce term) has to be counted rather than
  assumed. Record the number in the commit.

**Rejected, so the choice is not re-litigated:**

- **One texture with the coefficient sets packed along an axis** (`z = k*4 + c`, or `x` widened
  4×). Kills hardware trilinear filtering — the filter would blend across coefficient sets — and
  hardware trilinear between probes is not optional (W1 §3: probes are metres apart). Manual
  trilinear in the shader is eight fetches per volume and is not on the table.
- **DC-only RGB bounce** (one `Rgba16Float`, 8 B/probe, no directionality). A third of the memory
  and a third of the fetches, and §9.2 leans this way for the *dynamic* term. Rejected here for
  D4's own reason at the static term: a torch beside a wall must light the wall's facing side and
  not its far side, and a DC-only bounce glows both. It is a strict subset of what is built
  (drop two textures), so it stays available as a measured fallback if probe memory or fetch cost
  bites at district scale.
- **Two textures — full RGB DC plus a single luminance-weighted L1 direction** (16 B/probe,
  directionality preserved, colour-shift lost). A real saving, but it is a *reinterpretation* with
  its own reconstruction rule rather than an added coefficient set, and the notebook decision the
  arc is operating under says the opposite. Trigger for revisiting: probe memory becomes the
  binding constraint at streamed-world scale (`Plan/global-illumination.md` §10).

**The storage seam D4 requires**, mirroring W1's: `mesh.wgsl` reads the bounce through exactly one
function, `gi_bounce(world_pos, normal) -> vec3<f32>`, beside `gi_sky_visibility`. An ambient-cube
experiment later replaces those two functions and the bake's writes, and nothing else.

**The units, and why they compose exactly.** `ibl.wgsl`'s irradiance convolution stores *"the
cosine-weighted mean incoming radiance … (no 1/PI), so the mesh shader's `irradiance * albedo`
matches the point-light convention"*. The bounce volume must hold the same quantity, and the same
SH-L1 machinery delivers it for free. With a uniform direction set of `N` rays,

```
sh0 = (1/N) Σ L_i                    // = the DC, (1/4π)∫L dω
sh1 = 3 · (1/N) Σ L_i ω_i            // = the L1 vector, pre-scaled by the band's normalization
M(n) = sh0 + (2/3) · dot(sh1, n)     // = (1/π)∫ L(ω) max(cos θ, 0) dω
```

`M(n)` **is** the cosine-weighted mean incoming radiance, identical in kind and in normalization to
`irradiance_tex`. So the consumer's `bounce × diffuse` is unit-for-unit the same expression as the
IBL branch's `irradiance × diffuse`, and a fully-enclosing emitter of radiance `L` reconstructs to
exactly `L` — which §8 T5 asserts as a fixture rather than leaving it to arithmetic on paper. The
`3.0` scale and the `2/3` cosine convolution are W1's, unchanged.

---

## 4. The bake — what a ray brings back

`gi_bake.wgsl` keeps one compute entry point, one workgroup per 4×4×4 probe block, one march per
ray. The march changes from "did I hit?" to "**what** did I hit", and both terms fall out of the
same walk.

### 4.1 The march reports a hit, not a bit

`trace` becomes `trace_hit(start, dir, extent) -> Hit { hit: bool, cell: vec3<i32>, normal: vec3<f32> }`.
Visibility is `select(1.0, 0.0, hit.hit)` — the same value, from the same walk, and the exact-value
tests in §2 are what prove it. Every existing property is kept verbatim: Amanatides–Woo stepping,
the `nonzero` axis guard, the `exit <= max(enter, 0.0)` tangent escape (a tangent is an escape, not
a hit — the dark-rim fix), the L1-diameter step cap, and the boundary clamp.

**The hit normal is free and exact.** The DDA already branches on which axis it stepped; the face
the ray crossed to enter the occupied cell is exactly that voxel face, so `normal = -step_axis`
pointing back along the ray. Two edge cases, both stated rather than left emergent: a ray that
starts already inside the entry cell (the probe's own cell after push-out cannot be occupied, so
this is the boundary-clamp case) takes `normal = -dir`, normalized; a degenerate `normal` is never
produced because `step` is `±1` on exactly one axis per iteration.

### 4.2 The hit cell's outgoing radiance

```
L_out(cell, n) = albedo(cell).rgb * ( emissive(cell) + direct(cell, n) )
```

- **`albedo`** comes from `voxel_bake.rs`'s `Rgba32Float` albedo texture — which today has **no
  reader at all** and carries an `#[allow(dead_code)]` that W2 deletes. `VoxelBake::resolve` starts
  returning both textures.
- **`emissive`** is the `.r` channel of the `Rg32Float` texture the march already loads for
  occupancy (`.g`), so it costs one swizzle, not one fetch.
- **`albedo × emissive`, not `emissive` alone.** This is the same product `fs_main` forms
  (`lit + albedo * max(material.z, 0.0)`), and it is what makes the bounce *warm*: `gi-interior`'s
  torch is `color [1.0, 0.62, 0.28]` at `emissive: 4.0`, so its radiance is `[4.0, 2.48, 1.12]`. A
  scalar-emissive bounce would be white, and the wave's whole product is an orange pool.
  Rev 516 already made the voxelizer's emissive mirror the render path's `authored_material`
  selection, which is the prerequisite that makes this true for voxel models as well as primitives.
- **Escaped rays contribute exactly `vec3(0.0)` to the bounce.** The sky is W1's business: it
  arrives through the prefiltered IBL cube, modulated by the visibility volume. Adding sky radiance
  here would double-count it, forfeit Gate B, and re-import the decision W1 explicitly rejected.

### 4.3 Direct light at the hit cell

Evaluated **inline at the hit**, with the free face normal, mirroring `mesh.wgsl`'s own direct
terms so the bounce is consistent with the light the player sees:

```
direct(p, n) =  Σ_lights  color_l · max(dot(n, l), 0) · atten² ,   atten = clamp(1 - d/range, 0, 1)
             +  sun.color · max(dot(n, l_sun), 0) · sun_shadow(p, n)
```

- The premultiplied convention is `mesh.wgsl`'s verbatim (`radiance IS the diffuse response at
  ndl = 1, ranged atten²`). **One helper forms the premultiplied `Light`, and both the mesh pass
  and the bake call it** — the convention is never re-derived, which is the `cluster.rs` decision-8
  rule applied one crate-internal seam further.
- The light set is `RenderScene.lights` **whole**, in its pinned ascending-`EntityId` order — which
  is also the float summation order, so the sum is deterministic per machine exactly as the
  fragment loop's is.
- **No shadow rays for point lights.** §9.7 already rejects shadowed dynamic lights in the GI term
  (*"the grid's own occupancy provides the occlusion, coarsely, and that is the right amount for an
  indirect term"*), and the same holds for static ones at one bounce. The cosine test does most of
  the work for free: for the common failure case — a torch on the far side of a wall — the wall's
  near-side face normal points *away* from the torch, so `dot(n, l) ≤ 0` and the contribution is
  exactly zero. What survives is light leaking around a **corner** rather than through a wall,
  which is acceptable for an indirect term and is recorded as a watch item (§10) with the
  escalation (one shadow march per light per hit, behind a flag) deferred.
- **The sun is the one source that must be occluded.** It has no range, so without occlusion every
  cell in a sealed interior bounces sunlight and the room floods. `sun_shadow` is therefore **one
  additional DDA** from the hit cell — offset one cell along `n` to avoid self-hit — toward
  `-sun.direction`: escaped ⇒ 1.0, occupied ⇒ 0.0. It runs only when the scene has a sun and only
  when `dot(n, l_sun) > 0`, so a sunless scene (`gi-interior`) pays nothing.
  Note the deliberate divergence: this shadow is the grid's, not the render pass's shadow map, and
  the two will disagree at the margins. The shadow map is camera-fitted and not available to a
  cached load-time bake; for an indirect term the grid's answer is the right one.

### 4.4 One bounce

`L_out` is evaluated at the hit and the ray stops. A second bounce would re-march from the hit and
gather `L_out` again; per the plan that is "a constant, not a redesign", and the shape written here
keeps it so — the gather is a function of `(start, dir)` that could be called recursively-by-loop.
**W2 does not ship a bounce-count constant, an authored field, or a loop** (a knob the quality-tier
system will want to own is a knob authored in the wrong place — the `GI_BAKE_RAYS` precedent). It
ships one bounce and records the shape.

### 4.5 Accumulation and write

```
sh0 = vec3(0);  sh1r = vec3(0);  sh1g = vec3(0);  sh1b = vec3(0);
for i in 0..RAYS:
    dir = ray_direction(i)                      // unchanged: 64 spherical Fibonacci
    h   = trace_hit(start, dir, extent)
    vis += select(1.0, 0.0, h.hit);  visL1 += vis_i * dir      // W1's terms, bit-unchanged
    if bounce_enabled && h.hit:
        L = radiance(h)
        sh0 += L;  sh1r += L.r * dir;  sh1g += L.g * dir;  sh1b += L.b * dir
```

written as `vec4(sh0.r/N, 3·sh1r/N)` and its two siblings. A buried probe (push-out exhausted)
writes `vec4(0.0)` to all four volumes — darkness is the correct failure direction for bounce for
the same reason it is for visibility.

**`bounce_enabled` is a uniform flag in `GiBakeParams`**, false when the authored strength is
exactly `0.0`. False ⇒ the three volumes are never allocated, the radiance work never runs, and the
bake's cost and its visibility output are W1's exactly. That is Gate A's construction, and it also
means a scene wanting sky occlusion without bounce pays nothing for the feature.

### 4.6 What the bake deliberately does not carry

Sky light bouncing off surfaces (a sunlit wall lighting the ground beneath it). It would need the
sky gradient inside the bake — which W1 kept out on purpose — and the visibility term at the hit
cell, which the same dispatch is in the middle of writing. Out of scope per R4; the gap is real
outdoors and invisible in a night city. Trigger: the first daylit exterior scene where the missing
term is called by eye.

### 4.7 The cache key

`(grid.hash, probe_spacing.to_bits(), lights_signature, bounce_enabled)`, where `lights_signature`
is an XXH3 over, in order: a discriminant plus `direction`/`color` for the sun, then each
`PointLightData`'s `position`/`color`/`intensity`/`range` in the pinned list order. `O(lights)`
per frame — the voxelizer's `O(instances)` signature precedent, and a few hundred lights is free
beside the frame the renderer is already building. `normal_bias` stays out of the key (sample-time),
and the *value* of a non-zero `bounce` stays out of it too (also sample-time) — only the
zero/non-zero discriminant participates, because that is what changes what is written.

The documented consequence, symmetric with the mesh side and accepted under R3: **a light a system
moves invalidates the bake every frame**, exactly as a mesh a system moves invalidates the
voxelization every frame. `GiStatic` (D2b) is the recorded answer for both.

### 4.8 Determinism and cost

Unchanged in kind: one invocation per probe, no atomics, no cross-invocation reduction, a ray set
that is a pure function of the ray index. **Deterministic per machine**, sitting where `ibl.rs`
sits — gated by per-OS goldens, never claimed as cross-platform bit-identity.

The cost bound, stated so W3 has a number to argue from:
`probes × RAYS × (march_steps + lights + sun_march_steps)`. For `gi-interior` (~26 k probes at
`probe_spacing 0.5`, 64 rays, one point light, no sun) that is within the same order as W1's own
bake and must be **measured and recorded** (§9). At district scale — vigil at W4, ~480 k probes and
~112 lights — the `probes × rays × lights` term is billions of evaluations and **will not fit the
plan's ~200 ms budget**. That is W3's budget problem, not W2's, and two escalations are recorded
now so W3 decides on measurements rather than from scratch:

1. **A per-cell direct-radiance pre-pass**: one dispatch over *occupied* cells only, each looping
   the lights once, writing an RGB outgoing-radiance grid the march then reads with a single
   fetch — making the march's per-hit cost `O(1)` in light count. Its open problem is the per-cell
   normal (the grid stores none; the occupancy gradient is degenerate for a one-cell-thick wall,
   which is vigil's most common wall), and its resolution is what W3 must design. This is also
   §9.3 stage 2's shape, so W3′'s dynamic injection would replace its contents, not its structure.
2. **The binned light grid** the many-lights arc owns (§9.5). This plan does not build one, and
   must not: building a second binning structure is precisely what §9.5 forbids.

---

## 5. Consumption — `mesh.wgsl` and `EnvParams`

`EnvParams` grows **64 → 80 bytes**. There is no spare lane (`params.w` is `gi_strength`,
`gi_origin.w` the flag, `gi_scale.w` the normal bias), so one `vec4` is added:

```wgsl
struct EnvParams {
    params:     vec4<f32>, // x=ibl flag, y=ibl_intensity, z=specular max LOD, w=sky-visibility strength
    ao:         vec4<f32>, // x=ao flag, yz=1/render-extent px, w=ssao_direct
    gi_origin:  vec4<f32>, // xyz=volume origin, w=gi flag
    gi_scale:   vec4<f32>, // xyz=1/volume span, w=normal_bias
    gi_bounce:  vec4<f32>, // x=bounce strength, yzw=reserved (zero)
};
```

Group 3 gains bindings 7/8/9 (`gi_bounce_r`/`_g`/`_b`, `texture_3d<f32>`), sampled with the existing
`env_samp` for W1's reason: `ClampToEdge` on all three axes with linear min/mag is exactly what a
single-level 3D fetch wants.

```wgsl
fn gi_bounce(world_pos: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    let p  = world_pos + n * env.gi_scale.w;               // the same normal_bias leak control
    let uv = (p - env.gi_origin.xyz) * env.gi_scale.xyz;   // the same sample coordinate
    let r  = textureSampleLevel(gi_bounce_r, env_samp, uv, 0.0);
    let g  = textureSampleLevel(gi_bounce_g, env_samp, uv, 0.0);
    let b  = textureSampleLevel(gi_bounce_b, env_samp, uv, 0.0);
    let e  = vec3<f32>(sh_l1(r, n), sh_l1(g, n), sh_l1(b, n));
    return max(e, vec3<f32>(0.0));   // HDR radiance: floored at zero, NOT clamped to 1
}
```

`sh_l1(sh, n) = sh.x + (2.0/3.0) * dot(sh.yzw, n)` — the shared helper R6 rules on. The floor is
`max`, not W1's `clamp(…, 0, 1)`: this is HDR radiance and an SH-L1 field can ring negative in a
strongly directional bounce, but it has no upper bound to clamp to.

The application site, inside the branch W1 already has:

```wgsl
    if (env.gi_origin.w > 0.5) {
        acc = acc * mix(1.0, gi_sky_visibility(world_pos, n), env.params.w);   // unchanged
        if (env.gi_bounce.x > 0.0) {
            acc = acc + gi_bounce(world_pos, n) * diffuse * env.gi_bounce.x;
        }
    }
```

Four decisions in those three lines:

- **× `diffuse`, not raw and not `albedo`.** `gi_bounce` returns *incident* irradiance; the outgoing
  diffuse radiance is `E(n) × (1 - metallic) × albedo`, which is exactly `diffuse` — the same
  factor the IBL branch's `irradiance * diffuse` uses. Using `albedo` instead would light metals
  diffusely.
- **Added after AO and after the sky-visibility multiply, attenuated by neither.** Sky visibility is
  definitionally the *sky's* occlusion and bounce is not sky. AO is left off it because (a) any
  reordering of the existing expression moves every AO golden, and (b) darkening indirect bounce in
  a torch-lit interior removes the only light there is — the `ssao_direct` failure mode. Recorded
  as a deferred micro-decision with a trigger, not as a closed question.
- **Diffuse only.** Indirect specular is not in an SH-L1 volume's power; the prefiltered specular
  cube keeps that job. Stated so nobody looks for a missing highlight.
- **Nested inside the existing GI branch**, so the new condition is unreachable on a non-GI frame
  and cannot cost one. Gate A measures whether that is enough (§2); the escape if it is not is to
  drop the inner condition and rely on the `+0.0` identity.

`render.rs`'s `env_bind_group` fills `gi_bounce` and binds the three views (the fallback 1×1×1 on a
frame with no bounce). `shader_gen.rs` is regenerated with `node scripts/check.mjs --fix`, never
hand-edited. `fs_main` is not touched: emissive is added there, outside `shade`, and stays outside.

---

## 6. `GiVolume`'s W2 field

One field, **no new component L-code** — a bad value is the ordinary `L1014` at the field pointer,
which is the discipline that has now held for twelve waves.

| Field | Type | Default | Bound | Meaning |
|---|---|---|---|---|
| `bounce` | `f32` | `0.0` (R1) | `range(min = 0.0)` + `x-ui-range [0.0, 2.0]` | How strongly baked one-bounce irradiance is **added** to the ambient term. `0` skips the whole bounce bake and the shader branch; `1` is the physical value; above `1` is deliberate exaggeration. |

The bound is `minimum` with a wider-than-legal UI travel rather than a hard `[0, 1]`, following
`Environment.ibl_intensity` exactly (`range(min = 0.0)` + `x-ui-range [0.0, 4.0]`): it is a gain on
a physical quantity, and the engine does not know how much bounce an art direction wants. `x-ui-range`
lands on the field, which the `halation` test already pins as the placement contract.

`GiData` gains a matching `bounce: f32`, copied verbatim by `extract`'s `gi_data`. No sizing
arithmetic, no diagnostic, no freeze-list change: **no new component is registered**, so the
registry-order tripwire test is untouched and every component index is stable.

**`WorldHash` consequence — verified, not assumed.** Adding a field changes the serialized value of
an authored `GiVolume`, hence `WorldHash` for any scene that authors one. Exactly one committed
scene does: `examples/gallery/gi-interior.yaml`. A repo-wide search for it finds only
`docs/global-illumination.md`, `dev-docs/renderer-gi.md`, `dev-docs/engine-voxelizer.md`,
`crates/ludion-cli/embedded-docs/global-illumination.md`, `crates/ludion-cli/tests/render_gi.rs`,
`crates/ludion-engine/tests/gi_sky_visibility.rs` and the scene itself — **no `.lrp`, no `sim-hash`
reference, no committed snapshot**. So: **no replay re-records and no `sim-hash` reference moves.**
Re-verify before committing rather than trusting this paragraph.

Two gates *will* fire and are part of the wave, not surprises: **schema-drift** (the JSON Schema
grows a property — `check.mjs --fix` regenerates, and CI regenerates the TS protocol types) and the
**embedded-docs mirror** (editing `docs/global-illumination.md` fails `cargo build -p ludion-cli`
until `--fix` re-mirrors it).

---

## 7. `gi-interior` — the calibration scene, changed by one line

The scene was authored in W1 *for this wave*: its header comment already says the torch "is
emissive and, in W1, inert … authored now so W2 changes one number rather than the scene."

The change is `bounce: 1.0` in the `GiVolume` block, plus a comment edit recording that the torch is
now live. Nothing else — no new entity, no moved wall, no camera change. The shipped strength comes
off a **sweep sheet**, not off taste: `render --sweep` over `bounce` at 0 / 0.5 / 1 / 1.5 / 2,
committed to `Plan/artifacts/gi-wave2-bounce.png` (deleted before publication; in Lore history)
with a `gi-wave2-sweeps.md` note stating the
criterion (the torch pool must read as light rather than as a tint, and the room's back corner must
stay materially darker than the wall behind the torch).

Watch item for the sweep: `gi-interior` authors `PostProcess { lift: 0.002 }` because a fully
occluded surface received exactly zero ambient and 10% of the frame sat on the black rail. Bounce
lifts those regions off the rail on its own, so **check whether `lift` is still doing anything**
and record the answer; if it is now inert, removing it is a second one-line diff worth taking (it
would also remove a component from the scene, which is a `WorldHash` move — take it or leave it
deliberately, don't drift).

The golden re-blesses on Windows with an SSIM diff report; the macOS bless queues in
the private macOS queue.

---

## 8. The instrument — `--dump-gi` grows a BOUNCE block

W0's stated lesson stands doubled here: a probe volume is more likely to be subtly wrong than a
voxelizer, and a *radiance* volume more than a visibility one. The sheet gains **one** new block
beside VISIBILITY and BENT NORMAL:

- **BOUNCE** — the DC term (`sh0` per channel) as RGB, at a **fixed** exposure so two sheets from
  two scenes are comparable, with the exposure and the volume's actual maximum burned into the
  label. Absent (and labelled absent) when the scene authored `bounce: 0.0` and no bounce volume
  was baked — the sheet must say "not baked", never draw a black block that reads as "baked and
  dark".

`GiBake::resolve_and_read` stops returning `Vec<[f32; 4]>` and returns a struct carrying
`visibility: Vec<[f32; 4]>` and `bounce: Option<Vec<[f32; 3]>>` (the DC terms; the L1 bands are not
read back — see §11). `L2104` (no `GiVolume`) covers the missing-component case unchanged, and the
GPU-adapter requirement in the help text is unchanged.

A BOUNCE DIRECTION block is deferred (§11): three more blocks would triple the sheet's width for a
quantity the DC block's correctness already implies.

---

## 9. Tests

The wave-7/8 fixture style; every assertion names the quantity it measures. New physics fixtures
live in a new file, `crates/ludion-engine/tests/gi_bounce.rs`, built on `gi_sky_visibility.rs`'s
machinery (the block-YAML writer, the unique-per-call scratch dirs, `mean_luma`, the graceful GPU
skip) — reimplemented there, because integration test binaries share no module.

**A calibration note that matters:** W1's fixtures isolate GI with `ibl_intensity: 0.0`, which
severs the ambient term at its coefficient. **That lever does not work for bounce** — bounce is
added, not multiplied, and is not scaled by `ibl_intensity`. W2's fixtures isolate instead by
`bounce: 0.0` (Gate A) or by an emitter-free grid (Gate B).

1. **Gate A — skip neutrality.** `examples/gallery/meshes.yaml` with no `GiVolume`, and with one
   authored at `sky_visibility: 0.0, bounce: 0.0`, are **byte-exact**. Non-vacuity: the probe
   lattice is real, not the fallback (W1's `gi_lattice` guard). Lives beside W1's identity test in
   `gpu_render.rs`.
2. **Gate B — the additive identity.** A fixture with geometry, an `Environment`, **no emissive
   cell, no `PointLight`, no `DirectionalLight`**, rendered at `bounce: 0.0` and `bounce: 1.0` — byte-exact,
   with a non-vacuity guard asserting the bounce volumes **were** baked (a `bake_count`/
   `cached_probes` observation, or the `--dump-gi` readback reporting a present-but-all-zero
   bounce volume). This is the gate that exercises the bindings, the fetches and the add.
3. **The torch lights the room.** A closed box, one emissive cube against one wall, no other light.
   A floor patch beside the emitter reads **materially brighter** at `bounce: 1.0` than at `0.0`
   (assert a ratio, not an absolute, so it survives an exposure retune), **and** a patch behind an
   interior partition, out of the emitter's line of sight, holds within a small tolerance. Assert
   both halves — a bake that brightened *everything* passes the first alone.
4. **Albedo tints the bounce.** The same box with a **saturated red wall** facing a white floor,
   under a **white** emitter. The floor's bounce is red-shifted: assert the rendered `R/G` channel
   ratio rises measurably between `bounce: 0.0` and `bounce: 1.0`. This is the gate that fails if
   the bake drops `albedo` from `L_out` — the single most likely silent defect in §4.2.
5. **Magnitude, the unit gate.** A probe fully enclosed by an emissive shell of known radiance `L`
   with albedo `1.0` reconstructs `gi_bounce ≈ L` within a stated tolerance. A renderer-side unit
   test in `gi_bake.rs` against a synthetic grid (the `slab_grid` style), reading the volume back —
   this is what pins §3's units claim to arithmetic instead of prose.
6. **Distance falloff.** Two floor patches at `d` and `2d` from a point light along the same wall:
   the nearer is brighter, asserted as a ratio band consistent with `atten²`. Catches an attenuation
   that silently became linear, constant, or inverse-square.
7. **The sun bounces and is occluded.** A fixture with a `DirectionalLight`, an open lit floor
   beside a wall, and a sealed box elsewhere. The wall picks up a measurable warm bounce from the
   floor; the sealed box's interior does not brighten at all between `bounce: 0.0` and `1.0`. This
   is the sun-shadow-ray gate, and it must be **demonstrated able to fail** (mutate `sun_shadow` to
   return `1.0` unconditionally and confirm the sealed half reddens) — W1's leak-gate discipline,
   applied to the source that would otherwise flood every interior.
8. **No bounce leak through a wall.** Two adjacent sealed rooms sharing one wall, a bright emitter
   in room A: room B's interior does not brighten between `bounce: 0.0` and `1.0`. The W2 twin of
   W1's Risk-1 gate, and the one that would catch the push-out finding turning a visibility leak
   into a radiance leak (§10).
9. **Direct and emissive untouched at strength 0.** This is exactly Gates A + B — stated here so
   the §11 checklist has a line for it. `fs_main` is unedited, and `shade` adds the bounce inside
   the ambient block; nothing else can reach direct light or emissive.
10. **Bake-cache behaviour.** An unchanged light set is a cache hit (`bake_count` unchanged); a
    **moved** light re-bakes; a changed light **colour or intensity** re-bakes; flipping `bounce`
    between two non-zero values does **not** re-bake (strength is sample-time). Renderer-side unit
    tests in `gi_bake.rs`.
11. **Cross-process stability.** Two separate `render` processes on the bounce-lit scene produce
    byte-identical images (per-machine determinism; explicitly not a cross-platform claim). Lives in
    `crates/ludion-cli/tests/render_gi.rs` beside W1's, for the same reason W1 put it there.
12. **`--dump-gi` shows the bounce.** The sheet carries a BOUNCE block that is bright near the
    emitter's probes and dark in the sealed corner, and says "not baked" on a `bounce: 0.0` scene.
13. **Every W1 fixture stays green, unchanged.** State the audit explicitly: `gi_sky_visibility.rs`'s
    four tests, `gi_bake.rs`'s five unit tests, `render_gi.rs`, `gpu_render.rs`'s identity test and
    `voxelizer.rs`'s cache test. **No W1 assertion may move.** Which *could* legitimately move, and
    why none should:
    - `gi_bake.rs`'s exact-value assertions (`33/64`, `1.0`, `[0.0; 4]`) touch the visibility volume
      only, and W2 must not change it — §2 makes these the guard on the march refactor. Their
      `bake_and_read` helper's *signature* changes (the input grows lights/sun); that is mechanical.
    - `gi_sky_visibility.rs`'s ratio assertions could move if bounce leaked into a scene that did
      not author it. Under R1 (`bounce` defaults to `0.0`) none of those fixtures enable it, so
      they are green by construction — **a movement there is a defect, never a re-bless.**
    - `the_interior_darkens_while_the_exterior_holds` is the one that would visibly soften if bounce
      were ever on by default (bounce brightens a sealed room back up). It is the concrete reason
      R1's ruling matters.

Plus full `node scripts/check.mjs` green, every existing golden byte-identical except `gi-interior`,
no `run.lrp` re-record, no `sim-hash` reference moved.

---

## 10. Risks

- **A visibility leak becomes a radiance leak.** W1's own recorded finding: the probe-in-solid
  push-out can legitimately relocate a wall-embedded probe to the **bright** side of a wall that is
  thin relative to `probe_spacing`. In W1 that probe reported "sky reaches here"; in W2 it also
  carries whatever radiance the bright side sees, so the artifact is brighter and more coloured —
  the same defect, more visible. No new mitigation is built (the shipped protections are unchanged:
  push-out, `normal_bias` at sample time, and the authoring criterion "spacing ≤ thinnest
  occluder"), but T8 is the gate, and the hardening pass W1 recorded now has a second reason to
  exist. This is the wave's most likely way to look bad.
- **Cost at district scale.** §4.8: the `probes × rays × lights` term does not fit the plan's
  ~200 ms budget on a vigil district, and W4 is where vigil authors `GiVolume`. Two escalations are
  recorded; W3 must pick one on measurements. Discovering this at W4 with no written analysis is
  the failure this paragraph exists to prevent.
- **Emissive magnitudes are large in the real content.** Vigil authors emissive up to 5.0 on
  saturated palette colours; sixteen of twenty-one models emit. A physically-correct bounce off
  that content may blow past the grade's range (§9.9's warning). The `bounce` strength is the
  control, the `x-ui-range` to 2.0 admits that it will be authored below 1.0 as often as above, and
  W4 measures rather than assumes.
- **Double-count with W3′.** W2 bakes every light present at extract, including runtime-spawned
  ones (R3). When W3′'s dynamic injection lands, a light contributing to both terms is counted
  twice. The split must become explicit there — `GiStatic`, or a bake-side "static only" predicate.
  Recorded now so W3′ does not rediscover it as a bug.
- **Bake-cache thrash.** Any light a system moves invalidates the bake exactly as any mesh a system
  moves invalidates the voxelization. Same documented consequence, same recorded answer, no new
  concept — but it is now two ways to thrash instead of one.
- **Codegen perturbation.** A new uniform branch and three new bindings in a shipped shader can move
  pixels on an untaken path (the medium W3 finding). Gate A is the measurement; §2 records the
  escape.
- **Group-3 binding headroom.** Eight sampled textures in one group, nine in the fragment stage.
  Fine against WebGPU's guaranteed 16, but the next coefficient set has to count.

---

## 11. Work packages

Dependency-ordered. The files listed are the **ownership boundary** — no two packages in one wave
share one. Integration, `check.mjs`, the golden byte-identity sweep, and the commits are the
orchestrator's.

- **WP-1 — the authored field, end to end and inert.** `crates/ludion-engine/src/components.rs`
  (`GiVolume.bounce`, default, `range`, `x-ui-range`, docs), `crates/ludion-renderer/src/scene.rs`
  (`GiData.bounce`), `crates/ludion-engine/src/extract.rs` (`gi_data` copies it),
  `crates/ludion-engine/src/voxelizer.rs` (**doc-only**: correct `MAX_GI_PROBES`' per-probe byte
  figure, §3). Tests: round-trip, default, `L1014` on a negative value, registry order unchanged.
  Nothing consumes the field yet, so no image can move. **sonnet** — fully specified against an
  existing field. *Wave A.*
- **WP-2 — the bounce bake.** `crates/ludion-renderer/src/shaders/gi_bake.wgsl` (the `trace_hit`
  refactor, the face normal, radiance, the sun shadow march, the three writes),
  `crates/ludion-renderer/src/gi_bake.rs` (three volumes + fallbacks, the lights/sun uniform +
  storage buffer, the extended cache key, `resolve_and_read`'s new return shape),
  `crates/ludion-renderer/src/voxel_bake.rs` (hand back the albedo texture; delete its
  `#[allow(dead_code)]`), `crates/ludion-renderer/src/render.rs` (**`prepare_frame`'s `BakeInput`
  construction only**). Tests: §9 T5, T10, and the §2 guard that W1's visibility values are
  bit-unchanged. **opus** — the algorithmic heart, and the one package where a wrong choice is
  invisible until the sitting. *Wave B.*
- **WP-3 — consumption.** `crates/ludion-renderer/src/shaders/mesh.wgsl` (the `gi_bounce` helper,
  the R6 decision, the application site), `crates/ludion-renderer/src/render.rs` (`EnvParams` 64 →
  80, `env_bind_group`'s three bindings), `crates/ludion-renderer/src/shader_gen.rs` (regenerated).
  Tests: §9 T1 and T2, in `crates/ludion-engine/tests/gpu_render.rs`. **opus** — the pixel-moving
  edge, the layout growth, and the byte-identity argument. *Wave C.* (Shares `render.rs` with WP-2,
  hence a separate wave.)
- **WP-4 — `gi-interior` lights up.** `examples/gallery/gi-interior.yaml` (one line + comment),
  `examples/gallery/goldens/windows/gi-interior.png` (re-blessed),
  `Plan/artifacts/gi-wave2-bounce.png` (deleted before publication; in Lore history) + `gi-wave2-sweeps.md`,
  `Plan/artifacts/gi-wave2-gi-interior.diff.json`, the macOS queue line in the private macOS queue.
  Includes the §7 `lift` check. **sonnet** — with the sweep criterion stated in the brief so the
  choice is not taste. *Wave D.*
- **WP-5 — physics fixtures.** New file `crates/ludion-engine/tests/gi_bounce.rs`: §9 T3, T4, T6,
  T7, T8, and T7's demonstrated-able-to-fail note. **sonnet**, briefed with exact assertions and
  exact fixture geometry. *Wave D, parallel with WP-4.*
- **WP-6 — `--dump-gi`'s BOUNCE block.** `crates/ludion-engine/src/voxel_sheet.rs`,
  `crates/ludion-engine/src/render.rs` (the dump path), `crates/ludion-cli/src/render.rs` (help
  text), `crates/ludion-cli/tests/render_gi.rs` (§9 T11, T12). Consumes WP-2's new readback shape.
  **sonnet**. *Wave D, parallel.*
- **WP-7 — docs.** `docs/global-illumination.md` (the `bounce` field: what it does, what `0` costs,
  the units, that it is diffuse-only and indirect-only), `dev-docs/renderer-gi.md` (the bounce bake,
  the storage layout, the cache key, the district-scale note). Written through the `ludion-docs`
  skill; `crates/ludion-cli/embedded-docs/` re-mirrored by `--fix`. **sonnet**. *Wave D, parallel.*

Waves A → B → C → D. Wave A can start immediately; Wave D's four packages are file-disjoint and run
together.

---

## 12. Definition of done

- `node scripts/check.mjs` green (including schema-drift and the embedded-docs mirror after `--fix`).
- Every existing golden byte-identical **except** `examples/gallery/goldens/windows/gi-interior.png`,
  re-blessed with a committed SSIM diff report; the macOS bless queued in the private macOS queue.
- Gates A and B pass, and the §2 claim that W1's visibility volume is bit-unchanged is measured, not
  argued.
- All thirteen §9 tests pass; T7 (sun occlusion) and T8 (no bounce leak) are each **demonstrated
  able to fail**, with the mutation and the reddened numbers recorded in the commit — a leak gate
  never observed failing is not known to have teeth.
- `ludion render --scene examples/gallery/gi-interior.yaml --dump-gi out.png` shows a BOUNCE block
  bright around the torch and dark in the back corner.
- Bake time for `gi-interior` measured and recorded on the pinned GPU, alongside W1's number, plus
  the mesh `scene` pass's cost with bounce on vs off from one `profile.capture` — the three extra
  3D fetches are a per-frame cost and this repo does not ship untracked ones.
- The group-3 sampled-texture count recorded (§3) so the next coefficient set starts from a number.
- No `run.lrp` re-record, no `sim-hash` reference moved, wire surface unchanged at 27 methods.
- `Plan/global-illumination.md` §4's W2 paragraph marked delivered with any corrections to its
  assumptions; this file's Status filled in; the graphics-engine notebook gains a W2 Done entry
  (outcomes only) and its TODO's "then W2" line retired.

---

## 13. Micro-decisions deferred into implementation (record the answers in the commit)

- **R6's outcome**: shared `sh_l1` helper, or three inlined lines in `gi_bounce`.
- The **fixed exposure** the `--dump-gi` BOUNCE block normalizes by, and whether the DC-only block
  is legible enough to keep BOUNCE DIRECTION deferred.
- Whether the hit-cell normal wants the **entry face** or the **negated ray direction** on the
  boundary-clamp path specifically — measure on a fixture whose probes sit on the grid's faces.
- Whether the sun shadow march should start **one cell** off the face or use a small `t` epsilon.
- The shipped `bounce` for `gi-interior`, off the sweep sheet.
- Whether `gi-interior`'s `PostProcess { lift: 0.002 }` is still doing anything once the room is lit
  (§7), and whether removing it is worth the second `WorldHash` move.
- Whether the bake's lights signature should hash the raw `PointLightData` or the premultiplied
  `Light` — the latter is one fewer convention to keep in step, the former is one fewer conversion
  before a cache compare.

---

## 14. Explicitly deferred, with triggers

- **AO attenuating the bounce term.** Trigger: the sitting reports the bounce reads flat at contact
  scale, with no shadowing where surfaces meet.
- **Indirect specular from the volume.** Not expressible in SH-L1. Trigger: Alternative C's, unchanged.
- **A second bounce.** Trigger: a light-coloured interior reads too dark against reference. The
  shape is a loop bound (§4.4); the knob belongs to the quality-tier system, not to `GiVolume`.
- **Shadow rays for static point lights.** Trigger: corner-leak artifacts called by eye (§4.3).
- **Sky bounce off surfaces** (R4). Trigger: the first daylit exterior scene.
- **The per-cell direct-radiance pre-pass** and **the binned light grid** (§4.8). Trigger: W3's
  district-scale bake budget.
- **DC-only RGB bounce** (a strict subset) and **the 2-texture luminance-L1 packing** (§3). Trigger:
  probe memory or the three fragment fetches become the binding constraint.
- **`GiStatic`** (D2b), for both the mesh and the light sides. Trigger: W3′'s double-count, or
  content that animates an authored entity under a live `GiVolume` — whichever fires first.
