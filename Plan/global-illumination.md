# Plan: Global Illumination

> **CLOSED — shipped 2026-08-09; every wave is DELIVERED (§4's "W0 is DELIVERED (2026-08-07)",
> "W1 is DELIVERED (2026-08-07)", "W2 is DELIVERED (2026-08-07)", §9's "W3′ — DELIVERED
> (2026-08-08)" and "W4 is DELIVERED (2026-08-08/09)"; the four `Plan/gi-wave*.md` Status lines are
> the per-wave records). Owed: the look sitting, folded into the queued merged human gate.**
> Status: **ACCEPTED, building (2026-08-06).** D1–D8 locked in a grill-me session with the user;
> see **§0 for the rulings** (the reframes are load-bearing — style-agnostic authored spacing, the
> dynamic layer as a host quality knob, the temporal rule kept intact). Research + alternatives
> below stand as the record the decisions were made from. It belongs to the graphics-engine feature
> stream (the `environment-ibl.md` / `shadow-mapping.md` / `volumetric-medium.md` shape — a standalone
> plan, not a numbered engine phase). §6/§9.11's open-decision lists are superseded by §0.
>
> The one-line recommendation, so the reasoning below can be read against it: **bake a world-space
> irradiance volume by ray-tracing a voxelization of the loaded scene, at scene load, on the GPU,
> content-hash-cached — the `ibl.rs` shape verbatim — and sample it from the mesh shader's existing
> `@group(3)`.** Screen-space bounce (Alternative B) is a cheap later complement; realtime cone
> tracing (Alternative C) is trigger-gated on value, not on capability (see C2).
>
> **AMENDED 2026-08-01, under a proposed rule change (§7–§8).** The user asked what changes if the
> "no temporal accumulation" rule is relaxed. §7 proposes *sharpening* it rather than relaxing it
> (path-independent world-space caches permitted; screen-space history still rejected), and §8 gives
> the recommendation that follows: **the same volume, the same voxelizer, the same `@group(3)`
> binding — but the bake becomes a converging update loop, and dynamic lights bounce.** The
> amendment does not change what is built first; §4's W0/W1 stand unchanged.
>
> **§9 added 2026-08-01** under a long-term constraint the user set after reading the above: the
> 16-light cap is going away, projectiles will emit light, and **dynamic light must be in the GI**.
> Its headline: slow light amortizes, fast light is recomputed whole every frame with no history —
> one volume, two contributions, two update rules. Its uncomfortable corollary is recorded there
> rather than buried: that constraint **weakens §8's case for changing the temporal rule**, because
> the feature the relaxation was meant to enable arrives by a single-frame mechanism instead.
>
> **§10 added 2026-08-01, at user direction** — the user proposed a TLAS/BLAS design (bounding-box
> top level, per-mesh voxel grids as bottom level, priority-updated probes) and asked that it be
> explored **for the open-world case**. Its headline: at district scale the dense grid stands and
> W0/W1 are unchanged, but at streamed-world scale the dense grid dies on memory and the
> **BLAS-hybrid** (object-space grids for visibility, the §9 injection grid for radiance) becomes
> the favored escalation — trigger-gated, decided on measurements when streaming arrives. It also
> records a correction to an argument made against BLAS during the discussion.

---

## 0. Decisions locked (2026-08-06, grill-me session with the user)

The proposal's open decisions D1–D8 are answered; the arc is **accepted and building**, GI first
(the user chose the GI arc over the parked scalability work despite the near-term low-end deadline).
The §9 decisions D10–D13 are in play because the full injection grid was chosen, and resolve by
measurement at W3′, not now. Rulings and the reframes that moved them:

- **D1 — Alternative A** (baked world-space irradiance volume). B is an optional later wave, C
  trigger-gated. Decided on per-frame cost (A costs one texture fetch; C pays the up-to-16×
  `--scale` multiple and several ms on the M4) and on monetizing the voxel premise.
- **Dynamic scope — static bake + the full §9 injection grid**, but the **dynamic injection layer is
  a host-level quality knob** in the `--scale`/`Engine::set_render_scale` category (not world state,
  not a wire method, not in `WorldHash`): off on a low-end target, on for the 3090. It must be built
  as the **first citizen of a coming quality-tier / scalability system** (the user's concrete driver:
  the engine must run on an old laptop), not a one-off boolean. The static volume stays a
  scene-authored *look* component; its **bake resolution is a second quality-tier axis**.
- **D5 — Indirect only.** Clustered forward (shipped 2026-08-06) *retired* D5's only counter-argument
  (fixing nearest-16 popping — that path no longer exists), so carrying far-field direct would now
  purely double-count. The volume carries bounce + sky visibility + emissive-as-bounce; direct
  lighting is entirely clustered-forward's job.
- **D7/D8 — Keep the temporal rule intact. No relaxation.** §9.6's finding stands in full: dynamic-
  light GI arrives via the single-frame §9 injection (no history), so the whole feature ships with the
  "no temporal accumulation" rule untouched — static = load-time bake, dynamic = recomputed whole
  every frame. Pillar 3 keeps zero asterisks, no convergence-fixture gate, no per-frame convergence
  cost on the low-end budget. §7/§8's amortization is **deferred** as a pure optimization of the
  static half; trigger: an intolerable load-time bake hitch on low-end (revisit then, not before).
- **D2 — (a) scene as loaded.** Occupancy/albedo/emissive from static geometry present at load;
  runtime-spawned entities stay out of occupancy (consistent with §8: dynamic geometry receives and
  emits GI but does not occlude in v1). No new freeze-list component; `GiStatic` (option b) is a
  trivial additive upgrade the day content authors a moving occluder needing exclusion.
- **D3 — GPU bake** (`ibl.rs` shape). The dynamic half forces a GPU compute path regardless, and the
  engine already hard-requires GPU compute (`ibl.rs`), so this is no new demand on the low-end target.
  Low-end scales via bake resolution, not a CPU fork. CPU's cross-platform bit-identity buys little
  given per-OS golden sets.
- **D4 — SH-L1, spacing authored, storage behind a seam.** SH-L1 is the *style-neutral* directional
  format (scalar assumes a flat look and glows walls on both sides; ambient cube is diminishing
  returns) — the user's load-bearing framing: **vigil is one example game; Ludion must serve very
  different art directions**, so the engine hardcodes no look. **Probe density is an authored
  GI-component parameter** (world-units-per-probe or a count) + tier-scaled — *not* an engine constant
  (my earlier "1-unit" was overfit to vigil's 0.2-unit walls). The bake writes the probe format and
  `mesh.wgsl` samples it through **one helper**, so an **ambient-cube upgrade is an additive swap** —
  recorded as a candidate future experiment to try, not built now.
- **D6 — Dedicated `gi-interior` gallery scene**, authored in W1: a closed room + one open door + one
  interior torch + a dynamic light. Style-neutral instrument that isolates the three things that are
  GI and nothing else (no sky leak through walls, door-bounce onto the interior, warm indirect pool);
  **built-in registry** so `render --sweep` can drive spacing/intensity; carries the dynamic element
  so it doubles as the W3′ injection testbed. post-full/night-street stay secondary, re-bless at W4.

§9 leanings carried forward (resolve at W3′): **D10** the world-space bin is this plan's own to build
(clustered forward shipped the light *array* + counting-sort shape per its decision 8, not a grid);
**D11 binned gather** over fixed-point atomics, both deterministic, pinned by a cross-process
byte-identity fixture at a few hundred lights; **D12 dynamic term irradiance-only** leaning; **D13**
`stress-horde` supplies the "what is a lot" number by measurement before W3′ is sized.

**Parked, with the vacation/old-laptop deadline as the concrete trigger:** a proper
**scalability / quality-tier plan** is now warranted and is arguably more urgent than GI for the
low-end target (GI *adds* load, it does not relieve it). Its sharpest independent gap: `RenderScale`
refuses to go *below* native (`L4011` — "upscaling is a different feature"), so there is no way to
render cheaper than 1080p on a weak GPU today. Deferred by the user's "GI arc now" choice, but it
should get its own `Plan/` file and its host-knob design must anticipate driving GI's dynamic layer.

---

## 1. Why now, and what the engine is actually missing

The renderer's ambient story today is one number and one cube: `Environment` prefilters an analytic
sky gradient into an irradiance cube + a specular cube (`ibl.rs`), and `mesh.wgsl` applies it to
every surface **unoccluded**. There is no bounce, no light leak control, no sky visibility, and no
path by which a light — or an emissive voxel — illuminates anything it does not directly face.

Four measured facts from the record make the gap concrete:

1. **The ambient/IBL term is 1.4–2.4% of a vigil frame** (post-process notebook, `ssao_direct`
   entry). The night city is owned by point lights and emissive. Anything that only modulates
   ambient — SSAO did — is structurally invisible: measured at most **9 of 255 levels on 7.9% of
   the frame**.
2. **Emissive lights nothing.** Sixteen of vigil's twenty-one voxel models author `emissive`
   (torch flame at 5.0, creature glow, the idol's runes). Today emissive is added in `fs_main` outside
   `shade` — it is a bloom feed and nothing else. The torch flame does not light the torch post it
   sits on.
3. **The light budget is 16, with nearest-16 selection by camera ground target** (`LIGHT_COUNT`,
   `extract::select_lights`). A district authors ~10 today; the city-scale arc's 3× growth and any
   future lamp density walks straight into a cap whose failure mode is lights switching on and off
   as the camera moves.
4. **The content is already hand-faking it.** `district-plaza.yaml` authors an entity literally
   named **`moon-fill`** — a point light at y = 45, range 170, colour `[0.55, 0.62, 0.85]`, i.e. a
   manually placed sky-bounce term. That is a designer routing around a missing engine feature.

And the user's premise is right and load-bearing: **the world is already voxels.** Districts are
generated from `kind: map` sources that place `.voxel.yaml` models; the models carry a palette with
`color` / `emissive` / `roughness` / `metallic` per entry. A voxel grid is the data structure every
GI technique in this document wants, and we get it from the authoring format rather than building
it. Nothing else in the engine has that property.

**Headroom exists.** Vigil renders at 1.81 ms against a 16.6 ms ceiling on the pinned RTX 3090.
The binding constraint is the Apple M4 (post-full: 4.6 ms wall clock for the whole all-on chain),
which is why the recommendation is a load-time bake and not a per-frame trace.

---

## 2. The constraints that eliminate most of the literature

These are facts about this engine, not preferences. C1 and C3–C6 are hard: a proposal that violates
one is dead. **C2 is not of that kind** — it is scoped work, and it is listed here only because the
first draft of this document mistook it for a boundary and reached one conclusion too many from it.

**C1 — No temporal accumulation.** *(Amended 2026-10-19: not "ever". Screen-space history stays rejected until a recorded trigger fires; `Plan/reflections.md` decision 12 holds the first one.)* `docs/ludion-renderer.md`: *"No temporal accumulation —
test mode is the only mode. The same scene on the same adapter yields byte-identical pixels across
processes."* The post-process arc rejected TAA on the deeper reason: reprojection makes the image a
function of *camera history* rather than world state, and the agent's screenshot query would stop
being an observation of the world. This eliminates, as shipped-path techniques: **TAA-denoised
SSGI, DDGI / probe fields with temporal hysteresis, ReSTIR, screen-space radiance caches,
progressive path-traced accumulation, and any "converges over N frames" scheme.** `ludion render`
renders exactly one frame; whatever GI exists must be complete before that frame is drawn.

Corollary, and it is the whole design: **GI must be either precomputed before frame 1, or fully
recomputed within every frame.** There is no third option here.

**C2 — is not a constraint. It is scoped work.** *(Corrected by the user 2026-08-01, against this
document's first draft, which treated it as a boundary.)* The frame graph today has no compute passes
and no 3D or array textures — `PassKind` is a render-pass enum; `TextureDesc` carries an `Extent2d`
and `resource.rs` states *"Array textures are out of scope"* — and compute pipelines themselves are
already proven in-tree (`ibl.rs` runs three, outside the graph, hash-cached, binding into the mesh
pass's `@group(3)`). None of that bounds the design; it prices two build orders. **No option in §3
is eliminated by it.**

What "add it to the graph" actually costs is **three** items, and the third is the one that would
otherwise be discovered late:

1. `PassKind::Compute` — plus executor dispatch and timings attribution, so a compute pass shows up
   in `--dump-timings` and can carry a budget line like every other pass.
2. `TextureDesc` gaining 3D/array extents and a `StorageBinding` usage, with the corresponding
   validation.
3. **A persistent resource kind.** Every resource the graph owns today is `transient` (lifetime-
   computed and alias-eligible) or `output`. **A probe volume is neither: it is a cache whose contents
   must survive across frames**, so it has to be excluded from lifetime aliasing — either as a new
   resource kind, or as an *imported* external texture the renderer owns (which is the `ibl.rs` shape
   again, and is the cheaper of the two).

The two build orders, then, are a sequencing choice rather than a fork:

- **Seam A** — renderer-owned precompute bound into an existing group. Zero graph change, so W0/W1
  can land and be judged before any graph work starts. Its real cost is that a *recurring* per-frame
  cost would be invisible to `--dump-graph`, `--dump-timings` and every budget file.
- **Seam B** — the three items above. Buys instrumentation and budgeting for free at the point where
  the work becomes per-frame, and is reusable well beyond GI.

Recommended: **build W0/W1 on Seam A, and take Seam B at the moment the update becomes per-frame**
(§8's W2) — not because Seam B is expensive, but because the graph work should be motivated by a pass
that actually exists rather than speculatively.

**C3 — No committed derived data.** Voxel-roadmap decision 4: *"No committed derived mesh files —
two files describing one model will drift; derived diffs are noise."* A committed lightmap or probe
file is the same defect at larger scale (and would be binary, and would need its own bless ritual).
Bake at load, cache by content hash in memory, spill to a **gitignored** cache directory only if
bake time ever justifies it.

**C4 — GI is renderer-side and must not touch `WorldHash`.** Nothing about it enters the sim. But
authoring a new component into `env-night.yaml` changes the **scene text**, and a committed replay
embeds the scene text — so vigil's `run.lrp` re-records once, exactly as the medium waves did.
`env-night.yaml` is a `fragments:` entry in four `.map.yaml` sources, so four scenes regenerate.

**C5 — Determinism of the bake itself.** Fixed sample counts, a fixed direction set, no RNG — the
`ibl.rs` gate ("same input ⇒ identical bytes on a pinned GPU") applies unchanged. Note the engine
does *not* require the image to be bit-identical across platforms: the two pinned GPUs keep separate
golden sets (`goldens/windows/` | `goldens/macos/`). So a GPU bake is legitimate; CPU is not
required for correctness (see decision D3).

**C6 — Neutrality by construction.** Every wave of the post-process and medium arcs shipped
byte-identical goldens by *skipping* neutral code paths under a uniform branch rather than computing
with identity constants. A scene with no GI component must take the exact historical path — the
`Environment`/`PostProcess`/`ssao_direct` precedent, and it is not negotiable.

---

## 3. The alternatives

### Alternative A — Baked world-space irradiance volume (**recommended**)

At scene load: rasterize the loaded scene's static meshes into a **world-space voxel grid**
(occupancy + albedo + emissive per cell), light it, and store the result as a **3D irradiance
texture** the mesh shader samples at each fragment's world position with hardware trilinear
filtering. Sits behind `@group(3)` beside the IBL cubes and the AO texture, which is exactly the
group whose job is "the ambient/environment term".

Why it fits this engine specifically:

- It is the **`ibl.rs` shape verbatim** — Seam A, hash-cached, hot-reloads with the scene, and lands
  without waiting on graph work. That is a *sequencing* advantage, not a capability one (C2): it lets
  W0/W1 be judged by eye before anything is added to the frame graph.
- It **monetizes the voxel premise**: the tracing structure is a rasterization of geometry that was
  authored as voxels, and the palette already carries the albedo and emissive the bake needs.
- It answers all four gaps in §1 at once: bounce (fact 1), emissive-as-light (fact 2), all static
  lights baked regardless of the 16 cap (fact 3), and `moon-fill` deleted (fact 4).
- **Dynamic receivers are free.** Enemies, the player, projectiles sample the volume at their world
  position — a monster walking into a torch pool picks up the warm bounce with no extra work.
- It costs nothing per frame beyond one 3D texture fetch in the mesh fragment shader.

Its honest limitation: **dynamic emitters do not bounce.** Vigil has exactly one moving light (the
player's carried lantern) — it stays direct-only unless we accept a re-bake cadence, which C1 makes
unattractive. Worth naming out loud at the sitting; my read is that a lantern that lights walls
directly but does not bleed colour onto the ceiling is an invisible compromise in a top-down
twin-stick game.

Two orthogonal sub-decisions inside A:

**A-propagation: how the volume gets lit.**

| | A1 — ray-traced probes | A2 — LPV-style propagation |
|---|---|---|
| Method | Per probe, trace a fixed direction set (Fibonacci sphere, 32–64 rays) by DDA through the voxel grid; gather emissive + direct-lit voxel radiance; escaped rays sample the analytic sky | Inject direct light + emissive into the grid, then run N fixed iterations of a deterministic diffusion kernel through empty cells |
| Quality | Directional, correctly shadowed, sharp enough for an alley to stay dark | Soft, leaky, no visibility — light diffuses through thin walls |
| **Sky visibility for free** | **Yes** — an escaped ray *is* the sky term, which is the single biggest cheap win (§1 fact 1: today the sky lights the inside of a closed room) | No |
| Bake cost | GPU: ~38k probes × 64 rays ≈ 2.4 M rays at 2-unit spacing — single-digit ms. CPU: seconds | Milliseconds anywhere |
| Second bounce | Trace again against the first result; roughly doubles cost | Free (more iterations) |

**Recommend A1.** A2's leaking is precisely the failure mode a dense voxel city with 0.2-unit-thick
walls will exhibit worst, and A1's sky-visibility term is worth more than the bounce.

**A-storage: what a probe stores.**

| | Scalar irradiance | **SH-L1 (recommended)** | Ambient cube (6 axes) |
|---|---|---|---|
| Size / probe | 8 B (`RGBA16F`) | 32 B (4 × `RGBA16F`, or 2 textures) | 48 B |
| Directional | No — a wall lit on one side glows on both | Yes, one lobe — enough for a diffuse bounce | Yes, sharper |
| Volume for a district at 2-unit spacing (66 × 72 × 10 probes ≈ 48 k) | 0.4 MB | **1.5 MB** | 2.3 MB |
| At 1-unit spacing (~380 k probes) | 3 MB | **12 MB** | 18 MB |

**Recommend SH-L1 at 1–2-unit probe spacing**, resolved at the first look. Scalar is a false
economy in a night city (the whole point is that one side of a wall is torchlit and the other is
not); an ambient cube costs 50% more for a difference no one will call at this art direction.

The tracing grid should be **finer than the probe grid** — 0.25–0.5 world units, which is 1–2 model
voxels for the 0.2-scale buildings. A district at 0.5 units is 264 × 288 × 40 ≈ 3 M cells, ~12 MB
as RGBA8 albedo + emissive. Both sizes sit far inside `max_texture_dimension_3d` (2048).

### Alternative B — Screen-space bounce (SSGI / colour bleed) — a cheap complement, not the answer

Reuse the wave-7 machinery whole: the depth+normal prepass exists, the R2 sample kernel exists, the
`BAYER4` rotation + 4×4 box blur (whose width is exactly the Bayer period, so the pattern cancels
*exactly*) exists, `ssao_direct` exists. A one-bounce screen-space gather that samples the scene
colour buffer instead of only depth is a **small, well-precedented wave** — arguably the cheapest
real GI in the document, and it is deterministic without any temporal filter, which is unusual and
is a direct consequence of choices this engine already made.

But: it is view-dependent, it misses every off-screen emitter (a torch behind the camera contributes
nothing), it cannot relieve the 16-light cap, and its contribution is contact-scale. **Verdict: a
good *second* wave on top of A, a poor substitute for it.** Ship it only if A's volume proves too
coarse at contact scale — the volume's failure mode (blurry, missing the last half-metre) and B's
strength (contact-scale only) are complementary by construction.

### Alternative C — Realtime voxel cone tracing (VXGI) — trigger-gated, not now

Revoxelize each frame into a mipmapped 3D radiance texture and cone-trace diffuse (and optionally
specular) GI from each mesh fragment. It **does** satisfy C1 — a per-frame trace with no history is
a pure function of world state — so it is not disqualified on principle, unlike TAA. It is
disqualified on value:

- **A district is static.** Paying every frame for an answer that changes only when the scene
  changes is the wrong trade — this is the same reasoning that put IBL prefiltering at load. This is
  the load-bearing objection and it is about value, not capability.
- It costs several ms at 1080p, on a target whose binding platform (M4) has ~4.6 ms of measured post
  chain already.
- Cone tracing leaks notoriously at coarse mips, and a night city of 0.2-unit voxel walls is close to
  the worst case for it — the same leak problem a probe volume can attack directly with a validity
  term.
- It does want Seam B in full (C2's three items plus 3D mip generation). That is real work, but it is
  **not** a reason to reject it; the three bullets above are.

**Record its trigger**: the first game concept with destructible or streamed geometry, or one whose
*primary* lights move (a moving sun / day-night cycle, a vehicle's headlights). Note that the voxel
roadmap's "chunked voxel worlds" trigger and this one fire together, and the voxelizer from wave A0
is the shared foundation — building A first is not wasted work if C ever lands.

### Alternative D — Baked lightmaps (UV atlas) — rejected

Rejected on cost and on fit: greedy-meshed quads have no UVs, so it needs an unwrapper + an atlas
packer + a texture pipeline the engine deliberately does not have (textured PBR is trigger-gated and
import-only). It also produces exactly the committed derived data C3 forbids, and it lights nothing
dynamic — in a game whose entire cast is dynamic. A volume needs none of it. Worth stating that
lightmaps are the *higher-quality* answer for static surfaces; they are simply the wrong shape for
this engine's constraints and this game's content.

### Alternative E — Emissive voxels → virtual point lights + clustered forward — a different arc

Cluster each model's emissive voxels into a few deterministic virtual point lights at load, then
render many lights via a clustered/tiled forward path. This is real and it fixes §1 facts 2 and 3
directly — but it is the **many-lights** arc, not GI: it produces more *direct* light, no bounce, no
sky occlusion, and the post-process arc already recorded its own answer here (*"clustered forward is
the recorded answer if light count ever bites, measured not assumed"*). It stacks with A rather than
competing; if the 16-light cap becomes the loudest symptom, this is the cheaper targeted fix, and it
should be planned separately.

### Alternative F — The null option

Keep hand-authoring fill lights (`moon-fill`) and lean harder on the grade. It is honest to note this
*works* today and costs nothing. It stops working the moment districts multiply — every new district
re-authors the same fake by hand, and none of it responds to the player's position.

---

## 4. Recommended shape, as waves

Each wave is a commit with full `check.mjs` green, on the arc discipline the post-process plan
established: neutral paths skipped under a uniform branch, goldens byte-identical until **one**
pre-authorized pixel-moving wave.

> **W0 is DELIVERED (2026-08-07, Lore revs 508–509)** — see `Plan/gi-wave0-voxelizer.md`'s Status
> for the outcome record (decisions taken, deviations, and the two cost shapes W1's bake design
> must know). The paragraph below stands as the outline it expanded.

**W0 — The voxelizer, zero pixels.** A deterministic world-space voxelizer in the renderer (or
`ludion-engine`): placed static mesh instances → an occupancy/albedo/emissive grid. Reads `.voxel.yaml`
models through the mesher's existing bounds-checked accessor seam where possible; primitives
(`cube`, the ground plane) rasterize analytically. Debug-visible from day one — an agent must be able
to *look* at the voxelization, following the model-sheet precedent, because a silent voxelizer is
undebuggable. No shader change, no golden moves.

> **W1 is DELIVERED (2026-08-07, Lore revs 511–514 + 516–519)** — see
> `Plan/gi-wave1-sky-visibility.md`, whose Status carries the full outcome record: the feature path
> (load-time voxelization, the GPU bake into an SH-L1 probe volume, `mesh.wgsl`'s multiply), the
> `gi-interior` calibration scene with a first-bless golden and sweep-chosen spacing, the physics
> fixtures with the leak gate demonstrated able to fail, and the `--dump-gi` instrument.
> **Amendment to the paragraph below, by measurement:** the wave shipped **without**
> spending the pre-authorized re-bless — its product lands through a *new* `gi-interior` golden, so
> every existing golden is still byte-identical and the budget carries to W4. Two corrections to the
> outline's assumptions: the volume stores SH-L1 of a **scalar** visibility function which the shader
> **multiplies** by (escaped rays do *not* sample the sky gradient — baking RGB radiance would
> duplicate what the prefiltered IBL cube already carries, leave specular unoccluded, and forfeit the
> bit-preserving identity gate), and the arc's cost centre turned out to be the **CPU voxelizer**
> (310–340 ms) rather than the GPU bake (~60 ms), which is what W3's budget must address.

**W1 — Sky visibility, one pixel-moving re-bless.** Bake occlusion of the sky term only (no bounce):
probes trace, escaped rays sample the analytic sky gradient, result lands in the volume, `mesh.wgsl`
multiplies the IBL term by the volume's directional sky visibility. This is the cheapest slice with
a real visual product — alleys and interiors stop receiving open-sky fill — and it is the natural
place to settle probe spacing, leak control, and the neutrality gate. This wave is where the
goldens move.

> **W2 is DELIVERED (2026-08-07, Lore revs 525–531)** — see `Plan/gi-wave2-emissive-bounce.md`,
> whose Status carries the outcome record: the same march reports its hit, `L_out = albedo ×
> (emissive + direct)` lands in three per-colour-channel SH-L1 volumes, `mesh.wgsl` adds
> `gi_bounce × diffuse × bounce` inside the GI branch, `gi-interior` lights up as the wave's single
> golden move, and both leak gates were demonstrated able to fail. (The paragraph below predates
> clustered forward — the 16-light cap it mentions is already deleted; the standing value is that
> distant/culled lights still contribute to GI.)

**W2 — Emissive and static point lights as bounce sources.** Inject palette `emissive` and the
scene's static `PointLight`s / the sun into the grid, one bounce (a second is a constant, not a
redesign). This is the wave that makes a torch flame light its own post and warm the cobbles around
it, and that lets *all* static lights contribute regardless of the 16-light cap.

**W3 — Quality and gates.** Leak control (probe-in-solid rejection + push-out, a per-probe validity
term), the `--set`-driven intensity knob, budget entries for the bake (a load-time cost, so it is a
*bake-time* number, not a `[post_groups]` line), fixture tests in the wave-7/8 style: whole-image
byte-equality when off, a specific asserted darkening in a closed room, an asserted warm bleed beside
a torch, and emissive untouched at every setting.

> **W4 is DELIVERED (2026-08-08/09, Lore revs 582 + 584, as Phase 3 of the live-session arc —
> the private graphics-engine notebook's arc entry carries the full record).** `env-night.yaml` authors the
> volume (cell 0.5 / spacing 1.0 / bias 0.5 / sky 1.0 / **bounce 0.6** off a measured pool-lead
> p95/p50 ≥ 2.0 ladder); every vigil mover is `GiDynamic` (9 spawn sites, player + lantern, both
> swarm fixtures); `moon-fill` is deleted from the five gameplay/perf sources (model-stage-night
> keeps its fill — it authors no volume, so nothing below replaced it); **five** scenes regenerate
> (stress-horde consumes the fragment too — this section's "four" predates it); `run.lrp`
> re-recorded, route surviving; `district-plaza` re-blessed SSIM 0.98420 with the committed diff
> report — the wave's only golden move: `post-full`/`night-street` author no `GiVolume`, so the
> re-bless this paragraph forecast for them never came due. The thin-occluder rule is knowingly
> exceeded (0.4 u house-shell walls vs spacing 1.0 — honouring it needs 4.6 M probes, past the 4 M
> cap) with the leak measured absent (+0.054/255 over the darkest decile) and the re-take condition
> written in the scene header. E6: `gi_inject` 0.119 ms → ceiling 0.24 on the arena, with the budget
> stating the district's own 1.28 ms (11×: deeper lattice, rays hit-and-walk instead of escaping);
> district (voxelize, bake) ≈ (100, 12) ms vs the ~500 ms escalation trigger; a 20 s live window
> under Hold showed 0/1245 stale frames — both caches hit through real gameplay. Docs/topics were
> already current from W1–W3′; the mac bless queue lives in the private graphics-engine notebook. The sitting remains open, folded into the queued
> merged sitting.

**W4 — Content adoption + docs + the sitting.** vigil's `env-night.yaml` authors the GI component,
four scenes regenerate, `run.lrp` re-records, `district-plaza` + `post-full` + `night-street`
re-bless with diff reports, engine-guide chapter, `ludion docs` topics updated, mac bless queued in
the private macOS queue. Delete `moon-fill` — the arc's own proof that the feature replaced the fake.

Rough size: W0 and W1 are the substance; W2 is small once W0 exists; the whole arc is comparable to
the environment/IBL arc, not to the twelve-wave post-process arc.

---

## 5. Risks

- **Light leaking through thin geometry.** The single most likely way this looks bad. Vigil's walls
  are 0.2-unit voxels; a 1–2-unit probe grid cannot resolve them. Mitigations must be in W1, not
  deferred: trace at a finer grid than the probe spacing, reject probes inside solids, push probes
  out along the gradient, and consider a per-probe visibility/validity term (the static half of what
  DDGI uses a depth probe for — legitimate here because it is *baked*, so C1 does not bite).
- **The bake is invisible in a night scene, exactly as SSAO was.** The `ssao_direct` finding is the
  cautionary precedent: an effect confined to a term that is 2% of the frame moves 9 of 255 levels
  and no one can see it. This is why W2 (emissive + point lights as bounce sources) matters more
  than W1's sky term for *this* game, and why the arc must measure the frame-mean delta the way the
  medium waves did rather than assume the effect is visible.
- **Bake time enters the iteration loop.** The North Star is autonomous iterations per human look;
  a 5-second bake on every `ludion render` would be a tax on every one of them. Budget it explicitly
  (target: under ~200 ms for a district on the pinned GPU), hash-cache it like `ibl.rs`, and treat a
  breach as a P0 rather than something to live with.
- **Hot reload and `ludion dev`.** A geometry edit must re-bake. The hash key must cover the static
  set, and the cache must be a hit on every frame where nothing moved — hashing ~60 instances per
  frame is free, but the key's *contents* are a correctness question (include a moving enemy and the
  cache misses every frame).
- **Golden churn.** One pre-authorized pixel-moving wave (W1), everything after it neutral-by-default
  until W4's content adoption. The medium arc's discipline — a diff report committed with each moved
  golden — applies unchanged.
- **Scope gravity toward a full VXGI.** The voxelizer is a genuinely reusable structure and will
  invite "we already have the grid, let's cone-trace it." That is Alternative C and it has a written
  trigger; it does not ride in on W0's back.

---

## 6. Open decisions — these need answers before any code

**D1 — Which alternative, and how far.** A alone (recommended), A + B, or hold for C? My
recommendation is **A, with B recorded as an optional follow-on and C trigger-gated**.

**D2 — What counts as "static" for the bake.** The engine has no static/dynamic marker, and
`extract` runs every frame. Three shapes:
  - *(a)* **The scene as loaded is the bake input** — runtime-spawned entities (enemies, projectiles)
    neither occlude nor emit. Needs no new component; caveat: an authored entity that a system
    animates bakes in its start pose.
  - *(b)* **An explicit per-entity marker** (`GiStatic`, the `Fade` per-instance precedent) — explicit,
    diffable, lets a designer exclude a moving platform, costs one more component in the freeze list.
  - *(c)* Reuse `Collider.body: static` — rejected on principle (GI would inherit a physics concept,
    and decorative props legitimately have no collider).
  Leaning **(a)**, with (b) available later if content asks.

**D3 — GPU bake or CPU bake.** GPU follows `ibl.rs` exactly and is single-digit milliseconds. CPU
would be bit-identical across platforms and is a textbook fit for the just-landed
`ludion_core::par` (fixed chunking, ordered reduction) — but the engine explicitly does **not**
require cross-platform pixel identity (per-OS golden sets), so that benefit buys little. Leaning
**GPU**.

**D4 — Probe spacing and storage.** SH-L1 at 1 unit (12 MB/district, crisper) or at 2 units
(1.5 MB, softer)? This is a look call best made off a `--sweep`-style comparison at W1, not
decided in advance — but it needs to be *someone's* call at the first look, not left to drift.

**D5 — Does the volume carry indirect only, or also far-field direct?** Indirect-only is the clean
formulation and avoids double-counting with the 16-light uniform. Carrying far-field direct as well
would fix the light-popping artifact of nearest-16 selection — but it needs a careful "the near 16
are direct, everything else is in the volume" split to avoid double-lighting. Leaning
**indirect-only in this arc**, and pointing the popping problem at Alternative E where it belongs.

**D6 — Is `night-street` or `post-full` the calibration scene, or does GI want its own gallery
scene?** GI's product is an interior/alley contrast that neither existing scene stages well
(`post-full` is an open plaza; `night-street` is a canyon with a colonnade — closest, and it was
built for exactly this class of question). A dedicated `gi-interior` scene with a closed room, an
open door, and one torch is probably the honest instrument, and would be authored in W1.

---

## 7. The temporal rule: sharpen it, do not relax it

Raised by the user 2026-08-01: *what if the "no temporal accumulation" rule is relaxed for visual
effects only?* Three facts from the record, before the answer.

**The pillar never banned it.** `engine-pillars.md`, Pillar 3: *"Test mode disables temporal
accumulation and any nondeterministic post effects."* That sentence presumes a shipped path **with**
accumulation and a test mode that disables it. The absolute — *"No temporal accumulation — test mode
is the only mode"* — is a **renderer-doc rule** (`docs/ludion-renderer.md`, mirrored at
`ludion-renderer/src/lib.rs:21`) describing today's implementation. Changing it amends a doc rule; it
does not overturn a pillar.

**The TAA rejection was never a determinism argument.** `post-process-pipeline.md` decision 5,
verbatim: *"Not for determinism reasons — TAA's jitter is a Halton sequence indexed by frame, and it
is bit-reproducible."* Its three reasons were (a) motion vectors, i.e. MRT plus previous-frame
transforms threaded through `extract()`; (b) a single-frame `ludion render` has no history, so
goldening means rendering N frames and blessing frame N — a whole-harness change; and (c) decisively,
*"reprojection makes the image a function of the camera's history, not of world state… the agent's
screenshot query would stop being an observation of the world."*

**GI fires that decision's own recorded trigger.** Same decision, last line: *"What is genuinely
given up: TAA's amortisation of noise, which would let cheap stochastic SSAO, dithered transparency
and low-sample shadows resolve clean. **Trigger: the first effect whose quality is gated on
stochastic sampling.**"* A stochastic GI estimator is that effect. And wave 10 already drew the
adjacent line for grain: a frame index on `RenderScene` is *"deterministic world state, so **not**
what decision 5's anti-TAA argument rules out."*

### The cut is path-dependence, not subsystem

"Visual effects only" is not the useful line, because GI is *already* renderer-side and already
never touches `WorldHash` (C4). The sim/render split was never what the rule protected. What it
protects is **`image = f(world_state)`**. Split by path-dependence instead:

- **Class 1 — path-independent convergent world-space caches.** Probe volumes, voxel radiance.
  Converge to a unique fixed point determined by the world. Two replays reaching the same world state
  by different routes converge to the same pixels. No reprojection, no motion vectors, no camera
  dependence.
- **Class 2 — screen-space history.** TAA, temporally-denoised SSGI, motion blur. The image depends
  on the camera's path and the jitter phase. **Still rejected under this proposal** — decision 5's
  reason (c) is untouched by it.

**Proposed replacement rule.** *The renderer may carry state across frames only if that state is a
path-independent convergent function of the world, never of the camera's path through it. The
observation endpoints — `ludion render`, `render.screenshot`, `ludion test` — force convergence
rather than skipping the effect. The converged answer is normative; amortization is an optimization
of the live view.*

That formulation dodges all three of decision 5's objections: no motion vectors (a); no golden-harness
change, since `render` converges before capture (b); and the fixed point is world state (c).

### What it costs, stated honestly

- **Convergence transients are observable, and the live view lags.** In a moving world the cache
  never fully converges, so the human's live frame and the agent's forced-converge screenshot differ
  in proportion to how fast things move. That is a real qualification of Pillar 3's *"humans and
  agents see through the same eyes"* — the eyes agree on the converged answer and disagree on the
  approach to it. This is the one genuine loss and it needs an explicit user call.
- **Two paths rot.** Hard constraint, not a guideline: the converge path is **the same kernel with a
  loop count**, never a second implementation. The `ssao_direct` finding is the precedent for what
  happens otherwise — a path that validates, runs, and does nothing, with no diagnostic firing.
- **Flake risk.** *"Flaky tests are P0."* The converge loop must be **iteration-count**-budgeted,
  never time-budgeted; a wall-clock budget would make the golden a function of machine load.
- **New gate required.** A "two routes, one state" fixture: drive the same world state by two
  different command/camera routes, force convergence, assert byte-identical pixels. That test is what
  keeps the rule honest, and the repo has nothing like it today.
- **Rule surface to amend:** `docs/ludion-renderer.md`, `ludion-renderer/src/lib.rs:21`, and decision
  5 **amended rather than overturned** — its own trigger fired.

### What it does *not* buy

Nothing for Alternative B. Screen-space GI's quality upgrade is a temporal denoiser, which is Class 2
and stays rejected. A blanket relaxation would unlock it — at the price of the observation contract
this proposal is built to keep.

---

## 8. Revised recommendation, assuming the rule changes

**The structure does not change: same voxelizer, same world-space SH-L1 probe volume, same
`@group(3)` binding, same trilinear fetch in `mesh.wgsl`, still no committed derived data, still no
`WorldHash` impact.** Three things change.

**(1) The bake becomes a converging update loop.** Same kernel; `K` iterations on a forced converge,
one round-robin slice of the probes per live frame. The forced-converge path and the load-time bake
of §4 are *literally the same code* — amortization is a strict superset of the bake, which is why
§4's W0/W1 are unaffected and this decision can be deferred to W2 with evidence in hand.

**(2) Dynamic lights bounce — the actual content win.** The player's lantern, projectile glows and
the emissives on moving enemies now contribute indirect light. For a twin-stick shooter whose
spectacle *is* dynamic light, this is worth more than everything the static bake delivers. The voxel
grid's occupancy and albedo stay static; only its lighting updates per frame, which is cheap.
**Dynamic geometry is deliberately not re-voxelized in v1** — enemies receive GI and emit into it,
but do not occlude it. Trigger for dynamic occluders: a boss large enough that its missing bounce
shadow is called by eye.

**(3) It is the point at which Seam B is worth taking.** A per-frame probe update is a per-frame
compute pass writing a 3D texture. It *could* run outside the frame graph as `ibl.rs` does — but a
recurring per-frame cost invisible to `--dump-graph`, `--dump-timings` and every budget file is
exactly the untracked cost this repo's profile culture rejects. So this is where C2's three items get
built: `PassKind::Compute`, 3D/array `TextureDesc` + `StorageBinding`, and the **persistent resource
kind** the volume needs (it is a cache that survives frames, so it must be excluded from the graph's
lifetime aliasing). Budget it as its own group from the first commit. All three are reusable beyond
GI, which is part of why taking them here is good value rather than a tax.

### The argument this changes between techniques

Without the relaxation, a probe volume must converge fully within a frame, which is expensive — and
that is precisely what made realtime cone tracing (Alternative C) look competitive. Note that **VXGI
never needed the rule relaxed**: a per-frame cone trace carries no history at all. It needed Seam B
and a per-frame budget. With amortization available, probes cost a fraction of a cone trace per frame
and are far better conditioned for diffuse GI and leak control, so **relaxing the rule is what makes
probes clearly beat cone tracing rather than narrowly.** C stays trigger-gated, now on specular GI
rather than on dynamism.

### Revised waves

W0 (voxelizer) and W1 (sky visibility, the one pixel-moving re-bless) stand **exactly as written** in
§4. After them:

- **W2 — the update loop.** The bake's kernel gains an iteration bound and a round-robin slice
  index; `RenderScene` gains a frame index (already-sanctioned world state); the compute pass enters
  the frame graph (Seam B); the observation endpoints force convergence. **The "two routes, one
  state" fixture lands here** — it is this wave's real product, not the amortization.
- **W3 — dynamic light injection.** Lantern, projectiles, emissive enemies. Static-lit tracing and
  dynamic injection must not double-count: traced/amortized carries static lights + emissive + sky;
  dynamic emitters inject separately.
- **W4 — quality and gates**, **W5 — content adoption + docs + sitting**: §4's W3/W4 unchanged, plus
  a convergence-latency measurement and the per-frame budget entry.

### New risks this introduces

- **Convergence latency is visible on short-lived light.** A muzzle flash lasting three frames never
  converges and will bounce weakly or not at all. Either accept it (flashes are direct-only, and
  bloom already sells them), or evaluate dynamic-light bounce **analytically** against the probe's
  baked visibility instead of accumulating it — instant, path-independent, and more code. Decide at
  W3 with the flash in front of you.
- **Light trails behind moving emitters** — the classic probe-volume artifact: the lantern drags a
  fading indirect smear. Controlled by the hysteresis constant, which trades trail length against
  convergence noise. This is a look call for the sitting, not something to pick from a paper.
- **Per-frame cost lands on the M4**, which the static bake avoided entirely. The bake's cost was a
  load-time number; this one is in the frame budget beside a post chain already measured at 4.6 ms
  wall clock.
- **The live-vs-screenshot divergence is a permanent, documented property**, not a bug to be fixed
  later. It must land in `docs/determinism.md` in the same wave that ships the rule change.

### New open decisions

- **D7 — Which relaxation?** The §7 sharpened rule (Class 1 only, recommended), or a blanket
  relaxation that also admits screen-space history (unlocking SSGI denoising and TAA, at the cost of
  the observation contract)?
- **D8 — Is the live-view lag acceptable?** The one genuine loss. It is a product call about Pillar 3,
  not a technical one.
- **D9 — Hysteresis and round-robin period**, i.e. how many frames to converge. A look call, made at
  W2 against a moving lantern.

---

## 9. What if we have a lot of dynamic lights we want in the GI

> **Long-term constraint, set by the user 2026-08-01:** the 16-light cap is going away, projectiles
> will emit light, and dynamic light must contribute to GI. §§1–8 stand; this section says what
> changes under that constraint. It fires decision 6's recorded Forward+ trigger — *"content that
> provably exceeds what the cap plus nearest-16 selection can express"* — with a driver that decision
> did not anticipate (it guessed per-window lighting).

### 9.1 This is a different light population, not more of the same one

Projectile light in a twin-stick shooter has five properties that decide the whole design:

- **Numerous** — a horde encounter could carry hundreds of live bullets at once (`stress-horde` /
  `budget-horde` are the scenes that will say how many).
- **Short-lived** — a bullet lives tens of frames; a muzzle flash two to four.
- **Fast** — a bullet crosses a probe cell in a frame or two.
- **Individually negligible, collectively the effect** — one bullet's bounce is invisible; a stream
  raking a wall is the thing worth building.
- **Spatially correlated** — they leave a common origin in bursts along a line. They are nowhere near
  independent, and that is exploitable.

**These properties are hostile to §8's amortization and friendly to a grid.** That is the whole
section in one sentence.

### 9.2 The structural finding: split by temporal frequency, unify the storage

§8's amortized round-robin converges over tens of frames. A bullet *exists* for tens of frames and
moves a hundred units in them. Amortizing it does not produce its bounce — it produces a **comet
trail of stale indirect light**, and the trail is not tunable away: less hysteresis is noise, more is
smear. So:

> **Slow light amortizes. Fast light must be recomputed whole, every frame, with no history at all.**

The clean way to have both is to **keep one volume and two contributions**:

| | Static/slow term | Dynamic/fast term |
|---|---|---|
| Sources | sky, lamps, torches, static emissives, architecture | projectiles, flashes, explosions, the lantern, emissive enemies |
| Update | amortized round-robin, hysteresis (§8) — or a plain load-time bake (§4) | recomputed **from scratch every frame**, zero history |
| Method | ray-traced against the static voxelization | grid injection + one bounce (§9.3) |
| Cost driver | probe count | illuminated volume, **not** light count |

`mesh.wgsl` fetches both and sums. **Smearing is structurally impossible for the dynamic half** —
it has no history to smear. Do not unify the update rule; unify the storage.

Consequence for **D4**: dynamic bounce pools are 3–6 world units across, so 2-unit probe spacing
gives a bullet's bounce 1–3 probes and reads as a blob. **This constraint resolves D4 toward 1-unit
spacing.** Static + dynamic SH-L1 at 1 unit is ~24 MB/district, which is affordable. A cheaper
variant worth measuring: store the dynamic term as **irradiance only, no directionality** (one
texture instead of three) — projectile bounce is overwhelmingly ground-up and low-contrast.

### 9.3 Make the cost independent of light count — bin, then splat

The wrong shape is "each receiver iterates the lights." Probes × lights grows without bound and puts
the whole many-lights problem inside the GI. The right shape is two stages:

1. **Bin** every dynamic light into a coarse grid, summing radiance per cell. `O(lights)`, one pass,
   and it collapses a burst of 200 correlated bullets into a handful of occupied cells.
2. **Splat and gather** per *occupied cell*, not per light: for the solid surface voxels a cell
   illuminates, accumulate `albedo × radiance × N·L` into a dynamic radiance grid; probes then gather
   from it through the **same static visibility term the sky pass already baked**, so leak control is
   inherited rather than rebuilt.

The property that matters: after stage 1 the cost is bounded by **occupied cells × kernel**, so
**overlapping lights are free** and doubling the bullet count does not double the bill. Rough figures
to size it: 300 bullets at a 3-unit range on a 0.5-unit grid touch ~1.7k cells each unbinned; binned
first, a tight stream collapses to tens of cells. Both are small — but only the binned one has a
ceiling.

**The determinism trap, and it is a real one.** A scatter-splat accumulates in arbitrary order, and
float addition is not associative — byte-identical pixels across processes would be lost, which C5
forbids. Three ways out, and the language picks one for us:

- **WGSL has no float atomics** (`atomic<i32>` / `atomic<u32>` only), so the obvious wrong thing is
  not even expressible. Use **fixed-point integer atomics** — integer addition *is* associative, so
  the sum is exact regardless of order.
- Or **gather instead of scatter**: each cell loops its own bin in ascending light index.
  Deterministic by construction, provided the bin build is too (a counting sort / prefix pass, whose
  output order does not depend on which thread got there first).
- Rasterized additive splatting is also deterministic (blend order follows primitive order, which the
  API guarantees) — but it is the least flexible of the three.

Recommend **binned gather**, falling back to fixed-point atomics if the bin build proves awkward.
Either way, state the choice in the wave and pin it with a cross-process byte-identity fixture at a
few hundred lights, which is the only way this stays honest.

### 9.4 Why this does *not* argue for switching to cone tracing

The counter-proposal writes itself: once a per-frame lit voxel grid exists, cone-trace it and get
sharper GI plus specular. One engine-specific fact decides it:

> **`--scale` goes to 4.0** (`RenderScale::MAX`, `L4011`), i.e. **up to 16× the pixel count.** Any
> per-pixel GI cost is multiplied by that. A probe volume's cost is *independent of resolution* — the
> per-pixel part is a texture fetch — while a cone trace pays the full multiple.

Supersampling is the engine's only AA (TAA rejected, MSAA rejected, FXAA trigger-gated), so it is not
an optional mode to sacrifice. Probes stay the recommendation, and the injection grid is shared work
either way — building probes first forecloses nothing.

### 9.5 The grid has three consumers, so build it once

This is the strongest architectural argument in the section, and it changes the ordering of two
plans rather than just this one:

1. **Clustered forward (the cap removal)** needs exactly a per-cell light list.
2. **GI's dynamic injection** needs the same binning (§9.3 stage 1).
3. **The volumetric medium** needs it too — and this one is a content win nobody has asked for yet:
   `medium.wgsl` in-scatters an *analytic ambient* term and `shafts.wgsl` marches the **sun's shadow
   map only**, so today point lights do not scatter in the fog at all. A binned light grid is the
   missing input. **Tracer rounds lighting the night mist** is precisely the G7 "would you show
   someone?" register, on a night city that already ships a medium.

So: **the light-binning grid belongs to the many-lights arc, not to this plan**, and this plan should
declare a dependency on it rather than grow its own. If GI's dynamic half is built first it will
build a second binning structure, and *"before adding a second way to do something, remove the
first"* will have been violated by construction. Recommended order: cap removal + binning grid →
GI dynamic injection rides it → medium in-scatter rides it.

Note also that Alternative E (§3) stops being a competing arc under this constraint and becomes a
**shared component** of it.

**Satisfied for the array half — `Plan/clustered-forward.md` shipped the cap removal** (which also
dates §1 fact 3: `LIGHT_COUNT` and `extract::select_lights` no longer exist). Its decision 8 states
the seam precisely: *"what is shared is the light array and the binning code shape, not the grid…
This plan's grid is view-frustum-space — the right structure for a camera pass and the wrong one for
GI's world-space injection or the medium's in-scatter. Those consumers bin the same deterministic
array into their own world-space cells when their arcs land… What this plan owes them: the array's
order contract (ascending EntityId), its premultiplied-radiance convention, and the `Light` struct
staying the single wire format for 'a light, GPU-visible.'"* So this plan inherits the array and the
counting-sort shape (`cluster.rs`) and does **not** inherit a grid: the world-space bin stays this
plan's own to build, which is exactly what "build it once" bought — one light array, not one
structure forced to serve two spaces.

### 9.6 The finding that cuts against my own §8

Worth stating plainly because it is the opposite of what §8 argued: **this constraint weakens the case
for changing the temporal rule.**

§8's headline justification for relaxation was *"dynamic lights bounce."* Under §9.2 that is delivered
by a **single-frame** mechanism which needs no history, no relaxation, and no forced-converge path —
and which is strictly better for this light population than amortization would have been. What the
relaxation still buys is narrower: spreading the *static* half's convergence over frames instead of
paying it at load, and multi-bounce quality on slow-changing light.

That is not an argument against changing the rule. It is an argument that the rule change is now an
**optimization of the static half**, judged on its own merits (§7's costs — the live-vs-screenshot
divergence, the two-routes gate), rather than the enabler of the feature you actually want. **D7/D8
get easier, not harder**, and if the answer to D8 is "the live-view lag is unacceptable," dynamic GI
survives that verdict intact.

### 9.7 Rejected, with reasons

- **ReSTIR and the reservoir family.** The standard answer to "millions of lights" — and rejected
  here on principle rather than cost: its power *is* temporal + spatial reuse, and the temporal half
  in screen space is Class 2 (§7). Stripped of reuse it is a noisy estimator needing a denoiser,
  which is Class 2 again. Naming it explicitly so the question is not re-opened from first principles
  every time someone reads a paper.
- **Per-probe iteration over the light set** — the cost model is wrong (§9.3); it imports the whole
  many-lights problem into the GI stage.
- **Amortizing dynamic light** — §9.2; produces trails, and the trade has no good setting.
- **Shadowed dynamic lights in the GI term.** Hundreds of shadow maps is not a direction. The grid's
  own occupancy provides the occlusion, coarsely, and that is the right amount for an indirect term.

### 9.8 Cheap complements worth keeping on the table

- **Analytic ground-bounce lights.** Per projectile *cluster* (not per projectile), place one virtual
  light at the surface below it with the surface albedo, and let it ride the clustered-forward path.
  Near-free, artistically directable, and it captures the money shot for a top-down game — a bullet
  dragging a warm pool across cobblestones. If the schedule ever forces a choice, **this is the
  highest value-per-line item in the whole document.**
- **Screen-space bounce (Alternative B) gets stronger under this constraint, not weaker.** Its cost is
  **O(1) in light count** — the scene colour buffer already contains every light's direct
  contribution, however many there are. And its usual weakness is blunted here: a top-down camera
  frames the action, so projectile light is nearly always on-screen. It remains Class 1 only without a
  temporal denoiser (dither + the existing exactly-cancelling box blur), which caps its quality — but
  as the *contact-scale* half beside the volume's *coarse* half, the complementarity is now sharper
  than §3 gave it credit for.

### 9.9 Second-order consequences to plan for

- **Energy and double-counting.** The bullet's direct light already reaches the ground through
  Forward+; the GI term must add **only** the bounce. And hundreds of dim lights summing is correct
  physics but needs a global dynamic-GI strength knob to stay in the grade's range (AgX plus the
  vignette will absorb a lot, which is a reason to author it *after* the grade, not before).
- **Cell-crossing flicker.** A light moving between cells must not pop. The splat has to be a **smooth
  trilinear** deposit, never nearest-cell. Easy to get wrong, cheap to get right, and the single most
  likely source of "the GI shimmers when I strafe."
- **`WorldHash` moves when projectiles gain lights.** `PointLight` is a registered component, so
  attaching one to a projectile is a hash-moving edit: re-record `run.lrp`, the standard ritual.
- **`RenderScene.lights` and the uniform.** Removing the cap deletes `select_lights`' nearest-16 path
  and makes `LIGHT_COUNT` (reflected from WGSL into `shader_gen`) meaningless; the fixed uniform array
  becomes a storage buffer. That is the many-lights arc's work, but this plan depends on it and should
  say so.
- **Budget.** The injection + gather is a per-frame cost on the M4, beside a post chain already
  measured at 4.6 ms wall. It gets its own budget group from the first commit, measured on
  `stress-horde` with the bullet count that scene actually produces — measured, not assumed.

### 9.10 Where it lands

§4's W0/W1 and §8's W2 are unaffected. The dynamic half replaces §8's W3 rather than extending it:

- **W3′ — DELIVERED (2026-08-08, Lore revs 539–549; `Plan/gi-wave3-dynamic-injection.md` is the
  spec and outcome record).** Shipped with two corrections to this section's assumptions: the bin
  is this plan's own (`gi_bin.rs` — clustered forward's grid is view-frustum-space, the wrong
  domain, exactly as D10's alternative anticipated), and there is **no splat and no per-frame
  march at all** — the bake writes a per-(probe, ray) hit cache (16 of 64 rays, **strided**, since
  a contiguous Fibonacci prefix is a polar cap — the spec's R3 correction) that the `gi_inject`
  compute pass replays against a per-frame bin of `GiDynamic`-marked lights. Ships the dynamic
  volume term, the binned gather (D11), the cross-process fixture at 300 lights, the `gi_inject`
  budget, the quality toggle in all its homes, and the calibrator live.
- **W3.5′ (optional, cheap) — analytic ground-bounce lights** for projectile clusters, if the
  measured injection term reads as too soft at contact scale.
- Medium in-scatter from the grid is a **volumetric-medium** plan item, not this one; recorded here
  because this plan's grid is what unblocks it.

### 9.11 New open decisions

All four resolved at W3′ (2026-08-08; the spec's §0/D13 sections carry the full arguments):

- **D10 — RESOLVED: this plan's own bin.** The recommendation inverted on inspection: clustered
  forward's grid is view-frustum-space and the gather asks a world-space question, so `gi_bin.rs`
  inherits `cluster.rs`'s counting-sort *shape* over a different domain — and deliberately not its
  loose-binning stance (deterministic, fixed-order, with the aggregation-makes-it-value-bearing
  rule pre-written).
- **D11 — RESOLVED: binned gather.** Atomics are not merely unchosen but absent from the design
  (one invocation per probe, no cross-invocation reduction); the cross-process byte-identity
  fixture runs at the 300-light cap.
- **D12 — RESOLVED: irradiance-only**, one `Rgba16Float` volume, DC only — justified twice by D13
  (single-digit organic population, faint ground-up bounce). SH-L1 upgrade stays a recorded
  additive with its trigger in the spec's §16.
- **D13 — RESOLVED by measurement** (probe harness on real gameplay): peak 6–7 simultaneous
  projectile lights, mean 2–5, not spatially clustered; the 300 "cap" is an authored constant, not
  an observed population. The design is sized to the cap and shaped for single digits; stage-1
  aggregation deferred with a measured trigger.

---

## 10. Open world: when the dense grid dies, and what replaces it

> Added 2026-08-01 at user direction. The question: the user's original proposal — a TLAS of
> instance bounding boxes over per-mesh voxel BLASes — evaluated **under an open-world assumption**
> rather than the bounded-district assumption §3 priced everything against. The assumption changes
> the answer, and one argument made against BLAS in the discussion was wrong and is corrected here.

### 10.1 The bound that reframes the contest

Dense world-space storage is O(loaded world area). A district (132 × 144 × ~24 units at 0.5-unit
cells) is ~4 M cells / ~15 MB — trivial, which is why §3 never had to think about it. A 1 km²
streamed city at the same resolution is ~320 M cells / **~1.3 GB** — dead, and no streaming radius
makes it respectable at fine resolution. So at open-world scale the contest is not "grid vs BLAS";
it is between two **sparse** structures:

- **(a) TLAS/BLAS** — one object-space voxel grid per *unique asset*, shared by every instance of
  it, plus a top-level index over instance AABBs (the user's proposal).
- **(b) A camera-centered clipmap / brick map** — a sparse or cascaded world grid that revoxelizes
  as the camera moves (the VXGI/Teardown shape).

### 10.2 Where BLAS genuinely wins at that scale

- **Memory scales with unique assets, not world area.** A kit-instanced city is BLAS's best case:
  a few hundred unique models at ~10³–10⁴ cells each is nothing, and 100 k instances × ~32 B of
  TLAS entry is ~3 MB. The clipmap's fixed footprint is also affordable — but it buys resolution
  that *degrades with distance*, where BLAS keeps full resolution everywhere.
- **The BLAS is literally the authoring format.** A `.voxel.yaml` model's grid ships with the
  asset: zero build cost, exactly conservative, palette already carrying albedo/emissive. And the
  dominant non-voxel geometry — the map kit's merged-cube floors and ramparts — needs no
  voxelization at all: an axis-aligned box is *exact* under analytic ray intersection, so the TLAS
  leaf **is** the geometry.
- **Occlusion at authored resolution.** Vigil's walls are 0.2-unit voxels; a 0.5-unit world grid
  *undersamples* them, while an object-space BLAS traces them at native resolution — everywhere,
  at any distance.
- **Streaming and dynamics are its natural habitat.** Chunk stream-in is a list append, no
  voxelization pass. A moving rigid occluder is a TLAS entry update per frame — §8's boss trigger
  becomes near-free, where a world grid pays a revoxelization.

**Correction, recorded honestly:** the discussion's argument that per-instance voxelization "leaks
at seams by construction" applies to *re-voxelizing triangle meshes* into per-instance grids. It
does not apply here: the BLASes are source data, flush placements stay flush, and occlusion is
exact at authored resolution. On leak control BLAS is the **better** structure, not the worse one.

### 10.3 The structural problem: radiance at the hit point

This is the load-bearing issue, and it is about storage, not traversal. A GI ray needs the
**outgoing radiance** of what it hits, not just its albedo. A world grid answers by injection —
splat direct light + emissive into cells once per frame (§9.3), rays fetch. An object-space BLAS
**cannot store lighting**: two instances of one asset are lit differently, and a per-instance lit
cache destroys exactly the asset sharing that makes BLAS cheap.

Two resolutions:

- **DDGI-style hit shading** — at the hit, evaluate albedo × (direct light via the §9 binned light
  grid + *previous probe irradiance at the hit point*, which is also the free-infinite-bounce
  trick). Workable; per-ray cost is real and direct-light shadowing at the hit is approximate.
- **The hybrid (favored):** BLAS for *visibility*, a coarse world-space radiance cache for *what
  light is where* — and the cache is §9's injection grid, which clustered forward and the medium's
  in-scatter already want. Occlusion at authored resolution, radiance at injection resolution, and
  no second structure is built (§9.5's rule holds).

### 10.4 Honest costs

- **Two-level traversal in WGSL is real work**: a coarse world grid of instance lists as the TLAS
  index (a flat list is O(n) per ray), a ray transform into object space per candidate, and
  divergent inner loops on incoherent probe rays. Tolerable at amortized-probe ray counts
  (~10⁴–10⁵ rays/frame), and materially more code than one DDA.
- **The probe volume goes camera-centered under any tracer.** An open world's probes must scroll
  with the camera, which sharpens §7's framing: the image becomes f(world state, camera **pose**) —
  pose as a *parameter* is admissible; camera *history* stays banned. Scroll-in invalidation is a
  legitimate world-event priority under forced convergence — the one place a priority scheduler
  earns its keep over plain round-robin.
- **Watch item:** wgpu's experimental ray-query API. If it matures on both pinned backends,
  hardware TLAS/BLAS over the triangle meshes the renderer already draws replaces the software
  tracer outright — the BLAS direction has a hardware upgrade path a world grid lacks.

### 10.5 What does not change, and the trigger

§4's W0/W1 stand exactly as written at district scale. The probe volume, the `@group(3)` binding,
the `mesh.wgsl` trilinear fetch, and the §9 injection grid all survive a tracer swap; W0's
primitive voxelizer is reused as the injection-grid builder either way. **The tracer is the
swappable part.** Nothing built first forecloses the BLAS path — provided D15 below is answered
so the authored grids stay reachable.

**Trigger:** the city-scale arc's streaming trigger — concretely, a world extent where the dense
grid would exceed ~64 MB or the bake would exceed its ~200 ms budget (§5). When it fires, decide
BLAS-hybrid (favored, per §10.2–10.3) vs. clipmap **on measured traversal cost and memory**, not
from scratch; this section is the written analysis that decision starts from. Note Alternative C's
trigger (§3) and the voxel roadmap's chunked-worlds trigger fire in the same neighborhood, and the
three should be answered together when they do.

### 10.6 New open decisions

- **D14 — At the streaming trigger: BLAS-hybrid or clipmap?** Favored answer recorded (BLAS-hybrid,
  §10.2–10.3); decided then, on measurements, per §10.5.
- **D15 — Should W0 keep the authored voxel grids reachable?** `voxel_mesh.rs` today builds the
  occupancy grid locally inside `mesh()` and drops it; the asset cache retains only `MeshData`.
  W0's voxelizer API should expose a per-model grid accessor (retain or re-derive on demand) so a
  later BLAS build is an assembly job over data that already exists rather than a re-parse — cheap
  now, recorded so W0 does not accidentally close the door.
