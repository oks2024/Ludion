# Plan: Phase 12 — Spatial Module (E1)

> **CLOSED — shipped 2026-07-13; see §Status below, "Waves 3–6 landed 2026-07-13 … E1 exit gates:
> all mechanized criteria green — the wave is READY per the roadmap."**
> Status: **authored 2026-07-13, ready to start.** This is the wave-level plan for wave **E1** of the
> first-game arc; its brief is `Plan/engine-first-game.md` §"E1 — Spatial module" and the
> cross-cutting rules there, scheduled by a private roadmap (rule 5: author the
> numbered wave plan with current-code facts before the wave starts). The consumer is game phase
> **G1 — First Blood** (a private plan, §G1): ~1000 active entities needing neighbor queries
> (separation, aura pickups, nearest-enemy targeting), projectile overlap hit-tests, and kinematic
> move-and-slide against static geometry, at 60 fps (the E6 budget: ≈500 seeking enemies + 300
> projectiles + 200 pickups + hundreds of statics). Breakout and Sokoban hand-rolled their collision;
> this is the third occurrence — promotion time. The thesis: deterministic 2D-on-the-ground-plane
> (x, z) collision as a sim-side library — a `Collider` builtin component, an explicitly-scheduled
> per-frame spatial hash, pinned-order queries (overlaps / ground-plane raycast / k-nearest), and
> move-and-slide against statics — with **no rigid-body dynamics** (games write their own response).
> The E1 brief's "Settled scope" items are decided; its "Proposed design" items are confirmed or
> amended below with current-code evidence (amendments are called out inline and summarized in the
> handoff).

## Current-code facts that shape the work

- **Builtin components are registered by name, in an append-only order, in one function.**
  `builtin_registry()` (`crates/ludion-engine/src/components.rs:567`) registers each builtin with
  `registry.register::<T>("Name")`; registration order is the canonical scene/world-hash key order
  and is **append-only** — `Tilemap` was deliberately added *last* (`components.rs:585`) so no prior
  component's index shifts and existing scenes/replays stay byte-identical. A new builtin `Collider`
  is therefore registered **after `Tilemap`**, at the very end. Registration is the whole cost of
  "degrade nowhere": the registry (`crates/ludion-ecs/src/registry.rs`) derives the JSON Schema from
  `schemars` (`registry.rs:278`), which feeds scene validation (`check-scene`), the editor inspector,
  and snapshot/restore from that one table — a registered component travels through all of them for
  free (`builder.rs` module docs: "recording, replay, snapshot/restore, and `WorldHash` need no extra
  wiring").
- **Scene validation for a component is schema-driven, plus an optional cross-field hook.** Out-of-
  range or wrong-typed fields surface as `L1014` schema errors with a JSON Pointer at the exact field
  — `Mesh`'s `roughness`/`metallic` range annotations prove `#[schemars(range(...))]` reaches
  `check-scene` (`components.rs:684` test). Invariants JSON Schema *cannot* express (e.g. a strictly
  positive radius; `schemars` `range(min)` is inclusive and does not reach array elements) go through
  `SemanticValidate` + `register_validated` (`registry.rs:247`, the path `Tilemap` uses for ragged
  rows / unknown glyphs), returning `Diagnostic`s with component-relative JSON Pointers.
- **Systems are plain `fn(&mut World, &SystemCtx)` run in strict registration order; games add them
  through the builder.** `Schedule` is sequential, no parallelism, no auto-ordering
  (`crates/ludion-ecs/src/schedule.rs:1`); `EngineBuilder::system(name, run)` appends one
  (`crates/ludion-engine/src/builder.rs:122`) and the schedule is rebuilt from the list on every world
  reset (so it survives `scene.load`/`engine.restore`). This is exactly the seam E1 needs: the engine
  ships `spatial::rebuild` as an ordinary system a game registers at an explicit position — no hidden
  systems, order stays visible.
- **Typed queries yield component references but NOT entity ids** — the single most API-shaping fact.
  `World::query::<(&A, &mut B)>()` yields one reference tuple per matching entity in ascending
  entity-index order (`crates/ludion-ecs/src/query.rs:1`), with no `EntityId`. Both existing games
  work around this by walking `world.entities()` and testing `world.has::<T>` / `world.get::<T>`:
  breakout (`games/breakout/src/lib.rs:189` — *"Queries don't yield ids, so walk `entities()`"*) and
  sokoban (`games/sokoban/src/lib.rs:109` — `world.entities().find(|&e| world.has::<Tilemap>(e))`).
  `spatial::rebuild` must therefore read colliders the same way: `world.entities()`
  (`crates/ludion-ecs/src/world.rs:126`, ascending slot-index) + `world.get::<Collider>(e)` +
  `world.get::<Transform>(e)` (`world.rs:194`), capturing the `EntityId` alongside the data so query
  results can *return ids* (which raw queries cannot) — this is a real ergonomic win over the
  hand-rolled loops, not just a dedup.
- **`EntityId` is `Ord` (index, then generation).** `#[derive(… PartialOrd, Ord …)] struct EntityId
  { index: u32, generation: u32 }` (`crates/ludion-core/src/ids.rs:32`). "Pinned order = cell order
  then EntityId" is thus a total order out of the box: sorted cell key, then `EntityId` ascending.
  `iter_alive` already yields ascending slot-index (`crates/ludion-ecs/src/entity.rs:104`) and the
  LIFO free-list is conformance-pinned (`entity.rs:1`), so rebuild reads colliders in `EntityId`
  order already.
- **Demand evidence, two shapes of hand-rolled collision.** Breakout is *continuous* circle/AABB:
  `move_ball` reflects off walls/ceiling/paddle with `(x - px).abs() < PADDLE_HALF[0] + BALL_HALF`
  overlap tests (`games/breakout/src/lib.rs:126`), and `break_bricks` does AABB-vs-AABB with
  axis-of-least-penetration response over a `world.entities()` walk (`lib.rs:180`). Sokoban is
  *discrete grid*: `walkable(map, col, row)` reads `Tilemap::glyph_at` and pushes one cell per press
  (`games/sokoban/src/lib.rs:98`). E1 promotes the **continuous circle+AABB** case (breakout's
  shape); sokoban's grid case stays a tilemap concern (flow-field navigation is game-side per G6).
  **Both games are frozen proofs and must NOT be retrofitted** (roadmap: frozen proofs don't retrofit)
  — they are cited as the "third occurrence" that justifies promotion, not as migration targets.
- **`WorldHash` excludes resources and the free-list; it hashes registered component values of live
  entities.** `WorldHash::compute` walks `world.entities()` (ascending slot-index) and, per entity,
  each registered component in registry order (`crates/ludion-sim/src/hash.rs:99`), and the module
  normatively **excludes resources — including `DeterministicRng` — and allocator internals**
  (`hash.rs:39`). Consequences: (a) a per-frame `SpatialIndex` stored as a resource is *outside* the
  hash — it is derived scratch, so it needn't be `Serialize`; (b) `Collider` *is* a registered
  component, so its authored data (radius/shape/body — static config) enters the hash, appended last
  so no existing hash moves; (c) `move_and_slide` returns a delta the game applies to `Transform`, and
  `Transform` **is** hashed — the observable determinism surface is the resolved translation, exactly
  right.
- **Snapshots serialize only three known resources, not arbitrary ones.** `snapshot.rs` serializes
  `RunState`, `DeterministicRng`, and `Speed` (`crates/ludion-engine/src/snapshot.rs:43,56`) — the
  world's resource map is `Any`-typed and cannot serialize an unknown resource. A scratch
  `SpatialIndex` resource is therefore silently ignored by snapshot/restore (rebuilt next frame),
  which is correct: no snapshot bloat, no `Serialize` requirement.
- **Determinism enforcement is per-crate clippy config that does NOT inherit from the workspace root.**
  A new sim-side crate needs its own `clippy.toml` (`docs/crate-graph.md:128` gotcha). The list to
  copy verbatim is `crates/ludion-sim/clippy.toml` / `crates/ludion-engine/clippy.toml` (identical):
  ban `std::collections::HashMap`/`HashSet` (→ `ludion_core::collections::StableMap`/`StableSet`,
  `crates/ludion-core/src/collections.rs`, `BTreeMap`-backed sorted iteration) and
  `std::time::Instant::now`/`SystemTime::now` (time arrives as `SimTime`/`FrameIndex`). Randomness, if
  ever needed, is the one `DeterministicRng` resource (`crates/ludion-core/src/rng.rs`); transcendentals
  go through `ludion_core::math` (`math.rs`), though E1's math is dominated by `+ - *` and comparisons
  (exact in `f32`) — `sqrt` (`math.rs:62`) is only needed for a *reported* distance, and k-nearest
  ordering uses **squared** distance (exact, no `sqrt`, no float-order ambiguity).
- **Crate-graph placement is unambiguous, and it costs the engine one edge.** Boundary rules
  (`docs/crate-graph.md:33`): `ludion-spatial` sits beside `ludion-sim` on the CPU-only sim side,
  depending only on `ludion-core` (ids, collections, math) and `ludion-ecs` (`World`, `Component`,
  `Registry`, `SemanticValidate`, `SystemCtx`) — **never** `ludion-scene` (it doesn't parse YAML) nor
  anything GPU. Because `builtin_registry()` (in `ludion-engine`) must name `Collider` to register it,
  **`ludion-engine` gains a dependency edge → `ludion-spatial`** (engine is already the heavy sim-side
  hub, so this is free; the reverse — spatial → engine — is forbidden, as it would drag wgpu into a
  content-speed crate). The facade re-exports through `prelude` (`crates/ludion/src/prelude.rs:24`
  re-exports engine types; `:29` re-exports ecs types) — `Collider` (+ its enums) and the `spatial`
  function module join it.
- **Free L-codes: L1116–L1119 (voxel L1113–L1115 and import L1201–L1203 taken).** `codes.rs` (the
  `DECLARED_CODES` table, bijection- and reachability-tested since Phase 9). E1 needs **at most one**
  new code — `L1116` for a `Collider` semantic-validation failure (non-positive / non-finite extent,
  which schema range cannot express) — registered adjacent to the voxel block. Everything else the
  component needs is schema-borne (`L1014`) or the standard component codes.
- **`ludion profile` (the CLI) runs the built-in registry with NO game systems — so it cannot
  benchmark E1's rebuild/queries.** `profile_scene` builds via `Engine::with_project_root` (built-in
  registry) and its `[systems]` section is documented empty because "the CLI cannot link a game's own
  systems" (`crates/ludion-engine/src/profile.rs:56`; `budgets/pinned-rtx3090.toml:14–31` — "a game's
  own budget, profiled in-process, once that path exists"). `spatial::rebuild` is a game-registered
  system, invisible to `ludion profile`. The **existing measurement *mechanism*** to reuse is the one
  in `profile.rs`: median-over-N-frames (`median_ms`, `profile.rs:302`) of a **wall-clock span taken
  in a scoped `#[allow(clippy::disallowed_methods)]`** (the one sanctioned `Instant::now` site,
  `profile.rs:160`, justified as pure observation that cannot perturb the sim). E1's benchmark is a
  crate-level bench that reuses this pattern over a synthetic world — **not** a new criterion
  dependency (the repo has no `benches/`) and **not** `ludion profile`.
- **The editor variant widget only widgets *string*-property object variants.**
  `deriveVariantOptions` (`editor/app/src/viewer/inspect-fields.ts:198`) accepts an object `oneOf`
  branch only when its single required property is typed `string` (`:229` returns `null` otherwise →
  raw-JSON textarea fallback). `Collider.shape`'s object variants carry `radius` (number) and
  `half_extents` (array), so `shape` falls to the **raw-JSON textarea** — the same honest fallback the
  existing `Projection` struct-variant enum uses today. This is *not* a degrade of any existing widget
  (see decision 6). `Collider.body` (a unit-variant enum) renders as a plain enum dropdown.
- **Gallery convention: one scene per feature, auto-paired golden, exactly SSIM 1.0** on the same
  machine (`examples/README.md`; `ludion-testkit::discover`). **But `Collider` is render-inert** —
  `extract()` builds the `RenderScene` from visual components (`Sprite`/`Mesh`/`PointLight`/…) and
  never reads a collision component, and gallery scenes run **no game systems**, so a golden can show
  neither collision *response* (nothing moves it) nor the collider itself (nothing draws it). This
  strains the "gallery where the feature is visible" cross-cutting rule and forces an amendment
  (decision 8).

## Design decisions (settled by this plan)

1. **`Collider` is a builtin component on the ground plane (x, z), registered last.** A plain
   serde/schemars struct in `ludion-spatial`:
   ```rust
   Collider { shape: Shape, body: Body }
   // Shape (externally tagged, snake_case): Circle { radius: f32 } | Aabb { half_extents: [f32; 2] }
   // Body  (unit enum,        snake_case): Static | Dynamic
   ```
   The collider lives at `(Transform.translation[0], Transform.translation[2])` — **height (y) is
   ignored, and `Transform.rotation`/`scale` do not affect it** (the 2D-sim / 3D-eye pillar;
   a private plan, pillar 4). `half_extents` is a 2-vector (x, z), not 3 — there is no vertical
   extent. Serialized form mirrors `MeshSource` external tagging (`components.rs:115`): `body: static`
   is a bare string; `shape: { circle: { radius: 0.5 } }` is a one-key map. Registered **after
   `Tilemap`** via `register_validated::<Collider>("Collider")` (decision 5) so no existing
   component index shifts (all prior world hashes byte-identical) and the semantic validator is wired.
   Defaults (drift-safe — every field survives deserializing `{}`, per
   `assert_components_drift_safe`): `shape: Circle { radius: 0.5 }`, `body: Static`.
   *Rejected:* a 3-vector `half_extents` (invents a vertical extent the sim never uses); reading
   `Transform.scale` into the collider (couples render scale to sim collision — the tilemap already
   sets the "transform-scale-ignored" precedent, `components.rs:314`).

2. **The spatial index is a scratch resource, rebuilt by an explicitly-scheduled system.**
   `spatial::rebuild(world: &mut World, _ctx: &SystemCtx)` has the exact
   `fn(&mut World, &SystemCtx)` system signature, so a game registers it directly:
   `.system("spatial_rebuild", spatial::rebuild)` at a chosen schedule position (regularity over
   magic — no hidden system, order stays visible in the game's `game()`). It walks `world.entities()`,
   reads `(Collider, Transform)`, and stores a `SpatialIndex` **resource**
   (`world.insert_resource`). `SpatialIndex` is *not* `Serialize`: it is derived scratch, outside
   `WorldHash` and snapshots (facts above) — determinism comes from rebuild being a **pure function of
   world content visited in `EntityId` order**, not from persisting it. Queries read the resource; if
   rebuild has not run yet (frame 0 before its turn), queries see an empty index and return empty /
   the input delta (documented; not an error — a legitimate "before first rebuild" state).
   *Rejected:* `rebuild` returning an owned index the game threads by hand (a system cannot return a
   value; threading it defeats the point); an auto-inserted hidden rebuild system (violates the
   no-magic settled scope).

3. **The spatial hash is a uniform grid with a pinned total order.** `SpatialIndex` holds
   `StableMap<(i32, i32), Vec<Entry>>` where a cell key is `(floor(x / cell), floor(z / cell))` and
   `Entry { entity: EntityId, center: [f32; 2], shape: Shape, body: Body }`. `StableMap`
   (`BTreeMap`-backed) iterates cells in sorted key order; within a cell, entries are in insertion
   order = `world.entities()` order = `EntityId` ascending (asserted, not assumed — sorted defensively
   by `EntityId`). **Pinned query order is therefore: candidate cells in sorted `(cx, cz)` order, then
   `Entry`s in `EntityId` order** — the brief's "cell order then EntityId", deterministic by
   construction, conformance-tested by double-run bit-identity (the voxel mesher's pinned-sweep
   precedent, Phase 10 Wave 3). Cell size is a construction constant, tuned by the benchmark (decision
   4 / Wave 2) against the E6 entity mix — a small circle radius and a horde density imply a cell near
   the typical query radius; the value is **chosen from measurement, not guessed, before the API
   freezes.** Out-of-range cells are simply absent from the map (a sparse hash, unbounded world — no
   world-size assumption, mirroring the voxel mesher's out-of-bounds-as-empty seam).
   *Rejected:* a dense grid / fixed world bounds (the city has no fixed extent; sparse is honest);
   sorting query results by distance-then-cell (the brief pins *cell then id* for overlaps/raycast;
   only k-nearest sorts by distance — decision 4).

4. **Queries return `EntityId`s in pinned order; k-nearest breaks ties deterministically; the
   benchmark validates the design before the API freezes.** Three read-only queries over `&World`
   (reading the `SpatialIndex` resource):
   - `overlaps(world, shape, center) -> Vec<EntityId>` — all colliders whose shape overlaps the probe
     (circle-circle, circle-aabb, aabb-aabb — the three closed-form cases), in **cell-then-EntityId**
     order. This is the projectile hit-test (G1) and the general overlap primitive.
   - `raycast(world, origin: [f32;2], dir: [f32;2]) -> Option<RayHit>` where `RayHit { entity, t,
     point }` — the **nearest** ground-plane hit (slab test for AABB, quadratic for circle), ties
     broken by `EntityId`. Returns the first hit only (G1's line-of-sight / projectile-vs-wall).
   - `k_nearest(world, center: [f32;2], k, max_radius) -> Vec<EntityId>` — the k closest collider
     centers within `max_radius`, ordered by **(squared distance, EntityId)** — squared distance is
     exact in `f32`, so the ordering has no last-bit ambiguity. This is separation, aura pickups, and
     nearest-enemy targeting (G1).

     Each query optionally filters by `Body` (e.g. targeting wants dynamics; move-and-slide wants
     statics — decision 7). **The benchmark (Wave 2) runs before Waves 3–5 lock/expose the API**, per
     the roadmap risk "E1 pinned-order performance … benchmark the pinned design against the E6
     numbers before committing the API, not after": if the pinned order fights cache layout badly,
     the fix (cell size, entry layout, even the query signatures) happens in Wave 2, not after G1
     depends on the shape.
     *Rejected:* returning reference tuples like `World::query` (the whole point is to return ids the
     raw query cannot); an iterator API (a `Vec` in pinned order is simpler to test for double-run
     identity and cheap at these counts; revisit only if allocation shows up in the benchmark).

5. **Validation: schema for shape/range, one semantic hook (`L1116`) for what schema can't say.**
   `Collider` registers via `register_validated` (`registry.rs:247`). The `schemars` schema closes the
   `shape`/`body` enums and (via `#[schemars(range(min = 0.0))]`) rejects negative extents as `L1014`
   with a field pointer — for free, in `check-scene` and `component.set`. The `SemanticValidate` impl
   adds **`L1116`** for the strictly-positive/finite invariant schema cannot express: a zero or
   non-finite `radius`/`half_extents` component gets a `Diagnostic` with a component-relative JSON
   Pointer (`/shape/circle/radius`, `/shape/aabb/half_extents/0`) and a nearest-fix suggestion — the
   `Tilemap` semantic-validation precedent (`components.rs:432`). `L1116` is registered in
   `codes.rs` + `DECLARED_CODES` (bijection test enforces it) with an `ludion explain L1116` body.
   *Rejected:* leaning on `range(min)` alone (it is inclusive — a zero-radius collider would validate
   and then divide-by-zero in a query; and it does not reach `half_extents` array elements).

6. **The inspector edits `Collider` with zero widget work — `shape` on the raw-JSON fallback, exactly
   like `Projection` today.** `body` (unit enum) is a plain dropdown via the generic enum widget;
   `shape` (struct-variant enum with number/array payloads) is *unsupported* by
   `deriveVariantOptions` (which only widgets single-**string**-property object variants,
   `inspect-fields.ts:229`) and lands on the **raw-JSON textarea** — the same honest fallback the
   existing internally-tagged `Projection` enum uses. This satisfies "new builtins automatically get
   inspector editing" (every field renders and round-trips; complex fields via JSON is still editing)
   and **degrades nothing** (no existing widget regresses — the contrast with Phase 10, where a new
   `Mesh.shape` variant would have regressed an *existing* widget). A vitest asserts `Collider` renders
   (`body` dropdown, `shape` JSON textarea) and commits both back through `component.set`.
   *Rejected:* generalizing the variant widget to number/array single-property object variants now
   (real work for a field that reads fine as JSON; recorded as a non-goal with a designer-friction
   trigger); flattening `shape` into `{ shape: "circle"|"aabb", radius, half_extents }` to dodge the
   struct variant (dead fields per shape — the exact anti-pattern external tagging avoids).

7. **`move_and_slide` resolves a kinematic move against `Static` colliders only; dynamic-vs-dynamic
   stays game logic.** `move_and_slide(world, entity, delta: [f32;2]) -> [f32;2]` returns the
   **resolved** ground-plane delta for `entity`'s `Collider` swept against every `Static` entry in the
   index (never `Dynamic` — VS-style separation between movers is a game system built on `k_nearest`,
   per the settled scope). It is a pure read (no world mutation): the game applies the returned delta
   to `Transform.translation` [x, z] itself, keeping the one-command-queue discipline and putting the
   only hashed mutation in game code. Resolution is the standard axis-separated slide (attempt full
   delta; on a static overlap, cancel the penetrating axis and retry the other) — deterministic,
   pinned by iterating statics in cell-then-`EntityId` order. If `entity` has no `Collider`, the delta
   returns unchanged (documented; not an error — a non-colliding mover is legal).
   *Rejected:* mutating `Transform` inside `move_and_slide` (hides a hashed write in the library, off
   the game's visible schedule); resolving against dynamics (explicitly out of settled scope — the
   dynamic pair-cost is the game's design budget, not the engine's).

8. **The gallery deliverable is a *layout* golden, and the behavioral proof is the conformance suite —
   an amendment forced by an invisible feature.** `Collider` is render-inert and gallery scenes run no
   systems (fact above), so a golden can show neither collision response nor the collider. **Amendment
   to the cross-cutting "gallery where the feature is visible" rule:** `examples/gallery/colliders.yaml`
   renders `Mesh` primitives (spheres and boxes, lit) whose *visible sizes mirror their colliders'
   `radius`/`half_extents`, each entity also carrying the matching `Collider` — so the golden shows the
   authored collision **shapes and placement** (a human/agent can see the layout), and doubles as the
   **render-inertness proof**: the same scene with the `Collider` components removed renders
   byte-identically (asserted — colliders never touch the `RenderScene`). The **feature's real proof**
   is the pinned-order conformance suite (Wave 3), the microbench (Wave 2), and the G1 integration —
   stated plainly in the plan and the docs so no one mistakes a static layout render for behavioral
   coverage.
   *Rejected:* skipping the gallery scene (breaks the wave-ships-a-gallery-example rule outright); a
   scene that "animates" collision (impossible — gallery scenes have no systems).

9. **Additive-only, NO wire change (confirmed, flagged loudly).** `Collider` is registry-borne, so it
   flows through the existing `scene.load` / `component.get` / `component.set` / `component.remove`
   surface with **no new wire method** — exactly like Phase 10's `MeshSource::Voxel` and Phase 11's
   material fields. Component schemas are not in the protocol snapshot (`schema.rs` assembles wire
   types only), so **`schema_hash` does not move and the wire surface stays 26 methods**. The spatial
   *functions* (`rebuild`/`overlaps`/`raycast`/`k_nearest`/`move_and_slide`) are a Rust library
   surface a game calls in-process — never a wire command (the file/API is the interface; wire spatial
   commands would arrive only if the editor grew a collision-painting UI, with inverse-carrying
   responses, per the standing parity discipline). **If any wave discovers a wire change is needed,
   that is a flagged decision, not a drive-by — none is foreseen.**

10. **A 3D broadphase sits beside the 2D grid (`spatial3d`, Serious Isaac request 0005); additive, no
    prior decision amended.** A game needing hitscan, swept-sphere casts, radius queries and nearest
    neighbours in true 3D (a first-person shooter, a projectile pool) has no answer in the 2D grid:
    decision 1's colliders have no vertical extent. `ludion_spatial::spatial3d` adds `Index3d` next to
    `SpatialIndex`; it replaces nothing.

    The structure is a sorted-median-split BVH, not a 3D grid: item sizes are mixed (0.05 m
    projectiles to multi-metre bosses), so a grid needs a tuned cell size plus an oversize list; a BVH
    adapts by construction. First-hit casts of any length run front to back with best-`t` pruning,
    k-nearest is a plain branch and bound, there is no tunable to calibrate, and the flat arrays
    follow the Wave 2 bench lesson (`StableMap` traversal costs more than fat cells).

    **Pinned order.** The build sorts by `(morton, handle)`; overlaps come out sorted by handle;
    k-nearest by `(distance², handle)`; the first hit by `(t, handle)`. Every result is a pure
    function of the item *set*, never of push order or thread count. Parallelism lives only in the
    build's key pass, in fixed-size chunks.

    **Handles and filters.** A `Handle` is `Entity` or `Slot` (a caller-owned pool's slot), totally
    ordered. A `Filter` is a layer mask plus an `exclude` handle and an `owner` handle, so a projectile's
    cast never hits its shooter or the shooter's owner.

    **`SpatialVolume` is registered last, with a raw `u32` mask and `L1339`.** Sphere or box, centred
    on the entity's world translation (rotation and scale ignored, the axis-aligned precedent of
    decision 1). `layers` is a bitmask of game-defined layers, not tied to the named, rapier-only
    `physics.layers`; `0` is rejected because no filter can see it. Registered after every existing
    component and absent from existing scenes, so no scene, `WorldHash` or replay moves.

    **The `Index3d` resource is outside `WorldHash` and not carried by snapshots (yet).** Readers are
    scheduled after the rebuild, so a restore loses nothing they can see. A snapshot field would move
    `replay_content_hash` and force a corpus re-record, so the carry is deferred to the projectile
    pool (0006), which must ride snapshots anyway; one re-record covers both.

    **The 2D `raycast` is not routed through the new index.** A 2D collider has no vertical extent
    (decision 1) and the 3D index has no infinite-height primitive, so routing would need a
    2D-specific narrow phase anyway; its `t` would come from different formulas, so bit identity with
    the frozen 2D results (and vigil's hashes) could not be guaranteed. The 2D raycast is not on a hot
    path (Status, Wave 2).

    *Rejected:* a uniform 3D grid (a cell-size tunable and an oversize list for mixed sizes, and an
    inflated cell walk for long casts); rapier's query pipeline (bodies only, allocates per call);
    amending decisions 1–9.

## Status

- **Wave 1 landed 2026-07-13.** One amendment to decision 2, forced by the crate-graph rule the
  plan itself records: `ludion-spatial` cannot name `Transform` (that would need a spatial→engine
  edge — the forbidden direction), so `rebuild` is generic over a `GroundPosition` trait and
  `ludion-engine` owns `impl GroundPosition for Transform` (the one legal home of the
  "(translation.x, translation.z), y/rotation/scale ignored" rule). Games register
  `.system("spatial_rebuild", spatial::rebuild::<Transform>)` — the monomorphized fn keeps the exact
  system signature. Wave-1 queries are correct full-scans in pinned order; candidate-cell pruning is
  Wave 2's job alongside tuning the provisional `CELL_SIZE = 2.0`. Pinned collider-free world-hash
  fixture: `ec8755d1de2f3f84`.

- **Wave 2 landed 2026-07-13.** The E6 microbench is `crates/ludion-spatial/tests/e6_bench.rs` — a
  `#[test]` `#[ignore]`d by default, run explicitly with
  `cargo test -p ludion-spatial --release --test e6_bench -- --ignored --nocapture` — reusing the
  `profile.rs` pattern (median over 60 measured frames after 10 warmup; wall-clock spans in a scoped
  `#[allow(clippy::disallowed_methods)]` with the observation-only rationale). Synthetic world at
  the E6 mix (500 dynamic seeker circles + 300 projectile circles + 200 static pickups + 300 static
  AABBs, deterministic `DeterministicRng(0xE6)` layout over a 120×120 arena, dynamics drifting every
  frame); per frame it times one `rebuild`, 500 `k_nearest` (k=8, r=4, dynamics), and 300 `overlaps`
  (projectile circle, unfiltered). One deviation from this wave's text, resolved in the bench's doc
  comment: the "`move_and_slide` sample" cannot be timed because `move_and_slide` is Wave 4's
  deliverable — Wave 4 extends the bench with it. **Candidate-cell pruning landed here** (as the
  Wave-1 note assigned): `overlaps`/`k_nearest` walk only the probe's candidate cell rectangle —
  inflated by the index's `max_reach` for `overlaps` (entries are filed by center), clamped to the
  occupied cell bounds — in the same lexicographic cell order as a full scan, so results and pinned
  order are unchanged (asserted by `pruned_queries_match_a_full_scan_on_a_scattered_layout`; all
  Wave-1 tests pass unmodified). `raycast` deliberately stays a full scan: it is not on the benched
  hot path (G1 issues a handful of line-of-sight rays per frame, not hundreds) and measures in
  microseconds at these counts; a conservative DDA cell-walk is a recorded later option. **Measured
  medians on the pinned machine (release), `CELL_SIZE` sweep 1→2→3→4→6→8→12→16→24 ms frame sums
  0.79/0.63/0.57/0.50/0.44/0.41/0.38/0.37/0.39 — `CELL_SIZE = 16.0` chosen** (coarse cells beat the
  "cell near query radius" prior at E6 density: `StableMap` node traversal costs more than a fat
  cell's cache-friendly linear narrow-phase). At 16.0: rebuild **0.129 ms**, k_nearest×500
  **0.154 ms**, overlaps×300 **0.086 ms**, frame sum **0.369 ms**. **Recorded spatial sub-budget:
  1.5 ms per frame** (~4× the measurement, under the plan's ≤ ~2 ms guidance) — the bench asserts
  it; this is the number the G1/E6 `budgets/*.toml` `[systems]` entries will reference once the
  in-process game-profiling path exists. *(Wave 4 note: the bench now also times 500
  `move_and_slide` per frame — the sample this wave's text originally named — moving the measured
  frame sum to **~0.52 ms** (rebuild 0.131 / k_nearest×500 0.156 / overlaps×300 0.085 /
  move_and_slide×500 0.146); still ~3× under the unchanged 1.5 ms sub-budget.)*

- **Waves 3–6 landed 2026-07-13.** Wave 3: API frozen at decision 4's exact shapes; `RayHit`/`Entry`
  `#[non_exhaustive]`; conformance suite (double-run bit-identity, O(n²) oracle for set AND order over
  seeded worlds, tie-breaks incl. a bit-identical raycast `t` arranged so cell order opposes id order).
  Wave 4: `move_and_slide` (pure read, axis-separated test-and-cancel with documented x-preference,
  statics only, self-excluded by `EntityId`, starting-in-penetration resolves to zero by design);
  WorldHash-trace test via a `ludion-sim` dev-dep (outside the boundary graph). Wave 5: facade prelude
  re-exports (+ `GroundPosition` — the seam a custom position component implements); `colliders.yaml`
  gallery scene + blessed layout golden + byte-identical render-inertness assertion; all 10 prior
  goldens byte-identical; wire surface confirmed 26 methods, schema snapshot unmodified. **Decision 6
  amendment:** the "body renders as a dropdown with zero widget work" claim was factually wrong — the
  editor had no unit-only-enum widget; fixed with a minimal guard relaxation in `deriveVariantOptions`
  (`sawUnit && sawObject` → `sawUnit`), zero new widget code, no existing field's widget changed
  (`shape` stays on the raw-JSON fallback exactly as decided). Wave 6: `docs/ludion-spatial.md` topic,
  engine-guide "Collision and spatial queries" chapter (between Systems and Audio), crate-graph facade
  edge; embedded copies regenerated. **E1 exit gates: all mechanized criteria green — the wave is
  READY per the roadmap.** Handoff open item: G1 integrates and deletes `games/vigil`'s interim
  collision.

## Waves

Ordering rationale: build enough to measure (Wave 1) → **measure before the API commits** (Wave 2,
the roadmap-mandated benchmark) → lock behavior with conformance/property tests (Wave 3) →
move-and-slide (Wave 4) → expose + gallery + degrade-nowhere checks (Wave 5) → docs + exit + handoff
(Wave 6).

### Wave 1 — `ludion-spatial` crate, `Collider` component, spatial hash + rebuild + query implementations
- New crate `crates/ludion-spatial` → `ludion-core`, `ludion-ecs` only; its own `clippy.toml` copied
  verbatim from `crates/ludion-sim/clippy.toml`; `#![forbid(unsafe_code)]` + `#![warn(missing_docs)]`.
- `Collider` + `Shape`/`Body` enums per decision 1; `SemanticValidate` impl + `L1116` in
  `codes.rs`/`DECLARED_CODES` + `ludion explain L1116` body per decision 5.
- `ludion-engine` gains the dependency edge and registers `Collider` **last** in `builtin_registry()`
  (`register_validated`); `docs/crate-graph.md`'s edge list + gotchas updated.
- `SpatialIndex` + `rebuild` (decision 2), the uniform-grid hash (decision 3), and functional
  `overlaps`/`raycast`/`k_nearest` (decision 4) — implemented well enough to benchmark and test, API
  not yet frozen/exposed.
- **Exit gate (mechanized):** crate builds under `cargo clippy -D warnings` with the determinism
  denylist active; `Collider` round-trips through the registry (serialize→deserialize identity) and
  validates in a scene through `ludion_scene::check` (schema + semantic) — good doc validates, and
  each misshape (negative extent → `L1014`; zero/NaN extent → `L1116`; unknown shape key → `L1014`)
  asserts its code + JSON Pointer; `builtin_registry()` length grows by one and every prior
  component's registration index is unchanged (a byte-identical-world-hash assertion on an existing
  fixture); `assert_components_drift_safe` covers `Collider`; `node scripts/check.mjs` green.

### Wave 2 — the benchmark (BEFORE the API freezes) — pinned-order hash vs the E6 numbers
- A crate-level bench (a `#[test]`, `#[ignore]` by default so CI stays fast, run explicitly in
  `--release`) reusing the `profile.rs` measurement *pattern* — median over N frames (`median_ms`
  shape) of a wall-clock span in a scoped `#[allow(clippy::disallowed_methods)]` with the observation-
  only rationale — **not** criterion, **not** `ludion profile` (which cannot link this system; fact
  above). It constructs a synthetic world at the E6 mix (~500 dynamic "seekers" + 300 "projectiles" +
  200 "pickups" + a few hundred statics) in a deterministic layout, then times, per frame: one
  `rebuild`, 500 `k_nearest` (separation/targeting), 300 `overlaps` (projectile hits), and a
  `move_and_slide` sample; reports each median and their sum.
- **This is where cell size and entry layout are tuned** (decision 3): iterate until the per-frame
  spatial total fits a **sim sub-budget** carved from the 16.6 ms frame (a fraction, e.g. ≤ ~2 ms, set
  from the first measurement and recorded — this is the number the G1/E6 `budgets/*.toml` `[systems]`
  entries for the spatial systems will later reference once the in-process game-profiling path exists).
- **Exit gate (mechanized):** the bench prints the medians and asserts the pinned-order design meets
  the recorded sub-budget on the pinned machine; the measured numbers and the chosen cell size are
  written into this plan's status note. If the pinned order cannot meet budget, the API/layout changes
  here (the whole reason the benchmark precedes Waves 3–5). **Open item surfaced for the roadmap:** the
  whole-game budget-scene profiling (`games/vigil/scenes/budget-horde.yaml` through `ludion profile`)
  needs the "game systems profiled in-process" path that does not exist yet
  (`budgets/pinned-rtx3090.toml:28`) — building it is E6/G1 work, out of E1 scope; E1's decoupled
  microbench does not need it.

### Wave 3 — freeze the query API + pinned-order conformance and correctness
- Lock the public shapes of `overlaps`/`raycast`/`k_nearest` (names, return types, `Body` filter);
  apply `#[non_exhaustive]` where the Phase 9 API-freeze convention dictates (result structs like
  `RayHit`; the `Command`-style deliberate-exhaustive exceptions do not apply here).
- **Exit gate (mechanized):** **double-run bit-identity** — building the index twice over the same
  world and running each query twice yields identical `Vec<EntityId>`/`RayHit` (the voxel-mesher
  pinned-sweep precedent); **brute-force oracle** property tests — for random deterministic worlds,
  every query's result equals an O(n²) reference (same set, and for the pinned-order queries the same
  *order*); tie-break tests (co-located entities return in `EntityId` order; equidistant k-nearest
  ordered by `EntityId`); empty-index (pre-rebuild) returns empty; `node scripts/check.mjs` green.

### Wave 4 — `move_and_slide` against statics
- `move_and_slide` per decision 7 (axis-separated slide vs `Static` entries in pinned order, pure
  read, returns resolved delta).
- **Exit gate (mechanized):** unit tests — slide along a wall (tangential component preserved, normal
  component cancelled); corner (both axes blocked → near-zero delta); no static in range → delta
  unchanged; entity without a `Collider` → delta unchanged; **determinism** — same world + same delta
  → same resolved delta twice; a two-run WorldHash-trace test where a driver system applies the
  resolved delta to `Transform` and both runs match (the delta feeds a hashed write, so this is the
  real determinism assertion); `node scripts/check.mjs` green.

### Wave 5 — facade re-export, gallery, degrade-nowhere checks
- `ludion::prelude` re-exports `Collider` (+ `Shape`, `Body`) and the `spatial` module
  (`rebuild`/`overlaps`/`raycast`/`k_nearest`/`move_and_slide`); `prelude.rs` doc list extended.
- `examples/gallery/colliders.yaml` + blessed golden per decision 8 (visible collision-shape layout);
  `examples/README.md` gallery row added (noting the golden shows layout, not behavior).
- **Exit gate (mechanized):** gallery suite green (new golden SSIM 1.0, all priors byte-identical —
  `Collider` is render-inert); the render-inertness assertion (same scene ±`Collider` renders
  identically); **protocol snapshot byte-identical + wire surface still 26 methods + `schema_hash`
  unchanged** (existing tests prove it by construction — decision 9); inspector vitest (`body`
  dropdown + `shape` JSON textarea, both commit) per decision 6; a facade smoke registering `rebuild`
  as a system and calling each query; `node scripts/check.mjs` green.

### Wave 6 — docs + exit gates + handoff
- New crate doc `docs/ludion-spatial.md` (frontmatter `description:`, the shared section skeleton via
  the `ludion-docs` skill) pinning the pinned-order contract, the `Body`/`Static`-only `move_and_slide`
  rule, and the resource/rebuild lifecycle; engine guide gains a **"Collision and spatial queries"**
  chapter (authoring `Collider`, registering `spatial::rebuild` in the schedule, the four query calls,
  a copyable move-and-slide + k-nearest-separation example, and the plain statement that colliders are
  invisible and behavior is verified by tests, not by looking). `docs/crate-graph.md` (already touched
  in Wave 1) and the frontmatter gate cover the rest by construction.
- **Exit gates (roadmap "ready"):** all mechanized wave gates green; the benchmark meets the recorded
  sub-budget (Wave 2); docs topic + guide chapter shipped; gallery golden blessed. **Handoff to G1:**
  G1 adds `Collider` components to player/props/enemies, registers `spatial::rebuild` at an explicit
  schedule position, and **deletes `games/vigil`'s G0 interim hand-rolled collision** (the one-way
  rule — the interim's named replacement phase is G1). No human gate is owned by E1 itself (the
  spatial module is verified by hash/tests, not by eye); the milestone's human gate is G1's combat
  feel. This wave plan's brief-conformance amendments (decisions 6, 8) are recorded in
  `Plan/engine-first-game.md`'s E1 section if scope moved.

## Explicitly out of scope (triggers recorded)

- **Rigid-body dynamics, forces, constraint solving, dynamic-vs-dynamic response** — the settled
  scope's hard line; games write response logic on `k_nearest`/`overlaps`. Trigger: none foreseen; a
  future game would make its own case in its own plan.
- **Rotated boxes, polygon/convex shapes, 3D collision volumes** — trigger: a game that *measurably*
  needs them (the E1 brief's recorded trigger). `half_extents` is axis-aligned x/z only.
- **Incremental / persistent broadphase (rebuild only changed cells), spatial-hash caching across
  frames** — trigger: the Wave 2 benchmark (or a later E6 measurement) showing per-frame rebuild is
  the bottleneck. Until then, rebuild-per-frame is the regular, obviously-correct design.
- **Editor variant widget for number/array single-property object variants** (so `Collider.shape`
  gets a dropdown instead of the JSON textarea) — trigger: recorded designer friction editing
  `shape` (or `Projection`) by JSON. Decision 6.
- **`spatial.*` wire commands / a collision-painting editor UI** — trigger: the editor demands
  authoring colliders visually; would ship with inverse-carrying responses per the parity discipline.
  Decision 9.
- **Flow-field / grid navigation (`ludion-nav`)** — stays game-side (G6 builds a walkability grid from
  static colliders); trigger: a second consumer or a stabilized API (roadmap non-goal).
- **In-process game-systems profiling for `ludion profile`** — the path the whole-game E6 budget scene
  needs; E1's microbench is decoupled and does not build it. Trigger: E6/G1 wiring the budget scene.
  *(Fired 2026-07-15 — the facade's `--profile`/`--profile-check` mode landed for the first E6
  measurement.)*
- **`move_and_slide` depenetration** — the shipped semantics are pure rejection: a move whose
  destination still overlaps a static resolves toward `[0, 0]`, so an entity that *starts* overlapping
  a static (bad spawn, teleport, authored placement) is permanently stuck — every step it could take
  still overlaps, every step resolves to zero. Found 2026-07-15 by G1's first playtest (the spawner
  placed monsters inside building footprints; the whole horde froze). The game-side discipline is to
  never create an entity inside a static (validate candidate positions with `spatial::overlaps` and
  re-draw), which G1 now does. Trigger for an engine-level answer (an escape mode that permits moves
  strictly *reducing* overlap depth, or a `depenetrate` helper): a second game hitting it, or a
  legitimate mechanic (knockback, teleport, moving platforms) that can embed entities faster than
  spawn-time validation can prevent.

## Risks

- **Pinned-order performance (the roadmap's named E1 risk).** Determinism-pinned iteration
  (sorted cells, `EntityId` order) can fight cache-friendly layout. Contained by making the benchmark
  its own wave **before** Waves 3–5 commit the API (Wave 2), with cell size and entry layout tuned
  from measurement and the sub-budget recorded — the roadmap's explicit "benchmark before, not after"
  discipline, scheduled, not hoped.
- **Determinism is *claimed*, so it is pinned and double-run-tested, never emergent.** The cell→entry
  total order is specified (decision 3) and Wave 3 asserts double-run bit-identity + a brute-force
  oracle for both membership and order — the voxel-mesher precedent. Any later broadphase optimization
  must reproduce the pinned order or re-bless deliberately.
- **The invisible-feature gallery gap.** `Collider` renders nothing and gallery scenes run no systems,
  so the golden proves layout + render-inertness, not behavior (decision 8, an explicit amendment).
  The risk is someone reads the static golden as behavioral coverage; mitigated by stating in the plan,
  the README row, and the guide chapter that behavior is proven by the conformance suite and G1.
- **Registration-order discipline.** `Collider` must register **last**; an accidental insert mid-order
  would shift every later component's index and re-bless every world hash. Mitigated by the Wave 1
  byte-identical-hash assertion against an existing fixture and the append-only registry contract.
- **Query returns ids that raw `World::query` cannot** — the deliberate ergonomic delta from the
  hand-rolled `world.entities()` walks. The library must keep the `EntityId` alongside the data at
  rebuild time; a query that reconstructed ids from positions would be both slower and ambiguous. Kept
  honest by `Entry` carrying `entity: EntityId` from the start.
- **Sequencing vs open gates.** E1 shares only `codes.rs` (append-only `L1116`) and docs with the
  still-open Phase 8/9/10/11 human gates; no L-code collision (voxel/import/PBR added none in
  L1116+), no wire change. Safe to run with those gates open — but per roadmap rule 2, E1 is the one
  engine wave in flight during M0, and it must be *ready* before G1 starts its collision migration.
