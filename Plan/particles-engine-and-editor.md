# Plan: Particles — a stateless engine emitter and its editor surface

> Status: **Wave 4 COMPLETE (2026-09-06) — the plan's mechanized half is in the tree; what remains is the pinned-box measurements and the human sittings.**
> **2026-11-02 log — lit particles (Clockout 0037).** `ParticleEmitter.lit` (serde-default false, omitted from text when false, so no hash, snapshot or replay moved): a lit particle's colour is albedo × the light at its centre, the lit sprite's rule (`snippets/billboard_light.wgsl`, one definition shared with `sprite.wgsl`), shaded once per centre in the vertex stage. Gallery `particles-lit`, `docs/particles.md`. Decision 5 amended below.
> **2026-09-13 log — the held gameplay clock (shmup request 22).** `GameClock` (`ludion_core::time`, in the prelude) is the resource a game holds each frozen frame; the driver folds it at tick start and `SystemCtx` carries `game_frame`/`game_time`/`game_dt`. The kernel stays pure: `extract_inner` reads the clock once and passes gameplay time down, so emitters, flipbooks, clips and volleys freeze together. Snapshotted (the `rng` precedent, omitted when unheld), never a `WorldHash` term, held = 0 the identity — no scene, replay, hash or snapshot moved. `ludion docs game-clock` owns the rules; `crates/ludion-engine/tests/game_clock.rs` is the instrument.
> **2026-10-10 log — par over the emitter list.** Decision 9's first trigger is built: `extract_particles` evaluates emitters through `ludion_core::par` (one emitter a chunk, results concatenated in chunk order, so the list is element-for-element the sequential one; a frame under 4096 live particles runs on one thread and pays no spawn). RTX 3090, 120-frame release profile: extract median **1.129 ms** at 8 threads (1.134 / 1.106 / 1.345 / 1.124) against 2.898 at `--threads 1`; the loop test reads 3.296 ms @ 1 thread, 1.210 @ 8. The parallel reading still passes the 1 ms line, so the second trigger (stateless vertex-shader evaluation) is reached — recorded, not acted on. `extract_ms` re-calibrated 5.0 → 2.5. `tests/particles.rs` pins the list equal at 1/2/8 threads. Zero hash, schema, golden or replay movement; the M4 re-measure is owed.
> Wave 4 log: scale, measured before engineered (decision 9); zero wire / schema / hash / replay movement.
> **The stress scene** `examples/gallery/particles-stress.yaml`: twenty endless emitters over a 60×60
> floor — seven alpha smoke columns (rate 380, life [2.3, 3.3], discs 1.2 → 3.8 at softness 0.88: the
> overdraw register), seven additive HDR ember fountains (rate 830, life [1.0, 1.8]) and six additive
> floor-clipped mote drifts (rate 390, life [3.0, 4.2]) — interleaved in a checkerboard so the depth sort
> alternates blends; 24,006 live by rate × mean life, 21,829 counted at extract, no emitter above ~1,400
> (cap 4,096); no `Environment` / `PostProcess` / sun (the `lights-stress` rule);
> `Plan/artifacts/particles-wave4-stress.png`. The gallery suite reports it `L2201` no-golden-yet beside
> `particles` until the pinned GPUs bless. **The budget key** `Budget.extract_ms`: a ceiling on the median
> of `sim.sequential`'s `extract` entry — the offline driver's `SEQUENTIAL_NAMES` and a live capture's
> both carry one, so the line checks on either host, skipped (never failed) when none exists — the first
> budget line on that metric; `L4004`'s explain text, the CLI module doc and `dev-docs/budget-files.md`
> carry it; `provision.mjs`'s `BUDGET_STEPS` takes an optional `budgetParticlesStress` that neither OS
> passes yet. **The budget files are not written here** — measured on each box, never derived (an
> unmeasured ceiling is a guess wearing a gate's clothes); their headers owe decision 9's two triggers with
> the numbers read. **`budgets/pinned-rtx3090-particles-stress.toml` written 2026-09-07 on the RTX 3090:**
> extract 2.374 / 2.471 / 2.421 / 2.547 ms (median 2.446; the loop test 2.317 for 21,829 particles,
> ~0.106 µs each), `scene` 0.9595 — ceilings 5.0 / 2.0; **decision 9's `par` trigger fires** at 2.3x the
> 1 ms line (the laptop's 7.39 was 3.2x this box). `particles.png` + `particles-stress.png` first-blessed
> there; the M4 twin and both macOS goldens followed on the M4 2026-09-15. **The cost test** `particle_extract_cost_on_the_stress_scene` (`profile.rs`, the
> `cluster_build_cost` shape, `#[ignore]`d): `cargo test --release -p ludion-engine --lib
> particle_extract_cost -- --ignored --nocapture` prints extract per scene — on this laptop **7.39 ms for
> 21,829 particles** on the stress scene against 0.066 ms / 219 on `particles.yaml` and 0.010 ms / 0 on
> `lights.yaml`: ~0.34 µs a particle, all of it the particle share. Decision 9's `par` trigger is **1 ms on
> the reference box**; the laptop reads seven times over it, so the reference read is very likely to fire
> it — recorded, not acted on: `par` over the emitter list is an engine wave only once the reference box
> says so. **The density knob** `QualityParams.particle_density` (`full` / `high` / `medium` 1.0, `low`
> 0.25; override key `params.particle_density` in `0.0..=1.0`, `L0016` outside — `0.0` is legal, every
> plume silenced), spent in extract and nowhere else: `extract_at` gains a trailing `particle_density`
> (every golden, sheet, strip, dump and test passes `QualityParams::FULL.particle_density`; `extract`
> stays the three-argument tick-0 full-density seam), `Engine`'s cached extract passes its setting,
> `render_scene_with_registry` passes the caller's so `ludion render --quality low` thins while the graph
> dump and the voxel / GI sheets stay full; density 1.0 is `thinned`'s identity, so a full extract is
> byte-for-byte the pre-wave extract and every golden stands. **Ruled here:** the axis is *not* in the
> profile report's `QualityProfile` — the descriptor is wire-schema-hashed and this feature ships no
> schema revision (the no-report-section ruling), so a report names it through `preset`;
> `dev-docs/renderer-quality.md` records it as the axis's third exception beside `render_scale`'s. Tests:
> `only_the_cheapest_preset_thins_the_particle_population` + the preset-ladder monotone check
> (renderer), `particle_density_is_bounded_from_zero_up_to_the_authored_population` (override file),
> `low_quality_thins_the_particle_population` (`tests/quality.rs`, `particles.yaml`: 219 → 55),
> `the_engines_cached_extract_thins_the_stress_scene_at_low` (in-crate — the plan's exit assertion,
> through `Engine::extract_render_scene` on the stress scene), the `extract_ms` exceeded / skipped /
> parses trio. Docs: `docs/quality.md` (+ mirror; at 13.5 KB it was already over the 12 KB spoke budget
> before this wave, now 14.3 — the split is owed), `docs/particles.md`, `dev-docs/renderer-quality.md`,
> `dev-docs/budget-files.md` (its stale "no M4 twin for lights-stress" line corrected).
> **Owed on the reference box / the M4:** `budgets/pinned-{rtx3090,apple-m4}-particles-stress.toml`
> measured (the trigger read in the header), `particles-stress.png` first-bless beside `particles.png`,
> `extract_ms` on the E6 run. **Human gate (the plan's exit):** unchanged — the queued sittings.
> Before Wave 4: **Wave 3 COMPLETE (2026-09-05).**
> Wave 3 log: the consumers, engine-side nothing (no wire / schema / graph movement; every committed
> replay re-recorded once at the end). **Every emitter stands on its own scale-1 entity, never on a body**
> — the reference game's voxel bodies sit at a small `Transform.scale` and that scale multiplies an emitter's
> extents, speeds and sizes (the risk this plan recorded, now a rule in that game's own notes).
> (a) **Death**: one `Transform` + `ParticleEmitter` + a game-side countdown component per kill — a
> `rate: 0` burst of embers from a small sphere at `started` = the kill frame, hemisphere `spread_deg` 90,
> `floor` at the ground, `blend: alpha`, colour lerping from an HDR ember to a sooty transparent tone — one
> alpha emitter reading as embers dying into a dark puff (one entity per kill was the ruling; two blends
> would be two entities); `seed` is a hash of the kill's entity index, generation and frame with a salt,
> RNG-free. A reap system counts the burst down after the cull; the old entity-fragment code and its
> tunables are **deleted**. (b)(c)(d) A reconcile system: the want set from every standing emitter-bearing
> body vs the standing ambient-emitter entities keyed on a site name (the body kind is folded in because the
> two site namespaces may collide) — orphans despawned, the missing spawned at the body's translation plus
> the effect's lift, a key wearing a stale emitter **overwritten in place** (an endless emitter is
> steady-state, so nothing visibly restarts), the lift re-applied as a body swells; `seed` is a salted hash
> of the site name. A data-doc block (`#[serde(default)]`, `rate` 0 = no plume, a doc opts in) carries the
> emitter's six motion fields; the *look* is game-wide tunables, with a smaller, slower variant for an
> early-stage body. A per-register table covers the ambient effects, the last register's rising
> motes additive. (e) **Breakout**: `BrickBurst { expires_at }` + `reap_bursts` after `break_bricks` — 24
> particles in the brick's `Sprite` colour from a box of the brick's half-extents, seed = the brick's
> entity index.
> **Rulings here, deviating from the wave text:** the plan's "rising motes over the existing `GroundMarker`
> disc" matched nothing — that effect is one register of the game's own effect table — so consumers (c)
> and (d) are one per-register table; the doc block carries six motion fields rather than `rate`/`extent`
> alone; `budgets/pinned-apple-m4.toml` lists none of these systems and was left alone. **Tests**: the
> game's particle suite (7) — a kill stands one burst with `started` at the kill frame and it is reaped; a
> paused world extracts the same non-empty particle set twice; a drawn body breathes its doc's plume on its
> own entity; an early-stage one is smaller and slower; a plume scales with its register and the last
> register's lifts additive; a body's plume goes with the body; a second reconcile changes nothing — plus
> breakout's brick-break test; the `[systems]` budget gained the two reconcile systems at the 0.05 floor.
> **Re-record**: the game's replays and breakout's `run.lrp` through the replay-scoped bless — two segments
> moved, the others came back byte-identical (no kill or emitter body on their routes); `L1126` clean;
> `cargo test -p ludion-protocol` green. Verified on the laptop: every reference-game binary serially
> (`-j 1`) green including its golden suite — **all seven goldens scored SSIM 1.0000**, so at the boot
> camera the plumes sit below the metric and no re-bless is owed anywhere; the breakout suite green (golden
> untouched); map-drift and voxel-drift unmoved; `--replay-verify` on both re-recorded runs; the
> 1/2/8-thread identity gates over the new corpus for both games; fmt; clippy `-D warnings`.
> Because the goldens cannot see the plumes, the eye's instrument is a game-side golden test,
> `particle_consumers_draw_and_report`: three inline fixtures with their own close cameras, each rendered
> twice, the twin with every `ParticleEmitter` stripped, the pair written to
> `target/particles-report/<name>[-control].png`, only byte-inequality asserted (the look is the
> sitting's; the SSIM against the control is printed: 0.93, 0.97, 0.99). Three screenshots of these
> fixtures are kept in the private tree; the faintest registers read faint against the surrounding
> emissive (retune candidates for the sitting, not defects). **Owed on the reference box:** the E6 GPU run
> with `extract_ms` read. **Human gate (the plan's exit):** the queued sittings — does the death read
> better than the cubes, do the ambient bodies read as alive; the ambient effects are small at the follow
> camera and the sitting may retune them. Before Wave 3: **Wave 2 COMPLETE (2026-09-05).**
> Wave 2 log: the editor half, zero wire movement (29 methods; the parity vitest still walks every method
> string in `src/`). Host: `stage.rs` generalized to `StageSlot { Model, Effects }` — one `Stage`
> implementation, a slot naming its port (9412 / 9413), its missing-scene hint and whether the `voxels/**`
> watcher runs (model only); every `StageStatus` is stamped with its slot over the one `stage://status`
> stream, `AppStages` holds both and stops both on exit, the four commands are slot-first in `host.ts`.
> Stage content: the reference game's committed effects-stage scene (dark floor, one point light, fixed
> camera, `subject` = an endless additive ember plume clipped by `floor: 0`), reviewed through
> `Plan/artifacts/particles-wave2-effects-stage.png`. View: `effects-artist` in the registry (label
> `Effects`), `views/EffectsView.tsx` (stage lifecycle + layout, the Voxel Artist's shape with
> `StageViewport interactive={false}`) mounting `editor/effects/EmitterEditor.tsx` only once the stage is
> dialed — the one `useComponentCommit` (sliders, Restart and the loop share it; no undo, no dirty flag,
> no save: contract 6), `restart()` = `component.set` of `started` to the frame read off a `component.get`
> reply envelope (every reply dispatched through the sim carries `frame`), the loop re-firing it from the
> poll once the frame passes `started + duration + life[1]` (`effects/time.ts`, pure + tested),
> pause/step/speed = `sim.pause` / `sim.resume` / `sim.step {ticks: 1}` / `sim.set_speed`;
> `EmitterPanel.tsx` = the tab strip (`Pattern` declared, disabled, naming `Plan/projectile-patterns.md`),
> the time strip, and the inspector's own exported `FieldRow`s. Engine: `x-ui-range` on the six remaining
> register fields (`extent` 4, `burst` 512, `duration` 10, `life` 5, `speed` 20, `floor` ±4), `started` /
> `seed` deliberately without (`dev-docs/ludion-ecs.md`, the third no-travel ruling), pinned by
> `particle_emitter_travel_covers_every_register_field`; `inspect-fields.test.ts` carries the captured
> `schema_for!(ParticleEmitter)` and pins all 19 fields typed, none in the JSON bucket, both colours
> swatched, `shape` / `blend` as dropdowns. Palette: the `particles` archetype (`ParticleEmitter: {}`),
> order pin updated. Persona: `editor/personas/effects-artist.md` (live-edits the stage — the opposite of
> the Voxel Artist's file contract; never `scene.load` / `scene.save` / `asset.reload` it).
> The editor's private notebook carries the decisions; the live-smoke list is `Plan/editor-live-smoke.md`. Deviations from the wave text, ruled
> here: the view id is `effects-artist`, not `effects` (the drawer resolves the persona as
> `<personasDir>/<viewId>.md`); `started` / `seed` carry no travel (a slider over a frame stamp is the
> scrubber decision 6 rejects; a seed has no adjacent values); the `started` write goes through the shared
> commit hook, not a helper of its own (one write path); the hook lives in a child mounted after the dial,
> never behind a placeholder client. Verified on the laptop: `check.mjs --frontend-only` green (oxlint /
> tsc / vitest); ludion-engine fmt / clippy `-D warnings` / the travel pin green; the Tauri host lane (fmt
> / clippy `-D warnings` / test) green; `check-scene` on the stage OK; the artifact rendered on
> `NVIDIA GeForce GPU (Vulkan)`. **Human gate outstanding:** the live smoke at the next editor sitting
> (`Plan/editor-live-smoke.md`). Before Wave 2: **Wave 1 COMPLETE (2026-09-04).**
> Wave 1 log: `ParticleEmitter` + `EmitShape` + `ParticleBlend` (24th registry entry, `register_validated`,
> `L2107`, both registry tripwires moved 23 → 24, no hash/golden/scene moved); `particles.rs` — the
> `pub` kernel (`Motion` of ≤ 4 `Segment`s, `Segment::Ballistic { velocity, gravity, drag, duration }`
> with the exact closed form through `ludion_core::math::exp`, `position_at`) and the evaluator
> (`live_window` with the steady-state negative indices and the cap clamp, `evaluate(emitter,
> transform, now, density, basis)`, two salts, the fixed draw order life → speed → direction → offset);
> `extract_particles` in `(ascending EntityId, particle index)` order off a shared `CameraBasis` the
> sprite path now also uses; renderer `ParticleInstance` + `particle.wgsl` + `build_particle_draw`
> (one premultiplied pipeline, additive as a per-instance flag, `total_cmp` back-to-front sort, `None`
> when empty) drawn after sprites before text; `render --frames N` and `--frames a,b,c` over the sweep
> stitcher (`render_scene_with_registry` gained a trailing `frames: u64`); `examples/gallery/particles.yaml`
> + `Plan/artifacts/particles-wave1-frames.png`; `crates/ludion/examples/particle_burst.rs`;
> `docs/particles.md`, the `docs/components.md` split (PointLight calibration → `docs/lighting.md`,
> `GroundMarker` → the new `docs/ground-telegraphs.md`) and the 24th row; dev-docs updated. Proofs in
> the tree: hash-neutrality (`tests/particles.rs`: identical `WorldHash` trace + byte-identical `.lrp`
> extracted every frame vs never), `extract_is_deterministic` over emitters, the minimal-graph dump
> snapshot, GPU byte-identity of a `particles: []` frame, additive-only-brightens, 17 evaluator units.
> Deviations from the wave text, ruled here: `evaluate` takes the camera basis as a fifth argument (the
> instance carries `right`/`up`); `spread_deg`/`softness` bounds are schemars `L1014`, `L2107` covers
> the rest; seven fields already carry `x-ui-range` (Wave 2 completes the set); the steady-state test
> compares age spreads up to the emission-grid phase (tick 10,000 is not on a 1/rate grid). Verified
> on the laptop (which now renders on `NVIDIA GeForce GPU (Vulkan)`): fmt / clippy `-D warnings` /
> shader-drift / map-drift / voxel-drift / tauri lanes green; serial per-crate test lanes green except
> the known laptop reds (renderer `halation_*`, CLI `import exit_gate`, testkit's five exactly-1.0
> pinned-GPU goldens); the gallery tolerance suite 14/23 with the same eight provenance reds plus
> `particles` at `L2201` no-golden-yet. **Owed on the reference box / the M4:** first-bless
> `particles.png` on both pinned GPUs, the Windows `check.mjs` lane.
> Before Wave 1: **DESIGN SETTLED (2026-09-04, grilling session — every ruling the user's).**
> Authored 2026-09-03 at the user's direction as the feature `Particles engine and editor` (private
> notebook); revised 2026-09-04 after the grilling: the evaluator is a
> **shared kernel** whose gameplay sibling is `Plan/projectile-patterns.md` (hashed `Volley` entities and
> `kind: pattern` docs on the same closed form), the editor surface is an **Effects view** (decision 6),
> and the three `[USER CALL]`s are ruled (decision 12). This plan
> **fires the particle trigger by direction, not by measurement**: the three recorded conditions
> (a private plan's sitting judging the entity fragments insufficient;
> `Plan/phase-7-designer-flow.md:96` — feedback exceeding what sprites fake;
> `Plan/sprite-engine.md:134` — entity-spawned fragments blowing the stress budget) are all still
> *unmeasured* — the merged sitting has not happened and `shmup-stress` carries no fragment
> content — and `Plan/telegraphs-and-vfx.md:162` (decision 7) deliberately declined to fire it for
> the ambient-body visuals. The user opened the feature anyway; this plan records that the triggers were
> pre-empted, keeps the sitting as the exit gate, and builds the smallest system that carries the
> four consumers the reference game needs. Every claim of current-code fact below was read from the tree
> at Lore rev 665 on 2026-09-03.
>
> **Relationships.** Kernel supplier of `Plan/projectile-patterns.md` (the gameplay half — bullets as
> closed-form volleys, sequenced after this plan's Wave 1 and `Plan/sprite-engine.md` W1). Engine sibling
> of `Plan/telegraphs-and-vfx.md` (whose `GroundMarker` pass is
> the structural model: a conditional draw inside the scene pass, zero golden churn by construction)
> and of `Plan/voxel-animation.md` (whose `Animation { started: FrameIndex }` + `extract_at(now)`
> seam is the *time* model: visual-only by construction, sim time passed as a parameter). Consumed
> by the reference game (its ambient-body visuals, death fragments and growth effects). Anticipates one
> `Plan/sprite-engine.md` ruling (premultiplied alpha, one pipeline, additive as a per-instance flag)
> so the two blend paths agree when that plan lands; textured particles wait on its atlas. Rides
> `Plan/scalability.md`'s `QualityParams` for its one per-machine knob. The editor half is an Effects
> view in `Plan/editor-voxel-artist.md`'s stage-session shape (its emitter tab carries the field-slider
> rulings of `Plan/editor-field-sliders.md`; its pattern tab is the sibling plan's); **the wire surface
> stays at 29 methods** across both plans — every editor affordance is an existing command.

## Why

The reference game needs continuous, high-count visuals in four places, and today every one of them is
either an entity burst or nothing: pulsing organic bodies, a tear that streams motes, a projectile
volley, and a death effect. The juice pass answered every death with 5–8 emissive cubes because that was
the only mechanism. The engine has no way to draw two hundred short-lived
glowing points from one entity; a body that "pulses" with motes, a tear that streams motes, a death
that bursts into embers, a bolt that trails sparks — each would cost one hashed entity per point,
paid in `WorldHash` bytes, ECS churn and replay size. This plan adds one component and one draw so a
scene can author those effects as data, an agent can judge them from a render, and a designer can
tune them by slider in the editor — with the sim, the hash and every committed replay untouched by
construction.

## What exists today (verified)

- **Fragments are the only FX mechanism, and they are hashed entities.** `spawn_fragments`
  (the reference game's death code) spawns 5–8 `Cube` entities per death with a hashed launch vector,
  each a `Transform` + `Mesh` + `GiDynamic` + a fragment marker; a second system integrates, tumbles,
  shrinks and despawns them. It is the burstiest spawn in the game. Every fragment is a `WorldHash` term
  and a replay term.
- **The renderer has exactly two alpha-blended world primitives**, both drawn inside the `scene`
  pass after the opaque meshes and the sky (`crates/ludion-renderer/src/render.rs:2186-2215`):
  ground markers (`GroundMarkerInstance`, `scene.rs:187`; instanced quads, analytic SDF, unlit,
  `depth_state(false, Less)`, `build_ground_marker_draw` at `render.rs:1109` returning `None` for an
  empty list — the golden-neutrality inversion) and sprites (`SpriteInstance`, `scene.rs:91`; CPU
  billboarding in `extract.rs:901-959`, view-depth sorted and texture-batched in
  `build_sprite_draws` at `render.rs:1049`, **lit** two-sided through the cluster loop in
  `sprite.wgsl`, straight-alpha `alpha_blend()` at `render.rs:3422`, `depth_state(false,
  LessEqual)`). Text follows. Neither primitive is what a particle wants: sprites are lit and
  texture-batched, markers lie flat.
- **The graph derives order from reads alone** and has no write-after-write edge
  (`Plan/telegraphs-and-vfx.md` Status, `dev-docs/ludion-renderer.md` Rules), which is why markers
  draw inside the scene pass rather than as a trailing pass. A particle draw inherits the same
  constraint. `PostChain::for_scene` (`graph.rs:466`) lists every content flag that adds passes; the
  sky, IBL and markers add none.
- **Time already reaches extract as a parameter.** `extract_at(world, viewport, root, now)`
  (`crates/ludion-engine/src/extract.rs:214`) exists for exactly one consumer, the assembly clip
  evaluator (`animation_pose`, `:1314`; `elapsed_seconds`, `:1337` — ticks since `started` ×
  seconds-per-tick, never accumulated). The live path passes `self.driver.time()`
  (`engine.rs:2014`); the one-shot paths pass tick 0 (`extract`, `:188`). `Animation`
  (`components.rs:2164`) is the model of a visual-only, hashed data bag: `started: FrameIndex`,
  `speed`, the pose derived at render time, proven hash-neutral (`Plan/voxel-animation.md`).
- **`RenderScene` is plain data, `PartialEq`, and its extract is a pure read of the world**
  (`scene.rs:698`; `dev-docs/ludion-renderer.md` Rules; `tests/extract.rs:82`
  `extract_is_deterministic`). The renderer's byte-identity rule — same scene, same adapter, same
  bytes across processes; no temporal accumulation, test mode is the only mode — binds anything new.
- **Every one-shot render is a tick-0 frame.** `render_scene_with_registry`
  (`crates/ludion-engine/src/render.rs`) documents "the render is a static tick-0 frame either way";
  the module header says it "runs **zero ticks** for now". The gallery suite (`ludion test --dir`,
  `crates/ludion-cli/src/test.rs`; `ludion_testkit::golden` header line 10) goes through the same
  path. `ludion render --set`/`--sweep` (`main.rs:332`) override any component field by JSON pointer.
- **The registry is append-only and 23 deep**; `AudioEmitter` is last
  (`components.rs:2419-2503`). A new component appends 24th and moves no existing hash, golden or
  canonical scene (`dev-docs/ludion-engine.md` Rules). A component's semantic validator registers
  through `register_validated` (the `GroundMarker` shape, `:2482`, `L2106`). Highest render-domain
  code is `L2106`; **`L2107` is free**. Highest CLI code is `L4013`; `Plan/sprite-engine.md` has
  reserved `L4014+`, so this plan takes no CLI code.
- **Per-particle randomness has a sanctioned constructor**:
  `DeterministicRng::derived_salted(world_seed, frame, salt)` (`crates/ludion-core/src/rng.rs:221`)
  + `next_f32` (`:261`), the two-stream rule of `dev-docs/determinism.md` rule 3.
- **The profile report already measures extract per frame** (`extract_ms`,
  `crates/ludion-protocol/src/perf.rs:384`), so a CPU cost added inside extract is visible in every
  live capture with **no schema revision** — the additive `cluster` section cost every committed
  replay a re-record, and this plan avoids that by adding no report section.
- **The editor's inspector is schema-driven and edits any registered component with no editor
  code** (`editor/app/src/viewer/inspect-fields.ts`): a flat struct of numbers, fixed arrays, bools
  and unit enums gets typed widgets; **a nested struct or an internally-tagged enum falls to the
  raw-JSON widget** (header comment, lines 10–30; `GroundMarker.shape` degraded exactly so —
  `Plan/ui-widgets.md` W3 log). Sliders come from `x-ui-range` (`components.rs:181` precedent); a
  colour swatch comes from a 3/4-vector whose label matches `/colou?r/i` (`inspect-fields.ts:227`).
  The palette (`editor/app/src/editor/palette.ts:37-138`) is hand-curated; an archetype is a
  ready-to-send `entity.spawn`. The view registry (`editor/app/src/shell/views.ts`) has six built
  views; personas (`editor/personas/*.md`) pull `ludion docs` on demand.
- **`docs/components.md` is 12,363 B — 363 B over its 12 KB spoke budget**; the split is owed
  (a private TODO) and this plan's docs step pays it before adding a row.
- **Quality has a parameter struct** (`crates/ludion-renderer/src/quality.rs:38` `Quality`, `:158`
  `QualityParams`: `ssao_samples`, `shaft_steps`, `dof_taps`, `shadow_map_size`, `pcf`, `gi_rays`),
  masked per machine, FULL for goldens (`Plan/scalability.md`).
- **This checkout is a low-power laptop** (Intel/Dx12): it renders, so an agent can look, but it
  can bless no golden and measure no frame budget. Each wave's verification below is split into
  *here* and *owed on the reference box / the M4*.

## Decisions

**1. Particles are a pure function of `(emitter, sim time)`, evaluated at extract — never entities,
never a pool, never a GPU simulation.** A `ParticleEmitter` component describes an effect; each
frame `extract_at(now)` computes the live set analytically: particle *i* of a continuous emitter is
released at `t_i = started + i / rate`, a burst's *j* at `started`; a particle's age is
`now − t_i`, alive while `0 ≤ age < life_i`; its position is the closed-form ballistic solution
under gravity and linear drag; its size and colour are lerps over `age / life_i`; every per-particle
draw (life, speed, direction, spawn offset) comes from
`DeterministicRng::derived_salted(seed, FrameIndex(i), salt)` — so the same `(emitter, now)` yields
the same particles in any process, on any thread count, after any snapshot/restore. Only the live
window is iterated (`rate × life_max + burst` particles at most), never the history.

**The evaluator is a public kernel with two consumers.** `ParticleEmitter` (this plan) builds one
ballistic segment from its flat fields and draws discs; `Volley` (`Plan/projectile-patterns.md`) builds a
bullet kind's segment list from a `kind: pattern` doc and draws sprites — and its *sim-side hit test
calls the same function*. That is why evaluation is CPU Rust and stays so: the sim may read it, so there
is exactly one implementation of the motion and nothing to drift. The kernel's shape from Wave 1:
`Motion` = a list of at most four `Segment`s with `position_at(t)`, the `ballistic { gravity, drag }`
variant here, the `ramp` and `turn` variants added by the sibling plan as enum variants (engine-internal
`pub` types, never a component field — `ParticleEmitter` stays flat).
- *Rejected — entity per particle (today's fragments):* every point is a `WorldHash` term, a replay
  record, an ECS spawn/despawn and an inspector row; `Plan/sprite-engine.md:134` names this the
  budget breaker, and the ambient-body registers need hundreds of points per body.
- *Rejected — a stateful CPU pool in a resource stepped at the frame boundary:* the picture would
  depend on state outside the world. `engine.restore` would not restore it, so a restored session's
  screenshot differs from the recorded session's; `extract` would stop being a pure read; a paused
  engine rendered twice could differ. Every one of those is a determinism-tier guarantee a client
  already leans on.
- *Rejected — a stateful GPU compute simulation:* temporal accumulation on the GPU, which
  `dev-docs/ludion-renderer.md` forbids ("test mode is the only mode"), and a per-adapter,
  per-process image. A **stateless** GPU evaluation (the vertex shader computing particle *i* from
  the same closed form) is the recorded scale trigger (decision 9), not the first cut: CPU evaluation
  keeps one implementation of the motion that the tests, the renderer and the sibling plan's hit test
  all read (the `part_placements` lesson of `Plan/voxel-animation.md`: never two pose implementations).
  Ruled 2026-09-04: CPU first at low thousands on screen; a stateless GPU evaluation of the same closed
  form only behind decision 9's number; a stateful GPU sim never.

**2. Particles live in the emitter's frame; a trail is a burst left behind.** The closed form knows
the emitter's transform *now*, not where it was when particle *i* was released, so every particle's
offset is anchored at the emitter's current translation; `Transform.rotation` orients the emission
cone (its axis is the emitter's local +Y) and gravity stays world −Y; the uniform `Transform.scale.x`
multiplies the spawn extent, speeds and sizes (the `Sprite`/`GroundMarker` "authored × entity
scale" convention). A moving emitter therefore *carries* its plume. The game-side idiom for a trail
or a stationary spill from a moving thing is one short `duration` burst emitter spawned where the
thing was and reaped on a countdown — the `Fragment`/`ChargeTelegraph` reconcile shape
(`movement.rs:414`) at one entity per burst instead of one per point. Recorded as a limitation, not
hidden: a world-space `space:` option is the first thing a stateful design would offer and the
first thing this design cannot.

**3. One flat component, `ParticleEmitter`, widgetable by the generic inspector as-is.** Every field
is a scalar, a fixed array or a unit enum, because a nested struct or a tagged enum is the raw-JSON
bucket (`inspect-fields.ts` header) and the editor half of this plan is worth nothing if the
emitter edits as a text box. The 24th registry entry, `register_validated`, `L2107`:

```yaml
ParticleEmitter:
  shape: point            # point | sphere | box | disc (unit enum -> dropdown)
  extent: [0.5, 0.5, 0.5] # sphere: x = radius; box: half-extents; disc: x = radius in the local XZ
  rate: 40.0              # particles / second, continuous; 0 = burst only
  burst: 0                # particles released at `started`
  started: 0              # FrameIndex, the `Animation.started` convention
  duration: 0.0           # seconds of continuous emission after `started`; 0 = endless
  life: [0.8, 1.4]        # seconds, [min, max]
  speed: [1.0, 2.5]       # units / second, [min, max]
  spread_deg: 30.0        # cone half-angle about local +Y; 90 = hemisphere, 180 = sphere
  gravity: 0.0            # world -Y acceleration (a negative value lifts)
  drag: 0.0               # 1 / second, exponential
  floor: -3.4e38          # world Y below which a particle is not drawn (the `Fade.height_min` shape)
  start_size: 0.25        # world units, lerped to end_size over life
  end_size: 0.0
  start_color: [1, 1, 1, 1]   # linear RGBA, HDR-capable; lerped to end_color over life
  end_color: [1, 1, 1, 0]
  blend: alpha            # alpha | additive
  softness: 0.5           # 0 = hard disc, 1 = gaussian falloff
  seed: 0
```

`L2107` rejects non-finite or negative dimensions, `life`/`speed` with `min > max`, `spread_deg`
outside `[0, 180]`, `softness` outside `[0, 1]` (where schemars can express a bound the schema
rejects it as `L1014` and the validator covers only what it cannot), and a live population
`rate × life[1] + burst` above `PARTICLE_MAX_LIVE_PER_EMITTER` (4,096 — a typo in `rate` must be a
diagnostic, never a frozen machine). Field names ending in `_color` are deliberate: that is the
swatch rule. Names, defaults and the cap are ruled as listed (decision 12).
- *Rejected — a `kind: effect` data-doc referenced by path:* a preset document is the right shape
  once one effect is authored on many entity kinds, and it belongs to the Game Designer's doc forms;
  but the reference game spawns every consumer from code (its tunable consts are the presets), scenes author
  few emitters by hand, and a doc would put the inspector's live edit one indirection away.
  Deferred with its trigger below.
- *Rejected — keyframed size/colour curves:* the four consumers are two-point lerps; curves are the
  doc's job when the doc exists.

**4. Endless emitters are in steady state at every instant, including frame 0.** With
`duration: 0` the emitter has "always been running": the live window admits negative indices, so
`extract` at tick 0 already shows the full plume. That is what makes an authored ambient emitter
render in a gallery golden, a district golden and the editor's first frame without any warm-up,
and it costs nothing (the window is the same size at every `now`). `started` therefore matters only
to bursts and finite emitters — game code stamps it when the *choice* happens, and re-stamping it
every frame restarts the effect every frame (the `Animation` doc's own warning).

**5. One unlit, camera-facing, analytic-disc pipeline drawn inside the scene pass after the sprites
and before the text; no new graph pass; an empty list issues no draw.** `RenderScene.particles:
Vec<ParticleInstance>` (centre, the camera-basis right/up the sprite path already computes, size,
colour, softness, blend flag) in `(ascending EntityId, particle index)` order; the renderer sorts
back-to-front by view depth with the sprite path's NaN-safe index tie-break, packs one instance
buffer and issues one instanced draw with `particle.wgsl`: the six corners from the vertex index
(the sprite convention), a radial falloff shaped by `softness`, `fwidth` anti-aliasing at the hard
end (the marker convention), premultiplied output, `depth_state(false, LessEqual)`, HDR target.
Additive is a per-instance flag that forces the output alpha to zero under a `(One,
OneMinusSrcAlpha)` blend — `Plan/sprite-engine.md` decision 4's trick — so alpha and additive
particles share one pipeline, one buffer, one draw and one sort. An HDR colour rides bloom exactly
like an emissive mesh. Empty ⇒ no pipeline bound, no draw ⇒ every existing golden byte-identical
and the graph untouched (`a_ground_marker_scene_dumps_the_unchanged_minimal_graph`'s sibling test
pins it). Cost is attributed to `scene` in `--dump-timings`, as the sky's and the markers' are.
- *Rejected — a dedicated `particles` pass:* the write-after-write ordering problem the telegraph
  plan hit, and no way to depth-test against the scene's own attachment from a later pass.
- *Rejected — reuse the sprite pipeline:* it is lit (a cluster loop per fragment, wrong for
  embers), texture-batched (the white fallback per draw), straight-alpha, and cannot do additive
  without a second pipeline. A textured particle waits on the atlas regardless.
- *Rejected — soft-depth intersection, per-particle rotation:* each is deferred with a trigger below.
- *Amendment 2026-11-02 — opt-in lit particles (Clockout 0037).* Decision 5 held "unlit"; a mote pool in a lamp (a request from a shipping game) needs the opposite, so `ParticleEmitter.lit` (default false) opts an emitter in. The rule is the lit sprite's `shade_two_sided` (ambient, point, spot, sun; two-sided, unshadowed, no IBL), moved to one snippet shared with `sprite.wgsl`. It runs once per particle centre in the vertex stage: a CPU twin would be a second copy of the WGSL rule and book its cost to `extract_ms`, and per-fragment shading would cost a cluster loop per pixel for a flat disc. The unlit path is byte-identical (the flag rides the formerly padded `params.w`).

**6. Zero wire movement; the editor surface is an Effects view in the Voxel Artist's shape, plus a
palette archetype.** The view owns a stage session (a second host-spawned `ludion serve`, the
`Plan/editor-voxel-artist.md` host code reused) over a **committed per-project stage scene** —
the reference game's effects-stage scene: a dark floor, one point light, a fixed camera, a `subject`
entity carrying a `ParticleEmitter` — and two tabs. The **emitter tab** (this plan) is the generic
inspector over the subject's component: `engine.describe` + `component.set`, sliders from
`x-ui-range`, swatches from the `_color` names. The **pattern tab** (`Plan/projectile-patterns.md`) is
a schema form over a `kind: pattern` file. Time control is **forward-only**: restart (re-stamp
`started` with the session's current frame, read off any reply envelope's `frame`),
`sim.pause`/`sim.step`/`sim.set_speed`, and a loop toggle (restart on a period) — every one an existing
method, and Edit mode's frozen sim time makes a paused emitter a still that `sim.step` advances
deterministically. The Level Designer's palette gains a `particles` archetype (`entity.spawn`) so an
ambient emitter can be placed in a level. One persona, `editor/personas/effects-artist.md`, boots the
view's terminal for both tabs. Persistence follows the Art Director's rule for scenes: the tab never
writes a scene file; `scene.save` captures an authored emitter, and a code-spawned one is the persona's
to carry into `tunables.rs`. The stage never holds unsaved state (the Voxel Artist's contract 6), and a
project without a stage scene meets that plan's recorded scaffolding item, not a new one.
- *Rejected — inspector + palette only, with a Replay button on the `started` row (this plan's first
  cut):* ruled out 2026-09-04 because the editor loop is the reason the feature exists, and an FX
  tuning loop needs a stage that never holds unsaved state, a restart and a speed control; the Replay
  row is subsumed by restart.
- *Rejected — a true scrubber (drag to any time):* `engine.snapshot` at the start + `engine.restore`
  + `sim.step` would do it with existing methods but is heavier than restart-and-watch on a pure
  function of time; the trigger is recorded below.
- *Rejected outright — a `Restart`-style verb on the wire:* it is `component.set` of one field.

**7. The reference game's consumers are four, spawned game-side and tuned in its tunables and a data
doc; the death burst replaces the entity fragments.** (a) **Death**: one
`ParticleEmitter` burst entity per kill (embers + a dark alpha puff, gravity, `floor` at the ground),
reaped by a countdown — one hashed entity where there were 5–8, `spawn_fragments`/`update_fragments`/
fragment marker/the fragment tunables **deleted** (ruled 2026-09-04; "before adding a second way, remove
the first"). (b) **Ambient bodies**: the body carries a continuous additive
emitter whose `rate`/`extent` come from a data doc (a new `ambient:` block, per tier), an early
phase a smaller, slower one. (c) **Ambient effects**: an ambient
emitter per effect body scaled by its register. (d) **The last register**: rising motes
over an existing `GroundMarker` disc. Telegraph markers stay markers. Each
consumer is a `Running`-gated reconcile system (idempotent,
RNG-free, keyed on the body). A `ParticleEmitter` on a code-spawned body is hashed component bytes,
so **the game's run replay, segment replays and stress replay re-record once** at the end of the wave
(the established precedent: bless after the session test proves the route, read every
`L1126`, then `cargo test -p ludion-protocol` for the record-size ceiling).

**8. Second consumer: `games/breakout` bursts a broken brick.** One emitter entity per brick break
in the game's one existing system, the plan's proof that the component is engine-generic — the
`hello-paddle`/breakout convention every engine plan in this set followed. Cost: breakout's
`run.lrp` (39 KB) re-records once; its golden is a tick-0 frame with no break, untouched.

**9. Scale is measured before it is engineered: a stress gallery scene and a budget file, then a
quality knob, then — behind a measured trigger — parallel or GPU evaluation.**
`examples/gallery/particles-stress.yaml` authors ~24,000 live particles across a score of emitters
(alpha and additive mixed, so the sort and the overdraw are both exercised);
`budgets/pinned-rtx3090-particles-stress.toml` gates `[passes] scene` and an `extract_ms` ceiling
(the first budget line on that metric — `extract` is the sim thread's time, so a heavy evaluator
steals tick time and the number must be watched). `QualityParams.particle_density: f32` (FULL 1.0)
scales every emitter's `rate` and `burst` at extract — masked per machine, never content, goldens at
FULL. Triggers, recorded here so the answer comes from a number: `ludion_core::par` over the emitter
list when extract's particle share passes 1 ms at the stress population on the reference box; a
stateless vertex-shader evaluation when even that does not hold the line. The first is built (2026-10-10: `extract_particles` in `crates/ludion-engine/src/extract.rs`); the second is not.

**10. `ludion render --scene` gains `--frames N`** (tick the built-in schedule N frames before
extracting — the "zero ticks for now" the render module's header anticipates) and `--frames a,b,c`
stitches a labelled time strip through the existing sweep sheet stitcher. A burst is invisible at
tick 0; this is the agent's instrument for judging motion without writing a test, the
`render --clip/--strip` precedent for assemblies. No new diagnostic code: clap validates the list.
The gallery suite stays tick-0 by design — committed gallery goldens are steady-state emitters
(decision 4), and a burst mid-flight is proven by the evaluator's unit tests and looked at through
`--frames`.

**11. Docs: `docs/particles.md` (new spoke ≤ 12 KB) and the owed `docs/components.md` split first.**
The split moves the "Ground telegraphs" prose and the per-primitive detail out into the spokes that
own them (the `ui-list.md`-out-of-`screen-ui.md` precedent), so the table gains its 24th row under
budget. `dev-docs/renderer-passes.md`'s scene sentence gains the particle draw; `dev-docs/ludion-
engine.md`'s layout lists `particles.rs`; `docs/engine-guide.md` contents lists the topic; the
prelude and `docs/components.md` row point at it. The `ludion-docs` skill writes every topic.

**12. Rulings register (grilling session 2026-09-04; every answer the user's).**
1. **Fragments replaced.** The fragment code and tunables are deleted in Wave 3; the sitting judges
   the burst against `clip.mp4` (removed from the tree before publication, in Lore history) and git
   history. Keeping both was a second way to say "death".
2. **Effects view, not inspector-only** (decision 6); forward-only time control; a true scrubber
   behind its trigger; one `effects-artist` persona for both tabs; the palette archetype stays.
3. **Taste:** the field names/defaults above, `PARTICLE_MAX_LIVE_PER_EMITTER` 4,096, the laptop
   preset's `particle_density` 0.25 on `low`, the default `blend: alpha`, all four reference-game consumers with
   the death burst first (so the fragment deletion lands in the same wave).
4. **Scope boundary.** FX particles are never read by the sim. Gameplay bullets are the sibling plan's
   hashed `Volley` entities on this plan's kernel — option A of the grilling (closed-form volleys with
   a per-bullet kill bitset, one entity per fired volley). Rejected B, entity per bullet (the
   `games/shmup-stress` shape): roughly ten times the hash bytes and replay size, a per-frame extract
   walk over thousands of entities, a cancel that spawns and despawns thousands in one frame. Rejected
   C, one `ParticleEmitter` with a gameplay companion component: the same substance as A with the
   bullet description on a flat disc primitive, which makes the emitter large and puts sprite fields
   on a primitive that draws discs.
5. **Scale and look.** CPU first, low thousands on screen, decision 9's stateless GPU evaluation of the
   same closed form behind its number, a stateful GPU sim refused. Discs before textures. Emitter-frame
   only (decision 2) with the burst-left-behind trail idiom.
6. **Sequence.** This plan's W1 → W2 (the view) → W3 (consumers) → W4 (scale); `Plan/sprite-engine.md`
   W1 next; then `Plan/projectile-patterns.md`, whose budget gate and sprite W4's measurement are one
   reference-box run.

## Waves

### Wave 1 — the engine: component, evaluator, draw, instrument, docs
- **`crates/ludion-engine`**: `ParticleEmitter` + `EmitShape` + `ParticleBlend` in `components.rs`
  (appended 24th, `register_validated`, `L2107` in `ludion-core` codes with JSON Pointers per field);
  `particles.rs` — the motion kernel (`Motion`/`Segment::Ballistic`, `position_at(t)`, `pub`: the
  surface the sibling plan's `Volley` calls; its tests pin the closed form independently of any
  emitter) and the pure evaluator (`live_window`, `evaluate(emitter, transform, now, density)
  -> impl Iterator<Item = ParticleInstance>`; closed-form motion with `ludion_core::math::exp` for
  drag; `derived_salted` per particle); `extract_particles(world, ids, now)` in `extract.rs` feeding
  `RenderScene.particles`; prelude export; `crates/ludion/examples/particle_burst.rs` (a runnable
  example that stamps `started` on a key press).
- **`crates/ludion-renderer`**: `ParticleInstance` in `scene.rs`, `particle.wgsl` + its generated
  bindings (`shader-drift` gate), `build_particle_draw` (sort, pack, `None` when empty), the draw
  between sprites and text in `record_scene`, a `premultiplied_blend()` helper beside
  `alpha_blend()`.
- **`crates/ludion-cli`**: `render --frames N` and `--frames a,b,c` (decision 10).
- **Gallery**: `examples/gallery/particles.yaml` — three endless emitters on a dark floor with one
  point light: an alpha smoke column, an additive ember fountain under gravity, a mote disc with a
  `floor`; a `--frames` strip committed under `Plan/artifacts/` as the wave's eyes.
- **Docs**: decision 11.
- **Exit criteria, here (laptop)**: evaluator unit tests (window bounds for endless/finite/burst
  emitters; steady-state count `= rate × life` within one; closed-form positions at known `t` for
  `drag = 0` and `drag > 0`; the `floor` cut; the cap; `density` scaling); `L2107` per-field tests
  and `ludion explain L2107`; `extract_is_deterministic` extended to `particles`; **the
  hash-neutrality proof** — a ticked world with emitters extracted every frame vs never extracted:
  identical `WorldHash` trace and byte-identical `.lrp` (the `Animation` proof's twin); `dump-graph`
  on `particles.yaml` shows the unchanged minimal graph (snapshot test); the renderer's GPU test
  renders the gallery scene twice byte-identically and a marker-free/particle-free scene with
  `particles: []` byte-identically to before; the gallery suite's set of reds on this laptop does
  not grow (every red must be a known laptop-provenance golden); `node scripts/check.mjs
  --rust-only` green, run detached.
- **Owed on the reference box / the M4**: first-bless `particles.png` on both pinned GPUs; the
  Windows `check.mjs` lane.

### Wave 2 — the editor: the Effects view, emitter tab, palette, persona
- **Stage content**: the reference game's effects-stage scene (decision 6), reviewed through a
  `ludion render` of it committed under `Plan/artifacts/`.
- **Host** (`editor/app/src-tauri/`): the stage-session spawn generalized from the Voxel Artist's —
  one host function, two callers; no new capability.
- **View** (`editor/app/src/`): the `effects` entry in the view registry; the emitter tab over the
  stage subject — `x-ui-range` on every numeric field of `ParticleEmitter` (engine-side schemars, zero
  editor code) with travel ranges chosen for the reference game's registers, and an inspect-fields vitest over
  the real schemars output pinning that **every** field gets a typed widget and both colours swatch
  (the `ui-widgets` W3 precedent, which is also what catches a future nested field); the time strip
  (restart / pause / step / speed / loop, all through existing commands, `useComponentCommit` for the
  `started` write); the pattern tab declared and disabled until the sibling plan lands it.
- **Palette**: `palette.ts` gains a `particles` archetype (an endless emitter whose defaults render
  with no asset); the order pin updated.
- **Persona**: `editor/personas/effects-artist.md` (pulls `ludion docs particles`; the pattern topic
  line is the sibling's); the live-smoke list in `Plan/editor-live-smoke.md`.
- **Exit, here**: `node scripts/check.mjs --frontend-only` green and the Tauri host lane; the parity
  audit finds zero editor-private endpoints, the wire at 29. **Human**: the live smoke (open the view,
  drag a slider while the sim runs, restart a burst, pause, step, loop, place the archetype in a
  level) at the next editor sitting.

### Wave 3 — the consumers: the reference game and breakout
- Decision 7's four consumers as reconcile systems; the data doc's `ambient:` block (per
  tier, `#[serde(default)]`); the fragment deletion (ruled); tunable blocks per
  effect; breakout's brick burst (decision 8).
- **Tests**: the game's particle suite — a kill stands one burst emitter with `started` at
  the kill frame and it is reaped after its countdown; a drawn body carries the doc's emitter and an
  early-stage one the smaller; an emitter scales with its register; a paused world extracts
  byte-identical particle sets twice; the committed replay re-derives its trace (the replay test
  is the judge of what moved).
- **Re-records**: the game's run, segment and stress replays and breakout `run.lrp` — once, at the
  end, through the replay-scoped bless, then `cargo test -p ludion-protocol`.
- **Exit, here**: every reference-game suite green serially (`-j 1`), map-drift and voxel-drift
  unmoved, `L1126` clean, the E6 budget's `[systems]` table gains the reconcile systems at the floor.
- **Owed on the reference box / the M4**: the district and per-palier goldens re-blessed
  (the ambient plumes are visible in a ticked scene), the E6 GPU run with `extract_ms` read, the
  safe-area goldens re-proved unmoved (no emitter bodies there).
- **Human gate (the plan's exit)**: folds into the queued sittings — does the death read
  better than the cubes did, do the ambient bodies read as alive; the fragment
  deletion is confirmed or the burst is retuned there.

### Wave 4 — scale: the stress scene, the budget, the density knob
- `examples/gallery/particles-stress.yaml`; `budgets/pinned-rtx3090-particles-stress.toml` and the
  M4 twin (measured on each box, never derived); an `#[ignore]` CPU cost test over the committed
  stress scene (the `cluster_build_cost` shape); `QualityParams.particle_density` through the
  presets and the override file, its docs line in `docs/quality.md`; the two evaluation triggers
  recorded in the budget header with the numbers that would fire them.
- **Exit, here**: the cost test runs and prints; `--quality low` renders the stress scene with the
  reduced population (a count assertion on `RenderScene.particles`, no golden).
- **Owed on the reference box / the M4**: the budget files, `particles-stress.png` on both.

## Engine/game boundary

- **`crates/ludion-core`**: `L2107`.
- **`crates/ludion-engine`**: `ParticleEmitter` and its enums, the validator, `particles.rs`, the
  extract step, `QualityParams.particle_density` plumbing, the hash-neutrality test.
- **`crates/ludion-renderer`**: `ParticleInstance`, `particle.wgsl`, the sort/pack/draw,
  `premultiplied_blend()`, the quality param's mask.
- **`crates/ludion-cli`**: `render --frames`.
- **`editor/app`**: the Effects view (stage-session host reuse, emitter tab, time strip), the
  archetype, the vitest, the persona; nothing on the wire.
- **The reference game**: which bodies emit, every number, the reconcile systems, the doc block, the
  fragment deletion. **`games/breakout`**: the brick burst.
- **Generic second consumer**: breakout, plus any game with a death, a pickup, a projectile impact or
  an ambient source — every genre the engine names.

## Deferred / non-goals

- **Textured / atlas-frame particles** — trigger: `Plan/sprite-engine.md` Wave 1 lands its atlas;
  `ParticleEmitter.texture`/`frame` arrive as additive serde-defaulted fields then (only scenes
  carrying emitters re-record).
- **A `kind: effect` preset document** — trigger: one effect authored on three or more entity kinds
  in the reference game, or the Game Designer view wanting a doc form for it.
- **World-space emission from a moving emitter** (decision 2) — trigger: a sitting judging the
  burst-left-behind trail idiom insufficient for the tracer bolt.
- **Shadow-map sampling for lit particles** — trigger: a sitting sees motes glowing behind a wall.
- **A wrap term for lit particles** — trigger: a sitting sees a mote lane dim near the lamp's plane at a level camera (the `|n·l|` zero; `docs/particles.md` Gotchas).
- **Soft-depth intersection** (fading a particle where it crosses geometry) — trigger: hard cuts at
  the ground reading wrong at a sitting; needs a depth copy or a later pass, both real costs.
- **Per-particle rotation / spin, stretched sparks along velocity, sub-emitters, collision** —
  no consumer names them; each is a field or a second draw when one does.
- **Parallel or GPU evaluation** — decision 9's two measured triggers.
- **A true time scrubber in the Effects view** — trigger: restart-and-watch judged too slow at an
  editor sitting; `engine.snapshot` + `engine.restore` + `sim.step` is the shape, no wire movement.
- **Gameplay bullets are not deferred** — they are `Plan/projectile-patterns.md`, on this kernel.
- **A particle profile-report section** — `extract_ms` already carries the cost; a section is a
  schema rev that re-records every replay for a number that exists.

## Risks / open questions

- **Extract runs on the sim thread.** A large live population steals tick time before any GPU
  cost shows; `extract_ms` is the metric, Wave 4's budget line is the tripwire, and the density knob
  is the laptop's lever. Nothing here is measured yet on this checkout.
- **The per-frame sort.** Twenty-four thousand particles sorted per frame is a real CPU term;
  additive-only populations could skip it, but that is an optimization to earn from Wave 4's
  number, not to pre-build.
- **Overdraw.** Large soft alpha particles at 1080p are fill-bound; the stress scene's `scene`
  ceiling is where it shows. HDR embers under bloom are the cheap register; smoke is the expensive
  one.
- **Reference-game hash churn lands in one wave** (decision 7): every consumer moves
  the committed replays; the re-record is scheduled once, at Wave 3's end, and the district golden
  re-bless is owed on the box.
- **`Transform.scale.x` as the effect's scale** conflicts with nothing today (an emitter is its own
  entity in every consumer), but an emitter added to a scaled *model* entity would inherit that
  scale; the docs say so, and the reference game's consumers spawn emitters as their own bodies.
- **The steady-state window at negative indices** hashes `i` as a signed value; the evaluator's
  tests pin that the window at tick 0 equals the window at tick 10,000 in count and in the
  distribution of ages.
- **`docs/components.md`'s split** is a docs-skill task with its own review; it is scheduled inside
  Wave 1 rather than before it so the plan does not stall on a docs pass.
- The laptop cannot bless or measure; every "owed" line above names the box that can, and no
  golden is to be blessed here (a standing private rule).
