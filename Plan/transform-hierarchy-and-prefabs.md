# Plan: Transform hierarchy and prefabs — propagated parent transforms, runtime templates

> Status: **SETTLED 2026-09-30 by grill-me; nothing built.** Every decision below is ruled — see
> §Rulings, which overrides any "Recommendation" wording it contradicts. Source:
> `Plan/engine-gap-map.md` "Recommended order" item 3, first bullet. Current-code facts were read from
> the tree on 2026-09-30; re-grep before editing.

## Trigger

Four consumers wait on the same two primitives:

- **The platformer** (`Plan/character-controller.md`): held props, things bolted to moving platforms,
  imported multi-node models. (Rider *motion* is already settled without propagation — decision 6
  there — and this plan does not reopen it.)
- **external-game request 0012** ("spawn an authored group of entities at runtime, with its internal
  references remapped", priority *painful*): a fresh ten-part ragdoll goblin every launch, gear on
  breakable joints, dynamite spawned mid-round. The request folder is the priority queue.
- **World streaming W2** (`Plan/world-streaming.md`): `instantiate_into(&mut World, doc, region)`
  is a design with no code.
- **Editor prefabs**: the gap map lists "no prefabs" under Editor; the parity gate means wire methods
  first.

The gap map's instruction is explicit: grill the sim-side-versus-extract question first. It is
decision A below.

## What exists (2026-09-30)

**Hierarchy is one inert component.**

- `Parent(pub EntityId)` (`crates/ludion-ecs/src/hierarchy.rs`), registered like any component. Its
  module doc: "no automatic transform propagation, no child lists … Systems that care about hierarchy
  read `Parent` and do their own walking." `docs/components.md`: "a link only".
- Its schema carries the `x-entity-ref` mark with `hierarchy: true` (`entity_ref.rs`): never self
  (`L1017`), and a scene rejects cycles (`L1035`); `scene.save` refuses a cyclic world.
- **Nothing walks it.** `extract.rs` composes parent chains only *inside* one asset (assembly parts,
  skin joints), never across entities. `World::despawn` removes one entity; a child's `Parent` then
  dangles.
- **`Transform` is documented as world space** ("A position, orientation, and scale in world space",
  `crates/ludion-engine/src/components.rs`): `translation`, `rotation` as **Euler XYZ degrees**,
  `scale`. Every reader treats it as world: extract, particles, patterns (volley origins),
  `scene_physics` (the `PhysicsPose` seam), `skin_bone`, `ragdoll`, `transition` (carry),
  `voxelizer`, `render_view`, `debug_draw`; `path_follow` writes it.
- **Latent bug:** `ludion import` emits glTF nodes as entities with their **local** TRS plus a
  `Parent` (`crates/ludion-import/src/lib.rs`; `examples/import/expected/scene.yaml`). Since nothing
  propagates, any imported node below the root renders at its local offset from the world origin. A
  single-node or identity-root import hides it; the external game's `goblin-skinned` scene carries one such
  `Parent`.
- **Content:** no committed game scene uses `Parent` (vigil has none); only two CLI fixtures and the
  import example do.

**Instantiation is whole-world only.**

- `ludion_scene::instantiate(scene, registry) -> World` (`crates/ludion-scene/src/instantiate.rs`)
  spawns entities in document order into a **fresh** world, so *document position is the entity
  index* — the invariant that makes scene diagnostics' JSON Pointers line up with entities. Entity
  references in scene text are `{index: <document position>, generation: 0}`; nothing remaps them at
  load because the identity holds.
- `scene.save` (`Engine::world_to_scene`) flattens the live world back: live entities in slot order
  become positions, references are remapped through that table (`remap_entity_refs`).
- At runtime: `entity.spawn` (one entity, components by registry name) on the wire; `world.spawn()`
  plus `insert` in a system. Systems run with `&mut World`, so a system can spawn directly; the one
  command queue governs *protocol* traffic.
- The nearest thing to a prefab is **build-time**: a `kind: map-kit` `kinds:` entry stamped by a map
  marker, expanded by `generate-map` into a flat, drift-gated scene; `fragments:` concatenate whole
  scene files verbatim.
- `ragdoll::switch_to_ragdoll` is the one runtime group spawn: one entity per assembly part, joints
  wired, sim-side math through libm.

**Determinism and records.**

- `WorldHash` covers every **registered** component of every live entity; unregistered types are
  "invisible to the hash, exactly as they are invisible to scenes and snapshots" (`ludion-sim/src/hash.rs`).
- Replays embed sim-visible text inline: the scene, mid-session `scene.load`, data-doc reloads. Boot
  docs and assets are re-read from the content root ("a replay reproduces within its repo").
- Sim-side Euler↔quaternion goes through libm (`ludion_physics::euler_xyz_degrees_to_quat`,
  `ragdoll.rs`); extract's `euler_quat` uses glam with std trig. `Plan/rotation-representation.md`
  W1 (planned, not started) consolidates the conversions into `ludion_core::math`.
- Schedule order (`Engine::build_schedule`): `path_follow` (when a follower exists) → game systems in
  registration order → `physics_write`, `physics_step`, `physics_mirror`.
- 21 engine `kind:` rows in `ludion_scene::KINDS` — `Plan/document-kinds.md`'s "a twentieth kind is
  added" trigger has already fired.
- Wire: 31 methods (`render.debug` the latest).

**Related open request:** external-game request 0021 asks for presentation-only components excluded from
`WorldHash`. It is a different problem (presentation state a game *writes*) from derived state the
engine *computes*; decision A must not be solved by it, and it must not be solved by decision A.

## How other engines answer it

Researched for the options below; each line is what informs a decision, not a survey.

- **Bevy** (0.16+, 2025): `Transform` is local, `GlobalTransform` (an affine) is a stored component
  written by a propagation system late in the frame, driven by change detection. Reading
  `GlobalTransform` before propagation gives last frame's value — the well-known one-frame lag;
  `TransformHelper` computes a fresh value on demand as the workaround. Hierarchy became a
  *relationship* (`ChildOf` with an auto-maintained `Children`), and despawn became recursive by
  default. Scenes/prefabs: `DynamicScene` with a `MapEntities` remap; the patch-based BSN format is
  still in design.
- **Godot 4**: `Node3D.transform` is local; `global_transform` is computed lazily behind a dirty
  flag. Physics bodies simulate in global space and effectively ignore a moving parent. Prefabs are
  `PackedScene`s; a `.tscn` stores an instanced scene as a reference plus only the overridden
  properties; "editable children" and inherited scenes (variants).
- **Unity DOTS** (Entities 1.x): `LocalTransform` + `Parent`, a `Child` buffer maintained by a
  system from `Parent`, `LocalToWorld` computed by the transform system group each frame (sim-side,
  stored). **Physics bodies are un-parented at bake time** — a simulated body is always a root.
  Prefabs are baked entity groups; `Instantiate` clones a `LinkedEntityGroup` and remaps references
  inside the group — exactly external-game request 0012's shape.
- **Unity GameObjects**: nested prefabs and variants; per-instance overrides stored as
  `PropertyModifications` (object path + property path + value); apply/revert. Powerful, and the
  part of Unity most often cited as complex.
- **Unreal**: `USceneComponent` stores a relative transform and a cached `ComponentToWorld`, updated
  *eagerly* by setters that propagate to children immediately (no lag, because every write goes
  through a setter). Simulating physics detaches a component from its parent. Prefab roles are
  split between Blueprint classes (archetype + per-instance deltas) and Level Instances.

Three lessons carry over. Every engine makes **local the authored truth** and world derived. Every
engine that simulates physics **makes simulated bodies roots**. And every engine that stores the
derived world transform pays for its staleness with either change detection (Bevy), dirty flags
(Godot) or mandatory setters (Unreal) — all three are unavailable here (below).

## What the ECS freeze list implies

`engine-architecture.md` §4 rejects change detection and relations, and keeps hierarchy "a plain
`Parent(EntityId)` component". Concretely:

- **No incremental propagation.** Without change ticks nothing knows *which* transforms moved, so a
  stored world transform is either recomputed wholesale at fixed schedule points or is stale between
  them. Dirty flags would need setters, and component writes here are raw `&mut` access.
- **No auto-maintained `Children`.** A `Children` component kept in sync with `Parent` needs a hook
  on every `Parent` insert/remove — a relation in all but name. A stored child list written by
  engine verbs alone desyncs the first time game code does `world.insert(child, Parent(p))`. So the
  truth stays the parent link, and any child list is **derived** (one pass over the `Parent` column,
  which holds only parented entities).
- **Helpers over the `Parent` column are not a relation graph.** Cascade despawn, subtree
  collection, cycle checks and depth guards are functions over the one component; they sit inside
  the freeze list's letter. Changing what `World::despawn` *does* (cascade) is a semantic amendment
  to §4 and needs the user's ruling (decision E).
- **Budget.** `ludion-ecs` sits near its ≈3.5k-line budget; the hierarchy helpers are estimated at
  150–250 lines. Anything that needs `Transform` (which lives in `ludion-engine`) stays out of the
  ECS crate.

## Decisions

Each decision lists the options, what each costs, and a recommendation. Letters are stable handles
for the grilling.

### A. Where world transforms live — the central question

The four options, then the recommendation.

**A1. Stored and hashed `GlobalTransform` component (the Bevy / DOTS shape).** An engine system
recomputes a registered `GlobalTransform` for every entity at fixed points (after `path_follow`,
after game systems, after `physics_mirror`).

- *Determinism:* fine if composed with libm conversions and a fixed operation order; hashing it adds
  a cross-arch check of the composition itself.
- *Gameplay queries:* a plain component read — but **stale** between recompute points. A system that
  moves a parent and reads the child's world pose in the same tick reads last pass's value. Without
  change detection the fix is more recompute points or an on-demand helper — i.e. A3 anyway.
- *Hash, snapshots, save:* a derived value inside `WorldHash` roughly doubles transform hash bytes
  and adds no information (it is a function of `Transform` + `Parent`, both already hashed).
  `scene.save` and snapshots must learn to skip or re-derive it — a new "derived" registry flag.
- *Cost:* a full pass over every entity, several times per tick, even in vigil where nothing is
  parented.
- *par:* trivially parallel (each row walks its own ancestors, read-only; only the
  `GlobalTransform` column is written — legal in the region model).

**A2. Extract-only.** World transforms exist only in `extract()`; the sim never sees them.

- *Determinism:* none needed — render-only, glam and std trig are fine.
- *Gameplay queries:* **fails every consumer but rendering.** A held prop's hit test, a turret's
  muzzle position, a kinematic collider on a moving platform (physics needs its world pose), a
  volley spawned from a child emitter, carrying a subtree across a scene switch — each would walk
  `Parent` by hand in game code, which is today's gap restated.
- *Cost:* the cheapest; one batched pass per rendered frame.

**A3. Derived on demand; never stored, never hashed (recommended).** `Transform` becomes **local to
its parent when the entity has a `Parent`** (unchanged for roots). World pose is a *pure function*
of hashed state — `world_transform(world, entity)` walks the chain and composes root→leaf with one
sim-grade composition (libm conversions, a written-out scalar operation order). Sim systems,
physics and spatial call it when they need a world pose; `extract()` runs the same composition as
one memoized batch pass per frame.

- *Determinism:* the result is never stored, but it **feeds hashed state** (physics poses a
  kinematic child from it; game code acts on it), so the composition must be cross-arch bit-exact.
  That is the key design point: **"not hashed" does not mean "render-grade"**. A divergence in the
  composition surfaces in the first hashed component it touches, which the per-frame hash sequence
  and bisection already localize.
- *Gameplay queries:* always fresh — no lag, no recompute points to reason about.
- *Hash, snapshots, save, replays:* nothing new. `Transform` + `Parent` are already hashed, saved,
  snapshotted; restore needs no fix-up; the scene file holds only authored truth.
- *Cost:* zero for roots (world = local; vigil pays nothing). A parented entity costs one conversion
  and compose per ancestor per call. Extract's batch pass memoizes by entity, so each entity is
  composed once per frame. Estimated small; measured in W1 (gate below), with a per-tick memo as the
  recorded fallback (A3's trigger).
- *par:* the extract pass parallelizes like A1; sim callers are per-entity and need none.

**A4. `Transform` stays world; children store an offset (attachment).** A child carries an
`Attached { offset }`; an engine pass writes `child.Transform = parent world ∘ offset` at fixed
points.

- *Appeal:* no reader changes — every existing `Transform` reader stays correct, and the result is
  hashed for free.
- *Costs:* the same staleness as A1; a child's `Transform` written by game code is silently
  overwritten next pass (a footgun with no diagnostic, since nothing detects the write); scene text
  stores both the offset and a derived world `Transform` — derived data in the source of truth,
  which drifts. Diverges from every engine the agent's priors come from.

**Recommendation: A3.** Local is the authored truth (Pillar 4: the scene holds nothing derived),
freshness comes from purity instead of change detection (which the freeze list rejects), `WorldHash`
and every recorded artifact stay exactly as they are, and roots cost nothing — so vigil's hashes,
replays and budgets do not move. The price is a one-time audit of the engine's `Transform` readers
(decision B) and a sim-grade composition function (decision C).

### B. `Transform` semantics and the reader audit

Under A3, `Transform` means "relative to `Parent` when present, else world". Every engine reader is
classified once:

| Reader | Wants | Under A3 |
|---|---|---|
| `extract` (meshes, sprites, lights, camera, text, markers, `SkinBone`'s skin) | world | the batch pass |
| `particles` (emitter origin), `patterns` (volley origin) | world | `world_transform` |
| `scene_physics` / `PhysicsPose` write pass | world | `world_transform` for parented kinematic bodies |
| `physics_mirror` | writes world | only roots are simulated (decision D), so world = local |
| `ludion-spatial` colliders (`GroundPosition`) | world | `world_transform` for parented colliders |
| `path_follow` | writes | writes **local** — a path is in its parent's frame (a platform circling on a moving ship) |
| `transition` carry | identity | carries the subtree (decision F) |
| `ragdoll` | world | the root's world pose |
| `voxelizer` (GI bake), `render_view`, `debug_draw` | world | the batch pass |
| `ludion-camera` rig (character-controller W3, not built) | world target | `world_transform` |

Game code keeps reading `Transform` as local — correct for every root, which is every entity in
vigil and the external game today. `docs/components.md`'s `Transform` and `Parent` rows change, and a
`hierarchy` topic states the rule once. **Recommendation:** accept the semantic change; the only
committed content it affects is the glTF import output, whose rendering it *fixes*.

### C. The composition function and the rotation representation

- **Where:** `ludion_core::math`, beside the Euler↔quaternion pair that
  `Plan/rotation-representation.md` W1 moves there. **Prerequisite:** land that W1 first (or as this
  plan's W0) — one conversion home, libm-backed, used by sim and render alike.
- **Form:** compose as (translation, unit quaternion, scale) with an explicit scalar operation order
  (not glam's SIMD `Quat`/`Mat4` paths, whose instruction selection differs between SSE2 and NEON).
  A committed fixture of composed bit patterns, checked on both pinned machines, pins it cross-arch.
  Physics and ragdoll already run glam sim-side and were verified on the M4; the fixture makes the
  claim a gate for this function rather than an inference.
- **Non-uniform scale under rotation** composes to a shear, which TRS cannot hold. Render uses the
  full affine (correct shear). Sim callers get the affine plus a documented lossy TRS decomposition;
  a kinematic body under a non-uniformly scaled, rotated ancestor gets a warning diagnostic (bodies
  ignore scale anyway).
- **World→local** (keep-world reparenting, prefab placement bake) inverts the chain and converts
  back to Euler degrees — lossy, and the round-trip class `Plan/rotation-representation.md` lists as
  a W3 trigger. Expect noisy low digits in a saved keep-world reparent; quantize at the source (the
  gizmo), never in the writer.
- **Depth and cycles:** the walk stops at `MAX_HIERARCHY_DEPTH` (proposed 64). Verbs refuse cycles
  (`L1035`); a cycle made by raw game-code inserts is reported once through `report_diagnostic` and
  the entity is treated as a root — never an infinite loop, never a panic.
- **A dangling `Parent`** (a dead id) is treated as a root with a one-shot diagnostic; decision E
  makes it rare.

**Recommendation:** as listed; the open sub-question is only whether rotation-representation W1 is
a hard prerequisite (recommended) or folded into this plan's W1.

### D. Physics bodies under a parent

| Body under a `Parent` | Options | Recommendation |
|---|---|---|
| `rigid` (dynamic) | (a) refuse; (b) simulate in world, ignore the parent (Godot); (c) re-derive local after each step | **(a)**: `check-scene` error and a runtime diagnostic, fix "un-parent it, or join it with a `Joint`". The DOTS and Unreal precedent; the external game's goblin is exactly this case and is already joints, not parents. |
| `kinematic` | pose from the world transform | **Yes**: the write pass feeds `world_transform` into `set_next_kinematic_position`, so a crate bolted to a path-following platform, or a held prop, moves with contacts. |
| static (collider, no body) | (a) posed once; (b) re-posed each write | **Refuse under a parent** with the fix "make it kinematic". A static collider under a moving parent teleports without velocity, and contacts against it are wrong. |

The motor (`Plan/character-controller.md` decision 6) stays delta-based: the character is a root
kinematic capsule, and riding is not parenting. Propagation changes nothing in that plan except that
a platform may now *carry* attached geometry. **Held props** become a documented pattern: switch the
prop's body to kinematic, `reparent(prop, hand, KeepWorld)`; on release, un-parent keeping world,
back to rigid, set the velocity. Parenting to an *animated joint* (a socket) is not covered —
joint poses are extract-side — and stays deferred with `Plan/animation-clips.md`'s sockets.

### E. Despawn: cascade or not

- **E1. Cascade in `World::despawn` / `despawn_many` (recommended).** Descendants are collected from
  the `Parent` column and despawned in a defined order: the named entity first, then descendants in
  ascending slot index, so free-list reuse stays deterministic and testable. Bevy made the same
  change (recursive by default) because a dangling child is always a bug. Costs nothing where
  nothing is parented (vigil despawns hundreds of roots a tick: the `Parent` column is empty). An
  amendment to the §4 freeze list's semantics — **needs the user's ruling**.
- **E2. Primitive `despawn` stays; `despawn_tree` in `ludion-engine`**, used by `entity.destroy`.
  Game code calling `world.despawn` still orphans children — two ways, one of them wrong.
- **E3. Refuse to despawn an entity with children.** Explicit but hostile: every despawn site
  collects the subtree by hand.

Separately, **un-parenting before despawn** (keep the children, detach them) is an explicit
`reparent(child, None, KeepWorld)` call; no flag on despawn.

### F. Child lists, reparenting, carry

- **Child list:** derived only (freeze list). `children_of(world, e)` scans the `Parent` column;
  subtree collection builds a parent→children map in one pass. No stored `Children`.
- **Reparenting:** `Parent` stays writable raw (`component.set`, `world.insert`) with local-keeping
  semantics (the world pose jumps). One helper, `reparent(world, child, Option<parent>, Keep)` with
  `Keep::World | Keep::Local`, refuses cycles and computes the new local for `Keep::World`
  (decision C's inversion). On the wire, `entity.reparent` (decision K) — so the editor's outliner
  drag does not duplicate the inversion in TypeScript.
- **Scene switch carry:** a carried entity carries its whole subtree (a held torch goes with the
  player); a carried *child* whose parent is not carried is un-parented keeping world, with a
  warning. Today's carry is by list and has no notion of either.
- **`Residue`** capture is per entity by name; a subtree's members are captured individually, as
  now.

### G. Scene representation

- **G1. Flat list, `Parent` by document position (today; recommended).** No format change, the
  "document position is the entity index" invariant survives, JSON Pointers stay valid.
- **G2. Nested `children:` blocks.** Reads like a tree, but reparenting moves a whole YAML block
  (a large diff for a one-field change), and document position stops being entity index.
- **G3. `Parent` by name** (`Parent: lamp-post`). Kills the index shift when an entity is inserted
  above others — a real diff-noise cost today, for `Joint.a`/`b` too — but it is a change to every
  entity reference, not to hierarchy. Recorded as a follow-up if index-shift noise bites, not
  scheduled here.

The canonical writer needs no change under G1: order is slot order, and composition does not need
parents before children.

### H. The prefab document

- **H1. A prefab is a plain scene file.** Zero new format — but a scene carries a seed, a physics
  block and singletons (`Camera`, `DirectionalLight`, `Environment`, `PostProcess`, `GiVolume`)
  that must never be spawned twice (`L2101`), and nothing tells a tool the file is a template.
- **H2. `kind: prefab` (recommended).** The body is exactly a scene's `entities:` list — same
  component schema, same validator, same `L####` codes, same position-based references — plus
  prefab-only rules: no `seed`, no `physics:`, no singleton components, every entity **named and
  unique within the prefab** (the names are the handles overrides and provenance address). It is the
  22nd kind: `Plan/document-kinds.md`'s trigger has fired independently, and this plan should not
  wait on it but should add its row the table way.

```yaml
format: 1
kind: prefab
name: goblin
entities:
  - name: pelvis
    components:
      Transform: { translation: [0.0, 1.0, 0.0] }   # block style if the strict subset refuses flow maps here
      Collider: { shape: { capsule: { radius: 0.18, height: 0.3 } } }
      RigidBody: { mass: 8.0 }
  - name: thigh-l
    components:
      Transform: { translation: [0.1, 0.7, 0.0] }
      Joint: { a: { index: 0, generation: 0 }, b: { index: 1, generation: 0 }, kind: spherical }
```

References are positions *within the prefab*. A reference outside the prefab's range is a
`check-scene` error; a live entity outside the group (the player a turret targets) is bound by game
code after spawn — prefabs carry no parameters and no expressions (Pillar 6: Rust is the only
Turing-complete layer; overrides are the parameter mechanism).

### I. Runtime instantiation

One primitive for three callers — `scene.load`'s fresh world, world streaming's regions, prefabs:

```rust
ludion_scene::instantiate_into(world, entities, registry, &InstantiateOptions { placement, name_prefix, provenance })
    -> Result<Vec<EntityId>, Vec<Diagnostic>>
```

- **Order:** entities spawn in document order from the free list (LIFO reuse — deterministic),
  references remap position → new id through the returned table, ids come back in document order.
  All-or-nothing: validation runs before the first spawn, as `switch_to_ragdoll` does.
- **Placement:** composed onto each prefab entity **without an in-prefab `Parent`** (its roots) —
  baked, through decision C's composition. The alternative, a synthetic root entity parenting the
  whole group, is rejected: it would parent rigid bodies (decision D) and add an entity the author
  never wrote. A prefab that wants a single root authors one.
- **Names:** with an instance name, parts become `<instance>/<part>` (unique, stable, `Residue`-safe);
  without one they keep their bare names (the external game's disposable dynamite needs none).
- **From a system:** `spawn_prefab(world, "prefabs/goblin.prefab.yaml", placement, name)` reads a
  `PrefabLibrary` resource loaded lazily from the content root — the `PathLibrary` mold — and runs
  inside the tick, no queue involved (systems already own `&mut World`).
- **From the wire:** `prefab.spawn` applies at a frame boundary through the one queue.
- **Replays:** a wire `prefab.spawn` is recorded with the resolved prefab text inline (the
  `SceneLoad` rule). An in-system spawn records the prefab's resolved text inline the first time
  `PrefabLibrary` loads it in a session, and again on a mid-session reload (the `DocReload` rule),
  so a replay is self-contained and survives later prefab tuning.
- **Hash:** spawning adds entities like any spawn; no existing hash moves until content uses it.

`instantiate` (fresh world) becomes `instantiate_into` on an empty world, so there is one path.
World streaming's W2 then consumes the same function with a region offset and a `Region` tag.

### J. Prefabs in scenes: instances, overrides, nesting, variants

This is where editor prefabs live, and where the cost is.

- **J1. Stamp only.** Scenes stay flat; placing a prefab (agent or editor) instantiates and the next
  `scene.save` writes the expanded entities. Cheap, zero format change — but editing the prefab
  never reaches placed copies, so "change every lamp" is a find-and-replace across scenes.
- **J2. Build-time expansion** (the map-kit route): a source scene with prefab placements compiles to
  a flat, `generated_from`, drift-gated scene. Propagation by regeneration — but every hand-authored
  scene becomes generated, which is write-only for agents and breaks the editor's save.
- **J3. Instance entries with overrides (recommended, in its own wave).** A scene entity entry may
  be an instance:

```yaml
entities:
  - name: lamp-plaza-3
    prefab: props/lamp.prefab.yaml
    transform: { translation: [12.0, 0.0, -4.0], rotation: [0.0, 90.0, 0.0] }
    overrides:
      bulb:
        PointLight: { intensity: 2.0 }     # a partial component: listed fields only
    removed: [shade]                       # optional: parts this instance drops
```

  - **Load** expands in document order; the expanded parts occupy consecutive entity indices, and
    scene-level references to the entry resolve to its first surviving part. A diagnostic from the prefab file
    itself points into it; one raised after expansion points at the instance entry and names the part
    and prefab.
  - **Provenance:** each spawned part carries a registered `PrefabInstance { source, part, root }`
    component (hashed like any sim state; games may query it — "is this a goblin part").
  - **Save** collapses each instance back: re-expand the prefab unplaced, diff each live part
    against it field by field, write `overrides` / `removed`. Runtime-changed values become
    overrides, exactly as today's save captures live state.
  - **References into an instance** from outside it (a torch parented to `lamp-3`'s `hook` part):
    v1 refuses them at save with a diagnostic ("author it inside the prefab, or unpack the
    instance"); a `part:` qualifier on entity references is the recorded follow-up.
  - **Nested prefabs:** an entity entry *inside a prefab* may itself be an instance; expansion
    recurses depth-first, names compose (`turret/barrel/muzzle`), overrides address nested parts by
    that path, a cycle is an `L####` error.
  - **Variants** are a prefab containing a single instance entry plus overrides — no `base:` key,
    no second concept (Pillar 6: one way).
  - **Hot reload of a prefab file** re-expanding live instances in place (parts matched by name keep
    their ids) is deferred with a trigger (the editor live smoke asks for it); until then a prefab
    edit reaches instances at the next scene load, which is what `ludion render` does anyway.

**Recommendation:** J3, scheduled after the runtime wave, with J1's stamp available the moment the
runtime wave ships (`prefab.spawn` then `scene.save` *is* J1). Stop there if the grilling judges J3's
save-collapse cost larger than its value; J1 plus the map-kit route cover the agent's own loop.

### K. Wire methods (parity gate; each wave moves the schema hash once)

| Method | Wave | Shape | Recorded |
|---|---|---|---|
| `entity.reparent` | W1 | `{entity, parent: EntityId \| null, keep: world \| local}` | Command |
| world pose read | W1 | a `world: true` option on `query.entities` (preferred) or a new `entity.world_transform` | no |
| `entity.destroy` | W1 | unchanged schema; cascades under E1 | Command (as now) |
| `prefab.spawn` | W2 | `{path, transform?, name?}` → `{entities: [ids in prefab order]}` | Command, prefab text inline |
| `prefab.create` | W3 | `{entities, path}` → writes a new prefab from live entities (references outside the set are an error) | no (file write, like `scene.save`) |
| `prefab.apply` | W3 | `{instance, parts?}` → writes the instance's overrides into the prefab file | no |
| `prefab.revert` | W3 | `{instance, pointer?}` → drops overrides and re-applies prefab values | Command |
| `prefab.unpack` | W3 | `{instance}` → removes provenance; the parts become plain entities | Command |

Five to eight new methods over three schema bumps. An alternative that halves the count folds
apply/revert/unpack into one `prefab.edit {instance, op}`; the flat names read better to an agent and
match `entity.*`/`component.*`, so the table recommends them.

### L. Entity id stability and text-as-truth

- **Ids** of a spawned prefab are free-list-determined and returned in prefab order; editing a
  prefab's entity count shifts every later id in a session that spawns it — inherent, the same as
  any spawn today, and why replays embed the prefab text.
- **Save → load** renumbers (document positions), as today; `PrefabInstance.root` is an entity
  reference and remaps like `Joint.a`.
- **Generated prefabs:** a prefab may carry `generated_from:` (a bpy script, `ludion import`) and is
  then write-only and drift-gated like every generated document. Overrides in scenes survive
  regeneration as long as part names are stable — which is why parts must be named.
- **`ludion import` as a prefab producer** is the natural pairing (an imported model placed many
  times, its node hierarchy now correct): recommended as a W3 option, emitting `kind: prefab`
  beside today's scene output, one obvious way chosen when the grilling rules on it.
- **Map kits** could later emit instance entries instead of expanded primitives; not in scope.

## Waves

### W0 — prerequisite: one conversion home

`Plan/rotation-representation.md` W1 as written (libm Euler↔quaternion in `ludion_core::math`,
copies deleted), plus the scalar TRS composition and its cross-arch bit fixture. Its own gates
apply (goldens judged by SSIM; `WorldHash` expected not to move).

### W1 — hierarchy semantics

1. `Transform` local under `Parent`; `world_transform` (sim) and the memoized extract batch pass,
   both on W0's composition; the reader audit of decision B applied site by site.
2. Physics: kinematic children posed from world; rigid and static under a parent refused (check and
   runtime codes). Spatial colliders resolve world poses.
3. ECS: derived child map, subtree collection, cascade despawn (E1 if ruled), depth guard; the §4
   text amended if the ruling is E1.
4. `reparent` + `entity.reparent`; the world-pose read on the wire; carry takes subtrees; a
   `hierarchy` debug-draw category (parent→child lines, so a screenshot shows the tree).
5. Docs: `components.md` rows, a `hierarchy` topic, `dev-docs/ludion-ecs.md`; example
   `hierarchy.rs` (a turret on a moving platform, a held prop).

**Gates:**

- **Flatten oracle** (property test): for random trees, `world_transform` equals the pose of the
  same scene with world transforms baked flat, and a rendered hierarchy is golden-identical to its
  flattened twin on the pinned GPU.
- The import example renders correctly (its golden moves once, deliberately).
- Kinematic-child riding test through `physics_write`.
- Cascade despawn order and free-list reuse pinned.
- Cycle and dangling-parent diagnostics fire once and never hang.
- **No committed `WorldHash`, replay or budget moves** (vigil and the gallery have no `Parent`);
  the 1/2/8-thread identity gate green.
- A synthetic 10k-entity, depth-4 hierarchy measured in the profile at 8 threads, extract delta
  recorded in Status; A3's memo fallback triggers if it breaches the budget.

### W2 — runtime prefabs (answers external-game request 0012)

1. `kind: prefab`: reader, canonical writer, validator (prefab-only rules), `check-scene`, the
   `KINDS` row.
2. `instantiate_into` as the one instantiation path; `scene.load` rebuilt on it (a no-op for every
   hash).
3. `PrefabLibrary`, `spawn_prefab`, `PrefabInstance` provenance, `prefab.spawn`, replay records
   (inline text on spawn and reload).
4. Docs: a `prefabs` topic; example `prefab_spawn.rs` — a goblin-shaped jointed ragdoll spawned
   twice at two placements, then a one-body dynamite prop.

**Gates:**

- Remap correctness: `Joint.a`/`b`, `Parent` and a game component's marked field all land on the new
  ids.
- A spawn/despawn/spawn replay reproduces its hash trace from a checkout where the prefab file has
  since changed.
- All-or-nothing on a bad prefab.
- Thread identity.
- The external-game request's shape reproduced in the example; the request gets its Shipped section.

### W3 — prefabs in scenes (only if J3 is ruled in)

Instance entries, overrides and `removed`, nesting, variants-as-nesting, save collapse, the
cross-instance reference refusal, `prefab.create` / `apply` / `revert` / `unpack`, `ludion import`
emitting `kind: prefab` in place of its scene output.

**Gates:**

- load → save → byte-identical for a scene of instances with and without overrides (the canonical
  idempotence property extended).
- Nested-cycle and bad-override-path diagnostics point into the right file.
- `apply` then reload equals the pre-apply world.

### W4 — editor

A prefab list in the asset browser, place-by-drag over `prefab.spawn`, outliner grouping by
instance with override markers, apply/revert/unpack, outliner drag-reparent over `entity.reparent`,
the gizmo placed at the world pose while editing local. The live smoke goes to `Plan/owed.md`.

### Exit (human gate, in `owed.md`)

One short sitting: place, override, apply and reparent in the editor; the held-prop and
bolted-to-platform behaviours in the platformer's course. The verdict is recorded as a decision.

## Non-goals

- A stored `Children` component, relations, change detection (freeze list).
- A stored or hashed world transform (decision A) — unless W1's measurement fires the memo trigger,
  and even then the memo is an unregistered per-tick cache, never hashed or saved.
- Sockets on animated joints (animation-clips' deferred sockets).
- Prefab parameters or expressions (overrides are the parameter mechanism).
- Hot reload of live instances (trigger: the editor smoke asks for it).
- `Parent` by name / part-qualified references (trigger: diff noise or the first cross-instance
  attach request).
- External-game request 0021's presentation-only flag — related, separate, not solved here.

## Risks

- **Silent semantic change.** External game code reading a *child's* `Transform` as world is wrong
  after W1. Neither consuming game parents anything in code today; the release notes and the
  `hierarchy` topic say it in one line.
- **Cross-arch composition drift.** Guarded by W0's bit fixture on both pinned machines. If glam
  ever leaks back into the sim path, the fixture and the physics traces fail.
- **Euler round-trips** in keep-world reparent and placement bake: noisy low digits in saved text;
  a pole case could twitch. This is a named trigger for rotation-representation W3 (quaternion
  storage); the plan does not pre-empt it. Save's exact fallback cannot reach the root of a
  nested instance placed with a rotation; it settles after one save — a named input to that
  trigger.
- **Save collapse (W3) is the complex part** — Unity's prefab-override system is the cautionary
  tale. Mitigated by names as the only addressing, no structural edits inside instances except
  `removed`, no cross-instance references in v1, and the idempotence gate.
- **ECS budget.** Hierarchy helpers must stay near 200 lines; anything `Transform`-shaped lives in
  `ludion-engine`.
- **Extract cost on dense hierarchies.** Measured in W1; the memo is the recorded lever.
- **Kind count.** The 22nd kind lands while `Plan/document-kinds.md` is unbuilt; the row costs the
  usual ~8 edit sites.

## Rulings (2026-09-30)

The user's rulings from the grill-me session; letters are the decisions above.

1. **A3.** World pose is derived on demand from hashed `Transform` + `Parent` by one sim-grade
   composition (libm, fixed scalar order); never stored, never hashed; extract runs it as a memoized
   batch pass. `Transform` is local under a `Parent`; roots unchanged; the reader audit (B) is W1's
   first task. `path_follow` writes local (a path lives in its parent's frame).
2. **W0 is its own wave, first:** rotation-representation W1 + the scalar TRS composition + the
   cross-arch bit fixture. `ludion::math`'s re-exported pair (external-game request 0016) keeps its names.
3. **C as proposed:** shear composes to the full affine for render and a documented lossy TRS for
   sim (one-shot warning on a kinematic body under it); `MAX_HIERARCHY_DEPTH = 64`; a cycle from raw
   inserts or a dangling `Parent` is treated as a root with a one-shot diagnostic.
4. **D:** rigid and static under a parent refused (check and runtime); kinematic posed from world.
5. **E1:** `World::despawn` / `despawn_many` cascade (named entity, then descendants ascending
   slot). The architecture §4 freeze-list amendment is ruled in.
6. **F as proposed:** derived child lists only; raw `Parent` writes keep local; `reparent(…, Keep)`
   and `entity.reparent {keep: world | local}` refuse cycles; scene-switch carry takes subtrees.
7. **G1:** flat list, positional `Parent`; `Parent`-by-name is a follow-up on a diff-noise trigger.
8. **H2:** `kind: prefab`, named unique parts, no seed / `physics:` / singletons.
9. **Placement** baked onto the prefab's roots, no synthetic root; parts named `<instance>/<part>`
   when the spawn is named, bare otherwise.
10. **`PrefabInstance`** registered and hashed.
11. **Replays:** in-system spawns record prefab text inline on first use per session (§I); the W2
    gate stands as written.
12. **J3 in, as W3 after W2.** Per-part partial overrides (a part may gain a component) plus
    `removed`; no added parts. Nesting with composed name paths; variants are single-instance
    prefabs, no `base:`. References into an instance from outside are refused at save in v1; the
    `part:` qualifier waits for a real case.
13. **Wire:** W1 `entity.reparent` + a `world: true` option on `query.entities`; W2 `prefab.spawn`;
    W3 flat `prefab.create` / `apply` / `revert` / `unpack`.
14. **`ludion import` emits `kind: prefab`, replacing its scene output** (W3). The external game's
    `goblin-skinned` scene is affected: W3 leaves a note in the external game's request folder, never an edit in
    the game repo.
15. **Deferred with triggers:** hot reload of live instances (the editor live smoke asks); the
    per-tick world-pose memo (W1's measurement breaches the budget).
16. **Order W0 → W1 → W2 → W3 → W4**, dependency order. This plan owns external-game request 0012 (removed from the
    game-requests queue); one fresh Tech Lead per wave.

## Status

Settled 2026-09-30 (§Rulings).

- **W0 shipped 2026-10-01** — one conversion home and the sim-grade composition in
  `ludion_core::math` (`Trs::mul_trs`/`inverse`, `Affine3`, `to_trs_lossy`), cross-arch bit fixture
  `crates/ludion-core/tests/fixtures/trs_bits.txt`; detail and the deliberate leftovers in
  `Plan/rotation-representation.md` Status. M4 fixture run and gallery re-bless owed.
- **W1 mechanized 2026-10-01.** Shipped: the ECS `ChildMap` and cascade despawn; `world_transform` / `world_affine` with `L1218`–`L1220`; `extract`'s `RenderPose` over a dense slot memo; physics and the spatial index under a parent (`L1221`–`L1223`); `reparent`, `entity.reparent` (wire method 32), `query.entities {world}` and the destroy `descendants` payload; scene-switch carry of subtrees with `L3043`; the `hierarchy` debug-draw category; `docs/hierarchy.md`, the dev-docs and `crates/ludion/examples/hierarchy.rs`. Perf at 10k entities, depth 4, 8 threads: extract median delta 1.14 ms and fault scan 0.34 ms against limits 2.0 / 0.5 ms — the first cut was over, fixed by an allocation-free walk and a dense slot memo, so ruling 15's per-tick memo stays deferred. Deliberate moves: the import example's Windows golden re-blessed once; the 12 committed `.lrp` headers re-recorded (schema hash only, per-frame `WorldHash` unchanged).
- **W2 mechanized 2026-10-01.** Shipped: `kind: prefab` (`L1226`–`L1229` in `check-scene`); `instantiate_into` as the one instantiation path (the scene load is rebuilt on it); `PrefabLibrary`, `spawn_prefab` and `PrefabInstance` (`L1230` for an unresolvable prefab; `asset.reload` re-validates eagerly and refuses a bad edit with the prefab's own diagnostic); replay record kind 6 `prefab_text`; `prefab.spawn` (wire method 33); `docs/prefabs.md`, `crates/ludion/examples/prefab_spawn.rs` and its query-drift test, and the prefab thread-identity case. Deliberate moves: the 12 committed `.lrp` headers re-recorded (schema hash only, per-frame `WorldHash` unchanged). The review follow-up resolves replay prefab text per rebuild epoch, with reloads as drive actions and a reload flag on `prefab_text`. Owed: nothing.
- **W3 mechanized 2026-10-01.** Shipped: prefab instance entries in scenes and prefabs (`prefab:` with `name`, `transform`, `overrides`, `removed`; `L1231`–`L1233`, nesting, variants); load and replay of scene prefab reads; save collapse (`L1234`, save after apply); `prefab.create` / `apply` / `revert` / `unpack` (`L1235`–`L1238`, wire methods 34–37); `ludion import` emitting `kind: prefab`; `docs/prefab-instances.md`, `crates/ludion/examples/prefab_scene.rs` and the nested-instance thread-identity case. Deliberate moves: the 12 committed `.lrp` headers re-recorded (schema hash only, per-frame `WorldHash` unchanged). Follow-ups: content-root-relative import mesh paths (`--root`); the editor mesh lift ignores prefab-placed meshes. Owed: nothing.
- **W4 mechanized 2026-10-01.** Shipped in the editor: outliner drag-reparent over `entity.reparent` (undo bit-exact) and the gizmo at the world pose writing local; instances grouped in the outliner with apply / revert / unpack; override markers with per-field revert over `prefab.overrides` (wire method 38, a read-only query sharing save collapse's diff — an extension of ruling 13, approved by the feature lead because the parity gate allows no editor-private read); a prefab browser with place-by-drag over `prefab.spawn`; the mesh lift reads prefab parts. `prefab.overrides` cost on 50 instances + 3000 plain entities (release, reference box): OK 146 → 5.1 ms, refused 3753 → 5.2 ms (targets ≤ 8 ms each); the first read after a load adds a one-time ~40 ms parse. Faults outside instance members no longer fail the read; save still refuses them. Owed: the W4 live smoke (`Plan/editor-live-smoke.md`) and the Exit sitting (`Plan/owed.md`). Follow-ups, shipped 2026-10-02: a load seeds the overrides read's authored-document cache, so the first read after a load costs no more than any other; a scoped refusal names an unnamed entity by its world position, as save does; `prefab.overrides` reports each instance's `root_parts`, the editor's ↺ follows them, and the schema hash moves with the 12 `.lrp` headers re-recorded (per-frame `WorldHash` unchanged); a parented gizmo drag reports a fresh world rotation.
