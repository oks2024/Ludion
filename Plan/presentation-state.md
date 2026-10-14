# Plan: Presentation state — entities the sim never sees and the hash never covers

> Status: **built 2026-10-01 (subtasks 1–3).** Current-code facts were read from the tree on
> 2026-09-30 (file:line pointers are of that date; re-grep before editing). Compatible with
> `Plan/transform-hierarchy-and-prefabs.md` (open for grilling); see §Hierarchy and prefabs.

## Trigger

external-game request 0021. The external game's juice is presentation: score popups, a follow camera, and soon particle
emitters. None of it affects the sim (physics plus `Score`), but the engine has nowhere unhashed to put
it. So every popup colour or camera margin moves every `WorldHash` trace and forces every committed
`.lrp` to be re-blessed. The game also pools pre-baked `popup/<i>` entities and never spawns or
despawns them, only to keep the ids of its grab carriers stable.

## What exists (2026-09-30)

- `WorldHash::compute` (`crates/ludion-sim/src/hash.rs:106`) walks `World::entities()` (every live
  slot, ascending index), and for each entity every registered component it holds. Resources and the
  free list are excluded.
- `EntityAllocator` (`crates/ludion-ecs/src/entity.rs`) is one LIFO free list. Every spawn, whatever
  it is for, shifts the ids of later spawns. Live slot indices are capped at `MAX_ENTITY_SLOTS` = 2^24.
- Storages are dense columns with keys in ascending entity index (`storage.rs`). Joins gallop over
  the key slices (`query.rs`, `par_query.rs`).
- Systems are plain `fn(&mut World, &SystemCtx)`, run in order by `Schedule::run_with`
  (`schedule.rs:383`). Physics is installed after every game system (`Engine::build_schedule`,
  `engine.rs:689`).
- Particles are already presentation in effect: `particles::evaluate` is a closed form of
  (emitter, transform, time) run at extract. Only the `ParticleEmitter` component itself is hashed.

## Decisions

### 1. The flag lives on the entity, not on the component registration

A **presentation entity** is spawned with `World::spawn_presentation()`, or authored with
`presentation: true` in a scene. Every component it holds is presentation state: drawn and
inspectable, never hashed, never visible to a sim system. A presentation entity may hold any
registered component (`Transform`, `Sprite`, `Text`, `Camera`, `ParticleEmitter`, game types), so
there are no "presentation component types".

Why not a component flag:
- Request 0021 has two problems, and only an entity-level answer solves both. Hash exclusion alone
  could be done by flagging a component type. Id stability cannot: the popup *entity* still takes a
  slot from the sim free list.
- A component flag would make the same type hashed on one entity and invisible on another only
  through its registration. And a sim system that reads a flagged component on a sim entity would
  slip it into the hash, with nothing to stop it.
- Presentation data that decorates a sim entity (a popup over a ball, a trail behind it) goes on a
  presentation entity that names the sim entity. The reference runs from presentation to sim, which
  is the direction the guard (decision 5) allows.

Doing both would be two ways to do one thing (Pillar 6). One mechanism, the id range, gives hash
exclusion, id stability and the guard together.

### 2. A separate id range and allocator

Presentation ids come from a **second `EntityAllocator`**, with slot indices offset by
`PRESENTATION_BASE = MAX_ENTITY_SLOTS` (2^24). So `index < 2^24` is a sim entity, and
`2^24 ≤ index < 2^25` is a presentation entity. It is still one `EntityId` type with the same
generational safety and LIFO reuse.

- **Sim ids never move.** Spawning or despawning a presentation entity never touches the sim free
  list, so the external game can spawn and despawn popups at runtime instead of pooling them.
- **Every storage is partitioned for free.** Keys ascend, so all presentation rows form one
  contiguous tail of every column. A sim-phase join stops at one `partition_point`, and the hash's
  per-component cursors never reach the tail.
- `World::entities()` keeps its meaning: the canonical sim enumeration that the hash, snapshots,
  bisection and residue walk. `World::presentation_entities()` lists the others. A whole-world
  consumer that should also see presentation entities (extract, `render.bounds`, the inspector
  listing) asks for both explicitly. A missing popup is visible on screen; a popup leaking into a
  hashed output is silent. So the default is the safe one.

### 3. Snapshot, restore, save and replay keyframes

- **Snapshot.** Two additive optional fields, `presentation` (entity docs, same shape as `entities`)
  and `presentation_allocator`, both `skip_serializing_if` empty. A snapshot of a world without
  presentation entities is byte-identical to today, and `SNAPSHOT_FORMAT` stays 1.
- **Restore** rebuilds them, so a `ludion dev` restart keeps the camera where it was. Restore order:
  `restore_entities` clears the world (both allocators), then `restore_presentation` rebuilds the
  presentation allocator and its rows.
- **Keyframes** are snapshots, so they carry the fields too, and a seek restores the picture.
  `verify_replay_keyframes` and `bisect_divergence` compare **sim state only**: a presentation
  difference is never drift.
- **Replay.** Presentation systems run during a replay like any system, so the picture replays. They
  are not verified, so changing presentation code never invalidates a recording.
- **`scene.save`** writes presentation entities after the sim entities, each with
  `presentation: true`. A presentation entity spawned at runtime is saved like a sim entity spawned
  at runtime, under the same rule.
- **Scene-switch residue** captures sim entities only: it reapplies sim state into the next scene.
  Presentation entities do not survive a switch.

### 4. Scene writer, schema, editor

- **Scene format.** An optional entity-level key, `presentation: true`, emitted right after `name:`
  and omitted when false, so every committed scene's bytes stay put. Sim entities spawn in document
  order from the sim allocator and presentation entities from theirs. Adding a presentation entity
  anywhere in a scene therefore never shifts a sim id.
- **Entity references.** A sim entity's `x-entity-ref` field may not name a presentation entity: a
  new scene error, because the sim would hash a presentation id. The other direction is fine.
- **Schema.** Component schemas do not change, since presentation is not a property of a type.
- **Editor.** No change in this plan. The entity listing includes presentation entities, so the
  outliner shows them and the inspector edits them over `component.set`, like any entity. A
  presentation badge needs a listing field that moves the schema hash, so it is a follow-up for when
  someone asks for it.

### 5. The guard: phases, enforced by the world

Every system runs in a **phase**, which the `Schedule` sets on the world around each system body.
`EngineBuilder::system` registers `Sim`; the new `EngineBuilder::presentation_system` registers
`Presentation`. Host code (command application, extract, hashing, loading) runs in `Host`, with no
restrictions.

| | sim entity | presentation entity | resources |
|---|---|---|---|
| **Sim phase** | full access | invisible: queries stop before the tail; `has` is false; `get`/`get_mut`/`insert`/`remove`/`despawn` return `L0028`; `presentation_entities()` is empty; `spawn_presentation` panics with `L0028` | full access |
| **Presentation phase** | read-only: `get` and read-only queries see it; `get_mut`/`insert`/`remove`/`despawn` return `L0028`; a query with any `&mut` column joins over presentation rows only; `spawn` panics with `L0028` | full access | read; `resource_mut`/`insert_resource`/`remove_resource` panic with `L0028` unless the type is an allowlisted outbox (`World::allow_presentation_writes::<T>()`; the engine allowlists `Diagnostics` and `AudioQueue`) |

Why this guard:
- **It is structural, not a lint.** The world refuses the crossing, at the call, every run, in every
  build. A sim system cannot read presentation state, and a presentation system cannot write sim
  state or draw from the master `DeterministicRng`. So presentation code cannot move the hash, and
  sim code cannot depend on anything unhashed.
- **It costs almost nothing.** One integer compare on `get`, the sim's hottest path, and one
  `partition_point` per column per query.
- **Panics land in the existing boundary.** A `bool`, `Option` or bare `EntityId` cannot carry a
  diagnostic, so those calls panic with the `L0028` text. That follows `has`'s `L0026` precedent: the
  schedule's unwind boundary reports the panic as `L4007`, naming the system and the crossing.
- **No type-level guard.** One would need typed system parameters (a `SimWorld` view distinct from a
  `PresentationWorld`), which means rewriting every system signature in two games and the engine.
  The freeze list and Pillar 6 rule that out for a guard the world can enforce exactly.
- **Order does not affect soundness.** Neither phase can observe what the other writes, so even
  interleaved systems would be sound. The engine still runs presentation systems last, after the
  physics triple, so they read final poses.

Randomness in a presentation system comes from `DeterministicRng::derived(...)`, built from the
read-only `WorldSeed` and the frame. Those are the sanctioned constructors, and neither advances the
sim's stream.

What a game must know: a camera that the sim unprojects through (the external game's grab, vigil's aim) stays a
sim entity. Making it presentation turns every sim read of it into `L0028`. The error says so and
nothing desyncs. Engine sim systems (physics, path following) do not see presentation entities, so a
`Collider` on one does nothing.

### 6. Wire, schema hash, committed artifacts

- **Wire.** No method, param or result changes. `query.entities`, `component.get` and
  `component.set` already take any `EntityId`. The schema hash `b57faa179f02f6b6` does not move.
- **Hash rule.** `ludion.worldhash.v1` gains one sentence in rule 3: presentation entities (index ≥
  `PRESENTATION_BASE`) are not walked. The domain tag does not bump. Every world the old rule could
  hash contains no presentation entity, so it hashes to the same bytes. Bumping the tag would
  re-bless every committed trace for nothing.
- **Committed traces and replays.** None moves, because no committed scene or game uses the feature.
  Each subtask's verify proves it: the default `check.mjs` suite replays the corpus.

## Hierarchy and prefabs

These rules keep `Plan/transform-hierarchy-and-prefabs.md` compatible:
- A presentation entity may have a sim `Parent`, such as a popup riding a ball. A sim entity may not
  have a presentation `Parent`: its world transform would read invisible state. That plan's verbs and
  scene validation refuse it with the decision-4 error.
- Cascade despawn (that plan's E1) of a sim entity also despawns its presentation children. That is
  the one sim-phase write to a presentation entity, and it is sound because the presentation
  allocator never reaches the hash.
- A prefab entity may carry `presentation: true`. `instantiate_into` spawns it from the presentation
  allocator.

## Subtasks

1. **ECS core:** id range, phases and guards in `ludion-ecs`, the `L0028` code, and a hash test
   in `ludion-sim` (a private request plan).
2. **Engine wiring:** `EngineBuilder::presentation_system`, the outbox allowlist, extract and bounds
   over both ranges, snapshot and restore fields, keyframe verify and bisection on sim only, and a
   external-game-shaped integration test.
3. **Scene and docs:** the `presentation: true` key (parse, emit, validate, load, save), the
   sim→presentation reference error, the inspector listing, the `docs/` game topic and its mirror,
   and this Status.

No re-record step: nothing committed moves.

## Non-goals

- A per-component presentation flag (decision 1).
- An editor presentation badge (decision 4; it moves the schema hash).
- Presentation resources for games: mutable presentation state lives on presentation entities.
- Engine sim systems (physics, path following, animation playback) driving presentation entities.

## Status

- 2026-09-30: design settled by the planner for external-game request 0021. Nothing built.
- 2026-10-01: guard cost measured on the RTX 3090 (LUDION_THREADS=8). Microbenchmark, best-of-15
  over 4096 entities: `has` +1%, `get`/`get_mut` -8%, query rows at parity (the hit path for a live
  sim entity is one allocator lookup, plus one phase compare for a write; presentation and dead ids
  take cold paths). Vigil budget-horde, 8 interleaved pairs on a quiet box: every low-noise system
  is within 3% except a spawn placement system +7.4% (about 1 µs, attributed to codegen layout; no ceiling
  crossed); `frame_total` -1.3%. Every engine rtx3090 budget passes. Vigil's `fire` and
  `spatial_rebuild` were already over their E6 ceilings on the baseline: `Plan/owed.md`.
- 2026-10-01: scene and docs shipped. Entities take `presentation: true` (parse, emit after `name:`, load via `spawn_presentation`; `ludion_scene::instantiated_ids` maps document position to id); a sim→presentation reference is `L1036` (scene) / `L1018` (save); `scene.save` writes presentation entities last; `query.entities` lists them; `ludion docs presentation`.
