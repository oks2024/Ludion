# Plan: Clustered Forward — the cap removal (the many-lights arc)

> Status: **MECHANIZED-COMPLETE 2026-08-06 (user-directed the same day; Lore revs 492–497, wave log
> at the end). All four waves delivered; what remains is wave 3's human gate — the torch-lit night
> city read on the next sitting (streets leading the eye, hearths through doorways, tracer light on
> cobblestones, the `moon-fill` 0.5 confirm, the hearth leak-vs-glow call that decides the
> point-shadow trigger, and beacon dominance which only a live run can show).** It belongs to the
> graphics-engine feature stream (the `shadow-mapping.md` /
> `volumetric-medium.md` shape — a standalone plan, not a numbered engine phase).
>
> **This plan fires a recorded trigger, it does not reopen a decision.** `Plan/post-process-pipeline.md`
> decision 6 (settled with the user 2026-07-25) rejected deferred shading as a *direction* and named
> clustered forward "the recorded answer if light count ever bites, measured not assumed," with the
> trigger "content that provably exceeds what the cap plus nearest-16 selection can express." The
> user set that content constraint on 2026-08-01: torches in the streets, near the houses, inside
> the houses, and **light-emitting projectiles** — a district will author scores of static lights
> and a horde frame will carry a dynamic population the cap cannot express and the nearest-16
> selection would visibly pop. The deferred-vs-forward question was re-examined the same day
> (including "should a multi-genre engine carry both?") and re-settled: **one lighting architecture.**
> Two peer modes would put a second contract under every present and future screen-space effect,
> double the golden/budget verification matrix at its most expensive point, and violate "before
> adding a second way to do something, remove the first" — for a genre band that clustered forward
> demonstrably covers (the engine's AA is SSAA, whose supersampled G-buffer bandwidth punishes
> deferred quadratically; the content style is sprite/alpha-heavy, which needs a clustered forward
> path under deferred *anyway*). The falsifier stays recorded: deferred earns a plan only if the
> material model grows genuinely expensive per-pixel shading (textured PBR, decal layers) **and**
> opaque overdraw stays high despite a Z-prepass — neither trigger has fired.
>
> **Relationships:**
> - Decision 5 of a private plan ruled "raising `LIGHT_COUNT` or clustered
>   lighting is explicitly out of scope" — a scope ruling for that arc, answered here, not
>   contradicted.
> - `Plan/global-illumination.md` §9.5 orders the arcs: **this plan first**. What GI's dynamic
>   injection and the medium's point-light in-scatter share with this plan is the deterministic
>   global light array (one EntityId-ordered list of every light, position/radiance/range) and the
>   binning code shape — *not* the view-space cluster grid itself, which is this consumer's own
>   index. See decision 8 for the seam, stated precisely so §9.5's "build it once" is honored
>   without pretending a view-frustum structure can serve a world-space consumer.
> - `Plan/global-illumination.md` §9.8's "analytic ground-bounce lights" ride the path this plan
>   builds; nothing here needs to anticipate them beyond the light array being game-writable, which
>   `PointLight` on an entity already is.

## Why (the one-paragraph case)

The renderer lights every mesh and sprite fragment from a fixed 16-slot uniform array
(`lights.wgsl:7`, `shader_gen.rs:21`), fed by a deterministic nearest-16 selection in extract
(`extract.rs:422-465`) that today is dormant — no committed scene exceeds 10 lights. The moment a
district authors torch-lit streets and interiors, the cap truncates authored content; the moment
projectiles emit light, the selection *pops* — lights enter and leave the chosen 16 as the player
moves, a visible artifact with no good tuning. Clustered forward removes the cap instead of raising
it: all lights ride a storage buffer, the screen is divided into a fixed grid of frustum cells, each
cell carries the short list of lights that can reach it, and a fragment iterates its own cell's list
instead of the global 16. Cost becomes proportional to *lights actually near each pixel*, the
selection machinery is deleted (one way, not two), and the per-frame light population — hundreds of
short-lived projectile lights included — is bounded only by measurement, not by a compile-time
constant.

## What exists today (the inventory this plan builds on)

- **`PointLight`** (`crates/ludion-engine/src/components.rs:186`): `color`/`intensity`/`range`,
  position from `Transform`. No semantic validator — plain `register`, schema-only rejection.
  **This plan does not touch it**: zero new authoring surface.
- **The uniform**: `Lights { count, pads, ambient, items: [Light; 16] }`, 544 bytes, group 1
  binding 0 (`shader_gen.rs:242-269`, `593-595`), uploaded per frame by `build_lights`
  (`render.rs:1104-1131`, premultiplies `color·intensity`, truncates to 16).
- **The consumers**: exactly two, via the shared snippet `shaders/snippets/lights.wgsl` —
  `mesh.wgsl`'s `shade()` loop (mesh.wgsl:229-247; Cook-Torrance, attenuation
  `clamp(1 − dist/range, 0, 1)²` — **exactly zero at or beyond `range`**, which decision 3 leans
  on) and `sprite.wgsl`'s `shade_two_sided` (sprite.wgsl:50-63, same attenuation). Text/UI/prepass
  are unlit and untouched.
- **The selection**: `select_lights` (`extract.rs:422-436`) engages only above 16; at or below, the
  list passes through in ascending-EntityId extract order — a deliberate byte-identity invariant
  this plan preserves as the global light order.
- **Group budget**: the mesh pipeline uses all four bind groups (0 camera, 1 lights, 2 sun/shadow,
  3 env/AO — survey of `shader_gen.rs`/`render.rs:1680-1683`). There is no free group index, so the
  cluster data must grow **bindings on group 1**, not a new group. Sprite uses 0/1/2; group 1 is
  the lights group in both, so one snippet serves both again.
- **Firsts**: the workspace has **no storage buffer anywhere** (`var<storage` greps to zero) and
  the frame graph has no buffer resource kind. This plan introduces the storage-buffer binding but
  deliberately **not** a graph buffer resource or compute pass — the cluster build is CPU-side
  (decision 3), so the graph is untouched.
- **The light loops are ALU-only** — no texture sample inside either loop (the sun's PCF sits
  outside). That is what makes a non-uniform per-cluster loop bound legal WGSL, and it must stay
  true (Risks).

## The design

Three GPU-visible pieces, all group 1, all built CPU-side each frame:

1. **The light array** — `var<storage, read> light_list: array<Light>`; the existing 32-byte
   `Light` (position.xyz + range.w, premultiplied color), in ascending-EntityId extract order,
   unbounded. `select_lights` and the truncation are deleted.
2. **The cluster grid** — a fixed-count screen-space grid: `TILES_X × TILES_Y` tiles ×
   `SLICES` radial-distance shells (initial shape 16×9×16 = 2304 cells; a tunable const, measured
   in wave 2). *Fixed count, not fixed pixel size*, so the grid is identical at every `--scale` —
   scale invariance for free under the wave-3 extent rule. Slices are **radial distance from
   `cam_pos`** on an exponential ladder, not view-space Z: the camera uniform carries no view
   matrix (`Camera` is `view_proj` + `cam_pos`, shader_gen.rs:86-92), distance needs only
   `cam_pos`, and the conservatism decision 3 requires is easier to state for a sphere-vs-shell
   test than a sphere-vs-z-plane one. Ladder parameters ride the header uniform.
3. **The cluster tables** — `var<storage, read> cluster_table: array<vec2<u32>>` (offset, count
   into…) and `var<storage, read> cluster_lights: array<u32>` (light indices, ascending within
   each cell). The header (`ambient`, total count, ladder params, inverse render extent for the
   tile mapping) stays a small uniform at binding 0, so the snippet's shape is: header uniform +
   three storage buffers, one group, both shaders.

A fragment maps `@builtin(position)` → tile, `distance(cam_pos, world_pos)` → shell, fetches its
cell's (offset, count), and iterates that slice of `cluster_lights` in stored order.

**The identity argument (the plan's load-bearing proof, the spatial-index shape).** The clustered
loop is *arithmetically identical* to today's flat loop, by two properties together:

- **Per-set conservatism**: the CPU build must guarantee every light whose `range`-sphere
  intersects a cell appears in that cell's list. A light the test admits *spuriously* (conservative
  over-inclusion) contributes exactly `+0.0` to the fragment's accumulator — the attenuation is a
  hard zero at `dist ≥ range` and `acc + 0.0` is bit-preserving — so over-inclusion cannot move a
  pixel. A light the test *misses* is a visible hole, which is what the oracle fixture exists to
  catch (wave 2).
- **Total order**: within a cell, indices ascend, and the global array is extract order — so a
  fragment sums its contributing lights in exactly the order the flat loop summed them. Float
  addition order is preserved; no re-association.

What the argument does **not** cover is codegen: swapping a fixed uniform array for storage-buffer
indirection changes the generated shader for every mesh/sprite pipeline, and the volumetric-medium
wave 3 finding stands — a byte-identity claim is about generated code, not just values. Decision 6
takes the posture.

## Decisions

1. **One architecture: clustered forward. Deferred stays rejected; no dual mode, ever.** Restating
   post-process decision 6 with the 2026-08-01 re-examination folded in (see the status header for
   the argument and the falsifier). A future deferred path, if its trigger ever fires, arrives as
   its own plan and *replaces* — it does not coexist as a mode toggle.
2. **The cap and the selection are deleted, not raised.** `LIGHT_COUNT`, the 16-slot array,
   `select_lights`, and `build_lights`' truncation all go. Raising the constant was rejected in
   decision 5 of a private plan for per-fragment loop cost, and it keeps the pop
   artifact; the cluster path makes both the constant and the selection a second way to bound light
   cost. What survives is the *order* the selection protected: ascending-EntityId extract order is
   now the global array order, load-bearing for the identity argument.
3. **The cluster build is CPU-side, in the renderer's GPU-free prologue — a GPU compute build is
   deferred behind a measurement.** CPU build is deterministic by construction (plain `f32`
   arithmetic, the `dist2` discipline from `extract.rs:441` — scalar ops, no fused intrinsics),
   needs no graph buffer resources, no compute pass kind, no WGSL-side ordering proof, and at the
   target scale is small: a few hundred lights × a conservative cell-range each is tens of
   thousands of index appends. It lives in the renderer (not extract) because clustering is a
   property of how the renderer *consumes* lights, not of what the scene contains — extract keeps
   answering "which lights exist, in what order," the same split `PostChain::for_scene` draws.
   `ludion_core::par` (`par_map_chunks` / `ordered_reduce`) is available if the build ever measures
   hot, thread-count-invariant by design. Trigger for a GPU build: measured CPU cost > 0.3 ms at
   the wave-2 stress scene's population.
4. **Fixed cluster *count*, radial shells, ladder in the header uniform.** 16×9×16 initially —
   tunable, re-measured, never per-scene. Fixed count makes the grid `--scale`-invariant; radial
   slicing needs only `cam_pos`, so the `Camera` uniform does not grow. The shell ladder is
   exponential over a fixed distance range initially (constants in the header); a per-frame fitted
   range is deferred until wasted shell resolution *measures* as list bloat.
5. **Everything rides group 1.** The mesh pipeline has no free group; group 1 is already "the
   lights" in both consumers; the snippet grows bindings (header + three storage buffers), and both
   shaders keep composing one `Snippet::Lights`. Bindgen must learn `var<storage, read>` and
   runtime-sized arrays (a first — reflection, `min_binding_size` = one element's stride, BGL
   factory emission); the same-name-same-layout unification rule extends to storage structs
   unchanged.
6. **Byte posture: unlit pipelines byte-identical; lit pipelines arithmetically identical with ONE
   pre-authorized pixel-moving wave (wave 1), and wave 2 byte-identical to wave 1 by
   construction.** The shadow-mapping precedent. Wave 1 lands the *final* shader shape (cluster
   indirection) with a degenerate build that files every light into every cell — so the one codegen
   change happens once, and the re-bless (if bytes move at all; the arithmetic says LSB-scale or
   nothing) is paid once, with SSIM diff reports committed and macOS queued. Wave 2 then changes
   only the CPU build, shaders untouched — its gate is *byte identity* against wave 1 on every
   golden, which is exactly the identity argument made checkable. The degenerate build survives
   forever as the test oracle (decision 7), not as a shipping mode.
7. **The oracle is the degenerate build, and it is the plan's conformance gate.** A randomized
   fixture (the spatial index's brute-force-oracle pattern): N random lights, random camera, render
   once with the real build and once with the all-lights-in-every-cell build through the *same*
   pipeline — byte-compare. Any conservatism hole (a reaching light culled) shows as moved bytes;
   any spurious admit is proven harmless by the same comparison passing. Runs at several populations
   including above-cap counts the old path could never express.
8. **The seam with GI (§9.5), stated precisely: what is shared is the light *array* and the binning
   code shape, not the grid.** This plan's grid is view-frustum-space — the right structure for a
   camera pass and the wrong one for GI's world-space injection or the medium's in-scatter. Those
   consumers bin the *same deterministic array* into their own world-space cells when their arcs
   land (GI §9.3's counting-sort/gather discipline matches decision 3's). What this plan owes them:
   the array's order contract (ascending EntityId), its premultiplied-radiance convention, and the
   `Light` struct staying the single wire format for "a light, GPU-visible." No world-space
   structure is built here — building one now, unconsumed, would be the speculative generality the
   graph's MRT rule already rejected once.
9. **No new authoring surface, and — expected — no new L-code (the arc tradition, tenth-plus wave
   running).** `PointLight` is untouched; a scene that authored 10 lights renders identically-
   valued; a scene may now author 200. No per-scene light cap is invented: the storage buffers grow
   with content, the profile budgets bound the *cost*, and the cluster-occupancy introspection
   (wave 2) makes an absurd population diagnosable rather than rejected. If a hard ceiling ever
   proves necessary it arrives with a diagnostic and a measured justification, not as a guess here.

## Waves

### Wave 1 — the cap falls (storage lights, final shader shape, degenerate build)

- Bindgen: reflect `var<storage, read>` buffers and runtime-sized arrays; emit BGL entries with
  `min_binding_size` = element stride; extend the unification test set. `shader-drift` regenerates.
- `lights.wgsl` snippet rewritten to the final shape: header uniform (ambient, count, ladder,
  inverse extent) + `light_list` + `cluster_table` + `cluster_lights`; both loops (`shade`,
  `shade_two_sided`) become cluster-slice loops. `LIGHT_COUNT` deleted from WGSL and `shader_gen`.
- Renderer prologue: build the light array (extract order, premultiply as today) and the
  **degenerate** cluster tables (every light in every cell — the conservative extreme). Delete
  `select_lights` and the truncation; keep the ascending-EntityId order contract where the
  selection's doc comment kept it, with the why moved onto the array builder.
- **Tests:** an 18-light fixture where the 17th and 18th lights *visibly contribute* (the exact
  scene shape `select_lights_keeps_nearest…` used, now asserting the opposite outcome); ≤16-light
  frames measured against pre-wave goldens (LSB-scale expected — re-bless once with reports if
  bytes move, macOS queued); cross-process byte identity; sprite parity (a lit-sprite fixture above
  the old cap); text/UI goldens byte-identical (their shaders never composed the snippet).
- **Budgets:** re-measure `scene` on `lights.yaml` against its 0.06 ceiling (1.2× headroom — the
  indirection must be paid for honestly; a breach is engineered away or the ceiling moves with its
  calibration prose rewritten, never silently widened).

### Wave 2 — the real build (culling arrives; byte-identical by proof)

- The CPU binning: per light, screen-space AABB of the range-sphere → tile range (behind-camera /
  near-plane-spanning lights take the conservative full-tile fallback), radial interval
  `[d − range, d + range]` → shell range; append index to every covered cell; lists ascend by
  construction. `par_map_chunks` only if profiled hot.
- Shaders untouched. **Gate: every golden byte-identical to wave 1** — the identity argument,
  checked, not argued.
- **The oracle fixture** (decision 7): randomized clustered-vs-degenerate byte-compare at several
  populations (e.g. 8 / 64 / 300 lights), several cameras. This is the wave's conformance gate and
  it stays in the suite permanently.
- **The stress scene, built at the production workload class** (the spatial arc's e6 lesson — grow
  the bench to the real shape *before* claiming the win): a `lights-stress` gallery/budget scene
  with torch-density statics (~200, ranges 8–15 like vigil's) plus a moving-light population
  (~300, small ranges, projectile-like). Budget lines: `scene` at the stress scene, the cluster
  build's CPU cost, both with calibration prose.
- **Introspection (agent ergonomics):** the profile report gains cluster occupancy stats — total
  refs, max/mean list length, occupied-cell count — so a light-cost regression names its cause
  from one `ludion profile` run instead of a GPU capture.

### Wave 3 — vigil adopts (torches and tracer light; the game half)

- Torch pass over the districts: the kit already pairs torch-post geometry with `PointLight`s
  (`city.kit.yaml:120`); author them at the intended density — streets, house-adjacent, interiors —
  through the map/kit path (`edit-map → generate-map → map-check`, `map-drift` green). The
  discoverability beacons (emissive) are unaffected.
- Projectile light, game-side: `PointLight` on the projectile archetype (small range, warm color,
  intensity tuned off a sweep). **`WorldHash` moves** — `PointLight` is a registered component —
  so the committed replays re-record; the known ritual, one commit.
- E6 re-measure on the regenerated horde scenes with the light population live; per-OS budget
  updates with measurements recorded.
- **Human gate (rides the next sitting):** the torch-lit night city read — do the streets lead the
  eye, do interiors glow through doorways, does tracer light on cobblestones hit the G7 "would you
  show someone?" register — plus the interior-leak question that decides whether the point-shadow
  trigger (below) fires.

### Wave 4 — docs and close

- `ludion-renderer` topic: the cluster scheme, the group-1 layout, the identity argument's proof
  obligations on the candidates path (the spatial-index doc precedent); `ludion-engine`/extract:
  the order contract; engine guide "Lights" section: the cap is gone, what replaces the authoring
  guidance (`determinism` topic untouched — nothing here is sim-side). `embedded-docs/` re-mirrored.
- `Plan/post-process-pipeline.md`'s deferred-work line and decision 5's addendum
  in a private plan both point here; `Plan/global-illumination.md` §9.5's dependency is marked
  satisfied. Close ritual: Status flip, notebook, CLAUDE.md if the wire/CLI surface moved (it
  should not — zero protocol change in this plan).

## Deferred, with triggers

- **GPU compute cluster build** — trigger: the CPU build measures > 0.3 ms at the stress-scene
  population. Brings the frame graph's first buffer resource and compute pass kind; the
  determinism discipline is GI §9.3's (gather over scatter, or integer atomics — WGSL has no float
  atomics to get wrong).
- **Point/spot light shadows** — fired (Clockout 0007): opt-in under a caster budget,
  `Plan/shadow-mapping.md` decisions 9–11.
- **`SpotLight`** — trigger: content that needs a cone (a lighthouse, a searchlight, a doorway
  spill narrower than a sphere). Rides the same array and grid; a new component, so it arrives
  with its own validator and palette archetype.
- **Per-frame fitted shell ladder / exact sphere-frustum refinement / tile depth culling** —
  trigger: cluster occupancy stats show list bloat that measures on the scene pass. Conservatism
  stays mandatory; only tightness is negotiable.
- **The world-space light bin** (GI's injection, the medium's point-light in-scatter) — belongs to
  the GI arc per its §9.5; this plan's array is its input. Trigger lives there.

## Risks

- **Codegen byte movement in wave 1 is likely and bounded, not deniable.** Every mesh/sprite
  pipeline's generated code changes; the volumetric wave-3 precedent says even provably-unchanged
  arithmetic can move a pixel one step. Mitigation is structural: the final shader shape lands
  once (decision 6), the re-bless is pre-authorized once with committed reports, and wave 2 is
  byte-identical by gate. The macOS queue entry is written in the same commit.
- **Bindgen's storage-buffer support is new reflection surface.** Runtime-sized arrays,
  `min_binding_size` semantics, visibility inference, unification across the two consumers.
  Mitigated by the drift gate (a mis-generation fails `shader-drift` or the baked size asserts)
  and by the snippet being the single declaration site.
- **The `scene` ceiling on `lights.yaml` has 1.2× headroom and the indirection is not free.**
  Three lights through a cluster fetch may breach 0.06 ms. The honest outcomes are: engineer the
  overhead down, or move the ceiling with its calibration prose rewritten and the regression
  named. What is not acceptable is a quiet widening — the file's own header says one modest
  regression should fire it.
- **Non-uniform loop bounds forbid derivative ops inside the light loop.** True today (both loops
  are ALU-only; the sun's PCF sits outside), must stay true — a future effect wanting a texture
  sample per light moves the sample out of the loop or pays `textureSampleLevel`. Recorded in the
  snippet's doc block.
- **The GPU access-violation flake will look guilty again.** This plan adds pipeline/codegen
  churn, which shifts the schedule the P0 flake rides (two
  sightings, neither captured). Do not accept a small A/B as attribution; the LocalDumps registry
  key is the actual next step and is not this plan's.
- **Vigil's WorldHash/replay churn (wave 3) is a game-side cost this plan schedules but does not
  own.** Projectile `PointLight` moves every committed replay; the re-record ritual is known and
  single-commit, but it lands coordinated with the First Game instance, not under it.

## Wave log

### Wave 1 — landed 2026-08-06 (the cap falls)

- **Bindgen's storage support was mostly latent.** Reflection already walked storage globals and
  runtime-sized arrays; what was added is two guard errors (`WritableStorage` — nothing writes a
  storage buffer GPU-side; `RuntimeSizedMember` — no `#[repr(C)]` POD can mirror a runtime tail, so
  bind arrays bare) plus tests and doc. Two findings worth not re-deriving: **naga's `Layouter` is
  address-space-independent** (WGSL has one layout algorithm; uniform space only adds validation
  constraints), so the std140/std430 question dissolves and a struct shared between a uniform and a
  storage buffer unifies to one emitted POD; and **naga sizes a runtime array as exactly one
  element**, so `min_binding_size` = one element's stride falls out of the existing `min_size`
  plumbing by naga's own contract, not by new code.
- **The final shader shape landed exactly as designed** — `LightHeader` (32 B: ambient +
  `grid = (1/render_w, 1/render_h, ladder scale, ladder bias)`) + the three storage buffers on
  group 1. One refinement: the shell lookup is fed the **squared** camera distance
  (`shell = log2(d²)·scale + bias`, the `log2(d²) = 2·log2(d)` halving folded into `scale`
  CPU-side), so no fragment pays a square root — both consumers hand over `dot(to_camera,
  to_camera)` of a vector the mesh path already needed for `v`. Ladder range `[1, 512]`, both
  powers of two so the derived constants are exact.
- **The degenerate build's compact form is exact:** one shared index list `[0..N)` that every
  cell's `(0, N)` entry points at — never 2304 copies. An empty scene uploads one zeroed element
  (wgpu rejects a zero-sized binding) with every count 0.
- **Zero goldens moved.** Every golden — all 17 gallery scenes, both programmatic images, vigil's
  `district-plaza` — is **byte-identical**, verified by byte-compare rather than SSIM rounding.
  The risk section's "codegen byte movement is likely" did not materialize on this
  adapter/driver; the pre-authorized re-bless went unused and no macOS queue entry was written.
- `select_lights`, `camera_ground_target` and `dist2` deleted; the ascending-EntityId order
  contract now lives on `extract_lights`, reworded for the clustered world.
- Tests: the 18-light fixture asserts both halves of the identity argument (lights 17/18 visibly
  contribute AND the crowded centre is byte-identical with and without them — the `+0.0` proof,
  pinned directly); sprite parity above the old cap; 0/1/16/17/64/200 population byte-stability;
  24-light cross-process identity through the CLI.
- **The `scene` budget breach fired as the Risks section predicted**: 0.051 → 0.0614 ms on
  `lights.yaml` (three release runs, identical medians) — the fixed per-fragment indirection
  (header fetch, `log2` shell, table fetch) is ~20% of a pass whose light loop is 3 iterations.
  Ceiling moved 0.06 → 0.075 with the calibration prose rewritten and the regression named
  (`budgets/pinned-rtx3090.toml`), keeping the deliberate 1.2× posture.
- Stale cap statements corrected outside wave 4's doc pass (they had become falsehoods, not
  docs debt): `dev-docs/ludion-renderer.md` Rules, `dev-docs/ludion-shader-bindgen.md`'s two
  `LIGHT_COUNT` references, `docs/lighting.md`'s cap analogy (+ embedded mirror). Wave 4 still
  owns the real topics.

### Wave 2, second package — landed 2026-08-06 (stress scene, budget, occupancy stats)

- **The stress scene is `examples/gallery/lights-stress.yaml`, built to the workload class rather
  than to a round number** (662 entities): 200 torch statics at ranges 8–15 and 300 projectile-like
  movers at ranges 1.5–3.0, over a jittered 10×10 block grid, 60 props and a ground plane. It
  deliberately authors **no** `Environment`, `PostProcess` or sun — every absent feature is one fewer
  cost mixed into the `scene` pass, so what the budget gates is the light path and nothing else. The
  first render was blown out (500 lights × the intensities a 3-light scene wants); intensities came
  down to 0.55–1.05 (torches) and 0.45–0.85 (tracers) and the albedos with them, so the committed
  golden reads as overlapping warm pools with dark roofs — a golden a binning hole would visibly
  move, which a saturated cream field would not be.
- **The occupancy panel is a free by-product, and it is the arc's diagnostic instrument.**
  `ClusterStats` (lights, cells, occupied cells, total refs, max list, mean list over occupied cells)
  is reduced out of the prefix-sum walk `cull` already performs — the counts are in cache and already
  being read, so it costs a compare and a max per cell, not a pass. A unit test re-derives every
  field from `table`/`indices` the slow way and requires exact agreement, so the panel can never
  become a second opinion about tables it is supposed to summarize.
- **The introspection landed whole: `ProfileReport` grew a `cluster` section.**
  `ClusterProfile { build_median_ms, lights, cells, occupied_cells, total_refs, max_list_len,
  mean_list_len }`, optional and `skip_serializing_if`, filled by the **offline** driver only —
  the exact mirror of `transport`, which only the *server* can fill, and for the mirror-image reason:
  a live host builds its cluster tables inside a frame a profiler must neither stall nor make pay
  twice, while the offline driver owns its own loop. `profile_cluster` sits beside `profile_gpu` and
  **needs no adapter** — the binning is pure CPU arithmetic over the extracted scene — so this is the
  one section of a report that never degrades on a headless box. `ludion profile` on the stress scene
  now prints, in one run, `build_median_ms 0.0973` next to `total_refs 17121 / occupied 794 / max 74
  / mean 21.56`, which is precisely the pair that separates "shading got more expensive" from "the
  binning got looser".
- **The schema hash moved: `ea364b6e60cc89b4` → `5c7c89710edfd58c`, and all three committed replays
  re-recorded.** `ProfileReport` is reachable from `ResponseBody`, so *any* growth of it — a section
  or a corrected `///` — moves the hash, and `check_compatible` hard-rejects a `.lrp` whose header
  pins the old one. Paid here rather than folded into wave 3 on the orchestrator's call: the additive
  rev ritual is established, nothing was dirty, and deferring would not have saved the breakout
  re-record. The five-step checklist ran in order (`ludion devdocs protocol-evolution`), replays
  last. **Nothing sim-side moved, proven three ways:** the pinned `sim-hash` references are unchanged
  (`breakout-mock@300 → 8541f685e60de56e`, `lights@300 → ff47ca4f4dd2331e`); every replay test's
  live-vs-replayed trace assertion passes; and old-vs-new byte comparison confines every difference
  to the compressed header — 360 / 53 / 200 bytes at the head, with 38,012 / 1,472,678 / 2,720 bytes
  of record stream byte-identical.
- **The in-context measurement, which is not the synthetic one.**
  `cluster_build_cost_on_committed_scenes` (`ludion-engine`, `#[ignore]`d) loads the committed scene,
  extracts at 1920×1080 and times the real build. On `lights-stress.yaml`: **0.1256 ms for 500
  lights, 17,121 refs, 794/2,304 cells occupied, longest list 74, mean over occupied 21.56** — the
  occupancy figures identical to what the report prints, which is the cross-check that the two paths
  compute the same thing. Against decision 3's **0.3 ms** trigger: the report's own statistic reads
  0.102 ms (34%), the loop average 0.1256 ms (42%), and wave 1's synthetic standalone was 0.1693 ms
  (56%) — the in-context build is faster because this scene's lights spread over a plaza the camera
  sees obliquely, so more of them share few tiles than a synthetic cube did. The CPU build keeps its
  ground; `ludion_core::par` and the GPU compute build both stay deferred. `lights.yaml` reads
  0.0216 ms for 3 lights — the other end of the range, and the way to tell a *fixed* per-frame
  regression from a per-light one.
- **The budget is `budgets/pinned-rtx3090-lights-stress.toml`, two lines — one CPU, one GPU.**
  `cluster_build_ms = 0.25` (a new `Budget` scalar reusing `L4005`/`L4006`, **no new L-code**;
  skipped rather than failed when a report carries no cluster section, which is every `--attach`
  capture — the same skip rule `[post_groups]` uses) gates the work that *decides* which lights a
  cell holds; `[passes] scene = 1.4` gates the work of shading through what that decided. Eight
  120-frame release runs medianed 0.7045 on `scene` — a **bimodal** distribution, two tight clusters
  ~0.1 ms apart, a GPU clock/boost state rather than variance in the work, so the ceiling is 2× the
  median *and* clears the upper mode. `composite`, the `bloom` group, `frame_total_ms` and `[systems]` are
  deliberately absent: all four are extent-driven or schedule-driven, measure the same here as on
  `lights.yaml` (composite 0.0338 vs 0.0399; bloom 0.0543 summed vs 0.060), and are already gated
  there — a second ceiling on one regression is two files to re-bless for one cause. Wired into
  the private CI's job 8 and its Windows budgets configuration.
- **The `scene` pass is 11× `lights.yaml`'s at 167× the lights** (0.69 ms against 0.061 for 500
  lights against 3) — the thesis stated as a measurement: cost follows lights *near each pixel*, and
  66% of the grid is free frame.
- **One doc correction rode the hash move, and one deliberately did not.** `ProfileReport`'s "Both
  optional sections are absent in an offline report" became false the moment a third arrived that is
  *present* offline, so it was rewritten; `into_report` and the module's layer list gained the
  cluster case. `SimProfile::sequential`'s "the offline driver reports three (`world_hash`,
  `extract`, `command_drain`)" was **left alone because it is still true** — the cluster build got a
  section of its own rather than a fourth sequential entry, which was the design choice precisely so
  that sentence would not have to become a lie.
- macOS (wave 2): **no golden moved and no ceiling changed**, so the M4 queue entry was
  purely additive — one first bless (`goldens/macos/lights-stress.png`) and one file that must be
  *measured* on the M4 (`budgets/pinned-apple-m4-lights-stress.toml`), never derived from these
  numbers. `provision.mjs`'s `BUDGET_STEPS` grew an optional fourth argument so `budgets-macos` can
  take its third step in the same commit as that file. **Settled 2026-08-09 (Lore rev 585):**
  `lights-stress` first-blessed, `district-plaza` re-blessed, the M4 budget files measured
  (`--check` green).

### Wave 3 — landed 2026-08-06 (vigil adopts: torches, hearths, tracer light)

- **The torch pass went through the map/kit path only, and it is now 83 authored lights across the
  three districts against the 24 they carried** — plaza 32 posts + 18 hearths, market 22 + 12,
  cathedral 21 + 7, plus each district's `moon-fill` and the player lantern. In play the plaza
  measures **54 lights**, more than three times the cap that stood a week ago. Nothing about the
  authoring surface changed: a torch is a `kind: torch-post` marker in `district-*.map.yaml` and a
  `PointLight` entity in the district's `*-core.yaml` fragment, the pairing the plaza already used,
  and the generated scenes regenerate byte-identical (`map-drift` green).
- **Two classes, not one.** The `torch-light-*` set is the street: the fountain/entry rings, the
  plaza's central avenue in five ranks, alley junctions, the perimeter running lanes, the market's
  four aisle ranks and bazaar, the cathedral's nave aisles, both far gates, and one doorway torch per
  avenue-facing house — intensity 2.0–2.4 at ranges 10–14, inside the band the stress scene
  calibrated. The `hearth-light-*` set is the fires **inside** the buildings: one per house/stall/
  chapel marker at its own XZ, hung at hearth height (y = 2.6) at intensity 2.2 **range 7**, below the
  8–15 band deliberately — at that height a range-7 sphere reaches the ground only 6.5 u out, so the
  glow fills the interior floor and reads through the doorway without washing the street. They carry
  no post, which is why they are named apart.
- **The pairing is a test, not a convention.** A `PointLight` has no offset field and the post mesh is
  bottom-anchored, so the two halves of a torch *cannot* ride one entity and the map format has
  nothing that ties them together — at ~50 per district that is a real hazard, so
  `every_torch_post_is_paired_with_a_light` (vigil `tests/districts.rs`) checks all three districts:
  the `torch-post-<id>` / `torch-light-<id>` id sets must match exactly, agree on XZ, and put the
  light at flame height. **The gap this closes is a candidate trigger, recorded not acted on:** kit
  primitives cannot carry `components:` (and every primitive must name a mesh), which is the only
  reason the light lives in a second file. If a second kit ever needs the same pairing, that is the
  feature to add — one `kind: torch` emitting post + light — and this test becomes unnecessary.
- **The south gates are deliberately unflanked.** A flanking pair there sits 3–5 u from the player
  start with the entry ring already 6–10 u away; checked against the golden's own view it washed the
  whole bottom of the opening frame, so it was cut in all three districts (the market's east gate and
  the cathedral's west gate keep theirs — those mark genuinely unlit exits). The far gates read as a
  lit V from above, which is the wayfinding the discoverability arc wanted.
- **`moon-fill` retuned 1.2 / 1.0 / 1.1 → 0.5, and that is a consequence of the torch pass, not a
  separate taste edit.** At 45 u up with range 170 the fill lands ~0.4–0.55 of a street torch's peak
  on *every* cobble; mid-district that was most of the light in frame, and a sweep down the plaza
  avenue (1.2 / 0.8 / 0.5 / 0.0) showed the pools only start leading the read below ~0.8. The fill was
  calibrated when a district authored 8 torches and had to carry the ground on its own. Now its job is
  the leftovers — which is the read `env-night.yaml`'s prose already claims.
- **The tracer light is `PointLight { color: [1.0, 0.8, 0.45], intensity: 2.0, range: 2.5 }` on the
  bolt archetype** (`spawn_bolt`, vigil `common.rs`), picked off an A/B render of 1.0 / 1.6 / 2.4 at
  ranges 2.5 and 4.0 against a reference torch under the shipped night grade: 4.0 spread the pool thin,
  1.0 barely separated from the emissive-only bolt. The number is also derivable, which is why it is
  the shipped one — at the bolt's y = 0.6 the attenuation directly beneath is (1 − 0.6/2.5)² = 0.58, so
  2.0 puts the pool's peak at 1.16, within a percent of a street torch's peak on the ground below it.
  Same brightness, a fifth of the radius: a hot spark skimming the cobbles. Enemy shots deliberately
  stay emissive-only — the player's own fire should lead the read.
- **The route did not move, and that was predicted rather than discovered.** Torch posts and hearths
  carry no collider, so the nav grid, the spawn annulus and every RNG draw are untouched; the added
  statics shift EntityIds uniformly, which preserves every spatial tie-break's relative order. Every
  behavioural assertion in the vigil suite passed unedited — the full-run session test included — and
  only the artifacts moved.
- **Artifacts: `run.lrp` re-recorded, `district-plaza.png` re-blessed, `stress.lrp` and breakout
  byte-identical.** The Windows golden moved SSIM **0.9611** (worst tile top-center 0.5298 — the lit
  street beyond the fountain), report committed at
  `Plan/artifacts/clustered-forward-wave3-district-plaza.diff.json`; the start frame keeps its
  character (`image-stats` `clipped_high` 0.0015 → 0.0019, banding unchanged). `stress.lrp` came out
  byte-identical because that session never fires a bolt. One ride-along the re-record forced:
  `LARGEST_COMMITTED_RECORD` (ludion-protocol) 160 KiB → 192 KiB — the plaza keyframe grew to 177,754
  bytes with the new entities, and its own test names the number; the product with
  `DECOMPRESSED_HEADROOM` stays at 6 MiB, under the 8 MiB bomb the reader must still refuse.
- **E6 re-measured with the light population live, and the `scene` ceiling moved deliberately.**
  `budget-horde` now carries 40 street torches (`budget-torches.yaml`, a fragment of its own because
  `budget-swarm.yaml` is generated output) and the shipped tracer light on all 300 authored bolts —
  **341 lights in a measured frame against 3 before**. Sim is unmoved and comfortable: frame total
  **1.20 ms vs 16.6** (down from 1.77 — `par`'s 8-thread default, a throughput change, never a hash
  change), `update_flow_field` 0.63, `enemy_ai` 0.373. GPU `scene` **0.824 / 0.929 / 0.856 / 0.828 ms**
  over four 120-frame release runs, so the ceiling went 0.45 → **1.70** (2× the median, clearing the
  top reading) with the calibration prose rewritten and the cause named. A new top-level
  `cluster_build_ms = 0.15` gates the CPU binning at **0.060–0.068 ms measured** — decision 3's
  0.3 ms GPU-build trigger is still 4× away on a real game scene, not just a synthetic one.
- **The cluster numbers on the real districts, for the sitting.** Plaza **54 lights / 4,996 refs /
  2,160 of 2,304 cells occupied / longest list 15 / mean 2.31 / build 0.021 ms**, scene pass 0.219;
  market 38 / 3,909 / 2,160 / 9 / 1.81 / 0.015, scene 0.179; cathedral 32 / 3,700 / 2,160 / 12 / 1.71 /
  0.013, scene 0.152. The **94% occupancy is the moon fill alone** — one range-170 light reaches every
  cell, so it contributes a ref everywhere and the mean list is ~1.3 real lights on top of it. That is
  the shape to remember when reading a district's occupancy panel: a single global fill sets the floor.
- **Queued for the human sitting** (beyond the wave's own gate questions): whether the hearth glow
  through a doorway is worth the leak it also puts on the ground outside — the point-shadow trigger's
  evidence; whether the `moon-fill` 0.5 retune is right or wants to go further (0.0 was legible in the
  sweep); and whether the discoverability beacons still lead the read, which **cannot** be judged from
  the golden — beacons are spawned by `manage_district_content` at runtime, so no static render shows
  one.
