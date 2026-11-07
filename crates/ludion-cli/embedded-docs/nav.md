---
description: Navigation — the walkability grid from static colliders, shared flow fields, NavAgent steering
---

# Navigation: a grid from the colliders, one flow field per target, agents that steer

**Scope:** `NavConfig`, `Engine::nav`, `nav_update`, `NavAgent`, `NavSteering`, `nav_steer`. The
engine writes a desired velocity; moving the body is yours — `ludion docs motor` for a capsule,
`ludion docs components` for the rows. Static colliders come from `ludion docs physics` and
`ludion docs collision-layers`; the neighbour index is `ludion docs spatial-3d`.

## The loop

1. Configure: `.nav(NavConfig { nx: 64, nz: 64, ..default })`. Without it no grid exists and
   nothing in `WorldHash` changes.
2. Schedule `spatial3d_rebuild → nav_update → nav_steer → your mover`. The game schedules each;
   nothing runs implicitly.
3. Put `NavAgent { target, .. }` on entities; write `target` each tick (it is a world position).
4. Assert in a test on `NavSteering` and on positions after N ticks; prove determinism with
   `ludion sim-hash` twice.

```rust
Engine::builder()
    .nav(NavConfig { nx: 64, nz: 64, y_min: -2.0, y_max: 8.0, ..NavConfig::default() })
    .system("spatial3d_rebuild", spatial3d::rebuild::<Transform, TransformHierarchy>)
    .system("nav_update", nav_update)
    .system("chase", chase)     // writes NavAgent.target
    .system("nav_steer", nav_steer)
    .system("mover", mover)     // applies NavSteering
```

## Format

`NavConfig` (Rust only, no scene text). Distances are metres.

| Field | Default | Valid |
| --- | --- | --- |
| `origin` | `[0, 0]` | finite; the (x, z) of cell (0, 0)'s minimum corner |
| `cell_size` | `1` | finite, `> 0` |
| `nx`, `nz` | `64` | `>= 1`, `nx * nz <= 4194304` |
| `y_min`, `y_max` | `-8`, `32` | finite, `y_min < y_max`; floors are looked for inside |
| `agent_radius` | `0.4` | finite, `> 0`; a floor is dropped where a capsule this wide collides |
| `agent_height` | `1.8` | finite, `> 0`; the headroom a floor needs |
| `max_step` | `0.4` | finite, `>= 0`; largest step on flat ground (slopes widen it) |
| `max_slope_deg` | `45` | in `(0, 90)` |
| `flow_radius` | `48` | finite, `> 0`; how far a field reaches from its target |
| `ignore_layers` | `[]` | names declared in `physics.layers`, none empty |

A bad value is `L4032` with a JSON Pointer to the field; `ludion explain L4032`.

`NavAgent` fields and defaults: `ludion docs components`. `target` is `[0,0,0]`, `speed` `4`,
`flying` `false`, `arrive_radius` `0.5`, `separation_radius` `0.75`, `separation_strength` `2`,
`separation_neighbors` `4` (at most 16), `neighbor_layers` all bits (`0` turns separation off).
A negative or non-finite field, or too many neighbours, is `L1340`. `NavSteering { velocity }` is
what `nav_steer` writes each tick.

## Rules

- **One agent class per `Nav`.** Radius, height, step and slope belong to the config, not the agent.
- **Agents' own colliders go on a layer listed in `ignore_layers`**: the sampler sees every collider,
  so a resting agent otherwise blocks its own column.
- **The grid reads the last completed physics step**: a static spawned at tick N is in the grid from
  N + 1. A static added, moved or removed costs one region rebuild the next tick, in entity order —
  rooms assembled at runtime need nothing extra. `request_full_rebuild` after a change the diff
  cannot see (a `physics.layers` edit); `set_config` rebuilds too.
- **Derived state.** The grid and the fields never enter `WorldHash` or a snapshot; a restore
  re-samples. Walkers hold still (zero velocity) until the grid exists.
- **One shared field per target cell**: 100 agents chasing one target are one compute, redone only
  when the target changes cell or the grid changes.
- **Walkers** follow the field gradient; where a cell has no direction they head straight at the
  target. **Flyers** steer only (3D seek plus separation), never read the grid, and work with no
  `Nav` at all.
- **Separation** reads the `Index3d`: give agents a `SpatialVolume` whose layer is in
  `neighbor_layers`.
- **Thread-count invariant**: `nav_steer` fans out over `par`; the result is bit-identical at any
  count (`ludion devdocs sim-parallelism`).

## Feeding the motor

`NavSteering` is a velocity: multiply by `dt`, move before physics, and let the moveset own gravity.

```rust
fn motor_mover(world: &mut World, ctx: &SystemCtx) {
    let mut moves: Vec<_> = world.query_ids::<(&NavSteering,)>()
        .map(|(id, (s,))| (id, s.velocity)).collect();
    moves.sort_by_key(|(id, _)| *id); // a fixed order, so the trace is stable
    for (id, v) in moves {
        let _ = move_character(world, id, [v[0] * ctx.dt, -0.1, v[2] * ctx.dt]);
    }
}
```

Put the motors on a layer that ignores itself and list it in `ignore_layers`.

## Gotchas

- A target whose cell has no node (within `agent_radius` of a wall, off the grid, beyond
  `flow_radius`) gives walkers the straight-line fallback — which can face a wall. Aim at open floor.
- Stacked ramps link each cell to the closest-height layer per direction.
- Cost, unpinned M5 laptop, release (indicative; pinned figures are owed): `nav_steer` 100 agents
  0.03 ms, 500 agents 0.17 ms at one thread; one flow field over a 64 x 64 room 0.11 ms; full grid
  build 0.8 ms, region rebuild 0.16 ms; 500 `CharacterMotor`s fed by it 3.8 ms (the motor is
  serial). Reproduce with `cargo test -p ludion-engine --release --test it -- nav_bench --ignored
  --nocapture`.

## Next

- `ludion docs motor` — the capsule that applies a steering velocity.
- `ludion docs spatial-3d` — the index separation reads.
- `ludion docs components` — the `NavAgent` and `NavSteering` rows.
