# GI Wave 3′ — dynamic light injection (detailed implementation plan)

> Status: **DELIVERED (2026-08-08, Lore revs 539–550).** Ratified 2026-08-07 (R1–R7 as
> recommended, the W2 precedent; R6's preset defaults stay flagged for the scalability arc's laptop
> measurement per §15) and implemented as waves A (rev 543) → B (WP-3 rev 540 ∥ WP-2 rev 544) → C
> (rev 545) → D (revs 546–549, six parallel packages). **One design correction mid-implementation,
> measured then fixed (rev 545): R3's contiguous-prefix premise was false** — see the corrected R3
> and §4.2; the cache strides. Wave-log deltas from the written plan, so nobody re-derives them:
> WP-3's construction guards are `assert!` panics not diagnostics (orchestrator ruling — caller
> bugs, the crate's idiom); WP-1 found and closed a field-less-struct schema gap (empty `properties`
> forced so an extra key is the ordinary `L1014`); `PostChain.gi_inject` carries the *lattice*, not
> a flag (the transient's extent is graph structure); the §15 early-out resolves to per-hit skip
> alone; §9.1's intensity watch resolved KEPT (relative pool brightening +49% → +43%, corner
> holds); the stress budget's first calibration (0.0192 ms) was measured through mid-wave binaries
> and re-measured settled at **0.0822 ms median, ceiling 0.16**; `engine.describe`'s quality object
> gained the fourth toggle (an unwritten fourth home the three-homes framing missed). §14's
> checklist passed in full: check.mjs green (vitest's 6 statue-gemma reds the recorded baseline),
> exactly two golden events (`gi-interior` re-bless SSIM 0.9511 + `gi-inject-stress` first bless),
> no replay re-record, wire surface 27, zero new L-codes.
> Expands `Plan/global-illumination.md` §9/§9.10's W3′ outline
> against the code as W2 left it (`Plan/gi-wave2-emissive-bounce.md`'s Status is the predecessor's
> outcome record, and its shape is this document's template). It resolves the §9.11 leanings the
> plan deferred to this wave (D10–D12), consumes the fresh D13 measurement recorded below, pays the
> two debts W2 recorded for this wave by name (the R3 double-count, the §4.8 district-scale bake
> budget), and lands the GI arc's host quality knob against the scalability arc's just-landed
> `Quality` seam (`crates/ludion-renderer/src/quality.rs`, Lore rev 537).
>
> One line, so the sections below can be read against it: **dynamic lights bounce through a
> single-frame, zero-history path — a new `GiDynamic` marker splits the light set explicitly, the
> static bake keeps everything unmarked (and stops re-baking when marked lights move), and one new
> frame-graph compute pass replays each probe's *cached bake hits* against a world-space bin of the
> marked lights, writing an irradiance-only volume `mesh.wgsl` adds beside the baked bounce.** No
> splat, no atomics, no history, no per-frame march: the bake's own hit set is the static
> visibility the gather looks through, so leak control is inherited rather than rebuilt.

---

## The D13 measurement (2026-08-07, this session — the numbers this wave is sized against)

Measured on `district-plaza` real gameplay through a deterministic scratch probe harness:

- **Peak simultaneous projectile lights: 6** (auto mode, kiting, 6470 ticks) / **7** (twin-stick,
  fire held). Mean 2.0–5.2, p95 4. A realistic two-weapon ceiling is ~8–9.
- **The hard code ceiling is `MAX_PROJECTILES = 300`** (`games/vigil/src/tunables.rs:251`). The
  plan's "300 live bullets" is this *authored cap*, not an observed population; `budget-horde` is
  hand-placed **at** the cap as static load, not a firing scenario.
- Bolt lifetime: max 120 ticks (2.0 s), mean 36 (killed early on hit).
- **Spatially NOT clustered in real play**: at peak, 6 bolts occupy 6 distinct 4-unit cells (bbox
  6.3×32 units, elongated). §9.1's correlated-burst premise manifests only at the cap regime.
- Static lights on the same scene: 52. `EnemyShot` carries **no** `PointLight` today. The committed
  `stress.lrp` exercises 1000 *static* bolts (life 3600 s, no firing) + 5000 enemies — a load
  fixture, not a firing scenario.

Consequences taken below: the design is **sized to the cap (300)** as its ceiling and **shaped for
single digits** as its steady state; D12's irradiance-only leaning gets easier, not harder (the
organic term is faint, ground-up, and low-contrast); and §9.3's stage-1 radiance aggregation — whose
whole value is collapsing correlated bursts — is **deferred with a trigger** rather than built,
because the population it collapses does not occur outside the adversarial cap regime (§16).

---

## 0. Rulings needed before the wave starts

Seven questions this document takes a position on but cannot settle from the written record. Each is
marked **needs user/orchestrator ruling**; the recommendation is what the rest of the document is
written against, so a different ruling means an edit, not a redesign.

- **R1 — the split mechanism is a new marker component, `GiDynamic`, and absent means static.**
  The alternative D2a implied — classify by "spawned at load vs spawned at runtime" — was checked
  and is **rejected on principle, not feasibility**: `extract` sees only the `World`, the `World`
  records no spawn provenance, and any engine-side bookkeeping that recorded it would be
  *path-dependent hidden state* — an `engine.snapshot`/`engine.restore` round trip, or two replay
  routes reaching the same world state, would reclassify every light and move pixels, which is
  exactly the `image = f(world_state)` property the temporal rule protects. Making provenance
  honest means serializing it, at which point it *is* a component — so author the component,
  diffable and auditable, instead of an implicit one. `GiDynamic` is an empty (field-less) marker
  on the entity, registered append-only in the freeze list; its meaning covers the whole entity's
  GI role (§3): the entity's `PointLight` is **injected, never baked**, its `Mesh` neither occludes
  nor deposits emissive into the voxelization, and its motion invalidates **nothing**. Absent =
  today's behaviour exactly, so every existing scene's bake is byte-unchanged by construction.
  Rejected spellings: a `PointLight` field (adding a field moves `WorldHash` for *every* scene that
  authors any light — vigil's replays would re-record now instead of at W4) and the `GiStatic`
  direct sense (absent-means-dynamic silently removes every authored light from every existing
  bake). The authoring burden this accepts — **a game must mark every archetype a system moves** —
  is real and is documented + instrumented rather than solved (§3, §10, §12). **Recommend as
  stated.** This is the wave's one new registered component; the freeze-list consequence and the
  registry-order tripwire move are §3's.
- **R2 — the dynamic term rides the authored `bounce`, with no second strength field.** The
  injected term *is* the bounce term — the same physical quantity from lights that happen to move —
  so it is gated by `GiVolume.bounce > 0.0` and scaled at sample time by the same `env.gi_bounce.x`
  the static term uses. A scene at `bounce: 0.0` gets no injection, no matter what is marked; a
  second `dynamic_bounce` knob would be a second spelling of one number ("before adding a second
  way…"). **Recommend shared.** Cheap to reverse (one field later, additive).
- **R3 — D12 resolves to irradiance-only, and the gather budget is `K = 16` cached rays.** The
  dynamic term is **one** `Rgba16Float` volume holding RGB DC irradiance, no L1 bands — a third of
  the memory, bandwidth, and bindings of the static bounce trio, justified twice over by D13 (the
  organic population is single-digit, ground-up, low-contrast bounce; a wall glowing faintly on
  both sides of a bolt for 36 ticks is not a call anyone makes). The per-probe gather replays
  **16 of the bake's 64 rays, STRIDED (every 4th), not the contiguous prefix this ruling
  originally specified** — implementation (WP-4) measured that the Fibonacci set's
  well-distributed-at-any-prefix property holds for the golden-angle *azimuth* only, while the
  *elevation* sweeps monotonically, so a contiguous prefix is a ~59° polar cap that can never see
  a floor below the probe (an open-slab fixture read all zeros with a marked light directly
  overhead — the vigil ground-bounce product case failing). The strided subset spans elevation
  evenly at the same cost; the correction changed the cache's *contents* only, never a baked
  volume. The
  directional upgrade is a strict additive (three more volumes; §16, with the binding count that
  bounds it). **Recommend as stated.** Note `K` is a shader constant like `RAYS`: its quality-axis
  spelling belongs to the scalability arc's W2 (`gi_rays`' sibling), **not** to this wave.
- **R4 — the injection runs as the frame graph's first compute pass (Seam B, items 1 + 2).**
  `Plan/global-illumination.md` §8(3) names this exact moment: "take Seam B at the moment the
  update becomes per-frame… a recurring per-frame cost invisible to `--dump-graph`,
  `--dump-timings` and every budget file is exactly the untracked cost this repo's profile culture
  rejects." The injection is the arc's first per-frame GPU work, so `ludion-render-graph` gains
  `PassKind::Compute` (executor dispatch + timings attribution) and 3D extents + `StorageBinding`
  on `TextureDesc` — and **not** item 3 (the persistent resource kind): the dynamic volume is
  recomputed whole every frame, i.e. a perfectly ordinary *transient*, and the static volumes and
  the hit cache stay renderer-owned externals exactly as `ibl.rs`'s cubes do. What this buys
  concretely: the `gi_inject` pass appears in `--dump-timings`, gets a `[passes]` budget line, and
  rides the profiler's existing per-pass GPU timestamp ring **with no `ProfileReport` schema rev
  and no replay re-record** — the pass list is keyed by name, not by schema. The alternative (run
  it outside the graph like the bake) would need its own timestamp machinery to avoid shipping an
  untracked per-frame cost, which is comparable work for none of the reuse. **Recommend Seam B.**
  Needs a ruling because it is a cross-crate architectural commitment in a crate this arc has not
  touched.
- **R5 — the static bake adopts the same world-space bin this wave, and the per-cell
  direct-radiance pre-pass is REJECTED.** W2 §4.8's recorded escalation for the district-scale
  `probes × rays × lights` term was a pre-pass writing per-cell radiance, with the per-cell surface
  normal as its recorded open problem. This wave's answer **dissolves the problem instead of
  solving it**: the `probes × rays × lights` term dies by looking up the hit cell's *bin list*
  instead of looping every light at every hit — the same counting-sort bin the injection builds
  (§4.3), applied to the static subset at bake time. Every light the bin excludes at a hit is
  provably out of range of that cell, so its contribution was **exactly `+0.0`** (the `atten`
  clamp), and dropping an exact zero from an all-non-negative accumulation is bit-preserving — the
  `cluster.rs` identity argument, so the refactor is **byte-identity-provable**, gated by a
  binned-vs-degenerate bake oracle (§11 T8) plus W1's exact-value tests, and it keeps the *hit
  face normal*, which is what the pre-pass could not have. The one term the bin cannot touch — the
  sun's per-hit shadow march — gets its escalation **designed and deferred**: a per-(cell, face)
  sun-visibility bitmask pre-pass, byte-identical by construction because `sun_shadow(cell, n)` is
  already a pure function of `(cell, face)` and memoizing a pure function changes no value
  (§6.3); its trigger is W4's first measured district bake breaching the budget. **Recommend as
  stated.** Needs a ruling because it re-enters the shipped W2 shader.
- **R6 — the quality field is `gi_dynamic`: `false` in `LOW`, `true` everywhere else, and masking
  DROPS the term, never re-homes it.** The renderer's `Quality` struct (rev 537) grows a fourth
  toggle beside `ssao`/`shafts`/`dof`; the engine's TOML `[effects]` section and the
  `QualityDescriptor` are the other two homes (§8 — the agreed three-homes-move-together seam).
  The tempting alternative — masked ⇒ marked lights *bake* instead, which would make masked
  byte-identical to unmarked — is rejected because it hands the low-end machine the per-frame
  re-bake disaster the marker exists to prevent (a moving marked light would invalidate the bake
  every frame precisely on the hardware least able to pay it). Masked means: the marked lights
  contribute direct only, exactly as they do today under W2 with the R3 debt unpaid — no bake
  thrash, no injection, the pass never scheduled. Vocabulary throughout: **"quality
  preset/settings", never "quality tier"** (the scalability arc's recorded naming decision; the GI
  plan's older phrasing predates it). **Recommend as stated.**
- **R7 — golden policy: `gi-interior` re-blesses as the wave's single move, the new stress scene
  first-blesses, and nothing else moves.** The calibrator's mover was authored in W1 *for this
  wave* ("W3's injection testbed", its own header) and gains one line — `GiDynamic: {}` — which
  moves the golden three ways at once: its light leaves the static bake, its emissive sphere
  leaves the voxelization, and its light injects per frame at the default quality. The W2
  precedent (its R2) holds verbatim: the calibrator moving is what it is for, and the W4 content
  re-bless budget is untouched. Every other committed golden stays byte-identical — the quality
  default is `FULL` and no other committed scene authors `GiDynamic`, so this is a contract proven
  by the suite, not a hope. **Recommend as stated.**

---

## 1. Goal and the product

W1 made the sky stop reaching where it cannot; W2 made light arrive from surfaces the scene was
born with. W3′ makes **moving light bounce**: a light the sim spawns or animates contributes
one-bounce indirect light, recomputed whole every frame with zero history, while the static bake —
for the first time — is a **cache hit on every frame of real gameplay**, because the lights that
move are no longer its inputs.

The product, stated so it can be judged by eye: **`gi-interior`'s mover — the dim blue sphere
authored in W1 and baked-static ever since — becomes live.** Its faint blue pool on the floor and
wall beside it is now the injection's output, present at the default quality, absent at `low`, and
the sealed back corner it was deliberately kept away from stays uncontaminated. On vigil at W4
(not this wave), the same machinery is what lets a tracer bolt drag a warm indirect smear across
the cobbles and the lantern warm the walls the player walks past — without the world re-baking
once.

This wave is also where three debts recorded by name become paid:

1. **W2 R3's double-count**: a light now appears in exactly one term — baked (unmarked) or
   injected (marked) — asserted by fixture, not argued (§11 T5).
2. **W2 R3's bake thrash**: a *marked* light a system moves invalidates nothing; the bake's lights
   signature covers the static subset only (§3.2).
3. **W2 §4.8's district-scale budget**: the `probes × rays × lights` term is deleted by the bin
   (R5), with the arithmetic proven bit-identical rather than re-blessed (§6).

**What it is not.** No history, no accumulation, no frame index in `RenderScene`, no convergence:
the dynamic term is a pure function of this frame's world state, and `Plan/global-illumination.md`
D7/D8 stand untouched. It is bounce-only (the light's *direct* arrival at every surface is
entirely clustered forward's job — §9.9's energy rule); it is diffuse-only; it is not in
`WorldHash`, not in snapshots, not on the wire (the volume — the `GiDynamic` *component* is world
state and hashes like any other). And it is **not** the scalability arc's W2: `gi_rays`,
`gi_probe_scale`, and every parameter-modulation axis stay in that plan.

---

## 2. The neutrality contract, and this wave's golden policy

**Golden policy, stated plainly (R7): exactly one golden moves — `examples/gallery/goldens/windows/gi-interior.png`
— plus one first bless, the new `gi-inject-stress` scene's.** Every other committed golden stays
byte-identical. The macOS queue gains both (the `gi-interior` entry supersedes W2's queued one, the
same file); an SSIM diff report is committed beside the moved golden as
`Plan/artifacts/gi-wave3-gi-interior.diff.json`.

Four gates carry the claim, each **measured, never assumed** (the volumetric-medium W3 finding —
byte-identity is a property of generated code, not of values — applies to every one):

- **Gate A — unused-content neutrality, by construction.** A scene with no `GiDynamic` marker
  schedules no `gi_inject` pass (the graph simply lacks it — the fog-pass-drop precedent, 16 → 14),
  binds the 1×1×1 dynamic fallback with a zero flag, and renders **byte-identical** to the same
  build before the wave — across *both* quality states, which is what proves the new toggle and
  the new binding perturb nothing they don't own. Proven by the whole golden suite plus an explicit
  pass-count assertion (§11 T1, T2).
- **Gate B — the additive identity, through live bindings.** A scene with a marked light placed
  where **no cached hit is within its range** allocates the dynamic volume, runs the pass, writes
  all zeros, takes the shader branch, and adds exactly `+0.0` — byte-identical to the same scene
  masked. W2 Gate B's `−0.0` closure transfers whole: the accumulator at the add site is a sum of
  non-negative products and cannot be `−0.0`, the volume is written everywhere the pass runs (no
  undefined texels), and the `max(…, vec3(0.0))` floor holds the reconstruction at `+0.0`
  regardless (§11 T3).
- **Gate C — masked is the absent path.** `gi_dynamic: false` un-schedules the pass, binds the
  fallback, zeroes the flag — the *same* codegen path as Gate A's scene, never a cheaper variant.
  The scalability seam's named test shape (`a_masked_effect_renders_exactly_as_an_unauthored_one`)
  is instantiated as Gate A's cross-toggle identity; for a scene that *does* mark, "feature-absent"
  and "marker-absent" legitimately differ — by the bake exclusion, which is world-state semantics
  the quality knob must not reach (R6) — so the masked contract for marked content is Gate B's
  arithmetic plus the structural assertions, and this paragraph is the recorded reason the naïve
  marked-vs-unmarked byte compare is *not* a valid test (§11 T2 states it as a non-goal).
- **Gate D — the static refactor moves nothing.** The bin adoption in `gi_bake.wgsl` (R5) leaves
  every existing volume bit-identical: W1's exact-value assertions (`33/64`, `1.0`, `[0.0; 4]`)
  unmoved, W2's magnitude/falloff/leak fixtures green unchanged, the whole golden suite
  byte-identical, and the binned-vs-degenerate oracle (§11 T8) as the permanent gate. **Wave
  ordering is part of the proof**: the refactor lands and is verified byte-identical *before* the
  calibrator commit marks the mover, so the one golden move has exactly one cause.

---

## 3. The split becomes explicit — `GiDynamic`

### 3.1 The component

One registered marker, **no fields**:

| Component | Fields | Default | Meaning |
|---|---|---|---|
| `GiDynamic` | *(none)* | — (opt-in) | This entity is dynamic for GI. Its `PointLight` is injected per frame instead of baked; its `Mesh` neither occludes the probe rays nor deposits albedo/emissive into the voxelization; moving it invalidates neither the voxelization nor the bake. |

Authored as `GiDynamic: {}`. On an entity with neither `Mesh` nor `PointLight` it is inert — no
diagnostic, the same posture as any marker whose subject is absent. **No new L-code**: there is no
value to be wrong (an empty object's schema rejects extras as the ordinary `L1014`), and no
rejection class exists that an existing code does not already express — the discipline that has
held since wave 0 continues.

Why the whole entity and not just the light: the mesh side has the *same* debt. The voxelizer
consumes every placed instance and its signature covers every model matrix, so the day a vigil
district authors a `GiVolume` (W4), five thousand walking enemies would re-rasterize the
multi-million-cell grid every frame — 310–340 ms of CPU, per frame. `GiDynamic` on the enemy/bolt/
player archetypes (a handful of one-line game-side edits) is what makes W4 possible at all, and
splitting "dynamic light" from "dynamic occluder" into two markers would force every moving
lit entity to author both. One component, whole story. Consistent with §8's ruling: dynamic
geometry *receives* GI (it samples the volumes like everything else) but does not occlude it; a
marked entity's **emissive does not bounce** in this wave (its surface is not voxelized, and the
injection input is lights, not surfaces) — deferred with a trigger (§16).

The authoring burden accepted under R1, stated where a game author will find it (the
`docs/global-illumination.md` update, §13 WP-10): **mark every entity a system moves.** The
symptom of forgetting is not wrong pixels — an unmarked moving light renders *correctly*, because
the bake re-runs — it is a per-frame bake re-run visible in `profile.capture` and in the
`--dump-gi` header (§10), and for meshes a per-frame re-voxelization. Correct-but-slow, with the
instrument pointing at the cause.

### 3.2 Plumbing

- `extract` reads the marker once per entity: `PointLightData` grows `dynamic: bool` (renderer
  plain data, not wire, not hashed), and marked instances are **skipped by the voxelizer's
  collection and excluded from its instance signature** — so a marked mesh's motion no longer
  perturbs the signature at all (§11 T13). `RenderScene.lights` itself is untouched in content and
  order: **every light, marked or not, still reaches clustered forward** — the split governs GI
  only, and the pinned ascending-`EntityId` order is the same order both consumers read.
- `prepare_frame` partitions once: the static subset (unmarked, in extract order) feeds
  `BakeInput.lights` and the lights signature; the dynamic subset (marked, in extract order) feeds
  the bin (§4.3). The signature change **is** the thrash fix: a marked light's motion never
  reaches the key. Asserted both ways in §11 T5 — a moved marked light is a cache hit, a moved
  unmarked light still re-bakes (the documented W2 consequence, unchanged for unmarked lights).
- The sun is **always static**. A `GiDynamic` marker on the sun's entity affects only that
  entity's mesh and point light; the `DirectionalLight` bakes regardless. A moving sun invalidates
  the bake every frame today and continues to — the day-night-cycle answer is §8's deferred
  amortization, not this wave (§16).

### 3.3 `WorldHash` and the registry — verified, not assumed

Registering a component appends to the freeze list: the registry-order tripwire test moves by
exactly one appended entry, every existing component index is stable, and schema-drift regenerates
the JSON Schema + TS protocol types (`check.mjs --fix` / CI). `WorldHash` moves **only for scenes
that author the marker**. Exactly one committed scene will: `examples/gallery/gi-interior.yaml`. A
repo-wide search for `gi-interior` (re-run 2026-08-07) finds docs, tests, plans, and the editor's
`target.ts` — **no `.lrp`, no `sim-hash` reference, no committed snapshot**. So: **no replay
re-records and no `sim-hash` reference moves this wave**; vigil's re-record waits for W4 exactly as
the charter states. Re-verify before committing rather than trusting this paragraph.

---

## 4. What the wave stores, and why

Three structures. One is per-frame and transient; two are load-time by-products of the bake,
cached and invalidated with it — **path-independent functions of world state, not history**.

### 4.1 The dynamic volume — one `Rgba16Float` 3D texture, irradiance only (R3)

On the **same probe lattice** as the static volumes (same `count`, same `origin`/`span`, same
sample transform — §9.2's "unify the storage" taken literally): `rgb` = the DC term of one-bounce
irradiance from dynamic lights, `a` = written zero, reserved. Per-probe cost 8 B; a district's
460 k probes are ~3.7 MB. It is a **frame-graph transient** (R4): allocated by the graph on frames
that schedule `gi_inject`, written whole by the pass (every probe, every frame — zero history by
construction), sampled by the mesh pass, forgotten.

**Rejected, so the choice is not re-litigated:**

- **SH-L1 (the static trio's format).** Three more textures, three more fetches, three more
  bindings — for directionality on a term D13 measures as single-digit lights of range ~2.5–3.5
  whose bounce is ground-up and dies in 36 ticks. It remains the recorded upgrade (strictly
  additive: the volume trio + the L1 lanes of the gather are already computed as by-products and
  currently discarded), with the binding arithmetic that bounds it recorded in §7. Trigger in §16.
- **A separate lattice (coarser for the dynamic term).** A second `origin/scale` pair, a second
  sample transform, and a seam between the two terms' interpolation — for memory nobody is short
  of at 8 B/probe. The one lattice is also what makes the smoothness argument (§5.4) inherit W1's
  trilinear behaviour unchanged.

### 4.2 The hit cache — the bake's march, made replayable

The bake already knows, for every probe and every ray, *which cell the ray hit and through which
face* — and for static geometry that answer **never changes between bakes**. W3′ has the bake
write it down: a storage buffer of one `u32` per `(probe, k)` for `K = 16` of the 64 rays —
**strided, every 4th ray, because a contiguous Fibonacci prefix is a polar cap (R3's
correction)** — packing the flat cell index (≤ 2²⁵ under the `L2103` 32 M-cell budget) with a
3-bit face code (six axis faces; code 6 = the boundary-clamp case, whose normal is
`-ray_direction(k · stride)`, recomputable from the slot alone; code 7 = escaped). Written only when `bounce_enabled` (it rides the
same discriminant as the bounce trio), cached and invalidated with the bake, allocated ~30 MB at
district scale (460 k × 16 × 4 B; the `MAX_GI_PROBES` guardrail comment gains the hit-cache line —
256 MB at the 4 M cap, a number to state, not to reach).

This buffer is the wave's central move, and what it buys is stated plainly: **the per-frame gather
never marches.** Replaying a cached hit is two buffer/texture fetches; a fresh march is tens of
DDA steps. At the 300-light cap with the whole lattice active, the cached replay is ~15 M fetches
(§5.5) where a re-march would be back at the full bake's per-frame cost — the difference between a
sub-millisecond pass and a shipwreck. And it is the literal fulfilment of the charter's leak rule:
*the cached hit set IS the static visibility the bake already produced*, sampled at the same rays —
a probe inside a sealed room caches no hit outside it, so a bolt outside the wall contributes
nothing, with no new leak-control mechanism to build or to get wrong (§11 T4).

The hit cache is **not history**: it is a pure function of `(grid, lattice)`, keyed by the same
cache key as the volumes it rides beside, identical however the world arrived at its state.

### 4.3 The world-space bin — this plan's own, at last (D10)

The structure §9 promised and clustered forward's decision 8 explicitly did *not* ship (its grid is
view-frustum-space — the wrong space for a world-space gather). New module
`crates/ludion-renderer/src/gi_bin.rs`, inheriting `cluster.rs`'s **shape** — counting sort keyed
by light index, lights outer / cells inner, so every cell's list ascends by construction and needs
no sort — over a different domain:

- **Domain**: the voxel grid's world bounds (hit cells are voxel cells, so the lookup key is a hit
  cell's centre; a light outside the domain clamps its footprint into it, conservatively).
- **Shape**: fixed `32 × 16 × 32` cells over the grid's span (the `CLUSTER_TILES` posture — a
  constant grid, cell size a pure function of authored bounds; a district's cells come out ~4 × 1.5
  × 4.5 units, the same order as a light's range). A micro-decision may retune the constants
  (§15); the *fixedness* is the design.
- **Filing**: each light lands in every bin cell overlapped by its range-sphere's AABB, **expanded
  by one voxel cell** — the `TILE_PIXEL_MARGIN` posture, covering the float rounding between the
  CPU's cell arithmetic and the shader's evaluation points (cell centres).
- **Build**: sequential, on the render thread, at most 300 lights — microseconds, `par`
  deliberately unused, so thread-count invariance is trivial rather than argued. The arithmetic is
  additions, multiplications, and `floor` — no transcendentals — so it is also process- and
  platform-invariant, though nothing downstream requires the latter.

**Why this bin does not inherit `cluster.rs`'s "deliberately NOT bit-reproducible" stance, stated
so nobody relaxes it later.** `cluster.rs` may bin loosely because its consumer re-tests range
exactly and an admitted-but-out-of-range light contributes an exact `+0.0` — over-inclusion is
invisible. This wave's *gather* preserves that property (every per-light term carries the hard
`atten` zero, §5.2), so conservative slack is likewise invisible **here** — but the moment anyone
adds §16's deferred stage-1 aggregation (summing lights into per-cell virtual sources), the bin's
*assignment* becomes part of the radiance arithmetic and lands in pixels. The rule this plan
ships: **the bin build is deterministic and fixed-order as written, and any future change that
makes bin assignment affect a value (not just a loop bound) must carry the cross-process fixture
(§11 T10) as its gate.** The determinism is nearly free today; keeping it is what keeps the
aggregation door open.

Two consumers, one build (the "build it once" rule §9.5 demanded, landed): the **static** subset
is binned once per bake and uploaded beside the light buffer (§6.1); the **dynamic** subset is
binned per frame and uploaded to `gi_inject` (§5.1). Same code, different inputs.

---

## 5. The per-frame injection path

Scheduled only when *all* of: a `GiVolume` with `bounce > 0.0` resolved a real lattice, at least
one marked light exists, and `quality.gi_dynamic` is true. Any of them false ⇒ the pass does not
exist in the graph, the fallback binds, the flag is zero — Gates A/C.

### 5.1 CPU: partition and bin

`prepare_frame` splits the extracted lights by the `dynamic` tag (order-preserving), premultiplies
the dynamic subset into the generated `Light` POD (the one wire format for "a light, GPU-visible" —
decision 8's contract, reused not re-derived), builds the bin (§4.3), and uploads: a params
uniform (bin dims/origin/inverse-cell, `K`, counts), the `(offset, count)` table, the index list,
and the dynamic light array. All per-frame transients.

### 5.2 GPU: `gi_inject` — replay the cached hits against the bin

One compute dispatch over the probe lattice (`4×4×4` workgroups, the bake's own shape), in a new
`shaders/gi_inject.wgsl` — like `gi_bake.wgsl`, **not** part of the shader-bindgen surface
(compute, auto-derived layout, hand PODs; the precedent is stated in that shader's header and
repeated in this one):

```wgsl
// per probe:
var acc = vec3<f32>(0.0);
for (var k = 0u; k < K; k = k + 1u) {
    let h = hit_cache[probe_flat * K + k];
    if (h == ESCAPED) { continue; }                  // sky is W1's term; air carries nothing
    let cell = unpack_cell(h);
    let n    = unpack_normal(h, k);                  // face axis, or -ray_direction(k) for code 6
    let a    = textureLoad(albedo, cell, 0).rgb;
    let world = cell_center(cell);
    let bin  = bin_cell_of(world);
    let slice = bin_table[bin];                      // (offset, count)
    for (var i = 0u; i < slice.y; i = i + 1u) {
        let light = dyn_lights[bin_indices[slice.x + i]];
        // verbatim `direct_at` arithmetic: premultiplied color, ndl, ranged atten^2 —
        // the same convention as the bake and the fragment loop, never re-derived.
        acc = acc + a * light.color.rgb * ndl * atten * atten;
    }
}
textureStore(dyn_out, gid, vec4<f32>(acc / f32(K), 0.0));
```

Five properties, each doing a job named earlier in the arc:

- **Energy — bounce only, and only from dynamic lights.** The gather shades *surfaces* (cached
  hits) under the dynamic subset and integrates their outgoing radiance at the probe. The light's
  direct arrival at receiving surfaces is clustered forward's (§9.9); an escaped ray contributes
  zero (no sky — W1's term; no "direct at the probe" — a probe is not a surface); the hit cell's
  **emissive is deliberately not re-added** (it is static content and already lives in the baked
  bounce — adding it here would double-count every torch a bolt flies past); static lights are
  absent from the bin by construction. A light therefore contributes to exactly one term — §11 T5's
  gate.
- **Leak control — inherited, exactly.** The hits are the bake's own march; the sealed-room
  geometry that stopped W2's bounce stops this term identically, with the same `normal_bias`
  story at sample time (§7). §11 T4 is the twin of W2's T8, teeth demonstrated the same way.
- **Units — `(1/K) Σ L` is the static DC's own estimator on the prefix.** The same
  cosine-convention radiance, averaged over a uniformly distributed direction set, so
  `dyn × diffuse` composes with `irradiance × diffuse` and `gi_bounce × diffuse` unit-for-unit;
  the marked-vs-unmarked energy-agreement fixture (§11 T6) pins it to arithmetic.
- **Determinism — fixed order everywhere.** `k` ascends; each bin list ascends by construction;
  no atomics, no cross-invocation reduction, one invocation per probe. D11 resolves to **binned
  gather** — fixed-point atomics are not merely unchosen, they are absent from the design. GPU
  float arithmetic makes this per-machine deterministic (per-OS goldens gate it, the bake's own
  tier), and §11 T10 pins cross-process byte-identity at a few hundred lights, the D11 fixture the
  plan demanded.
- **Cost independent of light count in the steady state, bounded at the cap.** The pass's fixed
  term is `probes × K` cache reads with an immediate `ESCAPED`/empty-bin skip; the variable term is
  the per-hit bin loop, which D13 says is 0–2 lights organically and at most the cap under
  adversarial clustering.

### 5.3 Where it sits in the frame

`gi_inject` is declared before the scene pass, writing the transient dynamic volume the scene
pass declares as a read — the graph's ordinary dependency edge orders them, and the executor's
timestamp ring attributes its cost under its own name in `--dump-timings` and the profiler's GPU
section (R4's dividend: no schema rev). `pass::GI_INJECT` joins `KNOWN_PASS_NAMES`. The mesh
recorder receives the graph-owned view and folds it into `env_bind_group` (the shadow-wave
precedent for graph-owned views reaching external recorders); frames without the pass bind the
renderer-owned 1×1×1 fallback.

### 5.4 Smoothness — §9.9's flicker, answered structurally

The charter demands "trilinear smooth deposit, never nearest-cell." This design has **no deposit
at all**, and its smoothness is an argument, not a tuning: as a light moves continuously, every
cached hit's contribution moves continuously (`ndl`, `dist`, and `atten²` are continuous; `atten`
reaches zero *with zero slope* at range), and the hit set itself never changes (static geometry,
fixed rays). The only discrete event — a light entering/leaving a bin cell — is provably invisible:
conservative filing means a light leaves a bin cell only when its range-sphere no longer overlaps
it, at which point its contribution at every evaluation point inside that cell is already exactly
zero. Probe-to-fragment interpolation is the same hardware trilinear W1 shipped. §11 T7 measures
the claim anyway and demonstrates the gate can fail (a deliberately non-conservative bin steps).

### 5.5 Cost, sized against D13 and the M4

The numbers the budget conversation starts from (estimates to be replaced by measurement, stated
so the measurement has something to disagree with):

- **Steady state** (≤ 9 marked lights, `gi-interior` / vigil organic play): bin build ~µs; pass ≈
  `probes × K` mostly-skipped iterations + a few thousand shaded hits — expected well under
  0.1 ms on the 3090 and comfortably sub-millisecond on the M4.
- **The cap** (300 marked lights, the stress scene): the fixed term at district scale is ~7.4 M
  cache reads + bin lookups; the shaded term depends on clustering (D13: not clustered in real
  play). Expected order ~0.5–2 ms on the 3090; the M4 number is a measurement this wave owes, not
  a guess. Context: the M4's whole measured post chain is 4.6 ms wall against a 16.6 ms frame, and
  vigil renders at 1.20–1.81 ms on the 3090 — the quality knob exists precisely so a machine that
  cannot afford the term does not pay it.
- **Budget from the first commit** (§9.9's rule): the stress scene's budget file carries a
  `gi_inject` `[passes]` ceiling calibrated off the measured median (the profile-budget 2× posture),
  gated in CI like every pass ceiling; the mac owes its measured sibling (§14).

---

## 6. The static bake at district scale — W2 §4.8's debt

### 6.1 The bin replaces the flat light loop (R5)

`gi_bake.wgsl`'s `direct_at` stops looping `params.counts.w` lights and loops the hit cell's bin
slice instead — the same table/index/light-buffer layout as `gi_inject`, built once per bake from
the static subset (a per-bake CPU cost of `O(static lights)`, invisible beside the dispatch). The
term W2 named "billions of evaluations and will not fit the ~200 ms budget" — 480 k probes × 64
rays × ~112 lights ≈ 3.4 G light evaluations — becomes ~60 M × (bin occupancy ≈ 1–3), because a
district's torches are spread and a bin cell holds few of them.

**The byte-identity argument, in full, because it is the wave's most load-bearing claim:** a light
excluded from a hit's bin cell is, by the conservative filing rule, out of range of every point in
that cell, including the cell centre where `direct_at` evaluates — so its `atten` is exactly `0.0`,
its term is exactly `+0.0` (a product of non-negatives), and removing an exact `+0.0` from an
accumulator that is itself a sum of non-negatives (never `−0.0`) leaves every partial sum
bit-identical. The surviving lights keep their relative order (bin lists ascend). This is
`cluster.rs`'s identity argument transplanted one seam inward, and it is *gated*, not trusted: the
binned-vs-degenerate oracle (§11 T8) bakes both ways through one shader and byte-compares the
volumes, W1's exact-value tests stand as the march guard, and the golden suite seals it.

### 6.2 What remains, measured not assumed

After the bin, the district bake's cost shape is `probes × rays × march_steps` (W1 measured the
whole 1.12 M-probe bake at ~60 ms on the 3090 — the march already fits) **plus the sun's per-hit
shadow march**, which the bin cannot touch and vigil's moon makes real at W4. Projection: roughly
a doubling of the march term on a sunlit/moonlit district — order ~100–150 ms, inside the ~200 ms
bake target but without margin. This wave records the projection; **W4's first district bake
measures it**, and the escalation below is designed now so that measurement leads to a decision,
not a design session.

### 6.3 The designed escalation: the per-(cell, face) sun mask — and the per-cell-normal problem's
actual answer

W2's recorded pre-pass stumbled on the per-cell surface normal (the grid stores none; the
occupancy gradient is degenerate on one-cell walls, vigil's most common wall). The answer this
wave records: **never estimate a cell normal — enumerate faces.** The quantities the bake needs at
a hit are all per-`(cell, face)`, and the face is *known exactly* (the DDA's entry axis):

- For point lights, per-face direct radiance is what the bin already made cheap — no pre-pass
  needed, problem dissolved (R5).
- For the sun, `sun_shadow(cell, n)` is already a pure function of `(cell, face)` — so a pre-pass
  that runs one DDA per *(occupied cell, sun-facing face)* (≤ 3 faces each) and stores a 6-bit
  mask (one `R8Uint` grid, ~4 MB dense at district scale) is a **memoization, byte-identical by
  construction**, turning ~tens of millions of in-bake sun marches into ~hundreds of thousands of
  load-time ones. The boundary-clamp hit (normal = `-dir`, not a face) falls back to the inline
  march — rare and stated.

Deferred with its trigger (§16): W4's measured district bake breaching the budget. Not built now
because the projection says it fits without it, and a byte-identical optimization with no caller
demand is exactly what "vertical slice before general systems" exists to stop.

### 6.4 The CPU voxelizer's 310–340 ms, the policy stated

The voxelize is load-time and signature-cached (rev 511): paid once per scene load and once per
*geometry* change, never per frame — and after this wave, marked (moving) entities cannot touch
the signature at all, which is what keeps it load-time on a live game. The policy this wave
records, so W4 inherits a position instead of a surprise:

- The plan's "~200 ms" was the **bake's** target; the honest district load bill is
  **voxelize (310–340 ms CPU) + bake (projected ≲ 150 ms GPU)** ≈ half a second, paid at load and
  on geometry edits. For `ludion dev` that sits beside a 3.1 s rebuild and is acceptable; for the
  agent's one-shot `ludion render` iteration loop it is a real per-iteration tax on district-scale
  scenes, recorded as such against the North Star.
- It is a **bake-time budget number, not a `[post_groups]`/`[passes]` line** — nothing per-frame
  measures it. W4 records the measured pair (voxelize, bake) in its plan and the notebook when the
  first district bakes, alongside the E6 run; a *mechanized* load-time gate is deferred until
  there is a second data point to calibrate a ceiling from.
- The escalations, recorded not built: §6.3's sun mask (GPU side), and a `par`-chunked voxelizer
  (CPU side — chunk the grid by z-slabs with the LWW combine order preserved per slab; real design
  work because the combine rule is order-bearing). Trigger: the measured pair breaching ~500 ms
  total, or the dev-loop cost being called out in practice.

---

## 7. Consumption — `mesh.wgsl` and `EnvParams`

**No uniform growth.** `EnvParams.gi_bounce.yzw` was reserved-zero (W2 §5); `.y` becomes the
dynamic flag — `1.0` when the pass ran and the volume is real, `0.0` otherwise — the
`ssao_direct`-rides-a-pad precedent. Group 3 gains **binding 10**, `gi_dyn_tex`
(`texture_3d<f32>`), sampled with the existing `env_samp` for the same reasons as its siblings.
That puts the fragment stage at **10 sampled textures of the guaranteed 16** (nine from W2's count
plus this one); the deferred SH-L1 upgrade would take it to 13 — counted now, per W2's rule that
the next coefficient set starts from a number. `shader_gen.rs` regenerates (`--fix`); the mesh
BGL is bindgen-reflected, so shader-drift is the gate that proves it happened.

One new function, the storage seam's third door:

```wgsl
// One-bounce irradiance from dynamic lights, RGB, no directionality (D12) — the ONE function
// that reads the dynamic volume, beside gi_sky_visibility and gi_bounce. The sample point takes
// the same normal_bias push: the volume lives on the same lattice, and the wall-interpolation
// leak it guards against does not care which term is crossing the wall.
fn gi_dynamic(world_pos: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    let p  = world_pos + n * env.gi_scale.w;
    let uv = (p - env.gi_origin.xyz) * env.gi_scale.xyz;
    return max(textureSampleLevel(gi_dyn_tex, env_samp, uv, 0.0).rgb, vec3<f32>(0.0));
}
```

The application site, nested one level deeper than W2's so an injection-free frame cannot reach it:

```wgsl
    if (env.gi_origin.w > 0.5) {
        acc = acc * mix(1.0, gi_sky_visibility(world_pos, n), env.params.w);   // W1, unchanged
        if (env.gi_bounce.x > 0.0) {
            acc = acc + gi_bounce(world_pos, n) * diffuse * env.gi_bounce.x;   // W2, unchanged
            if (env.gi_bounce.y > 0.5) {
                acc = acc + gi_dynamic(world_pos, n) * diffuse * env.gi_bounce.x;  // W3′ (R2)
            }
        }
    }
```

The W2 decisions transfer without re-litigation: added after AO and the sky multiply and attenuated
by neither; `× diffuse`, never `× albedo`; diffuse only; direct light and emissive outside. The
`+0.0` argument for the untaken inner branch is Gate B's (§2).

---

## 8. The host quality knob — `gi_dynamic`, three homes that move together

The locked §0 ruling made the dynamic layer a **host quality setting, never world state**; the
scalability arc landed the seam this week and reserved this exact switch ("GI bounce count +
dynamic-layer switch wait for their features" — the private scalability notebook). The field's three homes
(the agreed seam, 2026-08-07):

1. **`ludion_renderer::Quality`** (rev 537): `pub gi_dynamic: bool` — `FULL`/`HIGH`/`MEDIUM` true,
   `LOW` false. The struct has no `non_exhaustive`, so every literal constructor in tree updates —
   a mechanical, compiler-enumerated edit, flagged for the shared-file coordination below.
2. **The engine's TOML resolution layer** (`crates/ludion-engine/src/quality.rs` — **on disk and
   committed as of ratification**, Lore rev "Scalability W1, the resolution chain"; verified
   2026-08-07): `[effects]` gains `gi_dynamic`. That section is `#[serde(deny_unknown_fields)]`,
   so the field landing there is what makes the spelling legal. A malformed value is their `L0016`
   refuse-to-start; this wave invents no code there.
3. **The `QualityDescriptor`** on describe/ProfileReport (theirs, one schema rev "defined
   completely up front" with a single replay re-record event budgeted). **Coordination item for
   the orchestrator, worth a message before their rev lands:** `gi_dynamic` should ride *their*
   up-front descriptor definition so the arc's one schema-rev event covers it — a second rev for
   one bool would spend a three-replay re-record this wave has no budget for. If their rev has
   already landed without it when W3′ implements, the field lands in the same commit as the toggle
   and the re-record ritual is theirs to sequence, per the seam agreement.

Masking semantics (R6, restated as the implementable rule): `quality.gi_dynamic == false` removes
the `gi_inject` pass from the graph, binds the fallback, zeroes `env.gi_bounce.y`. It never
reclassifies a light: the bake's inputs are world state and quality cannot reach them. The masked
frame is byte-for-byte the no-marked-content code path (Gates A/C), and the marked lights keep
their full direct contribution — "off" costs nothing and changes no sim-visible thing.

Test-side seam: fixtures drive the renderer-level `Quality` directly; the CLI/env spelling
(`--quality`, `LUDION_QUALITY`) is the scalability arc's surface and is exercised there, not
duplicated here.

---

## 9. The calibrator, and the stress scene

### 9.1 `gi-interior` — one line, again

The mover was authored in W1 with this wave named in the header ("W3's injection testbed"), kept
out of the doorway wedge and the sealed corner so it cannot flatter the read. The change:
`GiDynamic: {}` on the mover, plus the header comment rewritten to describe the new state (a
comment describes state, never schedule — the current "W3's injection testbed" phrasing retires
with the wave that lands it). Three consequences land in one diff, and the golden move records all
three (R7): the mover's light leaves the static bake, its emissive sphere leaves the voxelization
(the corner's faint blue brightening W2's header attributes to it will dim/move — expected, named
in the diff report), and the injected term appears at the default quality. `WorldHash` moves for
this scene (§3.3); nothing else committed authors the marker.

Watch item for the re-bless, recorded so it is checked rather than drifted past: the mover's
authored `intensity 0.35 / range 3.5` was tuned as a *baked* light. If the injected read is too
faint or too hot at K = 16, the retune is a sweep-sheet call
(`--sweep /entities/8/components/PointLight/intensity=…`) with the criterion stated (the pool must
read as light, the sealed corner must hold), not taste — the W1/W2 discipline.

### 9.2 `gi-inject-stress` — the cap, gated (D13's other half)

New built-in-registry scene `examples/gallery/gi-inject-stress.yaml`: a floor-and-walls field with
a `GiVolume` (`bounce: 1.0`) and **300 marked short-range lights** — the `MAX_PROJECTILES` cap as
a static layout, the `budget-horde` posture applied to injection, deliberately including both a
clustered patch (the §9.1 burst regime the bin's ceiling exists for) and a spread field (the
regime D13 says is real). It ships:

- A first-bless Windows golden (960×540) — the per-frame injection under per-OS golden gating at
  scale, and the visible proof the term is bounded, not blooming.
- `budgets/pinned-rtx3090-gi-inject-stress.toml` — the `gi_inject` `[passes]` ceiling calibrated
  off the measured median (plus the scene/cluster lines a budget file must carry to be checked
  against the whole run — the sibling-file precedent). Budget breaches reuse `L4005`; no new code.
- The home of §11 T10's cross-process fixture and the cap-regime measurement §5.5 owes.

The macOS queue gains: `gi-interior` re-bless, `gi-inject-stress` first bless, and a *measured*
`pinned-apple-m4-gi-inject-stress.toml` (never invented — the wave-11 rule). Vigil's own horde
numbers under GI land at W4 with the district adoption, where E6 already runs; the E6 `gi_inject`
line is pre-declared there rather than faked here.

---

## 10. The instrument — `--dump-gi` grows a DYNAMIC block

The sheet gains one block beside VISIBILITY, BENT NORMAL, and BOUNCE:

- **DYNAMIC** — the dynamic volume's RGB DC at the BOUNCE block's fixed-exposure Reinhard (one
  exposure convention across both radiance blocks, so a sheet reads as one instrument), with the
  volume's actual peak burned into the label. When the term is absent the block **says why**, never
  draws a black block: `not injected (quality: gi_dynamic off)` / `(no GiDynamic lights)` /
  `(bounce: 0.0)` / `(no GiVolume)` — the reason is the single most likely question an agent brings
  to the sheet.
- The sheet header gains the split census: `lights: N baked + M injected`, and the bake-cache
  status line — the one-glance answer to "is something thrashing the bake," the diagnosis §3.1
  promises for a forgotten marker.

Plumbing: the readback path renders one frame's injection input the same way a real frame does
(the dump is CLI-side and the CLI renders at the default quality — FULL — so the term is present
unless content says otherwise); `GiReadback` grows `dynamic: Option<Vec<[f32; 3]>>`. `L2104`
covers the no-`GiVolume` case unchanged; the GPU requirement in the help text is unchanged. No new
L-code.

---

## 11. Tests

The arc's fixture style: every assertion names the quantity it measures, every leak/step gate is
demonstrated able to fail with the mutation and the reddened numbers recorded in the commit. New
engine fixtures live in `crates/ludion-engine/tests/gi_dynamic.rs` (the `gi_bounce.rs` machinery
restated — integration binaries share no module); it joins **both** GPU keep-in-step lists
(`.config/nextest.toml`, `provision.mjs`) at birth, the rev-518 lesson.

1. **Gate A — unused content, both toggle states.** `meshes.yaml` (no marker) renders
   byte-identical before/after the wave and across `gi_dynamic` true/false; the graph schedules no
   `gi_inject` pass (pass-count assertion, the fog precedent). Fails if the new binding, branch, or
   toggle perturbs the untaken path — the medium-W3 codegen finding's gate.
2. **The pass exists exactly when it should.** On a marked scene: pass present at quality-on,
   absent at quality-off and at `bounce: 0.0`; `dump-graph` (scene-derived since post wave 0) is
   the assertion surface. Explicit non-goal, recorded in the test's doc: masked-vs-unmarked byte
   equality is *not* asserted, because the marker's bake exclusion is world-state semantics —
   §2 Gate C is the contract.
3. **Gate B — the additive identity through live bindings.** A marked light in open space, out of
   range of every surface: quality-on and quality-off render **byte-identical**, with the
   non-vacuity guard that the dynamic volume was allocated and written (all zeros). Fails if the
   pass, bindings, or the `+0.0` argument leak a bit.
4. **The injection lights the room, and only the room.** A sealed two-room fixture, a marked light
   in room A: a floor patch beside it brightens by an asserted ratio between quality-off and
   quality-on; room B holds within tolerance. **Demonstrated able to fail**: mutate the gather to
   ignore the hit cache (accumulate each light's term at the probe directly, no visibility) and
   record room B reddening. This is W2 T8's twin and the charter's "leaks are inherited-solved"
   made falsifiable.
5. **Exactly one term — the double-count gate, both halves.** Fixture with an unmarked torch and a
   marked lamp: (a) the *static* volumes byte-match a bake of the same scene with the marked lamp
   deleted (readback compare — exclusion is exact); (b) unmark the lamp ⇒ static volumes move and
   the pass disappears; (c) **moving the marked lamp is a bake cache hit** (`bake_count` stable —
   the R3 thrash debt's headstone) while moving the torch still re-bakes (the unchanged W2
   consequence, asserted so it stays a decision).
6. **Energy agreement — the unit gate.** One light, same position/intensity, baked (unmarked) vs
   injected (marked): the probe-readback DC of the injected term agrees with the static bounce's
   DC at the same probe within a stated tolerance band (K = 16 vs 64 variance bounded by fixture
   geometry — an enclosing-shell-style arrangement where every prefix ray hits). Fails if the
   `1/K` normalization, the premultiply, or the albedo factor drifts from the bake's convention.
7. **Smoothness — no step at a bin boundary.** Sweep a marked light across a bin-cell boundary in
   sub-cell steps (renderer-level, probe readback): the per-step DC delta stays under a bound
   derived from the analytic slope. **Demonstrated able to fail**: a mutated bin that files each
   light only into its centre cell produces a step at the boundary — the gate that keeps §5.4 an
   argument with teeth.
8. **The bin oracle — binned vs degenerate, both consumers.** A `cfg(test)` degenerate bin (every
   light in every cell) run through the same shaders: the static bake's volumes byte-match (R5's
   proof), and the injected volume byte-matches (conservatism's proof). The `cluster.rs`
   decision-7 discipline, transplanted.
9. **The static refactor moves nothing.** W1's exact-value tests (`33/64`, `1.0`, `[0.0; 4]`)
   unchanged; every W2 fixture green unchanged; the whole golden suite byte-identical at the
   refactor commit (before the calibrator commit — §2 Gate D's ordering). A movement here is a
   defect, never a re-bless.
10. **Cross-process byte identity at the cap — the D11 pin.** Two separate `render` processes on
    `gi-inject-stress` produce byte-identical images. Lives in `ludion-cli::render_gi`
    (`CARGO_BIN_EXE` — the rev-518 in-test-cargo-build race is the recorded reason). Per-machine
    claim, explicitly not cross-platform.
11. **`--dump-gi` shows the term and the census.** DYNAMIC block bright near the marked light;
    `not injected (…)` with the correct reason on each of the three absent cases; the
    `N baked + M injected` line correct on a mixed scene.
12. **Quality plumbing round-trips.** Renderer presets (`LOW` false, others true); the `[effects]`
    TOML spelling accepted and an unknown key still refused (their `deny_unknown_fields` gate,
    exercised not duplicated); the descriptor carries the field (or the same-commit rule fires —
    §8). Split across the owning crates' test homes.
13. **A marked mesh neither occludes nor deposits, and its motion is free.** A marked cube behind
    which visibility stays open, whose emissive stays out of the bounce bake, and whose *motion
    does not re-voxelize* (`rasterization_count` stable — the mesh-side twin of T5c, and the test
    that makes W4's five thousand enemies safe).
14. **Registry and hash discipline.** `GiDynamic` registered append-only (tripwire updated, every
    existing index stable); schema-drift regenerated; the §3.3 search re-run proving no committed
    replay or `sim-hash` reference moves; snapshot round-trip carries the marker (the generic
    component gate, named here so the checklist has the line).

Plus full `node scripts/check.mjs` green, every existing golden byte-identical except the two R7
names, no `run.lrp` re-record, wire surface unchanged at 27 methods.

---

## 12. Risks

- **Estimator variance at K = 16 — the mottle, squared.** W2's recorded finding (visible mottle at
  64 rays on flat surfaces) applies to a 16-ray estimate with more force. Three mitigants are
  structural — the term is faint (D13), irradiance-only storage low-passes it, and the pattern is
  *static per probe* (fixed rays, so motion of the light cannot make it crawl) — but the read is a
  sitting call. `K` is the knob, its cost is linear, and the sitting note rides the re-bless.
- **The adversarial cap on the M4.** 300 clustered lights × deep bin lists is the regime D13 says
  play does not produce but the code ceiling permits. The budget gate is sized at exactly that
  regime (§9.2); if the measured M4 number is unacceptable *at the cap while organic play is
  fine*, the recorded escalation is §16's aggregation, decided on that measurement — not a reason
  to pre-build it.
- **The forgotten marker.** A game that spawns moving lights or meshes without `GiDynamic` gets
  correct pixels and a per-frame bake/voxelize — the failure is a performance cliff with no wrong
  image, which is the hardest kind to notice. Mitigation is the instrument (§10's census + the
  profiler showing the bake), the docs stating the rule in the component's own entry, and W4's
  adoption checklist naming every vigil archetype. Accepted under R1; a diagnostic cannot fire
  without frame-to-frame comparison the render path is forbidden.
- **Frame-graph scope creep (WP-3).** `PassKind::Compute` + 3D/storage `TextureDesc` is real work
  in a crate below the renderer with hand-written-BGL exceptions of its own. The package is
  bounded to R4's two items — no persistent resource kind, no MRT, no graph-owned buffers — and
  its guard is the existing graph test suite plus the unchanged `frame_graph_64x64.json` snapshot
  (the base graph schedules no compute pass).
- **Shared-file coordination.** `crates/ludion-renderer/src/quality.rs` and the engine quality
  layer are the scalability instance's active surface, and the private macOS queue plus the
  image-stats budgets are other instances' dirty files. Every touch point is flagged in §13; the
  orchestrator sequences (and messages via exarch) before commits — the standing shared-worktree
  rule.
- **Codegen perturbation.** A new group-3 binding, a new nested branch, and a new store in the
  bake loop (the hit cache) can each move an untaken path's pixels — the medium-W3 finding, three
  times over. Gates A/D and T9 are the measurements; the escape for the mesh branch is the same as
  W2's (rely on the `+0.0` identity without the inner condition).
- **The boundary-clamp face.** Hits whose normal is `-dir` (face code 6) shade with a non-axis
  normal; the sun-mask escalation cannot memoize them and the gather must recompute
  `ray_direction(k)`. Rare by construction (probes on grid faces), handled explicitly, and the rim
  test (W1's) plus T6 cover the arithmetic — but it is the design's fiddliest corner and earns its
  named place here.
- **Group-3 headroom.** 10 of 16 sampled textures after this wave; the deferred directional
  upgrade takes 13. Counted (§7); the wave after that one starts from a number, not an assumption.

---

## 13. Work packages

Dependency-ordered; the files listed are the **ownership boundary** — no two packages in one wave
share a file. Integration, `check.mjs`, the golden byte-identity sweep, exarch coordination on
shared files, and the commits are the orchestrator's.

- **WP-1 — the marker, end to end, renderer-blind.** `crates/ludion-engine/src/components.rs`
  (`GiDynamic`, registration append-only, docs), `crates/ludion-engine/src/extract.rs` (the tag on
  `PointLightData`, the voxelizer-input + signature exclusion), `crates/ludion-renderer/src/scene.rs`
  (`PointLightData.dynamic`), `crates/ludion-engine/src/voxelizer.rs` (skip marked instances).
  Tests: §11 T13, T14, and the schema/registry gates. Nothing renderer-side consumes the tag yet;
  no pixel can move. **sonnet** — fully specified. *Wave A.*
- **WP-2 — the bin and the static bake's adoption.** `crates/ludion-renderer/src/gi_bin.rs` (new:
  the counting-sort bin, both-consumer API, the degenerate `cfg(test)` variant),
  `crates/ludion-renderer/src/gi_bake.rs` (the static-subset filter moves into `BakeInput`
  construction's caller contract, the bin upload, the hit-cache buffer + cache plumbing, the
  signature over the static subset), `crates/ludion-renderer/src/shaders/gi_bake.wgsl` (`direct_at`
  over the bin slice; the hit-cache stores), `crates/ludion-renderer/src/render.rs` (**the
  `BakeInput` partition only** — the W2 WP-2 scoping rule). Tests: §11 T5 (bake halves), T8
  (static half), T9. **opus** — the byte-identity-bearing package. *Wave B.*
- **WP-3 — the frame graph learns compute.** `crates/ludion-render-graph/*`: `PassKind::Compute`,
  executor dispatch + timings attribution, 3D extents + `StorageBinding` on `TextureDesc`,
  validation, snapshot untouched. No renderer changes. Guarded by the graph's own suite. **opus**
  — foreign crate, architectural. *Wave B, parallel with WP-2 (disjoint crates).*
- **WP-4 — the injection and its consumption.** `crates/ludion-renderer/src/shaders/gi_inject.wgsl`
  (new), `crates/ludion-renderer/src/gi_inject.rs` (new: pass setup, per-frame uploads, the
  scheduling predicate), `crates/ludion-renderer/src/render.rs` (graph declaration, recorder,
  `env_bind_group` binding 10, `env.gi_bounce.y`), `crates/ludion-renderer/src/shaders/mesh.wgsl`
  (`gi_dynamic` + the application site), `crates/ludion-renderer/src/quality.rs` (the field + the
  presets — **shared-file flag: the scalability instance**), `crates/ludion-renderer/src/shader_gen.rs`
  (regenerated), `KNOWN_PASS_NAMES`. Tests: §11 T1–T3, T8 (dynamic half) in the existing GPU
  homes. **opus** — the pixel-moving edge. *Wave C (needs WP-2 + WP-3).*
- **WP-5 — quality's other two homes.** `crates/ludion-engine/src/quality.rs` (`[effects]
  gi_dynamic` — on disk and committed as of ratification; **shared-file flag: the scalability
  instance**), the `QualityDescriptor` field per §8's coordination outcome. Tests: §11 T12.
  **sonnet**, with the coordination note in the brief. *Wave D.*
- **WP-6 — the calibrator.** `examples/gallery/gi-interior.yaml` (the marker + the header
  rewrite), the re-blessed golden + `Plan/artifacts/gi-wave3-gi-interior.diff.json`, the §9.1
  intensity check (sweep sheet only if the read demands it, criterion stated), the macOS queue
  entries. **sonnet.** *Wave D, parallel.*
- **WP-7 — the stress scene and its gates.** `examples/gallery/gi-inject-stress.yaml` + first-bless
  golden, `budgets/pinned-rtx3090-gi-inject-stress.toml` (measured, calibration prose), §11 T10 in
  `crates/ludion-cli/tests/render_gi.rs`, the §5.5 measurements recorded. **sonnet.** *Wave D,
  parallel.*
- **WP-8 — the instrument.** `crates/ludion-engine/src/voxel_sheet.rs` (DYNAMIC block + census),
  the engine dump path, `crates/ludion-cli/src/render.rs` help, §11 T11 tests. Consumes WP-4's
  readback shape. **sonnet.** *Wave D, parallel.*
- **WP-9 — the physics fixtures.** New `crates/ludion-engine/tests/gi_dynamic.rs`: §11 T4, T6, T7,
  each with exact geometry and the able-to-fail mutations specified in the brief; both keep-in-step
  lists. **sonnet.** *Wave D, parallel.*
- **WP-10 — docs.** `docs/global-illumination.md` (the marker, the mark-what-moves rule, the
  quality knob, what `low` costs), a new `dev-docs/renderer-gi-dynamic.md` spoke (the bin, the hit
  cache, the injection pass, the determinism obligations — `renderer-gi.md` and
  `renderer-gi-bounce.md` are near their size budgets, so the wave gets its own spoke rather than
  overstuffing one), `dev-docs/renderer-clustering.md` cross-note (the bin-shape inheritance),
  `ludion-render-graph` docs for compute passes. Through the `ludion-docs` skill; `--fix`
  re-mirrors. **sonnet.** *Wave D, parallel.*

Waves: **A → {B: WP-2 ∥ WP-3} → C → D** (five parallel packages). Wave B's two packages are
crate-disjoint; wave D's six are file-disjoint.

---

## 14. Definition of done

- `node scripts/check.mjs` green (schema-drift, shader-drift, embedded-docs mirror all fired and
  re-fixed); the vitest statue-gemma reds remain the only recorded exception if still present.
- Every existing golden byte-identical **except** `gi-interior.png` (re-blessed, SSIM diff report
  committed) and the new `gi-inject-stress.png` first bless; macOS queue carries both plus the
  measured M4 stress budget file.
- Gates A–D (§2) pass as measured claims; §11 complete with T4 and T7 **demonstrated able to
  fail**, mutations and reddened numbers in the commits.
- §11 T5/T13's debt assertions green: a moving marked light and a moving marked mesh invalidate
  nothing; the static bake is a cache hit under both.
- The bin oracle (T8) green on both consumers; W1's exact values untouched.
- `gi_inject` visible in `--dump-timings` and the profiler's GPU section on a marked GI scene;
  its `[passes]` ceiling committed with calibration prose; the §5.5 3090 + M4 measurements
  recorded (M4 via the queue if the mac is not at hand).
- `--dump-gi` on `gi-interior` shows the DYNAMIC block lit at the mover and the correct census;
  the three absent-reasons each verified once.
- The group-3 sampled-texture count (10 of 16) and the hit-cache memory line recorded in the
  commit; the `MAX_GI_PROBES` doc comment updated with both.
- No `run.lrp` re-record, no `sim-hash` reference moved (§3.3 re-verified), wire surface unchanged
  at 27 methods; zero new L-codes, stated in the closing commit.
- The quality field present in all three homes (or the same-commit rule exercised and recorded);
  the exarch coordination with the scalability instance logged.
- `Plan/global-illumination.md` §9.10's W3′ paragraph marked delivered with corrections; D10–D12
  marked resolved in §0's ledger; this file's Status filled in; the graphics-engine notebook gains
  the W3′ Done entry (outcomes only) and its TODO's W3′ line retired.

---

## 15. Micro-decisions deferred into implementation (record the answers in the commit)

- The bin's fixed dimensions (`32 × 16 × 32` is the starting point) and the exact filing margin;
  whether a degenerate-thin volume axis wants a floor.
- The hit-cache packing's final bit layout, and whether `ESCAPED` folds into the face code (7) or
  a sentinel word.
- Whether the injection pass takes a per-probe early-out on an empty bin *neighbourhood* or relies
  on the per-hit empty-slice skip alone — decided by the stress measurement, not in advance.
- The DYNAMIC block's shared fixed exposure — confirm the BOUNCE constant reads for a faint term
  or record the divergence.
- Whether `gi-interior`'s mover intensity retunes (§9.1), off a sheet with the stated criterion.
- Whether `MEDIUM` keeps `gi_dynamic: true` once the scalability arc measures presets on the
  laptop — their call, flagged in the field's doc.
- Where exactly the static-subset filter lives (`prepare_frame` partition vs a `BakeInput`
  constructor) — WP-2's boundary decision, whichever keeps `render.rs`'s diff smallest.
- Whether the stress scene's clustered patch and spread field are one scene or a `--set` variant —
  one scene preferred (two regimes, one golden), decided by what the golden legibly shows.

---

## 16. Explicitly deferred, with triggers

- **Stage-1 radiance aggregation** (§9.3's per-cell virtual-source collapse). Trigger: the
  measured `gi_inject` cost at the cap regime breaching its ceiling on either pinned GPU. Its
  determinism note is pre-written in §4.3: aggregation makes bin assignment value-bearing, so it
  arrives with the cross-process fixture as its gate and ascending-order accumulation as its rule.
- **A directional dynamic term** (SH-L1, three more volumes, bindings 11–13 of 16). Trigger: the
  sitting calls the both-sides-of-a-wall glow on a marked light, or content authors a dynamic
  light bright and long-lived enough to expose it.
- **Dynamic emissive surfaces as injection sources** (marked enemies' emissive glow lighting walls).
  Trigger: called by eye on the dressed build; the design space is a per-marked-instance virtual
  light derived at extract (the §9.8 shape), never a per-frame re-voxelization.
- **The per-(cell, face) sun-visibility mask pre-pass** (§6.3, designed, byte-identical by
  construction). Trigger: W4's first measured district bake breaching the bake budget.
- **A `par`-chunked CPU voxelizer** (§6.4). Trigger: the measured load pair breaching ~500 ms
  total, or the one-shot `render` iteration tax on districts biting in practice.
- **`gi_rays` / `gi_probe_scale` / a `K` quality axis** — the scalability arc's W2, by that plan's
  own reservation. This wave leaves `RAYS` and `K` shader constants.
- **§9.8's analytic ground-bounce lights** (W3.5′). Trigger: the injected term reads too soft at
  contact scale at the sitting — the recorded highest-value-per-line fallback.
- **A dynamic sun / day-night cycle in GI.** Trigger: content that moves the sun; the recorded
  answer is §8's static-half amortization proposal, judged then under its own D7/D8 costs.
- **Bake-time budget mechanization** (§6.4). Trigger: W4's measured pair exists to calibrate from.
