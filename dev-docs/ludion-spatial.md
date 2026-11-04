---
description: Deterministic 2D ground-plane collision — Collider, the spatial hash, pinned-order queries
---

# ludion-spatial

**Scope:** The `ludion-spatial` crate: the `Collider` builtin component, the per-frame
`SpatialIndex` broadphase and its `rebuild` system, the pinned-order queries
(`overlaps`/`overlaps_any`/`raycast`/`k_nearest`), and the kinematic `move_and_slide`. For the determinism rules
this crate enforces (and how) see `determinism`; for its position in the workspace see
`crate-graph`; for authoring `Collider` in a scene and the game-facing walkthrough see the engine
guide's "Collision and spatial queries" chapter. Read this when changing the crate; a game *using*
it needs only the guide chapter.

## Layout

- `crates/ludion-spatial/src/lib.rs` — crate doc, the `GroundPosition` trait (the position-component seam), re-exports.
- `crates/ludion-spatial/src/collider.rs` — `Collider { shape, body }`, `Shape` (`circle`/`aabb`, externally tagged), `Body` (`static`/`kinematic`/`rigid` + the contact material; `is_solver_owned` is a 3D shape or a `rigid` body), the `SemanticValidate` impl (`L1116`).
- `crates/ludion-spatial/src/spatial.rs` — `SpatialIndex` (static/kinematic `HalfIndex`es, each a sparse grid plus an oversize grid, `CELL_SIZE`), `rebuild`, `overlaps`/`overlaps_any`/`raycast`/`k_nearest`, `move_and_slide`, the narrow-phase math.
- `crates/ludion-spatial/src/joint.rs`, `crates/ludion-spatial/src/joint_motor.rs` — the `Joint` builtin component and its `JointKind`; the `JointMotor` builtin component.
- `crates/ludion-spatial/src/velocity.rs`, `crates/ludion-spatial/src/plane.rs` — the `Velocity` builtin component; `Plane`, which two world axes a game's 2D queries live in.
- `crates/ludion-spatial/tests/conformance.rs` — double-run bit-identity, the brute-force O(n²) oracle (set AND order), tie-break cases.
- `crates/ludion-spatial/tests/slide.rs` — `move_and_slide` behavior/edge cases, its oracle, the two-run `WorldHash`-trace test (dev-dep on `ludion-sim`).
- `crates/ludion-spatial/tests/e6_bench.rs` — the ignored release-mode E6 microbench (run explicitly; asserts the recorded 1.5 ms sub-budget).
- `crates/ludion-spatial/clippy.toml` — strict determinism denylist (see `determinism`).

## Rules

- **The pinned query order is a contract, not an implementation detail.** `overlaps` returns
  matches in sorted-cell-key-then-ascending-`EntityId` order; `k_nearest` orders by (squared
  distance, `EntityId`); `raycast` returns the nearest hit with equal-`t` ties broken by
  `EntityId`. The conformance suite asserts order against a grid-free oracle — an optimization
  that changes the order is a breaking change and must re-bless deliberately.
- **Collision is 2D on the ground plane (x, z).** A collider sits at
  `(Transform.translation.x, .z)`; height, rotation, and scale are ignored. `half_extents` is a
  2-vector — there is no vertical extent.
- **This crate never names `Transform`** (that would need a spatial→engine edge, the forbidden
  direction). `rebuild`/`move_and_slide` are generic over `GroundPosition` (the in-plane mapping
  only) and `R: ParentPose`; `ludion-engine` owns `impl GroundPosition for Transform` and
  `TransformHierarchy`. Games call `spatial::rebuild::<Transform, TransformHierarchy>`. A parented
  entity's center is `R::world_pose` projected on `Plane::axes`; `NoParent` reads the stored position.
- **`SpatialIndex` is derived, never hashed.** It is a non-`Serialize` resource outside `WorldHash`,
  rebuilt from world content each frame by the explicitly-scheduled `rebuild` system. Determinism
  comes from rebuild being a pure function of world content visited in `EntityId` order. A snapshot
  carries `filed_entries()` and a restore refiles them (`from_entries`), because a reader scheduled
  before the rebuild sees the index the previous tick filed, not one the restored poses can rebuild.
- **`move_and_slide` resolves against `Static` colliders only, and is a pure read.** Kinematic-vs-
  kinematic response is game logic built on `k_nearest`. The game applies the returned delta to
  `Transform` itself — the only hashed write stays in game code, on the game's visible schedule.
- **No rigid-body dynamics.** No forces, no constraint solving, no depenetration (a mover starting
  inside a static resolves to zero delta). Games write response logic on the queries.
- **Determinism denylist active** (own `clippy.toml`, verbatim from `ludion-sim`): no std hash
  containers, no wall clock. The one sanctioned `Instant::now` is the bench's scoped
  observation-only span.

## How it works

`rebuild<P>(world, ctx)` has the exact `fn(&mut World, &SystemCtx)` system signature; a game
registers it at a chosen schedule position (no hidden systems). It joins `query_ids::<(&Collider, &P)>()`
(ascending `EntityId`) and files one `Entry { entity, center, shape, body }` per collider into the
`SpatialIndex`, skipping every `is_solver_owned` collider (a 3D shape or a `rigid` body is
`ludion-physics`'s, never in the 2D index); once any `Parent` exists, a second pass replaces each
center with the entity's world pose (`R::world_pose`). The index holds two independent
`HalfIndex`es keyed by `Body` (`statics` and `dynamics`, the latter holding the kinematic
entries), each with its own cell grid, `max_reach`, and occupied bounds. Within a half, an entry
is filed under the single grid cell containing its center (`floor(coord /
CELL_SIZE)`, `CELL_SIZE = 16.0` — tuned by the E6 microbench, where coarse cells beat the "cell
near query radius" prior because `StableMap` node traversal costs more than a fat cell's linear
narrow-phase) *unless* its per-axis reach exceeds `OVERSIZE_REACH` (one cell), in which case it is
filed instead in that half's `oversize` grid and excluded from the half's `max_reach` and bounds.

Reach inflation is necessarily global to whatever set a probe walks — one outlier entry's reach
widens every probe over that set. The static/dynamic split means a body-filtered probe is never
widened by the other body's entries; the oversize grid means neither half's `max_reach` is ever
raised by a map-spanning collider (a handful of such entries — e.g. the merged perimeter walls a
generated map emits — are scanned in full instead, never pruned). Together they cap every probe's
inflation at one cell, regardless of what else is in the world.

Queries prune to a candidate cell rectangle walked in the same lexicographic order a full scan
would visit: `overlaps` inflates the probe's AABB by each admitted half's own `max_reach` (entries
are filed by center, so a large shape's cell can be far from its edges); `k_nearest` uses
`center ± |max_radius|` with no inflation (it measures center distance); `raycast` deliberately
stays a full scan over both halves (microseconds at these counts, not on the hot path). The four
streams this produces — each half's pruned grid plus its oversize grid — are merged into the one
pinned total order (sorted cell key, then `EntityId` ascending, the same order a full scan of the
union yields), so the split and the oversize bucket change *which* cells a query visits, never the
result set or its order. Before the first rebuild there is no index: queries return empty/`None`,
and `move_and_slide` returns its delta unchanged — documented states, not errors.

`move_and_slide<P>(world, entity, delta)` is an axis-separated test-and-cancel over endpoint
overlap tests against `Static` entries (the mover's own entry excluded by `EntityId`): try the
full delta; then x-only (x preferred, a fixed documented bias); then z-only; else `[0, 0]`.
Wall slides fall out (tangential survives, normal cancels); corners resolve to exactly zero.

Validation is two-layered: the schema rejects negative extents (`L1014`, inclusive `range(min)`),
and the `SemanticValidate` hook rejects zero/non-finite sizes (`L1116`, with pointers like
`/shape/circle/radius`) — the invariant JSON Schema cannot express.

## Verify

```
cargo test -p ludion-spatial
cargo clippy -p ludion-spatial --all-targets -- -D warnings
cargo test -p ludion-spatial --release --test e6_bench -- --ignored --nocapture
cargo test -p ludion --test spatial_smoke
```

The e6 bench prints per-class medians (rebuild / k_nearest×500 / overlaps×300 /
move_and_slide×500) and asserts their sum fits the recorded 1.5 ms sub-budget
(`Plan/phase-12-spatial.md` §Status). The facade smoke proves the whole surface works through
`use ludion::prelude::*`.

## Gotchas

- **`builtin_registry()` is append-only.** World hashes key on registration index, so inserting or
  reordering a component (`Collider` included) silently re-keys every hash — the pinned-hash test in
  `ludion-components`' `lib.rs` trips if this happens. Register new components at the end.
- Queries read a **snapshot**: after `rebuild`, moving an entity's `Transform` does not move its
  index entry until the next rebuild. `move_and_slide` reads the mover's *current* position but
  sweeps against the *indexed* statics — statics don't move, so this is exact; a "static" the game
  secretly moves between rebuilds is a lie the index will faithfully repeat.
- The mover's own index entry is excluded by `EntityId`, not by position or body — an entity is
  never blocked by its own stale snapshot, even when it is itself `Static`.
- `Entry` and `RayHit` are `#[non_exhaustive]` (the freeze convention for library-built,
  consumer-read result types): construct neither downstream; read fields.
- Map-scale colliders (e.g. a generated map's merged perimeter walls) are safe to author without
  budgeting: any entry whose reach exceeds `OVERSIZE_REACH` files into its half's `oversize` grid
  instead of raising that half's `max_reach`, so it can no longer inflate — and slow down — every
  other probe of that half. Any future change to the index's structure must preserve both the
  survivor set and the pinned total order (sorted cell key, then `EntityId`) exactly — the
  conformance suite's brute-force oracles and the 1/2/8-thread hash-identity gate are the proof
  obligations, not code review alone.
- `k_nearest` measures distance to collider **centers**, not shape surfaces — a big AABB's center
  can be farther than a small circle whose edge is nearer.
- The bench's wall-clock helper is the crate's only sanctioned `Instant::now` site (scoped
  `#[allow(clippy::disallowed_methods)]`, observation-only). Don't add another; route new timing
  through the same helper.
- `ludion profile` cannot see `spatial::rebuild` (the CLI links no game systems) — measure with
  the crate's own bench, not `ludion profile`.
