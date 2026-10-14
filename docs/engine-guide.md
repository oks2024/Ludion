---
description: The index of every game-facing topic — what each covers and the command that fetches it
---

# The Ludion engine guide

This file is an index: every chapter is its own topic.

`ludion docs` with no argument lists every game-facing topic and its one-line description;
`ludion docs <name>` prints one, and `--section "<heading>"` prints just one section of it. Pull a
topic only when you're about to work in that area. Some topics also cite
`ludion devdocs <name>`: engine-internals topics that resolve only inside an engine checkout, and
report `L0008` from a game project.

## Contents

- `ludion docs project-anatomy` — what `ludion new` scaffolded, file by file, and the content root.
- `ludion docs getting-started` — the starter game end to end: component, system, wiring root.
- `ludion docs components` — declaring components, drift safety, and the core scene/render set.
- `ludion docs components-ui` — `UiRect`/`UiText`/`UiList`/`Text`: the UI and world-text components.
- `ludion docs components-fx` — `Sprite`/`Tilemap`/particles/patterns/`Animation`/`AudioEmitter`.
- `ludion docs systems` — queries, time, input, RNG, determinism rules, presentation state.
- `ludion docs game-clock` — freezing gameplay time on demand while the sim keeps ticking.
- `ludion docs scene-format` — the strict-YAML scene document and its canonical form.
- `ludion docs tilemaps` — the `Tilemap` glyph grid, read by the renderer and your logic.
- `ludion docs collision` — 2D colliders, the spatial index, move-and-slide.
- `ludion docs physics` — rigid bodies; `collision-layers`, `physics-joints`, `physics-time-scale` beside it.
- `ludion docs hierarchy` — parent/child transforms; `prefabs`, `prefab-instances` beside it.
- `ludion docs visuals` — how a scene looks: eight topics, from materials to the imaging chain.
- `ludion docs screen-ui` — `UiRect`/`UiText`: the reference canvas, anchors, and draw order.
- `ludion docs ui-list` — `UiList`: the selectable menu, cursor, windowing.
- `ludion docs fonts` — baked TrueType faces and the `font` field.
- `ludion docs audio` — sound as frame-stamped events, verified in text.
- `ludion docs data-docs` — a game's declarative data as validated `kind:`-tagged files.
- `ludion docs map-authoring` — districts as text maps: `kind: map`/`map-kit`, generate and check.
- `ludion docs map-check` — reading the report: occupancy, `--traverse`, lints.
- `ludion docs player-profiles` — persistent save state, its migration and its replay.
- `ludion docs scene-transitions` — moving the player between districts inside one run.
- `ludion docs scene-residue` — what a district keeps between visits, and reading it back.
- `ludion docs verify-loop` — `cargo test`: sim assertions, `WorldHash`, and golden images.
- `ludion docs running` — hosting a scene, attaching peers, `render.bounds`, recording.
- `ludion docs export` — `ludion export`: a game as an Xcode project for iOS.
- `ludion docs quality` — per-machine render quality: `--quality`, the presets, the override file.
- `ludion docs dev-loop` — `ludion dev`, the live rebuild → snapshot → restore loop.
- `ludion docs recipes` — copyable patterns: spawning, collision, lights.
- `ludion docs wire-protocol` — the JSON-over-WebSocket protocol, from a bare socket.
- `ludion docs designer-flow` — the human-side walkthrough, for a designer briefing you.
- `ludion docs voxel-models` — voxel models and assemblies: eight topics, from the grid to a clip.
- `ludion docs sculpt-format` — implicit-surface models: primitives, folding, blend, bones.

## The thesis, in three lines

1. Every capability is a typed command; you apply a command and observe a **deterministic** response.
2. You verify that response by **hash and image comparison**, not by looking at a screen.
3. The CPU sim is bit-exact (fixed timestep, seeded RNG); images gate on SSIM against a golden.

So the loop is: change code → `cargo test` → read a hash or an SSIM diff → correct. A human watches
the live viewer when they want to; you never need to.
