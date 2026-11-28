---
description: Prefabs — kind: prefab entity groups spawned at runtime from a system or over the wire
---

# Prefabs

**Scope:** A prefab is an authored group of entities — a goblin's six jointed parts, a one-body dynamite stick — that a scene instances, or a sim system or a wire client spawns at a pose, with every reference inside the group remapped to the new copy's ids. Covers the file format, `spawn_prefab` and `prefab.spawn`, `PrefabInstance`, what a recording keeps, and the query declarations a spawning system needs. The entity and component syntax is `ludion docs scene-format` and `ludion docs components`; the wire framing is `ludion docs wire-protocol` §9.

## The loop

1. Author `prefabs/goblin.prefab.yaml` under the content root.
2. Check it: `ludion check-scene prefabs/goblin.prefab.yaml` — the scene's own codes plus `L1226`–`L1229`.
3. Spawn it from a sim system: `spawn_prefab(world, "prefabs/goblin.prefab.yaml", Some(placement), Some("goblin-1"), false)` (`use ludion::prelude::*`), or over the wire with `prefab.spawn`.
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
| `entities: []`: a prefab with no parts | `L1390` |
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
- **A fault on a named part reports once per `(source, part)`**, not once per instance: spawning the same faulty prefab twenty times logs one line, naming the first instance.
- **Read once.** The file is read and validated once per path per world, and every later spawn reuses it. Live instances never change when the file does.
- **`prepare_prefab(world, path)`** reads and validates a prefab ahead of its spawn from a sim system. It records the same way, spawns nothing, leaves `WorldHash` untouched, and a bad prefab returns the errors the spawn would. Call it a few ticks before the door, never on the spawn tick.
- **`presentation`** (the fifth argument; `presentation` over the wire) `true` spawns every part presentation, sticky; a sim-only component is `L1320`. A presentation system may spawn prefabs with `true` or an all-presentation prefab; a sim system asking for presentation, or a presentation system spawning sim parts, is `L0028` as a diagnostic (`ludion docs presentation`).

### Prewarming a room

`prewarm_prefab(world, path, placement)` makes the engine decode a prefab's assets and upload its meshes and material maps ahead of the spawn, so the later `spawn_prefab` uploads nothing. Call it from a sim system on the room-clear tick, never the spawn tick. The engine instantiates the prefab into a scratch world, so no entity is created and no id shifts. It holds one request: the same path and placement again is a no-op, a different one replaces it, and `PrefabPrewarm::clear()` (a resource in every world) drops it. Assets stay resident, never drawn, until then or until the world rebuilds. Nothing is hashed, recorded or put on the wire; a replay re-runs the system. A prefab cannot predict a reflection probe's store key, so pair it with `ProbeRefreeze` on the swap tick.

## `PrefabInstance`

Every part carries `PrefabInstance { source, part, root }`: the path as passed to `spawn_prefab`, the part's name without the instance prefix, and the instance's first part. It is registered, hashed and saved like any component, and `root` is remapped on load and restore like `Joint.a`.

Parts are not parented unless the prefab says so, so `entity.destroy` on the root removes only the root and its authored subtree. `despawn_instance(world, root)` removes the whole instance: every part, presentation parts included, in ascending id order, and returns those ids; `root` is `ids[0]` of `spawn_prefab`. A system calling it declares `.query::<(&PrefabInstance,)>()`.

## Rooms from maps

`ludion generate-map <map> --out rooms/x.prefab.yaml --prefab [--kit K] [--seed N] [--mirror x|z]` compiles a map into a `kind: prefab` room: one committed, drift-gated file per combination (`ludion map-drift` rechecks it with the flags recorded in `generated_from`: `ludion docs map-authoring`). A game that picks the kit, seed or mirror at run time stamps the map itself with `spawn_map`.

- **Mirror** reflects every transform; scale is never negative, so pieces keep their handedness and colliders stay exact. X negates yaw and Z turns it to 180−θ, so a piece faces its reflected direction. Built-in world-space vectors reflect too (`SpotLight`/`DirectionalLight` `direction`, `Velocity`, `NavSteering`, `NavAgent.target`). Game components are copied verbatim, so one holding a world direction, point or offset is not mirrored. A mirror on a map with fragments is `L1399`.
- **Paths and refusals.** `--prefab` rewrites asset paths relative to the prefab file, except inside game components, which are copied as written. The map's `physics:` is refused and its `seed:` is dropped.
- **The handle.** `ids[0]` of `spawn_prefab` is the room's handle: `despawn_instance(world, ids[0])` tears the whole room down.
- **`preload:`** the room prefabs a door may spawn, so the first spawn pays nothing. An entry whose `kind:` is `map` compiles the map with its own kit and seed (warming it, its kit and its fragments); a `kind: map-kit` entry reads the kit; the file name does not matter.
- **At run time.** `spawn_map(world, "rooms/crypt.map.yaml", MapSpawn { kit, seed, mirror, placement, name })` (`MapSpawn` is `Default`) from a sim system compiles the map as `generate-map … --prefab` would and spawns it like `spawn_prefab`: all-or-nothing, `ids[0]` the handle for `despawn_instance`, `PrefabInstance.source` the map's path; over the wire, `map.spawn` takes the same fields (`mirror` is `"x"`/`"z"`) and `prefab.despawn` is `despawn_instance`. `kit` is relative to the content root (not to the map file). Map, kit and fragments are read once per path per world and recorded inline, and the compiled room is cached per `(map, kit, seed, mirror)`: the first spawn of a combination compiles on its own tick, then a prefab spawn. `prepare_map(world, map, &spawn)` warms a runtime combination ahead from a sim system (same recording, nothing spawned, `WorldHash` untouched; `placement` and `name` ignored, a bad map returns the errors the spawn would), while `preload:` warms only the map's authored kit and seed. `asset.reload` of a held map, kit or fragment (or a prefab a fragment nests) drops the rooms that read it; live rooms never change. A map, kit or fragment that failed to read stays failed until `asset.reload` of that file, across a restart too, so a replay agrees. A map carrying `physics:`, a path that is absolute or leaves the content root, a file of the wrong `kind`, or a world with no library is `L1406`; a bad map or kit keeps `L1019`–`L1026`. A room root carrying `Animation` (without an assembly mesh) is `L1406`: a room has no clips. `scene.save` writes the room as plain entities, named or not, and `prefab.overrides`/`apply`/`revert` refuse it (`L1235`/`L1237`/`L1238`).
- **Nav and rooms.** A kit `box` collider is a 3D static, so physics blocks it and nav picks the room up the same tick when the spawning system runs before `nav_update`, else the next; `aabb`/`circle` stay 2D and invisible to both.

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

## Clips

A prefab may bind clips under an optional `clips:` map (short name to a `kind: animation` file, relative to the prefab); a glTF import writes it for rigid node animation. Playback, the `check-scene` clip checks and `L1314` are `ludion docs prefab-animation`.

## Next

- `ludion docs prefab-instances` — instance entries in scenes, save, apply/revert/unpack.
- `ludion docs prefab-animation` — `clips:` on a prefab, played on an instance's root part.
- `ludion docs hierarchy` — `Parent`, `reparent`, cascade despawn.
- `ludion docs physics-joints` — the `Joint` and `JointMotor` syntax a prefab's parts use.
- `ludion docs wire-protocol` — `prefab.spawn` §9.
- `ludion docs scene-format` — the entity list a prefab shares with scenes.
- `ludion docs components` — the `PrefabInstance` row.
