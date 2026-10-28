---
description: Debug draw — world-space lines a system draws under a named category, and how a host shows them
---

# Debug draw

**Scope:** drawing world-space lines from a system (`debug_draw`), choosing which categories a host
shows (`DebugCategories`), and the two offline render flags. The wire toggle is `render.debug`.
Runnable: `cargo run -p ludion --example debug_draw`.

## The pen

```rust
fn draw_motor(world: &mut World, _ctx: &SystemCtx) {
    debug_draw(world, "motor")
        .capsule([0.0, 0.5, 0.0], [0.0, 1.5, 0.0], 0.5, [0.1, 1.0, 0.3, 1.0])
        .ray([0.0, 1.0, 0.0], [2.0, 0.0, 0.0], [1.0, 0.8, 0.1, 1.0]);
}
```

`debug_draw(world, category)` returns a chainable `DebugPen`. Points are world space, colors linear
RGBA. Primitives: `line(a, b)`, `ray(origin, dir)` (its length is the ray's), `path(points)` (a
polyline), `cross(p, size)` (three axis lines, `2 * size` long), `axes(origin, rotation, size)` (rotated +X red, +Y green, +Z blue, `size` long, `rotation` a unit `[x, y, z, w]`), `aabb(min, max)`, `sphere(center,
radius)` (three great circles), `capsule(a, b, radius)` (`a == b` is a sphere). Categories are
`&'static str`; `"all"` is reserved for the host's filter.

## Categories

A category is a name a system draws under and a host enables: `motor` (the character motor's own: `ludion docs motor`), `lane` (its lane constraint: `ludion docs lane`), `camera`, `ai`. The engine
emits `path` — every `PathFollower`'s route, and an oriented follower's axes — `hierarchy` — a line from each parent's origin to its child's (`ludion docs hierarchy`) — and `probes` — each `ReflectionProbe`'s volume wireframe (box edges, three sphere great circles) and a cross at its capture point, coloured by rank like `--debug-view probe_index`, gray past the shaded cap — and `colliders` — every body in the physics world as a wireframe in its world pose (below). `DebugCategories::parse("path,motor")` builds a
filter; `all` enables every category, an empty list none (the default).

### `colliders`

Host-side, like `hierarchy`: drawn from the live physics world, current even paused, free unless
enabled. Boxes, spheres, capsules and cylinders draw parry outlines; a `mesh`, `convex` or
`heightfield` draws its triangle edges **minus the coplanar internal ones** (a flat floor reads as its
outline); a `compound` draws each part in its part pose. Colour by body kind (linear RGBA): static grey
`[0.55, 0.55, 0.55]`, kinematic cyan `[0, 0.85, 0.85]`, rigid orange `[1, 0.45, 0.05]`, a sleeping rigid
body at half that. A sensor draws green whatever its body kind (`[0.1, 0.5, 0.2]`), brighter while occupied (`[0.2, 1, 0.35]`). Budget: at most 200,000 segments a frame, bodies in creation order, no distance
cull (a level under 50k triangles is far below it). Non-`rigid` `circle`/`aabb` colliders live in the
2D spatial index, not the physics world, so they are **not drawn**; a `rigid` one is. Typical use:
`ludion render --scene S --out o.png --frames 2 --debug-draw colliders`.

## What a frame shows

- **The last completed tick.** The engine rolls the drawing over at each frame boundary, so a paused
  sim keeps its final drawing and nothing accumulates across ticks.
- **Host state, never sim state.** The filter lives on the `Engine` (`set_debug_draw`), not in the
  world: it is never hashed, snapshotted or recorded, and no system can read it. That is why the pen
  **always records** whether or not anything is shown — a system that branched on "is debug draw
  on" would make the sim depend on the host. A session with every category enabled hashes and
  replays identically to one with none.
- **1-px lines, after tonemapping.** They draw over the finished frame, before any UI, so bloom,
  depth of field and tonemapping never touch them.
- **X-ray.** A line behind scene geometry still draws, at 30% opacity: a hidden collider outline
  stays readable. A frame with no debug lines is byte-identical to one from an engine without the
  feature, and its frame graph has no extra pass.

## From the command line

- `ludion render --scene S --out o.png --debug-draw path` — the built-in host, which runs **no game
  systems**, so it shows the engine's own categories (`path`, `hierarchy`, `probes`, `colliders`). Combine with `--frames N` to see a
  follower after N ticks. Not valid with a `--frames a,b,c` strip, `--model`, `--sprite`, `--atlas` or
  `--sweep`.
- `<game> --scene S --render-out o.png --frames N --debug-draw motor` — the game's binary, which
  runs the game's systems, so its own categories draw.

The summary line echoes the request: `(debug draw: path)`. A category that drew nothing is named —
`(debug draw: motr,path; nothing drawn for: motr)` — so a typo shows.

## Over the wire

`render.debug` is the same filter as a protocol command. `{}` reads; `{"categories": ["motor"]}` sets
(`[]` none, `"all"` every category). The reply carries `enabled` (the set in force) and `drawn` (the
categories the last tick emitted, whether enabled or not — a category you enabled that is missing
from `drawn` is a typo or a quiet scene). It applies to the live stream and to `render.screenshot`,
so an agent that enables `motor` gets the lines in its screenshot. There is no per-call opt-out.
`view` (`"ssr_confidence"`, `"ssr_footprint"`, `"probe_index"`, or `null` to clear) shows a debug view in place of
the composite; a view never turns SSR on (`ludion docs wire-render`).
Runnable: `cargo run -p ludion-engine --example render_debug`.

## Gotchas

- The pen on a world with no `DebugDraw` resource (a bare `World::new()`) draws nothing.
- `debug_draw` is per tick and per category: draw every tick you want the line shown.

## Next

- `ludion docs path` — `PathFollower`, the source of the `path` category.
- `ludion docs systems` — writing the systems that draw.
- `ludion docs visuals` — the rest of the renderer's content.
