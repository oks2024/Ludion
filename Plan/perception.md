# Plan: Perception — sight zones, sample points, batched line of sight

> **Waves 1–2 mechanized — sight and stimulus memory ship; later waves are laid out below.**
> Driven by Neon Shroud request 0001 (stealth AI senses). Game-facing doc: `ludion docs perception`.

## Design decisions

1. **Layout mirrors nav.** The components live in `crates/ludion-components/src/perception.rs` (builtins, so scenes
   author them); the host is `crates/ludion-engine/src/perception.rs`: `PerceptionConfig`, `PerceptionResults`,
   `perception_update`. No new crate: the algorithm needs `PhysicsWorld`, `Transform` and `world_transform`,
   which already meet in the engine.
2. **Memory is one entity per `StimulusRecord` (Wave 2)**, each holding single marked refs (`perceiver`, optional
   `source`). Those fit `L1392` as it stands, so no sanctioned entity-ref collection is built. Its trigger stays the
   first component that truly needs a list of refs.
3. **Frames.** The zone frame is the entity's world rotation, then `look` (yaw about +Y, then pitch). It follows the
   camera convention: yaw 0 looks along −Z, positive yaw turns left, **positive pitch looks down**
   (`ludion docs camera`). The game's example writes `pitch_deg: -5` for "down"; the sign differs and is an owner item.
   Sweep yaw is added to `look.yaw_deg`.
4. **Wedge test per point** in the zone frame: `h = atan2(x, -z)`, `v = atan2(y, hypot(x, z))`. A point is in the zone
   when `|h| <= half_angle`, `|v| <= vertical_half_angle` and its eye distance is within the effective range. `in_zone`
   is any point in the zone, and only `in_zone` samples are reported. `distance`/`angle_deg` are for the target's
   world origin. A point outside the zone counts as not visible.
5. **Effective range** is `range * clamp(visibility, lo, hi)` when `range_by_visibility`; the clamp is unset by
   default (floored at 0). Otherwise `range`.
6. **Blockers per (zone, point):** `blocked_by` minus `see_through` when the point is within `see_through_max_range`
   of the eye (0 disables), sorted and deduped. **An empty set means the point is unblockable and no ray is cast**:
   an include-less `QueryFilter` hits everything.
7. **LoS rays** go eye to point (`Sweep::new(eye, point - eye, 0.0).excluding(target)`), grouped by blocker set in a
   `BTreeMap`, one `sweep_filter(include_set)` per group, run through `first_hits`. A hit on the perceiver itself (eye
   inside its own blocking collider) is re-cast with `first_hit_skipping(.., [perceiver, target])`. Rays are built in
   sorted order, so the result is thread-count invariant.
8. **`require`**: `any` (default) | `all` | `{ fraction: f }`, f in (0, 1], visible at `ceil(f * points)` seen points.
   `blocker` is the nearest hit of the lowest-index blocked point.
9. **Sweep** is a closed form of gameplay time: `t = game_time.seconds() + phase_s` in f64, reduced `rem_euclid(period_s)`
   before the f32 math. `ping_pong_linear`: travel, dwell, travel back, dwell, with travel `(period_s - 2 dwell_s) / 2`.
   `sine`: `amplitude * sin(2 pi t / period_s)`, `dwell_s` 0. Transcendentals go through `ludion_core::math`.
10. **Names.** `PerceptionConfig.classes` is at most 32 names. `detects: []` means every class. An undeclared class or
    layer reports one `L1572` per name (`report_full`) and the zone or point detects or sees nothing: conservative,
    never silent. With no `PhysicsWorld` no ray is cast and layer names are not checked.
11. **Results are a resource rebuilt every tick**, not hashed, not snapshotted. `perception_update` with no config is a
    no-op (the `nav_update` mold). `enabled: false` gives no samples. `evaluated_tick` is the current gameplay tick.
12. **Freshness:** transforms are current, LoS reads the last completed physics step.
13. **Scene files are block-style YAML**, so every authored example (docs, doc comments) is block style.
14. **One entity per memory record, single marked refs (W2).** `perceiver` is a marked `EntityRefSchema` and `source` an
    `Option<EntityRefSchema>`. This fits `L1392` with no ref collection (decision 2).
15. **Times are gameplay ticks (`u64`), not `SimTime`.** The `ludion-ai` `Timer` precedent: serializable, exact, and
    they survive restore because `GameClock` rides snapshots. Seconds convert with `ceil(x - x * 1e-6)`, a private copy of
    that rule (no `ludion-ai` dependency). A held `GameClock` stops `now`, so nothing is forgotten during a hold.
16. **`sensed_now` is true iff `last_sensed_tick == now`.** `perception_update` clears it on every record whose
    `last_sensed_tick < now`, so a `report_stimulus` before or after the update in one tick reads `true` until the next one.
17. **Sight refresh.** Each `visible` `SightSample` refreshes the record keyed `(perceiver, Sight, Some(target))`:
    `last_position` is the target's world origin, `zone` the lowest visible zone index that tick, `strength` 1,
    `last_direction` the normalized `new - old last_position` when the move exceeds 1e-4 m (else unchanged; `[0,0,0]` at
    creation). `first_sensed_tick` is set once.
18. **Forgetting and eviction run in `perception_update` after the refresh.** A record is despawned when
    `now - last_sensed_tick >= ticks(forget_s)`: sight uses `forget_sight_s`, hearing and custom `forget_hearing_s`. Then,
    per perceiver, while count > `max_records`, the least recently sensed goes, ties by `(sense, source)` ascending
    (`None` first). Records whose perceiver is dead or has no `memory` are despawned. A record whose `source` died is
    kept and ages out (the generational id never aliases). Spawns and despawns run in sorted key order.
19. **Anonymous stimuli (`source: None`) merge per `(perceiver, sense)`.** W3 adds the noise `kind` to the key, additively.
20. **Spawn ordering.** New records spawn in ascending `(perceiver, sense, source)` order after the tick's despawns, so
    entity ids are a pure function of the run. A perceiver without `memory` spawns nothing: W1 worlds are byte-identical.

## Waves

- **W1 (this):** sight — components, config, results, `perception_update`, `L1570`/`L1571`/`L1572`/`L4220`, docs.
- **W2 (shipped):** stimulus memory (`StimulusRecord` entities, `report_stimulus`, `stimuli`, `forget`/`forget_all`,
  despawn with the perceiver, snapshot rejoin).
- **W3:** hearing (`emit_noise`, `PendingNoise` on an engine entity, `NoiseZone`, occlusion via a `sound_occluder`
  convention, the surface-tag multiplier read from request 0009's tag, so W3 waits on 0009).
- **W4:** `period_ticks` stagger, ray budget and rotation cursor on a hashed `PerceptionState`, `L4221`, the
  `perception` trace channel, `engine.describe` counts, gallery `examples/gallery/perception.yaml`, the committed
  `WorldHash` trace, the bench (30 x 2 x 12 x 3).
- **W5:** debug draw category (rides request 0005), check-scene class and layer name checks.

## Out of scope (triggers recorded)

- A true cone shape: trigger is a game whose wedge misses at the edges.
- Nav-path sound occlusion: trigger is W3 landing with a game that needs it.

## Proposed — owner ruling owed

The gap-map pointer; the pitch sign (decision 3); the vertical default of 45 degrees; wedge versus cone; the default
ray budget 2048 and the bench target 0.5 ms (W4); no entity-ref collection (decision 2).

## Status

- **W1 mechanized (2026-11-18):** components (`Perceiver`, `SightTarget`, `SightZone`, `ZoneSweep`, `SightPoint`,
  `Require`, `Look`) appended to the builtin registry (62 components, both sim-only); `PerceptionConfig` /
  `PerceptionResults` / `perception_update` with `Engine::builder().perception`; tests in `crates/ludion-engine/tests/perception.rs`
  and the unit modules; `component_schemas.json` re-blessed (two components). No wire schema, trace, replay or golden moved.
- **W2 mechanized (2026-11-18):** `PerceiverMemory`, `Perceiver.memory`, `Sense` and the `StimulusRecord` component (63
  components, sim-only); `report_stimulus`, `stimuli`, `forget`, `forget_all`, `Stimulus` in `ludion-engine`; the memory
  pass inside `perception_update`; `L1573` / `L1574`; tests in `crates/ludion-engine/tests/perception_memory.rs`;
  `component_schemas.json` re-blessed. No wire schema, trace, replay or golden moved.
