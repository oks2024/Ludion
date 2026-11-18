---
description: Guard sight and memory: Perceiver zones, SightTarget points, line of sight, StimulusRecord ghosts
---

# Perception (sight and memory)

**Scope:** who sees whom each tick: `Perceiver` zones, `SightTarget` sample points, and the
`PerceptionResults` a game's AI reads, and the `StimulusRecord` a perceiver keeps of what it sensed. Cover and layers are `ludion docs collision-layers`; the
guard's route is `ludion docs patrol`.

## The loop

1. `Engine::builder().perception(PerceptionConfig { classes: vec!["player".into()] })`: the class names (at most 32). `build` refuses a bad list with `L4220`.
2. Schedule `.system("perception_update", perception_update)` **after whatever moves perceivers and targets**, and declare its queries: `.query::<(&Perceiver,)>().query::<(&SightTarget,)>().query::<(&StimulusRecord,)>()`.
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
| `memory` | keeps `StimulusRecord`s, see Memory | none |

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

## Memory

```yaml
components:
  Perceiver:
    sight:
      - range: 12.0
        half_angle_deg: 60.0
    memory:
      forget_sight_s: 12.0
      forget_hearing_s: 8.0
      max_records: 8
```

A perceiver with `memory` keeps one `StimulusRecord` **entity** per `(perceiver, sense, source)`. `perception_update` refreshes the sight record every tick the target is `visible`; a game feeds hearing or a custom sense with `report_stimulus(world, ctx, perceiver, Stimulus { sense, source, at, strength })`. Read them with `stimuli(world, perceiver)` (sorted `(sense, source)`); `forget(world, perceiver, source)` and `forget_all` drop them. Without `memory` nothing spawns and a world hashes as before.

| `StimulusRecord` field | Meaning |
| --- | --- |
| `perceiver`, `source` | marked entity refs; `source` is none for an anonymous stimulus |
| `sense` | `sight`, `hearing` or `{ custom: n }` |
| `last_position`, `last_direction` | where it was last sensed; the unit move since the previous sighting (sight only, else unchanged; `[0,0,0]` at creation) |
| `first_sensed_tick`, `last_sensed_tick` | gameplay ticks |
| `sensed_now` | true iff `last_sensed_tick` is this tick |
| `strength`, `zone` | `1.0` for sight; the lowest zone that saw it |

**Ghost recipe:** search where a source was last seen by reading `last_position` of a record with `sensed_now: false`.

- **Forgetting.** A record goes when `now - last_sensed_tick >= ticks(forget_s)` (`forget_sight_s` for sight, `forget_hearing_s` for hearing and custom; at least 1 tick). Ticks are gameplay time, so a held `GameClock` never forgets: the deadline moves by the hold's length.
- **Eviction.** Above `max_records` (1 to 64), the least recently sensed goes, ties by `(sense, source)`, anonymous first.
- Anonymous stimuli merge per `(perceiver, sense)`. Records are despawned with their perceiver, in the next update.
- Records are hashed and snapshotted like any entity, so a restore rejoins the uninterrupted run.
- `L1573` bad `memory` (or a non-finite record); `L1574` (warning, once per cause) `report_stimulus` dropped a stimulus: dead perceiver, no `memory`, or a non-finite `at` or `strength`.

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
- Results are not hashed or snapshotted; a restored world fills them on the next tick. Records are, because they are entities.
- Records are runtime entities: `scene.save` of a *running* world writes them like any spawned entity. A dead source keeps its record until it ages out.
- A perceiver never sees itself: a target equal to the perceiver is skipped.

## Next

`ludion docs collision-layers`, `ludion docs patrol`, `ludion docs systems`.
