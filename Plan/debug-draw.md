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
3. **`perception` shipped in `Plan/perception.md` W5** (decisions 40-44), through these primitives.
4. **Accepted by the owner 2026-11-19:** amend `Plan/character-controller.md` decision 17 to list these
   primitives and categories (a one-line pointer to this plan is in it now).
5. **Accepted by the owner 2026-11-19:** `nav_view_radius` as a host tunable. Trigger: a level where 30 m is wrong.
6. **Accepted by the owner 2026-11-19:** a scene-level nav config so the built-in `ludion render` host can draw `nav`. Trigger: a
   nav golden.
7. **The `nav` view grows to the 0004 features (W2).** `nav:grid` draws every class grid: class 0 keeps its
   colours and lift, class `k` takes a palette tint (blue, orange, green, violet, yellow, teal, cycling) and a
   further 0.03 m of lift per class. `nav:links` (new) draws each `NavLink` as a 12-segment Bézier arc with
   arrowheads, magenta, red when an end snaps for no class the link serves (the `build_links` rule, shared
   through `link_serves` and `snap_link_end`), dim when disabled. `nav:blockers` (new) draws each
   `NavBlocker`'s authored box by mode (red block, amber cost, blue interact), dim when disabled, never its
   inflated node coverage. `nav:flow` draws the cached fields of every class. Links and blockers are never
   culled; only flow and grid are, and the segment cap still truncates the grid last, later classes first.

## Waves

- **W1 — shapes and the nav view.** The six primitives, `nav`, `nav:grid`, `nav:routes`, `nav:flow`;
  tests, the `debug_draw` example, `docs/debug-draw.md`, `docs/nav.md`. **Mechanized.**
- **W2 — nav classes, off-mesh links, blockers.** Decision 7: per-class grids and flow, `nav:links`,
  `nav:blockers`. **Mechanized.**
- **W4 — goldens (0005-03).** (a) Decision 6's scene-level `nav:` config, so the built-in render host
  installs `Nav` and runs `nav_update` before drawing; (b) a `nav` gallery scene with two classes, a link
  and three blockers, rendered with `--debug-draw nav`; (c) a primitives golden through a game-free pen host
  (designed there). Each is a pinned bless on the RTX 3090.
- **W3 — `perception`.** Shipped in `Plan/perception.md` W5. **Mechanized.**

## Status

- **W1 (2026-11-18).** Shapes in `ludion-engine`'s `debug_draw.rs`; the nav line builder in `nav.rs`
  (`NAV_VIEW_RADIUS` 30 m, `NAV_MAX_SEGMENTS` 200,000). Pinned: `sphere_and_capsule_output_is_pinned`.
  No golden, schema, replay or hash moved.
- **W3 (2026-11-18).** The host-side `perception` category (`perception_debug.rs`) and the crate-private `pen_lines`
  in `debug_draw.rs`. No golden, schema, replay or hash moved.
- **W2 (2026-11-21).** Per-class grids and flow, `nav:links`, `nav:blockers` in `nav.rs`; `link_serves` and
  `snap_link_end` extracted from `build_links`. No golden, schema, replay or hash moved.

## Out of scope

- Text labels and dashed lines (none; a dimmer colour stands in). Trigger: a game that cannot read an overlay without them.
- Per-class sub-filters (`nav:grid:<class>`). Trigger: two overlapping class grids unreadable in play.
