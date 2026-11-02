---
description: Colliders, the spatial index rebuild you schedule, the four query calls, and move-and-slide
---

# Collision and spatial queries

**Scope:** Authoring `Collider`s and querying them: neighbor lookups, raycasts, and kinematic sliding against static geometry. The 2D tier has no dynamics — what happens on contact is your systems; rigid bodies are `ludion docs physics`.

Collision is deterministic and 2D, and **which** two world axes it runs on is your game's choice:
`Engine::builder().plane(Plane::Xy)` for a game whose content lives in world XY — the plane
`Sprite` and `Tilemap` draw on — or the default `Plane::Xz` for a ground-plane game seen from above
(`ludion docs visuals --section "Which plane is 2D?"`). The plane is authored once on the builder;
there is no flag and no runtime setter. Every 2D query reads the two **in-plane** axes of
`Transform.translation` — the off-plane axis, rotation, and scale are ignored — so a `Collider`
component gives an entity a shape there, and the `spatial` module (both in the prelude) gives you
neighbor queries and a kinematic slide.

**Authoring.** `shape` is `circle` (a `radius`) or `aabb` (2-vector `half_extents`, the two
in-plane axes — there is no off-plane extent); `body` is `static` (walls, props) or `kinematic`
(movers; a scene written as `dynamic` before the rename still loads — the canonical writer always
emits `kinematic`). Every size must be a strictly positive, finite number as an `f32` (`L1116`).

```yaml
  - name: player
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      Collider:
        shape:
          circle:
            radius: 0.5
        body: kinematic
  - name: wall
    components:
      Collider:
        shape:
          aabb:
            half_extents:
              - 4.0
              - 0.5
        body: static
```

**3D shapes and rigid bodies (`ludion-physics`).** `Collider`'s `shape` also carries eight 3D
variants — `sphere`, `box` (3-vector `half_extents`), `capsule` and `cylinder` (`half_height`,
`radius`; both with the axis along local Y), `convex` (hand-authored
`points`; the voxel ragdoll switch writes its own; no hull is generated from a mesh or model),
`heightfield`, `compound` and `mesh` (below) — and `body` also carries `rigid` (solver
tunables: `mass`, `friction`, `restitution`, `friction_combine`, `restitution_combine`, `linear_damping`, `angular_damping`,
`gravity_scale`, `lock`, `sleep_after_ticks` — which of them a live edit applies in place: `ludion docs physics`). This crate's own index and queries stay 2D-only: `spatial::rebuild` skips
any collider whose shape is 3D or whose body is `rigid`, so a physics scene indexes nothing for
those entities rather than indexing a lie. `ludion-physics` owns the dynamics half — `ludion docs physics`. A `sensor: true` collider is a trigger volume — `ludion docs physics-sensors`.

**Contact material.** `static` and `kinematic` take an optional map with the same four fields: `body: { static: { restitution: 0.3, friction_combine: min } }`. Defaults are friction `0.5`, restitution `0`, both rules `average`; the bare `static` / `kinematic` stays the written form at the defaults. Rules are `average`, `min`, `multiply`, `max`. A contact's value comes from both colliders; when their rules differ the stronger wins, `max` over `multiply` over `min` over `average`. A bouncy part over a dead ground: `restitution: 0.8, restitution_combine: max` on the part, the ground untouched. A live edit applies in place (`ludion docs physics`).

**Compound.** `compound: { parts: [{ shape, translation, rotation }] }` is one body and one collider
from several shapes — a mug is a cylinder plus an offset, turned box handle:

```yaml
shape:
  compound:
    parts:
      - shape:
          cylinder:
            half_height: 0.5
            radius: 0.4
      - shape:
          box:
            half_extents:
              - 0.25
              - 0.2
              - 0.06
        translation:
          - 0.55
          - 0.0
          - 0.0
        rotation:  # Euler XYZ degrees, as Transform
          - 0.0
          - 0.0
          - 15.0
```

`translation` and `rotation` default to identity. A part is a `sphere`, `box`, `capsule`, `cylinder`
or `convex`. The body has **one `mass`**, spread over the parts by volume; the centre of mass is
geometric and may sit off the entity origin. Contacts, joints, layers, blasts and `gravity_scale`
all see the one body. `L1189` is an empty `parts`, a nested `compound`, a `heightfield`, `circle` or
`aabb` or `mesh` part, or a non-finite offset (pointers under `/shape/compound/parts/<i>`); part sizes stay
`L1116` and a degenerate `convex` part `L1181`. A compound is **not a query shape**: `shape_cast` and
`overlap` return a `QueryInput` error (a `cylinder` works). The 2D tier ignores both new shapes.

**Mesh.** `mesh: { path: meshes/floor.mesh.yaml }` builds a triangle mesh from a `kind: mesh`
document for level geometry, with the solver's internal-edge fix so a capsule slides across seams
without catching. `path` is content-root relative, the same as `Mesh.shape: { asset }`. It is
`static` or `kinematic` only: on a `rigid` body it is `L1211` (pointer `/body`; use `convex` or
`compound`, no hull is generated). A blank `path`, a missing or invalid document, or a mesh the
solver refuses is `L1212`. By convention the collision mesh is a simplified copy exported from the
level's bpy script; the render mesh works for a greybox. A mesh is not a query shape and not a
compound part. The document is read when a scene loads, a snapshot restores or the scene switches,
not by `asset.reload` (a collider changing mid-run would break replays): edit the file, then
reload the scene or restore.

**The index is rebuilt by a system you schedule.** Nothing is hidden: register
`spatial::rebuild::<Transform, TransformHierarchy>` at an explicit position — before any system that queries — and the
schedule stays readable in your `game()`:

```rust,ignore
fn game() -> EngineBuilder {
    Engine::builder()
        .system("spatial_rebuild", spatial::rebuild::<Transform, TransformHierarchy>)
        .system("enemy_ai", enemy_ai)        // queries see this frame's positions
        .system("move_player", move_player)
}
```

**The second type parameter is the parent resolver.** A `Transform` game passes `TransformHierarchy`
(an entity under a `Parent` is indexed and swept at its composed world position); a custom pose
component with no hierarchy passes `NoParent`. The generic calls take `::<P, R>`: `spatial::rebuild`,
`spatial::move_and_slide`, physics `install`, `move_character` and `move_character_with` (old form
`::<P>`). `probe_character_ground`, `resize_character`, `fits_character` and `teleport_character`
take `::<P>` only.

```rust,ignore
.system("spatial_rebuild", spatial::rebuild::<Transform, TransformHierarchy>) // was ::<Transform>
```

Before the first rebuild (or in a world with no colliders) every query returns empty/`None` and
`move_and_slide` returns its delta unchanged — legitimate states, not errors. A world that holds a
2D collider but never schedules the rebuild is an error, reported once as `L1262`.

**The four calls.** All are pure reads over `&World`; the first three return `EntityId`s (which raw
`world.query` cannot), in a **pinned, reproducible order**:

- `spatial::overlaps(world, shape, center, body_filter) -> Vec<EntityId>` — everything overlapping
  a probe shape (projectile hit-tests). Order: grid cell, then ascending `EntityId`.
- `spatial::overlaps_any(world, shape, center, body_filter) -> bool` — whether `overlaps` would be
  non-empty, without allocating (a per-cell clearance probe).
- `spatial::k_nearest(world, center, k, max_radius, body_filter) -> Vec<EntityId>` — the k closest
  collider centers within `max_radius`, ordered by (distance, `EntityId`). Targeting, pickups,
  separation.
- `spatial::raycast(world, origin, dir, body_filter) -> Option<RayHit>` — the nearest in-plane
  hit (`RayHit { entity, t, point }`, hit point = `origin + t * dir`). Line of sight,
  projectile-vs-wall.
- `spatial::move_and_slide::<Transform, TransformHierarchy>(world, entity, delta) -> [f32; 2]` (3D characters with a physics world: `ludion docs motor`) — the resolved delta for
  a kinematic move swept against **static** colliders only (never kinematic or rigid bodies — mover-vs-mover
  response is your logic, built on `k_nearest`). Axis-separated: sliding along a wall keeps the
  tangential component and cancels the normal one; a corner resolves to zero. Exact contact (a
  mover tangent to a wall) is not blocked — only penetration is; `overlaps` stays inclusive. Pure read — *you*
  apply the result. **It never depenetrates**: a mover that already overlaps a static is stuck —
  every candidate destination still overlaps, so every move resolves to zero. Never spawn, teleport,
  or author an entity inside a static footprint; validate candidate positions with
  `spatial::overlaps` first and re-draw blocked ones. For a mover under a `Parent` the resolved
  delta is in **world** plane coordinates, but `Transform` is local: convert the delta into the
  parent's frame before adding it, or keep movers as roots.

Every `body_filter` is an `Option<Body>`: `None` for everything, `Some(Body::new_static())` /
`Some(Body::new_kinematic())` to filter; the filter matches the kind and ignores the contact material.

**Copyable example — slide the player, separate the flock:**

```rust,ignore
use ludion::prelude::*;

/// Move the player with a wall-slide: compute the wish, resolve it against statics, apply it.
fn move_player(world: &mut World, ctx: &SystemCtx) {
    let input = world.resource::<InputState>().unwrap();
    let mut wish = [0.0f32, 0.0];
    if input.is_held(Key::Right) { wish[0] += 6.0 * ctx.dt; }
    if input.is_held(Key::Left)  { wish[0] -= 6.0 * ctx.dt; }
    if input.is_held(Key::Up)    { wish[1] -= 6.0 * ctx.dt; }
    if input.is_held(Key::Down)  { wish[1] += 6.0 * ctx.dt; }

    let Some((player, _)) = world.query_ids::<(&Player,)>().next() else { return };
    let resolved = spatial::move_and_slide::<Transform, TransformHierarchy>(world, player, wish);
    let transform = world.component_mut::<Transform>(player).unwrap();
    transform.translation[0] += resolved[0]; // the only hashed write, in YOUR code
    transform.translation[2] += resolved[1];
}

/// Keep enemies apart: each backs away from its nearest few kinematic neighbors.
fn separate_enemies(world: &mut World, ctx: &SystemCtx) {
    let enemies: Vec<EntityId> = world.query_ids::<(&Enemy,)>().map(|(e, _)| e).collect();
    // Every push is read from the index's frozen centers first; the writes come after, so one
    // enemy's move never changes what the next one sees.
    let mut pushes = Vec::new();
    for enemy in enemies {
        let here = world.component::<Transform>(enemy).unwrap().translation;
        let center = [here[0], here[2]];
        let mut push = [0.0f32, 0.0];
        for entry in spatial::k_nearest_entries(world, center, 4, 1.5, Some(Body::new_kinematic())) {
            if entry.entity == enemy { continue; } // the asker is in its own results
            push[0] += center[0] - entry.center[0];
            push[1] += center[1] - entry.center[1];
        }
        pushes.push((enemy, push));
    }
    for (enemy, push) in pushes {
        let transform = world.component_mut::<Transform>(enemy).unwrap();
        transform.translation[0] += push[0] * 2.0 * ctx.dt;
        transform.translation[2] += push[1] * 2.0 * ctx.dt;
    }
}
```

**Colliders are invisible.** Nothing draws them, and adding or removing a `Collider` changes no
pixel (that render-inertness is itself asserted in the engine's test suite). Do not verify
collision by looking at the screen — assert on positions and `WorldHash` in headless tests, the
same way you verify every other sim behavior; `examples/gallery/colliders.yaml` shows a *layout*
(meshes sized to mirror their colliders), not behavior. If you want a body's shape visible, give
the entity a `Mesh` whose x/z size mirrors the collider, as that scene does.

Queries read the index **snapshot** from the last `rebuild`: if a system moves an entity after the
rebuild ran, queries still see its old position until the next frame's rebuild. Schedule
accordingly (rebuild first is the common case). A snapshot restore or replay seek carries the index,
so a reader scheduled before the rebuild continues exactly.

## Next

- `ludion docs physics` — rigid bodies: the `physics:` block, forces, contact events, solver queries.
- `ludion docs collision-layers` — which pairs never meet, and query filters.
- `ludion docs physics-joints` — the `Joint` component.
- `ludion docs systems` — the system contract and the determinism rules.
- `ludion docs tilemaps` — the `Tilemap` grid component.
- `ludion docs recipes` — copyable patterns.
- `ludion docs verify-loop` — the `cargo test` verification loop.
