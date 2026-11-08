---
description: Wire session levers: render.gi_rebake, render.size, render.view free camera, render.debug
---

# Wire: the host's render session levers

**Scope:** The `render.*` methods that set *host* state over a raw socket — handshake and envelope are `ludion docs wire-protocol`. All of them are host state, not world state: never hashed or snapshotted, and only `render.size` is recorded. Every field is optional; empty params `{}` is a pure read, and every reply is the *resulting* state, so a client never guesses whether its request was taken verbatim.

## Held GI and knowing when a picture is stale: `render.gi_rebake`

It holds or releases the global-illumination caches so a live scene whose movers are unmarked stops
paying a full revoxelize and bake every frame (`ludion docs global-illumination`). An omitted `mode`
leaves the lever alone:

```jsonc
// → hold the current lighting baseline; `{}` alone would just report
{"v": 1, "id": 3, "method": "render.gi_rebake", "params": {"mode": "hold"}}
// ← the lever after the call, plus the last produced frame's staleness
{"id": 3, "ok": {"kind": "gi_rebake", "mode": "hold", "stale": false}, "frame": 12, "game_frame": 12}
// → refresh the baseline once, without leaving hold
{"v": 1, "id": 4, "method": "render.gi_rebake", "params": {"rebake": true}}
```

It is never hashed, snapshotted or recorded, so a held session still records a replay that
reproduces bit-for-bit elsewhere. Holding means what you see is lit for a world that has moved on, so
staleness travels *with* the picture: `render.screenshot`'s reply carries `"stale": true|false`, and
every streamed frame's binary `LFR1` header carries it as bit 0 of the flags byte at offset 5 (a
decoder that ignores that byte simply never sees a stale frame).

## The render extent, never hardcoded: `render.size`

Sets the extent every streamed frame is produced at (and the default a dimensionless
`render.screenshot` uses), or reads it with empty params:

```jsonc
// → halve the pixel count the host has to move
{"v": 1, "id": 5, "method": "render.size", "params": {"width": 960, "height": 540}}
{"id": 5, "ok": {"kind": "render_size", "width": 960, "height": 540}, "frame": 12, "game_frame": 12}
```

Both fields are optional *together*: one axis alone, a zero, or an axis past 7680×4320 is `L3025` and
leaves the extent untouched. The extent is shared by every attached client — **last writer wins** —
and a host can start at a chosen extent with `--render-size WxH` or `LUDION_RENDER_SIZE` (`L0017`).

**A client must read the extent, never assume 1280×720.** `render.bounds` reports it as
`render_size`, in the same reply as the screen rects it is the pixel space for. A viewer that
hardcodes the old default mis-maps every pick and gizmo hit-test by the scale ratio the moment
anything resizes the session. Likewise `input.inject`'s `mouse_move` positions are render-target
pixels: unproject against the extent you read. A recording carries the extent it was armed at and
records each applied `render.size`, so a replay runs at its recorded extent — but sessions played at
different extents are different input traces, so golden session recordings are made at the host's
default extent.

## A free camera without moving the scene's own: `render.view`

Sets or clears a free pose the *stream* renders through instead of the scene's `Camera`:

```jsonc
// → translation, Euler-XYZ degrees rotation, an optional named field of view
{"v": 1, "id": 6, "method": "render.view", "params": {"pose": {"translation": [4.0, 2.0, 6.0], "rotation": [-15.0, 30.0, 0.0], "fov_y_deg": 50.0}}}
{"id": 6, "ok": {"kind": "render_view", "pose": {"translation": [4.0, 2.0, 6.0], "rotation": [-15.0, 30.0, 0.0], "fov_y_deg": 50.0}, "cull_from": "view"}, "frame": 12, "game_frame": 12}
```

`{}` alone reports the override without changing it, and `{"pose": null}` clears it — the three wire
shapes a plain `Option<Option<..>>` field exists to tell apart. An omitted `fov_y_deg` inherits the
scene camera's (or 60° when the world has none); a non-finite coordinate, or a `fov_y_deg` outside
the open interval `0..180`, is `L3032` and refuses the whole pose, leaving the override as it was.

`cull_from` (`"view"` default, `"game"`, `"frozen"`; absent keeps the mode in force) picks the camera
the GPU cull's camera view tests against while the image is still drawn through the flown view:
`game` follows the live scene camera, `frozen` holds the stream camera as the command applied (send
it again to recapture; `view` or `game` drop it). It shows what the game would cull, so a mesh the
cull camera cannot see is absent from the flown frame. Only the camera view moves; shadow views
cull as always. The reply always carries the mode. A `frozen` that cannot capture (an ambiguous scene
camera) applies nothing, the pose included. Like the pose it is host state, never hashed or recorded.

The override is never written to a scene, folded into `WorldHash`, captured by `engine.snapshot`, or
recorded into a replay (unlike `render.size`). A session flown to a rooftop hashes, records and
replays identically to one that never moved, and a frame after `{"pose": null}` is byte-identical to
one before the override was ever set.

**It moves the stream, not every query.** `render.screenshot` and `render.bounds` each take an
optional `camera` per call: `"scene"` (default) always answers through the world's own `Camera`, and
`"view"` looks through the override in force, falling back to the scene's camera when none is set. A presentation `CameraOffset` moves the scene camera's `render.bounds` rects with the image; the override is not offset (`ludion docs camera`).
Existing agent calls, tests and CLI paths keep the game's own framing while a human flies the viewport:

```jsonc
{"v": 1, "id": 7, "method": "render.bounds", "params": {"camera": "view"}}
```

An entity hidden by `Visibility` is still reported, with `hidden: true` (absent otherwise).

## What the game draws for debugging: `render.debug`

Games record world-space debug lines (a character's motor, a platform's path) under named categories.
`render.debug` chooses which categories the host renders into the stream *and* into
`render.screenshot`, so one screenshot can explain itself:

```jsonc
// → a list sets, [] turns everything off, "all" turns everything on
{"v": 1, "id": 8, "method": "render.debug", "params": {"categories": ["motor", "path"]}}
// ← the set in force, and the categories the last tick actually emitted lines for
{"id": 8, "ok": {"kind": "render_debug", "enabled": ["motor", "path"], "drawn": ["path"]}, "frame": 12, "game_frame": 12}
```

Any string is accepted; a category nobody draws is absent from `drawn`, which is how to tell a typo
from a quiet scene.

`view` shows a debug view in place of the composite, the same views as
`ludion render --debug-view`: `"ssr_confidence"`, `"ssr_footprint"`, `"local_shadow_atlas"` (the local-light shadow atlas, letterboxed: `ludion docs local-shadows`), `"sun_cascades"` (each lit surface tinted by its sun cascade: `ludion docs sun-cascades`) or `"probe_index"` (the winning
reflection probe per pixel, coloured by rank; the `probes` category draws the volumes). A name sets it, `null` clears it,
absent keeps it; the reply's `view` is the one in force. A view never turns SSR on: on a frame without
SSR the normal frame is drawn (`render.dump_graph` shows whether `ssr_debug` is scheduled).

```jsonc
{"v": 1, "id": 9, "method": "render.debug", "params": {"view": "ssr_confidence"}}
{"id": 9, "ok": {"kind": "render_debug", "enabled": [], "drawn": [], "view": "ssr_confidence"}, "frame": 12, "game_frame": 12}
```

## Next

- `ludion docs wire-protocol` — handshake, envelope, events.
- `ludion docs global-illumination` — what the held caches are.
