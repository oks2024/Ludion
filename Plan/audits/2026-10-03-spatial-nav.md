# Audit: spatial queries, navigation fields and path following (2026-10-03)

**Scope.** Report only, at rev `42a4b66e`. Covered: `crates/ludion-spatial/src/spatial.rs` (the
index, `rebuild`, `overlaps`/`raycast`/`k_nearest`/`k_nearest_entries`, `move_and_slide`/`_at`,
the narrow phase), skimmed `tests/conformance.rs`; `crates/ludion-engine/src/path_follow.rs` plus
`PathSampler` in `crates/ludion-scene/src/path.rs`; `games/vigil/src/nav.rs` (census, grid,
Dijkstra, gradient, spawn draw), the game-side readers of the field (enemy AI, a path follower, spawning, occlusion), and
the schedule in `lib.rs`; `crates/ludion-cli/src/map_check.rs` (occupancy/reachability); docs
`docs/collision.md`, `docs/map-check.md`, `dev-docs/ludion-spatial.md`. Prior items were checked
against `Plan/code-audit-2026-07-25.md` (C3, C4, C5 are fixed; C6 is still open and deferred by
that report, so it is not repeated here) and the other 2026-10-03 audits (snapshot-dev D4, physics
P3, vigil-gameplay V3 are not repeated). Static reading plus one run of the existing debug
`ludion map-check` binary on `games/vigil/scenes/district-breach.map.yaml`.

## Overall assessment

The spatial crate is in very good shape. Ordering is a real total order everywhere, the pruning is
backed by a written conservatism proof and an O(n²) oracle, and the edge cases (negative and
infinite coordinates, NaN radii, oversize colliders) are handled, so no correctness bug turned up
in `ludion-spatial` itself. The nav field's July findings are fixed: buffers are reused, the
gradient visits only reached cells, and the census runs once per frame. The real problems are at
the edges. `map-check` silently ignores every prefab-instanced collider, so its report on the town
square is confidently wrong. Bodies pressed against a wall usually stand on a cell the nav grid marks
blocked, so they lose the flow field exactly where they need it. And a game that forgets to schedule
`spatial::rebuild` gets no collision and no diagnostic. The rest is documentation drift and small
costs.

## High

None.

## Medium

### N1 — `map-check` drops every prefab instance from its occupancy probe without saying so, so its report on a prefab-built map is wrong · Medium · [verified]

**Evidence.** `occupancy_scene` (`crates/ludion-cli/src/map_check.rs:328-356`) keeps an entity only
if its own `components` hold a `Transform`. A prefab instance entry has empty `components`; its data
lives in `instance` (`crates/ludion-scene/src/lib.rs:260-262`), so every instance is filtered out.
The `colliders N` count (`map_check.rs:1281`) has the same blind spot. The same filter also drops
`Parent`, so a parented collider in a fragment would be probed at its local coordinates
(`crates/ludion-cli/src/placement.rs:329` keeps `Parent` for this reason). Running
`ludion map-check games/vigil/scenes/district-breach.map.yaml` reports `entities 317 … colliders 29`,
a grid with 15 blocked cells out of 480, `0 unreachable`, and three `L1028` "gate sealed behind
solid wall" warnings. The town square's houses, walls and plugs are prefab instances, and none of
them shows up. Nothing in the output or in `docs/map-check.md:40-42` says instances were skipped.
The only record is item 3 of the friction ledger of a private playbook.

**Why it matters.** `map-check` is the documented mid-loop verifier ("verify with `ludion
map-check`", `games/vigil/CLAUDE.md`). An agent iterating on a prefab-composed map gets a
confident all-walkable report and warnings that point the wrong way. It has no signal that the
tool cannot see the geometry. Vigil works around this with its own sealing tests, which a new game
won't have.

**Failure scenario.** An agent narrows an alley in a prefab-built district and reruns `map-check`.
The report is unchanged: 0 unreachable, the same three `L1028` lines. The agent concludes the edit
is safe.

**Fix.** Expand instances the way scene load does (the engine's prefab library) before stripping,
and keep `Parent`. Until then, count the skipped instance entries and print a warning line naming
them, so the report never claims more than it measured.

### N2 — A body pressed against a wall usually stands on a "blocked" nav cell, so it loses the flow field and falls back to straight-line seek · Medium · [verified mechanism; gameplay impact suspected]

**Evidence.** A cell is blocked when a `NAV_CLEARANCE` (0.6) circle at its center overlaps a static
(`games/vigil/src/nav.rs:443-452`). Blocked cells are never relaxed (`nav.rs:568`), so they keep
`distance = ∞`, and `flow_at` returns `None` for them (`nav.rs:149-151`). `enemy_ai` then uses
`flow_dir(..).unwrap_or(to)`, the straight line to the player (`movement.rs:205`), and
the path follower does the same. Take a wall face and a body resting flush against
it at distance `r` (`move_and_slide` puts sliding bodies there). The 1 m cell containing the body
has its center somewhere in `[r − 0.5, r + 0.5]` from the face, and that cell is blocked whenever
its center is within 0.6. Over the grid's phase relative to the wall, a flush swarm body (`r = 0.35`)
sits on a blocked cell 75% of the time, and a flush rusher (`r = 0.6`) 50% of the time.

**Why it matters.** Wall contact is exactly where the flow field earns its keep, and it is where
the reader drops it. The fallback straight line has a tangential component that can point the wrong
way around the obstacle. With `move_and_slide`'s fixed x-axis preference, it can drive the body into
a concave corner, where both axes block and the move resolves to `[0, 0]`. That is the wall-hugging
behaviour G6 was built to remove, at the bodies that touch walls most: separation pushes the outer
ring of a crowd into them.

**Failure scenario.** A swarm pack flows around a building corner, and separation presses its edge
onto the wall. Those bodies read `None`, seek straight at a player on the far side, slide along the
wall the short way into an alcove and stall until the player moves.

**Fix.** Give unreached cells that border reached ones a flow too. In `flow_at`, when the cell is
unreached, return the direction to its lowest-distance reached 8-neighbour (same corner-cut rule).
Alternatively, write that direction for blocked cells adjacent to `reached` during the gradient
pass. Either costs O(reached boundary) and keeps the result deterministic.

### N3 — Forgetting to schedule `spatial::rebuild` turns off all 2D collision and reports nothing · Medium · [verified]

**Evidence.** `index()` returns `None` without a `SpatialIndex` resource (`spatial.rs:600-603`).
`overlaps` and `k_nearest` then return empty, `raycast` returns `None`, and `move_and_slide_at`
returns the delta unchanged (`spatial.rs:616, 650, 715, 833`), all documented as "legitimate
states, not errors" (`docs/collision.md:104-105`). The engine never installs `rebuild` itself. A
game registers it by hand (`docs/collision.md:95-101`). No diagnostic code covers "world holds 2D
`Collider`s but no index was ever built" (no spatial entry in `crates/ludion-core/src/codes.rs`).
The engine already has this kind of check elsewhere: `path_follow::report_uninstalled`
(`path_follow.rs:205`, `L1216`), and `ludion-physics` registers its systems as one verb because a
wrong order "fails silently", calling that "the `spatial::rebuild` precedent without its footgun"
(`crates/ludion-physics/src/systems.rs:171`).

**Why it matters.** Errors are the UX budget. The symptom is that the player walks through walls
and bolts never hit, with nothing to iterate against. It is the 2D twin of physics P3 (which only
covers the rigid and 3D half).

**Failure scenario.** An agent builds a new game from `ludion new`, adds `Collider`s and
`move_and_slide`, and leaves out the `spatial_rebuild` line. Every test that checks only "the player
moved" passes.

**Fix.** At the boundary drain, report once (latched) when the world holds a 2D `Collider` and no
`SpatialIndex` exists after a full tick, for example as `L####` "colliders present but no
`spatial::rebuild` in the schedule". The help should give the one-line registration.

## Low

### N4 — The nav grid's 0.6 clearance is smaller than several of the bodies that read it · Low · [suspected]

`NAV_CLEARANCE = ENEMY_RADIUS = 0.6` (`nav.rs:77`) decides which gaps the field routes through, but
several enemy bodies (radii 0.8–0.9) and an escorted NPC (0.7) all steer by it. A cell-aligned gap of about 1.2–1.8 m reads walkable but does not fit those bodies, so the
field points them into a slot `move_and_slide` will not let them through. The town square plugs
every gap under 1.8 m (rev `bf24145d`), which hides this there. Other maps have no such guarantee.
**Fix:** derive the grid at the largest steering radius, or keep a second grid for large bodies.
At the very least, document the clearance as a map-authoring constraint next to the 1.8 m plug
rule.

### N5 — The nav module promises a non-allocating frame, but the Dijkstra heap is rebuilt every frame · Low · [verified]

`recompute` creates a fresh `BinaryHeap` on each call (`nav.rs:548-549`). It grows to the
frontier of the `NAV_FLOW_RADIUS` disk, which means several reallocations every Running frame.
Meanwhile the module docs (`nav.rs:23-25`) and the struct docs (`nav.rs:100-101`) say a
steady-state frame "allocates nothing". **Fix:** keep the heap's `Vec` in `NavField` (cleared,
capacity kept), like `reached`, or correct the docs.

### N6 — Separation is sized to skip only the asker, so near the player an enemy separates from three neighbours instead of four · Low · [verified]

`enemy_ai` asks `k_nearest_entries` for `SEPARATION_NEIGHBORS + 1` kinematic entries
(`movement.rs:224-230`), then drops itself and every non-`Enemy` row (`:231`). The comment above it
says "the query includes self and the player — skip both" (`:211-212`). The player, and an
escortee, are kinematic, so in the contact ring, where packing matters most, one of the five slots
goes to a body that is then discarded. **Fix:** ask for `SEPARATION_NEIGHBORS + 2` (self and the
player), or filter before the take. That moves the hash, so it needs a re-record.

### N7 — `move_and_slide` treats exact contact as overlap, so a mover resting exactly on a wall face cannot slide along it · Low · [verified by arithmetic]

The narrow phase is inclusive (`<=`, `spatial.rs:887, 890, 907`), and `move_and_slide_at` tests
endpoints with it (`:852-854`). Take a circle of `r = 1` at `(2, 0)` against an AABB at the origin
with half-extents `(1, 1)`: d² is exactly 1, so it overlaps. A tangential delta `[0, 0.1]` gives the
full move, x-only and z-only endpoints all at d² = 1, so all three block and the result is
`[0, 0]`. A slide can never produce exact contact (an accepted endpoint is strictly clear), but
authored or spawned positions on round coordinates can. The docs say only an overlapping mover is
stuck (`docs/collision.md:122-123`). **Fix:** use strict `<` in the `move_and_slide` blocked test,
treating contact as not penetration, or document that touching counts.

### N8 — The vigil static census sizes the nav grid from local `Transform`s · Low · [suspected, latent]

`scan_statics` reads `transform.translation` directly (`nav.rs:379-382`), while `spatial::rebuild`
places parented colliders at their composed world centre (`spatial.rs:574-576`). A static collider
under a `Parent` would therefore stretch, or fall outside, the grid's extent, and cells near it get
`flow_dir = None` (off-grid). Nothing in vigil parents a static collider today (the square's prefab
roots carry the collider and have no `Parent`). **Fix:** take the centre from
`Transform::derived_ground_position`, as `rebuild` does.

### N9 — The grid rebuild on district arrival is O(map area), with one `Vec` allocation per cell · Low · [suspected, unmeasured]

`build_grid` runs one `spatial::overlaps` per cell (`nav.rs:437-454`), and each returns a fresh
`Vec`. That is about 54k probes on the 224 × 224 plaza (the figure in `nav.rs:47`), all on the
arrival frame. No recorded measurement covers it: `nav_flow_field_timing` times only the
Dijkstra. It is plausibly a multi-millisecond spike on every gate crossing. **Fix:** measure it,
and if it shows, swap the `Vec`-returning probe for an any-hit check against the statics half (the
`move_and_slide` `blocked` closure is that walk already).

### N10 — Spatial documentation drift · Low · [verified]

- `dev-docs/ludion-spatial.md:18, 45, 60` describe `Body` as `static`/`dynamic` and the halves as
  `statics`/`dynamics`. The kinds are `static`/`kinematic`/`rigid` with a contact material.
  `:57` says `rebuild` walks `world.entities()` (it is a typed `query_ids` join, with a world-pose
  second pass under `Parent`). `:110`'s gotcha says "`Collider` must stay registered LAST", but it
  is registered fourth (`crates/ludion-engine/src/components.rs:3487`). The page also omits that
  `rebuild` skips 3D shapes and rigid bodies.
- `spatial.rs:753` says "never a `Dynamic`", a variant that no longer exists.
- `docs/collision.md:153-171`'s copyable separation example reads neighbours' live `Transform`
  in a sequential loop. That is the order-dependent, asymmetric Gauss–Seidel pattern vigil
  deliberately replaced (`movement.rs:214-222`). Its "the asker itself (distance 0, first)" is
  also wrong when two bodies share a position, since ties order by `EntityId`. The example should
  read `k_nearest_entries`' frozen `center`s.

## Checked and healthy

- **Pinned order and ties.** `overlaps` merges four streams by (cell key, `EntityId`) with no ties
  possible. `k_nearest` sorts on `(total_cmp(d²), EntityId)`. `raycast` breaks equal `t` by
  `EntityId`. The conformance oracle checks membership and order, plus explicit tie cases.
- **Boundaries and coordinates.** `cell_of` floors, so negatives are correct. Saturating
  `as i32` together with the clamp to occupied bounds keeps infinite or huge radii bounded. A NaN
  radius gives empty, not a panic. `max_radius` is inclusive and sign-blind consistently. The
  oversize split keeps a map-spanning wall from widening every probe.
- **Index staleness.** The index is rebuilt first in vigil's schedule, and every in-frame reader
  sees start-of-tick centres. This is documented (`docs/collision.md:181-183`) and is relied on
  deliberately as the Jacobi separation in `enemy_ai`. Bodies spawned during the frame are not
  hit-testable until the next rebuild, a one-frame lag that is deterministic and harmless at these
  speeds. Bolts (≤ 26 u/s, so ≤ 0.43 u per tick) cannot tunnel through a ≥ 0.35 enemy, because the
  endpoint test's combined diameter is ≥ 1.2 u.
- **Thread-count invariance.** `k_nearest_entries` and `move_and_slide_at` are pure reads of
  frozen data inside `par_for_each`. `flow_dir` reads an immutable resource.
- **Nav field (C3/C4/C5 fixed).** Buffers are reused and cleared over `reached` only. The gradient
  visits only `reached`. The census is cached per frame. Dijkstra pops are pinned by
  `(dist, flat index)`. Diagonals require both orthogonals walkable, in both the search and the
  gradient. An unreachable or degenerate cell yields `None`, never NaN, thanks to the `mag > 1e-6`
  guard. `spawn_candidates` scans only the annulus window. C6 (`k_nearest` allocate-and-sort) is
  unchanged and still deferred by the July report.
- **Path following.** A zero-length path is impossible (`validate` rejects coincident neighbours
  and fewer than two points, `path.rs:378-433`), so `rem_euclid(length)` cannot produce NaN.
  Ping-pong reflection and the f32 phase round trip are clamped by `PathSampler::locate`.
  `L1215`/`L1216` are latched once per entity (`diagnostics.rs:71-76`). The system is installed
  ahead of game systems, so a game's `spatial_rebuild` sees this tick's platform pose.
- **Rotated prefab footprints** are swapped by the generator (`art/town/square/build-square.mjs:65-75`),
  which makes up for the 2D tier ignoring rotation.

## Suggested order of work

1. **N1:** make `map-check` expand prefab instances (or at least warn with a skipped count). It is the
   only finding where a verification tool gives a false green.
2. **N3:** add the latched "colliders but no index" diagnostic. It is small and closes a silent
   failure class.
3. **N2:** add a neighbour fallback in `flow_at`. It moves the hash, so do it together with N6 in one
   re-record.
4. **N4:** decide on the clearance policy (larger probe or authoring rule) while touching nav.
5. **N5, N8, N9:** the heap buffer and the census world-pose are one-liners; measure N9 before acting.
6. **N7, N10:** doc fixes, plus a decision on whether contact counts as blocking.
