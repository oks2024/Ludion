# Plan: Surface tags — what a hit is standing on

> Status: **W1–W2 mechanized** (declared surfaces, `SurfaceTag`, `SurfaceOverlay`, `surface` on every hit that names a collider); W3–W4 planned. Origin: Neon Shroud request 0009 (footstep noise by floor material). Parent plans: `Plan/physics.md`, `Plan/physics-collision-layers.md` (the side-table pattern this mirrors), `Plan/character-controller.md` (its non-goal "Per-triangle surface tags" is lifted as tags only).

## Trigger

The first consumer is the external stealth game Neon Shroud (request 0009): the footstep noise a guard hears depends on the floor under the player — concrete, a metal grate, a puddle. The motor already reports `ground.entity`; the game needed the engine to answer "which surface" so noise tables stay game data.

## Decisions

1. **`SurfaceTag` is a side component, not a `Collider` field.** `Collider` has ~128 full struct literals across crates and games; a field breaks all of them. A tag is query-only data the solver never reads — the `QueryLayers` class: an entity-sorted side table refreshed by the write, resolved against the settings, an undeclared name degrades with one rejection and keeps its body. Live edits are free (no rebuild). Per-part tags later extend `SurfaceTag` (a `parts` list), not `CompoundPart`.
2. **Per-scene `physics.surfaces`.** A list of names; the order is the `SurfaceId`; at most 256. Optional `default_surface` is reported for any untagged collider hit (motor capsules included).
3. **Resolution order:** `SurfaceOverlay` containing the hit point → entity `SurfaceTag` → `default_surface` → `None`. Later waves insert triangle/cell → part between the overlay and the entity tag.
4. **An undeclared tag at runtime** is one `L1690` rejection; its hits report the default; the body is kept. `check-scene` reports it statically with the nearest declared name (`L1690`), an empty or repeated name or more than 256 is `L1691`, a tag with no claimed collider or motor is `L1692`.
5. **The engine stores no scalar per tag.** Noise and sound tables are game data.
6. **`QueryHit`, `PointHit`, `GroundHit` and `SurfaceHit` gain the public `surface` field**, not `#[non_exhaustive]` — ratified 2026-11-18 (no game constructs these literals); an additive change recorded in `Plan/api-freeze-engine.md`.
7. **The snapshot schema omits the new `scene_physics` fields — Proposed, owner ruling owed.** `surfaces`/`default_surface` on `ScenePhysicsDoc` carry `#[schemars(skip)]` (serde default and skip-when-empty kept), because a schema change moves `replay_content_hash` and fails every committed `.lrp` (L3010). Precedent: the snapshot's `unknown` field, `Plan/game-time-scale.md` decision 9. The schema names them again at the next corpus re-record. Guard: `the_snapshot_schema_omits_the_surface_fields`.
8. **`SurfaceOverlay { surface, priority }` needs a sensor volume.** It sits on an entity with a solver-claimed `Collider` with `sensor: true` (`L1693` otherwise); `priority` is an `i32`, default 0. Any hit whose point lies inside one or more overlays reports the highest priority's surface, ties to the lowest `EntityId`. Read at query time from the last completed step's pose (a moved kinematic overlay is seen from the next step). An undeclared overlay name is one `L1690` rejection and the overlay is inert. Tags only: an overlay never reaches the solver.
9. **`surface_under(point, reach, filter)`** answers "what is under this point": an overlay containing `point`, else a downward ray of `reach` through `filter`, resolved by `surface_at(entity, point)` at the hit point; `Ok(None)` on no hit. `surface_at(entity, point)` is the shared resolver (overlay, then the entity's tag) the query sites use. `surface_under` returns `Result<Option<SurfaceId>, PhysicsError>` (`Err(QueryInput)` on a non-finite point or a negative/non-finite reach), a deliberate divergence from request 0009's `Option<SurfaceId>` so every query reports bad input the same way.

## Waves

- **W1 — this wave.** `physics.surfaces`/`default_surface` (parse, emit, validate, snapshot), `SurfaceTag`, `SurfaceTable`/`SurfaceId`, `PhysicsWorld::{surface_id, surface_name, surface_of, sync_surface_tags}`, `surface` on the four hit structs, `check-scene` L1690/L1692, docs (`collision`, `motor`, `physics`).
- **W2 — overlays (mechanized).** `SurfaceOverlay { surface, priority }` and `PhysicsWorld::surface_under(point, reach, filter)` (puddles: the overlay wins at the hit point, ties lowest entity).
- **W3 — finer than the collider.** Per-part (`SurfaceTag.parts`), mesh `surfaces`/`triangle_surfaces` run-length, `ludion import` from glTF material / `extras.ludion_surface`, heightfield `surface_cells`, face resolution (ray feature id; shape-cast and motor nearest face, lowest index on ties).
- **W4 — surround.** `ContactEvent.surface_a/b`, map-kit `collider: { …, surface }`, a `surfaces` debug-draw category, a `surface-tags` gallery scene and golden, the motor-trace field.

## Out of scope

The 2D tier (`RayHit`, `move_and_slide`) — no trigger; nav costs by surface; voxel palettes; a BVH over overlays (trigger: a profile showing overlay resolution above 0.05 ms/tick; today a linear scan with an AABB reject).

**Proposed — owner ruling owed:** lifting `Plan/character-controller.md`'s non-goal (as tags only); a project-wide default surface list in `ludion.toml`; the mesh-format addition (W3); the map-kit `surface` key (W4).

## Status

- **W1 mechanized.** Codes `L1690`–`L1692`. Tests: `crates/ludion-physics/tests/surface_tags.rs` (strip walk, queries/probe/wall agreement, live edit, undeclared name, no surfaces, restore), `ludion-scene` unit and round-trip tests, `scene_check.rs` unit tests, the CLI `an_undeclared_surface_is_l1690`, the engine snapshot round trip. No trace, replay or golden moves: an absent tag is byte-identical and the snapshot fields are skipped when empty.
- **W2 mechanized.** Code `L1693`; `SurfaceOverlay`, `PhysicsWorld::{surface_at, surface_under}`, overlays refreshed by `sync_surface_tags`. Tests: `crates/ludion-physics/tests/surface_overlays.rs`, `scene_check.rs` unit tests, the CLI `an_overlay_on_a_solid_is_l1693`. Only `component_schemas.json` moves; no trace, replay or golden.
