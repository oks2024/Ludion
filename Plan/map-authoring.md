# Plan: Map Authoring — kit-of-parts levels from a text map source

> Status: **M0 COMPLETE (2026-07-19, Lore revs 256–258, three orchestrated waves).** Wave 1: the
> `kind: map`/`kind: map-kit` formats + the pure deterministic `ludion_scene::generate` + codes
> L1021–L1027 + `generated_from:` provenance. Wave 2: the `ludion generate-map` / `ludion map-check`
> CLI verbs + the drift gate (`scripts/map-drift.mjs`, wired into check.mjs + ci.yml). Wave 3: the
> reference-game scene rebuilt **generated and enlarged 2× linear** (from a 131-line map + a shared
> kit file + fragments incl. a shared HUD — vs a 900-line hand-authored
> original), run.lrp re-recorded with ZERO session-script edits, Windows golden re-blessed, drift
> gate live at 1 scene. **Sub-decisions settled:** commit-output **(a)** (generated scene committed,
> drift gate enforces byte-identity, discovery via `generated_from:`) — committed scenes and prefabs
> stay the drift-gated authoring output, while a room compiled at runtime by `spawn_map` is not a
> committed file and its sources (`*.map.yaml`, `*.kit.yaml`) stay drift-gated (owner approval for
> 0022, relay grill 2026-11-10); `map`/`map-kit` joined the
> engine's `ENGINE_KINDS` doc-walk skip set (Wave 3's find: sources live in the content root beside
> their scene, the Phase 16 `profile` precedent). **M1's nav knob stays untriggered:**
> the game's flow-field update measured 0.106 ms on the doubled arena vs the 0.20 ms ceiling (more extent,
> but also more blocked perimeter). M1 (protocol command + `ludion dev` hot loop) and M2 (GUI
> painter) remain future, triggers below. macOS golden re-bless queued.
>
> **Room assembly (Serious Isaac 0022, 2026-11-09):** `generate-map` gained `--kit/--seed/--mirror` (L1399 for a mirror with fragments), `--prefab` (a `kind: prefab` room, `ids[0]` its handle) and `despawn_instance`; `crates/ludion-cli/tests/room_assembly*.rs` proves spawn/teardown/respawn hash-identical at 1 and 8 threads, replay without the prefab, map and kit, and a ~340-entity bench. **Decision:** conservative path, commit-output (a) intact for committed files; a runtime `spawn_map` was approved by the owner (relay grill 2026-11-10) and shipped (below). **Finding:** kit colliders are 2D, so a generated room has no solver colliders and nav does not see it.
>
> **Runtime path (Serious Isaac 0022 amendment, 2026-11-11):** `spawn_map(world, map, MapSpawn { kit, seed, mirror, placement, name })` stamps a `kind: map` from a sim system as one prefab-style instance (`ids[0]` the handle, `PrefabInstance.source` the map key). Map, kit and fragment texts go through the `PrefabLibrary` text store (so recordings carry them as `prefab_text` records, no format change), the compile is the pure `generate` then the prefab loader (entity for entity the `generate-map --prefab` file), rooms are cached per `(map, kit, seed, mirror)`, `asset.reload` drops the rooms that read a reloaded text, and a `preload:` map warms it. `L1406` covers refused paths, wrong kinds and `physics:`. Proofs in `room_assembly_runtime.rs`; usage `ludion docs prefabs` "Rooms from maps". `scene.save` writes such a room as plain entities and `prefab.apply`/`revert` refuse it.
>
> Originally scheduled 2026-07-18 (user direction): the trigger fired early —
> the queued human-gate batch's judgment calls are hard to judge on the current small scenes, so
> full M0 — pipeline plus one scene rebuilt enlarged 2–4× — ran FIRST, and the gate batch plays once
> on the enlarged content. Churn (run.lrp re-record, golden re-bless) accepted, paid once.
>
> Originally: this plan exists so the decision to build a level-authoring
> pipeline is made from a written evaluation, not under pressure. It answers the standing question
> "can levels be considerably bigger, and is the authoring flow ready?" (raised 2026-07-18): the
> runtime can handle bigger levels; the bottleneck is that levels are authored entity-by-entity in
> YAML (one scene is 54 hand-placed entities / 900 lines for a 24×24 arena, HUD block triplicated;
> only ~25 of those entities are actually spatial props). This is the
> plan for the authoring lever that fixes that.

## Trigger criterion

Schedule **M0** when the game needs its first genuinely larger level, and drive it by rebuilding
**one** real reference-game level from a map source, not by designing a general editor in the abstract (vertical-slice rule). Do **not**
build the GUI painter (M2) until a human level designer actually needs to paint; its trigger is
recorded below.

## The problem, precisely

Making levels "considerably bigger" is not gated by the engine. Everything in the sim scales with
**entity/agent count**, not world extent — the spatial index is a uniform grid (`CELL_SIZE 16`,
indifferent to extent), and agent AI (~0.5–0.6 ms, the scaling frontier) is a function of agent
count. The one term that grows with **area** is the game's flow field: a full
player-seeded Dijkstra over every walkable cell every frame,
currently ~0.098 ms at ~1000 cells against a 16.6 ms budget. It is O(cells), so a 2–4× linear map is
still comfortable (~0.4–1.5 ms) and a 10×+ open world needs a coarser nav cell or a radius-capped
search (companion work item, M1).

The real bottleneck is **authoring**. A bigger level by hand is hundreds of hand-placed `Transform`
+ `Mesh` + `Collider` entities — tedious, error-prone (the "floor top must sit at Y=0" lesson is
exactly the kind of thing hand-authoring gets wrong), and a poor diff. The engine's own thesis already
names the fix: *"Scenes, materials, palettes, tilemaps: structured text with canonical serialization."*
A tilemap is the promised authoring primitive; this plan builds the kit-of-parts version of it.

## What already exists (and why it's only half the answer)

- **`Tilemap`** (`crates/ludion-engine/src/components.rs`) is a real, tested component: a glyph grid
  with a `legend: BTreeMap<String, TileAppearance>` and `rows: Vec<String>`, expanded **row-major and
  deterministically** into the **sprite batcher** at render time, with exact coordinate helpers
  (`cell_to_world` / `world_to_cell` / `glyph_at`) and validation codes (`L1019` ragged rows, `L1020`
  bad/legend-less glyph). It proves the **text ergonomics** we want — a level edit is a one-glyph diff,
  the minimal-diff pillar at its best.
- **But its expansion target is wrong for a kit-of-parts level.** `Tilemap` produces **flat tinted
  quads in the XY plane** and reports as **one renderable / one entity** — it has **no per-cell ECS
  entities and no colliders**, so its cells are invisible to the sim, the spatial index, and nav. A
  level needs each building/wall to be a **real static-collider entity on the XZ ground plane**
  (Y up), so that nav-census, spawn-validation, and the flow field see it for free. So we reuse
  `Tilemap`'s *format ergonomics and validation seam*, not its *expansion path*.
- **Precedent for "compile a higher-level source into canonical text":** `ludion import` compiles
  glTF into the canonical scene/mesh formats deterministically (same input → byte-identical tree).
  A map generator is the same shape — a deterministic compile verb — and should follow it. One
  accuracy note: import output has **no CI drift/regeneration gate today** — the drift gate this
  plan proposes is the first of its kind, to be wired into `check.mjs` + `ci.yml` when M0 lands.
- **E4 data-docs** (`doc_kind::<T>` / `DocStore<T>`, hot-reload via `asset.reload`) and **`ludion
  dev`** give the fast edit loop the generator plugs into.

## Core design

Two layers, matching the user's mental model ("paint the floor, then place markers for generic
items"), both **text, both game-agnostic in the engine**:

1. **The floor/fabric grid** — a glyph grid (reusing `Tilemap`'s `rows` + `legend` shape) for the
   dense, regular ground fabric: cobblestone, grass, a run of wall. Grid indexes the **XZ ground
   plane** — `col → +X`, `row → +Z` — with **Y the up-axis** where the floor rule lives.
2. **A marker list** — sparse, off-grid landmarks a grid is bad at expressing: a prop at an exact
   spot, a building rotated 23°, a gate. Each marker is `{ kind, x, z, rotation, variant }`, plus an
   optional `components:` passthrough (see Completeness below).

A **legend** maps each glyph and each marker `kind` to a **list of generic engine primitives** — a
mesh source (`voxel`/`mesh` path), a collider shape + extents, a Y-offset, an optional
rotation/variant set. The legend lives in a **shared kit file** referenced by every map,
and knows nothing about game props; those names live in the **game's** kit data. This is the
generic/game split:

- **Engine owns (generic):** the map-source format + schema, the deterministic **generator/expander**,
  the validation codes, the CLI verb, and the protocol command. Legend entries reference only builtins
  (`Mesh`, `Collider`, `Transform`).
- **The game owns:** the legend content — a glyph → a voxel model + its AABB collider, a floor glyph →
  a slab at `Y=-1`, a wall glyph → a wall piece. Pure data.

The generator **bakes the recorded floor rule once**: floor tiles emit at `Y=-1` (top on the Y=0
plane), props bottom-anchored at `Y=0` — the correctness an agent otherwise gets wrong by hand,
encoded in one place. Because expansion produces **real static-collider entities**, nav census, the
spatial index, and spawn-validation all pick them up with zero special-casing — the strongest argument
for real entities over render-time quads.

### Grid semantics: fabric on the grid, footprints as markers

- **A legend entry emits a *list* of primitives** (a stack), not one: a wall glyph emits its floor
  slab *and* the wall piece, so the grid never needs a second underlay layer. (An optional `base:`
  fill glyph can declare the default floor emitted under every non-empty cell.)
- **Same-glyph floor regions are rectangle-merged deterministically** (greedy, row-major, stable):
  the generator emits one scaled slab per merged rect, not one entity per cell. This matches
  existing practice — one reference-game floor is already 4 stretched slabs covering 24×24 — and is what
  keeps a 100×100 map at tens of floor entities instead of 10,000. File size, renderer load,
  and generated-diff size all depend on it.
- **The grid expresses the regular fabric only** — floor, plus modular runs (walls) whose kit
  footprint is a whole multiple of the cell. Kit pieces have intrinsic footprints (e.g. 8×7, 8×2), so cell ≠ prop is the norm, not the exception:
  anything with an irregular or multi-cell footprint is a **marker**. `cell_size` is chosen per map to match
  the kit module (likely 2 or 4 world units) and is decoupled from the game's nav cell size.

### Completeness: fragments + marker passthrough (what makes the drift gate sound)

A real level scene is mostly *not* spatial props: one reference-game scene is 900 lines / 54
entities, of which only ~25 are props (14 with colliders) — the rest is a ~290-line HUD block
(triplicated across three scenes), game singletons, the camera, 5 lights,
a gate entity, and 2 `Name`d inbound anchors. A grid + marker format that only expresses props
would force hand-edits *into the generated file*, which destroys "map source is truth" and makes
the drift gate unimplementable. Two mechanisms close the gap so the generated scene is **100%
derivable from the map source**:

- **`fragments:`** — the map source lists hand-authored entity-list YAML files merged verbatim
  into the generated scene in stable order. Player, run-state, camera, and HUD live here. The HUD
  becomes **one shared fragment referenced by all maps** — deleting the recorded
  ~290-line ×3 triplication as a side effect (record that payoff against the roadmap's widget-UI
  friction item when it lands).
- **Marker `components:` passthrough** — a marker may carry a verbatim component blob in addition
  to (or instead of) a legend `kind`, so gates (`Gate` + visible `Mesh`), `Name`d transition
  anchors, and torch lights (`PointLight` + post) are all expressible as markers.

### Determinism and diff stability (the generator's obligation)

The canonical writer preserves the `entities` sequence **verbatim** (it never sorts entities; only
components-within-entity and fields-within-component are normalized) and enforces **no entity-name
uniqueness** — so diff stability is entirely the generator's job, and it matters: without it, one
map edit could churn the whole generated file. Rules:

- **Coordinate-derived, unique entity names** (`floor-r012-c007`; merged rects encode their
  extent) and a **fixed emission order**: fragments → grid rects (row-major) → markers (source
  order). A one-glyph edit then touches only the affected rect/entity in the generated diff.
- Name uniqueness is a **generator-side validation** — gate anchors resolve by `name:`, so a
  collision is a real bug, not a style issue.
- **Variant/rotation selection is seeded from `(map seed, cell coords)`**, never from draw order —
  an edit in one corner must not reshuffle variants everywhere else.

### The crux decision: where does expansion happen?

Three models; the plan recommends **C** as primary with **B** as a one-shot escape hatch, and rejects
**A** for this use.

- **(A) Render-time only, like `Tilemap` today** — no colliders, invisible to nav/sim. **Rejected**
  as the target (kept only as the ergonomic precedent).
- **(B) Load-time expansion into real ECS entities, map component NOT round-tripped** — preserves
  minimal-diff at runtime and matches hot-reload, but hits a **canonical-writer round-trip trap**: the
  writer serializes the live World, so a `scene.save` after a GroundMap expands would bake hundreds of
  derived entities and a reload would double them. Resolving that (writer skips tagged-derived
  entities, or expansion lives below the serializable World while colliders must live *in* it) adds a
  "derived-but-real-entity" concept whose cost outweighs the benefit here. **Secondary at most.**
- **(C) A deterministic compile verb (recommended)** — the map source is the committed truth; a
  `ludion` generate verb (also a protocol command) expands it into a **scene of concrete entities**,
  exactly like `ludion import`. The live World only ever contains ordinary entities, so there is **no
  round-trip trap**. Minimal-diff is preserved at the layer that matters for an agent — **authoring**:
  you edit a small map file and recompile (`cargo build` for levels). Wire it into `ludion dev` so a
  map edit auto-regenerates-and-reloads, recovering the fast loop.

**Open sub-decision (settle in M0):** is the generated scene (a) **committed like `import`
output** and regenerated on demand, or (b) **build output** (loreignored/gitignored) regenerated by
`ludion dev`? Lean **(a)** initially — simplest, matches the `import` precedent, and leaves the golden
/ replay pipeline unchanged because tests still render a real committed scene file. The scene the
game loads becomes the generated one; its source is a `.map.yaml`
(committed, minimal-diff, agent-edited). Commit **the map source as truth**; if (a), the generated
scene rides alongside it and CI asserts it is byte-identical to a fresh regeneration (drift gate).

**Provenance (settle with it):** the canonical writer emits no comments — parse→emit strips them —
so a generated file cannot carry a "GENERATED — edit the `.map.yaml`" banner as a comment. Recommend
an optional top-level **`generated_from:`** scene field (serde-default, emitted after `name:`): it
marks the file for humans and agents, and it **doubles as the drift-gate discovery mechanism** — CI
regenerates every scene bearing the field and asserts byte-identity, with no separate manifest to
maintain. This strengthens lean (a).

## Why this helps an agent (the North Star)

Scored against *"how many autonomous iterations before a human must look at the screen?"*:

- **Compact, regeneratable diffs.** A level collapses from hundreds of hand-placed entities into a
  glyph grid + a short legend + a marker list. An agent edits that as text and regenerates.
- **Deterministic + verifiable headlessly.** Same map → byte-identical scene (canonical writer) →
  stable `sim-hash` → stable golden. The agent changes the map, regenerates, and verifies by hash and
  image comparison — **no human at the screen**. That is the metric moving.
- **Correctness encoded once.** The Y=-1 floor rule, collider extents, spawn-validation, deterministic
  placement order — the tool bakes them, so the agent stops re-deriving them per level.
- **The generator is a command**, so it satisfies the parity gate and is usable identically by the
  headless agent loop today and the human GUI later.

The **2D painting GUI does not move this metric** — agents write text, they do not paint. It is a
genuine convenience for the human level designer (a named first-class user), which is why it is real
and planned, but it is **layer 3, deferred, and built on top of the command**, per *agent-ergonomics
win* and *nothing ships in the editor unless it exists as a protocol command*.

## The agent contract (token economics)

The pipeline's token-budget rules, written down so every instance follows them:

- **Generated scenes are write-only for agents.** Never read, never grepped, never hand-edited —
  all inspection goes through `map-check`, `check-scene`, `sim-hash`, and (at the end of a change)
  goldens. The math: a 100×100 map source is ~3–4k tokens to read whole; the generated scene it
  replaces is tens of thousands. Reading generated output forfeits the entire savings.
- **One row per line keeps a local change a one-line `Edit`.** A one-glyph change needs neither a
  full rewrite nor a full read of the grid.
- **The kit legend lives in one shared kit file** referenced by every map (per-map
  overrides allowed) — legend content is written once, not per map, and a kit tweak is one
  diff, not N.
- **The mid-loop verifier is `map-check`** (M0 deliverable, below): the big-map iteration loop is
  edit-map → regenerate → map-check — a few hundred tokens of structured text per iteration, with
  rendering and sim-hash reserved for the end of a change, not every step. No screenshots, no
  reading YAML mid-loop: that is the North Star metric moving at map scale.

## Test oracle (why this is tractable)

- **Generation is deterministic:** `generate(map) → scene` twice yields **byte-identical** canonical
  YAML.
- **Generation is valid:** `generate(map)` then `check-scene` passes with zero diagnostics; malformed
  legend/markers fail with `L####` pointer diagnostics (reuse/extend `L1019`/`L1020`).
- **Generation is sound at runtime:** the generated scene's `sim-hash` is stable, and props land as
  real colliders (nav census non-empty, spawn-validation rejects overlaps).
- **Drift gate** (if the generated scene is committed): CI regenerates and asserts byte-identity
  against the committed artifact (discovery via the `generated_from:` field).
- **Diff locality:** change one glyph → regenerate → the generated-scene diff touches only the
  affected rect/entity (proves the naming/ordering/variant-seeding rules).
- **`map-check` is itself snapshot-tested** (text golden of the annotated report).
- **New `L####` codes** (registered in `ludion-core`, alongside the reused `L1019`/`L1020`):
  marker out of bounds, generated-name collision, unknown marker kind, dangling cross-map
  gate anchor.

The generator is never trusted, only diffed — same discipline as the determinism gates.

## Phases

### M0 — Format + generator spike, driven by one real level (~small)

- Define the map-source schema: floor glyph grid (`rows` + `legend`, reusing `Tilemap`'s shape and
  validation codes) + a marker list (`kind`, `x`, `z`, `rotation`, `variant`, optional
  `components:` passthrough) + `fragments:` (verbatim entity-list includes — player/run-state/
  camera/HUD; the HUD as one fragment shared by all maps) + a **shared kit file** mapping
  glyph/kind → a *list* of `{ mesh, collider, y_offset, rotation, variants }` primitives.
- Write the generator as a **plain deterministic function** first (`map source → Vec<entity>` /
  canonical scene YAML): fragments → merged grid rects (greedy row-major) → markers, coordinate-
  derived unique names, `(map seed, cell coords)` variant seeding, baking the Y=-1 floor rule. Add
  a **CLI verb** (`ludion generate-map` / working name — mirror `import`).
- **`ludion map-check`** (working name — the token-efficiency lever): validates the map source AND
  prints a compact annotated ASCII report computed from the *generated* entities via
  `ludion-spatial`: occupancy grid (static-collider probe, configurable radius — the engine-generic
  version of the reference game's nav census), reachability flood-fill from a named anchor (unreachable pockets
  flagged), the **intended-vs-actual walkability delta** (a floor glyph whose cell is actually
  blocked by an overhanging marker prop — the class of bug that today needs a playtest), marker
  out-of-bounds warnings, and a **cross-map link check** (every `Gate.to` + `anchor` resolves
  against the sibling map's generated anchors — today that fails only at runtime as L3019). No
  such dump exists anywhere today; this is the surface the agent iterates against.
- **Rebuild one enlarged reference-game level** from a map source (2–4× linear). Golden + `sim-hash` it.
  Record the real diff-size and the measured flow-field cost at the new cell count.
- **Settle the commit-output-vs-build-output sub-decision** (and the `generated_from:` provenance
  field with it) here from the real level; if (a), add the drift gate to `check.mjs` + `ci.yml`
  in the same change.

### M1 — Protocol command + hot loop + scaling knob

- Expose generation as a **protocol command** (`scene.generate` / working name) with a ≤10-line
  dispatch arm delegating to the M0 function — the parity gate, so the future GUI is a peer client.
- Wire the generator into **`ludion dev`**: a map edit regenerates and reloads (the tuning loop the
  game already has for docs).
- **Companion (only if the bigger map needs it):** coarsen the nav cell size or radius-cap the
  flow-field Dijkstra so the O(cells) per-frame cost stays well under budget at the new scale.
  *(Shipped 2026-07-22: the radius cap was built game-side. This companion is done.)*

### M1-E — Loop ergonomics (**SHIPPED 2026-07-23, Lore rev 303** — `Plan/map-tooling-ergonomics.md` Status has the detail; the six improvements below all landed)

Evaluation from a large-scale content arc's measured agent cost (per-wave token accounting + the Wave 3
map agent's self-audit — three maps redesigned at 33×36 in ~372k tokens). The finding: **the
loop converges fast (1–2 layout iterations per map) but only because the agent does all
origin/row/col ↔ world-unit math by hand** — that hand-math dominated the map-side cost and caused
the wave's only map bug (a perimeter gap authored on the wrong side), which `map-check` cannot flag
and a slow full-test roundtrip caught instead. Per-invocation, `map-check`'s per-cell
walkability-delta enumeration was the largest unused output. The write-only generated-scene rule
worked perfectly (≈0 tokens spent reading generated output) — no change there.

Improvements, ranked by measured leverage (1–3 estimated at 50–80k tokens + one test-suite
roundtrip saved on a Wave-3-shaped task; all live in `ludion-scene`/`ludion-cli`):

1. **Marker/coordinate table in `map-check`** — every marker/anchor/gate listed with world `(x,z)`
   AND cell `(col,row)`; plus an `--at x,z` query → cell, walkable/blocked, nearest marker. Kills
   the hand conversion math and the counting-characters-on-the-grid cross-referencing.
2. **Gate/anchor placement lint** — flag a gate marker with no adjacent perimeter gap, and an
   anchor/marker on a blocked or interior-unreachable cell, at authoring time (the L1027 spirit,
   applied to placement). Catches the wall-gap class of bug in-loop.
3. **`--quiet` scoped report** — grid + counts + reachability summary + gate links; the
   walkability-delta as a count, not an enumeration.
4. **Batch verbs** — `generate-map`/`map-check` over multiple sources in one invocation (a shared
   fragment edit currently forces N hand-looped regens).
5. **`generate-map --check`** — regen-to-temp + byte-identity assert without writing (exposes the
   drift gate's comparison to the mid-loop agent; replaces hand-rolled cp+diff).
6. **Grid ruler** — col/row index guides every 5 cells on the ASCII grid.

### M2 — GUI painter (deferred; peer client)

- A 2D top-down painter in the Tauri editor: paint the floor grid, drop markers, generate from the
  kit. It **emits the same map-source text and issues the same `scene.generate` command** — no
  editor-private path.
- **Trigger:** a human level designer actually needs to paint **and** the map format has stabilized
  over ≥2 real maps. Not before — it does nothing for the agent loop, and building it against an
  unproven format bakes in the wrong ergonomics.

## Alternatives considered and rejected

- **Keep hand-authoring (status quo)** — rejected; it is the motivating problem (doesn't scale, poor
  diff, floor-rule footguns).
- **Extend `Tilemap` in place (XY sprites, one entity)** — rejected as the *target* (wrong plane, no
  colliders, invisible to nav). Its *format ergonomics and validation seam* are reused.
- **Procedural generation (WFC / auto-layout)** — out of scope and against a core game decision: the
  world is a **fixed, handcrafted map**. The kit
  generator stamps an authored layout; it does not invent one.
- **GUI-first** — rejected; violates the parity gate and *agent-ergonomics win*, and helps humans, not
  the agent iteration loop.

## Current-code facts that shape the work

- `Tilemap` (`crates/ludion-engine/src/components.rs`): glyph grid + `legend` + `rows`, deterministic
  row-major expansion into the **sprite batcher** (render-time, one entity, no colliders), XY plane,
  helpers + `L1019`/`L1020`. Reuse the format; replace the expansion target.
- `ludion import` is the precedent for a deterministic compile-to-canonical-text verb; the CLI
  `Subcommand` enum (`crates/ludion-cli/src/main.rs`) and the `Command` enum + `dispatch.rs` ≤10-line
  arms (`crates/ludion-engine/`) are the patterns a new verb/command follows.
- The canonical writer preserves the `entities` sequence verbatim (components/fields are the only
  things reordered on write), supports **no comments** (parse→emit strips them) and no metadata
  field today, and entity `name:` is optional with **no uniqueness enforcement**
  (`crates/ludion-scene/src/emit.rs`, `lib.rs`) — the stability and provenance rules above exist
  because the engine guarantees none of them.
- `ludion import` output has **no CI drift/regeneration gate** (the only drift gates today are
  shader-bindgen and schema-drift) — this plan's drift gate is the first of its kind.
- One reference-game scene: 900 lines / 54 entities, ~25 spatial props (14 with colliders), ~290-line
  HUD block triplicated across three scenes; the floor is already 4 stretched slabs
  over 24×24 (the rect-merge precedent). The game's nav has no ASCII/debug dump of the
  nav grid — `map-check`'s report is genuinely new surface.
- E4 `doc_kind`/`DocStore` + `ludion dev`'s `asset.reload` watcher give the hot loop M1 plugs into.
- The **Y=-1 floor / Y=0 gameplay-plane rule** (2026-07-18 bugfix, recorded in a private notebook
  Lessons) is the correctness the generator must bake.
- The reference game's props are bottom-anchored voxel meshes with AABB static colliders (footprint
  half-extents in X/Z) — the legend maps glyph → prop + collider extents.
- The game's nav derives its grid from a **static-collider census**, so real expanded
  colliders trigger a rebuild automatically; the flow field's O(cells)-per-frame cost is the one
  area-scaling term (companion knob in M1).
- Seamless streaming stays the separate long-term goal; this plan
  is about **authoring** a bigger *loaded* level, not streaming an unbounded one.
