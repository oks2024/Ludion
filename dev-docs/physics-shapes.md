---
description: ludion-physics shape mapping onto parry and the collision-layer groups
---
# physics-shapes

**Scope:** how an authored `Collider` shape becomes a parry shape, and how `physics.layers` becomes interaction groups. Hub: `ludion devdocs ludion-physics`.

## Rules

### Shape mapping

| authored `shape` | parry shape |
|---|---|
| `sphere` | `Ball` |
| `box` | `Cuboid` |
| `capsule` | `Capsule` along world `y` |
| `cylinder` | `Cylinder` along world `y` (`ColliderBuilder::cylinder`) |
| `convex` | `ConvexPolyhedron` over the authored points |
| `heightfield` | `HeightField` |
| `compound` | `Compound` (`ColliderBuilder::compound`, one collider on one body) |
| `mesh` | `TriMesh` (`FIX_INTERNAL_EDGES`, `ColliderBuilder::trimesh_with_flags`) |
| `circle` | `Ball` |
| `aabb` | `Cuboid` (see below) |

`aabb` has only two half-extents: the off-plane one is **the first in-plane half-extent, repeated**,
a pure function of the data. An
in-plane shape in a world with no `Plane` resource is laid out on `Plane::Xz`. A heightfield's
authored grid is row-major and parry's is column-major, **row** advancing along world `z` and
**column** along `x`.

`ShapeKind` (with `Cylinder`, `Compound` and `TriMesh`) is the observable half of this table, so a test asserts the mapping without naming a
parry type. A compound's parts go through `part_shape` → `SharedShape`, at each part's `translation`
and Euler-degree `rotation` via `ludion_core::math::euler_xyz_degrees_to_quat`. **parry panics on a nested composite**, and
game code can insert a `Collider` bypassing the validator, so `part_shape` itself rejects a nested
`compound`, a `heightfield` and a 2D part (`L1189`) at runtime. A shape that cannot be built — a degenerate `convex` hull, a ragged or too-small
`heightfield` — produces **no body**, not retried until the entity's `Collider` changes (or, for a `mesh`, its document hash). This crate reports
no diagnostic for it.

A `mesh` reads its triangles from the `ColliderMeshes` resource (`meshes.rs`; a `BTreeMap` of lazy,
memoized loads, never hashed or snapshotted; the engine installs it at every load, restore and
switch, `asset.reload` never touches it). `BodyEntry.mesh_hash` records the document hash a body was
built from and rides the snapshot; `is_stale` compares it with the library's current one, so an
edited document rebuilds the body on the next tick and an unchanged one does not. The `rejected` map
keys on the hash too. A `mesh` or `heightfield` on `rigid` is `L1211` at `create`; everything else unbuildable is `L1212`, the loader's own `Diagnostic` riding in `ShapeRejection.related`.
### Collision layers

`CollisionLayers` (`systems.rs`) resolves the scene's table: layer `i` is bit `i` (`default` = 0), a
collider's membership is its layer's bit, and its filter is `!ignores_mask(i)` under
`InteractionTestMode::And`, which gives the symmetric "either side ignores" rule. These are
collision groups, not solver groups, so a filtered pair is never detected and emits no events.

- An unlayered world never sets groups, which keeps the blob byte-identical.
- An equal re-apply is a no-op with no wake — what makes a restore resume exactly.
- The table is not in the blob: it is re-derived from the snapshot's `scene_physics`.
- A table edit that removes a live body's layer drops the body with `L1186`.

## Verify

```
cargo test -p ludion-physics
```
