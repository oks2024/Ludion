---
description: Prefabs — kind: prefab entity groups spawned at runtime from a system or over the wire
---

# Prefabs

**Scope:** A prefab is an authored group of entities — a goblin's six jointed parts, a one-body dynamite stick — that a scene instances, or a sim system or a wire client spawns at a pose, with every reference inside the group remapped to the new copy's ids. Covers the file format, `spawn_prefab` and `prefab.spawn`, `PrefabInstance`, what a recording keeps, and the query declarations a spawning system needs. The entity and component syntax is `ludion docs scene-format` and `ludion docs components`; the wire framing is `ludion docs wire-protocol` §9.

## The loop

1. Author `prefabs/goblin.prefab.yaml` under the content root.
2. Check it: `ludion check-scene prefabs/goblin.prefab.yaml` — the scene's own codes plus `L1226`–`L1229`.
3. Spawn it from a sim system: `spawn_prefab(world, "prefabs/goblin.prefab.yaml", Some(placement), Some("goblin-1"))` (`use ludion::prelude::*`), or over the wire with `prefab.spawn`.
4. Observe: the returned ids, `query.entities`, the `WorldHash` — a spawn is ordinary hashed sim state.

The runnable version — a goblin launched twice and a dynamite stick, self-checking the remap — is `crates/ludion/examples/prefab_spawn.rs` (`cargo run -p ludion --example prefab_spawn`).

## Format

An envelope, then a scene's `entities:` list. `Joint.a`/`b`, `Parent` and any entity-reference field name a part by its **position in this list** (zero-based, `generation: 0`), exactly as in a scene:

```yaml
format: 1
kind: prefab
name: crate
entities:
  - name: base
    components:
      Transform: {}
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 0.25
              - 0.5
        body:
          rigid:
            mass: 4.0
  - name: lid
    components:
      Transform:
        translation:
          - 0.0
          - 0.5
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 0.05
              - 0.5
        body:
          rigid:
            mass: 0.5
      Joint:
        a:
          index: 0          # base
          generation: 0
        b:
          index: 1          # this part
          generation: 0
        kind:
          hinge:
            axis:
              - 1.0
              - 0.0
              - 0.0
            limits:
              - 0.0
              - 110.0
        break_impulse: 30.0
```

Block style only, as in a scene: a non-empty `[1, 2]` or `{x: 1}` is `L1005`; only `[]` and `{}` stay in flow style.

| Rule | Code |
|---|---|
| empty file, bad `format`/`kind`, unknown key, or a scene-only key (`seed`, `physics`) | `L1226` |
| a part with no `name`, an empty one, or one containing `/` | `L1227` |
| two parts with the same `name` | `L1228` |
| a part carrying a scene singleton: `Camera`, `DirectionalLight`, `Environment`, `PostProcess`, `GiVolume` | `L1229` |
| the file cannot be read (or the world has no prefab library) | `L1230` |

Everything else is the scene's own validation — `L1014` for a bad component value, `L1221`/`L1222` for a rigid or static collider under a `Parent`. A prefab has no `physics:` block, so the passes that need the running scene — collider layers against its `physics.layers` (`L1186`/`L1187`), atlas references, skin bones — run at the **first spawn**, not in `check-scene`. A scene's top-level `preload:` list runs them at load instead (path existence is checked by `check-scene`), so the first spawn pays nothing and a bad prefab fails the load.

## Spawning

- **`placement`** (the `Transform` argument; `transform` over the wire, translation / degrees / scale) is composed onto every **root** part — one with no in-prefab `Parent` — as `placement ∘ local`. There is no synthetic root: a prefab that wants one authors a part for it and parents the rest. `None` leaves the poses as authored.
- **`name`** makes the parts `<name>/<part>`; `None` keeps the bare part names.
- **The ids** come back in the prefab's part order, so `ids[i]` is part `i` and `ids[0]` is the instance's first part.
- **All-or-nothing.** A refused prefab spawns nothing; the `Err` carries every diagnostic.
- **Paths resolve against the document they are written in.** A prefab's asset fields, overrides and nested `prefab:` entries are relative to the prefab file, `..` allowed; an absolute, drive or scheme path is `L1311`. `prefab.create` and `prefab.apply` write prefab-relative paths (one that cannot be written relative is `L1236` / `L1237`). `spawn_prefab`'s path argument stays content-root-relative.
- **Read once.** The file is read and validated once per path per world, and every later spawn reuses it. Live instances never change when the file does.
- **Sim only.** A presentation part spawned from a sim system is `L0028`; the prefab library is a sim resource, so a presentation system cannot spawn at all.

## `PrefabInstance`

Every part carries `PrefabInstance { source, part, root }`: the path as passed to `spawn_prefab`, the part's name without the instance prefix, and the instance's first part. It is registered, hashed and saved like any component, and `root` is remapped on load and restore like `Joint.a`.

Parts are not parented unless the prefab says so, so despawning an instance is despawning every entity whose `PrefabInstance.root` is the root — `entity.destroy` on the root alone removes only the root and its authored subtree.

## Replays and reload

- **A recording carries the prefab text.** Each source read from disk is recorded inline — in-system spawns at the first load and at each reload, `prefab.spawn` with the command — and under replay the library never touches disk. A replay reproduces its hash trace after the prefab file changes or is deleted.
- **`asset.reload` re-validates a held prefab eagerly.** A bad edit is refused with the prefab's own diagnostic (the codes `spawn_prefab` lists) and the old text stays; a good one applies to later spawns only.

## Gotchas

- **A spawning system writes every component its prefab carries.** Query-drift (`ludion docs systems`) observes the spawn as writes, so the system declares each one or the gate reports `L1126`. For a prefab of `Transform`, `Collider`, `Velocity`, `Joint` and `JointMotor` parts, placed and named, the measured set is:

  ```rust
  Engine::builder()
      .system("launch", launch)
      .query::<(&mut Collider,)>()
      .query::<(&mut Joint,)>()
      .query::<(&mut JointMotor,)>()
      .query::<(&mut Name,)>()
      .query::<(&mut PrefabInstance,)>()
      .query::<(&Transform,)>()
      .query::<(&mut Transform,)>()
      .query::<(&mut Velocity,)>()
  ```

  One `&mut` line per component type any part of the prefab carries, plus `Name`, `PrefabInstance`, and both `Transform` shapes when a placement is passed. A `None` placement drops the `&Transform` read.
- **A reference to something outside the group cannot be authored.** A part's `Parent` or entity field can only name another part; bind a live outside entity (a target, a hand) in game code after the spawn, with `reparent` (`ludion docs hierarchy`) or a component write.
- **A rigid part cannot be parented.** Join rigid parts with a `Joint` (`ludion docs physics-joints`); a `Parent` on a rigid collider is `L1221`.
- **A prefab with a sim part starts with a sim part.** Presentation parts (`presentation: true`) follow the sim root; a presentation first part over a sim one is `L1036`, because `PrefabInstance.root` would name a presentation id (`ludion docs presentation`, "Set dressing").
- **Part names are the group's keys.** They must be unique (`L1228`) and slash-free (`L1227`) because instance paths compose with `/`.

## Prefabs in scenes

A scene (or another prefab) can place a prefab with one entity entry — `name`, `prefab:`, `transform`, `overrides`, `removed` — expanded at load, and `scene.save` writes live instances back as such entries. That format, nesting, `L1231`–`L1238` and `prefab.create`/`apply`/`revert`/`unpack` are `ludion docs prefab-instances`.

## Next

- `ludion docs prefab-instances` — instance entries in scenes, save, apply/revert/unpack.
- `ludion docs hierarchy` — `Parent`, `reparent`, cascade despawn.
- `ludion docs physics-joints` — the `Joint` and `JointMotor` syntax a prefab's parts use.
- `ludion docs wire-protocol` — `prefab.spawn` §9.
- `ludion docs scene-format` — the entity list a prefab shares with scenes.
- `ludion docs components` — the `PrefabInstance` row.
