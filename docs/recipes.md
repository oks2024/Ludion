---
description: Copyable patterns: spawn/despawn inside a system, AABB collision, driving lights from code
---

# Recipes

**Scope:** Short copyable patterns for things most games need. The API rules behind them are systems; the spatial index is collision.

**Spawn / despawn inside a system.** A system holds `&mut World`, so it can spawn and despawn
directly — but **not while a query iterator is borrowing the world**. Collect first, mutate after.

```rust
fn spawn_ball(world: &mut World) -> EntityId {
    let e = world.spawn();
    world.insert(e, Transform::default()).unwrap();
    world.insert(e, Sprite { color: [1.0, 1.0, 1.0, 1.0], ..Sprite::default() }).unwrap();
    world.insert(e, Ball { velocity: [3.0, 4.0] }).unwrap();
    e
}
```

**Collision the deterministic way — AABB in a system.** Read positions, decide, then despawn the
collected ids (`query_ids` yields each row's `EntityId`):

```rust
fn break_bricks(world: &mut World, _ctx: &SystemCtx) {
    // Read the ball's center once.
    let ball = world.query::<(&Transform, &Ball)>().next().map(|(t, _)| t.translation);
    let Some([bx, by, _]) = ball else { return };

    let dead: Vec<_> = world
        .query_ids::<(&Brick, &Transform)>()
        .filter(|(_, (_, t))| {
            let dx = (t.translation[0] - bx).abs();
            let dy = (t.translation[1] - by).abs();
            dx < 0.6 && dy < 0.3   // half-extents of ball+brick
        })
        .map(|(e, _)| e)
        .collect();
    let _ = world.despawn_many(&dead);
}
```

**Lighting a scene dynamically** — Breakout wants exactly this. Drive a `PointLight` from a system;
because `intensity` is component state, it is hashed and reproducible:

```rust
fn pulse_lights(world: &mut World, ctx: &SystemCtx) {
    let t = ctx.time.seconds() as f32;
    for (light,) in world.query::<(&mut PointLight,)>() {
        light.intensity = 1.5 + (t * 2.0).fract();   // deterministic: t is exact ticks/rate
    }
}
```

Move a light instead by mutating its entity's `Transform` in the same shape.

## Gotchas

- **A stale `EntityId` is an error, not a silent hit.** After you despawn an entity, drop its handle;
  reusing it is an `L0002` diagnostic (generational handles catch use-after-despawn).

## Next

- `ludion docs systems` — the system contract and the determinism rules.
- `ludion docs input` — reading keyboard, mouse and gamepad input.
- `ludion docs collision` — colliders and spatial queries.
- `ludion docs components` — declaring components, and the built-in visual set.
- `ludion docs verify-loop` — the `cargo test` verification loop.
