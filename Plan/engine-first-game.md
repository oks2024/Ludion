# Plan: Engine Evolutions for the First Game

> Status: **settled direction, per-wave detail** (grilling session 2026-07-12). This is the ENGINE side of the reference game's first
> prototype: every engine
> evolution the reference game (designed in a private plan, phases G0–G7) needs, as waves E1–E7 plus
> the continuous perf gate E6. **A private roadmap binds the two sides** — it
> says when each wave starts, what "ready" means, and what the game does meanwhile.
> Wave IDs are stable; when a wave is scheduled it gets a numbered wave-level plan
> (`Plan/phase-N-*.md`, numbering continues after Phase 11 PBR) with current-code facts gathered
> then — the sections below record the settled scope, the proposed design direction, non-goals,
> and the exit-gate shape, in the mold of `Plan/roadmap-voxel-mesh-pbr.md`'s phase sketches.
> "Settled" = decided in the grilling; "Proposed" = this document's design sketch, to be
> confirmed or amended by the wave plan.

## Why (the one-paragraph case)

The capability scan (2026-07-12) against the game's needs found five real gaps: input is
keyboard-only (twin-stick is inexpressible); collision/spatial queries don't exist (two games
have hand-rolled them; the third needs 1000 entities); UI has no screen-space layer (a HUD or
dialogue box is unbuildable); game content has no validated-hot-reloadable data path; and
nothing persists across runs but snapshots. Each gap gets an engine-level answer with the game
as its consuming proof — the engine-driver contract. Two more items ride along: a committed
1000-entity perf budget (E6, continuous) and in-game scene transitions (E7, needed when the
the world grows past one scene).

## Cross-cutting rules (bind every wave)

- **Determinism tier 1 everywhere sim-side**: no wall clock, seeded RNG only, pinned iteration
  orders, `ludion_core::collections`, per-crate clippy denylists on any new crate.
- **Additive protocol only**: no wave below is expected to change the wire surface (currently
  26 methods); if a wave plan discovers otherwise, that's a flagged decision, not a drive-by.
- **Registry-borne components come for free**: any new builtin component (E1's `Collider`,
  E3's UI components) automatically gets scene-file validation, inspector editing, and schema
  coverage — waves must include the inspector/goldens checks proving they degrade nowhere.
- **Docs or it didn't happen**: each wave lands with its agent-docs topic updates and an
  engine-guide section, gated by the wave's exit criteria (the guide is how THE-test agents
  consume the feature).
- **Every wave ships a gallery example** (`examples/gallery/`) with a blessed golden where the
  feature is visible, per the Phase 8/10 convention.

## E1 — Spatial module (consumed by G1)

**Demand.** 1000 active entities need neighbor queries (separation, pickups, nearest-target
queries), projectile hit tests, and kinematic movement against static geometry. Breakout and
Sokoban hand-rolled their collision; this is the third occurrence — promotion time.

**Settled scope.** Deterministic 2D-on-the-ground-plane (x,z) collision: circle + AABB shapes
as a component; a spatial hash rebuilt per frame; queries — overlaps, ground-plane raycast,
k-nearest; kinematic move-and-slide against statics. **No rigid-body dynamics, no forces, no
constraint solving** — games write their own response logic. Sized to the E6 budget
(~1000 dynamic + hundreds of static at 60 fps).

**Proposed design.**
- New sim-side crate `ludion-spatial` (deterministic denylist applied), the component
  registered in `builtin_registry`: `Collider { shape: Circle{radius} | Aabb{half_extents},
  body: Static | Dynamic }`. Height is ignored — the collider lives on the ground plane;
  the 3D transform is render-only, per the 2D-sim pillar.
- No hidden systems: the engine provides `spatial::rebuild(world)` and games register it at an
  explicit position in their schedule (regularity over magic; system order stays visible).
- Queries return entities in **pinned order** (spatial-hash cell order then EntityId) —
  determinism by construction, conformance-tested like the voxel mesher's pinned sweep.
- `move_and_slide(world, entity, delta) -> resolved delta` against `Static` colliders only;
  dynamic-vs-dynamic stays queries + game logic (VS-style separation is a game system).

**Non-goals (triggers recorded).** Rotated boxes, polygon shapes, 3D volumes, broadphase
persistence/incremental updates — trigger: a game that measurably needs them. Physics dynamics
— trigger: none foreseen; a future game would make its own case.

**Exit-gate shape.** Unit + property tests (query correctness, pinned-order double-run
identity); a microbench proving the E6 numbers; breakout/sokoban untouched (they keep their
hand-rolled logic — frozen proofs don't retrofit); gallery scene; docs topic + guide section;
G1 integrates and deletes its interim collision.

## E2 — Input wave: mouse + gamepad (consumed by G2)

**Demand.** The genre experiment (twin-stick vs auto) is inexpressible today: `InputState` is
keyboard-only (`ludion-sim/src/input.rs`). Settled: both devices in ONE wave — same plumbing.

**Settled scope.** Mouse (position, buttons, wheel) and gamepad (sticks, triggers, buttons) as
deterministic frame-stamped `InputState` data, recorded in replays exactly like keys; plus a
screen→ground-plane unproject helper for aim.

**Proposed design.**
- `InputState` grows `mouse: MouseState` (position in render-target pixel coords, button
  held/pressed/released edges, wheel delta) and `gamepad: GamepadState` (stick/trigger axes as
  `f32`, button edges). Determinism by construction: the sim only ever sees what the input
  script/replay recorded — axes are recorded post-capture, so replay is bit-exact regardless of
  OS jitter.
- `InputEvent` gains the matching variants; the LRP1 input script encodes them (format is
  versioned; old replays keep decoding).
- Capture lives out-of-sim in `ludion-window` (gamepad via a `gilrs`-class backend — the sim
  never sees an OS API). The editor viewer forwarding mouse/pad is **deferred** — trigger:
  designers playtesting through the editor.
- Unproject: pure helpers in the facade's math surface — `camera_ray(camera_transform,
  projection, screen_pos, viewport)` + `ray_ground_plane(ray) -> Option<[f32;2]>` — sim-usable
  (deterministic transcendentals via `ludion_core::math`).

**Non-goals.** Input remapping/bindings config (games map keys themselves until a second game
wants remapping); touch; multiple gamepads (player 1 only — trigger: a couch game).

**Exit-gate shape.** A replay containing mouse+pad input reproduces its WorldHash trace; window
host forwards all devices live; unproject unit tests against known camera poses; guide section
("Reading input") extended; a gallery-adjacent smoke (cursor-aimed turret example).

## E3 — Screen-space UI primitives (consumed by G3)

**Demand.** HUD, run summary, pause menu, dialogue panels. Today's only text is
world-space (`Text` component) — perspective-distorted and depth-clipped under the tilted
camera; a screen-anchored HUD is unbuildable.

**Settled scope.** Screen-space primitives as components — anchored rects/sprites/text with
resolution-independent coordinates and explicit draw order — rendered by a new overlay pass.
Composition by hand in game systems. **No widget toolkit, no layout engine, no focus
management** — deferred with the trigger "real friction composing by hand" (expected candidate:
a later game screen).

**Proposed design.**
- Components: `UiRect { anchor, offset, size, color, order }`, `UiSprite { … + texture }`,
  `UiText { … + text, size }`. `anchor` is a 9-point enum (corners/edges/center); offsets and
  sizes are in reference-canvas units (1080-height virtual canvas, scaled uniformly to the
  target) so one authored HUD fits every resolution.
- Entities carrying Ui components render only in a new frame-graph **overlay pass** after post
  (LDR, unaffected by tonemap/bloom), sorted by `order` then EntityId (pinned).
- Reuses the existing bitmap glyph atlas for `UiText`; world-space `Text` remains for in-world
  labels (different purpose, both stay — not a "second way").

**Non-goals.** Layout/flexbox, focus/navigation, 9-slice, animation, styling systems, input
routing (games hit-test via `InputState.mouse` + the same anchor math, exposed as a helper).

**Exit-gate shape.** UI gallery scene (HUD mock: bars, text, corners) with blessed golden at
two resolutions proving anchor scaling; pinned draw-order test; inspector edits a `UiRect`
without degrading; guide section with a copyable HUD example; G3 builds the real HUD and
deletes G0's world-space countdown.

## E4 — Game data-doc facility (consumed by G4, extended in G6)

**Demand.** Game content definitions (G4, G6) as declarative text with the engine
guarantees content already enjoys: validation with pointer errors, canonical emit, hot reload.
The `kind:`-dispatched doc pattern (`mesh`, `voxel` in `ludion-scene`) is the template — this
wave opens it to game-defined kinds.

**Settled scope.** Games register serde doc kinds; the engine provides strict-YAML parsing,
L-coded pointer-carrying validation, canonical emission, hot reload through the asset cache,
and check coverage. Behavior stays in Rust: docs name Rust-registered archetypes plus numbers.

**Proposed design.**
- `Engine::builder().doc_kind::<WeaponDoc>("weapon")` — `T: Serialize + DeserializeOwned +
  JsonSchema`. Validation = strict-YAML structural pass + schema check, errors as new L-codes
  in the free L111x/L112x space with JSON Pointers and nearest-match suggestions (generic
  "game doc" codes carrying the kind name — codes stay engine-owned).
- Loaded docs live in a `DocStore<T>` resource (BTreeMap by root-relative path + a generation
  counter); hot reload rides the existing watcher/`asset.reload` path and bumps the
  generation, so game systems re-read changed docs at a frame boundary — sim-deterministic
  because the reload lands as a frame-stamped event, replay-recorded like input.
- **CLI check without the engine knowing game types**: the facade's arg surface
  (`ludion::run`) gains `--check-docs` — the game binary validates every registered kind under
  its content root and exits with diagnostics. CI-able; `ludion check-scene` continues to own
  only engine kinds. (Design point to confirm at wave-plan time; the alternative — games
  emitting JSON Schema for external checking — is more machinery for the same gate.)
- Canonical emit ships with the wave (tools and agents write docs back without diff noise).

**Non-goals.** Doc cross-references/ID resolution (games resolve names themselves), doc
inheritance/templating, editor UI for docs — trigger: content volume that makes hand-editing
hurt.

**Exit-gate shape.** Round-trip byte-idempotence proptest for a registered kind; every
misshape class → L-coded pointer error test; the hot-reload wire test (edit doc → generation
bump → next frame differs → WorldHash trace unchanged until the event frame); `--check-docs`
smoke in CI; guide section; G4's THE-test content proof consumes exactly this.

## E5 — Text profile API (consumed by G5)

**Demand.** Meta-progression needs data that outlives runs and process restarts. Nothing
persists today but snapshots (whole-world, wrong tool). Settled: profiles are engine-owned
canonical text, and a run is reproducible from (seed, profile-at-start, input replay).

**Proposed design.**
- `Engine::builder().profile::<P>()` — `P: Serialize + DeserializeOwned + Default +
  JsonSchema`. Load-or-default at boot; in-sim access via a `Profile<P>` resource.
- **Saving follows the AudioQueue pattern**: the sim never touches I/O — it flags a save
  request; the host drains it at the frame boundary and writes atomically (temp + rename),
  canonical YAML (stable ordering, ryu floats). Save timing is thereby frame-stamped and
  replay-visible.
- Versioned like scenes (`format: 1`); loading an older/newer shape produces a migration
  report (the `Restored{report}` precedent): unknown fields reported, missing fields
  defaulted, never a silent drop or a crash. New L-codes for parse/validate.
- **Replay embedding**: LRP1 gains the profile-at-start inline (the scene-text precedent), so
  a committed `.lrp` reproduces a run bit-exactly on a machine with a different profile.
- Default location: under the project root (`save/profile.yaml`), path overridable — dev runs
  and tests point at scratch profiles.

**Non-goals.** Multiple profiles/slots (trigger: a second person plays), cloud/steam sync,
encryption (text-is-truth applies to saves; anti-cheat is a non-goal of the whole engine).

**Exit-gate shape.** Round-trip byte-idempotence proptest; migration-report tests (field
added/removed/retyped); atomic-write crash test (kill between temp and rename ⇒ old profile
intact); the reproducibility test (same seed + embedded profile + inputs ⇒ same WorldHash
trace, twice); guide section; G5 consumes it for unlocks.

## E6 — Perf budget gate (continuous from M1; owned jointly with the game)

**Demand.** Settled target: ~1000 active entities (agents, projectiles and pickups) moving in a lit scene at 60 fps on the pinned GPU — the ceiling of the genre
space being explored. Not a wave but a **standing gate**: measured at every milestone close
from M1 on.

**Proposed mechanics.**
- The budget scene is game content (a scene in the reference game, built in G1) and
  the thresholds live in `ludion profile` TOML budgets: total frame ≤ 16.6 ms, with sim and
  GPU sub-budgets split at wave-plan time from first measurements.
- When the gate fails, the fix is pulled as engine work **on demand** — candidate list, in
  expected order: spatial rebuild cost (E1), sprite/billboard batcher throughput, per-entity
  sim iteration overhead, mesh instancing path. No preemptive optimization: the profile
  numbers pick the target.
- **If a measurement shows the sim tick itself above ~half the 16.6 ms budget
  single-threaded, that is the recorded trigger of `Plan/deterministic-job-system.md`
  firing** — pull J0/J1 from that plan (its own oracle and phasing apply); do not improvise
  parallelism here. Two of its "do early" items bind this game's code from G1 on: keep game
  systems on typed queries (not arbitrary `&mut World` poking), and route all randomness
  through the one `DeterministicRng` resource so a later switch to derived per-entity streams
  is mechanical.

**Exit-gate shape (per milestone).** `ludion profile` on the budget scene, on the pinned
machine, within budgets — recorded in the roadmap's status board with the measured numbers.

## E7 — In-game scene transitions (consumed by G6; starts when M5 is in flight)

**Demand.** Levels-as-scenes needs the game to switch scenes mid-run, carrying the player
and run state, without breaking determinism or replays. Today a scene load builds a fresh
world at boot; there is no game-initiated switch.

**Proposed design.**
- The sim requests, the host performs (the queue pattern again): a `SceneSwitchRequest
  { scene_path, spawn_anchor }` resource the game sets; the engine, at the frame boundary,
  loads and validates the target scene, rebuilds the world, then reapplies the **carry set** —
  the player entity's components (serialized/respawned at the named anchor entity) plus
  game-registered carried resources (timers, health, inventory).
- The switch is a frame-stamped event in the replay; a replay crossing a transition
  reproduces exactly (scene text for every visited scene inlines into LRP1, extending the
  existing single-scene embedding).
- Anchors are plain named entities in the target scene (`Name` component convention —
  e.g. `gate-from-<scene>`), no new component needed unless the wave plan finds otherwise.

**Non-goals.** Async/streamed loading, preloading adjacent scenes, cross-scene queries —
all belong to the **seamless-streaming long-term goal** (recorded in a private notebook;
its entry point is the Phase 10 chunk-door seam plus this wave's carry-set semantics).
Trigger: the prototype proving the game, per the settled decision.

**Exit-gate shape.** Transition unit tests (carry set fidelity — byte-equal components after
respawn); the cross-transition replay determinism test; load-failure of the target scene is a
diagnosed error, not a crash mid-run; guide section; G6 ships two connected scenes on it.

## The render arc is not an E-wave

"State-of-the-art lighting" work is already owned by `Plan/roadmap-voxel-mesh-pbr.md`
(Phase 11 PBR: one shading model for the whole mesh path, emissive palettes riding HDR/bloom).
**This game is the content demand that roadmap was waiting for** — G7 consumes Phase 11 rather
than duplicating it here. Occlusion handling (dither/cutaway under the tilted camera) is a
new renderer item whose experiment lives in G7; if it needs engine support beyond game-side
tricks, it gets its own wave plan then. Per the settled decision, the render arc is agent-run
like every other wave, with the user directing closely and coding personally when time allows.

## Risks

- **API-before-consumer.** Every wave lands at most one milestone before its consuming game
  phase (roadmap rule); a wave whose consumer slips waits rather than growing speculative
  surface.
- **E1 pinned-order performance.** Determinism-pinned query order can fight cache-friendly
  layouts; the wave plan must benchmark the pinned design against the E6 numbers before
  committing the API, not after.
- **E2 float axes in replays.** Gamepad axes are analog floats; determinism holds because
  replays record post-capture values — but the wave must test replay round-tripping of exact
  f32 bit patterns (no lossy text encoding in LRP1's input script).
- **E3 scope gravity toward a UI framework.** The primitives ship without layout/focus by
  decision; the trigger for widgets is recorded friction, not ambition. A later menu-heavy screen is the
  expected stress — let it hurt first.
- **E4 CLI-check design** (`--check-docs` via the facade) is proposed, not settled — confirm
  at wave-plan time against the alternative (schema export) with current-code facts.
- **E7 carry-set semantics** are the seed of streaming's entity-migration model; design them
  as if chunks will one day carry entities the same way (same serialize-respawn seam), without
  building any of that now.
