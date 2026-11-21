---
description: Navigation — walkability grid, agent classes, flow fields, paths, NavLink, NavAgent steering
---

# Navigation: a grid from the colliders, one flow field per target, agents that steer

**Scope:** `NavConfig`, `Engine::nav`, `nav_update`, `NavAgent`, `NavSteering`, `nav_steer`, `NavGoal`, `NavRoute`,
`NavLink`, `NavBlocker`, `nav_link_done`, `nav_path` and its queries, `NavClass`. The
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
| `agent_radius` | `0.4` | finite, `> 0`; a floor is dropped where a capsule this wide collides (ignored when `classes` is set) |
| `agent_height` | `1.8` | finite, `> 0`; the headroom a floor needs (ignored when `classes` is set) |
| `max_step` | `0.4` | finite, `>= 0`; largest step on flat ground, slopes widen it (ignored when `classes` is set) |
| `max_slope_deg` | `45` | in `(0, 90)` (ignored when `classes` is set) |
| `classes` | `[]` | at most 8 `NavClass { name, radius, height, max_step, max_slope_deg }`, names non-empty and unique; see Classes |
| `flow_radius` | `48` | finite, `> 0`; how far a field reaches from its target |
| `ignore_layers` | `[]` | names declared in `physics.layers`, none empty (Rust type `LayerSet`; text is a list) |
| `max_path_requests_per_tick` | `8` | `>= 1`; routes `nav_steer` plans per tick |

A bad value is `L4032` with a JSON Pointer to the field; `ludion explain L4032`.

`NavAgent` fields and defaults: `ludion docs components`. `class` is `""` (the first class), `target` is `[0,0,0]`, `speed` `4`,
`flying` `false`, `arrive_radius` `0.5`, `separation_radius` `0.75`, `separation_strength` `2`,
`separation_neighbors` `4` (at most 16), `neighbor_layers` all bits (`0` turns separation off).
A negative or non-finite field, or too many neighbours, is `L1340`. `NavSteering { velocity }` is
what `nav_steer` writes each tick.

## Rules

- **Agent size belongs to the config, not the agent.** One class by default; several with `classes` (below).
- **Only static colliders shape the grid**, so motors, props and the player never block it;
  `ignore_layers` optionally drops static layers that are not ground or obstacles.
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
- **A `NavGoal` takes precedence over `NavAgent.target`**: a walker with one follows a planned route
  (below), not a flow field. Remove the `NavGoal` and the agent chases `target` again.
- **Separation** reads the `Index3d`: give agents a `SpatialVolume` whose layer is in
  `neighbor_layers`.
- **Thread-count invariant**: `nav_steer` fans out over `par`; the result is bit-identical at any
  count (`ludion devdocs sim-parallelism`).

## Paths

Flow fields serve many agents chasing one target; a path serves one question from A to B. Call these
after `nav_update` has built the grid (before it: `L4140`):

```rust
let req = PathRequest { max_cost: 60.0, ..PathRequest::new(from, to) };
let path = nav_path(world, &req)?;              // NavPath { status, cost, waypoints, expanded }
let metres = nav_distance(world, from, to, "")?;   // Option<f32>: None when unreachable
let ok = nav_reachable(world, from, to, "")?;
let on_floor = nav_project(world, p, "", 2.0)?;   // Option<[f32; 3]>: nearest walkable point
```

The `""` is the agent class (the first class); `PathRequest.class` is the same field.

- `status` is `Complete`, `Partial` (a path to the reachable node nearest the goal) or `Unreachable`;
  `partial: false` turns `Partial` into `Unreachable`.
- `waypoints` are the corners a straight-line walker needs (A* plus string pulling), start first.
  A complete path ends at the goal's own x and z when that cell is walkable.
- Both ends snap to the nearest node within `snap_radius` (default 1 m); an end with none is
  `Unreachable`. `max_cost` and `max_expanded` (default 20000) bound the search.
- A bad request (non-finite point, negative `max_cost` or `snap_radius`, zero `max_expanded`) is
  `L4141` with a pointer; the queries are pure and change nothing.

## Classes

One world can hold several agent sizes. `NavConfig.classes` lists up to 8 `NavClass`es; the grid is sampled
once per column (floors and headroom shared) and each class adds its own capsule test and its own height, step
and slope, giving one grid, flow cache and link table per class (all derived):

```rust
let config = NavConfig {
    classes: vec![
        NavClass { name: "walker".into(), radius: 0.3, max_slope_deg: 40.0, ..NavClass::default() },
        NavClass { name: "forklift".into(), radius: 1.0, max_slope_deg: 10.0, ..NavClass::default() },
    ],
    ..NavConfig::default()
};
```

- A class is chosen by name: `NavAgent: { class: forklift }`, `PathRequest { class: "forklift".into(), .. }`, the
  `class` argument of the queries. An empty name is the first class. With no `classes` there is one class, `""`,
  built from `agent_radius`, `agent_height`, `max_step` and `max_slope_deg`; a config without `classes` builds the
  same grid it always did.
- An unknown name is `L4147`: a query returns it as an error, `nav_steer` leaves that walker standing still (zero
  velocity) and reports it once. Flyers ignore `class`.
- `NavLink.classes: [walker]` limits a link to those classes (empty is all). A link is in a class's table only when
  both ends snap on that class's grid; `L4145` fires only when an end snaps on no permitted class. A `classes`
  entry naming no class is `L4147` once per link.
- `nav_raycast(world, from, to, class)` returns where a straight walk first leaves the class's grid (`None` when the
  whole line is walkable). `nav_random_point(world, around, radius, class, &mut rng)` is a uniform pick among the
  nodes within `radius` metres of walking distance of `around` (grid links only), one `gen_range_usize` draw.
- The `nav` debug view draws the first class.

## Goals and routes

Give a walker a `NavGoal { at }` (`replan: on_change` default, or `never`) beside its `NavAgent`.
`nav_steer` plans a route, stores it as a `NavRoute` (`waypoints`, `index`, `status`, `partial`, `cost`)
and steers along it; a game reads `NavRoute.status`: `planning` (waiting for the per-tick cap),
`following`, `arrived` (within `arrive_radius` of the last waypoint), `unreachable`, `at_link` or `at_blocker`.

- **Replanning is pure**: when the goal moves to another cell, or, under `on_change`, when the leg being
  walked (the stored waypoint pair, not where the agent stands) is no longer walkable (a static wall
  appeared). A goal that moves inside its cell is followed
  without planning. `unreachable` and partial routes stay until the goal changes cell.
- **At most `max_path_requests_per_tick` routes are planned per tick**, in ascending entity order; the
  rest stand in `planning` until the next tick.
- **`NavRoute` is hashed state, written by `nav_steer`; never insert one.** A restore resumes the route
  instead of planning again. An agent with no `NavGoal` has none, so such scenes hash as before.
- A flyer with a `NavGoal` seeks `at` in 3D and gets no route. Routed agents skip the flow field, and
  separation is added as for any agent.

## Links

A `NavLink { from, to, bidirectional, kind, cost, enabled, snap_radius }` entity adds an edge a walker
cannot walk: a ladder, a vault, a door. `from` and `to` are in the entity's frame (its `Transform` poses
them), each snapped to the nearest node within `snap_radius` (`0.75`); an end that finds none leaves the
link out and warns `L4145`. The edge costs the 3D distance between the nodes plus `cost` metres; `kind` is
free text for your game.

`nav_path` and routed agents plan across links (`NavPath.links` lists the waypoints where a traversal
starts). A walker that reaches a link start stops: `NavRoute.status = at_link`, `NavRoute.link = Some(link
entity)`, velocity exactly zero. **You play the traversal and move the body to the link's end, then call
`nav_link_done(world, agent)`**; the route continues from there (`L4146` if the agent is not `at_link`).
The engine never moves the body.

- Disabling or removing a link, or moving its ends off the nodes, replans the routes ahead of it (in both
  `replan` modes). An `at_link` route is never replanned until `nav_link_done`.
- The link table is derived and rebuilt every `nav_update`. Flow fields (agents with only a `target`) ignore
  links. A scene with no `NavLink` plans and hashes as before.

## Blockers

A `NavBlocker` entity closes, prices or gates part of the grid without re-sampling physics: a door, a
pushed crate, a mud patch. Every `nav_update` overlays the enabled blockers on each class grid.

```yaml
components:
  Transform:
    translation:
      - 12.0
      - 1.0
      - 12.5
  NavBlocker:
    shape:
      box:
        half_extents:       # default 0.5 each, before scale
          - 0.6
          - 1.0
          - 1.5
    mode: interact          # block (default) | cost | interact
    cost: 0.0               # metres, for `cost`
    classes:                # empty is every class
      - walker
    enabled: true
```

A node is covered when its position, in the box's frame, lies inside the half-extents inflated by the class
radius and the agent's body (`y` to `y + height`) overlaps the box vertically. A non-uniform scale is
approximated per axis. A moving blocker (a carried crate, a closing door) is followed every tick, at the
price of one flow-field rebuild per tick for the classes it touches. Where blockers overlap: block beats
interact beats cost.

- **`block`**: nothing enters the covered nodes (A*, fields, line walks).
- **`cost`**: entering a covered node from outside costs `cost` extra metres, **once per crossing**, so A*
  detours only when another way is cheaper. Switching a blocker to or from `cost` prices the next plan; it
  never replans a route by itself.
- **`interact`**: A* routes straight through, but the walker stops before the region with
  `NavRoute.status = at_blocker` and `NavRoute.blocker = Some(door entity)` (the lowest covering entity),
  velocity exactly zero. **You open it by disabling the blocker or switching it to `cost`; the walker
  continues the next tick.** There is no done call. If the blocker becomes `block` the route replans. Flow
  fields treat `interact` as blocked: a chaser does not open doors.
- **Replanning**: under `replan: on_change`, with a blocker on the grid, every remaining leg is checked, so a
  door that closes anywhere ahead replans the route (legs that start at a link or blocker are skipped). A
  route in another room is untouched.
- A `classes` name no class has is `L4147`, once per blocker; bad half-extents, a negative `cost` or an empty
  `classes` entry is `L1527`. A scene with no `NavBlocker` builds the same grids, routes and `WorldHash`.

`cargo run -p ludion --example nav_blocker` opens a door when the guard has waited.

## Avoidance

`NavAgent.avoidance` (walkers only) replaces the separation push with an ORCA velocity (reciprocal velocity
obstacles in the xz plane), capped at `speed`:

```yaml
NavAgent:
  speed: 5
  avoidance: { horizon_s: 1.5, max_neighbors: 6, priority: 0 }   # radius: absent = the class radius
```

- **Inputs** are start-of-tick state: the `max_neighbors` nearest walking agents on `neighbor_layers`, each at its
  previous `NavSteering.velocity`. An agent with no `avoidance` is dodged fully; separation still applies to it.
- **Priority:** equal `priority` shares the avoidance (half each); the higher one keeps its course and the lower
  takes it all. `radius` defaults to the agent's class radius, so a forklift keeps its larger berth.
- **Grid and blockers:** an avoided velocity is kept only if a one-cell walk along it stays on the class grid (a
  `block` blocker included); otherwise the agent takes its seek. An agent `at_link` or `at_blocker` stands still.
- **Right-hand bias:** with neighbours in range the preferred velocity is nudged 1% to the agent's right, so an
  exactly head-on pair (guards walking the same cell-centre line) parts to opposite sides instead of deadlocking.
- **Limits:** no wall constraints (a crowd can graze a corner) and no flyers (`flying` with `avoidance` is `L1528`).
- A `radius` or `horizon_s` not above 0, or `max_neighbors` outside 1 to 16, is `L1528`. An agent without the field
  steers, and hashes, as before.

### Obstacles

A non-agent moving body (a forklift, a rolling boss) that avoiding agents yield to carries `NavObstacle`:

```yaml
NavObstacle: { radius: 1.2 }   # enabled: false is ignored but keeps tracking its pose
```

- **Motion is read from the pose.** `nav_steer` writes `NavObstacleState { position, velocity }` (engine-written,
  hashed) from the entity's world position each tick, velocity being the last step's motion, so a kinematic,
  motor-driven or game-moved body all work.
- **Schedule:** the state lags one tick, so run the mover after `nav_steer`. Obstacles are seen only by agents
  with `avoidance`, with full responsibility (the agent steers around them); separation ignores them.
- **Range:** an obstacle counts when it is within the agent's query reach on xz and within the class height in y.
- A `radius` not above 0 is `L1528`. A world without `NavObstacle` steers, and hashes, as before.
- Covers Serious Isaac draft 0029: give the boss a `NavObstacle` and the horde `avoidance`.

`cargo run -p ludion --example nav_avoidance` crosses two crowds.

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

Put the motors on a layer that ignores itself, so they don't block each other. Nav needs no
`ignore_layers` for them: kinematic and rigid bodies are never sampled.

## Gotchas

- A target whose cell has no node (within `agent_radius` of a wall, off the grid) gives walkers the
  straight-line fallback — which can face a wall. Aim at open floor.
- An **agent** farther than `flow_radius` from its target gets no field direction and heads straight
  for it.
- Stacked ramps link each cell to the closest-height layer per direction.
- Cost, unpinned M5 laptop, release (indicative; pinned figures are owed): `nav_steer` 100 agents
  0.03 ms, 500 agents 0.17 ms at one thread; one flow field over a 64 x 64 room 0.11 ms; full grid
  build 0.8 ms, region rebuild 0.16 ms; 500 `CharacterMotor`s fed by it 3.8 ms (the motor is
  serial). Reproduce with `cargo test -p ludion-engine --release --test it -- nav_bench --ignored
  --nocapture`.

## Next

- `ludion docs motor` — the capsule that applies a steering velocity.
- `--debug-draw nav` (`nav:grid`, `nav:routes`, `nav:flow`) draws the grid, routes and flow arrows: `ludion docs debug-draw`.
- `ludion docs spatial-3d` — the index separation reads.
- `ludion docs patrol` — guards that walk `kind: patrol` routes as nav legs.
- `ludion docs components` — the `NavAgent`, `NavSteering`, `NavGoal` and `NavRoute` rows.
- `cargo run -p ludion --example nav_path` — a wall between an agent and its goal.
- `cargo run -p ludion --example nav_link` — a ladder between two floors.
- `cargo run -p ludion --example nav_class` — a walker and a forklift through one gap.
