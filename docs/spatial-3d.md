---
description: SpatialVolume and the 3D broadphase — layered sphere/box queries, swept-sphere cast, k-nearest
---

# Spatial 3D: a deterministic broadphase for hitscan and area queries

**Scope:** the `spatial3d` module of `ludion-spatial`: `SpatialVolume`, `L1339`, scheduling
`rebuild`, `Index3d` and its four queries. It sits beside the 2D `Collider` grid
(`ludion docs collision`); it is not the physics world (`ludion docs physics`).

## What it is

A bounding-volume hierarchy over caller-fed items: an opaque `Handle`, a sphere or AABB, and a `u32`
layer mask. `rebuild` refills it every tick from the world; it is a derived resource outside
`WorldHash`, and results depend only on the item set, never on push order or thread count.

## Authoring `SpatialVolume`

| Field | Default | Meaning |
| --- | --- | --- |
| `shape` | `sphere: { radius: 0.5 }` | `sphere: { radius }` or `aabb: { half_extents: [x, y, z] }`, centred on the world translation |
| `layers` | `1` | raw bitmask a query filter matches (unrelated to `physics.layers`) |

A radius or half extent that is not positive and finite, or `layers: 0`, is `L1339` with a pointer
(`/shape/sphere/radius`, `/layers`). `{}` is a 0.5 sphere on bit 0.

## Scheduling

```rust
Engine::builder()
    .system("spatial3d_rebuild", spatial3d::rebuild::<Transform, TransformHierarchy>)
    .system("shoot", shoot) // after the rebuild
```

**Readers must run after `rebuild`.** A reader before it sees last tick's index, and a snapshot does
not carry the index: after a restore it is empty until the rebuild runs, so a reader scheduled before
it sees nothing on that first tick. Under a `Parent` the centre is the composed world translation.
Rotation and scale are ignored.

## Queries

All take a `Filter` and clear the output buffer you pass; none allocates once the buffer has grown.

- `overlap_sphere` / `overlap_aabb` → `Handle`s, sorted by handle; touching counts.
- `k_nearest(point, k, max_radius, ..)` → `Neighbor { handle, distance_sq, center }`, ordered by
  `(distance_sq, handle)`, measured to each item's centre, `max_radius` inclusive.
- `cast(origin, delta, radius, ..)` → the first `CastHit`, minimum by `(t, handle)`; radius 0 is a segment.

A non-finite probe or a negative radius finds nothing. Queries are `&self`, so they fan out over
`ludion_core::par`; an indexed query is a pure function of the index.

## `Filter` and `Handle`

`Filter::new(layers)` sees items with `item.layers & layers != 0`; `.excluding(handle)` hides one
item (the shooter) and `.owned_by(handle)` a second (its owner), so a projectile ignores both.

`Handle::entity(id)` for an entity, `Handle::slot(index, generation)` for a non-entity item such as a
pooled projectile. `spatial3d::push_entity_volumes::<P, R>` pushes every entity volume without
building; add your own `index.push(Item { .. })` calls, then `index.build()`. Handles must be unique
within a build.

## The cast's contract

`t` is the fraction of `delta` travelled, in `[0, 1]`. A probe already overlapping an item hits at
`t = 0`. `point` is the item's surface point nearest the probe centre at `t`; `normal` is a unit
vector from the item toward the probe. A box face gives exactly an axis (`[-1, 0, 0]`, never
`[-0.99999, ..]`); an edge or corner gives the unit direction from that edge or corner to the probe
centre. A shooter that carries a volume starts inside it: exclude it, or every cast is `t = 0`.

## Limits

- Bounds are axis-aligned: spheres and AABBs only, rotation and scale ignored.
- The 2D `raycast` does not route here, and the 3D index never sees `Collider`.
- Cost is dominated by casts: with 5,000 projectiles in the index, a separate index of just the
  targets (enemies and walls) cuts the serial cast time by about a third, since projectile items
  share nodes the layer mask cannot prune.

## Example

From `crates/ludion/examples/spatial3d.rs` (`cargo run -p ludion --example spatial3d`):

```rust
let filter = Filter::new(ACTORS).excluding(Handle::entity(shooter));
let hit = index.cast(origin, [20.0, 0.0, 0.0], 0.25, &filter);
let mut near: Vec<Neighbor> = Vec::new();
index.k_nearest(origin, 2, 50.0, &filter, &mut near);
```

## Next

- `ludion docs collision` — the 2D `Collider` grid and its queries.
- `ludion docs systems` — the system contract and schedule order.
- `ludion docs projectiles` — pooled 3D shots swept against physics and the `spatial3d` index.
