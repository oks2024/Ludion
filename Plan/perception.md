# Plan: Perception — sight zones, sample points, batched line of sight

> **Waves 1–4 mechanized — sight, stimulus memory, hearing, the sight stagger and ray budget, and the `perception` trace channel ship; W5 is laid out below.**
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
21. **Surface multiplier is perception-owned data, keyed by surface name (W3).** Request 0009's `physics.surfaces`
    declares names only, so the scalar lives in `PerceptionConfig.surface_noise: Vec<SurfaceNoise { surface, multiplier }>`.
    `Noise.surface: Option<SurfaceId>` comes from the game (its footstep system has `MoveReport.ground.surface`) and is
    resolved through `PhysicsWorld::surface_name`; an unlisted surface or `None` is x1. This drops the dependency on
    0009 W2.
22. **`NoiseZone` has its own analytic shape (W3):** `shape: { box: { half_extents } } | { sphere: { radius } }` in the
    entity's world pose (box oriented by the rotation, scale ignored), no sensor collider and no physics read. Among the
    zones that contain `at` and whose `kinds` include the noise kind (empty = all) the highest `priority` wins, ties go to
    the lowest entity, and multipliers never stack.
23. **One entity per pending noise (W3).** `PendingNoise` (its `source` a marked `Option<EntityRefSchema>`) is the
    decision-14 rule again: no ref list. A singleton `PerceptionState { next_noise_id }` entity (hashed) holds the
    monotonic `NoiseId`. Both spawn lazily on the first `emit_noise`, so absent state is byte-identical. The budget
    cursor (W4) joins `PerceptionState`.
24. **Delivery order (W3):** noises in `NoiseId` order, listeners ascending. `perception_update` consumes and despawns
    every pending noise, in id order, so a noise emitted before the update in a tick lands that tick and one emitted
    after it the next.
25. **Occlusion (W3):** only when `noise.occlusion` and `sound_occluder_layers` is non-empty (an empty include set means
    everything hits, so it is skipped). One `ray_cast` from `at` to the ear against those layers counts distinct
    entities, less the source and the listener, capped at `max_occluders` (2); each multiplies the reach by
    `occlusion_factor` (0.5). The ray is skipped when the ear is beyond `radius x zone x surface x sensitivity`.
    The reach is `radius x zone x surface x sensitivity x occlusion_factor^occluders`, applied by repeated multiplication.
26. **Hearing memory (W3):** a delivery refreshes `(listener, Hearing, source)` for a listener with `memory`.
    `StimulusRecord.kind` (set on hearing records) joins the key **only when `source` is `None`**, so an anonymous noise
    merges per `(listener, Hearing, kind)` (the decision-19 addition). `strength` is the loudness, `last_position` the `at`.
27. **Config (W3):** `noise_kinds` (at most 32, the `classes` rules), `surface_noise`, `sound_occluder_layers`,
    `occlusion_factor` (in [0, 1]), `max_occluders` (1 to 8), all `#[serde(default)]`; `PerceptionConfig` loses `Eq`
    and `Default` is a hand impl. A bad one is `L4220`.
28. **Invalid noise (W3):** a non-finite or negative `at`/`radius`/`loudness` drops the noise in `emit_noise` with
    `L1577` (warning, once); an undeclared `kind` drops it with `L1572` (once per name). `L1575` bad `NoiseZone`,
    `L1576` bad `hearing`.
29. **`period_ticks: u32` on `Perceiver` (W4a)**, default 1 and skipped when 1; 0 is `L1570` at `/period_ticks`. A
    perceiver is **due** when it has never been evaluated and `tick % N == entity.index() % N` (the request's phase), or
    when `now - evaluated_tick >= N`. A deferred perceiver therefore stays due until served: one deferred before its
    first evaluation gets a status with `evaluated_tick = now - N`, so it is due every tick (its phase may slide) and
    `L4221` can measure its wait.
30. **Budget `max_sight_rays_per_tick: Option<u32>`, default `None` (unlimited).** `Some(0)` is `L4220` at
    `/max_sight_rays_per_tick`. The request's 2048 is the documented recommendation and stays Proposed as a default:
    any budget makes sight persisted hashed state (decision 31), which would reverse decision 11 in every perception
    world and move their hashes.
31. **Carried samples are hashed entities** whenever they can be needed: for a perceiver with `period_ticks > 1`, or
    for every perceiver when a budget is set ("persisting"). A derived resource cannot carry them (a `SnapshotDoc` field
    moves `replay_content_hash`; a ref list on a component breaks `L1392`). `PerceiverStatus { evaluated_tick }` goes on
    the perceiver's entity and carries no ref. One `SightSampleRecord` entity per persisted sample mirrors
    `SightSample` (`perceiver` and `target` marked refs, `blocker` a marked `Option`). A served perceiver's records are
    upserted by `(zone, target)`: vanished keys despawn, new keys spawn in ascending `(perceiver, zone, target)` after
    the despawns. Records and status of a perceiver that is dead, disabled or no longer persisting are removed.
32. **Rotation.** Due perceivers, sorted by id, start at the first id above `PerceptionState.sight_cursor` (marked
    `Option` ref, default none) and wrap. A perceiver's planned rays are its points in a wedge with a non-empty blocker
    set; a re-cast after a self-hit is not counted. It is served when it is the first this tick, or when
    `used + rays <= budget`. **The first one that does not fit stops the pass**: every later due perceiver is deferred,
    which keeps the rotation fair. The cursor becomes the last id served. `PerceptionState` is spawned lazily, through
    the `emit_noise` helper, only when a budget moves the cursor. A world with no budget and no stagger touches no
    record, status or state.
33. **Results.** A served perceiver gets fresh samples; a perceiver not served reads its records. Samples stay sorted by
    `(perceiver, zone, target)`. `zone_facing` is current for every perceiver every tick (no rays). **Only fresh samples
    refresh memory**, so `sensed_now` blinks off on a tick a guard is not evaluated (a docs gotcha).
34. **`L4221 PERCEPTION_DEFERRED`** (warning; `LINTS`, the `L1577` precedent): a due perceiver deferred while
    `now - evaluated_tick > 4 * period_ticks`; once per run (`report_full`, key `perception/deferred`).
35. **Counts.** `PerceptionResults::rays_cast()` (rays actually cast, re-casts included) and `deferred()` (due, not
    served, sorted). `engine.describe` gains `perception: { rays_cast, deferred }`, present once `PerceptionResults`
    exists (the `ai::describe_section` precedent).
36. **The trace is derived in the driver.** `trace()` reads `PerceptionResults` and the `StimulusRecord`s after each
    `SimStep` and diffs them against the previous tick's state, which it keeps in locals. Nothing is recorded sim-side
    (unlike `AiTrace`): the results are already a derived per-tick resource, so a sim hook would add nothing. It is
    unhashed, and it costs nothing when the channel is off.
37. **Line vocabulary** (`channel: "perception"`, then `event`; after the ai lines within a tick): `sight` (one line per
    `(perceiver, zone, target)` whose `visible` changed; absent means not visible; a key that dropped out of the results
    while visible prints `in_zone: false, visible: false` without sample fields), `noise` (one line per delivery, in
    `(listener, noise)` order, printed only on ticks where `results.tick()` advanced), `remember` / `forget` (a
    `StimulusRecord` that appeared or vanished, forgets first; a forget prints the last value seen; refreshes are not
    traced). Entities print through `trace::label`. The request asked for one line per *noise*; we print one per
    *delivery*, because the delivery is what a guard reacts to and the noise is already in `results.noises()`.
38. **Gallery placement.** `examples/gallery/perception.yaml` follows the `ragdoll-pose-blend` precedent: under a plain
    `Engine::builder()` (`ludion test`, the spelling sweep) it is a static scene and its golden is the tick-0 render.
    Perception runs only under the `perception_gallery` test builder, which installs `PerceptionConfig`, the intruder
    route and its footsteps. Unlike ai (decision 15 of `Plan/ai-behaviour.md`), every component in the scene is
    builtin, so the plain load succeeds.
39. **The bench reports and does not gate.** `perception_bench.rs` is `#[ignore]` and run in release. It prints the
    median and p95 of `perception_update` plus `rays_cast` at 1 and 8 threads and asserts only that the two `WorldHash`
    traces are identical. The 0.5 ms target stays Proposed. The request's game-side `tests/gameplay.rs` scripted route
    is the game's work.

## Waves

- **W1 (this):** sight — components, config, results, `perception_update`, `L1570`/`L1571`/`L1572`/`L4220`, docs.
- **W2 (shipped):** stimulus memory (`StimulusRecord` entities, `report_stimulus`, `stimuli`, `forget`/`forget_all`,
  despawn with the perceiver, snapshot rejoin).
- **W3 (shipped, Neon Shroud request 0001):** hearing (`emit_noise`, `PendingNoise` on an engine entity, `NoiseZone`,
  occlusion via `sound_occluder_layers`, the surface multiplier by name in `PerceptionConfig`; decisions 21-28).
- **W4a (shipped):** `period_ticks` stagger, ray budget and rotation cursor on a hashed `PerceptionState`, carried
  samples, `L4221`, `engine.describe` counts (decisions 29-35).
- **W4b (shipped):** the `perception` trace channel, gallery `examples/gallery/perception.yaml`, the committed
  `WorldHash` trace, the bench (30 x 2 x 12 x 3, 1 and 8 threads; decisions 36-39).
- **W5:** debug draw category (rides request 0005), check-scene class and layer name checks.

## Out of scope (triggers recorded)

- A true cone shape: trigger is a game whose wedge misses at the edges.
- Nav-path sound occlusion: trigger is W3 landing with a game that needs it.

## Proposed — owner ruling owed

`SurfaceSel::Under`: a `surface_under` convenience over request 0009 W2 so a game can name the surface at a point
instead of passing `MoveReport.ground.surface`; not built, the game passes the id today.

The gap-map pointer; the pitch sign (decision 3); the vertical default of 45 degrees; wedge versus cone; the default
ray budget 2048 (decision 30) and the bench target 0.5 ms (W4b); no entity-ref collection (decision 2).

## Status

- **W1 mechanized (2026-11-18):** components (`Perceiver`, `SightTarget`, `SightZone`, `ZoneSweep`, `SightPoint`,
  `Require`, `Look`) appended to the builtin registry (62 components, both sim-only); `PerceptionConfig` /
  `PerceptionResults` / `perception_update` with `Engine::builder().perception`; tests in `crates/ludion-engine/tests/perception.rs`
  and the unit modules; `component_schemas.json` re-blessed (two components). No wire schema, trace, replay or golden moved.
- **W2 mechanized (2026-11-18):** `PerceiverMemory`, `Perceiver.memory`, `Sense` and the `StimulusRecord` component (63
  components, sim-only); `report_stimulus`, `stimuli`, `forget`, `forget_all`, `Stimulus` in `ludion-engine`; the memory
  pass inside `perception_update`; `L1573` / `L1574`; tests in `crates/ludion-engine/tests/perception_memory.rs`;
  `component_schemas.json` re-blessed. No wire schema, trace, replay or golden moved.
- **W3 mechanized (2026-11-18):** `Hearing`, `Perceiver.hearing`, `NoiseZone` (+ `NoiseZoneShape`), `PendingNoise`,
  `PerceptionState` (three components appended, all sim-only) and `StimulusRecord.kind`; `Noise`, `NoiseId`,
  `emit_noise`, `HeardNoise`, `NoiseRecord`, `SurfaceNoise`, `PerceptionResults::heard`/`noises`, the hearing pass in
  `perception_update` and the generalized memory refresh; `L1575`/`L1576`/`L1577`; tests in
  `crates/ludion-engine/tests/perception_hearing.rs`; `component_schemas.json` re-blessed. No wire schema, trace, replay
  or golden moved.
- **W4a mechanized (2026-11-18):** `Perceiver.period_ticks`, `PerceptionConfig.max_sight_rays_per_tick`,
  `PerceiverStatus`, `SightSampleRecord` (two components appended, both sim-only; 75 in the registry),
  `PerceptionState.sight_cursor`; the rotation and persistence passes inside `perception_update`;
  `PerceptionResults::rays_cast`/`deferred`; the `perception` key of `engine.describe`; `L4221`; tests in
  `crates/ludion-engine/tests/perception_budget.rs` (incl. the byte-identity proof against a world that never ran the
  update); `component_schemas.json` re-blessed. No wire schema, `SnapshotDoc` schema, trace, replay or golden moved.
- **W4b mechanized (2026-11-18):** `TraceChannel::Perception` and `TraceRecord::Perception` (`sight`, `noise`, `remember`,
  `forget`; `PerceptionRecord` and four structs in `ludion-engine`), derived in `trace()` from `PerceptionResults`
  (`heard_all`, crate-private) and the `StimulusRecord`s, nothing sim-side (decisions 36-37); gallery scene
  `examples/gallery/perception.yaml` with `perception.trace.txt` and `goldens/windows/perception.png` (blessed on the
  RTX 3090 box; the macOS golden and the M4 reproduction of the trace are owed); tests
  `crates/ludion-engine/tests/perception_trace.rs`, `perception_gallery.rs` (committed trace, 1/2/8 threads, restore at
  tick 150, channel contents) and the ignored `perception_bench.rs`. Bench (release, 5950X + RTX 3090 box, 30 perceivers
  x 2 zones x 12 targets x 3 points, 1080 rays per tick): `perception_update` medians over four runs on a box shared
  with other builds: 0.68-1.21 ms at 1 thread, 0.64-0.96 ms at 8 threads (quietest run: 0.679 / p95 0.868 ms at 1,
  0.644 / p95 0.815 ms at 8); an 8-thread run is not reliably faster at this size. The 0.5 ms target stays Proposed
  and a quiet-box re-measure is owed. No wire schema, component
  schema, replay or existing golden moved.
