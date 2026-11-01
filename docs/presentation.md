---
description: Presentation entities and systems: drawn, never hashed, invisible to the sim; the L0028 rules
---

# Presentation state

**Scope:** juice that must not touch the simulation: score popups, a follow camera nothing
unprojects through, screen-shake bookkeeping. Defers to `ludion docs systems` (writing systems),
`ludion docs scene-format` (the file).

## The loop

1. Register the system after the sim ones: `Engine::builder().system("score", score).presentation_system("popups", popups)`.
   Presentation systems run last each tick, after physics.
2. In it, create entities with `world.spawn_presentation()`, or author them in a scene with `presentation: true`.
3. `ludion check-scene scene.yaml`, then step and compare `sim.hash`: it equals the same game without the presentation entities.

## Format

A presentation entity is an ordinary entity (any component, `Transform`, `Sprite`, `Name`) whose id
lies in a separate range (`ludion_ecs::PRESENTATION_BASE` and up; `is_presentation(id)`). It is drawn,
listed by `query.entities` and saved by `scene.save`, never hashed.

```yaml
entities:
  - name: ball                # sim entity: id 0
    components:
      Transform: {}
  - name: popup
    presentation: true        # after `name:`; omitted when false
    components:
      Transform: {}
```

`scene.save` writes sim entities first, then presentation ones. Entity references in a file name
document positions; `ludion_scene::instantiated_ids(&scene)` maps position to the spawned id, which
is not the position once the two kinds interleave. Sim ids never move: a scene with a popup spawns
its sim entities exactly as the scene without it.

## Rules

Crossing the boundary is `L0028`, in every direction:

| Who | Does | Result | Fix |
| --- | --- | --- | --- |
| sim system | `get`/`get_mut`/`insert`/`remove`/`despawn` on a presentation entity | `L0028` error | keep the data on a sim entity |
| sim system | `spawn_presentation` | panic `L0028` | spawn from a presentation system |
| presentation system | `get_mut`/`insert`/`remove`/`despawn` on a sim entity | `L0028` error | read it (`get`, read-only query) and write your own entity |
| presentation system | `spawn` | panic `L0028` | `spawn_presentation` |
| presentation system | `resource_mut`/`insert_resource` on a resource | panic `L0028` | only the outboxes `Diagnostics` and `AudioQueue` are writable; `DebugDraw` panics |

- Randomness: `DeterministicRng::derived(seed, frame, entity)`, never the master `DeterministicRng`.
- **Stays sim:** a camera the sim unprojects through (the external game's grab, a twin-stick game's aim; a `CameraRig` camera is sim, `ludion docs camera`) and anything with a `Collider`. Making it presentation turns every sim read of it into `L0028`.
- A sim entity's entity-reference field (`Parent`, `Joint` ends) may not name a presentation entity: `L1036` in a scene, `L1018` at `scene.save`. A presentation entity may name a sim one (a popup parented to the ball).
- Snapshot/replay: presentation state is snapshotted and restored, replayed, and never verified. Keyframe checks and `bisect` compare sim state only, so changing presentation code never invalidates a recording.

## Set dressing

Static visuals stay out of the hash: a prop's prefab marks its parts `presentation: true`; a prop that
needs a `Collider` keeps that part sim, **first**, and parents its visual parts to it.

```yaml
# lamp.prefab.yaml
entities:
  - name: post                  # sim root: Collider, ids and hash as without the bulb
    components:
      Transform: {}
      Mesh:
        shape: cube
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 0.5
              - 0.5
  - name: bulb
    presentation: true
    components:
      Transform: {}
      Mesh:
        shape: cube
      Parent:
        index: 0
        generation: 0
```

- **Sim part first** (`L1036`): every part's hashed `PrefabInstance.root` names the first part, so a presentation first part with a sim one after it would hash a presentation id. All-presentation prefabs are fine; an instance takes its parts' kind from the prefab (it has no `presentation` key).
- **Spawn from the scene or `prefab.spawn`.** A sim system's `spawn_prefab` refuses presentation parts (`L0028`).
- **Ground is one `MeshGrid` entity** with `presentation: true`, not a tile entity per cell (`ludion docs mesh-grids`).
- **`Fade` on the root fades the children**: a mesh with no `Fade` of its own takes its nearest ancestor's.

## Frame pacing

A window draws from a presenter thread at the display's refresh rate; the sim never waits on it. By default each tick's image is drawn once and repeated. `Engine::builder().present_interpolation(true)` instead draws every refresh blended between the last two ticks (the window lags up to one tick). Render-only: the sim, `WorldHash`, replays, screenshot, stream and goldens never blend.

- **Blends:** the pose of meshes, sprites, lights, text, ground markers, drop-shadow anchors, decals, and the camera's pose and field of view; particles (position, size, colour) and volley bullets (position), matched across ticks by a stable per-instance key. Births and deaths still appear at a tick.
- **Takes the newer tick:** fade, visibility, materials, UI, animation (clips and skinned poses stay stepped).
- **Snaps:** a spawn, `teleport_character`, `snap_rig`, a scene load, switch or restore, and a re-based or skipped tick.
- **A hand-written teleport** (writing `Transform` far from last tick) slides unless you `world.insert(id, NoInterpolate)` in the tick that writes it. Children follow; the marker lasts one tick and is never hashed.

## Gotchas

- An entity-count assertion over `world.entities()` excludes presentation entities; use `presentation_entities()` for those.
- `ludion explain L0028` / `L1036` carry the full text.

## Next

- `ludion docs systems` — writing systems and the determinism rules.
- `ludion docs scene-format` — the scene file the key lives in.
- `ludion docs components-fx` — particles, the other presentation-shaped effect.
