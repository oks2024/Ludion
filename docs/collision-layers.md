---
description: Collision layers — which rigid-body pairs never meet, the check-scene matrix, and query filtering
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
- `L1033` — the block is malformed: a duplicate, empty or `default` name, more than 31 layers, an
  `ignores` entry naming no declared layer (the message gives the nearest name) or repeated.
- `L1186` — a collider's `layer` is not declared. `check-scene` reports it; at runtime the collider
  gets no body until its `Collider` or the `layers` list changes.
- `L1187` — `layer:` on a 2D collider the physics tier does not claim. Reported by `check-scene` only.
- **Queries** take a `QueryFilter`: `QueryFilter::new().exclude(me).ignore_layer("dummy")` drops your
  own entity and every `dummy` hit. Result order is unchanged. An undeclared name is
  `PhysicsError::QueryInput`; `default` is always valid.

## Gotchas

- A filtered pair is never detected: no contact, no `ContactEvent`. A pair you forgot to ignore
  collides and a pair you ignored by mistake is silent. `ludion check-scene <scene> --bounds` prints
  the resolved matrix (`--json` adds it as the summary's `layers` field) — read which pairs never meet without launching.
- The layer filter wins over a joint's `collide: true`.
- A live edit of the `layers:` block or of a `layer:` takes effect on the next tick.
- Reordering `layers:` changes no result.

## Next

- `ludion docs physics` — `Collider`, the `physics:` block, contact events and queries.
- `ludion docs physics-joints` — the `Joint` component.
