# Plan: Navigation — grid, flow fields, agent steering

> **Mechanized — every subtask shipped; pinned bench figures and the vigil migration are owed.**
> Driven by the external FPS request 0007: navigation and crowd AI for 100–500
> agents, deterministic, with the 3D motor's cost at that count. Game-facing doc: `ludion docs nav`.

## Design decisions

1. **A layered 2.5D grid, sampled from static physics colliders only, not derived from voxels.**
   Kinematic and rigid bodies never shape the grid (a moving body would leave a stale hole and split
   a restored world's grid from the original). Ruled 2026-11-19: authored `NavBlocker`s overlay the
   sampled grid without re-sampling (decision 20). `ignore_layers` is optional. Any static collider is
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
8. **Several agent classes per `Nav`** (Neon Shroud request 0004 §1; owner ruling 2026-11-19, amends "one class per
   `Nav`"). `NavConfig.classes` lists up to 8 `NavClass`es (name, radius, height, step, slope); each gets its own grid,
   flow cache and link table, all derived (decision 6 holds). An empty `classes` is one implicit class named `""`
   built from the legacy `agent_*`, `max_step` and `max_slope_deg` fields, so a config without `classes` builds
   today's grid. `NavAgent.class`, `PathRequest.class` and `NavLink.classes` pick a class by name. `NavAgent` and
   `PathRequest` lose `Copy`, and `nav_distance`, `nav_reachable` and `nav_project` gain a `class` parameter
   (owner-accepted breaks).
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
17. **Classes share one down-walk.** Each column does one floor down-walk and one headroom ray; every class adds its
    own capsule overlap and derives its own height, step and slope from the shared samples. Sampling order is
    unchanged, so a single class samples exactly as before. The class count is at most 8; `dirty_rect` pads by the
    widest class radius.
18. **A class is chosen by name.** Empty is the first class. An unknown name is `L4147`: a query returns it as an
    error; `nav_steer` leaves a walker of that class standing still (zero velocity, no route write) and reports it
    once per entity; a `NavLink.classes` entry naming no class reports it once per link. `NavLink.classes` empty
    permits every class, and a link is in a class's table only when both ends snap on that class's grid; `L4145`
    fires only when an end snaps on no permitted class.
19. **`nav_raycast` and `nav_random_point` are class-aware queries.** `nav_raycast` returns the first point a
    straight walk leaves the class's grid (`None` when the whole line is walkable). `nav_random_point` is uniform
    over the nodes within `radius` metres of path distance (grid links, no off-mesh links) from `around`, in
    ascending `NodeId`, with one `gen_range_usize` draw.

20. **The blocker overlay is derived, per class grid, rebuilt every `nav_update`** from the enabled `NavBlocker`s in
    ascending `EntityId`. A node is covered when its xz lies inside the posed box inflated by the class radius and
    the agent's body `[y, y + height]` overlaps the box's vertical extent. Precedence: block > interact > cost. An
    overlay change bumps the grid generation, so flow fields recompute and a moving blocker costs one field rebuild
    per tick. Replans never read the generation (decision 12 holds). The overlay rewrites the grid's link arrays
    from pristine base copies, so A*, fields, line walks and the debug grid respect it without hot-loop edits.
21. **Modes.** `block` nodes cannot be entered by A*, fields, line walks or off-mesh edges. `cost` adds `cost` metres
    on each step that enters a cost node from a node without cost: once per crossing. `interact` nodes are
    passable for A* and blocked for flow fields (a chaser does not open doors). A string pull never walks into an
    interact node: the crossing is its own leg, as a link is.
22. **Routes stop at interact blockers.** `NavRoute.blockers: Vec<u32>` holds the waypoint indices `i` whose next leg
    enters an interact region. While the region at waypoint `i + 1` still resolves to an interact blocker, the
    walker at `i` stands still `at_blocker` with `NavRoute.blocker = Some(entity)` (lowest entity). When the game
    disables it or switches it to `cost`, the walker continues the next tick. There is no done call. If the region
    becomes `block`, the route replans.
23. **Replan triggers with blockers.** Under `replan: on_change`, and only when the class overlay is non-empty, every
    remaining leg is checked: a leg that is no longer walkable replans. Legs that start at a link or blocker index
    are skipped. A walkable check fails on entering a blocked node or entering an interact node from outside. A
    `cost` toggle never replans; it prices the next plan. With no blockers the current-leg rule is unchanged.
24. **Opt-in ORCA for walkers** (Neon Shroud request 0004 §6; owner ruling 2026-11-19, brings "Avoidance beyond
    separation" into scope and subsumes Serious Isaac draft 0029). `NavAgent.avoidance` swaps the separation push
    for an ORCA velocity in the xz plane (a 2D linear program, no sampling, no iteration count to tune), capped at
    `speed`. An agent without it steers exactly as before. Walkers only; a flyer with `avoidance` is rejected
    (`L1528`). A route `at_link`/`at_blocker` stands still with no avoidance, as it does with separation.
25. **Inputs are start-of-tick hashed state.** Neighbour agents: the `k_nearest` neighbours on `neighbor_layers` (at
    most `max_neighbors`, in `(distance², handle)` order) that are walking `NavAgent`s, each with its previous
    `NavSteering.velocity` and its radius (`avoidance.radius`, else its class radius). Obstacles: every enabled
    `NavObstacle` within range, in ascending entity order after the agents, each with its `NavObstacleState.velocity`.
    Other index items are ignored; a neighbour whose `|dy|` exceeds the agent's class height is ignored. The query
    radius is `radius + max_body_radius + speed · horizon_s`. Each agent is a pure function of this state, computed
    in the existing parallel phase, so results do not depend on the thread count.
26. **Responsibility and priority.** A neighbour with avoidance and equal `priority` shares the avoidance (½ each).
    A higher-priority neighbour, an agent without avoidance, or an obstacle leaves the agent the whole avoidance (1).
    The agent takes none toward a lower-priority neighbour with avoidance. Equal priority is symmetric, so no entity
    tie-break is needed; neighbour order already ties by handle.
27. **Obstacle motion.** `NavObstacleState { position, velocity }` is engine-written and hashed, like `NavSteering`.
    In `nav_steer`'s serial pre-pass each `NavObstacle` (ascending entity) gets
    `velocity = (pos − state.position) / ctx.dt` (zero on the first tick or when `dt` is 0), then `position = pos`.
    Its body can be kinematic, motor-driven or game-moved: only its pose is read.
28. **Composition with the grid.** An avoided velocity is kept only when the straight walk one `cell_size` along it
    stays on the agent's class grid (`walkable_line`, which sees blocker overlays). Otherwise the agent takes its
    seek velocity. The default radius is the class radius.

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
Wave 2a (request 0004 §1, §2), by commit subject: agent classes — `NavConfig.classes` and `NavClass`, one grid, flow cache and
link table per class from one shared down-walk, `NavAgent.class`, `PathRequest.class`, `NavLink.classes`, a `class` argument on
`nav_distance`, `nav_reachable` and `nav_project`, and the class-aware `nav_raycast` and `nav_random_point` (`ludion-nav` gains
`line_exit` and `reachable_within`); `L4147`; `ludion docs nav` §Classes and example `nav_class`. `NavAgent` and `PathRequest` lose
`Copy`. A config with no `classes` builds the same grid and routes; tests pin a single class equal to the legacy fields,
1- and 8-thread grids and traces, and the gap, ramp, link and unknown-class cases. Only the component schema snapshot moved.

Wave 4 (request 0004 §5), by commit subject: dynamic blockers — `ludion-nav` gains `NavOverlay`, `NodeMark`,
`NavGrid::set_overlay` (links rewritten from pristine base copies; a change bumps the generation) and
`string_pull_marked`; the engine gains `NavBlocker` (`block`, `cost`, `interact`; per class, box, posed), the
derived blocker table, `NavRouteStatus::AtBlocker` with `NavRoute.blockers`/`blocker`, and `NavPath.blockers`;
`L1527`; `ludion docs nav` §Blockers and example `nav_blocker`. A world with no `NavBlocker` builds the same
grids, routes and hashes. Tests pin the overlay rules (empty is identity, block, cost once per crossing, stop,
A* equals Dijkstra under costs) and the two-room cases: block, cost detour, interact wait and open, interact to
block replan, toggle scope, classes, a moving blocker, 1- and 8-thread traces and a mid-wait restore. Only the
component schema snapshot moved. An `at_blocker` walker also replans when its goal moves to another cell
(an `at_link` walker never does).

Wave 5a (request 0004 §6), by commit subject: agent avoidance — `ludion-nav` gains `avoid_velocity` (ORCA in the xz
plane with a fallback program for infeasible sets), `NavAgent.avoidance` (`NavAvoidance`: `radius`, `horizon_s`,
`max_neighbors`, `priority`) replaces separation for walkers, composing with agent classes (the class radius is the
default) and blockers (a velocity that leaves the class grid falls back to the seek); `L1528`; `ludion docs nav`
§Avoidance and example `nav_avoidance`. A world with no `avoidance` steers and hashes as before. Tests pin the pure
rule (no neighbours, head-on swap, static neighbour, responsibility 0, order independence) and the engine cases:
head-on pass, priority, classes, a blocker wall, identity for non-avoiders, 1- and 8-thread traces and a mid-crossing
restore. Only the component schema snapshot moved. A 1% right-hand bias on the preferred velocity breaks the exact
head-on symmetry deterministically (each agent steers to its own right, so a pair parts to opposite sides).

Wave 5b (request 0004 §6), by commit subject: `NavObstacle` — `NavObstacle { radius, enabled }` and the engine-written,
hashed `NavObstacleState { position, velocity }`. `nav_steer`'s serial pre-pass reads every obstacle's pose in
ascending entity order (enabled or not) and sets the velocity from the last step's motion (zero on the first tick); an
avoiding agent treats each enabled obstacle in reach (xz within the query radius, `|dy|` within the class height) as a
neighbour with responsibility 1, after the agents; separation ignores obstacles. `L1528` covers a non-positive radius;
`ludion docs nav` §Avoidance > Obstacles; the `nav_avoidance` example drives a forklift through the crossing. Tests:
yield with the state velocity pinned, a disabled obstacle that keeps tracking, 1- and 8-thread traces, a mid-pass restore,
identity for separation-only crowds with a far obstacle and the `L1528` cases. A world without `NavObstacle` is
unchanged; only the component schema snapshot moved. The obstacle's velocity comes from its pose, not physics `Velocity`.

## Waves (Neon Shroud request 0004)

The request asks for paths, patrols, off-mesh links, dynamic blockers and avoidance. Each wave is
independently shippable; later ones add fields and components, never reshape earlier ones.

- **1a — paths (shipped):** `find_path`, `string_pull`, `walkable_line`, `nearest_node`; `nav_path`,
  `nav_distance`, `nav_reachable`, `nav_project`; `NavGoal` and `NavRoute`; `L4140`, `L4141`, `L1520`, `L1521`.
- **1b — patrols (shipped):** `kind: patrol` documents (`ludion-scene`), `PatrolFollower`/`PatrolState`,
  `patrol_update`, `patrol_motion`; `L1522`–`L1525`, `L4142`–`L4144`; `ludion docs patrol`, example `patrol`
  (request §3). Amends `Plan/character-controller.md` decision 9 (patrols leave `kind: path`) — the Tech Lead
  ratified it. No gallery scene: `ludion render` runs no game systems, so a patrol would never move there.
- **2a — agent classes (shipped):** `NavConfig.classes`, `NavAgent.class`, `PathRequest.class`, `NavLink.classes`,
  `nav_raycast`, `nav_random_point`; `L4147`; `ludion docs nav` §Classes, example `nav_class` (decisions 8, 17–19).
  Amends decision 8 (one class per `Nav`); the trigger is met (walkers and forklifts) — **accepted 2026-11-19**.
- **2b — the `nav` trace channel:** nav records on the trace surface; it touches the shared trace code, so it is
  its own subtask after 2a.
- **3 — off-mesh links (shipped):** `NavLink`, `NavRouteStatus::AtLink`, `nav_link_done`; `L1526`, `L4145`, `L4146`
  (decisions 14–16).
- **4 — dynamic blockers (shipped):** a `NavBlocker` overlay and `AtBlocker`, without re-sampling statics; `L1527`
  (decisions 20–23). Amends decision 1 (statics only) — **accepted 2026-11-19**.
- **5a — agent avoidance (shipped):** `NavAgent.avoidance`, ORCA in `ludion-nav`; `L1528` (decisions 24–26, 28).
  Brings "Avoidance beyond separation" into scope and subsumes Serious Isaac draft 0029 — **accepted 2026-11-19**.
- **5b — `NavObstacle` (shipped):** non-agent moving bodies as avoidance obstacles (decisions 25, 27).

**Open questions, answered.** Planning is synchronous with a per-tick cap, not async (the cap bounds the
cost and keeps the result a function of the tick). Waypoint entity references wait for wave 3: `NavRoute`
keeps plain points. `nav_random_point` and `nav_raycast` shipped with wave 2a; the `nav` trace channel is wave 2b.

## Explicitly out of scope (triggers recorded)

- **A 3D flying field** — trigger: flyers stuck behind geometry in play.
- **3D avoidance for flyers** — trigger: flyers colliding in play.
- **Static-edge ORCA lines (walls as constraints)** — trigger: avoiding agents clip corners in corridors.
- **Links in flow fields** — trigger: a chasing crowd that must climb.
- **Voxel-derived walkability** — trigger: a voxel room the collider sampler misreads.
- **A door that one class may open and others not** (per-class `mode` on one blocker) — trigger: a game asks.
