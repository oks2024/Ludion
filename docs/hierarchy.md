---
description: Parent/child transforms — local Transform under a Parent, world_transform, reparent, cascade despawn
---

# Transform hierarchy

**Scope:** An entity with a `Parent` has a `Transform` that is *local* to it; the composed world pose is derived on demand. Covers authoring a parent in a scene, reading and re-pointing it from code and over the wire, what despawn, physics and a scene switch do with a subtree, and the diagnostics for a malformed chain. The component table row is `ludion docs components`; the wire framing is `ludion docs wire-protocol`.

## The loop

1. Author the link: `Parent` names the parent by its **position in the scene's `entities:` list** (zero-based, `generation: 0`), so a parent may be written before or after its child.

```yaml
  - name: platform
    components:
      Transform: {}
  - name: turret
    components:
      Parent:
        index: 1          # the platform: the second entity above (the camera is index 0)
        generation: 0
      Transform:
        translation: [0.0, 1.0, 0.0]   # one metre above the platform, whatever the platform does
```

2. Move the parent; its children follow with no code of their own.
3. Read the composed pose: `world_transform(&world, id)` (code), `query.entities` with `"world": true` (wire).
4. Move an entity between parents with `reparent(world, child, parent, Keep::World | Keep::Local)` (code) or `entity.reparent` (wire). `parent: None` / `null` makes it a root.
5. Check the scene: `ludion check-scene` refuses a loop (`L1035`) and a body the solver cannot hold under a parent (below).

The runnable version — a turret yawing on a moving platform, a prop grabbed by a hand and thrown — is `crates/ludion/examples/hierarchy.rs` (`cargo run -p ludion --example hierarchy`).

## Format

- **`Transform` under a `Parent` is local** — translation, Euler-degrees rotation and scale relative to the parent's composed pose. A root's `Transform` is its world pose. Nothing else changes for an unparented entity.
- **The world pose is derived.** `world_transform` returns a `WorldPose` (`trs` plus a `sheared` flag) composed up the chain. It is never stored in a component, never part of `WorldHash`, never written by `scene.save` — the saved `Transform` is the local one.
- **Raw `Parent` writes keep the local `Transform`.** `component.set Parent` (or inserting the component) points the entity at a new parent and leaves its `Transform` bits alone, so its world pose jumps. `reparent` / `entity.reparent` says which to keep: `world` rewrites the local `Transform` so the world pose is unchanged, `local` leaves the bits alone.
- **Cascade despawn.** Destroying an entity destroys its whole subtree: the named entity first, then its descendants in ascending slot order. `entity.destroy` returns the destroyed entity's components and a `descendants` list (parents before children) a client can use to undo it.
- **Presentation entities** (`ludion docs presentation`) may sit under a sim parent; a sim entity may never sit under a presentation one (`L1225` from `reparent`, `L1219` from a raw write).
- **Scene switch.** A carried entity brings its whole subtree and the `Parent` links are remapped to the respawned ids (`ludion docs scene-transitions`). A carried child whose parent stays behind arrives un-parented, keeping its world pose, with warning `L3043`.
- **Debug draw.** The `hierarchy` category draws one line per parented entity, from the parent's origin to its own: `ludion render --scene S --out o.png --debug-draw hierarchy` (`ludion docs debug-draw`).

## Rules

| Situation | What happens |
|---|---|
| `kinematic` collider under a parent | Posed from its **world** pose every tick; the parent drives it |
| `rigid` collider under a parent | Refused — `L1221`; no body until the `Parent` is removed (use a `Joint`, `ludion docs physics-joints`) |
| `static` solver collider (`box`, `sphere`, …) under a parent | Refused — `L1222`; remove the `Parent` or make it `kinematic`. The 2D tier's in-plane static colliders are unaffected |
| kinematic body under a rotated, non-uniformly scaled chain | World pose carries a shear, dropped for the solver — `L1223`, once per entity |
| `Parent` loop at runtime (raw write) | The entity reads as a root — `L1218`, once per loop |
| `Parent` names a dead or presentation entity | The chain stops there, the entity reads as a root — `L1219` |
| a chain deeper than 64 | The entity reads as a root — `L1220` |
| `reparent` that closes a loop | Refused, nothing changes — `L1224` |
| `reparent` past depth 64 | Refused, nothing changes — `L1260` |
| `ParentJoint` under a skinned `Parent` | `Transform` is local to the joint: sim reads compose its rest pose, the draw its posed pose (`ludion docs character-animation`); a non-skin parent `L1318`, an unknown joint `L1319` |
| `Visibility` on an ancestor | Folds by AND down `Parent`/`ParentJoint`: a hidden ancestor always wins, a part's own `visible: true` does not show it; `cast_shadows` folds the same way (`ludion docs presentation`) |
| a scene whose links form a loop | `L1035` at load and `check-scene`; `scene.save` refuses to write one — `L1018` |

`L1221` and `L1222` are reported by `check-scene` as well as at runtime, so a bad scene fails before it runs.

## Gotchas

- **A held object goes kinematic first.** `reparent` refuses a `rigid` body under a parent (`L1221`), so a grab is `collider.body = Body::new_kinematic()` then `reparent(world, prop, Some(hand), Keep::World)`; a throw is `reparent(world, prop, None, Keep::World)`, the body back to `Body::Rigid(..)`, then a `Velocity` write. The example does exactly this.
- **`Keep::World` keeps the world position exactly; only a shear is lossy.** A rotated child under a non-uniform accumulated scale has a world transform no `Transform` can hold (`WorldPose::sheared`), so its rotation and scale come out as the closest rigid pose (`Keep::World` and the physics pose both use it). Author uniform scales on a parent that rotates its children.
- **Raw `Parent` writes teleport.** Only `reparent` keeps the world pose; setting `Parent` by `component.set` moves the entity to wherever its old local `Transform` lands under the new parent.
- **`move_and_slide` answers in world coordinates.** For a mover under a `Parent`, convert the returned delta into the parent's frame before adding it to the local `Transform`, or keep movers as roots (`ludion docs collision`).
- **Systems need no `Parent`, `ParentJoint` or `Mesh` declaration to call `world_transform`** — the walk is not recorded; the `Transform` reads along it are, as `&Transform`.
- **Never hashed means never a replay input.** Gameplay that decides from a world pose is deterministic because the local `Transform` and `Parent` links are; do not cache a world pose across ticks.

## Next

- `ludion docs components` — the `Transform` and `Parent` rows.
- `ludion docs physics` — kinematic and rigid bodies; `ludion docs physics-joints` for a rigid body that must follow another.
- `ludion docs scene-transitions` — carrying a subtree across a switch.
- `ludion docs debug-draw` — the `hierarchy` category.
- `ludion docs wire-protocol` — `entity.reparent`, `query.entities` `world`, `entity.destroy` `descendants`.
- `ludion docs character-animation` — sockets: `ParentJoint` on a skinned `Parent`.
