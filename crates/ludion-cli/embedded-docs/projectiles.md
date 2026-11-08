---
description: Pooled 3D projectiles — swept against physics and the spatial3d index, hit responses, drawing
---

# Projectiles: thousands of fast 3D shots, swept deterministically

**Scope:** `ProjectileConfig`, the pool resource `Projectiles`, `spawn_projectile`,
`projectiles_step`, its events, `ProjectileVisuals`. For 2D bullet fields use `ludion docs
patterns`; for one-off physics bodies use `ludion docs physics`.

## The loop

1. Configure: `.projectiles(ProjectileConfig { capacity: 5_000, ..default })`. Without it there is no
   pool and nothing in `WorldHash` changes.
2. Schedule `projectiles_step` **after** `spatial3d_rebuild`, and every system that reads its events
   after the step. The game schedules it; nothing runs it implicitly.
3. Fire with `spawn_projectile(world, ProjectileDesc)`; a refusal is `L4026`, never a panic.
4. Read `Projectiles::events()` (`hits`, `expired`) in a later system.
5. Prove it: `ludion sim-hash` or a test over `WorldHash`.

```rust
Engine::builder()
    .projectiles(ProjectileConfig::default())
    .projectile_visuals(visuals())
    .system("spatial3d_rebuild", spatial3d::rebuild::<Transform, TransformHierarchy>)
    .system("fire", fire)
    .system("projectiles_step", projectiles_step)
    .system("score", score) // reads p.events().hits
```

The full runnable game is `crates/ludion/examples/projectiles.rs`
(`cargo run -p ludion --example projectiles`).

## Format

`ProjectileConfig` (host setting, validated at build; invalid is `L4027`):

| Field | Default | Range / meaning |
| --- | --- | --- |
| `capacity` | `1024` | `1..=32768` live shots |
| `gravity` | `[0, -9.81, 0]` | finite m/s²; scaled per shot by `gravity_scale` |
| `targets` | all bits | layer mask of the `spatial3d` index a shot can hit |
| `ignore_layers` | `[]` | `physics.layers` names the sweep skips |
| `include_sensors` | `false` | whether sensor colliders are hit |

`ProjectileDesc::new(position, velocity)` is a ray with no gravity or drag, ten seconds to live,
despawning on its first hit. Set fields after it:

| Field | Default | Meaning |
| --- | --- | --- |
| `gravity_scale`, `drag` | `0`, `0` | `v *= 1 / (1 + drag·dt)` per tick; `drag >= 0` |
| `radius` | `0` | sweep radius; `0` is a ray |
| `owner` | `None` | a `Handle` this shot never hits (the shooter) |
| `lifetime` | `10` | seconds, `> 0`; expiry emits an `Expired` event |
| `tag` | `0` | opaque `u32`, echoed in every event |
| `response` | `Despawn` | `Despawn`, `Reflect { restitution, max_bounces }`, `Pierce { max }`, `Stop`, `Settle { restitution, friction, rest_speed, floor_cos }` (`Response::settle(r, f, v)` fills the 45° floor) |
| `spin` | `[0; 3]` | world-space angular velocity, rad/s; integrates `orientation` |
| `orientation` | identity `[x, y, z, w]` | starting quaternion, normalised at spawn |
| `targets` | `None` | this shot's `spatial3d` layer mask; `None` uses the pool's `targets` |

## Rules

- **Per tick** the step integrates gravity and drag, then sweeps each shot's travel against the
  physics world (the last completed physics step) and the `spatial3d` index, and resolves the first
  hit by its response inside the step, so a ricochet continues the remaining travel the same tick.
  Casts fan out over `ludion_core::par`; results are identical at any thread count. After warm-up a
  step at one thread (`LUDION_THREADS=1`) allocates nothing; at more threads `par` spawns scoped
  workers each tick, which allocate. The engine's `projectiles_step` system takes the resource out of
  the world and puts it back, so it re-boxes that resource once per tick (the crate step itself does
  not allocate).
- **Responses:** `Reflect` bounces `v − (1 + restitution)(v·n)n` and the hit after `max_bounces`
  despawns. `Pierce` passes through up to `max` **index targets**; a world (physics) hit always
  despawns, except on an entity the shot already pierced, whose collider is skipped too. `Stop`
  parks the shot at the contact. Reflect and Pierce never re-hit the surface they just left.
- **Settle** is the gib: it bounces off every surface (normal speed × `restitution`; tangential speed
  and spin × `1 − friction`) and parks on a floor (`n.y >= floor_cos`, +Y up, default 45°) once
  slower than `rest_speed`, zeroing spin and keeping its orientation. Walls never park and bounces
  have no cap. A rest is final: a parked gib is never pushed and expires with `Expired`. Events are
  `Reflected` / `Stopped`. `spin` integrates `orientation` each tick; a shot using none of
  `spin`, `orientation` or `targets` hashes exactly as before.
- **Projectiles are not in the target index.** Shots never hit each other; put targets in the index
  with `SpatialVolume` (`ludion docs spatial-3d`).
- **Events** (`HitEvent`: id, handle, point, normal, `t`, arrival velocity, tag, outcome; `Expired`)
  are valid until the next step, ordered by slot then hit order. A held frame (`game_dt == 0`)
  freezes every shot.
- **Hashing:** `WorldHash` folds the pool as one block, only while the resource exists. Engine
  snapshots carry the pool, its generations, free list and pending events, and the `spatial3d` index;
  a restore reproduces the uninterrupted trace. A restore takes the **build's** `ProjectileConfig`
  (gravity, capacity, layers), not the snapshot's, and resizes the pool to it; a build with no pool
  drops the snapshot's. A capacity below the pool's high-water mark restores an empty pool instead
  (`L4031`).
- **Diagnostics:** `L4026` refused spawn (once per cause), `L4027` invalid config, `L4028` invalid
  visuals, `L4029` unresolved muzzle, `L4030` a pool holding shots whose step never ran (usually an
  unscheduled `projectiles_step`), `L4031` a restore refused for a too-small capacity, `L4033` a `Mesh`/`Voxel` look whose file is
  missing (refused at build) or fails to load (drawn as nothing, reported once as degraded).

## Game-code recipes

The engine owns motion and the first hit; behaviour is the game's, driven by events and `tag`.

```rust
// Homing: steer toward a target each tick before the step.
let projectiles = world.resource_mut::<Projectiles>().unwrap();
let at = projectiles.pool().position(id).unwrap();
projectiles.pool_mut().set_velocity(id, scale(normalize(target - at), speed));

// Splash and damage: on a HitEvent, query the index and look the tag up in your own table.
for hit in projectiles.events().hits.clone() {
    index.overlap_sphere(hit.point, radius, &Filter::new(ACTORS), &mut near);
    apply_damage(damage_for(hit.tag), &near);
}
```

```rust
// Gibs on a kill: settle on the floor, tumble, hit the world only.
for _ in 0..24 {
    let mut gib = ProjectileDesc::new(at, scatter(&mut rng, 2.0..8.0));
    gib.gravity_scale = 1.0;
    gib.radius = 0.08;
    gib.lifetime = 6.0;
    gib.tag = GIB_TAG; // visuals: tag -> Mesh { asset } look, scale, fade_out
    gib.response = Response::settle(0.35, 0.4, 0.6);
    gib.spin = [spin(&mut rng), spin(&mut rng), spin(&mut rng)];
    gib.targets = Some(0);
    spawn_projectile(world, gib);
}
```

## Drawing

`.projectile_visuals(ProjectileVisuals)` is a per-tag table with a `fallback` for tags without an entry:

| `ProjectileVisual` field | Default | Meaning |
| --- | --- | --- |
| `look` | `Billboard { softness: 1.0 }` | also `Cube`, `Sphere`, `Hidden`, `Mesh { asset }` (a `*.mesh.yaml`; skinned assets at bind pose), `Voxel { model }` (a `*.voxel.yaml`; `size` is metres per voxel, pivots on its occupied-voxel centre; not `kind: assembly`) |
| `size` | `0.05` | metres, positive and finite |
| `stretch` | `0` | seconds of travel in the streak: length `max(size, speed·stretch)` along the heading |
| `color` | white | linear RGBA |
| `emissive` | `4` | mesh emissive strength or billboard RGB multiplier |
| `additive` | `true` | billboard only |
| `from_muzzle` | `false` | draw this tag's new shots from the muzzle and converge them |
| `scale` | `[1, 1, 1]` | per-axis extent multiplier on `Cube`, `Sphere`, `Mesh`, `Voxel`; each finite and `> 0` (`L4028`) |
| `cast_shadows` | `false` | whether a mesh-instanced shot casts a shadow |
| `fade_out` | `0` | seconds before expiry over which extents shrink to nothing, `>= 0` (`L4028`); opaque |
| `medium` | `1.0` | fog medium scale in `[0, 1]` (`L4028`); `0` keeps the shot's hue in haze (`ludion docs atmosphere`) |

`Mesh` and `Voxel` always draw the shot's simulated orientation, extents `size × scale`, no `stretch`;
`Cube` and `Sphere` do once the shot has `spin` or a starting `orientation`, otherwise they face the
heading. Shots join the scene pass's existing instanced draws; there is no new pass. Under
`present_interpolation` they interpolate between ticks. `muzzle: Some("gun")` names a `Viewmodel`
entity: a shot of a `from_muzzle` tag first seen by extract is drawn leaving that point and eases onto its simulated path
over `converge_distance` (default 4 m). The offset is render-only and never reaches the sim or the
hash. A one-shot render (`ludion render`), a restore and the first frame after a load extract
unprimed, so shots in flight draw at their simulated position.

## Gotchas

- Scheduling `projectiles_step` before `spatial3d_rebuild` sweeps the previous tick's index; never
  scheduling it is `L4030`.
- Pierce counts index targets only; a pierce shot into a wall still despawns.
- A shot spawned already touching a collider hits at `t = 0` when it travels into it; touching and
  moving away or along the surface is no hit.
- Put actor colliders (capsules) on a layer the pool ignores (`ProjectileConfig::ignore_layers`) and
  give actors a `SpatialVolume`: a Pierce shot crosses index targets only, and a physics hit
  despawns it.
- Events are gone after the next step: copy what you need in the same tick.
- A gib whose `targets` is not `Some(0)` hits actors and raises `HitEvent`s.
- Cost at 5,000 live shots (and `bulk_gibs_bench`, 1,000 gibs beside them) is measured by `cargo
  test -p ludion-projectiles --release --test bulk_bench -- --ignored --nocapture`; the draw side
  by `projectiles_extract_bench` (`extract_cost_of_1000_gibs`); keep damage and homing work proportional to hits, not shots.

## Next

- `ludion docs spatial-3d` — the target index, `SpatialVolume`, queries for splash.
- `ludion docs physics` — the colliders a shot sweeps against, `physics.layers`.
- `ludion docs viewmodel` — the first-person gun a muzzle names.
- `ludion docs patterns` — 2D bullet volleys (a separate system).
- `ludion docs nav` — walkability grid, shared flow fields and `NavAgent` steering (feeds the motor).
- `ludion docs culling` — projectile mesh looks are GPU frustum culled.
