# Audit: the particle system and VFX (2026-10-03)

**Scope.** Report-only audit at rev `24016800`. Files covered: `crates/ludion-engine/src/particles.rs`
(the `Motion` kernel, `live_window`, `evaluate`), `ParticleEmitter` / `EmitShape` / `ParticleBlend` and
the `L2107` validator in `crates/ludion-engine/src/components.rs`, `extract_particles` /
`extract_ground_markers` / `RenderPose` in `crates/ludion-engine/src/extract.rs`, the particle draw
in `crates/ludion-renderer/src/render.rs` (`particle_order`, `particle_raws`, `build_particle_draw`,
`build_particle_pipeline`, `premultiplied_blend`) and `shaders/particle.wgsl`, `GameClock` in
`crates/ludion-core/src/time.rs`, the wire envelope's `frame`, the editor's Effects view
(`editor/app/src/views/EffectsView.tsx`, `editor/app/src/editor/effects/{EmitterEditor,EmitterPanel}.tsx`,
`time.ts`), `crates/ludion/examples/particle_burst.rs`, `docs/particles.md`, `docs/game-clock.md`,
`crates/ludion-engine/tests/particles.rs`, and the consumers (a game's death burst and
ambient plumes, breakout's brick burst). Plans: `Plan/particles-engine-and-editor.md`,
`Plan/telegraphs-and-vfx.md`. Static reading only; no tests were run.

## Overall assessment

The particle system is one of the cleanest subsystems in the engine. It is stateless by design: a
particle is a closed-form function of `(emitter, transform, gameplay time, density)`, with RNG
streams named by particle index. Snapshot/restore, replay, pause, thread count, and hash isolation
are therefore correct by construction rather than by bookkeeping, and the extract path never writes
back to the world. The defects are at the edges: the per-emitter cap the code claims to enforce
does not cover `burst`; the "stamp `started` with the gameplay frame" rule is contradicted by the
runnable example and cannot be followed over the wire at all; and colour channels skip the
non-negativity check every other HDR colour in the engine gets. No determinism defect was found.
Particles have no asset references, so they do not share patterns-sprites B2's hot-reload flaw.

## High

None.

## Medium

### K1 — The live-population cap clamps the continuous stream but not `burst`, so a Rust-built emitter can freeze extract · Medium · [verified]

**Evidence.** `live_window`'s doc comment says the range is clamped "because … a component built in
Rust never passes through a validator, and a render loop must not be freezable from game code"
(`crates/ludion-engine/src/particles.rs:366-369`). The clamp at `particles.rs:397-403` applies only to
the continuous `Range<i64>`. The burst count is returned unclamped (`particles.rs:374-378`), and
`evaluate` walks `0..i64::from(burst)` (`particles.rs:428-431`). `thinned` leaves `burst` untouched at
density 1.0 (`particles.rs:593-595`). The `L2107` validator does bound `rate × life[1] + burst`
(`components.rs:3248-3270`), but it only runs on documents and on `component.set`/`entity.spawn`
payloads (`commands/entity.rs:47,169`), never on a `world.insert` from game code. The unit test
`the_live_window_is_clamped_to_the_cap` (`particles.rs:980-989`) covers only `rate`.

**Why it matters.** The guarantee is explicit and does not hold. Each burst particle costs an RNG
derivation plus closed-form evaluation (about 0.1 µs on the reference box, per the plan's Wave 4
log). A `u32` burst therefore scales without bound, and extract runs on the thread that owns the
world.

**Failure scenario.** A game computes `burst: (damage * 40.0) as u32` or `burst: count.wrapping_sub(1)`.
A one-off value in the millions (or `u32::MAX` after an underflow) stalls every extracted frame for
seconds to minutes for `life[1]` seconds. The window, the wire, and the editor all hang with it, and
no diagnostic is produced.

**Fix.** In `live_window`, clamp `burst` to `PARTICLE_MAX_LIVE_PER_EMITTER.saturating_sub(continuous
span)`, so the combined set honours the cap the validator describes. Extend the cap test with a
`burst: u32::MAX` case.

### K2 — `started` must be a gameplay frame, but the runnable example stamps the sim frame and the wire exposes no gameplay frame · Medium · [verified]

**Evidence.**
- The rule: `docs/particles.md:81-86` and `docs/game-clock.md:50-55` require `ctx.game_frame`,
  "never `ctx.frame`".
- The example: `crates/ludion/examples/particle_burst.rs:61` writes `emitter.started = ctx.frame`, and
  `docs/particles.md:86` names this file as "the runnable pattern". Breakout does the same
  (`games/breakout/src/lib.rs:402`), harmlessly today because it never holds the clock.
- The wire: the response envelope carries only the sim `frame` (`crates/ludion-protocol/src/envelope.rs`
  module doc, `:289`). No method returns `GameClock` or a game frame. The editor's Restart and loop
  stamp that envelope frame (`editor/app/src/editor/effects/EmitterEditor.tsx:175-180`, `:205-220`).
- The size of the delay: `GameClock.held` accumulates for the whole session and never resets
  (`crates/ludion-core/src/time.rs:210-217,237-240`). `docs/game-clock.md:87` says a `ctx.frame` stamp
  is late "by however many frames the freeze lasted", which understates it: the stamp is late by every
  frame held since the session began, not only during a freeze that is still in progress.

**Why it matters.** Examples are the agents' primary documentation, and this one teaches the
pattern the docs forbid. Over the protocol (the agent's primary interface) there is no correct way
to fire a burst or a `PatternPlayer` in a game that holds the clock. Today the editor is shielded
only because the Effects stage is a bare `ludion serve` session that never holds.

**Failure scenario.** In vigil, every `Paused`, `LevelUp`, `Shop`, and `Summary` frame is held
(`games/vigil/src/run.rs:1120-1128`). After a player spends 90 s across level-up screens, an agent
attached to the game slot sends `component.set ParticleEmitter {…, started: <envelope frame>}`. The
burst becomes visible 5,400 frames later, and by then a `DeathBurst`-style reaper has despawned the
entity, so the effect never appears. A game written from `particle_burst.rs` loses every burst after
its first pause in the same way.

**Fix.** Change the example (and breakout) to `ctx.game_frame`. Fix `docs/game-clock.md:87` to say
"late by every frame held so far". Add the game frame to the wire, either as a `game_frame` field
beside `frame` in the response envelope (a schema-hash move) or as a read of `GameClock` through an
existing query. Switch the editor's two restart paths to that field.

## Low

### K3 — `start_color` / `end_color` are not validated, so a negative channel writes negative radiance into the HDR target · Low · [verified]

**Evidence.** `ParticleEmitter::validate_semantics` checks extents, life, speed, rate, duration,
drag, sizes, and gravity, but no colour (`components.rs:3186-3273`). The engine's convention
elsewhere is that colours are HDR but "negative light does not exist": `Environment` rejects them as
`L1128` (`components.rs:622-655`), and so does fog (`components.rs:1409`). The shader premultiplies
`in.color.rgb * weight` under `(One, OneMinusSrcAlpha)` (`shaders/particle.wgsl`, `fs_main`), so a
negative channel subtracts from the destination.

**Failure scenario.** A designer types `end_color: [-0.2, 0.1, 0.0, 0.0]` intending "darker". The scene
validates clean, and the plume writes negative values into the `Rgba16Float` scene target, which
then feed bloom and tonemapping. The result is a dark fringe, or worse, depending on how the post
chain handles negative input, and there is no diagnostic.

**Fix.** Add the `Environment` per-element check (finite, ≥ 0 for RGB; alpha in `[0, 1]`, or
≥ 0 since the shader clamps it) under `L2107` with `/start_color/<i>` pointers.

### K4 — The particle depth sort recomputes the view-space depth inside the comparator · Low · [verified]

**Evidence.** `particle_order` calls `view_depth(view, …)` (a `Mat4 × Vec4`) twice per comparison
(`crates/ludion-renderer/src/render.rs:2121-2129`). That is about 2·n·log₂n matrix products: roughly
640k on the 21,829-particle stress scene, every frame. `extract_particles` also grows its `Vec`
without a `reserve` (`extract.rs:1451-1465`), and `particle_raws` builds a second index `Vec` before
packing.

**Why it matters.** Decision 9's `par` trigger has already fired on extract cost. The renderer-side
sort is the next serial cost on the same stress path, and it is cheap to fix without touching
determinism.

**Fix.** Compute the depths once into a `Vec<(f32, u32)>`, then sort with
`total_cmp(...).then(index)`. The order is identical and NaN behaviour is unchanged.

### K5 — Purely cosmetic emitters live on hashed sim entities, so every look tweak moves `WorldHash` · Low · [verified]

**Evidence.** A game's death burst and ambient plumes are spawned with `world.spawn()`, and
breakout's brick burst is spawned the same way. Every registered component is hashed
(`crates/ludion-ecs/src/registry.rs:120,310`). `games/vigil/CLAUDE.md:61` records the consequence:
"a look change re-records `stress.lrp` too". The world already has a presentation id range for
exactly this kind of entity (`World::spawn_presentation`, `crates/ludion-ecs/src/world.rs:210-225`),
and `extract_particles` already walks it (`extract.rs:617-619`).

**Why it matters.** This is the North Star cost. Retuning a colour or `DEATH_BURST_COUNT` (pure
presentation) invalidates the replay corpus and forces a re-record cycle. Cosmetic entities also
consume sim entity slots, which couples the free-list order to visual content. No gameplay stream
keyed on a later entity's index was found to move today, but the coupling is latent.

**Fix.** It is documented and accepted today, so this is a design call. Either move FX emitters to
presentation entities (a presentation system that reads the kill and spawner state), or record the
decision in the plan as deliberate so that the next consumer does not copy it unknowingly.

## Checked and healthy

- **Determinism.** `evaluate` reads only `(emitter, transform, now, density, basis)`. Per-particle
  streams are `derived_salted(seed, index, salt)`, so draw order is independent of iteration, culling,
  frame rate, and thread count. Transcendentals go through `ludion_core::math` (`exp`, `sin`, `cos`,
  `powf`, `sqrt`). Time is derived from exact tick counts in `f64` (`seconds_since`), never
  accumulated, so rate-based emission has no fractional-particle drift. Negative indices wrap safely
  through `FrameIndex::new(i as u64)`.
- **The sim/presentation boundary.** Extract is a pure read: nothing in the particle path writes
  back. `tests/particles.rs::extracting_particles_changes_neither_the_hashes_nor_the_recording` pins
  this. Particle *positions* never reach gameplay; only the `Motion` kernel is shared, by design, with
  pattern hit tests.
- **Snapshot/restore, replay, hot reload.** There is no per-emitter accumulator, RNG cursor, or
  live-particle list to omit, and the component carries the whole state. Particles have no
  atlas/texture references, so patterns-sprites B2's reload flaw has no analogue. A game's doc
  changes reach emitters through an overwrite-in-place system.
- **Gameplay clock.** `extract_inner` folds `GameClock` once (`extract.rs:614-616`), so held games
  freeze their plumes; the reference game stamps its game frame.
- **Burst and window edges.** A burst at `elapsed == 0` is live exactly once (no double fire). Finite
  emitters admit indices `0..=floor(duration·rate)`. Boundary rounding only ever drops a particle
  whose age is within an ulp of 0 or of its life. Zero, negative, and NaN lives and ages are rejected
  per particle (`particles.rs:501-508`). The endless steady state is pinned by tests.
- **Parenting.** Emitters use `render_pose` world TRS, so a parented emitter follows the hierarchy;
  scale is `scale.x` by convention, as documented.
- **Sort and blend.** `particle_order` uses `total_cmp` with an index tie-break (NaN-safe, total). One
  premultiplied pipeline correctly serves alpha and additive. The pass is depth-tested against opaque
  geometry with no depth write, and a particle-free scene issues no draw.
- **Validation UX.** `L2107` messages name the field, the value, and the fix, with JSON Pointers.
  Non-finite YAML (`.inf`/`.nan`) is rejected at parse, and wire payloads are semantically validated.
- **Editor parity.** The Effects view uses only `component.get`/`component.set`, `sim.pause`/
  `resume`/`step`/`set_speed`, `query.entities`, `engine.describe`, and `asset.reload`. No private
  endpoint was found.
- **Quality knob.** `particle_density` scales `rate`/`burst` before the window and is the identity
  at 1.0, so full-quality extracts are byte-identical.
- **Ground markers** (`Plan/telegraphs-and-vfx.md`). Negative scale is clamped to zero coverage,
  `L2106` validates dimensions, and the draw order is as documented.

## Suggested order of work

1. **K1.** A one-line clamp plus a test that restores a stated safety guarantee.
2. **K2, doc and example half.** Switch `particle_burst.rs` and breakout to `ctx.game_frame` and
   correct `docs/game-clock.md:87` (no schema movement).
3. **K3.** Add colour validation under `L2107`.
4. **K2, wire half.** Expose the game frame on the protocol (schema-hash move; sequence it with the
   next wire change), then switch the editor's two restart paths to it.
5. **K4.** Precompute the sort keys. Fold this into the decision 9 `par` wave when it is scheduled.
6. **K5.** Make the design ruling: presentation entities for FX, or record the hashed choice in the
   plan.
