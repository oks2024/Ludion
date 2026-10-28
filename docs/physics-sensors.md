---
description: Sensor colliders: trigger volumes with deterministic enter/exit events and occupants
---

# Sensor colliders

**Scope:** `Collider { sensor: true }`: a volume that reports which bodies are inside it and never touches them. Rules, the event stream, timing, spawn/despawn behaviour and the pickup pattern. `Collider`, contacts and queries are `ludion docs physics`; which pairs meet is `ludion docs collision-layers`.

## Format

A sensor is a `static` or `kinematic` 3D collider with `sensor: true` (default `false`, omitted from text when false):

```yaml
  - name: pickup_zone
    components:
      Transform: { translation: [4.0, 1.0, 0.0], rotation: [0.0, 0.0, 0.0], scale: [1.0, 1.0, 1.0] }
      Collider:
        shape: { box: { half_extents: [1.0, 1.0, 1.0] } }
        body: static
        sensor: true
```

## Rules

- `L1291` — a sensor on `body: rigid`: refused. A sensor never collides, so it has nothing to simulate. Use `static`, or `kinematic` and move it.
- `L1292` — a `mesh` or `heightfield` sensor: refused. Neither has an interior, so it could only fire on a surface crossing.
- `L1293` — `sensor: true` on a 2D `circle`/`aabb` the physics tier does not claim. `check-scene` only; sensors exist in the solver tier alone.
- `L1291`/`L1292` are raised at document time (`check-scene`) and again at runtime for a collider written by game code; a refused collider gets no body.
- A `compound` is allowed: its parts form one sensor and report one entity-level event.
- Toggling `sensor` on a live `Collider` is a rebuild, like any shape edit: occupants of a sensor that stops being one exit with `Removed`.

## What a sensor detects

- Every non-sensor body kind: `static`, `kinematic`, `rigid`. Sensor-versus-sensor is never reported.
- The `collision-layers` table applies unchanged: a pair either side ignores is never a sensor pair.
- A sensor creates no contact, no impulse and no wake-up, and it does not push. A sleeping occupant stays inside.
- Overlap is judged by volume at the poses the step produced.

## The event stream

`PhysicsWorld::sensor_events() -> &[SensorEvent]` lists the transitions the last step produced, each `{ sensor, other, phase }`: `sensor` is the entity whose `Collider` has `sensor: true`, `other` the non-sensor entity. `phase` is `Entered` or `Exited { cause }`; the three causes:

| `ExitCause` | Meaning |
|---|---|
| `Separated` | both alive, volumes no longer overlap (moved, teleported, reshaped) |
| `Despawned` | the sensor or the occupant was despawned; the event still names the dead id |
| `Removed` | both alive but the pair no longer qualifies: a `Collider` removed, the sensor stopped being one, the occupant became one, or a `layers:` edit now ignores the pair |

The list is sorted ascending by `(sensor, other)`, a pair appears once per step, and it holds exactly one step's events. There is no "stay" event: `sensor_occupants(sensor) -> &[EntityId]` answers who is inside as of the last step, ascending (empty for a non-sensor or unknown id). A frozen step (`ludion docs physics-time-scale`) emits no events and keeps the occupants. Not hashed; it rides the snapshot.

**Latency.** The physics triple runs after every game system, so a system reads tick `N - 1`'s step. A pose written on tick `N` (a kinematic included) is judged by tick `N`'s step and read as `Entered` on tick `N + 1`.

## Spawn, despawn, teleport, restore

- A body spawned inside a sensor, or a sensor spawned around a body, reports `Entered` on the first step that has both.
- A despawn on either side reports `Exited { cause: Despawned }` on the next step. Check the cause before reading the entity's components (`Events name ids that may be gone`, `ludion docs physics`).
- A teleport out reports `Separated`; in, `Entered`. A kinematic follows its pose, so a teleport crossing the volume reports whatever the end pose says.
- A snapshot carries the occupancy baseline, the last events and the not-yet-judged despawns: a restored world reports the same events the uninterrupted run would. Older physics blobs no longer decode.

## Discrete detection

There is no continuous collision detection: a body that crosses a sensor entirely within one tick is never reported. Keep the per-tick relative displacement (body against sensor) under the sensor's thickness along that axis, or thicken the volume.

## Queries

`ray_cast`, `shape_cast` and `overlap` skip sensors. `QueryFilter::new().include_sensors()` keeps them, for a query that wants zones.

## A pickup

```rust
fn collect(world: &mut World, _ctx: &SystemCtx) {
    let Some(physics) = world.resource::<PhysicsWorld>() else { return };
    let picked: Vec<EntityId> = physics
        .sensor_events()
        .iter()
        .filter(|event| event.phase == SensorPhase::Entered)
        .filter(|event| world.has::<Pickup>(event.sensor))
        .map(|event| event.sensor)
        .collect();
    for pickup in picked {
        // The next step reports `Exited { cause: Despawned }` for this pickup and the entity
        // that entered it; that event is ignored because only `Entered` is acted on.
        world.despawn(pickup).ok();
    }
}
```

## Gotchas

- A sensor-sensor pair is silent: a trigger zone cannot detect another zone. Put the detected volume on a non-sensor body.
- Reading `sensor_events` twice in one tick returns the same list; it is replaced only by the next step, so handle each event in exactly one system.
- A fast body can tunnel through a thin sensor (see Discrete detection).

## Next

- `ludion docs physics` — `Collider`, contact events, queries.
- `ludion docs collision-layers` — the table that filters sensor pairs.
- `ludion docs debug-draw` — the `colliders` category draws sensors in their own tint.
