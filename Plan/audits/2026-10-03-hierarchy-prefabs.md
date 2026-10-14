# Audit: transform hierarchy and prefabs (2026-10-03)

**Scope.** `Plan/transform-hierarchy-and-prefabs.md` (W0–W4 mechanized) against the code at rev
`fe19a265`: `crates/ludion-ecs/src/hierarchy.rs` and the cascade in `crates/ludion-ecs/src/world.rs`;
`crates/ludion-engine/src/hierarchy.rs`, `instantiate.rs`, `prefab.rs`, `prefab_ops.rs`,
`commands/prefab.rs`, `commands/entity.rs` (`destroy`, `reparent`), the hierarchy-facing parts of
`extract.rs`, `scene_physics.rs`, `skin_bone.rs`, `transition.rs`, `path_follow.rs`, `patterns.rs` and
the audio sync in `engine.rs`; `crates/ludion-physics/src/systems.rs` (`write`) and
`crates/ludion-spatial/src/spatial.rs` (`rebuild`, `move_and_slide`); `crates/ludion-core/src/math/trs.rs`;
`crates/ludion-scene/src/{expand,instantiate,instance,prefab}.rs`; `collapse.rs` skimmed (its
reference refusals and placement recovery, not its full candidate pipeline); the editor's
`editor/app/src/editor/{hierarchy,undo,mutations}.ts` and `gizmo/drag.ts`; vigil's
`occlusion.rs` / `OCCLUDER_PREFAB_HEIGHTS`; `docs/hierarchy.md`. Static reading plus two throwaway
programs outside the repo (one against `ludion-core`) and one `ludion check-scene` run on a scratch
prefab scene.

## Overall assessment

The hierarchy core is careful and consistent: the three walks (`chain`, the extract memo
`WorldAffines::parented`, the fault scan `fault_of`) agree on cycles, dead parents and the 64-level
cap, cascade despawn order is defined and generation-safe, and every engine reader that wants a world
pose (extract, physics, spatial, audio, particles, patterns, carry, anchors) gets one. Prefab expansion
validates thoroughly — override typos, removed-part references, nested loops and cross-level parent
cycles are all caught with pointers into the right file. The one real correctness bug is in the
world→local inversion: `Keep::World` reparenting (the editor outliner's drag) moves the child's
*position* under a rotated, non-uniformly scaled parent, which the docs describe as only a rotation
loss under shear. The remaining findings are per-tick waste that the plan assumed away now that vigil's
town square carries `Parent` rows, unbounded library bookkeeping, and smaller edge cases.

## High

### H1 — `Keep::World` reparent moves the child's world position under a rotated, non-uniformly scaled parent · High · [verified]

**Evidence.** `crates/ludion-engine/src/hierarchy.rs:421` computes the new local as
`parent_world.inverse().mul_trs(&child_world)`. `Trs::inverse` (`crates/ludion-core/src/math/trs.rs:85`)
returns `r' = conj(r)`, `s' = 1/s` as a TRS, and `mul_trs` then forms the translation as
`t' + rotate(r', s' ⊙ c.t)` — it scales *before* un-rotating. The true local translation is
`s⁻¹ ⊙ rotate(r⁻¹, c.t − p.t)` (un-rotate, then un-scale). The two agree only for uniform scale or an
axis-aligned rotation. Reproduced against `ludion-core` in a scratch program: parent at the origin,
rotated 90° about Y, scale `[2, 1, 1]`; child at world `(1, 0, 0)`, unrotated. The computed local is
`(0, 0, 0.5)` and re-composing it gives world `(0.5, 0, 0)` — the child jumps half a metre, and it
is not sheared (`WorldPose::sheared` stays false because the child has no rotation). The same
expression localizes skinned parts at spawn (`crates/ludion-engine/src/skin_bone.rs:71`).

**Why it matters.** `entity.reparent {keep: world}` is what the editor outliner's drag and parent
picker send (`editor/app/src/editor/hierarchy.ts`, `reparentCommand`), and `docs/hierarchy.md`
says `Keep::World` is lossy only "under a shear", recommending only that rotating parents keep uniform
scale. In fact the *position* is wrong for any child (rotated or not) under such a parent, and nothing
reports it. The editor's own gizmo does this inversion correctly in TypeScript
(`editor/app/src/editor/gizmo/drag.ts:302`, `translated`: un-rotate, then divide by scale), so the
engine and its client disagree.

**Failure scenario.** A designer drags a lantern onto a wall segment authored as `scale: [4, 1, 0.5]`,
`rotation: [0, 90, 0]`. The lantern lands somewhere else along the wall, and the next `scene.save`
writes that wrong local.

**Fix.** Add one exact helper in `ludion_core::math` (e.g. `Trs::localize(&self, world: &Trs) -> Trs`),
with translation `s⁻¹ ⊙ quat_rotate(conj(r), w.t − t)` and rotation and scale as today (still lossy
under shear). Use it in `reparent` and `apply_bone_poses`. Add a property test: for random parents with
non-uniform scale, `parent.mul_trs(&parent.localize(&w)).translation ≈ w.translation`. Fix the
`docs/hierarchy.md` gotcha. Leave `collapse.rs:1344`/`:1743` alone: there `X.mul_trs(&Y.inverse())`
puts the lossy inverse on the child side, and the translation is exact.

## Medium

### M1 — Every `despawn` / `despawn_many` builds a full `ChildMap` once any `Parent` exists, even for an empty id list; vigil now pays it every tick · Medium · [verified behaviour; cost unmeasured]

**Evidence.** `crates/ludion-ecs/src/world.rs:245` (single despawn) and `:317` (`despawn_many`:
`self.has_parents().then(|| ChildMap::build_full(self))`, with no `ids.is_empty()` guard). The map is
a `Vec` of every `(child, parent)` link plus a `BTreeMap<EntityId, Vec<EntityId>>`. The plan priced the
cascade as free because "vigil despawns hundreds of roots a tick: the `Parent` column is empty"
(decision E1). That stopped being true when the town square replaced West Breach: there are 119 prefab
instances in `games/vigil/scenes/district-breach.yaml`, and most town prefabs parent one to six
presentation parts (`games/vigil/scenes/prefabs/town/*.prefab.yaml`). Vigil calls `despawn_many` from
about 15 per-tick sites, most with usually-empty lists (`games/vigil/src/movement.rs:471`, `:668`;
`combat.rs:83`, `:352`, `:428`; …). Each call now rebuilds a map of a few hundred links
for nothing.

**Why it matters.** This is per-tick waste that grows with district dressing, and it lands in the
driving game's sim budget, against a plan claim the code no longer meets.

**Fix.** In `despawn_many`, return early on empty `ids`. In both functions, build the map only if a
doomed id actually appears as a parent: scan the `Parent` column's values, a dense slice, before
building, or build lazily on first need. Re-measure vigil's tick with the profile afterwards.

## Low

### L1 — `PrefabLibrary.bound` and `.placements` grow with every spawn and are never pruned · Low · [verified]

`crates/ludion-engine/src/prefab.rs:631-641` inserts per spawned root, and `:210`/`:215` are the only
writers. Nothing removes an entry when the instance despawns, so the maps grow until the next world
rebuild (`install_prefab_library`). A game that spawns a prefab per shot or per round (the external game's
dynamite, the shmup's projectile use case) accumulates one `BTreeMap` node per spawn for the whole
session. `snapshots` also clones the full held-text map on every accepted reload (`:373`).

**Fix.** Prune the entry for each dead root lazily: on `bind`/`place`, drop entries whose root is no
longer alive, or sweep once per N spawns.

### L2 — Skinned-part localization ignores ancestors above a transform-less direct parent · Low · [verified by reading]

`crates/ludion-engine/src/skin_bone.rs:63-65` localizes against `world_transform(world, parent)`, which
returns `None` whenever the *direct* parent has no `Transform` (`hierarchy.rs:113`). The part then
stores its world pose as a local, and any `Transform` on a grandparent is applied twice. `reparent`
uses `chain_pose` for exactly this reason (`hierarchy.rs:417`).

**Fix.** Use `chain_pose` (or the new helper from H1) for the parent's pose.

### L3 — `reparent` checks only the new ancestor count, not the moved subtree's depth · Low · [verified by reading]

`crates/ludion-engine/src/hierarchy.rs:375-385` refuses with `L1220` when the child itself would exceed
`MAX_HIERARCHY_DEPTH`. It accepts a move that pushes the child's *descendants* past the cap. Those
descendants then silently degrade to roots (they render at their local pose at the world origin) and
surface only as a runtime `L1220` at the next drain.

**Fix.** Add the subtree's height (one `ChildMap` walk, only when the new depth is within 64 of the cap)
to the check.

### L4 — Per-tick string formatting and walks in the hierarchy reporters · Low · [verified by reading]

- `scene_physics::report_parented_bodies` (`crates/ludion-engine/src/scene_physics.rs:192-201`) formats
  the `named` label for every parented solver collider every tick, before it knows whether anything
  will be reported. For kinematic children it also re-runs `world_transform`, which allocates in
  `chain`.
- `report_hierarchy_faults` (`hierarchy.rs:470-500`) builds every faulty entity's message, and for a
  cycle walks `cycle_members`, every tick, although the `Diagnostics` latch drops all but the first.

Neither costs anything in a clean world, but a platform carrying a dozen kinematic crates pays the
formatting forever.

**Fix.** Build the label and message inside a closure, after a cheap latch probe. Skip non-sheared
kinematic bodies before formatting.

### L5 — `entity.destroy` collects descendants with a linear `Vec::contains` per child · Low · [verified by reading]

`crates/ludion-engine/src/commands/entity.rs:96-101`: `set` is the `Vec` from `descendants()`, and the
BFS calls `set.contains(&child)` for each child, which is O(n²) in subtree size. The output order is
correct (parent-first, as the editor's undo needs).

**Fix.** A `BTreeSet`, or check `seen` while walking.

### L6 — Stale module comment in the editor outliner · Low · [verified]

`editor/app/src/editor/hierarchy.ts:4` says "The engine does not walk hierarchies itself
(Plan/phase-6-editing.md, engine facts)". Since W1 the engine composes world poses, cascades despawn
and refuses cycles in `entity.reparent`; the editor's tolerance of dangling or cyclic links is still
right, but its stated reason is not.

**Fix.** Rewrite the header in present-state terms.

### L7 — vigil's prefab occluder height ignores the root's scale · Low · [suspected, latent]

`games/vigil/src/occlusion.rs:198-207` adds the unscaled `OCCLUDER_PREFAB_HEIGHTS` entry to the root's
`translation.y`, while a primitive occluder uses `0.5 * scale.y`. Every committed instance is placed at
scale 1, so nothing is wrong today. A scaled instance (the placement bakes its scale onto the root's
`Transform.scale`) would fade at the wrong height.

**Fix.** Multiply by `t.scale[1]`, or assert scale 1 in the existing heights test.

*Aside, outside the topic:* an untracked `crates/ludion-engine/src/engine.rs.orig` (a merge leftover)
sits beside `engine.rs`. A path-less or `--all` commit would sweep it into the tree.

## Checked and healthy

- **Walk consistency.** `chain`, `WorldAffines::parented` and `fault_of` agree on cycle → root, dead
  or presentation parent → chain stops at the last valid ancestor, and 64 ancestors allowed / 65
  refused. The memo resumes from a valid prefix, so it is bit-identical to `world_affine`.
- **Cascade despawn.** Named entity first, then descendants in ascending slot order. `ChildMap` matches
  parents by full `EntityId`, so a stale `Parent` is never a child of a reused slot. A presentation
  entity never takes a sim child. `despawn_many` skips descendants already freed.
- **Readers.** Extract (meshes, particles, lights, sun, camera via `render_pose`, `inherited_fade`), the
  physics write pass (rigid and static under a parent dropped; kinematic posed from world), the spatial
  rebuild and `move_and_slide`, the audio listener and emitters, volley origins, `path_follow` (writes
  local), carry (subtrees and `L3043`), the anchor lookup and ragdoll all use derived world poses.
  Roots are untouched, so vigil's hashes did not move.
- **Scene and prefab validation.** Nested prefab loops (`L1232`), unknown override parts and removed
  parts with nearest-match help (`L1233`), removing a part another part references, an override typo
  (ran `check-scene` on a scratch scene: `L1014` at
  `/entities/0/overrides/bulb/Transform/tranlsation`, "nearest match is `translation`"), cross-level
  parent cycles after expansion, and a presentation-first instance root (`L1036`). Diagnostics from a
  prefab file keep their pointer into that file and gain a `related` link to the instance entry.
- **Placement order.** `instantiate_expanded` places nested instances (post-order) before the outer
  instance and the spawn placement, giving `outer ∘ nested ∘ local`. An identity placement is not
  baked, and baked `-0.0` is unsigned.
- **All-or-nothing spawn.** A phase-boundary refusal and a dry-run deserialize run before the first
  spawn; the prefab unit tests pin the allocator state after a refusal.
- **Parity.** The outliner reparents only through `entity.reparent` (keep world). Undo uses keep local
  plus a `Transform` set. Destroy-undo consumes the `descendants` payload, which the engine emits
  parent-first via BFS. Prefab actions go over `prefab.*`. `dispatch.rs` arms are one-line delegations.
- **Determinism and records.** `PrefabInstance` is registered and hashed. `ChildMap` uses ordered maps.
  The library is rebuilt on restore, so no stale `bound`/`placements` survive one. vigil's occlusion
  keys on `PrefabInstance.source` of the root only, read from sim-visible state.

## Suggested order of work

1. **H1.** Add the exact localize helper with its property test, switch `reparent` and
   `apply_bone_poses` to it (fixing L2 in the same edit), and correct the `docs/hierarchy.md` gotcha.
2. **M1.** Guard and lazy-build the despawn `ChildMap`, then re-profile vigil.
3. **L3, L1.** The reparent subtree-depth check and pruning dead roots from the library maps.
4. **L4, L5, L6, L7.** Small cleanups; batch them with the next wave touching each file.
