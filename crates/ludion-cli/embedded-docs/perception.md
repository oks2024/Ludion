---
description: Guard senses: Perceiver sight zones, SightTarget, noises and NoiseZone, StimulusRecord memory
---

# Perception (sight, hearing and memory)

**Scope:** who sees whom each tick: `Perceiver` zones, `SightTarget` sample points, and the
`PerceptionResults` a game's AI reads, the noises it hears, and the `StimulusRecord` a perceiver keeps of what it sensed. Cover and layers are `ludion docs collision-layers`; the
guard's route is `ludion docs patrol`.

## The loop

1. `Engine::builder().perception(PerceptionConfig { classes: vec!["player".into()], ..Default::default() })`: the class names (at most 32), and the hearing settings below. `build` refuses a bad config with `L4220`.
2. Schedule `.system("perception_update", perception_update)` **after whatever moves perceivers and targets**, and declare its queries: `.query::<(&Perceiver,)>().query::<(&SightTarget,)>().query::<(&StimulusRecord,)>()`.
3. Read `world.resource::<PerceptionResults>()`: `sight(perceiver)` (a slice), `samples()`, `zone_facing(perceiver, zone)`.
4. `ludion check-scene` validates both components; `ludion explain L1570` for a refusal.

Results are rebuilt every tick, sorted `(perceiver, zone, target)`, identical at any thread count, and never hashed or snapshotted (carried samples, below, are). With no config the system does nothing, and a world without the components hashes byte-identically.

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
| `hearing` | hears `emit_noise` noises, see Hearing | none |

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

## Hearing

```yaml
components:
  Perceiver:
    hearing:
      ear:
        - 0.0
        - 1.6
        - 0.0
      sensitivity: 1.0
      hears:
        - footstep
        - gunshot
  NoiseZone:
    shape:
      box:
        half_extents:
          - 20.0
          - 5.0
          - 20.0
    multiplier: 0.5
    kinds:
      - footstep
    priority: 0
```

Declare the kinds on the config: `PerceptionConfig { noise_kinds, surface_noise: vec![SurfaceNoise { surface, multiplier }], sound_occluder_layers, occlusion_factor (0.5), max_occluders (2), .. }`. A game then calls `emit_noise(world, ctx, Noise { at, radius, loudness, kind, source, surface, occlusion }) -> Option<NoiseId>` (`surface` is the `SurfaceId` of `MoveReport.ground.surface`). `perception_update` delivers it to every enabled perceiver with `hearing` that lists the kind and whose ear (`ear`, local) is within

`radius × NoiseZone × surface_noise × sensitivity × occlusion_factor^occluders`.

- **Zone.** Among the `NoiseZone`s that contain `at` and list the kind (empty `kinds` = all) the highest `priority` wins, ties to the lowest entity; multipliers never stack. `shape` is a `box` (oriented by the entity's rotation, scale ignored) or a `sphere`, boundaries inclusive. Rain is one big `NoiseZone` with `multiplier` below 1.
- **Surface.** `surface_noise` maps a `physics.surfaces` name to a multiplier; `None` or an unlisted surface is ×1.
- **Occlusion.** Only for `occlusion: true` and a non-empty `sound_occluder_layers`: one ray from `at` to the ear counts the distinct colliders on those layers (not the source or the listener), up to `max_occluders`, each multiplying the reach by `occlusion_factor`. Put walls and doors on a layer such as `sound_occluder`: one layer set for what muffles sound, shared by hearing and audio occlusion.
- **Timing.** A noise emitted before `perception_update` in a tick is delivered that tick; one emitted after it, the next. Each noise is delivered once, in `NoiseId` order. Pending noises are entities (`PendingNoise`, plus a `PerceptionState` counter), so a snapshot between emission and delivery loses nothing.
- **Results.** `results.heard(listener)`: `HeardNoise { listener, noise, at, kind, loudness, source, distance, effective_radius, occluders }` (the listener's own reach), sorted by noise id. `results.noises()`: `NoiseRecord { id, at, kind, loudness, source, effective_radius }` with the source-side radius (zone and surface applied), the number to draw a ring from. `HeardNoise.occluders` is the occluder count after the `max_occluders` cap, for drawing muffled rings.
- **Memory.** A listener with `memory` gets a hearing `StimulusRecord` per delivery, keyed `(listener, hearing, source)`; an anonymous noise merges per `(listener, hearing, kind)` (`StimulusRecord.kind`). `strength` is the loudness.
- **Codes.** `L1575` bad `NoiseZone`; `L1576` bad `hearing`; `L1577` (warning, once) `emit_noise` dropped a non-finite or negative `at`, `radius` or `loudness`; an undeclared `kind` is `L1572`.
- A world that never emits and has no `NoiseZone` hashes as before.

## Budget and stagger

```yaml
Perceiver:
  period_ticks: 3        # evaluate sight every 3rd tick (default 1; 0 is L1570)
```

`PerceptionConfig { max_sight_rays_per_tick: Some(2048), .. }` caps the line-of-sight rays per tick (`None`, the default, is no cap; 2048 is the recommended value; `Some(0)` is `L4220`).

- **Stagger.** A perceiver never evaluated waits for its phase (`tick % period == entity index % period`); after that it is due `period` ticks after its last evaluation. One deferred before its first evaluation gets `PerceiverStatus.evaluated_tick` one period back, so it stays due every tick until served.
- **Budget.** Due perceivers are served in id order from the one after `PerceptionState.sight_cursor`, wrapping. A perceiver's rays are its points in a wedge with a non-empty `blocked_by`. The first one served is always served, even over the cap; the first that does not fit stops the pass and every later one is deferred. A deferred perceiver stays due, so every one is served in turn.
- **Carried samples.** A perceiver not evaluated this tick keeps its last samples, with their old `evaluated_tick`, in `results.sight(..)`. `zone_facing` is current every tick (it costs no rays).
- **Counts.** `results.rays_cast()` (re-casts included) and `results.deferred()` (due, not served, sorted); `engine.describe` shows them under `perception`.
- **`L4221`** (warning, once): a perceiver has waited over four periods for a turn. Raise the budget or the `period_ticks` of idle guards.
- **Hashed state.** Carried samples are `SightSampleRecord` entities, one per `(perceiver, zone, target)`, and `PerceiverStatus { evaluated_tick }` rides the perceiver. They exist only for a perceiver with `period_ticks > 1`, or for every perceiver when a budget is set, and despawn with it. A world with neither hashes as before.

## Cover recipe

Declare layers `wall`, `cover_low`, `cover_high`, `cover_overhead` under `physics.layers` and put props on them. One target per posture, each with a point at its height:

| Posture | Point `y` | `blocked_by` |
| --- | --- | --- |
| stand | 1.6 | `wall`, `cover_high` |
| crouch | 0.7 | `wall`, `cover_low`, `cover_high` |
| prone | 0.2 | `wall`, `cover_low`, `cover_high` |

Low cover hides crouch and prone from a ray that passes through it; high cover hides all three. Add `cover_overhead` to a prone point's `blocked_by` and `see_through: [cover_overhead]` on the inner zone so a guard close enough sees under a table.

## Tracing

The `perception` trace channel (`ludion docs verify-loop`) prints what changed, derived from the results and the records after each tick; nothing is recorded sim-side, so tracing cannot move the hash. Each line is `channel`, `event`, `tick`, then the fields:

```
{"channel":"perception","event":"sight","tick":31,"perceiver":"guard","zone":"narrow","target":"mark","in_zone":true,"visible":true,"points_visible":1,"distance":10.2}
{"channel":"perception","event":"noise","tick":14,"listener":"guard","noise":0,"kind":"footstep","source":"mark","distance":2.7,"effective_radius":6.0,"occluders":0}
{"channel":"perception","event":"remember","tick":31,"perceiver":"guard","source":"mark","sense":"sight","at":[-5.8,0.0,-10.0]}
{"channel":"perception","event":"forget","tick":150,"perceiver":"guard","source":"mark","sense":"sight","at":[5.8,0.0,-10.0]}
```

- `sight`: one line per `(perceiver, zone, target)` whose `visible` changed since the previous traced tick; absent means not visible. A key that dropped out of the results while visible prints `in_zone: false, visible: false` and no sample fields. `zone` is the zone's `name` (its index when unnamed).
- `noise`: one line per delivery (a noise heard by two listeners prints twice), ordered `(listener, noise)`. A tick the game held (`GameClock`) prints none, so a held frame repeats nothing.
- `remember` and `forget`: a `StimulusRecord` that appeared or vanished; `sense` is `sight`, `hearing` or `custom:<k>`, `kind` appears for a hearing record, `at` is its `last_position` (a forget prints the last one seen). A refresh prints nothing.
- Order within a tick: `sight`, `noise`, `forget`, then `remember`, after the `ai` lines.
- `ludion trace` runs built-ins only, so it never ticks `perception_update`: use a game's `--trace` or `trace_with(builder, ..)` with `TraceChannel::Perception`.

## Gotchas

- **An empty `blocked_by` means nothing can block the point:** no ray is cast and it is seen through walls. (An include-less physics filter would hit everything; the engine never builds one.)
- A world with no `PhysicsWorld` (no colliders) casts no rays: every in-wedge point is seen.
- A facing never `look`s: the zone follows the entity's rotation, so a body that never turns sees only along its start direction.
- Results are not hashed or snapshotted; a restored world fills them on the next tick. Records are, because they are entities (so are `SightSampleRecord`s).
- Records are runtime entities: `scene.save` of a *running* world writes them like any spawned entity. A dead source keeps its record until it ages out.
- **A staggered or deferred guard's `sensed_now` blinks:** only fresh samples refresh memory, so `sensed_now` is false on ticks the guard is not evaluated. Read `visible` and `evaluated_tick` from the results, or `last_sensed_tick` from the record.
- **A ray budget makes sight hashed state** (`SightSampleRecord`, `PerceiverStatus`, the cursor): turning it on moves the world hash and a replay's.
- A perceiver never sees itself: a target equal to the perceiver is skipped.
- An empty `sound_occluder_layers` means no occlusion, not "everything occludes".
- Surface multipliers are surface *names* in the perception config, not a physics property.

## Debug draw

`--debug-draw perception` (`ludion docs debug-draw`) draws what the last completed tick decided, host-side at extraction: read-only, never hashed. It needs a game's build that ticks `perception_update` (the built-in `ludion render` host runs no system, so it draws nothing). `perception:zones`, `perception:rays`, `perception:noises` and `perception:memory` pick one part.

- **Zones:** each enabled perceiver's zone wedge at its range: green when any target is `visible`, amber when one is `in_zone` only, grey otherwise. Both half-angles at most 90 degrees draw a frustum, a wider zone a flat wedge at eye height.
- **Rays:** eye to every sample point of each in-zone target, green where the point is in `points_visible`, red otherwise, plus a red cross at the blocker's origin. Results hold no hit point, so a ray is drawn to the point, not to the wall.
- **Noises:** a ring of `effective_radius` per noise and a line from the noise to each ear that heard it.
- **Memory:** a cross at each record's `last_position`: sight cyan, hearing yellow, custom magenta, dimmed when not `sensed_now`.

Geometry is the current transforms, verdicts are the last results: a staggered perceiver's rays show its carried verdict against the points as they are now.

## Checks

`L1572` is also found without a tick, once per occurrence with a JSON Pointer and the nearest declared name. `ludion check-scene` links no game, so it checks layers only: a `see_through` or `blocked_by` name missing from `physics.layers`. A game's `--check-scene` and the wire's `scene.check` on a game host also check classes (`detects`, `SightTarget.class`) and noise kinds (`hears`, `NoiseZone.kinds`) against its `PerceptionConfig`. A scene with no collider has no physics world at run time, so the layer check is stricter than a tick there.

## Next

No later wave. `ludion docs collision-layers`, `ludion docs patrol`, `ludion docs systems`.
