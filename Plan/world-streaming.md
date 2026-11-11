# Plan: World streaming — one seamless world from many regions

> Status: **evaluated, not scheduled.** Every claim of current-code fact below was read from the
> tree on 2026-09-14. Sibling of `Plan/physics.md` under the open-world feature (private notebook); unlike physics it has a consumer today — vigil's fourteen gated
> districts — so its first waves can ship as a vigil improvement before any new game exists.
> Predecessors: `Plan/map-authoring.md` (the region format this plan composes),
> a private plan (3× districts, the measurements this plan extends),
> `Plan/scalability.md` (the quality axes the culling and LOD knobs join).

## Trigger criterion

W0 (the measurement) and W1 (culling) need no trigger: both pay for themselves on vigil today. W2
onward starts on either of two events: **the G6/G7 sitting rules that gated districts hurt the
city read** (the "districts crossed, route cost felt" verdict in a private plan), or **a
scheduled slice needs a playable area larger than one district** (the open-world game). Not before.

## The problem, precisely

A Ludion world is one scene file, instantiated into a fresh `World` (`ludion_scene::instantiate`,
`crates/ludion-scene/src/instantiate.rs`), and a district switch rebuilds that world from scratch at
a frame boundary (`crates/ludion-engine/src/transition.rs`), carrying only the listed entities, the
RNG, the frame index and the registered resources. The docs state the non-goals plainly: no
streamed or preloaded loading, no cross-scene queries, one district at a time
(`docs/scene-transitions.md`). Around that single-world assumption sit five structures that scale
with the whole world rather than with what is near the player:

- **Extract walks every entity every frame** and every `Mesh` not hidden by the authored `Visibility`
  flag becomes an instance: no camera-frustum culling, no distance culling (`crates/ludion-engine/src/extract.rs`,
  `place`). Instance buffers are rebuilt from scratch every frame (`render.rs`, `prepare_frame`).
- **The GI grid is world-anchored** to the scene's bounds with a hard cell cap (`MAX_GI_CELLS`,
  `crates/ludion-engine/src/voxelizer.rs`); at vigil's `cell_size: 0.5` a 320 u district is already
  near it, and one far entity stretches the bounds.
- **Voxel meshes have no LOD**: one greedy mesh per model document, drawn at full detail at any
  distance (`crates/ludion-engine/src/voxel_mesh.rs`; the doc calls its out-of-bounds read "the
  chunked-voxel-worlds seam").
- **`WorldHash` walks every live entity** (`crates/ludion-sim/src/hash.rs`), so replay-with-hash
  cost is linear in loaded entities wherever the camera is.
- **The asset cache never evicts** (`crates/ludion-engine/src/asset_cache.rs`).

Two structures already scale correctly and stay as they are: the spatial index is a hashed sparse
grid unbounded in extent, and the clustered light grid is camera-anchored. The shadow window is
anchored to the sun entity's transform and already follows the player by parenting.

**Coordinates are f32 and stay f32.** At 1 km from the origin an f32 ulp is about 0.06 mm; at 10 km
about 0.6 mm. A city-sized world needs no origin rebasing and no f64 sim. The plan records the
threshold, not a mechanism: camera-relative rendering (subtract the camera position on the CPU,
f32 on the GPU) is the first lever if vertex jitter is ever observed past ~10 km, and an f64 sim is
never on the table — it would move every hash and slow every system for a case no slice has.

## What vigil gains (the first consumer)

vigil's fourteen districts are each a `kind: map` with its own `origin`, and every one of them is
centred on the world origin — they do not share a frame. Gates carry the player between them by a
whole-world rebuild. What streaming changes for the game:

- **The city becomes geography.** Districts sit at real offsets in one world; crossing between them
  is walking, not a load. The G6 verdict ("districts crossed, route cost felt") stops being a
  gate-and-anchor illusion. Quest windows and the run clock already assume the crossings take time.
- **Gates and anchors are deleted** from the district maps; `SceneSwitch` stays for the camp and the
  death return, which are real scene changes. `Residue` becomes region-keyed instead of scene-keyed
  and does the same job when a region unloads and reloads.
- **Per-district state (sector state and similar)** already models the city per district; it keeps the district
  as their unit with no change — the region is the district.
- **Nothing in balance moves**: the spawner is an annulus around the player, the flow field is a
  player-seeded disk, `enemy_ai` scales with enemy count. Only the walkability grid is full-map today
  and becomes per-region (`games/vigil/src/nav.rs`).
- **The cost is the churn ledger**: every district replay re-recorded, the district goldens
  re-blessed on both boxes, the fourteen maps given offsets, the gate entities deleted.

## Core design

### The region is the district; the world is a placement list

A new document, `kind: world`, places regions:

```yaml
format: 1
kind: world
name: vigil-city
regions:
  - name: plaza
    map: district-plaza.map.yaml      # or a plain scene for a hand-authored region
    offset: [0.0, 0.0]                # world (x, z) added to the region's own origin
  - name: market
    map: district-market.map.yaml
    offset: [240.0, 0.0]
shared:                               # scene files instantiated once, never unloaded
  - env-night.yaml
  - hud.yaml
stream:
  load_radius: 160.0                  # regions whose footprint is within this of the anchor load
  unload_radius: 240.0                # hysteresis: unload only past this
```

The region file stays exactly the map format that exists; `offset` is the only new coordinate, and
the region's own `origin` is region-local. A region's **footprint** is its grid's world rectangle
(the `rows` extent at `cell_size`, plus its markers), computed by the generator and written into the
generated scene, so the runtime never parses a map. `shared:` replaces the `fragments:` that every
district repeats today (`env-night.yaml`, `hud.yaml`); a region's `fragments:` keep working for its
own hand-authored core. A world with one region and no `stream:` is exactly today's scene.

### Streaming is a sim-side policy, never a renderer decision

The loaded set is a **pure function of sim state**: a `StreamAnchor` resource (a world position,
by default the carried player's) and the world document's radii. The engine evaluates it at the
frame boundary, after the transition drain, and loads or unloads whole regions. Because the
anchor is hashed sim state and the policy is data, replay reproduces every load at the same frame,
the loaded set is deterministic, and `WorldHash` over the loaded entities is a valid trace. The
camera never influences it — the renderer draws what is loaded, and a headless run streams
identically to a windowed one.

### Loading into a live world

`ludion_scene::instantiate` gains a sibling, `instantiate_into(&mut World, doc, region)`, that spawns a
document's entities into an existing world in document order, tagging each with a `Region`
component (the region name; hashed like any component). Unload despawns a region's entities in
ascending entity order, so the LIFO free-list reuse is deterministic. A region's `Residue` entities
are captured on unload and re-applied on reload, keyed by region name — the transition mechanism
with a different key. Entity names stay globally unique across the world; the generator prefixes a
region's coordinate-derived names with the region name, which also keeps diffs local.

### Replay carries the world inline

Today an `.lrp` header embeds the scene document and each switch embeds its target document. A
world session embeds the world document and every region document it loaded, at the frame it
first loaded them, so a replay reproduces without disk — the contract that already holds for gates.

### The loader thread and the hitch rule

Parsing and instantiating a region on the sim thread is a hitch. A loader thread parses region
documents into a staging form ahead of need (a **prefetch radius** beyond `load_radius`), and the
sim splices the staged entities at the frame the policy demands. Readiness timing is
non-deterministic; the splice frame is not: **if a region is not staged when the policy needs it,
the sim waits** — a hitch, never a divergence. Prefetch makes the wait rare; the profiler's `stream`
row makes it visible.

### Rendering what is loaded

- **Culling in extract**: camera-frustum rejection for instances, a separate shadow-frustum test so
  off-screen casters still cast, and a per-scale distance cutoff under the scalability system.
  Goldens do not move by construction when only invisible instances are dropped.
- **GI per region**: the grid becomes per-region and world-anchored at the region's footprint, so
  the cell cap is per region and a bake stays valid while the region is loaded. Region borders are
  the visible risk (light leaking or a seam at the edge); the plan's answer is a border margin
  baked from the neighbour's geometry, measured, not assumed.
- **Voxel LOD**: a model's LOD chain is a 2× voxel downsample (majority palette index per block)
  re-meshed by the same greedy mesher, generated at asset load, deterministic and cached with the
  model. The downsample chain shipped under `Plan/gpu-driven-culling.md` decisions 30–35, selected by
  screen size in the GPU cull with `Camera.lod_bias`, not by distance in extract. Impostors are deferred
  until a measurement shows the chain is not enough.
- **Persistent instance buffers per region** replace the per-frame rebuild only if W0 shows the
  rebuild on the profile; otherwise the rebuild stays.

## Authoring: an agent edits one region, never the world

The map loop is the unit and stays unchanged: edit one region's map, `generate-map` on that file,
`map-check` on that file. Three things are added, none of them a new way to do an existing thing:

- **`world-check`** (a `ludion` verb and a protocol command): every region's footprint against its
  neighbours (overlap is an error with the offending cells), **seam continuity** (a walkable cell on
  a region's edge must meet a walkable cell across the border, else the road dead-ends at an
  invisible wall — reported as the pair of cells with both files' row/col), dangling `shared:` and
  `map:` paths, and duplicate entity names across regions. The report is terse like `map-check`'s.
- **A region-scoped session.** `ludion serve --world city.world.yaml --at x,z` and `ludion render
  --world … --at x,z` set the `StreamAnchor` to a point, so an agent loads and looks at the ring
  around the region it is editing without walking a player there. The editor is a peer client of the
  same command.
- **Diff locality is preserved by construction**: one region file per edit, region-prefixed names,
  the world document touched only when a region moves or is added. The drift gate runs over every
  generated region as it does today.

`generate-map` never learns about the world: it emits one scene per map as now, plus the footprint
line. Composition is the world document's job alone.

## Engine/game boundary

Engine: the world document and its schema, the streaming policy and `StreamAnchor`, region load
and unload, region-keyed residue, the loader thread, culling, per-region GI, voxel LOD,
`world-check`, the replay records. Game: where the anchor is (the player, a vehicle, a cutscene
point), what a region's own systems do when it loads (vigil's nav grid derives per region from the
static census exactly as it re-derives on a switch today), and every gameplay meaning of a region.

## Waves

### W0 — the measurement (no engine change)

Compose vigil's fourteen districts at real offsets into one flat scene (a throwaway script
concatenating the generated scenes with translated transforms; about 3 300 named entities, roughly
850 × 850 u) and measure it loaded whole on the 3090 box: extract and `scene` pass time, instance
count, replay-with-hash cost per tick, the GI cell count against the cap, memory. Record the
numbers in Status. They decide two things: whether persistent instance buffers are needed, and
whether the GI grid must go per-region in W3 or can wait.

### W1 — culling (ships on vigil today)

Camera-frustum and shadow-frustum rejection in extract, a distance cutoff as a scalability axis,
and instance counts in the profile report. Pinned by the goldens staying byte-identical on both
boxes and by a `culling` test that counts instances for a camera placed to exclude known entities.

### W2 — the world document, region streaming, `world-check`

`kind: world`, `instantiate_into`, the `Region` component, `StreamAnchor`, the frame-boundary
policy with hysteresis, region-keyed residue, the replay records, `world-check`, the `--world`/`--at`
session forms, `docs/world-authoring.md`. Synchronous loading on the sim thread in this wave — the
hitch is measured, not hidden. A gallery world of four small regions with a replay crossing every
border is the oracle; the 1/2/8-thread identity gate runs over it.

### W3 — vigil adopts

The fourteen districts placed into `vigil-city.world.yaml`, gates and anchors deleted, `shared:`
taking the environment and HUD, the nav grid per region, sector state confirmed
region-keyed, the churn ledger paid once (replays re-recorded, goldens re-blessed, both boxes). The
G6 re-attempt rides the next sitting on this build.

### W4 — the loader thread and per-region GI

Prefetch staging on a loader thread with the hitch rule, the profiler's `stream` row, the GI grid
per region with the border margin measured on vigil's densest pair of neighbours.

### W5 — voxel LOD

The downsample chain shipped (`Plan/gpu-driven-culling.md` decisions 30–35: selection by screen size in the
GPU cull, the `voxel-horde` gallery golden); W5 keeps the impostors-on-trigger and a
`city-stress` budget on both boxes: the fourteen-district world with a horde, the first budget that
names an instance count.

## Alternatives considered and rejected

- **One giant scene file.** Kills diff locality and the per-region drift gate; a 60 000-line file
  is not reviewable by anyone.
- **Additive scenes with no world document, *by command*** (load a scene into the live world by
  command). The primitive exists inside W2, but without the world document there is no footprint,
  no seam check and no deterministic policy — the agent would be scripting loads by hand. Narrowed:
  `spawn_map` is an additive spawn *from a sim system*, where the game's own policy decides; it has
  no world document, footprint or seam check, and is a narrower primitive W2 can build on (owner
  approval for 0022, relay grill 2026-11-10).
- **A regular chunk grid over one voxel volume.** Right for a Minecraft-shaped game, wrong for a
  district city authored as maps with kits; the map is the unit the tooling and the designers
  already have.
- **Origin rebasing or f64 now.** No slice is within two orders of magnitude of needing it.
- **Renderer-driven streaming** (load what the camera sees). Breaks determinism and headless parity
  in one move.

## Risks / open questions

- **GI seams at region borders** — the one visible risk; W4 measures the margin on real content.
- **Hash churn** — `Region` on every entity and region-prefixed names move every district hash once;
  the ledger is paid in W3 in one commit, after the schema snapshot.
- **The hitch before W4** — synchronous loads of a 5 000-line region on the sim thread; acceptable
  for the mechanized gates, measured before any sitting.
- **Cross-region queries** — `spatial::overlaps` sees only loaded colliders; an enemy chasing across
  an unloaded border is a game rule (despawn at the edge), recorded in the docs, not an engine fix.

## Status

Nothing shipped.
