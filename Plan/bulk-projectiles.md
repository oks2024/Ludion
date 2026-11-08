# Plan: Bulk 3D projectiles

> **Mechanized — every subtask shipped; pinned bench figures and the pinned golden blesses are owed
> (`Plan/owed.md`).** Driven by the external FPS requests 0006 (shots) and 0012 (gibs): thousands of
> fast shots in a 3D room, deterministic, swept (no tunnelling), drawn leaving the gun. Game-facing
> doc: `ludion docs projectiles`; crate doc: `ludion devdocs ludion-projectiles`.

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
    late latch and visuals on or off. Convergence is a per-tag opt-in (`ProjectileVisual::from_muzzle`,
    default off): an enemy's shots should not start at the player's gun.
11. **`first_hit` at `t = 0` uses a solid point projection**, because parry's EPA allocates for a ball
    that starts touching or inside a collider. A start that touches and travels away from, or along,
    the surface is no hit (the ray form tests a step along the ray against the shape); inward travel
    and an inside start still hit at `t = 0`.
13. **The pool capacity cap is `1 << 15`** (32,768): 4× the 5,000-shot target, rounded to a power of
    two. Per-slot step scratch is about 0.7 KB, so 2^20 would have meant roughly 0.9 GB. The pool digest
    is domain `v2` and covers high water, the free list and dead-slot generations.
14. **A restore takes the build's `ProjectileConfig`, not the snapshot's** (`ludion dev` restarts
    through `engine.restore`, so a changed gravity or capacity must apply). The snapshot's config is
    still validated; the pool is resized. A build with no pool drops the snapshot's. A capacity below
    the high-water mark restores nothing and warns `L4031`: truncating would drop live shots and leave
    a free list and generations no run produced.
15. **`projectiles_step` re-boxes its resource once per tick.** The ECS has no take-and-scope
    pattern, and the step needs the physics world and index while it mutates the pool; the crate
    step allocates nothing at one thread.
12. **L4030** warns once when a pool holds shots but `projectiles_step` never ran, the usual sign of
    an unscheduled step.
16. **Gibs ride the shot pool**, not rigid bodies (request 0012's numbers: 2,000 rigid bodies cost 4.6 ms
    mean / 9.6 ms p99 per step, against the game's whole 3.0 ms physics budget, with ~750 live gibs
    at gibfest) and not a second pool (one sweep, one hash block, one
    snapshot). The new per-shot fields (`spin`, `orientation`, `targets`) are optional and
    byte-identical when absent: an ext bit in the parked byte (codec v1 and digest `v2` stay) and
    `Settle` as response tag 4, so committed snapshots, replays and hashes do not move.
17. **Orientation and spin conventions.** Orientation is an `[x, y, z, w]` quaternion, spin a
    world-space angular velocity in rad/s, integrated `q += ½·dt·(ω,0)⊗q` and renormalised with
    `ludion_core::math::sqrt`, the crate's only root. Integration is skipped at bit-zero
    spin, so a spin-less shot never renormalises.
18. **Settle semantics.** It parks on `n.y >= floor_cos` (+Y up, default 45°) rather than against
    gravity, because gravity can be zero or scaled negative per shot. Walls never park and bounces
    are uncapped: only a floor contact is a rest, so a gib that hits a wall keeps bouncing instead
    of freezing mid-air. Only a physics-world contact parks: an index target (a `SpatialVolume`) is
    not ground, so a gib lands on it as on a wall, and the gib rests where the world is. `rest_speed`
    must be `> 0` and well above the resting re-bounce, about `restitution·|g|·dt/(1+restitution)`
    per tick, or the gib jitters. A rest is final: no push, no
    re-wake, expiry as usual. No new `HitOutcome`: `Reflected` / `Stopped` keep every consumer's
    match and the event codec unchanged.
19. **Per-shot `targets` only.** The index mask is what a gib needs to ignore actors; `ignore_layers`
    stays pool-wide because it is a physics-sweep filter built once per step.
20. **The Mesh look.** `scale` is a per-axis multiplier on all mesh-instanced looks (default 1, exact
    for existing visuals). A mesh draws the simulated orientation, never the heading; `Cube`/`Sphere`
    do once the shot has spin or an orientation. `cast_shadows` is opt-in (default `false`, tracers
    cast none). `fade_out` shrinks extents only, so it stays opaque. Such shots blend as
    `ProjectileItem::Posed`, a full TRS blend, while existing shots keep the translation-only one.
    `L4033` has two tiers: a missing file is refused at build; a load failure degrades, reports once
    and draws nothing. Skinned assets draw at bind pose.
21. **The Voxel look.** `size` is metres per voxel and the model document's own `scale` is ignored,
    so a tag's size means one thing across looks. The model pivots on the centre of its occupied
    voxels, so it tumbles and rests about its middle rather than the mesher's bottom-centre origin.
    The Mesh look pivots the same way, on its asset's bounding-box centre (one code path), so
    every asset look tumbles and rests around the sweep point; a pivot of zero is skipped.
    Assemblies are not a
    look: they are several models.
22. **The gib bench rows.** `bulk_gibs_bench` runs 1,000 settling, spinning gibs beside 5,000 shots,
    after an untimed 300-tick pre-roll so the window holds flying, bouncing and parked gibs with
    ongoing churn (about 39 % parked); gravity is set in the gib arm only so the shots-only row
    stays the run that produced the recorded figures. `extract_cost_of_1000_gibs` prices 1,000
    posed Mesh/Voxel gibs on top of 5,000 shots.

## Status

Shipped, by commit subject: the allocation-free physics `first_hit` sweep; the `ludion-projectiles`
pool crate and its byte codec; the engine hosting (`.projectiles(config)`, `projectiles_step`, the
resource digest, snapshots, L4026/L4027, corpus re-recorded with records identical); the drawing
(`.projectile_visuals`, L4028); muzzle convergence (L4029, gallery `projectile-muzzle`); the
allocation-free `t = 0` sweep and L4030; this bench and `ludion docs projectiles`.

Review fixes, sim side (06a): pierce skips a pierced entity's collider, outbound `t = 0` is no hit,
position-scaled push-out, digest v2, capacity cap 2^15, alloc claims scoped to one thread.

Review fixes, engine side (06b): `from_muzzle` per-tag convergence, restore under the builder's
config (`ProjectilePool::resize`, `L4031`), the per-tick re-box documented.

Bench (release, unpinned M5 laptop, 2026-11-07, indicative only): 5,000 live shots against a
120×12×120 room, 40 pillars and 500 index targets — `step + digest` 1.9 ms at 1 thread (step 1.8,
digest 0.08) and 0.47 ms at 8 threads; `extract_present_scene` for 5,000 drawn shots adds 0.13 ms.
The 1- and N-thread digest traces are identical. Pinned figures owed: `Plan/owed.md`.

Gibs (request 0012), by commit subject: gibs on the pool, `Response::Settle` with `spin`, `orientation`
and per-shot `targets` (r2211, sim); the Mesh look with `scale`, `cast_shadows`, `fade_out`, `Posed`
and L4033 (r2215); the Voxel look (r2216). Gallery `projectile-gibs`, `projectile-gibs-settle` and
`projectile-gibs-voxel`, macOS blessed on the unpinned M5. This arc adds the gib bench rows, the
`ludion docs projectiles` coverage and `ludion devdocs ludion-projectiles`.

Review fixes (12e): Settle parks only on physics-world contacts (index targets bounce); the Mesh look
pivots on its bounding-box centre like Voxel (the committed gib goldens did not move: the crystal is
centred); `rest_speed` must be `> 0`, with the resting re-bounce documented.

Gib bench (release, unpinned M5 laptop, 2026-11-08, indicative only): 5,000 shots + 1,000 gibs,
`step + digest` 1.92 ms at 1 thread (step 1.82, digest 0.10) and 0.51 ms at 8 threads, about 389 of
1,000 gibs parked; the digest traces are identical. `extract_present_scene` for 1,000 Mesh/Voxel
gibs on top of 5,000 shots adds about 0.03 ms (0.143 to 0.170 ms). Pinned figures owed: `Plan/owed.md`.

## Explicitly out of scope (triggers recorded)

- **Volleys are not bridged to the pool.** Trigger: a 2D-pattern game needs physics-swept bullets, or
  a 3D game wants `kind: pattern` timelines for pooled shots.
- **Textured or UV-mapped shot looks.** Mesh and Voxel looks draw the asset's own material. Trigger: a
  game needs a per-shot texture.
- **`kind: assembly` / `sculpt` looks.** Trigger: a game wants a multi-part or sculpted shot.
- **Pushing or re-waking parked debris.** A rest is final. Trigger: an explosion must kick rests.
- **Alpha or translucent fade.** `fade_out` shrinks, opaque. Trigger: a game wants a dissolve.
- **Per-shot `ignore_layers`.** Pool-wide only. Trigger: a shot class needs its own physics filter.
