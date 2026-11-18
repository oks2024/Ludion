# Plan: Debug-draw shapes and the `nav` view

> Status: **Wave 1 mechanized.** Driven by Neon Shroud request 0005 (debug-draw shapes and a nav
> view). Game-facing doc: `ludion docs debug-draw`. The base feature is `Plan/character-controller.md`
> decision 17.

## Context

The pen (`debug_draw`) had lines, boxes, spheres and capsules only. A stealth game draws sight cones,
hearing circles and a nav overlay while it tunes AI, and drew them from `line` loops in every game
(Neon Shroud request 0005). Wave 1 adds the shapes and the engine's own `nav` view.

## Decisions

1. **Six primitives on `DebugPen`:** `circle`, `arc`, `wedge`, `ring_sector`, `cone`, `frustum_wedge`.
   Angles are degrees, a positive sweep is right-handed about the plane normal. The chord count is
   size-adaptive and deterministic: the full-circle count keeping a chord's sagitta near 2 cm, clamped
   to 8..=64 (1 m is 16, today's count), scaled by the swept fraction. Degenerate input (non-finite,
   zero normal, `forward` along the normal, non-positive size, an angle out of range) draws nothing and
   reports nothing: the pen is a render-only surface and its input is often a live game value.
   `sphere` and `capsule` are frozen: a bit-pin test holds their output, so no existing drawing moves.
2. **`nav` is a host-side category**, drawn at extraction from the `Nav` resource and the `NavRoute` /
   `NavAgent` components like `colliders`: read-only, never a flow-field build, never hashed. Sub-filters
   `nav:grid`, `nav:routes`, `nav:flow` (`nav` is all three; `DebugCategories::parse` already passes the
   colon through). Flow arrows and grid nodes draw within a fixed 30 m of the game camera, routes are
   never culled, and a 200,000-segment cap truncates the grid last.
3. **`perception` belongs to `Plan/perception.md` W5**, which will use these primitives. Nothing here
   names it.
4. **Proposed — owner ruling owed:** amend `Plan/character-controller.md` decision 17 to list these
   primitives and categories (a one-line pointer to this plan is in it now).
5. **Proposed:** `nav_view_radius` as a host tunable. Trigger: a level where 30 m is wrong.
6. **Proposed:** a scene-level nav config so the built-in `ludion render` host can draw `nav`. Trigger: a
   nav golden.

## Waves

- **W1 — shapes and the nav view.** The six primitives, `nav`, `nav:grid`, `nav:routes`, `nav:flow`;
  tests, the `debug_draw` example, `docs/debug-draw.md`, `docs/nav.md`. **Mechanized.**
- **W2 — nav classes, off-mesh links, blockers.** Overlays for them (an unsnapped link in red). Each
  lands with its Neon Shroud 0004 wave; those features do not exist yet.
- **W3 — `perception`.** Rides `Plan/perception.md` W5.
- **Goldens of the primitives on the pinned GPUs.** Owed: needs a game-free host that runs a system (the
  built-in render host runs none).

## Status

- **W1 (2026-11-18).** Shapes in `ludion-engine`'s `debug_draw.rs`; the nav line builder in `nav.rs`
  (`NAV_VIEW_RADIUS` 30 m, `NAV_MAX_SEGMENTS` 200,000). Pinned: `sphere_and_capsule_output_is_pinned`.
  No golden, schema, replay or hash moved.

## Out of scope

- Text labels and dashed lines (none; a dimmer colour stands in). Trigger: a game that cannot read an overlay without them.
- A scene-level nav config and a `nav_view_radius` tunable (decisions 5, 6).
- The `perception` category (decision 3).
