---
description: The MeshGrid component: a glyph grid of oriented mesh cells for ground and floors, one entity
---

# Mesh grids

**Scope:** Authoring a `MeshGrid` — a `Tilemap` for 3D meshes: one entity draws hundreds of ground or floor cells, each with its own mesh and orientation. Flat sprite grids are tilemaps (`ludion docs tilemaps`); the component table is components-fx.

## The loop

1. Write the grid into the scene (below); `ludion check-scene <scene>` reports `L1019`/`L1020`/`L1243`/`L1244` with JSON pointers.
2. `ludion render --scene <scene> --out <png>` and look: every cell's orientation reads off the picture.
3. A ground edit is a glyph-grid diff; `render.bounds` reports the whole grid as one renderable.

## Format

```yaml
  - name: ground
    presentation: true            # recommended for colliderless ground
    components:
      Transform: {}               # the grid is local to its entity: the full pose applies
      MeshGrid:
        cell_size: 4.0            # default 1.0, must be > 0
        legend:
          a:                      # a legend entry IS a `Mesh` value
            shape:
              asset: meshes/street-ground/cobble-4x4-a.mesh.yaml
          b:
            shape:
              asset: meshes/street-ground/cobble-4x4-b.mesh.yaml
        rows:                     # first row is the far (-Z) row; ' ' is an empty cell
          - "abba "
          - "baab "
        orient:                   # optional, shaped like rows; omitted = every cell 0
          - "0123 "
          - "4567 "
```

`MeshGrid: {}` is valid and draws nothing. A legend value takes every `Mesh` field and static shape (asset, voxel, sculpt, assembly, primitives), so colour, material and emissive vary per glyph. A skinned assembly is `L1244`, raised by the file check (its document decides the body): cells share one entity, so they cannot pose or animate per cell; place it as its own entity. A legend `{ character: … }` is `L1246`.

**Placement.** Cell `(col, row)` sits at local `(col·cell_size, 0, row·cell_size)`: column → +X, row → +Z, cell `(0, 0)` at the entity origin. The entity's whole `Transform` (and `Parent`) applies, unlike a `Tilemap`.

**Orientation.** `0`-`3` is that many quarter-turns about local +Y (exactly `Transform.rotation: [0, 90k, 0]`); `4`-`7` is the turn `k-4` after a mirror of local X (exactly `Transform.scale: [-1, 1, 1]`). A space is `0`. Mirrored cells light correctly.

**Empty cell.** A space draws nothing and needs no legend entry; it can never be a legend key.

## Rules

| Code | Raised when |
|---|---|
| `L1019` | rows are ragged (`/rows/<n>`) |
| `L1020` | a row glyph has no legend entry, or a legend key is not one glyph or is a space |
| `L1243` | `orient` differs in row count or width from `rows`, or holds a glyph outside `0`-`7`/space (`/orient`, `/orient/<n>`) |
| `L1244` | a legend entry is a skinned assembly, found by the file check (`/legend/<glyph>`) |
| `L1101` | a legend asset fails to load; the pointer is `/entities/<n>/components/MeshGrid/legend/<glyph>/shape/...`, reported once per glyph; `check-scene` raises it for a glyph a row uses |

`component.set` runs the same checks and rejects a bad grid before any world change.

## Gotchas

- Pure render data: no sim system reads it, so mark colliderless ground `presentation: true` (`ludion docs presentation`); as a sim entity it is one hashed entity, still cheap.
- No colliders per cell; give the ground a collider entity of its own.
- `Fade` on the grid entity fades every cell.

## Next

`ludion docs components-fx`, `ludion docs presentation`, `ludion docs tilemaps`. A runnable example: `examples/gallery/mesh-grid.yaml`.
