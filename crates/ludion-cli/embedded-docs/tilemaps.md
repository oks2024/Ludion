---
description: The Tilemap grid component: legend and rows, placement convention, validation, reading it in code
---

# Tilemaps

**Scope:** Authoring a `Tilemap` and reading the same glyph grid back from game logic, so a level edit changes the picture and the rules at once. The component table it sits in is components.

`Tilemap` is the built-in content type for a grid of cells — the shape a tile game (Sokoban,
Breakout) is built from. One entity holds the whole grid; a level edit is a glyph-grid diff, which is
the minimal-diff pillar at its best. (A grid of 3D meshes, such as a cobble ground, is a `MeshGrid`:
`ludion docs mesh-grids`.) It has four fields:

- `cell_size` — the world-unit width and height of one square cell.
- `legend` — a map from a **single glyph** to a tile appearance (`texture` **or** `atlas` + `frame`,
  plus `color`, `lit` and `flip_x`/`flip_y` — the same field shapes as `Sprite`, so a whole stage can
  read one atlas and one mirrored corner glyph serves all four corners). A glyph is exactly one
  Unicode scalar value (one `char`).
- `rows` — a list of **equal-length** strings, one glyph per cell, top row first.
- `layer` — the `i16` sort key every cell shares (default `0`), the `Sprite.layer` meaning for the
  whole grid: a stage is one background plane, so it sorts as one.

```yaml
  - name: room
    components:
      Transform:
        translation:                      # the grid origin: the center of cell (0, 0)
          - -1.5
          - 1.0
          - 0.0
      Tilemap:
        cell_size: 1.0
        legend:
          "#":
            texture: assets/brick.png
            lit: true
          ".":
            color:
              - 0.2
              - 0.4
              - 0.8
              - 1.0
            lit: true
        rows:
          - "####"
          - "#..#"
          - "####"
```

A cell takes light only where its legend entry says `lit: true` (the default is unlit), so the scene
above still needs a `Camera` and a `PointLight` — `examples/gallery/tilemap.yaml` is the whole file.

At render time each cell expands into the existing sprite batcher — no new draw passes, and a cell is
a quad exactly like a `Sprite`. To leave a cell visually empty, give its glyph a background
color; there is no implicit blank glyph.

## Placement convention (game logic depends on this)

The grid is **axis-aligned in the world XY plane** — not the (x, z) plane colliders live on;
`ludion docs visuals --section "Which plane is 2D?"` states both once. Only the entity's
`Transform.translation` is used — as the **grid origin, the center of cell `(col 0, row 0)`** (the
first glyph of the first row).
`Transform.rotation` and `scale` are ignored: a tilemap is a grid, not a transformable sprite. From
the origin:

- **column** index increases along **+X** (`col * cell_size`);
- **row** index increases along **−Y** (`row * cell_size` downward) — first row at the top, matching
  reading order.

So cell `(col, row)`'s world center is
`[origin.x + col*cell_size, origin.y - row*cell_size, origin.z]`. For `render.bounds` the whole grid
is **one** renderable whose AABB unions every cell.

## Validation

`check-scene` (and a wire `component.set`) reject a malformed tilemap with a JSON Pointer to the
problem:

- **`L1019`** — ragged rows: some row's glyph count differs from row 0's (the grid is not
  rectangular). Pointer: `/…/rows/<n>`.
- **`L1020`** — invalid glyph: a row uses a glyph with no `legend` entry, or a `legend` key is not a
  single glyph. Pointer: `/…/rows/<n>` or `/…/legend/<key>`. Run `ludion explain L1020` for the full
  rule.

## Reading a tilemap from a system

Game logic reads the same component the renderer draws — that is how walls, floors, and targets are
one source of truth. `Tilemap` (and `TileAppearance`) are in `ludion::prelude`. Read cells with
`glyph_at(col, row)`, and map between cell coordinates and world space with `cell_to_world` /
`world_to_cell` (both take the grid origin — the tilemap entity's `Transform.translation`), so you
never re-derive the coordinate math:

```rust
use ludion::prelude::*;

/// Snap every `Mover` to the center of the tilemap cell it currently overlaps, but only if that cell
/// is walkable (glyph `.`) — the tilemap is the level's collision *and* its art, read the same way.
fn snap_movers_to_walkable_cells(world: &mut World, _ctx: &SystemCtx) {
    // The tilemap entity: the one that has a `Tilemap`. Read its grid and origin up front.
    let grid = world.single::<Tilemap>();
    let Some(grid) = grid else { return };
    let map = world.get::<Tilemap>(grid).unwrap().unwrap().clone();
    let origin = world.get::<Transform>(grid).unwrap().unwrap().translation;

    // Collect first (the query borrows the world), then apply.
    let movers: Vec<EntityId> = world.entities().filter(|&e| world.has::<Mover>(e)).collect();
    for mover in movers {
        let Some(Some(transform)) = world.get::<Transform>(mover).ok() else { continue };
        let (col, row) = map.world_to_cell(origin, transform.translation);
        // `world_to_cell` may return a negative or out-of-range cell — neither names a glyph.
        let glyph = usize::try_from(col)
            .ok()
            .zip(usize::try_from(row).ok())
            .and_then(|(c, r)| map.glyph_at(c, r));
        if glyph == Some('.') {
            let snapped = map.cell_to_world(origin, col, row);
            world.get_mut::<Transform>(mover).unwrap().unwrap().translation = snapped;
        }
    }
}

/// A game marker component riding on the same entities.
#[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct Mover;
```

`glyph_at` takes `(col, row)` and returns `Option<char>` (`None` outside the grid); `row_count()` and
`col_count()` give the grid dimensions. Because game logic and the renderer read the same glyph grid,
a level change is a one-line edit that both the picture and the rules pick up.

## Next

- `ludion docs sprites` — the sprite layer's hub; a tilemap spends the same committed atlas.
- `ludion docs components` — declaring components, and the built-in visual set.
- `ludion docs systems` — the system contract, input, and the determinism rules.
- `ludion docs collision` — colliders and spatial queries.
- `ludion docs scene-format` — the strict-YAML scene document.
