# Plan: Debug-draw shapes and the `nav` view

> Status: **Waves 1, 2, 3 and 4 mechanized; pinned goldens owed.** Driven by Neon Shroud request 0005 (debug-draw shapes and a nav
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

8. **A scene may author a top-level `nav:` block (W4a).** Its keys are `NavConfig`'s fields, every one
   optional: the mapping is merged over `NavConfig::default()` and deserialized, so a scene writes only what
   differs; `classes` items are `NavClass`, all five keys required. Only the built-in hosts (`ludion render`,
   `ludion test`, the testkit's bare goldens, frame/view/sequence previews) read it, installing `Nav` and
   refreshing the grid before every extract; a game's engine configures nav with `.nav(..)` and ignores the
   block. `scene.save` preserves it; `--set` refuses it (like `physics`); maps cannot declare it. Errors:
   unknown key, wrong type or a class missing a field is `L2330` at `/nav/...`; `NavConfig::validate` failures
   keep `L4032` re-rooted under `/nav`; a `nav:` without `physics:` is `L2331` (the grid samples static
   colliders). Absent `nav:` is byte-identical everywhere.

9. **The primitives golden is a driven golden through a game-free pen host (W4b).** The pen is a system-side
   API, so `common::debug_shapes_builder` (the testkit's shared gallery module, no game crate, the pattern of
   `camera-rig` and `camera-chase`) registers one system, `draw_shapes`, that draws every primitive each tick under
   `shapes` on a 5 x 3 grid of 2.5 m cells; the `debug-shapes` scene is the floor and camera, and the golden
   renders it for one frame with `debug_draw: ["shapes"]`. No engine API, component or scene key: a scene-authored
   pen would be a second way to draw debug lines. Trigger to revisit: a designer needs debug shapes with no Rust.

## Waves

- **W1 — shapes and the nav view.** The six primitives, `nav`, `nav:grid`, `nav:routes`, `nav:flow`;
  tests, the `debug_draw` example, `docs/debug-draw.md`, `docs/nav.md`. **Mechanized.**
- **W2 — nav classes, off-mesh links, blockers.** Decision 7: per-class grids and flow, `nav:links`,
  `nav:blockers`. **Mechanized.**
- **W4a — scene `nav:` and the nav golden (0005-03).** Decision 8: the scene-level `nav:` config, so the
  built-in render host installs `Nav` and refreshes the grid before drawing; a `nav` gallery scene with two
  classes, a link and three blockers, rendered with `--debug-draw nav`. **Mechanized.**
- **W4b — primitives golden (0005-04).** Decision 9: the `debug-shapes` gallery scene and a game-free pen host
  (`common::debug_shapes_builder`) drawing every primitive. **Mechanized.** Pinned bless on the RTX 3090 and the M4.
- **W3 — `perception`.** Shipped in `Plan/perception.md` W5. **Mechanized.**

## Status

- **W1 (2026-11-18).** Shapes in `ludion-engine`'s `debug_draw.rs`; the nav line builder in `nav.rs`
  (`NAV_VIEW_RADIUS` 30 m, `NAV_MAX_SEGMENTS` 200,000). Pinned: `sphere_and_capsule_output_is_pinned`.
  No golden, schema, replay or hash moved.
- **W3 (2026-11-18).** The host-side `perception` category (`perception_debug.rs`) and the crate-private `pen_lines`
  in `debug_draw.rs`. No golden, schema, replay or hash moved.
- **W2 (2026-11-21).** Per-class grids and flow, `nav:links`, `nav:blockers` in `nav.rs`; `link_serves` and
  `snap_link_end` extracted from `build_links`. No golden, schema, replay or hash moved.

- **W4a (2026-11-22).** `nav:` in `ludion-scene` (`nav_block.rs`, parse, overrides, emit) and `scene_nav.rs` in
  `ludion-engine` (`install`, diagnostics); `nav::refresh`; the `nav` gallery scene. No schema, replay or hash
  moved; goldens owed on the pinned boxes.
- **W4b (2026-11-22).** No engine change; goldens owed on the pinned boxes: `debug-shapes.png` (bare) and
  `debug-shapes-driven.png` on the RTX 3090 and the M4.

## Out of scope

- Text labels and dashed lines (none; a dimmer colour stands in). Trigger: a game that cannot read an overlay without them.
- Per-class sub-filters (`nav:grid:<class>`). Trigger: two overlapping class grids unreadable in play.
- A game engine reading the scene `nav:` block. Trigger: a game that wants one nav config for its engine and the built-in hosts.
