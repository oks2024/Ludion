---
description: Wire verbs for parent/child transforms and prefabs: entity.reparent, world poses, prefab.*
---

# Wire: hierarchy and prefab verbs

**Scope:** `entity.reparent`, world poses in `query.entities`, cascading `entity.destroy`, and the `prefab.*` methods over a raw socket. Handshake and envelope are `ludion docs wire-protocol`; the model behind them is `ludion docs hierarchy` and `ludion docs prefabs`.

## Parent/child transforms

A `Transform` under a `Parent` is local. `entity.reparent` re-points the parent, keeping the world pose
(`"keep": "world"`) or the local `Transform` bits (`"local"`); `"parent": null` makes a root:

```jsonc
// → move entity 4 under entity 2 without moving it in the world
{"v": 1, "id": 9, "method": "entity.reparent", "params": {"entity": {"index": 4, "generation": 0}, "parent": {"index": 2, "generation": 0}, "keep": "world"}}
// ← what it was before — the undo inverse is the reverse reparent with keep "local", then component.set Transform
{"id": 9, "ok": {"kind": "reparented", "previous_parent": null, "previous_transform": {"translation": [1.0, 1.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]}}, "frame": 12, "game_frame": 12}
```

A refusal (a loop `L1224`, a chain past depth 64 `L1260`, a sim entity under a presentation one
`L1225`, a rigid or static solver body under a parent `L1221`/`L1222`) changes nothing.
`query.entities` takes `"world": true` and then reports each match's composed pose beside its
components; `sheared` flags a pose a TRS cannot hold:

```jsonc
{"v": 1, "id": 10, "method": "query.entities", "params": {"filter": ["Transform"], "include": [], "world": true}}
{"id": 10, "ok": {"kind": "query_result", "entities": [
  {"entity": {"index": 4, "generation": 0}, "components": {}, "world": {"translation": [1.0, 2.0, 0.0], "rotation": [0.0, 30.0, 0.0], "scale": [1.0, 1.0, 1.0], "sheared": false}}
]}, "frame": 12, "game_frame": 12}
```

`entity.destroy` cascades: its reply carries the root's `components` and a `descendants` list, every
parent before its children, each with the `Parent` ids it had; respawn the root, then each descendant
in order, remapping the old ids to the fresh ones:

```jsonc
{"id": 11, "ok": {"kind": "entity_destroyed", "components": {"Transform": {"translation": [0.0, 1.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]}}, "descendants": [
  {"entity": {"index": 3, "generation": 0}, "components": {"Parent": {"index": 2, "generation": 0}, "Transform": {"translation": [0.0, 0.0, 2.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]}}}
]}, "frame": 12, "game_frame": 12}
```

## Prefabs: `prefab.spawn`, `map.spawn`, `prefab.despawn`, `prefab.create`, `prefab.apply`, `prefab.revert`, `prefab.unpack`, `prefab.overrides`

`prefab.spawn` instantiates a `kind: prefab` file and replies with the new ids in the prefab's part
order, the first being the instance's first part, which every part's `PrefabInstance.root` names:

```jsonc
// → spawn prefabs/crate.prefab.yaml as instance "a", shifted 5 units along x
{"v": 1, "id": 12, "method": "prefab.spawn", "params": {"path": "prefabs/crate.prefab.yaml", "name": "a", "transform": {"translation": [5.0, 0.0, 0.0]}}}
// ← one id per part; undo is prefab.despawn on entities[0]
{"id": 12, "ok": {"kind": "prefab_spawned", "entities": [{"index": 4, "generation": 0}, {"index": 5, "generation": 0}]}, "frame": 12, "game_frame": 12}
```

`name` makes the parts `a/<part>` (omitted, they keep their bare names) and `transform` is baked onto
every root part. `presentation: true` spawns every part presentation (`L1320` for a sim-only component). The spawn is all-or-nothing: a bad path (`L3040`), a missing file (`L1230`) or an
invalid prefab changes nothing. The prefab's text is recorded inline with the command, so a replay
survives later edits to the file.

`map.spawn` is the wire face of `spawn_map`: it compiles a `kind: map` file into a room and replies
with the same `prefab_spawned` body, `entities[0]` the instance root. `kit` (content-root-relative)
and `seed` override the map's own, `mirror` is `"x"` or `"z"` (omitted, none), and `placement` and
`name` act as `transform` and `name` do above. `seed` is a u64, imprecise in JS above 2^53.

```jsonc
// → a room from the map, seed 7, mirrored across x, named "r1"
{"v": 1, "id": 19, "method": "map.spawn", "params": {"map": "rooms/crypt.map.yaml", "seed": 7, "mirror": "x", "name": "r1"}}
{"id": 19, "ok": {"kind": "prefab_spawned", "entities": [{"index": 8, "generation": 0}, {"index": 9, "generation": 0}]}, "frame": 12, "game_frame": 12}
```

A path outside the content root (map or kit) is `L3040`; a missing or wrong-`kind` map is `L1406`; a
mirror on a map with `fragments:` is `L1399`; any refusal changes nothing. Map, kit and fragment texts
are recorded inline before the command, so a replay survives deleting the files.

`prefab.despawn` removes every part of an instance given its root (`entities[0]` of `prefab.spawn` or
`map.spawn`), unparented parts included, which `entity.destroy` on the root leaves alive:

```jsonc
// → remove the whole instance
{"v": 1, "id": 20, "method": "prefab.despawn", "params": {"root": {"index": 8, "generation": 0}}}
// ← every removed part, sorted by id
{"id": 20, "ok": {"kind": "prefab_despawned", "entities": [{"index": 8, "generation": 0}, {"index": 9, "generation": 0}]}, "frame": 12, "game_frame": 12}
```

A dead id is `L0002`; a part that is not the root, or an entity outside any instance, is `L1235`;
neither changes anything. Both commands are recorded and replay like `prefab.spawn`.

The next four edit prefabs and their instances; `instance` is any part of the instance:

```jsonc
// → write live entities out as a prefab (the first is its root); overwrites an existing prefab, refuses any other existing file (`L1236`)
{"v": 1, "id": 13, "method": "prefab.create", "params": {"entities": [{"index": 6, "generation": 0}, {"index": 7, "generation": 0}], "path": "prefabs/lamp.prefab.yaml"}}
// ← the normalized path
{"id": 13, "ok": {"kind": "prefab_created", "path": "prefabs/lamp.prefab.yaml"}, "frame": 12, "game_frame": 12}

// → write an instance's edited parts back into its prefab file; "parts" narrows it
{"v": 1, "id": 14, "method": "prefab.apply", "params": {"instance": {"index": 4, "generation": 0}}}
{"id": 14, "ok": {"kind": "prefab_applied", "path": "prefabs/crate.prefab.yaml", "parts": ["lid"], "removed": []}, "frame": 12, "game_frame": 12}

// → restore an instance (or one "/<part>/<Component>/<field>" pointer inside it) from its prefab
{"v": 1, "id": 15, "method": "prefab.revert", "params": {"instance": {"index": 4, "generation": 0}, "pointer": "/lid/Transform"}}
{"id": 15, "ok": {"kind": "prefab_reverted", "entities": [{"index": 5, "generation": 0}], "not_restored": []}, "frame": 12, "game_frame": 12}

// → drop the PrefabInstance links, leaving plain entities
{"v": 1, "id": 16, "method": "prefab.unpack", "params": {"instance": {"index": 4, "generation": 0}}}
{"id": 16, "ok": {"kind": "prefab_unpacked", "entities": [{"index": 4, "generation": 0}, {"index": 5, "generation": 0}]}, "frame": 12, "game_frame": 12}
```

An entity that is not part of a prefab instance is `L1235`. `prefab.revert` and `prefab.unpack` are
recorded commands; `prefab.create` is not, and `prefab.apply` is recorded as the prefab reload it
causes, so a replay reproduces both without touching the file.

`prefab.overrides` reads what `scene.save` would write for each instance (`instances` narrows it, any
part names one; omitted, every live instance). Unrecorded, a pure read:

```jsonc
{"v": 1, "id": 17, "method": "prefab.overrides", "params": {}}
// "/<part>" + a pointer is a prefab.revert pointer; an instance the save would refuse carries "diagnostic" instead
{"id": 17, "ok": {"kind": "prefab_overrides", "instances": [{"root": {"index": 4, "generation": 0}, "prefab": "prefabs/crate.prefab.yaml", "name": "a", "placement_differs": false, "overrides": {"lid": ["/Transform/translation"]}, "removed": [], "root_parts": ["base"]}]}, "frame": 12, "game_frame": 12}
```

Each override pointer, prefixed with `/<part path>`, is a `prefab.revert` pointer, except the
`Transform`/`Parent` of a part in `root_parts`: listed, but revert leaves them alone (the placement is
the instance's). A component the prefab part lacks is the one pointer `/<Component>`.

## Next

- `ludion docs wire-protocol` — handshake, envelope, events.
- `ludion docs hierarchy` — `Parent`-local transforms.
- `ludion docs prefabs` — the `kind: prefab` format.
