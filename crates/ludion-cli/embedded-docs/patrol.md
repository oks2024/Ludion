---
description: The kind: patrol document, PatrolFollower and patrol_update: guards walking nav legs
---

# Patrols

**Scope:** a guard route authored as text (`kind: patrol`), the `PatrolFollower` that walks it as nav
legs, and the facing it exposes. Pathing is `ludion docs nav`; the behaviour that pauses a patrol is
`ludion docs ai`; a spline ride is `ludion docs path`; gameplay time is `ludion docs game-clock`.

## The loop

1. Write `patrols/yard.patrol.yaml` under the content root (format below).
2. `ludion check-scene patrols/yard.patrol.yaml` — `L1522`/`L1523` with JSON Pointers.
3. Give the guard `NavAgent: {}` and `PatrolFollower: { route: patrols/yard.patrol.yaml }`.
4. Schedule `nav_update`, `patrol_update`, `nav_steer`, your mover, in that order.
5. Read `patrol_motion(world, guard)` each tick and turn the body to `look_deg` yourself.

## Format

```yaml
format: 1
kind: patrol
name: yard                  # optional
generated_from: gen/yard.py # optional provenance, no drift gate
mode: loop                  # loop | ping_pong | once (default loop)
points:
  - at:
      - 0.0
      - 0.0
      - 0.0
    wait: 1.5               # seconds, default 0
    face_deg: 90.0          # world yaw while stopped; default: the leg's heading
    look:                   # played from the stop's start, offsets from the facing
      - yaw_deg: -45.0
        hold: 0.5
      - yaw_deg: 45.0
        hold: 0.5
    tag: door               # free label for the game
```

Block sequences only (`at:` then three `- n` lines). A stop lasts `max(wait, Σ look.hold)`; after the
last look the facing returns to the base yaw: `face_deg`, else the yaw of the leg just walked, else
`0`. Yaw is degrees with −Z forward, so travel along +x is −90. A one-point `loop` is a post: it
repeats its look cycle. Canonical text always spells `mode`.

| Code | Raised for |
|---|---|
| `L1522` | empty/non-mapping root, envelope, wrong type, unknown key (nearest suggested), bad `mode`, `at` not three numbers, a look missing `yaw_deg` or `hold` |
| `L1523` | no points, a non-finite or out-of-f32 number, negative `wait`, a look `hold` of 0 or less, an empty `tag` |
| `L1524` | a `PatrolFollower.route` that is absolute or climbs out with `..` |
| `L1525` | a `PatrolState` timer negative or non-finite, a non-finite `look_deg` |

## Components

- `PatrolFollower`: `route` (`""` is inert), `start` (stop to head for first, `0`), `paused` (`false`),
  `resume` (`nearest`, `next` or `last`).
- `PatrolState`, **written by `patrol_update`**, hashed: `point`, `forward`, `phase` (`walking`,
  `stopped`, `paused`, `done`), `stop_left`, `stop_elapsed`, `look_deg`, `arrived` (true only on the
  tick a stop began). A scene without a follower has none, so its hash is unchanged.

## Rules

- **Gameplay time.** Waits and looks count `game_dt`; a `GameClock` hold freezes `stop_left`.
- **Arrival latency.** `patrol_update` reads the agent's `NavRoute` before `nav_steer` runs, so a stop
  begins one tick after the agent got there. A route that only reaches part-way (`partial`) is not
  arrival.
- **The engine outputs the facing and never turns the body.** `look_deg` is `None` while walking.
- **Unreachable stops are skipped**, once (`L4143` per entity and point), so one bad point never
  freezes a patrol. A route that fails to load is `L4142` and the guard is left alone; no `NavAgent`
  is `L4144`.
- **Pause.** While `paused`, `patrol_update` leaves the guard and its `NavGoal` to your code. On
  unpause it rejoins by `resume`; a finished `once` patrol stays `done`.
- **Library.** Documents load lazily through the content root, are never hashed or snapshotted, and a
  rebuilt or restored world reloads them from disk. `asset.reload` does not refresh a loaded patrol.

## Gotchas

- Legs are nav paths, never splines; a stop must be on walkable ground.
- `patrol_update` is game-scheduled: without the system a `PatrolFollower` does nothing.
- Order matters: after `nav_update`, before `nav_steer`.

## Next

- `ludion docs nav` — the grid and routes the legs use.
- `ludion docs components` — the `PatrolFollower` and `PatrolState` rows.
- `cargo run -p ludion --example patrol` — a loop around a wall and a post that looks around.
