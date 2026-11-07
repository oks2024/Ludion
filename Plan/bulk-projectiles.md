# Plan: Bulk 3D projectiles

> **Mechanized — every subtask shipped; pinned bench figures and the pinned golden blesses are owed
> (`Plan/owed.md`).** Driven by the external FPS request 0006: thousands of fast shots in a 3D room,
> deterministic, swept (no tunnelling), drawn leaving the gun. Game-facing doc: `ludion docs
> projectiles`; crate doc: `ludion devdocs ludion-projectiles`.

## Design decisions

1. **A new crate `ludion-projectiles` over `ludion-physics` and `ludion-spatial`.** Shots are not
   entities: an ECS entity per shot costs spawn, hierarchy and hash overhead the pool avoids, and the
   crate keeps both worlds' dependencies out of the pool's core types.
2. **The engine steps gravity, drag, lifetime and the swept first hit** (`PhysicsWorld::first_hit`
   plus `Index3d::cast`). Games should not each reimplement a sweep that must agree with the
   physics shape set.
3. **Hit responses run in-step** — `Despawn`, `Reflect { restitution, max_bounces }`, `Pierce { max }`,
   `Stop` — because a ricochet or pierce needs mid-tick continuation of the remaining travel.
4. **Homing, splash and damage are the game's**, driven by `HitEvent` and the opaque `tag: u32`. The
   engine has no health or damage model to guess at.
5. **Storage is SoA with generational ids**: the step is a streaming pass, and a stale id never
   addresses a recycled slot.
6. **One `WorldHash` block, only while the pool exists**, via a registry resource digest (hash rule
   6); without a pool every hash and trace is unchanged. Snapshots carry the pool, its pending
   events and the `spatial3d` index, so a mid-flight restore reproduces the uninterrupted trace.
7. **Projectiles stay out of the target index**: the 3D broadphase bench showed a targets-only
   index is the cheaper cast path, and shots never need to hit each other.
8. **Casts fan out over `par`**: chunked, thread-count-invariant; the bench asserts the digest trace
   at 1 and N threads is identical.
9. **No re-hit**: the last contact is excluded from the next cast and the shot is pushed out by an
   epsilon, so Reflect and Pierce cannot stick to or re-pierce a surface. Pierce memory also skips the
   pierced entity's physics collider (`first_hit_skipping`), so an actor with both a collider and an
   index volume is hit once. The push-out is `SKIN` scaled by the position's magnitude, so it
   survives f32 rounding far from the origin.
10. **Muzzle convergence is render-only**: extract captures an offset when it first sees a shot's
    generational id (the muzzle under the frame's resolved camera, minus the drawn position) and eases
    it to zero over `converge_distance` (4 m). The sim never sees it; `WorldHash` is pinned equal with
    late latch and visuals on or off.
11. **`first_hit` at `t = 0` uses a solid point projection**, because parry's EPA allocates for a ball
    that starts touching or inside a collider. A start that touches and travels away from, or along,
    the surface is no hit (the ray form tests a step along the ray against the shape); inward travel
    and an inside start still hit at `t = 0`.
13. **The pool capacity cap is `1 << 15`** (32,768): 4× the 5,000-shot target, rounded to a power of
    two. Per-slot step scratch is about 0.7 KB, so 2^20 would have meant roughly 0.9 GB. The pool digest
    is domain `v2` and covers high water, the free list and dead-slot generations.
12. **L4030** warns once when a pool holds shots but `projectiles_step` never ran, the usual sign of
    an unscheduled step.

## Status

Shipped, by commit subject: the allocation-free physics `first_hit` sweep; the `ludion-projectiles`
pool crate and its byte codec; the engine hosting (`.projectiles(config)`, `projectiles_step`, the
resource digest, snapshots, L4026/L4027, corpus re-recorded with records identical); the drawing
(`.projectile_visuals`, L4028); muzzle convergence (L4029, gallery `projectile-muzzle`); the
allocation-free `t = 0` sweep and L4030; this bench and `ludion docs projectiles`.

Review fixes, sim side (06a): pierce skips a pierced entity's collider, outbound `t = 0` is no hit,
position-scaled push-out, digest v2, capacity cap 2^15, alloc claims scoped to one thread.

Bench (release, unpinned M5 laptop, 2026-11-07, indicative only): 5,000 live shots against a
120×12×120 room, 40 pillars and 500 index targets — `step + digest` 1.9 ms at 1 thread (step 1.8,
digest 0.08) and 0.47 ms at 8 threads; `extract_present_scene` for 5,000 drawn shots adds 0.13 ms.
The 1- and N-thread digest traces are identical. Pinned figures owed: `Plan/owed.md`.

## Explicitly out of scope (triggers recorded)

- **Volleys are not bridged to the pool.** Trigger: a 2D-pattern game needs physics-swept bullets, or
  a 3D game wants `kind: pattern` timelines for pooled shots.
- **Asset-mesh and textured shot looks.** The looks are billboard, cube, sphere and hidden. Trigger: a
  game needs a non-primitive or textured shot.
