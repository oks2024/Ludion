---
description: Index of the eight voxel-model topics: the format, the loop, sheets, craft, ops, assemblies
---

# Voxel models

**Scope:** the index for the voxel family. Each entry below is its own topic; pull the one covering
what you are about to author. The engine-side machinery is `ludion docs engine-guide`'s business, not
this page's. Smooth organic or hard-surface forms a grid would take many cells to approximate are a
different `Mesh.shape` sibling — `ludion docs sculpt-format`.

A **voxel model** is a small 3D grid of colored cubes, authored as one strict-YAML file
(`voxels/*.voxel.yaml`) and greedily meshed into ordinary geometry at load. It is a `Mesh.shape`,
not a second render mode. A model too articulated for one grid becomes an **assembly** of parts with
declarative animation clips. Eight topics carry it:

- `ludion docs voxel-format` — the document: glyph slices, orientation, palette, origin and sizing,
  and how a scene references one.
- `ludion docs voxel-authoring` — the author → check → render loop and the `check-scene` text views.
- `ludion docs voxel-sheets` — the `render --model` contact sheet: views, game rigs, lineups,
  palette sweeps.
- `ludion docs voxel-craft` — what makes a model *read* at game scale once it is valid.
- `ludion docs voxel-ops` — `ludion voxel new/apply/upscale/diff`, for models past hand-editing
  size.
- `ludion docs voxel-generation` — building a model from a committed script, and the drift gate.
- `ludion docs voxel-assemblies` — multi-part models: `kind: assembly`/`kind: part`, pivots, joint
  plugs, the humanoid part convention.
- `ludion docs voxel-animation` — `kind: animation` clips, the `Animation` component, posed sheets.
