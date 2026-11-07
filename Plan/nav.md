# Plan: Navigation — grid, flow fields, agent steering

> **Mechanized — every subtask shipped; pinned bench figures and the vigil migration are owed
> (`Plan/owed.md`).** Driven by the external FPS request 0007: navigation and crowd AI for 100–500
> agents, deterministic, with the 3D motor's cost at that count. Game-facing doc: `ludion docs nav`.

## Design decisions

1. **A layered 2.5D grid, sampled from static physics colliders, not derived from voxels.** Any
   static collider is walkable ground, not only voxel rooms, and bridges and overhangs are extra
   layers in a column.
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
`ludion docs nav`.

Bench (release, unpinned M5 laptop, 2026-11-07, indicative only; 64 × 64 m room, ramp, pillars,
chase target crossing a cell every 30 ticks): `nav_steer` 100 agents 0.03 ms at 1 thread and 0.04 ms
at 8, 500 agents 0.17 ms and 0.11 ms; one flow field over 4,096 nodes 0.11 ms; full grid build
0.79 ms, region rebuild 0.16 ms; 100 `CharacterMotor`s fed by `NavSteering` 0.71 ms, 500 motors
3.8 ms at 1 or 8 threads (the motor is serial; whole tick 4.2 ms). The 1- and N-thread `WorldHash`
traces are identical. Pinned figures owed: `Plan/owed.md`.

## Explicitly out of scope (triggers recorded)

- **Several agent classes per world** — trigger: a game with two agent sizes.
- **A 3D flying field** — trigger: flyers stuck behind geometry in play.
- **Avoidance beyond separation.**
- **Voxel-derived walkability** — trigger: a voxel room the collider sampler misreads.
