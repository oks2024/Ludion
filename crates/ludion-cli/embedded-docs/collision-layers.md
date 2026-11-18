---
description: Collision layers — which pairs never meet, query-only layers, include masks, the check-scene matrix
---

# Collision layers

**Scope:** which rigid-body pairs meet and what a query sees. `Collider` and the `physics:` block are
`ludion docs physics`; a jointed pair is `ludion docs physics-joints`.

## Format

Layers are declared once under `physics:`, each with the layers it ignores; a collider names one:

```yaml
physics:
  space: free
  layers:
    - name: dummy
      ignores:
        - dummy
        - sling
    - name: sling
entities:
  - name: dummy-upper-leg-l
    components:
      Collider:
        shape:
          capsule:
            half_height: 0.2
            radius: 0.08
        body:
          rigid:
            mass: 6.0
        layer: dummy
```

| Field | Meaning | Default |
| --- | --- | --- |
| `layers[].name` | the layer's name, unique, not `default` | required |
| `layers[].ignores` | layer names this one never meets (`default` allowed) | `[]` |
| `Collider.layer` | the layer this collider is on | `default` |

## Rules

- **A pair collides unless either side ignores the other.** One declaration covers both directions,
  so `dummy` ignoring `sling` also keeps `sling` out of `dummy`. Ignoring itself (`dummy: [dummy]`)
  makes a figure's own parts pass through each other.
- A collider with no `layer:` is on the implicit `default`; at most **31** layers can be declared.
- A sensor sits on one layer and follows the same table and pair rule (`ludion docs physics-sensors`).
- `L1033` — the block is malformed: a duplicate, empty or `default` name, more than 31 layers, an
  `ignores` entry naming no declared layer (the message gives the nearest name) or repeated.
- `L1186` — a collider's `layer`, or a `QueryLayers` entry, is not declared. `check-scene` reports it; at
  runtime a collider gets no body until its `Collider` or the `layers` list changes, while a bad
  `QueryLayers` entry keeps the body and gives the entity no query-only layers (one `L1186`, mask `0`)
  until the entry or the table changes.
- `L1187` — `layer:` on a 2D collider the physics tier does not claim. Reported by `check-scene` only.
- `L1500` — a `QueryLayers` entry is empty or repeated, or (`check-scene`) names the entity's own body
  layer. `L1501` — `QueryLayers` on an entity with no solver-claimed `Collider` and no `CharacterMotor`
  (`check-scene` only).
- **Queries** take a `QueryFilter`: `QueryFilter::new().exclude(me).ignore_layer("dummy")` drops your
  own entity and every `dummy` hit. Result order is unchanged. An undeclared name is
  `PhysicsError::QueryInput`; `default` is always valid.

## Query-only layers

A collider is on **one body layer**, the solver's. `QueryLayers` adds names that only queries see, so
a wall can stay `default` for the solver and still be a `light_occluder` to a sight check. Put it
beside a `Collider` or a `CharacterMotor`; every name is `default` or declared under `physics.layers`:

```yaml
entities:
  - name: container
    components:
      Collider:
        shape:
          box:
            half_extents: [1.0, 1.0, 1.0]
        body: static
        layer: cover_high
      QueryLayers:
        layers: [light_occluder]
```

- A query sees the body layer **plus** these names, for both `include_layer` and `ignore_layer`.
- The solver never sees them: no contact, trace, replay or `WorldHash` changes, and editing
  `QueryLayers` rebuilds nothing. A scene with none is byte-identical.
- **The character motor tests the body layer only**, because it emulates the solver's matrix: a wall
  tagged `QueryLayers: [actors]` still blocks a motor that ignores `actors`.

## Include masks

`include_layer(name)` (or `include_set(&LayerSet)`) keeps only hits whose layers intersect the included
set; no include means no clause. Every drop is an OR, so `ignore_layer` wins over an include:

```rust
// What blocks the light, but not the crouching cover the player hides behind.
let sight = QueryFilter::new().include_layer("light_occluder").ignore_layer("cover_low");
let hits = physics.ray_cast(eye, dir, 30.0, &sight)?;
```

Blasts take the same filter.

## Gotchas

- A filtered pair is never detected: no contact, no `ContactEvent`. A pair you forgot to ignore
  collides and a pair you ignored by mistake is silent. `ludion check-scene <scene> --bounds` prints
  the resolved matrix (`--json` adds it as the summary's `layers` field) — read which pairs never meet without launching.
  A layer that entities list in `QueryLayers` gains ` (N query-only)` in text; `--json` gives every layer a `query_only` count.
- The layer filter wins over a joint's `collide: true`.
- A live edit of the `layers:` block or of a `layer:` takes effect on the next tick.
- Reordering `layers:` changes no result.

## Next

- `ludion docs physics` — `Collider`, the `physics:` block, contact events and queries.
- `ludion docs physics-joints` — the `Joint` component.
- `ludion docs physics-sensors` — trigger volumes, filtered by the same table.
