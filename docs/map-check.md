---
description: Reading a map-check report: occupancy, reachability, --traverse timing, and the placement lints
---

# Reading a map-check report

**Scope:** The mid-edit verifier of the map loop — the ASCII report `ludion map-check` prints, the `--traverse` sizing pass, the kit block that supplies its walker, and the placement lints. The `kind: map`/`kind: map-kit` documents it reads are map-authoring; the scene `generate-map` emits from the same sources is scene-format.

`map-check` is the token-efficient half of the loop — it generates in memory and prints an ASCII
report instead of the scene, so you read a few hundred tokens rather than the emitted YAML.

## The loop

```
ludion map-check scenes/district-plaza.map.yaml             # occupancy + reachability, before you compile
ludion map-check scenes/*.map.yaml                          # both map verbs batch over several sources
ludion map-check scenes/district-plaza.map.yaml --at -6,-4  # what is at this world position?
ludion map-check scenes/district-plaza.map.yaml --traverse  # how long is a gate-to-gate crossing?
                                                            # (a 4-connected path: diagonals walk it faster)
```

## Reading the report

```
grid 33x36 (1188 cells), cell_size 4
    0    5    10   15   20   25   30
 0  #################################
    #...............................#
 5  #.......##......#......##.......#
  legend: # blocked  . walkable  M marker  A anchor  ? unreachable

blocked 180  walkable 1008  unreachable 0
entities 62 (fragments 31, grid 7, markers 24), colliders 21
reachability: from first marker `market-hall` — 0 unreachable walkable cells
walkability delta: 48 cells blocked by an overhanging prop
gate links: 2 checked, 0 dangling
```

It speaks both coordinate systems at once — the ruler and the per-marker table give each marker its
`world (x, z)` **and** its `cell (cN,rN)`, so you never convert by hand. Occupancy is probed by
testing a `--radius` (default `0.5`) circle against the generated static colliders of the scene **as
the game loads it**, at the points of a sub-cell lattice with about two samples to the radius, so a
street that bends through a gap narrower than a cell stays walkable. The lattice can miss a gap up
to about a step·√2 wider than the walker, so its reachability is a heuristic; the **seal check**
(below) is the sound one. A cell is *walkable* when any
of its samples is free, *blocked* when all are, *reached* when any is. Prefab instances are expanded
and `Parent` is resolved, so a house placed as a prefab, or a collider parented to a moved entity,
blocks its world cells, and `colliders` counts them (`, prefab instances K (P parts)` follows when
there are any). A
static `box` with no `Parent` is probed by its yaw-turned XZ rect when its top is above nav's
`max_step` (0.4), and otherwise counts as floor. Other 3D and rigid colliders belong to the physics
solver and are absent from the 2D index the probe uses: the summary says how many (`, U not probed`). A prefab that fails to expand is an error — the scene
would not load either; the error is reported against the fragment that instances it. Reachability is
a 4-connected BFS over the free samples, started at the free sample nearest `--anchor <entity name>`
(else the first marker, else the grid centre) among its cell and the four beside it — an anchor
standing inside a prop still reaches the ground around it. `--at x,z` probes that exact point, the
walker standing there, not its cell.

**Outside.** Blank glyphs (and cells past the authored rows) that nothing blocks, the walk never
reaches, and that join the grid edge are *outside* the map: they print as a blank, count as
`outside N` in the summary, and are neither walkable nor unreachable. A blank pocket walled in on all
sides is a real sealed pocket and stays `?`.

**Seal.** A second flood, on a finer lattice (four samples to the radius, shrunk by half a sample
diagonal), asks whether a walker can go from the start to the grid edge. It is **sound** — it never
misses a way out the walker fits — and may flag a gap up to twice the shrink narrower than the walker.
The report prints `seal: sealed from …`, `seal: LEAKS` (warning `L1040`, with the edge point and where
the walk leaves the grid's walkable area; a rerun at the full radius says *walker-wide* or *squeezes
through a gap* the walker may fit) or `seal: not checked`. A gate's trigger never stops the walk, since
a game may let a player pass one without crossing (a hold gate, a key held through an arrival): close
every opening in the frame with a collider behind its gate.

## Sizing a district: `--traverse`

A district's size is a *time*, not a footprint. `--traverse` reruns that BFS from each `Gate` marker
and prints, per gate pair, the shortest walkable crossing as cells → world units → seconds at
`--speed` u/s (default `6`, valid only with `--traverse`), measured over the same lattice, so the
distances are in lattice steps and a cell count may carry a tenth:

```
traverse: speed 6 u/s, 2 gates, 1 pair
  gate-to-cathedral -> gate-to-plaza  25 cells  100 u  16.7 s  transition (~30 s)
```

The last column names the band the crossing lands in — **transition** ~30 s, **standard** 60–90 s,
**signature** 120–180 s, else `below`/`above` those. Buckets split at the band midpoints (45 s,
105 s): the band says which gabarit you are nearest, the seconds whether you are inside it. The
number is a floor — shortest path, no combat — so felt time runs longer.

A `Gate` whose `to` is empty (an unbuilt target on a built-dark map) is timed like any other gate;
its rows end `[inert gate: empty `to`]`. Only the `Gate` component counts — a game's own gate type
is not read.

## The kit's `probe:` block

An optional **`probe:`** block on the `kind: map-kit` document names the walker `map-check` measures
with — your game's own player constants, so a report is never quoted at a generic default:

```yaml
probe:
  radius: 0.9   # the player's collision radius, world units
  speed: 6      # the player's speed, world units per second
```

Each field resolves **flag → kit → built-in default** (`--radius` 0.5, `--speed` 6), and the report
header names the source of each (`probe: radius 0.9 (kit), speed 6 u/s (kit)`). A non-positive value
is `L1022`. The block is read by `map-check` alone: `generate-map` ignores it, so adding it never
moves a generated scene.

## Rules

- **A gate's `anchor` must name an entity the *target* district generates.** `map-check` resolves it
  against the sibling map (or its committed scene) and raises `L1027` when it dangles — otherwise the
  failure only surfaces at runtime, as an `L3019` mid-run.
- **`L1040` the walker can leave the map** — a warning from the seal check: plug the gap with a
  collider (`--at x,z` probes the reported point), or cap the opening behind its gate. The repository's
  `map-drift` gate refuses a committed map that raises it, and one whose seal is not checked.
- **Four placement lints are warnings, not errors**, so they surface in the report without failing the
  command: `L1028` a gate the anchor cannot reach (neither its cell nor a neighbouring cell is on a
  walkable path from the anchor — pass `--anchor` to choose where the walk starts), `L1029` a marker on a cell blocked by *another* entity's collider or in a sealed pocket,
  `L1031` two gates with no walkable path between them (a bisected map — `L1028` sees one gate, this
  the pair) — all three heuristics over the occupancy probe. `L1032` is structural, not a probe
  heuristic: a marker whose `components:` bag authors `Residue` has no explicit `name:`, so its `Name`
  is the index-derived fallback and re-binds to a different marker whenever one is inserted or
  reordered above it. A marker blocked by its **own** collider is a footprint, not a bug, and never
  fires `L1029`. `L1031` fires with or without `--traverse`.

## Gotchas

- **The walkability delta is the bug class a playtest usually finds.** `map-check` reports cells whose
  glyph says *floor* but which a marker prop actually blocks; a non-zero delta is only a problem when
  it is somewhere the player must walk.

## Next

- `ludion docs map-authoring` — the map and kit documents, and the compile loop this verifies.
- `ludion docs scene-transitions` — the gates a map's `Gate` markers drive.
- `ludion docs scene-residue` — the `Residue` marking `L1032` guards.
- `ludion docs collision` — the colliders the occupancy probe tests against.
