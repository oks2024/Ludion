---
description: Rigid bodies — the scene physics block, RigidBody, forces, contact events, solver queries
---

# Rigid-body physics (`ludion-physics`)

**Scope:** Authoring 3D shapes and rigid bodies, the scene `physics:` block, forces, contact events and solver queries. The 2D tier (colliders, spatial index, `move_and_slide`) is `ludion docs collision`; which pairs never meet is `ludion docs collision-layers`; a jointed pair is `ludion docs physics-joints`; slowing or freezing the step is `ludion docs physics-time-scale`; trigger volumes are `ludion docs physics-sensors`; a character capsule is `ludion docs motor`.

A 3D shape (`sphere`, `box`, `capsule`, `cylinder`, `convex`, `heightfield`, `compound`) or `body: rigid` of any shape belongs to `ludion-physics`;
everything else stays in the 2D index (`ludion docs collision`); an entity is claimed by exactly one tier.

**The scene's `physics:` block is the only way to opt in**, and every host that builds a world from
the scene — `serve`, `render`, `sim-hash`, `replay` — installs the rigid-body write/step/mirror
triple when it is present:

```yaml
physics:
  space: free              # required: `free` (full 3D) or `planar` (the engine's Plane locks it)
  gravity:  # optional, default one standard gravity down y
    - 0.0
    - -9.81
    - 0.0
  solver_iterations: 4         # optional, 1..64, default 4
  stabilization_iterations: 1  # optional, 1..64, default 1
  sleep_after_ticks: 30        # optional, scene-wide fallback for a body that omits its own (simulated ticks)
```

`space: planar` locks the off-plane translation axis and the two in-plane rotation axes, OR'd with
the body's own `lock`. Absent, nothing about the scene's dynamics changes and no hash moves.

A solver collider (a 3D shape or `body: rigid`) in a world with no `physics:` block is never
simulated: `ludion check-scene` reports it as `L1261` at the collider, and one spawned by game code
is reported once at runtime.

**A rigid body**, minimally:

```yaml
  - name: crate
    components:
      Transform:
        translation:
          - 0.0
          - 3.0
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 0.5
              - 0.5
        body:
          rigid:
            mass: 2.0
```

Every `RigidBody` field but `mass` defaults (`body: { rigid: {} }` is a valid 1 kg box). A rigid or static solver body cannot sit under a `Parent` (`L1221`/`L1222`); a kinematic one can, and is posed from its world pose (`ludion docs hierarchy`).
Level geometry is authored with 3D shapes (`box`, `heightfield`, `convex`, `mesh`), not `aabb`: an `aabb`
only reaches the solver when its body is `rigid`. Collision ignores `Transform.scale` (own or inherited): a scaled solver collider raises `L1290`; bake the scale into the shape.

**Live edits.** A live `Collider` edit reaches the solver by one of two routes:

| field | edit |
|---|---|
| `friction`, `restitution`, `*_combine` (any body kind), `linear_damping`, `angular_damping`, `gravity_scale`, `sleep_after_ticks` | in place |
| shape, body kind, `mass`, `lock`, `layer` | rebuild |

In place wakes the body (a static or kinematic edit wakes what touches it); its handle, `Joint` zeros (`ludion docs physics-joints`)
and contacts survive, and the new values govern the next step. A rebuild replaces the body at its current pose
and velocity (the solver's, with no `Velocity` component): sleep restarts, `Joint` zeros are re-captured, contacts continue.
Any difference outside the in-place set rebuilds. `PhysicsWorld::gravity_scale`, `friction` and
`restitution` read the live values back.

**Gravity scale.** `RigidBody.gravity_scale` multiplies the scene's gravity for one body: `1.0` default
(the writer omits it), `0.0` floats, negative rises; a value beyond `f32` is `L1116` at `/body/rigid/gravity_scale`.
To levitate a zone: `overlap`, edit each rigid hit's `Collider` to `0.0`, count ticks per entity
in hashed state, edit it back to `1.0` (the doctest on `PhysicsWorld::gravity_scale`).

**Scale.** Hundreds of rigid props are comfortable; 512 is the gated scale (`physics-stress`), a few
thousand the wall (single-threaded solver).

**Forces and impulses.** There is no force accumulator. To set a velocity, write `Velocity`
directly (a write also wakes a sleeping body). To push a body, queue an impulse (`N·s`, `mass * Δvelocity`)
on the `PhysicsWorld` resource — no `Velocity` component needed, and the body's own inertia decides the spin:

- `apply_impulse(entity, impulse)` — at the centre of mass, no spin.
- `apply_impulse_at_point(entity, impulse, point)` — at a world point; the offset from the centre of mass spins it.
- `radial_impulse(Blast) -> Result<u32, _>` — every rigid body whose collider touches the ball `radius`
  around `center` is pushed away from it; returns the blast's index among this tick's blasts.

A queued call takes effect on the tick it was queued in: the write system applies the queue just before
the step, in call order, so queue from a system that runs before it (under a `physics:` block every game
system does). It wakes a sleeper and moves `rigid` bodies only — `static` and `kinematic` bodies are
skipped, and an entity with no body, or one of those, is a dropped call reported once per offender (a prefab part, a named entity, or an unnamed entity's slot) as
`L3038`. A non-finite input, a `radius` or `min_distance` that is not above zero, or an ignored layer the
scene does not declare is `Err(PhysicsError::QueryInput)` and nothing is queued.

```rust
fn keg(world: &mut World, ctx: &SystemCtx) {
    let Some(physics) = world.resource_mut::<PhysicsWorld>() else { return };
    if ctx.frame.as_u64() == 60 {
        let blast = Blast {
            center: [0.0, 0.2, 0.0],
            radius: 4.0,
            impulse: 12.0,
            falloff: Falloff::Linear,
            filter: QueryFilter::new(),
        };
        physics.radial_impulse(blast).expect("a valid blast queues");
    }
    for hit in physics.blast_hits() {
        // The previous tick's blast: `hit.entity` was pushed by `hit.impulse` at `hit.point`.
    }
}
```

A blast tests colliders against the last completed step, like the queries, so a body created or rebuilt
this tick (spawned, or its `Collider` edited) is not hit until the next real step — under a `freeze()`
that is the whole freeze, a multi-tick hit-stop included. Each hit body is pushed along
the line from `center` to its collider's closest point, applied there, so an off-centre hit spins; with
the centre inside the collider the direction is centre → its centre of mass, and a centre exactly on it
pushes nothing and reports nothing. Hit bodies are visited in `EntityId` order. `d` is the distance from
`center` to that closest point (`0` inside):

| `falloff` | share of `impulse` |
|---|---|
| `Constant` | `1` |
| `Linear` | `1 - d / radius` |
| `InverseSquare { min_distance }` | `(min_distance / max(d, min_distance))²` |

`InverseSquare` has no hard edge: it fades but never reaches zero, and only the `radius` cut ends it — a
body at the radius still gets a share. `blast_hits() -> &[BlastHit]` (`blast`, `entity`, `impulse`, `point`)
holds exactly one tick's hits, grouped by blast in call order: a system reading it sees the previous tick's. Not hashed; rides the snapshot. A snapshot refuses to
encode while calls are still queued, which a system that runs before the write system never leaves behind.
No occlusion is offered: a wall between the centre and a body does not shield it.

**Teleports.** Writing a rigid body's pose (its `Transform`) teleports it: the solver body keeps its handle,
`Velocity` (unless you also write it) and `Joint` zeros, and wakes with its sleep countdown restarted, as does
anything resting on it. The solver sees the new pose at this tick's step, so queries and blasts made before
it still read the old one: do not blast a body on the tick you teleport it. Contacts follow the step: `Stopped`
out of contact, `Started` into it. A jointed figure is moved by writing every body the same tick by the same
rigid transform; a lone moved body is pulled back by its joint. A `static` body follows its pose too: moving it re-poses the solver body and wakes what rests on it. A kinematic body's pose write is a velocity (the pose delta over one tick): snapping a moving platform back launches whatever touches it. `teleport_kinematic(world, entity)` makes the next write a teleport instead: the body is placed at its pose with zero velocity and riders are left behind (a path follower's wrap or jump does this itself).

**Contact events.** `contact_events() -> &[ContactEvent]` names which collider pairs started or
stopped touching on the last step: the pair (`a < b` by `EntityId`), the phase, and — on a start
only — the pair's summed normal-impulse magnitude over its first base tick of simulated time (the touch step at full speed; see `physics-time-scale`), the same at any `solver_iterations` (`0.0` on a stop), and where it touched:
`point`, the world-space midpoint between the two surfaces, and `normal`, the unit world-space normal
pointing from `a` towards `b` (named like a `QueryHit`'s), both at the poses before the step moved the bodies. Both come from one solver contact of the pair, picked by
normal impulse descending, then depth descending, then the smaller point, then the smaller normal — never the solver's own order — so a
resting start with no impulse yet gets a deterministic point too. A pair starts the step it touches: one inside the prediction margin with a gap still open has not started. Both are
`[0.0; 3]` on a stop. Sorted `(a, b)` ascending, one step's events: a system reading it this tick sees tick `N - 1`'s (the triple runs after every game system). Not hashed; rides the snapshot. A spark at every touch:

```rust
fn sparks(world: &mut World, _ctx: &SystemCtx) {
    let Some(physics) = world.resource::<PhysicsWorld>() else { return };
    let starts: Vec<_> = physics
        .contact_events()
        .iter()
        .filter(|event| event.phase == ContactPhase::Started)
        .map(|event| (event.point, event.normal))
        .collect();
    for (point, normal) in starts {
        let spark = world.spawn();
        let pose = Transform { translation: point, rotation: [0.0, 0.0, 0.0], scale: [1.0, 1.0, 1.0] };
        world.insert(spark, pose).unwrap();
        world.insert(spark, Spark { facing: normal }).unwrap();
    }
}
```

**Events name ids that may be gone.** A contact, sensor (`ExitCause::Despawned`, `ludion docs physics-sensors`) or joint-break event is read a tick after its step, so it can name an entity a system despawned since: check it is alive before reading its components.

**Time scale.** `scale_time`, `freeze` and `last_step_dt` slow or stop the step alone (a hit-stop, slow motion); an impact's impulse survives it: `ludion docs physics-time-scale`.

**Queries.** `ray_cast`, `shape_cast` and `overlap` are pure reads against the solver, reading the
state as of the **last completed step** — an entity created, moved, teleported or despawned this
tick is visible only after the step that follows. The two casts return every hit sorted
`(distance, entity)` ascending; `overlap` returns every hit entity sorted ascending. Each takes a `&QueryFilter` (your own body, ignored
layers — `ludion docs collision-layers`; sensors are skipped unless the filter calls `include_sensors()`) and
reports errors rather than an empty result: a non-finite or negative input, a `rotation` that is not
a unit quaternion, or a query shape the solver cannot build, is
`Err(PhysicsError::QueryInput | QueryShape)`. Filtering by body kind (`static`/`kinematic`/`rigid`)
is the caller's job. A straight-down `ray_cast` through a `heightfield` vertex or cell
edge always hits; a slanted ray lying exactly in a grid plane can miss at a shared edge or vertex.

**First hit.** `first_hit`/`first_hits` sweep a sphere (radius 0 is a ray) along a segment and return
the nearest `SweepHit` by `(t, entity)` with no allocation. `sweep_filter` resolves a `QueryFilter`
once per tick; a `Sweep` can also skip one collider (`excluding`). A probe that starts inside reports
`normal = -direction`. An invalid probe is `None`. The batch form is thread-count invariant.

**Collision layers** — which pairs never meet, and query filtering: `ludion docs collision-layers`.

**Diagnostics a physics author meets:** `L1033` — the `physics:` block itself is malformed (a bad
`space`, a `gravity` triple of the wrong length, an iteration count outside `1..=64`); `L1116` — a
collider size or a rigid body's `mass` is not strictly positive and finite; `L1181` — a `convex`
hull's points are coincident, collinear or coplanar; `L1182` — a `heightfield` grid is ragged or
smaller than 2×2; `L1211` — a `mesh` or `heightfield` collider on a `rigid` body; `L1212` — a `mesh` collider's
document cannot be built (a missing file refuses the scene load or switch; a restore or runtime
spawn gets no body). A shape the solver still rejects gets no body and is retried only once
its `Collider` changes, reported once per offender. Rapier mapping: `ludion devdocs ludion-physics`.

## Next

- `ludion docs collision` — the 2D tier: colliders, the spatial index, `move_and_slide`.
- `ludion docs debug-draw` — the `colliders` category: every body as a wireframe, coloured by kind.
- `ludion docs collision-layers` — which pairs never meet, and query filters.
- `ludion docs physics-joints` — the `Joint` and `JointMotor` components.
- `ludion docs physics-time-scale` — `scale_time`, `freeze`, hit-stop.
