---
description: The built-in components and builtin_registry() — the append-only order and what it pins
---

# The built-in component set

**Scope:** `components.rs` and `builtin_registry()` — what ships registered, in what order, and why
that order cannot move. Declaring a component in *game* code is `ludion docs components`; the
registry machinery and `register_validated` are `ludion devdocs ludion-ecs`; the schema hash a
registration moves is `ludion devdocs protocol-evolution`; the crate at large is
`ludion devdocs ludion-engine`.

## Layout

- `crates/ludion-engine/src/components.rs` — the built-in components: `Transform`, `Sprite`, `Mesh` (+`Primitive`), `PointLight`, `Camera` (+`Projection`), `Text`, plus screen-space `Anchor` (nine-point, every variant doc-commented for the inspector dropdown), `UiRect` and `UiText` (both `register_validated`, `L1118`: size must be positive and finite). `builtin_registry()` lives beside them.

## Rules

- **`builtin_registry()` registration order is load-bearing and append-only:** Transform, Sprite,
  Mesh, PointLight, Camera, Text, Parent (from `ludion-ecs`), Name (reserved — authored through the
  entity `name:` field, never a `components:` block), Tilemap, Collider (from `ludion-spatial`),
  UiRect, UiText, DirectionalLight, Environment, Fade, PostProcess. It fixes the canonical component
  key order in scene files *and* the `WorldHash` byte stream, so reordering or inserting mid-list
  invalidates every committed golden hash and canonical scene. Every addition since `Text`/`Parent`
  has landed strictly at the end for exactly that reason.
- **One registry for everything.** `Engine`, `check-scene`, `sim-hash`, `render` and `dump-graph` all
  call `builtin_registry()`; no client defines its own.
- **Components use plain serde/schemars fields**, never `glam` types (glam lacks `JsonSchema`);
  rotations are Euler XYZ **degrees**. `extract()` is where glam matrices are born.

## Gotchas

- `Parent` is registered in `builtin_registry()` but lives in `ludion-ecs`, not
  `components.rs` — don't look for it in this crate.
- The extract-side consequence of *not* registering a component — typed access hiding it, so it
  never renders — is in `ludion devdocs ludion-engine`'s `## Gotchas`, which owns the extract traps
  for the whole crate.

## Verify

```
cargo test -p ludion-engine
cargo run -p ludion-cli -- check-scene examples/gallery/meshes.yaml
cargo run -p ludion-cli -- sim-hash --scene examples/gallery/meshes.yaml --frames 60
```

A registration that moved the order shows up as a changed `sim-hash` final hash and as a re-ordered
`components:` block the moment any client saves a scene — both before any golden image does.
