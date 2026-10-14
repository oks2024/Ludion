---
description: Deterministic rigid-body dynamics — the rapier3d wrapper, its pin and the resolved-feature gate
---

# ludion-physics

**Scope:** The `ludion-physics` crate: the boundary rule, the world resource and the one verb that
installs its systems, and the mapping from the authored `Collider`/`Velocity` surface onto rapier.
Defers to `ludion devdocs physics-events-queries` (events, queries, shape rejection) and
`ludion devdocs physics-pin` (the feature set and its gate). See `Plan/physics.md` for the decision
to adopt rapier.

## Boundary rule

**`ludion-physics` is the only crate in the workspace that names a rapier or parry type.**
`Collider` and `Velocity` stay pure data in `ludion-spatial`; every other crate reaches physics only
through this crate's public surface, **the whole physics API a game needs**. It does **not** depend
on `ludion-engine`, so it cannot name the engine's `Transform`: `PhysicsPose` is the seam.

## The world resource

`PhysicsWorld` is a world *resource*, so it is invisible to `WorldHash`; what reaches the hash is
the pose and `Velocity` the mirror writes back into components, and a solver divergence therefore
shows up in the hash trace one tick later. It holds rapier's sets, the `IntegrationParameters`, the
gravity vector, and the **creation-order handle table** — a `Vec`, and the only thing this crate
iterates; a rapier set is never walked, and the table is never rebuilt from one, it rides in the
snapshot instead. The entity→row map beside it is a lookup accelerator, iterated by nothing.

`PhysicsWorld::new(TickRate)` sets `dt` and leaves every other solver tunable at the pin's default.
Gravity defaults to `[0, -9.81, 0]`; a top-down `Plane::Xz` game zeroes it through
`PhysicsSettings::gravity`, which `apply_settings` writes into the solver every tick — the only way.

## The install verb

`install::<Transform>(&mut schedule, settings)` (`settings: impl Into<PhysicsSettings>` — a bare
`PhysicsSpace` still converts) appends `physics_write`, `physics_step`, `physics_mirror` in that
order at the position it is called. **One verb, not three**: the order is load-bearing and a wrong
order fails silently — mirror before step writes back last tick's pose, write after step throws
this tick's contacts away. The `spatial::rebuild` precedent without its footgun.

`ludion-engine` is the only caller: it maps a loaded scene's `physics:` block onto `PhysicsSettings`
and calls `install` once, always after every game system — the block is the only opt-in there is,
not a builder verb.

`write` reconciles the body set with the claimed colliders (created and retired in ascending entity
order) and pushes the authored state in; `step` advances the
solver one fixed timestep with no hooks and no event handler; `mirror` walks the table in creation
order and writes each rigid body's pose and velocity back.

## Which colliders physics claims

`claims(&Collider)` is the rule, and it is the **exact complement** of what `spatial::rebuild`
skips: a 3D shape, or a `rigid` body of any shape. Two tiers, not two ways — `spatial::*` answers
"what is near this point on the plane this frame" with no bodies and no solver, `physics::*`
simulates, and an entity is in one tier or the other and never both, complementary rather than
merely disjoint, so a collider cannot fall through the gap either.

A `circle` or `aabb` reaches the solver **only when its body is `rigid`** — the plane-locked
2D-physics case — so a *static* `aabb` wall or a *kinematic* `circle` mover is `ludion-spatial`'s
and never becomes a solver body; a physics scene's level geometry is therefore authored with 3D
shapes (`box`, `heightfield`, `convex`) rather than `aabb`. This keeps a twin-stick game's thousands
of in-plane colliders out of the solver.

**Nothing is allocated until something is claimed.** `write` returns early when it claims no entity
and no world exists yet, so the `PhysicsWorld` resource appears on the first tick that claims one
and `step`/`mirror` are no-ops before then: a 2D game with `install` pays one query per tick.

**One owner per phase.** Game code writes `Velocity`, the pose of kinematic bodies, and the pose of rigid
bodies as a teleport (`push`: `set_position(_, true)`, no rebuild, no `wake_touching`); the write
system pushes that in; the step runs; the mirror pulls back. Impulses (`impulse.rs`) are the
exception: `apply_impulse`, `apply_impulse_at_point` and `radial_impulse` queue a call on the
`PhysicsWorld`, and `write` drains the queue at its end, after every body is created, rebuilt and
pushed, so a `Velocity` the game pushed the same tick never overwrites one. Rapier applies them, so
spin comes from its inertia. A blast skips colliders inserted since the last step
(`fresh_colliders`): rapier resolves a stale BVH leaf by slot alone, so a slot reused this tick
would be hit otherwise; a frozen step keeps the list across the tick boundary, so it rides the
snapshot too (the blob's last-but-one field) and a body rebuilt during a hit-stop stays blast-immune
until the next real step, restored or not. `blast_hits` resets every tick and is not hashed; it rides
the snapshot too (adding these fields changed the format, so the `physics-stress` and `ragdoll`
replays' keyframes are re-recorded, their per-frame hashes unchanged). `to_bytes` refuses with an `Encode`
error while calls are still queued: the queue is not in the blob, so encoding one would drop it. A
game edit is detected by comparing a component against **the exact bits the component read back
after the mirror last wrote it** (a lossy pose type, `Transform`'s Euler degrees, reports different
bits than the solver held) — never a prediction — so a difference is an edit by definition and an
untouched component pushes nothing (how a resting body sleeps); a write that differs also **wakes**
a sleeping body. A rigid body without a `Velocity` component simply does not expose its velocity;
the mirror never inserts one.

A live edit to `Collider` itself — shape, body kind, mass, lock or layer — is symmetric: `write`
compares it against the exact `Collider` the body was last built from and, on a difference,
**rebuilds** (an edit of only the contact material, damping, `gravity_scale` or `sleep_after_ticks`
is applied in place by `retune_in_place` instead), at the entity's current pose and velocity rather
than the authored one. Every body still touching it wakes first, so a rebuild never leaves something
resting on a shape that just shrank or vanished; contacts are keyed by entity pair, so a pair still
touching after the rebuild emits nothing. An untouched `Collider` costs one comparison.

## The pose trait and the Euler convention

`PhysicsPose` has four required methods: `translation`/`set_translation` and
`rotation_quat`/`set_rotation_quat`, the rotation a quaternion `[x, y, z, w]`. A fifth,
`derived_world_pose`, defaults to `None` (the stored pose is world space); the engine's `Transform`
impl returns the composed pose for a parented entity, and the write pass feeds it to kinematic bodies
in place of the stored one. A rigid or static body under a parent gets no body (`L1221`/`L1222`), and
`L1223` is raised once per entity for a kinematic body under a sheared chain. A quaternion at the
boundary keeps every Euler question in one place, the implementor, not on each side of the mirror.
The engine's convention is **Euler XYZ degrees, intrinsic** (the product `qx · qy · qz`).
`ludion_core::math::euler_xyz_degrees_to_quat` and `quat_to_euler_xyz_degrees` are that convention
with the transcendentals on pure-Rust libm, never glam's own, whose platform C library differs in
the last bit across OS and arch; render (`extract`) builds every model matrix from the same pair.

## Shapes and layers

The shape table, `aabb`/heightfield layout, mesh colliders and `CollisionLayers`: `ludion devdocs physics-shapes`.

## Tunables

| authored field | where it lands |
|---|---|
| `TickRate` | `IntegrationParameters::dt`, set once at world build |
| `physics.gravity` | `rapier.gravity`, via `apply_settings` |
| `physics.solver_iterations` | `IntegrationParameters::num_solver_iterations`, via `apply_settings` |
| `physics.stabilization_iterations` | `IntegrationParameters::num_internal_stabilization_iterations`, via `apply_settings` |
| `RigidBody::mass` | `ColliderBuilder::mass` (overriding density) |
| `RigidBody::friction` / `restitution` / `*_combine`; `ContactMaterial` on static and kinematic | the collider's material and `CoefficientCombineRule` |
| `RigidBody::linear_damping` / `angular_damping` | `RigidBodyBuilder::linear_damping` / `angular_damping` |
| `RigidBody::gravity_scale` | `RigidBody::set_gravity_scale` (builder `RigidBodyBuilder::gravity_scale`) |
| `RigidBody::lock` | `LockedAxes`, OR'd with the plane's under `PhysicsSpace::Planar` |
| `RigidBody::sleep_after_ticks` (else `physics.sleep_after_ticks`, else 30) | `RigidBodyActivation::time_until_sleep` |
| `physics.layers` | collider `InteractionGroups`, via `apply_settings` when the table changes |
| `Velocity` | `set_linvel` / `set_angvel`, and `linvel`/`angvel` at creation |

The gravity and iteration rows are re-applied every tick (`apply_settings`, called from `write`
before any body is created or pushed); every other row is creation-time only, except that a
`Collider` edit of the contact material (any body kind), damping, `gravity_scale` or `sleep_after_ticks` alone is
applied in place by `retune_in_place` in `write` (exact match on every other field, else the edit
rebuilds). The setters do not clear the rest timer, so the body would fall straight back asleep;
`retune_in_place` also calls `wake_up(true)`.

**Sleeping is authored in ticks.** The solver's threshold is in seconds, so `sleep_after_ticks` is
converted once at world build to `(ticks - 0.5) * dt` — the half-tick offset because the solver
accumulates `dt` per step and sleeps at `>=`, and repeated addition of `dt` is not bit-equal to
`n * dt`, so a threshold at exactly `ticks * dt` could land on either side by an ulp.

## Space

`PhysicsSpace` is `PhysicsSettings::space`, part of `install`'s second argument, and it is *stated*
rather than inferred. `Free` takes no locks from the world's `Plane`; `Planar` locks the off-plane
translation axis and the two in-plane rotation axes on every rigid body, leaving rotation about the
plane normal free — the one rotational degree of freedom a 2D game has. A body's own
`RigidBody::lock` is OR'd in under both.

It is an argument, not a reading of the world: an engine always installs a `Plane` (default `Xz`),
whose locks freeze `y`, the axis gravity points down. `Free` opts a 3D game out of it.

## Events, queries and shape rejection

See `ludion devdocs physics-events-queries` for the contact-event diff, the three read-only queries
and the shape-rejection retry path.

## Time scale and snapshot

`scale_time`, `freeze` and the bincode snapshot: `ludion devdocs physics-time-snapshot`.


## The pin and its feature gate

`rapier3d` is pinned with `=` in `[workspace.dependencies]` (`=0.35.3` today, vendored and patched under `vendor/rapier3d/`, with parry3d vendored beside it under `vendor/parry3d/`) — a pin move is a
hash-moving event the pin-upgrade ritual (`Plan/physics.md` W4) gates. See `ludion devdocs
physics-pin` for the feature set, why each feature is on, and `scripts/physics-features.mjs`, the
gate that verifies it.

## Verify

```
cargo clippy -p ludion-physics --all-targets --all-features -- -D warnings
cargo nextest run -p ludion-physics
node scripts/physics-features.mjs
LUDION_BLESS=1 cargo test -p ludion-physics --test pile    # re-record the reference trace
```

`crates/ludion-physics/tests/traces/pile.txt` is the committed per-tick digest of a 100-box pile; it
must be *varied* (`the_trace_is_varied`, since a sleeping pile repeats one line), and the resume
gate replays it from mid-flight snapshot bytes and requires a line-for-line match.