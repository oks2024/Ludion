---
description: The shape:voxel/shape:mesh sculpt parts — reading another document's own shape as a field
---

# Sculpt source parts

**Scope:** the two sculpt part shapes that read another document's own shape as their field —
`voxel` and `mesh` — instead of describing a primitive with dimensions. The rest of the sculpt schema
(primitives, folding, deformers, bones, overrides) is `ludion docs sculpt-format`.

## Format

| shape | own keys |
| --- | --- |
| `voxel` | `path`, `size` |
| `mesh` | `path`, `size` (optional) |

A `voxel` part reads another `.voxel.yaml` document's occupancy as its field — a bevelled, smoothed
or carved read of existing content. `path` resolves relative to *this* document's own directory (the
`kind: assembly` `file:` rule; `..` is fine, an absolute, drive or device path is refused, `L1157`); `size` (world units per voxel, `> 0`) is addressed whole as
`/parts/<i>/size`, never by component. The part's frame keeps the voxel model's bottom-centre origin
scaled by `size`, so `at` places what a `Mesh.shape: voxel` entity's `Transform` would. Every common
part key applies. An empty `path` or non-positive `size` is `L1157`; a missing source is `L1101`, an
unparsable one `L1113` — both re-pointed at `/parts/<i>/path`.
`{ name: b, shape: voxel, path: "../voxels/b.voxel.yaml", size: 0.25, material: S }` is one.

A `mesh` part reads another `.mesh.yaml` document's own triangles as its field, even a non-watertight
one (`ludion devdocs sculpt-sources`) — an imported or generated mesh blends, bevels and carves like a
voxel part. `path` resolves the `voxel` rule exactly; `size` (world units per document unit, `> 0`)
is optional, default `1` — an import is already in world units, a normalized generated mesh is fitted
with this. The part's local frame is the mesh's own coordinates, so `at` places it as a
`Mesh.shape: asset` entity's `Transform` would. Every common part key applies, and `size` is an
override pointer exactly like a voxel part's: `/parts/<i>/size` on a part that omits it creates the
key first, seeded from its default `1` then set to the override's value — `AppliedOverride::from`
comes back unset, the `rotation` idiom, and the writer then emits the key. An empty `path` or
non-positive `size` is `L1157`; a missing source is `L1101`, an unparsable one the mesh document's own
`L1110`/`L1111` — all re-pointed at `/parts/<i>/path`.

## Gotchas

- **A mesh part contributes form, not surface.** Its field is sampled once onto a grid 64 cells along
  the mesh's longest axis, then re-sampled at the sculpt's `resolution`: detail finer than that, and
  the mesh's textures, do not survive — the part takes a palette `material` like any other.

- **A mesh part is never reported `L1158`-thin, only surfaceless** (`ludion docs sculpt-format`) —
  its thinness isn't a property the document carries, unlike a voxel part's own voxel `size`.

## Next

- `ludion docs sculpt-format` — the rest of the schema, and how a scene references a sculpt model.
- `ludion docs sculpt-craft` — what makes a valid model actually read.
- `ludion docs voxel-format` — the `.voxel.yaml` document a `shape: voxel` part reads.
- `ludion devdocs sculpt-sources` — the field these parts sample: BVH, winding number, EDT, cost.
