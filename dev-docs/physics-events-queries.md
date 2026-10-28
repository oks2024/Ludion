---
description: Physics contact events, the three read-only queries, and shape-rejection retry, mapped onto rapier3d
---

# Physics events, queries and shape rejection

**Scope:** How `ludion-physics` (`crates/ludion-physics/src/{world,queries}.rs`) maps its contact
events, its ray/shape/overlap queries and its shape-rejection retry onto rapier3d/parry3d calls.
Defers to `ludion devdocs ludion-physics` for the crate's boundary rule, the write/step/mirror
triple, and the Shape mapping and Tunables tables this topic cites.

## Contact events, mapped

`PhysicsWorld::step` clears the previous step's list, calls `rapier.step()`, then diffs. Rapier's
own `EventHandler` is not used: its `Started` fires before the solver has run, before any contact
has a resolved impulse — and impulse is exactly what the launcher's scoring needs. Instead
`collect_contacts` (`world.rs`) walks `narrow_phase.contact_pairs()` once, keeping only pairs with
`has_any_active_contact()`, and reads each pair's two entities off the colliders' `user_data`
(`entity_bits`/`entity_from_bits` encode an `EntityId` into the `u128` every collider this crate
creates carries — never unset for a pair this crate built). That list is sorted `(a, b)`, then
merge-diffed with a single pass against the previous step's already-sorted `touching`: no second
sort. A pair only in the new set emits `Started` with `total_impulse_magnitude()` (rapier sums it over the substeps, so it is the step's and independent of `solver_iterations`); a pair only in
the old set emits `Stopped` with `0.0`; a pair in both emits nothing. `touching` — not `contacts` —
is what the snapshot carries forward, so a restored world's first diff has the same baseline the
uninterrupted run would have had.

A start's `point` and `normal` come from `contact_point` (`world.rs`), which walks the pair's
`solver_manifolds()` and, for each `solver_contacts` entry, reads the manifold point it names
(`contact_id`, masked with `NEW_CONTACT_BIT`). The candidate's point is the midpoint of the manifold
point's `local_p1` and `local_p2`, each carried to world space through the manifold's sub-shape frame
and the collider's **detection** pose (`subshape_pos.prepend_to(pose)`), and its normal is
`local_n1` rotated by the first collider's frame. The local frames are relative to the poses the
step's collision detection ran at, before `advance_to_final_positions` moves the bodies, so
`PhysicsWorld::step` records every collider's parent body pose composed with its offset into one
reused buffer (`detection_poses`) just before `rapier.step()`; reading the post-step `co.position()`
instead skews a moving or rotating first collider's point by a step of travel and its normal by the
step's rotation. Rapier's own world-space contact fields are recycled between steps and can be
stale, so they are never read. The pick is by `total_cmp`: impulse descending, then `dist` ascending (it is signed, so a
penetration is negative and the deepest goes first), then point, then normal, both lexicographic —
never the solver's iteration order; a resting start with zero impulse still lands on a
deterministic contact. `collect_contacts` then flips the normal to point from the lower entity to the higher
when the collider order was the other way round, matching the `a < b` canonicalization. A stop
carries `[0.0; 3]` for both. Both are derived per step and deterministic for a given
`solver_iterations`; the pick is an argmax over per-point normal impulses, which the iteration count
redistributes, so a single-contact start (a sphere) is stable across counts to tolerance while a
multi-point start (a box landing near-flat) may pick a different corner at a different count.

## Joint break events, mapped

`PhysicsWorld::step` clears `joint_events` alongside `contacts`, then — after `collect_contacts` —
`collect_broken_joints` (`world.rs`) walks the joint table (creation order) once: an entry with a
`break_impulse` and a live `handle` reads `rapier.impulse_joints.get(handle).impulses`, a rapier
`[Real; 6]` (`LinX/Y/Z/AngX/Y/Z`), and compares the linear part's magnitude (indices `0..3`) against
the threshold. Past it, `remove_impulse_joint` fires, `handle` becomes `None`, the entry is marked
`broken` (so `reconcile_joints` never tries to reattach it before the mirror removes its `Joint`
component), and a `JointBroken { joint: carrier, a, b, impulse }` is pushed. The walk is table order,
not carrier order, so the list is sorted by `joint` (carrier) before `step` returns — the
`ContactEvent` contract, even though a break, unlike a contact transition, never repeats for the same
carrier.

## Sensors, mapped

`collect_sensors` (`sensors.rs`) runs last in `step`, after `collect_broken_joints`. For each table entry with `source.sensor` (creation order) it calls `query_pipeline().intersect_shape` with the sensor collider's own pose and shape, keeping candidates that are not the sensor itself, not sensors (the explicit sensor–sensor filter: rapier would report them), and pass `collision_groups().test`. Rapier's intersection graph is not used: its narrow phase runs before kinematic bodies move, so it would judge a kinematic at last tick's pose; the step's closing broad-phase update leaves the query pipeline at the post-step poses. The pairs `(sensor, other)` are sorted and deduped, then merge-diffed in one pass against `sensing`, the previous baseline, into `sensor_events` (cleared at the top of `step`); `occupants` is `sensing`'s second column, rebuilt on every collect and on restore so `sensor_occupants` can return a slice.

A baseline pair absent from the new set gets its cause from `exit_cause`: `Despawned` if either id is in `despawned`; else `Removed` if either row is gone from the table, the sensor flag no longer holds on the sensor, the other became a sensor, or the collision groups no longer test; else `Separated`. `despawned` is fed by the write system (`note_despawned`, entities the ECS no longer holds) and cleared at the end of a non-frozen step, so a frozen step keeps it for the step that judges the exit. A sensor toggle is a collider rebuild, which is why it reads `Removed`.

`query_drops` drops sensor hits in `ray_cast`, `shape_cast` and `overlap` unless `QueryFilter::include_sensors` is set; it checks the table row's `source.sensor`.

## Queries, mapped

All three (`queries.rs`) read `rapier.query_pipeline()` against the state as of the last step; none
touches the step pipeline. `ray_cast` calls `intersect_ray` directly. `shape_cast` does not use
rapier's own first-hit `cast_shape` — its tie-break among equal times of impact is BVH traversal
order, not the wrapper's pinned `(distance, entity)` order — so it gathers candidates from
`intersect_aabb_conservative` over the union of the start and end poses' AABBs, then casts each
candidate exactly with `dispatcher.cast_shapes`, using `ShapeCastOptions::with_max_time_of_impact`
to bound the distance the same way `max_distance` bounds `ray_cast`. `overlap` calls
`intersect_shape`. All three build their own query shape through `shape_builder`, the same function
`create` uses for an authored `Collider`, so a query shape fails to build under exactly the rule the
`ludion devdocs ludion-physics` Shape mapping table states — `QueryShape` carries the same
`L1181`/`L1182` a scene-authored collider would get. Each function sorts its own output after
collecting; none of rapier's iteration orders is exposed.

Each takes a `QueryFilter`. `PhysicsWorld::query_drops` (`world.rs`) resolves its ignored layer names
before any candidate is gathered, so a bad name errors even in an empty world, and reads each hit's
layer from the wrapper's own table row (`source.layer`), never from rapier's groups, whose builder
default is `all()`. It only drops hits after collection, so the order contract is untouched.

## Shape rejection

A `Collider` whose shape the solver cannot build gets no body. `create` (`world.rs`) remembers the
exact rejected `Collider` in `rejected: StableMap<EntityId, Collider>`, so an unclaimed entity is
not re-attempted every tick — the retry resumes only once the entity's `Collider` changes (including
changing back to the same bad value, which clears and re-populates the map entry). Each rejection is
also pushed onto `pending_rejections`, a queue this crate cannot drain itself: it does not depend on
`ludion-engine` and cannot see its `Diagnostics` resource. `ludion-engine`'s
`scene_physics::report_rejections` drains `take_rejections` every tick and reports each one through
`diagnostics::report`, whose own once-per-`(code, key)` latch (`ludion-engine/src/diagnostics.rs`)
is what keeps a standing rejection from printing every tick — `pending_rejections` itself carries no
memory across a restore. `retire` drops a `rejected` entry too, once its entity leaves the live set,
so a despawned entity's dead shape does not sit in the map forever.

`reconcile_joints` (`world.rs`) follows the same seam for a `Joint` whose `a` or `b` has no body,
with the opposite bound on `pending_missing`: it queues the carrier only on the **transition** into
inert (the entry's `handle` was `Some` and became `None` this tick), not every tick the condition
still holds — a bare host that never calls `take_missing_joints` (`render`, `sweep`) must not grow
the queue without bound. `scene_physics::report_missing_joints` drains it every tick and reports
each carrier through the same `diagnostics::report` latch, as `L3035`.

## Verify

```
cargo clippy -p ludion-physics --all-targets --all-features -- -D warnings
cargo nextest run -p ludion-physics
```
