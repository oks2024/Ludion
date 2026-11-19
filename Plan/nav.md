# Plan: Navigation — grid, flow fields, agent steering

> **Mechanized — every subtask shipped; pinned bench figures and the vigil migration are owed.**
> Driven by the external FPS request 0007: navigation and crowd AI for 100–500
> agents, deterministic, with the 3D motor's cost at that count. Game-facing doc: `ludion docs nav`.

## Design decisions

1. **A layered 2.5D grid, sampled from static physics colliders only, not derived from voxels.**
   Kinematic and rigid bodies never shape the grid (a moving body would leave a stale hole and split
   a restored world's grid from the original). `ignore_layers` is optional. Any static collider is
   walkable ground, not only voxel rooms, and bridges and overhangs are extra layers in a column.
2. **Walkability is sampled per column**: a downward ray walk finds every floor, an upward ray
   measures headroom, and a capsule overlap of the agent's radius drops floors an agent cannot
   stand on. Columns are spread over `par`.
3. **Flying is steering only**: flyers seek in 3D and add separation, never read the grid.
4. **One shared bounded Dijkstra flow field per target node**, recomputed when the target changes
   cell or the grid's generation changes — 100 agents on one target cost one compute.
5. **A deterministic static diff, in entity order, drives a region rebuild**: an added, moved or
   removed static collider re-samples only the columns it touches, so rooms assembled at runtime
   stay walkable.
6. **Nav is derived state**: the grid and the fields stay out of `WorldHash` and snapshots; a
   restored world re-samples from its restored physics.
7. **Agents emit a velocity** (`NavSteering`); moving the body stays game or motor code, so the
   engine owns no movement model to guess at.
8. **One agent class per `Nav`**: radius, height, step and slope are the config's.
9. **The crate is `ludion::nav` at the facade root**: a prelude `nav` clashed with a game module.
10. **Point-to-point A* beside the flow fields** (Neon Shroud request 0004). `find_path` over the same links
    answers "path from A to B, reachable, how far, nearest walkable point"; fields stay for many agents
    chasing one target (decision 4 is unchanged). Both ends snap to the nearest node within a radius;
    a goal that cannot be reached is a partial path unless the caller turns that off. A routed agent is a
    `NavAgent` that also carries a **`NavGoal` component** — not a `NavAgent.goal` field as the request
    drew it: presence is the opt-in, `NavAgent` (shared and locked) stays untouched, and a scene with no
    `NavGoal` is byte-identical to today. A `NavGoal` takes precedence over `target`.
11. **A planned `NavRoute` is hashed sim state** — an exception to decision 6. The grid is derived and
    restarts after a restore; a route planned from the position the agent has *now* would start somewhere
    else than the uninterrupted run's route and diverge. The component exists only on an agent with a
    `NavGoal`, so goal-less scenes hash as before. All integer fields are `u32`.
12. **Replan triggers are pure functions of hashed state and the current grid**: the goal moved to another
    cell, or the leg being walked is no longer walkable (`walkable_line`). Never the grid's generation or
    `Nav::rebuilds()`, which restart with a restore. Planning is serial in ascending `EntityId`, at most
    `max_path_requests_per_tick` per tick (default 8), before the parallel steer; steering stays a pure
    function of start-of-tick state, so results are identical at any thread count.
13. **Patrols are `kind: patrol` documents walked as nav legs** (Neon Shroud request 0004 §3).
    - A stop has `at`, `wait`, `face_deg`, a `look` sequence and a `tag`; modes `loop`, `ping_pong`, `once`.
      A post is a one-point loop.
    - `patrol_update` (game-scheduled, between `nav_update` and `nav_steer`) writes the guard's `NavGoal`
      and counts waits and looks on gameplay time. `PatrolState` is hashed; the document cache is not.
    - **The engine outputs the facing and never turns the body:** `PatrolState.look_deg` is the world yaw to
      face (−Z forward). The request's `turn_rate_deg` is dropped; turning speed belongs to whoever turns.
    - Arrival is the agent's `NavRoute` reporting `arrived` for the stop's point, read before `nav_steer`:
      one tick of latency, deterministic. A partial route is not arrival.
    - A stop lasts `max(wait, Σ look.hold)`, one tick at least. Base yaw is `face_deg`, else the heading of
      the leg just walked, else `0`.
    - An unreachable stop is skipped (`L4143`, once per entity and point). Legs are nav paths, never
      splines; `follow: spline` waits for a rail-like patrol request.
    - `paused` hands the guard to behaviour code; unpausing rejoins by `resume` (`nearest`, `next`, `last`).
      A finished `once` patrol stays `done`.
14. **`NavLink` is a component that adds a directed edge between two grid nodes** (Neon Shroud request 0004 §4).
    Fields `from`, `to` (the entity's frame: posed by `world_affine` when it has a `Transform`, world
    coordinates otherwise), `bidirectional` (default `true`), `kind` (free text for the game), `cost` (metres
    *added* to the 3D distance between the two snapped nodes), `enabled`, `snap_radius` (default `0.75`); every
    field defaulted, so `{}` deserializes. **Differences from the request:** no `classes` field (wave 2 adds
    it); `kind` is free text, not an enum; the current link is reported on `NavRoute.link`, not
    `NavSteering.link` (decision 7 keeps `NavSteering` a velocity; decision 10 puts route state on `NavRoute`).
15. **The link table is derived** (decision 6 holds). `nav_update` rebuilds it every call after the grid, from the
    enabled `NavLink`s in ascending `EntityId`, snapping each end with `nearest_node(.., snap_radius)`. An end that
    snaps to nothing warns `L4145` once per link per grid build. Flow fields ignore links.
16. **Routes cross links without entity-ref lists.** `NavRoute` gains `links: Vec<u32>` (waypoint indices where a
    traversal starts; waypoint `i` is the link start, `i + 1` its end), `link: Option<EntityId>` (one ref, set only
    while `at_link`) and the status `at_link`. The link entity resolves from the table by its (start node, end
    node) pair, lowest entity first. A walker that reaches a link start stops with zero velocity and waits for
    `nav_link_done`; the engine never moves the body. A pending link that no longer resolves (disabled, removed,
    moved) replans under both `replan` modes; a route `at_link` is never replanned. Request amendment 2's link
    half, flow fields ignoring links and a zero velocity while `at_link` are **accepted by the owner 2026-11-19**.

**Known second way (temporary):** vigil keeps its game-side 2D nav (`games/vigil/src/nav.rs`).
Moving it to the layered grid moves its `WorldHash`; the exit is the owed migration, gated by
`games/vigil/tests/nav_equivalence.rs`, owing a pinned replay re-record and a human sitting on enemy
chase feel.

**Decision — blocked-target seeding.** vigil seeds a target whose own cell is blocked and lets a
body on a blocked cell read its lowest reached neighbour; `ludion-nav` does neither (no node means
the straight-line fallback). Decided: `ludion-nav` adopts both rules (the same `NEIGHBORS` order and
corner-cut), because radius inflation makes wall-flush targets routine and a straight line into a
wall is the failure the field exists to prevent. It lands before or with the migration, and
`nav_equivalence` extends to blocked targets.

## Status

Shipped, by commit subject: `ludion-nav` holds layered walkability and flow fields; the engine
builds a navigation grid from static physics colliders (`Nav`, `nav_update`, L4032); nav agents
steer through the shared flow fields (`NavAgent`, `NavSteering`, `nav_steer`, L1340); this bench and
`ludion docs nav`. Review fixes: static-only sampling, in-link flow search.

Bench (release, unpinned M5 laptop, 2026-11-07, indicative only; 64 × 64 m room, ramp, pillars,
chase target crossing a cell every 30 ticks): `nav_steer` 100 agents 0.03 ms at 1 thread and 0.04 ms
at 8, 500 agents 0.17 ms and 0.11 ms; one flow field over 4,096 nodes 0.11 ms; full grid build
0.79 ms, region rebuild 0.16 ms; 100 `CharacterMotor`s fed by `NavSteering` 0.71 ms, 500 motors
3.8 ms at 1 or 8 threads (the motor is serial; whole tick 4.2 ms). The 1- and N-thread `WorldHash`
traces are identical. Pinned figures owed.

Wave 1a (request 0004), by commit subject: `ludion-nav` gains `find_path` (A* over the flow field's links,
checked equal to its Dijkstra on 1,000 pairs), `walkable_line`, `string_pull` and `nearest_node`; the engine
gains `nav_path`, `nav_distance`, `nav_reachable`, `nav_project` and routed agents (`NavGoal`, `NavRoute`,
`NavConfig::max_path_requests_per_tick`). Tests pin the pure rules: a goal agent walks around a wall and
arrives, the goal moving a cell or a wall across the leg replans (and `replan: never` does not), a cap of 8
defers two of ten agents one tick, 1- and 8-thread traces match, and a mid-route restore rejoins. Only the
component schema snapshot moved.

Wave 1b (request 0004 §3), by commit subject: patrols — `kind: patrol` documents, `PatrolFollower` and
`PatrolState`, `patrol_update` and `patrol_motion`; `L1522`–`L1525`, `L4142`–`L4144`.

Wave 3 (request 0004 §4), by commit subject: `ludion-nav` gains `NavLinks`, `find_path_linked` and
`string_pull_linked`; the engine gains `NavLink`, `NavRouteStatus::AtLink` with `NavRoute.links`/`link`, and
`nav_link_done`; `L1526`, `L4145`, `L4146`. A scene with no `NavLink` routes and hashes as before. Only the
component schema snapshot moved.

## Waves (Neon Shroud request 0004)

The request asks for paths, patrols, off-mesh links, dynamic blockers and avoidance. Each wave is
independently shippable; later ones add fields and components, never reshape earlier ones.

- **1a — paths (shipped):** `find_path`, `string_pull`, `walkable_line`, `nearest_node`; `nav_path`,
  `nav_distance`, `nav_reachable`, `nav_project`; `NavGoal` and `NavRoute`; `L4140`, `L4141`, `L1520`, `L1521`.
- **1b — patrols (shipped):** `kind: patrol` documents (`ludion-scene`), `PatrolFollower`/`PatrolState`,
  `patrol_update`, `patrol_motion`; `L1522`–`L1525`, `L4142`–`L4144`; `ludion docs patrol`, example `patrol`
  (request §3). Amends `Plan/character-controller.md` decision 9 (patrols leave `kind: path`) — the Tech Lead
  ratified it. No gallery scene: `ludion render` runs no game systems, so a patrol would never move there.
- **2 — agent classes:** a per-class grid (radius, height, step, slope) and `PathRequest.class`. Amends
  decision 8 (one class per `Nav`); the trigger is met (walkers and forklifts) — **accepted 2026-11-19, scheduled
  (R13)**. Also `nav_random_point`, `nav_raycast` and the `nav` trace channel.
- **3 — off-mesh links (shipped):** `NavLink`, `NavRouteStatus::AtLink`, `nav_link_done`; `L1526`, `L4145`, `L4146`
  (decisions 14–16).
- **4 — dynamic blockers:** a `NavBlocker` overlay and `AtBlocker`, without re-sampling statics. Amends
  decision 1 (statics only) — **accepted 2026-11-19, scheduled (after wave 2)**.
- **5 — avoidance:** `NavAgent.avoidance` and `NavObstacle`; brings "Avoidance beyond separation" into
  scope and subsumes Serious Isaac draft 0029 — **accepted 2026-11-19, scheduled (after wave 4)**.

**Open questions, answered.** Planning is synchronous with a per-tick cap, not async (the cap bounds the
cost and keeps the result a function of the tick). Waypoint entity references wait for wave 3: `NavRoute`
keeps plain points. `nav_random_point`, `nav_raycast` and the `nav` trace channel wait for wave 2.

## Explicitly out of scope (triggers recorded)

- **Several agent classes per world** — trigger: a game with two agent sizes (met: wave 2).
- **A 3D flying field** — trigger: flyers stuck behind geometry in play.
- **Avoidance beyond separation** — wave 5.
- **Links in flow fields** — trigger: a chasing crowd that must climb.
- **Voxel-derived walkability** — trigger: a voxel room the collider sampler misreads.
