---
description: Prefab instances in scenes — `prefab:` entries, overrides, nesting, save, apply/revert/unpack
---

# Prefab instances in scenes

**Scope:** placing a `kind: prefab` file in a scene (or inside another prefab) as one entity entry, editing it with overrides, and how `scene.save` writes live instances back. Covers the entry format, nesting and variants, the `L1231`–`L1238` codes, and the four operations that edit prefabs from instances. The prefab file format and runtime spawning are `ludion docs prefabs`; the scene document is `ludion docs scene-format`; the wire framing is `ludion docs wire-protocol` §9.

## The loop

1. Author an entry in a scene's `entities:` (below), the prefab file beside it.
2. Check it: `ludion check-scene scenes/main.yaml` — expansion errors are `L1231`–`L1233`; the prefab file's own errors are `L1226`–`L1229`.
3. Load or render it: `ludion render --scene scenes/main.yaml --out shot.png`, or host it with `ludion serve`; the instance is ordinary entities in the world.
4. Edit live (a system, `component.set`, `entity.reparent`), then `scene.save` writes the instance back as its entry.

The runnable version — a lamp instanced twice, overridden, applied, reverted and unpacked, self-checking each save — is `crates/ludion/examples/prefab_scene.rs` (`cargo run -p ludion --example prefab_scene`).

## Instance entries

```yaml
entities:
  - name: lamp-a
    prefab: prefabs/lamp.prefab.yaml
    transform:
      translation:
        - -3.0
        - 0.0
        - 0.0
    overrides:
      bulb:
        PointLight:
          intensity: 9.0
    removed:
      - cap
```

Block style only — a non-empty `[1, 2]` or `{x: 1}` is `L1005`.

| Key | Meaning |
|---|---|
| `name` | required, unique in the document (`L1231`); the instance's identity for overrides and save |
| `prefab` | path; **every path resolves against the document it is written in** (a scene's directory is the content root), so a scene's `prefab:` entries and overrides are content-root-relative, while a prefab's asset fields, overrides and nested `prefab:` entries are relative to the prefab file. `./` and backslashes are normalized, `..` is allowed, absolute is refused |
| `transform` | `translation` / `rotation` (degrees) / `scale`, composed onto the prefab's root parts; omitted = as authored |
| `overrides` | part path → component → **partial** value: only the listed fields change, and a part may gain a component it lacked |
| `removed` | part paths dropped from the instance |

- **Expansion.** Parts become consecutive entities in document order, named `<instance>/<part>`, each carrying `PrefabInstance` (the outermost source, the composed part path, the first part as `root`).
- **References are document positions.** An entity reference inside `overrides`, or in a scene entity naming the instance, is a position in the containing document; a reference to the entry resolves to its **first surviving part** (after `removed`).
- **`L1233`** — `overrides`/`removed` name no part of the prefab, remove a part another part refers to, or remove every part.
- **`L1232`** — a prefab instances itself, directly or through nesting. **`L1231`** — a missing or duplicate `name`, an unknown key, a malformed `transform`.

## Nesting and variants

An entry may sit inside a prefab. Names and override paths compose with `/` (`turret/barrel/muzzle`); placements compose inner-then-outer; `PrefabInstance.source` is the outermost prefab. A **variant** is a prefab holding one instance entry plus overrides — there is no `base:` key.

A prefab edit reaches every instance **at the next load**. Live instances are not re-expanded; hot reload of instances is not built, so `asset.reload` of a prefab changes later spawns only.

## Saving

`scene.save` collapses each live group whose parts are all named `<name>/<part>` back into one entry: the placement is recovered from the first part's pose, component differences from a fresh expansion become partial `overrides`, vanished parts become `removed`, and nested instances collapse at the outermost level. Unnamed spawns (`spawn_prefab` with `None`) and mixed naming stay flat entities.

- Overrides are bit-exact — a tolerance would be a lossy writer. load → save → load → save is byte-identical. A moved instance keeps a `transform` when it reproduces the live poses bitwise; otherwise its roots' live poses are written as overrides. Only the root of a nested instance placed with a non-identity `transform` may settle after one save, because its pose is baked through Euler angles.
- **After an apply** the save diffs each instance against the text it was expanded from and writes overrides relative to the current file: an untouched instance saves clean and picks up the applied value on reload. An override on a part the file no longer has is refused; a stale `removed` is dropped.
- **`L1234`** refuses what an entry cannot express: an outside reference into a part other than the first (author it inside the prefab, or `unpack` the instance), a lost component, a part whose live components the entry cannot reproduce, an unknown or duplicate part path, a duplicate instance name, an instance that would not re-expand to the live world.

## Editing prefabs from instances

| Rust (`use ludion::prelude::*`) | Wire | Effect |
|---|---|---|
| `Engine::create_prefab(&ids, path)` | `prefab.create` | live entities → a new prefab file; the first is the root; names unique and slash-free; references must stay inside the set (`L1236`) |
| `Engine::apply_prefab(part, parts)` | `prefab.apply` | the instance's edits → the prefab file; `parts` narrows it; the anchor's `Transform` is never written (`L1237`) |
| `revert_prefab(world, part, pointer)` | `prefab.revert` | prefab values back, gained components removed, roots keep their placement, removed parts are not respawned; `pointer` is `/<part>[/<Component>[/<field>]]` (`L1238`) |
| `unpack_prefab(world, part)` | `prefab.unpack` | drops `PrefabInstance`; names stay, the entities become plain (`L1235` when not an instance) |

`apply` diffs the instance against the text it came from and merges into the current file, so another edit in the file survives. After `engine.restore` it recovers the placement from the first root, and a rotated multi-root instance may write low-digit noise. `create` refuses a path that names a generated prefab (`L1236`).

**Reading overrides.** `prefab.overrides` (wire only) reports what `scene.save` would write for each instance, through the same collapse: per instance root the `prefab`, `name` (`null` for an unnamed spawn), `placement_differs` (the saved `transform` differs bitwise from the placement it was built at), `overrides` (part path → leaf pointers `/<Component>[/<field>…]`), `removed`, and `root_parts` (the prefab's root part paths; empty with a `diagnostic`). `/<part path>` plus a pointer is a `prefab.revert` pointer, with one exception: a part in `root_parts` has its `Transform` and `Parent` listed (they are real overrides) but revert leaves them alone, since the placement is the instance's. A component the part holds and the prefab part lacks is the single pointer `/<Component>`, which revert removes. An instance the save would refuse (`L1234`, `L1230`) carries that `diagnostic` and nothing else; the call still succeeds. `instances` narrows it (any part names an instance, a non-instance is `L1235`). Unrecorded, never hashed.

`part` is any part of the instance. `revert` and `unpack` are recorded commands; `create` writes only a file and is not; `apply` is recorded as the prefab reload it causes, and other live instances are not re-expanded. Wire JSON is `ludion docs wire-protocol` §9.

## Imports

`ludion import` of a glTF writes `model.prefab.yaml` with `generated_from` set, not a scene: it instances from any scene, since the asset paths inside a prefab resolve against the prefab file's directory. A generated prefab is never edited by hand and `prefab.apply` on it is `L1237` — change the source and import again. A glTF camera is `L1202`.

## Next

- `ludion docs prefabs` — the file format and runtime `spawn_prefab`.
- `ludion docs scene-format` — the entity list instances live in.
- `ludion docs wire-protocol` — the edit methods and `prefab.overrides`, §9.
- `ludion docs hierarchy` — `Parent` and propagated transforms.
