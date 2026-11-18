---
description: Guard sight: Perceiver zones, SightTarget sample points, batched line of sight, PerceptionResults
---

# Perception (sight)

**Scope:** who sees whom each tick: `Perceiver` zones, `SightTarget` sample points, and the
`PerceptionResults` a game's AI reads. Cover and layers are `ludion docs collision-layers`; the
guard's route is `ludion docs patrol`.

## The loop

1. `Engine::builder().perception(PerceptionConfig { classes: vec!["player".into()] })`: the class names (at most 32). `build` refuses a bad list with `L4220`.
2. Schedule `.system("perception_update", perception_update)` **after whatever moves perceivers and targets**, and declare its queries: `.query::<(&Perceiver,)>().query::<(&SightTarget,)>()`.
3. Read `world.resource::<PerceptionResults>()`: `sight(perceiver)` (a slice), `samples()`, `zone_facing(perceiver, zone)`.
4. `ludion check-scene` validates both components; `ludion explain L1570` for a refusal.

Results are rebuilt every tick, sorted `(perceiver, zone, target)`, identical at any thread count, and never hashed or snapshotted. With no config the system does nothing, and a world without the components hashes byte-identically.

**Freshness:** transforms are current; line of sight reads the last completed physics step. A sweep reads gameplay time, so a held `GameClock` freezes it.

## Format

Scene files are block-style YAML (a non-empty `[a, b]` is `L1005`).

```yaml
components:
  Perceiver:
    eye:
      - 0.0
      - 1.6
      - 0.0
    look:
      pitch_deg: 5.0
    sight:
      - name: inner
        range: 6.0
        half_angle_deg: 60.0
        detects:
          - player
        see_through:
          - cover_overhead
        see_through_max_range: 3.0
      - name: outer
        range: 14.0
        half_angle_deg: 30.0
        range_by_visibility: true
```

| `Perceiver` field | Meaning | Default |
| --- | --- | --- |
| `enabled` | `false` gives no samples | `true` |
| `eye` | local offset of the eye | `[0, 0, 0]` |
| `look` | `yaw_deg`, `pitch_deg` from the entity's rotation | `0, 0` |
| `sight` | 1 to 8 zones | required |
| `sweep` | `amplitude_deg`, `period_s`, `dwell_s`, `phase_s`, `shape` (`ping_pong_linear` \| `sine`) added to every zone's yaw | none |

| `sight[]` field | Meaning | Default |
| --- | --- | --- |
| `name` | unique in the perceiver | `""` |
| `range` | metres, above 0 | required |
| `half_angle_deg` | horizontal, in (0, 180] | required |
| `vertical_half_angle_deg` | in (0, 90] | `45` |
| `detects` | class names; empty means every class | `[]` |
| `see_through`, `see_through_max_range` | layers dropped from `blocked_by` for points within that many metres of the eye | `[]`, `0` (off) |
| `range_by_visibility`, `visibility_clamp` | range times the target's `visibility`, clamped to `[lo, hi]` (unset: floored at 0) | `false`, unset |

```yaml
components:
  SightTarget:
    class: player
    visibility: 0.5
    points:
      - offset:
          - 0.0
          - 1.6
          - 0.0
        blocked_by:
          - wall
          - cover_high
    require:
      fraction: 0.5
```

`SightTarget`: `class` (a config class), `visibility` (`1`), 1 to 8 `points` (`offset` in the target's frame, `blocked_by` layers), `require`: `any` (default), `all`, or `fraction: f` with f in (0, 1] (visible when at least `ceil(f * points)` points are seen).

## Rules

- **Frame.** The zone faces the entity's rotation, then `look`, then the sweep yaw. Yaw 0 looks along −Z, **positive yaw turns left, positive pitch looks down** (the camera rig's convention, `ludion docs camera`). `zone_facing` returns the result.
- **Wedge, not cone.** A point is in the zone when its horizontal angle is within `half_angle_deg`, its vertical angle within `vertical_half_angle_deg`, and its distance within the effective range. A point outside counts as not seen.
- **A sample exists only when some point is in the wedge.** `SightSample`: `visible`, `points_visible` (a bitmask: bit `i` is point `i` in the wedge with a clear line of sight), `blocker` (the nearest hit of the lowest-index blocked point), `distance` and `angle_deg` (to the target's origin), `effective_range`, `evaluated_tick`.
- **Line of sight** is a ray from the eye to each in-wedge point, cast only against the point's blocker set (`blocked_by` minus `see_through` inside its range). The target's own collider is skipped; so is the perceiver's when the eye sits inside it.
- **Sweep** is a closed form of gameplay time: `ping_pong_linear` runs −amplitude to +amplitude, dwells, returns, dwells (`period_s > 2 * dwell_s`); `sine` needs `dwell_s: 0`.
- **Codes.** `L1570` bad `Perceiver`; `L1571` bad `SightTarget`; `L4220` bad config; `L1572` (warning, once per name) an undeclared class or layer: the zone detects nothing of that class, a point with an unknown `blocked_by` layer is never seen, an unknown `see_through` layer is dropped.

## Cover recipe

Declare layers `wall`, `cover_low`, `cover_high`, `cover_overhead` under `physics.layers` and put props on them. One target per posture, each with a point at its height:

| Posture | Point `y` | `blocked_by` |
| --- | --- | --- |
| stand | 1.6 | `wall`, `cover_high` |
| crouch | 0.7 | `wall`, `cover_low`, `cover_high` |
| prone | 0.2 | `wall`, `cover_low`, `cover_high` |

Low cover hides crouch and prone from a ray that passes through it; high cover hides all three. Add `cover_overhead` to a prone point's `blocked_by` and `see_through: [cover_overhead]` on the inner zone so a guard close enough sees under a table.

## Gotchas

- **An empty `blocked_by` means nothing can block the point:** no ray is cast and it is seen through walls. (An include-less physics filter would hit everything; the engine never builds one.)
- A world with no `PhysicsWorld` (no colliders) casts no rays: every in-wedge point is seen.
- A facing never `look`s: the zone follows the entity's rotation, so a body that never turns sees only along its start direction.
- Results are not hashed or snapshotted; a restored world fills them on the next tick.
- A perceiver never sees itself: a target equal to the perceiver is skipped.

## Next

`ludion docs collision-layers`, `ludion docs patrol`, `ludion docs systems`.
